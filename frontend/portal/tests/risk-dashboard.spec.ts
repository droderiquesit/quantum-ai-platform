/**
 * `/risk`: exposure against concentration limits, the halt state, and the two
 * things `GET /risk` says outright it cannot measure in this process.
 *
 * The failures these tests prevent:
 *
 * * **a breach decided in the browser.** The utilisation bar used to paint
 *   itself red from `share > limit` — the risk engine's own comparison, made a
 *   second time in the one place this platform forbids risk logic, on two
 *   floats that had already crossed out of `Decimal`. `bucket.breached` is the
 *   platform's verdict and the only one on screen. Asserted with two buckets
 *   whose served verdict deliberately disagrees with the comparison, one in
 *   each direction, so a console that re-derived would fail both ways rather
 *   than only when the two happen to differ;
 * * **a flat book and a platform nobody could ask rendering alike.** "No
 *   exposure on any axis" is the answer an operator is happiest to believe and
 *   the most dangerous one to guess at. Four states — nothing has arrived yet,
 *   the route answered and the book is flat, the platform was unreachable, and
 *   the credential was refused — each get their own block, and each test
 *   asserts the other three are absent;
 * * **a section the platform says it cannot measure being shown as a zero.**
 *   `limit_utilisation` and `tail_risk` answer `available: false` with a
 *   reason, and the page renders the reason. A limit shown at 0% is headroom
 *   nobody measured;
 * * **an absence with no name.** The halt *record* — when each halt began, who
 *   cleared it, on what basis — is not served; `/risk` answers the current trip
 *   and a count. It is rendered through `NOT_YET_SERVED`, the console's
 *   vocabulary for a missing route, rather than left as a blank panel;
 * * **a control.** This page filters and reads. It writes nothing, and there is
 *   no order-entry path anywhere on it.
 *
 * Bodies follow `GET /risk` as `qip-api/src/routes.rs::risk` serialises it.
 */
import { expect, test, type Page } from "@playwright/test";
import {
  GATEWAY,
  RISK_BODY,
  healthy,
  servePlatform,
  servePlatformUnreachable,
} from "./support/platform";

/** The header's posture pair, so a posture label has a posture to sit beside. */
const POSTURE = {
  "/autonomy": { level: "paper_trading", ceiling: "paper_trading", live: false, history: [] },
} as const;

const GOVERNANCE_CLEAN = { agents: 7, findings: [] } as const;

/**
 * A book with exposure in it, and two buckets whose served `breached` verdict
 * deliberately contradicts `share > limit`.
 *
 * The real platform computes `breached` as that comparison, so these bodies
 * are counterfactual on purpose — exactly as `treasury-mandates.spec.ts`
 * serves an `investable` that is not `capital - floor`. The property under
 * test is that the console renders the platform's verdict rather than making
 * its own, and a fixture where the two agree cannot tell the difference.
 */
const RISK_OBSERVED = {
  exposure: {
    available: true,
    buckets: [
      {
        axis: "instrument",
        bucket: "ZZZ-FICTIONAL-ALPHA",
        gross: "1000",
        net: "-250",
        // 10% of a 30% limit. A console deriving for itself paints this clear.
        share: 0.1,
        limit: 0.3,
        breached: true,
      },
      {
        axis: "venue",
        bucket: "simulated-venue",
        gross: "500",
        net: "500",
        // 42% of a 25% limit. A console deriving for itself paints this red.
        share: 0.42,
        limit: 0.25,
        breached: false,
      },
    ],
  },
  concentrations: { available: true, findings: [] },
  kill_switch: {
    halted: false,
    halted_scopes: [],
    tripped_by: "",
    reason: "",
    clearances: 0,
  },
  limit_utilisation: RISK_BODY.limit_utilisation,
  tail_risk: RISK_BODY.tail_risk,
} as const;

/** The same route, answering with a book that is genuinely flat. */
const RISK_FLAT = {
  ...RISK_OBSERVED,
  exposure: { available: true, buckets: [] },
} as const;

/** A platform that refuses the console's credential on every route. */
async function serveDenied(page: Page, detail: string): Promise<void> {
  await page.route(GATEWAY, async (route) => {
    await route.fulfill({
      status: 403,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify({ error: detail }),
    });
  });
}

/**
 * A platform that has been asked and has not yet answered.
 *
 * The fourth state, and the only one that cannot be produced by a body: the
 * request is held open until the returned function is called, so the loading
 * block is on screen for as long as the test needs it rather than for one
 * frame. Release before the test ends so no handler is left mid-flight.
 */
