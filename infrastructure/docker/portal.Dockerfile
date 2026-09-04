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
EXPOSE 8080
CMD ["node", "portal/server.js"]
