/**
 * What the front door must be, and must never become again.
 *
 * The site is a licensed template with this platform's content in it, and the
 * failures it has already shipped are all of one kind: something the template
 * supplied that was never true here. A fabricated market ticker. A placeholder
 * phone number. Payment icons on a platform that takes no payments. The
 * vendor's own wordmark in the footer. Image paths that were relative, so the
 * first nested route would have 404'd every picture on it.
 *
 * Each test below exists so a future edit cannot quietly bring one back: a
 * marketing site's lies are discovered by customers, not by compilers.
 */
import { expect, test } from "@playwright/test";

const PORTAL = process.env.NEXT_PUBLIC_ALGORIK_PORTAL_URL ?? "http://127.0.0.1:3400";

/** Every page this site serves. A route added without a line here is a route
 *  nothing crawls, which is how the last set of broken images survived. */
const PAGES = [
  "/",
  "/platform",
  "/technology",
  "/security",
  "/institutional",
  "/developers",
  "/company",
  "/contact",
  "/legal",
  "/legal/terms",
  "/legal/privacy",
  "/legal/risk-disclosures",
];

/**
 * Collect every response the page received that the browser treats as a
 * failure, plus every request that never completed.
 *
 * This is the only way to prove the image fix. A 404'd image still renders —
 * as a broken box — and the document still returns 200, so a test that
 * asserts the page loaded proves nothing at all about what is on it.
 */
async function loadAndRecordFailures(page, path) {
  const failures = [];
  page.on("response", (response) => {
    if (response.status() >= 400) failures.push(`${response.status()} ${response.url()}`);
  });
  page.on("requestfailed", (request) => {
    failures.push(`failed (${request.failure()?.errorText}) ${request.url()}`);
  });
  const response = await page.goto(path, { waitUntil: "networkidle" });
  return { response, failures };
}

test.describe("every page loads everything it asks for", () => {
  for (const path of PAGES) {
    test(`${path} issues no failing request`, async ({ page }) => {
      const { response, failures } = await loadAndRecordFailures(page, path);
      expect(response?.status(), `${path} did not answer 200`).toBe(200);
      // Assert the premise before the property: a page that requested nothing
      // would pass a "no failures" assertion while proving nothing.
      const assets = await page.evaluate(() =>
        performance.getEntriesByType("resource").filter((entry) => entry.name.includes("/assets/") || entry.name.includes("/_next/")).length,
      );
      expect(assets, `${path} requested no assets at all — the premise is wrong`).toBeGreaterThan(0);
      expect(failures, `${path} issued requests that failed:\n${failures.join("\n")}`).toEqual([]);
    });
  }
});

test("every navigation and footer destination resolves", async ({ page, request }) => {
  await page.goto("/");

  const containers = [
    [".header-lower .main-menu", "the desktop navigation"],
    [".mobile-menu", "the mobile menu"],
    ["footer.main-footer", "the footer"],
  ];

  const internal = new Set();
  let external = 0;
  for (const [selector, label] of containers) {
    const container = page.locator(selector).first();
    await expect(container, `${label} is not on the page`).toBeAttached();
    const hrefs = await container.locator("a[href]").evaluateAll((links) =>
      links.map((link) => link.getAttribute("href")),
    );
    expect(hrefs.length, `${label} carries no links`).toBeGreaterThan(0);
    for (const href of hrefs) {
      if (href.startsWith("/")) internal.add(href.split("#")[0] || "/");
      else external += 1;
    }
  }

  // The premise: the chrome really does offer the whole site, not four pages.
  expect(internal.size, "the chrome offers fewer destinations than the site has pages").toBeGreaterThanOrEqual(PAGES.length);

  for (const href of internal) {
    const response = await request.get(href);
    expect(response.status(), `${href} is linked from the chrome and does not resolve`).toBe(200);
  }
  expect(external, "the portal doors are missing from the chrome").toBeGreaterThan(0);
});

