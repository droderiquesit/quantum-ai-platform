//! One objective for each of the eight critical paths, with its source series.

use crate::slo::{Slo, SloStatus, SloWindow};

/// An objective for one critical path, with the series that feeds it. A path
/// whose series nothing records evaluates with zero observations and so reads
/// as unmeasured ([`SloStatus::is_observed`]), never as met.
#[derive(Clone, Debug, PartialEq)]
pub struct CriticalPath {
    pub slo: Slo,
    /// The series whose good/total counts feed this objective. Where nothing
    /// emits it yet, this is the name it must be emitted under.
    pub source_series: &'static str,
}

impl CriticalPath {
    pub fn evaluate(&self, good: u64, total: u64) -> SloStatus {
        self.slo.evaluate(good, total)
    }
}

/// Market-data freshness, decision latency, order/fill integrity, ledger
/// commit, reconciliation, model serving, data provenance and quantum job
/// success.
pub fn critical_path_slos() -> Vec<CriticalPath> {
    use crate::metrics::names;
    let p = |slo, source_series| CriticalPath { slo, source_series };
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
        ),
        p(
            Slo::latency("decision-latency", "fastbrain", 0.99, 50.0, SloWindow::Day),
            names::EXECUTION_LATENCY_MS,
        ),
        p(
            Slo::availability("order-fill-integrity", "execution", 1.0, SloWindow::Month),
            names::CENTRAL_FILLS_ATTRIBUTED,
        ),
        p(
            Slo::latency("ledger-commit", "ledger", 0.999, 100.0, SloWindow::Day),
            names::LEDGER_COMMIT_LATENCY_MS,
        ),
        p(
            Slo::availability("reconciliation", "execution", 1.0, SloWindow::Month),
            names::CENTRAL_RECONCILIATION_BREAKS,
        ),
        p(
            Slo::availability(
                "model-serving-package-freshness",
                "deepbrain",
                0.99,
                SloWindow::Week,
            ),
            "qip_model_package_age_seconds",
        ),
        p(
            Slo::ratio_at_least(
                "data-provenance-lineage-completeness",
                "data",
                1.0,
                0.999,
                SloWindow::Day,
            ),
            "qip_lineage_complete_ratio",
        ),
        p(
            Slo::availability("quantum-job-success", "deepbrain", 0.95, SloWindow::Week),
            "qip_quantum_jobs_total",
        ),
    ]
}
