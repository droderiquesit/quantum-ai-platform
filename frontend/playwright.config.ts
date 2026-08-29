import { defineConfig, devices } from "@playwright/test";

/**
 * The suite runs against a production build of the app with no backend behind
 * it. Every test stubs the network at the browser boundary (`page.route`), so
 * the application code under test is the same code that runs in a deployment:
 * there is no mock data path inside the app itself.
 */
const PORT = Number(process.env.PLAYWRIGHT_PORT ?? 3311);
const BASE_URL = `http://127.0.0.1:${PORT}`;

/**
 * A second app, in front of a real upstream, for the service-worker suite.
 *
 * That suite cannot use `page.route`: Playwright fulfils an intercepted request
 * before the service worker receives a `fetch` event, so a test of what the
 * worker caches would pass no matter what the worker did. Here the request
 * really crosses a socket, through the real gateway handler, so the worker
 * really gets to make — and be judged on — its decision.
 */
const WORKER_PORT = Number(process.env.PLAYWRIGHT_WORKER_PORT ?? 3312);
const WORKER_BASE_URL = `http://127.0.0.1:${WORKER_PORT}`;
const UPSTREAM_PORT = Number(process.env.PLAYWRIGHT_UPSTREAM_PORT ?? 3313);

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
      // The worker suite needs the app instance that has a real upstream
      // behind it; running it here would point it at the dead port.
      testIgnore: /worker\.spec\.ts/,
    },
    {
      name: "tablet-chromium",
      use: { ...devices["Desktop Chrome"], viewport: { width: 900, height: 1180 } },
      testMatch: /(shell|navigation)\.spec\.ts/,
    },
    {
      name: "worker-chromium",
      use: { ...devices["Desktop Chrome"], baseURL: WORKER_BASE_URL },
      testMatch: /worker\.spec\.ts/,
    },
  ],
  webServer: [
    {
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
    {
      command: `node tests/support/upstream-stub.mjs`,
      url: `http://127.0.0.1:${UPSTREAM_PORT}/api/v1/health`,
      reuseExistingServer: !process.env.CI,
      timeout: 30_000,
      stdout: "ignore",
      stderr: "pipe",
      env: { PORT: String(UPSTREAM_PORT) },
    },
    {
      command: `npm run start -- --port ${WORKER_PORT} --hostname 127.0.0.1`,
      url: WORKER_BASE_URL,
      reuseExistingServer: !process.env.CI,
      timeout: 120_000,
      stdout: "ignore",
      stderr: "pipe",
      env: {
        QIP_API_BASE_URL: `http://127.0.0.1:${UPSTREAM_PORT}`,
        QIP_API_TIMEOUT_MS: "2000",
        NEXT_PUBLIC_QIP_ENVIRONMENT: "test",
      },
    },
  ],
});
