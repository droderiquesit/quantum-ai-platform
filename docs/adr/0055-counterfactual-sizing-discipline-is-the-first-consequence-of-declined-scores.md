# 0055 — Counterfactual sizing discipline is the first consequence of `declined_scores`

**Status:** *accepted*, 2026-09-12.

**Relates to:** blueprint §12.1–§12.4 (`docs/architecture/algorik-blueprint-v10.1-source.md`),
`docs/DELIVERY-STATUS.md` §12.3 and §11.2, and `.claude/rules/domains/risk-and-execution.md`'s
prohibition on escalating autonomy from a model or agent finding.

**Does not touch:** `backend/crates/services/qip-risk-engine/**`,
`qip-execution-engine/**` order-placement logic, `qip-capital/**`, any capital
envelope, any risk limit, and any autonomy ceiling. Nothing here approves,
places, or routes an order. This narrows one input — the confidence a
construction sizes an instrument's thesis against — and every actual
order-level effect still flows through the existing, unmodified risk and
execution gates.

---

## Context

`docs/DELIVERY-STATUS.md` scored blueprint §12.3 ("What It Changes") `ABSENT`:
`Platform::score_declined` (§12.2, `REACHED`) reconstructs the book state at
every risk-gate refusal, simulates the fill, charges realistic costs, and
accumulates a `DeclinedScore` per refused order — but nothing read the
accumulator. `grep -rn 'declined_scores()' --include=*.rs backend/crates`
found exactly one caller, `qip-kernel/tests/learning.rs`. Blueprint §12's own
framing is blunt about the cost of that: "the risk gate already logs every
veto... if four hundred cycles are vetoed in a day and three hundred and
eighty would have been profitable, that is an enormous signal being discarded
daily."

§12.3's own table names four possible consequences of a finding: a rule
recalibrated, a venue dropped, an allocator objective revised, a sizing
function adjusted. This record builds exactly one of the four and states,
rather than implies, why the other three stay open.

## What the data actually supports

Before choosing a consequence, the two structures a consequence would have to
read were checked rather than assumed:

- **`DeclinedScore`** (`platform.rs`) carries `order_id`, `object_id`, `gate`,
  `declined_at`, `scored_at`, `would_have_earned`, `regret`, `alternatives`.
  No strategy id, no family, no venue — a declined order was refused before
  it reached one, so `ActualTrade` for a declined path is constructed against
  `UNROUTED_VENUE` (`platform.rs`, inside `evaluate_alternatives`'s `None`
  arm). `grep -n 'struct DeclinedPath' -A 10` and `grep -n 'pub struct
  DeclinedScore' -A 10` in this file name every field either type carries,
  and neither names a family or a venue.
- **What consumes strategy-family confidence today** is
  `central/learning.rs`'s `assess_overfitting` /
  `qip_lifecycle::DemotionMonitor`, and it reads a *pilot-versus-live*
  comparison for a strategy that was actually funded and traded — a
  different question from "what would a refused order have done", answered
  from a different kind of evidence (realised returns, not a counterfactual
  fill). Attributing a `DeclinedPath` to a family would require a field
  nothing populates today.

This is why the task's own candidate — discount a strategy family's sizing
from a persistent pattern of poor counterfactual outcomes — is **not** what
gets built. Building it would mean inventing an upstream signal (a
declined-order-to-family link) that does not exist, which is the thing this
task's own brief says to avoid. **The instrument (`object_id`) is the only
grouping `declined_scores()` can honestly support without adding one.**

## Decision

**A new method, `Platform::counterfactual_sizing_multiplier(object_id)`,
reads the bounded `declined_scores` history for one instrument and returns a
fraction in `(0, 1]` that can only narrow
[`Platform::sizing_confidence`](../../backend/crates/runtime/qip-kernel/src/platform.rs) —
never widen it — and `sizing_confidence` multiplies its own result by that
fraction before returning.**

