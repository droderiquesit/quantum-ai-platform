# The fleet worker's image (ADR 0102): the worker, the gateway it calls, and
# the price table. Three files and a Node runtime, and nothing else.
#
#   podman build -f infrastructure/fleet/worker/Dockerfile -t fleet-worker scripts/
#
# The build context is scripts/, not the repository root. The worker has no
# checkout by design (ADR 0102 decision 1: a worker with the repository in its
# image has the repository as its prompt-injection surface), and a context
# that cannot see the tree cannot copy it in by a careless `COPY . .`. The
# three COPY sources below are the whole of what reaches the image; the
# dispatcher and every test stay on the desktop.
#
# The base is pinned by digest, not by tag: a tag is a name its owner can move
# after the review that approved it (`.claude/rules/domains/infrastructure.md`,
# and `infrastructure/docker/README.md` for the same discipline on the other
# images). The digest is the multi-arch index for
# docker.io/library/node:22-slim, resolved read-only on 2026-10-04:
#
#   skopeo inspect --raw docker://docker.io/library/node:22-slim | sha256sum
#     43ac6c60b8f89723f746e8a92ce91abd5017e627ce1ddfe4238355d3a30b772c
#   skopeo inspect docker://docker.io/library/node:22-slim
#     "Digest": "sha256:43ac6c60...", "Created": "2026-09-23T21:10:23Z",
#     NODE_VERSION=22.23.3
#
# The two commands agree, which is the check that the pinned value is the
# index and not one platform's manifest. Node 22 rather than the `lts` tag's
# 24 (24.21.0 that day): 22 is the major `ci.yml` and the portal image pin
# and the one the tests are run on at the desk, so the bytes that pass the
# tests and the bytes that run the packet are the same runtime. It is in LTS
# maintenance; moving to 24 is an edit to this line and a re-run of the tests
# inside the image.
FROM docker.io/library/node:22-slim@sha256:43ac6c60b8f89723f746e8a92ce91abd5017e627ce1ddfe4238355d3a30b772c

# Nothing here installs, resolves or fetches a package: the worker is Node
# built-ins only (ADR 0102 decision 6, item 8). npm, corepack and yarn are in
# the image only because the base carries them, and their bundled trees are
# what failed `deploy.yml`'s scan on the portal image (see
# `infrastructure/docker/portal.Dockerfile`). Deleted rather than excused, and
# the build fails if the delete silently did nothing.
RUN set -eu; \
    rm -rf /usr/local/lib/node_modules /opt/yarn-* \
           /usr/local/bin/npm /usr/local/bin/npx /usr/local/bin/corepack \
           /usr/local/bin/yarn /usr/local/bin/yarnpkg; \
    test ! -e /usr/local/lib/node_modules; \
    test ! -e /usr/local/bin/npm; \
    test ! -e /usr/local/bin/yarn; \
    node --version

# The repository's own relative layout, because the worker imports
# `../model-gateway.mjs` and reads `./prices.json` beside itself. Owned by
# root and not writable by the user below: a worker cannot rewrite its own
# price table.
WORKDIR /app/scripts
COPY model-gateway.mjs ./
COPY fleet/worker.mjs fleet/prices.json ./fleet/

# Non-root, like every other image here. The credential is the metadata
# server's token for the attached account; nothing is mounted and no key
# exists to protect on disk.
USER node

ENTRYPOINT ["node", "/app/scripts/fleet/worker.mjs"]
