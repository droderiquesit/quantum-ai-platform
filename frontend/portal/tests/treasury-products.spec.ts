/**
 * `/treasury/products`: the product catalogue and every account's entitlement
 * against it, read from `GET /ledger/users` and rendered as answered.
 *
 * The failures these tests prevent:
 *
 * * an empty catalogue and an unreachable platform rendering the same — the
 *   mutation that caught this class in this repository was on the ledger's
 *   sibling surface, and the confusion it produces is the worst kind: a desk
 *   reading "no products" during an outage concludes the factory registered
 *   none, and stops looking for the outage;
 * * a capability decided in the browser — asserted by the refusal *reason*
 *   appearing verbatim, because a page that computed "refused" for itself
 *   would have no platform sentence to show;
 * * a withdrawal shown as anything but refused, or a control that could ask
 *   for one — asserted on the platform's own reason and on the absence of any
 *   form, submit control or non-GET request;
 * * a missing evaluation read as a refusal — asserted by the absence block,
 *   because "nobody decided" and "decided no" are different facts and only one
 *   of them is an answer.
 *
 * The bodies are the example in `backend/crates/apps/qip-api/ROUTES-LEDGER.md`,
 * the contract the route is built to. They are not captured from a running
 * process: no deployment has enrolled a user against a registered product.
 */
import { expect, test, type Page } from "@playwright/test";
import { GATEWAY, healthy, servePlatform, servePlatformUnreachable } from "./support/platform";

const WITHDRAWAL_REFUSED =
  "capital does not leave the platform: ADR 0021 refuses the signing and withdrawal half of the treasury and ADR 0023 keeps that in force; a withdrawal is a separate, later, separately approved decision";

const INVEST_REFUSED = "alice holds the viewer role, which does not invest";

const ALICE = {
  user_id: "alice",
  mandate: {
    capital: "1000",
    currency: "USD",
    risk_tolerance: "1",
    liquidity_floor: "0",
    investable: "1000",
    exploration_share: "0",
    jurisdiction: "GB",
    permitted_families: { any: true, families: [] },
  },
  eligibility: {
    eligible: true,
    verified_at: "2025-10-08T08:00:00Z",
    can_invest: true,
    jurisdiction: "GB",
    expires_at: "2026-10-08T08:00:00Z",
    refused: null,
    reason: null,
  },
  balances: [],
  entitlements: [
    {
      family: "research-tests",
      role: "viewer",
      evaluated_at: "2025-10-09T08:53:20Z",
      can_view: { granted: true, reason: "alice holds a mandate in GB" },
      can_invest: { granted: false, reason: INVEST_REFUSED },
      can_withdraw: { granted: false, reason: WITHDRAWAL_REFUSED },
    },
  ],
  entitlements_note: null,
} as const;

const LEDGER_USERS = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  evaluated_as_role: "viewer",
  products: ["research-tests"],
  fills_journalled: 2,
  users: [ALICE],
} as const;

/**
 * A platform that refuses the console's credential on one route.
 *
 * `/ledger/users` requires the analyst role and the other treasury routes do
 * not, so a deployment whose credential is a viewer reads three of the four and
 * is refused this one. The header matters: `client.ts` reads `x-qip-gateway`
 * before the status, and a 403 without it would be reported as the gateway's
 * own refusal rather than the platform's — two faults with two remedies.
 */
