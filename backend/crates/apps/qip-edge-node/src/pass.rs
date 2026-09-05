//! One pass of the node: feed, decide, act, reconcile.
//!
//! The step `main.rs` runs on its serve loop once the halt has been polled
//! and the mesh exchanged. It is in the library so a test can drive the
//! assembled cell through it against the same gateway and feed the binary
//! holds, and prove the pass-time series move — which the binary, being a
//! binary, cannot.
//!
//! # What a pass may cost
//!
//! Everything in it is arithmetic and memory. The feed reads a book held in
//! this process, the gateway matches in this process, and the reconciler
//! compares two vectors. The one call in the cell that may block —
//! `Cell::flush` — is not here; the serve loop runs it separately. So a pass
//! blocks the health server for exactly one pass, bounded by the feed's
//! instrument and level caps and the cell's own strategy budget, and never
//! by anything outside the process.
//!
//! # A halted node runs no pass
//!
//! The cell's own `work` counts a halted pass and refuses it under the halt
//! that stopped it. This loop does not reach it: a node that is halted feeds
//! its books — a cell that stops seeing the market cannot tell whether it is
//! safe to resume — and then does nothing else, so that a halted node's
//! pass counter is flat and its order counter cannot move by any path. The
//! halt itself is already charted by the gauge every halt wire writes.
//!
//! # Where the requote sits in the pass
//!
//! After the venue's reports have been drained into the cell and before the
//! cell decides, and only past the halt check. The order is the whole of
//! the requoter's correctness: judged before the drain, a fill that arrived
//! this pass is not yet booked and the replacement would carry a quantity
//! that no longer exists; judged before the halt check, a halted node would
//! send. Every call the cell makes to the venue on the pass goes through
//! [`RequotingPlacer`], so a replacement's fresh venue id reaches the cell
//! as the id the cell sent — see `crate::reprice`.

use crate::feed::{FeedTick, SimulatedFeed};
use crate::gateway::SimulatedGateway;
use crate::reprice::{Requote, Requoter, RequotingPlacer};
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_edge::cell::{Cell, ConfirmedFill, WorkReport};

/// Running totals for the health surface.
///
/// The metric registry holds the same facts as series; this is the
/// JSON-shaped copy a probe reads without a scrape, kept because the two
/// answer different questions — a chart wants a rate, a probe wants "has
/// this node ever run a pass".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PassStats {
    /// Passes in which `Cell::work` ran.
    pub passes: u64,
    /// Turns of the loop that found the cell halted and ran no pass.
    pub halted: u64,
    pub refusals: u64,
    pub signals: u64,
    pub orders: u64,
    /// Fills the venue reported and the cell confirmed, on any turn of the
    /// loop — a halted node still learns what filled.
    pub fills: u64,
    /// Resting orders withdrawn at their time to live, on any turn.
    pub expired: u64,
    /// Resting orders withdrawn for drift and re-sent at the touch: one
    /// cancel acknowledged, one new order. Counted only when the
    /// replacement reached the venue; a refused cancel or replacement is
    /// reported on the outcome and counted nowhere.
    pub repriced: u64,
    /// Reconciliation breaks found after a pass; each one has halted the cell.
    pub breaks: u64,
}

/// What one turn of the loop did.
#[derive(Debug)]
pub enum PassOutcome {
    /// The cell was halted; the feed was published, fills already out were
    /// confirmed, and nothing else ran.
    Halted {
        feed: FeedTick,
        fills: Vec<ConfirmedFill>,
    },
    /// The pass ran.
    Ran {
        feed: FeedTick,
        report: WorkReport,
        /// What the requoter did to each resting order it did not leave
        /// alone, before the cell decided. Empty with no requoter.
        requotes: Vec<Requote>,
        /// Disagreements between the cell's fills and the venue's account,
        /// as the reconciler described them. Non-empty means the cell is now
        /// halted by its kill switch.
        breaks: Vec<String>,
    },
}

