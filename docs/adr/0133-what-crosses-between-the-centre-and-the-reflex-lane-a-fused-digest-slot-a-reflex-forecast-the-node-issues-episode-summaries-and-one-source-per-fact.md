# ADR 0133: What crosses between the centre and the reflex lane: a fused digest slot, a reflex forecast the node issues, episode summaries, and one source per fact

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). Additive contract changes on the
  existing policy channel and cell report; no dependency, no paid resource,
  no new transport, no paper-boundary change.
- **Date**: 2026-10-18
- **Extends**: ADR 0088 (a slot is versioned and carries its own TTL), ADR 0100 (the cell
  report and the fabric), ADR 0116 (Lane 0 is the reflex pass), ADR 0128 (the
  spine carries policy down and deltas up).
- **Keeps**: ADR 0008 (cells decide alone), ADR 0024 (scrape posture for
  telemetry), `lane_placement.rs` (the node's closure excludes
  `qip-world-model`), ADR 0125 (advisory inputs never enlarge a size).

## Context

Six rows ask for something to cross the lane boundary, and each is PARTIAL on
"the contract does not exist":

- **R12-TICK-2086**: a distilled form of the fused state reaches regional
  reflex models. It exists centrally:
  `qip_kernel::fusion::OpportunityMap` is "the distilled fused state (2086):
  bounded, ranked, expiring", with `authorise_for(card, region)` refusing to
  ship it where the source model card's training entitlements do not permit
  (R12-TICK-2091). No policy slot carries it and no edge crate reads it.
- **R12-WORLD-3084, 7197**: calibrated distributions from microseconds, served
  by reflex/microstructure models (fill probability, queue movement, adverse
  selection, executable edge). The central lattice refuses the microsecond
  row by design (`qip-kernel/src/lattice_stage.rs`: "The microsecond row
  promotes only to the local reflex"), and `lane_placement.rs` forbids
  `qip-world-model` in the node's closure, so neither side can issue it.
- **R12-EXPAND-6038**: lessons propagate globally. The cell keeps
  `qip_contracts::regional_episode::RegionalEpisode`s locally; the
  `CellReport` the centre ingests (`qip-kernel/src/central/plane.rs`) carries
  none, so `Platform::learn_from_cells` learns from fills and refusals only.
- **R12-REFLEX-5034, 5047**: telemetry drained asynchronously to the regional
  fabric, and fills forwarded asynchronously to learning. Learning already
  takes cell fills through `Platform::ingest_cell_report` over the spine; the
  node's metrics are scraped at its health port.

Locate them with
`grep -n 'pub struct OpportunityMap\|pub fn authorise_for' backend/crates/runtime/qip-kernel/src/fusion.rs`,
`grep -n 'pub struct PolicyPayload' backend/crates/libs/qip-contracts/src/policy.rs`,
`grep -n 'pub struct RegionalEpisode' backend/crates/libs/qip-contracts/src/regional_episode.rs`,
`grep -n 'pub struct CellReport' backend/crates/runtime/qip-kernel/src/central/plane.rs` and
`grep -n 'pub fn learn_from_cells\|pub fn ingest_cell_report' backend/crates/runtime/qip-kernel/src/platform.rs`.

## Decision

1. **The fused digest is a policy slot** (R12-TICK-2086). The wire shape of
   `OpportunityMap` moves to `qip_contracts::policy` as `FusedDigest`
   (as-of, ranked entries with direction, score and expiry, the source
   digest), carried in a new additive `fused_digest: Slot<FusedDigest>` with
   the slot freshness rules every other slot has. The kernel builds it per
   cell and calls `authorise_for` with the cell's region before it enters the
   payload; a refusal leaves the slot unproduced and is journaled. A cell
   reads it as a model input, exactly as it reads `belief_priors` — never as
   a sizing authority, and under ADR 0125 an input from the centre may not
   enlarge a size the cell's own envelope and models did not reach.
2. **The microsecond forecast is the node's, under a contract in
   `qip-contracts`** (R12-WORLD-3084, 7197). A `ReflexForecast` contract —
   target (fill probability, queue movement, adverse selection, executable
   edge), horizon in microseconds, a calibrated distribution, issued-at and
   expires-at, and the model that issued it — lives in `qip-contracts`, which
   both lanes link. The node's reflex models issue it; `qip-world-model` stays
   out of the node's closure, and the central lattice keeps refusing the row.
   The centre scores reflex forecasts after the fact from the outcomes that
   reach it, so calibration is checked by a party that did not issue them.
3. **Episode summaries ride the cell report** (R12-EXPAND-6038).
   `CellReport` gains a bounded `episodes` list: the episodes recorded since
   the last report, capped per report and per kind, oldest dropped first with
   the drop counted, so a noisy cell cannot grow the report. The centre passes
   them to `learn_from_cells`. What propagates back down is a `ReflexDelta`:
   a signed change to a reflex package (ADR 0131), published two-person,
   never a raw episode sent to other cells.
4. **One source per fact** (R12-REFLEX-5034, 5047). Learning keeps taking
   fills from the cell report; the fabric's copy of the same fills is the
   ledger's input and the audit record, and learning does not also consume
   it, because two inputs for one fact disagree on timing and duplication and
   the learner would have to reconcile them. Telemetry stays scrape-only from
   the node's health port (ADR 0024); it is not a fabric record kind, because a
   metric drained through a durable log is a metric delayed and duplicated.
   The model acknowledgement the rows mention is already on the record:
   the cell journals `Decision::PolicyApplied` when it swaps in a verified
   payload, whose `trained_models` slot is the model the cell runs.

## Consequences

- The reflex lane gets the centre's distilled view and its own calibrated
  microsecond forecasts, and the centre learns from cell episodes, without a
  new transport and without the node linking the world model.
- REFLEX-5034 and 5047 are met by decision, not new code: the register should
  record the single-source rule as the design, and those rows' remaining work
  is a test pinning that learning reads the report and not the fabric.
- Code slices left: `FusedDigest`, the slot, the kernel's per-cell build with
  `authorise_for`, and an edge test that a stale or region-refused digest is
  not read (TICK-2086); `ReflexForecast` and a node issuer with a calibration
  test (WORLD-3084, 7197); the report's `episodes` with a bound test and a
  `qip-kernel/tests/central.rs` test that an ingested report's episodes reach
  `learn_from_cells` (EXPAND-6038).

## What it costs

- The policy payload grows a slot and the cell report a list; both are inside
  the signed or sequenced bytes and every version boundary must handle the
  absent case.
- A reflex forecast issued by the node is calibrated only after its outcomes
  arrive centrally; a badly calibrated reflex model trades on its forecasts
  until the next package. That is the same exposure the cell's models have
  today, made visible.
- Learning that ignores the fabric's copy of fills misses a fill the report
  dropped; the report's sequencing is what prevents that, and a hole in it is
  a reconciliation break the centre already counts.

## What would make this wrong

- If cells needed each other's episodes faster than a two-person package
  cycle can deliver (for example a venue outage learned in one region and
  needed in another within seconds), decision 3's ReflexDelta is too slow and
  a narrowing-only peer signal over the ADR 0116 link would be the next
  question.
- If the fabric's fill records ever became more complete than the report's
  (the report capped, the fabric not), decision 4's single source would be the
  wrong one, and learning should move to the fabric with the report dropped,
  not both.

## Rows

R12-TICK-2086, R12-WORLD-3084, R12-WORLD-7197, R12-EXPAND-6038,
R12-REFLEX-5034, R12-REFLEX-5047. Each stays PARTIAL until its slice lands with
a mutated test.
