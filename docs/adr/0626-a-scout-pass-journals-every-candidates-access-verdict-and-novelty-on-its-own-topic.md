# ADR 0626: A scout pass journals every candidate's access verdict and novelty on its own topic

- **Status**: Accepted, 2026-11-15 (wave b52, lane 13).
- **Date**: 2026-11-15
- **Rows**: R12-DATA-1103, R12-DATA-1102.
- **Builds on**: ADR 0180 decision 2 (every first-pass access-rights verdict is
  journaled on the source topic). This record fixes the shape that decision
  left open; it changes no decision in it.

## Context

`Platform::assess_sources` wrote one record, `SourceLifecycleChanged`, and only
when policy moved a registration that already stood. A first-seen source
refused for its publisher's robots rules or its licence moved nothing, so the
platform's answer to "may this be read?" lived in the returned decisions and
nowhere durable. The novelty the finder scores (the uniqueness term of
`SourceScores`) was never recorded for any candidate.

## Decision

1. **A new topic, `SourceScouted`** (`learning.source_scouted`), in the learn
   group, retained as `Episodic` beside `SourceLifecycleChanged`. Not a second
   body on the lifecycle topic: every reader of that topic decodes every frame
   as a transition, and the existing kernel suite does exactly that.
2. **One frame per pass**, not per candidate. It lists, in the finder's own id
   order, each candidate's `first_pass` flag (absent from the registry when the
   pass began, read before the finder registers anything), its access verdict
   (host, robots, licensing, overall, and a `permitted` boolean) and its
   novelty (the uniqueness score in `[0, 1]`).
3. **A disabled check records `None`, never a default.** `ScoutChecks`
   switches each of the two findings off per pass; `assess_sources` uses both
   on, `assess_sources_scouted` takes the choice. A recorded "permitted" for a
   pass that never asked would be a licence nobody read, and `None` is what
   makes disabling a check change the recorded outcome (the row's check).
4. **Raised on a journal refusal**, as the lifecycle record is: the finder has
   decided, and a verdict the log does not hold is the gap being closed.

## Not decided here

The discovery cadence stays off in committed environments (ADR 0180, ADR 0157):
scout egress is unbuilt and billed. This record makes a pass that does run
leave its findings on the log; it does not make one run by default, so the
rows' default-path half remains an owner decision.

## What it costs

`Topic::ALL` grows by one, so every exhaustive match over topics and the
retention tests must list `SourceScouted`. Each scout pass that runs adds one
`Episodic` frame carrying a verdict per candidate, so the frame grows with the
candidate count and its storage and replay cost are unmeasured. A journal
refusal of the scout frame raises out of `assess_sources`, as a refusal of the
lifecycle record already does, so there is a second write on which a pass can
fail after the finder has already decided. The committed
environments still run no discovery cadence, so the record is empty there until
an owner turns one on. No dependency, no live-order path, no change to the
paper-trading layers.

## What would make this wrong

A reader needs the verdict per candidate rather than per pass, for instance a
query by single source id that must decode and scan whole-pass frames, and that
scan proves too slow or too large. A pass grows to a candidate count at which
its single frame, and the idempotency key that joins every candidate id, is
larger than the journal or its readers can carry; no bound on either is set
here. A consumer treats a recorded `permitted` as current when it only states
the verdict at that pass, and acts on a stale one. Or the lifecycle and scout
topics disagree about one source and an operator cannot say which is
authoritative.
