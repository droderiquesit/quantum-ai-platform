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
   covered by this record; the agent stops and reports it.

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
   under decision 1's test (nothing destroyed beyond a replaced binding),
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
  record did not describe. Then the dispatch is not made.
- Any later reading of this record as authorising an apply outside dev, a
  live ceiling, or a capital-movement path. It authorises one workflow
  action in one environment on one instruction, and says so.

## Applied by this record

The dispatch of `infra.yml` `dev` `up` after the plan is read, and its
re-dispatch after each fix under decision 6; every run's URL and terminal
status are recorded in `docs/ops/missing-infrastructure-register.md`
beside the observation of what the apply produced. Runs 34 and 35 on
`e1711fb` are the first two entries: both failed on the cluster, and the
fix is the commit that carries this amendment.
