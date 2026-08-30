/**
 * The installed application: its navigation, its manifest, and its offline page.
 *
 * What the service worker caches is asserted in `worker.spec.ts` instead. It
 * needs a real upstream over a real socket, because `page.route` fulfils a
 * request before the worker ever receives a `fetch` event — a worker test
 * written against these stubs would pass whatever the worker did.
 */
import { devices, expect, test } from "@playwright/test";
import { GATEWAY, healthy, servePlatform } from "./support/platform";

const PHONE = devices["Pixel 7"].viewport;

test.describe("the phone layout", () => {
  test.use({ viewport: PHONE });

  test("the hamburger opens the off-canvas navigation and the overlay closes it", async ({
    page,
  }) => {
    await servePlatform(page, healthy());
    await page.goto("/");
    // The premise: on a phone the sidebar starts off-canvas.
    const overlay = page.getByTestId("sidebar-overlay");
    await expect(page.getByTestId("mobile-menu")).toBeVisible();

    await page.getByTestId("mobile-menu").click();
    await expect(overlay).toBeVisible();
    await expect(
      page.getByTestId("sidebar").getByRole("link", { name: "Executive dashboard" }),
    ).toBeVisible();

    // Click right of the 260px panel: the panel sits above the overlay, so a
    // click at its left edge would test the panel, not the dismissal.
    await overlay.click({ position: { x: 380, y: 300 } });
    await expect(overlay, "the overlay outlived its dismissal").toHaveCount(0);
  });

  test("the kill switch stays reachable when the header has no room", async ({ page }) => {
    // It has been pushed off the right edge before, by chrome that had no
    // business outranking it.
    await servePlatform(page, healthy());
    await page.goto("/");
    const control = page.getByTestId("kill-switch-open");
    await expect(control).toBeVisible();
    const box = await control.boundingBox();
    expect(box, "the kill switch has no box").not.toBeNull();
    expect(
      (box?.x ?? 0) + (box?.width ?? 0),
      "the kill switch runs past the right edge of a phone screen",
    ).toBeLessThanOrEqual(PHONE?.width ?? 0);
  });

  test("every section is reachable from the off-canvas sidebar", async ({ page }) => {
    await servePlatform(page, healthy());
    await page.goto("/");
    await page.getByTestId("mobile-menu").click();
    const sidebar = page.getByTestId("sidebar");
    for (const href of [
      "/",
      "/signals",
      "/loop",
      "/markets",
      "/data-sources",
      "/portfolio",
      "/orders",
      "/order-entry",
      "/risk",
      "/capital",
      "/strategies",
      "/models",
      "/agents",
      "/system",
      "/integrations",
      "/command/regions",
      "/command/alerts",
      "/intelligence/predictions",
      "/intelligence/correlation",
      "/intelligence/news",
      "/intelligence/regimes",
      "/research/backtesting",
      "/research/quantum",
      "/portfolio/positions",
      "/portfolio/pnl",
      "/risk/limits",
      "/risk/audit",
      "/execution/fills",
      "/execution/venues",
      "/execution/arbitrage",
      "/operations/mesh",
      "/operations/telemetry",
      "/admin/autonomy",
      "/admin/access",
    ]) {
      await expect(
        sidebar.locator(`a[href="${href}"]`),
        `${href} is not reachable from a phone`,
      ).toHaveCount(1);
    }
  });

  test("choosing a section closes the off-canvas panel it was chosen from", async ({ page }) => {
    await servePlatform(page, healthy());
    await page.goto("/");
    await page.getByTestId("mobile-menu").click();
    const capitalLink = page.getByTestId("sidebar").locator('a[href="/capital"]');
    await expect(capitalLink).toBeVisible();
    await capitalLink.click();
    await expect(page).toHaveURL(/\/capital$/);
    await expect(
      page.getByTestId("sidebar-overlay"),
      "the panel stayed over the page it navigated to",
    ).toHaveCount(0);
  });
});

test("the manifest declares an installable app and says paper trading in it", async ({
  request,
}) => {
  const response = await request.get("/manifest.webmanifest");
  expect(response.ok(), "the manifest is not served").toBe(true);
  const manifest = (await response.json()) as {
    display: string;
    start_url: string;
    description: string;
    icons: { sizes: string; purpose?: string }[];
  };
  expect(manifest.display).toBe("standalone");
  expect(manifest.start_url).toContain("/");
  // The install sheet may be the only sentence read before this reaches a home
  // screen. It has to carry the boundary, not just the product name.
  expect(manifest.description.toLowerCase()).toContain("paper");
  expect(manifest.description.toLowerCase()).toContain("no control");
  // Android crops to a circle; without a maskable icon it crops the mark.
  expect(manifest.icons.some((icon) => icon.purpose === "maskable")).toBe(true);
});

test("the offline page shows no figures, because it knows none", async ({ page }) => {
  await page.route(GATEWAY, (route) => route.abort("failed"));
  await page.goto("/offline");
  await expect(page.getByText("This device has no connection to the platform.")).toBeVisible();
  await expect(page.getByTestId("paper-trading-banner")).toBeVisible();
});
