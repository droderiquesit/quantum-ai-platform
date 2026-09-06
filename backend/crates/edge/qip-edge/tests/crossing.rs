//! §27.1's crossing interval, driven through `Cell::work` rather than the
//! private seam.
//!
//! The unit tests beside `cross_internally` prove the window's arithmetic on
//! nets they build by hand. What they cannot prove is that a *pass* is what
//! advances a `Passes` window — `work` owns the counter — or that a cross
//! admitted by the window reaches the report, the chain and the series by
//! the same path a per-net cross does. So this drives two strategies that
//! cancel completely, pass after pass, and reads what the cell reports.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, CrossingInterval, Placer, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Metrics, labels, names};
use qip_orderbook::venue::VenueState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

/// A book quoting 99 / 101, so the mid is 100.
fn book() -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    for (index, (side, price, size)) in
        [(BookSide::Bid, "99", "500"), (BookSide::Ask, "101", "400")]
            .iter()
            .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            object(),
            Origin::new(venue(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side: *side,
                price: d(price),
                quantity: d(size),
                order_count: None,
            },
            when,
            when,
        ))?;
    }
    Ok(state)
}

/// A strategy whose one rule always holds, so every pass raises the same
/// signal at the same size.
fn firing_strategy(id: &str, kind: SignalKind, size: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            kind,
            Expr::Flag(true),
            Expr::Exact(d(size)),
            Expr::Statistic(0.5),
            10,
        ),
    );
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
            vec![venue()],
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

#[derive(Debug, Default)]
struct PaperGateway;

impl Placer for PaperGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        Ok(())
    }
}

/// A cell running the given strategies, each firing at its size every pass,
/// with the crossing cap measured over `interval` when one is given.
fn cell_with(
    strategies: &[(&str, SignalKind, &str)],
    interval: Option<CrossingInterval>,
) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut config = CellConfig::new(CELL, REGION).with_venue(venue());
    if let Some(interval) = interval {
        config = config.with_crossing_interval(interval)?;
    }
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));
    cell.track(book()?);
    for (id, kind, size) in strategies {
        let (compiled, program) = firing_strategy(id, *kind, size)?;
        cell.deploy_with_pricing(
            compiled,
            program,
            signed_envelope(id)?,
            PricingPolicy::Marketable,
        )?;
    }
    Ok((cell, metrics))
}

/// A cell whose two strategies want exactly opposite things every pass.
fn cancelling_cell(interval: Option<CrossingInterval>) -> Result<(Cell, Arc<Metrics>)> {
    cell_with(
        &[
            ("alpha", SignalKind::Enter, "100"),
            ("beta", SignalKind::Exit, "100"),
        ],
        interval,
    )
}

fn strategy(id: &str) -> StrategyId {
    StrategyId::new(id)
}

/// The price the chain sealed the one cross in `report` at, read back from
/// the journal entry rather than from the report, so a settlement can be
/// checked against what a replay would see.
fn journaled_cross_price(cell: &Cell) -> Result<Decimal> {
    let prices: Vec<Decimal> = cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::CrossedInternally {
                quantity, price, ..
            } if quantity != "0" => Decimal::parse(price),
            _ => None,
        })
        .collect();
    match prices.as_slice() {
        [price] => Ok(*price),
        other => Err(Error::invalid(format!(
            "expected exactly one non-zero cross in the chain, found {other:?}"
        ))),
    }
}

fn work(cell: &mut Cell, at: Timestamp) -> Result<WorkReport> {
    cell.work(at, &mut PaperGateway)
}