This is §12.3's fourth named consequence, "a sizing function adjusted from a
counterfactual result", built at the seam DELIVERY-STATUS §11.2 already names
as the sizing function's production home:
`sizing_confidence` → `sizeable_theses` → `construct_from` → `stage_decide`,
run every cycle by every composition root that calls `Platform::run_cycle`
(`qip-api`, `qip-fastbrain`, `qip-deepbrain`). Not a demo, not a test-only
entry point — the same seam DELIVERY-STATUS §11.2 already scores as
production-reached for the valuation mark's own confidence narrowing.

### The rule, stated exactly

For one `object_id`, over its scored entries in `declined_scores`:

1. **Below `COUNTERFACTUAL_SIZING_MIN_SAMPLE = 10` observations, the method
   returns `Decimal::ONE` unconditionally.** Ten matches
   `qip_learning_engine::self_model::MINIMUM_SAMPLE` — the platform's
   existing answer to "how many observations make an evidence-weighted
   estimate trustworthy" — rather than a second, differently-sized answer to
   the same question nobody could reconcile with the first. A handful of
   bad-looking declines on one name is a coincidence, not a finding, and
   recalibrating on it is recalibrating on noise — the same discipline
   `qip_world_model::granger`'s significance bar applies to a causal claim
   (ADR 0054).
2. **At or above the floor, compute the unfavourable fraction**: the share of
   scored entries with `regret == false` — the twin's own answer to "would
   this trade have beaten standing aside", inverted, so `!regret` means the
   decline was *correct*. If that fraction is at least
   `COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION = 0.75`, the instrument's
   proposals have persistently been the kind a gate is right to refuse, which
   is evidence about whatever is proposing them, not about the gate. Below
   0.75 — a control earning its place slightly more often than not, §12.3's
   own second row — nothing happens.
3. **On the bar clearing, the method returns a single constant,
   `COUNTERFACTUAL_SIZING_DISCOUNT = 0.5`,** rather than a curve shaped by the
   fraction or the sample size. One auditable number a person can name
   ("this instrument's sizing confidence is halved") rather than a formula
   tuned after the fact to fit a backtest.
4. **There is no branch that returns more than `Decimal::ONE`.** A rule that
   vetoes mostly *profitable* paths — §12.3's first row, "the rule is too
   tight, recalibrate with evidence rather than intuition" — produces
   exactly the same output as no finding at all: `Decimal::ONE`. This is not
   an oversight; it is how §12.4's own guardrail is honoured structurally:
   "a veto rule may only be loosened through the full approval path, never
   automatically from counterfactual evidence." A function with no path back
   above one cannot violate that guardrail by a future edit missing a check —
   there is nothing to check.

Evidence read live from `declined_scores`, not written by a separate pass:
`declined_scores` is already bounded (`DECLINED_HISTORY = 256`), already the
production record `score_declined` (called from `stage_learn`) maintains, and
assessing it at the point of use is the same pattern `central_degradation`
already uses for `CausalGraphFreshness`, `SelfModelFreshness` and
`BeliefFreshness`, and `sizing_confidence` already uses for a valuation
mark's own decayed confidence. Writing a second, mutable per-object discount
during `stage_learn` and reading it back at DECIDE would duplicate a fact
`declined_scores` already carries — exactly the "no second source of truth
for a fact the event log already holds" rule in
`.claude/rules/architecture/00-boundaries.md`.

## What was checked before committing to it

- **No new dependency.** The method is arithmetic over an existing `Vec` and
  a `Decimal` constant built with `Decimal::from_raw`, the same technique
  `AlternativeMenu::standard`'s own `smaller: Decimal::from_raw(500_000_000)`
  already uses. `serde`/`serde_json` remain the only third-party crates
  (ADR 0002, ADR 0009).
- **No risk-engine, execution, or autonomy path touched.**
  `grep -rn 'counterfactual_sizing_multiplier\|COUNTERFACTUAL_SIZING' backend/crates`
  finds only `qip-kernel/src/platform.rs` and its own tests. `sizing_confidence`
  is read by `sizeable_theses`, which only removes a thesis from what
  `construct_from` may size or narrows the shared weakest-mark multiplier —
  it does not touch `qip-risk-engine`'s pre-trade checks, which run
  independently and after sizing, on whatever quantity the construction
  produced.
