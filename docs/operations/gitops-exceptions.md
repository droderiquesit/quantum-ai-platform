# GitOps exceptions — what is deployed outside Argo CD and Kargo

Argo CD and Kargo exist by
[ADR 0036](../adr/0036-argo-cd-and-kargo-return-on-a-control-plane-cluster.md)
and are the only path by which a Cloud Run service changes; the path, stage
by stage with what proves each, is [deployment-path.md](deployment-path.md).
This file is the list of what that path does *not* carry, each with the
reason and what would fold it in. A GitOps deployment with an unstated
exception is a deployment whose drift detection has a hole nobody is
watching, so the list is meant to be short, honest and shrinking.

This document has been three things. It was the record of the first GitOps
cut-over's exceptions under ADR 0017; then, under ADR 0024, a banner saying
the controllers no longer existed; now, under ADR 0036, the list again. The
one thing it held through all three is the gap the first cut-over left — a
build that crash-looped on boot produced a green pipeline because nobody
verified a promotion. That gap stays closed: the proof moved from the
pipeline's `gcloud run services update` to Argo CD's post-sync hook, which
reads the *serving* revision through `status.traffic` and fails the sync
unless it carries the manifest's digest. `a_promotion_names_who_verifies_it`
reads the hook.

**Nothing under ADR 0036 is applied.** Every row below describes the design
as committed, and the first plan a person reads is where it meets a project.

## Outside GitOps, and why

