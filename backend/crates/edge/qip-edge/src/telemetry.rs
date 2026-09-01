//! The cell's metric seam.
//!
//! A cell knows several things no operator could see. Its policy freshness was
//! computed, formatted into a display string and journaled; its degradation
//! narrowing decided how large it may size and then evaporated; `is_halted()`
//! — the single most important boolean in the cell — reached a JSON health
//! body nothing collects as a series. A fact a process knows and never records
//! is a fact nobody can chart, alert on, or correlate against the central
//! plane that caused it.
//!
//! **The cell records into a registry it is given.** [`CellMetrics`] holds an
//! `Arc<Metrics>` handed to it by the composition root, never one it reached
//! for. That keeps the boundary rule intact in both directions: `qip-edge`
//! depends on `qip-observability`, which is a library holding an in-memory
//! `BTreeMap` behind a mutex and performs no I/O, and the decision about
//! *where* the numbers are served from stays in `qip-edge-node` where every
//! other deployment decision lives.
//!
//! **Nothing here can block or fail the hot path.** Every method returns `()`.
//! The only synchronisation is `Metrics`' own mutex, whose critical section is
//! a `BTreeMap` lookup and an integer add, and which recovers from poisoning
//! rather than panicking. There is no I/O, no allocation of unbounded size, no
//! error to propagate and nothing a caller must check — so a recording site
//! cannot become a reason an order was not sent.
//!
//! **Cardinality is bounded by construction.** `cell` and `region` are fixed
//! for the life of the process. `venue` is bounded by the cell's configured
//! venue set. `gate` is bounded by the string literals `Cell::refuse` is
//! called with. `source` and `kind` are enums, and `capability` is the three
//! policy-fed variants of one. Nothing here is labelled by instrument,
//! strategy or order id, and that is deliberate: a series per order id is a
//! memory leak wearing a dashboard. Each `with(...)` call below says what
//! bounds its own label.

use qip_contracts::degradation::{Capability, DegradationState, Freshness};
use qip_contracts::signal::SignalKind;
use qip_contracts::venue::VenueId;
use qip_observability::metrics::{Histogram, Labels, Metrics, names};
use std::sync::Arc;

/// Buckets for the netting ratio.
///
/// It is a ratio of gross intent to net order volume, so it starts at exactly
/// `1.0` — a cell whose strategies never agree or offset — and rises without
/// bound as they do. The lower buckets are tight because the interesting
/// question is whether a set that claims diversity is actually netting at all,
/// and `1.0` against `1.1` is the whole answer to it.
fn netting_ratio_buckets() -> Histogram {
    Histogram::with_bounds(vec![1.0, 1.05, 1.1, 1.25, 1.5, 2.0, 3.0, 5.0, 10.0, 50.0])
}

/// Where a cell's facts go.
///
/// The default discards into a registry nobody reads, so a `Cell` assembled
/// without telemetry — every unit test in the tree, and any caller that has
/// not been given a handle — records into somewhere harmless rather than
/// forcing an `Option` check at two dozen call sites. An `Option` would put a
/// branch on the hot path whose only purpose is to decide whether to do
/// nothing.
#[derive(Debug)]
pub struct CellMetrics {
    metrics: Arc<Metrics>,
    /// `cell` and `region`, resolved once. Cloned per recording rather than
    /// rebuilt, because the label set is the series identity and building it
    /// twice from different code would be two series for one fact.
    base: Labels,
}

impl CellMetrics {
    /// Record into `metrics`, tagging everything with this cell and region.
    ///
    /// Every metric name is described here. Describing them where the recorder
    /// is assembled is the one place guaranteed to run exactly once, and
    /// `Metrics::describe` keeps the text by name so a description registered
    /// before the first observation is not lost.
    pub fn new(metrics: Arc<Metrics>, cell: &str, region: &str) -> Self {
        let mut base = Labels::new();
        base.insert("cell".to_string(), cell.to_string());
        base.insert("region".to_string(), region.to_string());
        let recorder = Self { metrics, base };
        recorder.describe();
        recorder
    }

    /// A recorder whose numbers nobody reads.
    pub fn silent() -> Self {
        Self {
            metrics: Arc::new(Metrics::new("qip-edge")),
            base: Labels::new(),
        }
    }

