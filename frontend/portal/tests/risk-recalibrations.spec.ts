/**
 * `/risk/recalibrations`: the proposals the platform generated about its own
 * risk bounds, read from `GET /risk/recalibrations` and rendered as answered.
 *
 * The failures these tests prevent:
 *
 * * **A bound differenced in the browser.** The two bounds arrive as the
 *   platform's own numbers and a console that subtracted them, or expressed
 *   the move as a percentage, would be doing risk arithmetic in the one place
 *   the domain rule forbids it. Asserted by the exact delta of the fixture's
 *   two bounds being absent from the page.
 * * **`enacted` read as "the limit moved".** Two signatures emit a limit set
 *   as an artefact for a deployment to commit; the process that served the
 *   request keeps the bounds it booted with. Asserted on the text the chip
 *   carries wherever the word appears.
 * * **A simulated figure rendered as money.** `would_have_earned` is what
 *   refused paths would have made in a world that did not happen. Asserted by
 *   the `SIMULATED` marker beside it.
 * * **An empty list rendered as a table with no rows**, or as a gap rather
 *   than as the observed zero it is.
 * * **A control on a page in the risk section.** Loosening a limit is two
 *   operators' signatures at a route this console declares no write against.
 *
 * The bodies are built to the contract `qip-api`'s `rule_views.rs` serialises
 * — `{limits, open[], history[]}` with `RecalibrationProposal` carrying
 * `{rule, kind, current_bound, proposed_bound, evidence, rationale, outcome,
 * at}` — and not captured from a running process: no deployment of this
 * platform has generated a recalibration.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

/**
 * Two bounds chosen so that neither is a substring of the other and so that
 * their difference, 25.25, appears nowhere else in the fixture. The
 * "nothing is differenced" assertion is only worth anything because of that:
 * a delta that happened to equal a figure the page legitimately shows would
 * make the assertion unfalsifiable.
 */
const CURRENT_BOUND = 12.5;
const PROPOSED_BOUND = 37.75;
const DELTA_IF_COMPUTED = "25.25";

const OPEN_PROPOSAL = {
  rule: "order-notional",
  kind: "MaxOrderNotional",
  current_bound: CURRENT_BOUND,
  proposed_bound: PROPOSED_BOUND,
  evidence: {
    sample: 40,
    regrets: 31,
    regret_fraction: 0.775,
    would_have_earned: { simulated_value: "8214.6301", simulated: true },
    window: ["2026-03-01T00:00:00Z", "2026-03-08T00:00:00Z"],
    newest: "ord01J9Z0FAKE0000000000TEST",
    scored_orders: ["ord01J9Z0FAKE0000000000TEST", "ord01J9Z0FAKE0000000001TEST"],
  },
  rationale:
    "31 of 40 paths refused by order-notional between 2026-03-01T00:00:00Z and 2026-03-08T00:00:00Z would have beaten standing aside",
  outcome: "proposed",
  at: "2026-03-08T09:15:00Z",
} as const;

const ENACTED_PROPOSAL = {
  ...OPEN_PROPOSAL,
  rule: "cash-buffer",
  kind: "MinCashBuffer",
  outcome: "enacted",
  at: "2026-02-02T11:00:00Z",
} as const;

const RECALIBRATIONS = {
  limits: "conservative-default",
  open: [OPEN_PROPOSAL],
  history: [ENACTED_PROPOSAL, OPEN_PROPOSAL],
} as const;

test("an open proposal shows both bounds as the platform sent them, its own rationale, and no arithmetic of this console's", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/risk/recalibrations": RECALIBRATIONS });
  await page.goto("/risk/recalibrations");

  // The premise: the route's answer landed. Without it every absence below
  // holds equally for a page that never rendered.
  const content = page.locator("#content");
  await expect(page.getByTestId("recalibrations-limits")).toHaveText("conservative-default");
  await expect(page.getByTestId("recalibrations-open-count")).toHaveText("1");
  await expect(page.getByTestId("recalibrations-open-proposal")).toHaveCount(1);

  // Both bounds, exactly as the numbers arrived.
  await expect(page.getByTestId("recalibrations-current-bound")).toHaveText(String(CURRENT_BOUND));
  await expect(page.getByTestId("recalibrations-proposed-bound")).toHaveText(String(PROPOSED_BOUND));

  // And nothing derived from them. The delta appears nowhere, which is what
  // distinguishes "renders two bounds" from "reasons about two bounds".
  await expect(content).not.toContainText(DELTA_IF_COMPUTED);

  // The rationale is the platform's sentence, verbatim — not a paraphrase.
  await expect(page.getByTestId("recalibrations-rationale")).toHaveText(OPEN_PROPOSAL.rationale);

  // The simulated figure carries its flag. A counterfactual earning rendered
  // bare would read as money the desk made.
  const earned = page.getByTestId("recalibrations-would-have-earned");
  await expect(earned).toContainText("8,214.6301");
  await expect(earned).toContainText("SIMULATED");

  // The paper declaration, on the page and not only in the chrome.
  await expect(page.getByTestId("recalibrations-paper-label")).toHaveText("PAPER TRADING");
  await expect(content).toContainText("Nothing here loosens a limit");

  // No control. The chrome's one form is the kill-switch dialog, inside
  // <dialog> and outside #content.
  await expect(content.locator("button[type=submit], form, input, textarea, select")).toHaveCount(0);
  await expect(
    content.getByRole("button", { name: /^(sign|approve|loosen|enact|propose|withdraw|submit|buy|sell|trade)/i }),
  ).toHaveCount(0);
  expect(writes, `the page attempted a write: ${writes.join(", ")}`).toEqual([]);
});

test("an enacted proposal says the artefact was emitted and this process kept the bounds it booted with", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/risk/recalibrations": RECALIBRATIONS });
  await page.goto("/risk/recalibrations");

  // The premise: the history table rendered both records, and one of them is
  // the enacted one. A page with no enacted row would satisfy the assertion
  // below vacuously.
  await expect(page.getByTestId("recalibrations-history-row")).toHaveCount(2);
  const enacted = page.locator('[data-testid="recalibrations-history-row"][data-outcome="enacted"]');
  await expect(enacted).toHaveCount(1);
  await expect(enacted).toContainText("cash-buffer");

  // The word "enacted" is only safe next to what it actually means.
  const chip = enacted.locator("span.chip");
  await expect(chip).toHaveAttribute(
    "title",
    /this process keeps the limits it booted with/,
  );
});

test("no open proposal is the observed empty state, with the record count that tells it from an unread route", async ({
  page,
}) => {
  await servePlatform(page, {
    ...healthy(),
    "/risk/recalibrations": { limits: "conservative-default", open: [], history: [] },
  });
  await page.goto("/risk/recalibrations");

  // The premise: the route answered — the limit set's name is on the screen,
  // which an unreachable route could not have put there.
  await expect(page.getByTestId("recalibrations-limits")).toHaveText("conservative-default");

  await expect(page.getByTestId("recalibrations-open-empty")).toBeVisible();
  await expect(page.getByTestId("recalibrations-open-empty")).toContainText("Observed, not unread");
  await expect(page.getByTestId("recalibrations-open-list")).toHaveCount(0);
  await expect(page.getByTestId("recalibrations-history-table")).toHaveCount(0);
  await expect(page.getByTestId("recalibrations-history-empty")).toBeVisible();
});
