# 0054 — Temporal precedence is the first real causal-edge establishment method

**Status:** *accepted*, 2026-09-12.

**Relates to:** blueprint §9.1/§9.2 (`docs/architecture/algorik-blueprint-v10.1-source.md`),
`docs/DELIVERY-STATUS.md` §9.1, §9.2, §9.3 and the "Interlocks worth reading
as pairs" causal-graph bullet, and ADR 0002/ADR 0009 (the two-dependency
policy this record stays inside of).

**Does not touch:** `backend/crates/services/qip-risk-engine/**`,
`qip-execution-engine/**`, `qip-portfolio-engine/**`, `qip-capital/**`, any
order-placement or capital-envelope path, or any autonomy ceiling. This is a
read/inference addition to the UNDERSTAND stage's world model; nothing here
sizes a position, places an order, or changes what autonomy level is
deployable.

---

## Context

`docs/DELIVERY-STATUS.md`'s causal-graph interlock bullet stated the defect
plainly: **the causal graph is empty in every process**, because
`qip_world_model::world::seed_demo_world` is the only thing that ever writes
a `CausalEdge`, and it has no production caller. §9.2 lists six ways an edge
may be established — natural experiments, instrumental variables,
Granger-style lead-lag with controls, structural constraints, the platform's
own order flow, and hypothesis-plus-falsification — and none of the six had
a line of code behind it: a search for any of the three most literal method
names (`natural_experiment`, `instrumental_variable`, `granger`) across
`backend/crates` returned nothing. This is the interlock's own framing of
why closing it needs "real causal-edge extraction, not a wiring fix", and it
is the reason the Phase 2 wiring pass
that closed eleven other rows (2026-09-12) explicitly left this one alone.

