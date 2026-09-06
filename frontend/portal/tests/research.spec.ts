/**
 * The three pages that stopped illustrating and started reading the research
 * routes: `/intelligence/predictions`, `/intelligence/correlation` and
 * `/research/backtesting`.
 *
 * Each used to render a seeded illustration under a SIMULATED DATA banner
 * because no route carried the fact. `GET /api/v1/predictions`,
 * `/correlation` and `/backtests` now do, and each page renders what the
 * route answers and nothing else. The failures these tests prevent:
 *
 * * a page that shows a forecast, a coefficient or a curve the platform never
 *   sent — asserted by the absence of the simulated banner, by empty bodies
 *   rendering as stated empties, and by the backtesting page drawing no
 *   equity curve when the route says none is kept;
 * * a refusal rendered as a blank — asserted by the platform's own reason
 *   appearing verbatim, and by the evidence beside it (the instruments
 *   observed, the declared minimum) being on the screen;
 * * a correlation matrix read as time-aligned — asserted by the alignment
 *   caveat, in the route's own words, being visible whenever a matrix is.
 *
 * The empty bodies below are copied verbatim from a `qip-api` process at
 * `paper_trading` on 2026-09-04 after three cycles with no observations fed
 * in; the populated ones follow the shapes `qip-api/tests/research.rs`
 * asserts on.
 */
import { expect, test } from "@playwright/test";
import { healthy, servePlatform } from "./support/platform";

// --- bodies copied from a running qip-api -----------------------------------

const NO_CALIBRATION =
  "no calibration has been computed. The LEARN stage grades a claim only once its horizon has passed and the platform's own series can settle it informatively; until one has, the platform has written down confidences and has not yet learned whether they held.";

const PREDICTIONS_EMPTY = {
  as_of_cycle: 3,
  window: 1024,
  held: 0,
  open: 0,
  resolved: 0,
  instruments: {},
  calibration: { subject: "calibration", available: false, reason: NO_CALIBRATION },
};

const TOO_FEW_SERIES =
  "fewer than two instruments have enough closes for a correlation to be estimated. A coefficient over a handful of prints is a number with no evidence behind it, so the view refuses below the stated minimum rather than reporting one; the instruments the platform has seen, and how many closes each holds, are listed so the shortfall is a fact rather than a blank.";

const CORRELATION_REFUSED = {
  subject: "correlation",
  available: false,
  reason: TOO_FEW_SERIES,
  as_of_cycle: 3,
  minimum_closes: 30,
  instruments_observed: [],
};

const NO_EQUITY_CURVE =
  "no equity curve is kept. The ledger records returns as the evidence a gate saw and the band it produced, not a path a page could plot; a curve drawn here would be one nobody computed.";

const NO_DEFLATED_SHARPE =
  "the holdout gate computes the deflated Sharpe at admission and records it in its `deflated_sharpe_above_selection` finding, served under each ledger entry's gate; the ledger keeps no numeric copy and this process does not recompute one. The band's `sharpe` is the annualised holdout Sharpe the gate admitted on.";

const BACKTESTS_EMPTY = {
  strategies: [],
  trial_book: { attached: true, durable: true, families: [] },
  deflated_sharpe: { available: false, reason: NO_DEFLATED_SHARPE },
  equity_curve: { available: false, reason: NO_EQUITY_CURVE },
};

// --- populated bodies, in the shapes research.rs asserts on ----------------

const ALIGNMENT =
  "by position from the most recent close backwards; the tape keeps closes without their instants, so two series are aligned by count rather than by timestamp";

const CORRELATION_AVAILABLE = {
  available: true,
  as_of_cycle: 1,
  statistic: "pearson correlation of simple returns between consecutive closes",
  alignment: ALIGNMENT,
  window_closes: 60,
  window_returns: 59,
  minimum_closes: 30,
  instruments: ["obj-AAA", "obj-BBB", "obj-DDD"],
  matrix: {
    "obj-AAA": { "obj-AAA": 1, "obj-BBB": -1, "obj-DDD": null },
    "obj-BBB": { "obj-AAA": -1, "obj-BBB": 1, "obj-DDD": null },
    "obj-DDD": { "obj-AAA": null, "obj-BBB": null, "obj-DDD": null },
  },
  excluded: [
    { instrument: "obj-CCC", closes: 10, reason: "fewer than the 30 closes the estimate requires" },
  ],
  undefined: [
    { a: "obj-AAA", b: "obj-DDD", reason: "zero return variance inside the window on at least one side" },
  ],
};

