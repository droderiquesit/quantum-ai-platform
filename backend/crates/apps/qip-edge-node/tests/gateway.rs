//! The venue seam, exercised in the deployable that ships it.
//!
//! Every test here drives the same types `main.rs` assembles: a [`Cell`] built
//! the way the node builds one, placing through the [`SimulatedGateway`] the
//! node constructs, with fills coming back on the drop-copy channel the node
//! drains. The suite in `qip-brokers` proves the matching engine; this file
//! proves the *composition* — that an order leaving the cell reaches that
//! engine and that what the venue says happened is what reconciliation hears.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_edge::cell::{Cell, CellConfig, Placer};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge_node::gateway::SimulatedGateway;
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::features::BookPressure;
use qip_feature_dag::state::MarketState;
use qip_financial::quality::LicensingClass;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;

const CELL: &str = "london-1";
const ENVELOPE_KEY: &[u8] = b"gateway-test-envelope-key";
const STRATEGY: &str = "gateway-book-pressure";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn t(offset: i64) -> Timestamp {
    start().saturating_add(Duration::from_secs(offset))
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn venue(name: &str) -> VenueId {
    VenueId::new(name)
}

fn level(
    symbol: &str,
    at: VenueId,
    sequence: u64,
    side: BookSide,
    price: &str,
    size: &str,
    when: Timestamp,
) -> MarketMessage {
    MarketMessage::new(
        object(symbol),
        Origin::new(at, "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: Decimal::parse(price).expect("a decimal literal"),
            quantity: Decimal::parse(size).expect("a decimal literal"),
            order_count: None,
        },
        when,
        when,
    )
}

fn book_at(venue_name: &str, symbol: &str) -> qip_orderbook::venue::VenueState {
    let id = venue(venue_name);
    let mut state =
        qip_orderbook::venue::VenueState::aggregated(object(symbol), id.clone(), VenueStatus::Open);
    for (index, (side, price, size)) in
        [(BookSide::Bid, "99", "900"), (BookSide::Ask, "101", "300")]
            .iter()
            .enumerate()
    {
        state
            .apply(&level(
                symbol,
                id.clone(),
                index as u64,
                *side,
                price,
                size,
                t(index as i64),
            ))
            .expect("a well-formed level");
    }
    state
}

/// A feature engine fed the same one-sided book, so the strategy fires on a
/// number the market produced.
fn fed_features(symbol: &str) -> Result<FeatureEngine> {
    let subject = object(symbol);
    let mut features = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    features.register(Box::new(BookPressure::new(subject, 5)))?;
    for (index, (side, price, size)) in
        [(BookSide::Bid, "99", "900"), (BookSide::Ask, "101", "300")]
            .iter()
            .enumerate()
    {
        features.ingest(&level(
            symbol,
            venue("XLON"),
            index as u64,
            *side,
            price,
            size,
            t(20),
        ))?;
    }
    Ok(features)
}

fn compiled_strategy() -> Result<(CompiledStrategy, Program)> {
    let subject = object("ACME");
    let pressure =
        qip_contracts::FeatureKey::new("book_pressure", subject.clone()).with("levels", 5);
    let mut catalogue = FeatureCatalogue::new();
    catalogue.declare(pressure.clone(), Type::Statistic)?;
    let spec = StrategySpec::new(
        StrategyId::new(STRATEGY),
        subject,
        Duration::from_millis(250),
    )
    .with_rule(Rule::new(
        "enter",
        SignalKind::Enter,
        Expr::feature(pressure).greater_than(Expr::Statistic(0.4)),
        Expr::Exact(dec!("100")),
        Expr::Statistic(0.62),
        500,
    ));
    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn grant() -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        qip_contracts::capital::CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue("XLON")],
            start(),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signed = build(&sign_payload(ENVELOPE_KEY, &unsigned.signing_payload()))?;
    VerifiedEnvelope::verify(signed, ENVELOPE_KEY, CELL, t(10))
}

/// A cell assembled the way `main.rs` assembles one, with the strategy live.
fn armed_cell() -> Result<Cell> {
    let mut config = CellConfig::new(CELL, "europe-west2");
    config = config.with_venue(venue("XLON"));
    let mut cell = Cell::new(config, fed_features("ACME")?)?;
    cell.track(book_at("XLON", "ACME"));
    let (strategy, program) = compiled_strategy()?;
    cell.deploy(strategy, program, grant()?)?;
    Ok(cell)
}

// --- the seam, end to end ----------------------------------------------------

