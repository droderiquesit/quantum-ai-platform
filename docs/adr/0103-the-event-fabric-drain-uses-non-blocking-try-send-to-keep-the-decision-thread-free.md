# ADR 0103: The event fabric drain uses non-blocking try_send to keep the decision thread free

- **Status**: Accepted, 2026-10-06, as part of M5 implementation (ADR 0100 §6).
- **Date**: 2026-10-06
- **Relates to**: ADR 0100 (decision §6, "The hot path never waits"), ADR 0099, ADR 0003 (paper trading).

## Context

ADR 0100 decision §6 requires that "The decision thread hands a batch to a bounded channel with `try_send`, and that is all it does." This decision isolates the hot path from I/O and synchronisation delays, and is load-bearing for maintaining the cell's decision latency within the SLA.

The event fabric drain thread — the producer that pulls from the bounded channel and writes segment batches to disk — is subject to I/O jitter. If the decision thread had to wait for the drain to confirm receipt, a slow disk or a network buffer would propagate into the decision cycle. This would violate ADR 0008 ("edge cells decide alone") and ADR 0003 ("paper trading by default") by making the cell's operation depend on infrastructure outside its control.

## Decision

**The decision thread SHALL use `bounded_channel` and `try_send` exclusively.** It SHALL NOT use any blocking send, blocking receive, or any synchronisation primitive that waits for external acknowledgement. The semantics are:

1. **Success case**: A batch is queued successfully; `try_send` returns `Ok(())`.
2. **Backpressure case**: The bounded channel is at capacity; `try_send` returns `Err(TrySendError::Full(batch))`.
3. **Halt case**: Backpressure is treated as spool pressure — a fourth reading-style halt wire — and is applied in `Cell::work` before the next pass. The cell narrowing on backpressure happens once per pass; a pass without narrowing allows one more batch onto the ring.

**Invariants maintained:**
- No write locks are held by the decision thread beyond releasing a batch to the channel.
- No condition variable wait, no `Mutex::lock()` blocking, no `join()` on a drain thread.
- Backpressure acts as a control signal, not a stall: the cell continues to work, only narrowing scope in the next cycle.

## Rationale

**Latency isolation.** The decision thread's latency budget is fixed by the venue's tick rate (ADR 0100 §26.2). Every microsecond the drain spends in a blocking operation is latency the cell trades for correctness. Paper trading's margin is small and has no forgiveness.

**Composability with the paper boundary.** Paper layer 3 (ADR 0003) requires the decision thread to never wait on anything external. A blocked send violates this structurally and would require a fourth layer to wrap the channel. Non-blocking semantics let the boundary hold in code.

**Pressure signal as control, not as failure.** The cell knows what to do with spool pressure — it narrows — because the mirror already reads spool state every pass (ADR 0100, the seam where spool-pressure halt is applied). A stalled channel is legible as "spool is full", not as "something is wrong and we don't know what."

## What it costs

**Spool memory footprint.** Because the drain might not keep up with produce rate in the short term, the bounded channel must size for the worst case: peak produce rate multiplied by the maximum latency between drain wakeups. This is sized in configuration and proved in `a_spool_under_normal_load_returns_to_baseline` (ADR 0100 §8, test 8).

**Complexity in error handling.** A full channel is not an error — it is information — and the decision must treat it as such. Mishandling a full channel (by logging an error, by panic, or by silently dropping the batch) is a defect that must be caught by tests.

## Alternatives rejected

- **Blocking `send()` with a timeout.** A timeout is still a form of blocking, inherits the latency problem, and adds the question "what timeout is safe?" A TLA+ analysis of the spool and drain would be required to prove a timeout never fires on a well-tuned system; the non-blocking path avoids the question entirely.
- **A separate producer thread with backoff.** The producer would still be under the decision thread, blocking it while waiting for the producer to unblock. This moves the problem rather than solving it.
- **Draining to memory instead of disk.** The spool is on-disk by ADR 0100 decision §3 (producer-retained durability). The topology is fixed.

## What would make this wrong

- **The decision thread ever calls `.unwrap()` on a `try_send` result.** This hides a backpressure condition and will cause a panic under load, which is worse than a controlled halt.
- **Backpressure treated as an error rather than a control signal.** A log line "warn: channel full, dropping batch" is the shape of a silent correctness violation.
- **Spool returning to baseline but with a higher memory floor.** The test `a_spool_under_normal_load_returns_to_baseline` catches memory retention, and retention that only appears under peak load still violates this decision.
- **Joining or blocking on the drain thread from the decision thread.** This reintroduces the latency dependency this decision exists to prevent.

Co-Authored-By: Claude Haiku 4.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01PQgAtHf7btZ8uy61HoLEvQ
