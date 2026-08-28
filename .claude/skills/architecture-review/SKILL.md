---
name: architecture-review
description: Decide whether a proposed change fits the architecture and whether it needs an ADR. Use before implementing anything crossing a crate boundary, adding a dependency, or changing a contract.
---

# Architecture Review

**Trigger** — A design that touches more than one crate, adds a dependency, or changes a public contract.

**Outcome** — An accept/revise decision with reasons, and an ADR where the decision is consequential.

## Prerequisites

The proposal; `docs/adr/`; `.claude/rules/architecture/`.

## Steps

1. Read the ADRs that already cover the area. Several decisions here are settled and reopening one needs an ADR, not an argument.
2. Check the dependency direction: libs ← services ← runtime ← apps. A violation is a rejection, not a note.
3. Check for a new dependency. Two crates are permitted; anything else needs an ADR first.
4. Check the change does not create a second source of truth for a fact already recorded.
5. Ask whether the guarantee can be structural rather than checked at runtime.
6. Decide. Name at least one rejected alternative and why.
7. Draft the ADR if the decision is consequential.

## On failure

If the change requires a settled decision to be reopened, stop and put the question to the user with the ADR that settles it.

## Result format

Decision, reasons, rejected alternatives, ADR number if one is needed.

## Evidence

Real command output, quoted. Never report a step as done that you did not run.
A summary that omits a failure is a false statement about the system.