#[test]
fn a_cell_order_reaches_the_matching_engine_and_reconciles_through_drop_copy() -> Result<()> {
    let mut cell = armed_cell()?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;

    // Contra liquidity at the price the cell's book implies (mid = 100), so
    // the buy the strategy raises has something real to cross.
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("100"), t(15))?;

    let report = cell.work(t(20), &mut gateway)?;
    assert!(
        report.refusals.is_empty(),
        "a fully-equipped cell refused: {:?}",
        report.refusals
    );
    let order = report
        .orders
        .first()
        .expect("the strategy fired and an order was sent");
    assert!(
        order.simulated,
        "the gateway's own answer sets this, and it is a paper venue"
    );
    assert_eq!(
        gateway.submitted_count(),
        1,
        "the venue saw a different count"
    );

    // The venue's account of what happened, on the independent channel.
    let copies = gateway.drain_drop_copies();
    assert!(
        !copies.is_empty(),
        "the venue filled nothing against a crossing touch"
    );
    let venue_filled: Decimal = copies.iter().map(|fill| fill.quantity).sum();
    assert_eq!(
        venue_filled, order.quantity,
        "the venue filled a different size than it was asked"
    );
    for copy in copies {
        cell.observe_drop_copy(copy);
    }

    // Full fill, both channels agree: reconciliation is clean.
    let breaks = cell.reconcile(t(30));
    assert!(
        breaks.is_empty(),
        "a fully-filled order broke reconciliation: {breaks:?}"
    );
    Ok(())
}

#[test]
fn a_partial_fill_at_the_venue_is_a_break_rather_than_a_rounding_up() -> Result<()> {
    let mut cell = armed_cell()?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;

    // Less than the order's size rests on the contra side, so the venue can
    // honestly fill only part of it. The cell's belief and the venue's account
    // now differ, and that difference must surface as a break — a reconciler
    // that assumes the rest invents a position nobody holds.
    //
    // The contra is 20 against a desired 100 because a cell with no policy
    // payload sizes at its conservative floor (the degradation table's 0.375),
    // so the order that reaches the venue is 37.5 — and the fixture's premise
    // below asserts the fill really was partial, which is what caught this
    // number when the floor landed.
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("20"), t(15))?;

    let report = cell.work(t(20), &mut gateway)?;
    let order = report.orders.first().expect("an order was sent");
    let copies = gateway.drain_drop_copies();
    let venue_filled: Decimal = copies.iter().map(|fill| fill.quantity).sum();
    assert!(
        venue_filled < order.quantity,
        "the fixture must produce a partial fill"
    );
    for copy in copies {
        cell.observe_drop_copy(copy);
    }

    let breaks = cell.reconcile(t(30));
    assert_eq!(
        breaks.len(),
        1,
        "a half-filled order reconciled clean: {breaks:?}"
    );
    Ok(())
}

// --- the gateway's own honesty ------------------------------------------------

#[test]
fn the_gateway_is_simulated_because_the_venue_says_so() -> Result<()> {
    let gateway = SimulatedGateway::new(venue("XLON"), 1, start())?;
    assert!(gateway.is_simulated());
    assert_eq!(gateway.class(), "simulated");
    // Nothing this binary can construct answers otherwise: `AdapterClass` has
    // no live variant, which `qip-brokers` proves with compile_fail doctests.
    Ok(())
}

#[test]
fn an_unlisted_instrument_is_listed_synthetically_and_stamped_as_such() -> Result<()> {
    let mut gateway = SimulatedGateway::new(venue("XLON"), 1, start())?;
    let subject = object("NEWNAME");
    gateway.seed_touch(&subject, Side::Sell, dec!("42"), dec!("10"), start())?;

    let mut probe = gateway;
    // Place against it to prove the listing admits real orders...
    probe.place(
        "order-1",
        &subject,
        &venue("XLON"),
        BookSide::Ask,
        dec!("10"),
        dec!("42"),
        t(1),
    )?;
    let copies = probe.drain_drop_copies();
    assert_eq!(copies.len(), 1, "the synthetic listing did not trade");
    Ok(())
}

