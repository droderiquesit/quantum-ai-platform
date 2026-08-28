---
name: documentation-update
description: Bring documentation back into agreement with the code. Use after behaviour changes and whenever a document may have gone stale.
---

# Documentation Update

**Trigger** — A behaviour change, or a document suspected of describing something that no longer exists.

**Outcome** — Documents that match the code, verified by the documentation suite.

## Prerequisites

The change; the affected documents.

## Steps

1. Find every document that describes the changed behaviour — including header comments in workflows and manifests, which go stale invisibly.
2. Correct them to what the code does. A comment describing configuration a maintainer must perform, when that configuration no longer exists, is worse than no comment: it sends them to set values nothing reads.
3. Check for overclaims. `documentation.rs` refuses 'production ready', 'battle tested', and 'quantum advantage over' — and is right to. State what was measured, not what would be nice to conclude.
4. Check the reconciliation matrix if component status changed.
5. `cargo test -p qip-acceptance --test documentation`.

## On failure

If a document describes something you cannot verify, mark it explicitly as unverified rather than deleting it or asserting it.

## Result format

Documents changed, claims corrected, and the documentation suite result.

## Evidence

Real command output, quoted. Never report a step as done that you did not run.
A summary that omits a failure is a false statement about the system.
