/**
 * The chrome: what is true on every route, whatever the platform says.
 *
 * These are the assertions that must not depend on a page, a preference or a
 * state. A paper-trading declaration that is conditional is a declaration that
 * will one day be absent on the one screen where it mattered.
 */
import { expect, test } from "@playwright/test";
import { NAV_ITEMS } from "../src/lib/nav";
import { RISK_BODY, healthy, servePlatform, servePlatformUnreachable } from "./support/platform";

// Derived, not typed. The list was hand-maintained under a comment saying "so
// a new section cannot ship a route the declaration is missing from" — which
// required someone to remember, and covered 15 of the 33 routes the map
// declares. `/offline` is outside the map and is named separately.
const ROUTES = [...NAV_ITEMS.map((item) => item.href), "/offline"] as const;

/** Every page that renders the autonomy level. Verified by grep, not assumed. */
const POSTURE_ROUTES = ["/", "/risk", "/system", "/risk/audit", "/admin/autonomy"] as const;

test("the route list under test is the console's own map", () => {
  // The premise for every loop below. A derived list that silently resolved to
  // nothing would make three tests pass by iterating zero routes.
  expect(ROUTES.length).toBeGreaterThan(30);
  expect(ROUTES).toContain("/risk");
});

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

test("a panel that shows autonomy shows the trading posture beside it", async ({ page }) => {
  // The banner is inside `<main>` but outside `#content`, so scoping to
  // `#content` asserts the *page* carries the label — not the chrome above it.
  // Without that scope this test passes on every route in the console and
  // guards nothing, which is precisely how `/risk` came to render
  // "Autonomy: paper_trading / Live: no" with no paper token on the page.
  //
  // The match is the delimited token. `system/page.tsx` already contains the
  // substring "paper" inside a hint while carrying no label, so
  // `contains("paper")` here would be satisfied by the defect.
  await servePlatform(page, { ...healthy(), "/risk": RISK_BODY });
  for (const route of POSTURE_ROUTES) {
    await page.goto(route);
    const content = page.locator("#content");
    // The premise: this page really does render the posture. Otherwise the
    // assertion below would hold just as well for a page that rendered nothing.
    await expect(content, `no autonomy rendered on ${route}`).toContainText(/autonomy/i);
    await expect(
      content.getByText("PAPER TRADING", { exact: true }).first(),
      `posture on ${route} carries no PAPER TRADING label`,
    ).toBeVisible();
  }
});
