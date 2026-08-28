---
name: test-change
description: Verify that a change is actually covered, and prove each test fires. Use after implementation and whenever coverage is claimed.
---

# Test Change

**Trigger** — A change whose tests have not been independently verified.

**Outcome** — A mutation report proving each test guards what it claims.

## Prerequisites

The diff; the test files; `.claude/rules/architecture/01-testing-strategy.md`.

## Steps

1. Identify the behaviours the change introduced or altered.
2. For each, find the test. A behaviour with no test is a gap — report it.
3. For each test: break the implementation in the specific way the test exists to catch, run it, confirm it fails **and that the failure message is the right one**.
4. Restore byte-for-byte. `git diff` must be empty for the mutated file.
5. Re-run and confirm green.
6. Check for the substring trap: any assertion matching a token with a longer neighbour must match the delimited form.

## On failure

A mutation that does not fire means the test does not guard the behaviour. Fix the test; do not report the mutation as inconclusive.

## Result format

Per test: the mutation, whether it fired, and the restore confirmation. Plus totals from `--no-fail-fast`.

## Evidence

Real command output, quoted. Never report a step as done that you did not run.
A summary that omits a failure is a false statement about the system.
