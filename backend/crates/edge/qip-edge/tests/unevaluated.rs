//! What a cell does about a figure it cannot evaluate.
//!
//! The finding this suite answers. `qip-risk`'s `RiskState` gained
//! `unevaluated`, a map of figures a producer set out to compute and could
//! not, and `PreTradeChecker::check` refuses every order while one entry
//! stands. That is the central plane only:
//!
//! ```text
//! $ grep -rn "RiskState\|PreTradeChecker" backend/crates/edge
//! $ echo $?
//! 1
//! ```
//!
//! A cell reaches a venue through its own gates and never sees that map. The
//! question is whether it therefore has the hole `unevaluated` was added to
//! close: a control whose input is missing, which *abstains* rather than
//! refusing, so that at the venue "the control did not run" and "the control
//! passed" are the same event.
//!
//! It does not, and this file is what makes that enforced rather than
//! asserted. Every figure `Cell::work` sizes against is either measured from
//! the cell's own book on this pass or refused under a named gate literal
//! before an order exists — the edge's idiom for the same discipline, reached
//! by a different mechanism because a cell decides alone (ADR 0008) and may
//! not import the centre's risk state without creating the second source of
//! truth `.claude/rules/architecture/00-boundaries.md` forbids. The argument
//! in full, and what would overturn it, is in
//! `docs/architecture/edge-fail-closed-figures.md`.
//!
//! The table below is the load-bearing part. Add a gate to `Cell::work` that
//! reads a figure and takes a `None` arm quietly, and the premise test still
//! passes while nothing here notices — so the table must be extended with the
//! figure whenever one is added. That is stated as a duty because no test can
//! discover a control that was never written.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::degradation::{Capability, Freshness};
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{BeliefPriors, CausalDigest, EpisodicDigest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::collections::BTreeMap;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-tests";

/// The size every strategy below asks for. Well inside the touch of
/// [`book`], so the depth rule is not the one answering.
const ASK: &str = "10";

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

fn level(sequence: u64, side: BookSide, price: &str, size: &str) -> MarketMessage {
    MarketMessage::new(
        object(),
        Origin::new(venue(), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: d(price),
            quantity: d(size),
            order_count: None,
        },
        t(1),
        t(1),
    )
}

/// 99 bid for 500, 101 offered for 400: a mid of 100 and depth on both sides.
fn book(status: VenueStatus) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(), venue(), status);
    state.apply(&level(0, BookSide::Bid, "99", "500"))?;
    state.apply(&level(1, BookSide::Ask, "101", "400"))?;
    Ok(state)
}

/// A book with a bid and no offer: depth on the side a buy takes is absent,
/// and so is the mid, because a mid is an average of two numbers.
fn one_sided_book() -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    state.apply(&level(0, BookSide::Bid, "99", "500"))?;
    Ok(state)
}

fn firing_strategy(id: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(d(ASK)),
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

/// A payload whose three capability-bearing slots were produced at
/// `issued_at`, so the sizing multiplier is one and every size below is the
/// size the gates judge.
fn fresh_payload(sequence: u64, issued_at: Timestamp) -> PolicyPayload {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, issued_at);
    payload.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::new(),
        },
        issued_at,
    );
    payload.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: Vec::new(),
        },
        issued_at,
    );
    payload.episodic_digest = Slot::produced(
        EpisodicDigest {
            digest: "d".to_string(),
            episodes: 0,
        },
        issued_at,
    );
    payload
}

/// Which figure a fixture withholds from the cell.
#[derive(Clone, Copy, Debug)]
enum Withheld {
    /// The cell holds no book for the instrument at all.
    Book,
    /// A book exists but a sequence gap abandoned it.
    StaleBook,
    /// A full book at a venue this cell cannot reach.
    Unreachable,
    /// A full book at a venue that is reachable and not trading.
    NotTrading,
    /// A book with one side, so there is no mid to reason at.
    Mid,
    /// A deployment that never stated how its intents should be priced.
    PricingPolicy,
    /// Nothing. The premise fixture.
    Nothing,
}

/// A cell with one always-firing strategy, one venue and a fresh policy, less
/// whatever `withheld` names.
fn cell_without(withheld: Withheld) -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    let fresh = fresh_payload(1, t(5)).signed(POLICY_KEY)?;
    cell.apply_policy(VerifiedPolicy::verify(fresh, POLICY_KEY, CELL, t(5))?, t(5))?;

    match withheld {
        Withheld::Book => {}
        Withheld::StaleBook => {
            let mut state = book(VenueStatus::Open)?;
            state.reset("a sequence gap was abandoned");
            cell.track(state);
        }
        Withheld::Unreachable => cell.track(book(VenueStatus::Unreachable)?),
        Withheld::NotTrading => cell.track(book(VenueStatus::Halted)?),
        Withheld::Mid => cell.track(one_sided_book()?),
        Withheld::PricingPolicy | Withheld::Nothing => cell.track(book(VenueStatus::Open)?),
    }

    let (compiled, program) = firing_strategy("alpha")?;
    let envelope = signed_envelope("alpha")?;
    match withheld {
        // `deploy` is the constructor that states no pricing policy. It is a
        // real API rather than a contrivance, and this is what it costs.
        Withheld::PricingPolicy => cell.deploy(compiled, program, envelope)?,
        _ => cell.deploy_with_pricing(compiled, program, envelope, PricingPolicy::Marketable)?,
    }
    Ok(cell)
}

