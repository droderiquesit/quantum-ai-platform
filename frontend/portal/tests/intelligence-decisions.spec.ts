/**
 * `/intelligence/decisions`: the decision record behind each proposal.
 *
 * The failure these tests prevent has already happened, on the wire rather
 * than on the page. `GET /proposals` served an id, a status *word*, a leg
 * count, gross, turnover and a rationale. The `Proposal` behind it already
 * carried the control that vetoed it and the reason, the weights each leg was
 * sized between, the reference price it was sized at and what the
 * construction gave up. A console could therefore report that a proposal was
 * `vetoed` and could not say by what or why — the one question an operator
 * asks of a refusal — and blueprint §40.2's "why not the obvious trade?" was
 * scored against the console for an answer the platform had written down.
 *
 * So the assertions below are about fields, not about layout:
 *
 * * the veto's control and reason reaching the screen, asserted with a reason
 *   sentence the page has no knowledge of;
 * * sizing shown as a *movement* — from, to and the move — because a target
 *   weight alone cannot say whether the platform added to a position or cut
 *   it;
 * * an exact decimal rendered as it arrived, never reparsed into a float: the
 *   reference price is asserted to its last digit;
 * * a draft distinguished from a decision, because "nobody has ruled" and
 *   "nobody approved" are different claims;
 * * the paper-trading declaration present, and no control that acts — every
 *   request the page makes is a GET;
 * * an unreachable platform saying so rather than rendering an empty record.
 *
 * The released proposal is a real body, copied from what
 * `qip-api`'s `tests/proposals.rs` produces after one cycle over the kernel's
 * own tape fixture. The vetoed and draft ones are built to the same contract,
 * because no cycle in this repository has yet vetoed a proposal.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform, servePlatformUnreachable } from "./support/platform";

const RELEASED = {
  id: "prop-1",
  status: "released",
  decision: { status: "released", at: "2025-10-09T08:53:20.000Z" },
  created_at: "2025-10-09T08:53:20.000Z",
  as_of: "2025-10-09T08:53:20.000Z",
  equity: "3750000 USD",
  gross: 0.04,
  target_net: 0.04,
  turnover: 0.02,
  estimated_cost_bps: 0.6,
  rationale:
    "expresses 1 approved thesis(es) at 4.0% gross, sized by quadratic_program: solved classically by quadratic_program",
  compromises: [
    "1 name(s) against a 8.0% cap reach 4.0% gross, short of the 95.0% target; the cap wins and the book is deliberately under-invested",
    "obj-AAA: sizing bound narrowed from 8.00% to 4.00% on counterfactual evidence (cap 0.5); gross reaches 4.00%, 4.00% short of the cap-only gross and not reallocated",
  ],
  checks_passed: ["risk-monitor", "compliance"],
  legs: 1,
  leg_detail: [
    {
      instrument: "obj-AAA",
      side: "buy",
      quantity: "1664.007210444",
      reference_price: "90.143840158",
      notional: "150000.000000023",
      current_weight: 0,
      target_weight: 0.04,
      weight_change: 0.04,
      estimated_cost_bps: 15,
      hypotheses: ["hyp-1-obj-AAA"],
    },
  ],
} as const;

const VETO_REASON =
  "max-position-weight would reach 11.2% against a 10.0% cap on obj-BBB";

const VETOED = {
  id: "prop-2",
  status: "vetoed",
  decision: { status: "vetoed", at: "2025-10-09T09:03:20.000Z", by: "risk-monitor", reason: VETO_REASON },
  created_at: "2025-10-09T09:03:20.000Z",
  as_of: "2025-10-09T09:03:20.000Z",
  equity: "3750000 USD",
  gross: 0.11,
  target_net: 0.11,
  turnover: 0.06,
  estimated_cost_bps: 1.4,
  rationale: "expresses 1 approved thesis(es) at 11.2% gross",
  compromises: [],
  checks_passed: ["compliance"],
  legs: 1,
  leg_detail: [
    {
      instrument: "obj-BBB",
      side: "buy",
      quantity: "4600.5",
      reference_price: "91.25",
      notional: "419795.625",
      current_weight: 0.02,
      target_weight: 0.112,
      weight_change: 0.092,
      estimated_cost_bps: 22,
      hypotheses: ["hyp-2-obj-BBB"],
    },
  ],
} as const;

const DRAFT = {
  id: "prop-3",
  status: "draft",
  decision: { status: "draft" },
  created_at: "2025-10-09T09:13:20.000Z",
  as_of: "2025-10-09T09:13:20.000Z",
  equity: "3750000 USD",
  gross: 0,
  target_net: 0,
  turnover: 0,
  estimated_cost_bps: 0,
  rationale: "no thesis cleared the action bar this cycle",
  compromises: [],
  checks_passed: [],
  legs: 0,
  leg_detail: [],
} as const;

const PROPOSALS = { proposals: [RELEASED, VETOED, DRAFT] } as const;

test("a refused proposal names the control that refused it and the reason, and sizing is shown as a movement", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, { ...healthy(), "/proposals": PROPOSALS });
  await page.goto("/intelligence/decisions");

  // The premise: the page rendered and all three records landed. Without
  // this every assertion below could pass over an empty list.
  await expect(page.getByTestId("decisions-page")).toBeVisible();
  await expect(page.getByTestId("decisions-count")).toHaveText("3");
  await expect(page.getByTestId("decision-card")).toHaveCount(3);

  const vetoed = page.locator('[data-testid="decision-card"][data-proposal="prop-2"]');

  // The field the route used to drop. Asserted verbatim: the page has no
  // knowledge of this sentence, so it can only have come off the wire.
  await expect(vetoed.getByTestId("decision-reason")).toHaveText(VETO_REASON);
  // And the control that gave it. "Vetoed" without a name is the state this
  // page exists to end.
  await expect(vetoed.getByTestId("decision-by")).toContainText("risk-monitor");
  await expect(vetoed.getByTestId("decision-status")).toHaveText("vetoed");

  // Sizing as a movement. A target weight alone cannot say whether the
  // platform added to a position or cut it, so all three are on screen.
  const legs = vetoed.getByTestId("decision-legs");
  await expect(legs).toContainText("2.00%");
  await expect(legs).toContainText("11.20%");
  await expect(legs).toContainText("9.20%");

  // Money, to its last digit. A page that parsed this into a float would
  // render 90.14384015800001 or 90.1438402; both are wrong and only an exact
  // assertion catches either. `formatDecimal` groups the whole part for
  // reading and never touches the fraction — it is a string transform and
  // parses nothing — so the quantity is asserted in its grouped spelling
  // with all nine fractional digits still there.
  const released = page.locator('[data-testid="decision-card"][data-proposal="prop-1"]');
  await expect(released.getByTestId("decision-legs")).toContainText("90.143840158");
  await expect(released.getByTestId("decision-legs")).toContainText("1,664.007210444");

  // The optimiser's own account of what it gave up, verbatim and unranked.
  await expect(released.getByTestId("decision-compromises")).toContainText(
    "sizing bound narrowed from 8.00% to 4.00% on counterfactual evidence",
  );
  await expect(released.getByTestId("decision-compromises")).toContainText(
    "the book is deliberately under-invested",
  );

  // A draft has not been reviewed. Saying that is different from naming
  // nobody as its approver.
  const draft = page.locator('[data-testid="decision-card"][data-proposal="prop-3"]');
  await expect(draft.getByTestId("decision-by")).toContainText("not yet reviewed");
  await expect(draft.getByTestId("decision-no-legs")).toBeVisible();

  // Posture, and the boundary. Nothing on this page may act.
  await expect(page.getByTestId("decisions-paper-label")).toHaveText("PAPER TRADING");
  expect(writes).toEqual([]);
});

test("an unreachable platform is said plainly rather than drawn as an empty decision record", async ({
  page,
}) => {
  await servePlatformUnreachable(page);
  await page.goto("/intelligence/decisions");

  // The premise: the page itself rendered, so what follows is about the
  // record and not about a page that failed to mount.
  await expect(page.getByTestId("decisions-page")).toBeVisible();
  // The honest report, asserted on the panel's own words rather than on the
  // first occurrence of "unreachable" anywhere in the shell — the status bar
  // also names the feed state, and a test that matched it would pass with
  // the panel rendering nothing at all.
  const content = page.locator("#content");
  await expect(content).toContainText("The platform could not be reached.");
  // The platform's own detail, not a paraphrase of it.
  await expect(content).toContainText("the platform is not answering on 127.0.0.1:8080");
  // And the distinction that matters: nothing staged is a claim the platform
  // never made, so neither a record nor the empty state may be drawn.
  await expect(page.getByTestId("decision-card")).toHaveCount(0);
  await expect(page.getByTestId("decisions-empty")).toHaveCount(0);
});
