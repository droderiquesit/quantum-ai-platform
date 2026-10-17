# Domain: infrastructure and cloud

**Scope** — `infrastructure/**`, `.github/workflows/**`

## Approved

- Terraform 1.9.8, `hashicorp/google ~> 6.12`. Modules under
  `infrastructure/terraform/modules/`; environments under
  `infrastructure/environments/<env>/terraform.tfvars`.
- **Workload Identity Federation only.** No service-account keys anywhere,
  including in examples.
- Secrets reach a process as files: a Secret Manager volume on a Cloud Run
  service, a tmpfs the execution node's startup script fetches into. Never an
  environment value (ADR 0024).
- Binary Authorization on every deployed image; upstream images pinned by
  digest, never by tag — a policy that trusts a tag trusts whoever can push it.
- Workflows derive their identity from committed tfvars. A repository variable
  is forbidden: one set from a broken shell once carried an apt install
  advisory into the workload-identity audience, and every run afterwards failed
  on an audience nobody could explain.

## Prohibited

- Applying without showing the plan.
- Deleting cloud resources without explicit approval naming the resource.
- Any new `${{ vars.* }}` in a workflow — an acceptance test refuses it.
- Widening an IAM grant to make an error go away. Find the one missing
  permission and add that.
- Touching resources this repository did not create.

## Required evidence

`terraform fmt -check`, `terraform validate`, the `infrastructure` acceptance
suite, and — for a validation change — a real plan proving the gate fires on a
bad value **and admits a good one**. The second half is what distinguishes a
working gate from one that refuses everything.

## Known gap (flagged 2026-10-17, no rule changed)

`.claude/rules/02-change-management.md` says `prod` is refused by both
`infra.yml` and the deploy gate. Read against the workflows, only half of that
holds as written: `infra.yml` refuses `prod` unconditionally (lines 261-264,
"prod infrastructure is never changed from this workflow"), while
`deploy.yml` lists `prod` as a `workflow_dispatch` option (lines 65-69) and its
refusal step (lines 135-137) fires only when `TARGET_ENVIRONMENT == 'prod'`
**and** the event is not `workflow_dispatch`. A human dispatch of `deploy.yml`
for `prod` is therefore not refused by that step; the remaining protection is
the human dispatch itself and whatever the later jobs and environment
protection require. Re-read both files before relying on either description.