- **Cardinality.** No new telemetry series was added. `.claude/rules/domains/observability.md`'s
  discipline against labelling a metric by instrument (unbounded cardinality)
  argued against adding a `{object=...}` gauge here; the existing
  `sizing_confidence` and `declined_scores()` accessors are already public
  and already the way an operator or a test inspects either fact.

## Alternatives considered and rejected

**Discount a strategy family's sizing or confidence**, the task's own
starting candidate. Rejected on the evidence check above: no declined path is
attributed to a family anywhere in the tree, and building the attribution
would be new upstream signal invention the brief itself rules out.

**Drop a venue that feasibility rejections cluster on** (§12.3's third row).
Rejected: feasibility rejections are an edge-plane concept
(`qip-edge::feasibility`, out of this task's scope crates) and, separately,
a declined order in the central platform never reaches a venue at all
(`ActualTrade` for a decline is `UNROUTED_VENUE`), so there is no venue signal
in `declined_scores` to cluster on without inventing one.

**Revise the allocator's objective from an unfunded-family comparison**
(§12.3's fourth row as the blueprint states it, not the sizing row this
record actually builds). Rejected for the same reason as the family-discount
candidate: no family attribution exists to compare "unfunded" against
"funded".

**Recalibrate the risk gate itself** — loosen or tighten the threshold a
control fires at, from the regret rate. Rejected outright:
`.claude/rules/domains/risk-and-execution.md` prohibits "weakening any of the
three paper-trading layers" and, independently of that boundary, §12.4's own
guardrail forbids automatic loosening of a veto rule. Tightening an already
narrow control from a *favourable* pattern (mostly-correct declines) has no
row in §12.3's table asking for it either — the table's second row says
"quantify by how much and defend it", a narrative action for a person, not an
automatic one.

**A discount curve shaped by the fraction or the sample size**, rather than
one constant. Rejected: a curve turns "why is this instrument's confidence
0.63 today" into a question about a formula's shape instead of one about a
threshold a person chose on purpose and can point to.

**Write the discount during `stage_learn` into a new per-object field, read
back at DECIDE.** Rejected: `declined_scores` is already the record of this
fact, bounded and journaled through the LEARN stage; a second field
recomputing the same thing from the same source on a different cadence is a
second source of truth that can drift from the first, which
`.claude/rules/architecture/00-boundaries.md` rules out on its own terms, not
just as a style preference.

## What it costs

**Half of one input to sizing, for an instrument with a persistent, adverse
counterfactual record, until the pattern in the bounded 256-entry window
changes.** Because the fraction is recomputed from the live window rather
than latched permanently once tripped, a run of favourable evidence after a
discount fires lifts the fraction back under 0.75 on its own — there is no
separate "un-discount" step to build or forget to call, and no separate
one-way ratchet to defend as intentional. That is a design choice with a
cost: an instrument could, in principle, oscillate in and out of the
discount as its 256-entry window turns over, which is more responsive than a
sticky penalty and less protective than one that only accumulates.

**Three of the four named consequences remain exactly as absent as
`docs/DELIVERY-STATUS.md` stated**: no rule is recalibrated, no venue is
dropped, and no allocator objective is revised from a counterfactual result.
§12.3 moves from `ABSENT` to `PARTIAL`, not `REACHED`.

## What would make this wrong

- **A declined-order-to-family or declined-order-to-venue link is built
  later and this record is not revisited.** The day either exists, the
  narrower-than-blueprint scope this record argues for (object only, because
  that is all the data supports today) becomes a choice to defend again
  rather than a fact about the data.
- **The 0.75/10/0.5 constants are tuned to make a demonstration look better**,
  rather than left as the stated, argued values above. ADR 0054's own
  "alternatives considered" section names the same failure for a different
  threshold, and it applies here unchanged.
- **A future change adds a branch to `counterfactual_sizing_multiplier` that
  can return more than `Decimal::ONE`.** That is the one change this record
  states outright must not happen without a new record, because it is the
  exact automatic loosening §12.4's guardrail forbids.