#[test]
fn the_same_seed_replays_the_same_session_exactly() -> Result<()> {
    // The gateway is deterministic from its seed, which is what makes a
    // session replayable from its journal. Proved on a venue that *has*
    // something to randomise: `orderly()` sets the rejection probability to
    // zero and zero jitter, so under it every seed behaves identically and
    // this test would pass while proving nothing about seeding at all.
    let run = |seed: u64, rejection: f64| -> Result<Vec<(String, String)>> {
        let mut gateway =
            SimulatedGateway::with_rejection_probability(venue("XLON"), seed, rejection, start())?;
        let subject = object("ACME");
        gateway.seed_touch(&subject, Side::Sell, dec!("100"), dec!("10000"), start())?;
        let mut fills = Vec::new();
        for index in 0..40 {
            // Some of these the venue refuses from its own draw; determinism
            // means the same ones, in the same order, every run.
            let _ = gateway.place(
                &format!("order-{index}"),
                &subject,
                &venue("XLON"),
                BookSide::Ask,
                dec!("25"),
                dec!("100"),
                t(index),
            );
            for copy in gateway.drain_drop_copies() {
                fills.push((copy.order_id, format!("{}@{}", copy.quantity, copy.price)));
            }
        }
        Ok(fills)
    };

    let first = run(11, 0.25)?;
    assert_eq!(
        first,
        run(11, 0.25)?,
        "the same seed produced a different session"
    );
    assert!(
        !first.is_empty(),
        "the fixture traded nothing, so this proves nothing"
    );
    assert!(
        first.len() < 40,
        "the venue refused nothing at a quarter rejection rate, so the draw is not \
         being exercised and determinism across it is untested"
    );

    // A different seed draws differently. This is the half that fails if the
    // seed is ignored — which is exactly what an orderly venue would hide.
    assert_ne!(
        first,
        run(12, 0.25)?,
        "two seeds produced identical sessions; the rejection draw is not seeded"
    );
    Ok(())
}

// --- the listing helper's own refusals ---------------------------------------

#[test]
fn a_synthetic_listing_refuses_nonsense_and_records_its_provenance() -> Result<()> {
    use qip_brokers::exchange::{ExchangeSettings, SimulatedExchange};
    let mut exchange =
        SimulatedExchange::new(venue("XLON"), ExchangeSettings::orderly(), 1, start());
    assert!(
        exchange
            .list_synthetic(object("X"), "X", dec!("0"), dec!("1"), dec!("1"), start())
            .is_err(),
        "a zero-price listing was accepted"
    );
    exchange.list_synthetic(
        object("X"),
        "X",
        dec!("10"),
        dec!("1"),
        dec!("0.01"),
        start(),
    )?;
    let listing = exchange.listing(&object("X")).expect("just listed");
    assert_eq!(
        listing.provenance.licensing,
        LicensingClass::Synthetic,
        "a listing the simulator invented is not licensed as synthetic, so it could \
         be mistaken for vendor market data"
    );
    assert!(
        listing.provenance.source.contains("simulated venue"),
        "the provenance does not name what invented it: {}",
        listing.provenance.source
    );
    Ok(())
}

// --- the twelve-item payload, consumed where it changes behaviour ------------

use qip_contracts::degradation::StrategyClass;
use qip_contracts::policy::{BeliefPriors, CausalDigest, EpisodicDigest, PolicyPayload, Slot};
use qip_edge::VerifiedPolicy;

/// A signed, verified payload for this cell. `fresh` fills the three
/// capability slots at `now`, so the cell narrows nothing; otherwise every
/// slot is unproduced and the cell sits at its conservative floor.
fn verified_policy(sequence: u64, halted: bool, fresh: bool, now: Timestamp) -> VerifiedPolicy {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, now);
    payload.halted = halted;
    if fresh {
        payload.belief_priors = Slot::produced(
            BeliefPriors {
                priors: std::collections::BTreeMap::from([("ACME".to_string(), 0.8)]),
            },
            now,
        );
        payload.causal_digest = Slot::produced(
            CausalDigest {
                active_edges: vec!["rates->ACME".to_string()],
            },
            now,
        );
        payload.episodic_digest = Slot::produced(
            EpisodicDigest {
                digest: "abc".to_string(),
                episodes: 3,
            },
            now,
        );
    }
    let signed = payload
        .signed(ENVELOPE_KEY)
        .expect("the test key is not empty");
    VerifiedPolicy::verify(signed, ENVELOPE_KEY, CELL, now).expect("signed for this cell")
}

