# ADR 0460: The deep brain triages each incident by what it touched, and re-runs every drafted replay fixture as an autonomous test loop

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation**,
  session t50-17 (wave580). An in-repository design decision: it adds a
  pure triage function and two counters to `qip-observability`, and a
  severity, an owner and a periodic fixture re-run to the deep brain's
  operations loop. It adds no remediation action, widens no runbook, adds no
  dependency, introduces no figure the owner did not state, and touches no
  paper-trading layer.
- **Date**: 2026-11-12
- **Rows**: R12-OBS-1172 (stays PARTIAL; see "What this leaves open").

## Context

R12-OBS-1172 asks for Operations/AIOps with "self-observing platform, SLOs,
anomaly detection, incident triage, auto-remediation, deployment
verification and autonomous test loops" (v12.0 §3, p7), each item mapped to
code reached from a composition root with a named test. The blueprint gives
one line and no further text for either item this record decides.

Before this change the deep brain's operations loop
(`qip-deepbrain/src/operations.rs`, reached from `node.rs` every iteration)
opened an incident on a level shift, built its timeline, drafted a replay
fixture and ran the one runbook. Nothing said how much an incident mattered
or whose it was: every incident read the same, so a shift that coincided
with a venue filling far from the twin was filed beside one the runbook had
already contained. And each fixture's own `proposed_test` text — "replay the
window and assert it finds this shift … then show a window recorded after
the fix replaying to no shift" — was a sentence for a person; no process ran
it.

## Decision

1. **Severity is decided by what the incident touched and whether anything
   contained it, never by a score.** `qip_observability::aiops::Severity` is
   `Minor < Major < Critical`. `Critical`: an entity carrying trading
   behaviour was implicated in the incident window, or a committed
   critical-path objective was measured and missed on the iteration the
   incident opened. `Major`: no runbook action was applied, so a person must
   contain it. `Minor`: a runbook contained it and nothing trading-facing was
   involved. A second numeric bar on the shift score was rejected: the score
   already cleared the detector's bar to open the incident, and a larger one
   would be a figure nobody chose.
2. **The owner is the most-implicated entity, falling back to the process
   the shift was found in.** `EntityGraph::implicated` counts, per entity
   other than the deep brain's own deployment node, the signals an
   `abnormal` predicate accepts in `[shift start − INCIDENT_LOOKBACK, now]`
   — the timeline's window — ranked by count, ties in key order so a replay
   ranks identically. The deep brain's predicate is the pair of tests that
   already put an event on the timeline: a model reported `drifted`, a venue
   fill past `VENUE_BEHAVIOUR_BPS`. Triage and timeline therefore cannot
   disagree about what was unusual. `rank_incidents` orders a set most
   severe first and, within a severity, oldest first.
3. **The triage is on the journalled record.** `IncidentRecord.triage` is an
   `Option` with `#[serde(default)]` so a record journalled before triage
   existed still decodes; the topic's schema version does not change.
   `qip_aiops_incidents_triaged_total{severity}` counts each, the label
   bounded by the enum.
4. **The autonomous test loop re-runs every drafted fixture once per
   detector half (`SHIFT_HALF` iterations)** — the fewest fresh samples that
   can change a verdict, so no new cadence is introduced. Each check first
   replays the fixture's window from the journal and requires its own shift
   (`no_longer_reproduces` otherwise: a regression test that no longer
   reproduces its regression guards nothing). It then sets the shift's own
   pre-shift half beside the newest half of samples journalled after the
   window and asks the same detector whether the level is still shifted
   from that baseline: `still_shifted`, `cleared`, or `awaiting_samples`
   while fewer than one half has been recorded. Comparing against the
   pre-shift baseline, rather than looking for a new shift after the window,
   is the point: a series that stays flat at the shifted level shows no new
   shift and has not recovered. `qip_aiops_fixture_checks_total{outcome}`
   counts each check.

## Consequences

- An operator reading the deep brain's iteration record sees
  `incident INC-… [critical, owned by venue XLON]` rather than an
  undifferentiated incident, and the journal holds the same triage.
- A fixture that stops reproducing, or a shift that has not cleared, is
  named on the iteration record; one that cleared or is waiting is counted
  only.
- The loop reads the operations journal's samples once per check, every six
  iterations; the journal is the record, so the cost grows with it. A
  bounded index over samples is the remedy if it ever shows in iteration
  time, which the same loop would detect.

## What this leaves open

- **Deployment verification** has no composition-root caller.
  `Platform::release_decision` is reached only from
  `qip-kernel/tests/self_observation.rs`; neither `qip-api` nor `deploy.yml`
  asks it. Exposing it as a route means extending the API route table, the
  portal's endpoint mirror and the OpenAPI document together, and a deploy
  step that reads a running stage service's budget needs an authenticated
  caller the workflow does not have today.
- Triage covers the deep brain's own incidents only; no other process opens
  one.
