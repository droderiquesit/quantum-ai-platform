# 0036 — Argo CD and Kargo return, on a control-plane cluster that runs no trading binary

**Status:** accepted, by explicit and repeated owner instruction given on
2026-09-04. Nothing in it is applied; see "Nothing is applied by this record".
**Supersedes:** ADR 0017 (its decision is re-taken here on a different
runtime; its text stays as the record of the first attempt and of why the
chart is not coming back). ADR 0024 decision 2 — "the GitOps runtime is
retired" — for the controllers only. ADR 0024 decisions 1, 3's *proof*, 4
and 5 stand; decision 3's *mechanism* (`gcloud run services update` from the
pipeline, and `images.tfvars`) is replaced.
**Does not touch:** the trading runtime of ADR 0022, 0024 and 0035 — Cloud
Run for every warm binary, one bare Compute Engine machine per region for the
execution node — or the paper-trading boundary's three layers (ADR 0021).
**Keeps:** the provider policy (`hashicorp/google` and `hashicorp/google-beta`
only), the two-crate Rust dependency policy (ADR 0002, 0009), Workload
Identity Federation only, secrets as files, Binary Authorization with one rule
and no exemptions, and the prod refusals in `deploy.yml` and `infra.yml`.

## Context

ADR 0017 made delivery GitOps — a Helm chart, Argo CD and Kargo — on the GKE
runtime of ADR 0011. ADR 0024 retired all of it with the cluster: the
blueprint's runtime is Cloud Run and Compute Engine, neither is a Kubernetes
workload, and the pipeline moving each service with `gcloud run services
update` and proving the serving revision closed the one gap the GitOps
cut-over had left, a crash-looping build that produced a green run.
`docs/operations/deployment-path.md` described that path this morning as the
only one, and `no_kubernetes_manifest_helm_chart_or_gitops_controller_remains`
refuses a manifest, a chart, a controller directory, a workflow running
`kubectl`, and a GKE resource in the Terraform.

The owner has decided, explicitly and more than once today, that Argo CD and
Kargo come back. The authoritative path becomes:

```
GitHub Actions (ci.yml, the gate)
  → deploy.yml (build, test, security scan, sign, attest)
  → Artifact Registry, by digest
  → Kargo promotion (a commit naming the digest)
  → Argo CD reconciliation (Config Connector applies it to Google Cloud)
  → the runtime: Cloud Run services; the execution nodes stay with Terraform
```

What the owner did *not* decide, and this record does not do, is move any
trading binary onto Kubernetes. The controllers get a cluster because they
are Kubernetes controllers and need one; the platform's own binaries do not
run as Pods, and the acceptance suite keeps refusing a `Deployment`,
`StatefulSet` or `Job` whose image is a `qip-*` binary.

This record exists because the reversal is consequential, because three
agents are implementing it concurrently and need one written shape to build
against, and because ADR 0024 is otherwise the last word and would be read as
still in force.

## Decision

1. **A GKE Autopilot control-plane cluster per environment, in the
   `management` trust zone.** Private nodes, private endpoint, no public
   endpoint, Workload Identity on. Three things run on it: Config Connector
   (installed as the GKE addon, so the Terraform provider set stays
   `google`/`google-beta` and gains no `kubernetes`, `helm` or `kubectl`
   provider), Argo CD, and Kargo. The cluster is Terraform's — `modules/control-plane`
   or an equivalent under `infrastructure/terraform/modules/` — beside the
   foundation it already owns. **No trading binary ever runs as a Pod.** The
   acceptance suite's refusal of a Kubernetes workload whose image is a
   `qip-*` binary is kept and pointed at the new manifest tree; the cluster
   is a place controllers live, and nothing else.

