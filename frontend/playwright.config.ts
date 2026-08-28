import { defineConfig, devices } from "@playwright/test";

/**
 * The suite runs against a production build of the app with no backend behind
 * it. Every test stubs the network at the browser boundary (`page.route`), so
 * the application code under test is the same code that runs in a deployment:
 * there is no mock data path inside the app itself.
 */
const PORT = Number(process.env.PLAYWRIGHT_PORT ?? 3311);
const BASE_URL = `http://127.0.0.1:${PORT}`;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 2 : undefined,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : [["list"]],
  timeout: 45_000,
  expect: { timeout: 10_000 },
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    // A command centre is a desktop tool first; the tablet breakpoint is
    // covered by its own project below.
    viewport: { width: 1600, height: 1000 },
  },
  projects: [
    {
      name: "desktop-chromium",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1600, height: 1000 } },
    },
    {
      name: "tablet-chromium",
      use: { ...devices["Desktop Chrome"], viewport: { width: 900, height: 1180 } },
      testMatch: /(shell|navigation)\.spec\.ts/,
    },
  ],
  webServer: {
    command: `npm run start -- --port ${PORT} --hostname 127.0.0.1`,
    url: BASE_URL,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    stdout: "ignore",
    stderr: "pipe",
    env: {
      // Deliberately pointed at a port nothing listens on. Any request the
      // tests forget to stub fails loudly as a gateway error rather than
      // silently succeeding against something real.
      QIP_API_BASE_URL: "http://127.0.0.1:9",
      QIP_API_TIMEOUT_MS: "1500",
      NEXT_PUBLIC_QIP_ENVIRONMENT: "test",
    },
  },
});
