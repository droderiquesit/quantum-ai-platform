# kargo/

The chain ADR 0036 decision 6 describes, with the two gates R12 CICD-9112
adds: one project, one warehouse, six links —
`dev → test → replay → paper → stage → prod`. `dev` promotes automatically;
`test`, `replay`, `paper` and `stage` on a person's approval; `prod` is
refused by its own first step. `base/` is the whole chain; `overlays/<env>/`
only names the Argo CD application that environment's Kargo can reach.

The blueprint's environment names map onto the chain as follows: its
`integration` environment is the chain's `test` (the first environment after
dev, with its own project and control plane); `replay` and `paper` are the
two gate stages below, which have no environment; `stage` and `prod` are
themselves.

## One registry, one build, the same digest everywhere

`deploy.yml` builds each commit **once**, into the build registry —
`qip-dev` in the dev project (`BUILD_ENVIRONMENT: dev`) — and every later
environment is promoted that digest. A dispatch for `test`, `stage` or
`prod` never builds: it finds the digest in the build registry, scans it
again, and attests it with *that* environment's own attestor, so each
environment's Binary Authorization policy still admits only what its own
attestor signed. If the build environment's run has not pushed the commit,
the dispatch stops ("a promotion attests the image built once and never
builds one").

Every stage's `registry` and `warehouseRegistry` are the build registry,
so the digest a promotion writes into `envs/<stage>/kustomization.yaml` is
the digest the warehouse discovered, in a registry that holds it. The
three platform images in `envs/{test,stage,prod}/kustomization.yaml` name
the build registry for the same reason; the vendored images are mirrored
per environment and stay in each environment's own registry.

Until 2026-10-11 each environment built into its own `qip-<env>`, so a
promotion to `test` wrote dev's digest under test's registry path — a
registry that held no such digest — and a rebuild there would have been
different bytes from the ones dev tested. That is the failure removed.

What the single registry needs from Terraform, and does not have yet
because no environment after dev is provisioned: dev's
`registry_promotion_readers` must name each later environment's
`qip-ci-<env>` account (to describe, scan and attest), `qip-infra-<env>`
(to bake an execution node's image from `qip-edge-node`), and its Cloud Run
service agent `service-<number>@serverless-robot-prod.iam.gserviceaccount.com`
(to pull). `modules/registry` grants each `artifactregistry.reader` and
nothing that writes.

The decision to serve every environment from dev's project is recorded
here and in the workflow; it has no ADR of its own yet, and ADR 0036's
"what would make this wrong" names it as the one that needs one.

## Reproducible builds

The build's `SOURCE_DATE_EPOCH` is the commit's timestamp
(`git log -1 --format=%ct`), never the clock; BuildKit's
`rewrite-timestamp=true` clamps every layer's file times to it; the
Dockerfile refuses a build without it. For the four workspace binaries,
`deploy.yml` builds the commit a second time with no cache and refuses to
push unless both builds are the same image id. `qip-portal` cannot pass
that check: its `generateBuildId` follows the epoch, but every `next build`
writes four fresh per-build secrets (the preview-mode keys and the Server
Actions key). So `deploy.yml` rebuilds it with no cache, exports both
images' filesystems and compares them file by file, excluding only the four
paths `.github/reproducibility/portal-exceptions.txt` names, and refuses any
other difference or a listed file gone (R12-CICD-10117). That runs on the
runner path only; the private pool does not compare the portal. The
builds still run on GitHub-hosted
runners; a Cloud Build private pool (the other half of CICD-9097) is not
built here.

## The gates

`replay` and `paper` are stages with no environment. Each runs
`base/gatetask.yaml`, which commits the freight's three digests, its source
commit and the promotion's name into `gates/<stage>/freight.yaml`; `git log`
on that file is the gate's audit trail. Nothing promotes either
automatically, and Kargo offers `paper` only freight that passed `replay`,
and `stage` only freight that passed `paper`.

What the person promoting checks, and what is not yet automated:

- **replay** — the event log replayed under the candidate digests
  reproduces the recorded decisions. No in-tree job runs that replay
  against a digest today, so the check is the person's, and a Kargo
  `verification` that runs it is open work (Kargo verifications need Argo
  Rollouts' analysis templates, which are not installed).
- **paper** — the candidate digests ran a paper-trading soak without a
  reconciliation break or a halt nobody explained. Every environment is
  paper-only (ADR 0003); this gate is a soak, not a change of posture.
  One part of it is automated since R12-CICD-7120:
  `base/papertask.yaml`'s `verify-paper-ceiling` runs before `record-gate`
  and fails the promotion unless `QIP_AUTONOMY_CEILING` is exactly
  `paper_trading` in all six `RunService` manifests under `envs/stage/` and
  `envs/prod/`, so a failed check records nothing and `stage` is offered
  nothing. It reads configuration, not the image, and it has never been
  evaluated by a controller: see the next section.

## Why `paper` has never received freight

Read from the manifests, not from a run. Nothing in the chain blocks freight:
the warehouse's subscriptions and the three-tag criterion match what
`deploy.yml` pushes (the commit sha as an immutable tag, into the one build
registry), `dev` promotes automatically, and `replay` and `paper` request
freight from `test` and `replay` with no `verification` anywhere, so Kargo
treats a successful promotion as verified. `paper` is reached only by a
person promoting out of `replay`. It has not been reached because nothing is
running: ADR 0093 suspended the dev control-plane cluster, so no Kargo has
discovered a digest, and `gates/*/freight.yaml` are empty for that reason.
That is a state of the world, not a defect in the tree. **Cloud validation is
blocked** on that suspension and on a billing decision; no live promotion
through `paper` has been run or is claimed.

## What refuses what

| Refusal | Where | Mechanism |
|---|---|---|
| Freight mixing commits | `Warehouse.spec.freightCreationCriteria` | the three images' tags — the commit sha `deploy.yml` pushes — must be equal, so no such freight is created |
| A tag that is not a commit sha | `Warehouse` subscriptions' `allowTagsRegexes` | `^[0-9a-f]{40}$` |
| A per-environment rebuild | `deploy.yml` | only `BUILD_ENVIRONMENT`'s run builds or pushes; any other dispatch with no digest in the build registry stops |
| Freight skipping a gate | `Stage.spec.requestedFreight.sources.stages` | `paper` takes from `replay`, `stage` from `paper` |
| An automatic gate | `ProjectConfig` | only `dev` has a promotion policy |
| A promotion to `prod` | `Stage/prod`'s promotion template | its only step is `fail`, naming ADR 0024 and 0036; there is no policy for it either |
| An unattested digest | Cloud Run's admission, not Kargo | the commit lands, Argo CD reports `Degraded`, a person reads it (decision 8) |

`deploy.yml` and `infra.yml` refuse prod on their own; the `fail` step does
not know about them, and that is the point of three.

## What a promotion writes

`base/promotiontask.yaml`: clone this repository at its default branch, set
the three images in `envs/<stage>/kustomization.yaml` by digest, commit with
the freight, every digest and the source commit in the message, push, and
notify the stage's Argo CD `Application` when this control plane can reach
it. `base/gatetask.yaml` does the same for a gate, into `gates/<stage>/`,
and notifies nothing. The committer is the write-scoped GitHub App of ADR
0036 decision 3, whose credential the bootstrap projects into the `qip`
namespace as `qip-repository`.

`argocdApplication` is set, in each overlay, only for the stage whose
Application lives on that control plane; today only `dev`'s exists.
