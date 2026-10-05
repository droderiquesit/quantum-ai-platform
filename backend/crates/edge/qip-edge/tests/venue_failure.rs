//! What a cell does when one venue stops being usable and the others have
//! not: a feed that goes silent, a venue that rejects its orders, a sequence
//! gap, a book that crosses.
//!
//! Each of these already had a mechanism somewhere in the tree and none of
//! them reached an order. `CellConfig::max_staleness` was read by nothing;
//! `qip_routing::health` had no caller; the sequencer's deadline was passed
//! only when a message arrived, and the reset it emitted named an
//! identifier no book answers to, so a gap was journaled as having reset
//! the affected books and reset none. These tests drive the paths a
//! deployed cell takes — `on_bytes`, `work`, a `Placer` — and assert on the
//! orders that reach the gateway and on the chain.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{
    Cell, CellConfig, GATE_SILENT_FEED, GATE_VENUE_QUARANTINE, Placer, PricingPolicy, WorkReport,
};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use qip_protocols::FeedKey;
use qip_protocols::decoder::{Decoder, Diagnostics};
use qip_routing::health::HealthPolicy;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const XLON: &str = "XLON";
const XPAR: &str = "XPAR";
const FEED: &str = "feed-a";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-venue-failure-tests";

/// Whole seconds past the fixture's epoch.
fn t(secs: i64) -> Timestamp {
    at_ms(secs * 1_000)
}

/// Milliseconds past the fixture's epoch, for the steps that have to fall
/// inside or outside the sequencer's fifty-millisecond gap deadline.
fn at_ms(millis: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000).saturating_add(Duration::from_millis(millis))
}

fn venue(name: &str) -> VenueId {
    VenueId::new(name)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

/// One level per line, `sequence ⇥ object ⇥ B|A ⇥ price ⇥ size`, with the
/// sequence on the wire rather than counted by the decoder — so a test can
/// lose a message the way a network does, by not sending it.
#[derive(Debug)]
struct LineDecoder {
    venue: VenueId,
    consumed: usize,
    diagnostics: Diagnostics,
}

impl Decoder for LineDecoder {
    fn decode(&mut self, bytes: &[u8], captured_at: Timestamp) -> Result<Vec<MarketMessage>> {
        let text = std::str::from_utf8(bytes).map_err(|e| Error::invalid(e.to_string()))?;
        let mut messages = Vec::new();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            let [sequence, name, side, price, size] = fields[..] else {
                return Err(Error::invalid("a line has five fields"));
            };
            let side = match side {
                "B" => BookSide::Bid,
                "A" => BookSide::Ask,
                other => return Err(Error::invalid(format!("side {other} is neither B nor A"))),
            };
            let sequence: u64 = sequence
                .parse()
                .map_err(|_| Error::invalid("a sequence is a number"))?;
            messages.push(MarketMessage::new(
                object(name),
                Origin::new(self.venue.clone(), FEED, 0, sequence),
                MessageBody::LevelSet {
                    side,
                    price: d(price),
                    quantity: d(size),
                    order_count: None,
                },
                captured_at,
                captured_at,
            ));
        }
        self.consumed = bytes.len();
        Ok(messages)
    }

    fn protocol(&self) -> &str {
        "line"
    }

    fn consumed(&self) -> usize {
        self.consumed
    }

    fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }
}

fn firing_strategy(id: &str, instrument: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    // A constant rule on purpose: it reads no feature, so nothing in the
    // feature graph can be what stops it. Whatever stops its order is a
    // gate on the venue.
    let spec = StrategySpec::new(
        StrategyId::new(id),
        object(instrument),
        Duration::from_secs(30),
    )
    .with_rule(Rule::new(
        "always",
        SignalKind::Enter,
        Expr::Flag(true),
        Expr::Exact(d("10")),
        Expr::Statistic(0.5),
        10,
    ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn signed_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue(XLON), venue(XPAR)],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))
}

/// A gateway that remembers every order it was asked to place and refuses
/// the ones bound for a venue that is down.
#[derive(Debug, Default)]
struct Gateway {
    down: Vec<VenueId>,
    /// Every call to `place`, accepted or not.
    attempts: Vec<(VenueId, ObjectId)>,
}

impl Gateway {
    fn attempts_at(&self, name: &str) -> usize {
        self.attempts
            .iter()
            .filter(|(at, _)| at.as_str() == name)
            .count()
    }
}

impl Placer for Gateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.attempts.push((venue.clone(), object_id.clone()));
        if self.down.contains(venue) {
            return Err(Error::unavailable(format!(
                "{} is not accepting orders",
                venue.as_str()
            )));
        }
        Ok(())
    }
}

