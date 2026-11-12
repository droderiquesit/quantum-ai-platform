# ADR 0452: A Cloud Run request service is released by a traffic-split canary held by an SLO gate, and rolled back by promoting the previous freight

- **Status**: **Accepted** 2026-11-12 under the owner's 2026-10-31 delegation
  of in-repository design decisions (`docs/blueprint/owner-decisions-2026-10-31.md`),
  session t50-13 (wave580). It adds no controller, no dependency and no
  billed resource.
- **Date**: 2026-11-12
- **Row**: R12-CICD-9116 — "GKE services canary with SLO gate and instant
  previous-digest rollback"; verification "Rollback to the previous digest is
  one action".
- **Builds on**: ADR 0036 (Kargo promotes, Argo CD syncs, Config Connector
  reconciles Cloud Run), ADR 0114 D4 and Amendment 1 (this row is a
  capability built on the runtime of record, not the Argo Rollouts mechanism,
  which stays refused), ADR 0115 (every environment is promoted the digest
  built once), ADR 0121 (each process pushes its own series to Cloud
  Monitoring).

## Context

Rolling back used to mean dispatching `deploy.yml` for an earlier commit, and
since ADR 0036 it means a Kargo promotion of earlier freight, which swaps the
image digest and makes Cloud Run create, pull and start a new revision. That
is neither instant nor proven to land on the previous digest's bytes, and
every release took 100% of traffic at once. The requirement's mechanism, Argo
Rollouts on GKE, is refused by ADR 0114 D4; Amendment 1 lists this row as the
capability to build on Cloud Run: a traffic-split canary, an SLO gate read
from the pushed series, and rollback to the previous promoted digest.

## Decision

1. **Only a request service is canaried by traffic, and today that is
   `qip-api`.** A traffic split gives two revisions a share of *requests*.
   `qip-fastbrain` and `qip-deepbrain` are `minInstanceCount = maxInstanceCount
   = 1` singletons running a loop; a split would run two loops, two decision
   cycles over one book. They keep the plain digest swap. A singleton is never
   canaried by traffic.

2. **The manifest is the state; Kargo's promotion writes the split.**
   `envs/<env>/api.yaml` always carries two traffic targets: index 0 tagged
   `stable`, index 1 tagged `standby`. Every revision is named
   `qip-<env>-api-<first twelve hex of its image digest>`, so the previous
   promoted digest's revision has a name the promotion knows without asking
   Cloud Run. `kargo/base/promotiontask.yaml` reads the two targets and, for
   the incoming digest's revision `next`:
   - `next` is `standby` — completion of a canary, or return to the previous
     digest: 100% to `next`, the old stable kept at 0% under `standby`;
   - `next` is `stable` — a canary rolled back (or re-asserted): 100% stable,
     the canary kept at 0% under `standby`;
   - otherwise — a new digest: `100 - canaryPercent` stable, `canaryPercent`
     to `next` under `standby`.
   `canaryPercent` is 10, a design value of this record and not an owner
   figure; a value outside 1–50 fails the promotion rather than being
   clamped. An environment never promoted into (`TO-PIN`) takes its first
   promotion at 100%: there is nothing to split with.

3. **The SLO gate is an Argo CD PostSync hook that fails closed.**
   `envs/<env>/slo-gate.yaml` runs a wave after `prove-serving.yaml`, finds
   the revision under `standby` with a share above zero, soaks 600 s, and
   reads that revision's own `qip_service_requests_total` and
   `qip_service_errors_total` from Cloud Monitoring. It fails the sync — and
   so the promotion's `argocd-update`, and so the freight's verification in
   that stage — unless the canary served at least one request at
   availability ≥ 0.99, the objective `service_objectives()` already declares
   for `SERVICE_ERROR_OBJECTIVE`. No series, no request, an unreadable answer
   and an API error are each a refusal. To tell the canary's series from the
   stable revision's, `qip-telemetry-drain` now leads the `generic_task` task
   id with `K_REVISION`, which Cloud Run sets, and refuses a malformed one at
   start-up. The Argo CD identity gains `roles/monitoring.viewer`, read only.

4. **Rollback to the previous digest is one action, and it is a person's.**
   `kargo promote --stage <env> --freight <previous freight>` routes 100% back
   to the previous digest's revision, which is still in the service at 0%:
   no build, no pull, no new revision. Completing a canary whose gate passed
   is the same single action with its own freight. No controller and no agent
   moves traffic on its own; an automatic revert on a refused gate is
   R12-CICD-9115's, and needs its own record.

## What it costs

- **A configuration edit to `api.yaml` must rename the revision.** The name
  is derived from the digest, and Cloud Run refuses a revision name that
  already exists with a different configuration. An edit that changes the
  template without a new digest fails its sync loudly until the name is
  changed (append `-c<n>`); the next promotion restores the digest form.
- **Completion is a second promotion.** A canary sits at its share until a
  person re-promotes its freight or promotes the previous one. Kargo cannot
  require that a re-promotion's freight passed the gate in that stage; the
  gate's verdict is in the failed sync and Kargo's freight view, not enforced
  on the second promotion.
- **A 600 s soak lengthens every promotion of qip-api**, and a canary that
  receives no request in the soak is refused — an idle dev rolls back rather
  than passes.
- **Unproven against a live controller.** The control plane is suspended
  (ADR 0093). The Kargo expressions (`Digest[7:19]`, `int(...)`, integer
  `value`s in `yaml-update`), Cloud Run's acceptance of two targets naming
  one revision under different tags, and the Monitoring `starts_with` filter
  are read and pinned by `qip-acceptance`'s `cloud_run_canary` suite and have
  never been evaluated.

## Rejected

- *Argo Rollouts, or its AnalysisTemplate CRDs for a Kargo verification*:
  ADR 0114 D4 refuses the mechanism, and the CRDs are not installed.
- *`gcloud run services update-traffic` from `deploy.yml`*: since ADR 0036
  the manifest owns traffic; an out-of-band split is drift Config Connector
  reverts.
- *Revision names from the freight*: the previous digest's revision would
  then be known only through Kargo's history; a digest-derived name makes
  "previous digest" and "previous revision" the same fact.

## What would make this wrong

A canary revision whose series cannot be selected by its task-id prefix; a
traffic split on a singleton; a gate that passes on no data; a rollback that
needs more than one promotion or creates a revision.
