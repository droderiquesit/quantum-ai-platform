# ADR 0118: The GCP tenant is a platform folder holding one labelled, budgeted project per environment and a seed project, applied once by the owner

- **Status**: **Accepted**, 2026-10-11, by the owner in the session
  conversation: "Create a brand new project in gcp to deploy from scratch ...
  as a cloud engineer you own the gcp tenant keep it clean and organized
  appropriately".
- **Date**: 2026-10-11
- **Builds on**: ADR 0113 §4 (D7 — one project per environment now, a
  per-environment folder of projects at the second region), ADR 0115 (one
  build registry, dev's, promoted by digest), ADR 0024 and ADR 0036 (the
  platform root runs from `infra.yml` under Workload Identity Federation),
  ADR 0040 decision 13 and ADR 0093 (dev's teardown and the suspended control
  plane).
- **Implemented by**: `infrastructure/foundation/` (the root and its
  `tests/`), `scripts/bootstrap-foundation.sh`,
  `infrastructure/environments/dev/terraform.tfvars` (`project_id`,
  `project_number = 0`), the project-number refusal in `infra.yml`,
  `deploy.yml`, `vendor.yml`, `image.yml`, `fleet.yml` and in the platform
  root's `var.project_number`, `qip-acceptance`'s `tenant_foundation` suite,
  `docs/ops/runbooks/new-project-from-scratch.md`.

## Context

Nothing in this repository could create a project. The platform root plans
and applies *inside* a project, as `qip-infra-<env>`, an identity that lives
in the project it manages; `scripts/bootstrap-deploy.sh` refuses to create one
because that is an act against an organisation and a billing account. So every
project this platform has run in was made by hand: no folder, no budget, labels
if somebody remembered. `algorik-platform-dev` is the result. It has been
applied, torn down in three dispatches (ADR 0040 decision 13), re-applied with
no service serving, and had its control plane suspended (ADR 0093), and it
still holds nine KMS keys, five `force_destroy = false` buckets, an empty VPC
and the state bucket outside any state. It is not a clean place to deploy from
scratch, and nothing in it says who owns it or what it may cost.

The owner asked for a new project and for the tenant to be kept clean and
organised. The question is what "the tenant" is as code, who applies it, and
how the platform root is pointed at what it creates without a workflow ever
holding organisation-level rights.

## Decision

1. **A second Terraform root, `infrastructure/foundation/`, with its own
   state, applied once by the owner.** Its rights — create a folder, create
   projects, link billing, set organisation policy — are the owner's and no
   workflow's. One state for both roots would mean either `qip-infra-dev`
   holding those rights or the owner applying the platform by hand; both undo
   ADR 0024.

2. **The layout.** Under the organisation, one folder (`algorik-platform`).
   In it: a seed project `<prefix>-seed`, which holds the foundation's state
   bucket and nothing that runs, and one project `<prefix>-<env>` per enabled
   environment. The four environments are exactly the values the platform
   root's `var.environment` accepts; **dev is on, test, stage and prod are
   declared and off**, and off creates nothing. This is ADR 0113 §4's "now" —
   one project per environment — inside a folder, so the trigger in §4 (a
   folder per environment, split by security/data/execution/services/
   engineering) is a change of shape inside one folder rather than a migration
   out of the organisation's root.

3. **No-org mode.** An account with no organisation gets the same projects
   with no parent, held together by billing, labels and naming. The org-only
   controls are not silently dropped: the root's `controls_not_applied` output
   lists them, and the platform root still sets the key-creation and
   key-upload policies per project.

4. **Every project is labelled** `owner`, `environment`, `cost-center`,
   `managed-by=terraform` — `environment` and `managed-by` set by the root and
   not overridable — **billing-linked, and budgeted on its own**, alerting the
   billing account's administrators at 50%, 90% and 100% of actual spend. One
   budget per project, never one across the account, so the alert names the
   project spending. Label values are team names, never addresses: labels are
   copied into the billing export.

