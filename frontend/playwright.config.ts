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

/**
 * A fourth app instance with authentication REQUIRED, for the identity
 * journey. The other instances run open because their suites predate
 * accounts and exercise the console directly; this one exists to prove the
 * gate itself — sign-up through sign-out — against the development identity
 * provider, with its own throwaway store wiped on every start.
 */
const AUTH_PORT = Number(process.env.PLAYWRIGHT_AUTH_PORT ?? 3314);
const AUTH_BASE_URL = `http://127.0.0.1:${AUTH_PORT}`;

/**
 * `next start` runs as NODE_ENV=production, where the session signer refuses
 * to invent a key (replicas could not verify each other's cookies). The test
 * key is set here, visibly a test value, and long enough to pass the length
 * check.
 */
const TEST_SESSION_SECRET = "playwright-test-signing-key-not-production-0000";

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
      // worker.spec needs the instance with a real upstream behind it, and
      // auth.spec needs the instance with authentication required; running
      // either here would test the wrong server.
      testIgnore: /(worker|auth)\.spec\.ts/,
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
    {
      name: "auth-chromium",
      use: { ...devices["Desktop Chrome"], baseURL: AUTH_BASE_URL, viewport: { width: 1280, height: 900 } },
      testMatch: /auth\.spec\.ts/,
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
        ALGORIK_SESSION_SECRET: TEST_SESSION_SECRET,
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
        ALGORIK_SESSION_SECRET: TEST_SESSION_SECRET,
      },
    },
    {
      // The identity store is wiped before start so every run begins from
      // zero users — a journey test against yesterday's store is a test whose
      // premise depends on which tests ran yesterday.
      command: `rm -rf .algorik-test-identity && npm run start -- --port ${AUTH_PORT} --hostname 127.0.0.1`,
      url: AUTH_BASE_URL,
      reuseExistingServer: false,
      timeout: 120_000,
      stdout: "ignore",
      stderr: "pipe",
      env: {
        QIP_API_BASE_URL: "http://127.0.0.1:9",
        QIP_API_TIMEOUT_MS: "1500",
        NEXT_PUBLIC_QIP_ENVIRONMENT: "test",
        ALGORIK_AUTH_REQUIRED: "true",
        ALGORIK_IDENTITY_STORE_DIR: ".algorik-test-identity",
        ALGORIK_SESSION_SECRET: TEST_SESSION_SECRET,
        // Playwright serves plain HTTP on 127.0.0.1, where Chromium refuses
        // to store a Secure cookie at all. This is the one explicit downgrade
        // — production defaults to the strict __Host- form.
        ALGORIK_COOKIE_SECURE: "false",
      },
    },
  ],
});
