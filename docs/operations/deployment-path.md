# The deployment path

There is one way a change reaches a running state, and this document is it.
Every stage names the file that performs it and the thing that proves it
happened, because a deployment nobody can prove is a deployment nobody can
roll back, and a second path — a script, a Makefile target, a pipeline step
that also moves a service — is the place two claims about what is running
come to disagree.

**Argo CD and Kargo exist by [ADR 0036](../adr/0036-argo-cd-and-kargo-return-on-a-control-plane-cluster.md);
there is no other deployment path.** They run on a GKE Autopilot
control-plane cluster per environment, in the `management` trust zone, beside
Config Connector, and nothing else runs there: no `qip-*` binary is a Pod, and
the acceptance suite refuses one. The runtime they deploy *to* is the one
[ADR 0024](../adr/0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md)
provisions and ADR 0036 leaves alone — a Cloud Run service per warm binary,
and one Compute Engine execution node per region from `modules/execution-node`.
The version of this document written earlier on 2026-09-04 described the
pipeline moving each service with `gcloud run services update` and recording
`images.tfvars`; that mechanism is replaced, and its proof is kept in a
different place (below).

**Nothing below is applied yet.** ADR 0036 records a design three agents are
building; the first plan, the first bootstrap and the first sync are all
ahead. Every "proves" in this document is what the file says, until a run
shows it.

## The path

```
push to the default branch
  │
  ▼
ci.yml ──── the gate: fmt, clippy, tests, release build, dependency policy,
  │         audit, sbom, portal, landing, trivy fs, trunk, terraform
  │         fmt/validate, secret scan
  │ (workflow_run, success only)
  ▼
deploy.yml
  ├─ gate    refuses prod unless a person dispatched; refuses a commit
  │          ci did not pass
  ├─ images  one job per binary: build (labelled with the source sha) →
  │          scan → push by commit tag → sign and attest the registry's
  │          digest
  └─ nodes   roll the execution node groups under their policy
  │          (the one thing this workflow still moves; see gitops-exceptions.md)
  ▼
Artifact Registry (immutable tags; one image per commit per binary)
Binary Authorization (one attestor; default rule denies the unattested)
  │
  ▼
Kargo (on the control-plane cluster)
  ├─ Warehouse   sees the new digests; freight = one commit's images
  ├─ Stage dev   promotes automatically: a commit into
  │              infrastructure/gitops/envs/dev/ naming the digests
  ├─ Stage test  ┐ promote on a person's approval, same commit shape
  ├─ Stage stage ┘
  └─ Stage prod  exists; promotion refused by policy until an ADR lifts it
  │
  ▼
Argo CD (on the same cluster)
  ├─ Application dev          automated sync, prune + selfHeal
  ├─ Application test/stage   manual sync
  ├─ Application prod         manual sync, and nothing to sync
  └─ post-sync hook           reads the serving revision and fails the sync
                              unless it carries the manifest's digest
  │
  ▼
Config Connector (GKE addon) ──► Cloud Run services
infra.yml (dev/test/stage, manual dispatch) ─ plan / up / down ─► the
  foundation, the cluster, the controllers' bootstrap, the execution nodes
scripts/bootstrap-deploy.sh (first apply of any environment; the only
                             path that can touch prod)
```

Two side paths feed it and neither deploys the platform's own code:

- `vendor.yml` mirrors the third-party images the platform runs — the Envoy
  egress sidecar, OpenObserve, and now Argo CD's and Kargo's images, each a
  line in `infrastructure/egress/vendored-images.txt` by digest — into the
  environment's registry, proves the digest survived the copy, scans, and
  attests with the same attestor. It exists so the admission policy keeps
  exactly one rule with no exemptions, controllers included.
- `scripts/deploy-frontends.sh` deploys the portal and the landing. It is
  the one documented exception for a Cloud Run workload, and its own section
  below says why it is still a script.

## Every stage, and what proves it

