//! One pass of the node: feed, decide, act, reconcile.
//!
//! The step the decision loop (`scheduler::DecisionLoop::turn`) runs once
//! the wires, the mesh's inbound control and journal pressure have been
//! applied. It is in the library so a test can drive the
//! assembled cell through it against the same gateway and feed the binary
//! holds, and prove the pass-time series move — which the binary, being a
//! binary, cannot.
//!
//! # What a pass may cost
//!
//! Everything in it is arithmetic and memory. The feed reads a book held in
//! this process, the gateway matches in this process, and the reconciler
//! compares two vectors. The one call in the cell that may block —
//! `Cell::flush` — is not here, and since the outbox was wired it does not
//! block either: the decision loop flushes through a mirror that only offers
//! to a bounded channel. So a pass costs the decision thread exactly one
//! pass, bounded by the feed's instrument and level caps and the cell's own
//! strategy budget, and never anything outside the process.
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
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_edge::cell::{Cell, ConfirmedFill, WorkReport};
use qip_observability::golden::{GoldenSignals, Outcome};
use qip_observability::metrics::Metrics;
use qip_observability::sampling::LineSampler;
use std::sync::Arc;

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
    /// Trade edges of the arbitrage graph re-quoted because their book had
    /// moved, summed over every pass that ran a refresh (§30.1).
    pub edges_repriced: u64,
    /// Trade edges left holding their rate because their book had not
    /// moved, summed the same way. The denominator for the line above: a
    /// node whose books never move and a node with no desk both report zero
    /// re-priced, and only this tells them apart.
    pub edges_unchanged: u64,
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
        /// Boxed because a `WorkReport` is several vectors wide and the
        /// halted arm is two, and an enum sized for its widest arm on every
        /// pass — including the halted ones — is what the lint refuses.
        report: Box<WorkReport>,
        /// What the requoter did to each resting order it did not leave
        /// alone, before the cell decided. Empty with no requoter.
        requotes: Vec<Requote>,
        /// Disagreements between the cell's fills and the venue's account,
        /// as the reconciler described them. Non-empty means the cell is now
        /// halted by its kill switch.
        breaks: Vec<String>,
    },
}

/// The stderr lines the pass loop may write, bounded by a sample rate
/// (OBS-021).
///
/// A requote outcome is minted per stale resting order per book update, so a
/// line for each makes stderr proportional to message count, and on a
/// deployed node stderr is billed log ingestion read by nobody at that rate.
/// The sampler was first wired in `main.rs`, where no test could reach it:
/// a later edit restoring a bare `eprintln!` in the loop would have compiled,
/// passed every suite and put the volume back. It lives here so
/// `tests/pass.rs` drives it through real passes.
///
/// Nothing is lost by the bound. Every requote and every break is a series
/// (`qip_edge_orders_repriced_total`, `qip_edge_reconciliation_breaks_total`)
/// and a journal entry, and the first line of a window carries how many the
/// last one dropped.
#[derive(Debug)]
pub struct PassLog {
    sampler: LineSampler,
}

impl PassLog {
    /// At most `per_window` lines per `window`; a zero rate or window is
    /// refused by the sampler rather than silently dropping everything.
    pub fn new(per_window: u32, window: Duration) -> Result<Self> {
        Ok(Self {
            sampler: LineSampler::new(per_window, window)?,
        })
    }

    /// Write what one turn of the loop has to say, and return how many lines
    /// were written.
    ///
    /// A failed write is ignored on purpose: `eprintln!` panics when stderr
    /// is closed, and a cell that stops deciding because nobody is reading
    /// its log has the dependency backwards.
    pub fn write(
        &mut self,
        now: Timestamp,
        outcome: &Result<PassOutcome>,
        sink: &mut dyn std::io::Write,
    ) -> usize {
        let offered: Vec<String> = match outcome {
            Ok(PassOutcome::Ran {
                requotes, breaks, ..
            }) => requotes
                .iter()
                .map(|requote| format!("qip-edge-node: requote: {}", requote.describe()))
                .chain(
                    breaks
                        .iter()
                        .map(|detail| format!("qip-edge-node: reconciliation break: {detail}")),
                )
                .collect(),
            Ok(PassOutcome::Halted { .. }) => Vec::new(),
            Err(error) => vec![format!(
                "qip-edge-node: the pass failed: {}",
                error.message()
            )],
        };
        let mut written = 0;
        for line in offered {
            if let Some(line) = self.sampler.offer(now, &line) {
                let _ = writeln!(sink, "{line}");
                written += 1;
            }
        }
        written
    }
}

/// A log sink the decision thread can write to without waiting on whoever
/// reads it (OBS-9142).
///
/// [`PassLog`] bounds how many lines a window writes, not how long one write
/// takes. The loop handed it `std::io::stderr()` directly, and stderr is a
/// pipe: when the collector behind it stalls, the pipe fills, `write` blocks,
/// and the decision thread stops between its pass and its journal flush for
/// as long as the reader is gone. The sampler made that rare, not
/// impossible. Each complete line is handed to a writer thread over a bounded
/// channel with `try_send`; a full channel drops the line and counts it, and
/// nothing on the calling thread ever waits. A dropped line loses nothing the
/// log alone held: every line the pass log writes is also a series and a
/// journal entry (see [`PassLog`]).
#[derive(Debug)]
pub struct DetachedSink {
    lines: std::sync::mpsc::SyncSender<Vec<u8>>,
    pending: Vec<u8>,
    dropped: u64,
}

