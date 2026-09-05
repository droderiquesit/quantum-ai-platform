/**
 * `/treasury/accounts`: one account as the ledger holds it — mandate,
 * eligibility verdict, books and entitlements — read from `GET /ledger/users`.
 *
 * The failures these tests prevent:
 *
 * * an account id the ledger does not hold rendering as a ledger with no
 *   accounts, or as an outage. Three different facts, three different remedies;
 *   a bookmark left behind by a retired mandate produces the first, and reading
 *   it as the second tells an operator the ledger is empty when it is not;
 * * a page claiming the account is the reader's. The platform serves no
 *   per-user route, evaluates every entitlement as the ledger's viewer role
 *   against a user id, and is reached with one deployment credential shared by
 *   every browser session — so the account shown is the one an operator picked.
 *   Asserted on the page saying so, because the standing attribution finding is
 *   that a console naming the signed-in person as the subject of a
 *   platform-recorded fact is naming an identity nothing downstream records;
 * * an expected inflow folded into an available balance — asserted by the
 *   available cell carrying the platform's figure and not the sum;
 * * a withdrawal shown as anything but refused, or any control at all on a page
 *   that has none.
 *
 * Bodies are the contract in `backend/crates/apps/qip-api/ROUTES-LEDGER.md`,
 * not a capture: no deployment has enrolled a user with a declared inflow.
 */
import { expect, test, type Page } from "@playwright/test";
import { GATEWAY, healthy, servePlatform, servePlatformUnreachable } from "./support/platform";

const WITHDRAWAL_REFUSED =
  "capital does not leave the platform: ADR 0021 refuses the signing and withdrawal half of the treasury and ADR 0023 keeps that in force; a withdrawal is a separate, later, separately approved decision";

const ALICE = {
  user_id: "alice",
  mandate: {
    capital: "1000",
    currency: "USD",
    risk_tolerance: "0.5",
    liquidity_floor: "250",
    investable: "750",
    exploration_share: "0",
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
  balances: [
    {
      strategy: "AAA",
      currency: "USD",
      settled: "250.75",
      reserved: "0",
      available: "250.75",
      expected_inflows_total: "500",
      expected_inflows: [{ reference: "wire-0001", amount: "500", declared_at: "2025-10-09T08:00:00Z" }],
      entries: 2,
      last_entry_at: "2025-10-09T08:53:20Z",
    },
  ],
  entitlements: [
    {
      family: "research-tests",
      role: "viewer",
      evaluated_at: "2025-10-09T08:53:20Z",
      can_view: { granted: true, reason: "alice holds a mandate in GB" },
      can_invest: { granted: false, reason: "alice holds the viewer role, which does not invest" },
      can_withdraw: { granted: false, reason: WITHDRAWAL_REFUSED },
    },
  ],
  entitlements_note: null,
} as const;

const DESK = {
  ...ALICE,
  user_id: "desk",
  mandate: { ...ALICE.mandate, jurisdiction: "ZZ", permitted_families: { any: true, families: [] } },
  balances: [],
  entitlements: [],
  entitlements_note: "no product to evaluate against",
} as const;

const LEDGER_USERS = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  evaluated_as_role: "viewer",
  products: ["research-tests"],
  fills_journalled: 2,
  users: [ALICE, DESK],
} as const;

/** A platform that refuses the console's credential on one route. See the products spec. */
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

test("the account page renders the asked-for account's mandate, the ledger's refusal verdict in its own words, its books with declared inflows kept out of available, and no control at all", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/ledger/users": LEDGER_USERS });
  await page.goto("/treasury/accounts?user=alice");

  // The premise: the page rendered and the asked-for account is the one on it.
  await expect(page.getByRole("heading", { name: "Account", exact: true })).toBeVisible();
  await expect(page.getByTestId("account-id")).toHaveText("alice");
  await expect(page.getByTestId("account-detail")).toHaveAttribute("data-user", "alice");
  await expect(page.getByTestId("account-choice")).toHaveCount(2);

  // The declaration, on the page and not only in the chrome.
  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("treasury-body-posture")).toHaveText("PAPER TRADING");

  // The mandate, as the platform's figures and not the page's arithmetic.
  // `formatDecimal` groups thousands and never rounds; the platform's exact
  // decimal text is what is grouped, which is why this reads "1,000".
  await expect(page.getByTestId("account-capital")).toHaveText("1,000");
  await expect(page.getByTestId("account-investable")).toHaveText("750");
  await expect(page.getByTestId("account-book-count")).toHaveText("1");
  await expect(page.getByTestId("account-entries")).toHaveText("2");

  // The eligibility verdict: the ledger's stable token and its own sentence.
  await expect(page.getByTestId("account-eligibility")).toHaveAttribute("data-eligible", "false");
  await expect(page.getByTestId("account-eligibility-verdict")).toHaveText("not_yet_verified");
  await expect(page.getByTestId("account-eligibility-reason")).toContainText(
    "an operator must record a verification",
  );

  // Available is the platform's figure; the declared inflow sits beside it and
  // is not in it. 750.75 is what a page that summed them would show.
  await expect(page.getByTestId("account-available")).toHaveText("250.75");
  await expect(page.getByTestId("account-expected")).toContainText("wire-0001");
  await expect(page.locator("#content")).not.toContainText("750.75");

  // Withdrawal refused in the platform's words, and no control anywhere.
  const withdrawal = page.getByTestId("withdrawal-entitlement");
  await expect(withdrawal).toHaveCount(1);
  await expect(withdrawal).toContainText(WITHDRAWAL_REFUSED);
  await expect(withdrawal).not.toContainText("GRANTED");
  const content = page.locator("#content");
  await expect(content.locator("button[type=submit], form")).toHaveCount(0);
  await expect(
    content.getByRole("button", { name: /^(propose|approve|sign|transfer|withdraw|invest|submit)/i }),
  ).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("the page never claims the account is the reader's own, and says whose evaluation it is showing", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/ledger/users": LEDGER_USERS });
  await page.goto("/treasury/accounts");

  // The premise: with no account asked for, the page defaulted to the first the
  // ledger listed — and says that is what it did.
  await expect(page.getByTestId("account-id")).toHaveText("alice");
  const defaulted = page.getByTestId("account-defaulted");
  await expect(defaulted).toBeVisible();
  await expect(defaulted).toContainText("not the account of whoever is signed in");
  await expect(defaulted).toContainText("operator@env");

  // The entitlements are the platform's for the ledger's viewer role, against a
  // user id — stated, because the console cannot bind a session to an account.
  await expect(page.getByTestId("account-entitlement")).toHaveCount(1);
  await expect(page.locator("#content")).toContainText(
    "Evaluated as the ledger’s viewer role against this user id, not as the person reading",
  );
  await expect(page.locator("#content")).not.toContainText("Your account");
});

