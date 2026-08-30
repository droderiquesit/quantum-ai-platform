# The landing on Cloud Run — the public front door.
#
# Build context is frontend/landing, which keeps its own dependency tree
# deliberately (ADR 0015/0016). Same standalone-output shape as the portal:
# the runtime stage is server.js plus traced node_modules only.

FROM node:22-alpine AS build
# NEXT_PUBLIC_* values are inlined into the client bundles at build time,
# so the portal URL is a build argument, not a runtime setting — changing
# it means rebuilding, which is the honest cost of a static inline.
ARG PORTAL_URL=http://127.0.0.1:3400
ENV NEXT_PUBLIC_ALGORIK_PORTAL_URL=$PORTAL_URL
WORKDIR /src
COPY . .
RUN npm ci --no-audit --no-fund && npm run build

FROM node:22-alpine
USER node
WORKDIR /app
ENV NODE_ENV=production PORT=8080 HOSTNAME=0.0.0.0
COPY --from=build --chown=node:node /src/.next/standalone ./
COPY --from=build --chown=node:node /src/.next/static ./.next/static
COPY --from=build --chown=node:node /src/public ./public
EXPOSE 8080
CMD ["node", "server.js"]