/// A cell on both venues with an empty book for each listing and one
/// always-firing strategy per instrument, fed by [`LineDecoder`].
fn fed_cell(listings: &[(&str, &str)]) -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION)
        .with_venue(venue(XLON))
        .with_venue(venue(XPAR));
    // The premise every staleness assertion below rests on.
    assert_eq!(config.max_staleness, Duration::from_secs(5));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    for name in [XLON, XPAR] {
        cell.protocols_mut().register(
            venue(name),
            FEED,
            Box::new(LineDecoder {
                venue: venue(name),
                consumed: 0,
                diagnostics: Diagnostics::default(),
            }),
        )?;
    }
    for (at, instrument) in listings {
        cell.track(VenueState::aggregated(
            object(instrument),
            venue(at),
            VenueStatus::Open,
        ));
        let strategy = format!("s-{instrument}");
        let (compiled, program) = firing_strategy(&strategy, instrument)?;
        cell.deploy_with_pricing(
            compiled,
            program,
            signed_envelope(&strategy)?,
            PricingPolicy::Marketable,
        )?;
    }
    Ok(cell)
}

fn feed(cell: &mut Cell, at: &str, lines: &str, now: Timestamp) -> Result<usize> {
    cell.on_bytes(&FeedKey::new(venue(at), FEED), lines.as_bytes(), now)
}

/// Where the pass sent an order and for what, sorted, so an assertion states
/// the set and does not also pin the order the netting keys happen to sort in.
fn sent_to(report: &WorkReport) -> Vec<(&str, &str)> {
    let mut sent: Vec<(&str, &str)> = report
        .orders
        .iter()
        .map(|order| (order.venue.as_str(), order.object_id.as_str()))
        .collect();
    sent.sort_unstable();
    sent
}

fn refused_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        // The whole token: `stale_book` contains `book`.
        .filter(|(recorded, _)| recorded == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

fn journaled<T>(cell: &Cell, pick: impl Fn(&Decision, Timestamp) -> Option<T>) -> Vec<T> {
    cell.journal()
        .entries()
        .iter()
        .filter_map(|entry| pick(&entry.decision, entry.at))
        .collect()
}

#[test]
fn a_venue_whose_feed_stalls_gets_no_new_order_once_the_staleness_limit_is_crossed_while_a_venue_still_heard_keeps_trading()
-> Result<()> {
    // The failure this prevents: a feed that stops is not a sequence gap.
    // Nothing resets the book, it goes on serving the mid it held when the
    // feed died, and a strategy that reads no feature of that instrument
    // keeps sending against it. `max_staleness` existed to stop that and
    // was read by nothing.
    let mut cell = fed_cell(&[(XLON, "obj-ACME"), (XPAR, "obj-BETA")])?;
    let mut gateway = Gateway::default();
    feed(
        &mut cell,
        XLON,
        "1\tobj-ACME\tB\t99\t500\n2\tobj-ACME\tA\t101\t400",
        t(10),
    )?;
    feed(
        &mut cell,
        XPAR,
        "1\tobj-BETA\tB\t49\t500\n2\tobj-BETA\tA\t51\t400",
        t(10),
    )?;

    // Premise: both strategies trade while both feeds are heard.
    let report = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        sent_to(&report),
        vec![(XLON, "obj-ACME"), (XPAR, "obj-BETA")],
        "the premise failed: {:?}",
        report.refusals
    );

    // XLON says nothing from here on; XPAR's handler keeps saying it is
    // alive. Four seconds in, the limit has not been crossed and nothing
    // has stopped — the gate is the threshold, not the first quiet second.
    cell.feed_heartbeat(&venue(XPAR), t(14));
    let report = cell.work(t(14), &mut gateway)?;
    assert_eq!(
        sent_to(&report),
        vec![(XLON, "obj-ACME"), (XPAR, "obj-BETA")],
        "a feed quiet for less than the limit stopped a strategy: {:?}",
        report.refusals
    );
    let before = gateway.attempts_at(XLON);

    // Six seconds: past the limit. The strategy on the silent venue is
    // refused under the gate that says why, nothing reaches that venue, and
    // the strategy on the venue still heard is untouched.
    cell.feed_heartbeat(&venue(XPAR), t(16));
    let report = cell.work(t(16), &mut gateway)?;
    assert_eq!(
        sent_to(&report),
        vec![(XPAR, "obj-BETA")],
        "the stalled venue still received an order, or the live one stopped"
    );
    let refusals = refused_under(&report, GATE_SILENT_FEED);
    assert_eq!(
        refusals.len(),
        1,
        "exactly the one strategy on the silent feed is refused: {:?}",
        report.refusals
    );
    assert!(
        refusals[0].contains("the feed for XLON has been silent for 6000 ms"),
        "{}",
        refusals[0]
    );
    assert_eq!(
        report.refusals.len(),
        1,
        "something else was refused too: {:?}",
        report.refusals
    );
    assert_eq!(
        gateway.attempts_at(XLON),
        before,
        "an order reached the gateway for a venue the cell cannot see"
    );

    // Heard again: the strategy resumes without being redeployed.
    cell.feed_heartbeat(&venue(XLON), t(17));
    cell.feed_heartbeat(&venue(XPAR), t(17));
    let report = cell.work(t(17), &mut gateway)?;
    assert_eq!(
        sent_to(&report),
        vec![(XLON, "obj-ACME"), (XPAR, "obj-BETA")],
        "the strategy did not resume once its feed was heard: {:?}",
        report.refusals
    );
    Ok(())
}

