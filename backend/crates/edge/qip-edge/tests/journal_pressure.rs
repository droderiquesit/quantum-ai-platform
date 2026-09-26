//! ADR 0100 §6's fourth halt wire, driven through the cell.
//!
//! The spool a node writes its journal through can fill, lose its fence, stop
//! being written, or go stale. A cell that keeps adding exposure while the
//! record of that exposure has nowhere to go is trading with no record, and
//! the only honest answers are to size down while the spool is filling and to
//! stop adding exposure once it cannot take more — while still withdrawing
//! and still learning what filled, because both of those reduce or reveal
//! what the cell already holds.
//!
//! Every test here drives a `Cell`; the pure arithmetic of the reading is
//! small enough to be asserted through the same seam the node calls.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{
    Cell, CellConfig, ExecutionReport, GATE_JOURNAL_PRESSURE, Placer, PricingPolicy, WorkReport,
};
use qip_edge::dropcopy::DropCopyFill;
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::pressure::{Exhaustion, Freshness, JournalPressure, Narrowing};
use qip_edge::quoting::Admission;
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
const SYMBOL: &str = "ACME";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn level(sequence: u64, side: BookSide, price: &str, size: &str, when: Timestamp) -> MarketMessage {
    MarketMessage::new(
        object(),
        Origin::new(venue(), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: d(price),
            quantity: d(size),
            order_count: None,
        },
        when,
        when,
    )
}

/// A two-sided book with a mid of 100.
fn book() -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    for (index, (side, price, size)) in
        [(BookSide::Bid, "99", "500"), (BookSide::Ask, "101", "400")]
            .iter()
            .enumerate()
    {
        state.apply(&level(index as u64, *side, price, size, t(index as i64)))?;
    }
    Ok(state)
}

/// A strategy whose one rule always holds, so what stops an order is a gate
/// rather than a quiet market.
fn firing_strategy(id: &str, size: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            SignalKind::Enter,
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

/// A cell with a priced book and one firing strategy, with or without the
/// journal wire.
fn cell(wired: bool, pricing: PricingPolicy) -> Result<Cell> {
    let mut config = CellConfig::new(CELL, REGION).with_venue(venue());
    if wired {
        config = config.with_journal_wire();
    }
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    cell.track(book()?);
    let (compiled, program) = firing_strategy("alpha", "10")?;
    cell.deploy_with_pricing(compiled, program, signed_envelope("alpha")?, pricing)?;
    Ok(cell)
}

/// A venue that accepts everything, can withdraw, and reports exactly the
/// fills a test queues on it — never one it synthesised from an order.
#[derive(Debug, Default)]
struct ScriptedVenue {
    placed: Vec<(String, Decimal)>,
    cancelled: Vec<String>,
    /// The quantity each accepted order is still open for.
    open: Vec<(String, Decimal)>,
    pending: Vec<ExecutionReport>,
}

impl ScriptedVenue {
    /// Queue a fill the venue will report on the next drain, and take it off
    /// the order's open quantity so a later cancel answers what is left.
    fn fill(&mut self, order_id: &str, quantity: Decimal, price: Decimal, at: Timestamp) {
        for (id, open) in &mut self.open {
            if id == order_id {
                *open -= quantity;
            }
        }
        self.pending.push(ExecutionReport {
            order_id: order_id.to_string(),
            venue: venue(),
            quantity,
            price,
            at,
        });
    }
}

impl Placer for ScriptedVenue {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed.push((order_id.to_string(), quantity));
        self.open.push((order_id.to_string(), quantity));
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.pending)
    }

    fn can_cancel(&self) -> bool {
        true
    }

    fn cancel(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _at: Timestamp,
    ) -> Result<Decimal> {
        self.cancelled.push(order_id.to_string());
        Ok(self
            .open
            .iter()
            .find(|(id, _)| id == order_id)
            .map_or(Decimal::ZERO, |(_, quantity)| *quantity))
    }
}

fn refusals_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(g, _)| g == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

