/**
 * `/execution/withdrawals`: the venues the platform withdrew from its own
 * feasibility evidence, read from `GET /venues/withdrawals`.
 *
 * The failures these tests prevent:
 *
 * * **The signature path shown without the refusal beside it.** The route
 *   serves both precisely because the person reading the list is the person
 *   about to call it, and ADR 0065 means this deployment's standing bearer
 *   token attests nobody's presence — so the call cannot be taken. A console
 *   that published the path alone would send an operator into a recovery that
 *   404s on authority rather than on the venue.
 * * **A withdrawal whose record has aged out rendered as a zero share.** "No
 *   evidence" and "evidence no longer retained" are opposite facts about a
 *   fail-closed control and must not look alike.
 * * **An empty withdrawn set read as "nothing was ever withdrawn".** A venue
 *   withdrawn and then reinstated leaves the set empty too;
 *   `withdrawals_recorded` is the only thing that separates them, so it is
 *   asserted to be on the screen and to change the sentence.
 * * **A control that reinstates.** Putting a venue back is two operators'
 *   signatures at a route this console declares no write against.
 *
 * The bodies are built to the contract `qip-api`'s `venue_views.rs`
 * serialises — `{withdrawn[], withdrawals_recorded, reinstatement_path,
 * reinstatement_refusal}` — and not captured from a running process: no
 * deployment of this platform has withdrawn a venue.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

const REFUSAL =
  "signing a venue reinstatement needs an authentication instant and this credential is a standing bearer token";

const WITHDRAWN = {
  withdrawn: [
    {
      venue: "simulated-venue",
      withdrawal: {
        venue: "simulated-venue",
        constraint: "feasibility_lot_size",
        sample: 64,
        count: 41,
        share: 0.640625,
        seams: ["desk", "edge"],
        cycle: 907,
        at: "2026-04-11T06:30:00Z",
      },
      awaiting_countersignature: true,
    },
    {
      // The retention arm: still withdrawn, record no longer readable.
      venue: "forgotten-venue",
      withdrawal: null,
      awaiting_countersignature: false,
    },
  ],
  withdrawals_recorded: 5,
  reinstatement_path: "/api/v1/venues/:venue/reinstatements",
  reinstatement_refusal: REFUSAL,
} as const;

test("a withdrawn venue shows the cluster it was withdrawn on, the seams that contributed, and the refusal a signature would meet", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/venues/withdrawals": WITHDRAWN });
  await page.goto("/execution/withdrawals");

  // The premise: the route's answer landed and both rows rendered.
  const content = page.locator("#content");
  await expect(page.getByTestId("venue-withdrawals-count")).toHaveText("2");
  await expect(page.getByTestId("venue-withdrawals-recorded")).toHaveText("5");
  await expect(page.getByTestId("venue-withdrawals-row")).toHaveCount(2);
  await expect(page.getByTestId("venue-withdrawals-empty")).toHaveCount(0);

  // The cluster, as the review computed it. The share is the platform's own
  // figure; this page divides nothing.
  const withRecord = page.locator('[data-testid="venue-withdrawals-row"][data-venue="simulated-venue"]');
  await expect(withRecord.getByTestId("venue-withdrawals-constraint")).toHaveText("feasibility_lot_size");
  await expect(withRecord.getByTestId("venue-withdrawals-refusal-count")).toHaveText("41");
  await expect(withRecord.getByTestId("venue-withdrawals-sample")).toHaveText("64");
  await expect(withRecord.getByTestId("venue-withdrawals-share")).toHaveText("64.06%");
  await expect(withRecord.getByTestId("venue-withdrawals-seams")).toHaveText("desk, edge");
  await expect(withRecord.getByTestId("venue-withdrawals-awaiting-chip")).toBeVisible();
  await expect(page.getByTestId("venue-withdrawals-awaiting")).toHaveText("1");

  // The path and, beside it, the platform's own refusal for this caller.
  await expect(page.getByTestId("venue-withdrawals-reinstatement-path")).toHaveText(
    `POST ${WITHDRAWN.reinstatement_path}`,
  );
  await expect(page.getByTestId("venue-withdrawals-refusal")).toHaveText(REFUSAL);
  await expect(page.getByTestId("venue-withdrawals-refusal-absent")).toHaveCount(0);

  // The paper declaration, on the page and not only in the chrome.
  await expect(page.getByTestId("venue-withdrawals-paper-label")).toHaveText("PAPER TRADING");
  await expect(content).toContainText("Nothing here withdraws a venue");

  // No control anywhere on the page, and no write attempted.
  await expect(content.locator("button[type=submit], form, input, textarea, select")).toHaveCount(0);
  await expect(
    content.getByRole("button", { name: /^(sign|approve|reinstate|withdraw|permit|submit|buy|sell|trade)/i }),
  ).toHaveCount(0);
  expect(writes, `the page attempted a write: ${writes.join(", ")}`).toEqual([]);
});

test("a withdrawal whose record has aged out says so, and shows no share in its place", async ({ page }) => {
  await servePlatform(page, { ...healthy(), "/venues/withdrawals": WITHDRAWN });
  await page.goto("/execution/withdrawals");

  // The premise: the row exists and is the one with no record. Without this,
  // the absence of a share below would hold for a row that never rendered.
  const forgotten = page.locator('[data-testid="venue-withdrawals-row"][data-venue="forgotten-venue"]');
  await expect(forgotten).toBeVisible();
  await expect(forgotten.getByTestId("venue-withdrawals-record-absent")).toBeVisible();
  await expect(forgotten).toContainText("no longer holds the record");

  // And nothing that could be read as evidence: no share cell at all, rather
  // than a share of zero per cent.
  await expect(forgotten.getByTestId("venue-withdrawals-share")).toHaveCount(0);
  await expect(forgotten).not.toContainText("0.00%");

  // It is still withdrawn, asserted on the chip rather than on the row's
  // prose: the retention sentence contains the word "withdrawn" too, so a
  // `toContainText` on the row would pass with the chip deleted.
  await expect(forgotten.locator("span.chip").first()).toHaveText("withdrawn");
});

test("an empty withdrawn set still distinguishes never-withdrawn from withdrawn-and-restored", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    "/venues/withdrawals": {
      withdrawn: [],
      withdrawals_recorded: 3,
      reinstatement_path: "/api/v1/venues/:venue/reinstatements",
      reinstatement_refusal: null,
    },
  });
  await page.goto("/execution/withdrawals");

  // The premise: the route answered — the record count is on the screen, which
  // an unreachable route could not have put there.
  await expect(page.getByTestId("venue-withdrawals-recorded")).toHaveText("3");
  await expect(page.getByTestId("venue-withdrawals-count")).toHaveText("0");

  const empty = page.getByTestId("venue-withdrawals-empty");
  await expect(empty).toBeVisible();
  await expect(empty).toContainText("Observed, not unread");
  // Three records and an empty set means a venue was put back — the sentence
  // must say that, not "nothing has ever been withdrawn here".
  await expect(empty).toContainText("a venue has been withdrawn before and is no longer");
  await expect(empty).not.toContainText("nothing has ever been withdrawn here");
  await expect(page.getByTestId("venue-withdrawals-list")).toHaveCount(0);

  // A null refusal is the other arm, and it must not read as permission.
  await expect(page.getByTestId("venue-withdrawals-refusal")).toHaveCount(0);
  await expect(page.getByTestId("venue-withdrawals-refusal-absent")).toContainText(
    "still needs a second person",
  );
});

test("the share the platform sent is the share on the screen, even when it disagrees with the count over the sample", async ({
  page,
}) => {
  // This test exists because the suite did not have it and the page's central
  // claim was therefore unguarded. Every other fixture here carries
  // `share === count / sample`, which is what `qip-kernel`'s `venue_review`
  // computes today. That agreement makes "this page renders the review's
  // figure" and "this page divides two numbers it was given" produce identical
  // pixels, so a mutation replacing `row.withdrawal.share` with
  // `row.withdrawal.count / row.withdrawal.sample` was run against the suite
  // and all three tests still passed. A property no fixture can distinguish is
  // not covered by the tests that appear to cover it.
  //
  // So this body is deliberately inconsistent: 41 of 64 is 64.06 per cent, and
  // the share says 87 per cent. That is not a shape the current kernel emits,
  // and the point is that the console must not be the thing that decides so.
  // The denominator is already not a constant of the platform — `VenueCluster`
  // documents the sample as "not a single figure every venue shares", and
  // ADR 0062's echo handling weighs a venue as `refusals + echoes.min(refusals)`
  // rather than as a bare count — so the arithmetic behind `share` can move
  // without the wire shape changing at all. If it does, a console that divides
  // would quietly disagree with the platform that acted, and disagree in the
  // direction of a smaller number, which reads as a venue withdrawn on weaker
  // evidence than it actually was.
  await servePlatform(page, {
    ...healthy(),
    "/venues/withdrawals": {
      withdrawn: [
        {
          venue: "simulated-venue",
          withdrawal: {
            venue: "simulated-venue",
            constraint: "feasibility_lot_size",
            sample: 64,
            count: 41,
            share: 0.87,
            seams: ["desk"],
            cycle: 907,
            at: "2026-04-11T06:30:00Z",
          },
          awaiting_countersignature: false,
        },
      ],
      withdrawals_recorded: 1,
      reinstatement_path: "/api/v1/venues/:venue/reinstatements",
      reinstatement_refusal: REFUSAL,
    },
  });
  await page.goto("/execution/withdrawals");

  const row = page.locator('[data-testid="venue-withdrawals-row"][data-venue="simulated-venue"]');
  // The premise, asserted first: the row rendered and carries both operands.
  // Without this the share assertion below would hold on a page that rendered
  // no row at all.
  await expect(row).toBeVisible();
  await expect(row.getByTestId("venue-withdrawals-refusal-count")).toHaveText("41");
  await expect(row.getByTestId("venue-withdrawals-sample")).toHaveText("64");

  // The property. `87.00%` is the field; `64.06%` is what dividing would give.
  // Both are asserted, because asserting only the first would pass on a page
  // that rendered both figures somewhere.
  await expect(row.getByTestId("venue-withdrawals-share")).toHaveText("87.00%");
  await expect(row.getByTestId("venue-withdrawals-share")).not.toHaveText("64.06%");
  await expect(row).not.toContainText("64.06%");
});