#[test]
fn a_stalled_feed_is_journaled_once_as_an_incident_and_its_return_as_a_reconciliation_of_what_rested_there()
-> Result<()> {
    // The failure this prevents: a cell that went quiet with nothing in the
    // chain saying why. A gap journals itself; a feed that simply stops
    // produces no message to hang a record on.
    let mut cell = fed_cell(&[(XLON, "obj-ACME"), (XPAR, "obj-BETA")])?;
    let mut gateway = Gateway::default();
    feed(
        &mut cell,
        XLON,
        "1\tobj-ACME\tB\t99\t500\n2\tobj-ACME\tA\t101\t400",
        t(10),
    )?;
    feed(
        &mut cell,
        XPAR,
        "1\tobj-BETA\tB\t49\t500\n2\tobj-BETA\tA\t51\t400",
        t(10),
    )?;
    let report = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        sent_to(&report),
        vec![(XLON, "obj-ACME"), (XPAR, "obj-BETA")],
        "the premise failed: {:?}",
        report.refusals
    );
    // Premise for the reconciliation below: the order sent to XLON is still
    // open, because this gateway reports no fill.
    let resting_at_xlon = gateway.attempts_at(XLON);
    assert_eq!(resting_at_xlon, 1);

    let silences = |cell: &Cell| {
        journaled(cell, |decision, at| match decision {
            Decision::FeedSilent {
                venue,
                last_heard,
                limit_ms,
            } => Some((venue.clone(), *last_heard, *limit_ms, at)),
            _ => None,
        })
    };
    let reconciliations = |cell: &Cell| {
        journaled(cell, |decision, at| match decision {
            Decision::FeedReconciled {
                venue,
                silent_from,
                resting_orders,
                stale_books,
            } => Some((
                venue.clone(),
                *silent_from,
                *resting_orders,
                *stale_books,
                at,
            )),
            _ => None,
        })
    };
    assert!(
        silences(&cell).is_empty(),
        "an incident was journaled for a feed that was being heard"
    );

    // Two passes inside the silence: the incident is recorded on the first
    // and not again on the second, and it names the silent venue only.
    for now in [16, 18] {
        cell.feed_heartbeat(&venue(XPAR), t(now));
        cell.work(t(now), &mut gateway)?;
    }
    assert_eq!(
        silences(&cell),
        vec![(XLON.to_string(), t(10), 5_000, t(16))],
        "the stall was not journaled exactly once, for the silent venue, at the pass that \
         found it"
    );
    assert!(
        reconciliations(&cell).is_empty(),
        "a reconciliation was journaled while the feed was still silent"
    );

    // The feed speaks again, by the path a packet takes.
    feed(&mut cell, XLON, "3\tobj-ACME\tB\t99\t450", t(20))?;
    assert_eq!(
        reconciliations(&cell),
        vec![(XLON.to_string(), t(10), resting_at_xlon, 0, t(20))],
        "the feed's return was not journaled as one reconciliation naming the interval and \
         the order left resting across it"
    );

    // And a venue heard again is a venue that can go silent again: the
    // bookkeeping that made the incident once-only was cleared with it.
    cell.feed_heartbeat(&venue(XPAR), t(26));
    cell.work(t(26), &mut gateway)?;
    assert_eq!(
        silences(&cell).len(),
        2,
        "a second stall after a recovery was not journaled"
    );
    Ok(())
}