#[test]
fn a_cell_with_no_policy_sizes_at_the_conservative_floor_and_a_fresh_payload_restores_it()
-> Result<()> {
    // §6.2 through its real consumer. A cell nobody ever shipped policy to has
    // no belief priors and no causal digest, so it must size as if both are
    // stale — 0.75 × 0.5 of the ask — because full-confidence sizing without a
    // belief state was an overclaim, not a default.
    let mut cell = armed_cell()?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("100"), t(15))?;

    let report = cell.work(t(20), &mut gateway)?;
    let order = report.orders.first().expect("an order was sent");
    // The strategy asks for 100 (the harness's compiled strategy); the floor
    // multiplier is 0.375.
    assert_eq!(
        order.quantity,
        dec!("37.5"),
        "a policy-less cell did not size at the conservative floor"
    );

    // A fresh payload restores full-confidence sizing.
    let mut fresh_cell = armed_cell()?;
    fresh_cell.apply_policy(verified_policy(1, false, true, t(18)), t(18))?;
    let mut fresh_gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;
    fresh_gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("100"), t(15))?;
    let fresh_report = fresh_cell.work(t(20), &mut fresh_gateway)?;
    let fresh_order = fresh_report.orders.first().expect("an order was sent");
    assert_eq!(
        fresh_order.quantity,
        dec!("100"),
        "a cell with fresh belief and causal policy still narrowed"
    );
    Ok(())
}

#[test]
fn a_central_policy_halt_stops_the_cell_and_only_a_newer_payload_releases_it() -> Result<()> {
    // The flow-6 gap, closed at the cell: the centre can now stop a region.
    // Release is deliberately harder than engage — a halted cell resumes only
    // from a *newer* signed payload that says so, never from a replay of the
    // one before the halt.
    let mut cell = armed_cell()?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("100"), t(15))?;

    // Premise: the cell trades before the halt.
    let before = cell.work(t(16), &mut gateway)?;
    assert!(
        !before.orders.is_empty(),
        "the cell did not trade before the halt, so the halt below stops nothing"
    );

    cell.apply_policy(verified_policy(2, true, true, t(17)), t(17))?;
    assert!(cell.is_halted(), "a policy halt did not halt the cell");
    let halted = cell.work(t(18), &mut gateway)?;
    assert!(halted.orders.is_empty(), "a halted cell placed an order");
    assert!(
        halted
            .refusals
            .iter()
            .any(|(gate, _)| gate == "policy_halt"),
        "the refusal does not name the policy halt, so an operator cannot \
         tell which release discipline applies: {:?}",
        halted.refusals
    );

    // A replay of the pre-halt payload must not release it.
    let replay = verified_policy(2, false, true, t(17));
    assert!(
        cell.apply_policy(replay, t(19)).is_err(),
        "a payload at the halted sequence was accepted, so a replay can \
         un-halt a cell the centre stopped"
    );
    assert!(cell.is_halted());

    // A genuinely newer payload releases it.
    cell.apply_policy(verified_policy(3, false, true, t(20)), t(20))?;
    assert!(
        !cell.is_halted(),
        "a newer releasing payload did not release"
    );
    let after = cell.work(t(21), &mut gateway)?;
    assert!(
        !after.orders.is_empty(),
        "the released cell did not resume trading"
    );
    Ok(())
}

#[test]
fn a_strategy_that_recognises_situations_pauses_when_episodic_memory_goes_stale() -> Result<()> {
    // §6.2 row 3 through the pause gate, with the premise asserted both ways:
    // the same cell, same market, trades when the strategy is price-only and
    // refuses when it is reclassified — so the pause is the classification's
    // doing, not the fixture's.
    let mut cell = armed_cell()?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("100"), t(15))?;
    // No policy has ever arrived, so episodic memory reads unavailable — and a
    // price-only strategy must trade through that regardless.
    let priced = cell.work(t(16), &mut gateway)?;
    assert!(
        !priced.orders.is_empty(),
        "a price-only strategy paused on an episodic loss it does not depend on"
    );

    cell.classify(STRATEGY, StrategyClass::SituationalRecognition)?;
    let paused = cell.work(t(17), &mut gateway)?;
    assert!(
        paused.orders.is_empty(),
        "a situational-recognition strategy traded without episodic memory"
    );
    assert!(
        paused
            .refusals
            .iter()
            .any(|(gate, _)| gate == "degradation_pause"),
        "the pause is not journaled under its own gate: {:?}",
        paused.refusals
    );
    Ok(())
}

use qip_contracts::policy::HaltCommand;
use qip_edge::VerifiedHalt;

fn verified_halt(issued_at: Timestamp, reason: &str) -> VerifiedHalt {
    let signed = HaltCommand::new(CELL, issued_at, reason)
        .signed(ENVELOPE_KEY)
        .expect("the test key is not empty");
    VerifiedHalt::verify(signed, ENVELOPE_KEY, CELL, issued_at).expect("signed for this cell")
}

