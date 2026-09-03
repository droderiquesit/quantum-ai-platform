# Wave 7 CI/CD status — infra.yml and deploy.yml against `dev`

Read-only audit. No workflow file, tfvars, or any path outside this new
document was touched. Source: the GitHub REST API
(`api.github.com/repos/droderiquesit/quantum-ai-platform/actions/...`),
reachable unauthenticated for this public read; run IDs, timestamps and log
lines below are quoted from that API's JSON and from
`GET /actions/jobs/{id}/logs`, not inferred from the coloured Actions UI.
Repository state read at `HEAD` of
`claude/algorik-architecture-refactor-pmp0zy` (`624a89a`), infra and
deploy workflow definitions as committed there.

## 1. The last successful (or furthest-progressed) `infra.yml` run

The most recent run whose job **concluded `success`** is run **#15**
(`id 33682113626`, `https://github.com/droderiquesit/quantum-ai-platform/actions/runs/33682113626`),
dispatched `2026-09-02T20:54:58Z`, completed `2026-09-02T20:56:07Z`, on
`claude/algorik-architecture-refactor-pmp0zy` at head `a4811b808b01d93912ef610a75705b4e4fb65651`.
Its single job is named `plan dev` — the dispatch inputs were
`environment=dev`, `action=plan` — and every step through `terraform init`,
`plan` and `what exists now` reports `success`; `up` and `down` show
`completed skipped`, which is the workflow's own `if:` gating doing its job,
not a partial failure.

What that plan actually found, quoted from the job log
(`GET /actions/jobs/100420884844/logs`):

```
Plan: 61 to add, 4 to change, 70 to destroy.
```

and, from the `what exists now` step:

```
157 resources in dev state
```

This was **read-only** — nothing was created, changed or destroyed. `dev`'s
Terraform state at that moment held 157 resources, and the committed
configuration disagreed with 135 of them (61 to add, 4 to change, 70 to
destroy) — consistent with a state that still carries GKE-era resources the
ADR 0024 cutover (`808ca32`, "Wire the blueprint runtime into the root
module and take the cluster's Terraform with it") replaced in code but that
have not yet been reconciled by a completed `apply`.

**No `up` (apply) run against the current, post-cutover `infra.yml` has ever
succeeded.** Every `up dev` dispatch on `claude/algorik-architecture-refactor-pmp0zy`
(runs #13, #14, #16–#25 — ten dispatches between `2026-09-02T20:30:15Z` and
`2026-09-03T01:12:31Z`) concluded `failure`. The furthest-progressed `up` is
the most recent, **run #25** (`id 33702713985`,
`https://github.com/droderiquesit/quantum-ai-platform/actions/runs/33702713985`,
dispatched `2026-09-03T01:12:31Z`, concluded `2026-09-03T01:13:51Z`,
conclusion `failure`). Its plan was small — the state had converged a great
deal since run #15 —

```
Plan: 5 to add, 3 to change, 1 to destroy.
```

and then failed applying that plan with this exact error
(`GET /actions/jobs/100485242380/logs`, line 709):

```
Error: Error deleting contents of object envoy-16bc4c53e18e7501.yaml: googleapi: Error 403:
qip-infra-dev@algorik-dev.iam.gserviceaccount.com does not have storage.objects.delete access
to the Google Cloud Storage object. Permission 'storage.objects.delete' denied on resource
'//storage.googleapis.com/projects/_/buckets/qip-egress-dev-algorik-dev/objects/envoy-16bc4c53e18e7501.yaml'
(or it may not exist)., forbidden
```

Its own `what exists now` step reported `145 resources in dev state` —
12 fewer than run #15 reported six hours earlier, i.e. the environment moved
under other dispatches in between (consistent with the parallel session this
task named; not asserted as this audit's own doing, since this audit made no
mutating call).