/// A cell holding the instrument at both venues, XLON the tighter, with the
/// quarantine armed at a threshold three rejections reach.
fn quarantining_cell(venues: &[&str]) -> Result<Cell> {
    let mut config = CellConfig::new(CELL, REGION).with_venue_health(HealthPolicy {
        min_samples: 3,
        quarantine_reject_rate_f64: 0.5,
        quarantine_for: Duration::from_secs(60),
        ..HealthPolicy::default()
    });
    for name in venues {
        config = config.with_venue(venue(name));
    }
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    for (name, bid, ask) in [(XLON, "99.5", "100.5"), (XPAR, "99", "101")] {
        if !venues.contains(&name) {
            continue;
        }
        let id = venue(name);
        let mut state = VenueState::aggregated(object("obj-ACME"), id.clone(), VenueStatus::Open);
        for (sequence, side, price) in [(0, BookSide::Bid, bid), (1, BookSide::Ask, ask)] {
            state.apply(&MarketMessage::new(
                object("obj-ACME"),
                Origin::new(id.clone(), FEED, 0, sequence),
                MessageBody::LevelSet {
                    side,
                    price: d(price),
                    quantity: d("500"),
                    order_count: None,
                },
                t(1),
                t(1),
            ))?;
        }
        cell.track(state);
    }
    let (compiled, program) = firing_strategy("alpha", "obj-ACME")?;
    cell.deploy_with_pricing(
        compiled,
        program,
        signed_envelope("alpha")?,
        PricingPolicy::Marketable,
    )?;
    Ok(cell)
}

fn quarantine_entries(cell: &Cell) -> Vec<String> {
    journaled(cell, |decision, _| match decision {
        Decision::Refused { gate, reason } if gate == GATE_VENUE_QUARANTINE => Some(reason.clone()),
        _ => None,
    })
}

#[test]
fn a_venue_that_rejects_its_orders_is_quarantined_with_a_journaled_reason_and_gets_no_order_until_the_cooldown_lapses()
-> Result<()> {
    // The failure this prevents: a venue that is down fails the pass —
    // `gateway.place`'s error leaves `work` — and the next pass sends to it
    // again, so it takes every pass down with it for as long as it is down.
    let mut cell = quarantining_cell(&[XLON, XPAR])?;
    let mut gateway = Gateway {
        down: vec![venue(XLON)],
        ..Gateway::default()
    };

    // Premise: XLON is the venue the cell prefers, and it fails. Three
    // passes, three rejections — the evidence the policy asks for.
    for now in [10, 11, 12] {
        assert!(
            cell.work(t(now), &mut gateway).is_err(),
            "the premise failed: a pass sending to a venue that is down did not fail"
        );
    }
    assert_eq!(gateway.attempts_at(XLON), 3);
    assert_eq!(gateway.attempts_at(XPAR), 0);
    assert!(quarantine_entries(&cell).is_empty());

    // The fourth pass finds the venue quarantined: journaled once with the
    // evidence and the instant it ends, and the flow goes to the venue that
    // still takes orders instead of failing the pass.
    let report = cell.work(t(13), &mut gateway)?;
    assert_eq!(sent_to(&report), vec![(XPAR, "obj-ACME")]);
    let entries = quarantine_entries(&cell);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(
        entries[0].contains("XLON rejected 3 of 3 orders"),
        "{}",
        entries[0]
    );
    assert!(
        entries[0].ends_with(&format!("quarantined until {}", t(72))),
        "the entry does not say when the quarantine ends: {}",
        entries[0]
    );
    assert_eq!(refused_under(&report, GATE_VENUE_QUARANTINE).len(), 1);

    // It holds: no further order reaches XLON, and the chain does not
    // repeat the entry on every pass.
    let report = cell.work(t(14), &mut gateway)?;
    assert_eq!(sent_to(&report), vec![(XPAR, "obj-ACME")]);
    assert_eq!(
        gateway.attempts_at(XLON),
        3,
        "an order reached a quarantined venue"
    );
    assert_eq!(quarantine_entries(&cell).len(), 1);

    // The cooldown runs from the last rejection, t(12), for sixty seconds.
    // Past it the venue is tried again — still down, so the pass fails once
    // more and the next one journals a second quarantine.
    assert!(cell.work(t(73), &mut gateway).is_err());
    assert_eq!(
        gateway.attempts_at(XLON),
        4,
        "a venue whose quarantine lapsed was not tried again"
    );
    let report = cell.work(t(74), &mut gateway)?;
    assert_eq!(sent_to(&report), vec![(XPAR, "obj-ACME")]);
    assert_eq!(quarantine_entries(&cell).len(), 2);
    Ok(())
}

#[test]
fn with_nowhere_else_to_go_every_order_for_a_quarantined_venue_is_refused_under_its_own_gate()
-> Result<()> {
    // The other half of "no new order is routed to it": a cell with a
    // second venue routes around the first and refuses nothing, so this is
    // the case where the refusal itself has to hold the line.
    let mut cell = quarantining_cell(&[XLON])?;
    let mut gateway = Gateway {
        down: vec![venue(XLON)],
        ..Gateway::default()
    };
    for now in [10, 11, 12] {
        assert!(cell.work(t(now), &mut gateway).is_err());
    }
    assert_eq!(gateway.attempts_at(XLON), 3);

    for now in [13, 14, 15] {
        let report = cell.work(t(now), &mut gateway)?;
        assert!(report.orders.is_empty(), "{:?}", report.orders);
        assert!(
            !report.refusals.is_empty()
                && report
                    .refusals
                    .iter()
                    .all(|(gate, _)| gate == GATE_VENUE_QUARANTINE),
            "the signal was not refused under the quarantine gate alone: {:?}",
            report.refusals
        );
    }
    assert_eq!(
        gateway.attempts_at(XLON),
        3,
        "an order reached the gateway for a quarantined venue"
    );
    Ok(())
}

