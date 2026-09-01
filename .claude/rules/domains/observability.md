# Domain: observability and SRE

**Scope** — `backend/crates/libs/qip-observability/**`, `docs/ops/observability/**`, and the
health surfaces in `backend/crates/apps/**`

## The state of this domain

**Both planes emit and both are scrapable. What is still missing is proof
of ingestion, and the edge plane's recording sites in `Cell::work` reach no
deployed process until the node runs passes.** Read all of this before
writing anything about the domain: two earlier versions of this file were
each false in one direction — one said nothing wrote to `Telemetry`, the next
said the edge plane could not emit — and agents who believed either were told
a closed gap was still open.

`qip-kernel`'s `Platform` records in `platform.rs` — at least sixteen sites
as of `6fb5fed`, counted with
`grep -c 'metrics\.\(count\|gauge\|increment\|observe[a-z_]*\)(' backend/crates/runtime/qip-kernel/src/platform.rs`;
recount before quoting a number, because an earlier version of this file
carried a count from a version of the file that no longer existed. What is
recorded matters more than how many times: cycles and per-stage runs,
durations and problems; the kill-switch gauge; limit breaches; permission
denials; orders submitted, refused, filled and the live-fill alarm. All three central binaries construct the `Telemetry` the
cycle writes to and serve its snapshot: `qip-api` from `routes.rs` (`scrape`),
`qip-fastbrain` and `qip-deepbrain` from the registry handle their health
servers hold, taken from the same `Telemetry` before it moves into the
`Platform`. `/metrics` is empty only until the first cycle records something,
which is the honest answer rather than a gap. `qip-market-ingestion` also
records, but `IngestionService` is constructed only in its own tests, so those
three sites reach no deployed process.

**The edge plane emits through `qip_edge::CellMetrics` and is scraped at
`/metrics` on `qip-edge-node`'s health port.** `qip-edge` depends on
`qip-observability` — a library holding a `BTreeMap` behind a mutex, no I/O —
and a `Cell` records into a registry it is *given* by `Cell::with_metrics`,
never one it reached for. `qip-edge-node` constructs one `Telemetry`, takes
the registry handle first, hands it to the cell and to `MeshSeries`, and
serves the same handle's snapshot as Prometheus exposition; every other path
still answers the JSON health body. The series and what each is keyed on:

- `qip_edge_halted{source}` — a gauge per halt discipline (`kill_switch`,
  `policy`), written wherever either halt can change and at wiring time, so a
  cell halted before its first pass still reports halted.
- `qip_edge_capability_freshness{capability}` and `qip_edge_sizing_multiplier`
  — the §6.2 table as the pass actually sized against it, recorded per pass.
  Only the three policy-fed capabilities are published; `ingestion` and
  `counterfactual_scoring` are deliberately absent because the cell never
  measures them and a permanent `unavailable` would be a number nobody
  computed.
- `qip_edge_policy_sequence` — the payload the cell has *applied*, for
  correlation against what the centre believes it published.
- `qip_edge_work_passes_total`, `qip_edge_refusals_total{gate}`,
  `qip_edge_signals_raised_total{kind}`, `qip_edge_orders_placed_total{venue}`,
  `qip_edge_intents_cancelled_total`, `qip_edge_internal_crosses_total{venue}`,
  `qip_edge_netting_ratio` (histogram), `qip_edge_reconciliation_breaks_total`.
- From the node: `qip_edge_mesh_{deltas,grants,policy_frames}_total{outcome}`
  as deltas of the link's cumulative counters, and
  `qip_edge_mesh_circuit{state}`.

Every label is bounded by something fixed at deployment or by an enum or a
source-file literal: `cell` and `region` are one value per process, `venue`
is the configured venue list, `gate` is the set of string literals
`Cell::refuse` is called with, and `source`, `capability`, `kind`, `outcome`
and `state` are enums. Nothing is labelled by instrument, strategy or order
id. Each recording site is proven by a test in
`backend/crates/edge/qip-edge/tests/telemetry.rs` that drives the cell through
the event and asserts the series moved, and each was mutation-verified.

Two honest limits on the edge half. First, this build of `qip-edge-node`
configures no venue feed and never calls `Cell::work`: the halt gauge, policy
sequence and mesh series reach a deployed process, and the pass-time series
(freshness, refusals, signals, orders, netting, crosses) will only once the
node runs passes. Second, the edge-node health server is single-threaded, so
a scrape's exposition is rendered on the thread that flushes the journal — the
same thread that already renders the JSON body, and not the order path.

**A central-plane reconciliation break is now recorded too.**
`Platform::ingest_cell_report` counts `qip_central_reconciliation_breaks_total`
by the direction of the gap and `qip_central_cell_halts_total` by cause, on
the outcome rather than the report, so a refused report charts no halt. The
`qip_central_` prefix keeps it distinct from the edge's own break counter,
which records what the cell found rather than what the centre acted on. Two
facts still have no production caller at all: `Platform::learn_from`, which
produces the belief calibration, is called by nothing in the tree, and
`Platform::evaluate_alternatives`, which scores counterfactuals, is called
only by `qip-kernel`'s own tests.

`workload_metrics_exist` remains `false` everywhere — the default in
`infrastructure/terraform/variables.tf` and in the observability module, and
commented out in `environments/dev/terraform.tfvars` — and flipping it still
requires evidence a pod actually scraped. All four alert policies name
descriptors the kernel does record, and a collector selecting `qip-fastbrain`
and `qip-deepbrain` on `/metrics` is declared; no collector yet selects
`qip-edge-node`, and no alert policy names an edge descriptor. What is missing
is proof of ingestion, not emission.

Do not describe this platform as observable. That still holds, on today's
evidence: nothing has been shown to scrape any pod, the edge series have no
collector and no alert, and no alert policy names either `qip_central_`
descriptor, so a reconciliation break on either plane is charted and still
pages no one. Closing the remainder is tracked work.

## Approved

- Metrics recorded at the seam where the fact becomes known, not inferred later.
- Health endpoints reporting real readiness — storage proven writable, ports
  bound — rather than process liveness.

## Prohibited

- Naming a metric in an alert policy that nothing emits. Cloud Monitoring
  refuses a policy naming a descriptor it has never ingested, and a policy
  stored but never evaluated reads in the console as a project being watched,
  which is worse than the gap it replaces.
- Logging a token, key, or account identifier.
- A health check that reports ready before its dependencies are proven.

## Required evidence

The emitting code plus a test asserting the metric is recorded. Flipping
`workload_metrics_exist = true` requires evidence a pod actually scraped.
