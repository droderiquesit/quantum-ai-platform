/**
 * `/intelligence/explanations`: blueprint §40.2's seven questions, each beside
 * the route that answers it or the statement that none does.
 *
 * The failures these tests prevent:
 *
 * * a question rendered out of the blueprint's order, or one dropped — the
 *   seven are asserted by id in order, from the DOM;
 * * coverage inflated — the three declared absences must render as named
 *   missing endpoints, and the half-answered ones must say which half is
 *   missing beside the half that is shown; a page that upgraded "partial" to
 *   "answered" on a successful request fails the coverage assertion;
 * * a refused estimate counted as a zero — the self-model's uncalibrated row
 *   is counted as a refusal, not omitted;
 * * a simulated figure rendered as money — `would_have_earned` must carry the
 *   simulated flag beside it;
 * * a paraphrased absence — `/pnl`'s reason must be the platform's sentence;
 * * a control that could act, or any write, on a page whose "Acts on" cell is
 *   "Nothing — understanding".
 *
 * Bodies are built to the contracts the other cognition and risk specs use.
 */
import { expect, test } from "@playwright/test";
import { QUESTIONS } from "../src/lib/explanations";
import { healthy, servePlatform } from "./support/platform";

const SELF_MODEL = {
  components: [
    { kind: "detector", key: "momentum", samples: 42, accuracy: "0.6739", calibrated: true },
    { kind: "analyst", key: "macro-desk", samples: 3, accuracy: null, calibrated: false },
    { kind: "rung", key: "deep", samples: 17, accuracy: "0.5217", calibrated: true },
  ],
  minimum_sample: 10,
} as const;

const PRECEDENTS = {
  precedents: [
    { similarity: "0.91", outcome: "resolved", age: "3d" },
    { similarity: "0.74", outcome: "pending", age: "11h" },
  ],
} as const;

const RECALIBRATIONS = {
  limits: "conservative-default",
  open: [
    {
      rule: "order-notional",
      kind: "MaxOrderNotional",
      current_bound: 12.5,
      proposed_bound: 37.75,
      evidence: {
        sample: 40,
        regrets: 31,
        regret_fraction: 0.775,
        would_have_earned: { simulated_value: "8214.6301", simulated: true },
        window: ["2026-03-01T00:00:00Z", "2026-03-08T00:00:00Z"],
        newest: "ord01J9Z0FAKE0000000000TEST",
        scored_orders: ["ord01J9Z0FAKE0000000000TEST"],
      },
      rationale: "31 of 40 paths refused by order-notional would have beaten standing aside",
      outcome: "proposed",
      at: "2026-03-08T09:15:00Z",
    },
  ],
  history: [],
} as const;

const ORDERS = { orders: [], refusals: 7, reconciliation_breaks: [] } as const;

const MODELS = {
  registry: { subject: "models", available: false, reason: "no model registry is composed into this process" },
  observed_use: { agent_runs: 5, model_calls: 12, tokens: 4400, cost_micros: 1234567 },
} as const;

/** The platform's own sentence for the absence, not a paraphrase of it. */
const PNL_REASON =
  "profit, loss and realised alpha are computed by the attributor inside the cycle and are not exposed by the platform";
const PNL = { subject: "attribution", available: false, reason: PNL_REASON } as const;

const BODIES = {
  ...healthy(),
  "/cognition/self-model": SELF_MODEL,
  "/cognition/precedents": PRECEDENTS,
  "/risk/recalibrations": RECALIBRATIONS,
  "/orders": ORDERS,
  "/models": MODELS,
  "/pnl": PNL,
};

