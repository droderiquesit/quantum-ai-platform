//! One objective for each of the eight critical paths, with its source series
//! and the rule that turns that series into good and total counts.
//!
//! Until 2026-10-11 each path named a series and nothing read it: two of the
//! eight names (`qip_model_package_age_seconds`, `qip_quantum_jobs_total`)
//! were recorded by nothing at all, and no code evaluated any objective from
//! a snapshot, so a critical path could miss its target and no number moved.
//! [`evaluate`] now reads a snapshot through each path's [`SourceRule`] and
//! [`record`] writes the result as fixed-label gauges, labelled only by the
//! objective names below.

use crate::metrics::{Metrics, Snapshot, labels, names};
use crate::slo::{Slo, SloStatus, SloWindow};

/// How a path's series becomes good and total counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceRule {
    /// A counter labelled `outcome`; good is the named outcome's count and
    /// total is every outcome's.
    CounterOutcome { good: &'static str },
    /// A latency histogram; good is the observations in buckets whose upper
    /// bound is at or below the objective's threshold. Conservative by
    /// construction: an observation in a bucket straddling the threshold is
    /// counted bad, so bucketing can make the path look worse and never
    /// better.
    LatencyUnder,
    /// The series is a gauge or an unlabelled counter that carries no
    /// good/total split. Evaluates unmeasured, and says so, rather than
    /// inventing a denominator.
    NotCountable,
}

/// An objective for one critical path, with the series that feeds it. A path
/// whose series nothing records evaluates with zero observations and so reads
/// as unmeasured ([`SloStatus::is_observed`]), never as met.
#[derive(Clone, Debug, PartialEq)]
pub struct CriticalPath {
    pub slo: Slo,
    /// The series whose good/total counts feed this objective.
    pub source_series: &'static str,
    pub rule: SourceRule,
}

impl CriticalPath {
    pub fn evaluate(&self, good: u64, total: u64) -> SloStatus {
        self.slo.evaluate(good, total)
    }

    /// Good and total counts read from `snapshot` under this path's rule.
    pub fn counts(&self, snapshot: &Snapshot) -> (u64, u64) {
        match self.rule {
            SourceRule::CounterOutcome { good } => (
                snapshot.counter(self.source_series, &labels([("outcome", good)])),
                snapshot.counter_total(self.source_series),
            ),
            SourceRule::LatencyUnder => {
                let threshold = self.slo.latency_threshold_ms.unwrap_or(f64::NEG_INFINITY);
                snapshot
                    .series
                    .iter()
                    .filter(|s| s.name == self.source_series)
                    .filter_map(|s| match &s.value {
                        crate::metrics::MetricValue::Histogram(h) => Some(h),
                        _ => None,
                    })
                    .fold((0, 0), |(good, total), h| {
                        let under: u64 = h
                            .bounds
                            .iter()
                            .zip(&h.counts)
                            .filter(|(bound, _)| **bound <= threshold)
                            .map(|(_, count)| *count)
                            .sum();
                        (good + under, total + h.count)
                    })
            }
            SourceRule::NotCountable => (0, 0),
        }
    }
}

/// The service name of the objectives the regional execution node evaluates
/// against its own registry. A path whose series only the node records is
/// evaluated there and nowhere else: the deep brain's registry never holds the
/// series, so evaluating it there would publish a permanent "unmeasured" for a
/// path that is in fact measured one process away.
pub const EDGE_NODE_SERVICE: &str = "edge-node";