| Stage | Where | What proves it |
|---|---|---|
| The commit passed the gate | `ci.yml`; `deploy.yml` job `gate` | On the automatic path, `github.event.workflow_run.conclusion == 'success'`; on a dispatch, `gh api .../workflows/ci.yml/runs?head_sha=<sha>&status=success` counting at least one. A dispatch for a commit ci never passed is refused with `no successful ci run for <sha>; nothing is deployed.` |
| Nobody deploys prod by accident | `deploy.yml` step `refuse a production deployment that nobody dispatched`; `infra.yml` step `refuse what this workflow must never do`; the prod `Stage`'s promotion policy under `infrastructure/gitops/` | All three refuse. The first two are unchanged from ADR 0024 and `production_is_never_deployed_automatically` and `the_infrastructure_workflow_cannot_touch_production` pin their text; the third is new and the acceptance suite reads it. Three refusals are three, not one with two spares |
| Identity comes from committed configuration | every workflow's `derive the identity from the tfvars` step | The provider, service account, attestor and key version are a pure function of `project_id`, `project_number` and `region` in `infrastructure/environments/<env>/terraform.tfvars`. `no_workflow_depends_on_a_repository_variable` refuses any `${{ vars.* }}`; the WIF pool's `attribute_condition` admits this repository's `refs/heads/*` and nothing else. The controllers hold no GitHub Actions identity at all: Config Connector, Argo CD and Kargo run under GKE Workload Identity as their own service accounts |
| The image is the commit | `deploy.yml` job `images`, steps `build` and `push` | Tagged `<binary>:<full sha>` and labelled `org.opencontainers.image.revision=<sha>`; `modules/registry` sets `immutable_tags = true`, so the tag names one image for ever. A commit whose image the registry already holds is not rebuilt (step `whether the registry already holds this commit's image`); an image built before the label existed carries none, and the promotion step falls back to the tag and says so |
| The image was scanned before anything could admit it | step `scan image` | `trivy image --severity CRITICAL,HIGH --exit-code 1`, before push, sign and attest; it runs on a reused image too |
| The bytes are attested | step `sign the pushed image` | The digest is read back from the registry, never from the local daemon, and `gcloud beta container binauthz attestations sign-and-create` names that digest with key version 1 of `qip-<env>-attestor`. An existing attestation for the same digest is success. What the attestation means and does not is in `infrastructure/terraform/modules/binaryauthorization/OUT-OF-BAND.md` |
| The freight is one commit's images | Kargo `Warehouse` and the promotion's first step | The `Warehouse` subscribes to each catalogue binary's repository and records digests. The promotion reads every image's revision label and refuses freight whose images name different shas — a slow matrix job cannot ship the API from one commit and the fast brain from another |
| The promotion is a commit | Kargo `Stage`, promotion step | The digests are written into `infrastructure/gitops/envs/<env>/` and committed to the default branch by `qip-kargo-<env>[bot]`, the message naming the freight, each digest, the source sha and the `deploy.yml` run. `dev` promotes on its own; `test` and `stage` on a person's approval in Kargo; `prod` never, until an ADR lifts the policy |
| The manifest holds the catalogue's invariants | `catalogue.tf`; the parity test in `backend/crates/tests/qip-acceptance/tests/` | Internal ingress for every catalogue workload, `binaryAuthorization.useDefault`, secrets as 0400 volumes with the `_FILE` path, image by digest, the workload's own service account, the zone's subnet and tag, `deletion-policy: abandon`, `QIP_AUTONOMY_CEILING` equal to `paper_trading`. A manifest disagreeing with its catalogue entry on any of these is a red build |
| The service exists and is acquired, not replaced | Config Connector on the first sync of an environment | `RunService` `metadata.name` is `qip-<env>-<name>`, the name Terraform created it under. Config Connector adopts a resource that exists by name; Terraform released it with `removed { lifecycle { destroy = false } }` so the apply that did so destroyed nothing. The evidence that acquisition was clean is a revision count that did not move on first sync, recorded in `docs/ops/missing-infrastructure-register.md` when it happens. A service that does not exist is *created* by Config Connector from the manifest — the one thing `deploy.yml` refused to do, and permitted here because the manifest carries everything Terraform's module carried |
| The service moved and the new revision is Ready | Argo CD `Application`, resource health | Argo CD's health check for `RunService` reads the resource's `Ready` condition; `Degraded` fails the sync. A revision Binary Authorization refused for want of an attestation shows here: the `RunService` never becomes `Ready`, and the sync fails naming the condition's message |
| The serving revision runs the attested bytes | Argo CD post-sync hook | Reads `status.traffic`, not `spec.template`: every revision with a non-zero share is described, and its container named after the catalogue entry must carry exactly the digest the manifest names. This is `prove-serving.py`'s two questions, moved from the pipeline to the reconciler and not weakened. `a_promotion_names_who_verifies_it` reads the hook rather than the workflow |
| Drift is put back | Argo CD `Application` for `dev`, `selfHeal: true`, `prune: true` | A service edited in the console is reconciled to the manifest on the next cycle; a resource removed from the manifest is released (`deletion-policy: abandon`), never destroyed. `test`, `stage` and `prod` sync when a person syncs them and drift shows as `OutOfSync` until then |
| The execution node was replaced under its policy | `deploy.yml` step `replace every execution node under its group's rolling policy` | `rolling-action replace --max-surge 1 --max-unavailable 0` per `qip-<env>-exec-*` group. This is outside GitOps and named as such in [gitops-exceptions.md](gitops-exceptions.md); the node's boot image is an image contract, not something this pipeline bakes (`modules/execution-node/README.md`) |
| Infrastructure changed only after a plan | `infra.yml` | `plan` is read-only; `up` applies with `-auto-approve` because the dispatch is the review, then runs the controllers' bootstrap (`kubectl apply` of the vendored, digest-pinned manifests under `infrastructure/gitops/bootstrap/`, with credentials from `gcloud container clusters get-credentials` under WIF); `down` is targeted at `module.execution_node` alone and leaves the cluster standing. Prod is refused by both the choice list and a step behind it; a person applies prod interactively with `scripts/bootstrap-deploy.sh prod`, which never auto-approves |

