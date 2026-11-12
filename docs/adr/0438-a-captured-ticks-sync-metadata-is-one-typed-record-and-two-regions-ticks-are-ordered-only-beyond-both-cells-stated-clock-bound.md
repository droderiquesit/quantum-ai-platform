# ADR 0438: A captured tick's sync metadata is one typed record, and two regions' ticks are ordered only beyond both cells' stated clock bound

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11**, session t50-06. An in-repository design
  decision. It adds no dependency, no live path and no limit, and touches no
  paper-trading layer. It states no clock figure: the one figure the decision
  needs is left to whoever measures it.
- **Date**: 2026-11-12
- **Rows**: R12-TICK-2048.

## Context

R12-TICK-2048 reads: "Captured events carry cross-asset and cross-region
synchronisation metadata so reactions can be learned across instruments and
venues." Its check is inspection: each named event type has a typed record
and a capture path.

On 2026-10-25 an independent check refuted the row. It found two faults, and
both were real:

- The edge node's lake line did carry `region`, `cell`, `clock_source` and
  `cross_partition`. But they were fields of an ad hoc `serde_json` object
  built in `qip-edge-node`'s drain. Nothing typed them, and
  `qip_storage::lake_history` dropped all four. A line claiming that another
  venue's tick came `before`, when the lag sat inside both clocks' error,
  was a line a lead/lag model would learn from.
- The alignment related partitions within one cell only. Lines from two
  regions met nowhere. The only cross-region order a reader had was a sort
  on two cells' timestamp columns, and that sort learns the inter-cell clock
  error as a reaction.

## Decision

1. **One typed record.** `qip_contracts::sync::SyncMetadata` holds the
   region and cell that captured the tick, a `ClockSource`, the asset key
   (venue, feed, partition, instrument) and a `CrossPartition`, which is the
   alignment against every other partition the cell holds, or the
   discipline's refusal. The drain builds it once per tick and writes its
   fields at the top level of the line. These are the keys the lake already
   held, plus `venue` and `instrument`.
2. **Decoding re-runs the refusals.** The record has no derived decoder. It
   refuses an unnameable region or cell, a blank asset key, and a clock
   source that disagrees with the alignment (an unplaced tick with
   alignments, or a placed tick with a refusal). It also refuses more than
   `MAX_SYNC_ALIGNMENTS` alignments, an alignment against the tick's own
   partition, a negative doubt, and an order that the lag and its doubt do
   not support. The lake reader decodes the record onto
   `RecordedTick::sync`. A line that carries the asset key and fails the
   record is counted unparsed. It is not read as a plain tick.
3. **Across cells, each tick is widened by its cell's clock bound.** The
   tick desk is where lake lines from every cell meet.
   `qip_sequencing::CrossRegionAligner` relates each tick there to the
   latest tick of every partition that another cell captured. It adds the
   cell clock bound to each side, so twice over a pair, and states an order
   only beyond that widened bound. It reports no arrival order across
   cells, because two cells' arrivals are on two clocks. Its working set
   holds at most `MAX_REGION_PARTITIONS` keys.
4. **The bound is stated, never assumed.** The deep brain reads it from
   `QIP_DEEPBRAIN_TICK_CELL_CLOCK_BOUND_NS`. No one has stated a figure for
   how closely a cell's clock holds the reference, so the variable has no
   default. When it is unset, a round computes no cross-region order and
   records why in `cross_region_withheld`. When it is set, the round reports
   counts (`CrossRegionSummary`), not the pairs themselves.

## Consequences

- A forged or corrupted order on a lake line no longer reaches a reader.
  Tests in `qip-contracts/tests/sync_metadata.rs` and
  `qip-edge-node/tests/lake_clock.rs` prove this.
- Lines written before the asset key existed have no `instrument` key and
  read back with `sync: None`. They are not stranded and are not refused.
- Cross-region ordering stays withheld until an operator states a measured
  bound. It does not run on an invented figure. No execution node is
  deployed (`execution_nodes_enabled = false` in dev), so no deployed
  process captures yet. The committed capture path is the drain, behind
  `QIP_VENUE_FEED=simulated` in the execution node's template.

## What it costs

- Every lake line carries two more fields (`venue`, `instrument`), and the
  drain keeps them for each partition's latest tick in its bounded working
  set.
- The lake reader decodes and validates the record on every line it reads.
  A record that fails is one fewer tick a replay sees, and it is counted.
- The deep brain links `qip-sequencing`, a workspace-internal edge library
  with no I/O, so the cross-cell rule and the in-cell rule run the same
  `align`.
- Until someone states a measured bound, the desk learns no cross-region
  reaction. That is the intended cost of not inventing the figure.

## What would make this wrong

A measured inter-cell clock bound that varies by cell would make one shared
figure too coarse. In that case the bound should be carried per region on
the record, not set once for the desk.
