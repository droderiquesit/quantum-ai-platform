# ADR 0148: A counterfactual has one reference: a missed opportunity is scored as a claim in basis points and never priced at a side nobody proposed, and every alternative is a named arm of the twin's menu

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval ("I
  allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). That approval lets this record settle what each counterfactual
  is measured against and who may read it. It does not change any limit or
  threshold value, admit any dependency, or touch a paper-trading layer.
- **Date**: 2026-10-19
- **Extends**: ADR 0055 and ADR 0063 (declined and filled paths are priced by the
  twin; the only consequence is a cap that cannot exceed one), ADR 0125 (an
  advisory result narrows or vetoes and never enlarges), ADR 0007 (attribution
  must reconcile exactly).
- **Keeps**: ADR 0003 (paper only), ADR 0006 (a classical baseline), the
  `Simulated` wrapper (nothing a counterfactual produced reaches a realised
  line), and blueprint section 12.4's guardrail that nothing is loosened from
  counterfactual evidence.

## Context

LEARN prices what the platform did not do. It does so for refused orders
(`score_declined`) and for the sizes not taken on filled ones (`score_filled`),
through the `qip-twin` `CounterfactualEngine`. Seven register rows ask for more
of it, and every one of them hides the same question: *against what?* A
counterfactual is a difference, and a difference with an invented reference is
a number nobody computed. The rows are
R12-TICK-2077, R12-MODEL-6004, R12-MODEL-4025, R12-CAPITAL-4092,
R12-CAPITAL-9039, R12-TICK-10103 and R12-MODEL-10096.

What the code says, read before deciding:

- **A missed opportunity has no input to price.** `Action::MissedOpportunity`
  in `qip-twin/src/capture.rs` requires `would_have_earned: Simulated<Decimal>`,
  and nothing in the kernel produces the variant. `capture_unworked` records
  `Action::ExpiredOpportunity` for an opportunity that lapsed in the queue and
  documents why it is unpriced: no side and no size was ever proposed.
- **The platform already scores the thing a missed opportunity is.** Every
  hypothesis REASON forms passes through `record_prediction` before the
  approve-or-reject branch, so a hypothesis rejected on review carries an open
  prediction. `calibrate_resolved` selects due predictions on
  `is_open() && resolves_at <= now` with no filter on approval, and the episode
  recorded with `DecisionTaken::RejectedOnReview` resolves with
  `EpisodeOutcome::realised_move_bps`. A rejected claim is therefore already
  graded, in basis points of the reference the claim was made against, with no
  size.
- **The twin's menu already contains every alternative the rows ask for.**
  `AlternativeMenu::alternatives_to` yields `Trade`, `DoNotTrade`, `LargerSize`,
  `SmallerSize`, `DifferentVenue`, `DifferentRegion`, `DelayedEntry`,
  `DifferentHedge` and `NoHedge`. `NoHedge` is the unhedged position, and
  `DoNotTrade` on an order that reduces a holding is that holding kept to the
  horizon. `ActualTrade::hedged_with` is called only from the twin's own tests:
  no kernel path puts a hedge on, because `run_hedge_brain` "returns proposals
  and never orders".
- **Order to model attribution already exists, one hop away.**
  `AttributionLineage::note_release` stamps each accepted order with seven keys,
  including the `model:<reference>` entries of the hypotheses it expresses, and
  `fills_expressing` joins through it. `FillScore` simply does not copy them.
  What is empty is the source: `synthesise` passes `models: Vec::new()` to the
  reasoning engine, so no registered model contributes to any hypothesis in the
  cycle today.
- **The shadow vocabulary is duplicated.** `qip-capital`'s
  `ShadowPortfolioUniverse` has no caller outside its own crate and an
  acceptance test; the kernel's shadows are the twin's menu arms.
- **The model rollback path does not live in the kernel.** `Platform::rollback_model`
  takes the `ModelRegistry` as an argument, and the registry belongs to the
  deep brain (`roll_back_degraded` in `qip-deepbrain/src/learning.rs`), which
  selects on drift alone and, by design, leaves a degraded model with no
  predecessor in place because `decision_eligibility` already refuses it.

