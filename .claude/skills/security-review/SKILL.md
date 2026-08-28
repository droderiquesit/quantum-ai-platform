---
name: security-review
description: Independently review a change for security and safety, including the paper-trading boundary. Use for anything touching risk, execution, credentials, infrastructure, or authentication.
---

# Security Review

**Trigger** — A diff touching a consequential path, or a request for sign-off.

**Outcome** — Findings with concrete failure scenarios, and an explicit statement about the paper boundary.

## Prerequisites

The diff; `.claude/rules/01-security-and-safety.md`. **You must not have written the code.**

## Steps

1. Confirm you did not implement this. If you did, hand it to someone else.
2. Check all three paper-trading layers are intact: the Terraform validation, `AutonomyLevel::deployable`, and the type-level constraints. Name each.
3. Look for credential material in the diff — code, comments, fixtures, test names, examples.
4. Check every new external call for a timeout and an allowlisted destination.
5. Check every new control actually fires. A limit that cannot trigger is worse than none.
6. Ask what happens when each flag is set the wrong way.
7. `./scripts/check-secrets.sh`; the `security.rs` and `compliance_proof.rs` suites.

## On failure

If you cannot establish that the paper boundary holds, say so and stop. Do not sign off conditionally.

## Result format

Findings most-severe first, each with a specific failure scenario; the paper-boundary statement; what you checked and what you did not.

## Evidence

Real command output, quoted. Never report a step as done that you did not run.
A summary that omits a failure is a false statement about the system.
