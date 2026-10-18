# Break-glass: changing a running environment when GitOps cannot

Companion to [the operator runbook](../RUNBOOK.md) and
[the incident playbooks](incidents.md); same labels (**ran**, **repo**,
**not run**). Written 2026-10-18 against the configuration as committed. No
process of this platform runs in any cloud environment, so this procedure has
never been used; it is built from what the controllers would do.

**The rule it serves** (requirement R12-RES-10133, blueprint §22): if GitHub
or the GitOps control plane is lost, the running system is unchanged and no
new deploy happens; an emergency change goes through a pre-approved
break-glass path and is reconciled to Git afterwards. The first half is
structural — every `RunService` carries `cnrm.cloud.google.com/deletion-policy:
abandon`, so losing Argo CD or Config Connector releases a service and never
deletes it (`gitops.rs::a_cloud_run_service_with_cnrm_deletion_policy_abandon_survives_gitops_control_plane_loss`).
This page is the second half.

## When this page applies, and when it does not

Use it only when **both** hold:

1. A running service must change *now* to stop harm — a halt that the
   kill-switch file cannot deliver, a revision serving a digest that must stop
   serving — and
2. the Git path cannot deliver the change in time: GitHub is unreachable, the
   `deploy.yml` gate cannot run, or the GKE control plane that hosts Argo CD
   and Kargo is suspended (ADR 0093) or unreachable.

It does **not** apply to a rollback that Git can carry
([incidents.md, "A failed deploy"](incidents.md#a-failed-deploy)): revert the
promotion commit and let Argo CD reconcile. It never applies to a change of
autonomy ceiling: `AutonomyLevel::deployable` refuses a live ceiling in the
binary and Terraform refuses it at plan time, and no emergency changes either
(`.claude/rules/01-security-and-safety.md`).

## Who may act

The owner, or a person the owner has named in writing for the incident,
acting under their **own** Google identity through IAM. This repository
carries no account identifier, so there is no committed break-glass role to
assume: the authority is whatever `roles/run.admin` and
`roles/iam.serviceAccountUser` the person already holds in the environment's
project, granted outside this repository. An agent never acts here —
`scripts/agent-policy.json` refuses an agent actor on `infra.yml`, and this
page is a human procedure.

Two people are involved whenever two are reachable: one runs the command,
the other reads it back before it runs. When one person is all there is, the
record below is what the second reader checks afterwards.

## What may be changed

Only the two operations that cannot cause a new deploy:

| Operation | Command shape | Why it is allowed |
|---|---|---|
| Stop a revision serving | `gcloud run services update-traffic <service> --region <region> --to-revisions <previous>=100` | Moves traffic between revisions Binary Authorization has **already admitted**; introduces no image |
| Halt a workload | `gcloud run services update <service> --region <region> --min-instances 0 --max-instances 0` | Scales to nothing; introduces no image |

Nothing else. In particular, **not** `gcloud run deploy` with any image, and
**never** `--breakglass` or a `breakglassJustification` on a `RunService`:
that bypasses the admission policy for the revision, and
`gitops.rs` refuses any manifest that carries one. An image that is not
attested is not deployed by anyone, in any emergency.

## What is recorded, at the time

Before the command runs, in the incident record (a file under
`docs/ops/incidents/` on a branch, or a local file if GitHub is down, to be
committed when it is back):

1. The exact command, the project, the environment, and the UTC time.
2. The reason, in one sentence naming the harm being stopped.
3. Who ran it and who read it back.
4. The `terraform output console_front_door` or `gcloud run services describe`
   line showing the state *before*.

After it runs: the same `describe` line showing the state *after*. Cloud
Audit Logs record the call under the person's identity as well; the record
here is so a reviewer reads one page rather than a log query.

## Reconciliation to Git — the step that closes the break-glass

The change is not finished until Git says the same thing the environment
does. Within one working day of GitHub being reachable:

1. **Open a pull request** that makes the committed state match the
   emergency state, or that deliberately undoes it:
   - traffic moved to a previous revision: edit
     `infrastructure/gitops/envs/<env>/kustomization.yaml`'s `images` block to
     the digest that revision serves, so the next sync does not move traffic
     back;
   - a workload halted: set the service's scaling in
     `infrastructure/gitops/envs/<env>/<service>.yaml` to the halted values, or
     restore the service through Git when the incident is over.
   The pull request links the incident record.
2. **Let the controller reconcile.** `dev` syncs on its own with `prune` and
   `selfHeal` (`gitops.rs::only_dev_syncs_automatically_and_it_prunes_and_heals`),
   which means a break-glass change in `dev` is *reverted by the next
   reconcile* unless step 1 lands first or the Application's automated sync is
   paused for the incident — pause it, and record that you did. `test`,
   `stage` and `prod` sync when a person syncs them, so the manual change
   stands until the pull request is merged and a person syncs.
3. **Prove it.** The post-sync hook's `serves` line, or `gcloud run services
   describe`, showing the environment matches the merged commit, goes into
   the incident record, and the record closes with the pull request number.

A break-glass change with no pull request is drift, and drift in `dev` is
undone by the controller while drift in `prod` is a deployment nobody can
reproduce from the repository. Either way the event log cannot explain it,
which is the one thing this platform is for.

## What this page cannot claim

- **Not run.** No environment runs a service, so no step here has been
  exercised. The first use records the actual commands and their output on
  this page, with the date.
- The IAM a person holds is outside this repository, so nothing here can
  prove the person who needs to act can. Check it in a game day, not in an
  incident: `gcloud projects get-iam-policy <project>` filtered to your own
  identity.