#[test]
fn a_halt_command_stops_the_cell_and_a_payload_racing_it_cannot_release_it() -> Result<()> {
    // The in-flight race the release barrier exists for. A releasing payload
    // issued *before* the halt was decided is a decision made in ignorance of
    // it, and applying it would let ordinary delivery jitter un-halt a cell
    // the centre just stopped. Only a payload issued after the halt releases.
    let mut cell = armed_cell()?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("100"), t(15))?;

    // Premise: trading before the halt.
    assert!(!cell.work(t(16), &mut gateway)?.orders.is_empty());

    cell.apply_halt(
        verified_halt(t(17), "drop-copy disagreement at the centre"),
        t(17),
    );
    assert!(
        cell.is_halted(),
        "a verified halt command did not halt the cell"
    );
    // Idempotent: the same halt again is one halt, not an error.
    cell.apply_halt(
        verified_halt(t(17), "drop-copy disagreement at the centre"),
        t(17),
    );
    assert!(cell.is_halted());

    // A releasing payload issued at the barrier instant does not release; the
    // payload's other content still applies. The sequence is fresh, so only
    // the barrier can be what refuses the release.
    cell.apply_policy(verified_policy(10, false, true, t(17)), t(18))?;
    assert!(
        cell.is_halted(),
        "a payload issued at the halt instant released it, so delivery \
         jitter can un-halt a cell"
    );
    assert_eq!(
        cell.policy_sequence(),
        Some(10),
        "the racing payload's policy content was discarded along with its \
         release, which conflates the two"
    );

    // A payload issued after the halt releases it.
    cell.apply_policy(verified_policy(11, false, true, t(19)), t(19))?;
    assert!(
        !cell.is_halted(),
        "a post-halt releasing payload did not release"
    );
    Ok(())
}

#[test]
fn a_halt_command_verifies_only_with_the_right_key_and_cell() {
    // The forged-halt trade-off, tested from the refusing side: an
    // unauthenticated stop-lever on a polled inbox would let anyone who can
    // inject frames stop a region at will.
    let signed = HaltCommand::new(CELL, t(5), "reason")
        .signed(ENVELOPE_KEY)
        .expect("signable");
    assert!(VerifiedHalt::verify(signed.clone(), b"other-key", CELL, t(5)).is_err());
    assert!(VerifiedHalt::verify(signed.clone(), ENVELOPE_KEY, "other-cell", t(5)).is_err());

    // Re-dating a signed halt would move the release barrier; the signature
    // covers the instant, so the edit must refuse.
    let mut redated = signed;
    redated.issued_at = t(50);
    assert!(
        VerifiedHalt::verify(redated, ENVELOPE_KEY, CELL, t(50)).is_err(),
        "a halt re-dated after signing still verified, so the release \
         barrier can be moved by anyone on the path"
    );
}

#[test]
fn a_replayed_halt_cannot_re_halt_a_released_cell_and_a_fresh_one_still_can() -> Result<()> {
    // The bounded denial of service the review named: a captured signed halt,
    // re-delivered after a legitimate release, re-halted the cell in the gaps
    // between publishes. Both halves of the fix are asserted, because the
    // guard must refuse exactly the replay and nothing else — a guard that
    // also slowed a fresh halt would trade a nuisance for a safety property.
    let mut cell = armed_cell()?;

    // Halt, then legitimately release with a newer payload.
    let original = verified_halt(t(17), "drop-copy disagreement");
    cell.apply_halt(original.clone(), t(17));
    assert!(
        cell.is_halted(),
        "the premise failed: the halt did not engage"
    );
    cell.apply_policy(verified_policy(2, false, true, t(19)), t(19))?;
    assert!(
        !cell.is_halted(),
        "the premise failed: the release did not release"
    );

    // The captured frame comes back. It is genuinely signed and genuinely
    // verified — the transport bought it nothing, and neither does replay.
    cell.apply_halt(original, t(25));
    assert!(
        !cell.is_halted(),
        "a replayed halt at the resolved barrier re-halted a released cell"
    );

    // A fresh halt — issued after the barrier — engages unconditionally.
    cell.apply_halt(verified_halt(t(26), "a new decision"), t(26));
    assert!(
        cell.is_halted(),
        "the replay guard also refused a fresh halt, which trades a nuisance \
         for the safety property"
    );

    // And an engaged cell is never released by this path: an old halt
    // arriving while halted changes nothing.
    cell.apply_halt(verified_halt(t(18), "stale duplicate"), t(27));
    assert!(cell.is_halted());
    Ok(())
}

// --- intent netting: the self-trade the cell used to permit -------------------

