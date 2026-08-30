/**
 * The public site: reachable, branded, honest.
 *
 * The honesty test is the one that earns its keep. Marketing copy gets edited
 * under deadline by people who never read the safety rules, and a promise of
 * returns on a trading platform's public page is a regulatory event, not a
 * wording preference. `forbiddenClaims` in @algorik/brand is the rule;
 * this suite is what makes it bind.
 */
import { expect, test } from "@playwright/test";
import { auditCopy } from "@algorik/brand";

const MARKETING_ROUTES = [
  "/welcome",
  "/platform",
  "/technology",
  "/security",
  "/institutional",
  "/developers",
  "/company",
  "/legal/terms",
  "/legal/privacy",
  "/legal/risk-disclosures",
] as const;

test("every public page renders, carries the brand, and states the paper boundary", async ({
  page,
}) => {
  for (const route of MARKETING_ROUTES) {
    await page.goto(route);
    await expect(
      page.getByRole("img", { name: "Algorik" }).first(),
      `no Algorik lockup on ${route}`,
    ).toBeVisible();
    await expect(
      page.getByText("Simulated execution only — no live orders are submitted."),
      `the paper-trading line is missing from ${route}`,
    ).toBeVisible();
  }
});

test("no public page makes a claim the platform may not make", async ({ page }) => {
  for (const route of MARKETING_ROUTES) {
    await page.goto(route);
    const copy = await page.locator("body").innerText();
    // Premise first: an empty body would pass the audit vacuously.
    expect(copy.length, `${route} rendered no copy at all`).toBeGreaterThan(200);
    const findings = auditCopy(copy);
    expect(
      findings,
      `${route} carries a forbidden claim: ${findings.map((f) => `"${f.claim}" (${f.instead})`).join("; ")}`,
    ).toEqual([]);
  }
});

test("the landing header's calls to action lead to the real doors", async ({ page }) => {
  await page.goto("/welcome");
  await page.getByRole("link", { name: /get started/i }).first().click();
  await expect(page).toHaveURL(/\/sign-up$/);
  await page.goto("/welcome");
  await page.getByRole("link", { name: /^sign in$/i }).first().click();
  await expect(page).toHaveURL(/\/sign-in$/);
});

test("the legal drafts say they are drafts", async ({ page }) => {
  // A draft terms page that presents as effective is a legal exposure the
  // repository created for its owner. The notice is load-bearing.
  for (const route of ["/legal/terms", "/legal/privacy", "/legal/risk-disclosures"]) {
    await page.goto(route);
    await expect(
      page.getByText(/draft for review/i).first(),
      `${route} does not declare itself a draft`,
    ).toBeVisible();
  }
});
