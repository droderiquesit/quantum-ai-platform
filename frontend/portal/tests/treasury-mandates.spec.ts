/**
 * `/treasury/mandates`: the mandate register — every agreement the ledger
 * holds, term by term, read from `GET /ledger/users`.
 *
 * The failures these tests prevent:
 *
 * * a register that computes. Money on this surface is the platform's exact
 *   `Decimal` text and this console does no arithmetic on it: `investable` is
 *   the figure the route answered, and there is no total row. A page that
 *   summed two mandates would be publishing a number the platform never
 *   computed. Asserted by the investable cell carrying the platform's figure
 *   even when it disagrees with capital − floor, and by the sum of the two
 *   capitals appearing nowhere;
 * * an empty register and an unreachable platform rendering alike. Those are
 *   opposite facts with opposite remedies — nobody enrolled a mandate, versus
 *   nobody could ask — and the confusion has been caught by a mutation on this
 *   surface before;
 * * a credential the route refuses being shown as an empty ledger. The route
 *   requires the `analyst` role and the portal grants `viewer` on
 *   self-registration, so this is the state a signed-up reader actually meets;
 * * the three fields the platform holds about a mandate and this route does not
 *   carry — the mandate id, the instant it was registered, and the desk ceiling
 *   it was admitted under — being filled in, or silently omitted. Filled in is
 *   an invention; omitted is indistinguishable from the platform not knowing;
 * * a control. No route creates, amends or retires a mandate, so a page with an
 *   edit control would imply a path that does not exist.
 *
 * Bodies follow the contract in `backend/crates/apps/qip-api/ROUTES-LEDGER.md`.
 */
import { expect, test, type Page } from "@playwright/test";
import { GATEWAY, healthy, servePlatform, servePlatformUnreachable } from "./support/platform";

const ALICE = {
  user_id: "alice",
  mandate: {
    capital: "1000",
    currency: "USD",
    // Deliberately not capital − liquidity_floor. The platform computes
    // `investable` and this page renders what it computed; a console doing the
    // subtraction itself would show 750 here and the assertion below would
    // catch it.
    investable: "700",
    risk_tolerance: "0.5",
    liquidity_floor: "250",
    exploration_share: "0.05",
    jurisdiction: "GB",
    permitted_families: { any: false, families: ["research-tests"] },
  },
  eligibility: {
    eligible: false,
    verified_at: null,
    can_invest: null,
    jurisdiction: null,
    expires_at: null,
    refused: "not_yet_verified",
    reason: "alice is not eligible (not_yet_verified): an operator must record a verification",
  },
  balances: [],
  entitlements: [],
  entitlements_note: "no product to evaluate against",
} as const;

const DESK = {
  ...ALICE,
  user_id: "desk",
  mandate: {
    ...ALICE.mandate,
    capital: "10000000",
    investable: "10000000",
    liquidity_floor: "0",
    risk_tolerance: "1",
    exploration_share: "0",
    jurisdiction: "ZZ",
    permitted_families: { any: true, families: [] },
  },
} as const;

const LEDGER_USERS = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  evaluated_as_role: "viewer",
  products: ["research-tests"],
  fills_journalled: 2,
  users: [ALICE, DESK],
} as const;

/** A platform that refuses the console's credential on one route. */
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

