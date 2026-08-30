# The portal on Cloud Run, per the v4 architecture's app tier.
#
# Build context is frontend/ — the npm workspace root — because the portal's
# dependencies hoist there and Next's standalone tracer needs the workspace
# root to see them (outputFileTracingRoot in the portal's next.config.ts).
# The runtime stage carries only the traced output: server.js and the
# node_modules the tracer proved are reached, not the 400MB install.
#
# The builder tag pins the node major; exact digests live in the lockfile
# where dependency review happens, matching the rust builder's convention.

FROM node:22-alpine AS build
WORKDIR /src
COPY . .
RUN npm ci --no-audit --no-fund
WORKDIR /src/portal
RUN npm run build

FROM node:22-alpine
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