Locate them with
`grep -n 'MissedOpportunity\|ExpiredOpportunity' backend/crates/services/qip-twin/src/capture.rs`,
`grep -n 'fn alternatives_to' backend/crates/services/qip-twin/src/counterfactual.rs`,
`grep -n 'models: Vec::new()' backend/crates/runtime/qip-kernel/src/platform.rs` and
`grep -n 'fn roll_back_degraded' backend/crates/apps/qip-deepbrain/src/learning.rs`.

## Decision

1. **A missed opportunity is scored as a claim, in basis points, and is never
   priced in money.** The opportunities REASON took up and did not size
   (rejected on review, not sizeable, withheld on a limit under ADR 0147) are
   graded by the path they already take: prediction, episode,
   `realised_move_bps`. `Action::MissedOpportunity` keeps no producer, and its
   doc comment says so and names this record, so it stops reading as a gap.
   `Action::ExpiredOpportunity` stays unpriced. The reference for every
   declined or missed thing is the **move the claim named, over the claim's
   own horizon**; the only money figure in any counterfactual is one the twin
   computed for an order somebody proposed.
2. **What is not scored is counted, never zeroed.** A new counter
   `qip_opportunities_unpriced_total{reason}` has the closed label set
   `expired_unworked` (lapsed before REASON took it up), `no_mechanism`
   (`synthesise` returned `None`: the anomaly implies no claim) and
   `undirected_claim` (`record_prediction` returned `false`: a `RegimeShift`
   or `EventOccurs` has no sign to grade). The label is an enum of three;
   nothing runtime-supplied reaches it.
3. **The alternative to a reducing or unwinding order is the holding kept; the
   alternative to a hedged order is the unhedged position.** Both are arms the
   twin already prices (`DoNotTrade` on a reducing order, `NoHedge` where
   `ActualTrade.hedge` is set), marked at the same horizon on the same tape as
   every other arm. No queue of hedge proposals is built, because the platform
   puts no hedge on: a "hedged path" does not exist to be replayed on the paper
   path, and `NoHedge` equals `Trade` until one does. What is missing is the
   label. At release, `ReleaseContext` is stamped with an `ExposureEffect` of
   `Adds`, `Reduces` or `Unclassified`, computed from the held lot's sign
   against the order's side with the same function ADR 0147 uses for a thesis
   (a flat book is `Adds`, the restrictive reading), and `FillScore` carries it
   from the lineage, `Unclassified` when the lineage was evicted. Reported
   regret is split by it; **the sample `sizing_review` reads is unchanged**,
   because excluding reducing fills from a cap's sample would arm fewer caps,
   and loosening a cap from counterfactual evidence is what section 12.4
   forbids. A later record may separate them and must show it does not loosen.
4. **`FillScore` gains `models: Option<Vec<String>>`.** `None` means the
   lineage was evicted and the order's models are unknown. `Some(vec![])` means
   the lineage is known and no registered model contributed, which is true of
   every fill today. `Some(refs)` holds only the `model:` entries of the
   order's `Dimension::Model` key; the `analyst:` and `detector:` entries are
   not models and are not copied. `Platform::counterfactual_by_model()` returns
   the size regret per model reference from the `Some` scores. It has **no
   consumer** in this record: the read is empty until a served model enters
   `SynthesisInput.models`, and a consumer of an empty read is a control that
   cannot fire. When one exists it may *hold* a promotion or add a refutation
   to the re-verification dossier, never promote or enlarge.