    fn describe(&self) {
        let m = &self.metrics;
        m.describe(
            names::EDGE_WORK_PASSES,
            "passes of the cell's decide-and-act loop",
        );
        m.describe(
            names::EDGE_HALTED,
            "whether the cell is stopped, by which halt is in force",
        );
        m.describe(
            names::EDGE_REFUSALS,
            "gates that refused, by gate — why the cell was quiet",
        );
        m.describe(names::EDGE_SIGNALS_RAISED, "signals raised, by kind");
        m.describe(
            names::EDGE_CAPABILITY_FRESHNESS,
            "capability freshness: 0 fresh, 1 stale, 2 unavailable",
        );
        m.describe(
            names::EDGE_SIZING_MULTIPLIER,
            "the degradation table's sizing multiplier in force",
        );
        m.describe(
            names::EDGE_POLICY_SEQUENCE,
            "the sequence of the policy payload this cell has applied",
        );
        m.describe(
            names::EDGE_NETTING_RATIO,
            "gross intent over net order volume, per pass",
        );
        m.describe(
            names::EDGE_ORDERS_PLACED,
            "orders sent to a venue, by venue",
        );
        m.describe(
            names::EDGE_INTENTS_CANCELLED,
            "net intents that cancelled to zero and never reached a venue",
        );
        m.describe(
            names::EDGE_INTERNAL_CROSSES,
            "portions crossed between the platform's own strategies, by venue",
        );
        m.describe(
            names::EDGE_RECONCILIATION_BREAKS,
            "disagreements between the cell's fills and the venue's own account",
        );
    }

    /// The registry this recorder writes to, for a composition root that has
    /// to serve the same one it handed over.
    pub fn registry(&self) -> &Arc<Metrics> {
        &self.metrics
    }

    fn with(&self, key: &str, value: &str) -> Labels {
        let mut labels = self.base.clone();
        labels.insert(key.to_string(), value.to_string());
        labels
    }

    /// One pass of `Cell::work` began.
    ///
    /// Recorded unconditionally at the top of the pass, including the halted
    /// one that returns immediately. Without it every other edge series is
    /// unreadable: a refusal count of zero means "nothing was refused" and
    /// "the cell never ran" identically, and those are the two most different
    /// states a cell has.
    pub fn work_pass(&self) {
        self.metrics
            .count(names::EDGE_WORK_PASSES, self.base.clone());
    }

    /// The halt state, by source.
    ///
    /// Both sources are written on every call, so a release shows as the
    /// series falling to zero rather than as a series that stops being
    /// updated. A gauge that goes stale at `1` and a cell that is still halted
    /// look identical on a chart.
    pub fn halt(&self, kill_switch: bool, policy: bool) {
        // `source` takes exactly the two literals below — one per halt
        // discipline the cell has — so this is two series per cell.
        self.metrics.gauge(
            names::EDGE_HALTED,
            self.with("source", "kill_switch"),
            f64::from(u8::from(kill_switch)),
        );
        self.metrics.gauge(
            names::EDGE_HALTED,
            self.with("source", "policy"),
            f64::from(u8::from(policy)),
        );
    }

    /// A gate refused.
    ///
    /// `gate` is a string literal at every call site — `Cell::refuse` is
    /// never handed a formatted string, and the reason, which is formatted,
    /// goes to the journal and not to a label. The series count is the number
    /// of distinct literals in `cell.rs`, which is a property of the source
    /// and not of the market.
    pub fn refusal(&self, gate: &str) {
        self.metrics
            .count(names::EDGE_REFUSALS, self.with("gate", gate));
    }

    /// A strategy raised a signal.
    ///
    /// Keyed on the signal's kind, a four-variant enum, and deliberately not
    /// on the strategy or the instrument that raised it: both are unbounded
    /// over the life of a cell, and the question this series answers — is the
    /// cell seeing anything to act on — does not need either.
    pub fn signal(&self, kind: SignalKind) {
        self.metrics
            .count(names::EDGE_SIGNALS_RAISED, self.with("kind", kind.as_str()));
    }

