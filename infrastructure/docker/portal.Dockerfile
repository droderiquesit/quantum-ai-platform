# The portal on Cloud Run, per the v4 architecture's app tier.
#
# Build context is frontend/ — the npm workspace root — because the portal's
# dependencies hoist there and Next's standalone tracer needs the workspace
# root to see them (outputFileTracingRoot in the portal's next.config.ts).
# The runtime stage carries only the traced output: server.js and the
# node_modules the tracer proved are reached, not the 400MB install.
#
# Both stages are pinned by digest, not by tag. The lockfile pins the npm tree
# by integrity hash and says nothing whatever about the base image, so a comment
# claiming the digests "live in the lockfile" described a control that was not
# there — and the second stage is not a builder: its bytes are the deployed
# image. The digest is the multi-arch index for docker.io/library/node:22-alpine
# (node 22.23.2), read from Docker Hub's registry v2 API. Both stages name the
# same one, so the runtime cannot drift away from what the build ran on.

FROM node:22-alpine@sha256:c610fcdfb1d5b4740dd70c284ed3cb16bb857e0f7166196e36a5501df7a3aa32 AS build
WORKDIR /src
COPY . .
RUN npm ci --no-audit --no-fund
WORKDIR /src/portal
RUN npm run build

FROM node:22-alpine@sha256:c610fcdfb1d5b4740dd70c284ed3cb16bb857e0f7166196e36a5501df7a3aa32
# Same non-root discipline as the platform images.
USER node
WORKDIR /app
ENV NODE_ENV=production PORT=8080 HOSTNAME=0.0.0.0
# Standalone output is rooted at the workspace, so server.js sits under
# portal/; static assets and public/ are served from paths relative to it.
COPY --from=build --chown=node:node /src/portal/.next/standalone ./
COPY --from=build --chown=node:node /src/portal/.next/static ./portal/.next/static
COPY --from=build --chown=node:node /src/portal/public ./portal/public
# Remove npm from the runtime image, because nothing here runs it.
#
# `deploy.yml` run 209 failed this image on `trivy --severity CRITICAL,HIGH
# --exit-code 1 --ignore-unfixed`: one CRITICAL and ten HIGH, every one of
# them in the npm tree the base image ships at
# /usr/local/lib/node_modules/npm — `tar` (CVE-2026-59873, node-tar denial of
# service via a crafted gzip bomb, 7.5.11 against a fixed 7.5.19), plus
# `brace-expansion`, `pacote`, `sigstore`, `picomatch` and `ip-address`.
# None is a dependency of the portal: they are npm's own, and the lockfile
# does not name one of them.
#
# The runtime stage runs `node portal/server.js` against the standalone
# output the builder traced. It installs nothing, fetches nothing, and
# resolves nothing at start; npm is present only because the base image
# carries it. So the vulnerable bytes are deleted rather than excused —
# the same answer `infrastructure/docker/argocd-patched.Dockerfile` gives
# for git-lfs, and for the same reason: the derived image is scanned by the
# gate the base failed, so if the finding survived, the gate would still
# refuse it. `deploy.yml`'s own comment says the gate is not relaxed for
# this image, and it is not relaxed here.
#
# `corepack` goes with it: it is npm's package-manager shim, it is dead
# weight in a runtime that resolves nothing, and leaving it would leave a
# second copy of some of the same trees.
#
# The refusal to ship is the last line rather than a comment. A `RUN rm`
# that silently did nothing would leave the bytes and pass this stage, and
# the finding would come back from the scanner instead of from here.
USER root
RUN set -eu; \
    rm -rf /usr/local/lib/node_modules/npm \
           /usr/local/lib/node_modules/corepack \
           /usr/local/bin/npm /usr/local/bin/npx /usr/local/bin/corepack; \
    test ! -e /usr/local/lib/node_modules/npm; \
    test ! -e /usr/local/bin/npm; \
    node --version

# The other half of the same finding, in the operating system rather than in
# npm: `libcrypto3` and `libssl3` at 3.5.7-r0, against a fixed 3.5.8-r0
# (CVE-2026-14456, unbounded memory in OpenSSL). The gate is
# `--severity CRITICAL,HIGH --ignore-unfixed`, so a HIGH with a fix
# published refuses the image exactly as the CRITICAL does, and deleting
# npm alone would have left the build failing on a second cause that looks
# like the first.
#
# Upgraded by name rather than by `apk upgrade`, which would move every
# package in the image and make the diff between two builds of the same
# commit unreviewable. Two packages move, both from the same source, and
# the build asserts afterwards that they actually moved: an upgrade that
# silently found nothing would ship the vulnerable bytes and pass this
# stage, which is the failure the `test ! -e` above exists to prevent one
# layer up.
#
# The base image's digest is deliberately not bumped to chase this. A newer
# `node:22-alpine` is a different Node, a different npm and a different
# everything, reviewed by nobody, to fix two named packages; the digest
# above stays the reviewed one and the two packages are named here where a
# reader can see them.
# `>=` rather than `=`: the constraint is the fix, not one build of it.
# `apk` refuses the install outright if it cannot satisfy the bound, so the
# floor is enforced by the package manager and a pin to one exact release
# would instead break the build on the day Alpine ships the next one — a
# failure that says nothing about this image's security.
#
# No pipe anywhere in this stage. `apk info -v … | head -1` reads the
# version through a pipe, and the default `/bin/sh` reports only the right
# side's status, so an `apk info` that failed would be hidden by a `head`
# that succeeded and the case below would fall through to success on an
# empty string. That is hadolint DL4006, and it is the same defect this
# session opened with in `argocd-patched.Dockerfile`; the word-splitting
# below takes the first field with no second process involved.
RUN set -eu; \
    apk add --no-cache "libcrypto3>=3.5.8-r0" "libssl3>=3.5.8-r0"; \
    installed="$(apk info -v libcrypto3)"; \
    set -- ${installed}; \
    installed="$1"; \
    echo "libcrypto3 is now ${installed}"; \
    case "${installed}" in \
      libcrypto3-3.5.[0-7]-*|"") \
        echo "libcrypto3 reads '${installed}'; the upgrade did not take and the image would ship the finding" >&2; \
        exit 1 ;; \
    esac
USER node
EXPOSE 8080
CMD ["node", "portal/server.js"]