#[test]
fn over_a_three_pass_interval_a_repeated_full_cancellation_crosses_on_the_second_pass_at_the_mid()
-> Result<()> {
    // Pass one: 100 against 100 is 100 of 200 gross, over the cap, refused.
    // Pass two: the window now holds pass one's 200 gross and nothing
    // crossed, so 100 of 400 is a quarter and the cross is admitted — both
    // strategies filled at the mid, and still no order at the venue.
    let (mut cell, metrics) = cancelling_cell(Some(CrossingInterval::Passes(3)))?;

    let first = work(&mut cell, t(50))?;
    assert_eq!(
        first.cancelled.len(),
        1,
        "the premise failed: the two intents did not cancel: {:?}",
        first.refusals
    );
    assert!(
        first.crosses.is_empty(),
        "the first pass has no window behind it and must read per net"
    );

    let second = work(&mut cell, t(51))?;
    assert_eq!(
        second.crosses.len(),
        1,
        "the second pass did not cross inside the window: {:?}",
        second.refusals
    );
    let cross = &second.crosses[0];
    // Both sides fill in full: the matched size is what each strategy asked
    // for after the degradation floor narrowed it — a cell with no policy
    // sizes conservatively — so it is read from the net rather than assumed.
    let asked: Vec<Decimal> = second.cancelled[0]
        .contributors
        .iter()
        .map(|contributor| contributor.signed_size.abs())
        .collect();
    assert_eq!(asked.len(), 2, "the premise failed: {:?}", second.cancelled);
    assert_eq!(
        asked[0], asked[1],
        "the premise needs equal and opposite asks"
    );
    assert!(asked[0].is_positive(), "the premise needs a non-zero ask");
    assert_eq!(cross.quantity, asked[0], "both sides fill in full");
    assert_eq!(
        cross.price,
        d("100"),
        "the cross is priced at the book's mid"
    );
    assert_eq!(cross.bought, vec![StrategyId::new("alpha")]);
    assert_eq!(cross.sold, vec![StrategyId::new("beta")]);
    assert!(
        second.orders.is_empty(),
        "a cross is a booking on top of netting and nothing reached the venue"
    );

    // Passes three and four: the third crosses (200 of 600), the fourth is
    // judged over passes two to four only — 300 of 600, half — and is
    // refused. It is `work` that advances the window; a counter that stood
    // still would leave pass one inside it and the fourth would cross too.
    let third = work(&mut cell, t(52))?;
    assert_eq!(
        third.crosses.len(),
        1,
        "the third pass did not cross: {:?}",
        third.refusals
    );
    let fourth = work(&mut cell, t(53))?;
    assert!(
        fourth.crosses.is_empty(),
        "the fourth pass crossed, so the window did not slide with the passes"
    );
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_INTERNAL_CROSSES,
            &labels([("cell", CELL), ("region", REGION), ("venue", VENUE)])
        ),
        2,
        "the two window-admitted crosses were not counted on the venue that priced them"
    );
    Ok(())
}

#[test]
fn with_no_interval_the_same_two_passes_never_cross() -> Result<()> {
    // The default, held here through `work` as the unit test holds it at the
    // seam: unset, the second pass is judged exactly as the first, and the
    // full cancellation is refused under the cap both times.
    let (mut cell, _metrics) = cancelling_cell(None)?;
    for pass in [t(50), t(51)] {
        let report = work(&mut cell, pass)?;
        assert_eq!(
            report.cancelled.len(),
            1,
            "the premise failed: {:?}",
            report.refusals
        );
        assert!(
            report.crosses.is_empty(),
            "the per-net default crossed a full cancellation at {pass:?}"
        );
        assert!(
            report
                .refusals
                .iter()
                .any(|(gate, _)| gate == "internal_cross_cap"),
            "the refusal did not name the cap: {:?}",
            report.refusals
        );
    }
    Ok(())
}

// --- settlement (§27.1 "both strategies receive their full intended fill") --

#[test]
fn a_booked_cross_moves_both_contributors_lots_and_cash_at_the_journaled_mid_and_the_cash_legs_cancel()
-> Result<()> {
    // One hundred against forty inside one pass: forty crosses (under the
    // cap, 40 * 5 < 140 * 2) and sixty goes to the venue. Before this the
    // cross was journaled and nothing moved — a reader of
    // `crossed_internally` in the chain assumed books that did not exist
    // (traceability F7). Now the buyer's lot is up and its cash down by the
    // notional at the price the chain sealed, the seller's the reverse, and
    // the venue-facing aggregate has not moved because the venue saw none of
    // it.
    let (mut cell, metrics) = cell_with(
        &[
            ("alpha", SignalKind::Enter, "100"),
            ("beta", SignalKind::Exit, "40"),
        ],
        None,
    )?;
    let (alpha, beta) = (strategy("alpha"), strategy("beta"));
    assert!(
        cell.strategy_position(&alpha, &venue(), &object())
            .is_zero()
            && cell.strategy_cash(&alpha).is_zero()
            && cell.strategy_position(&beta, &venue(), &object()).is_zero()
            && cell.strategy_cash(&beta).is_zero(),
        "the premise failed: a fresh cell already holds lots or cash"
    );

    let report = work(&mut cell, t(50))?;
    assert_eq!(
        report.crosses.len(),
        1,
        "the premise failed: no cross was booked: {:?}",
        report.refusals
    );
    let cross = &report.crosses[0];
    assert!(
        cross.quantity.is_positive(),
        "the premise failed: the cross has no size"
    );
    assert_eq!(cross.bought, vec![alpha.clone()], "the buyer is alpha");
    assert_eq!(cross.sold, vec![beta.clone()], "the seller is beta");
    assert_eq!(
        report.orders.len(),
        1,
        "the premise failed: the residual did not reach the venue"
    );

    // The price is read back from the chain, not from the report: the
    // journal is what a replay sees, so the books must agree with it.
    let journaled = journaled_cross_price(&cell)?;
    assert_eq!(
        journaled, cross.price,
        "the report and the chain disagree on the cross price"
    );
    let notional = journaled
        .checked_mul(cross.quantity)
        .ok_or_else(|| Error::invalid("the fixture notional overflowed"))?;
    assert!(
        notional.is_positive(),
        "the premise needs a positive notional"
    );

    assert_eq!(
        cell.strategy_position(&alpha, &venue(), &object()),
        cross.quantity,
        "the buyer's lot did not rise by the crossed quantity"
    );
    assert_eq!(
        cell.strategy_position(&beta, &venue(), &object()),
        -cross.quantity,
        "the seller's lot did not fall by the crossed quantity"
    );
    assert_eq!(
        cell.strategy_cash(&alpha),
        -notional,
        "the buyer did not pay quantity times the journaled mid"
    );
    assert_eq!(
        cell.strategy_cash(&beta),
        notional,
        "the seller did not receive quantity times the journaled mid"
    );
    assert!(
        (cell.strategy_cash(&alpha) + cell.strategy_cash(&beta)).is_zero(),
        "the two cash legs of a cross are not equal and opposite"
    );
    assert!(
        cell.position(&venue(), &object()).is_zero(),
        "a cross moved the venue-facing position, which the venue never saw"
    );
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_INTERNAL_CROSSES,
            &labels([("cell", CELL), ("region", REGION), ("venue", VENUE)])
        ),
        1,
        "the settled cross was not counted on the venue that priced it"
    );
    Ok(())
}

