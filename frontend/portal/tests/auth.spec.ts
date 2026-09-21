/**
 * The authentication vertical slice, end to end, and the refusals that make
 * it worth having.
 *
 * Runs only against the auth-required instance (port 3314), whose identity
 * store is wiped at server start. The store starts empty, so the journey test
 * creates the very user it signs in — the premise is established by the test
 * itself rather than assumed of the environment. It is wiped at server start
 * and not between attempts, which is why the address is per-attempt; see
 * `emailForAttempt`.
 *
 * The negative tests matter more than the happy path. A sign-in that works is
 * table stakes; a forged cookie that reads the book anyway is the incident.
 */
import { expect, test, type TestInfo } from "@playwright/test";

const PASSWORD = "a-long-development-passphrase";

/** The one mutable thing shared across serial tests: the account exists. */
test.describe.configure({ mode: "serial" });

/**
 * The account this attempt enrols, keyed on the attempt number.
 *
 * **Why this is not a constant, which it was.** The group is serial and CI
 * retries once, so a failure anywhere re-runs every test here from the top —
 * but the identity store is wiped at *server* start, not per attempt, so the
 * account the first attempt created is still there. A fixed address therefore
 * made the retry submit a sign-up for an account that already existed: a
 * different scenario from the one it meant to repeat, testing the duplicate
 * enrolment path while reporting the journey's name. Either outcome was
 * wrong to read — a green retry said the journey works when the journey had
 * not been run, and a red one blamed sign-up for a collision the harness
 * caused.
 *
 * Keying on `testInfo.retry` keeps the premise the file's header claims:
 * every attempt enrols an address no attempt has used, so the journey test
 * still creates the very account it signs in and still proves it against an
 * empty-for-this-address store. Playwright retries a serial group as a unit
 * and gives every test in it the same `retry`, which is what lets the tests
 * below share the address the journey test enrolled.
 *
 * Nothing here is a real address or a real credential: `algorik.test` is a
 * reserved test domain and the passphrase is the development provider's.
 */
function emailForAttempt(testInfo: TestInfo): string {
  return `journey-attempt-${testInfo.retry}@algorik.test`;
}

test("a signed-out visitor at the root lands on the front door, not a login wall", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page).toHaveURL(/\/welcome$/);
  // The landing page is public and says what the platform is.
  await expect(page.locator("body")).toContainText(/paper[- ]trading/i);
});

test("the whole journey: sign up, verify, sign in, use the portal, sign out", async ({
  page,
}, testInfo) => {
  const EMAIL = emailForAttempt(testInfo);
  // Sign up.
  await page.goto("/sign-up");
  await page.getByTestId("auth-accounttype").selectOption("individual");
  await page.getByTestId("auth-email").fill(EMAIL);
  await page.getByTestId("auth-password").fill(PASSWORD);
  await page.getByTestId("auth-password-confirm").fill(PASSWORD);
  await page.getByTestId("auth-terms").check();
  await page.getByTestId("auth-privacy").check();
  await page.getByTestId("auth-risk").check();
  await page.getByTestId("auth-submit").click();

  // Verification, with the development code shown on screen and labelled.
  await expect(page).toHaveURL(/\/verify-email/);
  const codeBlock = page.getByTestId("dev-code");
  await expect(codeBlock, "no development code was surfaced").toBeVisible();
  await expect(page.getByText("DEVELOPMENT IDENTITY")).toBeVisible();
  const code = (await codeBlock.innerText()).replace(/\D/g, "").slice(-6);
  expect(code, "the dev block did not contain a six-digit code").toHaveLength(6);
  await page.getByTestId("auth-code").fill(code);
  await page.getByTestId("auth-submit").click();

  // Sign in.
  await expect(page).toHaveURL(/\/sign-in/);
  await page.getByTestId("auth-email").fill(EMAIL);
  await page.getByTestId("auth-password").fill(PASSWORD);
  await page.getByTestId("auth-submit").click();

  // The portal, with its chrome and its declaration.
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByTestId("paper-trading-banner")).toBeVisible();
  await expect(page.getByTestId("account-menu")).toBeVisible();

  // The gateway serves a signed-in browser. The upstream here is a dead port,
  // so the honest expectation is "not 401": unauthenticated is this slice's
  // refusal, unreachable is the environment's.
  const throughGateway = await page.request.get("/api/gateway/health");
  expect(throughGateway.status(), "the gateway refused a signed-in session").not.toBe(401);

  // Sign out, and prove the server no longer honours the old session.
  await page.getByTestId("account-menu").click();
  await page.getByTestId("sign-out").click();
  await expect(page).toHaveURL(/\/welcome/);
  const afterSignOut = await page.request.get("/api/gateway/health");
  expect(afterSignOut.status(), "a revoked session still reads the platform").toBe(401);
});