/// Three listings — two instruments on XLON's stream, one on XPAR's — fed
/// and traded once at `t(10)`, so every later assertion starts from a cell
/// that was pricing from all three books.
fn three_books() -> Result<(Cell, Gateway)> {
    let mut cell = fed_cell(&[(XLON, "obj-ACME"), (XLON, "obj-CARB"), (XPAR, "obj-BETA")])?;
    let mut gateway = Gateway::default();
    feed(
        &mut cell,
        XLON,
        "1\tobj-ACME\tB\t99\t500\n2\tobj-ACME\tA\t101\t400\n\
         3\tobj-CARB\tB\t19\t500\n4\tobj-CARB\tA\t21\t400",
        t(10),
    )?;
    feed(
        &mut cell,
        XPAR,
        "1\tobj-BETA\tB\t49\t500\n2\tobj-BETA\tA\t51\t400",
        t(10),
    )?;
    let report = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        sent_to(&report),
        vec![(XLON, "obj-ACME"), (XLON, "obj-CARB"), (XPAR, "obj-BETA"),],
        "the premise failed — the clean interval before the fault does not trade: {:?}",
        report.refusals
    );
    Ok((cell, gateway))
}

fn resets(cell: &Cell) -> Vec<(String, String, Timestamp)> {
    journaled(cell, |decision, at| match decision {
        Decision::BookReset { venue, object, .. } => Some((venue.clone(), object.clone(), at)),
        _ => None,
    })
}

