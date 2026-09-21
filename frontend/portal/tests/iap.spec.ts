/**
 * Google sign-in, as this console actually does it: Identity-Aware Proxy's
 * asserted identity, verified.
 *
 * Runs only against the IAP instance (port 3317), which is configured with an
 * audience and a loopback key set served by `support/iap-stub.mjs`. The stub
 * mints real ES256 assertions against a keypair it generates at start-up, so
 * what these tests exercise is the signature check in `lib/server/iap.ts` and
 * not a stub of it.
 *
 * The refusals matter more than the happy path, and they are the tests with
 * the teeth: a console that accepts a header nobody signed is a console
 * anybody signs into with `curl -H`, and the only thing standing between those
 * two sentences is the verification this file drives.
 */
import { existsSync } from "node:fs";
import { expect, test, type APIRequestContext } from "@playwright/test";

/**
 * The audience the console is configured with. It must match
 * `ALGORIK_IAP_AUDIENCE` in `playwright.config.ts` exactly — an audience is
 * compared for equality, which is the property `another_backend_service`
 * below exists to prove.
 */
const AUDIENCE = "/projects/000000000000/global/backendServices/1111111111111111111";

/** Visibly a test account at a reserved example domain; no real address. */
const EMAIL = "console-reader@example.test";

const STUB = `http://127.0.0.1:${process.env.PLAYWRIGHT_IAP_STUB_PORT ?? 3318}`;

/**
 * The store the IAP instance is pointed at and must never create. It is
 * wiped by that server's own start command, so its absence afterwards is a
 * fact about this run and not about yesterday's.
 */
const STORE_DIR = ".algorik-test-iap";

async function assertion(
  request: APIRequestContext,
  overrides: Record<string, string> = {},
): Promise<string> {
  const query = new URLSearchParams({ aud: AUDIENCE, email: EMAIL, ...overrides });
  const response = await request.get(`${STUB}/mint?${query.toString()}`);
  expect(response.ok(), "the assertion stub did not mint a token").toBeTruthy();
  const token = await response.text();
  // The premise of every test below. A stub that answered with an error page
  // would make each refusal test pass for the wrong reason — the console would
  // be refusing garbage rather than refusing a well-formed forgery.
  expect(token.split("."), "the stub did not answer with a three-part token").toHaveLength(3);
  return token;
}

function header(token: string): Record<string, string> {
  return { "x-goog-iap-jwt-assertion": token };
}

test("a verified IAP assertion signs a person in with no cookie having been set at all", async ({
  browser,
  request,
}) => {
  const token = await assertion(request);
  const context = await browser.newContext({ extraHTTPHeaders: header(token) });
  const page = await context.newPage();

  // The premise, asserted rather than assumed: this instance requires
  // authentication, so a visitor with neither cookie nor assertion is sent to
  // the front door rather than into the console.
  const anonymous = await browser.newContext();
  const anonymousPage = await anonymous.newPage();
  await anonymousPage.goto("/");
  await expect(anonymousPage).toHaveURL(/\/welcome$/);
  await anonymous.close();

  await page.goto("/");
  await expect(page, "the assertion did not get past the navigation gate").toHaveURL(/\/$/);
  await expect(page.getByTestId("paper-trading-banner")).toBeVisible();

  // The whole point of deriving the session from the assertion rather than
  // from a record: nothing was stored, here or on the server. The portal's own
  // account store is per-instance and in-memory on Cloud Run, which is why its
  // manifest pins maxInstanceCount to one; an identity carried on the request
  // is the same identity on every instance, and this assertion is what says so.
  const cookies = await context.cookies();
  expect(
    cookies.filter((cookie) => cookie.name.includes("algorik_session")),
    "a session cookie was set, so the identity is no longer carried by the request alone",
  ).toHaveLength(0);

  // Nor on the server. `identityStore` writes its file eagerly on any
  // mutation, so the directory's absence is the evidence that signing in
  // through IAP consulted and created no per-instance record — which is the
  // half of `maxInstanceCount: 1` this change is meant to make unnecessary.
  expect(
    existsSync(STORE_DIR),
    "the console wrote a per-instance account store for an assertion-derived session",
  ).toBe(false);

  // And the session endpoint agrees, from the header alone.
  const session = await page.request.get("/api/auth/session", { headers: header(token) });
  expect(session.status()).toBe(200);
  const body = (await session.json()) as { status: string; session?: { user?: { email?: string; roles?: string[] } } };
  expect(body.status).toBe("authenticated");
  expect(body.session?.user?.email).toBe(EMAIL);
  // IAP's access list says who may reach the console. It does not say what
  // they may do, and nothing in the assertion is allowed to grant more.
  expect(body.session?.user?.roles).toEqual(["viewer"]);

  await context.close();
});