The only `infra.yml` run that ever reached `up ... success` at all is run
**#10** (`id 32796139654`, job `up dev`, conclusion `success`,
`2026-08-25T01:04:32Z`–`2026-08-25T01:19:13Z`), but its step list — `recover
a cluster the taint has deadlocked` in place of the current workflow's
`repair a tainted service Cloud Run still has` — and its head SHA
(`9854ec77d733a4007d269f18644a3822916d6d04`, on the older
`claude/autonomous-investment-platform-76gt4y` branch, `git log --oneline`
titled "Give the pool that fails the quota the disks that fit it") place it
**before** the ADR 0024 cutover: it applied the GKE-era Terraform, not the
Cloud Run one this task is auditing. `git merge-base --is-ancestor
9854ec77d7… 808ca32` returns false — it is not an ancestor of the cutover
commit at all; it is on the sibling, superseded branch. It is not evidence
that the current `infra.yml` has ever applied cleanly.

## 2. Has `deploy.yml` ever succeeded against the environment `infra.yml` set up, and what state are the three services in

**No `deploy.yml` run on the current branch/workflow definition has fully
succeeded.** The four dispatches against
`claude/algorik-architecture-refactor-pmp0zy` (runs #103–#106, all
`workflow_dispatch`, `environment=dev`, between `2026-09-02T22:39:14Z` and
`2026-09-02T23:59:37Z`) all conclude `failure`, and three of the four
(#104, #105, #106) get the images job (`build and push qip-api`,
`qip-fastbrain`, `qip-deepbrain`, `qip-edge-node`) to `success` — build,
scan, sign and attest all pass — then fail in the `deploy to cloud run
(dev)` job every time. Run #103 fails earlier, at `the test suite passed`.

The most recent, run **#106** (`id 33697544122`,
`https://github.com/droderiquesit/quantum-ai-platform/actions/runs/33697544122`,
dispatched `2026-09-02T23:59:37Z`), fails moving `qip-dev-api`
(`GET /actions/jobs/100470621853/logs`):

```
Creating Revision.......................................................failed
Deployment failed
ERROR: (gcloud.run.services.update) The user-provided container failed the configured startup probe checks.
Logs URL: https://console.cloud.google.com/logs/viewer?...resource.labels.revision_name%3D%22qip-dev-api-00005-b9c%22
```

and the workflow's own "why the revision did not start" diagnostic step (the
one added specifically to avoid sending a person to fetch this by hand)
pulls the real cause from Cloud Logging:

```
qip-egress   STARTUP HTTP probe failed 15 times consecutively for container "qip-egress"
             on port 9900 path "/healthz". The instance was not started.
```