/// Feed, decide, act, and reconcile — once.
///
/// Takes the simulated gateway and the simulated feed by their own types:
/// there is no signature here that accepts a live gateway, so the pass loop
/// cannot be pointed at one by a later edit to `main.rs` without this
/// function changing. The requoter is optional — a node with no declared
/// requote policy leaves every resting order where it was sent — and when
/// present it sees the venue only through the same gateway the cell does.
pub fn run_pass(
    cell: &mut Cell,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    requoter: Option<&mut Requoter>,
    stats: &mut PassStats,
    now: Timestamp,
) -> Result<PassOutcome> {
    let tick = feed.publish(gateway, cell, now)?;
    // From here every call the cell makes to the venue is mapped, so a
    // replacement's venue id reaches the cell as the id the cell sent.
    let mut gateway = RequotingPlacer::new(gateway, requoter);
    // The venue's answers about orders already out, before the halt check:
    // a halted node sends nothing and still has to book what filled, or the
    // reconciler below compares the venue's account with a record that
    // stopped listening.
    let mut already_out = cell.confirm_execution_reports(&mut gateway, now);
    // And withdraw what has rested past its time to live, on a halted node
    // too: withdrawing is not sending. Withdrawing confirms internally, so
    // the fills it books arrive on the tail of the cell's record and this
    // takes them by position.
    //
    // By position, and not by expired order id, because the record is
    // cumulative. A partially filled order stays open — `confirm` closes at
    // `filled >= quantity` — and its fill stays in `fills()` until a clean
    // reconciliation settles a *closed* order. Filtering by id therefore
    // matched that old fill again on the turn the order expired, counted it
    // in `stats.fills` a second time and spliced it into `report.fills` a
    // second time, and `main.rs` publishes that report to the centre, which
    // attributes fill shares and calls quantity beyond what it saw sent an
    // `unsent_fill` break. The same filter dropped the other direction: the
    // internal confirm drains every pending report, so a fill on a still
    // working order was excluded here and `Cell::work` then found the
    // channel empty — booked by the cell, charted by the venue counter, and
    // in no delta the centre ever saw.
    let known = cell.fills().len();
    let expired = cell.withdraw_expired(&mut gateway, now);
    stats.expired = stats.expired.saturating_add(expired.len() as u64);
    // Nothing between the two lines above removes from the record — only
    // `settle`, reached from `reconcile`, does — so the tail is exactly what
    // the withdrawal confirmed, and empty when it confirmed nothing.
    already_out.extend_from_slice(&cell.fills()[known..]);
    stats.fills = stats.fills.saturating_add(already_out.len() as u64);
    if cell.is_halted() {
        stats.halted = stats.halted.saturating_add(1);
        return Ok(PassOutcome::Halted {
            feed: tick,
            fills: already_out,
        });
    }

    // Staleness is judged only now: past the halt check, because a requote
    // sends; after the drain, because a fill booked above is a quantity the
    // replacement must not carry. Once per book update, which is once per
    // pass — the feed published exactly one above.
    let requotes = gateway.reprice(cell, now);
    stats.repriced = stats.repriced.saturating_add(
        requotes
            .iter()
            .filter(|requote| matches!(requote, Requote::Replaced { .. }))
            .count() as u64,
    );

    let mut report = cell.work(now, &mut gateway)?;
    stats.passes = stats.passes.saturating_add(1);
    stats.refusals = stats.refusals.saturating_add(report.refusals.len() as u64);
    stats.signals = stats.signals.saturating_add(report.signals.len() as u64);
    stats.orders = stats.orders.saturating_add(report.orders.len() as u64);
    stats.fills = stats.fills.saturating_add(report.fills.len() as u64);
    // One list, oldest first, so the report names every fill this turn
    // confirmed whichever side of the halt check it was confirmed on.
    report.fills.splice(0..0, already_out);

    // The venue's own account of what filled, on the channel the cell does
    // not write. Drained every pass so a disagreement is found on the pass
    // after the fill rather than at some later probe, and reconciled every
    // pass because the reconciler is what turns a disagreement into a halt.
    for fill in gateway.drain_drop_copies() {
        cell.observe_drop_copy(fill);
    }
    let breaks: Vec<String> = cell
        .reconcile(now)
        .iter()
        .map(|discrepancy| discrepancy.describe())
        .collect();
    stats.breaks = stats.breaks.saturating_add(breaks.len() as u64);

    Ok(PassOutcome::Ran {
        feed: tick,
        report,
        requotes,
        breaks,
    })
}
