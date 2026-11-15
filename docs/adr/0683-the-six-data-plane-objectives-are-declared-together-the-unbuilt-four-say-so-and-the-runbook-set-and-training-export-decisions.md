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


## R12-OBS-9074: the runbook set is signed by the image it is compiled into

### Context

The deep brain's runbooks are committed code. The 2026-10-31 reader found no
check could be written inside the build that a tampered build could not also
fake, and that a real signer outside the build (ADR 0110's KMS artefact
signing) is null everywhere, billed, and deferred. The owner ruled on
2026-10-31 (ADR_THEN_IMPLEMENT): the set is signed by the image attestation it
is compiled into; the deep brain runs only under the deny-by-default Binary
Authorization policy whose attestor key CI alone holds; no runbook is read at
run time.

### Decision

6. The signature is the image's Binary Authorization attestation. This ADR
   adds no signer, no KMS grant and no Terraform.
7. `operations::runbook_set_digest` is the SHA-256 of the set's canonical text
   (name order, length-prefixed fields, target order), and
   `REVIEWED_RUNBOOK_SET_DIGEST` is the digest of the set a person reviewed.
   `Operations::build` runs `runbooks()` through `verified_runbooks`, which
   refuses (`Error::denied`, naming both digests and the remedy) a set that
   digests otherwise, so the process stops rather than hold an unreviewed
   remediation set.
8. `the_shipped_runbook_set_is_the_reviewed_one` pins the constant to the
   shipped set, so a commit that changes one without the other fails in the
   pipeline, before an image exists to attest. The check inside the process is
   defence against a *mismatch of reviewed and shipped*, not against a
   tampered build; the attestation is the answer to that, and this ADR says
   so rather than letting the digest read as a signature.

### Residual risk

The constant was computed by an independent script, not by running the Rust,
because nothing was compiled. If the pin test fails on first run, the
constant is wrong and the correct value is in its failure message; no deployed
process is affected, since the pipeline runs the test before an image is
built.
