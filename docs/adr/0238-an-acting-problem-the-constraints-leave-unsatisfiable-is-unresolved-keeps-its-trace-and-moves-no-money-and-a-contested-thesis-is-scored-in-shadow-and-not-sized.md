# ADR 0238: An acting problem the constraints leave unsatisfiable is unresolved, keeps its trace and moves no money, and a contested thesis is scored in shadow and not sized

- **Status**: **Accepted 2026-10-31 under the owner's delegation**: decision
  10 of `docs/blueprint/owner-decisions-2026-10-31.md`, whose ruling for
  R12-RES-7106 is recorded in full in
  `docs/blueprint/lanes/wave1031-owner-rulings-2026-10-31.json`
  (ADR_THEN_IMPLEMENT). The limits of that delegation hold here. This
  record sets no risk number, changes no limit, threshold or limit set,
  edits no rule of the shipped pack, and touches no paper-trading layer. It
  only removes a road to sizing. Owner rulings 18 and 20 on the same day
  settled how it lands: the tests change their scenarios rather than the
  pack, the record version marks the rule change, and "contested" is read
  narrowly (decisions 7 to 11). Against main this record is a tightening,
  because contested theses used to be sized small.
- **Date**: 2026-10-31
- **Amends**: ADR 0212 decision 1, and ADR 0150 decision 14 as ADR 0212
  scoped it. A contested thesis is no longer sized small; it is unresolved
  and scored in shadow.
- **Keeps**: ADR 0212 decisions 2 to 7. A symbolic contradiction is still
  `Infeasible`, outranks everything here, and keeps its own words and
  alternates.
- **Rows**: R12-RES-7106.

## Context

R12-RES-7106 reads: "On symbolic contradiction/unsatisfied constraints the
conclusion/action is marked infeasible or unresolved, the trace retained,
alternates searched, and money-moving invariants fail closed."

ADR 0212 met the first half, symbolic contradiction, and declined the
second. Its decision 1 observed that every contested thesis has an
unsatisfiable acting problem, because `r10-eligible` requires an
uncontested panel, and argued that reading "unsatisfied constraints" as
covering it would block dissent, which ADR 0150 decision 14 had chosen to
size small. The derivation was marked `Feasible`, `stage_reason` queued it
on `pending_theses`, and DECIDE sized it on
`Hypothesis::confidence_for_sizing`.

An independent skeptic refuted the row's COMPLETE claim on part 4 of the
AUDIT-5 bar (`docs/blueprint/lanes/audit-5-bar.md`): a clause an ADR
withdrew is unmet unless the requirement's own text allows withdrawing it,
and this one does not. The owner ruled on 2026-10-31:

> The requirement stands as written, and I will not narrow it to
> self-contradiction to pass the row. When the platform's own acting
> constraints are unsatisfiable, the conclusion is Unresolved: its trace
> and alternates are kept, it is scored in shadow, and it is never queued
> to be sized. Dissent stays distinct from contradiction, so a contested
> thesis is Unresolved, not Infeasible. If that empties the paper book,
> that is what the eligibility rule says, and it is no reason to size what
> the rule refuses.

ADR 0212's distinction was right and survives. Dissent is not a
contradiction. What it got wrong was the consequence: "not a
contradiction" was read as "feasible, so size it", and the requirement has
a second mark for exactly this case.

## Decision

1. **A fourth mark, `Feasibility::Unresolved`.** `feasibility_of` returns
   it for every derivation whose acting step is
   `Step::Derived(Answer::Unsatisfiable { .. })` and whose parts do not
   collide. It carries two things:
   - `violated_assumptions`: the constraints that block acting, as
     `constraint: <id>`, the same lines the record's own
     `violated_assumptions` holds for its acting part.
   - `alternates`: the premises of the pack that would have to change for
     the acting problem to become satisfiable. For
     `acting-requires-eligibility`, each top-level premise of a rule
     concluding `eligible = true` that does not hold, with the value the
     record had (a contested thesis reads
     `r10-eligible: needs contested = false (recorded true)`). For
     `a-halt-forbids-acting`, each rule concluding `halted = true` that
     fired. The premises are read by the engine's own `Cond::holds`, made
     public for this, so the list cannot disagree with the evaluation that
     produced the verdict. When the verdict was refused there are no facts
     to search, and the alternates are kept as that refusal rather than as
     an empty list.

   The mark is journaled with the rest of the derivation under
   `kernel/symbolic`, and `verify` recomputes it on replay like the other
   two.

