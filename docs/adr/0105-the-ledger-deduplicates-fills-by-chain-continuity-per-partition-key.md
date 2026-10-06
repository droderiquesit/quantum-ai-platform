# ADR 0105: The ledger deduplicates fills by chain continuity per partition key

- **Status**: Accepted, 2026-10-06, as part of M5 implementation (ADR 0100 §4).
- **Date**: 2026-10-06
- **Relates to**: ADR 0100 (decision §4, "Ordering, idempotency and fencing"), ADR 0007 (exact attribution), ADR 0089 (event anchoring).

## Context

The ledger consumes the P1 outcomes chain (fills, cancels, settlement records) from the event fabric broker. The broker guarantees per-partition order and fencing across epochs (ADR 0100 §4); it does not guarantee exactly-once delivery. A consumer that crashes and restarts must be able to detect which records it has already posted.

The naive approach — "if we've already seen this offset, skip it" — fails because broker loss can cause a segment to be re-appended. A ledger that tracks "offset 1000 has been seen" will skip legitimate re-sends after a crash.

The correct approach uses **chain continuity**: the ledger tracks the hash of the last record it posted and verifies that the next record's `previous_hash` matches. Only records that extend the chain (new watermark chain) are posted; duplicates have the same hash and are skipped.

## Decision

**The ledger maintains a durable chain tail per partition key.** The durable state is:

1. **Balances table.** Account balances (decimal).
2. **Partition tails table.** One row per partition key (e.g., `"cell:newyork-1:reflex"`) holding `(partition_key, last_hash, last_offset, created_at, updated_at)`.
3. **Consumer offsets table.** One row per consumer group holding the offset up to which fills have been posted.

All three are written in a single `WriteBatch` (ADR 0100 §4, decision 4) so that the ledger never reaches a state where balances and partition tails disagree.

**On each produce from the fabric:**

1. Read the partition tail for the incoming record's key.
2. If the record's `previous_hash` matches the stored tail, the record is new: post it, update balances, write new tail and offset in a single WriteBatch.
3. If the record's `previous_hash` does not match, the record is a duplicate (same epoch, same sequence): skip it, do not write.
4. If the record's `previous_hash` refers to a hash not yet in the tail, the chain is broken: record the break in the event log, do not post, and raise an alert.

**Chain break handling:**

A break means either:
- The ledger crashed and restarted mid-write, leaving a partial tail (application bug).
- The broker lost a segment and re-sent from an earlier epoch (broker bug or intentional recovery).
- A network partition caused the ledger to see records out of order (network bug).

The response is deterministic: do not post (no double-entry violation), and let the operator decide whether to manually advance the tail or re-sync the consumer offset. This is the same boundary-preservation strategy as ADR 0100 §2 (ledger writer is separate from API; keeps financial state independent).

## Rationale

**No replay of stable history.** Every time a record is posted, its hash is stored. A consumer that crashes and replays the broker's segment will produce the same records in the same order (watermark chain is deterministic); the hash will match and the post will be skipped. This holds even if the segment was re-appended by the broker after a loss.

**Idempotency without coordination.** The ledger does not need to talk to the broker to know if a record is new. It computes the watermark of each record as it arrives and compares to its own tail. This is local state and requires no external coordination.

**Single-writer invariant (ADR 0100 §2).** Only `qip-ledgerd` writes to the ledger's durable store. A second consumer of the same broker partition would maintain its own partition tails, so two ledgers would agree on the outcome only if they used the same deduplication rule. This decision enforces that rule structurally in code, not in documentation.

**Auditability (ADR 0007).** The partition tail is the record of what the ledger has accepted. A gap or a jump in offset is visible as a break in the tail hash, and a human can read the event log to find the cause.

## Cost

**Three durable tables instead of one.** A ledger that only tracked balances would be simpler, but would lose the chain continuity invariant. The extra tables are small (one row per partition key; the cell has a handful) and accessed on every post. The cost is negligible.

**Watermark computation per record.** Each incoming record triggers a hash computation (already done by the broker; the ledger re-verifies). Not a bottleneck at sub-millisecond fill rates.

## Alternatives rejected

- **Offset-only deduplication.** "If offset <= consumed_offset, skip" fails on broker loss because the broker can re-send an old offset with a new segment. Double-entry happens.
- **Sequence number in the Decision struct.** ADR 0100 §4 says sequences are assigned at drain time and are not globally unique; a resend of the same decision to a different broker instance would carry the same sequence but a different context. Chain continuity is the only source-of-truth witness.
- **Timestamp-based deduplication.** "If timestamp <= last_timestamp, skip" fails because the broker can re-seal old records and ship them in a new segment. Ordering of timestamps is not ordering of causality; watermarks are.
- **A consensus protocol or a quorum read.** The ledger is the single writer by design. Asking a quorum whether a record has been seen reintroduces the distributed coordination this design exists to avoid (ADR 0100 §3).

## What would make this wrong

- **A record posted twice into the same account.** Test: `a_duplicate_fill_is_posted_only_once` (ADR 0100 §8, test 4) runs a fill, simulates a fabric outage, replays the segment, and asserts balances unchanged.
- **A record skipped that should have been posted.** This is harder to test directly (requires a broker sending two different records with the same offset), but the chain-break alert would fire. Test: the alert is raised when previous_hash cannot be resolved.
- **A partition tail that advances without a corresponding balance change.** The WriteBatch commit is atomic; a crash mid-write leaves the tail in a consistent state with balances. Test: `the_ledger_and_its_chain_tails_remain_in_sync_after_every_crash`.
- **The tail persisted incorrectly (wrong hash, wrong offset, or both).** Test: mutation the tail computation, confirm the verification fails for a subsequent record.

Co-Authored-By: Claude Haiku 4.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01PQgAtHf7btZ8uy61HoLEvQ
