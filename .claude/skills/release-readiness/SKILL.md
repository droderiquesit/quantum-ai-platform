---
name: release-readiness
description: Assess whether a change is ready to deploy, gate by gate. Use before any deployment and when asked whether something can ship.
---

# Release Readiness

**Trigger** — A candidate for deployment.

**Outcome** — A gate-by-gate verdict with evidence, and a rollback plan.

## Prerequisites

A green suite; the diff; the target environment.

## Steps

1. Walk the Definition of Done table in `.claude/rules/02-change-management.md`. For each gate: run it, quote the output, or state why it does not apply.
2. Check the deploy path: does CI pass on this exact commit?
3. Check the target environment's tfvars: is the autonomy ceiling paper trading?
4. Confirm images are signed and attested, and upstream images pinned by digest.
5. State the rollback: what command, how long, and what it restores.
6. Name every gate you skipped and why. A skipped gate that is not named is a false readiness claim.

## On failure

Any failing gate means not ready. Say which. Do not average gates into an overall impression.

## Result format

Per gate: ran / passed / skipped-with-reason, and the evidence. Then a single ready or not-ready, and the rollback plan.

## Evidence

Real command output, quoted. Never report a step as done that you did not run.
A summary that omits a failure is a false statement about the system.
