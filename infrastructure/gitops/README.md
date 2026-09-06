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
