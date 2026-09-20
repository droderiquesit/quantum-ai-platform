# syntax=docker/dockerfile:1
#
# Argo CD, with the two vulnerable third-party binaries removed rather than
# excused.
#
# ## Why this file exists at all
#
# `vendor.yml`'s blocking CRITICAL gate refuses the upstream Argo CD image on
# CVE-2025-68121 (GO-2026-4337, unexpected session resumption in crypto/tls,
# fixed in go1.24.13 / go1.25.7 / go1.26.0-rc.3). The finding is not in Argo
# CD's own binary. It is in two third-party binaries the image copies in:
#
#   /usr/local/bin/kustomize   built with go1.24.0
#   /usr/local/bin/git-lfs     built with go1.25.3
#
# Pinning a newer Argo CD does not clear it, and that was measured rather
# than read off a release note — each image's `COPY /usr/local/bin/<tool>`
# layer was pulled from quay's v2 API and the Go build stamp read out of the
# binary. v3.5.3 ships byte-identical kustomize and git-lfs layers to v3.5.2;
# v3.6.0-rc1 rebuilds git-lfs and helm and still carries kustomize on
# go1.24.0. Nor is there a newer kustomize to adopt: the Go module proxy ends
# at v5.8.1, dated 2026-02-09, which is the version already in the image.
#
# The reason upstream's binary is old is worth stating, because it is what
# makes this file the right answer rather than a workaround: Argo CD does not
# build kustomize. `hack/installers/install-kustomize.sh` downloads the
# prebuilt GitHub release tarball, which is why a go1.24.0 binary sits inside
# an image whose own builder stage is a much newer Go. The bytes are stale
# because they were downloaded, not because anything requires them.
#
# ## What this does, and why it is not a suppression
#
# It removes the vulnerable bytes. Nothing is told to look away: the derived
# image is scanned by the same `--severity CRITICAL --exit-code 1` gate the
# upstream image failed, and attested by the same attestor. If the finding
# survived, the gate would still refuse it.
#
#   * **git-lfs is deleted.** It is dead weight in this platform: no
#     `.gitattributes` in this repository declares a `filter=lfs`, no
#     Repository secret sets `enableLfs`, and Argo CD shells out to `git lfs`
#     only inside `if m.IsLFSEnabled()`. Deleting it removes a whole binary's
#     worth of attack surface and breaks nothing.
#   * **kustomize is rebuilt from its own tagged source** at the same version
#     upstream ships, on a Go toolchain that carries the fix. kustomize is
#     genuinely required — every Argo CD Application here points at a
#     kustomization — so it is replaced, not removed.
#
# ## Why overwriting the file needs no Argo CD configuration
#
# Argo CD's `getBinaryPath()` falls back to the literal `kustomize` on PATH
# when no per-version path is registered, so writing over
# /usr/local/bin/kustomize is enough. The alternative — an initContainer and
# a `kustomize.path.*` entry in argocd-cm — would leave the vulnerable binary
# in the image, where Trivy scans the filesystem and would still find it.
#
# ## The two things that could have made this subtly wrong, both checked
#
# 1. **The version string.** Argo CD parses `kustomize version` and gates
#    feature behaviour on the parsed semver, so a build reporting something
#    else would change behaviour silently. A binary built by
#    `go install sigs.k8s.io/kustomize/kustomize/v5@v5.8.1` reports `v5.8.1`,
#    identical to the bundled one — the module version is carried in Go build
#    info and needs no ldflags.
# 2. **The rendering.** Both binaries were run against all five of this
#    platform's own overlays — envs/dev and the four bootstrap overlays — and
#    produced byte-identical output, some 62,000 lines. That is the check
#    worth trusting, because it compares the two on the exact inputs this
#    cluster will feed them.
#
# ## Pins
#
# The base is passed in by `vendor.yml` as the digest already reviewed in
# infrastructure/egress/vendored-images.txt, so this file and that list
# cannot disagree about which Argo CD is being patched. The builder is
# pinned by digest for the same reason every other image here is: a tag is
# whatever its owner says it is today.
ARG ARGOCD_BASE
ARG KUSTOMIZE_VERSION=v5.8.1

FROM docker.io/library/golang@sha256:564e366a28ad1d70f460a2b97d1d299a562f08707eb0ecb24b659e5bd6c108e1 AS build
ARG KUSTOMIZE_VERSION
# GOTOOLCHAIN=local refuses a toolchain switch: kustomize's go.mod declares
# `go 1.24.0`, and without this Go would be free to download and use a
# different toolchain than the one pinned above — which is the whole control
# this stage exists to exercise.
ENV CGO_ENABLED=0 GOFLAGS=-trimpath GOTOOLCHAIN=local
RUN go install "sigs.k8s.io/kustomize/kustomize/v5@${KUSTOMIZE_VERSION}"
# Prove the rebuild before it is allowed to leave this stage. A binary that
# reports a different version would change Argo CD's behaviour silently, and
# the build must fail rather than ship it.
RUN set -eu; \
    built="$(/go/bin/kustomize version)"; \
    echo "rebuilt kustomize reports: ${built}"; \
    case "${built}" in \
      "${KUSTOMIZE_VERSION}"*) ;; \
      *) echo "rebuilt kustomize reports '${built}', not ${KUSTOMIZE_VERSION}" >&2; exit 1 ;; \
    esac; \
    go version -m /go/bin/kustomize | head -3

FROM ${ARGOCD_BASE}
USER root
COPY --from=build /go/bin/kustomize /usr/local/bin/kustomize
RUN set -eu; \
    rm -f /usr/local/bin/git-lfs; \
    git config --system --unset-all filter.lfs.clean   || true; \
    git config --system --unset-all filter.lfs.smudge  || true; \
    git config --system --unset-all filter.lfs.process || true; \
    git config --system --unset-all filter.lfs.required || true; \
    # Refuse to ship if either change did not take. A derived image that
    # silently kept the vulnerable binary would be attested and admitted.
    test ! -e /usr/local/bin/git-lfs; \
    /usr/local/bin/kustomize version
# Back to the uid the upstream image runs as. ARGOCD_USER_ID=999 is set in
# the base; leaving this image running as root would be a far worse defect
# than the one it fixes.
USER 999