5. **The four-way split stamps three quantities, each read off the stage that
   produced it.** `q_ref` is the quantity the thesis implies at the budget
   before any narrowing multiplier; `q_sized` is `ProposalLeg.quantity` after
   the multipliers and caps; `q_final` is the quantity of the order at
   release. All three are stamped on `ReleaseContext` and kept on
   `AttributionLineage` beside the existing keys, signed by the order's side.
   With `move = mark - arrival`, the terms are `q_ref * move` (prediction),
   `(q_sized - q_ref) * move` (sizing), `(q_final - q_sized) * move`
   (allocation) and, for a partial fill, `-(q_final - q_filled) * move`
   (unfilled), which telescope to `q_filled * move` exactly, so the sum equals
   the held-move term of the existing per-fill split by construction (ADR 0007).
   The allocation counterfactual is `q_sized`: the book weight with nothing
   between the proposal and the release. If no stage changes a quantity between
   them, `q_final == q_sized` and the allocation term is a **measured** zero.
   **`q_ref` must be reported by the constructor, not recomputed.**
   `construct_from` mutates the survival governor (`governor.size`,
   `signals_mut`), so a second unnarrowed run is not side-effect free and is
   refused as a method. If the constructor cannot report its own pre-narrowing
   quantity without a second pass, the code lane stops and the split stays
   unbuilt, and the register says why.
6. **Shadows are the twin's menu arms, scored sequentially.** The kernel does not
   compose `qip-capital::ShadowPortfolioUniverse`: it would be a second answer
   to "what if the size were different" beside the twin's `LargerSize` and
   `SmallerSize`, and principle 6 says the louder of two such claims is the
   wrong one. The sizes are the menu's (`AlternativeMenu::standard`), priced
   per cycle under the existing per-cycle cap with deferral counted
   (`COUNTERFACTUALS_DEFERRED`), one at a time. No parallel batch: `tokio` is
   off the decision core (ADR 0109), the work is CPU on a bounded window, and
   a batch's only gain would be latency nobody has measured as a problem. The
   compact output is the existing `CounterfactualJournal` in the cycle entry.
   `qip-capital::shadow` stays a library vocabulary for allocation-level
   shadows, a different grain, until the allocator exposes alternate
   allocations.
7. **The twin's entry is already journaled as a labelled statistic, and nothing
   more of it is.** `FillScore.trade_error_bps` is the signed residual of the
   twin's simulated entry against the venue's fill, crossing from
   `Simulated<Decimal>` through `as_f64_for_statistics` at the single stated
   point in `fill_error_bps`. The predicted *price* is not journaled: a
   `Decimal` simulated price beside a fill price is the money figure
   `Simulated` exists to keep apart. Slippage against the arrival price is
   already captured (`slippage_bps`, `with_slippage_bps`). A comparison of the
   twin's simulated P&L against a realised P&L at the horizon is **not
   built**: an opening fill realises nothing (`capture_submission` books it at
   zero), so the "realised" side would be a mark the platform does not hold,
   and the difference on one tape decomposes into the entry error above and the
   cost calibration `fill_calibration` already tracks.
8. **A promoted model is re-verified on a cycle count and demoted through the
   existing path.** `Platform::reverify_models(&registry, now)` runs
   `tournament.verify_model` for each promoted production model when the
   cycle count is a multiple of `PlatformConfig::model_reverify_every_cycles`
   (non-zero, validated; default one `REGIME_WINDOW`, 20 cycles, chosen and not
   measured), journals each result as `ModelVerified`, and returns the
   references whose verdict `is_refuted()`. Wall time is not read, so a replay
   re-verifies on the same cycles. The deep brain's `roll_back_degraded`
   takes drifted models *and* refuted ones. A refuted model with a known-good
   predecessor goes through `rollback_model`. One without a predecessor is
   left in place, as drift already leaves it, and the app records a failing
   `EvaluationRecord` (dataset `reverification`, carrying the verdict summary)
   so `decision_eligibility` refuses it; a later passing evaluation supersedes
   it because the card reads its latest. The centre's self-model row narrows
   through the same `observe_model_drift` channel until it is resolved.
   Counterfactual scores do not enter this decision (decision 4); VerificationResult
   driving **solver policy weights** is not built, because solver routing is
   ADR 0134's and the classical baseline of ADR 0006 does not take a weight
   from a model verdict.

