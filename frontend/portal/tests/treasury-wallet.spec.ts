/**
 * `/treasury/wallet`: holdings observed at venues beside the ledger's view,
 * and the reconciliation outcome per venue-asset, from `GET /wallet`.
 *
 * The failures these tests prevent:
 *
 * * an unassembled wallet rendered as an empty account — asserted by the
 *   "not assembled" state carrying the platform's reason verbatim, and no
 *   holdings table drawn beside it;
 * * a halt rendered as a row among rows — asserted by a `halt` outcome
 *   raising an alert region with the alert's own message and the platform's
 *   own halt count;
 * * a page in the treasury section without the paper-trading declaration, or
 *   with a control that could move capital.
 *
 * The first body is the example in `backend/crates/apps/qip-api/ROUTES-LEDGER.md`
 * — what every current deployment answers. The second is built to the same
 * contract's stated shape for an assembled wallet with one halt; no
 * deployment has assembled one, so it cannot be captured.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

const NOT_ASSEMBLED =
  "no wallet is assembled in this process. A wallet is a read model over holdings observed through read-only channels, and the kernel observes no custodian, venue balance or chain address; until an observation source is wired in there is nothing to pair with the ledger, and a wallet showing zero would read as an empty account rather than an unobserved one.";

const WALLET_NOT_ASSEMBLED = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  assembled: false,
  reason: NOT_ASSEMBLED,
  as_of: null,
  holdings: [],
  reconciliation: { outcomes: [], halted_venue_assets: 0 },
} as const;

const HALT_MESSAGE =
  "halt sim-venue/USD: delta_beyond_tolerance — observed 990 against expected 1000 (delta -10, tolerance 1) as of 2025-10-09T08:50:00Z via read_only_api_key; investigate at the venue and the ledger, the wallet writes no correction";

const WALLET_WITH_HALT = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  assembled: true,
  reason: null,
  as_of: "2025-10-09T08:53:00Z",
  holdings: [
    {
      venue: "sim-venue",
      asset: "USD",
      observed_quantity: "990",
      observed_at: "2025-10-09T08:50:00Z",
      provenance: "read_only_api_key",
      ledger_expected: "1000",
    },
    {
      venue: "sim-venue",
      asset: "EUR",
      observed_quantity: "500",
      observed_at: "2025-10-09T08:50:00Z",
      provenance: "statement",
      ledger_expected: "500",
    },
  ],
  reconciliation: {
    outcomes: [
      { outcome: "reconciled", venue: "sim-venue", asset: "EUR", delta: "0" },
      {
        outcome: "halt",
        venue: "sim-venue",
        asset: "USD",
        delta: "-10",
        alert: {
          venue_asset: { venue: "sim-venue", asset: "USD" },
          cause: "delta_beyond_tolerance",
          expected: "1000",
          observed: "990",
          delta: "-10",
          tolerance: "1",
          observed_at: "2025-10-09T08:50:00Z",
          provenance: "read_only_api_key",
          message: HALT_MESSAGE,
        },
      },
    ],
    halted_venue_assets: 1,
  },
} as const;

test("an unassembled wallet is said to be unassembled, in the platform's words, and the page holds no control that moves capital", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/wallet": WALLET_NOT_ASSEMBLED });
  await page.goto("/treasury/wallet");

  const content = page.locator("#content");
  await expect(page.getByRole("heading", { name: "Wallet" })).toBeVisible();

  // The state, verbatim, and no table drawn as though a wallet held nothing.
  await expect(page.getByTestId("wallet-not-assembled")).toBeVisible();
  await expect(page.getByTestId("wallet-not-assembled")).toContainText(NOT_ASSEMBLED);
  await expect(page.getByTestId("wallet-holdings")).toHaveCount(0);
  await expect(page.getByTestId("wallet-halts")).toHaveCount(0);

  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("treasury-body-posture")).toHaveText("PAPER TRADING");
  await expect(content).toContainText("Nothing on this page can move capital.");

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

test("a halt is the loudest thing on the page and carries the alert's own message", async ({ page }) => {
  await servePlatform(page, { ...healthy(), "/wallet": WALLET_WITH_HALT });
  await page.goto("/treasury/wallet");

  // The premise: the wallet is assembled and both holdings are drawn.
  await expect(page.getByTestId("wallet-not-assembled")).toHaveCount(0);
  await expect(page.getByTestId("wallet-holding")).toHaveCount(2);

  // The halt count is the platform's, the alert region is present, and the
  // message is the platform's sentence and not a paraphrase.
  await expect(page.getByTestId("wallet-halt-count")).toHaveText("1");
  const halts = page.getByTestId("wallet-halts");
  await expect(halts).toBeVisible();
  await expect(halts).toHaveAttribute("role", "alert");
  await expect(halts).toContainText("HALT");
  await expect(halts).toContainText("sim-venue/USD");
  await expect(halts).toContainText(HALT_MESSAGE);

  // The reconciled row is a row; the halted row is marked.
  await expect(page.getByTestId("wallet-outcome")).toHaveCount(2);
  await expect(page.locator('[data-testid="wallet-outcome"][data-alert="true"]')).toHaveCount(1);

  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.locator("#content").locator("button[type=submit], form")).toHaveCount(0);
});
