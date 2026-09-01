# Domain: observability and SRE

**Scope** — `backend/crates/libs/qip-observability/**`, `docs/ops/observability/**`, and the
health surfaces in `backend/crates/apps/**`

## The state of this domain

**The central plane emits and is scrapable. The edge plane cannot emit at
all.** Read both halves before writing anything about this domain: an earlier
version of this file said nothing wrote to `Telemetry`, which was false, and
agents who believed it were told a closed gap was still open.

`qip-kernel`'s `Platform` records at twenty-four sites in `platform.rs` —
cycles and per-stage runs, durations and problems; the kill-switch gauge;
limit breaches; permission denials; orders submitted, refused, filled and the
live-fill alarm. All three central binaries construct the `Telemetry` the
cycle writes to and serve its snapshot: `qip-api` from `routes.rs` (`scrape`),
`qip-fastbrain` and `qip-deepbrain` from the registry handle their health
servers hold, taken from the same `Telemetry` before it moves into the
`Platform`. `/metrics` is empty only until the first cycle records something,
which is the honest answer rather than a gap. `qip-market-ingestion` also
records, but `IngestionService` is constructed only in its own tests, so those
three sites reach no deployed process.

**The edge plane emits nothing and has no scrape surface.** `qip-edge-node`
declares `qip-observability` in its `Cargo.toml` and never constructs a
`Telemetry`; `qip-edge` has no telemetry dependency at all, so a cell
physically cannot emit however much it knows. Its uplink and downlink counters
exist only inside the JSON health body its health server writes, which nothing
collects as a series.

**Several safety facts are computed and then discarded.** Policy freshness
(`qip-contracts::policy::Freshness`) reaches `qip-edge`'s `Cell` and is
formatted into a display string for the journal. Degradation narrowing and
halt acceptance take the same route. A central-plane reconciliation break
trips a scoped kill switch and raises an incident without recording anything.
Two further facts have no production caller at all: `Platform::learn_from`,
which produces the belief calibration, is called by nothing in the tree, and
`Platform::evaluate_alternatives`, which scores counterfactuals, is called
only by `qip-kernel`'s own tests. A fact a process knows and never records is
a fact no operator can ever see.

`workload_metrics_exist` remains `false` everywhere — the default in
`infrastructure/terraform/variables.tf` and in the observability module, and
commented out in `environments/dev/terraform.tfvars` — and flipping it still
requires evidence a pod actually scraped. But the reason is no longer that
nothing emits. All four alert policies now name descriptors the kernel does
record, and a collector selecting `qip-fastbrain` and `qip-deepbrain` on
`/metrics` is declared; what is missing is proof of ingestion, not emission.

Do not describe this platform as observable. That still holds, on the evidence
above rather than the old one: a cell that cannot emit, a policy staleness
nobody can chart, and a reconciliation break that pages no one. Closing the
remainder is tracked work.

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