### Where "observable" stops

The last stage — the running process being seen — is not proven by this
path, and this document does not claim it. Nothing scrapes a Cloud Run
service today (`infrastructure/terraform/modules/observability/NOT-SCRAPED.md`),
the execution node's Ops Agent receiver reaches a node only once one exists,
and every alert policy is gated on `workload_metrics_exist = false` in every
environment. The evidence a deployment produces is the promotion commit, the
Argo CD sync record with the hook's `<service> serves <image> from <revision>`
line, and the `RunService` status. `.claude/rules/domains/observability.md`
holds the rest.

## Rollback

**What was verified by reading the design, not by running it.** No
promotion has happened.

A rollback is a git operation and nothing else:

- **Revert the promotion commit.** `git revert <promotion sha>` on the
  default branch puts the environment's manifests back to the digests they
  named before; Argo CD reconciles `dev` on its own and `test`/`stage` when
  a person syncs. The hook proves the serving revision carries the reverted
  digest, exactly as it proved the forward move.
- **Or re-promote earlier freight.** Kargo keeps every piece of freight the
  `Warehouse` produced; promoting an older one writes the same commit shape
  with older digests. Either way the record is a commit and the proof is a
  sync.

What is no longer a rollback, so nobody reaches for it:

- **Re-dispatching `deploy.yml` at a prior commit.** It still works for what
  it does now — it rebuilds nothing when the registry holds the commit's
  image, scans the registry's copy, and treats the existing attestation as
  success — but it moves no service. It produces freight; a person then
  promotes it, which is the same as re-promoting.
- **`infra.yml up`.** Terraform no longer manages the services. An apply
  changes nothing about what serves, and cannot.
- **`gcloud run services update` by hand.** In `dev` it is undone by
  `selfHeal` on the next reconcile; elsewhere it is drift an `OutOfSync`
  status names until somebody syncs over it. Neither is a rollback; both
  are the thing the reconciler exists to catch.

For `prod` there is no rollback because there is no promotion: the `Stage`
is refused by policy until an ADR lifts it.

**Not exercised.** The first revert, or the first re-promotion, is what
proves this section; until then it describes what the files say. The
evidence to record when it happens: the revert or promotion commit sha, the
Argo CD sync id, and the hook's `serves` line.

## Tracing a serving revision to a commit and an artefact

Every link in the chain is a fact something else recorded, so the walk works
in both directions without trusting the manifest alone:

1. **Revision → digest.** `gcloud run revisions describe <revision>
   --format=json`, the container named after the catalogue entry, its
   `image` field: `<region>-docker.pkg.dev/<project>/qip-<env>/<binary>@sha256:...`.
   The `RunService`'s `status` on the cluster says the same thing, and the
   two agreeing is what the hook checked.
2. **Digest → attestation.** `gcloud container binauthz attestations list
   --attestor qip-<env>-build --artifact-url <image@digest>`. Present means
   the pipeline signed exactly these bytes; absent means Cloud Run would
   have refused the revision, which is why step 1 cannot produce a digest
   that fails this step.
3. **Digest → freight.** `kargo get freight --project qip-<env>` (from a
   workstation with cluster credentials, or the Kargo UI on the private
   endpoint) — the `Freight` naming this digest.
4. **Freight → promotion commit.** `git log -- infrastructure/gitops/envs/<env>/`:
   the bot's commit names the freight, every digest, the source sha and the
   `deploy.yml` run id.
5. **Digest → source commit, independently.** `gcloud artifacts docker
   images list <repository>/<binary> --include-tags --filter="digest=sha256:..."`.
   The tag is the full commit sha, and the image's
   `org.opencontainers.image.revision` label says the same sha; the pipeline
   pushes no other tag and the registry lets nobody move one. If the tag
   and the promotion commit's source sha disagree, the promotion commit is
   wrong and the tag is right.
6. **Commit → run.** The run id in the promotion commit; the run's `gate`
   job names the ci run it accepted, and ci's `test` job holds the
   `test result:` lines for the tree that was built.
7. **Commit → tree.** `git show <sha>` — the same sha as the tag, the same
   sha as the label, the same sha `DEPLOY_SHA` checked out.

A manifest that names a digest the runtime does not serve is not a lie, it
is the window between a promotion commit and its reconcile — and in that
window `gcloud run services describe` and the `RunService` status are the
answer.