5. **Organisation policy on the folder**, where the owner has the role:
   `iam.disableServiceAccountKeyCreation` and
   `iam.disableServiceAccountKeyUpload`, `storage.uniformBucketLevelAccess`,
   `compute.skipDefaultNetworkCreation`, `compute.vmExternalIpAccess` denied to
   all. `iam.allowedPolicyMemberDomains` is optional and off: a wrong customer
   id locks the owner out of their own folder. On the folder, not the
   organisation, because the organisation may hold things this repository did
   not create.

6. **Nothing the foundation creates bills while idle.** Folders, projects,
   API enablement, budgets and policies are free; the foundation's state is a
   few kilobytes in one bucket. Everything that runs is the platform root's,
   behind the owner's switches in each environment's tfvars, which this record
   does not change.

7. **Account identifiers are runtime inputs.** `ORG_ID`,
   `BILLING_ACCOUNT_ID` and the prefix are arguments to
   `scripts/bootstrap-foundation.sh` and `-var`s to one plan; they are written
   to no file. Project ids derived from the prefix are committed where the
   platform reads them, as they always were.

8. **The project number is recorded, not guessed.** Google assigns it at
   creation, and every workflow builds its workload-identity audience from it,
   so it cannot be committed before the project exists. dev's tfvars carry
   `project_number = 0` — the value the unprovisioned environments already
   carry — and every identity step refuses 0 beside a real project id by name,
   as does `var.project_number` at plan time. The bootstrap writes the real
   number into the tfvars and the two manifests that embed it, and the owner
   commits that diff.

9. **One command, then workflows.** The owner runs
   `scripts/bootstrap-foundation.sh` in Cloud Shell; it plans, shows, asks,
   applies the foundation, records dev, and chains to
   `scripts/bootstrap-deploy.sh dev`, which asks again before the platform
   root's first apply. It never creates a key, never auto-approves, never
   pushes. Everything after is `infra.yml` → `deploy.yml` → `infra.yml apps`
   under Workload Identity Federation.

10. **`algorik-platform-dev` is retired by the owner once the new project is
    healthy** — a project shutdown, not a Terraform destroy, because what is
    left in it is outside every state. The runbook gives the command; no
    workflow runs it.

## What it costs

- A second state and a second root to keep green. The tests under
  `infrastructure/foundation/tests/` plan it with a mocked provider, and
  `tenant_foundation` holds its API list equal to the platform root's.
- Two lists of the platform's unconditional APIs. The acceptance test is the
  price of not making the foundation read the platform root's module.
- A commit after the bootstrap. The number cannot be known sooner; the
  alternative, a workflow looking it up, needs a credential the workflow
  cannot have before it has the number.
- The fleet (ADR 0102) runs in dev's project, read from dev's tfvars, so it
  moves with it — but not by itself: its own state bucket and grants are the
  manual steps in `infrastructure/fleet/README.md`, redone in the new
  project before `fleet.yml` can plan there. Whatever the fleet held in the
  old project goes with the old project. This record does not bootstrap it.

## What would make this wrong

- The second region arrives (ADR 0113 §4's trigger). The folder then gains a
  folder per environment and per-concern projects; this root is where that
  happens, and this record is amended rather than replaced.
- The owner's account cannot hold the policy-admin role. `org_policies_enabled
  = false` is the answer, and `controls_not_applied` says what is missing.
- Billing budgets prove insufficient as a control. A budget alerts; it does not
  stop spending. A Pub/Sub-driven cap would be a new record, not a tweak here.

## The paper-trading boundary

Untouched. No `autonomy_ceiling` moves; Terraform still refuses
`supervised_live`, `limited_autonomous_live` and `autonomous_live` at plan
time; the composition roots and the type system are not touched. The
foundation creates no workload and holds no venue credential.

## Gates

`terraform fmt -check -recursive infrastructure/`, `terraform validate` and
`terraform test` in `infrastructure/foundation` and `infrastructure/terraform`,
`shellcheck scripts/bootstrap-foundation.sh`, `./scripts/check-secrets.sh`,
`cargo test -p qip-acceptance --no-fail-fast`. No apply was run by anyone
writing this record; the first is the owner's.