fn work(cell: &mut Cell) -> Result<WorkReport> {
    cell.work(t(10), &mut PaperGateway)
}

fn refusals_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        // The whole token, not a prefix: `book` is a prefix of nothing here
        // today, but `stale_book` contains `book` and a `contains` would have
        // read one refusal as both.
        .filter(|(g, _)| g == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

#[test]
fn the_intact_cell_sends_one_order_so_every_withheld_figure_below_is_about_that_figure()
-> Result<()> {
    // The premise of the table. A fixture already refused for some other
    // reason proves nothing about the figure a row withholds, and a cell that
    // never traded would make every row below pass forever.
    let mut cell = cell_without(Withheld::Nothing)?;
    let report = work(&mut cell)?;
    assert_eq!(
        report.orders.len(),
        1,
        "the intact fixture sent no order, so every row of the table is vacuous: {report:?}"
    );
    assert!(
        report.refusals.is_empty(),
        "the intact fixture refused something: {:?}",
        report.refusals
    );
    assert_eq!(report.orders[0].quantity, d(ASK));
    Ok(())
}

#[test]
fn a_figure_the_cell_cannot_evaluate_refuses_under_a_named_gate_and_sends_nothing() -> Result<()> {
    // The property `RiskState::unevaluated` holds at the centre, held here by
    // the cell's own machinery instead: no figure `Cell::work` sizes against
    // can be absent and silently skipped. Each row names the gate literal the
    // refusal must be counted under, because a refusal filed under the wrong
    // gate is an operator reading the wrong chart, and a refusal filed under
    // none is the abstention this whole file exists to rule out.
    let rows: [(Withheld, &str, &str); 6] = [
        (
            Withheld::Book,
            "venue_selection",
            "no venue this cell may reach quotes the instrument",
        ),
        (
            Withheld::StaleBook,
            "stale_book",
            "a sequence gap was abandoned",
        ),
        (
            Withheld::Unreachable,
            "venue_selection",
            "no venue this cell may reach quotes the instrument",
        ),
        (Withheld::NotTrading, "venue_status", "the venue is halted"),
        (Withheld::Mid, "pricing", "the book serves no usable price"),
        (
            Withheld::PricingPolicy,
            "pricing",
            "deployed with no pricing policy",
        ),
    ];

    for (withheld, gate, expected) in rows {
        let mut cell = cell_without(withheld)?;
        let report = work(&mut cell)?;
        assert!(
            report.orders.is_empty(),
            "withholding {withheld:?} still sent an order: {report:?}"
        );
        let reasons = refusals_under(&report, gate);
        assert_eq!(
            reasons.len(),
            1,
            "withholding {withheld:?} produced no refusal under {gate}: {report:?}"
        );
        assert!(
            reasons[0].contains(expected),
            "the refusal for {withheld:?} does not say what is missing: {}",
            reasons[0]
        );
    }
    Ok(())
}

#[test]
fn a_cell_that_has_been_told_nothing_sizes_at_the_floor_rather_than_in_full() -> Result<()> {
    // The other half of the discipline, and the one that does not refuse: a
    // capability nobody has said anything about reads as unavailable, not as
    // healthy, so a cell whose policy feed has died narrows instead of
    // trading as though the centre were still talking to it. The failure this
    // prevents is the monitoring gap that is indistinguishable from good
    // news.
    //
    // The fixture is `cell_without(Nothing)` with the policy never applied,
    // so the only difference from the premise test above is the payload.
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    cell.track(book(VenueStatus::Open)?);
    let (compiled, program) = firing_strategy("alpha")?;
    cell.deploy_with_pricing(
        compiled,
        program,
        signed_envelope("alpha")?,
        PricingPolicy::Marketable,
    )?;

    let narrowing = cell.narrowing(t(10));
    for capability in [
        Capability::CausalGraph,
        Capability::EpisodicMemory,
        Capability::BeliefState,
    ] {
        assert_eq!(
            narrowing.freshness(capability),
            Freshness::Unavailable,
            "{} reads as something other than unavailable with no payload at all",
            capability.as_str()
        );
    }
    let multiplier = narrowing.sizing_multiplier();
    assert!(
        multiplier < Decimal::ONE && multiplier.is_positive(),
        "an uninformed cell sizes at {multiplier}, which is not a narrowing"
    );

    let report = work(&mut cell)?;
    assert_eq!(
        report.orders.len(),
        1,
        "the uninformed cell stopped rather than narrowed, which is not what ADR 0008 asks \
         for: {report:?}"
    );
    let sent = report.orders[0].quantity;
    assert_eq!(
        sent,
        d(ASK)
            .checked_mul(multiplier)
            .expect("a representable size"),
        "the order was not sized by the multiplier the table reported"
    );
    assert!(
        sent < d(ASK),
        "an uninformed cell sent {sent} against an ask of {ASK}, so absence cost it nothing"
    );
    Ok(())
}