## The frontends: the one documented exception

`scripts/deploy-frontends.sh` is the only path for the portal and the
landing, and register decision F8 in
[missing-infrastructure-register.md](../ops/missing-infrastructure-register.md)
asks whether the script should exist. The decision recorded here and
restated by ADR 0036: **it stays, for now, and the way to retire it is a
`catalogue.tf` entry and a `RunService` manifest for each, not a Kargo
`Warehouse` subscription bolted onto the script.** Folding them is not a
bounded change, for reasons that are each a property of the pipeline rather
than a shortage of effort:

- **The build is not the catalogue's build.** The matrix builds every image
  from the one `infrastructure/docker/Dockerfile` with `--build-arg BINARY`;
  the frontends have their own Dockerfiles, are built by Cloud Build rather
  than the runner, and the landing's build takes the portal's live URL as a
  build argument, so the two are a dependency chain rather than two matrix
  entries. Until they are built by `deploy.yml`, they produce no freight for
  a `Warehouse` to see.
- **The identity is wrong for it.** The script reads `terraform output` for
  the API's internal URL, the attestor and the key version, which needs
  `terraform init` against the state bucket; `qip-ci-<env>` deliberately
  cannot read state, and widening its grant to make a fold work is the thing
  `.claude/rules/domains/infrastructure.md` forbids by name.
- **The acceptance suite reads the script as shell.** `every_cloud_run_service_this_repository_deploys_is_subject_to_the_admission_policy`,
  `no_workload_runs_as_the_projects_default_compute_identity`,
  `no_secret_this_repository_deploys_reaches_a_process_as_an_environment_value`
  and `console_route.rs` parse `gcloud run deploy` invocations out of
  `scripts/deploy-frontends.sh`. They live under `backend/crates`, which a
  change to this path does not own; deleting the script deletes their
  subject and fails the suite.

What the script already holds, so that the exception is bounded rather than
open: both services are deployed by digest read back from the registry,
attested with the pipeline's attestor and key version read from Terraform
output, opted into the admission policy with `--binary-authorization=default`,
run under a named identity each, take every secret as a file, and the routed
revision is read back and compared to the digest before the script reports
success — the same discipline as the Argo CD hook, re-asserted on the
script's text by the tests above. It is dev-only by literal
(`PROJECT="algorik-dev"`), which is a limit, not a feature.

**What closes it:** two `catalogue.tf` entries, two `RunService` manifests
under `infrastructure/gitops/envs/<env>/`, and a build path for a Next.js
image beside the Rust one in `deploy.yml`, so the `Warehouse` sees them and
Kargo promotes them like the other three. That is the cloud-platform
engineer's change and an ADR's, and this document should lose this section
when it lands.

## What is deliberately not a path

- **No Makefile target applies or deploys.** `make infra` is `fmt -check`
  and `validate`; the Makefile says why an `apply` target is the wrong
  affordance.
- **No local Terraform wrapper.** `scripts/tf-agent.sh`, which minted an
  impersonated token and `exec`ed `terraform "$@"` against a project id
  hard-coded to dev, is removed: nothing referenced it, it duplicated the
  impersonation `scripts/bootstrap-deploy.sh` performs with
  `GOOGLE_IMPERSONATE_SERVICE_ACCOUNT`, and it was a one-line `apply`
  affordance for an agent — the class of thing
  `.claude/hooks/guard-dangerous-command.py` exists to stop. A read-only
  `terraform plan` or `output` from a workstation sets that same variable
  and runs `terraform` directly.
- **No second secret seeder.** `scripts/seed-secret-versions.sh` is removed:
  it was written for the Kubernetes CSI driver, nothing referenced it, and
  `scripts/bootstrap-deploy.sh`'s seventh step seeds the same six generated
  secrets on the same never-overwrite rule. `infra.yml` seeds the
  OpenObserve credential on `up` for the same reason in the same shape. The
  GitHub App keys the controllers use are *not* seeded: they are created by
  a person at GitHub, added to Secret Manager by that person, and read into
  the cluster by the bootstrap; nothing generates them.
- **No `kubectl` outside the bootstrap step, and no `helm`, `argocd` or
  `kargo` CLI in any workflow.** The bootstrap applies pinned bytes and
  nothing else; promotion is Kargo's, sync is Argo CD's, and a workflow
  that drove either from the outside would be the second path this
  document exists to refuse.
- **No `gcloud run services update` or `gcloud run deploy` for a catalogue
  service, from any workflow.** The pipeline stopped moving services under
  ADR 0036; a step that moves one again is a second writer to the same
  image and the acceptance suite refuses it.
- **No `${{ vars.* }}`, no service-account key, no `latest` tag, no
  Helm provider, no Pod running a `qip-*` image.** Each is refused by a
  named test in `infrastructure.rs`, and each has a history in this
  repository of why.
