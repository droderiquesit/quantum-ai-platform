---
name: implement-slice
description: Implement one vertical slice of work to the house standard, with tests. Use for the actual coding step of a planned task.
---

# Implement Slice

**Trigger** — A task with acceptance criteria and allowed paths.

**Outcome** — Working code, tests that would fail if it broke, and quoted evidence.

## Prerequisites

The task brief; the domain rule for the paths; the surrounding code read, not skimmed.

## Steps

1. Read the neighbouring code first — its comment register, naming, and test style. Match it. Code that reads as foreign is a maintenance cost even when correct.
2. Implement the smallest change that satisfies the criteria.
3. Write tests named as full sentences, each asserting its own premise before its conclusion.
4. Mutation-verify each test: break the code, confirm the test fails for the right reason, restore byte-for-byte, confirm it passes.
5. `cargo fmt --all`; `cargo clippy --workspace --all-targets`; `cargo test --workspace --no-fail-fast`.
6. `./scripts/check-dependencies.sh` and `./scripts/check-secrets.sh`.
7. Re-read your own diff adversarially before declaring it done.

## On failure

If a pre-existing test fails, that is evidence about your change until proven otherwise. Never weaken it. If it encodes genuinely wrong old behaviour, say which test, what it asserted, and why the new assertion is correct.

## Result format

What the platform now does or refuses that it did not; the gate output; the mutation report per test.

## Evidence

Real command output, quoted. Never report a step as done that you did not run.
A summary that omits a failure is a false statement about the system.
