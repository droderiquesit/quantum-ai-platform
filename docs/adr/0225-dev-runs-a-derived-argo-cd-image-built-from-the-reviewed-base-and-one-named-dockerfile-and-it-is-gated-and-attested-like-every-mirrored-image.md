# ADR 0225: Dev runs a derived Argo CD image, built from the reviewed base and one named Dockerfile, and it is gated and attested like every mirrored image

- **Status**: **Accepted**, 2026-10-24, on delegated authority, as the record
  of a decision already applied. The owner delegated every remaining decision
  on 2026-10-24 (`docs/blueprint/owner-decisions-2026-10-24.md`), and the
  wave580 parent assigned this record to lane L9-DEPLOY-PREP. ADR 0093 says
  its remedy (2) "needs its own record, because it changes what 'vendored'
  means in this repository". The remedy has been running in dev since
  `vendor.yml` run 23 (2026-09-20), and until this record nothing recorded
  it. It decides no deployment and no new spend.
- **Date**: 2026-10-24
- **Keeps**: ADR 0093 unedited (its suspension is history; this record does
  not amend it), ADR 0036 (the control plane and the GitOps path), ADR 0028
  (a third-party image is adopted by review and mirroring, never by a
  scanner exception), ADR 0109 (no workspace dependency is added; the Go
  build lives in a discarded builder stage).

## Context

ADR 0093 (2026-09-20) suspended dev's control-plane cluster because upstream
`quay.io/argoproj/argocd` failed `vendor.yml`'s blocking gate
(`--severity CRITICAL --exit-code 1 --ignore-unfixed`) on CVE-2025-68121
(GO-2026-4337). The finding is in two binaries Argo CD downloads rather than
builds: kustomize, built on go1.24.0, and git-lfs. ADR 0093 measured that no
Argo CD tag then published fixed either binary. It named three remedies:
(1) upstream ships a fixed kustomize; (2) this platform builds kustomize from
source on a patched toolchain over the reviewed base; (3) a scanner
exception, which it refused.

Remedy (2) was built the same day, in the commit "Remove the vulnerable
bytes from the Argo CD image instead of excusing them" (the head
`vendor.yml` run 23 ran, `dce149dc`; that commit is not in this branch's
history, which reached it through a later merge), and has been the image dev
runs since. What was built, as the tree has it on 2026-10-24:

- `infrastructure/egress/derived-images.txt`, one line:
  `vendor/argocd infrastructure/docker/argocd-patched.Dockerfile vendor/argocd-patched v3.5.2-patched`.
  The base is `vendor/argocd`, the line in
  `infrastructure/egress/vendored-images.txt` pinned at
  `quay.io/argoproj/argocd@sha256:e2aadfae709d904e87f46ba4aa49601d827b3022db22cd4d03aae816a2e7097b`
  (v3.5.2).
- `infrastructure/docker/argocd-patched.Dockerfile`, which:
  - builds kustomize `v5.8.1` from its tagged source
    (`go install sigs.k8s.io/kustomize/kustomize/v5@${KUSTOMIZE_VERSION}`) in
    a builder pinned at
    `golang@sha256:564e366a28ad1d70f460a2b97d1d299a562f08707eb0ecb24b659e5bd6c108e1`,
    and refuses the build unless the result reports the same version;
  - copies it over `/usr/local/bin/kustomize` in the base;
  - deletes `/usr/local/bin/git-lfs` and its git filter configuration, and
    refuses to ship if the binary is still there.
- `vendor.yml` builds it, pushes it under a tag fingerprinted by the base
  digest and the Dockerfile (`v3.5.2-patched-38c29b313586` in the runs read
  below), scans it with the same blocking gate, and signs it with the same
  attestor. The base line stays in the mirror list, still scanned, still
  left unattested when it fails.
- Dev's bootstrap overlay pins the derived digest:
  `infrastructure/gitops/bootstrap/argocd/overlays/dev/kustomization.yaml`
  names `vendor/argocd-patched` at
  `sha256:d0c2521327609c71da266792ccf3118f39df71e74205695d518bcedebce63f49`
  (`e3f81695`). The test, stage and prod overlays carry the stand-in
  `sha256:04fbf149…`, a digest built into a project that no longer exists;
  those three environments are unprovisioned.