fn whole_book(bid: &str, ask: &str) -> Vec<MessageBody> {
    [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .map(|(side, price)| MessageBody::LevelSet {
            side,
            price: d(price),
            quantity: d("400"),
            order_count: None,
        })
        .collect()
}

#[test]
fn a_sequence_gap_or_a_crossed_book_marks_exactly_the_affected_books_unreliable_until_they_are_rebuilt()
-> Result<()> {
    // The failure this prevents, and it had happened: the sequencer's reset
    // carries an identifier of its own, the cell applied it by instrument,
    // and it matched no book — so a gap was journaled as "the affected books
    // are reset" and the cell went on pricing from all of them. And the
    // deadline that produces the reset was only ever passed when a message
    // arrived, so a feed that went quiet behind a gap never reached it.
    let (mut cell, mut gateway) = three_books()?;

    // Sequences 5 and 6 never arrive on XLON's stream. Seven is held.
    feed(&mut cell, XLON, "7\tobj-ACME\tB\t99\t450", t(11))?;
    assert!(
        resets(&cell).is_empty(),
        "a gap that might still fill discarded a book"
    );

    // A pass past the deadline, with no further message to carry the
    // clock: the gap is abandoned and exactly the books on that venue are
    // discarded, at this instant. XPAR's book is not touched and trades.
    cell.feed_heartbeat(&venue(XPAR), t(12));
    let report = cell.work(t(12), &mut gateway)?;
    assert_eq!(
        resets(&cell),
        vec![
            (XLON.to_string(), "obj-ACME".to_string(), t(12)),
            (XLON.to_string(), "obj-CARB".to_string(), t(12)),
        ],
        "the abandoned gap did not discard exactly the books on its venue"
    );
    assert_eq!(
        sent_to(&report),
        vec![(XPAR, "obj-BETA")],
        "an order was priced from a discarded book, or the unaffected venue stopped"
    );
    let stale = refused_under(&report, "stale_book");
    assert_eq!(stale.len(), 2, "{:?}", report.refusals);
    assert!(
        stale
            .iter()
            .all(|reason| reason.contains("sequence gap 5..=6")),
        "{stale:?}"
    );

    // Rebuilt, each book closes its own interval in the chain, and the
    // clean interval after it trades as the one before it did.
    cell.apply_snapshot(
        &venue(XLON),
        &object("obj-ACME"),
        &whole_book("99", "101"),
        t(13),
    )?;
    cell.apply_snapshot(
        &venue(XLON),
        &object("obj-CARB"),
        &whole_book("19", "21"),
        t(13),
    )?;
    let closed = journaled(&cell, |decision, at| match decision {
        Decision::BookResynchronised {
            venue,
            object,
            unreliable_from,
            ..
        } => Some((venue.clone(), object.clone(), *unreliable_from, at)),
        _ => None,
    });
    assert_eq!(
        closed,
        vec![
            (XLON.to_string(), "obj-ACME".to_string(), Some(t(12)), t(13)),
            (XLON.to_string(), "obj-CARB".to_string(), Some(t(12)), t(13)),
        ],
        "the unreliable interval is not bounded in the chain, per book"
    );
    cell.feed_heartbeat(&venue(XLON), t(13));
    cell.feed_heartbeat(&venue(XPAR), t(13));
    let report = cell.work(t(13), &mut gateway)?;
    assert_eq!(
        sent_to(&report),
        vec![(XLON, "obj-ACME"), (XLON, "obj-CARB"), (XPAR, "obj-BETA"),],
        "the clean interval after the rebuild does not trade: {:?}",
        report.refusals
    );

    // A book that passes through a cross inside one frame and ends it
    // uncrossed is a venue moving up, not a corrupt book: the new bid is
    // published before the old ask is withdrawn.
    feed(
        &mut cell,
        XPAR,
        "3\tobj-BETA\tB\t51.5\t100\n4\tobj-BETA\tA\t53\t400\n5\tobj-BETA\tA\t51\t0",
        t(14),
    )?;
    assert_eq!(
        resets(&cell).len(),
        2,
        "a book crossed only between two lines of one frame was discarded"
    );

    // A book left crossed at the end of a frame is corrupt, and only that
    // one instrument is discarded for it.
    feed(&mut cell, XPAR, "6\tobj-BETA\tB\t54\t100", t(15))?;
    assert_eq!(
        resets(&cell)[2..],
        [(XPAR.to_string(), "obj-BETA".to_string(), t(15))],
        "the crossed book was not discarded, or another book was"
    );
    cell.feed_heartbeat(&venue(XLON), t(15));
    let report = cell.work(t(15), &mut gateway)?;
    assert_eq!(
        sent_to(&report),
        vec![(XLON, "obj-ACME"), (XLON, "obj-CARB")]
    );
    let stale = refused_under(&report, "stale_book");
    assert_eq!(stale.len(), 1, "{:?}", report.refusals);
    assert!(stale[0].contains("the book crossed by 1"), "{}", stale[0]);
    Ok(())
}

#[test]
fn a_gap_raises_a_snapshot_request_and_the_book_is_priced_from_again_only_once_the_gap_has_closed_and_the_snapshot_is_applied()
-> Result<()> {
    // The failure this prevents: a book discarded and never rebuilt — the
    // cell had no way to be handed a snapshot, so the first abandoned gap
    // would have ended that venue for the life of the process — and its
    // opposite, a book declared whole while the stream is still broken.
    let (mut cell, mut gateway) = three_books()?;
    assert!(
        cell.snapshot_requests().is_empty(),
        "the premise failed: a request stands before anything went wrong"
    );

    feed(&mut cell, XLON, "7\tobj-ACME\tB\t99\t450", t(11))?;
    cell.feed_heartbeat(&venue(XPAR), t(12));
    cell.work(t(12), &mut gateway)?;

    // The request: both books on the gapped venue, each with the reason.
    let requests = cell.snapshot_requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.venue.as_str(), request.object_id.as_str()))
            .collect::<Vec<_>>(),
        vec![(XLON, "obj-ACME"), (XLON, "obj-CARB")],
        "the abandoned gap did not raise a snapshot request for each book it took"
    );
    assert!(
        requests
            .iter()
            .all(|request| request.reason.contains("sequence gap 5..=6")),
        "{requests:?}"
    );

    // The stream breaks again before the snapshot arrives: ten is held
    // behind eight and nine. Continuity is not re-established, so the
    // snapshot is refused and the book stays discarded.
    let after = |millis: i64| at_ms(12_000 + millis);
    feed(&mut cell, XLON, "10\tobj-ACME\tA\t102\t400", after(10))?;
    let refusal = cell
        .apply_snapshot(
            &venue(XLON),
            &object("obj-ACME"),
            &whole_book("100", "102"),
            after(20),
        )
        .expect_err("a book was declared whole while its stream still had a gap open");
    assert!(
        refusal.message().contains("still has a sequence gap open"),
        "{}",
        refusal.message()
    );
    assert_eq!(
        cell.snapshot_requests().len(),
        2,
        "the refused snapshot cleared the request anyway"
    );

    // The gap fills. Now the same snapshot is accepted, the request for
    // that book is withdrawn, and the next order is priced from the
    // snapshot — a marketable buy at the snapshot's ask, which no message
    // on the stream ever carried at that size.
    feed(
        &mut cell,
        XLON,
        "8\tobj-ACME\tB\t99\t440\n9\tobj-ACME\tB\t99\t430",
        after(30),
    )?;
    cell.apply_snapshot(
        &venue(XLON),
        &object("obj-ACME"),
        &whole_book("100", "102"),
        after(40),
    )?;
    assert_eq!(
        cell.snapshot_requests()
            .iter()
            .map(|request| request.object_id.as_str())
            .collect::<Vec<_>>(),
        vec!["obj-CARB"],
        "the request for the rebuilt book still stands, or the other was withdrawn with it"
    );
    cell.feed_heartbeat(&venue(XPAR), t(13));
    let report = cell.work(t(13), &mut gateway)?;
    let rebuilt: Vec<_> = report
        .orders
        .iter()
        .filter(|order| order.object_id.as_str() == "obj-ACME")
        .collect();
    assert_eq!(rebuilt.len(), 1, "{:?}", report.refusals);
    assert_eq!(
        rebuilt[0].price,
        d("102"),
        "the order was not priced from the snapshot"
    );
    // And the book nobody rebuilt is still refused.
    assert_eq!(refused_under(&report, "stale_book").len(), 1);

    // A snapshot nobody asked for is refused rather than applied over a
    // book the cell trusts.
    let unasked = cell
        .apply_snapshot(
            &venue(XLON),
            &object("obj-ACME"),
            &whole_book("90", "110"),
            t(13),
        )
        .expect_err("a snapshot was applied over a trusted book");
    assert!(
        unasked.message().contains("is not awaiting a snapshot"),
        "{}",
        unasked.message()
    );
    Ok(())
}