The platform already has what a first, honest producer needs:
`Platform::observe` absorbs real bar closes into `price_history`, keyed per
instrument, from whatever feed a composition root is wired to (not
`seed_demo_world`'s synthetic fixtures), and `qip-numerics` already carries
OLS regression with calibrated t-test p-values (`stats::ols`,
`Regression::p_values`) and the incomplete-beta machinery
(`distributions::regularised_incomplete_beta`) `student_t_cdf` is built on.

## Decision

**Implement exactly one of the six establishment methods — Granger-style
temporal precedence — as `qip_numerics::stats::granger_causality` (the
statistic) and `qip_world_model::granger::establish_temporal_precedence`
(the domain wrapper that turns a significant test into a `CausalEdge`), and
reach it from a production caller: `Platform::discover_temporal_precedence`,
run from `stage_understand` every cycle against `self.price_history` and
`self.bar_history` — real ingested bars, never the demo seed.**

### Why this method, and not one of the other five

Each of the other five was rejected for a reason this record states rather
than leaves implicit, following CLAUDE.md's "judging a proposal" checklist
(does it add a dependency; does it weaken a guarantee for speed; is its
evidence checkable by a person):

- **Natural experiments** and **instrumental variables** both require an
  *exogeneity assumption* — that the instrument or the experiment's timing
  affects the outcome only through the proposed channel — which is a claim
  about the world this platform has no way to verify from data it ingests.
  Blueprint §9.2 itself calls natural experiments "the strongest available
  evidence outside a real experiment", which is exactly the problem: their
  strength depends on a human judgement about *why* an event is exogenous,
  and asserting that judgement in code would be exactly the "refuse rather
  than guess" principle failing in the harder direction — guessing that an
  assumption holds because checking it is inconvenient.
- **Structural constraints** (arbitrage relationships, accounting
  identities) are real and "certain" per §9.2's own table, but they are a
  different kind of fact than a `CausalEdge` — a relationship that must
  hold by construction, not one estimated from evidence with a strength and
  a confidence. Building this well needs its own record naming which
  identities qualify, and forcing it through today's `CausalEdge` shape
  (strength in `[0, 1]`, a mechanism, decay) would misrepresent a certainty
  as a estimate.
- **The platform's own order flow** is the method blueprint §9.2 singles
  out as the cleanest evidence available — "a small intervention with a
  known cause" — but it presupposes fills the platform has actually made.
  Every execution node has `execution_nodes = {}` in every environment
  (`grep -rn execution_nodes infrastructure/environments/*/terraform.tfvars`),
  so there is no fill history to feed this method today; building it now
  would be a producer with nothing to produce from, indistinguishable from
  not building it at test time.
- **Hypothesis-plus-falsification** needs a falsifier evaluated against
  held-out data, which `docs/DELIVERY-STATUS.md` §14.3 already names as
  built but inert (`falsifiers_triggered` is constructed only as
  `Vec::new()`). This method's edge would be exactly as sourced as its
  falsifier evaluation, so building it ahead of that seam would produce
  edges nothing has actually falsified — the "weak until tested" row of
  §9.2's own table, made permanently weak by a missing dependency rather
  than by honest uncertainty.

**Granger-style lead-lag is the one method computable today, from data this
platform already ingests in production, against a standard significance
test.** It needs only two aligned return series — `price_history` already
holds one per instrument, keyed and bounded — and a nested F-test against a
null of no improvement is textbook, not a judgement call about the world.
Blueprint §9.2 itself calls it "weak alone, useful as a filter", which is an
honest ceiling this record does not try to raise: every edge it can produce
says only that one series' past helps predict another's future beyond what
that series' own past already does, and nothing about why.

### What was checked before committing to it

`backend/crates/libs/qip-numerics/src/stats.rs` already had `ols` with
calibrated p-values (`ols_p_values_are_calibrated_under_the_null` proves
the false-positive rate matches the nominal alpha) and
`backend/crates/libs/qip-numerics/src/distributions.rs` already had
`regularised_incomplete_beta`, the one piece an F-test needs beyond what
`student_t_cdf` already proves against published vectors — the standard
identity `F_cdf(x; d1, d2) = I_{d1x/(d1x+d2)}(d1/2, d2/2)` reuses it exactly
rather than adding new numerical machinery. **No dependency was added.**
`f_cdf` and `granger_causality` are pure Rust in `qip-numerics`, the crate
this workspace already treats as the place statistics shared across services
and the runtime live, using only `serde`/`serde_json` transitively per ADR
0002/ADR 0009.

### The bar, and why it sits where it does

- **Single lag (`TEMPORAL_PRECEDENCE_LAG = 1`).** A joint test over several
  lags has no one coefficient to read a direction from, and the edge this
  method produces needs an unambiguous sign to choose between
  `Mechanism::TemporalPrecedence` (same direction) and
  `Mechanism::InverseTemporalPrecedence` (opposite) — the two new
  `Mechanism` variants this record adds, each documented as naming no
  economic channel.
- **Minimum 60 observations** (`TEMPORAL_PRECEDENCE_MIN_OBSERVATIONS`)
  before a pair is tested at all — below this, an F-test on `2*lag+1`
  parameters has too few residual degrees of freedom for the large-sample
  justification behind the F approximation to be trusted, whatever p-value
  it reports.
- **`p < 0.01`** (`TEMPORAL_PRECEDENCE_ALPHA`), not the conventional 0.05.
  This method is meant to run over many instrument pairs every cycle, and
  at 0.05 roughly one pair in twenty clears the bar on pure noise alone —
  exactly the false-positive shape the mandatory refusal test
  (`an_independent_pair_of_series_produces_no_edge`,
  `two_independent_instruments_produce_no_causal_edge`) exists to catch.
  This is a floor, not a multiple-comparisons correction: a real correction
  needs a stated family size, which depends on how many pairs a caller
  actually tests per pass, and is left open below.
- **A minimum partial R² of 0.02** (`TEMPORAL_PRECEDENCE_MIN_EFFECT`) below
  which nothing is written even if `p < 0.01`: enough bars make a
  statistically significant but economically trivial effect common, and this
  refuses that case rather than sizing exploration against a rounding error
  wearing a confident-looking p-value.
- **Confidence capped at 0.5** (`TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING`),
  computed from `(1 - p_value)` and never a constant, but never allowed to
  reach `seed_demo_world`'s mechanism-backed default of 0.7. Precedence is
  not a mechanism, and §9.4's "confounders are often unobserved" limit
  applies to every edge this method can produce without exception, because
  nothing in it adjusts for one.

## Alternatives considered and rejected

**Widen the threshold to 0.05 to produce more edges.** Rejected: the whole
point of a "weak alone, useful as a filter" method is that its false-positive
rate matters more than its recall, and DELIVERY-STATUS's own history already
has an example of what a control that fires too easily costs (§7.5's
`MaxExpectedShortfall`, which shipped in every default limit set and could
never trigger — the opposite failure, but the same lesson: a control's bar
is not a convenience to be adjusted until the number looks better).

**Test every pair in the universe, unbounded.** Rejected: an unbounded
pairwise scan makes this pass's cost grow with the square of however many
instruments the platform ends up tracking, which is exactly the unbounded
working set CLAUDE.md's "compounding policy" and this crate's own bounded
buffers (`CAUSAL_SUPPORT_RETAINED`, `SERIES_HISTORY`) refuse elsewhere.
`Platform::discover_temporal_precedence` caps pairs tested per cycle at 200
and covers a larger universe over successive cycles instead, the same
bounded-and-deterministic discipline `price_history`'s own `BTreeMap`
ordering already gives every other pass that walks it.

**Read a mechanism from the pair's names or sector** (guessing, say, that two
instruments in the same sector share `Mechanism::Sentiment`). Rejected
outright: that is exactly "a value silently corrected is a caller bug that
survives" in reverse — inventing a mechanism this statistic did not measure.
The two new `Mechanism` variants name what was actually established
(precedence, not a channel) rather than dressing a filter up as a finding.