fn resting() -> Result<PricingPolicy> {
    PricingPolicy::rest_at_mid(Duration::from_secs(600))
}

#[test]
fn journal_pressure_narrows_sizing_before_it_halts_new_exposure() -> Result<()> {
    // RES-062: the spool narrows the cell before it is exhausted, not at it.
    // Two cells identical in every respect but the reading: the narrowed
    // one must ask for exactly half of what the normal one asks for, and
    // only an exhausted reading may stop it asking at all. The failure this
    // prevents is a "narrow" arm that reads as protection and sizes at full.
    let mut normal = cell(true, PricingPolicy::Marketable)?;
    let mut narrow = cell(true, PricingPolicy::Marketable)?;
    normal.apply_journal_pressure(JournalPressure::Normal, t(9))?;
    narrow.apply_journal_pressure(JournalPressure::Narrow(Narrowing::half()), t(9))?;

    let mut normal_venue = ScriptedVenue::default();
    let mut narrow_venue = ScriptedVenue::default();
    let full = normal.work(t(10), &mut normal_venue)?;
    let narrowed = narrow.work(t(10), &mut narrow_venue)?;
    assert_eq!(
        full.orders.len(),
        1,
        "the premise failed: the normal cell placed nothing to compare against: {:?}",
        full.refusals
    );
    assert!(
        full.orders[0].quantity.is_positive(),
        "the premise failed: the normal cell sized to nothing"
    );
    assert_eq!(
        narrowed.orders.len(),
        1,
        "a narrowed spool stopped the cell outright, which is Exhausted's job: {:?}",
        narrowed.refusals
    );
    assert!(!narrow.is_halted(), "a narrowing halted the cell");
    assert_eq!(
        narrowed.orders[0].quantity * d("2"),
        full.orders[0].quantity,
        "a narrowed spool did not halve the size the cell asked for"
    );
    assert!(
        narrow.journal().entries().iter().any(|entry| matches!(
            &entry.decision,
            Decision::Refused { gate, .. } if gate == GATE_JOURNAL_PRESSURE
        )),
        "the narrowing was not journaled under the journal-pressure gate"
    );

    // And only now, exhausted, does it stop adding exposure.
    narrow.apply_journal_pressure(JournalPressure::Exhausted(Exhaustion::OverBudget), t(11))?;
    let halted = narrow.work(t(12), &mut narrow_venue)?;
    assert!(
        halted.halted && halted.orders.is_empty(),
        "an exhausted spool left the cell adding exposure: {:?}",
        halted.orders
    );
    assert_eq!(
        refusals_under(&halted, GATE_JOURNAL_PRESSURE).len(),
        1,
        "the halted pass was not refused under the journal-pressure gate: {:?}",
        halted.refusals
    );
    Ok(())
}

#[test]
fn an_exhausted_journal_halts_new_exposure_while_withdrawals_and_fill_confirmations_continue()
-> Result<()> {
    // RES-013: an exhausted spool stops the cell adding exposure and must
    // not stop it reducing or learning about the exposure it has. A halt
    // that also stopped fill confirmation would leave the cell blind to a
    // fill the venue reported, and the reconciler would read it as a fill
    // the cell never knew about.
    let mut cell = cell(true, resting()?)?;
    cell.apply_journal_pressure(JournalPressure::Normal, t(9))?;
    let mut gateway = ScriptedVenue::default();
    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        1,
        "the premise failed: nothing was left resting: {:?}",
        first.refusals
    );
    let order = first.orders[0].clone();

    cell.apply_journal_pressure(JournalPressure::Exhausted(Exhaustion::Unwritable), t(11))?;
    assert!(
        cell.is_halted(),
        "an unwritable spool did not halt the cell"
    );
    let partial = d("1");
    assert!(
        partial < order.quantity,
        "the premise failed: the fill would close the order and leave nothing to withdraw"
    );
    gateway.fill(&order.order_id, partial, order.price, t(11));

    let pass = cell.work(t(12), &mut gateway)?;
    assert!(
        pass.halted && pass.orders.is_empty(),
        "an exhausted spool left the cell placing orders: {:?}",
        pass.orders
    );
    assert_eq!(gateway.placed.len(), 1, "a halted cell reached the venue");
    assert_eq!(
        pass.fills.len(),
        1,
        "the halted cell did not confirm the fill the venue reported"
    );
    assert_eq!(pass.fills[0].quantity, partial);
    assert_eq!(
        gateway.cancelled,
        vec![order.order_id.clone()],
        "the halted cell left its resting order at the venue instead of withdrawing it"
    );
    Ok(())
}

