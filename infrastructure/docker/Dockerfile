# One image per binary, from the same source tree.
#
# `BINARY` selects which. One file rather than five means the five images
# cannot drift apart in the ways that matter — base image, user, filesystem
# layout — and drift between images is the kind of difference nobody notices
# until one of them behaves differently under load.
#
# The result is a statically linked binary on an empty filesystem. There is no
# shell, no package manager and no libc to reach: a container with nothing in it
# is a container an attacker who reaches it can do very little with. The
# platform depends on serde and serde_json and nothing else, and implements its
# own HTTP, hashing and numerics, so there is nothing here that needs a dynamic
# library.
#
# backend/rust-toolchain.toml pins the toolchain to 1.94.1 and this image's own
# RUST_VERSION is 1.94.1, so the compiler that builds here is the one the test
# suite ran against and rustup fetches nothing at build time.
#
# Pinned by digest, not by tag. `rust:1.94-alpine` moves under this file every
# time the base is rebuilt, so two builds of the same commit could use two
# different compilers and two different alpine package sets, and the second one
# would be the first anybody heard of it. A tag is a name its owner can move
# after the review that approved it. The tag is kept beside the digest because
# it says what the bytes are; Docker resolves the digest and ignores it.
#
# This is the discipline infrastructure/egress/vendored-images.txt already
# applies to the two third-party images, applied to the builders as well. The
# digest is the multi-arch index for docker.io/library/rust:1.94-alpine, read
# from Docker Hub's registry v2 API. Moving to a newer base is an edit to this
# line, which is a line a reviewer sees.

FROM rust:1.94-alpine@sha256:77237dd363a0b127bb5ef532c2d64c0deb380b738e43a9c4bdac73398d6d0a08 AS build

ARG BINARY
ARG SOURCE_DATE_EPOCH
ARG QIP_SOURCE_COMMIT

# No package is installed here. musl-dev and gcc, the only build dependencies
# (alpine's Rust images target musl, which is what makes the output static),
# are layers of the digest above: rust-lang/docker-rust's own Dockerfile runs
# `apk add musl-dev gcc` when it builds the image, so their revisions are
# frozen by the digest like everything else in it.
#
# This line used to run `apk add --no-cache musl-dev` again, which refreshed
# alpine's index over the network on every build: the one step whose input was
# not pinned by anything in this repository (CICD-048). Pinning its revision
# instead was tried and is worse — alpine keeps only the latest revision in a
# release branch, so a pin breaks the build on the day alpine rotates it, and
# the failure names a version nobody chose. Moving musl or gcc is now a digest
# edit, which is a line a reviewer sees.
#
# The acceptance test `the_build_stage_installs_nothing_the_digest_does_not_pin`
# refuses an `apk` invocation coming back.

WORKDIR /src
# The build context is the repository root; only the Rust workspace under
# backend/ participates in the image. Copying the whole context would drag the
# frontend's node_modules into every image build for nothing.
COPY backend/ .

# --locked, because a build that resolves a different dependency graph from the
# one the test suite ran against is not the thing that was tested.
#
# SOURCE_DATE_EPOCH is required and is the commit's own timestamp —
# `deploy.yml` passes `git log -1 --format=%ct` (R12 CICD-9097). It used to
# fall back to `$(date +%s)` when absent, and `deploy.yml` passed exactly
# that, so two builds of one commit embedded two times and could never share
# a digest. A build without it is refused rather than stamped with the
# clock: an image whose timestamp is the build's is an image nobody can
# rebuild. The pipeline also builds every image twice and compares, so
# whether this is enough is checked on every build rather than asserted here.
#
# QIP_SOURCE_COMMIT is required too, and is the full sha being built: the
# kernel reads it at compile time into every decision record's
# `code_version` (R12-GOV-6121), and a deployed central binary built without
# it refuses to start. Nothing set it until 2026-10-11, so every deployed
# decision named the crate version and no commit. It is a function of the
# commit like the epoch, so both builds of one commit still agree.
RUN test -n "$BINARY" \
  && test -n "$SOURCE_DATE_EPOCH" \
  && test "${#QIP_SOURCE_COMMIT}" -eq 40 \
  && test -z "$(printf '%s' "$QIP_SOURCE_COMMIT" | tr -d '0-9a-f')" \
  && export SOURCE_DATE_EPOCH QIP_SOURCE_COMMIT \
  && cargo build --release --locked --bin "$BINARY" \
  && install -D "target/release/$BINARY" /out/qip

FROM scratch

# A fixed non-root uid, set here because there is nowhere else left to set it.
# Cloud Run runs what the image says: there is no pod security context and no
# admission controller that would refuse a root image on the way in — the
# manifests that once carried this were retired with Kubernetes under ADR 0024.
# Numeric, because scratch has no /etc/passwd in which to resolve a name.
USER 10001:10001

COPY --from=build /out/qip /qip

ENTRYPOINT ["/qip"]
