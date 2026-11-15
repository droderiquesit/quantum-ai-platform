# ADR 0683: The six data-plane objectives are declared together, the unbuilt four say so, and two decisions on runbooks and the training export

- **Status**: **Proposed 2026-11-15**, lane 41 of wave b52. Written without a
  build (owner instruction of 2026-11-15); the pipeline builds and tests it.
- **Date**: 2026-11-15
- **Rows**: R12-OBS-9163, R12-OBS-9074, R12-MODEL-6002. All stay PARTIAL
  until the pipeline has run the tests named in `docs/blueprint/lanes/b52-41.json`.

## R12-OBS-9163: data-plane objectives

### Context

The requirement names six data objectives: GCS archive delay, source
re-fetch health, Bigtable p99, knowledge-delta freshness, BQ ingestion
freshness, lineage completeness. `slo::default_slos` declared three, and
`qip-observability/tests/telemetry.rs::data_slo_metrics_integrate_with_slo_evaluation`
holds that set at exactly three. No `SourceRule` turned any of them into good
and total counts, so each read unmeasured whatever was recorded. An earlier
draft added re-fetch health to `critical_path_slos` and failed
`the_catalogue_holds_one_objective_per_critical_path_and_an_unfed_one_reads_unmeasured`,
which holds that catalogue at eleven.

### Decision

1. The six live in a catalogue of their own, `qip_observability::data_plane::data_plane_objectives`,
   which `critical_paths::evaluate_where` chains after the reflex objectives.
   Neither the eleven-entry critical-path catalogue nor the three-entry
   `default_slos` data set is touched; the three existing objectives are taken
   from `default_slos` by name so their targets are stated once.
2. Two are measured. Lineage completeness reads
   `qip_data_lineage_records_total`. Re-fetch health reads a new counter,
   `qip_data_refetch_checks_total{outcome=served|withdrawn}`, recorded in
   `Platform::record_refetch_health` beside the gauge, because a gauge carries
   no good/total split. The counter is unlabelled by source: an objective is
   one figure for the plane.
3. Four are `SourceRule::Unbuilt`, name `NO_SOURCE_SERIES`, and say why:
   archive delay (no process ingests a GCS archive), Bigtable p99,
   knowledge-delta freshness and BQ ingestion freshness (the stores are not
   on). They evaluate unmeasured. This is the honest reading, not a gap
   hidden: a `LatencyUnder` bound to a histogram nothing records would read
   as a recording site somebody deleted.
4. **Provisional figures, chosen in the restrictive direction**: Bigtable
   read p99 at most 10 ms for 99% of reads (the platform's own risk
   pre-check ceiling); knowledge-delta and BQ ingestion freshness at most 300
   seconds for 99% of checks. Nobody gave these; each is a named constant
   marked PROVISIONAL in `data_plane.rs`, to be replaced by whoever owns the
   figure. They cannot page anything: the objectives read unmeasured.
5. `DATA_ARCHIVE_DELAY_MINUTES`'s doc said it was "recorded when archive
   ingestion completes". No such ingestion exists; the doc now says so.

### What remains

Archive delay needs a process that ingests a GCS archive and learns when it
was written; the other three need their stores. Until then the row cannot be
COMPLETE.

## Row R12-OBS-9074 and R12-MODEL-6002: see the sections below.
