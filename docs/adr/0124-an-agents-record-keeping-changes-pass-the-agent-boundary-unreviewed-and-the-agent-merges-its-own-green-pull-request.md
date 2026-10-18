# ADR 0124: An agent's record-keeping changes pass the agent boundary unreviewed, the agent merges its own green pull request, and dev reads the range verdict as advice

- **Status**: **Accepted**, 2026-10-18, by the owner in session: "I merged
  but you need to enable yourself to do that", then "Yes" to the proposal
  below, with the limits it names.
- **Date**: 2026-10-18
- **Amends**: ADR 0117 (the agent boundary and its gates) — narrows what
  needs a person to the paths that decide what a gate means, and records
  the 2026-10-12 range advisory under a number that exists.
- **Keeps**: ADR 0003 (paper only); GOV-9067's deploy half (no agent starts
  `deploy.yml`, `infra.yml`, `fleet.yml`, `image.yml`, `vendor.yml` or
  `branch-cleanup.yml`); the protected status of every workflow, gate script
  and policy, the tfvars, the paper-trading Terraform layer, `CLAUDE.md` and
  every file under `.claude/rules/`.

## Context

Every pull request that carries an ADR or a correction to a domain note is
red on `agent boundary` until the owner reviews its head commit, because
`docs/adr/**` and `.claude/rules/**` are protected paths
(`scripts/agent-policy.json`, `protected_paths.globs`). Since 2026-10-17
nearly every pull request the agent opens carries one or both: a contract
change needs an ADR first (`.claude/rules/architecture/00-boundaries.md`),
and a register re-grade corrects the domain notes whose counts and dates
have moved. PR #229 touched eight protected paths; six of them were records
(ADR 0123, its index row, three domain notes, `CLAUDE.md`'s stack facts) and
two were workflows. The owner merged it by hand and asked that the agent be
able to do that itself.

Separately, `deploy_range_advisory` in the same policy cited "ADR 0123,
owner-accepted 2026-10-12" for the rule that dev reads the deploy gate's
range verdict as advice. No record with that number was ever written, and on
2026-10-17 the number was claimed for ledger operations. A decision cited by
a number that names a different decision is a decision nobody can read; it
is recorded here, under its own number, and both citations now point here.

## Decision

1. **Self-reviewable protected paths.** `scripts/agent-policy.json` gains
   `self_reviewable_protected_paths`: `docs/adr/README.md` when GitHub
   reports it *modified*, and `docs/adr/0*.md` when GitHub reports the file
   *added*. `scripts/agent-guard.mjs` reads each file's status from the
   objects GitHub already returns (`change` from the pull request's files,
   `range` from each commit's files), judges a rename on the path it left as
   well as the one it took, and treats a protected path as needing a person
   unless it is self-reviewable; the refusal names only the paths that still
   need one, so the reviewer reads what is actually theirs. A bare path, a
   removed, renamed or copied file, and an edit to an accepted record all
   need a person. Why these two: each records a decision, and the gates
   read them only to hold their form — the index must list every record and
   every record must state its cost; the suites that read an ADR's text
   (`blueprint_rules` on ADR 0117's status, `documentation` on ADR 0003)
   read accepted records, which the added-only rule leaves under review.
   Why nothing under `.claude/rules/`: the proposal the owner accepted put
   the domain notes here, and the independent security review of this
   change took them out — they carry Prohibited lists that bind agents, and
   the session's own permission layer refused an agent edit to its rules as
   self-modification, which is the right refusal to keep. Why added-only
   for a record: a new record is a proposal the index makes visible; an edit
   to an accepted one rewrites a decision somebody relied on.
2. **The agent merges its own pull request** once ci is green on the head
   and every check that still needs a person has one. The owner's
   instruction above is the explicit approval
   `.claude/rules/02-change-management.md` requires for a merge. The rule
   file itself is left as it was: the session's permission layer refused
   the agent's edit to its own rules as self-modification, which is the
   right refusal, so this record and the runbook are where the
   authorisation is written down, and a session that cannot read them asks
   before merging. The guard cannot see a merge — `docs/ops/runbooks/
   development-agent-controls.md` states that limit — and this decision does
   not pretend it can; it is an authorisation on record, not a control. It
   covers merging only. Starting any workflow in `dispatch_refused_for_agents`
   stays refused, and prod is never deployed.
3. **dev reads the agent-boundary range verdict as advice** (accepted
   2026-10-12, recorded here): refusals are printed and the deploy proceeds,
   for dev and no other environment, which `rangeIsAdvisory` already holds.

## Consequences

- A pull request whose protected paths are a new record and its index row
  passes the boundary. The owner's review moves to where a change alters
  what a gate means or what an agent is told: a workflow, a policy, a tfvars
  file, any rule file.
- A rename of a protected file is judged on both of its paths. Before this
  record only the destination was, so a workflow or a rule file moved under
  an unprotected name left its rule unreviewed; the security review of this
  change found that route and it is closed here, for every destination.
- An agent can write an ADR no person read before it merged. The ADR is a
  record; every gate it might describe is code on a protected path the agent
  still cannot change alone, and the index row makes the record visible.
  `every_decision_record_is_listed_in_the_index` and
  `every_decision_record_states_what_it_costs` still hold it to the form.
- The dangling "ADR 0123" citations in the policy and the guard now read
  "ADR 0124 §3".

## What it costs

- The domain notes stay under review, so a register re-grade that corrects
  their counts and dates still waits on the owner. The security review
  priced that wait as worth paying, and this record agrees.
- `main` carries no branch protection today, so `agent boundary` blocks
  nothing by itself: it is a verdict the owner reads before merging and the
  deploy gate re-checks for test and stage. This record changes none of
  that and says it rather than implying the check is required.
- Agent-written decision records enter the register with one reader.
  Reopening one is a person's edit under the added-only rule, so an agent
  that wants to change a decision must write a superseding record, which is
  what the rules already ask for.
- `added` is read from GitHub's file status. A record GitHub reports as
  `renamed` or `modified` is judged as an edit and needs a person — the
  strict direction, and the one that costs a review rather than admits one.
- The merge authorisation lives in a rule file and this record, not in a
  control. Nothing refuses an agent merge; that was already true (the
  runbook and the GOV-9067 register row both say so), and this record says
  it rather than implying a gate exists.

## What would make this wrong

- If a gate ever read a *new* ADR's text to decide anything — today the
  suites read accepted records, which stay under review — the added-only
  exemption would have to go first.
- If `.claude/rules/domains/**` were ever proposed for this list again, the
  security review's objection stands: those notes bind agents.
- If an agent-written ADR were found to have "accepted" a change to a
  protected gate that then landed without the gate's own protected-path
  review, the added-only exemption should be withdrawn.
- If branch protection ever makes `agent boundary` a required check, a
  record-only pull request would again wait on a person, which is the wait
  this record exists to remove; revisit rather than widen the exemption.

## R12 ids

GOV-9067 (the merge half is now the owner's authorisation on record, not a
gate; the deploy half stands), CICD-9076, SEC-9078.