test("sign in and get started lead to the portal, from every door", async ({ page }) => {
  // Scoped per container on purpose: the mobile menu and footer carry their
  // own copies of these links, so an unscoped "a sign-in link exists
  // somewhere" passed while the header's own door was broken — the mutation
  // run proved it. Each door is asserted where it stands.
  await page.goto("/");
  for (const [scope, label] of [
    [".header-top", "the header topbar"],
    [".mobile-menu", "the mobile menu"],
    ["footer.main-footer", "the footer"],
  ]) {
    const container = page.locator(scope).first();
    await expect(
      container.locator(`a[href="${PORTAL}/sign-in"]`).first(),
      `Sign In in ${label} does not point at the portal`,
    ).toBeAttached();
    await expect(
      container.locator(`a[href="${PORTAL}/sign-up"]`).first(),
      `Get Started in ${label} does not point at the portal`,
    ).toBeAttached();
  }
});

test("the posture is stated on every page", async ({ page }) => {
  for (const path of PAGES) {
    await page.goto(path);
    await expect(
      page.getByText("PAPER TRADING — simulated execution only").first(),
      `${path} does not state the posture`,
    ).toBeAttached();
  }
});

test("no page offers a control that submits anything", async ({ request }) => {
  // The template shipped a contact form and a search box, neither wired to
  // anything — one still posted to index-3.html. A form that accepts a
  // message and discards it is the failure this site exists not to commit,
  // and the platform's own rules forbid any control that could place an order.
  for (const path of PAGES) {
    const body = await (await request.get(path)).text();
    expect(body, `${path} carries a <form>`).not.toMatch(/<form[\s>]/);
    expect(body, `${path} carries a submit button`).not.toMatch(/type="submit"/);
  }
});

test("nothing the template invented survived", async ({ request }) => {
  for (const path of PAGES) {
    const response = await request.get(path);
    expect(response.status(), `${path} does not resolve`).toBe(200);
    const body = await response.text();
    expect(body, `${path} still carries lorem`).not.toMatch(/lorem ipsum/i);
    expect(body, `${path} still carries the demo ticker`).not.toMatch(/US30/);
    expect(body, `${path} still carries a fabricated address`).not.toMatch(/Brisbane Cir/);
    expect(body, `${path} still carries the placeholder phone number`).not.toMatch(/91 2345 678/);
    expect(body, `${path} shows payment icons on a platform that takes no payments`).not.toMatch(/We Accept/);
    expect(body, `${path} carries the template vendor's brand`).not.toMatch(/[Ff]or[Tt]radex/);
    expect(body, `${path} links to a static template page`).not.toMatch(/href="[^"]*\.html"/);
    expect(body, `${path} links to a template demo route`).not.toMatch(/href="\/?index-[0-9]/);
    // Every asset reference must be rooted. This is the class of defect the
    // crawl above catches at runtime; catching it in the markup as well means
    // a page nobody added to PAGES cannot reintroduce it silently.
    expect(body, `${path} carries a relative asset reference`).not.toMatch(/(?:src|href)="assets\//);
  }
});

test("the disclosures a trading site must carry are published and say the true thing", async ({ request }) => {
  const risk = await (await request.get("/legal/risk-disclosures")).text();
  expect(risk, "the risk disclosures do not say the platform is simulated")
    .toMatch(/Simulated results do not predict real trading/);
  expect(risk, "the risk disclosures do not say no live order is submitted")
    .toMatch(/submits no orders to any live venue/);
  expect(risk, "the draft status is not declared").toMatch(/has not been reviewed by counsel/);

  for (const path of ["/legal/terms", "/legal/privacy"]) {
    const response = await request.get(path);
    expect(response.status(), `${path} does not resolve`).toBe(200);
    expect(await response.text(), `${path} does not declare its draft status`)
      .toMatch(/has not been reviewed by counsel/);
  }
});

test("the old routes still land somewhere real, and an unknown one does not", async ({ request }) => {
  // /about and /error were pages once. /error returned HTTP 200 while
  // displaying "404", which is a page lying about its own status.
  for (const [from, to] of [["/about", "/company"], ["/error", "/"]]) {
    const response = await request.get(from, { maxRedirects: 0 });
    expect(response.status(), `${from} no longer redirects`).toBe(308);
    expect(response.headers()["location"], `${from} redirects somewhere unexpected`).toBe(to);
  }
  const missing = await request.get("/no-such-page-anywhere");
  expect(missing.status(), "an unknown path does not answer 404").toBe(404);
});
