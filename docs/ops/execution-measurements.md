# Execution capabilities — what was measured, and what the number is not

**Date:** 2026-09-05. **Source:**
`backend/crates/tests/qip-acceptance/tests/performance.rs`, the section headed
"the execution capabilities". Every row below is a figure that section
printed on the run it was written from, and the acceptance test
`the_execution_measurements_document_names_only_tests_this_file_holds_and_says_what_a_number_is_not`
refuses a row naming a test that file does not hold, a measurement that file
makes without a row here, and any edit that drops the caveats in this
paragraph.

The traceability document
([`docs/architecture/algorik-blueprint-traceability.md`](../architecture/algorik-blueprint-traceability.md))
scored the execution plane's capabilities as TESTED and none as MEASURED
until its re-score of 2026-09-05, which cites this document. This document
is the first set of numbers, and it is important to be exact
about what kind of number each is.

## How to read a figure

* **Machine.** A shared Linux container with 4 cores, one test thread, no
  affinity, no isolation from the other agents and builds sharing the box.
  The profile is **release** (`cargo test --release`); the same tests build
  under `debug` several times slower, and a figure quoted without its profile
  is not a figure.
* **What it is.** The cost of one in-process seam, on this machine, with the
  fixture built before the clock starts. Each test asserts its premise — that
  the workload ran the number of items it claims, and produced the outcome
  the capability exists to produce — before it reads a clock.
* **What it is not.** **An in-process number on a shared container is not a
  deployment measurement.** There is no network here, no venue, no execution
  node, no colocation, and no I/O on any timed path. **Nothing is deployed:**
  `execution_nodes = {}` in every environment, and no Cloud Run workload runs
  `Cell::work`. None of these rows says anything about latency to a venue,
  and none should be quoted as if it did.
* **The bound.** Each test asserts a per-operation ceiling one to two orders
  of magnitude above the figure observed, so the assertion trips only on a
  change of complexity class — a clone per message, a scan where there was a
  lookup — and never on a slow morning. The number is the output; the bound
  is a floor under it, not a target.

## The figures

Per-operation, release profile, 4 cores, single-threaded, 2026-09-05.
The test column is the function name in `performance.rs`.