## The evidence, read from the runs on 2026-10-24

- **`vendor.yml` run 31** (`38193569061`, `workflow_dispatch`, 2026-10-12,
  conclusion `success`). The base refused, as expected:
  `vendor/argocd (quay.io/argoproj/argocd@sha256:e2aadfae…) fails the CRITICAL gate; mirrored, not attested.`
  The run then printed `mirrored and attested 9 image(s).`, built
  `vendor/argocd-patched`, and its blocking scan listed 0 findings. It
  printed
  `##[notice]vendor/argocd-patched = sha256:d0c2521327609c71da266792ccf3118f39df71e74205695d518bcedebce63f49`.
  That is the digest `e3f81695` pinned.
- **`infra.yml` run 121** (`38988479925`, `apps dev`, 2026-10-17, conclusion
  `success`) printed
  `bootstrapped qip-dev-control-plane; qip-dev is Synced and Healthy, and Argo CD reconciles infrastructure/gitops/envs/dev from here on.`
  and
  `IAP is enabled on qip-dev-portal; it is read back as the run.googleapis.com/iap-enabled annotation (true) …`.
  So Argo CD started from the derived image and reconciled dev. This is the
  first run in which ADR 0093's cluster served its purpose.
- **`vendor.yml` run 32** (`39087301496`, `push` of the merge of #229,
  2026-10-18, conclusion `failure`). It found no fault in the remedy's bytes,
  but the gate's verdict on them changed:
  - The derive step found the fingerprinted tag already built at
    `sha256:d0c25213…` and re-scanned it, as designed: a vulnerability
    database moves under a fixed image.
  - The blocking scan now refuses it:
    `github.com/argoproj/argo-cd/v3 │ CVE-2026-77459 │ CRITICAL │ fixed │ v3.5.2+dirty │ 3.3.15, 3.4.10, 3.5.4, 3.6.0-rc2 │ Argo CD: AppProject destination-namespace and resource restrictions bypassed by PreDelete/PostDelete resource hooks…`.
    The finding is in Argo CD's own `usr/local/bin/argocd`, which the
    Dockerfile does not touch. It is fixed upstream in 3.5.4.
  - Earlier in the same run, the mirror step died at redis:
    `public.ecr.aws` answered the copy `TOOMANYREQUESTS: Data limit
    exceeded`, so redis and every line after it were not mirrored in that
    run.
  - The verdict step still printed
    `every vendored image is attested, or superseded by an attested derived image.`,
    because the attestation run 31 made on `d0c25213…` still exists. The
    job failed regardless, on the derive step's exit code.

## Decision

1. **A derived image is a legitimate way to adopt a third-party image whose
   upstream finding has no upstream fix**, on exactly these terms, which the
   tree already enforces:
   - the bytes that run are the reviewed base digest plus exactly the change
     in one named, committed Dockerfile;
   - the result goes through the same blocking CRITICAL gate as its base and
     is signed by the same attestor, and a derived image that still carries
     a finding is refused exactly as its base was;
   - the base stays in `vendored-images.txt`, mirrored, scanned and, when it
     fails, unattested;
   - every derived image is one line in `derived-images.txt`, and the
     overlay pins its digest, never its tag.
2. **"Vendored" therefore means one of two promises, and each file states
   its own.** For a line in `vendored-images.txt`, the bytes reviewed are the
   bytes that run. For a line in `derived-images.txt`, the bytes that run
   are the reviewed base plus the named Dockerfile. The second promise is
   weaker, and `derived-images.txt`'s header says so.
3. **A derivation removes or rebuilds third-party binaries. It never edits
   the platform's own logic into them.** The Argo CD Dockerfile rebuilds one
   tool at its upstream version and deletes another the platform does not
   use. A derivation that patched Argo CD's source would make this platform
   the maintainer of an Argo CD fork, and it needs its own record.