2. **Argo CD and Kargo are installed from vendored, digest-pinned upstream
   manifests under `infrastructure/gitops/bootstrap/`.** Their images are
   listed in `infrastructure/egress/vendored-images.txt`, mirrored into the
   environment's registry, scanned and attested by `vendor.yml` exactly as
   the Envoy sidecar and OpenObserve are, so Binary Authorization keeps its
   one rule and no `exempt_image_patterns` entry appears. The bootstrap is
   applied by a step in `infra.yml` — `kubectl apply` against the private
   endpoint, with credentials from `gcloud container clusters get-credentials`
   under the same Workload Identity Federation the workflow already
   authenticates with. No Helm provider, no Helm at all, no service-account
   key. The chart ADR 0017 introduced does not return: a chart's `required`
   values guarded a templating engine that no longer exists, and a rendered,
   pinned manifest is reviewable as bytes.

3. **The repository credential is a GitHub App installation, read-only for
   Argo CD and write-scoped for Kargo, and it lives in Secret Manager.** Two
   installations of two Apps on this one repository, because the two
   controllers need different rights and one credential holding both would
   give the reconciler the right to write the thing it reconciles:
   - `qip-argocd-<env>`: `contents: read` only. Argo CD reads manifests and
     writes nothing.
   - `qip-kargo-<env>`: `contents: write`, on this repository alone. Kargo's
     promotion is a commit.

   A GitHub App rather than a deploy key, for three reasons that are each a
   property of the credential and not a preference. An installation token is
   minted per use and expires in an hour, so what a controller holds at rest
   is the App's private key and never a bearer token; a deploy key is itself
   the long-lived bearer. An App is scoped to named repositories and
   permissions, where a deploy key is read or read-write on a repository
   with nothing finer. And an App's commits carry its own identity
   (`qip-kargo-<env>[bot]`) in `git log`, which is what makes a promotion
   commit distinguishable from a person's. The App's private key is the one
   long-lived secret this design introduces, and its cost is stated below.
   The key reaches the controller the way every secret on this platform
   does — from Secret Manager, as a file — with the bootstrap step reading
   the version through `gcloud` and writing it as a Kubernetes `Secret` in
   the controller's namespace; the cluster's etcd is encrypted with a key
   from the environment's KMS ring. No Kubernetes secret is ever committed.

4. **Cloud Run services move from `modules/cloudrun` to Config Connector
   `RunService` manifests under `infrastructure/gitops/envs/<env>/`, one per
   workload in `catalogue.tf`'s `cloud_run_catalogue`, and OpenObserve with
   them.** Every invariant the module held is held by the manifest and
   asserted by a test against the catalogue:
   - `INGRESS_TRAFFIC_INTERNAL_ONLY` for every workload whose
     `traffic_class` is `trading`, and for every catalogue workload today;
     OpenObserve's `open-anonymous` posture stays exactly as ADR 0030 and
     0033 record it and is the only exception the test admits.
   - `binaryAuthorization.useDefault: true`, never a named policy and never
     `breakglassJustification`.
   - Secrets as volumes at `/var/run/secrets/qip/<key>/<file>`, mode 0400,
     with the `_FILE` variable carrying the path; `secret_env` only for a
     vendored image, as ADR 0031 permits.
   - The image by digest — `<registry>/<binary>@sha256:...` — and never a
     tag. A manifest naming a tag fails the parity test.
   - `template.serviceAccount` is the workload's own `qip-<name>-<env>`
     account, which **Terraform keeps creating** (see 5). Direct VPC egress,
     `ALL_TRAFFIC`, the trust zone's subnet and network tag. The instance
     floor and ceiling from the catalogue entry, with `min_instances > 0`
     only where the entry justifies it in writing. The startup and liveness
     probes on the catalogue's `health_path`. The egress sidecar where the
     entry has `egress_proxy = true`, mounted from the same bootstrap bucket;
     the fast brain has none, and the parity test refuses one.
   - `cnrm.cloud.google.com/deletion-policy: abandon` on every `RunService`,
     which is the module's `deletion_protection = true` in the form Config
     Connector has: deleting the manifest, or Argo CD pruning it, releases
     the object and destroys no service. Removing a service stays a
     deliberate two-step by a person.

   `catalogue.tf` remains the source of truth for the invariants. It stops
   instantiating `modules/cloudrun` for the service itself and keeps the
   catalogue map; the acceptance suite's parity test reads each manifest
   beside its catalogue entry and fails on any of the properties above
   disagreeing.

