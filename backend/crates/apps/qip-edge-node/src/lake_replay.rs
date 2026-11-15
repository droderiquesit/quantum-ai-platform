//! Replaying retained tick-lake history through a fresh cell (R12-TICK-9006).
//!
//! `run_replay_pass` proved in `tests/lake_replay.rs` that a cell fed the
//! retained history decides as the original did, but nothing a person could
//! run called it, and the lake it read in that test was a memory store that
//! died with the test. This module is the seam a shipped binary calls: it
//! reads a *durable* lake (any [`BlobStore`], the file-backed one in
//! practice), refuses what the history cannot honestly be replayed as, and
//! drives every retained pass through the same decide-act-reconcile step the
//! live pass runs.
//!
//! The report carries a digest over every pass's decisions and the journal's
//! hash-chained tail, so "yields the same decisions" is a comparison of two
//! strings a person can run, not a claim the program makes about itself.

use crate::feed::ReplayFeed;
use crate::gateway::SimulatedGateway;
use crate::pass::{PassOutcome, PassStats, run_replay_pass};
use qip_contracts::message::MarketMessage;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::time::Timestamp;
use qip_edge::cell::Cell;
use qip_storage::BlobStore;
use qip_storage::lake::{Lake, Manifest};
use qip_storage::lake_history::HistoryQuery;
use std::collections::{BTreeMap, BTreeSet};

/// The manifest name a replay pins its read under. Pinning records which
/// segments the replay read, so the same replay can be shown to have read
/// the same bytes.
pub const REPLAY_MANIFEST: &str = "edge-node-replay";

/// The most segments and ticks one replay holds. Bounded, as the lake's own
/// reader requires: a replay of unbounded history is a memory fault with a
/// long fuse.
pub const MAX_SEGMENTS: usize = 10_000;
/// See [`MAX_SEGMENTS`].
pub const MAX_TICKS: usize = 1_000_000;

/// Retained ticks grouped by the pass that received them (its capture
/// time), in pass order.
#[derive(Debug)]
pub struct RetainedHistory {
    passes: BTreeMap<Timestamp, Vec<MarketMessage>>,
    ticks: usize,
}

impl RetainedHistory {
    /// Read every tick `datasets` entitle the replay to, from `store`.
    ///
    /// Refused, not defaulted, when `datasets` is empty (an empty grant reads
    /// nothing and a replay of nothing is not a reproduction), when any
    /// segment is withheld for want of a grant, when a retained line does not
    /// parse, and when the lake holds no tick at all — an empty lake is what
    /// a memory-backed node leaves behind, and replaying it would report
    /// "reproduced" over zero decisions.
    pub fn read(store: &dyn BlobStore, datasets: &BTreeSet<String>) -> Result<Self> {
        if datasets.is_empty() {
            return Err(Error::invalid(
                "a replay names the datasets it is entitled to read; none were named, and an \
                 empty grant reads nothing",
            ));
        }
        let lake = Lake::new(store);
        let query = HistoryQuery {
            from_date: "0000-01-01".to_string(),
            to_date: "9999-12-31".to_string(),
            venues: BTreeSet::new(),
            instruments: BTreeSet::new(),
            granted: datasets.clone(),
            max_segments: MAX_SEGMENTS,
            max_ticks: MAX_TICKS,
        };
        // Pinned under a name taken from the segments themselves, because
        // the lake describes an input set once and refuses a second manifest
        // of one name. A fixed name made the second replay of the same lake
        // fail; a name derived from the listing means the same segments
        // reuse their manifest and a lake that grew gets a new one.
        let listing = lake.list_market_history(&query)?;
        let name = format!(
            "{REPLAY_MANIFEST}-{}",
            sha256_hex(format!("{:?}", listing.segments).as_bytes())
        );
        match lake.write_manifest(&Manifest {
            name: name.clone(),
            segments: listing.segments,
        }) {
            Ok(()) => {}
            Err(error) if error.message().contains("already exists") => {}
            Err(error) => return Err(error),
        }
        let history = lake.read_market_history(&name, datasets, MAX_TICKS)?;
        if !history.withheld.is_empty() {
            return Err(Error::denied(format!(
                "{} retained segment(s) are not covered by the named datasets; replaying the rest \
                 would present a partial market as the whole one",
                history.withheld.len()
            )));
        }
        if history.unparsed != 0 {
            return Err(Error::invalid(format!(
                "{} retained line(s) decode as no tick shape; a replay does not guess at them",
                history.unparsed
            )));
        }
        let ticks = history.ticks.len();
        if ticks == 0 {
            return Err(Error::invalid(
                "the lake retains no ticks for the named datasets; there is no history to replay \
                 (a node whose lake is a memory store retains nothing across a restart)",
            ));
        }
        let mut passes: BTreeMap<Timestamp, Vec<MarketMessage>> = BTreeMap::new();
        for tick in history.ticks {
            passes
                .entry(tick.message.capture_time)
                .or_default()
                .push(tick.message);
        }
        Ok(Self { passes, ticks })
    }

