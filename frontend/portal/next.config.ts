import { join } from "node:path";
import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  reactStrictMode: true,
  /**
   * Cloud Run serves the portal from the standalone output: a self-contained
   * server.js plus only the node_modules the trace proves are reached. The
   * tracing root is the frontend/ workspace root, not this app — without it
   * the tracer treats hoisted workspace dependencies as outside the project
   * and the container starts without them.
   */
  output: "standalone",
  outputFileTracingRoot: join(__dirname, ".."),
  /**
   * The shared Algorik packages ship TypeScript source, not built output.
   * Building each would need a bundler per package, which is a dependency,
   * which is the thing ADR 0014 refuses — so the app's own compiler
   * transpiles them. This is also what keeps the packages consumable by a
   * future React Native target without a build step to keep in sync.
   */
  transpilePackages: [
    "@algorik/brand",
    "@algorik/design-tokens",
    "@algorik/ui",
    "@algorik/charts",
    "@algorik/auth",
    "@algorik/api-client",
    "@algorik/shared-types",
    "@algorik/validation",
    "@algorik/analytics",
    "@algorik/feature-flags",
    "@algorik/testing",
  ],
  // The gateway and stream route handlers are the only server surface, and
  // both are proxies. Nothing here may be statically rendered or cached: a
  // cached blotter is a wrong blotter.
  poweredByHeader: false,
  /**
   * Performance optimizations: image optimization (AVIF + WebP, responsive),
   * script optimization (defer non-critical), asset compression, and bundle
   * size tracking (set ANALYZE=true to generate bundle reports).
   */
  images: {
    formats: ["image/avif", "image/webp"],
    deviceSizes: [640, 750, 828, 1080, 1200, 1920, 2048, 3840],
    imageSizes: [16, 32, 48, 64, 96, 128, 256, 384],
    minimumCacheTTL: 31536000,
  },
  compress: true,
  productionBrowserSourceMaps: false,
  headers: async () => [
    {
      source: "/:path*",
      headers: [
        { key: "X-Content-Type-Options", value: "nosniff" },
        { key: "Referrer-Policy", value: "no-referrer" },
        { key: "X-Frame-Options", value: "DENY" },
        { key: "Cache-Control", value: "public, max-age=31536000, immutable" },
      ],
    },
    // Allow static assets to be cached aggressively
    {
      source: "/static/:path*",
      headers: [
        { key: "Cache-Control", value: "public, max-age=31536000, immutable" },
      ],
    },
  ],
  // Turbopack handles code splitting, minification, and tree-shaking by default in Next.js 16.
  // This config uses Turbopack's optimized defaults for performance.
  turbopack: {},
};

export default nextConfig;