/// A second strategy over the same instrument, entering on the same feature.
///
/// Deliberately identical in everything but its id: two strategies that agree
/// are the netting case, and two that disagree are built from this by giving
/// one an opposite rule.
fn second_strategy(id: &str, kind: SignalKind, size: &str) -> Result<(CompiledStrategy, Program)> {
    let subject = object("ACME");
    let pressure =
        qip_contracts::FeatureKey::new("book_pressure", subject.clone()).with("levels", 5);
    let mut catalogue = FeatureCatalogue::new();
    catalogue.declare(pressure.clone(), Type::Statistic)?;
    let spec = StrategySpec::new(StrategyId::new(id), subject, Duration::from_millis(250))
        .with_rule(Rule::new(
            "enter",
            kind,
            Expr::feature(pressure).greater_than(Expr::Statistic(0.4)),
            Expr::Exact(Decimal::parse(size).expect("a decimal literal")),
            Expr::Statistic(0.62),
            500,
        ));
    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn grant_for(strategy: &str) -> Result<VerifiedEnvelope> {
    grant_over(strategy, vec![venue("XLON")])
}

fn grant_over(strategy: &str, venues: Vec<VenueId>) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        qip_contracts::capital::CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            venues.clone(),
            start(),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signed = build(&sign_payload(ENVELOPE_KEY, &unsigned.signing_payload()))?;
    VerifiedEnvelope::verify(signed, ENVELOPE_KEY, CELL, t(10))
}

/// The armed cell, plus a second strategy trading the same instrument.
fn cell_with_two_strategies(kind: SignalKind, size: &str) -> Result<Cell> {
    let mut cell = armed_cell()?;
    let (compiled, program) = second_strategy("book-pressure-two", kind, size)?;
    cell.deploy(compiled, program, grant_for("book-pressure-two")?)?;
    Ok(cell)
}

/// The same pair, but the second strategy's envelope does not cover the venue
/// the cell trades — so its per-strategy gate refuses before netting sees it.
fn cell_with_a_barred_second_strategy(kind: SignalKind, size: &str) -> Result<Cell> {
    let mut cell = armed_cell()?;
    let (compiled, program) = second_strategy("book-pressure-two", kind, size)?;
    let elsewhere = grant_over("book-pressure-two", vec![venue("XPAR")])?;
    cell.deploy(compiled, program, elsewhere)?;
    Ok(cell)
}

#[test]
fn two_strategies_agreeing_send_one_order_carrying_both_contributors() -> Result<()> {
    // Blueprint §27's first row. Before netting, each deployed strategy placed
    // its own order: two agreeing strategies sent two orders, paid the spread
    // twice, and showed the venue two prints where the platform meant one.
    let mut cell = cell_with_two_strategies(SignalKind::Enter, "100")?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("500"), t(15))?;

    let report = cell.work(t(20), &mut gateway)?;
    // The premise: both strategies really did fire. A netting assertion over
    // one signal would pass while proving that the second never ran.
    assert_eq!(
        report.signals.len(),
        2,
        "the fixture needs two firing strategies, got {}",
        report.signals.len()
    );
    assert_eq!(
        report.orders.len(),
        1,
        "two agreeing strategies sent {} orders; netting did not collapse them",
        report.orders.len()
    );
    let order = &report.orders[0];
    assert_eq!(
        order.contributors.len(),
        2,
        "the order carries {} contributor(s), so a fill cannot be traced back \
         to both strategies that caused it",
        order.contributors.len()
    );
    // Contributor sizes sum to what was actually sent.
    let summed: Decimal = order
        .contributors
        .iter()
        .map(|contributor| contributor.signed_size)
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(summed.abs(), order.quantity);
    assert_eq!(
        gateway.submitted_count(),
        1,
        "the venue saw more than one order"
    );
    Ok(())
}

