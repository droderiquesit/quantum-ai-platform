# ADR 0720: The fabric SDK keeps thread-backed subscriptions and an in-tree telemetry hook, and the Spanner ledger waits on a measured load and an owner ceiling

- **Status**: Proposed 2026-11-16, for the owner. Decision 2 concerns billed
  infrastructure (a Spanner Enterprise Plus instance) and names a monthly
  ceiling the owner has not stated. Decision 1 amends no Accepted record.
- **Date**: 2026-11-16
- **Rows**: R12-FABRIC-7045, R12-LEDGER-10041

## Context

R12-FABRIC-7045 requires a Rust SDK with idempotent producer sequencing,
consumer groups, async subscriptions, local ring-buffer integration and
OpenTelemetry hooks. The register records that `qip-transport` has a producer
and consumer whose sequences agree with the served broker, and that
`GroupCoordinator` exists in `qip-streaming`. Async subscriptions and OTel
hooks are the gap. Owner decision O09 asked to admit an OpenTelemetry exporter
crate and `tokio` on the SDK path. The owner's ruling of 2026-10-31
(decision 14 of `docs/blueprint/owner-decisions-2026-10-31.md`) declined it.
ADR 0026 rejects an OpenTelemetry crate. ADR 0109 Amendment 2 keeps `tokio`
off the decision core and the cell's pass without a committed measurement, and
the edge node's drain reaches the SDK.

R12-LEDGER-10041 requires ledger, position, obligation, capital and
idempotency records in Spanner Enterprise Plus, multi-region, isolated from
analytics. ADR 0113 (Accepted) moved the ledger of record to Spanner. ADR 0393
fixes `nam3` with the leader at `us-east4`, and ADR 0394 validates that
configuration. The register records a Spanner REST client in `qip-storage`, a
`SpannerBackend` in `qip-ledgerd`, and Terraform for the instance, with
`enable_spanner = false` in every environment. The tests use an in-process
double, which does not show that Google answers as modelled. The instance is
recurring cost.

## Decision

1. **R12-FABRIC-7045 (build, in-tree; declines O09).** The SDK's telemetry
   hooks are a trait defined in the SDK crate with no third-party type in its
   signature. It reports only through the existing OTLP drain that ADR 0156
   names as the telemetry home. No OpenTelemetry exporter crate is admitted
   and no `tokio` reaches the SDK. Async subscriptions stay thread-backed: a
   subscription is a thread that delivers into a bounded channel, with an
   explicit timeout on every call that leaves the process. The ring-buffer
   integration and the use of `GroupCoordinator` by the SDK are in-tree work
   under this decision and need no dependency. Idempotent sequencing is
   unchanged. A duplicate producer sequence is deduplicated by the broker, as
   it is today. The row stays PARTIAL until the hook, the subscription and the
   group use exist and the check in the requirement is run.
2. **R12-LEDGER-10041 (leave to the owner).** The ledger of record goes to
   Spanner Enterprise Plus as ADR 0113 and ADR 0393 say. Nothing is enabled by
   this record. `enable_spanner` stays `false`, and no first run is claimed
   from the in-process double. Enabling it is the owner's protected tfvars
   change, made after the base deploy has run.
3. **R12-LEDGER-10041 (needs a figure).** Two figures are needed and none is
   chosen here: the measured ledger load (writes per second and stored bytes)
   from a deployed `qip-ledgerd`, and a monthly ceiling the owner states for
   the instance. Until both exist, nothing in this record permits an apply.
   A stated ceiling is a hard stop, not a target.
4. **R12-LEDGER-10041 (closing evidence).** The row can leave PARTIAL only
   when one `qip-ledgerd` delivery is recorded as committed through the real
   multi-region Enterprise Plus database, one start is recorded as refused
   against an unreachable database, and the analytics read path is shown not to
   read the ledger database directly. No claim of any of these is made here.
5. **No dependency is admitted by this record.** The Spanner client stays the
   in-tree REST client of ADR 0113. A gRPC client, if ever wanted, needs its
   own ADR under ADR 0109.

## What it costs

- The SDK has no async runtime of its own. A caller wanting one wraps the
  thread-backed subscription itself. Subscription fan-out is bounded by
  threads, which costs memory per subscriber.
- Telemetry from the SDK appears only through the existing drain. No exporter
  features and no vendor-native tracing are available from the SDK.
- The ledger stays on its current backend until the owner acts. The register
  keeps R12-LEDGER-10041 at PARTIAL, and the financial truth is not yet on a
  multi-region store. Spanner Enterprise Plus is priced by the register at
  $2,705 to $8,140 a month; those are the register's figures, not a choice made
  here.

## What would make this wrong

- A committed measurement showing the thread-backed subscription misses a
  latency or fan-out need. Then the `tokio` question reopens under ADR 0109
  Amendment 2 with that measurement, and not before.
- The OTLP drain proving unable to carry the SDK's hook data without a
  schema change. Then the hook needs its own record.
- A measured ledger load that fits a cheaper single-region store with
  acceptable recovery from a regional loss. That would be an owner decision
  amending ADR 0113, not a quiet substitution.
- The first real run showing Google answers differently from the in-process
  double. The double's own header warns of this, and the evidence in decision 4
  would then fail.