| Capability | Workload | Observed (release) | Bound | Date | Test | What the number does and does not prove |
|---|---|---|---|---|---|---|
| Central OMS submission (`qip-execution-engine` `oms.rs`) | 20,000 market orders through `OrderManager::submit`: validate, kill switch, autonomy level, five pre-trade limits, state machine, frictionless simulated fill | 3.41 µs/op (293,156 ops/s) | 500 µs/op | 2026-09-05 | `central_order_submission_costs_what_the_execution_measurements_say` | Proves the central submission path is a few microseconds of pure computation against a constant risk state. Does not include the kernel's per-cycle risk state, the event log, or any venue; the "venue" is the in-process simulator and every fill is marked simulated. Not a deployment measurement. |
| Central instrument feasibility (`with_instrument_feasibility`) | 20,000 orders, alternately on-lot and off-lot, against a lot-1/tick-0.01 grid installed for the instrument | 3.58 µs/op (279,108 ops/s) | 500 µs/op | 2026-09-05 | `central_instrument_feasibility_costs_what_the_execution_measurements_say` | Proves the grid is judged ahead of the safety controls at negligible cost and refuses exactly the off-lot half under `feasibility_lot`. Does not prove anything about a real listing's grid, which nothing here reads. Not a deployment measurement. |
| Edge work pass, fill confirmation, drop-copy reconciliation, region hold (`qip-edge` `Cell::work`, `confirm_execution_reports`, `reconcile`, `RegionTable` at the cell) | 2,000 passes of one always-firing marketable strategy over a two-level book with a region table wired; each pass confirms its own acceptance-time fill; the drop copy is observed and reconciled after each pass | work 20.98 µs/pass (47,657 passes/s); reconcile + settle 1.51 µs/pass (663,382/s) | 5,000 µs and 1,000 µs per pass | 2026-09-05 | `an_edge_work_pass_with_a_fill_and_its_drop_copy_costs_what_the_execution_measurements_say` | Proves one pass of the cell's loop — confirm, expire, narrow, evaluate, gate, hold, net, send, commit — and the reconcile that settles it cost tens of microseconds in-process with one strategy and one instrument. Does not include the feed, the mesh, the journal flush, or a node's loop; `qip-edge-node` runs passes only under `QIP_VENUE_FEED=simulated` and none is deployed. Not a deployment measurement. |
| Intent netting (`Cell::work` phase two) | 1,000 passes of four agreeing strategies netted into one order carrying four contributors | 35.35 µs/pass (28,290 passes/s) | 10,000 µs/pass | 2026-09-05 | `netting_four_intents_into_one_order_costs_what_the_execution_measurements_say` | Proves four intents net to one order per pass and the pass grows by roughly the cost of the extra strategy evaluations. Does not measure a netting ratio under real strategy diversity — the four agree by construction, so the ratio is one. Not a deployment measurement. |
| Internal crossing (`cross_internally`, `book_cross`) | 1,000 passes of a hundred against forty: forty crosses at the mid inside the cell, sixty goes to the venue, both strategies' lots and cash move each pass | 27.50 µs/pass (36,362 passes/s) | 10,000 µs/pass | 2026-09-05 | `an_internal_cross_costs_what_the_execution_measurements_say` | Proves a cross under the per-net cap is admitted, booked and journaled on every pass at a cost indistinguishable from an uncrossed pass. Does not exercise the crossing-interval window or the settlement of the venue-bound residual at the centre. Not a deployment measurement. |
| Resting-order expiry (`withdraw_expired`, `PricingPolicy::RestAtMid`) | 1,000 passes two seconds apart, each resting an order at the mid for one second and withdrawing the previous one through the venue's cancel | 17.98 µs/pass (55,607 passes/s) | 5,000 µs/pass | 2026-09-05 | `a_resting_orders_expiry_costs_what_the_execution_measurements_say` | Proves an expired order is withdrawn through the cancel path, closed and settled every pass, with the venue asked exactly once per expiry. Does not measure a real venue's cancel round trip; the cancel here returns in-process. Not a deployment measurement. |
| Edge feasibility gate (`qip_edge::feasibility::assess`) | 200,000 intents, alternately on-lot and off-lot, against a lot-1/tick-0.01 exchange model with a known touch size | 0.31 µs/op (3,190,498 ops/s) | 20 µs/op | 2026-09-05 | `the_edge_feasibility_gate_costs_what_the_execution_measurements_say` | Proves the pure gate is sub-microsecond and refuses exactly the off-lot half under `feasibility_lot`. Does not include the book read the cell does before calling it, nor the policy slot's constraints (`None` here). Not a deployment measurement. |
| Region reservation ledger (`RegionTable::reserve`, `commit`) | 200,000 hold-and-commit pairs of 100 against an opening of one billion, through the shared mutex | 0.14 µs/op (7,046,472 ops/s) | 20 µs/op | 2026-09-05 | `a_region_reservation_hold_and_commit_costs_what_the_execution_measurements_say` | Proves the ledger's hold path is a mutex acquisition and a comparison, and that what was committed left the free balance exactly. Does not measure contention — one thread, one cell — and per-region reservation still has no producer at the centre (traceability F6). Not a deployment measurement. |
| Sequencing (`qip-sequencing` `Sequencer::accept`) | 200,000 contiguous level-set messages on one stream, accepted in batches of 100 | 0.60 µs/op (1,666,413 ops/s) | 20 µs/op | 2026-09-05 | `sequencing_a_contiguous_stream_costs_what_the_execution_measurements_say` | Proves the contiguous case releases every message with one `StreamStarted` and no gap or duplicate. Does not measure a gap, a reorder, or the `on_bytes` decode in front of it. Not a deployment measurement. |
| Line arbitration (`LineArbiter::accept`) | 100,000 units delivered on two redundant lines, A always first, window of 400; 200,000 deliveries timed | 0.65 µs per delivered unit (1,547,524/s) | 20 µs/op | 2026-09-05 | `arbitrating_two_redundant_lines_costs_what_the_execution_measurements_say` | Proves every unit is published once from A and recognised as a duplicate from B. Does not measure a lagging or lossy line — the first attempt used a 64-unit window under 100-message batches and the arbiter correctly reported B's copies as `Missed`, which is the test's premise doing its job. Not a deployment measurement. |
| Capital envelope verification (`VerifiedEnvelope::verify`) | 20,000 verifications of one correctly signed envelope: HMAC over the signing payload, constant-time compare, cell and validity window | 3.04 µs/op (329,426 ops/s) | 500 µs/op | 2026-09-05 | `verifying_a_capital_envelope_costs_what_the_execution_measurements_say` | Proves the trust-root check in front of every deployment costs microseconds and refuses another key. Does not say anything about the signing scheme's strength — HMAC proves possession of a shared secret, not a signer's identity. Not a deployment measurement. |
| Policy payload verification and application (`VerifiedPolicy::verify`, `Cell::apply_policy`) | 2,000 unproduced payloads in strictly increasing sequence, each verified and applied, each sealing a chain entry | 23.26 µs/op (42,990 ops/s) | 2,000 µs/op | 2026-09-05 | `verifying_and_applying_a_policy_payload_costs_what_the_execution_measurements_say` | Proves the anti-replay sequence, the halt barrier and the journal entry cost tens of microseconds per payload. Does not measure a payload with produced slots, whose narrowing is a function of `now`, nor the mesh that carries it. Not a deployment measurement. |
| Journal chain (`Journal::record`, `verify`, `ship`) | 50,000 refusals recorded, the chain re-verified, the tail shipped to an in-memory mirror in one batch | record 2.62 µs/entry; verify 1.90 µs/entry; ship 0.27 µs/entry | 200, 200 and 50 µs/entry | 2026-09-05 | `the_journal_chain_costs_what_the_execution_measurements_say` | Proves the hash chain costs a JSON serialisation and a SHA-256 per entry on the pass and again on replay, and that a shipped batch chains onto genesis. Does not measure a `FileMirror`, which is the one call that blocks. Not a deployment measurement. |
| Multi-leg group (`LegGroup`) | 20,000 two-leg groups assembled, both legs filled, assessed and settled complete | 1.75 µs/op (572,617 ops/s) | 200 µs/op | 2026-09-05 | `a_two_leg_group_completing_costs_what_the_execution_measurements_say` | Proves the lifecycle's happy path is microseconds and that a fully filled group is judged complete rather than unwound. Does not measure an unwind, a deadline, or the placement of the reversing orders. Not a deployment measurement. |