5. **Terraform keeps the foundation and releases the services from state
   without destroying them.** Network, trust zones, secrets, KMS, Binary
   Authorization, IAM, the registry, the egress proxy's bucket and the
   workload service accounts and their grants stay in Terraform; the cluster
   joins them. The service resource leaves. The cut is inside
   `modules/cloudrun`, and it has to be, because that module creates the
   workload's identity, its secret grants and its configuration buckets in
   the same block as the service: the `google_cloud_run_v2_service` and its
   invoker binding are deleted from the module and released with

   ```hcl
   removed {
     from = module.cloud_run.google_cloud_run_v2_service.workload
     lifecycle { destroy = false }
   }
   ```

   and the same for `google_cloud_run_v2_service_iam_member.invokers` and
   for `module.openobserve`; everything else in the module — the identity,
   `telemetry`, `logging`, `mounted`, `env`, `egress_bootstrap`, the config
   and collector buckets and their objects — stays exactly where it is, so
   nothing is destroyed and nothing is renamed. The module's preconditions on
   the service move with the service into the parity test, since a
   precondition on a resource that is not declared checks nothing. Config
   Connector acquires each service by name on first reconcile: a `RunService`
   whose `metadata.name` is `qip-<env>-<name>` adopts the existing Cloud Run
   service and reconciles its spec, so the first sync creates a revision only
   where the manifest and the running service disagree — which is what the
   parity test exists to make zero. The `deployer_service_account` grant
   (`iam.serviceAccountUser` on each workload account) moves from `qip-ci-<env>`
   to the Config Connector identity, which is now the one caller creating
   revisions; `qip-ci-<env>` loses `roles/run.developer`
   (`modules/cicd/main.tf`, resource `deploy`), because the pipeline no
   longer moves a service, and keeps the log-reader and registry grants the
   `images` job still needs.

6. **Kargo: one `Project` per environment chain, one `Warehouse`, four
   `Stage`s.** The `Warehouse` subscribes to the environment's Artifact
   Registry repository for each catalogue binary, selecting the newest build
   and recording the digest; the tag is the commit sha and immutable, so
   "newest" is "the latest commit the pipeline attested". Freight that mixes
   commits — three images from two shas because a matrix job was slow — is
   refused at promotion by a step that reads each image's
   `org.opencontainers.image.revision` label and requires all three equal.
   `deploy.yml`'s build gains `--label org.opencontainers.image.revision=<sha>`;
   the Dockerfile sets no such label today, and an image built before this
   record carries none, for which the step falls back to the tag and says
   so. The stages:
   - **`dev`** promotes automatically: the promotion writes each `RunService`'s
     image digest into `infrastructure/gitops/envs/dev/` and commits to the
     default branch, the commit message naming the freight, every digest, the
     source commit from the label, and the `deploy.yml` run that attested it.
     A bot committing to the default branch is already this repository's
     practice — `deploy.yml` pushes `images.tfvars` there today — and this
     replaces that commit rather than adding a second.
   - **`test`** and **`stage`** promote on a person's approval in Kargo,
     the same commit shape into the environment's directory.
   - **`prod`**'s `Stage` exists, so the chain is whole and a person can see
     what is eligible, and its promotion is refused by policy until an ADR
     lifts it. ADR 0024's prod refusal in `deploy.yml` and `infra.yml`'s
     refusal of a prod dispatch are preserved word for word; a Kargo
     promotion to prod that a person could click is a fourth path around
     both, and the policy is what closes it. The refusal is in the Stage's
     own configuration, in the repository, and the acceptance suite reads it.

