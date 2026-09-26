# Master scorecard

Updated at each rung of the worker ladder. Blueprint numbers are read from
[the matrix](../blueprint/traceability-matrix.md), never typed from memory.

## 2026-09-25 — after M2, before rung 1

```text
Blueprint requirements:        1,566 (1,559 applicable)
Complete:                      34      (2.2%)
Implemented:                   878     (56.3%)
Tested:                        739     (47.4%)
Integrated:                    733     (47.0%)
Deployable:                    404     (25.9%)
E2E working:                   23      (1.5%)

Build:                         PASS (cargo build, clippy 0 warnings, fmt clean)
Unit + integration tests:      6,284 passing / 0 failing   (workspace, 3b48f91b)
Acceptance suites:             447 passing / 0 failing     (34 binaries, 729c2665)
Frontend:                      portal lint/typecheck/build pass; 188 Playwright passing

Security (Semgrep, triaged):   0 critical, 0 high, 54 medium/low warnings (45 unpinned
                               actions, 9 unlogged buckets); 2 false-positive errors
INCORRECT rows (defects):      64

Critical path (M5):            0 / 10 steps
Active tasks:                  0        Blocked tasks: 0 (slice); deploy blocked (billing)
Agents:                        0 active

Token cost (sub-agents, M0–M3 workflows):
  discovery 12.9M · gap analysis 33.2M · design panel 2.8M · packet DAG 1.7M tokens
GCP compute:                   $0 (nothing provisioned; billing disabled)
```

## 2026-09-26: after rung 2 (wave 2 merged)

```text
Critical path (M5):            2 / 10 steps (SLICE-49, SLICE-06 merged)
Slice packets merged:          19 / 57     Defect packets merged: 3 / 14 (FIX-04, 08, 12)
Build:                         PASS (fmt exit 0; clippy --workspace -D warnings exit 0)
Workspace tests:               467 binaries, 6,363 passed, 0 failed, 0 ignored (2b0f33b9, clean target)
Rejections at review:          5 of 22 packets (every one found by the lead re-running mutations
                               or probing, never by the worker's own report)
Blueprint matrix:              not re-rendered this rung; the numbers above are from the gates
```

Two measurement faults found and fixed this rung, both recorded because each
made a gate report something other than what ran:
- A shared `CARGO_TARGET_DIR` across checkouts reused qip-acceptance binaries
  whose `CARGO_MANIFEST_DIR` pointed at another worktree. The source-scanning
  suites in gate `5c7930c0` read that worktree, not the integration tree.
  Every gate from `f99bf62a` on uses `backend/target/integration`, from the
  integration checkout only.
- A full disk killed the linker with SIGBUS (signal 7), which reads like a
  code fault and is not. Gates now build without debug info.

## Ladder

| Rung | Concurrency | Packets | Accepted | Rejected | Merge conflicts | Accepted lines | Output tokens | Lines / 1k tokens |
|---|---|---|---|---|---|---|---|---|
| 1 | 9 | wave 1 (SLICE-01, 02, 03, 04, 11, 49, 50, 51, 52) | 9 | 1 (SLICE-02: tests could not fail; repaired by the lead) | 0 | — | — | — |
| 2 | 4-8 | wave 2 (SLICE-05 to 10, 12, 13, 14, 26) + FIX-04, 08, 12 | 13 | 4 sent back (SLICE-06 torn-length misread, 08 coverage 4/30, 14 HLC regression untested, 26 kill-switch change untested) | 0 | — | — | — |
