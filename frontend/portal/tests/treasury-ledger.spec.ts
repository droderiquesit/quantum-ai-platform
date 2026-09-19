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
 *   control inside the page, and no non-GET request leaving it;
 * * the held bucket folded into available, or into the expected total — the
 *   three figures ADR 0085 keeps apart are asserted apart: the platform's
 *   `uninvestable` in its own cell, and none of the sums a page that added
 *   any two of them would show anywhere on the page;
 * * a declared inflow shown without the platform's `inflow_posting` sentence
 *   beside it, or with a paraphrase — asserted verbatim, because the sentence
 *   is the platform's constant and a paraphrase would go on saying "never
 *   posted" the day the platform deleted the constant and started posting;
 * * a form that could declare or cancel an inflow — the two operator routes
 *   are named and the refusal every credential meets is rendered, labelled as
 *   the route contract's statement and not as an answer the platform gave
 *   this page, which it did not: `/ledger/users` carries no such field.
 *
 * The body is the example in `backend/crates/apps/qip-api/ROUTES-LEDGER.md`,
 * the contract the route is built to, with one expected inflow and a non-zero
 * `uninvestable` added so the separation has something to separate. The
 * non-zero bucket is a shape this build never serves — nothing posts an
 * arrival, so the figure is `"0"` on every balance — and that is the point:
 * a page proven only against zero is a page whose sum with zero is invisible.
 * It is not captured from a running process: no deployment has yet enrolled
 * a user with a declared inflow.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

const WITHDRAWAL_REFUSED =
  "capital does not leave the platform: ADR 0021 refuses the signing and withdrawal half of the treasury and ADR 0023 keeps that in force; a withdrawal is a separate, later, separately approved decision";

/** `INFLOW_POSTING` in `backend/crates/apps/qip-api/src/ledger_views.rs`, character for character. */
const INFLOW_POSTING =
  "no declared inflow is ever posted by this build: nothing here can say a user's wire landed, so an expected inflow stays expected until an operator cancels it, and `uninvestable` is zero on every balance until a reconciled statement exists (ADR 0085)";

const LEDGER_USERS = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  evaluated_as_role: "viewer",
  products: ["research-tests"],
  fills_journalled: 2,
  inflow_posting: INFLOW_POSTING,
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
          uninvestable: "125",
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
  await expect(page.getByTestId("ledger-expected-total")).toHaveText("500");
  await expect(page.getByTestId("ledger-expected")).toContainText("wire-0001");
  await expect(content).not.toContainText("750.75");

  // The held bucket is its own figure, in its own cell, labelled as held and
  // not sized against — and it is summed with nothing. 375.75 is available
  // plus held; 625 is held plus the expected total; 875.75 is all three.
  await expect(page.getByTestId("ledger-uninvestable")).toHaveText("125");
  await expect(page.locator("#content th", { hasText: "Held, not sized against" })).toHaveCount(1);
  await expect(content).not.toContainText("375.75");
  await expect(content).not.toContainText("625");
  await expect(content).not.toContainText("875.75");

  // The declaration itself: one row, by the reference the user supplied, with
  // its amount and the instant it was declared.
  const inflow = page.getByTestId("ledger-inflow");
  await expect(inflow).toHaveCount(1);
  await expect(inflow).toHaveAttribute("data-reference", "wire-0001");
  await expect(inflow).toContainText("wire-0001");
  await expect(inflow).toContainText("500");
  await expect(inflow).toContainText("declared");

  // And beside it the platform's own sentence, verbatim. `toHaveText` is a
  // whole-text match, so a paraphrase, a truncation or an addition fails it.
  await expect(page.getByTestId("ledger-inflow-posting")).toHaveText(INFLOW_POSTING);

  // The two routes a declaration is made and cancelled at are named, with the
  // refusal every credential meets — labelled as the contract's statement,
  // because the platform returned no such sentence to this page.
  const declaration = page.getByTestId("inflow-declaration");
  await expect(declaration).toBeVisible();
  await expect(page.getByTestId("inflow-route-declare")).toHaveText(
    "POST /api/v1/ledger/users/{user}/expected-inflows",
  );
  await expect(page.getByTestId("inflow-route-cancel")).toHaveText(
    "DELETE /api/v1/ledger/users/{user}/expected-inflows/{reference}",
  );
  await expect(page.getByTestId("inflow-declaration-refusal")).toContainText("standing bearer token");
  await expect(page.getByTestId("inflow-declaration-refusal")).toContainText("ADR 0076");
  await expect(page.getByTestId("inflow-declaration-refusal-source")).toContainText(
    "not an answer the platform returned to this page",
  );
  await expect(declaration.locator("form, input, textarea, select, button")).toHaveCount(0);

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

test("a book with nothing declared shows held as its own zero, no inflow row, and still the posting sentence", async ({
  page,
}) => {
  // The shape this build actually serves: `uninvestable` is "0" on every
  // balance and nothing is declared. The sentence is still rendered, because
  // it is also what says the zero means "nothing posts an arrival" rather
  // than "nothing arrived past the ceiling" — two different facts a bare
  // zero cannot tell apart.
  await servePlatform(page, {
    ...healthy(),
    "/ledger/users": {
      ...LEDGER_USERS,
      users: [
        {
          ...LEDGER_USERS.users[0],
          balances: [
            {
              ...LEDGER_USERS.users[0].balances[0],
              uninvestable: "0",
              expected_inflows_total: "0",
              expected_inflows: [],
            },
          ],
        },
      ],
    },
  });
  await page.goto("/treasury/ledger");

  // The premise: the row rendered with the platform's available figure.
  await expect(page.getByTestId("ledger-balance-row")).toHaveCount(1);
  await expect(page.getByTestId("ledger-available")).toHaveText("250.75");

  await expect(page.getByTestId("ledger-uninvestable")).toHaveText("0");
  await expect(page.getByTestId("ledger-expected-total")).toHaveText("0");
  await expect(page.getByTestId("ledger-inflow")).toHaveCount(0);
  await expect(page.getByTestId("ledger-inflow-posting")).toHaveText(INFLOW_POSTING);
  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
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
