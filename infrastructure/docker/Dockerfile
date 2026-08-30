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
# The toolchain is pinned by backend/rust-toolchain.toml, so the tag below only
# has to be new enough to honour it.

FROM rust:1.94-alpine AS build

ARG BINARY

# musl-dev is the only build dependency: alpine's Rust images target musl, which
# is what makes the output static.
# Unpinned deliberately (hadolint DL3018): musl-dev tracks the alpine minor
# inside the pinned rust image tag above, which is the actual version pin.
# Pinning here would break every time alpine rotates the package revision,
# and the failure would name a package version nobody chose.
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

# The same uid the manifests set. `runAsNonRoot` is checked against the image's
# user when the manifest does not name one, so an image that would run as root
# is refused at admission rather than discovered later.
USER 10001:10001

COPY --from=build /out/qip /qip

ENTRYPOINT ["/qip"]