/// Market-data freshness, decision latency, order/fill integrity, ledger
/// commit, reconciliation, model serving, data provenance and quantum job
/// success.
pub fn critical_path_slos() -> Vec<CriticalPath> {
    let p = |slo, source_series, rule| CriticalPath {
        slo,
        source_series,
        rule,
    };
    vec![
        p(
            Slo::latency(
                "market-data-freshness",
                "market-ingestion",
                0.999,
                1000.0,
                SloWindow::Day,
            ),
            names::EDGE_CAPABILITY_FRESHNESS,
            SourceRule::NotCountable,
        ),
        // Decision latency: a pass from reading the feed to its decisions
        // acted on, timed by the node's pass meter around every pass that
        // ran. This named `EXECUTION_LATENCY_MS` and the fast brain until
        // 2026-10-18, a series no production site records, so the path was
        // unmeasured in every process while the node recorded the real one.
        p(
            Slo::latency(
                "decision-latency",
                EDGE_NODE_SERVICE,
                0.99,
                50.0,
                SloWindow::Day,
            ),
            names::EDGE_DECISION_LATENCY_MS,
            SourceRule::LatencyUnder,
        ),
        p(
            Slo::availability("order-fill-integrity", "execution", 1.0, SloWindow::Month),
            names::CENTRAL_FILLS_ATTRIBUTED,
            SourceRule::NotCountable,
        ),
        p(
            Slo::latency("ledger-commit", "ledger", 0.999, 100.0, SloWindow::Day),
            names::LEDGER_COMMIT_LATENCY_MS,
            SourceRule::LatencyUnder,
        ),
        p(
            Slo::availability("reconciliation", "execution", 1.0, SloWindow::Month),
            names::CENTRAL_RECONCILIATION_BREAKS,
            SourceRule::NotCountable,
        ),
        // Model serving: a promoted model whose package was written where
        // serving reads it, out of every promotion. Recorded by the deep
        // brain at the artifact write.
        p(
            Slo::availability("model-serving-package", "deepbrain", 0.99, SloWindow::Week),
            names::MODEL_PACKAGES,
            SourceRule::CounterOutcome { good: "written" },
        ),
        p(
            Slo::ratio_at_least(
                "data-provenance-lineage-completeness",
                "data",
                1.0,
                0.999,
                SloWindow::Day,
            ),
            // Records whose lineage reached the event log, out of every
            // record received. Recorded by the platform where it absorbs.
            names::DATA_LINEAGE_RECORDS,
            SourceRule::CounterOutcome { good: "linked" },
        ),
        // Quantum job success: attempts that produced a run, out of every
        // attempt. Recorded by the deep brain from the cycle's journalled
        // solver routing.
        p(
            Slo::availability("quantum-job-success", "deepbrain", 0.95, SloWindow::Week),
            names::QUANTUM_JOBS,
            SourceRule::CounterOutcome { good: "succeeded" },
        ),
    ]
}

/// Every critical path evaluated against one snapshot, in catalogue order.
pub fn evaluate(snapshot: &Snapshot) -> Vec<SloStatus> {
    evaluate_where(snapshot, |_| true)
}

/// The critical paths `keep` admits, evaluated against one snapshot, in
/// catalogue order. A process evaluates the paths whose series it records:
/// the node those of [`EDGE_NODE_SERVICE`], the deep brain the rest.
pub fn evaluate_where(snapshot: &Snapshot, keep: impl Fn(&CriticalPath) -> bool) -> Vec<SloStatus> {
    critical_path_slos()
        .iter()
        .filter(|path| keep(path))
        .map(|path| {
            let (good, total) = path.counts(snapshot);
            path.evaluate(good, total)
        })
        .collect()
}

/// Write each status as `qip_slo_budget_consumed{objective}` and
/// `qip_slo_observations{objective}`. The observations gauge is written for
/// every objective, including an unmeasured one at zero, so a chart shows
/// the path as unmeasured rather than leaving it absent.
pub fn record(metrics: &Metrics, statuses: &[SloStatus]) {
    for status in statuses {
        let objective = status.slo.name.as_str();
        metrics.gauge(
            names::SLO_OBSERVATIONS,
            labels([("objective", objective)]),
            status.observations as f64,
        );
        if status.is_observed() {
            metrics.gauge(
                names::SLO_BUDGET_CONSUMED,
                labels([("objective", objective)]),
                status.budget_consumed,
            );
        }
    }
}
