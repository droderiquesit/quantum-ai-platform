/**
 * The chrome: what is true on every route, whatever the platform says.
 *
 * These are the assertions that must not depend on a page, a preference or a
 * state. A paper-trading declaration that is conditional is a declaration that
 * will one day be absent on the one screen where it mattered.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform, servePlatformUnreachable } from "./support/platform";

const ROUTES = [
  "/",
  "/portfolio",
  "/orders",
  "/risk",
  "/markets",
  "/data-sources",
  "/order-entry",
  "/system",
  // One from each of the sections added for the PEOS eight-section map, so a
  // new section cannot ship a route the declaration is missing from.
  "/command/alerts",
  "/intelligence/predictions",
  "/research/quantum",
  "/portfolio/pnl",
  "/risk/audit",
  "/execution/fills",
  "/operations/telemetry",
  "/admin/autonomy",
] as const;

test("the paper trading declaration is on every route", async ({ page }) => {
  await servePlatform(page, healthy());
  for (const route of ROUTES) {
    await page.goto(route);
    // Wait for the platform's answer to land first. Without this the assertion
    // below can be satisfied by the first paint, when `live_capable` is still
    // null and the banner renders unconditionally — which let a mutation that
    // blanked the declaration for a paper-only platform pass this test.
    await expect(
      page.getByText("platform paper-only"),
      `the platform's answer never landed on ${route}`,
    ).toBeVisible();
    const banner = page.getByTestId("paper-trading-banner");
    await expect(banner, `no declaration on ${route}`).toBeVisible();
    await expect(banner).toHaveText("PAPER TRADING");
  }
});

test("the declaration survives a platform that cannot be reached", async ({ page }) => {
  // The case it exists for. During an incident the upstream is exactly what is
  // missing, and a banner assembled from upstream state would vanish at the
  // moment an operator most needs to know what this console can do.
  await servePlatformUnreachable(page);
  for (const route of ROUTES) {
    await page.goto(route);
    await expect(
      page.getByTestId("paper-trading-banner"),
      `the declaration disappeared on ${route} when the platform was unreachable`,
    ).toBeVisible();
  }
});

test("the console reports the platform's own live capability, in both directions", async ({
  page,
}) => {
  // "Paper only" is a property of the deployment, not of this console. The
  // banner reads it live and says which it saw, because a console that asserts
  // the safe answer without checking is a console that will one day assert it
  // while the process behind it is configured otherwise.
  await servePlatform(page, healthy());
  await page.goto("/");
  await expect(page.getByText("platform paper-only")).toBeVisible();
  await expect(page.getByText("platform is live-capable")).toHaveCount(0);

  // The premise for the second half: the same page, the same code, a different
  // answer from the platform.
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await servePlatform(page, healthy({ live_capable: true }));
  await page.goto("/");
  await expect(page.getByText("platform is live-capable")).toBeVisible();
  await expect(page.getByText("platform paper-only")).toHaveCount(0);
});

test("an unreachable platform is reported and never rendered as an empty book", async ({ page }) => {
  // The failure this prevents: a gateway error rendering as zero positions and
  // zero orders, which reads exactly like a flat, healthy book.
  await servePlatformUnreachable(page);
  await page.goto("/portfolio");
  const body = page.locator("body");
  await expect(body).toContainText(/unreachable|not answering/i);
});