2. **A contradiction outranks it.** A derivation whose pack finds acting
   eligible against unsatisfiable constraints is both, and stays
   `Infeasible` with its contradictions and its revision-task alternates.
   The collision is the finding a reviewer must act on. `contradictions`
   is unchanged: an unsatisfiable acting problem on an ineligible verdict is
   still not a contradiction.

3. **Dissent is unresolved, not infeasible.** A contested thesis's
   derivation is `Unresolved`. Nothing reads `contested`, `dissenters` or
   the panel's coverage to decide this: the pack's `r10-eligible` already
   does, and the acting constraints follow from its verdict.

4. **`stage_reason` keeps it off `pending_theses`.** The arm comes after
   the infeasibility arm, the veto, review's rejection and the verifier
   society's refutation, and before justification and the queue. Each
   earlier arm names a different owner, so each is still reported in its
   own words: rejected on review, vetoed by the pack on a hard stop, or
   refuted by the society (ADR 0170). A refutation is the more specific
   finding, because it says the explanation is false and not only that the
   platform may not act on it. Every one of these arms forms no proposal,
   so the order changes only which reason the stage reports. An
   unresolved thesis keeps review's approval in the hypothesis status and
   the approved count, because review did approve it. The stage reports
   `symbolically unresolved, scored in shadow, no proposal formed: …`,
   naming the constraints and the alternates, and the episode records
   `not_sizeable`.

5. **Scored in shadow.** The claim is written by `record_prediction`
   before any of these arms, so an unresolved thesis is graded in LEARN
   against what its subject did, like any other claim. It moves no money:
   nothing is queued, no proposal leg forms, no order exists.

6. **Every unsatisfiable acting step, not only dissent.** On the shipped
   pack the acting problem is unsatisfiable when `r10-eligible` does not
   fire or a halt holds. That covers a confidence under 5000 basis points,
   a contested panel, a tripped kill switch, live authority, negative lot
   or cash headroom, and a contract expiring before the horizon. The hard
   stops among them keep their veto arm, which runs first. The rest were
   sized before this record, and none is now.

7. **A journal sealed before this record goes on verifying (owner
   decision 18, condition 5).** The mark is recomputed on replay, and ADR
   0212 sealed `Feasible` where this record computes `Unresolved`, so
   without a gate every journal written before the change would stop
   verifying, and the log could no longer reproduce its own history. The
   gate is the record's version. `SymbolicDerivation::SCHEMA_VERSION` is 2
   (`symbolic_stage::UNRESOLVED_SINCE_VERSION`). The fields did not
   change; the rule the mark is computed by did, and a version is how the
   log names that. `verify_sealed` recomputes the mark under the rule in
   force for the version the record's envelope names. `replay` and
   `replay_recorded` pass that version, and `verify` reads a record as
   this build's version. A version-1 record carrying `Feasible` for an
   unsatisfiable acting problem verifies. A version-2 record carrying the
   same mark does not, so the old rule cannot be claimed by a record
   sealed under the new one. A version this build does not know is
   refused. The version sits in the envelope, which the hash chain seals,
   so a record cannot be re-labelled to the older rule without breaking
   the chain. `SymbolicDerivation` is not in any schema lock: not in
   `qip-events/schemas.lock.json`, `qip-api`'s `COMMITTED_SCHEMAS` or
   `qip-kernel`'s `body_schema_lock.rs`. So no locked body changed
   version. The owner accepted this on 2026-10-31 (decision 20 (A)). The
   version bump marks the rule change and nothing else, and the body is
   not to be added to a schema lock on its account.

