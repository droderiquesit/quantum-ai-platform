# Domain: infrastructure and cloud

**Scope** — `infrastructure/**`, `.github/workflows/**`

## Approved

- Terraform 1.9.8, `hashicorp/google ~> 6.12`. Modules under
  `infrastructure/terraform/modules/`; environments under
  `infrastructure/environments/<env>/terraform.tfvars`.
- **Workload Identity Federation only.** No service-account keys anywhere,
  including in examples.
- Secrets reach pods as files via the Secret Manager CSI driver.
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