| What | Deployed by | Why it is outside | What would fold it in |
|---|---|---|---|
| **The execution nodes** — `qip-edge-node` on one bare Compute Engine machine per region, in shadow mode (ADR 0024, 0035) | Terraform: `modules/execution-node` from `execution_nodes` in the tfvars, applied by `infra.yml`; replaced under the group's rolling policy by `deploy.yml`'s `replace every execution node under its group's rolling policy` step | A node is an instance template and a managed instance group, not an image a Kargo `Warehouse` can subscribe to. Its boot image is an image contract nothing in this repository bakes (`modules/execution-node/README.md`); `qip-edge-node`'s container image is built and attested by `deploy.yml` as the artefact that contract names, and nothing on Cloud Run runs it. It is also the one thing that bills while idle, and `infra.yml`'s `down` is written around it | A baked boot image with a digest the pipeline attests, and a Config Connector `ComputeInstanceTemplate`/`ComputeInstanceGroupManager` pair a `Warehouse` could subscribe to. That is a decision about the node's image lifecycle before it is a decision about GitOps, and it needs its own record |
| **The frontends** — the portal and the landing | `scripts/deploy-frontends.sh`, dev-only by literal, by digest, attested with the pipeline's attestor, routed revision read back before success | They are built by Cloud Build from their own Dockerfiles, not by `deploy.yml`'s matrix, so they produce no freight; the landing's build takes the portal's live URL as an argument, so they are a chain; and the acceptance suite reads the script as shell. [deployment-path.md](deployment-path.md#the-frontends-the-one-documented-exception) has the full argument and register decision F8 asks the question | Two `catalogue.tf` entries, two `RunService` manifests, and a Next.js build path beside the Rust one in `deploy.yml`. Then the `Warehouse` sees them and this row goes |
| **The foundation** — network, trust zones, secrets, KMS, Binary Authorization, IAM, the registry, the egress proxy's bucket and bootstrap, the workload service accounts and their grants, and the control-plane cluster itself | Terraform, applied by `infra.yml` after a plan a person reads, or by `scripts/bootstrap-deploy.sh` for a first apply and for prod | This is not an exception so much as the boundary: GitOps carries what runs, Terraform carries what it runs *on*. A reconciler that could create a KMS key or widen an IAM grant on a sync would put the platform's security posture on the deploy path, where a promotion commit could change it | Nothing should. A change to the foundation is a plan a person reads, by design |
| **The controllers' own bootstrap** — Config Connector's configuration, Argo CD and Kargo from `infrastructure/gitops/bootstrap/` | `infra.yml`'s `up`, one `kubectl apply` of vendored, digest-pinned manifests under WIF | Argo CD cannot install itself, and a controller managing its own manifest is the loop that makes a bad upgrade unrecoverable from git. The bootstrap is the only `kubectl` in any workflow and applies pinned bytes only | Argo CD managing Argo CD is a pattern upstream supports and this repository declines for the reason given; revisit only if the bootstrap step becomes the thing that breaks |
| **The GitHub App private keys** — `qip-argocd-<env>` (contents: read) and `qip-kargo-<env>` (contents: write, this repository only) | A person, at GitHub, then into Secret Manager; the bootstrap reads the version through `gcloud` and writes the Kubernetes `Secret` | A credential for a third party's API cannot be generated by an apply. ADR 0036 records why an App and not a deploy key, and names the key as the one long-lived secret the design introduces | A rotation schedule with an owner, in this file when the first key exists. Until then: no key exists, no schedule exists, and this sentence is the record of that |
| **OpenObserve's *credential*** — the root email and password | `infra.yml`'s `seed any credential that has no version` step on `up`, never overwriting | A generated value, seeded once, read by nothing in the workflow; the `RunService` for OpenObserve itself moves under GitOps with the rest (ADR 0036 decision 4), and its `secret_env` stays exactly as ADR 0031 permits | Nothing; the value is not configuration and has no place in a manifest |

## What is not on this list, deliberately

- **OpenObserve's service.** It moves with the three catalogue services into
  a `RunService` manifest reading its vendored digest. That retires the
  `terraform apply -replace=...` wrinkle `modules/cloudrun`'s `ignore_changes`
  comment records for a vendored digest bump: Config Connector reasserts the
  manifest's digest, so a bump in `vendored-images.txt` is a manifest change
  and a promotion like any other. Its `open-anonymous` posture and
  `allUsers` invoker stay exactly as ADR 0030 and 0033 record and are the
  one exception the parity test admits to internal ingress.
- **Prod.** Prod is not an exception to GitOps; it is inside it, with a
  `Stage` and an `Application` that exist and a promotion policy that
  refuses. `deploy.yml` and `infra.yml` keep their own prod refusals
  regardless. Three refusals are three.

## What each part of the ADR 0017 stack became, for the record

Kept because ADR 0024 wrote it and readers arrive from its links; each row
says what ADR 0036 did to it.

| ADR 0017 | Under ADR 0024 | Under ADR 0036 |
|---|---|---|
| Argo CD Applications, `infrastructure/gitops/argocd` | `catalogue.tf` — one Cloud Run service per deployable | Back, as one `Application` per environment under `infrastructure/gitops/`, syncing `RunService` manifests; `catalogue.tf` stays the source of truth for the invariants |
| Kargo promotion, `infrastructure/gitops/kargo` | The `deploy` job in `deploy.yml`; digests in `images.tfvars` | Back, as a `Project`, a `Warehouse` and four `Stage`s; the promotion commit replaces `images.tfvars`, and `deploy.yml` loses the rollout |
| KEDA scaling, `infrastructure/gitops/keda` | Cloud Run's own scale-to-zero, bounded by `min_instances`/`max_instances` | Unchanged; the bounds are in the `RunService` now and the parity test checks them against the catalogue |
| cert-manager | Nothing: it existed for Kargo's webhook certificates | Kargo's webhook certificates are part of its vendored bootstrap; whether that means cert-manager returns is the implementing agent's finding, and if it does it is one more vendored, attested image on the same list |
| The Helm chart, `infrastructure/helm/qip` | `catalogue.tf` and `modules/cloudrun` | Not back. Pinned manifests, not a chart; ADR 0036 says why |
| The manifests, `infrastructure/kubernetes/base` | The same, plus `modules/execution-node` for the cell that was a StatefulSet | Not back. No `qip-*` binary is a Pod, and the acceptance suite refuses one |
| The Cloud Run "exception" for the frontends | `scripts/deploy-frontends.sh` | Still the exception; the row above |
| Edge cells synced by hand | `execution_nodes` in the tfvars | Still Terraform's; the first row above |
