# kargo/

The chain ADR 0036 decision 6 describes: one project, one warehouse, four
stages, `dev` automatic, `test` and `stage` on a person's approval, `prod`
refused by its own first step. `base/` is the shape; `overlays/<env>/` is
the environment's registry in the warehouse's subscriptions and in each
stage's variables.

## What refuses what

| Refusal | Where | Mechanism |
|---|---|---|
| Freight mixing commits | `Warehouse.spec.freightCreationCriteria` | the three images' tags — the commit sha `deploy.yml` pushes — must be equal, so no such freight is created |
| A tag that is not a commit sha | `Warehouse` subscriptions' `allowTagsRegexes` | `^[0-9a-f]{40}$` |
| A promotion to `prod` | `Stage/prod`'s promotion template | its only step is `fail`, naming ADR 0024 and 0036; there is no policy for it either |
| An unattested digest | Cloud Run's admission, not Kargo | the commit lands, Argo CD reports `Degraded`, a person reads it (decision 8) |

`deploy.yml` and `infra.yml` refuse prod on their own; the `fail` step does
not know about them, and that is the point of three.

## What a promotion writes

`base/promotiontask.yaml`: clone this repository at its default branch, set
the three images in `envs/<stage>/kustomization.yaml` by digest, commit with
the freight, every digest and the source commit in the message, push, and
notify the stage's Argo CD `Application`. The committer is the write-scoped
GitHub App of ADR 0036 decision 3, whose credential the bootstrap projects
into the `qip` namespace as `qip-repository`.

## What this cannot do today, stated plainly

`deploy.yml` builds into the registry of the environment it targets, and
each environment has its own attestor. The warehouse on `dev`'s control
plane subscribes to `dev`'s registry; a promotion to `test` therefore writes
`dev`'s digests into `envs/test/kustomization.yaml` under `test`'s registry
path, and that registry holds no such digest and `test`'s attestor never
signed it. The promotion commits, the sync — when `test` has a control
plane, which it does not — reports `Degraded`, and nothing deploys. The
same holds for `stage` and `prod`.

That is not a bug in these files; it is the ADR's chain meeting the
repository's one-registry-per-environment fact, and it needs a decision the
ADR itself anticipates in "what would make this wrong": either one
registry and one attestor serving every environment so that a digest is a
digest everywhere, or one warehouse per environment subscribed to that
environment's own builds, which makes the chain nominal. Until that record
exists, `dev` is the only stage a promotion can reach the runtime through,
and `argocdApplication` is set for `dev` alone in `overlays/dev/` because
`dev`'s Argo CD is the only one this Kargo can reach.
