/**
 * The session gate's default, proven on the instance that never set it.
 *
 * `ALGORIK_AUTH_REQUIRED` is absent from this server's environment on purpose
 * (see `playwright.config.ts`, GATE_PORT). The gate used to be off unless the
 * variable was `"true"`, so a deployment that forgot it — or lost it in a
 * template, or misspelled it — served the whole console anonymously with the
 * platform's own bearer token attached to every request. That is the failure
 * these tests exist to make loud: silence must be the closed gate.
 *
 * Both gateways are asserted, because they used to disagree. The REST gateway
 * had a gate the flag could switch on; the SSE gateway had none, and
 * `/api/stream/orders` is `/api/gateway/orders` over time.
 */
import { expect, test } from "@playwright/test";

test("with the variable unset, the REST gateway refuses an anonymous caller before the platform is asked", async ({
  request,
}) => {
  const response = await request.get("/api/gateway/health");
  expect(response.status(), "an anonymous request read the platform through the gateway").toBe(401);
  // The refusal is this console's own, and says so: a 401 from the platform
  // would carry `www-authenticate: Bearer` and no `gateway` field, and would
  // mean the credential had been sent. Here nothing was.
  const body = (await response.json()) as { gateway?: string; error?: string };
  expect(body.gateway).toBe("unauthenticated");
  expect(response.headers()["www-authenticate"]).toBeUndefined();
});

test("with the variable unset, the SSE gateway refuses an anonymous caller too", async ({
  request,
}) => {
  const response = await request.get("/api/stream/health", {
    headers: { accept: "text/event-stream" },
  });
  expect(response.status(), "an anonymous request opened a platform stream").toBe(401);
  expect(response.headers()["content-type"]).not.toContain("text/event-stream");
  const body = (await response.json()) as { gateway?: string };
  expect(body.gateway).toBe("unauthenticated");
});

test("with the variable unset, a signed-out browser is sent to sign in, not to the console", async ({
  page,
}) => {
  await page.goto("/risk");
  await expect(page).toHaveURL(/\/sign-in\?next=%2Frisk$/);
  // Premise for the redirect: it landed on a real sign-in form, not an error.
  await expect(page.getByTestId("auth-email")).toBeVisible();
  // The paper-trading declaration is on the auth group too, and this is the
  // screen a locked-out person photographs.
  await expect(page.locator("body")).toContainText(/paper trading/i);
});

test("with the variable unset, the bare root goes to the front door", async ({ page }) => {
  await page.goto("/");
  await expect(page).toHaveURL(/\/welcome$/);
});
