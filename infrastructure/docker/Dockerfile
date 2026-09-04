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

# musl-dev is the only build dependency: alpine's Rust images target musl, which
# is what makes the output static.
#
# Unpinned deliberately (hadolint DL3018), and the limit of that is stated here
# rather than left to be discovered: the digest above freezes the alpine minor
# and the repository list this pulls from, but not the package revision alpine
# serves today. Pinning the revision would break every time alpine rotates it,
# and the failure would name a version nobody chose. So this line is the one
# part of the build that is not byte-reproducible — and it is a C toolchain in
# the builder, not anything that ships: the final stage is scratch.
# hadolint ignore=DL3018
RUN apk add --no-cache musl-dev

WORKDIR /src
# The build context is the repository root; only the Rust workspace under
# backend/ participates in the image. Copying the whole context would drag the
# frontend's node_modules into every image build for nothing.
COPY backend/ .

# --locked, because a build that resolves a different dependency graph from the
# one the test suite ran against is not the thing that was tested.
RUN test -n "$BINARY" \
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