test("the gateway refuses a browser with no session at all", async ({ page }) => {
  const response = await page.request.get("/api/gateway/portfolio");
  expect(response.status()).toBe(401);
});

test("a tampered session cookie reads nothing", async ({ page, context }, testInfo) => {
  const EMAIL = emailForAttempt(testInfo);
  // Sign in for real first, so the premise — a working cookie — is proven
  // before one byte of it is changed.
  await page.goto("/sign-in");
  await page.getByTestId("auth-email").fill(EMAIL);
  await page.getByTestId("auth-password").fill(PASSWORD);
  await page.getByTestId("auth-submit").click();
  await expect(page).toHaveURL(/\/$/);

  const cookies = await context.cookies();
  const session = cookies.find((cookie) => cookie.name.includes("algorik_session"));
  expect(session, "no session cookie was set by sign-in").toBeTruthy();

  // Flip one character of the signed value. If the gateway accepts this, the
  // signature check is decorative.
  const value = session!.value;
  const tampered = value.slice(0, -2) + (value.endsWith("A") ? "B" : "A") + value.slice(-1);
  await context.clearCookies();
  await context.addCookies([{ ...session!, value: tampered }]);

  const response = await page.request.get("/api/gateway/health");
  expect(response.status(), "a tampered cookie was honoured").toBe(401);
});

test("a wrong password is refused with words, and account existence is not revealed", async ({
  page,
}, testInfo) => {
  const EMAIL = emailForAttempt(testInfo);
  await page.goto("/sign-in");
  await page.getByTestId("auth-email").fill(EMAIL);
  await page.getByTestId("auth-password").fill("not-the-password-at-all");
  await page.getByTestId("auth-submit").click();
  const error = page.getByTestId("auth-error");
  await expect(error).toBeVisible();
  const text = (await error.innerText()).toLowerCase();
  // The same wording must cover "no such account": neither word may appear.
  expect(text).not.toContain("exist");
  expect(text).not.toContain("found");
});

test("a mutating call without the CSRF header is refused", async ({ page }, testInfo) => {
  const EMAIL = emailForAttempt(testInfo);
  // The cookie half of the pair is present (page load set it); the header
  // half is deliberately absent — which is exactly what a cross-site form
  // submission looks like.
  await page.goto("/sign-in");
  const response = await page.request.post("/api/auth/sign-in", {
    data: { email: EMAIL, password: PASSWORD },
  });
  expect(response.status()).toBe(403);
});

test("the next parameter cannot send a signed-in user off-origin", async ({ page }, testInfo) => {
  const EMAIL = emailForAttempt(testInfo);
  await page.goto("/sign-in?next=https%3A%2F%2Fevil.example%2Fphish");
  await page.getByTestId("auth-email").fill(EMAIL);
  await page.getByTestId("auth-password").fill(PASSWORD);
  await page.getByTestId("auth-submit").click();
  // safeRedirect refuses an absolute URL and falls back to the dashboard.
  await expect(page).toHaveURL(/127\.0\.0\.1:\d+\/$/);
});

test("a protected page visited signed-out redirects to sign-in and preserves intent", async ({
  page,
}) => {
  await page.goto("/risk");
  await expect(page).toHaveURL(/\/sign-in\?next=%2Frisk/);
});
