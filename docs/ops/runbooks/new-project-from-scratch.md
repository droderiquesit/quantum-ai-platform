# A new project, from scratch

How the owner stands the platform up in a brand-new Google Cloud project, and
then retires the old one. The layout and the reasons are
[ADR 0118](../../adr/0118-the-gcp-tenant-is-a-platform-folder-holding-one-labelled-budgeted-project-per-environment-and-a-seed-project-applied-once-by-the-owner.md);
this page is the sequence.

**This page said "Nothing on this page has been run" until 2026-10-24, and
that is no longer what the record shows.** `algorik-qip-dev` now has project
number `101249892564` (`project_number` in
`infrastructure/environments/dev/terraform.tfvars`; the portal's old URL in
the same file carries the previous project's `523718313246`), and `infra.yml`
runs 108–122 operated that project. Run 121 (`38988479925`, `apps dev`,
2026-10-17) printed `qip-dev is Synced and Healthy`. Which command created the
project is not recorded in the tree, so read the steps below as the sequence,
not as a transcript of one run.

Paper trading only, before, during and after: no step here moves an autonomy
ceiling, and `infrastructure/terraform/variables.tf` still refuses the three
live levels at plan time.

## What you need

- Owner rights at the organisation — or, with no organisation, on the
  billing account: `resourcemanager.folderCreator`,
  `resourcemanager.projectCreator`, `billing.user` on the billing account,
  and `orgpolicy.policyAdmin` for the folder's policies (without it, set
  `org_policies_enabled = false` in `infrastructure/foundation/terraform.tfvars`;
  the apply then lists what it did not set).
- Three values, typed at the prompt and never committed:
  - **ORG_ID** — `gcloud organizations list` (the `ID` column), or `none`.
  - **BILLING_ACCOUNT_ID** — `gcloud billing accounts list` (`ACCOUNT_ID`).
  - **PREFIX** — every project is `<PREFIX>-<env>`. The repository is
    committed for **`algorik-qip`**, so dev is `algorik-qip-dev`. Project ids
    are global and can never be reused, so if `algorik-qip-dev` is taken the
    script offers to rewrite the committed id to your prefix and shows you
    every file.