test("an assertion signed by a key Google never published is refused", async ({ request }) => {
  // Signed with a key that is not in the published set, under the published
  // key's own `kid`. A verifier that finds the key by `kid` and forgets to
  // check the signature accepts this; one that checks it does not. There is no
  // other difference between this token and the one the test above signs in
  // with.
  const forged = await assertion(request, { key: "rogue" });
  const response = await request.get("/api/auth/session", { headers: header(forged) });
  expect(response.status()).toBe(200);
  expect((await response.json()) as { status: string }).toEqual({ status: "unauthenticated" });

  // And the gateway, which is the thing that actually holds the platform's
  // credential, refuses to forward for it.
  const gateway = await request.get("/api/gateway/health", { headers: header(forged) });
  expect(gateway.status(), "a forged assertion reached the platform gateway").toBe(401);
});

test("an assertion minted for another backend service is refused", async ({ request }) => {
  // In production this is a real, currently-valid, Google-signed token: IAP
  // mints one per backend service, and anybody with access to any
  // IAP-protected resource in any project holds one. Accepting it because the
  // signature checks out is the whole reason `aud` is not optional.
  const elsewhere = await assertion(request, {
    aud: "/projects/000000000000/global/backendServices/9999999999999999999",
  });
  const response = await request.get("/api/auth/session", { headers: header(elsewhere) });
  expect((await response.json()) as { status: string }).toEqual({ status: "unauthenticated" });
});

test("an audience that merely extends the configured one is refused", async ({ request }) => {
  // Substring matching is a trap this repository has been bitten by before.
  // A backend service id is a decimal number, and `startsWith` on an audience
  // ending `1111111111111111111` is true of one ending `11111111111111111110`.
  const extended = await assertion(request, { aud: `${AUDIENCE}0` });
  const response = await request.get("/api/auth/session", { headers: header(extended) });
  expect((await response.json()) as { status: string }).toEqual({ status: "unauthenticated" });
});

test("an assertion whose header claims no algorithm is refused even though its signature is genuine", async ({
  request,
}) => {
  // `alg: none` — the oldest JWT defect there is — but over a *real* ES256
  // signature by the published key. The empty-signature form of this token
  // would be refused by any length check, so it would pass whether or not the
  // algorithm was ever looked at, and would guard nothing. This form is
  // refused by exactly one thing: a verifier that picks its algorithm from its
  // own configuration rather than from the token.
  const confused = await assertion(request, { alg: "none" });
  const response = await request.get("/api/auth/session", { headers: header(confused) });
  expect((await response.json()) as { status: string }).toEqual({ status: "unauthenticated" });
});

test("an expired assertion is refused", async ({ request }) => {
  // Well past the tolerated clock skew, so this is expiry and not rounding.
  const stale = await assertion(request, { ttl: "-3600" });
  const response = await request.get("/api/auth/session", { headers: header(stale) });
  expect((await response.json()) as { status: string }).toEqual({ status: "unauthenticated" });
});

test("the unsigned authenticated-user headers grant nothing on their own", async ({ request }) => {
  // IAP also adds `x-goog-authenticated-user-email`, unsigned. It is the
  // convenient one and it is worth exactly what the last hop is worth. A
  // console that read it would be signed into by anyone who could set a
  // header.
  const response = await request.get("/api/auth/session", {
    headers: {
      "x-goog-authenticated-user-email": `accounts.google.com:${EMAIL}`,
      "x-goog-authenticated-user-id": "accounts.google.com:000000000000000000000",
    },
  });
  expect((await response.json()) as { status: string }).toEqual({ status: "unauthenticated" });
});

test("the sign-in page reports the verified Google identity instead of asking for a password", async ({
  browser,
  request,
}) => {
  const token = await assertion(request, { hd: "example.test" });
  const context = await browser.newContext({ extraHTTPHeaders: header(token) });
  const page = await context.newPage();
  await page.goto("/sign-in");

  await expect(page.getByTestId("iap-signed-in")).toBeVisible();
  await expect(page.getByTestId("iap-email")).toHaveText(EMAIL);
  await expect(page.getByText("Workspace domain example.test")).toBeVisible();

  // There is no second credential to ask for, so there is no field to ask in.
  // A password box on this page would be the redundant sign-in this design
  // exists to not build.
  await expect(page.getByTestId("auth-password")).toHaveCount(0);

  // Posture is rendered wherever posture is shown — the auth layout's footer
  // carries it on every page of the group, including this one.
  await expect(page.locator("body")).toContainText(/paper trading only/i);

  await page.getByTestId("iap-continue").click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByTestId("paper-trading-banner")).toBeVisible();

  await context.close();
});

test("the sign-in page reached without a valid assertion says so rather than claiming Google is unconfigured", async ({
  page,
}) => {
  // This instance *is* configured for Google identity. Telling someone who
  // dialled the origin directly that it is not would send them to an operator
  // to enable something already enabled.
  await page.goto("/sign-in");
  await expect(page.getByTestId("iap-signed-in")).toHaveCount(0);
  await expect(page.getByTestId("auth-google-posture")).toContainText(
    /Identity-Aware Proxy/i,
  );
  await expect(page.getByTestId("auth-google-posture")).not.toContainText(
    /Available once Google identity is configured/i,
  );
});
