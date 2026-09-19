/**
 * `/capital/exploration`: what the platform is spending to learn, read from
 * `GET /exploration` and rendered as answered.
 *
 * The failures these tests prevent:
 *
 * * **A share defaulted where none is declared.** A ledger with no desk
 *   mandate has no exploration ceiling and the route answers an absence with
 *   a reason rather than a number. A console that rendered `0` there would
 *   show a mandate that explores nothing, which is a different fact from a
 *   mandate that does not exist.
 * * **Held, committed and spent summed in the browser.** Any two of them
 *   added together count every open probe twice. Asserted by the exact sums
 *   of the fixture's figures being absent from the page — and the fixture is
 *   chosen so no sum coincides with a figure the page legitimately shows.
 * * **An empty open list read as "nothing was ever bought".** Every probe
 *   settling leaves the list empty too; `opened_total` is what separates the
 *   two and is asserted to change the sentence.
 * * **A per-kind learning table drawn from guessed columns.** The route
 *   reports the table unavailable at the tip this page was written against;
 *   the reason must be on the screen and nothing in its place. Where the
 *   route serves a body instead, it is rendered verbatim and labelled unread,
 *   not through columns transcribed from a kernel struct.
 * * **A control that adjusts the share.** The share is a mandate term
 *   changed through the capital path by an authenticated operator; this
 *   console declares no such write.
 *
 * The bodies are built to the contract the `exploration` handler in
 * `qip-api`'s `routes.rs` serialises — `{share, held, committed, spend,
 * open_count, opened_total, settled_total, abandoned_total,
 * subjects_forgotten, open[], learned}` — and not captured from a running
 * process: no deployment of this platform has opened a probe.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

const LEARNED_REASON =
  "the per-kind information gain is keyed by a capital-crate enum this process may not name; see the handler's documentation";

const SHARE_ABSENT_REASON = "the ledger holds no desk mandate, so no exploration ceiling has been declared";

/**
 * Three figures whose pairwise and total sums — 1250, 325.5 and 1325.5 —
 * appear nowhere else in the fixture. The "nothing is summed" assertion is
 * only worth anything because of that.
 */
const HELD = "1000";
const COMMITTED = "250";
const SPEND = "75.5";
const SUMS_IF_COMPUTED = ["1,250", "1250", "325.5", "1,325.5", "1325.5"] as const;

const OPEN_PROBES = [
  {
    id: "prb-simulated-0001",
    kind: "unfamiliar_venue",
    learns: "a venue's fill and adverse-selection behaviour",
    subject: "simulated-venue",
    maximum_loss: "125.25",
    uncertainty_at_open: 0.62,
    opened_at: "2026-04-11T06:30:00Z",
    expires_at: "2026-04-12T06:30:00Z",
  },
  {
    id: "prb-simulated-0002",
    kind: "capacity_at_size",
    learns: "where a strategy's capacity actually decays",
    subject: "sim-family-alpha",
    maximum_loss: "80",
    uncertainty_at_open: 0.41,
    opened_at: "2026-04-11T07:00:00Z",
    expires_at: "2026-04-13T07:00:00Z",
  },
] as const;

const DECLARED = {
  share: { declared: true, share: "0.025" },
  held: HELD,
  committed: COMMITTED,
  spend: SPEND,
  open_count: 2,
  opened_total: 7,
  settled_total: 4,
  abandoned_total: 1,
  subjects_forgotten: 3,
  open: OPEN_PROBES,
  learned: { subject: "learned", available: false, reason: LEARNED_REASON },
} as const;

test("the declared share, the three figures kept apart, every open probe as its question, and the learning table's absence are all rendered — and nothing acts", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/exploration": DECLARED });
  await page.goto("/capital/exploration");

  // The premise: the route's answer landed and both probes rendered.
  const content = page.locator("#content");
  await expect(page.getByTestId("exploration-open-count")).toHaveText("2");
  await expect(page.getByTestId("exploration-probe")).toHaveCount(2);
  await expect(page.getByTestId("exploration-open-empty")).toHaveCount(0);

  // The ceiling, as the mandate states it, with no absence block beside it.
  await expect(page.getByTestId("exploration-share")).toHaveText("0.025");
  await expect(page.getByTestId("exploration-share-declared")).toHaveText("declared");
  await expect(page.getByTestId("exploration-share-absent")).toHaveCount(0);

  // Three numbers, each the field it came from, and none of their sums.
  await expect(page.getByTestId("exploration-held")).toHaveText("1,000");
  await expect(page.getByTestId("exploration-committed")).toHaveText("250");
  await expect(page.getByTestId("exploration-spend")).toHaveText("75.5");
  for (const sum of SUMS_IF_COMPUTED) {
    await expect(content, `a sum the page must not compute is on the screen: ${sum}`).not.toContainText(sum);
  }

  // The account's counters.
  await expect(page.getByTestId("exploration-opened-total")).toHaveText("7");
  await expect(page.getByTestId("exploration-settled-total")).toHaveText("4");
  await expect(page.getByTestId("exploration-abandoned-total")).toHaveText("1");
  await expect(page.getByTestId("exploration-forgotten")).toHaveText("3");

  // One probe, field for field: the question first, then the terms.
  const probe = page.locator('[data-testid="exploration-probe"][data-probe="prb-simulated-0001"]');
  await expect(probe.getByTestId("exploration-probe-learns")).toHaveText(
    "a venue's fill and adverse-selection behaviour",
  );
  await expect(probe.getByTestId("exploration-probe-kind")).toHaveText("unfamiliar_venue");
  await expect(probe.getByTestId("exploration-probe-subject")).toHaveText("simulated-venue");
  await expect(probe.getByTestId("exploration-probe-bound")).toHaveText("125.25");
  await expect(probe.getByTestId("exploration-probe-uncertainty")).toHaveText("0.62");
  await expect(probe).toHaveAttribute("data-expires", "2026-04-12T06:30:00Z");
  await expect(probe.getByTestId("exploration-probe-expires")).toContainText("2026-04-12");

  // The half the route does not serve: its reason, and nothing in its place.
  await expect(page.getByTestId("exploration-learned-absent")).toBeVisible();
  await expect(page.getByTestId("exploration-learned-reason")).toHaveText(LEARNED_REASON);
  await expect(page.getByTestId("exploration-learned-unread")).toHaveCount(0);

  // The paper declaration, on the page and not only in the chrome.
  await expect(page.getByTestId("exploration-paper-label")).toHaveText("PAPER TRADING");
  await expect(content).toContainText("Nothing here adjusts the share");

  // No control anywhere on the page, and no write attempted.
  await expect(content.locator("button[type=submit], form, input, textarea, select")).toHaveCount(0);
  await expect(
    content.getByRole("button", { name: /^(adjust|set|open|settle|fund|submit|buy|sell|trade)/i }),
  ).toHaveCount(0);
  expect(writes, `the page attempted a write: ${writes.join(", ")}`).toEqual([]);
});