/// A gateway with a cancel path, which remembers what rests where and every
/// cancel it was asked for — and can be told one venue refuses them.
#[derive(Debug, Default)]
struct CancellingGateway {
    resting: std::collections::BTreeMap<String, (VenueId, Decimal)>,
    /// Every cancel asked for, honoured or not.
    cancels: Vec<(String, VenueId)>,
    refuses_cancels: Vec<VenueId>,
}

impl CancellingGateway {
    fn resting_at(&self, name: &str) -> Vec<String> {
        self.resting
            .iter()
            .filter(|(_, (at, _))| at.as_str() == name)
            .map(|(order_id, _)| order_id.clone())
            .collect()
    }
}

impl Placer for CancellingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.resting
            .insert(order_id.to_string(), (venue.clone(), quantity));
        Ok(())
    }

    fn can_cancel(&self) -> bool {
        true
    }

    fn cancel(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        _at: Timestamp,
    ) -> Result<Decimal> {
        self.cancels.push((order_id.to_string(), venue.clone()));
        if self.refuses_cancels.contains(venue) {
            return Err(Error::unavailable(format!(
                "{} is not answering cancels",
                venue.as_str()
            )));
        }
        self.resting
            .remove(order_id)
            .map(|(_, quantity)| quantity)
            .ok_or_else(|| Error::not_found(format!("{order_id} does not rest here")))
    }
}

/// Two passes with both feeds heard, so each venue holds two orders, then
/// one pass six seconds into XLON's silence.
fn stall_xlon_with_orders_resting(gateway: &mut dyn Placer) -> Result<(Cell, WorkReport)> {
    let mut cell = fed_cell(&[(XLON, "obj-ACME"), (XPAR, "obj-BETA")])?;
    feed(
        &mut cell,
        XLON,
        "1\tobj-ACME\tB\t99\t500\n2\tobj-ACME\tA\t101\t400",
        t(10),
    )?;
    feed(
        &mut cell,
        XPAR,
        "1\tobj-BETA\tB\t49\t500\n2\tobj-BETA\tA\t51\t400",
        t(10),
    )?;
    cell.work(t(10), gateway)?;
    cell.feed_heartbeat(&venue(XPAR), t(14));
    cell.work(t(14), gateway)?;
    cell.feed_heartbeat(&venue(XPAR), t(16));
    let report = cell.work(t(16), gateway)?;
    Ok((cell, report))
}

fn venue_withdrawals(cell: &Cell) -> Vec<(String, String, String)> {
    journaled(cell, |decision, _| match decision {
        Decision::VenueWithdrawn {
            order_id,
            venue,
            withdrawn,
            reason,
        } => {
            assert!(
                reason.contains("the feed for XLON has been silent"),
                "the entry does not say what failed: {reason}"
            );
            Some((order_id.clone(), venue.clone(), withdrawn.clone()))
        }
        _ => None,
    })
}

