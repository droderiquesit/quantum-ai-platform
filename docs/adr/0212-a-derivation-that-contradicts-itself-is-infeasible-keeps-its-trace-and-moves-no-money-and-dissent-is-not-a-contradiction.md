# ADR 0212: A derivation that contradicts itself is infeasible, keeps its trace and moves no money; dissent is not a contradiction

- **Status**: **Accepted 2026-10-25 under the owner's delegation** ("Make my
  decision on all lets move", `docs/blueprint/owner-decisions-2026-10-25.md`,
  extending `docs/blueprint/owner-decisions-2026-10-24.md`). The limits of
  that delegation hold here. This record sets no risk number, changes no
  limit, threshold or limit set, and touches no paper-trading layer.
- **Date**: 2026-10-25
- **Amends**: ADR 0150 decision 14, by scoping it. It also draws one line
  inside ADR 0150 decision 13's "refused part" (see decision 4).
- **Extends**: ADR 0125 decision 2. The symbolic stage may subtract a
  proposal and never create, enlarge or re-sign one.
- **Rows**: R12-RES-7106.

## Context

R12-RES-7106 reads: "On symbolic contradiction/unsatisfied constraints the
conclusion/action is marked infeasible or unresolved, the trace retained,
alternates searched, and money-moving invariants fail closed. Check: unit:
A contradictory derivation yields infeasible with trace and blocks money
movement."

ADR 0150 decision 14 withdrew that row's blocking clause. Its argument was
about *dissent*. Agents disagree, `Hypothesis::confidence_for_sizing`
narrows, and blocking would mean "never form a view on a contested
question". That argument is right, and it is not about the row. The row is
about *symbolic contradiction*: a derivation whose own steps cannot all be
true together, or whose constraints are unsatisfiable against its own
conclusion. Those are different facts, and decision 14 treated them as one.

The gap was real in code, not only in prose. `SymbolicDerivation::veto`
reads only a *derived* rule verdict. When the rule pack's evaluation fails
because one rule sets a fact against a value another rule established, or
because a step reaches a state the pack's invariant forbids, the verdict is
`Step::Refused`. `veto` reads that as "not a rule firing" and returns
`None`. A hypothesis that review approved was then sized and sent, and the
record on the log said its own reasoning had collided.

The three cases look the same on a dashboard. Each now has its own owner:

| Case | What it is | Owner | Effect on money |
|---|---|---|---|
| Dissent | The panel disagrees; `contested` | ADR 0150 d14 | Narrowed by `confidence_for_sizing`, held small, scored |
| Refused part | A reasoner rejects a malformed input (a confidence outside `[0, 1]`, a refused timeline link) | ADR 0150 d13, ADR 0125 d2 | None today; d13's narrowing waits on the risk lane's multiplier |
| Symbolic contradiction | The derivation's parts collide | **This record** | None may move |

## Decision

1. **Decision 14 stands for dissent.** A contested thesis is still held
   small and scored. Nothing here reads `contested`, `dissenters` or the
   panel's coverage. A contested thesis's acting problem is unsatisfiable,
   because `r10-eligible` requires an uncontested panel. That is the pack
   and the constraints *agreeing* that the thesis is not eligible at full
   strength, and it is **not** a contradiction. Reading "unsatisfied
   constraints" as covering it would block every contested thesis, which is
   the conflation decision 14 was right to refuse.

2. **A symbolic contradiction is one of three collisions, each read off the
   record alone** (`symbolic_stage::contradictions`):
   - **The pack refused facts it was actually given.** `rule_inputs` refuses
     only a malformed confidence, and then it records no facts. A refused
     verdict that carries facts is therefore the pack's own evaluation
     colliding. Either a rule derived a fact against an established one, or
     a step reached a state an invariant forbids.
   - **A derived verdict that does not reproduce** under `RulePack::verify`.
   - **The pack finds acting eligible while the acting constraints are
     unsatisfiable.** The two parts answer "may the platform act" both ways.

   On the shipped pack (`reason/advisory-eligibility` v2) no real input
   reaches any of the three: its invariants are theorems of its rules. The
   control exists for the revision that breaks that, and the test drives one
   through the default cycle (see Evidence). A control that only a code
   change can trip is still a control. The code change is the realistic
   failure, and before this record that change would have traded.

3. **Infeasible, with the trace kept.** Every derivation now carries
   `feasibility: Option<Feasibility>`. The value is `Feasible`, or
   `Infeasible { contradictions, alternates }`. It is computed from the parts
   by `feasibility_of` and journaled with the rest of the record under
   `kernel/symbolic`. `verify` recomputes the value on replay and refuses a
   record whose mark does not reproduce. The trace is the whole record, not
   a summary: the pack's own sentence stays in `rules.verdict`, the refusal
   stays in `violated_assumptions`, and the stage detail says
   `infeasible (n contradiction(s))`.