8. **Fixtures, not the pack (owner decision 18, option 1).** The rule
   pack and `r10-eligible`'s premises stay as shipped. There is no
   test-only seam to set the kernel's pack, and no threshold changed. A
   test whose premise is a traded cycle reaches eligibility by changing
   its scenario's inputs only, and every assertion stays byte-identical.
   The kernel suites share
   `backend/crates/runtime/qip-kernel/tests/support/eligible_panel.rs`,
   which adds four inputs:
   - a small daily drift, so the simulation analyst's bootstrap of the
     tape agrees with the reversion buy;
   - a macro calendar, so a second independent origin supports the claim;
   - a filed revenue figure three days before the drop, so the move has a
     catalyst;
   - forty readings of each alternative-data series, with the fixture's
     `licensed_datasets` naming those vendor datasets.

   A private fund needs the alternative data, because the equity analyst
   declines anything that is not equity. The licence list is fixture
   configuration, not scenario data, and it is named here so that nobody
   mistakes it for data. It admits only the vendor data the scenario
   itself supplies. The owner accepted it for fixtures only (decision 20
   (B)). The default `PlatformConfig` licenses exactly what it did before
   this record, which is nothing.
   `config::tests::the_default_configuration_licenses_no_dataset_for_decisions`
   holds that. The acceptance vertical slice already carried the macro
   calendar and a declared catalyst, so it needed only a drift of five
   basis points a day.

9. **What "contested" means (owner decision 20 (C), fixed before this
   record lands).** A contested thesis is one the panel disputes on its
   evidence or its direction: an analyst's finding against it,
   conflicting evidence, a contradiction. Two negative findings are
   objections, but they are not dissent about the thesis, and
   `OrganisationReport::contests_the_thesis` does not count them:
   - **(a) Risk control reporting a blocking limit.** Its direction is
     negative exactly when a limit blocks, and that limit still refuses at
     its own gate. Counting the limit a second time in REASON would remove
     the reduce-only exit and every hedge on a limited book.
   - **(b) The adversarial reviewer's re-argument objection.** It is
     raised only because the book already holds the subject. Counting it
     would mean never adding to, hedging or planning the exit of a held
     position. The reviewer now states the count
     (`holding_objection_count` beside `objection_count`). A review that
     raises any other objection is still dissent.

   The kernel feeds `contests_the_thesis` to the symbolic stage and to
   the REASON detail line's `contested` clause. The raw two-sided
   `is_contested` stays for the episode records that report it.
   The tests in `qip-investment-agents`'s `chief::contest_tests` are:
   - `a_risk_control_limit_report_alone_does_not_contest_the_thesis`;
   - `a_reviewer_objecting_only_that_the_book_holds_the_name_does_not_contest_the_thesis`;
   - `a_genuine_opposing_finding_still_contests_the_thesis`.

   The last one's kernel half, that such a thesis is unresolved and forms
   no proposal, is
   `symbolic_federation::a_cycle_whose_panel_is_contested_forms_no_proposal_and_journals_its_routing_as_unresolved`.

10. **A thesis whose own evidence contradicts it is unresolved and not
    sized.** That is stricter than ADR 0150 decision 14's "sized small".
    Evidence contradicts a thesis only through a finding against it,
    which is dissent, so the conflict narrowing in
    `Hypothesis::confidence_for_sizing` no longer narrows anything that
    gets sized on the default cycle. That property was removed on
    purpose. `valuation_seam` now asserts the new one, under the name
    `a_thesis_whose_evidence_contradicts_itself_is_unresolved_and_not_sized`.

11. **A tripped kill switch halts the cycle before anything trades,
    wherever it stops.** Since this record that stop is REASON, because
    `r10-eligible` requires `kill_switch = false`. `qip-acceptance`'s
    `a_tripped_kill_switch_halts_the_cycle_before_anything_trades`
    asserts the property itself: nothing moves, the report says halted,
    and the halt is on the record either as REASON's unresolved stop
    naming the switch or as ACT's `kill-switch` gate. ACT's gate still
    guards every order sent by any road, and it stays a control somebody
    can reach.
    `platform::decide_tests::acts_kill_switch_gate_refuses_every_sized_leg_once_the_switch_trips_and_fills_nothing`
    drives it directly.

## What changes and what does not

