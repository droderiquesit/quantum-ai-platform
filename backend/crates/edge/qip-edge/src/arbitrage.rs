//! The arbitrage desk: the scanner, its whitelist and its capital, as one
//! thing the cell can be handed.
//!
//! Blueprint §30 makes arbitrage one family of ten with the cleanest
//! correctness properties, and §27.2 makes its legs the one thing netting
//! must never touch. Both were true of the types before this module existed
//! — `qip-arbitrage` produced `CycleLeg`s that cannot be built nettable, and
//! `net` gave each one a group of its own — and false of the platform,
//! because nothing deployed ever called the scanner. The cell's work loop ran
//! compiled strategy programs and consulted no graph; a placement audit found
//! the scanner constructed by no composition root. This module is what the
//! cell holds so that it can.
//!
//! # What the desk consumes
//!
//! The cell's own books, and nothing else. [`ArbitrageDesk::refresh`]
//! re-quotes every trade edge from the touch the cell's
//! [`crate::seam::CellLiquidity`] serves — through the
//! [`LiquiditySource`] seam `qip-arbitrage` defined for exactly this — and
//! the scan then walks the same books at size. A book that is stale or
//! empty re-quotes its edge at zero, which the search refuses, so a gap on
//! one feed takes that edge out of every cycle for as long as the gap lasts
//! and puts it back the moment the book resynchronises. No remote call, no
//! ambient clock: `now` is a parameter, and two passes over the same books
//! at the same instant find the same cycles in the same order.
//!
//! # What bounds it
//!
//! Three explicit numbers, every one refused at zero. The search's own
//! `max_candidates` and `max_cycle_edges` bound how many cycles one scan can
//! propose and how long each may be; [`ArbitrageDesk::new`] refuses a search
//! setting with either at zero, because a scanner that proposes nothing is a
//! desk that reads as quiet rather than as misconfigured. The desk's own cap,
//! `max_cycles_per_pass`, bounds how many surviving opportunities the cell
//! will commit in one pass, and every opportunity past it is refused and
//! counted rather than dropped. Per-pass allocation is therefore bounded by
//! `max_candidates × max_cycle_edges` legs plus the cap's worth of cycles,
//! and nothing the desk holds grows from one pass to the next: the graph is
//! re-quoted in place, and the scan's report is dropped with the pass.
//!
//! # What the desk does not do
//!
//! It does not send. Every leg it produces goes through the cell's own
//! feasibility gate, the desk's capital envelope, and the cell's one order
//! path, in that order — the chain §33 names, with netting skipped because
//! §27.2 forbids it for a leg. And it does not coordinate: the cell's
//! [`crate::cell::Placer`] can place, and can withdraw a resting order only
//! where the gateway says it has a cancel path (`Placer::can_cancel`), but
//! it cannot send a compensating order — so a cycle whose later leg the
//! venue refuses after an earlier one filled cannot be unwound from here.
//! This sentence said "can place and cannot cancel" until 2026-09-19, which
//! stopped being true when `Cell::withdraw_expired` and the resting leg of
//! §32.1's passive-first arrived. What the cell does instead is stated at
//! `Cell::place_cycle`.

use crate::envelope::VerifiedEnvelope;
use qip_arbitrage::graph::EdgeKind;
use qip_arbitrage::{ArbitrageGraph, LiquiditySource, OpportunityScanner, ScanReport, SizePolicy};
use qip_contracts::capital::Utilisation;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};

/// The scanner, its whitelist of conversions, the sizes it prices at, and
/// the capital its legs are admitted against.
#[derive(Debug)]
pub struct ArbitrageDesk {
    strategy: StrategyId,
    scanner: OpportunityScanner,
    graph: ArbitrageGraph,
    sizes: SizePolicy,
    envelope: VerifiedEnvelope,
    utilisation: Utilisation,
    max_cycles_per_pass: usize,
    leg_validity: Duration,
    /// Per edge index, the `(observed_at, observations)` pair the edge was
    /// last re-quoted from, or `None` for an edge never re-quoted. The
    /// affected-edge filter of blueprint §30.1: a book whose pair has not
    /// moved has not moved, so its edge keeps the rate it holds.
    last_seen: Vec<Option<(Timestamp, u32)>>,
}

