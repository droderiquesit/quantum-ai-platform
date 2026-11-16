# ADR 0682: The ledger daemon adopts the shared telemetry drain, and every binary without an exporter is named with its reason

- **Status**: **Proposed 2026-11-15**, lane 41 of wave b52. Written without a
  build (owner instruction of 2026-11-15); the pipeline builds and tests it.
- **Date**: 2026-11-15
- **Rows**: R12-OBS-9140.
- **Amends**: nothing. Applies ADR 0156 5.5.

This record first held two decisions. The first, that drawn tail worlds and
the capacity probe journal `MonteCarlo` episodes (R12-EXPAND-6033), was taken
out on 2026-11-16 with its code: the episodes it journaled were not counted in
the stress report, and the existing test
`every_scenario_simulate_runs_leaves_a_synthetic_episode_naming_its_input_digest`
found thirteen episodes on the log where the report held nine. What remains
is the decision whose code stays.

## Context

ADR 0156 5.5 directs every deployed root to export its metrics through
`qip-telemetry-drain`. Four roots did; `qip-ledgerd` did not, and no list said
which binaries export nothing and why, so a binary could ship with no exporter
and no record that it had none.

## Decision

1. `qip-ledgerd` adopts `qip-telemetry-drain` as ADR 0156 5.5 directs.
2. The reflex node (`qip-edge-node`) links no exporter. A drain in its closure
   puts a thread that performs network I/O beside the decision loop;
   `qip-telemetry-drain` is a Lane 1 crate and ADR 0116 says a second hosted
   Lane 1 crate reopens the sidecar rather than widening the hosted set. The
   node's OTLP export waits on that record.
3. Every binary that has no exporter is named on a committed list with the
   reason, and an acceptance test holds the list to the binaries that really
   do not call the drain. The row stays open while the list is non-empty.

## What it costs

`qip-ledgerd` gains a dependency on `qip-telemetry-drain` and a thread that
exports when `QIP_OPENOBSERVE_URL` and its organisation are set; unset, it
exports nothing, as before. Decision 2 leaves the reflex node with no OTLP
export, and decision 3 keeps R12-OBS-9140 open while the list is non-empty.
Two of the three named exemptions (`qip-web` and `qip-cli`) rest on no record
of their own yet; decision 40 of the owner-decisions file requires each to be
given one or removed. The cost of the export itself is unmeasured: nothing has
been shown to ingest.

## What would make this wrong

A second Lane 1 crate has to be hosted by the node, which under ADR 0116
reopens the sidecar and with it decision 2. A binary on the exemption list is
found to call the drain, or one off the list is found not to, which the
acceptance test exists to catch. A collector is deployed and shows that the
ledger's export adds load the daemon cannot carry.

## Evidence

- `qip-ledgerd` depends on `qip-telemetry-drain`; `src/openobserve.rs` is the
  same thin module the other four roots hold, and `main.rs` spawns the drain
  after `start` returns, handing it the ledger's own `Metrics`.
- `qip-acceptance/tests/otlp_coverage.rs` enumerates `backend/crates/apps/*`
  and fails a binary that neither links the drain nor is named, with a reason,
  on `EXPORTERLESS`. `qip-edge-node`, `qip-web` and `qip-cli` are named.
- No manifest sets a collector URL (ADR 0156 5.4); nothing has been shown to
  ingest.