- **Changes.** An approved thesis whose acting constraints are
  unsatisfiable forms no proposal where it used to be sized small. Every
  such derivation record carries `Unresolved` instead of `Feasible`. The
  REASON detail line gains `, unresolved, scored in shadow` for it. Under
  a tripped kill switch an approved thesis now stops in REASON, because
  `r10-eligible` requires `kill_switch = false`. Before, it reached ACT's
  `kill-switch` refusal gate. ACT's gate still refuses anything queued by
  another road, but through the public API no road reaches it with the
  switch globally tripped (see What it costs).
- **Does not change.** The shipped rule pack and its version. Every risk
  limit, threshold, limit set, capital grant and multiplier. The veto's
  hard stops and their ordering. `contradictions`, the `Infeasible` mark
  and its alternates. `confidence_for_sizing`, which still narrows a thesis
  that is sized. The three paper-trading layers: Terraform's refusal of the
  live rungs, `AutonomyLevel::deployable` in the composition roots, and the
  edge `Cell`'s paper-only constructors.
- **Records written before this one** carry `Feasible` for an unsatisfiable
  acting problem at version 1, and go on verifying under ADR 0212's rule
  (decision 7). Records sealed from this one on are version 2.

## What it costs

- **The paper book empties on a bare price tape.** The tape the kernel's
  own tests drove, a nine percent drop the panel contests, used to open a
  position and now opens none. The cost is wider than dissent. The
  confidence premise of `r10-eligible` (`confidence_bp >= 5000`) used to be
  a sizing input and is now a stop. REASON's prior is 0.25, so on a bare
  tape the default panel lands below it. A throwaway probe on 2026-10-31
  ran the default cycle over 192 single-instrument price-only tapes. The
  last bar's move ranged over ±5, ±9, ±15 and ±30 percent, with four
  noise amplitudes, three drifts and two volumes. The highest admitted
  confidence recorded was 4823 bp. With the inputs of decision 8 a listed
  equity reached 0.54 to 0.61 and traded. The owner accepted this in the
  ruling: "If that empties the paper book, that is what the eligibility
  rule says, and it is no reason to size what the rule refuses."
- **Every test this record broke is resolved, and none was loosened.**
  Thirty-three were restored by their inputs under decision 8. Decision 9
  restored three more by their inputs: the illiquid-book test, the
  compounding test and the catalogue hedge test. Their assertions are
  byte-identical, and their scenarios only had to stop counting a limit
  report or a holding objection as dissent. Two tests changed what they
  assert, and each change is a decision of this record, not a test
  weakened to pass: the conflict-narrowing test (decision 10) and the
  kill-switch test (decision 11). Before decision 9 those three tests
  showed what a broader reading of "contested" would have cost: the
  platform could never add to, hedge or exit-plan a held position, and a
  book under any blocking limit formed no new thesis at all.
- ADR 0150 decision 14's learning argument, that the platform learns most
  from the questions it was least sure of, is now served by shadow scoring
  alone. A contested claim is still written, graded and calibrated. What
  is lost is the realised fill and slippage of a small position, which a
  claim cannot supply.
- An operator reading `not_sizeable` episodes sees more of them. The stage
  problem names which of the three arms produced each.

## What would make this wrong

- An unresolved thesis reaching `pending_theses` by any road.
  `a_contested_thesis_is_unresolved_not_a_contradiction_and_forms_no_proposal`
  fails if `stage_reason` queues it.
- A contested thesis marked `Infeasible`. The same test asserts the mark
  is `Unresolved`, and the ADR 0212 mutation (drop `eligible &&` in
  `contradictions`) still fails it.
- An eligible-and-unsatisfiable derivation losing its `Infeasible` mark to
  this one. `feasibility_sealed_under` checks contradictions first.
- A journal sealed before this record failing to verify, or a record
  sealed since verifying under the old rule.
  `symbolic_stage::tests::a_record_sealed_under_adr_0212_still_verifies_and_one_sealed_since_cannot_claim_its_rule`
  fails in either case.
- Shadow scoring silently stopping: an unresolved thesis whose claim is
  never written would be neither sized nor graded. The rewritten test
  asserts the claim is open.

## Evidence

- `backend/crates/runtime/qip-kernel/src/symbolic_stage.rs`:
  `Feasibility::Unresolved`, `feasibility_of`, `resolutions`,
  `SymbolicDerivation::unresolved`, and the mark in `detail` and `verify`.
