# ADR 0238: An acting problem the constraints leave unsatisfiable is unresolved, keeps its trace and moves no money, and a contested thesis is scored in shadow and not sized

- **Status**: **Accepted 2026-10-31 under the owner's delegation**: decision
  10 of `docs/blueprint/owner-decisions-2026-10-31.md`, whose ruling for
  R12-RES-7106 is recorded in full in
  `docs/blueprint/lanes/wave1031-owner-rulings-2026-10-31.json`
  (ADR_THEN_IMPLEMENT). The limits of that delegation hold here. This
  record sets no risk number, changes no limit, threshold or limit set,
  edits no rule of the shipped pack, and touches no paper-trading layer. It
  only removes a road to sizing.
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

## What changes and what does not

- **Changes.** An approved thesis whose acting constraints are
  unsatisfiable forms no proposal where it used to be sized small. Every
  such derivation record carries `Unresolved` instead of `Feasible`. The
  REASON detail line gains `, unresolved, scored in shadow` for it. Under
  a tripped kill switch an approved thesis now stops in REASON. Before, it
  reached ACT's `kill-switch` refusal gate. ACT's gate still refuses
  anything queued by another road.
- **Does not change.** The shipped rule pack and its version. Every risk
  limit, threshold, limit set, capital grant and multiplier. The veto's
  hard stops and their ordering. `contradictions`, the `Infeasible` mark
  and its alternates. `confidence_for_sizing`, which still narrows a thesis
  that is sized. The three paper-trading layers: Terraform's refusal of the
  live rungs, `AutonomyLevel::deployable` in the composition roots, and the
  edge `Cell`'s paper-only constructors.
- **Records written before this one** carry `Feasible` for an unsatisfiable
  acting problem. They no longer replay, because `verify` recomputes the
  mark and finds `Unresolved`. That is the same position ADR 0212 put
  older records in. The record says what the platform concluded under the
  rule then in force, and the replay says the rule has changed.

## What it costs

- **The paper book empties on every fixture tape probed.** The tape the
  kernel's own tests drive, a nine percent drop the panel contests, used to
  open a position and now opens none. The cost is wider than dissent. The
  confidence premise of `r10-eligible` (`confidence_bp >= 5000`) used to be
  a sizing input and is now a stop. REASON's prior is 0.25, so on one
  instrument the default panel lands below it. A throwaway probe on
  2026-10-31 ran the default cycle over 192 single-instrument tapes. The
  last bar's move ranged over ±5, ±9, ±15 and ±30 percent, with four
  noise amplitudes, three drifts and two volumes. The highest admitted
  confidence recorded was 4823 bp. No tape produced an eligible thesis,
  so none formed a proposal. The probe was not committed. The owner
  accepted this in the ruling: "If that empties the paper book, that is
  what the eligibility rule says, and it is no reason to size what the
  rule refuses." This record does not change the 5000 bp premise, which
  would be a threshold change. Whether a fixture can reach eligibility
  without one is the owner's question, and it is recorded below.
- **Tests whose premise is a traded cycle fail.** A test that drives the
  default cycle and asserts a proposal, a position or a solve as its
  *premise* now fails that premise. The property it guards is downstream,
  in valuation, capital, solver routing or position records, and is not
  disproved. These tests are not loosened, skipped or rewritten here. The
  rule is never to weaken a test to pass. Restoring them needs one of
  three things, and each is the owner's call:
  1. a fixture that reaches eligibility under the shipped pack;
  2. a test-only seam to set the kernel's rule pack from outside the
     crate, which ADR 0212 decision 7 forbids today;
  3. a change to the eligibility premise, which is a threshold change.

  The lane's report lists them by name.
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
  this one. `feasibility_of` checks contradictions first.
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
