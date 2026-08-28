# Domain: observability and SRE

**Scope** — `crates/libs/qip-observability/**`, `ops/observability/**`, and the
health surfaces in `crates/apps/**`

## The state of this domain

**Nothing currently writes to `Telemetry`.** Every process constructs one and
none records to it, so `/metrics` serves an empty surface — and the four Cloud
Monitoring alert policies are gated behind `workload_metrics_exist = false`
because no metric descriptor has ever been ingested.

Do not describe this platform as observable. Closing this is tracked work.

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
