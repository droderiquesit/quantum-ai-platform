# ADR 0748: On memory storage the fast brain's log is bounded at a quarter of its container, and replays from a checkpoint, not from genesis

- **Status**: Proposed 2026-11-28, for the owner
- **Date**: 2026-11-28
- **Rows**: R12-WORLD-6015
- **Decision record it implements**: owner decision 3 in
  `docs/blueprint/owner-decisions-2026-11-28.md`

## Context

R12-WORLD-6015's reaction loop (`Platform::react_to_world_events`,
`granger::apply_reaction`) is built and tested at the fast brain's committed
100 ms cadence. Its one open ground was reach: on the committed `memory`
store, the fast brain's in-memory log grew past its 2 GiB Cloud Run container
(`infrastructure/terraform/catalogue.tf`, `fastbrain`, `memory = "2Gi"`) in
minutes, while the first material release on the committed seed arrives about
thirteen hours after start. A deployed node never met one.

Measured on this branch's base, on committed configuration, through
`node::step_with`:

- **Every journaled record is held twice.** The platform's `EventLog` and the
  kernel journal (`DurableLogTransport`, always in memory) each hold the same
  frame for every journaled topic. Over the first hundred cycles the journal's
  copy was 2.84 MB of the event log's 4.19 MB of record text (68 %).
- **Text understates memory.** The payload is a `serde_json::Value` tree. At
  1,000 cycles, unbounded, the record text came to under half the resident
  memory; an estimate that charges a `BTreeMap` node per object, capacities
  and allocator headers came to 1.11 GB against 1.18 GB resident.
- **The bytes per cycle rise, then level.** About 0.6 MB a cycle at cycle 500
  and about 2.4 MB at 2,000, once REASON starts convening agents
  (`agent.run_completed`, about 30 records a cycle) — for the two logs
  together.
- **Within one log nothing large repeats from cycle to cycle.** Compared field
  by field, consecutive cycles' records share only fields under half a
  kilobyte; the federation's repeats were already written as references by
  `JournaledReview` v2 (PR #297).

The duplicated payload is therefore the mirror itself. Removing it means the
journal stops being a second chain over the same frames, which changes what
every consumer of `Platform::journal()` and `replay_journal` reads. That is
not done here; see "What it costs".

## Decision

1. **Bytes, not records.** `EventLog` keeps an estimated in-memory footprint
   of every retained record (`qip_events::log::record_footprint`) and their
   sum (`EventLog::retained_bytes`). The estimate is calibrated against the
   resident memory of the committed fast brain and is stated as an estimate.
2. **The ceiling.** `Platform::bound_memory_log` states a ceiling over the
   event log and the kernel journal **together**, because both are in memory
   and both are what grew. It is refused for a platform whose event log is a
   file: a file is the durable record and is never released.
   `qip-fastbrain`'s `main.rs` calls it with
   `config::MEMORY_LOG_CEILING_BYTES` whenever its store is not durable. That
   constant is one quarter of `config::CONTAINER_MEMORY_BYTES`, 512 MiB of
   2 GiB. The binary cannot read `catalogue.tf`, so the container figure is
   a constant, and `qip-fastbrain/tests/memory_log.rs` reads `catalogue.tf`
   and fails when the two disagree.
3. **The release.** At the end of `run_cycle`, after the cycle's journal
   entry, when the two logs together exceed the ceiling, each releases its
   oldest records as one whole range from the head
   (`EventLog::release_through`) down to a quarter of the ceiling, so the two
   come back to half of it. A release never takes the newest record, never
   splits a cycle that has not finished, and is refused on a file-backed or
   inspected log.
4. **The checkpoint.** The platform then journals an `EventLogCheckpointed`
   record (topic `system.event_log_checkpointed`, System group, retained
   permanently) to both logs, carrying each released range — its first and
   last sequence, the last record's hash, its record count and bytes — and
   the ceiling and the bytes held when it ran.
5. **Verification from the checkpoint.** `EventLog::verify_from_checkpoint`
   holds the first retained record to the released range's last hash, which
   the log recorded when it released the range, and every later record as
   `verify_retained_chain` holds it. It is never weaker than that check.
   `EventLog::dropped` counts released records, so a replay re-anchoring on
   `retained_anchor` can account for them.
6. **Replay from the checkpoint.** `replay_journal` returns what is retained
   after the newest checkpoint. A federation review written as a reference to
   a pass inside the released range (up to 99 passes back) is rebuilt from
   the bases the checkpoint carries (`federation_bases`), which
   `federation_stage::replay_each` starts from. Every retained review
   replays; `qip-fastbrain/tests/memory_log.rs` asserts it.

**Said out loud: on memory storage the fast brain's log no longer replays
from genesis.** It replays from the newest checkpoint. That log never survived
a restart, and no durable record changes: the deep brain's committed storage
target is also `memory`, and a file-backed log is never released.

## What it costs

- **History in memory is about ten to twenty seconds.** At the levelled rate
  of about 2.4 MB a cycle for the two logs, 512 MiB holds a little over two
  hundred cycles, about twenty seconds at 100 ms, and a release leaves about
  half of that. Anything that reads the retained log reads only that:
  `Platform::heal_desk_gap` re-derives skipped desk fills from it, and a fill
  released before a heal is not re-derived; `journal_once`'s idempotency
  check forgets a key whose record was released, as it already did for an
  evicted one; `routings_scored`, `campaign_results` and the other replays
  over the log see only the retained span.
- **The mirror is not removed.** Every journaled record is still held twice.
  Removing the copy is a change to what `Platform::journal()` is, and it is
  left for a separate record. The ceiling counts both copies, so the bound
  holds either way; it costs half the history the ceiling could hold.
- **The ceiling bounds the log, not the process.** The other three quarters
  are the book, the federation, the models and the feed. This ADR does not
  bound them. The soak (`qip-fastbrain/examples/memory_log_soak.rs`) prints
  resident memory beside the log's bytes so that a reader can see both.
- **Cycle cost is not addressed.** In release, a cycle costs about 10 ms at
  1,000 cycles and about 90 ms by 6,000, most of it in LEARN
  (`falsification::Book::review`) and in REASON once agents convene, not in
  the log. A node at a 100 ms cadence is near saturation by then. That is a
  separate defect and this record does not claim to fix it.
- **One more topic.** `Topic::ALL` grows to 95.

## What would make this wrong

- **The estimate drifts from the heap.** If `record_footprint` reads
  materially below resident memory, the process holds more than the ceiling
  says. The soak prints both, and a reader should re-run it after any change
  to the shape of a journaled body.
- **A replay needs a record from the released range** that the checkpoint
  does not carry. The federation bases are carried because a retained review
  names them. A future reference-compacted encoding must add what it names to
  the checkpoint, or its retained records will not replay.
- **A durable fast-brain log is funded.** Then the right answer is the file
  log the deep brain and the API can use, archived and segmented, and this
  release would be removed rather than extended to it.
- **The container memory changes** without the constant. The test that reads
  `catalogue.tf` exists to make that a failure rather than a drift.