    /// The degradation narrowing in force, at the instant it was derived.
    ///
    /// Freshness is a function of *now*, so it becomes known once per pass and
    /// not when a payload was applied. Recording it at the seam where the cell
    /// consults it is what makes the series say what the cell actually sized
    /// against, rather than what it would have sized against at some earlier
    /// instant.
    ///
    /// Only the capabilities a policy payload feeds are published: the causal
    /// graph, episodic memory and the belief state — the three
    /// `PolicyItem::capability` maps, and the two that set the sizing
    /// multiplier. `Ingestion` is deliberately absent because
    /// `Cell::narrowing` never observes it: book staleness is refused per book
    /// at the routing seam and counted under the `stale_book` gate, and the
    /// table's `unavailable` for it is `nothing_known()`'s default, not a
    /// measurement. `CounterfactualScoring` never ships and §6.2 gives its
    /// loss no trading impact. Publishing either would put a permanent `2` on
    /// a chart whose whole purpose is a `max`, and an operator would learn to
    /// ignore the one series that pages on a real narrowing.
    ///
    /// Each of the three is written on every pass, so a capability that goes
    /// from stale back to fresh is a series falling to zero rather than one
    /// that stopped being updated.
    pub fn narrowing(&self, state: &DegradationState) {
        for capability in [
            Capability::CausalGraph,
            Capability::EpisodicMemory,
            Capability::BeliefState,
        ] {
            let severity = match state.freshness(capability) {
                Freshness::Fresh => 0.0,
                Freshness::Stale => 1.0,
                Freshness::Unavailable => 2.0,
            };
            // `capability` is one of the three variants named above; the
            // series count is three per cell, whatever the payload carries.
            self.metrics.gauge(
                names::EDGE_CAPABILITY_FRESHNESS,
                self.with("capability", capability.as_str()),
                severity,
            );
        }
        // The multiplier is `Decimal` because it scales money. This is the
        // crossing point to `f64`, and it is a reporting one: the number is
        // exported for a human to look at and is never multiplied back into a
        // size. The size the cell actually uses stays `Decimal` throughout.
        self.metrics.gauge(
            names::EDGE_SIZING_MULTIPLIER,
            self.base.clone(),
            state.sizing_multiplier().to_f64(),
        );
    }

    /// The sequence of the policy payload the cell has applied.
    ///
    /// The central plane knows what it published; this is what arrived and was
    /// accepted. The two being charted side by side is the only way a stuck
    /// downlink is visible as anything other than a cell that has quietly
    /// stopped changing its mind.
    pub fn policy_applied(&self, sequence: u64) {
        // A sequence is a counter's worth of range in a gauge on purpose: it
        // is a position, not a rate, and `increase()` over it would be
        // meaningless. Above 2^53 the `f64` stops being exact, which is some
        // hundreds of thousands of years of policy at any rate a signing
        // central plane can produce.
        self.metrics.gauge(
            names::EDGE_POLICY_SEQUENCE,
            self.base.clone(),
            sequence as f64,
        );
    }

    /// Gross intent over net order volume for one pass (§27).
    pub fn netting_ratio(&self, ratio: f64) {
        self.metrics.observe_with(
            names::EDGE_NETTING_RATIO,
            self.base.clone(),
            ratio,
            netting_ratio_buckets,
        );
    }

    /// An order reached a venue.
    ///
    /// `venue` is one of `CellConfig::venues` — `Cell::venue_for` selects
    /// from that list and nothing else — so the series count is the size of a
    /// list fixed at deployment. The order id and the instrument are not
    /// labels: an order id is a new series per order, which is a registry
    /// that grows without bound for as long as the cell trades.
    pub fn order_placed(&self, venue: &VenueId) {
        self.metrics.count(
            names::EDGE_ORDERS_PLACED,
            self.with("venue", venue.as_str()),
        );
    }

    /// A net that cancelled to zero. An outcome, not an absence — which is
    /// exactly why it is counted rather than inferred from an order that did
    /// not appear.
    pub fn intent_cancelled(&self) {
        self.metrics
            .count(names::EDGE_INTENTS_CANCELLED, self.base.clone());
    }

    /// A cross was booked between two of the platform's own strategies.
    ///
    /// Keyed on the venue whose mid priced it, bounded exactly as
    /// [`Self::order_placed`] is: the venue came from the net intent, which
    /// took it from the configured list. The strategies on each side are in
    /// the journal, not on a label.
    pub fn internal_cross(&self, venue: &VenueId) {
        self.metrics.count(
            names::EDGE_INTERNAL_CROSSES,
            self.with("venue", venue.as_str()),
        );
    }