4. **One line inside decision 13.** Decision 13 calls a derivation "with a
   `Step::Refused` part" a finding about the evidence that narrows rather
   than vetoes. That remains true of malformed input. It was never meant for
   a refused *verdict* that carries facts. That refusal is the pack
   contradicting itself or its facts, and it is decision 2's first case
   here. Refused timeline links, a refused posterior, abduction or
   intervention, and a rule refusal on a malformed confidence all stay with
   decision 13 and are not blocked by this record.

5. **Alternates are searched where the engine offers them, and they are
   advisory.** An infeasible mark carries the revision tasks that
   `RulePack::contradictions` raises on the facts the pack was given. These
   are `RuleAgainstRule`, `RuleAgainstObservation` and
   `InvariantAgainstObservation`, each citing the derivation's idempotency
   key as its evidence. A search the engine refused is kept as its refusal
   (`Step::Refused`), not as an empty list. Nothing reads the tasks to
   decide (ADR 0125). They are what a reviewer revising the pack starts
   from. `ContradictionResolution` and `ResolutionMethod` stay what decision
   14 left them: about sources disagreeing, with `trust_score` as their one
   consumer. They are not used here.

6. **The money-moving path fails closed on it, on the default cycle.**
   `stage_reason` reads `SymbolicDerivation::infeasibility` beside the veto.
   An infeasible conclusion is demoted to rejected, is reported as
   `symbolically infeasible, no proposal formed: …`, and is never queued in
   `pending_theses`. That queue is the only road to DECIDE's sizing, a
   proposal leg, and an order. The arm is checked before the veto and before
   review's verdict, because a contradiction is a finding about the
   reasoning itself and outranks what anyone said of its conclusion. A
   record with no mark at all is read as "not established" and refused too.
   No freshly derived record lacks one.

7. **The pack is held, not swapped.** `Platform` holds
   `symbolic_rule_pack`, set to `symbolic_stage::rule_pack()` at
   construction. Nothing outside the kernel's own tests sets it, and there
   is no setter. `derive_under` takes the pack by name. `derive` and
   `verify` use the build's pack, so a record written under any other pack
   does not replay as a proof.

## What changes and what does not

- **Changes.** A self-contradicting derivation forms no proposal where it
  used to be sized. Every derivation record gains a feasibility mark, and
  older records decode with `None`. The REASON detail line gains a clause
  only when a derivation is infeasible.
- **Does not change.** Dissent sizing (`confidence_for_sizing`, the conflict
  narrowing, the confidence governor). The veto's hard stops. Decision 13's
  treatment of malformed input. Every risk limit, threshold, limit set,
  capital grant and multiplier. The pre-trade checker. The three
  paper-trading layers: Terraform's refusal of the live rungs,
  `AutonomyLevel::deployable` in the composition roots, and the edge
  `Cell`'s paper-only constructors. A refusal on contradiction is not a
  numeric limit. It sets no number and only removes a proposal.

## What it costs

- A pack revision that introduces a collision stops every approved
  hypothesis it touches until the pack is fixed. That is intended: the
  alternative is trading on a proof that says it is wrong.
- `verify` now has one more thing to refuse a record on.

## What would make this wrong

- A contested thesis marked infeasible. The dissent test fails if that
  happens, and the fix is the definition in decision 2, not this record's
  intent.
- A contradiction class that blocks a case decision 13 owns, such as a
  refused timeline link. That would be silencing evidence findings, which
  ADR 0125 decision 2 forbids.
- A second road from REASON to `pending_theses` that does not pass the
  arm in decision 6.

## Evidence

- `backend/crates/runtime/qip-kernel/src/symbolic_stage.rs`: `Feasibility`,
  `contradictions`, `feasibility_of`, `derive_under`,
  `SymbolicDerivation::infeasibility`, and the feasibility check in
  `verify`.
- `backend/crates/runtime/qip-kernel/src/platform.rs`: `SymbolicReading`,
  `stage_reason`'s infeasibility arm, `reason_symbolically`, and the
  `symbolic_rule_pack` field.
- `platform::symbolic_contradiction_tests::a_contradictory_derivation_yields_infeasible_with_its_trace_and_blocks_money_movement`.
  Under a pack revision with two rules that conclude opposite values from an
  approved review, the default cycle journals the derivation as infeasible
  with the pack's words and a `RuleAgainstRule` alternate. It queues
  nothing, proposes no leg and opens no position. The same tape under the
  shipped pack proposes a leg and opens one.
- `platform::symbolic_contradiction_tests::a_contested_thesis_is_not_a_contradiction_and_still_forms_a_proposal_sized_small`.
  The contested thesis's acting problem is unsatisfiable. Its derivation is
  `Feasible`, it is sized below its admission confidence, and it proposes a
  leg.