#[test]
fn a_cross_above_the_cap_is_still_refused_and_moves_no_lot_or_cash() -> Result<()> {
    // The settlement sits behind the cap, not beside it. A full cancellation
    // under the per-net default is over the forty percent cap by arithmetic
    // (see `Cell::cross_internally`); it must be refused under the cap's own
    // gate as before, and a refused cross must leave every book untouched —
    // a lot that moved on a cross the chain says was refused is a position
    // no record explains.
    let (mut cell, _metrics) = cancelling_cell(None)?;
    let (alpha, beta) = (strategy("alpha"), strategy("beta"));

    let report = work(&mut cell, t(50))?;
    assert_eq!(
        report.cancelled.len(),
        1,
        "the premise failed: the two intents did not cancel: {:?}",
        report.refusals
    );
    assert!(
        report
            .refusals
            .iter()
            .any(|(gate, _)| gate == "internal_cross_cap"),
        "the premise failed: the cap did not refuse the cross: {:?}",
        report.refusals
    );
    assert!(
        report.crosses.is_empty(),
        "a cross above the cap was booked"
    );

    for id in [&alpha, &beta] {
        assert!(
            cell.strategy_position(id, &venue(), &object()).is_zero(),
            "{} holds a lot from a cross the cap refused",
            id.as_str()
        );
        assert!(
            cell.strategy_cash(id).is_zero(),
            "{} holds cash from a cross the cap refused",
            id.as_str()
        );
    }
    Ok(())
}

#[test]
fn a_cross_with_two_strategies_on_one_side_is_refused_rather_than_settled_by_a_guess() -> Result<()>
{
    // Sixty and forty buying against forty selling: forty would cross, and
    // it is under the cap (40 * 5 < 140 * 2). But the record names one
    // size and two buyers, so the settlement cannot be read from it — only
    // guessed, by splitting evenly or pro rata — and the centre refuses the
    // same record for the same reason. The cell refuses it under its own
    // gate before the record exists: nothing is booked, counted or moved,
    // and the intents still net as before.
    let (mut cell, metrics) = cell_with(
        &[
            ("alpha", SignalKind::Enter, "60"),
            ("gamma", SignalKind::Enter, "40"),
            ("beta", SignalKind::Exit, "40"),
        ],
        None,
    )?;

    let report = work(&mut cell, t(50))?;
    assert_eq!(
        report.signals.len(),
        3,
        "the premise failed: the three strategies did not all fire"
    );
    assert!(
        !report
            .refusals
            .iter()
            .any(|(gate, _)| gate == "internal_cross_cap"),
        "the premise failed: the cap refused a cross this test needs under it: {:?}",
        report.refusals
    );
    assert!(
        report
            .refusals
            .iter()
            .any(|(gate, _)| gate == "internal_cross_attribution"),
        "the two-buyer cross was not refused under the attribution gate: {:?}",
        report.refusals
    );
    assert!(
        report.crosses.is_empty(),
        "a cross with two buyers was booked: {:?}",
        report.crosses
    );
    assert_eq!(
        report.orders.len(),
        1,
        "the net residual did not reach the venue"
    );
    for id in ["alpha", "gamma", "beta"] {
        let id = strategy(id);
        assert!(
            cell.strategy_position(&id, &venue(), &object()).is_zero()
                && cell.strategy_cash(&id).is_zero(),
            "{} was settled on a cross that was refused",
            id.as_str()
        );
    }
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_INTERNAL_CROSSES,
            &labels([("cell", CELL), ("region", REGION), ("venue", VENUE)])
        ),
        0,
        "a refused cross was counted as a cross"
    );
    Ok(())
}