7. **Argo CD: one `Application` per environment, automated for `dev`,
   manual for the rest.** `dev` syncs with `prune` and `selfHeal` on, so a
   service edited by hand in the console is put back to what the manifest
   says — the drift that the pipeline could only find at the next deploy is
   now found on the next reconcile, which is what ADR 0017 wanted and could
   not have on the runtime it had. With `deletion-policy: abandon`, prune
   releases and never destroys. `test`, `stage` and `prod` sync when a person
   syncs them. Argo CD's health check for a `RunService` reads the resource's
   own `Ready` condition, and a post-sync hook reads the *serving* revision
   through `status.traffic` — the same two questions `prove-serving.py`
   asked — and fails the sync when the routed revision's workload container
   does not carry the digest the manifest names. That is ADR 0024 decision
   3's proof, moved from the pipeline to the reconciler and not weakened.

8. **`deploy.yml` keeps build, scan, sign, attest and push, and loses the
   rollout.** The `deploy` job's `gcloud run services update`, `prove-serving.py`,
   the `images.tfvars` write and its commit-and-push are removed; Kargo's
   promotion commit is the record and Argo CD's hook is the proof. The
   execution-node rolling replacement stays in the workflow, because the
   node is not a Cloud Run service and is not moving (see 9). Binary
   Authorization is unchanged: a `RunService` naming a digest the attestor
   never signed is refused at admission by Cloud Run, so Kargo promoting an
   unattested image cannot deploy it; it produces a `RunService` whose
   `Ready` condition is false, an Argo CD health of `Degraded`, and a failed
   sync a person reads.

9. **Rollback is a git operation.** Revert the promotion commit, or
   re-promote earlier freight; Argo CD reconciles to the reverted manifest.
   There is no digest input, no re-dispatch at a prior commit, no branch to
   create. The pipeline's reuse path — skipping the build for a commit whose
   image the registry holds — stays, because a rollback of the *pipeline's*
   artefact is still a re-run at a prior commit, but it is no longer how a
   *service* is rolled back.

10. **Every deployed revision traces in both directions.** Revision →
    `RunService` digest (the resource's `status`) → attestation (the
    attestor, by digest) → freight (the Kargo `Freight` naming the digest) →
    promotion commit (`git log` on `infrastructure/gitops/envs/<env>/`, the
    bot's commit naming the freight and the source sha) → source commit (the
    tag, the label, and the commit the message names) → `deploy.yml` run →
    `ci.yml` run and its `test result:` lines. `docs/operations/deployment-path.md`
    walks it.

11. **The trading runtime does not change.** Cloud Run for every warm binary;
    one bare Compute Engine machine per region for `qip-edge-node` under
    systemd, in shadow mode, one of them in `dev` under ADR 0035; the
    execution nodes' instance groups, templates and rolling replacement stay
    with Terraform and `deploy.yml`, outside GitOps, and
    `docs/operations/gitops-exceptions.md` says so. No `Pod` runs a `qip-*`
    image. The paper-trading boundary's three layers — the plan-time refusal
    of the three live rungs in `variables.tf`, `AutonomyLevel::deployable` in
    the three composition roots, and `Cell::new` taking no ceiling but paper
    trading — are untouched, and `QIP_AUTONOMY_CEILING` in every `RunService`
    manifest is the literal `paper_trading`, which the parity test asserts
    against the catalogue's `var.autonomy_ceiling` and the environment's
    tfvars in the same way `nothing_added_here_raises_the_autonomy_ceiling_anywhere`
    does today. A manifest is a value somebody edits; the test is what makes
    a live rung in one a red build rather than a deployment.

## What was rejected, and why