## Consequences

- No counterfactual in the platform is priced against a side, size or price
  that nobody proposed. A reader can ask of any figure "whose order?" and get an
  order id.
- The learning loop loses nothing it has: rejected and withheld claims are
  already graded; what changes is that the platform says so, and counts the
  opportunities it cannot grade.
- Reducing fills become visible as such. A regret finding no longer silently
  averages a sale that cut a position with a purchase that opened one.
- A model that begins to drive proposals will be attributable from the first
  fill, with no further kernel change, and will be re-verified on a fixed cycle
  cadence from the first cycle after promotion.
- Code slices (each needs its mutation): the doc and the counter; the
  `ExposureEffect` stamp and `FillScore` field; `FillScore.models` and
  `counterfactual_by_model`; the three quantities on `ReleaseContext` and the
  split (conditional, see decision 5); `reverify_models`, the config field and
  the extension of `roll_back_degraded`.

## What it costs

- **No aggregate "forgone profit" for the opportunities not taken.** A desk that
  wants one number for what was left on the table does not get it; it gets
  a move in basis points per claim and a count of the rest. That is deliberate
  and it is a loss of a headline figure.
- **Every `FillScore` and `ReleaseContext` grows.** All new fields are
  `#[serde(default)]` so an older journal replays as having stamped nothing,
  which is what it did; a field read from an older record says `None` or
  `Unclassified`, never a value.
- **A re-verification pass per cadence tick** per promoted model, journaled. The
  default of 20 cycles is a guess about how fast a verdict can change, and it
  will be wrong in an unknown direction until a refuted model is seen late or
  early.
- **A dead read by design.** `counterfactual_by_model` returns nothing today.
  It is built now so attribution is accumulating when the first model drives a
  proposal, not afterwards; the cost is a method with no caller, which this
  record names so it is not mistaken for the kind of control that cannot fire.
- **`q_ref` may never be buildable cleanly.** If the constructor cannot report
  it, the split stays unbuilt and the register keeps the gap.

## What would make this wrong

- If the desk needs the forgone figure in money, for a decision, and the
  basis-point scoring is shown not to rank missed claims the way a sized
  reference would (measured as rank disagreement between `realised_move_bps`
  and a money score on a pilot set of declined orders), decision 1 is too weak.
  The remedy is a declared reference size under its own record, journaled and
  flagged as a limit-adjacent value, not a default in code.
- If the sizing cap ever reads `ExposureEffect`, decision 3's promise that it
  does not has been broken, and any cap armed from the narrowed sample is a
  loosening that needed the approval path.
- If a re-verification verdict flips model status more than once in a
  `REGIME_WINDOW` on unchanged evidence, the verifier society is not
  deterministic on the inputs decision 8 gives it, and the cadence is hiding a
  defect rather than catching drift.
- If `q_final` differs from `q_sized` on the central path for a reason no stage
  owns, the allocation term is not a stage's effect but an unexplained residual,
  and decision 5's definition of allocation is wrong.

## Dissent

The strongest counter-position says a missed opportunity should be priced in
money at a stated reference size. The desk's three questions are what the
vetoes cost, whether the sizes were right, and what was never taken; the first
two are in money and the third is in basis points, so the three cannot be added,
ranked or charted on one axis, and the asymmetry flatters inaction because only
the third is unquantified in the unit the desk thinks in. A fixed reference
size (one lot, or a fraction of equity) is arbitrary but constant, and a constant
arbitrary reference makes missed claims comparable with each other, which the
basis-point figure also does.

It lost because the reference size is a number nobody proposed, and a money
figure at that size would be summed into the same `forgone` total as figures
for orders the platform actually considered; the type that stops that
(`Simulated`) stops a *declared* alternative, not a conjured one. The
basis-point figure is scale-free, already produced and already graded. What
would revive it is the measured rank disagreement above together with an owner
decision on the reference size, which is a value that bounds exposure and so
belongs to its own record.

