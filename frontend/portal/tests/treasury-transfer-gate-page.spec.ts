/**
 * `/treasury/transfer-gate`: the seven veto-only checks of blueprint §37.3
 * and the last assessment, from `GET /transfer-gate`.
 *
 * The failures these tests prevent:
 *
 * * seven green ticks nothing earned — asserted by "no assessment yet" when
 *   `last_assessment` is null, and every check reading "not assessed";
 * * the checks drawn in an order the gate does not run them in — asserted on
 *   the DOM order against the fabric's `GateCheck::ALL`;
 * * a veto rendered as a row among rows — asserted by the vetoing check
 *   marked, the checks before it passed and the ones after not reached;
 * * a page in the treasury section without the paper-trading declaration,
 *   without `executes: false` rendered, or with a control that composes or
 *   submits an intent.
 *
 * The first body is the example in `backend/crates/apps/qip-api/ROUTES-LEDGER.md`
 * — what every current deployment answers. The second carries a vetoed
 * assessment in the contract's stated shape; no deployment has one.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

/** `GateCheck::ALL`, in assessment order. A page that rendered these alphabetically would pass a set check. */
const CHECKS = [
  "corridor_authority",
  "caps",
  "minimum_interval",
  "stated_purpose",
  "source_balance",
  "velocity_and_anomaly",
  "kill_switch",
] as const;

const GATE_NOTE =
  "the gate is veto-only and has no transfer engine behind it: an approval is a record that the seven checks passed, and nothing in this platform consumes one. No caller has yet assessed an intent.";

const TRANSFER_GATE = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  checks: [
    { order: 1, name: "corridor_authority", alerts: true },
    { order: 2, name: "caps", alerts: false },
    { order: 3, name: "minimum_interval", alerts: false },
    { order: 4, name: "stated_purpose", alerts: false },
    { order: 5, name: "source_balance", alerts: false },
    { order: 6, name: "velocity_and_anomaly", alerts: true },
    { order: 7, name: "kill_switch", alerts: false },
  ],
  last_assessment: null,
  kill_switch: { halted: false, halted_scopes: [], tripped_by: null, reason: null, tripped_at: null },
  executes: false,
  note: GATE_NOTE,
} as const;

const VETO_REASON =
  "15000 exceeds the per-transfer cap of 10000; split it across the minimum interval or lower the amount";

const TRANSFER_GATE_VETOED = {
  ...TRANSFER_GATE,
  last_assessment: {
    assessed_at: "2025-10-09T08:53:00Z",
    outcome: "vetoed",
    check: "caps",
    reason: VETO_REASON,
    alert: false,
  },
} as const;

test("the seven checks are listed in the gate's order, no assessment is reported as none, and the page holds no control that moves capital", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/transfer-gate": TRANSFER_GATE });
  await page.goto("/treasury/transfer-gate");

  const content = page.locator("#content");
  await expect(page.getByRole("heading", { name: "Transfer gate" })).toBeVisible();
  // The premise: the route's answer landed, so the checks below are marked
  // from it rather than from the page's own loading state.
  await expect(page.getByTestId("gate-executes")).toContainText("executes: false");
  await expect(page.getByTestId("gate-executes")).toContainText(GATE_NOTE);

  // The order, read from the DOM.
  const names = await page
    .locator('[data-testid^="gate-check-"]')
    .evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-testid")));
  expect(names).toEqual(CHECKS.map((check) => `gate-check-${check}`));
  await expect(page.getByTestId("gate-roster-mismatch")).toHaveCount(0);
  for (const check of CHECKS) {
    await expect(page.getByTestId(`gate-check-${check}`)).toContainText("not assessed");
  }
  await expect(page.getByTestId("gate-check-corridor_authority")).toContainText("veto alerts a person");

  // No assessment is an absence, and is said to be one.
  await expect(page.getByTestId("gate-no-assessment")).toBeVisible();
  await expect(page.getByTestId("gate-no-assessment")).toContainText("no assessment yet");
  await expect(page.getByTestId("gate-assessment")).toHaveCount(0);
  await expect(page.getByTestId("gate-kill-switch")).toContainText("armed, not tripped");

  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("treasury-body-posture")).toHaveText("PAPER TRADING");
  await expect(content).toContainText("Nothing on this page can move capital.");

  await expect(content.locator("button[type=submit], form")).toHaveCount(0);
  const formsOutsideDialog = await page
    .locator("form")
    .evaluateAll((forms) => forms.filter((form) => form.closest("dialog") === null).length);
  expect(formsOutsideDialog).toBe(0);
  // Anchored on the verb: the page's own refresh control is named "Refresh
  // transfer gate", and an unanchored /transfer/ matched it.
  await expect(content.getByRole("button", { name: /^(assess|propose|approve|sign|transfer|submit)/i })).toHaveCount(0);
  await expect(content.locator("input, textarea, select")).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("a vetoed assessment marks the check that fired, the ones before it as passed, and the ones after as not reached", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/transfer-gate": TRANSFER_GATE_VETOED });
  await page.goto("/treasury/transfer-gate");

  await expect(page.getByTestId("gate-no-assessment")).toHaveCount(0);
  const assessment = page.getByTestId("gate-assessment");
  await expect(assessment).toBeVisible();
  await expect(assessment).toContainText("VETOED");
  await expect(assessment).toContainText("by caps");
  await expect(assessment).toContainText(VETO_REASON);

  await expect(page.getByTestId("gate-check-corridor_authority")).toContainText("passed");
  await expect(page.getByTestId("gate-check-caps")).toContainText("VETOED");
  await expect(page.getByTestId("gate-check-caps")).toHaveAttribute("data-alert", "true");
  for (const check of CHECKS.slice(2)) {
    await expect(page.getByTestId(`gate-check-${check}`)).toContainText("not reached");
  }

  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.locator("#content").locator("button[type=submit], form")).toHaveCount(0);
});
