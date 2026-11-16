//! The six data-plane objectives of R12-OBS-9163 (ADR 0683), each declared
//! with a target and each bound to the series that feeds it, or to an
//! [`SourceRule::Unbuilt`] that says what is missing.
//!
//! The requirement names six: GCS archive delay, source re-fetch health,
//! Bigtable p99, knowledge-delta freshness, BQ ingestion freshness and lineage
//! completeness. `slo::default_slos` declared three of them (service
//! `data-plane`) and a test holds that set at three, so the other three are
//! declared here rather than there; evaluating all six from one place is what
//! lets "a deleted recording site reads unmeasured" be asserted for each.
//!
//! Two are measured today. Lineage completeness reads
//! `qip_data_lineage_records_total`, recorded by `Platform::count_lineage`;
//! re-fetch health reads `qip_data_refetch_checks_total`, recorded beside the
//! gauge in `Platform::record_refetch_health`. The gauge could not feed an
//! objective (it carries no good/total split), which is the whole reason the
//! counter exists. The other four have nothing to record from: no process
//! ingests a GCS archive, and the Bigtable, BigQuery and knowledge-delta
//! stores are not on. Each is declared and evaluates unmeasured, saying why,
//! and names [`NO_SOURCE_SERIES`] so nobody greps for a writer that never
//! existed.

use crate::critical_paths::{CriticalPath, NO_SOURCE_SERIES, SourceRule};
use crate::metrics::names;
use crate::slo::{Slo, SloWindow, default_slos};

/// The service the six objectives are declared under, which is the one
/// `default_slos` already uses for its three.
pub const DATA_PLANE_SERVICE: &str = "data-plane";

pub const ARCHIVE_DELAY_OBJECTIVE: &str = "data-archive-delay";
pub const REFETCH_HEALTH_OBJECTIVE: &str = "data-refetch-health";
pub const LINEAGE_COMPLETENESS_OBJECTIVE: &str = "data-lineage-completeness";
pub const BIGTABLE_P99_OBJECTIVE: &str = "data-bigtable-p99";
pub const KNOWLEDGE_DELTA_FRESHNESS_OBJECTIVE: &str = "data-knowledge-delta-freshness";
pub const BQ_INGESTION_FRESHNESS_OBJECTIVE: &str = "data-bq-ingestion-freshness";

/// PROVISIONAL. Nobody has given the Bigtable read latency objective, and no
/// Bigtable store is on to measure one. Ten milliseconds at the 99th
/// percentile is chosen in the restrictive direction: the platform's own
/// risk pre-check is held to ten milliseconds, so a store read that cannot
/// meet the same figure would be the slower half of any decision it feeds.
/// Replace it when somebody who owns the figure gives it.
pub const BIGTABLE_P99_THRESHOLD_MS: f64 = 10.0;
/// PROVISIONAL, as above: the target share of reads inside the threshold.
pub const BIGTABLE_P99_TARGET: f64 = 0.99;
/// PROVISIONAL. The longest, in seconds, a knowledge delta may take to reach
/// the world model after it was knowable. Five minutes, the tighter of the
/// two plausible figures (five and fifteen); nothing measures it.
pub const KNOWLEDGE_DELTA_FRESHNESS_CEILING_SECONDS: f64 = 300.0;
/// PROVISIONAL. The longest, in seconds, between a record being knowable and
/// it landing in BigQuery. Five minutes for the same reason.
pub const BQ_INGESTION_FRESHNESS_CEILING_SECONDS: f64 = 300.0;
/// PROVISIONAL. The share of checks inside either freshness ceiling.
pub const FRESHNESS_TARGET: f64 = 0.99;

const ARCHIVE_DELAY_UNBUILT: &str = "no process ingests a GCS archive and learns how long ago \
     it was written; qip-fabricd writes the archive and records the segments it has not yet \
     archived, which is a different fact";
const BIGTABLE_UNBUILT: &str = "no Bigtable store is on, so no read latency is recorded; the \
     target and threshold are provisional";
const KNOWLEDGE_DELTA_UNBUILT: &str = "nothing records how long after it became knowable a \
     knowledge delta reached the world model; the ceiling is provisional";
const BQ_UNBUILT: &str = "no BigQuery ingestion is on, so no freshness is recorded; the ceiling \
     is provisional";

/// The three objectives `default_slos` declares for the data plane, taken by
/// name so the declaration and the evaluation carry one target, and the three
/// this module declares. A name `default_slos` stopped declaring drops out
/// and is caught by the test that holds all six.
pub fn data_plane_objectives() -> Vec<CriticalPath> {
    let declared = default_slos();
    let from_defaults = |name: &str, source_series, rule| {
        declared
            .iter()
            .find(|slo| slo.name == name && slo.service == DATA_PLANE_SERVICE)
            .map(|slo| CriticalPath {
                slo: slo.clone(),
                source_series,
                rule,
            })
    };
    let own = |slo, source_series, rule| {
        Some(CriticalPath {
            slo,
            source_series,
            rule,
        })
    };
    [
        from_defaults(
            ARCHIVE_DELAY_OBJECTIVE,
            NO_SOURCE_SERIES,
            SourceRule::Unbuilt {
                why: ARCHIVE_DELAY_UNBUILT,
            },
        ),
        from_defaults(
            REFETCH_HEALTH_OBJECTIVE,
            names::DATA_REFETCH_CHECKS,
            SourceRule::CounterOutcome { good: "served" },
        ),
        from_defaults(
            LINEAGE_COMPLETENESS_OBJECTIVE,
            names::DATA_LINEAGE_RECORDS,
            SourceRule::CounterOutcome { good: "linked" },
        ),
        own(
            Slo::latency(
                BIGTABLE_P99_OBJECTIVE,
                DATA_PLANE_SERVICE,
                BIGTABLE_P99_TARGET,
                BIGTABLE_P99_THRESHOLD_MS,
                SloWindow::Day,
            ),
            NO_SOURCE_SERIES,
            SourceRule::Unbuilt {
                why: BIGTABLE_UNBUILT,
            },
        ),
        own(
            Slo::availability(
                KNOWLEDGE_DELTA_FRESHNESS_OBJECTIVE,
                DATA_PLANE_SERVICE,
                FRESHNESS_TARGET,
                SloWindow::Day,
            ),
            NO_SOURCE_SERIES,
            SourceRule::Unbuilt {
                why: KNOWLEDGE_DELTA_UNBUILT,
            },
        ),
        own(
            Slo::availability(
                BQ_INGESTION_FRESHNESS_OBJECTIVE,
                DATA_PLANE_SERVICE,
                FRESHNESS_TARGET,
                SloWindow::Day,
            ),
            NO_SOURCE_SERIES,
            SourceRule::Unbuilt { why: BQ_UNBUILT },
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}