const PREDICTIONS_POPULATED = {
  as_of_cycle: 2,
  window: 1024,
  held: 2,
  open: 1,
  resolved: 1,
  // ZZZ first on purpose: the page must keep the platform's order, not sort.
  instruments: {
    "obj-ZZZ": {
      predictions: [
        {
          hypothesis: "hyp-zzz",
          cycle: 1,
          statement: "obj-ZZZ closes higher within the horizon",
          metric: "close:obj-ZZZ",
          direction: "up",
          confidence: 0.71,
          expected_move_bps: 45,
          horizon_seconds: 86_400,
          made_at: "2026-09-04T15:00:00Z",
          resolves_at: "2026-09-05T15:00:00Z",
          state: "held",
          scored_at: "2026-09-05T15:01:00Z",
        },
      ],
    },
    "obj-AAA": {
      predictions: [
        {
          hypothesis: "hyp-aaa",
          cycle: 2,
          statement: "obj-AAA closes lower within the horizon",
          metric: "close:obj-AAA",
          direction: "down",
          confidence: 0.58,
          expected_move_bps: -30,
          horizon_seconds: 3_600,
          made_at: "2026-09-04T16:00:00Z",
          resolves_at: "2026-09-04T17:00:00Z",
          state: "open",
          scored_at: null,
        },
      ],
    },
  },
  calibration: {
    available: true,
    evaluations_in_window: 1,
    material: false,
    report: { evaluated: 1, brier_score: 0.0841 },
  },
};

const BACKTESTS_POPULATED = {
  strategies: [
    {
      strategy: "AAA",
      family: "research-tests",
      cell: "london-1",
      venue: "XNYS",
      stage: "holdout",
      registered_at: "2026-09-04T15:00:00Z",
      holdout: {
        submitted: true,
        observations: 400,
        trials_this_run: 12,
        periods_per_year: 252,
        cross_validation: { folds: 5, observations: 400, purged: 40, embargoed: 20 },
        leakage_findings: [],
      },
      trial_account: {
        on_evidence: false,
        reason:
          "the submitted evidence carries no trial account of its own; the gate charges the family's trial book directly, and the lifetime count it deflated against is `family_lifetime_trials`",
      },
      family_lifetime_trials: 12,
      holdout_band: {
        present: true,
        sharpe: 1.234,
        lower: 0.456,
        upper: 2.012,
        standard_error: 0.397,
        observations: 400,
        periods_per_year: 252,
        trials: 12,
        method: "lo (2002) standard error, annualised",
        as_of: "2026-09-04T15:00:00Z",
      },
      ledger: [
        {
          from: "candidate",
          to: "holdout",
          at: "2026-09-04T15:00:00Z",
          approver: null,
          rationale: "the holdout gate passed",
          gate: {
            stage: "holdout",
            passed: true,
            findings: [
              {
                check: "deflated_sharpe_above_selection",
                passed: true,
                detail: "deflated Sharpe 0.91 clears the selection threshold over 12 trials",
              },
            ],
          },
        },
      ],
    },
  ],
  trial_book: { attached: true, durable: false, families: [{ family: "research-tests", lifetime_trials: 12 }] },
  deflated_sharpe: { available: false, reason: NO_DEFLATED_SHARPE },
  equity_curve: { available: false, reason: NO_EQUITY_CURVE },
};

// --- predictions -------------------------------------------------------------

test("the predictions page renders an empty predictions body as a stated zero with the calibration reason verbatim, and no illustration", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/predictions": PREDICTIONS_EMPTY });
  await page.goto("/intelligence/predictions");

  const empty = page.getByTestId("predictions-empty");
  await expect(empty).toBeVisible();
  // The premise beside the conclusion: three cycles ran, so "no claim" is a
  // fact about the loop rather than a page that has not loaded.
  await expect(empty).toContainText("3 cycle(s)");
  await expect(page.getByTestId("prediction-row")).toHaveCount(0);

  // The platform's reason, in its words, not this console's.
  await expect(page.getByTestId("calibration-unavailable")).toContainText(NO_CALIBRATION);

  // The illustration is gone, in both of its labels, and so are its
  // fictional tickers.
  await expect(page.getByTestId("simulated-banner")).toHaveCount(0);
  await expect(page.getByText("simulated data")).toHaveCount(0);
  await expect(page.getByText(/EQ-AURORA|FX-KESTREL|CM-THALASSA/)).toHaveCount(0);
});

test("the predictions page renders each instrument in the platform's own order with the claim's own direction, confidence and verdict", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/predictions": PREDICTIONS_POPULATED });
  await page.goto("/intelligence/predictions");

  const instruments = page.getByTestId("prediction-instrument");
  await expect(instruments).toHaveCount(2);
  // Served order, not alphabetical: a page that sorted would put AAA first.
  await expect(instruments.nth(0)).toContainText("obj-ZZZ");
  await expect(instruments.nth(1)).toContainText("obj-AAA");

  const rows = page.getByTestId("prediction-row");
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText("held");
  await expect(rows.nth(0)).toContainText("0.71");
  await expect(rows.nth(0)).toContainText("+45 bps");
  await expect(rows.nth(1)).toContainText("open");
  await expect(rows.nth(1)).toContainText("down");
  await expect(rows.nth(1)).toContainText("0.58");

  const report = page.getByTestId("calibration-report");
  await expect(report).toBeVisible();
  await expect(report).toContainText("0.0841");
  await expect(page.getByTestId("calibration-unavailable")).toHaveCount(0);
});

// --- correlation -------------------------------------------------------------