#[test]
fn a_cell_built_with_the_journal_wire_halts_new_exposure_until_it_is_handed_a_reading() -> Result<()>
{
    // Fails engaged: a node that armed the wire and never fed it must trade
    // nothing, because a wire whose state is unknown is a spool that may be
    // gone. The release is the first reading, and nothing else.
    let mut cell = cell(true, PricingPolicy::Marketable)?;
    assert_eq!(
        cell.journal_pressure(),
        Some(JournalPressure::Exhausted(Exhaustion::NeverApplied)),
        "a wired cell did not start in its never-applied state"
    );
    assert!(
        cell.is_halted(),
        "a wired cell with no reading reads as running"
    );

    let mut gateway = ScriptedVenue::default();
    let unfed = cell.work(t(10), &mut gateway)?;
    assert!(
        unfed.halted && unfed.orders.is_empty() && gateway.placed.is_empty(),
        "a wired cell with no reading placed an order"
    );
    assert_eq!(
        refusals_under(&unfed, GATE_JOURNAL_PRESSURE).len(),
        1,
        "the unfed pass was not refused under the journal-pressure gate: {:?}",
        unfed.refusals
    );

    // The node cannot claim the cell's own construction state.
    assert!(
        cell.apply_journal_pressure(JournalPressure::Exhausted(Exhaustion::NeverApplied), t(11))
            .is_err(),
        "a reading claiming never_applied was accepted"
    );

    cell.apply_journal_pressure(JournalPressure::Normal, t(11))?;
    let fed = cell.work(t(12), &mut gateway)?;
    assert_eq!(
        fed.orders.len(),
        1,
        "the first normal reading did not release the wire: {:?}",
        fed.refusals
    );
    Ok(())
}

#[test]
fn a_cell_built_without_the_journal_wire_trades_exactly_as_before() -> Result<()> {
    // The wire is opt-in. The same scripted session on an unwired cell and
    // on a wired cell handed Normal before every pass must leave identical
    // chains, entry for entry and digest for digest; a difference would be
    // the wire leaking into cells that never asked for it — the demo, the
    // chaos and e2e suites, the legacy node mode.
    let mut unwired = cell(false, resting()?)?;
    let mut wired = cell(true, resting()?)?;
    assert_eq!(
        unwired.journal_pressure(),
        None,
        "a cell built without the wire reads one"
    );
    assert!(
        unwired
            .apply_journal_pressure(JournalPressure::Normal, t(9))
            .is_err(),
        "an unwired cell accepted a reading it will never act on"
    );

    let mut unwired_venue = ScriptedVenue::default();
    let mut wired_venue = ScriptedVenue::default();
    for (pass, at) in [(0, t(10)), (1, t(20)), (2, t(30))] {
        wired.apply_journal_pressure(JournalPressure::Normal, at)?;
        let left = unwired.work(at, &mut unwired_venue)?;
        let right = wired.work(at, &mut wired_venue)?;
        assert_eq!(
            left.orders, right.orders,
            "pass {pass} placed different orders on the two cells"
        );
    }
    assert!(
        !unwired_venue.placed.is_empty(),
        "the premise failed: the session placed nothing, so identical chains prove nothing"
    );
    assert_eq!(
        unwired.journal().entries(),
        wired.journal().entries(),
        "the unwired cell's chain differs from a wired cell handed Normal every pass"
    );
    Ok(())
}

