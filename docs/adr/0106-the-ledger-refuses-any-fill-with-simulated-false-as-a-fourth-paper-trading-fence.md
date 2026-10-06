# ADR 0106: The ledger refuses any fill with simulated=false as a fourth paper-trading fence

- **Status**: Accepted, 2026-10-06, as part of M5 implementation (ADR 0100 §9).
- **Date**: 2026-10-06
- **Relates to**: ADR 0003 (paper trading by default), ADR 0021 (the blueprint expects live capital), ADR 0100 (decision §9, "Paper trading stays intact"), ADR 0001 (Rust everywhere, type system as enforcement).

## Context

The platform has three independent layers that enforce paper trading (ADR 0003, ADR 0100 §9):

1. **Terraform:** The infrastructure refuses `supervised_live`, `limited_autonomous_live`, and `autonomous_live` at plan time.
2. **Composition roots:** `AutonomyLevel::deployable` in `qip-api`, `qip-fastbrain`, and `qip-deepbrain` refuses live ceilings at start-up.
3. **Type system:** `qip-edge::Cell` has no constructor taking a ceiling other than `paper_trading`.

M5 adds a fourth fence at the ledger writer, the process that posts fills to durable balances. This fence is structural, like the type system, but operates in a different process, so a corruption or a bypass in one process cannot infect the ledger.

## Decision

**`qip-ledgerd` SHALL refuse to post any `Decision::Filled` record unless its `simulated` flag is `true`.** The refusal is:

1. **Detected at the deserialize stage.** As each record arrives from the broker, the ledger checks the `simulated` flag before any processing.
2. **Logged with full context.** The refusal is recorded in the event log with the original record's offset, timestamp, partition key, and a message naming why: `"Refusing simulated=false fill; paper trading boundary"`
3. **Metrics recorded.** A counter `qip_ledger_paper_boundary_refusals_total` increments, with a label for the reason (always `"simulated_false"`).
4. **Non-fatal.** The record is skipped; consumer offset advances; no exception is raised. The next record is processed. This prevents a single corrupted record from halting the ledger.

**If `simulated` is absent, the record is treated as `simulated=false` and refused.** This is the "fail closed" principle (CLAUDE.md principle 3): a missing or malformed flag defaults to live, which is refused.

**The refuse happens only at the ledger.** The broker does not check `simulated`. The edge node produces fills with `simulated=true` in the first vertical slice (ADR 0100 §8, tape → cell → fabric → ledger). The central plane is expected to consume fills from the fabric in a later vertical slice and will need its own fence.

## Rationale

**Defense in depth.** Three independent fences have already prevented a live order path. A fourth fence at the ledger makes it structurally impossible for a fill to reach the balance sheet if it was produced with live semantics, even if all three upstream fences were somehow bypassed.

**Witness at the boundary.** The ledger is the process that actually changes the balance sheet. If a fill reaches the ledger, it is the ledger's responsibility to check it one more time. This is the principle of never trusting the previous layer's validation.

**Operational clarity.** When a fill is refused at the ledger, the operator has full context: the offset in the broker, the timestamp, the partition key, the reason. This makes it possible to diagnose why a fill was produced with `simulated=false` (if it is a real breach) or to recognize that the fence is working as designed.

**Type system continuation.** The edge cell's type system (ADR 0100 §9, layer 3) ensures that only `simulated=true` fills are created. The ledger's type system (this fence) ensures that only records marked `simulated=true` are posted. The two together create an invariant that the balance sheet never reflects a live trade.

## Cost

**Minimal.** A boolean check on every fill is nanoseconds. The refusal is logged but not alarmed; the operator sees the counter and the log line if they look, but it does not page anybody unless the rate becomes pathological (which would indicate a real breach).

**One more table to define the contract.** The `Decision::Filled` struct gains a `simulated: bool` field. It is required, not optional, so the compiler catches any code that creates a fill without stating the value.

## Alternatives rejected

- **No ledger fence.** Relies on the three upstream fences being perfect. One bypass — in Terraform, in a composition root, or in the type system — would reach the balance sheet unguarded.
- **A single upstream fence (the strongest one of the three).** Each of the three upstream fences can be bypassed independently: Terraform plan is not always checked before apply, composition-root refusals happen at a single entry point, and the type system can be evaded with unsafe code (though denied here). No single fence is sufficient alone.
- **Panic on `simulated=false`** instead of refuse and skip. A panic stops the ledger, which is worse than skipping one record. The cell can still make decisions; the ledger just won't record them. Better to keep the ledger alive and alert the operator than to stop the system.
- **A permission check on the ledger's service account.** The ledger is deployed with `QIP_AUTONOMY_CEILING` set to `paper_trading` (ADR 0100 §9), so the permission check would be redundant. The `simulated` flag is the write-side equivalent: checking what you are allowed to do (permission) versus checking what you are doing (data-driven sanity check). Both are desirable.

## What would make this wrong

- **A `simulated=false` fill posted to the balance sheet.** Test: `a_live_fill_is_refused_at_the_ledger` runs the broker, creates a fill with `simulated=false`, sends it through the fabric, and asserts the ledger skips it and the balance is unchanged. This test must be separate from the happy path so it is mutation-verified independently.
- **A missing `simulated` field treated as `true` instead of refused.** Test: mutation the default in the refusal logic, confirm the test fails.
- **The refusal counter not incremented or the event log not written.** Test: assert the counter is nonzero and the log contains the refuse message.
- **The ledger panicking instead of continuing to the next record.** This would halt the ledger on a malformed or malicious input and is a denial-of-service vulnerability. Test: send 10 records in a batch, the third one malformed; assert the first, second, fourth and fifth are posted and the third is skipped.

## Relationship to the first vertical slice

The first vertical slice (ADR 0100 §8) runs with the simulated feed enabled (`QIP_VENUE_FEED=simulated`), so every fill has `simulated=true` by design. This fence does not fire in the slice and contributes only to the safety argument that a live fill cannot reach the balance sheet if a fence upstream is overcome.

In later slices (not part of M5), when the central plane produces fills and when real venues are added, this fence will be active and will refuse any fill that was not marked as simulated by the originating system.

Co-Authored-By: Claude Haiku 4.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01PQgAtHf7btZ8uy61HoLEvQ
