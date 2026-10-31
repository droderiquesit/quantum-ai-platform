# ADR 0213: An incident is journalled and drafted with its replay fixture; only a runbook acts, and only on this node

- **Status**: **Accepted 2026-10-30 under the owner's delegation** ("Make my
  decision on all lets move", `docs/blueprint/owner-decisions-2026-10-25.md`,
  extending `docs/blueprint/owner-decisions-2026-10-24.md`). The limits of
  that delegation hold here. This record sets no risk number, adds no
  anomaly threshold, changes no limit, threshold or limit set, and touches
  no paper-trading layer.
- **Date**: 2026-10-30
- **Confirms**: ADR 0114 D8. Agents draft and people act. Nothing here opens
  an incident, an issue or a pull request outside the platform.
- **Extends**: the deep brain's operations loop
  (`backend/crates/apps/qip-deepbrain/src/operations.rs`) and its remediation
  journal (R12-CICD-9081).
- **Rows**: R12-OBS-9158, R12-RES-9087.

## Context

R12-OBS-9158 reads: "An SRE agent reads telemetry, compares runbooks and
recent diffs, opens incidents/PRs, scales permitted services and executes
pre-approved rollback/restart/drain actions with audit. Check: integration:
A detected anomaly yields an incident and only a runbook action, audited."

R12-RES-9087 reads: "SRE agents learn from incidents and turn each
regression into a replay/fuzz/game-day test before proposing a fix. Check:
inspection: Each incident record links a new test that fails before the
fix."

Before this record the deep brain already had most of the first row's
mechanism, and none of the second's:

- `Operations::observe` runs on every iteration of the deep brain's run loop.
  `main.rs` builds `Operations::journalled` unconditionally and hands it to
  `node::run_with` as `Loops::operations`. A level shift in the node's own
  iteration time, from `detect_level_shift` with its one existing score,
  opened an `Incident`.
- The one runbook, `shed-research-on-iteration-shift`, switched off this
  node's own research capability through `CapabilitySwitch`. The
  `Remediator` refused any action no runbook names, and journalled the
  authorising record before the effect.
- The incident itself existed only in memory and on the cycle's printed
  line. Nothing durable said "incident N happened, here is what was done
  about it". The series the shift was found in was a buffer that died with
  the process, so the anomaly could not be reproduced afterwards. No test,
  fixture or draft came out of an incident.

There were two possible misreadings of "opens incidents/PRs". One is to build
an adapter that files an issue in a tracker or opens a pull request. ADR 0114
D8 refuses that by design: it is an external action on an agent's
initiative. The other is to do nothing because of that refusal. D8 allows
drafting, and this record uses that allowance.

## Decision

1. **A detected anomaly yields an incident record that is journalled and
   drafted, never sent.** `IncidentRecord` holds the incident id, the shift,
   the timeline, every audit record of the actions taken, the actions left
   for a person, the fixture link, and the channel it was drafted to. It is
   appended to the remediation journal (hash-chained, beside the platform's
   log) under the producer `deepbrain/aiops-incident`. It is then appended
   to the **operations outbox**, a second hash-chained log beside the
   journal, `<event log>.aiops-outbox.jsonl`. The record's `channel` is
   `qip_agency::outbox::OUTBOX_CHANNEL`. That is the same kind of surface as
   the agency's outbox: a file a person reads. It is not the agency's file.
   The agency door is that file's only writer, and a second writer on one
   chain makes a chain neither writer can vouch for. Opening the incident in
   a tracker, or a pull request, is a person's act through their own
   credential. D8 holds.

2. **Only a pre-approved runbook acts, only on this process, and every
   action is audited in the incident record.** For an iteration-time shift
   the diagnosis compares the shift with what changed and proposes three
   kinds of action:
   - shed this node's optional load (`disable_capability research`), always;
   - scale the node (`scale qip-deepbrain`), always;
   - roll back the model package (`rollback model-package`), only where a
     package was promoted within the incident's existing one-hour lookback.

   Only a proposal a committed runbook names (`runbook_for`) reaches the
   `Remediator`. The others are written into the record as `for_a_person`,
   each with the reason. The record's `actions` are read back from the
   remediator's own trail, filtered by incident id, so the record and the
   trail cannot disagree.

   **Scaling, rollback, restart and drain of other services stay human
   actions, and this part of R12-OBS-9158 stays unmet.** Each one needs a
   cloud credential, such as Cloud Run admin or a deploy identity. This
   process does not hold one and will not be given one. Change management
   already forbids an agent deploying. A runbook entry for them would be an
   effector holding a credential that can change production, which is the
   operational-change surface D8 refuses.

3. **Every incident record carries a drafted regression fixture, and the
   action waits for it.** Every iteration's time is journalled as an
   `IterationSample` before the detector reads it. The sample carries the
   cycle, the platform log's last sequence and the cycle's correlation id.
   A sample the journal refuses never enters the series, so the series and
   the journal always hold the same points. When a shift opens an incident
   the loop builds a `ReplayFixture`, which names:
   - the window by journal sequence range, and the SHA-256 of the window's
     record hashes;
   - the platform log's sequence range over the window, and the cycles'
     correlation ids;
   - the detector, by series and half-window;
   - the anomaly the window must reproduce;
   - in words, the test a person commits.

   The loop then **replays the window from the journal**, using
   `replay_fixture`. It drafts the fixture to the outbox only if the replay
   finds the same shift. The incident record links the draft by its outbox
   payload hash and sequence. With no reproducing fixture, no automated
   action is carried out. The remediation is reported as withheld and
   counted under `outcome="withheld_no_fixture"`. This is R12-RES-9087's
   "before proposing a fix", made an ordering the code holds rather than a
   habit.

