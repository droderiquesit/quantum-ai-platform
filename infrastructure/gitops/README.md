# infrastructure/gitops

The delivery path of ADR 0036. Nothing here has been applied; every sentence
below about what a controller does is a sentence about a configuration
until `infra.yml`'s `up` and its bootstrap have run for an environment and a
person has read what the first sync did.

```
deploy.yml  →  Artifact Registry, by digest, attested
                  ↓ Warehouse discovers the newest attested build (kargo/)
               Kargo promotion: a commit to envs/<env>/kustomization.yaml
                  ↓ Argo CD syncs envs/<env> (argocd/)
               Config Connector reconciles each RunService into Cloud Run
                  ↓ post-sync hook proves the routed revision serves the digest
```

| Path | What |
|---|---|
| `bootstrap/` | The three controllers and the Config Connector operator as vendored, digest-pinned manifests, and Config Connector's one object; applied by `infra.yml` in order — `bootstrap/README.md` |
| `envs/<env>/` | One `RunService` per catalogue workload (and OpenObserve where it is deployed), the invoker bindings, the proving hook, and the `kustomization.yaml` whose `images` block is the record of what the environment serves — `envs/README.md` |
| `argocd/` | The `qip` project and one `Application` per environment: automated for `dev`, manual for the rest |
| `kargo/` | The project, its promotion policy, the promotion task, the warehouse and the four stages — `kargo/README.md` |

## How an operator reaches these things

**No name in here resolves from a domain anybody bought, and none needs to.**
ADR 0095. There are two doors and they are different shapes, because the
things behind them are different shapes.

### The console — a Google-issued URL, behind IAP

```
terraform output console_front_door
# url   = https://qip-dev-portal-<project-number>.us-east4.run.app
# mode  = cloud-run-iap
# grant = gcloud iap web add-iam-policy-binding …
```

Cloud Run enforces Identity-Aware Proxy on the service itself, across every
ingress path including that URL, so there is no load balancer, no reserved
address, no certificate anybody orders and no DNS record anybody creates. The
hostname exists as soon as Cloud Run has a service and Google manages the
certificate.

The access list is empty in every environment — an IAM member is an account
identifier and this repository carries none — so the door comes up admitting
nobody. Run the `grant` command the output prints to admit yourself.
`--resource-type=cloud-run` is the part that is easy to lose: the same
subcommand without it edits the *project's* IAP policy.

`modules/iap-run/README.md` has the detail, including the one field
(`iapEnabled`) that Config Connector's `RunService` cannot carry yet and how
`infra.yml`'s `apps` stage sets and verifies it.

### Argo CD and Kargo — the Connect gateway, and nothing published

These two run **in the cluster**, and that is the whole difference. A Cloud
Run service gets a `run.app` hostname for nothing; a GKE Gateway gets an IP
address and nothing else — **Google publishes no DNS name for one**, and a
Google-managed certificate is issued for a domain or not at all. There is no
Google-provided hostname to reach for here, and the two things people reach
for instead are both worse than the gap: a third-party wildcard resolver puts
a control plane's hostname in somebody else's DNS, and a self-signed
certificate trains an operator to click through a warning in front of the one
console that can reconcile arbitrary manifests into this cluster.

So they are reached the way this private cluster was always designed to be
reached, and the way `infra.yml` itself reaches it:

```
gcloud container fleet memberships get-credentials qip-dev-gitops \
  --project algorik-dev

kubectl -n argocd port-forward svc/argocd-server 8080:443   # https://localhost:8080
kubectl -n kargo  port-forward svc/kargo-api     8081:443   # https://localhost:8081

argocd --core                       # against the kubeconfig the gateway wrote
kargo login --kubeconfig
```

This publishes **nothing**: no address, no certificate, no listener, nothing
on the internet for anybody to find or scan. There is no password to hold
either — Argo CD's admin account is off, Dex is deleted, Kargo's admin account
and OIDC are off, and an operator acts as their own GKE identity. What a
person can do is what their IAM role lets the Kubernetes API do, which is the
same audit trail everything else here has.

`modules/gitops-gateway` and `bootstrap/gateway/base/` are kept for the day
somebody owns a domain and decides these should be published; there is no
`overlays/<env>/` for any environment, so the bootstrap step applies nothing
and says so. The root refuses the half-configuration — `gitops_gateway_enabled`
true with either hostname empty — rather than reserving an address and
ordering a certificate for a name that cannot resolve.

## What the acceptance suite holds these files to

`infrastructure/terraform/catalogue.tf` is the source of truth for every
invariant a `RunService` carries; the parity test reads each manifest beside
its entry. `modules/cloudrun` still creates the workload's identity, its
secret grants and the buckets its files are published to, and exports the
environment, secret paths and configuration paths a manifest must match.
A manifest that disagrees — a tag instead of a digest, a trading workload
on any ingress but internal, a `qip-*` image in a Pod, a missing
`deletion-policy: abandon`, a `QIP_AUTONOMY_CEILING` that is not
`paper_trading` — fails the build, not the sync.

## Two things a reader should know before believing this tree

**`gcs` volumes.** The API and the deep brain mount the egress proxy's
bootstrap, and all three central workloads mount `universe.json`, as Cloud
Storage volumes — `modules/cloudrun` created them that way and the running
`dev` services carry them. The Config Connector `RunService` reference
(`run.cnrm.cloud.google.com/v1beta1`, read on 2026-09-04) lists
`secret`, `emptyDir` and `cloudSqlInstance` volumes and no `gcs`. The
manifests here carry the `gcs` volumes anyway, exactly as the services run,
and every Application syncs with `Validate=true`: if the operator's schema
does not admit the field, the sync is refused with a validation error and
nothing is stripped from the running service. If it does admit it, the
first sync acquires the service unchanged. What must not happen is the
third thing — a schema that silently prunes the volume and a reconcile that
removes the mount — and the validation flag is what makes it the first
thing instead. Until a real sync has answered which, ADR 0036 decision 4 is
not proven for these four workloads. `envs/README.md` has the detail.

**One registry per environment.** `deploy.yml` builds into the environment
it targets and each environment has its own attestor, so an image in
`dev`'s registry is not admissible in `test`'s. The Kargo chain here is the
ADR's — one warehouse, four stages — and a promotion past `dev` writes a
digest the target project's registry does not hold. `kargo/README.md` says
what that costs and what decision closes it.
