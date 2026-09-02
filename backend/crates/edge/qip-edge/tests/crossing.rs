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
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, CrossingInterval, Placer, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
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

/// A cell whose two strategies want exactly opposite things every pass, with
/// the crossing cap measured over `interval` when one is given.
fn cancelling_cell(interval: Option<CrossingInterval>) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut config = CellConfig::new(CELL, REGION).with_venue(venue());
    if let Some(interval) = interval {
        config = config.with_crossing_interval(interval)?;
    }
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));
    cell.track(book()?);
    for (id, kind) in [("alpha", SignalKind::Enter), ("beta", SignalKind::Exit)] {
        let (compiled, program) = firing_strategy(id, kind, "100")?;
        cell.deploy(compiled, program, signed_envelope(id)?)?;
    }
    Ok((cell, metrics))
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
