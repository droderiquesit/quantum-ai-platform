# ADR 0121: Cloud Run metrics are pushed to Cloud Monitoring from the process, and the refused sidecar is no longer awaited

- **Status**: **Accepted**, 2026-10-11, by the owner in the session, under the
  instruction: "You make the recommended decision and have full permission".
- **Date**: 2026-10-11
- **Amends**: ADR 0026's surviving position that the Cloud Run half waits on
  Google's managed-Prometheus sidecar. The execution node's Ops Agent
  Prometheus receiver (ADR 0026 option (a) for the node) is unchanged.
- **Keeps**: ADR 0028/0032/0033 (the OpenObserve drain, still pointed at
  nothing while it is anonymous), ADR 0113 §2 (Google APIs are reached by the
  in-tree REST adapter over `rustls`, never an SDK), ADR 0112 (TLS to Google
  APIs).

## Context

Nine alert policies in `infrastructure/terraform/modules/observability/main.tf`
are declared and gated off behind `workload_metrics_exist = false`, because
nothing has been shown to ingest a metric from any process. For Cloud Run
the only collector was `cloud-run-gmp-sidecar`, and the only published
version fails the platform's own vulnerability gate
(`docs/ops`' record of `vendor.yml` run 11; `.claude/rules/domains/observability.md`).
The register scores R12-OBS-9143 BLOCKED on "an upstream fixed sidecar
image". That is a wait with no owner and no date, on a component the platform
does not control, for the three central binaries that hold the platform's
most important series — the kill-switch gauge, limit breaches, orders
refused, and the reconciliation breaks the alert policies page on.

The obstacle that made a push path unattractive is gone: the process can now
speak TLS to Google APIs (ADR 0112), and the REST adapter pattern for Google
APIs already exists (ADR 0113 §2).

## Decision

1. **Each central binary on Cloud Run pushes its registry snapshot to the
   Cloud Monitoring API** (`projects.timeSeries.create`) over the in-tree
   REST adapter and `rustls`, with a metadata-server token, from a dedicated
   exporter thread. Never from a request thread and never from the cycle:
   the exporter reads the same `Telemetry` registry handle the `/metrics`
   scrape reads.
2. **Interval and bounds.** Every 60 seconds; one request per interval,
   batched within the API's per-request series limit; an explicit timeout on
   every call; a failed push is counted
   (`qip_metrics_export_failures_total{reason}`, `reason` an enum) and
   dropped, never queued without bound. Counters are written as cumulative
   series with the process start as their start time, gauges as gauges,
   histograms as distributions with the registry's own bucket bounds.
3. **The descriptors the alert policies query are the descriptors pushed.**
   The change that implements this rewrites the nine policies' queries to the
   pushed metric types in the same commit, and a plan shows each resolves;
   `workload_metrics_exist` is flipped only after a pushed series has been
   read back from the API in `dev`, which is the ingestion proof the
   observability rules require.
4. **The sidecar declaration stays dormant, not deleted.**
   `metrics_collector_image_digest` remains null; if Google publishes a
   version that passes the scan gate, the choice between the two is a new
   record, not a silent re-enable.

## What it costs

- **Code on the central binaries' outbound path**: a token refresh, a JSON
  encoder for time series, and a thread. It is off the decision thread, but
  it is one more consumer of the egress path and of the registry lock.
- **Per-series ingestion cost** in Cloud Monitoring, unpriced here; the label
  bounds the observability rules already enforce are what keep it bounded.
- **The metric names the policies query change**, which is churn across the
  module, its documentation and `NOT-SCRAPED.md`.
- **Two exposition paths**: `/metrics` for a scraper and a push for Cloud
  Run. They read one registry, so they cannot disagree about a value, but
  they can disagree about whether it was delivered.

## Alternatives rejected

- **Keep waiting for an upstream sidecar.** No date and no owner; the alerts
  that matter most stay unevaluated meanwhile.
- **Build and vendor our own collector image from patched source.** It moves
  the scan problem in-house and adds a Go supply chain the platform would
  have to rebuild for every upstream advisory.
- **A scanner exception for the known CVE.** Refused by the observability
  rules in terms: "do not clear it with a scanner exception".
- **An OpenTelemetry SDK crate.** ADR 0026 option (c), still rejected: a
  large dependency tree for a mapping the registry can encode itself.

## What would make this wrong

- An export call on the cycle's thread or a request thread, or a push with no
  timeout.
- An export backlog that grows without bound when the API is unreachable.
- `workload_metrics_exist` flipped before a pushed series was read back.
- An alert policy naming a metric type the exporter does not push.
- A label value drawn from a runtime string rather than an enum, a fixed
  configuration value or a source literal.

## Dependency direction

The time-series encoding is a pure function of a registry `Snapshot` in
`qip-observability` (lib, no I/O). The HTTP call, the token and the thread
live in the composition roots (`apps/qip-api`, `qip-fastbrain`,
`qip-deepbrain`) through `qip-transport`, the one lib permitted sockets. No
lib gains I/O; no service depends on the runtime; nothing depends on an app.
**No new dependency.**

## R12 ids

Unblocks to build: **R12-OBS-9143**.

## The paper-trading boundary

Untouched. Exporting metrics opens no order path; the three layers and the
ledger's fourth fence stand.

## Gates

None run. Markdown only, and no shell in the session that wrote it.
