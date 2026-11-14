# ADR 0499: A Granger edge carries its fitted slope, and the NOW Brain's causal estimator joins the standard ensemble on that slope, at the edge's lag, on returns only

- **Status**: **Accepted 2026-11-14 under the owner's 2026-10-31 delegation**
  (a session decides the in-repository design question it meets, by ADR),
  session t50-36. The delegation's limits hold: it sets no figure the owner
  has not stated, changes no requirement's wording, adds no dependency and
  touches no paper-trading layer. The one constant it introduces — sixteen
  promoted returns kept per variable so a parent can be read as of its lag —
  is a working-set bound, not a limit; nothing sizes, vetoes or trades on it.
  Amends ADR 0498's "What remains".
- **Date**: 2026-11-14
- **Rows**: R12-WORLD-3078.

## Context

ADR 0498 put the neural and symbolic estimators in `NowBrain::standard` and
left `CausalFilter` out, for the reason an earlier skeptic gave: a causal
edge's `strength` is, on a Granger edge, the test's partial R² — a measure
of fit, not a slope — and the filter applied no lag. Fused by default, its
candidate would have pulled every promoted belief, volatility and spread
included, toward `R²·parent`, which estimates nothing. The kernel also
signed hand-asserted mechanism claims and passed their claimed fraction as
if it were fitted.

The regression already produced the right number and threw it away:
`GrangerCausalityTest::coefficient` is the coefficient of the effect's
return on the cause's lagged return, used only for its sign.

## Decision

1. **`CausalEdge::slope: Option<f64>`**, set through `with_slope`, which
   refuses a NaN or a zero. `establish_temporal_precedence_controlling_for`
   records `test.coefficient` on every edge it writes. Optional and skipped
   when absent, so every edge recorded before it decodes unchanged and every
   hand-asserted claim honestly carries none.
2. **The kernel passes only fitted edges.** `Platform::now_causal_parents`
   passes an edge's slope (signed by the fit) and its lag, and leaves out
   every edge without a slope. A claimed supply-chain link no longer reaches
   a state estimator dressed as a regression.
3. **`CausalParent::lag`, and a lagged read.** The brain keeps the last
   sixteen promoted beliefs of each return and reads a parent's belief as
   last promoted on a reading observed no later than the lag before the
   child's reading. A newer parent belief is a pairing the regression never
   fitted. A zero lag is not read.
4. **Returns only (`now::CAUSAL_VARIABLE`).** The slopes are fitted on
   returns. The restriction is held at three seams — what the history
   keeps, what `causal_beliefs` returns, and what `CausalFilter::propose`
   answers — so a regression in any one does not open it.
5. **`CausalFilter` joins `NowBrain::standard`**, making seven.
6. **Several parents are averaged, never summed.** Each edge's slope comes
   from its own one-parent regression, so each parent is a separate
   prediction of the same return. The first registration summed them, and
   two correlated parents each fitted at 0.8 predicted 1.6 times a shared
   move. On the shipped fast-brain tape that moved every causal child's
   belief far enough that all ten sized theses fell on one side of the
   self-model row, and `qip-fastbrain`'s tape test failed its premise. The
   predictions are now combined by precision, with their spread added to
   the variance, as the ensemble combines candidates.

## What it costs

- One more worker per variable per step, and sixteen `(time, mean,
  variance)` triples per return variable, dropped on expiry and bounded by
  the store's own variable bound.
- A priced instrument with a fitted parent now has a promoted return that
  leans toward `slope × lagged parent`, which is what the lattice reads. The
  candidate is the regression's own conditional prediction without its
  intercept and the effect's own lag term: for daily returns both are near
  zero, but on a strongly autocorrelated series the candidate understates
  the effect's own persistence. Its variance carries no residual term, so
  it is if anything over-confident; the ensemble averages means with equal
  weight, so the over-confidence widens nothing and narrows only the
  `within` term by one member's share.
- A hand-asserted mechanism claim contributes nothing to the NOW state. It
  still drives every other consumer of the causal graph.

## What would make this wrong

- The temporal-precedence pass admits spurious edges at a rate that makes
  the causal member a source of noise: disputes naming `causal-parents` that
  resolve against it. The bar (`p < 0.01`, partial R² ≥ 0.02, 60
  observations) is the pass's, not this record's; if that happens, the bar
  is what to revisit.
- A writer starts recording a slope fitted on something other than returns,
  or at a lag other than the edge's. The slope's doc comment states its
  units; a second writer must keep them.
