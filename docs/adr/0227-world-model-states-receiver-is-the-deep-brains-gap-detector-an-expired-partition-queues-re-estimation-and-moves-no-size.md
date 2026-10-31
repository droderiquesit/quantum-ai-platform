# ADR 0227: WorldModelState's receiver is the deep brain's gap detector; an expired partition queues re-estimation and moves no size

- **Status**: **Accepted 2026-10-31 under the owner's delegation**. This is
  decision 10 of `docs/blueprint/lanes/wave1031-owner-rulings-2026-10-31.json`,
  relayed by the integration parent, which assigned R12-CONTRACT-7075 to
  lane L2-INTEL. The record sets no risk number and no threshold, and it
  touches no paper-trading layer.
- **Date**: 2026-10-31
- **Extends**: the NOW stage's projection of a §27 `WorldModelState` per
  moved partition (`backend/crates/runtime/qip-kernel/src/now_stage.rs`).
  It also extends the deep brain's model-drift gap
  (`qip-expansion/src/detectors.rs::model_drift`, R12-EXPAND-6046).
- **Rows**: R12-CONTRACT-7075.

## Context

R12-CONTRACT-7075 asks for a `WorldModelState` that a second brain can
read. The NOW stage already journals one for every partition a pass moved a
belief in, on `world_model.updated`, inside each `NowReview`. Nothing read
it. Each state carries an `expires_at`, the instant after which it no longer
describes its partition. The platform had no consumer that noticed when that
instant passed with no successor, so a partition whose model had stopped
being refreshed looked, to the curriculum, the same as one that was still
described.

## Decision

1. **The receiver is the deep brain's gap detector.**
   `ResearchLoop::expired_world_states` runs every sweep. It reads the NOW
   reviews the platform journaled since the last sweep, through
   `qip_kernel::now_stage::reviews`. It re-admits each state through
   `WorldModelState::new` and keeps the newest per partition scope. A state
   that does not re-admit is skipped, not believed.
2. **An expired partition raises a model-drift gap, once.** A partition
   gets one `GapTrigger::ModelDrift` observation
   (`qip_expansion::detectors::world_state_expired`, evidence
   `world_state:<scope>` and `model:<model>`) when two things hold: its
   newest state's `expires_at` has passed, and no newer state has arrived.
   The expiry is reported once. A newer state clears it. An expired state
   is a model that stopped describing its data, so it has the same want as
   a drifted one, re-estimation, and it raises the same trigger.
3. **The state never moves a size and never chooses an action.** The
   receiver reads the platform's log and nothing else. Its only output is
   an observation, and the sweep turns that into queued research.
4. **One reader for the review.** The platform journals a review as a
   sealed stream envelope, so decoding the log record directly reads the
   envelope's fields as the review's and fails. A reader outside the kernel
   that did so would silently see no review at all. The first draft of this
   receiver made exactly that mistake, and its own test caught it.
   `now_stage::reviews` is therefore the one way to read them.

## Consequences

- `qip-deepbrain/tests/research.rs::a_partition_whose_world_state_expired_queues_a_model_drift_gap`
  runs a platform whose NOW stage journals a state. A sweep before the
  state's expiry raises no gap for the partition. A sweep after it raises
  exactly one, and the next sweep raises none. Dropping the read fails the
  test, and so does never marking an expiry reported.
  `a_newer_world_state_clears_the_expiry_of_the_one_it_replaced` sweeps
  between cycles. A partition refreshed before its state expires raises no
  gap at the old expiry and one at the successor's. Never replacing a held
  state fails it.
- The working set is bounded at `MAX_WORLD_STATE_PARTITIONS`, and each
  review is read once, by cycle cursor.

## What it costs

- Every sweep walks the platform's `world_model.updated` records to find
  the new ones. The cursor skips reviews it has read, but finding them is
  linear in the log.
- A partition the NOW stage moves rarely will expire and raise a gap even
  when nothing about its model is wrong, only quiet. That is a curriculum
  item nobody needed. It is bounded at one per expiry, and it is the
  conservative error: asking for a re-estimate that was not needed costs
  research time, not capital.
- The cursor and the newest-state table live in memory. After a restart,
  the first sweep re-reads every review and can report an expiry it
  reported before the restart once more. The cursor is the log sequence and
  not the review's cycle, because a platform restarted over an inherited
  log numbers its cycles from zero again. A cycle cursor would skip every
  review the new run wrote until it caught up, and the first draft had
  exactly that bug.

## What would make this wrong

A partition whose state is meant to outlive its `expires_at`, for example a
slow-moving structural model whose projection is deliberately stamped with a
short expiry. Every such expiry would then be a false gap, and the fix
belongs in the projection's expiry, not in this receiver.