## Code it constrains

- The capture vocabulary: `grep -n 'MissedOpportunity\|ExpiredOpportunity' backend/crates/services/qip-twin/src/capture.rs`.
- The unworked path: `grep -n 'fn capture_unworked' backend/crates/runtime/qip-kernel/src/platform.rs`.
- The claim path: `grep -n 'fn record_prediction\|fn calibrate_resolved' backend/crates/runtime/qip-kernel/src/platform.rs`.
- The twin's arms: `grep -n 'fn alternatives_to\|NoHedge\|fn hedged_with' backend/crates/services/qip-twin/src/counterfactual.rs`.
- The scoring: `grep -n 'fn score_filled\|fn score_declined\|pub struct FillScore\|fn fill_error_bps' backend/crates/runtime/qip-kernel/src/platform.rs`.
- The stamps: `grep -n 'pub struct ReleaseContext\|pub fn note_release' backend/crates/runtime/qip-kernel/src/attribution.rs`.
- The constructor: `grep -n 'fn construct_from' backend/crates/runtime/qip-kernel/src/platform.rs` and `grep -n 'pub struct ProposalLeg' backend/crates/services/qip-portfolio-engine/src/proposal.rs`.
- Model hooks: `grep -n 'pub fn rollback_model\|pub fn promote_model_from' backend/crates/runtime/qip-kernel/src/model_serving.rs`, `grep -n 'pub fn verify_model' backend/crates/runtime/qip-kernel/src/model_tournament.rs`, `grep -n 'fn roll_back_degraded' backend/crates/apps/qip-deepbrain/src/learning.rs`.
- The duplicate vocabulary: `grep -rn 'ShadowPortfolioUniverse' backend/crates`.

## Rows unblocked

- **R12-TICK-2077**: decided; a missed opportunity is a graded claim in basis points, never a money price, and the unscoreable remainder is a counted series; register wording restated.
- **R12-MODEL-6004**: decided; the alternative is `NoHedge` or the held position at the same horizon, both existing arms; the missing piece is the `ExposureEffect` label, and no hedge queue is built.
- **R12-MODEL-4025**: decided; `FillScore.models` from the lineage's `model:` keys with `None` for unknown and `Some(empty)` for none, read by `counterfactual_by_model`, no consumer yet.
- **R12-CAPITAL-4092**: decided; three stamped quantities, a telescoping split, allocation as a measured term; conditional on the constructor reporting `q_ref`.
- **R12-CAPITAL-9039**: decided; shadows are the twin's menu arms, sequential, no composition of `qip-capital::shadow`, the cycle journal is the compact output; register wording restated.
- **R12-TICK-10103**: decided; the simulated entry is already a labelled signed-bps statistic; no price, no simulated-versus-realised P&L is journaled; register wording restated.
- **R12-MODEL-10096**: decided; cycle-count re-verification, refuted models demoted through `rollback_model` or made ineligible, no solver-weight coupling.

## Not decided here

- **RISK**: no limit, cap multiplier or threshold value is added or changed. The
  sizing review's bars (`COUNTERFACTUAL_SIZING_MIN_SAMPLE`, the unfavourable
  fraction, the halved cap) are read, not touched. A declared reference size
  for a money-priced miss (see Dissent) would be a value that bounds exposure
  and is left to its own record. The re-verification cadence is an operational
  count, chosen and not measured, not an exposure value.
- **PROTECTED**: no edit to `.github/**`, `scripts/**`, `.claude/**` or
  `infrastructure/environments/*` is made or required. The observability rule
  file will need a line for the new counter after it lands; that file is
  protected and is described here, not edited.
- **BILLED**: nothing. The counter and the journal fields add no resource, and
  re-verification runs in the process.
- **PAPER-BOUNDARY**: nothing. Every figure here is simulated, a statistic or
  a label; no order path, venue or autonomy level is touched, and no
  counterfactual result can enlarge a size, a cap or a model's authority.