async function serveStalled(page: Page): Promise<() => void> {
  let release: () => void = () => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(GATEWAY, async (route) => {
    await held;
    await route.fulfill({
      status: 200,
      headers: { "x-qip-gateway": "upstream", "content-type": "application/json" },
      body: JSON.stringify({ subject: "stalled", available: false, reason: "released" }),
    });
  });
  return release;
}

test("the exposure table paints the platform's breach verdict and never re-derives it", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    ...POSTURE,
    "/risk": RISK_OBSERVED,
    "/system/governance": GOVERNANCE_CLEAN,
  });
  await page.goto("/risk");

  // The premise: the table rendered with both buckets in it, so the
  // assertions below are about what it painted and not about an empty page.
  await expect(page.getByTestId("risk-exposure-table")).toBeVisible();
  await expect(page.getByTestId("risk-exposure-row")).toHaveCount(2);

  const alpha = page.locator('[data-testid="risk-exposure-row"][data-axis="instrument"]');
  const venue = page.locator('[data-testid="risk-exposure-row"][data-axis="venue"]');
  await expect(alpha).toHaveCount(1);
  await expect(venue).toHaveCount(1);

  // The platform said breached on a bucket 10% into a 30% limit, and clear on
  // one 42% into a 25% limit. Both are the platform's verdict, both are what
  // the row and the bar carry, and both are the opposite of what the browser
  // would conclude.
  await expect(alpha).toHaveAttribute("data-breached", "true");
  await expect(alpha.locator('[role="img"]')).toHaveAttribute("data-breached", "true");
  await expect(venue).toHaveAttribute("data-breached", "false");
  await expect(venue.locator('[role="img"]')).toHaveAttribute("data-breached", "false");

  // The platform's own figures, rendered rather than recomputed.
  await expect(alpha).toContainText("ZZZ-FICTIONAL-ALPHA");
  await expect(alpha).toContainText("10.00%");
  await expect(alpha).toContainText("30.00%");

  // The posture declaration is on the page, not only in the chrome.
  await expect(
    page.locator("#content").getByText("PAPER TRADING", { exact: true }).first(),
  ).toBeVisible();
});

test("the breached-only filter selects on the platform's verdict, not on the numbers beside it", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    ...POSTURE,
    "/risk": RISK_OBSERVED,
    "/system/governance": GOVERNANCE_CLEAN,
  });
  await page.goto("/risk");

  // Premise: both rows are there before the filter is touched.
  await expect(page.getByTestId("risk-exposure-row")).toHaveCount(2);

  await page.getByRole("button", { name: "Breached only" }).click();

  // One row survives, and it is the one the platform called breached — the one
  // whose share is *below* its limit. A filter reading the numbers would keep
  // the other.
  await expect(page.getByTestId("risk-exposure-row")).toHaveCount(1);
  await expect(page.getByTestId("risk-exposure-row")).toHaveAttribute("data-axis", "instrument");
});

test("the platform's two unmeasurable sections are rendered as its own refusals, never as zeroes", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    ...POSTURE,
    "/risk": RISK_OBSERVED,
    "/system/governance": GOVERNANCE_CLEAN,
  });
  await page.goto("/risk");

  // Premise: the risk body landed, so these blocks stand beside data.
  await expect(page.getByTestId("risk-exposure-table")).toBeVisible();

  const unmeasured = page.getByTestId("risk-unmeasured");
  await expect(unmeasured.locator("[data-state-block=not-available]")).toHaveCount(2);
  await expect(unmeasured).toContainText("The platform serves no limits in this deployment.");
  await expect(unmeasured).toContainText("which would read as headroom the platform has not measured");
  await expect(unmeasured).toContainText("The platform serves no tail_risk in this deployment.");
  await expect(unmeasured).toContainText("capability gate");

  // Nothing anywhere reports a limit at zero per cent.
  await expect(page.getByTestId("risk-unmeasured")).not.toContainText("0.00%");
});

test("the halt record and the compliance register are named as missing routes, with nothing in their place", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    ...POSTURE,
    "/risk": RISK_OBSERVED,
    "/system/governance": GOVERNANCE_CLEAN,
  });
  await page.goto("/risk");

  const absences = page.getByTestId("risk-absences");
  await expect(absences.locator("[data-state-block=endpoint-missing]")).toHaveCount(2);
  await expect(absences).toContainText("GET /api/v1/kill-switch/history is missing");
  await expect(absences).toContainText(
    "A count cannot say when a halt began, how long it ran, or who cleared it",
  );
  await expect(absences).toContainText("GET /api/v1/compliance is missing");
  await expect(absences).toContainText("This console does not synthesise data it was not given");

  // And the one place the platform does answer about obligations.
  const link = page.getByTestId("risk-to-compliance");
  await expect(link).toHaveAttribute("href", "/compliance");
  await link.click();
  await expect(page.getByTestId("compliance-page")).toBeVisible();
});