#[test]
fn a_reading_whose_heartbeat_is_older_than_the_bound_is_judged_exhausted_stale() -> Result<()> {
    // The last good reading is the one a dead writer leaves behind, so it is
    // exactly the one that must not be believed once its heartbeat has aged
    // past the bound.
    let bound = Duration::from_secs(5);
    assert_eq!(
        Freshness::judge(Duration::from_secs(5), bound)?,
        Freshness::Fresh,
        "the premise failed: a heartbeat exactly at the bound read as stale"
    );
    assert_eq!(
        JournalPressure::Normal.judged(Duration::from_secs(1), bound)?,
        JournalPressure::Normal,
        "the premise failed: a fresh reading was not passed through"
    );
    let stale = Duration::from_secs(6);
    assert_eq!(Freshness::judge(stale, bound)?, Freshness::Stale);
    for last_good in [
        JournalPressure::Normal,
        JournalPressure::Narrow(Narrowing::half()),
    ] {
        assert_eq!(
            last_good.judged(stale, bound)?,
            JournalPressure::Exhausted(Exhaustion::Stale),
            "a stale heartbeat left {last_good:?} in force"
        );
    }
    // A more specific exhaustion keeps its own cause.
    assert_eq!(
        JournalPressure::Exhausted(Exhaustion::Fenced).judged(stale, bound)?,
        JournalPressure::Exhausted(Exhaustion::Fenced)
    );

    // Refused, not clamped: a zero or negative bound, a heartbeat from the
    // future, and a narrowing that would widen.
    assert!(Freshness::judge(Duration::from_secs(1), Duration::ZERO).is_err());
    assert!(Freshness::judge(Duration::from_secs(1), Duration::from_secs(-1)).is_err());
    assert!(Freshness::judge(Duration::from_secs(-1), bound).is_err());
    assert!(Narrowing::new(d("1.5")).is_err());
    assert!(Narrowing::new(Decimal::ZERO).is_err());
    assert_eq!(Narrowing::new(d("0.5"))?, Narrowing::half());

    // And the judged reading, handed to a wired cell, halts it.
    let mut cell = cell(true, PricingPolicy::Marketable)?;
    cell.apply_journal_pressure(JournalPressure::Normal, t(9))?;
    assert!(!cell.is_halted(), "the premise is a running cell");
    cell.apply_journal_pressure(JournalPressure::Normal.judged(stale, bound)?, t(10))?;
    assert!(cell.is_halted(), "a stale spool left the cell running");
    Ok(())
}

#[test]
fn a_requote_under_exhausted_journal_pressure_withdraws_and_does_not_replace() -> Result<()> {
    // Red-team M16: the node's requoter runs before `Cell::work`, so a halt
    // checked only inside `work` never saw a requote, and an exhausted cell
    // went on replacing resting orders — new orders the spool could not
    // record. Under Exhausted the requote is neither fundable nor spendable,
    // and the resting order is withdrawn by the halt's mass cancel instead
    // of being replaced.
    let mut cell = cell(true, resting()?)?;
    cell.apply_journal_pressure(JournalPressure::Normal, t(9))?;
    let mut gateway = ScriptedVenue::default();
    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        1,
        "the premise failed: nothing was left resting to requote: {:?}",
        first.refusals
    );
    assert!(
        cell.requote_fundable(&venue(), t(11)),
        "the premise failed: a running cell with a full budget could not fund a requote"
    );
    let tokens_before = cell.quote_budget()[0].tokens;

    cell.apply_journal_pressure(JournalPressure::Exhausted(Exhaustion::Fenced), t(11))?;
    assert!(
        !cell.requote_fundable(&venue(), t(11)),
        "an exhausted spool left a requote fundable"
    );
    let spend = cell.spend_requote(&venue(), t(11));
    let Admission::Refused { reason } = spend else {
        panic!("an exhausted spool admitted a requote's spend: {spend:?}");
    };
    assert!(
        reason.contains("journal spool is exhausted"),
        "the refusal does not name the journal: {reason}"
    );
    assert_eq!(
        cell.quote_budget()[0].tokens,
        tokens_before,
        "a refused requote spent messages anyway"
    );

    // What the node does on a halted pass: withdraw first.
    let withdrawn = cell.withdraw_expired(&mut gateway, t(12));
    assert_eq!(
        withdrawn,
        vec![first.orders[0].order_id.clone()],
        "the exhausted cell left its resting order at the venue"
    );
    assert_eq!(
        gateway.placed.len(),
        1,
        "the withdrawn order was replaced at the venue"
    );
    Ok(())
}