test("an account id the ledger does not hold is not an empty ledger and not an unreachable platform", async ({
  page,
}) => {
  // Premise first: the same stub, asked for an account it does hold, renders it.
  await servePlatform(page, { ...healthy(), "/ledger/users": LEDGER_USERS });
  await page.goto("/treasury/accounts?user=desk");
  await expect(page.getByTestId("account-id")).toHaveText("desk");
  await expect(page.getByTestId("account-unknown")).toHaveCount(0);

  // Asked for one it does not: its own block, naming the count it does hold.
  await page.goto("/treasury/accounts?user=nobody");
  const unknown = page.getByTestId("account-unknown");
  await expect(unknown).toBeVisible();
  await expect(unknown).toContainText('The ledger holds no account "nobody".');
  await expect(unknown).toContainText("The route answered 2 accounts");
  await expect(page.getByTestId("account-detail")).toHaveCount(0);
  // Not the ledger-is-empty block, and not the outage block.
  await expect(page.getByTestId("account-none")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  // The accounts it does hold are still one click away.
  await expect(page.getByTestId("account-choice")).toHaveCount(2);

  // A ledger with nothing in it: a third block, in different words again.
  await servePlatform(page, { ...healthy(), "/ledger/users": { ...LEDGER_USERS, users: [] } });
  await page.goto("/treasury/accounts?user=nobody");
  await expect(page.getByTestId("account-none")).toBeVisible();
  await expect(page.getByTestId("account-none")).toContainText("The ledger holds no account.");
  await expect(page.getByTestId("account-unknown")).toHaveCount(0);

  // And the platform that could not be reached: a fourth, in its own colour.
  await servePlatformUnreachable(page);
  await page.goto("/treasury/accounts?user=nobody");
  const unreachable = page.locator("[data-state-block=disconnected]").first();
  await expect(unreachable).toBeVisible();
  await expect(unreachable).toContainText("The platform could not be reached.");
  await expect(page.getByTestId("account-none")).toHaveCount(0);
  await expect(page.getByTestId("account-unknown")).toHaveCount(0);
});

test("an account with no book says no book has been opened rather than showing a balance of zero", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/ledger/users": LEDGER_USERS });
  await page.goto("/treasury/accounts?user=desk");

  // The premise: this is the desk's account and it carries no balance row.
  await expect(page.getByTestId("account-id")).toHaveText("desk");
  await expect(page.getByTestId("account-book-count")).toHaveText("0");
  await expect(page.getByTestId("account-balance-row")).toHaveCount(0);

  const noBooks = page.getByTestId("account-no-books");
  await expect(noBooks).toBeVisible();
  await expect(noBooks).toContainText("No book has been opened for this account.");
  await expect(noBooks).toContainText("an account with no book, not a book at zero");
  // The platform's own note for an account with no entitlement, rendered as it came.
  await expect(page.getByTestId("account-entitlements-note")).toContainText(
    "no product to evaluate against",
  );
});

test("a credential the route refuses says so, and is not shown as an account the ledger does not hold", async ({
  page,
}) => {
  await serveDenied(page, "/ledger/users", healthy());
  await page.goto("/treasury/accounts?user=alice");

  const denied = page.locator("[data-state-block=refused]").first();
  await expect(denied).toBeVisible();
  await expect(denied).toContainText("may not read /api/v1/ledger/users");
  await expect(denied).toContainText("this route requires the analyst role");
  await expect(page.getByTestId("account-unknown")).toHaveCount(0);
  await expect(page.getByTestId("account-none")).toHaveCount(0);
  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
});
