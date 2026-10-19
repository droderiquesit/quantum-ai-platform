# ADR 0145: An ambient draft has no identified effect, so the unidentified effect is recorded and acts on nothing

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval
  ("I allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). That approval authorises this record's engineering choices; it
  does not move ADR 0114 D8's refusal of external action, ADR 0141's refusal
  of any level above narrow-reversible, or the paper-trading boundary, and
  none of them is touched.
- **Date**: 2026-10-19
- **Extends**: ADR 0114 Amendment 1 (D8: one drafting adapter whose only
  effect is an internal outbox), by saying what the platform may do with the
  one measured outcome that adapter produces.
- **Keeps**: ADR 0114 D8, ADR 0141 (authority is `ActionPolicy`'s and nothing
  above narrow-reversible is granted), ADR 0125 (an advisory record only
  subtracts), ADR 0005 (confidence is arithmetic), ADR 0003.

## Context

On the deep brain's default path, `AmbientAgency` writes a draft to the
internal outbox, holds it until a later sweep raises the same subject again,
and then runs observe-effect, attribute and update through
`qip_agency::engine::learn`. The effect it files is the subject's reading at
the follow-up sweep less its reading at the sweep that drafted: the status quo
carried forward is the no-action baseline. The attribution says what it is:
`Identification::NotIdentified`, identifiability `0.0`, one
`HorizonObservation`, the `Communication` policy held to a predicted uplift of
zero, the outcome kept in `ActionOutcomeMemory` as `Observed::Measured`, and a
`DraftLearning` journaled beside the outbox (or journaled withheld, naming the
stage that refused it).

Locate it with
`grep -n 'fn learn_stages\|Identification::NotIdentified\|identifiability: 0.0\|engine::learn(' backend/crates/apps/qip-deepbrain/src/ambient_agency.rs`.

Fourteen register rows are PARTIAL because that effect is *measured, not
identified* and each asks for something that would make it more: an identified
estimator (AGENCY-1006), a structural model to estimate the counterfactual
(AGENCY-1025), a feed into the world-model federation or the causal graph
(AGENCY-1184, 1199, 6078, COVERAGE-7189), A/B variants (AGENCY-7030), a second
horizon (AGENCY-6104), a federation-backed simulator (COVERAGE-7186), a gate on
drafting (AGENCY-6108), and a full `engine::run` pass on the ambient door
(AGENCY-1182, 6105, 6106, 7027).

Read against the code, three things the rows say need correcting before
deciding.

First, **the refusal to feed the federation is a convention, not a structure.**
`Federation::update_from_attribution` (`qip-world-model/src/federation.rs`)
calls `attribution.validate()` and checks the goal id names a live model, and
nothing else. `validate` refuses an *unidentified* attribution only when it
claims identifiability above zero; a `NotIdentified` attribution at zero is
valid, and the method would journal it as `Event::AttributionApplied`, whose
payload carries the effect's mean and deviation and **not** the identification
marker. The method's own doc says "the identifiability marker tells whether
the effect is causal or merely correlational" and the journal record it writes
drops that marker. The only thing between an ambient null effect and a
federation journal entry is that no production code calls the method
(`grep -rn 'update_from_attribution' backend/crates --include=*.rs` finds the
definition and tests in `qip-world-model` only).

Second, **the memory can already recommend on the unidentified effect.**
`ActionOutcomeMemory::recommend_for` (`qip-agency/src/memory.rs`) reads
`Observed::Measured` records, keeps a sequence with at least `min_support`
rows, and returns the one whose mean `observed - counterfactual` is strictly
positive. It never reads `attribution_confidence`. The ambient door files its
outcomes as measured at confidence zero, so on every running path the only
records `recommend_for` can read are unidentified ones, and a subject whose
deviation happened to rise after three drafts would yield `Some(..)`. The
door's `ask` only counts the answer (`recommended_draft`), which is why
nothing acts on it today; the guard is the door's discretion, not the memory's
behaviour. The comment in `tests/ambient_agency.rs` that "the draft's effect is
held at zero" describes the *predicted* effect, not the filed one.

Third, **ADR 0114 does not contain the word "experiment".** The module
documentation of `ambient_agency.rs`, the register and the retriage all say
"ADR 0114 refuses an experiment on the people a draft reaches". 0114 D8
refuses external action and approves drafting; the experiment refusal is a
reading of it (the reader of the outbox is the only actor, and randomising
what that person is shown is not drafting). This record makes that reading its
own decision rather than an inherited citation, so the revival condition below
points at something that exists. Also stale: row AGENCY-1025 cites
`estimate.rs::no_action_baseline`; the symbol is
`InterventionPlan::no_action_baseline` (`qip-contracts/src/expansion.rs`), a
declared string on the plan, and the ambient counterfactual is the
`no_action_counterfactual` the door files from `PendingDraft::reading`.

## Decision

1. **The ambient effect is recorded as what it is, and no more.** A follow-up
   reading less the drafting-sweep reading is filed `NotIdentified`, at
   identifiability zero, with its confounders and rival explanations named, and
   journaled as `DraftLearning`. It is evidence of exactly one thing: *the
   draft is not shown to move the subject.* It is never described as an effect
   of the draft, a lift, or a causal estimate, in a record, a metric or a
   register row.
2. **It acts on nothing.** Each consumer is named and closed:
   - *The world-model federation.* `Federation::update_from_attribution`
     refuses an attribution whose `identification` is not identified, with a
     message naming this record. The refusal is in the method, so a future
     caller cannot reach the journal with a correlation by forgetting a check
     the caller held (principle 4).
   - *The causal graph.* `AffordanceGraph` has no operation that takes an
     attribution or an estimate, and none is added: a tool edge carries
     declared reversibility, latency, cost and authority, not a learned
     strength. The register's "update an edge" has nothing to update, and
     inventing a learned-strength field to receive an unidentified number is
     the failure, not the closure.
   - *The memory's recommendation.* `ActionOutcomeMemory::recommend_for`
     counts only records whose `attribution_confidence` is above zero. A
     record is kept regardless (the log and the memory are the record of what
     happened); it is not evidence a recommendation may rest on. Because
     `engine::attribute` refuses a claimed confidence above the attribution's
     identifiability, and `EffectAttribution::validate` refuses identifiability
     above zero on a non-identified effect, "confidence above zero" is exactly
     "rests on an identified attribution". Until one exists the door's `ask`
     answers "nothing", which is the truth.
   - *Drafting.* The memory's answer does not gate a draft (AGENCY-6108). The
     door asks, counts and drafts. A draft withheld because the subject
     "drifted after the last one" would be an action taken on a correlation.
   - *Authority.* `ActionPolicy::widen` already refuses a non-identified
     attribution, and the deep brain's policy starts at `Shadow` with no widen
     call anywhere on its path. Unchanged.
3. **The platform assigns no treatment.** No A/B draft variants, no seeded
   hold-out of drafts, sweeps or subjects, no content perturbation
   (AGENCY-7030). Two drafts per opportunity defeat `AmbientIntake`'s dedup on
   the signal id, and a hold-out is an experiment under another name. No
   `estimate.rs` estimator is fed the ambient outcome, because none has a
   control; `operator_formed_comparison` and `Experiment` stay what they are
   (the latter draws its split from a committed seed and is the only source of
   an identified label).
4. **No structural model of "a notice to a person changes X" is written.**
   The ambient path declares none, and a model written per opportunity would
   restate the declaration (the fault AGENCY-1056's audit named). The
   no-action counterfactual stays the status quo carried forward, stated as
   such in `counterfactual_method`. Row AGENCY-1025's "estimated, not assumed"
   is therefore recorded as *assumed, by decision, and labelled* (decision 1).
5. **The ambient door runs the guard, not `engine::run`.** The draft is one
   tool, one step, to one internal adapter; its candidates are "draft" and "do
   nothing", and the choice was already made by the attention router and the
   promotion policy before the door sees the signal. Running eleven stages over
   that restates the standing policy as a plan and needs a structural model
   decision 4 refuses. `DraftLearning` keeps its eight seated-not-run stages
   and its documentation that `passed` is evidence from `ObserveEffect` on
   (AGENCY-1182, 6105, 6106, 7027). The shadow binary (`qip agency shadow`) is
   where `engine::run` runs; it simulates against declared structural models
   and the twin's futures.
6. **No federation-backed simulator.** A `ScenarioSource` over federation
   members is not built (COVERAGE-7186). A federation view is a probability
   over a proposition, not a structural model of what a lever does, and ADR
   0125 keeps those records subtractive; letting them supply the scenarios an
   engine ranks candidates by would let an advisory view choose among actions.
7. **A second horizon exists only where a producer observes one.**
   `EffectAttribution::validate` keeps requiring at least one distinct horizon
   (AGENCY-6104); it is not raised to two, which broke fourteen tests for a
   property no producer can supply: the simulator evaluates its structural
   model at the plan's one `time_to_effect_secs`, and the ambient door removes
   a draft from `pending` at its first follow-up. Adding a second sweep to
   the door would add a second horizon *to an unidentified effect*, which
   nothing consumes; no producer is built without a named consumer. A producer
   that really observes a second horizon (a simulator path at another time, a
   second follow-up) states it, and a record that gates authority on
   multi-horizon evidence would then require two.
8. **What would change this** is stated once, so it is a condition and not a
   mood: an identified estimator fed by a control that a new ADR amending 0114
   authorises (who is held out, who consents, what it costs the desk), *and* a
   consumer that needs the answer (a record under ADR 0141 granting a level
   whose evidence requirement it satisfies). Either alone changes nothing.

## Consequences

- "Measured" and "identified" stay two words. The memory, the journal and the
  register can say a draft's effect was measured and unidentified without any
  consumer treating the first as the second.
- Code slices left, each with a mutation: the federation refusal and its
  test; the `recommend_for` confidence filter and its test; one acceptance
  suite pinning the boundary by source inspection (the idiom of
  `qip-acceptance/tests/scope_inspections.rs`). The other eleven rows close by
  restating register wording.
- The `Event::AttributionApplied` journal record is not changed; it is simply
  unreachable for an unidentified attribution. Adding an identification field
  to it is a sealed-record change and is not needed once the method refuses.

## What it costs

- The platform will not learn from drafting. After a thousand drafts the
  memory holds a thousand measured nulls and recommends nothing, by design.
  The cost of that is the information a hold-out would have bought, and it is
  real: the desk never learns whether an ambient draft is worth the attention
  it takes to read.
- `recommend_for` becomes less permissive. A caller that files measured
  outcomes at confidence zero and expects recommendations (none exists in the
  tree; the CLI files simulated ones, which it never reads) now gets `None`.
- The `Event::AttributionApplied` rows in `qip-world-model` tests keep passing
  because they use `Identification::Experiment`; a test that built an
  unidentified attribution to exercise the happy path would have to change.

## What would make this wrong

- If the desk read the outbox and acted on drafts at a rate that made the
  draft the main cause of the subject's next move, "not shown to move
  anything" would be a measurement defect rather than a truth. The fix is
  instrumenting the reader (an acknowledgement record in the outbox's own
  log), not an estimator over the subject's reading.
- If any consumer other than the three named were found taking
  `attribution_confidence` or the effect mean from an ambient record, the
  list in decision 2 is incomplete; the acceptance suite's source pin is the
  place it would show.
- If `recommend_for` were wanted on simulated outcomes, the confidence filter
  would need to be keyed on `Observed` as well. It is not: the CLI's
  simulated records never reach it (`simulated.rs` says so).

## Dissent

The strongest counter-position is that a seeded hold-out is cheap, honest and
already half built. `estimate::Experiment::design` assigns units from a
committed seed, so the control is reproducible from the log by anyone holding
the seed; the unit could be a *sweep* or a *subject class* and not a person;
the people "reached" are the owner's own research desk, who can consent; and
an identified answer to "does an ambient draft move anything" is exactly what
a record under ADR 0141 would need before any authority is granted. Refusing
it forever, the argument runs, leaves the agency permanently at `Shadow` by
absence of evidence.

It lost on three grounds. The outcome variable is the subject's own reading at
the next sweep, which depends on the market and on whatever the desk does for
reasons the draft did not cause; with nothing in the process reading the
outbox, the treatment (a draft was written) has no measured path to the
outcome, so even a clean hold-out estimates the effect of *writing a file*,
which is zero by construction and informs no one. The cost is not zero: a
held-out sweep is a signal the router judged worth waking a specialist for
and the desk never sees. And each consumer of the answer is refused on
independent grounds (decision 2), so the experiment would buy a number with
nobody to give it to. It is revived by decision 8: a consumer that needs it
and an ADR that amends 0114 to say who is held out and who agrees.

## Code it constrains

- `qip-deepbrain/src/ambient_agency.rs`: `AmbientAgency::learn_stages`,
  `learn_from`, `ask`, `await_follow_up`, `DraftLearning` —
  `grep -n 'fn learn_stages\|fn learn_from\|fn ask\|fn await_follow_up\|struct DraftLearning' backend/crates/apps/qip-deepbrain/src/ambient_agency.rs`.
- `qip-world-model/src/federation.rs`: `Federation::update_from_attribution` —
  `grep -n 'pub fn update_from_attribution' backend/crates/services/qip-world-model/src/federation.rs`.
- `qip-agency/src/memory.rs`: `ActionOutcomeMemory::recommend_for` —
  `grep -n 'pub fn recommend_for' backend/crates/services/qip-agency/src/memory.rs`.
- `qip-agency/src/affordance.rs`: `AffordanceGraph` (must stay without an
  attribution-taking operation) —
  `grep -n 'pub fn ' backend/crates/services/qip-agency/src/affordance.rs`.
- `qip-contracts/src/expansion.rs`: `EffectAttribution::validate` (one distinct
  horizon minimum, unchanged) — `grep -n 'pub fn validate' backend/crates/libs/qip-contracts/src/expansion.rs`.
- `qip-agency/src/estimate.rs`: `Experiment`, `operator_formed_comparison`
  (untouched; the only sources of an identification label).

## Rows unblocked

- **R12-AGENCY-1006** — decided: the learned effect is measured and
  unidentified by decision; an identified estimator waits on decision 8. Restate.
- **R12-AGENCY-1025** — decided: the counterfactual is the status quo carried
  forward and says so; no structural model is written; the cited symbol was
  `InterventionPlan::no_action_baseline`, not an `estimate.rs` item. Restate.
- **R12-AGENCY-1182** — decided: the ambient loop closes on one step by
  design; `engine::run` is the shadow binary's. Restate.
- **R12-AGENCY-1184** — decided and built: `update_from_attribution` refuses an
  unidentified attribution.
- **R12-AGENCY-1199** — decided and built: same refusal; the attribution
  updates policy and memory, never a world model, until identified.
- **R12-AGENCY-6078** — decided: the graph has no edge-update operation and
  gets none; boundary pinned by an acceptance suite.
- **R12-AGENCY-6104** — decided: one distinct horizon stays the minimum;
  a second comes only from a producer that observes it. Restate.
- **R12-AGENCY-6105** — decided: no per-opportunity structural model; the
  ambient path declares none. Restate.
- **R12-AGENCY-6106** — decided: the deep brain door runs the guard, not the
  engine; its eight seated stages stay documented as seated. Restate.
- **R12-AGENCY-6108** — decided and built: `recommend_for` ignores zero-
  confidence records; the answer gates nothing.
- **R12-AGENCY-7027** — decided: no causal simulation on the ambient path;
  the in-process guard is the policy and identity component. Restate.
- **R12-AGENCY-7030** — decided: no assignment, no variants, no hold-out.
  Restate.
- **R12-COVERAGE-7186** — decided: no federation-backed simulator. Restate.
- **R12-COVERAGE-7189** — decided and built: an unidentified measured effect
  updates neither the federation nor the causal graph.

## Not decided here

No item below is one of the four hard limits; each is left to a record that
has to say so.

- Any randomised hold-out, assignment or perturbation of what a reader is
  shown: needs an ADR amending 0114 (decision 8).
- Any adapter whose effect leaves the internal outbox, and any authority above
  `Shadow` on the ambient path: ADR 0114 D8 and ADR 0141 stand.
- Instrumenting whether the outbox is read (an acknowledgement record): not
  needed by any consumer yet; a candidate if decision 8 is ever pursued.
- BILLED, RISK, PROTECTED, PAPER-BOUNDARY: none created, changed or loosened.
  The paper-trading boundary is untouched; nothing here reaches an order.