    /// How many ticks the history holds.
    pub fn ticks(&self) -> usize {
        self.ticks
    }

    /// How many passes the history spans.
    pub fn passes(&self) -> usize {
        self.passes.len()
    }

    /// The instants of the retained passes, in order.
    pub fn instants(&self) -> impl Iterator<Item = Timestamp> + '_ {
        self.passes.keys().copied()
    }
}

/// What a replay decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LakeReplayReport {
    /// Passes replayed.
    pub passes: usize,
    /// Ticks presented to the cell.
    pub ticks: usize,
    /// Orders the cell decided to place across the replay.
    pub orders: usize,
    /// SHA-256 over every pass's rendered decision and the journal's chain
    /// tail. Equal digests mean equal decisions, entry for entry.
    pub digest: String,
}

impl LakeReplayReport {
    /// One line a person can compare against another run's.
    pub fn describe(&self) -> String {
        format!(
            "replayed {} pass(es), {} tick(s), {} order(s) decided; decision digest {}",
            self.passes, self.ticks, self.orders, self.digest
        )
    }
}

/// Render a pass whole, so the digest moves when any order, fill, refusal,
/// signal, cancellation, requote or break does. The feed's own tally is
/// left out: the original counts the instruments the venue listed and the
/// replay counts the instruments that ticked, and neither is a decision.
fn decided(outcome: &PassOutcome) -> String {
    match outcome {
        PassOutcome::Ran {
            report,
            requotes,
            breaks,
            ..
        } => format!("ran {report:?} requotes {requotes:?} breaks {breaks:?}"),
        PassOutcome::Halted { fills, .. } => format!("halted fills {fills:?}"),
    }
}

/// Replay `history` through `cell`, one pass per retained instant.
///
/// `exogenous` runs before each pass, against the venue the cell sends to:
/// whatever the original session did to its simulated venue that the lake
/// does not hold (the lake holds the market view, not the fill model). With
/// none, the replay is the lake's market view against a venue that only
/// answers what the cell sends.
///
/// The gateway is the simulated one by type. A replay sends to a venue that
/// matches orders, and that venue is a simulator, never a live one (ADR 0003).
pub fn replay_lake(
    cell: &mut Cell,
    gateway: &mut SimulatedGateway,
    feed: &mut ReplayFeed,
    history: &RetainedHistory,
    mut exogenous: impl FnMut(&mut SimulatedGateway, Timestamp) -> Result<()>,
) -> Result<LakeReplayReport> {
    let mut stats = PassStats::default();
    let mut rendered = String::new();
    let mut orders = 0usize;
    for (now, ticks) in &history.passes {
        exogenous(gateway, *now)?;
        let outcome = run_replay_pass(cell, gateway, feed, ticks, None, &mut stats, *now)?;
        if let PassOutcome::Ran { report, .. } = &outcome {
            orders = orders.saturating_add(report.orders.len());
        }
        rendered.push_str(&decided(&outcome));
        rendered.push('\n');
    }
    if let Some(last) = cell.journal().entries().last() {
        rendered.push_str(&last.digest);
    }
    Ok(LakeReplayReport {
        passes: history.passes(),
        ticks: history.ticks(),
        orders,
        digest: sha256_hex(rendered.as_bytes()),
    })
}
