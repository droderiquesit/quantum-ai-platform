# ADR 0066: A regime narrows an allocation and never favours it, and a cadence may skip a record but never a control

- **Status**: Proposed
- **Date**: 2026-09-14
- **Supersedes**: nothing
- **Related**: ADR 0055 (a counterfactual result narrows sizing and only narrows it), ADR 0061 (rule regret is a proposal, a defence or a dormancy finding), ADR 0063 (a sizing cap is the second consequence of counterfactual scores), ADR 0064 (a family's funding standing is measured and no weight moves)

## Context

Four blueprint sections were scored `ABSENT` in `docs/DELIVERY-STATUS.md` and
are built together here because they share two seams rather than four
mechanisms.

- **§23.3 Regime-Conditional Allocation.** A regime *is* classified in
  production — `Platform::market_regime`, reached from the cost router's
  intelligence rung — and no allocation reads it. The platform could name the
  regime and sized identically in all five.
- **§19.2 Evaluation Tiers.** Five tiers, a cadence each, and a hot-tier cap of
  1,200 stated as a measurement: "the count at which measured p99 evaluation
  reaches seventy percent of the 90 µs budget".
- **§23.6 Adaptive Cadence and Sequencing.** A table of signals and the work
  each triggers, closing with "Nothing changed. Do not run. This is the
  saving."
- **§18.4 Compounding Policy.** Reinvestment cadence, fee-tier accumulation,
  threshold crossing, withdrawal drag, minimum viable scale.

**The two seams.** The first is the blueprint's own §19 taxonomy of ten alpha
families: §23.3 asks what a regime does to a *family*, and §19.2 assigns a tier
by a family's *horizon*. Neither question can be answered by either of the two
things this workspace already calls a family — `families::FamilyId` is a
correlation cluster recomputed every cycle, and `qip_lifecycle::StrategyFamily`
is a provenance key naming a sweep — so the taxonomy is a third thing,
`qip_optimization_engine::universe::AlphaFamily`, and the module documentation
says in its first paragraph which of the three a reader is holding.

The second seam is cadence. §19.2's per-tier cadence, §23.6's trigger table and
§18.4's reinvestment schedule are the same question — *does this run now?* —
asked of three subjects. One answer is given, counted in the platform's own
cycle count, because a wall-clock cadence needs a "when did this last run"
that something would have to store, and the event log is already the record of
what ran.

## Decision

### 1. A regime may narrow a weight bound and may never widen one

`regime::Stance::multiplier` has no arm above one. The favouring half of
§23.3's table leaves `regime::favoured` as a set of family names carrying no
number at all, and reaches a proposal's own `compromises` as a sentence.

This is ADR 0061's and ADR 0063's asymmetry applied to a third row, and the
argument is the same one with a new subject: the classifier this reads is four
branches over a drawdown, a spread and a sign-persistence count. That is
enough evidence to take risk off. It is not enough to put risk on, and a
platform that can widen its own bounds because a classifier changed its mind
has an escalation path whose authority is a heuristic.

### 2. An instrument whose alpha family nobody recorded is sized as the family the regime suits least

No production path attributes an alpha family to a sized instrument: a thesis
carries an object, a conviction and a price. So the function production
actually calls is `regime::unattributed_multiplier`, a minimum over the whole
table.

The alternative — an unknown family is unaffected — was rejected because it
would make §23.3 a control that cannot fire: the only input it would ever
receive in production is "unknown". This repository keeps the receipt for that
shape under `MaxExpectedShortfall`. Making the absence of attribution *cost*
something is also the right incentive: the way to stop being sized as the
worst-suited family is to record which family it is.

Two values are reachable in production today — three quarters under a regime
the platform can name, a half under one it cannot — and they differ, which is
what makes the regime an input to the size rather than a constant wearing a
control's clothes.

### 3. Reinvestment is planned and never performed

`qip_capital::compounding::ReinvestmentPlan` is a record. No function in that
module takes `&mut` anything holding money, the type has no method that
applies it, and no caller anywhere takes one. Reinvestment is the one direction
in this platform that *adds* to what may be deployed; a policy that redeployed
profit on an arithmetic threshold would be an escalation path with no person in
it.

What is automatic is the refusal. A lot below the smallest position the desk's
mandate will hold, or one whose redeployment would cost more than the ceiling
allows, is declined with the figures that declined it. Both arms fire on a
small book, which is the book §18.4 is about.

### 4. The compounding policy restates no number the mandate already states

The redeployment cost is the mandate's `turnover_cost_bps`; the minimum
reinvestment lot is `minimum_position` of the book's current equity — below
that, a redeployment cannot buy a position the mandate would keep. Principle 6:
two independent claims about one fact will disagree, and the louder one will be
wrong.

### 5. A cadence may skip work that produces a record and never a control

This is the rule §23.6 needed before it could be built here at all. The
section's third row says "regime belief changed → trigger regime-conditional
weighting". Taken literally it is a defect: the regime narrowing is a control,
and a control that runs only when a signal moved stops holding when nothing
moves. So the narrowing is recomputed on every construction, and the cadence
governs only the evaluation-tier census and the reinvestment plan, both of
which produce a line in a cycle entry and nothing else.

Two of §23.6's four rows are built and two are not, and the module says which
and why: nothing in this platform measures a whitelist hit rate, and the
centre holds no inventory-deviation figure that could be read without
inventing a second one. A four-armed table with two arms no input can reach
would read as a scheduler and be half a scheduler.

### 6. An oversubscribed hot tier is refused, not demoted

`TierPlan::assign_counted` refuses a population whose hot tier exceeds the cap
and names the two things a caller may do instead — reclassify into a colder
tier, or run a second node. It does not choose the overflow itself: a demotion
nobody decided changes which strategies see an event, and the first evidence of
it would be a latency number nobody could attribute.

An unclassified strategy is assigned the **coldest** tier. The hot tier's cap
is a latency budget and may not be spent on a guess; a family name is matched
whole against the ten, never by prefix, so `momentum-v3` is not evidence that a
sweep harvests continuation.

## What this deliberately does not do

- **No weight moves on a family standing.** ADR 0064 stands unchanged. The
  regime narrowing is keyed on an instrument's tape, not on a family's
  funding record, and `family_review` still returns no money type.
- **Withdrawal drag and minimum viable scale are not built.** §18.4 names both.
  Each needs a forward growth rate the platform does not measure, and a
  compounding cost projected from an assumed return is a number that would read
  as a measurement.
- **The fee ladder is not restated.** `FeeVolumeLedger` accumulates trailing
  volume per venue and answers "how far from this threshold" and "what would
  this rate improvement be worth", both against figures the caller reads off
  `qip_routing::FeeSchedule`. A second ladder here would be a second answer to
  what a venue charges. **`FeeTier` therefore remains unreached**: it lives in
  `qip-edge/qip-routing`, nothing outside that crate's own tests constructs a
  `FeeSchedule`, and the accumulator's seam waits for one.
- **No new event topic.** The tier census and the reinvestment plan reach the
  cycle entry's detail. Giving either its own topic is a `qip-events` change
  outside this lane.

## Consequences

- Every sized name is narrowed by the regime its own instrument is in, so a
  book on a tape the platform cannot classify is a smaller book. That is a
  behavioural change to sizing and is visible in a proposal's `compromises`.
- The platform states, on the cycles where it is due, what its registered
  population would cost the hot tier and whether realised profit is worth
  redeploying — and says nothing at all on the cycles where neither has a
  subject.
- The paper-trading boundary is untouched. Nothing here constructs an order,
  names a venue class, reads a credential or takes an autonomy ceiling; all
  three layers — Terraform's refusal, `AutonomyLevel::deployable`, and the
  type-level absence of a live constructor in `qip-edge` and `qip-cost-router`
  — are as they were.