    pub fn reconciliation_break(&self) {
        self.metrics
            .count(names::EDGE_RECONCILIATION_BREAKS, self.base.clone());
    }
}

impl Default for CellMetrics {
    fn default() -> Self {
        Self::silent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_observability::metrics::labels;

    fn recorder() -> CellMetrics {
        CellMetrics::new(Arc::new(Metrics::new("qip-edge")), "cell-a", "eu-west")
    }

    #[test]
    fn a_released_halt_reads_as_zero_rather_than_as_a_series_that_stopped() {
        // The failure this prevents: writing the gauge only while halted. The
        // series would sit at 1 forever after the first halt and an operator
        // would page on a cell that resumed hours ago.
        let recorder = recorder();
        recorder.halt(true, false);
        let halted = recorder.registry().snapshot();
        assert_eq!(
            halted.gauge(
                names::EDGE_HALTED,
                &labels([
                    ("cell", "cell-a"),
                    ("region", "eu-west"),
                    ("source", "kill_switch")
                ])
            ),
            Some(1.0),
            "the premise failed: the kill-switch gauge was never set to 1"
        );

        recorder.halt(false, false);
        let released = recorder.registry().snapshot();
        assert_eq!(
            released.gauge(
                names::EDGE_HALTED,
                &labels([
                    ("cell", "cell-a"),
                    ("region", "eu-west"),
                    ("source", "kill_switch")
                ])
            ),
            Some(0.0),
            "a released kill switch left the gauge asserting the cell is still halted"
        );
    }

    #[test]
    fn only_the_capabilities_a_payload_feeds_are_published_and_each_reports_even_when_absent() {
        // Two properties, and both matter. The three policy-fed capabilities
        // must appear even when nothing was observed: absence is the worst
        // case in the §6.2 table, and a missing series reads as good news on
        // every dashboard ever built. And the two the cell never measures
        // must *not* appear: `nothing_known()` reports ingestion as
        // unavailable by default, not by observation, and a permanent `2` on
        // a chart whose purpose is a `max` teaches an operator to ignore it.
        let recorder = recorder();
        recorder.narrowing(&DegradationState::nothing_known());
        let snapshot = recorder.registry().snapshot();
        for capability in [
            Capability::CausalGraph,
            Capability::EpisodicMemory,
            Capability::BeliefState,
        ] {
            assert_eq!(
                snapshot.gauge(
                    names::EDGE_CAPABILITY_FRESHNESS,
                    &labels([
                        ("capability", capability.as_str()),
                        ("cell", "cell-a"),
                        ("region", "eu-west")
                    ])
                ),
                Some(2.0),
                "{} did not report as unavailable with nothing known",
                capability.as_str()
            );
        }
        for capability in [Capability::Ingestion, Capability::CounterfactualScoring] {
            assert_eq!(
                snapshot.gauge(
                    names::EDGE_CAPABILITY_FRESHNESS,
                    &labels([
                        ("capability", capability.as_str()),
                        ("cell", "cell-a"),
                        ("region", "eu-west")
                    ])
                ),
                None,
                "{} was published although the cell never measures it",
                capability.as_str()
            );
        }
    }

    #[test]
    fn the_netting_ratio_lands_in_a_bucket_that_separates_no_netting_from_some() {
        // A ratio of 1.0 is a strategy set that never offsets and 1.2 is one
        // that does. Buckets that put both in the same bin would answer §27's
        // question with "yes" whatever the truth was.
        let recorder = recorder();
        recorder.netting_ratio(1.0);
        recorder.netting_ratio(1.2);
        let snapshot = recorder.registry().snapshot();
        let histogram = snapshot
            .histogram(
                names::EDGE_NETTING_RATIO,
                &labels([("cell", "cell-a"), ("region", "eu-west")]),
            )
            .expect("the netting ratio histogram was not recorded at all");
        assert_eq!(histogram.count, 2, "both observations should be counted");
        assert_eq!(
            histogram.counts[0], 1,
            "1.0 did not land in the first bucket, so no-netting is indistinguishable from netting"
        );
    }
}