#[test]
fn when_one_venue_fails_every_order_resting_there_is_withdrawn_and_the_other_venue_keeps_its_orders_and_keeps_trading()
-> Result<()> {
    // The failure this prevents: only a halt pulled an order early. One
    // venue going dark left its orders resting in a market the cell could no
    // longer see, to be filled at prices it was no longer reading.
    let mut gateway = CancellingGateway::default();
    let (cell, report) = stall_xlon_with_orders_resting(&mut gateway)?;

    // Premise, read off the venue's side: two orders went to each venue
    // before the stall, and XPAR was sent a third on the pass that found it.
    assert_eq!(sent_to(&report), vec![(XPAR, "obj-BETA")]);
    assert_eq!(gateway.resting_at(XPAR).len(), 3);

    // Every order that rested at the failed venue received a cancel, none
    // at the other venue did, and nothing is left resting there.
    let cancelled: Vec<&str> = gateway.cancels.iter().map(|(_, at)| at.as_str()).collect();
    assert_eq!(
        cancelled,
        vec![XLON, XLON],
        "the cancels sent are not exactly one per order resting at the failed venue"
    );
    assert!(
        gateway.resting_at(XLON).is_empty(),
        "an order is still resting at the failed venue: {:?}",
        gateway.resting_at(XLON)
    );

    // In the chain under its own kind, with the venue's answer and the
    // fault — not as a mass cancel, because nothing halted.
    let withdrawn = venue_withdrawals(&cell);
    assert_eq!(withdrawn.len(), 2, "{withdrawn:?}");
    let cancelled_ids: Vec<&String> = gateway.cancels.iter().map(|(id, _)| id).collect();
    for (order_id, at, quantity) in &withdrawn {
        assert!(cancelled_ids.contains(&order_id), "{order_id}");
        assert_eq!(at, XLON);
        assert!(
            Decimal::parse(quantity).is_some_and(|open| open.is_positive()),
            "the entry does not carry what the venue said was still open: {quantity}"
        );
    }
    assert!(
        journaled(&cell, |decision, _| matches!(
            decision,
            Decision::MassCancelled { .. } | Decision::OrderExpired { .. }
        )
        .then_some(()))
        .is_empty(),
        "a venue's failure was journaled as a halt's mass cancel or as an expiry"
    );
    assert!(!cell.is_halted());
    Ok(())
}

#[test]
fn a_failed_venue_that_refuses_the_cancel_is_journaled_as_a_break_and_the_order_is_not_recorded_as_withdrawn()
-> Result<()> {
    let mut gateway = CancellingGateway {
        refuses_cancels: vec![venue(XLON)],
        ..CancellingGateway::default()
    };
    let (cell, _) = stall_xlon_with_orders_resting(&mut gateway)?;

    // Premise: the cancels were attempted, one per order resting there.
    assert_eq!(gateway.cancels.len(), 2);
    assert_eq!(gateway.resting_at(XLON).len(), 2);

    // The attempt and its failure are in the chain; a withdrawal is not.
    let breaks = journaled(&cell, |decision, _| match decision {
        Decision::ReconciliationBreak { detail } => Some(detail.clone()),
        _ => None,
    });
    assert_eq!(breaks.len(), 2, "{breaks:?}");
    for (order_id, _) in &gateway.cancels {
        assert!(
            breaks
                .iter()
                .any(|detail| detail.contains(order_id.as_str())
                    && detail
                        .contains("on XLON was withdrawn and the venue refused to withdraw it")),
            "the refused cancel of {order_id} is not journaled: {breaks:?}"
        );
    }
    assert!(
        venue_withdrawals(&cell).is_empty(),
        "an order the venue refused to withdraw was recorded as withdrawn"
    );
    // An order whose state at a venue is unknown stops the cell, as a
    // refused cancel does on every other path.
    assert!(cell.is_halted());
    Ok(())
}

#[test]
fn a_cell_that_cannot_withdraw_from_a_failed_venue_says_so_in_the_chain_once_rather_than_withdrawing_nothing_quietly()
-> Result<()> {
    // `Gateway` has no cancel path. Calling `cancel` on it would turn every
    // pass of the outage into a reconciliation break; saying nothing would
    // leave the chain silent about exposure the cell knows it cannot reduce.
    let mut gateway = Gateway::default();
    let (mut cell, _) = stall_xlon_with_orders_resting(&mut gateway)?;
    cell.feed_heartbeat(&venue(XPAR), t(18));
    cell.work(t(18), &mut gateway)?;

    let said = journaled(&cell, |decision, _| match decision {
        Decision::Refused { gate, reason } if gate == qip_edge::cell::GATE_VENUE_WITHDRAWAL => {
            Some(reason.clone())
        }
        _ => None,
    });
    assert_eq!(
        said.len(),
        1,
        "two passes into the outage the cell said it {} times: {said:?}",
        said.len()
    );
    assert!(
        said[0].starts_with("2 order(s) rest at XLON and cannot be withdrawn"),
        "{}",
        said[0]
    );
    assert!(venue_withdrawals(&cell).is_empty());
    assert!(!cell.is_halted());
    Ok(())
}
