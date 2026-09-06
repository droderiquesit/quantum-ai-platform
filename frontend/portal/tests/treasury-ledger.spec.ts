/**
 * `/treasury/ledger`: users, mandates, per-strategy balances and entitlements,
 * read from `GET /ledger/users` and rendered as answered.
 *
 * The failures these tests prevent:
 *
 * * a page in the treasury section without the paper-trading declaration —
 *   asserted on the page's own label, not the chrome's, and on the body's own
 *   `posture` literal rendered beside it;
 * * a withdrawal shown as anything but refused — asserted on the reason the
 *   platform gave, verbatim, because the platform's type has one arm and a
 *   page that could render the other would be a page ahead of the platform;
 * * an expected inflow folded into an available balance — asserted by the
 *   available cell carrying the platform's figure and not the sum, because
 *   `CashBalance::available` excludes declared inflows by construction and a
 *   page that added them back would size the reader's expectations against
 *   money that may never arrive;
 * * a control that could move capital — asserted by no form and no submit
 *   control inside the page, and no non-GET request leaving it.
 *
 * The body is the example in `backend/crates/apps/qip-api/ROUTES-LEDGER.md`,
 * the contract the route is built to, with one expected inflow added so the
 * separation has something to separate. It is not captured from a running
 * process: no deployment has yet enrolled a user with a declared inflow.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

const WITHDRAWAL_REFUSED =
  "capital does not leave the platform: ADR 0021 refuses the signing and withdrawal half of the treasury and ADR 0023 keeps that in force; a withdrawal is a separate, later, separately approved decision";

const LEDGER_USERS = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  evaluated_as_role: "viewer",
  products: ["research-tests"],
  fills_journalled: 2,
  users: [
    {
      user_id: "desk",
      mandate: {
        capital: "1000000",
        currency: "USD",
        risk_tolerance: "1",
        liquidity_floor: "0",
        investable: "1000000",
        exploration_share: "0",
        jurisdiction: "ZZ",
        permitted_families: { any: true, families: [] },
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
          can_view: { granted: true, reason: "desk holds a mandate in ZZ" },
          can_invest: { granted: false, reason: "desk holds the viewer role, which does not invest" },
          can_withdraw: { granted: false, reason: WITHDRAWAL_REFUSED },
        },
      ],
      entitlements_note: null,
    },
  ],
} as const;

test("the ledger page carries the paper label, refuses withdrawal in the platform's words, keeps expected inflows out of available, and holds no control that moves capital", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/ledger/users": LEDGER_USERS });
  await page.goto("/treasury/ledger");

  // The premise: the page rendered and the route's answer landed on it.
  // Without this, every absence below holds for a page that failed to render.
  const content = page.locator("#content");
  await expect(page.getByRole("heading", { name: "Ledger" })).toBeVisible();
  await expect(page.getByTestId("ledger-user-count")).toHaveText("1");
  await expect(content).toContainText("desk");

  // The declaration, on the page and not only in the chrome: the page's own
  // static label, and the body's `posture` literal rendered as it came.
  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("treasury-body-posture")).toHaveText("PAPER TRADING");
  await expect(content).toContainText("Nothing on this page can move capital.");

  // The withdrawal entitlement: refused, with the platform's reason verbatim.
  const withdrawal = page.getByTestId("withdrawal-entitlement");
  await expect(withdrawal).toHaveCount(1);
  await expect(withdrawal).toContainText("refused");
  await expect(withdrawal).toContainText(WITHDRAWAL_REFUSED);
  await expect(withdrawal).not.toContainText("GRANTED");

  // Available is the platform's figure; the declared inflow is shown beside
  // it and is not in it. 750.75 is what a page that summed them would show.
  await expect(page.getByTestId("ledger-available")).toHaveText("250.75");
  await expect(page.getByTestId("ledger-expected")).toContainText("500");
  await expect(page.getByTestId("ledger-expected")).toContainText("wire-0001");
  await expect(content).not.toContainText("750.75");

  // No control on the page can move capital. The page holds no form and no
  // submit control; the chrome's one form is the kill-switch halt dialog,
  // which predates this section, is pinned by boundary.spec, and is inside
  // <dialog>, not inside the page.
  await expect(content.locator("button[type=submit], form")).toHaveCount(0);
  const formsOutsideDialog = await page
    .locator("form")
    .evaluateAll((forms) => forms.filter((form) => form.closest("dialog") === null).length);
  expect(formsOutsideDialog).toBe(0);
  await expect(
    content.getByRole("button", { name: /^(propose|approve|sign|transfer|withdraw|submit)/i }),
  ).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("a ledger with no product says why entitlements are absent rather than showing none", async ({ page }) => {
  await servePlatform(page, {
    ...healthy(),
    "/ledger/users": {
      ...LEDGER_USERS,
      products: [],
      users: [
        {
          ...LEDGER_USERS.users[0],
          entitlements: [],
          entitlements_note: "no strategy family is registered with the central factory, so there is no product to evaluate against",
        },
      ],
    },
  });
  await page.goto("/treasury/ledger");
  await expect(page.getByTestId("ledger-user-count")).toHaveText("1");
  await expect(page.getByTestId("ledger-entitlements-note")).toContainText(
    "no strategy family is registered with the central factory",
  );
  await expect(page.getByTestId("withdrawal-entitlement")).toHaveCount(0);
  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
});