Review `infrastructure/foundation/terraform.tfvars` first: the budgets (dev
100, seed 5, in the billing account's currency), the `owner` and
`cost-center` labels, and which environments are on (dev only).

## The one command

In [Cloud Shell](https://shell.cloud.google.com), signed in as yourself:

```
git clone https://github.com/droderiquesit/quantum-ai-platform.git
cd quantum-ai-platform
scripts/bootstrap-foundation.sh <ORG_ID|none> <BILLING_ACCOUNT_ID> algorik-qip
```

It needs `gcloud`, `terraform` 1.9.x, `git` and `jq`. Cloud Shell has the
first, third and fourth; if `terraform version` does not print `Terraform
v1.9.`, install 1.9.8 from
`https://releases.hashicorp.com/terraform/1.9.8/` into `~/bin` first.

### What it asks, in order

1. *Only if your prefix is not `algorik-qip`:* "dev's committed project id is
   'algorik-qip-dev', but PREFIX gives '…'. This rewrites … Type 'yes'". It
   lists the files first.
2. After the foundation plan: **"Apply exactly this plan? Type 'yes'"**. Read
   the plan. Expect, in org mode: 1 folder; 5 organisation policies (6 with
   domain-restricted sharing);
   1 project created (`<prefix>-dev`) and 1 updated in place (the seed, moved
   into the folder and labelled); 19 API enablements on dev and 7 on the seed;
   2 budgets. Nothing else. In no-org mode: no folder and no policies.
3. `scripts/bootstrap-deploy.sh dev` then runs and shows the **platform
   root's** plan, and Terraform asks its own `Enter a value:` — type `yes`
   only after reading it. It creates the workload-identity pool
   `qip-github-dev`, the account `qip-infra-dev`, the state bucket
   `<prefix>-dev-qip-tfstate`, and the rest of the platform root as committed,
   including — because dev's tfvars say `gitops_enabled = true` — the
   control-plane cluster. **That cluster bills while it exists.** If you do
   not want it yet, answer `no` there, set `gitops_enabled = false` in
   `infrastructure/environments/dev/terraform.tfvars` (in a fresh project this
   plans cleanly; ADR 0093's refusal applies only to a state that already
   holds the cluster), and rerun the script — every earlier step is skipped
   as already done. Without the cluster nothing deploys: Cloud Run services
   are reconciled by Config Connector on it (ADR 0036).

### What it prints at the end

```
Created or adopted (re-running this script changes none of it):
  folder        folders/<id>
  seed project  algorik-qip-seed  — foundation state in gs://algorik-qip-seed-foundation-tfstate
  project       algorik-qip-dev  number <n>  (dev)
  budgets       algorik-qip-dev=100, algorik-qip-seed=5  (alerts at 50/90/100%)
  platform      algorik-qip-dev-qip-tfstate, pool qip-github-dev, account qip-infra-dev, …
Not applied:
  - iam.allowedPolicyMemberDomains (optional; no customer ids given)
```

followed by `git status` of the files it wrote — at least
`infrastructure/environments/dev/terraform.tfvars` (`project_number = <n>`),
`infrastructure/gitops/envs/dev/invokers.yaml` and `portal.yaml` — and the
commands to commit them on a branch.

### Then commit the number

The workflows read only committed tfvars, and every one of them refuses
`project_number = 0` with "the number Google assigned to algorik-qip-dev has
not been recorded yet". Commit what the script wrote on a branch, open a pull
request, and merge it. These are protected paths: an agent may not merge
them for you.

## The workflow sequence

All under Workload Identity Federation, from GitHub → Actions:

1. **`infra.yml`**, `environment=dev`, `action=plan`. Read it. In a fresh
   project after the bootstrap's apply it should be close to no changes.
2. **`infra.yml`**, `action=up`. Applies Terraform and installs the
   controllers on the control plane. The bootstrap destroyed nothing here, so
   the "reclaim what the teardown left standing" step finds nothing to import.
3. **`vendor.yml`**, then **`deploy.yml`**. The new registry is empty: every
   digest in `infrastructure/gitops/envs/dev/kustomization.yaml` and the
   controller overlays was built into the old project's registry and must be
   re-mirrored and rebuilt. `deploy.yml` refuses while `ci` is red on the
   commit.
4. **`infra.yml`**, `action=apps`. Applies the Argo CD Application and the
   Kargo chain and waits for `qip-dev` Synced and Healthy.

The Argo CD image is no longer the blocker this paragraph named until
2026-10-24. Upstream `quay.io/argoproj/argocd` still fails `vendor.yml`'s
CRITICAL gate (ADR 0093), but `vendor.yml` now derives
`vendor/argocd-patched` (`infrastructure/egress/derived-images.txt`), and run
31 (2026-10-12) scanned it at 0 CRITICAL and attested it (`e3f81695`). Step
3's mirror therefore includes a gated Argo CD image, and step 4 reached
Synced and Healthy in run 121.

## How to verify

```
gcloud projects describe algorik-qip-dev --format='value(parent.type,parent.id,labels)'
gcloud billing projects describe algorik-qip-dev --format='value(billingEnabled)'
gcloud billing budgets list --billing-account=<BILLING_ACCOUNT_ID> --format='table(displayName,amount.specifiedAmount.units)'
gcloud resource-manager org-policies list --folder=<FOLDER_ID>
gcloud iam workload-identity-pools describe qip-github-dev --location=global --project=algorik-qip-dev
gcloud iam service-accounts keys list --iam-account=qip-infra-dev@algorik-qip-dev.iam.gserviceaccount.com --managed-by=user   # must be empty
```

and an `infra.yml` `plan` run whose log shows the identity step passing and
`No changes.` or only the changes you expect.

## How to tear down

- **The platform in dev:** `infra.yml` `action=teardown`, as before (it
  leaves KMS keys and `force_destroy = false` buckets standing, by design).
- **An environment's project:** set `enabled = false` for it in
  `infrastructure/foundation/terraform.tfvars` and rerun the script. The plan
  will **refuse**: projects carry `deletion_policy = "PREVENT"`, because a
  deleted project id can never be used again and takes every key and audit
  log with it. Deleting one is a deliberate act: `gcloud projects delete
  <id>` (30 days recoverable with `gcloud projects undelete`), then
  `terraform -chdir=infrastructure/foundation state rm
  'google_project.environment["<env>"]'`.
- **The foundation itself** is not torn down; it costs nothing while idle.

## Retiring the old project

`algorik-platform-dev` is retired **after** the new project serves (or at
least plans and applies cleanly), never before — until then it is the only
record of what ran.

1. Confirm nothing still points at it:
   `git grep -n algorik-platform-dev -- infrastructure scripts .github`
   should print only comments and history.
2. Save anything you want to keep: the old state bucket
   `gs://algorik-platform-dev-qip-tfstate` (`gcloud storage cp -r` it into the
   seed project), and the fleet's `gs://algorik-platform-dev-fleet-tfstate` if
   the fleet ran there (ADR 0102; the fleet's own README is its bootstrap in
   the new project).
3. Shut it down — one command, as the owner:
   ```
   gcloud projects delete algorik-platform-dev
   ```
   This is a shutdown, not a Terraform destroy: the KMS keys, the
   `force_destroy = false` buckets, the empty VPC and the state bucket are
   outside every state, and only deleting the project removes them. Billing
   stops immediately; the project is recoverable with
   `gcloud projects undelete algorik-platform-dev` for 30 days and then
   gone, id included.
4. Unlink it from the billing account if it still shows:
   `gcloud billing projects unlink algorik-platform-dev`.

No workflow and no agent runs step 3.