test("the seven questions are the blueprint's, in its order, with coverage declared rather than inferred", async ({
  page,
}) => {
  // The premise, from the table the page renders: seven questions, the
  // blueprint's order, and a coverage split that is not all one value — a
  // table of seven "answered" would make the rest of this test vacuous.
  expect(QUESTIONS.map((question) => question.id)).toEqual([
    "position",
    "belief",
    "size",
    "declined",
    "selection",
    "unknowns",
    "cost",
  ]);
  // 1 answered, 5 half, 1 absent. This read 1/3/3 until `GET /proposals`
  // began projecting the decision record the DECIDE stage had always
  // written: the hypotheses each leg expresses, the weights it was sized
  // between, and the control that vetoed it with its reason. That moved
  // *position* and *size* from absent to partial — neither to answered,
  // because a hypothesis id is not a confidence and a weight movement is not
  // the blueprint's four sizing terms shown apart. Only *selection* is still
  // absent. The premise above still holds: this is not a table of sevens.
  const coverage = QUESTIONS.map((question) => question.coverage);
  expect(coverage.filter((value) => value === "answered").length).toBe(1);
  expect(coverage.filter((value) => value === "partial").length).toBe(5);
  expect(coverage.filter((value) => value === "absent").length).toBe(1);

  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await servePlatform(page, BODIES);
  await page.goto("/intelligence/explanations");

  await expect(page.getByRole("heading", { name: "Explanations", exact: true })).toBeVisible();
  await expect(page.getByTestId("explanations-paper-label")).toHaveText("PAPER TRADING");

  // The order, from the DOM and not from the table.
  const rendered = await page
    .getByTestId("explanations-question")
    .evaluateAll((panels) => panels.map((panel) => panel.getAttribute("data-question")));
  expect(rendered).toEqual(QUESTIONS.map((question) => question.id));
  const renderedCoverage = await page
    .getByTestId("explanations-question")
    .evaluateAll((panels) => panels.map((panel) => panel.getAttribute("data-coverage")));
  expect(renderedCoverage).toEqual(coverage);

  // The headline split, as counts of the declared table — every route above
  // answered, and the split is unchanged, because coverage is a statement
  // about what the routes carry and not about this request.
  await expect(page.getByTestId("explanations-answered")).toHaveText("1");
  await expect(page.getByTestId("explanations-partial")).toHaveText("5");
  await expect(page.getByTestId("explanations-absent")).toHaveText("1");

  // The blueprint's "what answers it" cell, verbatim, on every question.
  const answeredBy = await page
    .getByTestId("explanations-answered-by")
    .evaluateAll((cells) => cells.map((cell) => cell.textContent?.trim()));
  expect(answeredBy).toEqual(QUESTIONS.map((question) => question.answeredBy));

  // 6, answered: the self-model's count and its refusals, from the body.
  await expect(page.getByTestId("explanations-self-model-count")).toHaveText("3");
  await expect(page.getByTestId("explanations-self-model-refused")).toHaveText("1");

  // 2, half: the episodes, and the causal-path half named as missing.
  await expect(page.getByTestId("explanations-precedents-count")).toHaveText("2");
  await expect(page.getByTestId("explanations-belief-missing")).toContainText("reaches no route");

  // 4, half: the gate, its regret, the simulated flag on the figure, the
  // refusal count, and the per-order half named as missing.
  await expect(page.getByTestId("explanations-regret")).toHaveCount(1);
  await expect(page.getByTestId("explanations-regret-rule")).toHaveText("order-notional");
  await expect(page.getByTestId("explanations-regret-sample")).toHaveText("40");
  await expect(page.getByTestId("explanations-regret-count")).toHaveText("31");
  await expect(page.getByTestId("explanations-regret-earned")).toHaveText("8,214.6301");
  await expect(page.getByTestId("explanations-regret")).toContainText("simulated");
  await expect(page.getByTestId("explanations-regret")).not.toContainText("NOT FLAGGED");
  await expect(page.getByTestId("explanations-refusals")).toHaveText("7");
  await expect(page.getByTestId("explanations-declined-missing")).toContainText("not per declined order");

  // 7, half: cost in the budget's unit, the platform's own sentence for the
  // missing return, and no ratio anywhere.
  await expect(page.getByTestId("explanations-cost")).toHaveText("1.23");
  await expect(page.getByTestId("explanations-return")).toContainText(PNL_REASON);
  await expect(page.getByTestId("explanations-cost-missing")).toContainText("half a ratio is not a ratio");

  // 5: the one named absence left, with the path such a route would have.
  // Questions 1 and 3 were here too until `GET /proposals` began projecting
  // the decision record; they are asserted as halves above rather than
  // deleted from this test, so the crossing is visible in the diff rather
  // than being a count that quietly shrank.
  const absent = page.locator('[data-testid="explanations-question"][data-coverage="absent"]');
  await expect(absent).toHaveCount(1);
  await expect(absent.nth(0)).toContainText("GET /api/v1/explanations/selection/{strategy} is missing");

  // 1 and 3, now halves: each names the decision record it reads and what it
  // still cannot say. Asserted so that a future edit which quietly upgraded
  // either to "answered" fails here.
  const position = page.locator('[data-testid="explanations-question"][data-question="position"]');
  await expect(position).toHaveAttribute("data-coverage", "partial");
  const size = page.locator('[data-testid="explanations-question"][data-question="size"]');
  await expect(size).toHaveAttribute("data-coverage", "partial");

  // Nothing acts. The chrome's one form is the kill-switch dialog, outside
  // the page.
  const content = page.locator("#content");
  await expect(content.locator("button[type=submit], form, input, textarea, select")).toHaveCount(0);
  await expect(
    content.getByRole("button", { name: /^(grade|explain|recall|submit|buy|sell|trade|invest|size)/i }),
  ).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("a platform that answers nothing leaves coverage as declared and shows no figure", async ({ page }) => {
  // Every route unstubbed: the catch-all answers each as an absence. The
  // page must not read that as seven unanswered questions — coverage is
  // what the routes carry — and must not put a number where none arrived.
  await servePlatform(page, healthy());
  await page.goto("/intelligence/explanations");

  await expect(page.getByTestId("explanations-question")).toHaveCount(7);
  await expect(page.getByTestId("explanations-answered")).toHaveText("1");
  await expect(page.getByTestId("explanations-partial")).toHaveText("5");
  await expect(page.getByTestId("explanations-absent")).toHaveText("1");
  await expect(page.getByTestId("explanations-self-model-count")).toHaveCount(0);
  await expect(page.getByTestId("explanations-regret")).toHaveCount(0);
  await expect(page.getByTestId("explanations-cost")).toHaveCount(0);
  // The absences the platform itself stated are rendered as such.
  await expect(page.locator('[data-question="unknowns"]')).toContainText("no stub for /cognition/self-model");
  await expect(page.getByTestId("explanations-paper-label")).toHaveText("PAPER TRADING");
});