**Wire the writer into `stage_discover` instead of `stage_understand`.**
Rejected on fit rather than correctness: `stage_discover` is where anomalies
are found from a single series' own history, and this method's subject is a
relationship between two series, which is what UNDERSTAND already builds
(the world model, the relationship graph, the causal graph). Nothing about
the choice is load-bearing; either stage runs every cycle in every
composition root that calls `run_cycle`.

## What it costs

**A second causal-edge mechanism with no proposed economic channel.**
`Mechanism::TemporalPrecedence` and `Mechanism::InverseTemporalPrecedence`
both document, in `causal.rs`, that they name no mechanism at all — a
reader of `Effect::explain()`'s prose for a propagated shock now sometimes
sees "established only by a lagged statistical test" instead of a sentence
about supply chains or discount rates, which is a real difference in what
the explanation can say about *why*.

**A per-cycle pass with a bounded but nonzero cost.** Up to 200 pairs are
tested per `stage_understand` call once `price_history` holds enough
instruments, each a pair of nested OLS fits. Diagnostic rather than a
control — a malformed pair (a non-finite return) is skipped rather than
stopping the pass, the same discretion `capacity_probe`'s own `None` already
uses.

**The causal graph is no longer guaranteed empty**, which changes what a
reader of `state.causal_claim_count` or `WorldModel::causal()` should
expect: most cycles will still find nothing (the bar is deliberately strict
and most pairs in a small, mostly-independent universe will not clear it),
but an operator can no longer assume zero as this section's permanent
answer. `docs/DELIVERY-STATUS.md`'s causal-graph interlock bullet is
corrected to say so.

## What would make this wrong

- **A false-positive rate measurably above the stated alpha in production**,
  discovered by comparing edges written against instruments later shown to
  share no real relationship. That would mean the significance bar, the
  effect-size floor, or the pair cap needs revisiting — not silently, and
  not by loosening the bar to make the graph look fuller.
- **A second establishment method reusing `Mechanism::TemporalPrecedence`**
  for something that does propose a channel. The variant's whole
  justification is that it names *no* mechanism; a future method that adds
  one belongs on its own variant.
- **A caller feeding this method fill history once execution nodes are
  deployed**, without also building the platform's-own-order-flow method
  §9.2 names separately. The two are different evidence (observational
  lead-lag versus a known intervention) and conflating them would overstate
  what either establishes.
- **Confounder modelling, natural experiments, instrumental variables,
  structural constraints, or hypothesis-plus-falsification landing without
  their own ADR.** This record closes one of six methods and says so
  explicitly; the other five, and §9.1's confounders layer, remain exactly
  as open as `docs/DELIVERY-STATUS.md` states.
