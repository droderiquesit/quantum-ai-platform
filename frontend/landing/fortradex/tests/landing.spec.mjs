/**
 * What the front door must be, and must never become again.
 *
 * The template arrived with a fabricated market ticker, a placeholder phone
 * number, payment icons and lorem routes. Each was removed deliberately, and
 * each of these tests exists so a future edit cannot quietly bring one back:
 * a marketing site's lies are discovered by customers, not by compilers.
 */
import { expect, test } from "@playwright/test";

const PORTAL = process.env.NEXT_PUBLIC_ALGORIK_PORTAL_URL ?? "http://127.0.0.1:3400";

test("the front door states the paper posture and carries nothing fabricated", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByText("Paper trading — simulated execution only").first()).toBeVisible();
  // The demo shipped invented US30 prices, a fake phone number and payment
  // icons. Their absence is asserted, not assumed.
  const body = await page.content();
  expect(body, "a fabricated market ticker is back").not.toMatch(/US30/);
  expect(body, "the placeholder phone number is back").not.toMatch(/91 2345 678/);
  expect(body, "payment icons are back on a platform that takes no payments").not.toMatch(/We Accept/);
  await expect(page.locator("h2", { hasText: "Paper-trading discipline" }).first()).toBeVisible();
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
    ["footer, .main-footer", "the footer"],
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

test("every navigation destination exists and carries no demo copy", async ({ page, request }) => {
  for (const path of ["/platform", "/about", "/contact"]) {
    const response = await request.get(path);
    expect(response.status(), `${path} does not resolve`).toBe(200);
    const body = await response.text();
    expect(body, `${path} still carries lorem`).not.toMatch(/lorem ipsum/i);
    expect(body, `${path} still carries the demo ticker`).not.toMatch(/US30/);
    expect(body, `${path} still carries a fabricated address`).not.toMatch(/Brisbane Cir/);
  }
  // The premise: the nav genuinely links to these three.
  await page.goto("/");
  for (const path of ["/platform", "/about", "/contact"]) {
    await expect(page.locator(`a[href="${path}"]`).first()).toBeAttached();
  }
});
