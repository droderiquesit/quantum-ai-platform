# Argo CD on the platform's clusters

The reconciler ADR 0017 decided on. The cluster stops being pushed to and
starts pulling: desired state is this repository at a revision, and the only
writer to the cluster is the operator that reads git.

## What is installed, and what deliberately is not

**Argo CD core** (`core-install`), not the full install. The difference is
the API server, the UI, Dex, and the notifications controller — every one of
them a listening surface in a cluster whose posture is default-deny, and none
of them needed for reconciliation. Operators who want the UI run
`argocd admin dashboard` through their own kubectl credentials, which is a
port-forward under an identity IAM already vetted, not a service waiting on
the network. If the desk later decides the UI earns its surface, that is an
upgrade of this directory in a reviewed commit, not a default.

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

Applying dev, from the repository root, with cluster credentials already
fetched:

```sh
kubectl apply -k infrastructure/gitops/argocd/overlays/dev
```

Idempotent, and deliberately a kubectl command rather than a pipeline step
for now: the reconciler is the one workload that cannot be delivered by the
reconciler, and while the kubectl deploy path still exists (the ADR's
migration window) a second unattended writer would mean two systems applying
this namespace. When deploy.yml's kubectl path retires, installing and
upgrading Argo CD itself becomes the runbook's only remaining kubectl.

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

## What is deferred, and why

- **The dev Application** (this repository → the qip Helm chart) needs a
  read credential for a private repository. The options and their trade-offs
  are a decision to record when taken — a deploy key in Secret Manager
  projected into Argo CD's repository Secret is the current intent — and
  until then Argo CD reconciles nothing: installed, constrained, pointed at
  nothing.
- **Kargo** (promotion) follows once the Application exists; it needs
  cert-manager, which will arrive through the same vendored-image path.
