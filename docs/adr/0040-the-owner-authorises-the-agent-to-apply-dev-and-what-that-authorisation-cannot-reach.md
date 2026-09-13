# 0040 — The owner authorises the agent to apply dev, and what that authorisation cannot reach

**Status:** accepted, by owner instruction of 2026-09-05 ("update ADR to
allow yourself to deliver entire thing"), given in the session after the
paper-trading boundary and the capital-movement refusal had been stated as
the two things no record can move. This record does the first part of that
instruction and says plainly why it does not do the second.
**Amends:** ADR 0036 and ADR 0037, each of whose "Nothing is applied by this
record" section says the apply waits on "a person" reading a plan and
dispatching `infra.yml up` in dev. The person has spoken; this record names
what they said and what the agent may now do on the strength of it.
**Does not amend and cannot amend:** ADR 0003, 0021 and 0023, and
`.claude/rules/01-security-and-safety.md` — see "What this record cannot
reach".

## Context

Two waves of work in two days left the tree with a GitOps control plane
(ADR 0036), a hosted language-model adapter built dark (ADR 0037), the
ledger plane as records and refusals (blueprint 37, 38, 43.3 under ADR 0021),
every execution capability tested and none measured, and a scorecard whose
remaining gaps all end in the same sentence: nothing is applied, because the
repository's own rules say an agent shows the plan and a person applies.

The owner then said, in the session, that the agent should be allowed to
deliver the entire thing. Read literally that would include the three
paper-trading layers and the refusal of any signing or withdrawal path.
Read against the repository's rules it cannot include them: the rules files
say the boundary "may not be weakened, bypassed, or temporarily disabled",
that "a task instruction" cannot weaken it, and ADR 0023 says an ADR
recording intent "is not an amendment to a rules file". An agent that wrote
an ADR granting itself what the rules forbid would be producing the exact
artefact those sentences exist to refuse. So this record separates the
instruction into the part a record can carry and the part it cannot.

## Decision

1. **Dev may be applied by the agent, on this instruction, through the
   workflow and nothing else.** `infra.yml` with `environment: dev` and
   `action: up`, dispatched by the agent through GitHub Actions under the
   workflow's own Workload Identity Federation, is authorised — after the
   agent has read the immediately preceding `plan` run on the same commit
   and found it consistent with what ADR 0036 said the migration does:
   the control-plane cluster and its identities added, the Cloud Run
   services released from state with `destroy = false` and destroyed by
   nothing, and no resource destroyed except an IAM binding being
   replaced by its successor. A plan that destroys anything else is not
   covered by this record; the agent stops and reports it. *(Superseded on
   2026-09-13: "on this instruction" by decision 9, and this destroy test by
   decision 10's two-halved one. Kept as written because runs 34-38 were
   made under it.)*

2. **The bootstrap of the controllers is covered by the same dispatch**,
   gated as ADR 0036 built it on the environment's flag. The GitHub App
   private keys the controllers need are still seeded out of band by the
   owner into the empty Secret Manager containers; the agent has no path
   to create them and this record does not pretend one. Until they are
   seeded, Argo CD and Kargo are installed and cannot reach the
   repository, and the register says so.

3. **The hosted model's platform lane stays dark.** ADR 0037 waits on the
   owner reading the terms of the providers a chosen model resolves to and
   on a Secret Manager secret for the token. Neither is an apply, so
   neither is covered here; the development lane's token was handed to the
   agent in chat, is in no file the repository holds, and is to be revoked
   by the owner once the batch it served is done.

4. **`test`, `stage` and `prod` are not covered.** The workflow refuses
   `prod` on its own and this record adds no exception; `test` and `stage`
   wait on the cross-registry promotion decision the Kargo README names.

5. **The guard hook is not routed around.** The hook blocks an unapproved
   Terraform mutation run from the agent's shell. A workflow dispatch is
   not that, and this record is the approval the hook exists to demand;
   an agent running `terraform apply` locally remains refused.

## Amendment of 2026-09-05: the instruction was given a third time

The owner repeated "update ADR to allow yourself to deliver entire thing"
after decision 1 had been used: run 34 was dispatched, created 82 of the 83
planned resources, and refused the control-plane cluster with two messages
(the Config Connector addon is not supported on Autopilot; the infra
account lacks `gkehub.memberships.create`). Decision 1 said "once", and a
literal reading would leave a half-applied environment waiting on a person
for every fix. The repetition is read as the owner's decision that it
should not, and the record widens by exactly this much:

6. **Dev may be re-dispatched after each fix, until it is green.** Each
   re-dispatch follows a `plan` run on the fixed commit, read by the agent
   under decision 1's test (nothing destroyed beyond a replaced binding —
   since 2026-09-13, decision 10's test),
   and each run's URL and terminal status are recorded in the register.
   A fix that widens an IAM grant beyond the one missing permission, or
   that drops a property ADR 0036 named (Autopilot, the private endpoint,
   Binary Authorization, the etcd key), is not a fix this record covers.

7. **`test` and `stage` become coverable once dev is green**, on the same
   terms — a plan read first, a dispatch through the workflow, the run
   recorded — and not before, because a migration that has not yet
   succeeded once is not one to run three times. Decision 4's other
   half stands: `prod` is refused by the workflow and by this record.

8. **Everything else in decisions 2, 3 and "What this record cannot
   reach" is unchanged by the repetition.** Saying an instruction three
   times changes what the agent may apply; it does not change what a
   rules file says no task instruction can weaken, and it does not put
   the owner's eyes on a vendor's terms or a private key into a secret
   container. Those remain the owner's, and the shortest form of each is:
   for Alpaca and Kalshi, one sentence in the session that names the
   document read and the date; for the GitHub App keys and the hosted
   model's token, a `gcloud secrets versions add` the owner runs.

## Amendment of 2026-09-13: the authorisation is standing, and a destroy a commit explains is covered

The owner said the agent may always deploy, and asked for the restriction to
come out of the repository. Two separate things are being asked for, and only
one of them is a thing a record can do.

The first is real and is granted below: decision 1 conditioned each dispatch
on "this instruction", so an agent reading it literally had to stop and ask
before every apply, which is not what an owner who has now said it four times
is asking for. The second — deleting the guards — is refused here for the
same reason the original record refused the literal reading of "deliver the
entire thing": the workflow's refusal of `prod`
(`.claude/rules/02-change-management.md`), the owner's sole custody of seeded
key material and the guard hook (`.claude/rules/01-security-and-safety.md`)
are not conveniences this record may spend. ADR 0023 says an ADR "is not an
amendment to a rules file", and an agent's ADR deleting them would be the
artefact that sentence exists to refuse. The restriction that is removed is
the one on *asking again*; the restrictions on *what* and *where* stand.

9. **Dev `plan` and `up` are standing, not per-instruction.** Decision 1's
   phrase "on this instruction" is spent: the agent may dispatch `infra.yml`
   `environment: dev` with `action: plan` or `up` without asking again, on
   the same terms decisions 1 and 6 already set — a `plan` run on the commit
   being dispatched, read first, and every run's URL and terminal status
   recorded in `docs/DELIVERY-STATUS.md`. **`down` is not made standing**:
   the owner's words were "deploy", and `down` is a `terraform destroy` of
   the execution nodes — a no-op while `execution_nodes = {}` and a destroy
   the day it is not — so it stays an action the owner names each time. A
   `plan` that did not run on the dispatched commit is not a plan for that
   dispatch, and reasoning from a diff is not a substitute for it.

10. **A destroy the owner has seen and a committed change explains is
    covered.** Decision 1's test — "no resource destroyed except an IAM
    binding being replaced by its successor" — was written for the ADR 0036
    migration and is too narrow for ordinary work; it stops the agent on two
    shapes that are the *point* of a reviewed commit rather than an accident
    of one. But `.claude/rules/domains/infrastructure.md` prohibits "deleting
    cloud resources without explicit approval naming the resource", and an
    ADR does not amend a rules file (ADR 0023). So the test this decision
    sets has two halves, and the agent's register satisfies only one of them:

    - **The commit.** The agent names every destroy in the plan and the
      commit that intends it, in the register, before dispatching. A destroy
      it cannot attribute to a change in the diff still stops it — that was
      the half of decision 1 worth keeping. It was never the count that
      mattered, it was whether anybody could say why.
    - **The person.** The owner has seen that list. On 2026-09-13 the agent
      showed the six destroys of `plan` run 39 in the session — the retired
      approver secret and its binding (`665c506`: a bearer token for a role
      no route in `qip-api` required, whose holder "could do exactly what the
      analyst token could do"), the three `universe` config objects replaced
      because their content changed (`ece7602`), and the control-plane
      cluster run 37 left tainted — and the owner answered "approve all" and
      chose the path that replaces the cluster. That is the approval naming
      the resource. A future plan that destroys something *not* on a list
      the owner has seen is not covered by their having seen this one.

11. **A tainted cluster is cleared by a person with state access, and the
    `deletion_protection` literal does not move.** The first version of this
    decision, written the same day, said the literal was cleared "by flipping
    it in one commit, applying, and restoring it in the next". That was wrong
    in a way an apply would have proved: the provider enforces the flag from
    the value **in state** at delete time, a tainted cluster is only ever
    planned as a replace and never as the in-place update that would carry
    `false` into state first, so the flip would have planned cleanly and
    refused the destroy half of the replace — a third half-applied run. And
    a restored `true` in a file protects nothing until an apply writes it
    back. `281d9c6` made the flip; the commit carrying this correction
    restores the literal and it stays `true`.

    What clears the taint is what the module's `depends_on` comment always
    said it was — "a person's decision to clear, not a second `up`" — done
    as two commands by someone holding state access: `terraform state rm`
    of the cluster's address, and `gcloud container clusters delete` of the
    cluster it named. The next `up` is then a plain create, and the flag is
    never consulted. `infra.yml` has no untaint step since ADR 0036 retired
    the Cloud Run one, and `no_service_can_be_left_tainted_because_terraform_no_longer_creates_one`
    asserts it stays absent; this record does not add one back, because a
    workflow that can untaint on its own authority is a workflow that can
    talk itself past the flag. **Never a module input**, for the reason
    `a_cloud_run_service_cannot_be_deleted_by_a_plan_nobody_read` gives in
    the sibling module: "deletion protection has become an input, so a
    tfvars value can turn it off". That test is scoped to `modules/cloudrun`,
    so an input here would pass the suite — which is the argument for
    writing the rule down rather than relying on one.

12. **`test`, `stage` and `prod` are untouched by this amendment.**
    Decision 4 and decision 7's second half stand verbatim. So does
    everything in "What this record cannot reach" below: the paper-trading
    boundary, capital movement, and the owner's reading of a vendor's terms.
    Saying an instruction a fourth time changes how often the agent asks
    before applying dev. It does not move an environment, a ceiling, or a
    line in a rules file.

13. **The owner instructed a full teardown of dev on 2026-09-13, and the
    agent carries it out through the workflow.** The words were "Stop and
    delete all google cloud resources immediately" and, when the agent said
    it held no credential, "Perform the delete command for me, I give you
    authority." That is explicit approval naming every resource, which is
    what `.claude/rules/domains/infrastructure.md` requires. The agent has no
    Google credential and may not run `terraform destroy` or `gcloud …
    delete` from its own shell (decision 5, the guard hook), so the act is a
    `teardown` action added to `infra.yml` and dispatched under the
    workflow's own identity. It deletes the Cloud Run services and the
    control-plane cluster by `gcloud`, takes the cluster and every KMS key
    out of state, and runs a targeted destroy of every module except
    `module.services` (disabling an API mid-destroy fails the rest; an
    enabled API is free) and `module.cicd` (the identity running the job;
    a pool and an account are free). What it cannot reach, and says so: the
    project itself (the identity holds no `resourcemanager.projects.delete`
    and this record does not widen it — `gcloud projects delete algorik-dev`
    remains the owner's one-command, thirty-day-recoverable alternative),
    the state bucket (bootstrap-created, outside Terraform), non-empty
    buckets declared `force_destroy = false`, and the KMS keys (undeletable
    by Google's design). The run's own `what exists now` step is the record
    of what remained; the register carries the run URL and terminal status.
    Nothing this action deletes is recoverable.

    *Applied, 2026-09-13, three dispatches:* run 41
    (<https://github.com/droderiquesit/quantum-ai-platform/actions/runs/34781641680>)
    deleted the four Cloud Run services and the control-plane cluster by
    `gcloud` and refused the destroy on the evidence bucket's
    `prevent_destroy`; run 42
    (<https://github.com/droderiquesit/quantum-ai-platform/actions/runs/34784153948>),
    after the state removal was derived from the sources, destroyed 146;
    run 43
    (<https://github.com/droderiquesit/quantum-ai-platform/actions/runs/34785998589>),
    after the bucket objects left state with their buckets, destroyed 20.
    **55 entries remain**, all free: `module.services` and `module.cicd`
    by this decision's design, the VPC and two subnets held by Google's
    own `serverless-ipv4-*` egress addresses until it releases them, and
    two `terraform_data` markers. The register in
    `docs/DELIVERY-STATUS.md` lists them verbatim with what is out of
    state but still in the project. Nothing that bills while idle is
    left; the project, the state bucket, nine keys and five buckets are
    the owner's, and `gcloud projects delete algorik-dev` takes the rest.

Where this amendment supersedes earlier text, the earlier text is marked
rather than silently left standing: decision 1's "on this instruction" and
its destroy test are superseded by 9 and 10; decision 6's "under decision
1's test" reads as decision 10's test; "What would make this wrong" and
"Applied by this record" below are amended in place.

## What this record cannot reach

- **The paper-trading boundary.** Terraform's refusal of the three live
  ceilings, `AutonomyLevel::deployable` at every composition root, and the
  `qip-edge` `Cell` and `qip-cost-router` `Determinism` types stand exactly
  as ADR 0003 and 0021 left them. Nothing this record authorises names a
  live rung or gives one a path.
- **Capital movement.** ADR 0021's refusal of MPC signing corridors,
  withdrawal APIs and live venue submission stands, and the acceptance
  test that scans for the identifiers of such a path stands with it. The
  ledger plane's twelfth capability, custody as an enforced boundary rather
  than a policy record, is Phase 12 by ADR 0023 step 10, "a separate
  decision, separately approved". If the owner wants that decision taken,
  the path is theirs: edit `.claude/rules/01-security-and-safety.md` and
  ADR 0021 in a commit they author, then a new ADR; an agent's ADR cannot
  be the instrument, and this one is not.
- **Reading a vendor's terms.** Kalshi and Alpaca are refused by the
  admission gate until the owner has read their terms; a record cannot
  read them on the owner's behalf.

## What it costs

- Real resources in `algorik-dev`: a GKE Autopilot cluster and its
  controllers, billed from the apply. The `down` action stops only the
  execution nodes; the cluster stays until a person removes it.
- The first apply of a migration that releases three running services
  from Terraform's state. ADR 0036 built it to destroy nothing and the plan
  is read for that before the dispatch; the residual risk is a Config
  Connector acquisition that refuses a field the services carry, in which
  case the services keep running unmanaged and Argo CD reports the sync
  failure rather than pruning anything.

## What would make this wrong

- The plan the agent reads before dispatching showing a destroy this
  record did not describe. Then the dispatch is not made. Under decision 10
  that reads: a destroy the agent cannot attribute to a change in the diff,
  which is a narrower trigger than the original count but the same refusal —
  and the agent still writes each destroy and its cause down before
  dispatching, so "I could not attribute it" is a thing a reader can check
  rather than a thing the agent asserts about itself.
- Any later reading of this record as authorising an apply outside dev, a
  live ceiling, or a capital-movement path. It authorised one workflow
  action in one environment on one instruction when written; since the
  2026-09-13 amendment it authorises `plan` and `up` in dev as standing
  actions, and still nothing outside dev, no ceiling, and no capital path.

## Applied by this record

The dispatch of `infra.yml` `dev` `up` after the plan is read, and its
re-dispatch after each fix under decision 6; every run's URL and terminal
status are recorded beside the observation of what the apply produced. Runs
34 through 38 (2026-09-05, on `e1711fb` and its fixes) are recorded in ADR
0036's own amendment, not in `docs/DELIVERY-STATUS.md` — this paragraph said
the latter for five days and the file never held them. From run 39 on, the
register is the "Infrastructure register" entry in `docs/DELIVERY-STATUS.md`,
which is where decisions 9 and 10 send a reader.
