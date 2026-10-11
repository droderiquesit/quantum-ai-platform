# ADR 0115: Every image is built once, into the dev registry, and later environments are promoted its digest

- **Status**: **Accepted**, 2026-10-11, by the owner in the session
  conversation, under the instruction to adopt the recommended answer for
  every open decision: "Update as needed remove my decisions make the
  recommended".
- **Date**: 2026-10-11
- **Requirements**: R12-CICD-9097, R12-CICD-9112 (both INCORRECT before this
  record's change); R12-SEC-9124 is unaffected.
- **Implemented by**: `.github/workflows/deploy.yml` (`BUILD_ENVIRONMENT:
  dev`, the promotion path that refuses to build), `infrastructure/docker/Dockerfile`
  (refuses a build without `SOURCE_DATE_EPOCH`), `infrastructure/gitops/kargo/`
  (the dev → test → replay → paper → stage → prod chain),
  `infrastructure/terraform/modules/registry` (`promotion_readers`).

## Context

Until 2026-10-11 `deploy.yml` built every image again for every environment,
into a registry of its own (`qip-${TARGET_ENVIRONMENT}`), and stamped the
build with `SOURCE_DATE_EPOCH=$(date +%s)`. Two builds of one commit were
therefore two different artefacts, and the thing tested in dev was never the
thing that ran in stage. Blueprint GCP v3.0 §14.2 asks for the opposite: one
immutable artefact promoted dev → integration → replay → paper → staging →
prod. A promotion that rebuilds is not a promotion; it is a second release
that happens to share a commit hash.

The `kargo/README.md` that preceded this change said that serving every
environment from one registry needed a recorded decision. This is it.

## Decision

1. **Build once.** Each commit's images are built once, in the `dev`
   environment's project, into its registry `qip-dev`. `BUILD_ENVIRONMENT` is
   read from dev's committed tfvars, never from a repository variable.
2. **Byte-reproducibly.** `SOURCE_DATE_EPOCH` is the commit time
   (`git log -1 --format=%ct`); layer timestamps are clamped to it; the
   Dockerfile refuses to build without it. The four Rust binaries are built
   twice and nothing is pushed unless the two images match. The portal is
   exempt and says so (Next.js stamps a random build id); its digest is still
   built once and promoted.
3. **Promote, never rebuild.** A dispatch for test, replay, paper, stage or
   prod scans the existing digest and attests it with its own environment's
   attestor, or stops; it never builds. The prod refusal, scan, signing,
   attestation and Binary Authorization gates are unchanged.
4. **Read-only access across projects.** Later environments read `qip-dev`
   through `registry_promotion_readers`, which grants `artifactregistry.reader`
   and nothing else. No later environment can write to the build registry.
5. **Two human gates.** `replay` and `paper` are Kargo approval stages that
   commit the promoted digests to `gitops/gates/<stage>/freight.yaml`, so what
   passed each gate is in the log.

## What it costs

- **Blast radius.** Every environment's images now live in dev's project. A
  compromised dev registry, or one deleted by mistake, affects prod's ability
  to pull. Binary Authorization still refuses an image no attestor signed,
  so a substituted image does not run; but an unavailable registry stops
  every environment's deploys, not only dev's.
- **Coupling.** Prod's deploy depends on dev's project existing and being
  reachable. Tearing dev down (it has been torn down before) blocks
  promotions until it is back.
- **Build time.** The Rust images are built twice per commit for the
  reproducibility comparison.
- **Runner requirement.** Timestamp rewriting needs BuildKit 0.13 or later.
- **No automated replay check yet.** The `replay` gate is a person's
  approval; an automated replay against the gate's freight is still to be
  built.

## What would make this wrong

- If an auditor or regulator requires each production artefact to be held in
  a production-owned project, the build registry moves to a dedicated build
  project (not prod's, not dev's) with the same read-only grants, and this
  record is superseded.
- If dev is routinely torn down, the coupling cost above dominates and the
  same move applies.
- If the two-build comparison is found to pass while the images differ in a
  way that matters (for example, a non-deterministic embedded resource the
  comparison does not see), reproducibility is not established and the
  comparison must be widened before this record's claim stands.

## Amendment 1 — 2026-10-11, the private pool is approved and the portal's exemption is to be closed

Made by the owner in the session under the instruction "You make the
recommended decision and have full permission". The status and decisions
above are unchanged.

1. **The Cloud Build private pool is approved for `dev`** (R12-CICD-9097):
   apply `modules/build-pool` through `infra.yml` with the plan shown, then
   set `cloud_build_private_pool_enabled = true` in `dev`'s tfvars in a
   separate commit, so that every build of record runs inside private
   networking with a keyless identity rather than on a hosted runner. The
   approval is for this apply; it is not standing approval for the next one.
2. **The portal's exemption from the two-build comparison is to be closed,
   not kept.** The random build id is the stated cause; the portal's build id
   is set to the commit, the comparison is extended to the portal image, and
   if a further source of non-determinism remains the exemption is kept only
   with that source named in `deploy.yml` beside it.
