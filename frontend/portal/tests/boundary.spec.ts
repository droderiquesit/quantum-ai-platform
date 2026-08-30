/**
 * The paper-trading boundary, from the browser's side.
 *
 * The boundary itself is held by the platform, not by this console: `qip-api`
 * serves no write route for an order and answers `POST /api/v1/orders` with
 * 405. That is the right place for it — a boundary the UI holds is a boundary
 * that moves when somebody edits the UI.
 *
 * What these tests pin is the console's half of the contract: that it sends
 * paper on every ticket, that it shows the platform's refusal rather than
 * reporting a success, and that it cannot clear a halt it did not cause.
 */
import { expect, test } from "@playwright/test";
import { RISK_BODY, healthy, servePlatform } from "./support/platform";

const HEADER = "x-qip-gateway";

const RISK = { "/risk": RISK_BODY };

test("a ticket is sent as paper, and the platform's refusal is what the operator sees", async ({
  page,
}) => {
  const sent: Array<Record<string, unknown>> = [];

  await servePlatform(page, { ...healthy(), ...RISK });
  // Intercepted ahead of the general stub so the write path is observed rather
  // than answered by the catch-all 404.
  await page.route("**/api/gateway/orders", async (route) => {
    if (route.request().method() !== "POST") {
      await route.fulfill({
        status: 200,
        headers: { [HEADER]: "upstream", "content-type": "application/json" },
        body: JSON.stringify({ orders: [], refusals: 0, reconciliation_breaks: [] }),
      });
      return;
    }
    sent.push(route.request().postDataJSON() as Record<string, unknown>);
    // Exactly what the running platform answers, checked against it directly.
    await route.fulfill({
      status: 405,
      headers: { [HEADER]: "upstream", "content-type": "application/json" },
      body: JSON.stringify({ error: "that method is not allowed here" }),
    });
  });

  await page.goto("/order-entry");
  await page.locator("#instrument").fill("ACME");
  await page.locator("#quantity").fill("10");
  await page.locator("#limitPrice").fill("100");
  // `#reason` and not a label match: the kill switch in the chrome also has a
  // field labelled "Reason", and it sits inside a closed dialog.
  await page.locator("#reason").fill("boundary specification test");
  await page.getByTestId("submit-paper-order").click();

  // The premise: a request was actually issued. Without this, the assertions
  // below would hold just as well for a button wired to nothing.
  await expect
    .poll(() => sent.length, { message: "the ticket was never sent" })
    .toBeGreaterThan(0);

  // Every ticket carries paper: true. Not a default the platform applies — a
  // fact the console states on the wire, so a platform that ever grew a write
  // route would receive an explicitly paper order from this console.
  expect(sent[0]?.paper).toBe(true);

  // And the refusal is surfaced, not swallowed into a success.
  await expect(page.locator("body")).toContainText(/not allowed/i);
});

test("the console can halt the platform and offers no way to clear a halt it did not cause", async ({
  page,
}) => {
  // Asymmetric on purpose: anything that notices trouble may stop the platform,
  // and only an operator with an identity may start it again. A console that
  // could clear its own halt would make the halt advisory.
  await servePlatform(page, { ...healthy(), ...RISK });
  await page.goto("/");

  await page.getByTestId("kill-switch-open").click();
  // The premise: the dialog is open and offering the halt path.
  await expect(page.getByTestId("kill-switch-confirm")).toBeVisible();
  // While the platform is not halted, no clear control exists at all.
  await expect(page.getByTestId("kill-switch-clear")).toHaveCount(0);

  // The confirm stays disabled until a reason is recorded: a halt with no
  // reason in the event log is a halt nobody can explain afterwards.
  await expect(page.getByTestId("kill-switch-confirm")).toBeDisabled();
  await page.getByTestId("kill-switch-reason").fill("boundary specification test");
  await expect(page.getByTestId("kill-switch-confirm")).toBeEnabled();
});

test("a halted platform is shown as halted rather than as quiet", async ({ page }) => {
  await servePlatform(page, {
    ...healthy({ halted: true }),
    "/risk": {
      ...RISK_BODY,
      kill_switch: {
        ...RISK_BODY.kill_switch,
        halted: true,
        reason: "venue reconciliation break",
      },
    },
  });
  await page.goto("/");
  await expect(page.locator("body")).toContainText(/halt/i);
});