4. **The ordinary remedy wins when it exists.** When an upstream release
   fixes a finding, the base is bumped. A derivation is not kept to avoid
   bumping. Run 32's CVE-2026-77459 is that case: it is in Argo CD itself,
   fixed in 3.5.4, so the remedy is a base bump to a release at or above
   3.5.4 with the same Dockerfile, not a second derivation.

## Consequences

- Dev's running Argo CD is `d0c25213…`, which the gate passed on 2026-10-12
  and refused on 2026-10-18. Its attestation from run 31 still exists, so
  Binary Authorization admits it. The gate's verdict no longer matches the
  attestation. Until the base is bumped, any `vendor.yml` run fails, and a
  rebuilt registry (owner step 7 of
  `docs/ops/runbooks/dev-redeploy-readiness.md`) would hold no attested
  Argo CD. This record does not bump the base. It changes which upstream
  digest is reviewed, and that is a reviewed change of its own, with a
  fresh `vendor.yml` run and a re-pinned overlay.
- The verdict step's message in run 32 read as success on an image the same
  run refused. That message reads the attestation store, not this run's scan.
  This record names the gap; it does not change `vendor.yml`, which is a
  protected path.
- Public ECR rate-limits anonymous pulls. One refusal stopped the mirror
  loop at redis in run 32, so a second run can fail for a reason unrelated
  to any image.

## What it costs

- **A weaker promise for the derived line.** A reviewer must read a
  Dockerfile as well as a digest. The derived digest is per-environment,
  because a container build does not reproduce byte-for-byte, so test,
  stage and prod each need their own build.
- **The platform becomes a builder of one third-party binary.** kustomize
  v5.8.1 is compiled here from its tagged source on a Go toolchain chosen
  here. A behaviour difference between that build and upstream's is this
  platform's to find. It was checked once: the version string is the same,
  and the platform's five overlays rendered byte-identical. It is not
  re-checked on every build.
- **Rebuilt kustomize stays at v5.8.1** although v5.8.2 (2026-09-30) and
  v5.8.3 (2026-10-08) now exist on the Go module proxy. Argo CD gates
  behaviour on kustomize's parsed version, so moving it is a reviewed
  change, not a free bump.
- **The derivation does not track Argo CD's own findings.** Run 32 shows
  that a derived image ages like any other, and that the fix for an Argo CD
  finding is a base bump this record cannot make by itself.

## What would make this wrong

1. An upstream Argo CD image whose bundled kustomize and git-lfs are built
   outside CVE-2025-68121's range. ADR 0093's measurement method applies:
   read the binaries' build stamps; do not rely on a release note. The
   derivation is then removed, and the overlays pin the upstream digest
   again.
2. Any derivation that changes behaviour the platform depends on, for
   example a kustomize build whose output differs on this repository's
   overlays. The derivation is then wrong and is withdrawn, not tuned.
3. A gate verdict that admits a derived image which its own run refused, or
   an attestation that outlives a refusal and is then read as a pass. Run 32
   shows that the verdict wording can do this. If an owner decision relies
   on that wording, the attestation is not evidence for that decision.

## Dissent

The finding is very likely unreachable here. This Argo CD renders only this
repository's own overlays and terminates no TLS session that an attacker
controls with the vulnerable code. On that reading the derivation spends a
builder's responsibility to remove a risk that does not reach anything.
This record keeps the derivation anyway, for the reason ADR 0093 refused an
exception: an argument that a finding is unreachable is how an exception is
written, and an exception written once outlives the release that needed it.

## Code it constrains

- `infrastructure/egress/derived-images.txt`
- `infrastructure/docker/argocd-patched.Dockerfile`
- `vendor.yml`'s derive and verdict steps (protected; unchanged by this
  record)
- `infrastructure/gitops/bootstrap/argocd/overlays/*/kustomization.yaml`

## Not decided here

- The Argo CD base bump to 3.5.4 or later that CVE-2026-77459 now needs, and
  the overlay re-pin that follows it.
- Changing `vendor.yml`'s verdict wording, or authenticating its public ECR
  pulls.
- Moving kustomize past v5.8.1.
- Derived images for test, stage or prod.