test("nothing has arrived yet, a flat book, an unreachable platform and a refused credential are four different blocks", async ({
  page,
}) => {
  // 1. Nothing has arrived yet. The request is held open, so the loading block
  //    is the state under test rather than a frame between two others.
  const release = await serveStalled(page);
  await page.goto("/risk");
  const loading = page.locator('[aria-busy="true"]').first();
  await expect(loading).toBeVisible();
  await expect(loading).toContainText("loading");
  await expect(page.getByTestId("risk-exposure-empty")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);
  release();
  await page.unrouteAll({ behavior: "ignoreErrors" });

  // The premise for the three below: the same page with exposure in it renders
  // the table, so each absence is an absence of data and not a broken page.
  await servePlatform(page, {
    ...healthy(),
    ...POSTURE,
    "/risk": RISK_OBSERVED,
    "/system/governance": GOVERNANCE_CLEAN,
  });
  await page.goto("/risk");
  await expect(page.getByTestId("risk-exposure-table")).toBeVisible();
  await expect(page.getByTestId("risk-exposure-empty")).toHaveCount(0);
  await page.unrouteAll({ behavior: "ignoreErrors" });

  // 2. The route answered and the book is flat.
  await servePlatform(page, {
    ...healthy(),
    ...POSTURE,
    "/risk": RISK_FLAT,
    "/system/governance": GOVERNANCE_CLEAN,
  });
  await page.goto("/risk");
  const flat = page.getByTestId("risk-exposure-empty");
  await expect(flat).toBeVisible();
  await expect(flat).toContainText("No exposure is recorded on the all axis.");
  await expect(flat).toContainText("A read that succeeded and found a flat book");
  await expect(page.getByTestId("risk-exposure-table")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);
  await page.unrouteAll({ behavior: "ignoreErrors" });

  // 3. A platform nothing could reach: its own block, in its own colour.
  await servePlatformUnreachable(page);
  await page.goto("/risk");
  const unreachable = page.locator("[data-state-block=disconnected]").first();
  await expect(unreachable).toBeVisible();
  await expect(unreachable).toContainText("The platform could not be reached.");
  await expect(unreachable).toContainText("is not current");
  await expect(page.getByTestId("risk-exposure-empty")).toHaveCount(0);
  await expect(page.getByTestId("risk-exposure-table")).toHaveCount(0);
  await expect(page.locator("[data-state-block=refused]")).toHaveCount(0);
  await page.unrouteAll({ behavior: "ignoreErrors" });

  // 4. A credential the route refuses. `/risk` requires the viewer role, and a
  //    deployment whose console credential lacks it meets this rather than an
  //    empty dashboard.
  await serveDenied(page, "this route requires the viewer role");
  await page.goto("/risk");
  const denied = page.locator("[data-state-block=refused]").first();
  await expect(denied).toBeVisible();
  await expect(denied).toContainText("may not read /api/v1/risk");
  await expect(denied).toContainText("this route requires the viewer role");
  await expect(page.getByTestId("risk-exposure-empty")).toHaveCount(0);
  await expect(page.getByTestId("risk-exposure-table")).toHaveCount(0);
  await expect(page.locator("[data-state-block=disconnected]")).toHaveCount(0);
});

test("the risk dashboard writes nothing and offers no way to compose an order", async ({ page }) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, {
    ...healthy(),
    ...POSTURE,
    "/risk": RISK_OBSERVED,
    "/system/governance": GOVERNANCE_CLEAN,
  });
  await page.goto("/risk");

  // Premise: the page is fully rendered, so "no control" is a statement about
  // a page that drew rather than one that did not.
  await expect(page.getByTestId("risk-exposure-table")).toBeVisible();

  const content = page.locator("#content");
  await expect(content.locator("form, input, button[type=submit]")).toHaveCount(0);
  await expect(
    content.getByRole("button", {
      name: /^(buy|sell|submit|send|place|order|trade|execute|cancel|amend|liquidate|hedge)/i,
    }),
  ).toHaveCount(0);

  // Exercise both controls the page does have. Neither is a write.
  await page.getByRole("button", { name: "Breached only" }).click();
  await page.getByLabel("Filter by axis").selectOption("venue");
  await expect(page.getByTestId("risk-exposure-row")).toHaveCount(0);
  expect(writes).toEqual([]);
});
