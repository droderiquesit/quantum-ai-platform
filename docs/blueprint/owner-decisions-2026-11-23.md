# Owner decisions, 2026-11-23: ADR 0724 and ADR 0725 accepted by merging pull request #288

Pull request #288 was opened on 2026-11-22 carrying two decision records in
**Proposed**, each standing between a built row and its regrade, and each a
question the agent that built the row could not answer for itself. Its
description said, in its first line:

> "This pull request waits on two owner decisions. Merging it is the owner
> accepting ADR 0724 and ADR 0725. The agent that opened it will not merge it."

The owner merged it on 2026-11-23 at 00:03:29 UTC. GitHub records the merge
against the repository owner's account, not against the agent session that
opened the pull request, and the agent did not merge it. That merge is the
acceptance, and this file is where the repository says so.

## Accepted here

Both as written, with the limits their own bodies record.

| Record | What it decides |
|---|---|
| 0724 | The Meta Brain measures models by regime from the forecaster tournament's own record. The per-(model, regime) cells are handed to every arbiter with the panel; the Meta Brain's manifest still grants `read_research_memory` and nothing else, and decision 2 of ADR 0466 stands unamended |
| 0725 | An ambient brain covers a scope only through a finding the deep brain composes from its sweep. The brain registry records and does not cover, and the desk's scopes keep their tests. For an ambient brain, the deployed deep brain is the production organisation |

Where a record's own Status line still reads Proposed, this file governs, as
`owner-decisions-2026-11-16.md` governs the forty-one records it accepted. An
accepted record is a protected path, and an agent does not edit its Status
line to say what this file already says.

## What the acceptance settles, in the owner's terms

**0724.** The forecaster tournament's records are what `read_model_registry`
gates for the model-failure probe, and ADR 0466 decision 2 says what an
arbiter may reach beyond the panel "is still gated". A reader held the
hand-off against that sentence. The owner was shown three choices in the pull
request: accept the hand-off as written, require the read to go through the
gate, or keep the hand-off and add an access record. The owner took the
first. So the per-regime model cells reach the Meta Brain with the panel,
ungated and with no access record, and that is now a decision and not an
oversight. From those cells the Meta Brain can derive a forecaster's mean
Brier, which the gate otherwise protects; that was stated in the pull request
and is part of what was accepted.

**0725.** The check on R12-AMBIENT-3027 says the brain is "constructed in the
production organisation". The attention router is built by the deep brain and
has no seat in the desk's standard roster. The owner was shown two choices:
accept that the deployed deep brain counts as the production organisation for
an ambient brain, or say a roster seat was meant. The owner took the first.

**The quiet sweep, and two corrections to this file.** ADR 0725 decision 4
says a sweep whose router returned nothing composes no finding. The pull
request the owner merged offered "Accept ADR 0725 as written", and in the
same description listed the quiet sweep as "still to fix" because the
2026-10-31 ruling on the row asks for "one AgentFinding per ResearchRound".
Those two sentences cannot both hold, and the pull request should not have
put them side by side.

As first written, this file said the owner had accepted the quiet-sweep rule
outright; readers pointed out the pull request's "still to fix" note, and the
file was corrected to say the ruling governed, and the code was changed to
compose a finding on every round. Readers then refuted that change too: on a
quiet round the router is not consulted at all, so the finding's zeros were
written by the code, not read from the router, and coverage came from holding
a router rather than from what it routed, which is the registration-style
coverage the ruling itself rejects. That is decision 4's own reasoning.

**This is not settled, and it is the owner's to settle.** A first reading
here held that the owner's direct acceptance of ADR 0725 outranks the
2026-10-31 ruling, because that ruling was written by the orchestrator under
delegation. Two readers refuted that too, and they are right: the owner's
words of 2026-10-31 were "You can make all my decisions go", ADR 0725 itself
calls the ruling the owner's, and nothing in the repository ranks one owner
act above the other. The code is at ADR 0725 decision 4 (a quiet sweep
composes no finding), because that is what is on `main` and both attempts to
change it were refuted. R12-AMBIENT-3027 stays PARTIAL until the owner says
which governs:

| Choice | What follows |
|---|---|
| ADR 0725 decision 4 governs | A quiet sweep composes no finding, and its gauge reads 0 like a removed router. The row closes on the code as it stands |
| The ruling governs: one finding per round | The router must be consulted on every round, so that a quiet round's finding reports something the router measured, not a zero the code wrote. That is a design change to the router and an amendment to ADR 0725, and both come back for acceptance |

## What this file does not do

- **No register row is regraded by this file.** Acceptance removes the one
  ground two rows were held on. Each still goes back to two independent
  readers, and is regraded only if both fail to refute it on the tree as it
  then stands.
- **No guard, grant or limit changes.** The Meta Brain's manifest is what it
  was; a test pins it. Nothing on the order path reads a model cell or an
  attention finding, and both findings stay advisory.
- **The paper boundary is untouched.** Neither record goes near
  `infrastructure/terraform/variables.tf`, the composition roots' autonomy
  check or the `qip-edge` cell.
- **Nothing is deployed.** Decision 39 stands: billing is paused, and
  dispatching `infra.yml` or `deploy.yml` stays the owner's.
