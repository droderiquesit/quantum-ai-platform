/**
 * `/cognition/self-model`: the platform's measured accuracy per origin, read
 * from `GET /cognition/self-model` and rendered as answered.
 *
 * The failures these tests prevent:
 *
 * * a refused estimate rendered as a number — asserted by the row whose
 *   `accuracy` is `null` reading "below minimum sample (n < 10)" with the
 *   route's own threshold, and no "0" or "—" in its place, because a zero
 *   would read as an origin measured to be always wrong;
 * * rows re-ordered by this console — asserted on the DOM order against the
 *   body's order, which is deliberately not alphabetical and not by accuracy;
 * * an empty model rendered as a table with no rows — asserted by the empty
 *   state saying so and no table drawn beside it;
 * * a page in the cognition section without the paper-trading declaration, or
 *   with a control that could grade, re-weight or submit anything.
 *
 * The bodies are built to the contract as it was stated to this console —
 * `{components: [{kind, key, samples, accuracy, calibrated}], minimum_sample}`
 * — and not captured from a running process: the route is being added to the
 * API in parallel with this page, so no deployment has answered it yet.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

const SELF_MODEL = {
  components: [
    { kind: "detector", key: "momentum", samples: 42, accuracy: "0.6739", calibrated: true },
    { kind: "analyst", key: "macro-desk", samples: 3, accuracy: null, calibrated: false },
    { kind: "rung", key: "deep", samples: 17, accuracy: "0.5217", calibrated: true },
    { kind: "strategy_family", key: "aaa-mean-reversion", samples: 10, accuracy: "0.5000", calibrated: true },
  ],
  minimum_sample: 10,
} as const;

test("the self-model page renders rows as received, says a refused estimate is refused, and holds no control that acts", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/cognition/self-model": SELF_MODEL });
  await page.goto("/cognition/self-model");

  // The premise: the page rendered and the route's answer landed on it.
  // Without this, every absence below holds for a page that failed to render.
  const content = page.locator("#content");
  await expect(page.getByRole("heading", { name: "Self-model" })).toBeVisible();
  await expect(page.getByTestId("self-model-count")).toHaveText("4");
  await expect(page.getByTestId("self-model-minimum")).toHaveText("10");
  await expect(page.getByTestId("self-model-row")).toHaveCount(4);

  // The declaration, on the page and not only in the chrome.
  await expect(page.getByTestId("cognition-paper-label")).toHaveText("PAPER TRADING");
  await expect(content).toContainText("Nothing on this page can act.");

  // The order, read from the DOM: the body's, which is neither alphabetical
  // by key nor descending by accuracy, so a page that sorted would fail.
  const keys = await page
    .locator('[data-testid="self-model-row"] td:nth-child(2)')
    .evaluateAll((cells) => cells.map((cell) => cell.textContent?.trim()));
  expect(keys).toEqual(["momentum", "macro-desk", "deep", "aaa-mean-reversion"]);

  // The refused row: the platform's threshold in the sentence, and no figure.
  const refused = page.locator('[data-testid="self-model-row"][data-refused="true"]');
  await expect(refused).toHaveCount(1);
  await expect(refused).toContainText("macro-desk");
  await expect(refused.getByTestId("self-model-accuracy")).toHaveText("below minimum sample (n < 10)");
  await expect(refused).toContainText("uncalibrated");

  // The estimated rows carry the platform's exact decimal text.
  const accuracies = await page
    .getByTestId("self-model-accuracy")
    .evaluateAll((cells) => cells.map((cell) => cell.textContent?.trim()));
  expect(accuracies).toEqual(["0.6739", "below minimum sample (n < 10)", "0.5217", "0.5000"]);
  await expect(page.getByTestId("self-model-empty")).toHaveCount(0);

  // No control on the page can act. The chrome's one form is the kill-switch
  // halt dialog, which is inside <dialog>, not inside the page.
  await expect(content.locator("button[type=submit], form, input, textarea, select")).toHaveCount(0);
  const formsOutsideDialog = await page
    .locator("form")
    .evaluateAll((forms) => forms.filter((form) => form.closest("dialog") === null).length);
  expect(formsOutsideDialog).toBe(0);
  await expect(
    content.getByRole("button", { name: /^(grade|calibrate|reweight|re-weight|recall|submit|buy|sell|trade)/i }),
  ).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("an empty self-model says it holds no component rather than drawing an empty table", async ({ page }) => {
  await servePlatform(page, { ...healthy(), "/cognition/self-model": { components: [], minimum_sample: 10 } });
  await page.goto("/cognition/self-model");

  await expect(page.getByTestId("self-model-count")).toHaveText("0");
  await expect(page.getByTestId("self-model-minimum")).toHaveText("10");
  await expect(page.getByTestId("self-model-empty")).toBeVisible();
  await expect(page.getByTestId("self-model-empty")).toContainText("holds no component");
  await expect(page.getByTestId("self-model-table")).toHaveCount(0);
  await expect(page.getByTestId("self-model-row")).toHaveCount(0);
  await expect(page.getByTestId("cognition-paper-label")).toHaveText("PAPER TRADING");
});