## What could not be measured in-process, and why

* **Routing (`qip-routing`: `Router`, children, order types, `Repricer`).**
  `qip-acceptance` does not depend on `qip-routing`, and adding the
  dev-dependency is a `Cargo.toml` change outside this slice's paths. The
  crate has its own tests; a figure needs a caller here first.
* **The node's pass loop (`qip-edge-node` `run_pass`).** An application crate,
  not a library the acceptance suite can link. Its passes reach a process only
  under `QIP_VENUE_FEED=simulated`, and no node is deployed.
* **The arbitrage desk at the cell (`Cell::scan_cycles`).** Needs an installed
  desk and a cycle whitelist the centre does not yet produce
  (`ArbitrageInstaller` waits on it). The scanner alone is already measured in
  the same file as "arbitrage scan"; the seam that admits its legs into a pass
  is not.
* **The central plane's ingest of a cell's report (`CentralPlane::ingest`).**
  Reached through `Platform::ingest_cell_report`, which needs a whole kernel
  and a signed report; the kernel-cycle test in the same file bounds the cycle
  as a whole and does not separate this seam.
* **Anything with a wire on it.** Mesh delivery, the venue adapter, the drop
  copy's transport, the halt flag's file poll. Every timed path here is in
  memory, by construction and by the workspace's rules.

## Running it

```
cd backend && cargo test -p qip-acceptance --test performance --release -- --nocapture
```

Re-run before quoting a number: the container's other tenants move the
figures by tens of percent from run to run. A figure that trips a bound is a
change of complexity class until proven otherwise, and the bound is not the
thing to edit.