impl DetachedSink {
    /// Spawn the writer thread over `sink`, holding at most `capacity` lines
    /// in flight. A zero capacity is refused: it would drop every line while
    /// reading as a working log.
    pub fn spawn<W: std::io::Write + Send + 'static>(mut sink: W, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a detached log sink with no capacity drops every line; give it room for at \
                 least one",
            ));
        }
        let (lines, inbox) = std::sync::mpsc::sync_channel::<Vec<u8>>(capacity);
        std::thread::Builder::new()
            .name("qip-edge-node-log".to_string())
            .spawn(move || {
                // Ends when the sender is dropped with the loop. A failed
                // write is ignored for the reason `PassLog::write` gives.
                while let Ok(line) = inbox.recv() {
                    let _ = sink.write_all(&line);
                    let _ = sink.flush();
                }
            })
            .map_err(|error| Error::io(format!("the log writer thread did not start: {error}")))?;
        Ok(Self {
            lines,
            pending: Vec::new(),
            dropped: 0,
        })
    }

    /// Lines refused because the writer thread was behind or gone.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl std::io::Write for DetachedSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        // Only whole lines travel, so a drop never leaves half a line in the
        // log for the next one to be glued onto.
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            if self.lines.try_send(line).is_err() {
                self.dropped = self.dropped.saturating_add(1);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The node's four golden signals, with the pass as its unit of work
/// (OBS-018).
///
/// The cell's own series say what a pass decided. None said how long the
/// turn took, how many ended badly, or how much of the health surface's
/// patience one consumed. The allowance is the decision loop's period: a
/// pass that takes the whole period is a loop that can no longer keep its
/// own cadence. A closure
/// rather than a second `run_pass` signature, so the loop still calls
/// [`run_pass`] by name and this only stands around it.
#[derive(Debug)]
pub struct PassMeter {
    golden: GoldenSignals,
    metrics: Arc<Metrics>,
    allowance_millis: f64,
}

impl PassMeter {
    /// `allowance` is the decision loop's period (it was the health
    /// surface's request timeout while passes ran on the probe thread): the
    /// denominator of saturation. Zero is refused at start-up rather than
    /// divided by on every pass.
    pub fn new(metrics: Arc<Metrics>, allowance: std::time::Duration) -> Result<Self> {
        if allowance.is_zero() {
            return Err(Error::invalid(
                "configuration: the pass meter needs the health surface's request allowance, and \
                 it was zero; pass the request timeout the node serves under",
            ));
        }
        metrics.describe(
            qip_observability::metrics::names::EDGE_DECISION_LATENCY_MS,
            "milliseconds from a pass reading the feed to its decisions acted on, per pass the \
             cell ran; the reflex decision-latency objectives are read from it",
        );
        Ok(Self {
            golden: GoldenSignals::new(Arc::clone(&metrics)),
            metrics,
            allowance_millis: allowance.as_secs_f64() * 1000.0,
        })
    }

    /// Run `pass`, time it on a monotonic clock, and record it.
    ///
    /// A turn that found the cell halted is served, not failed: the loop did
    /// what a halted node does, and the halt has its own gauge. A pass that
    /// returned an error or ended in a reconciliation break is a failure of
    /// this service. Saturation is the share of the request allowance the
    /// turn consumed — at one, a probe queued behind the pass has waited as
    /// long as the node itself would wait for a request.
    pub fn measure(&self, pass: impl FnOnce() -> Result<PassOutcome>) -> Result<PassOutcome> {
        let began = std::time::Instant::now();
        let outcome = pass();
        let millis = began.elapsed().as_secs_f64() * 1000.0;
        let ended = match &outcome {
            Ok(PassOutcome::Ran { breaks, .. }) if !breaks.is_empty() => Outcome::Failed,
            Ok(_) => Outcome::Served,
            Err(_) => Outcome::Failed,
        };
        self.golden.finished(millis, ended);
        // Decision latency is a pass that *decided*: a halted turn decides
        // nothing and an errored one is the error series' business, so
        // neither is folded into the percentile the reflex objective reads.
        if let Ok(PassOutcome::Ran { .. }) = &outcome {
            self.metrics.observe_latency_ms(
                qip_observability::metrics::names::EDGE_DECISION_LATENCY_MS,
                qip_observability::metrics::Labels::new(),
                millis,
            );
        }
        // The allowance was proven positive at construction and an elapsed
        // time is finite and non-negative, so the one refusal `saturation`
        // has cannot occur here.
        let _ = self.golden.saturation(millis, self.allowance_millis);
        outcome
    }
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
    // ADR 0084 §3: the gateway is moved to the pass instant before anything
    // reads the venue, so an order the cell scheduled for release by now is
    // at the venue — or withdrawn for being late — before the feed publishes
    // the book it changed and before the cell asks what filled.
    gateway.advance_to(now)?;
    let tick = feed.publish(gateway, cell, now)?;
    // §36.3's node-crash row. While the cell is reconciling before it
    // resumes, the venue's own account of what it holds open is taken here —
    // from the gateway itself, before it is wrapped, because the account is
    // the venue's answer and not the requoter's — and handed to the cell
    // below, past the halt check. Taken only while the discipline stands:
    // outside it the cell refuses the account rather than comparing it
    // against a working set the pass is still changing.
    let account = match cell.awaiting_reconciliation() {
        Some(_) => Some(gateway.venue_account(now)?),
        None => None,
    };
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

    // The account, before anything this pass could decide. A disagreement
    // here is a reconciliation break, which halts the cell — so this runs
    // before the requoter and before `work`, both of which send.
    if let Some(account) = account {
        cell.observe_venue_account(account, now)?;
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
    if let Some(refresh) = report.edge_refresh {
        stats.edges_repriced = stats.edges_repriced.saturating_add(refresh.repriced as u64);
        stats.edges_unchanged = stats
            .edges_unchanged
            .saturating_add(refresh.unchanged as u64);
    }
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
        report: Box::new(report),
        requotes,
        breaks,
    })
}