test("the register lists every mandate the ledger holds with the platform's own figures, and derives no money of its own", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/ledger/users": LEDGER_USERS });
  await page.goto("/treasury/mandates");

  // The premise: the page rendered and the register has both mandates in it.
  await expect(page.getByRole("heading", { name: "Mandates", exact: true })).toBeVisible();
  await expect(page.getByTestId("mandate-row")).toHaveCount(2);
  await expect(page.getByTestId("mandate-count")).toHaveText("2");

  // The declaration, on the page and not only in the chrome.
  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("treasury-body-posture")).toHaveText("PAPER TRADING");

  const aliceRow = page.locator('[data-testid="mandate-row"][data-user="alice"]');
  await expect(aliceRow).toHaveCount(1);
  // `formatDecimal` groups thousands and never rounds, so the platform's exact
  // decimal text is what is grouped.
  await expect(aliceRow.getByTestId("mandate-capital")).toHaveText("1,000");
  await expect(aliceRow.getByTestId("mandate-floor")).toHaveText("250");
  // The platform said 700. 750 is what a console subtracting for itself would
  // show, which is the whole point of rendering the platform's field.
  await expect(aliceRow.getByTestId("mandate-investable")).toHaveText("700");
  await expect(aliceRow.getByTestId("mandate-jurisdiction")).toHaveText("GB");
  await expect(aliceRow.getByTestId("mandate-families")).toHaveText("research-tests");

  const deskRow = page.locator('[data-testid="mandate-row"][data-user="desk"]');
  await expect(deskRow.getByTestId("mandate-capital")).toHaveText("10,000,000");
  await expect(deskRow.getByTestId("mandate-families")).toHaveText("any family");
  await expect(deskRow.getByTestId("mandate-jurisdiction")).toHaveText("ZZ");

  // Two jurisdictions, counted from the answer rather than assumed.
  await expect(page.getByTestId("mandate-jurisdictions")).toHaveText("2");
  await expect(page.getByTestId("mandate-products")).toHaveText("1");

  // No total. 10,001,000 is the sum of the two capitals and appears nowhere,
  // because this console does not add money the platform sent as exact text.
  await expect(page.locator("#content")).not.toContainText("10,001,000");

  // No control on the page, and nothing it could have posted.
  const content = page.locator("#content");
  await expect(content.locator("button[type=submit], form")).toHaveCount(0);
  await expect(
    content.getByRole("button", {
      name: /^(edit|amend|retire|propose|approve|sign|transfer|withdraw|invest|submit)/i,
    }),
  ).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("the register names the three facts about a mandate the route does not carry, and fills in none of them", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/ledger/users": LEDGER_USERS });
  await page.goto("/treasury/mandates");

  // The premise: the register rendered, so the panel below is beside data and
  // not standing in for it.
  await expect(page.getByTestId("mandate-row")).toHaveCount(2);

  const id = page.getByTestId("mandate-gap-id");
  await expect(id).toBeVisible();
  await expect(id).toContainText("MandateRegistry::registration");
  await expect(id).toContainText("No mandate id");

  const registered = page.getByTestId("mandate-gap-registered");
  await expect(registered).toBeVisible();
  await expect(registered).toContainText("registered_at");

  const ceiling = page.getByTestId("mandate-gap-ceiling");
  await expect(ceiling).toBeVisible();
  await expect(ceiling).toContainText("MandateRegistry::desk_mandate");

  // And the provenance: enrolled from configuration, with no route that could
  // change one — which is why the page has no control.
  await expect(page.getByTestId("mandate-provenance")).toContainText("UserMandate");
  await expect(page.getByTestId("mandate-provenance")).toContainText(
    "capital the platform promised to somebody nobody named",
  );
});

test("a ledger holding no mandate, a platform that cannot be reached, and a credential the route refuses are three different blocks", async ({
  page,
}) => {
  // Premise first: the same stub with mandates in it renders them, so the
  // three states below are absences of data and not a page that never worked.
  await servePlatform(page, { ...healthy(), "/ledger/users": LEDGER_USERS });
  await page.goto("/treasury/mandates");
  await expect(page.getByTestId("mandate-row")).toHaveCount(2);
  await expect(page.getByTestId("mandate-none")).toHaveCount(0);

  // An observed empty register.
  await servePlatform(page, { ...healthy(), "/ledger/users": { ...LEDGER_USERS, users: [] } });
  await page.goto("/treasury/mandates");
  const none = page.getByTestId("mandate-none");
  await expect(none).toBeVisible();
  await expect(none).toContainText("The ledger holds no mandate.");
  await expect(none).toContainText("a read that succeeded and found nothing");
  await expect(page.getByTestId("mandate-table")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);

  // A platform nothing could reach: its own block, in its own colour.
  await servePlatformUnreachable(page);
  await page.goto("/treasury/mandates");
  const unreachable = page.locator("[data-state-block=disconnected]").first();
  await expect(unreachable).toBeVisible();
  await expect(unreachable).toContainText("The platform could not be reached.");
  await expect(page.getByTestId("mandate-none")).toHaveCount(0);
  await expect(page.getByTestId("mandate-table")).toHaveCount(0);

  // A credential below the route's role. `/ledger/users` requires analyst and
  // the portal grants viewer on self-registration, so this is what a signed-up
  // reader actually meets.
  await serveDenied(page, "/ledger/users", healthy());
  await page.goto("/treasury/mandates");
  const denied = page.locator("[data-state-block=refused]").first();
  await expect(denied).toBeVisible();
  await expect(denied).toContainText("may not read /api/v1/ledger/users");
  await expect(denied).toContainText("this route requires the analyst role");
  await expect(page.getByTestId("mandate-none")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  // The declaration survives every one of them.
  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
});

test("the register is reachable from the console's own navigation and names the route it reads", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/ledger/users": LEDGER_USERS });
  await page.goto("/treasury/ledger");

  // The premise: the sidebar rendered and carries the treasury section.
  const sidebar = page.getByTestId("sidebar");
  await expect(sidebar.locator('a[href="/treasury/ledger"]')).toHaveCount(1);

  const link = sidebar.locator('a[href="/treasury/mandates"]');
  await expect(link, "the mandate register is not reachable from the navigation").toHaveCount(1);
  await link.click();
  await expect(page.getByRole("heading", { name: "Mandates", exact: true })).toBeVisible();
  await expect(page.getByTestId("treasury-declaration")).toContainText("GET /ledger/users");
});
