/**
 * The console in the posture a deployment is in: an identity project named,
 * so Google answers the credential question and the local development store
 * must not.
 *
 * Runs only against the instance on port 3316, which has
 * `ALGORIK_IDENTITY_PROJECT_ID` set and no `ALGORIK_IDENTITY_API_KEY`. That
 * combination is the one way to drive the Identity Platform branch of
 * `src/lib/server/identity.ts` from a test: it is refused before any call
 * leaves the process, so the suite needs no project, no credential and no
 * network, and it still runs the real route handler on the real branch.
 *
 * What is proven here, and what is not. Proven: a console pointed at Identity
 * Platform refuses when it cannot reach it, says which variable is missing,
 * issues no session, and never falls back to the development store. **Not
 * proven: that a real Identity Platform sign-in works.** Nothing in this
 * repository can prove that — it needs a project, a key and a deployment, and
 * `grep -rn ALGORIK_IDENTITY_PROJECT_ID infrastructure/` finds the variable
 * only in a Terraform output that no Cloud Run service consumes. Read this
 * file as the refusal half, which is the half that can go silently wrong.
 *
 * The fallback is the defect this file exists to prevent. A console that
 * answered a Google outage by checking a password against a JSON file on one
 * instance's disk would authenticate a stranger with a credential the desk
 * never issued, and every other suite here runs in development mode, where
 * consulting that store is the correct behaviour and would notice nothing.
 */
import { existsSync } from "node:fs";
import { expect, test } from "@playwright/test";

/** Matches `ALGORIK_IDENTITY_STORE_DIR` for this instance in playwright.config.ts. */
const DEV_STORE_DIR = ".algorik-test-identity-provider";

const EMAIL = "provider-posture@algorik.test";
const PASSWORD = "a-long-development-passphrase";

test("with a project named and no key, sign-in is refused as unavailable and the credential is never judged", async ({
  page,
}) => {
  await page.goto("/sign-in");
  // The CSRF pair, primed the way the pages prime it.
  const primed = await page.request.get("/api/auth/csrf");
  const token = ((await primed.json()) as { token?: string }).token;
  expect(token, "the csrf endpoint issued no token, so nothing below is a test of sign-in").toBeTruthy();

  const response = await page.request.post("/api/auth/sign-in", {
    headers: { "x-algorik-csrf": token! },
    data: { email: EMAIL, password: PASSWORD },
  });

  // 503 and not 401: nothing about the password was checked, and a 401 would
  // tell the person their credential was wrong when it was never read.
  // 503 and not 500: this was the shape before the guard existed — a thrown
  // Error crossing the route handler, which the console's own client cannot
  // parse and reports as the console being broken rather than the deployment
  // being half-configured.
  expect(response.status(), "the refusal is not the deployment-level one").toBe(503);

  const body = (await response.json()) as { ok: boolean; failure: { code: string; message: string } };
  expect(body.ok).toBe(false);
  expect(body.failure.code).toBe("provider_unavailable");
  // The variable name is the actionable part. A name is not a secret; a value
  // would be, and none appears.
  expect(body.failure.message).toContain("ALGORIK_IDENTITY_API_KEY");
  expect(body.failure.message).not.toContain("algorik-playwright-no-such-project");

  const cookies = await page.context().cookies();
  expect(
    cookies.find((cookie) => cookie.name.includes("algorik_session")),
    "a refused sign-in still issued a session",
  ).toBeFalsy();
});

test("the refusal reaches the person at the form, and the console stays shut", async ({ page }) => {
  await page.goto("/sign-in");
  await page.getByTestId("auth-email").fill(EMAIL);
  await page.getByTestId("auth-password").fill(PASSWORD);
  await page.getByTestId("auth-submit").click();

  const error = page.getByTestId("auth-error");
  await expect(error, "the form reported nothing").toBeVisible();
  const text = await error.innerText();
  expect(text).toContain("ALGORIK_IDENTITY_API_KEY");
  // The sentence must not read as the person's mistake, and must not read as
  // the console guessing: it says nobody was signed in.
  expect(text.toLowerCase()).toContain("nobody was");

  await expect(page, "a refused sign-in moved the browser into the console").toHaveURL(/\/sign-in/);
  const throughGateway = await page.request.get("/api/gateway/health");
  expect(throughGateway.status(), "the gateway served a browser that never signed in").toBe(401);
});

test("sign-up in this posture opens no account, offers no development code, and writes no development store", async ({
  page,
}) => {
  // Premise: the store this test is about does not exist yet. The server's
  // command wipes it at start, so a stale directory would make the assertion
  // below pass for the wrong reason.
  expect(existsSync(DEV_STORE_DIR), "the development store existed before the attempt").toBe(false);

  await page.goto("/sign-up");
  await page.getByTestId("auth-accounttype").selectOption("individual");
  await page.getByTestId("auth-email").fill(EMAIL);
  await page.getByTestId("auth-password").fill(PASSWORD);
  await page.getByTestId("auth-password-confirm").fill(PASSWORD);
  await page.getByTestId("auth-terms").check();
  await page.getByTestId("auth-privacy").check();
  await page.getByTestId("auth-risk").check();
  await page.getByTestId("auth-submit").click();

  // Premise for everything after it: the attempt was made and answered.
  await expect(page.getByTestId("auth-error"), "sign-up reported nothing at all").toBeVisible();

  // The development provider's two tells. Either one on this instance means
  // the console answered a configured project with the offline store.
  await expect(page).toHaveURL(/\/sign-up/);
  await expect(page.getByTestId("dev-code")).toHaveCount(0);
  await expect(page.getByText("DEVELOPMENT IDENTITY")).toHaveCount(0);

  // The store is written eagerly by `identityStore.createUser`, so its absence
  // is evidence no account was opened rather than an inference from the screen.
  expect(existsSync(DEV_STORE_DIR), "a configured deployment wrote to the development store").toBe(false);
});
