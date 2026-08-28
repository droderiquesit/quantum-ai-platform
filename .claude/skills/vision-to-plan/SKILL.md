---
name: vision-to-plan
description: Turn a product outcome into a dependency-ordered task graph with acceptance criteria and owners. Use at the start of any multi-step piece of work, before implementation begins.
---

# Vision To Plan

**Trigger** — A stated outcome larger than one well-understood fix.

**Outcome** — A task graph each node of which names its owner, allowed paths, and the evidence that will close it.

## Prerequisites

`docs/product/VISION.md`; `docs/architecture/diagram-reconciliation.md`; a clean or knowingly-dirty working tree.

## Steps

1. Restate the outcome as something checkable. Show the restatement.
2. `git status` and read the reconciliation matrix. Establish what exists before planning what to add — prior audits go stale, so verify any claim you build on.
3. List assumptions, constraints and risks. Each assumption is documented and reversible.
4. Write acceptance criteria, each with the command or observation that settles it.
5. Decompose into tasks. For each: objective, owner agent, allowed paths, dependencies, evidence.
6. Check the path ownership is disjoint across anything that will run in parallel. Overlap here is the most expensive mistake in the whole loop — two agents editing one file destroys work that has to be redone.
7. Record the graph with TaskCreate.

## On failure

If the outcome cannot be made checkable, stop and ask. A plan against an unfalsifiable goal cannot be completed, only abandoned.

## Result format

The task graph, the acceptance criteria, the assumptions, and the first task to start.

## Evidence

Real command output, quoted. Never report a step as done that you did not run.
A summary that omits a failure is a false statement about the system.