#[test]
fn a_reconciliation_break_found_while_the_journal_is_exhausted_still_trips_the_kill_switch()
-> Result<()> {
    // The journal wire releases itself when the spool recovers. If a break
    // found while it held the cell asked "is the cell already halted?" and
    // took the journal halt for an answer, it would trip nothing — and the
    // cell would resume trading on a book that disagrees with the venue the
    // moment the disk freed up. The break must trip the kill switch, whose
    // release needs an operator credential, whatever else is holding the
    // cell.
    //
    // `break_cycle`, the other kill-switch seam, is not driven here: it is
    // reached only from inside `Cell::work` after the halt gate, so a cell
    // held by the journal wire returns before any cycle leg is sent and no
    // cycle can break while the wire is engaged.
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION)
        .with_venue(venue())
        .with_journal_wire();
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));
    cell.track(book()?);
    let (compiled, program) = firing_strategy("alpha", "10")?;
    cell.deploy_with_pricing(
        compiled,
        program,
        signed_envelope("alpha")?,
        PricingPolicy::Marketable,
    )?;
    let by = |key: &str, value: &str| labels([("cell", CELL), ("region", REGION), (key, value)]);

    cell.apply_journal_pressure(JournalPressure::Normal, t(9))?;
    let mut gateway = ScriptedVenue::default();
    let first = cell.work(t(10), &mut gateway)?;
    let Some(order) = first.orders.first().cloned() else {
        panic!(
            "the premise failed: nothing was sent for the venue to disagree about: {:?}",
            first.refusals
        );
    };

    cell.apply_journal_pressure(JournalPressure::Exhausted(Exhaustion::Unwritable), t(11))?;
    assert!(
        cell.is_halted() && !cell.autonomy().kill_switch().is_globally_tripped(),
        "the premise is a cell held by the journal wire alone"
    );

    // The venue's own account says half the order traded; the cell has
    // confirmed nothing. That is a break.
    cell.observe_drop_copy(DropCopyFill {
        order_id: order.order_id.clone(),
        venue: order.venue.clone(),
        quantity: order.quantity / d("2"),
        price: order.price,
        at: t(12),
    });
    let breaks = cell.reconcile(t(12));
    assert_eq!(
        breaks.len(),
        1,
        "the premise failed: a half fill the cell never confirmed reconciled clean"
    );
    assert!(
        cell.autonomy().kill_switch().is_globally_tripped(),
        "a break found while the journal was exhausted did not trip the kill switch"
    );

    // The spool recovers. The journal wire releases; the kill switch must not.
    cell.apply_journal_pressure(JournalPressure::Normal, t(13))?;
    assert!(
        cell.is_halted(),
        "the spool recovering resumed a cell that had found a reconciliation break"
    );
    let pass = cell.work(t(14), &mut gateway)?;
    assert!(
        pass.halted && pass.orders.is_empty(),
        "the cell traded again after a break because the spool recovered: {:?}",
        pass.orders
    );
    assert_eq!(
        refusals_under(&pass, "kill_switch").len(),
        1,
        "the pass after recovery was not refused under the kill switch: {:?}",
        pass.refusals
    );
    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.gauge(names::EDGE_HALTED, &by("source", "kill_switch")),
        Some(1.0),
        "the kill switch the break tripped is not charted"
    );
    assert_eq!(
        snapshot.gauge(names::EDGE_HALTED, &by("source", "journal")),
        Some(0.0),
        "the recovered spool still charts the journal halt"
    );
    Ok(())
}
