# Test baseline — 2026-09-25

The M0 test baseline for the ADR 0099 programme. Every number below was taken
from a run whose output was read.

| Gate | Command | Commit | Result |
|---|---|---|---|
| Rust tests, whole workspace | `cargo test --workspace --no-fail-fast` | `5259b637` (= `main` `53fc1f42` + docs) | exit 0; 448 `test result:` lines; **6,276 passed, 0 failed, 0 ignored** |
| The same, after the blueprint renderer landed | same, in a clean worktree | `3b48f91b` | exit 0; 449 binaries; **6,284 passed, 0 failed** (+8 new: 5 renderer tests, 2 unit, 1 acceptance) |
| Acceptance suites | `cargo test -p qip-acceptance --no-fail-fast` | `d864dab8` | 34 binaries, **447 passed, 0 failed** |
| Lint | `cargo clippy --workspace --all-targets` | `5259b637` | `Finished`, exit 0, **0 warnings** |
| Format | `cargo fmt --all --check` | each commit | clean |
| Portal lint / typecheck | `npm run lint`, `npm run typecheck` in `frontend/` | `5259b637` | no errors printed |
| Portal build | `npm run build` in `frontend/` | `5259b637` | built; all routes emitted |
| Landing build | `npm ci && npm run build` in `frontend/landing` | `5259b637` | built |
| Design tokens | `npm run tokens:check` | `5259b637` | `globals.css matches packages/design-tokens (26 tokens per theme)` |
| Portal behaviour | `npx playwright test` in `frontend/portal` | `3b48f91b` | **188 passed** (1.9 min), exit 0 |
| Hooks | `python3 .claude/hooks/test_hooks.py` | `5259b637` | `all hook tests pass (0 failures)` |
| CI on PR #17 | GitHub Actions, 14 jobs × 2 runs | `5259b637` | **28 pass, 0 fail** |

## Not run, and why

- **Landing Playwright (13 tests).** CI's `frontend-landing` job runs it on
  every PR. It was not rerun locally.
- **Load and soak testing against a running system.** Nothing runs anywhere
  (see [the gap analysis](gap-analysis.md)). `performance.rs` and `stress.rs`
  run in-process only, inside the acceptance totals above.

## What the numbers do not say

The suite is large and green. That measures what the code does *in tests*,
not what the platform does. The v11.6 matrix shows 47.4% of requirements
tested and 1.5% demonstrated end to end, and no process of the platform has
been observed running in any environment. A passing test on a capability
that no deployed process reaches is evidence about the code, not about the
platform.