#[test]
fn two_strategies_disagreeing_cross_internally_and_the_venue_sees_nothing() -> Result<()> {
    // The self-trade. Before netting, one strategy's buy and another's sell
    // both reached the venue and could match each other — a regulatory
    // problem and a pure loss at once, and the defect this slice exists to
    // remove. Now they cancel internally and neither order is sent.
    let mut cell = cell_with_two_strategies(SignalKind::Exit, "100")?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("500"), t(15))?;
    gateway.seed_touch(&object("ACME"), Side::Buy, dec!("100"), dec!("500"), t(15))?;

    let report = cell.work(t(20), &mut gateway)?;
    // Premise first, and it is the whole test: both strategies fired, in
    // opposite directions. Without this the assertions below would pass on a
    // cell that simply did nothing.
    assert_eq!(
        report.signals.len(),
        2,
        "the fixture needs two firing strategies"
    );
    assert!(
        report.signals.iter().any(|s| s.kind == SignalKind::Enter)
            && report.signals.iter().any(|s| s.kind == SignalKind::Exit),
        "the two strategies did not disagree, so nothing could cross"
    );

    assert!(
        report.orders.is_empty(),
        "opposing intents reached the venue as {} order(s) — this is the \
         self-trade netting exists to prevent",
        report.orders.len()
    );
    assert_eq!(
        gateway.submitted_count(),
        0,
        "the venue saw an order from two intents that cancelled"
    );
    assert_eq!(
        report.cancelled.len(),
        1,
        "the cancellation was not recorded, so the cell cannot explain why it \
         was quiet"
    );
    let cancelled = &report.cancelled[0];
    assert_eq!(cancelled.contributors.len(), 2);
    assert!(cancelled.is_cancelled());
    // Gross survives: something was intended even though nothing was sent.
    assert!(cancelled.gross_size.is_positive());
    Ok(())
}

#[test]
fn the_netting_ratio_reports_what_the_strategy_set_actually_cost() -> Result<()> {
    // §27 calls the ratio the single best summary of strategy-set diversity.
    // The case is chosen so the number is not one: a 100 buy against a 40
    // sell partially offsets, so gross and net genuinely differ and a ratio
    // hard-coded to 1.0 — or computed from the net alone — cannot pass. Two
    // agreeing strategies would give exactly one and prove nothing.
    let mut cell = cell_with_two_strategies(SignalKind::Exit, "40")?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("500"), t(15))?;
    gateway.seed_touch(&object("ACME"), Side::Buy, dec!("100"), dec!("500"), t(15))?;

    let report = cell.work(t(20), &mut gateway)?;
    // Premise: both fired, in opposite directions, and something survived the
    // offset. Without all three the ratio below would describe a different
    // situation than the one the test claims to cover.
    assert_eq!(
        report.signals.len(),
        2,
        "the fixture needs two firing strategies"
    );
    assert_eq!(
        report.orders.len(),
        1,
        "a partial offset must still send the surviving remainder"
    );
    let order = &report.orders[0];
    assert_eq!(order.contributors.len(), 2);

    // Gross and net are read off the order the cell actually sent, so the
    // expectation is not a constant copied from the implementation.
    let gross = order
        .contributors
        .iter()
        .map(|contributor| contributor.signed_size.abs())
        .fold(Decimal::ZERO, |a, b| a + b);
    assert!(
        gross > order.quantity,
        "the premise failed: nothing offset, so gross {gross} equals net {}",
        order.quantity
    );
    let expected = gross.to_f64() / order.quantity.to_f64();
    assert!(
        expected > 1.1,
        "the offset was too small to distinguish a real ratio from one"
    );

    let ratio = report
        .netting_ratio
        .expect("an order was sent, so the ratio is defined");
    assert!(
        (ratio - expected).abs() < 1e-9,
        "the cell reported a netting ratio of {ratio}, but the order it sent \
         implies {expected}"
    );
    Ok(())
}

#[test]
fn a_strategy_its_own_envelope_refuses_never_reaches_the_netting_set() -> Result<()> {
    // §28's ordering, and the reason it is an ordering rather than a
    // preference: the per-strategy gates run *before* netting, "because a
    // strategy that has exhausted its budget must not contribute to a net
    // intent at all". Run the other way round, a refused strategy's phantom
    // sell would still cancel a permitted strategy's buy, and the cell would
    // sit out a trade it was entitled to make on the strength of an order
    // nobody was allowed to send.
    let mut cell = cell_with_a_barred_second_strategy(SignalKind::Exit, "100")?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("500"), t(15))?;
    gateway.seed_touch(&object("ACME"), Side::Buy, dec!("100"), dec!("500"), t(15))?;

    let report = cell.work(t(20), &mut gateway)?;
    // Premise, in three parts: both strategies fired, they disagreed, and the
    // second was genuinely refused by its own envelope. Drop any one and the
    // assertion below describes a different situation.
    assert_eq!(
        report.signals.len(),
        2,
        "the fixture needs two firing strategies"
    );
    assert!(
        report.signals.iter().any(|s| s.kind == SignalKind::Enter)
            && report.signals.iter().any(|s| s.kind == SignalKind::Exit),
        "the two strategies did not disagree, so the ordering would not matter"
    );
    assert!(
        report.refusals.iter().any(|(gate, _)| gate == "capital"),
        "the second strategy was not refused by its envelope: {:?}",
        report.refusals
    );

    assert_eq!(
        report.orders.len(),
        1,
        "the permitted strategy's order was cancelled by a strategy that was \
         never allowed to trade"
    );
    let order = &report.orders[0];
    assert_eq!(
        order.contributors.len(),
        1,
        "a refused strategy is carried as a contributor to a live order"
    );
    assert_eq!(order.contributors[0].strategy.as_str(), STRATEGY);
    assert_eq!(order.side, BookSide::Ask);
    assert!(
        report.cancelled.is_empty(),
        "a refused strategy still cancelled something"
    );
    Ok(())
}

