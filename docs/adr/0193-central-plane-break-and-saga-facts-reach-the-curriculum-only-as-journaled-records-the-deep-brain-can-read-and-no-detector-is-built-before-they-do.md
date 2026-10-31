# 0193. Central-plane break and saga facts reach the curriculum only as journaled records the deep brain can read, and no detector is built before they do

- Status: accepted 2026-10-31. Number claimed by lane L4-quant on 2026-10-24 under the 2026-10-24 delegation; body written by lane L4b-expand under the owner's 2026-10-31 delegation, which moved the row to it
- Rows: R12-EXPAND-6066 (repeated vetoes, adverse selection, failed arbitrage sagas, reconciliation breaks and incorrect forecasts become explicit learning objectives; the trigger yields exactly one queued item naming its origin)
- Related: ADR 0128 (the mesh spine is the only cross-process path), ADR 0151 decision 12 (a detector with no honest input is not built), ADR 0062 (feasibility refusals as venue evidence)

## Context

R12-EXPAND-6066 names five triggers. Three are read by the deep brain's sweep
today: repeated vetoes (`detectors::repeated_vetoes_priced` over the kernel's
declined scores), refusal clusters (over feasibility refusals) and incorrect
forecasts (`low_confidence` over the calibration). Two are not: adverse
selection, and failed arbitrage sagas and reconciliation breaks.

The missing two have no input the sweep can read. Reconciliation breaks reach
`CentralPlane::record_halt` in `qip-kernel/src/central/plane.rs`, which
counts them on `qip_central_reconciliation_breaks_total` and keeps no history;
the plane runs in `qip-api`, fed by `mesh.rs`'s `ingest_cell_report`, while the
curriculum desk runs in `qip-deepbrain`. Multileg sagas live in
`qip-execution-engine/src/multileg.rs` and are not composed into the kernel.
Adverse selection is measured nowhere: the quote loop records fills and
cancels, not the price move after a fill. ADR 0128 admits one cross-process
path, the mesh spine, and a detector that read a count it cannot attribute
would raise one gap per sweep for as long as the counter stayed above zero.

## Decision

1. **The facts travel as journaled kernel records, in the process that owns
   them.** `CentralPlane::record_halt` will journal one bounded
   `ReconciliationBreakRecord` per break (direction, cell, order, quantities,
   cycle) on the kernel event log, under `kernel/reconciliation-break`, beside
   the counter it already moves; and the execution engine's saga, once it is
   composed into the kernel, journals one `SagaOutcomeRecord` per saga that did
   not complete (legs placed, legs filled, the leg that failed, the unwind).
   Both are bounded by the log's retention classes, not by a new buffer.
2. **The curriculum reads them off the log, in the process that holds the
   log.** The deep brain's sweep already decodes kernel records off the
   platform log (`Platform::knowledge_deltas`, ADR 0151); break and saga
   records are read the same way. Where the plane runs in `qip-api` and the
   sweep in `qip-deepbrain`, the records reach the deep brain only when the
   mesh spine carries them (ADR 0128). No second cross-process path is opened
   for them: a break frame is designed when the first consumer that needs one
   across processes is named, and until then the detectors run over the log
   the sweep can see, which is the deep brain's own.
3. **Each trigger yields exactly one queued item naming its origin, and the
   detectors are built only after the records exist.** `reconciliation_breaks`
   raises one `Execution` gap per cell with a break in the sweep, with the
   break record ids as evidence and `detector:reconciliation_break` as its
   origin; `failed_sagas` raises one `Execution` gap per venue with a failed
   saga. Adverse selection needs a measurement the platform does not take
   (the mark against the fill some interval later); it is named here as
   unmeasured and no detector is built on a guessed one (ADR 0151 decision 12
   applies).
4. **Order of work.** The records first (kernel), then the detectors
   (`qip-expansion`), then the sweep call (`qip-deepbrain`), each with a test
   that fails when its producer is removed. The row stays PARTIAL until the
   third lands and a mutation fires on the default path.

## Consequences

- The counter and the record agree by construction: both are written at
  `record_halt`, from the same break.
- The deep brain's sweep gains two triggers with real inputs once the records
  land, and gains nothing before then; a sweep that read the counter alone
  would have raised the same gap every cycle with nothing new to say.
- Adverse selection stays an unmeasured name in the register, which is what
  it is.

## What it costs

- A break record per break on the event log, in the observation retention
  class; a burst of breaks is bounded by the log's eviction, counted on
  `evicted_observations`.
- Two detectors and a sweep call that do not exist yet; this record decides
  their shape and does not deliver them.
- Nothing crosses the process boundary that did not already, so a break
  found by `qip-api`'s plane reaches `qip-deepbrain`'s curriculum only through
  the spine, when a frame for it is designed.

## What would make this wrong

A second brain that must act on a break within a cycle of its finding. That
needs a frame on the spine, not a log read, and this record would be amended
to name the frame and its consumer. Or a measurement of adverse selection
that the twin can compute from fills it already holds: then the third
detector is built on it rather than named as unmeasured.

## Code it constrains

`qip-kernel/src/central/plane.rs` (`record_halt`),
`qip-expansion/src/detectors.rs`, `qip-deepbrain/src/research.rs`,
`qip-execution-engine/src/multileg.rs` once composed.
