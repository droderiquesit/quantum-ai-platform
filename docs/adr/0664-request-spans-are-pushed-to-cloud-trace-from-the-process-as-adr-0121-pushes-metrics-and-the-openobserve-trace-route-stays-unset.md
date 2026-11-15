# ADR 0664: Request spans are pushed to Cloud Trace from the process as ADR 0121 pushes metrics, and the OpenObserve trace route stays unset

- **Status**: **Proposed**, 2026-11-15, by lane 32 of wave b52. The
  requirement text (R12-OBS-10059, "Warm/API requests emit traces to Cloud
  Trace") stands by the owner ruling of 2026-10-31 (ADR_THEN_IMPLEMENT); this
  record is the ADR that ruling asks for, and it is not accepted until the
  owner or the orchestrator says so.
- **Date**: 2026-11-15
- **Keeps**: ADR 0121 (the push shape this copies), ADR 0028/0156 (the
  OpenObserve drain, whose trace route stays unset), ADR 0109 (no new
  dependency without its own record), `00-boundaries.md` (no lib performs I/O
  except `qip-transport`).

## Context

`qip-api` opens a request-scoped span per API request
(`apps/qip-api/src/tracing.rs`) and `Tracer::export` renders the ring as
OTLP/JSON for the OpenObserve drain. The requirement names Cloud Trace.
`QIP_OPENOBSERVE_URL` is unset in every deployment, so no span leaves a
process on any default path, and nothing has observed an exported trace.

## Decision

1. **Each binary that holds a `Tracer` pushes finished spans to Cloud Trace
   v2** (`projects.traces.batchWrite`, `POST
   https://cloudtrace.googleapis.com/v2/projects/{project}/traces:batchWrite`)
   from the exporter thread that already pushes metrics, never from a
   request thread. The body is a pure function of `Span` values
   (`qip_observability::cloud_trace::batch_write_body`), no I/O.
2. **Bounds.** One request per interval, at most 200 spans per request (the
   API's batch limit is far higher; the bound keeps one failure small), an
   explicit timeout on every call, a failure counted by an enum `reason` and
   dropped, never queued without bound. Fields over the API's limits are
   truncated with the truncated byte count the API's `TruncatableString`
   carries, never silently cut; a span the API would refuse (id not 32/16
   lowercase hex, no end instant) is named and left out rather than failing
   the batch.
3. **Identity** is the workload identity already used for Cloud Monitoring;
   the role is `roles/cloudtrace.agent`, and the grant is one missing
   permission, not a widening.
4. **The OpenObserve trace route stays unset.** Both routes are not run for
   one span; choosing the second again is a new record.
5. **Proof of ingestion** is a trace read back from the API in `dev` for one
   `qip-api` request id. Until then the series is emitted, not observed.

## What it costs

- A second outbound call on the exporter thread, and Cloud Trace ingestion
  cost, unpriced here.
- Span attributes become Cloud Trace attributes: nothing secret may be an
  attribute value (`01-security-and-safety.md`).

## Alternatives rejected

- **OpenTelemetry SDK crate:** a new dependency tree for a body this small.
- **Cloud Run's automatic tracing only:** records the platform's own span, not
  the request's internal structure.
- **Keep OpenObserve as the only exporter:** the requirement names Cloud Trace.

## What would make this wrong

- An export on a request thread, or with no timeout, or an unbounded backlog.
- A span with a secret in an attribute.
- The role widened beyond `roles/cloudtrace.agent` to make an error go away.
- A claim of "traces observed" with no trace read back.

## Dependency direction

Encoding in `qip-observability` (lib, no I/O); the call in the composition
roots through `qip-transport`. **No new dependency.**

## R12 ids

R12-OBS-10059.

## The paper-trading boundary

Untouched; tracing opens no order path. The three layers stand.

## Gates

Markdown record; the encoder's own gates are in the commit that builds it.