#[test]
fn the_delta_the_centre_receives_names_every_strategy_behind_a_netted_order() -> Result<()> {
    // Attribution is a central-plane job and netting is an edge-plane fact, so
    // the contributor vector has to cross the uplink or the centre attributes
    // a netted fill to `strategy` alone — the largest contributor — and credits
    // one strategy with another's trade.
    let mut cell = cell_with_two_strategies(SignalKind::Enter, "100")?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("500"), t(15))?;

    let report = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        report.signals.len(),
        2,
        "the premise needs two firing strategies"
    );
    assert_eq!(report.orders.len(), 1, "the premise needs one netted order");

    let delta = cell.state_delta(&report, t(20));
    assert_eq!(delta.orders.len(), 1);
    let sent = &delta.orders[0];
    assert_eq!(
        sent.contributors.len(),
        2,
        "the delta named {} contributor(s) for an order two strategies caused",
        sent.contributors.len()
    );
    // The signed shares sum to what the order actually was, so the centre can
    // check the decomposition rather than trust it.
    let summed: Decimal = sent
        .contributors
        .iter()
        .map(|contributor| contributor.signed_size)
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(summed.abs(), sent.quantity);
    // And each contributor carries the revisions its own strategy read. The
    // strategies here share a feature, so the assertion is that the list is
    // populated at all — an empty one would make the field unattributable.
    for contributor in &sent.contributors {
        assert!(
            !contributor.inputs.is_empty(),
            "{} contributed with no feature revisions, so its share of a fill \
             cannot be traced to the values that caused it",
            contributor.strategy.as_str()
        );
    }
    Ok(())
}

#[test]
fn a_netted_order_spends_every_contributing_strategy_s_own_envelope() -> Result<()> {
    // Netting collapses two strategies into one order, and the capital that
    // order commits has to come out of both envelopes in proportion to what
    // each asked for. Charging it all to one strategy would let the other keep
    // spending capital it has already used, which is a limit that cannot fire.
    let mut cell = cell_with_two_strategies(SignalKind::Exit, "40")?;
    let mut gateway = SimulatedGateway::new(venue("XLON"), 7, start())?;
    gateway.seed_touch(&object("ACME"), Side::Sell, dec!("100"), dec!("500"), t(15))?;
    gateway.seed_touch(&object("ACME"), Side::Buy, dec!("100"), dec!("500"), t(15))?;

    let report = cell.work(t(20), &mut gateway)?;
    assert_eq!(report.orders.len(), 1, "the premise needs one netted order");
    let order = &report.orders[0];
    assert_eq!(
        order.contributors.len(),
        2,
        "the premise needs two contributors"
    );

    let delta = cell.state_delta(&report, t(20));
    let charge = |strategy: &str| -> Decimal {
        delta
            .utilisation
            .iter()
            .find(|entry| entry.strategy.as_str() == strategy)
            .map(|entry| entry.utilisation.gross_committed)
            .expect("both strategies are deployed, so both report utilisation")
    };
    let big = charge(STRATEGY);
    let small = charge("book-pressure-two");

    // Both spent something: an all-to-the-largest charge leaves the other at
    // zero and its envelope untouched.
    assert!(
        big.is_positive() && small.is_positive(),
        "one contributor was charged nothing: {big} and {small}"
    );
    // Together they were charged exactly what was sent — not what was asked.
    // Charging each its own unnetted ask would total the gross, which is
    // larger, and would overstate the capital actually at risk.
    assert_eq!(
        big + small,
        order.quantity * order.price,
        "the envelopes together were charged {} for an order worth {}",
        big + small,
        order.quantity * order.price
    );
    // And in the right direction: the strategy that asked for more paid more.
    assert!(
        big > small,
        "the larger contributor was charged {big} against the smaller's {small}"
    );
    Ok(())
}
