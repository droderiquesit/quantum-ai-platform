import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  reactStrictMode: true,
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
  headers: async () => [
    {
      source: "/:path*",
      headers: [
        { key: "X-Content-Type-Options", value: "nosniff" },
        { key: "Referrer-Policy", value: "no-referrer" },
        { key: "X-Frame-Options", value: "DENY" },
      ],
    },
  ],
};

export default nextConfig;
