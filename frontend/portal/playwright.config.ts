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
 * A second app, in front of a real upstream, for the service-worker suite and
 * for the one that reads what the gateway put on the wire.
 *
 * Neither can use `page.route`: Playwright fulfils an intercepted request
 * before the service worker receives a `fetch` event, so a test of what the
 * worker caches would pass no matter what the worker did — and it fulfils it
 * before the gateway runs at all, so a test of what the gateway removes from a
 * body would pass no matter what the gateway did. That second blindness is not
 * hypothetical: two pages claimed no upstream address reached a browser, every
 * assertion behind the claim was a DOM assertion against a stub, and the
 * address was in the network tab the whole time. Here the request really
 * crosses a socket, through the real gateway handler, so both components get
 * to make — and be judged on — their decisions.
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
 * A fifth instance with ALGORIK_AUTH_REQUIRED deliberately UNSET, for the
 * gate's default. The instances above say which posture they want in so many
 * words; this one says nothing, which is what a deployment that forgot the
 * variable says, and the suite against it proves that silence is the closed
 * gate. It cannot share the auth instance: that one writes `"true"`, and a
 * test of the default that runs where the default is overridden proves the
 * override.
 */
const GATE_PORT = Number(process.env.PLAYWRIGHT_GATE_PORT ?? 3315);
const GATE_BASE_URL = `http://127.0.0.1:${GATE_PORT}`;

/**
 * A sixth instance in the posture a deployment is in when it names an identity
 * project: `ALGORIK_IDENTITY_PROJECT_ID` set, so `developmentProviderActive()`
 * is false and every credential question goes to Google.
 *
 * It is deliberately given no API key, which is the one way to exercise that
 * branch without a credential, a project or a byte leaving the process — the
 * provider refuses before it would call out. What the suite against it proves
 * is the property that has no other test: a console pointed at Identity
 * Platform **refuses** rather than falling back to the development store. That
 * fallback is the failure this instance exists to make impossible to
 * introduce; nothing else in the suite would notice it, because every other
 * instance is in development mode where the local store is the right answer.
 *
 * It cannot share the auth instance: that one has no project configured, which
 * is the opposite posture, and a test of the platform branch that runs on the
 * development branch proves the development branch.
 */
const IDENTITY_PORT = Number(process.env.PLAYWRIGHT_IDENTITY_PORT ?? 3316);
const IDENTITY_BASE_URL = `http://127.0.0.1:${IDENTITY_PORT}`;

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
      // worker.spec and wire.spec need the instance with a real upstream
      // behind it, auth.spec needs the instance with authentication required,
      // and gate.spec needs the one where nothing was said about it; running
      // any of them here would test the wrong server — this one is pointed at
      // a dead port on purpose.
      testIgnore: /(worker|wire|auth|gate|identity-provider)\.spec\.ts/,
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
      // The same instance, because what this suite needs is exactly what that
      // one needs: a real gateway with a real upstream behind it. Separate
      // project rather than a second server, since a second server would be a
      // second thing to keep in step with the first.
      name: "wire-chromium",
      use: { ...devices["Desktop Chrome"], baseURL: WORKER_BASE_URL },
      testMatch: /wire\.spec\.ts/,
    },
    {
      name: "auth-chromium",
      use: { ...devices["Desktop Chrome"], baseURL: AUTH_BASE_URL, viewport: { width: 1280, height: 900 } },
      testMatch: /auth\.spec\.ts/,
    },
    {
      name: "gate-chromium",
      use: { ...devices["Desktop Chrome"], baseURL: GATE_BASE_URL },
      testMatch: /gate\.spec\.ts/,
    },
    {
      name: "identity-chromium",
      use: { ...devices["Desktop Chrome"], baseURL: IDENTITY_BASE_URL, viewport: { width: 1280, height: 900 } },
      testMatch: /identity-provider\.spec\.ts/,
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
        // The explicit kiosk opt-out. These suites predate accounts and
        // exercise the console anonymously; the gate is closed unless a
        // deployment writes this, and this is a deployment writing it.
        ALGORIK_AUTH_REQUIRED: "false",
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
        // The worker suite reads the gateway anonymously through a real
        // socket; same explicit opt-out as the first instance.
        ALGORIK_AUTH_REQUIRED: "false",
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
    {
      // The store directory is named and wiped like the auth instance's, and
      // the suite asserts it is never created: in this posture the console
      // must not touch the development store at all. See IDENTITY_PORT.
      command: `rm -rf .algorik-test-identity-provider && npm run start -- --port ${IDENTITY_PORT} --hostname 127.0.0.1`,
      url: `${IDENTITY_BASE_URL}/sign-in`,
      reuseExistingServer: false,
      timeout: 120_000,
      stdout: "ignore",
      stderr: "pipe",
      env: {
        QIP_API_BASE_URL: "http://127.0.0.1:9",
        QIP_API_TIMEOUT_MS: "1500",
        NEXT_PUBLIC_QIP_ENVIRONMENT: "test",
        ALGORIK_AUTH_REQUIRED: "true",
        // A project name, not a project: no call is ever made with it, because
        // the missing API key refuses first. Visibly a test value.
        ALGORIK_IDENTITY_PROJECT_ID: "algorik-playwright-no-such-project",
        ALGORIK_IDENTITY_STORE_DIR: ".algorik-test-identity-provider",
        ALGORIK_SESSION_SECRET: TEST_SESSION_SECRET,
        ALGORIK_COOKIE_SECURE: "false",
      },
    },
    {
      // ALGORIK_AUTH_REQUIRED is not in this env on purpose. See GATE_PORT.
      command: `npm run start -- --port ${GATE_PORT} --hostname 127.0.0.1`,
      url: `${GATE_BASE_URL}/welcome`,
      reuseExistingServer: !process.env.CI,
      timeout: 120_000,
      stdout: "ignore",
      stderr: "pipe",
      env: {
        QIP_API_BASE_URL: "http://127.0.0.1:9",
        QIP_API_TIMEOUT_MS: "1500",
        NEXT_PUBLIC_QIP_ENVIRONMENT: "test",
        ALGORIK_SESSION_SECRET: TEST_SESSION_SECRET,
        ALGORIK_COOKIE_SECURE: "false",
      },
    },
  ],
});
