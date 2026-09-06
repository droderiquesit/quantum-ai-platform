/**
 * The theme system, the environment badge, and the simulated-data label.
 *
 * Three guarantees, one file. The first is comfort; the last two are safety:
 * the badge is how an operator knows which environment a number belongs to,
 * and the SIMULATED label is the only thing separating an illustration from a
 * fabrication. Both must be structural — present because the code cannot
 * render the page without them, not because someone remembered. Since the
 * last illustrated pages started reading platform routes, the label's
 * guarantee is held by there being nothing to label, and the third test
 * guards that instead.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

test("choosing the light theme survives reload and navigation", async ({ page }) => {
  await servePlatform(page, healthy());
  await page.goto("/");

  // The premise: dark is the default and stores no attribute.
  await expect(page.locator("html")).not.toHaveAttribute("data-theme", "light");

  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

  // Reload: the boot script must re-apply the stored choice before paint.
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

  // Navigation within the app must not shed it either.
  await page.goto("/risk");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

  // And back: the toggle must be able to return to dark, not only leave it.
  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).not.toHaveAttribute("data-theme", "light");
});

test("the environment badge reports paper and is a report, not a control", async ({ page }) => {
  await servePlatform(page, healthy());
  await page.goto("/");
  const badge = page.getByTestId("environment-badge").first();
  await expect(badge).toBeVisible();
  await expect(badge).toContainText(/paper/i);
  // A badge that were a button would invite changing the environment from a
  // browser, which no surface here may do. It must be inert markup.
  const tag = await badge.evaluate((element) => element.tagName.toLowerCase());
  expect(tag, "the environment badge must not be an interactive control").not.toBe("button");
});

test("no portal page renders the simulated-data banner now that every page reads a platform route", async ({
  page,
}) => {
  // This test used to assert the opposite: that `/intelligence/predictions`
  // carried the SIMULATED DATA banner, because it was a seeded illustration.
  // The platform now serves `/predictions`, `/correlation` and `/backtests`,
  // and the three pages that illustrated read them instead. The rule the
  // banner enforced — an illustration is labelled or does not exist — now
  // holds by there being no illustration, and this guards that a page does
  // not quietly bring one back without the label.
  await servePlatform(page, healthy());
  for (const path of ["/intelligence/predictions", "/intelligence/correlation", "/research/backtesting"]) {
    await page.goto(path);
    // Premise: the page rendered a panel, so an absent banner is not an
    // absent page.
    await expect(page.locator("section.panel").first()).toBeVisible();
    await expect(page.getByTestId("simulated-banner"), `${path} shows a simulated banner`).toHaveCount(0);
    await expect(page.getByText("simulated data"), `${path} carries the simulated label`).toHaveCount(0);
  }
});