- **Argo CD and Kargo on a cluster that also runs the platform's binaries**
  — ADR 0011's shape, back again. Rejected because the owner did not decide
  it, ADR 0022 and 0024 decided against it on the blueprint's authority, and
  the whole of ADR 0024 decision 1 (secrets as volumes on Cloud Run, the bare
  execution node, the trust zones' network tags) would have to be re-argued.
- **Terraform keeps the services and Argo CD drives Terraform** (a
  Terraform controller, or Kargo committing `images.tfvars` for `infra.yml`
  to apply). Rejected because it puts an apply on the deploy path, and this
  repository's rule that an apply is a plan a person reads would then be
  violated on every promotion — or the rule would be kept and every
  promotion would wait for a dispatch, which is the pipeline with more
  moving parts.
- **A Helm chart for the controllers.** Rejected: pinned manifests are
  reviewable bytes, `vendor.yml` already attests images and not charts, and
  a Helm provider is a third provider.
- **A deploy key for the repository connection.** Rejected for the reasons
  in decision 3; recorded here so the next reader does not reopen it.
- **Kargo promoting the execution nodes.** Rejected: a node is an instance
  template and a group, not an image a `Warehouse` can subscribe to, and its
  boot image is not something this pipeline bakes (`modules/execution-node/README.md`).
  It stays outside GitOps and is named as such.
- **The frontends in this change.** `scripts/deploy-frontends.sh` stays the
  one path for the portal and the landing until an entry for each exists in
  the catalogue with a build path beside the Rust one; folding them is the
  change `deployment-path.md` describes, and it is not this one.

## What it costs

- **A cluster per environment bills while idle.** Autopilot charges for the
  Pods it schedules and the control plane; Argo CD, Kargo and Config
  Connector are always on. Until now the only thing in a deployment that
  cost money standing was an execution node, and `infra.yml`'s `down` was
  written around that. Four clusters is four control planes' worth of
  standing cost, and `down` does not touch them.
- **A Kubernetes control surface the platform did not have.** A private
  endpoint is still an API server; an identity that can `kubectl apply`
  into the control-plane cluster can change what Config Connector applies
  to every Cloud Run service in the environment. ADR 0024's "no Kubernetes
  here" is reversed for the control plane only, and the threat model gains a
  section it lost.
- **A long-lived private key in Secret Manager.** The GitHub App's key is
  the first credential of its class this platform holds for a third party's
  API. It is not a service-account key and the rule against those is not
  weakened, but it is a static secret that must be rotated on a schedule
  someone owns; that schedule is named in the runbook, not assumed.
- **Egress from the management zone to GitHub and to the registry.** Argo
  CD and Kargo need `api.github.com` and `github.com`; the registry is a
  Google API and stays on Private Google Access. The zone's egress allowlist
  grows by exactly those names, in the one committed bootstrap, and a wider
  allowance is the reversal condition ADR 0017 named and this record keeps.
- **A second source of what is running, for the window between a promotion
  and its reconcile.** The promotion commit says what should serve; the
  `RunService` status says what does. The Argo CD hook closes the window
  with a proof, but the window exists and `gcloud run services describe` is
  still the answer inside it.
- **Two more vendored images to keep current.** Each is a digest someone
  bumps in a commit, scanned by `vendor.yml` at `CRITICAL` blocking and
  `HIGH` reported — and Argo CD's bundled tooling is the image whose
  findings the acknowledgement file that `vendor.yml`'s comment describes
  was written for, so the first scan may block.
- **The acceptance suite's guard is re-scoped, not deleted.**
  `no_kubernetes_manifest_helm_chart_or_gitops_controller_remains` refused a
  `kind:` line anywhere under `infrastructure/` and a workflow running
  `kubectl`; both are now true of the design. The test keeps every property
  it can — no chart, no `infrastructure/kubernetes`, no `helm`, no
  `argocd`/`kargo` CLI in a workflow, no GKE resource outside the control-plane
  module, no Pod running a `qip-*` image, no `kind: Deployment` or
  `StatefulSet` under `infrastructure/gitops/envs/` — and drops exactly the
  two assertions this record contradicts, naming this record in the comment.
- **Everything ADR 0024 called out as unfinished is still unfinished.**
  Nothing scrapes a Cloud Run service; `execution_nodes` is non-empty in
  `dev` alone; the centre-to-node path is unwired. A new delivery path
  delivers the same three services to the same runtime.

## What must never change

- No `Pod` runs a `qip-*` image. The cluster is for controllers.
- The paper-trading boundary's three layers, and `paper_trading` as the only
  value `QIP_AUTONOMY_CEILING` takes in any manifest a plan or a sync can
  carry.
- Binary Authorization's one rule with no exemptions; every controller
  image vendored and attested.
- No service-account key, no Helm provider, no third Terraform provider, no
  `${{ vars.* }}`, no `latest` tag.
- Prod is promoted by nobody until an ADR says otherwise, and `deploy.yml`
  and `infra.yml` keep their prod refusals regardless of what Kargo permits.
- A `RunService` is never deleted by a prune. `deletion-policy: abandon` is
  the module's `deletion_protection`, and removing it from a manifest fails
  the parity test.
- Every secret a workload reads is a file, except where ADR 0031 says
  otherwise for a vendored image.

## Nothing is applied by this record

No cluster exists, no controller runs, no `RunService` has been applied and
no service has been released from state. The implementing agents produce
the Terraform, the manifests, the workflow changes and the tests; `infra.yml`'s
`plan` is dispatched, and a person reads it before `up`. The first plan
proposes a cluster per environment and releases three services from state;
the first `up` runs the bootstrap; the first Argo CD sync of `dev` is the
first evidence that Config Connector acquired a service rather than
replacing it — and that evidence is a revision count that did not move,
recorded in `docs/ops/missing-infrastructure-register.md` when it exists.
Until then every sentence above about what Argo CD or Kargo does is a
sentence about a configuration.

## What would make this wrong

- **Config Connector creating a new revision on acquisition** for a service
  the parity test said matched. That is either a property the test does not
  cover or a field Config Connector defaults differently from the provider;
  the first sync of `dev` is where it shows, and the remedy is the test, not
  a manual `gcloud run services update` to make the status agree.
- **A `qip-*` image appearing in a Pod spec** for any reason — a "temporary"
  job, a debug container, a migration. The cluster is then the runtime ADR
  0024 retired, and this record was the door.
- **Kargo's prod policy being lifted without an ADR**, or `deploy.yml`'s and
  `infra.yml`'s refusals being read as redundant now that Kargo has one.
  Three refusals are three; the day one is removed because another exists is
  the day the remaining one is next.
- **The GitHub App's permissions widening** past `contents: read` for Argo
  CD and `contents: write` on this repository for Kargo, or the management
  zone's egress allowlist growing past GitHub. ADR 0017's own reversal
  condition, unchanged.
- **The standing cost being paid for an environment nobody promotes to.**
  If `test` and `stage` never receive a promotion, they are clusters
  reconciling nothing; a follow-up record should decide whether one
  control-plane cluster serves several environments or whether those
  environments exist at all.
- **A live-order or live-transfer path appearing** on the reasoning that
  delivery is now automated. ADR 0021 and 0023 govern that, and a delivery
  mechanism is not an occasion to revisit them.

## Amendment, same day: cert-manager is the third vendored controller

The implementation vendors cert-manager beside Argo CD and Kargo, under
`infrastructure/gitops/bootstrap/cert-manager/`, and the acceptance suite
admits it there. Kargo's admission webhooks serve TLS from a cert-manager
`Certificate` issued by a self-signed `Issuer`, with the CA injected into
four webhook configurations; the only alternative the chart offers is an
operator-supplied certificate spliced into manifests by substitution, which
is the shape ADR 0017 recorded and ADR 0024 retired. Three digest-pinned
images, mirrored and attested like every other foreign image and used for
nothing else, is the smaller cost, and ADR 0024's own note that cert-manager
existed only for these webhooks is exactly why it returns with them. Where
this record says the cluster runs Config Connector, Argo CD and Kargo, read
cert-manager beside them; `bootstrap/README.md` carries the order and the
reasoning.
