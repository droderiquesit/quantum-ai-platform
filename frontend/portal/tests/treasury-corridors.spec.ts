/**
 * `/treasury/corridors`: the corridor registry and the destination allowlist
 * from `GET /corridors`, as records with a lifecycle stage and caps.
 *
 * The failures these tests prevent:
 *
 * * "no registry" rendered as "an empty registry" — asserted by each
 *   registry's not-held state carrying the platform's reason verbatim; the
 *   two are different facts and only the second is a control doing its job;
 * * a usable-from instant computed by the page — asserted by the cell
 *   carrying the registry's own `usable_from`, and a revoked destination
 *   reading "never" whatever its signature date;
 * * a page in the treasury section without the paper-trading declaration, or
 *   with a control that proposes, reviews, signs, activates, suspends or
 *   revokes.
 *
 * The first body is the example in `backend/crates/apps/qip-api/ROUTES-LEDGER.md`
 * — what every current deployment answers. The second is built to the
 * contract's stated record shapes; no deployment holds a registry, so it
 * cannot be captured.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

const NO_CORRIDOR_REGISTRY =
  "no corridor registry is held in this process. A corridor is the signed record of where capital may go and under what caps; the kernel composes no treasury and has proposed, reviewed or signed none, so there is no corridor to list — not an empty registry that admits nothing, but no registry at all.";

const NO_DESTINATION_ALLOWLIST =
  "no destination allowlist is held in this process. A destination is proposed, verified by a person with the institution and signed before it can be used; none has been.";

const CORRIDORS_NOT_HELD = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  corridors: { held: false, reason: NO_CORRIDOR_REGISTRY, records: [] },
  destinations: { held: false, reason: NO_DESTINATION_ALLOWLIST, records: [] },
} as const;

const CORRIDORS_HELD = {
  posture: "PAPER TRADING",
  served_at: "2025-10-09T08:53:20Z",
  corridors: {
    held: true,
    reason: null,
    records: [
      {
        id: "lon-usd-to-custodian",
        source: { region: "GB", currency: "USD", venue: "sim-venue" },
        source_class: "fiat_at_institution_of_record",
        kind: "institution_approval_flow",
        destination: { asset: "USD", address: "custodian-account-0001" },
        caps: {
          max_per_transfer: "10000",
          max_per_hour: "20000",
          max_per_day: "50000",
          max_cumulative: "1000000",
          min_interval_seconds: 3600,
          permitted_hours: { start: 8, end: 17 },
        },
        purpose: "pre-position settlement cash at the custodian ahead of the London open",
        stage: "time_delayed",
        proposed_by: "treasury-analyst",
        proposed_at: "2025-10-08T09:00:00Z",
        reviewed_by: "treasury-reviewer",
        reviewed_at: "2025-10-08T10:00:00Z",
        signed: true,
        activation_at: "2025-10-09T11:00:00Z",
      },
    ],
  },
  destinations: {
    held: true,
    reason: null,
    records: [
      {
        asset: "USD",
        address: "custodian-account-0001",
        status: "signed",
        proposed_by: "treasury-analyst",
        proposed_at: "2025-10-08T09:00:00Z",
        usable_from: "2025-10-09T11:00:00Z",
      },
      {
        asset: "USD",
        address: "old-account-0000",
        status: "revoked",
        proposed_by: "treasury-analyst",
        proposed_at: "2025-09-01T09:00:00Z",
        usable_from: "2025-09-02T09:00:00Z",
      },
      {
        asset: "EUR",
        address: "custodian-account-0002",
        status: "verified",
        proposed_by: "treasury-analyst",
        proposed_at: "2025-10-09T08:00:00Z",
        usable_from: null,
      },
    ],
  },
} as const;

test("registries the process does not hold are said not to be held, in the platform's words, and the page holds no control that moves capital", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/corridors": CORRIDORS_NOT_HELD });
  await page.goto("/treasury/corridors");

  const content = page.locator("#content");
  await expect(page.getByRole("heading", { name: "Corridors" })).toBeVisible();

  await expect(page.getByTestId("corridors-not-held")).toContainText(NO_CORRIDOR_REGISTRY);
  await expect(page.getByTestId("destinations-not-held")).toContainText(NO_DESTINATION_ALLOWLIST);
  await expect(page.getByTestId("corridor")).toHaveCount(0);
  await expect(page.getByTestId("destination-registry")).toHaveCount(0);

  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.getByTestId("treasury-body-posture")).toHaveText("PAPER TRADING");
  await expect(content).toContainText("Nothing on this page can move capital.");

  await expect(content.locator("button[type=submit], form")).toHaveCount(0);
  const formsOutsideDialog = await page
    .locator("form")
    .evaluateAll((forms) => forms.filter((form) => form.closest("dialog") === null).length);
  expect(formsOutsideDialog).toBe(0);
  await expect(
    content.getByRole("button", { name: /^(propose|review|approve|sign|activate|suspend|revoke|transfer|submit)/i }),
  ).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("a held registry renders each corridor's stage and caps, and each destination's usable-from as the registry states it", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/corridors": CORRIDORS_HELD });
  await page.goto("/treasury/corridors");

  // The premise: both registries are held and their records are drawn.
  await expect(page.getByTestId("corridors-not-held")).toHaveCount(0);
  await expect(page.getByTestId("corridor-count")).toHaveText("1");
  await expect(page.getByTestId("destination")).toHaveCount(3);

  const corridor = page.getByTestId("corridor");
  await expect(corridor).toContainText("lon-usd-to-custodian");
  await expect(corridor).toContainText("time_delayed");
  await expect(corridor).toContainText("signature on record");
  await expect(page.getByTestId("corridor-activation-at")).toContainText("activates at 2025-10-09 11:00:00.000");
  const caps = page.getByTestId("corridor-caps");
  await expect(caps).toContainText("10,000");
  await expect(caps).toContainText("1,000,000");
  await expect(caps).toContainText("1h");
  await expect(caps).toContainText("08:00–17:00 UTC");

  // Usable-from, per destination, in document order: the registry's instant,
  // never for a revoked one, and not yet for one that is only verified.
  const usable = page.getByTestId("destination-usable-from");
  await expect(usable.nth(0)).toHaveText("2025-10-09 11:00:00.000");
  await expect(usable.nth(1)).toContainText("never");
  await expect(usable.nth(2)).toContainText("not yet");

  await expect(page.getByTestId("treasury-paper-label")).toHaveText("PAPER TRADING");
  await expect(page.locator("#content").locator("button[type=submit], form")).toHaveCount(0);
});