/// What one [`ArbitrageDesk::refresh`] did to the graph (§30.1, "recompute
/// weight for affected edges only").
///
/// Two counts rather than one, for the reason every idle reading in this
/// crate is charted: a pass that re-priced nothing because no book moved and
/// a pass that re-priced nothing because the desk holds no trade edge read
/// identically on `repriced` alone. `unchanged` is the denominator that
/// tells them apart, and the two sum to the graph's trade-edge count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EdgeRefresh {
    /// Trade edges whose book had moved since the desk last read it, and
    /// which were re-quoted from the touch.
    pub repriced: usize,
    /// Trade edges whose book had not moved, left holding the rate they had.
    pub unchanged: usize,
}

impl EdgeRefresh {
    /// Every trade edge the refresh looked at.
    pub const fn total(self) -> usize {
        self.repriced + self.unchanged
    }
}

impl ArbitrageDesk {
    /// Assemble a desk, refusing what could not run.
    ///
    /// * The envelope must name `strategy`: the desk's legs are admitted
    ///   against it and attributed to that strategy, and a grant for another
    ///   strategy spent here would be capital nobody approved for this use.
    /// * The graph must hold at least one trade edge and no synthetic edge.
    ///   The cell re-quotes trade edges from its books and has no book for a
    ///   synthetic; pricing one from the template's stale rate would put a
    ///   number nobody observed into every cycle through it.
    /// * The search must be able to propose something, and the cap must
    ///   admit something, or the desk is quiet by construction and reads as
    ///   quiet by market.
    /// * A leg validity of zero would build legs that expire at the instant
    ///   they are made, which `Opportunity::cycle_legs` refuses one at a time
    ///   and this refuses once.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        strategy: StrategyId,
        scanner: OpportunityScanner,
        graph: ArbitrageGraph,
        sizes: SizePolicy,
        envelope: VerifiedEnvelope,
        max_cycles_per_pass: usize,
        leg_validity: Duration,
    ) -> Result<Self> {
        if envelope.strategy() != &strategy {
            return Err(Error::denied(format!(
                "an envelope for strategy {} cannot fund the arbitrage desk {}",
                envelope.strategy().as_str(),
                strategy.as_str()
            )));
        }
        let trades = graph
            .edges()
            .iter()
            .filter(|edge| matches!(edge.kind, EdgeKind::Trade { .. }))
            .count();
        if trades == 0 {
            return Err(Error::invalid(
                "the arbitrage graph holds no trade edge; a desk with nothing to re-quote from a \
                 book would scan a template forever",
            ));
        }
        if let Some(synthetic) = graph
            .edges()
            .iter()
            .find(|edge| matches!(edge.kind, EdgeKind::Synthetic { .. }))
        {
            return Err(Error::invalid(format!(
                "conversion {} is synthetic and the cell holds no book to re-quote it from; a \
                 cycle through it would be priced on the template's rate rather than the market",
                synthetic.label()
            )));
        }
        let search = scanner.search_settings();
        if search.max_candidates == 0 || search.max_cycle_edges == 0 {
            return Err(Error::invalid(
                "the search proposes nothing: max_candidates and max_cycle_edges must both be \
                 positive, or the desk is quiet by construction",
            ));
        }
        if max_cycles_per_pass == 0 {
            return Err(Error::invalid(
                "a cap of zero cycles per pass commits nothing; refuse the cap rather than build \
                 a desk that refuses every opportunity it finds",
            ));
        }
        if leg_validity.is_zero() || leg_validity.as_nanos() < 0 {
            return Err(Error::invalid(format!(
                "a leg validity of {} nanoseconds builds legs that expire as they are made",
                leg_validity.as_nanos()
            )));
        }
        let edge_count = graph.edge_count();
        Ok(Self {
            strategy,
            scanner,
            graph,
            sizes,
            envelope,
            utilisation: Utilisation::default(),
            max_cycles_per_pass,
            leg_validity,
            last_seen: vec![None; edge_count],
        })
    }

    pub fn strategy(&self) -> &StrategyId {
        &self.strategy
    }

    pub fn graph(&self) -> &ArbitrageGraph {
        &self.graph
    }

    pub fn envelope(&self) -> &VerifiedEnvelope {
        &self.envelope
    }

    pub fn utilisation(&self) -> &Utilisation {
        &self.utilisation
    }

    pub(crate) fn utilisation_mut(&mut self) -> &mut Utilisation {
        &mut self.utilisation
    }

    pub(crate) fn replace_envelope(&mut self, envelope: VerifiedEnvelope) {
        self.envelope = envelope;
    }

    pub const fn max_cycles_per_pass(&self) -> usize {
        self.max_cycles_per_pass
    }

    pub const fn leg_validity(&self) -> Duration {
        self.leg_validity
    }

    /// Re-quote every trade edge whose book has moved since the desk last
    /// read it, from the touch the source serves now (§30.1's edge update).
    ///
    /// Consuming offers converts `from` into `to` at one over the ask;
    /// consuming bids converts at the bid. A side with no touch — the book is
    /// stale, unusable, or empty on that side — re-quotes at zero, which the
    /// search treats as no edge. The evidence behind the quote travels with
    /// it: `observed_at` is when the book last changed and `observations` how
    /// many messages built it, and both feed the net-edge calculator's
    /// uncertainty haircut, so a book the cell has barely seen is haircut as
    /// such rather than trusted as the template's `observations` claimed.
    ///
    /// # Which edges are affected, and why that pair is the test
    ///
    /// An edge is re-quoted only when the `(observed_at, observations)` pair
    /// the source reports for its book differs from the pair the desk last
    /// re-quoted it from. Both come through the source's own usability rule,
    /// so a book that goes stale or whose venue halts reports no pair at all
    /// — which differs from any pair a live book reported — and its edge is
    /// re-quoted to zero on the next pass rather than kept at the last
    /// price the book showed before it was known to be wrong. Every message
    /// a book applies moves at least one of the two, so a book that has
    /// moved is never mistaken for one that has not. The first refresh after
    /// assembly, and after [`Self::forget_books`], re-quotes every edge,
    /// because the template's rate is a placeholder and not a quote.
    ///
    /// What this does **not** do, said plainly: the scan that follows still
    /// walks the whole graph. The candidate index and the incremental
    /// rescan the blueprint pairs this with live in `qip-arbitrage`'s
    /// search, and until they exist the affected set bounds only the
    /// re-quoting, not the search. The count is returned rather than
    /// dropped so the cell can chart it, and so that the incremental rescan
    /// has the trigger it needs the day it is built.
    ///
    /// Only trade edges are touched. A transfer's rate is one by definition,
    /// and [`Self::new`] refused any synthetic.
    pub fn refresh(&mut self, source: &dyn LiquiditySource) -> Result<EdgeRefresh> {
        let mut report = EdgeRefresh::default();
        for index in 0..self.graph.edge_count() {
            let Some(edge) = self.graph.edge(index) else {
                continue;
            };
            let EdgeKind::Trade { market, side } = &edge.kind else {
                continue;
            };
            let venue = &edge.from.venue;
            let observed_at = source.as_of(venue, market).unwrap_or(Timestamp::EPOCH);
            let observations = source.observations(venue, market);
            let seen = Some((observed_at, observations));
            if self.last_seen.get(index).copied().flatten() == seen {
                report.unchanged += 1;
                continue;
            }
            let rate = source
                .touch(venue, market, *side)
                .and_then(|(price, _)| match side {
                    BookSide::Ask => Decimal::ONE.checked_div(price),
                    BookSide::Bid => Some(price),
                })
                .unwrap_or(Decimal::ZERO);
            self.graph
                .refresh_trade(index, rate, observed_at, observations)?;
            // Recorded only once the graph holds the quote, so a refused
            // refresh leaves the edge marked unread and the next pass tries
            // it again rather than skipping an edge that never took its
            // price.
            if let Some(slot) = self.last_seen.get_mut(index) {
                *slot = seen;
            }
            report.repriced += 1;
        }
        Ok(report)
    }

    /// Forget which books the desk has read, so the next [`Self::refresh`]
    /// re-quotes every trade edge.
    ///
    /// Called by the cell when a book is replaced wholesale rather than
    /// updated by a message — [`crate::cell::Cell::track`] — because a
    /// replacement state can carry the same `(observed_at, observations)`
    /// pair as the one it displaced while showing a different touch, and the
    /// affected-edge filter would then keep a rate no book holds.
    pub fn forget_books(&mut self) {
        self.last_seen = vec![None; self.graph.edge_count()];
    }

    /// Run the scanner over the graph as last refreshed, against the source.
    ///
    /// Exposed so a caller holding the same books can reproduce what the cell
    /// found — which is what makes the cell's cycle decisions checkable
    /// against the scanner rather than merely asserted by the cell.
    pub fn scan(&self, source: &dyn LiquiditySource, now: Timestamp) -> ScanReport {
        self.scanner.scan(&self.graph, source, &self.sizes, now)
    }
}