async function serveDenied(page: Page, path: string, bodies: Record<string, unknown>) {
  await page.route(GATEWAY, async (route) => {
    const url = new URL(route.request().url()).pathname.replace("/api/gateway", "");
    if (url === path) {
      await route.fulfill({
        status: 403,
        headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
        body: JSON.stringify({ error: "this route requires the analyst role" }),
      });
      return;
    }
    const key = Object.keys(bodies).find((k) => url === k || url.endsWith(k));
    await route.fulfill({
      status: 200,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify(
        key === undefined
          ? { subject: url.replace(/^\//, ""), available: false, reason: `no stub for ${url}` }
          : bodies[key],
      ),
    });
  });
}

test("the products page renders every account's entitlement against a product with the platform's own reasons, and holds no control that could ask to withdraw", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/ledger/users": LEDGER_USERS });
  await page.goto("/treasury/products");

  // The premise: the page rendered and the route's answer landed on it. Without
  // this, every absence below holds for a page that never rendered.
  await expect(page.getByRole("heading", { name: "Product entitlements" })).toBeVisible();
  await expect(page.getByTestId("product-count")).toHaveText("1");
  await expect(page.getByTestId("product-card")).toHaveCount(1);
  await expect(page.getByTestId("product-family")).toHaveText("research-tests");
  await expect(page.getByTestId("product-entitlement-row")).toHaveCount(1);

  // The declaration, on the page and not only in the chrome.
  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("treasury-body-posture")).toHaveText("PAPER TRADING");

  // The capabilities, with the platform's sentences verbatim. A page that
  // decided "refused" for itself would have no such sentence to render.
  const row = page.getByTestId("product-entitlement-row");
  await expect(row).toContainText("alice");
  await expect(row).toContainText("alice holds a mandate in GB");
  await expect(row).toContainText(INVEST_REFUSED);
  await expect(page.getByTestId("product-invest-grants-research-tests")).toHaveText("0 of 1 may invest");

  // Withdrawal: refused, in the platform's words, with no granted arm shown
  // and no control that could ask for one.
  const withdrawal = page.getByTestId("withdrawal-entitlement");
  await expect(withdrawal).toHaveCount(1);
  await expect(withdrawal).toContainText("refused");
  await expect(withdrawal).toContainText(WITHDRAWAL_REFUSED);
  await expect(withdrawal).not.toContainText("GRANTED");
  await expect(page.getByTestId("product-withdrawal-grants")).toHaveText("0");

  const content = page.locator("#content");
  await expect(content.locator("button[type=submit], form")).toHaveCount(0);
  await expect(
    content.getByRole("button", { name: /^(propose|approve|sign|transfer|withdraw|invest|submit)/i }),
  ).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("a catalogue the platform answered as empty does not read like a platform that could not be reached", async ({
  page,
}) => {
  // The premise for the contrast: both halves must be observed in one run, or
  // this asserts that two things differ without having seen either.
  await servePlatform(page, {
    ...healthy(),
    "/ledger/users": {
      ...LEDGER_USERS,
      products: [],
      users: [{ ...ALICE, entitlements: [], entitlements_note: "no product to evaluate against" }],
    },
  });
  await page.goto("/treasury/products");

  const empty = page.getByTestId("product-catalogue-empty");
  await expect(empty).toBeVisible();
  await expect(empty).toContainText("The platform has registered no product.");
  await expect(empty).toContainText("a catalogue that was read and found empty");
  await expect(empty.locator("[data-state-block=empty]")).toHaveCount(1);
  await expect(page.getByTestId("product-count")).toHaveText("0");
  await expect(page.getByTestId("product-card")).toHaveCount(0);
  // The empty catalogue never claims the platform was unreachable.
  await expect(page.locator("#content")).not.toContainText("The platform could not be reached.");

  // The other half of the contrast, in the same test, against the same page.
  await servePlatformUnreachable(page);
  await page.goto("/treasury/products");
  const unreachable = page.locator("[data-state-block=disconnected]").first();
  await expect(unreachable).toBeVisible();
  await expect(unreachable).toContainText("The platform could not be reached.");
  await expect(page.getByTestId("product-catalogue-empty")).toHaveCount(0);
  await expect(page.locator("#content")).not.toContainText("The platform has registered no product.");
});

test("a credential the route refuses says the credential was refused and does not render an empty catalogue", async ({
  page,
}) => {
  await serveDenied(page, "/ledger/users", healthy());
  await page.goto("/treasury/products");

  const denied = page.locator("[data-state-block=refused]").first();
  await expect(denied).toBeVisible();
  await expect(denied).toContainText("may not read /api/v1/ledger/users");
  await expect(denied).toContainText("this route requires the analyst role");
  // Neither of the other two absences: an insufficient role is not an empty
  // catalogue and is not an outage.
  await expect(page.getByTestId("product-catalogue-empty")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  // The declaration survives a refusal, because that is when an operator most
  // needs to know what this console can do.
  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
});

test("an account the platform did not evaluate against a listed product is shown as an absence and never as a refusal", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    "/ledger/users": {
      ...LEDGER_USERS,
      products: ["research-tests", "unevaluated-family"],
    },
  });
  await page.goto("/treasury/products");

  // The premise: two products listed, and alice carries an evaluation for one
  // of them. Without this the absence below could be an empty page.
  await expect(page.getByTestId("product-count")).toHaveText("2");
  await expect(page.getByTestId("product-card")).toHaveCount(2);

  const absent = page.getByTestId("product-entitlement-absent");
  await expect(absent).toHaveCount(1);
  await expect(absent).toContainText("answered no entitlement for this account against this product");
  await expect(absent).toContainText("not a refusal, which nobody decided");
  // The product with no evaluation reports no grant and no refusal chip.
  const unevaluated = page.locator('[data-testid=product-card][data-product=unevaluated-family]');
  await expect(unevaluated.getByTestId("withdrawal-entitlement")).toHaveCount(0);
  await expect(page.getByTestId("product-invest-grants-unevaluated-family")).toHaveText("0 of 1 may invest");
});