test("the correlation page renders the platform's refusal verbatim with the evidence beside it, and no matrix", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/correlation": CORRELATION_REFUSED });
  await page.goto("/intelligence/correlation");

  await expect(page.getByTestId("correlation-refusal")).toContainText(TOO_FEW_SERIES);
  // The evidence: the minimum the route declared, and the (empty) list of
  // what it observed — stated as empty, not left blank.
  await expect(page.getByText("30 closes per instrument")).toBeVisible();
  await expect(page.getByTestId("correlation-none-observed")).toBeVisible();
  await expect(page.getByTestId("correlation-observed-row")).toHaveCount(0);

  // No matrix and no pair: a refusal draws nothing.
  await expect(page.locator("table[aria-label*='Pearson correlation']")).toHaveCount(0);
  await expect(page.getByTestId("correlation-pair-row")).toHaveCount(0);
  await expect(page.getByTestId("simulated-banner")).toHaveCount(0);
  await expect(page.getByText("simulated data")).toHaveCount(0);
});

test("the correlation page shows the matrix with the alignment caveat in the route's words, nulls as unmeasured, and the exclusions by name", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/correlation": CORRELATION_AVAILABLE });
  await page.goto("/intelligence/correlation");

  // Premise: the matrix is on the screen.
  const matrix = page.locator("table[aria-label*='Pearson correlation']");
  await expect(matrix).toBeVisible();

  // The caveat a reader most needs: aligned by position, not by timestamp,
  // in the route's own words.
  const alignment = page.getByTestId("correlation-alignment");
  await expect(alignment).toBeVisible();
  await expect(alignment).toContainText(ALIGNMENT);

  // One defined pair, at the value the platform served; the null pairs are
  // not ranked as zero.
  const pairs = page.getByTestId("correlation-pair-row");
  await expect(pairs).toHaveCount(1);
  await expect(pairs.first()).toContainText("obj-AAA × obj-BBB");
  await expect(pairs.first()).toContainText("-1.00");

  // The exclusion and the undefined pair, each with the platform's reason.
  const excluded = page.getByTestId("correlation-excluded-row");
  await expect(excluded).toHaveCount(1);
  await expect(excluded.first()).toContainText("obj-CCC");
  await expect(excluded.first()).toContainText("fewer than the 30 closes");
  await expect(page.getByTestId("correlation-undefined-row")).toContainText("zero return variance");
  await expect(page.getByText("59 returns")).toBeVisible();
});

// --- backtesting -------------------------------------------------------------

test("the backtesting page renders an empty ledger as a stated zero and the equity-curve reason verbatim, drawing no curve", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/backtests": BACKTESTS_EMPTY });
  await page.goto("/research/backtesting");

  await expect(page.getByTestId("backtests-empty")).toBeVisible();
  await expect(page.getByTestId("backtest-strategy")).toHaveCount(0);

  // The trial book is real even when empty: attached and durable, as served.
  const book = page.getByTestId("trial-book");
  await expect(book).toBeVisible();
  await expect(book).toContainText("no holdout evaluation has been charged");

  // The reasons, verbatim, and no chart anywhere in their place.
  await expect(page.getByTestId("equity-curve-absent")).toContainText(NO_EQUITY_CURVE);
  await expect(page.getByTestId("deflated-sharpe-absent")).toContainText(NO_DEFLATED_SHARPE);
  await expect(page.locator("svg[role='img']")).toHaveCount(0);
  await expect(page.getByText(/harbour-lantern/)).toHaveCount(0);
  await expect(page.getByTestId("simulated-banner")).toHaveCount(0);
  await expect(page.getByText("simulated data")).toHaveCount(0);
});

test("the backtesting page renders a strategy's holdout evidence, band and gate findings as served, labelled PAPER TRADING, and still draws no curve", async ({
  page,
}) => {
  await servePlatform(page, { ...healthy(), "/backtests": BACKTESTS_POPULATED });
  await page.goto("/research/backtesting");

  const strategy = page.getByTestId("backtest-strategy");
  await expect(strategy).toHaveCount(1);
  await expect(strategy).toContainText("AAA");
  await expect(strategy).toContainText("research-tests");
  await expect(strategy).toContainText("XNYS");
  await expect(strategy).toContainText("stage · holdout");
  // Posture beside the venue and the rung, as the rule requires.
  await expect(strategy).toContainText("PAPER TRADING");

  // The band as served: centre and bounds, not recomputed.
  await expect(strategy).toContainText("1.234");
  await expect(strategy).toContainText("0.456 … 2.012");
  await expect(strategy).toContainText("400");

  // The gate's own deflated-Sharpe finding, under the move that carried it.
  const finding = page.getByTestId("gate-finding");
  await expect(finding).toHaveCount(1);
  await expect(finding).toContainText("deflated_sharpe_above_selection");
  await expect(finding).toContainText("clears the selection threshold");
  await expect(page.getByTestId("trial-account-absent")).toContainText("family_lifetime_trials");

  // Evidence present, and still no curve: the route says none is kept.
  await expect(page.getByTestId("equity-curve-absent")).toContainText(NO_EQUITY_CURVE);
  await expect(page.locator("svg[role='img']")).toHaveCount(0);
});
