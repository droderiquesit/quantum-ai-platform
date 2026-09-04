/**
 * The telemetry surface, and what it must not let a reader believe.
 *
 * An empty panel that looks healthy is worse than one that says no collector
 * is attached. Nothing is scraped and no alert policy is evaluated, and a page
 * of counters with a mild disclaimer reads as an instrumented platform.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform, servePlatformUnreachable } from "./support/platform";

/**
 * `GET /api/v1/system/metrics`, served so the counter panel actually renders.
 *
 * Without it `servePlatform`'s catch-all answers `available: false` and the
 * panel renders a stated absence instead of counters — which would make the
 * premise below a claim about a page that never drew the thing the page is
 * for. The values are distinctive so the premise cannot be satisfied by a
 * zero that some other panel happened to render.
 */
const METRICS = {
  "/system/metrics": {
    cycles: 41_237,
    events_logged: 91_733,
    opportunities_queued: 12,
    proposals: 7,
    orders: 3,
    fills: 2,
    refusals: 1,
    live_fills: false,
  },
};

test("the telemetry page states that nothing collects these counters", async ({ page }) => {
  await servePlatform(page, { ...healthy(), ...METRICS });
  await page.goto("/operations/telemetry");

  // The premise: the page rendered its counters. A page that failed to render
  // would satisfy nothing below and could still be mistaken for a pass. The
  // assertion is on a value only this stub could have produced, not on the
  // panel's static heading, which is on the screen either way.
  await expect(page.getByText("41,237")).toBeVisible();
  await expect(page.getByText("91,733")).toBeVisible();

  const gap = page.getByTestId("collection-gap");
  await expect(gap).toBeVisible();
  // The claim, not a synonym of it: that nothing collects, and that the gate
  // keeping every alert policy unstored is named so a reader can go check.
  await expect(gap).toContainText("nothing collects them");
  await expect(gap).toContainText("workload_metrics_exist");
  await expect(gap).toContainText("pages no one");
});

test("a telemetry page with no counters to show says so rather than drawing a flat line", async ({
  page,
}) => {
  // The failure this prevents: an unreachable platform rendering as a chart of
  // zeroes, which on a telemetry page reads as a quiet system rather than as a
  // blind one.
  await servePlatformUnreachable(page);
  await page.goto("/operations/telemetry");
  await expect(page.locator("body")).toContainText(/unreachable|not answering/i);
  await expect(page.getByText("nothing observed yet").first()).toBeVisible();
  // And the standing declaration is still there when the platform is gone.
  await expect(page.getByTestId("collection-gap")).toContainText("nothing collects them");
});