test("a mandate that declares no share is rendered as the platform's stated absence, and no default is shown in its place", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    "/exploration": {
      ...DECLARED,
      share: { subject: "exploration_share", available: false, reason: SHARE_ABSENT_REASON },
    },
  });
  await page.goto("/capital/exploration");

  // The premise: the route answered — a figure only the body could have put
  // on the screen is there — so the absence below is the route's and not an
  // unreachable platform's.
  await expect(page.getByTestId("exploration-held")).toHaveText("1,000");

  const absent = page.getByTestId("exploration-share-absent");
  await expect(absent).toBeVisible();
  await expect(page.getByTestId("exploration-share-reason")).toHaveText(SHARE_ABSENT_REASON);
  await expect(absent).toContainText("No default is rendered");

  // And no share figure at all — not the declared arm, and not a zero.
  await expect(page.getByTestId("exploration-share")).toHaveCount(0);
  await expect(page.getByTestId("exploration-share-declared")).toHaveCount(0);
});

test("an empty open list over a book that has opened probes says they settled, not that nothing was ever bought", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    "/exploration": { ...DECLARED, open_count: 0, open: [] },
  });
  await page.goto("/capital/exploration");

  // The premise: the route answered and the total that decides the sentence
  // is on the screen.
  await expect(page.getByTestId("exploration-opened-total")).toHaveText("7");
  await expect(page.getByTestId("exploration-open-count")).toHaveText("0");

  const empty = page.getByTestId("exploration-open-empty");
  await expect(empty).toBeVisible();
  await expect(empty).toContainText("Observed, not unread");
  await expect(empty).toContainText("every question bought has settled or was abandoned");
  await expect(empty).not.toContainText("never bought a question");
  await expect(page.getByTestId("exploration-open-list")).toHaveCount(0);
});

test("an empty open list over a book that has never opened a probe says so", async ({ page }) => {
  await servePlatform(page, {
    ...healthy(),
    "/exploration": {
      ...DECLARED,
      open_count: 0,
      opened_total: 0,
      settled_total: 0,
      abandoned_total: 0,
      open: [],
    },
  });
  await page.goto("/capital/exploration");

  await expect(page.getByTestId("exploration-opened-total")).toHaveText("0");
  const empty = page.getByTestId("exploration-open-empty");
  await expect(empty).toBeVisible();
  await expect(empty).toContainText("never bought a question");
  await expect(empty).not.toContainText("settled or was abandoned");
});

test("a learning table the route does serve is rendered verbatim and labelled unread, not through guessed columns", async ({
  page,
}) => {
  // Not a shape the current handler emits — its own test pins `learned` to
  // an absence — so this is the arm that guards against the console
  // inventing the platform's answer once the route closes the half. The key
  // is deliberately not a `ProbeKind` token, so a page that rendered a table
  // keyed on the enum would have nothing to show and this test would notice.
  await servePlatform(page, {
    ...healthy(),
    "/exploration": {
      ...DECLARED,
      learned: { "not-a-probe-kind": { probed: 3, probed_gain: 0.9 } },
    },
  });
  await page.goto("/capital/exploration");

  // The premise: the route answered.
  await expect(page.getByTestId("exploration-held")).toHaveText("1,000");

  const unread = page.getByTestId("exploration-learned-unread");
  await expect(unread).toBeVisible();
  await expect(unread).toContainText("not been taught its shape");
  await expect(page.getByTestId("exploration-learned-raw")).toContainText("not-a-probe-kind");
  await expect(page.getByTestId("exploration-learned-absent")).toHaveCount(0);
});