`qip-dev-api-00005-b9c` is the fifth revision Cloud Run has attempted for
this service, so the service (`qip-dev-api`) and at least four earlier
revisions exist — the deploy script's own existence check
(`gcloud run services describe qip-dev-api`) would have refused the run with
a different, explicit message ("… does not exist … Run the infra workflow
first …") had the service not been there, and that message does not appear
in the log. **Runs #104 and #105 fail at the identical point**
(`deploy to cloud run (dev)` job, same egress-sidecar startup-probe symptom,
confirmed from their job lists — `id 100460466965` and `100465338910`
respectively, both `failure`), so this is a repeated, not a one-off,
failure mode on this branch.

What this audit **can** say about the three services' state, and no more:

- `qip-dev-api`, `qip-dev-fastbrain`, `qip-dev-deepbrain` exist in
  `algorik-dev`/`us-east4` — every recent deploy run got past the
  "does not exist" refusal for `api` (the first name the catalogue-driven
  loop tries) without printing it.
- `qip-dev-api`'s most recent attempted revision (`00005-b9c`, from run
  #106, `DEPLOY_SHA=6d52f8ebc981c179b8953cba8647f75a7bbd873b`) never became
  Ready — Cloud Run does not move traffic to a revision that fails its
  startup probe, so whatever revision was Ready before this attempt (if any)
  is still the one serving, or none is, if this was `api`'s first revision
  ever to reach `Ready`. **This audit did not call `gcloud run services
  describe` or any equivalent** — it has no cloud credentials and none were
  requested — so which revision `qip-dev-api` actually routes traffic to
  right now, and whether `fastbrain` and `deepbrain` (never reached by this
  loop, since it stops at the first failure) carry the GKE-era digests still
  recorded in `infrastructure/environments/dev/images.tfvars` or something
  newer, is **not established by this audit** and should not be reported as
  known.
- `infrastructure/environments/dev/images.tfvars` — read directly from the
  worktree, not from the API — carries this header:

  ```
  # These three are the digests the GKE runtime's last reconciled values file
  # carried, which Binary Authorization admitted on that cluster: the same
  # bytes, in the same registry, at the same digest. Nothing here has been
  # deployed to Cloud Run — see ADR 0024 — so the first pipeline run overwrites
  # this file with what it actually moved the services to.
  ```

  and `git log --oneline -- infrastructure/environments/dev/images.tfvars`
  shows exactly **one** commit ever touching it — `808ca32`, the cutover
  commit itself. `deploy.yml`'s own "commit the deployed digests" step never
  ran (it is gated on the rollout step succeeding, and the rollout step has
  never succeeded on this branch), so **the digests this file names have
  never been confirmed served by a successful Cloud Run rollout**; they are
  carried over from the GKE runtime's last state, not attested against
  Cloud Run.

## 3. A workflow-level correctness issue found from reading the YAML

`.github/workflows/infra.yml`'s `the state bucket grant` step
(lines 129–134) scopes its one IAM exception narrowly, and says why in the
comment above it: the account's project-level role deliberately lacks
`storage.objects.delete` so nothing it runs can delete from the evidence
bucket, and the grant here is the single, bucket-scoped exception —

```yaml
- name: the state bucket grant
  run: |
    gcloud storage buckets add-iam-policy-binding \
      "gs://${{ steps.identity.outputs.project }}-qip-tfstate" \
      --member="serviceAccount:${{ steps.identity.outputs.account }}" \
      --role="roles/storage.objectAdmin" --quiet >/dev/null
```

— naming **only** the `-qip-tfstate` bucket. But `main.tf`'s
`module.egress_proxy` (via `modules/egress-proxy`) manages a
`google_storage_bucket_object.bootstrap` in a *different* bucket,
`qip-egress-<env>-<project>` (`qip-egress-dev-algorik-dev` in dev), and that
object is content-addressed — its name changes when the rendered Envoy
bootstrap changes (`envoy-16bc4c53e18e7501.yaml` in the failing plan), which
means Terraform must **delete the old object** on every apply that changes
the bootstrap. Nothing in this workflow, and nothing declaratively in
`modules/cicd` (per the same comment block's own account of what that
module grants), gives `qip-infra-<env>` delete on the egress bucket. This is
not a hypothetical: it is the literal, current blocker — run #25's `up`
failed with exactly

```
Error: Error deleting contents of object envoy-16bc4c53e18e7501.yaml: googleapi: Error 403:
qip-infra-dev@algorik-dev.iam.gserviceaccount.com does not have storage.objects.delete access
```

The state-bucket grant pattern is right in shape (a narrow, bucket-scoped
exception rather than widening the project role) but incomplete in scope: it
was written for the one bucket Terraform's *state* needs to overwrite, and
the egress bootstrap object is a second bucket Terraform's *plan* needs to
mutate that nobody extended the same pattern to. The fix this audit is not
authorised to make (`.claude/rules/domains/infrastructure.md` forbids
widening an IAM grant to make an error go away, and names the answer as
"find the one missing permission and add that") is a second, equally narrow
`add-iam-policy-binding` against `qip-egress-<env>-<project>` — or the
equivalent declarative grant in `modules/cicd`, if that module wants to hold
it, since the workflow's own comment (lines 123–128) already picks that
module as the place a *reviewed, deliberate* exception should live rather
than accumulate ad hoc in the workflow step.

A second, lower-severity observation: `deploy.yml`'s rollout step
(`move every service to the attested digest and prove it serves it`)
iterates `$PAIRS` — `api fastbrain deepbrain`, in `catalogue.tf`'s
declaration order — inside one `run:` block under `set -euo pipefail`, and
`exit 1` on the first service whose `gcloud run services update` or
serving-proof check fails. There is no `continue`, and no per-service
`if:`, so a failure moving `api` (the case observed in every recent run)
means `fastbrain` and `deepbrain` are **never even attempted** in that
dispatch — not skipped-and-reported, simply not reached. This is arguably
correct behaviour for a pipeline whose network the report above says nothing
about ordering guarantees for, and stopping rather than leaving two of three
services silently unmoved is defensible; but it does mean that "the deploy
job failed" conflates three different questions — did `api`, `fastbrain`
and `deepbrain` each get a rollout attempt — into one job conclusion, and
nothing in the four recent failing runs distinguishes "fastbrain would have
failed too" from "fastbrain was never tried." This audit does not have
evidence either way for `fastbrain` or `deepbrain` on `claude/algorik-architecture-refactor-pmp0zy`,
and says so rather than assuming the egress-sidecar probe failure that hit
`api` would necessarily repeat for them (they carry the same sidecar
configuration per `catalogue.tf`, which makes it plausible, but plausible is
not evidence).

## 4. Plain summary

As of this audit (`2026-09-03`, read against the GitHub API with no
mutating calls made):

- **`infra` is at run #25 (dev), conclusion `failure`** — `up dev`,
  dispatched `2026-09-03T01:12:31Z`, failed applying a 5-add/3-change/1-destroy
  plan on `storage.objects.delete` denied against the egress-config bucket.
  The most recent `infra` run to conclude `success` is run #15 (dev),
  a read-only `plan` on `2026-09-02T20:54:58Z` that itself reported 157
  resources in state against a 61-add/4-change/70-destroy plan — i.e. `dev`
  was not applied to convergence even then. No `up` against the current,
  post-cutover `infra.yml` has ever succeeded; the one successful `up`
  in the workflow's history (run #10, `2026-08-25`) predates the ADR 0024
  Cloud Run cutover and applied the retired GKE Terraform.
- **`deploy` has never fully succeeded against this environment.** The most
  recent attempt, run #106, built, scanned, signed and attested all four
  images successfully, then failed moving `qip-dev-api` to the new revision
  because its `qip-egress` sidecar failed its startup HTTP probe on
  `:9900/healthz` fifteen times in a row. `fastbrain` and `deepbrain` were
  never reached in that run. `images.tfvars` in `dev` still names the
  digests carried over from the retired GKE runtime, not anything a
  Cloud Run rollout has attested.

## Sources

- `GET /repos/droderiquesit/quantum-ai-platform/actions/workflows/infra.yml/runs?per_page=25`
- `GET /repos/droderiquesit/quantum-ai-platform/actions/workflows/deploy.yml/runs?per_page=25`
  and `?status=success&per_page=10`
- `GET /repos/droderiquesit/quantum-ai-platform/actions/runs/{id}` and
  `/jobs` for runs `33682113626`, `33702713985`, `33697544122`,
  `33695801022`, `33694165521`, `33691422584`, `32796139654`, `33396101167`
- `GET /repos/droderiquesit/quantum-ai-platform/actions/jobs/{id}/logs` for
  jobs `100420884844`, `100485242380`, `100470621853`
- `.github/workflows/infra.yml`, `.github/workflows/deploy.yml`,
  `.github/workflows/vendor.yml`,
  `infrastructure/terraform/main.tf`, `infrastructure/terraform/catalogue.tf`,
  `infrastructure/environments/dev/images.tfvars`,
  `infrastructure/environments/dev/terraform.tfvars`, all read at
  `624a89a` on `claude/algorik-architecture-refactor-pmp0zy`
- `git log`, `git merge-base --is-ancestor` in this worktree, for the
  authorship and ancestry claims about commits `9854ec7`, `808ca32`,
  `a7a8534`