4. **No new numeric thresholds.** The detector's score, its half-window and
   its retained series are unchanged. The one-hour lookback is the
   timeline's existing one, now a named constant (`INCIDENT_LOOKBACK`) so
   that the diagnosis and the timeline cannot drift apart.

## What changes and what does not

- **Changes.** The deep brain writes one sample record per iteration to the
  remediation journal. Each incident adds a fixture draft and an incident
  draft to a new outbox file, and an incident record to the journal. The
  cycle line now names the person's actions, the fixture's digest, and that
  nothing was sent. A restarted node numbers its incidents on from those its
  journal holds. Before this, a restart reused `INC-deepbrain-000001`. A
  non-finite iteration time is now refused before it is journalled or read.
- **Does not change.** The detector. The runbook set. The `Remediator`'s
  refusal and its authorise-then-apply journalling. `CapabilitySwitch`. The
  agency door and its outbox. Every risk limit, threshold, limit set,
  capital grant and multiplier. The three paper-trading layers: Terraform's
  refusal of the live rungs, `AutonomyLevel::deployable` in the composition
  roots, and the edge `Cell`'s paper-only constructors.

## What it costs

- One journal append per iteration on the deep brain, and three appends per
  incident, as described under "Changes". A cycle is measured in minutes, so
  an append is invisible against it, but it is a write on every iteration
  where there was none.
- An incident whose window cannot be replayed from the journal takes no
  automated action. If the journal is broken, the node keeps its research
  load through a shift it would otherwise have shed. That is intended: an
  action with no reproducible cause is the thing R12-RES-9087 forbids.
- Every action the requirement describes beyond shedding this node's own
  load is manual work for a person, as D8 already makes publishing and
  outreach.

## What is still unmet, stated plainly

- **R12-OBS-9158** "opens incidents/PRs": drafted, not opened. "Scales
  permitted services and executes rollback/restart/drain": proposed and
  written down for a person, not executed. Its check, "a detected anomaly
  yields an incident and only a runbook action, audited", is met.
- **R12-RES-9087** "a new test that fails before the fix": the platform
  drafts the replay fixture and the test's statement, and links them by
  digest. Committing the test is a person's act, because an agent may not
  write the repository on the platform's initiative. So is showing that it
  fails before the fix. The recorded window always replays to the shift, so
  "fails before the fix" is a property of the test the person writes
  against the fixture, typically "a window recorded after the fix replays to
  no shift". The platform cannot assert it for them. Fuzz and game-day tests
  are not drafted.
- The journal grows by one sample per iteration on `system.alert`, an
  irreplaceable class, within the log's default ceiling of one million
  records. At a one-minute cycle that is years. A node that reaches it
  refuses further samples and so further incidents, and the cycle line shows
  that as an operations failure. Rolling the journal is future work.

## What would make this wrong

- An adapter that sends an incident or fixture draft anywhere on the
  platform's initiative.
- A runbook naming `scale`, `rollback`, `restart` or `drain` against
  anything but this process, or an effector holding a cloud credential.
- An automated action carried out for an incident whose record has no
  reproducing fixture.
- An incident record whose `actions` differ from the remediator's trail for
  that incident.

## Evidence

- `backend/crates/apps/qip-deepbrain/src/operations.rs`: `IterationSample`,
  `ReplayFixture`, `FixtureLink`, `IncidentRecord`, `OperationsDraft`,
  `replay_fixture`, `runbook_for`, `RemediationJournal::{open_pair,
  incidents, samples, drafts, linked_fixture, verify}`, and
  `Operations::{proposals, draft_fixture, open_incident}`.
- `backend/crates/apps/qip-deepbrain/tests/research.rs::a_detected_anomaly_yields_a_journaled_incident_a_runbook_action_and_nothing_else_all_audited`.
  One shift makes exactly one journalled incident. Its `actions` equal the
  journal's two audit records (authorise, outcome), each naming the runbook
  that authorised it. Only research is switched off. `scale qip-deepbrain`
  is left for a person and never reaches the remediator. The outbox holds
  the fixture, then the incident, and both chains verify. Two mutations
  were run against it: skipping an audit entry in the record fails the
  `actions == journal` assertion, and admitting every proposal to the
  remediator yields three audit records where two are expected.
- `…/tests/research.rs::each_incident_record_links_a_drafted_replay_fixture_that_reproduces_it`.
  Two shifts make two incidents. Each record's link resolves to a drafted
  fixture by digest, and its journal window replays to that incident's own
  anomaly. A fixture whose `expected` is altered still replays to the
  journal's answer, and a window moved by one sequence is refused. One
  mutation was run: dropping the link from the record fails with
  "INC-deepbrain-000001 links no fixture".
