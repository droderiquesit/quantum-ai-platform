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
3. **Counted.** The refusal increments `qip_ledger_live_fill_refused_total` (`LEDGER_LIVE_FILL_REFUSED` in `qip-observability`, recorded from `LedgerStore` through `LedgerTelemetry::live_fill_refused`). *Correction, 2026-10-06:* this item named a counter `qip_ledger_paper_boundary_refusals_total` with a `reason` label. Nothing records that series and nothing ever has; it is not emitted, and a dashboard or alert written against it would read a permanent zero as "no live fill has been seen".
4. **Non-fatal to the process, fatal to the cell's stream.** No posting is written and the ledger does not panic. The refusing cell is parked under `ParkReason::LiveFill`, which holds until an operator releases it (*correction, 2026-10-06:* this item said the record was skipped and the next one processed; the store parks instead, so a live fill stops that cell's stream rather than passing silently).

**If `simulated` is absent, the record is treated as `simulated=false` and refused.** This is the "fail closed" principle (CLAUDE.md principle 3): a missing or malformed flag defaults to live, which is refused.

**The refuse happens only at the ledger.** The broker does not check `simulated`. The edge node produces fills with `simulated=true` in the first vertical slice (ADR 0100 §8, tape → cell → fabric → ledger). The central plane is expected to consume fills from the fabric in a later vertical slice and will need its own fence.

## Rationale

**Defense in depth.** Three independent fences have already prevented a live order path. A fourth fence at the ledger refuses any fill *marked* `simulated=false` before it reaches the balance sheet, even if all three upstream fences were somehow bypassed. Its scope is the flag, not the money: see "Relationship to the first vertical slice" for what it does not catch.

**Witness at the boundary.** The ledger is the process that actually changes the balance sheet. If a fill reaches the ledger, it is the ledger's responsibility to check it one more time. This is the principle of never trusting the previous layer's validation.

**Operational clarity.** When a fill is refused at the ledger, the operator has full context: the offset in the broker, the timestamp, the partition key, the reason. This makes it possible to diagnose why a fill was produced with `simulated=false` (if it is a real breach) or to recognize that the fence is working as designed.

**Type system continuation.** The edge cell's type system (ADR 0100 §9, layer 3) holds the cell's ceiling at paper. The ledger's check (this fence) ensures that only records marked `simulated=true` are posted. Together they guarantee that the balance sheet never reflects a fill its producer *reported* as live — not that it never reflects a trade that was live, because the flag is a claim the producer makes about itself.

## What it costs

**Minimal.** A boolean check on every fill is nanoseconds. The refusal is counted on `qip_ledger_live_fill_refused_total` and parks the cell; no alert policy reads that counter, so it pages nobody until one is written.

**One more table to define the contract.** The `Decision::Filled` struct gains a `simulated: bool` field. It is required, not optional, so the compiler catches any code that creates a fill without stating the value.

## Alternatives rejected

- **No ledger fence.** Relies on the three upstream fences being perfect. One bypass — in Terraform, in a composition root, or in the type system — would reach the balance sheet unguarded.
- **A single upstream fence (the strongest one of the three).** Each of the three upstream fences can be bypassed independently: Terraform plan is not always checked before apply, composition-root refusals happen at a single entry point, and the type system can be evaded with unsafe code (though denied here). No single fence is sufficient alone.
- **Panic on `simulated=false`** instead of refuse and skip. A panic stops the ledger, which is worse than skipping one record. The cell can still make decisions; the ledger just won't record them. Better to keep the ledger alive and alert the operator than to stop the system.
- **A permission check on the ledger's service account.** The ledger is deployed with `QIP_AUTONOMY_CEILING` set to `paper_trading` (ADR 0100 §9), so the permission check would be redundant. The `simulated` flag is the write-side equivalent: checking what you are allowed to do (permission) versus checking what you are doing (data-driven sanity check). Both are desirable.

## What would make this wrong

- **A `simulated=false` fill posted to the balance sheet.** Test: `a_live_fill_is_refused_at_the_ledger` runs the broker, creates a fill with `simulated=false`, sends it through the fabric, and asserts the ledger skips it and the balance is unchanged. This test must be separate from the happy path so it is mutation-verified independently.
- **A missing `simulated` field treated as `true` instead of refused.** Test: mutation the default in the refusal logic, confirm the test fails.
- **The refusal counter not incremented.** Test: assert `qip_ledger_live_fill_refused_total` is nonzero after a refused fill (`qip-ledgerd/src/telemetry.rs` tests), and that the cell is parked under `ParkReason::LiveFill` (`qip-ledgerd/tests/store.rs`).
- **The ledger panicking instead of continuing to the next record.** This would halt the ledger on a malformed or malicious input and is a denial-of-service vulnerability. Test: send 10 records in a batch, the third one malformed; assert the first, second, fourth and fifth are posted and the third is skipped.

## Relationship to the first vertical slice

The first vertical slice (ADR 0100 §8) runs with the simulated feed enabled (`QIP_VENUE_FEED=simulated`), so every fill has `simulated=true` by design. This fence does not fire in the slice.

**What the fence does and does not cover** (*corrected 2026-10-06*; this paragraph said the fence meant "a live fill cannot reach the balance sheet"). It refuses fills **marked** non-simulated. It cannot see where an order actually went. A provider-sandbox adapter (`RestGateway`) pointed at a production host would place real orders and still report `simulated=true` — `qip-brokers` has no live adapter class, so every adapter reports paper — and its fills would pass this fence. What keeps that adapter's fills out of the ledger today is the pass-path restriction in `qip-edge-node`: `run_pass` takes only `&mut SimulatedGateway`, start-up refuses a simulated feed on any other gateway (`NodeGateway::simulated_for_feed`), and the pass loop narrows with `NodeGateway::simulated_mut`, which is `None` for the REST variant. If that restriction is ever relaxed, this fence is not a substitute for it.

When the central plane produces fills in a later slice, this fence refuses any fill its producer did not mark simulated. It is not a step towards live venues: paper trading is not a phase (ADR 0003), and a live path requires an accepted ADR (the proposed ADR 0107).

Co-Authored-By: Claude Haiku 4.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01PQgAtHf7btZ8uy61HoLEvQ
