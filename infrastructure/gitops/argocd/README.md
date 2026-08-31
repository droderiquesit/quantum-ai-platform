# Argo CD on the platform's clusters

The reconciler ADR 0017 decided on. The cluster stops being pushed to and
starts pulling: desired state is this repository at a revision, and the only
writer to the cluster is the operator that reads git.

## Cutover status

**The kubectl path is retired.** As of this commit, `deploy.yml` no longer
applies manifests with `kubectl apply`. Instead it builds, scans, pushes,
signs and attests images, then writes the promoted digests to
`infrastructure/helm/qip/values-<env>-images.yaml` and commits them to git.
Argo CD's automated sync (prune + self-heal) reconciles the cluster to match.

The old sed-rendered manifests in `infrastructure/kubernetes/base/` are
superseded by the Helm chart in `infrastructure/helm/qip/`. They remain in
the repository for reference but are no longer applied by any pipeline step.
See `infrastructure/kubernetes/base/README.md`.

## What is installed, and what deliberately is not

**The full install, less dex.** The first commit of this directory installed
core only — no API server, no UI — on the reasoning that every listening
surface in a default-deny cluster must earn its place. The desk then asked
for the web UI explicitly, which is exactly the reviewed decision that
paragraph demanded, so the API server, UI and notifications controller are
in. Dex stayed out on its own demerits: it federates SSO nothing configures,
and its current image scanned with five CRITICALs (vendored-images.txt
refuses it by name), so the kustomization deletes its Deployment and Service
and the UI authenticates with the built-in admin account.

### Reaching the UI

No Ingress or LoadBalancer exists — nothing routes into this cluster from
outside, and that stays true. The UI is reached over the operator's own
kubectl credentials:

```sh
kubectl port-forward svc/argocd-server -n argocd 8443:443
# then open https://localhost:8443 — user `admin`, password from:
kubectl get secret argocd-initial-admin-secret -n argocd \
  -o jsonpath='{.data.password}' | base64 -d
```

An internet-facing route (IAP behind a managed certificate) is a decision
for when the algorik.ai domain lands, not something to improvise around a
self-signed certificate and a password.

## Where the images come from

Binary Authorization's default rule denies what the platform's attestor has
not signed, and Argo CD's upstream images are exactly that. They are adopted,
not exempted: `infrastructure/gitops/vendored-images.txt` names each source
digest, and `.github/workflows/vendor.yml` mirrors it into the environment's
registry, proves the digest survived, scans it with the platform's own Trivy
gate, and attests it. The overlays substitute those vendored references, so
the policy keeps exactly one rule and this namespace gets no exemption
pattern.

## Layout

| Path | What |
|---|---|
| `base/upstream/core-install-v3.5.2.yaml` | Upstream manifest, verbatim — see "Upgrading" |
| `base/namespace.yaml` | Namespace with restricted pod security, default-deny, DNS |
| `base/egress-policies.yaml` | The egress each component is permitted — the ADR's boundary |
| `overlays/<env>/kustomization.yaml` | Vendored image digests for that environment's registry |
| `apps/project.yaml` | The `qip` AppProject — source, destination, kind allowlists |
| `apps/dev.yaml` | Dev Application — automated sync, prune, self-heal |
| `apps/test.yaml` | Test Application — automated sync, prune, self-heal |
| `apps/stage.yaml` | Stage Application — automated sync, prune, self-heal |
| `apps/prod.yaml` | Prod Application — automated sync, prune, self-heal |
| `apps/edge.yaml` | Edge cell template — manual sync, applied by runbook |

Applying dev, from the repository root, with cluster credentials already
fetched:

```sh
kubectl apply -k infrastructure/gitops/argocd/overlays/dev
```

Idempotent, and deliberately a kubectl command rather than a pipeline step
for now: the reconciler is the one workload that cannot be delivered by the
reconciler, and installing and upgrading Argo CD itself is the runbook's only
remaining kubectl.

## The Applications

Each environment has an Argo CD Application in `apps/` that reads the Helm
chart at `infrastructure/helm/qip/` with environment-specific values files.
All central-plane Applications (dev, test, stage, prod) use automated sync
with `prune: true` and `selfHeal: true`:

- **prune**: resources removed from the chart are deleted from the cluster.
- **selfHeal**: manual `kubectl edit` changes are reverted on the next sync.

The prod Application is gated by the `prod` GitHub environment's required
reviewers — a person must approve the workflow run before the digests commit
lands. Argo CD itself is unattended once the commit is on the branch.

Edge cells use manual sync. ADR 0008: bringing a cell up is a deliberate act
with a runbook, not something a pipeline does unattended.

## Upgrading

The upstream file is verbatim so an upgrade is reviewable as a diff against
upstream, not an archaeology of local edits:

1. Fetch `manifests/core-install.yaml` at the new tag from the argo-cd
   repository; commit it as `base/upstream/core-install-<tag>.yaml` and
   point `base/kustomization.yaml` at it.
2. Resolve the new image digests (argocd, and whatever redis the new
   manifests pin) and add them to `vendored-images.txt` — the vendor
   workflow mirrors and attests on push.
3. Update every overlay's digests to match.
4. `kubectl apply -k` the overlay, and prune what the new version no longer
   ships.

## The AppProject

`apps/project.yaml` confines the reconciler to this repository, the qip
namespace, and Namespace/StorageClass/ComputeClass as the only cluster-scoped
kinds. Edge cluster destinations are wildcarded so each cell's Application
can target its own cluster without a project change per cell.

## What is deferred, and why

- **Kargo** (promotion) follows now that the Applications exist; it needs
  cert-manager, which will arrive through the same vendored-image path.
- **Cloud Run frontends** are out of scope — they deploy through
  `gcloud run deploy` and are not Kubernetes workloads. See
  `docs/operations/gitops-exceptions.md`.