- `backend/crates/runtime/qip-kernel/src/platform.rs`: `SymbolicReading`'s
  `unresolved` field, `reason_symbolically`, and `stage_reason`'s
  unresolved arm.
- `backend/crates/services/qip-reasoning-engine/src/rules.rs`: `Cond::holds`
  made public, with its reason.
- `platform::symbolic_contradiction_tests::a_contested_thesis_is_unresolved_not_a_contradiction_and_forms_no_proposal`,
  which replaces
  `a_contested_thesis_is_not_a_contradiction_and_still_forms_a_proposal_sized_small`.
  The old test asserted the property ADR 0212 decision 1 chose. This record
  reverses that property, so the test asserts the new one. It is not a
  weakening: it keeps every assertion that dissent is not a contradiction
  and adds that no proposal, leg or position forms, the claim is open, and
  the mark carries its constraints and alternates.
- `platform::symbolic_contradiction_tests::a_contradictory_derivation_yields_infeasible_with_its_trace_and_blocks_money_movement`
  keeps its assertions. Its control used to be the shipped pack on the same
  contested tape, which proved the tape trades. That pack now trades
  nothing on the tape, so the control became `admitting_pack`: the shipped
  pack without `r10-eligible`'s confidence and contested premises. It
  exists only in the test module, and the kernel still holds `rule_pack()`.
  The revised pack is the control plus the two colliding rules, so the
  two still differ by the collision alone. A third run adds the collision
  to the shipped pack. It asserts the mark is `Infeasible`, not
  `Unresolved`, and that the stage reports it as infeasible.
- Mutations, each run and each restored byte-for-byte with `cmp`. Making
  `stage_reason` queue an unresolved thesis failed the rewritten test at
  "an unresolved thesis was queued for sizing". Returning `Feasible` for
  an unsatisfiable acting step failed it at "a contested thesis was not
  marked unresolved". Emptying the alternates failed it at "the alternates
  do not name the premise dissent failed". Letting an unsatisfiable acting
  step outrank a contradiction failed the contradiction test at "a
  self-contradicting derivation was not marked infeasible". ADR 0212's
  mutation, dropping `eligible &&` in `contradictions`, still fails both.
- Replay gating (decision 7): `symbolic_stage::UNRESOLVED_SINCE_VERSION`,
  `feasibility_sealed_under` and `verify_sealed`, proven by
  `symbolic_stage::tests::a_record_sealed_under_adr_0212_still_verifies_and_one_sealed_since_cannot_claim_its_rule`.
  That test seals a record into an envelope, erases it and decodes it back
  the way `replay` does. Two mutations were run against it:
  - recomputing every record under the current rule in `verify_sealed`
    fails it at "a record sealed under ADR 0212 stopped verifying";
  - making the new rule unreachable fails its premise.
- The new default, pinned through the public API (owner decision 18,
  condition 3):
  `symbolic_federation::a_cycle_whose_panel_is_contested_forms_no_proposal_and_journals_its_routing_as_unresolved`.
  It reads the sealed log through `replay_recorded`. Two mutations were
  run against it:
  - queuing an unresolved thesis fails it;
  - returning `Feasible` for an unsatisfiable acting step fails it at "the
    contested thesis was not journaled unresolved: Some(Feasible)".
- The kill-switch stop in REASON (condition 4):
  `symbolic_federation::a_tripped_kill_switch_stops_an_eligible_thesis_in_reason_as_unresolved_and_nothing_moves`.
  Handing the symbolic stage `kill_switch: false` fails it at "the halted
  cycle's thesis was not stopped in REASON".
- Fixtures (decision 8): `tests/support/eligible_panel.rs`, used by these
  suites:
  - `capital.rs`, `position_record.rs`, `solver_routing.rs`,
    `symbolic_federation.rs` and `valuation_seam.rs`;
  - `platform::self_model_tests`, which uses it through `include!`.

  `qip-acceptance`'s `vertical_slice_decide.rs` and
  `federation_disagreement.rs` each gained a five-basis-point drift. In
  every one of these files the diff touches only fixture builders, call
  sites that feed a fixture, and comments; no assertion line changed.
