# ADR 0686: The placement optimiser proposes within the ledger, and the capital society reads the survival budget and the settled plan

- **Status**: Proposed 2026-11-15 by wave b52 lane 43. Written without a
  compiler: nothing below has been built or run, and the figures marked
  provisional are chosen in the restrictive direction.
- **Date**: 2026-11-15
- **Rows**: R12-CAPITAL-4090, R12-CAPITAL-4085, R12-CAPITAL-9047,
  R12-CAPITAL-4079.
- **Extends**: ADR 0229 (capital agents only propose), ADR 0230 (the
  coordinator settles in waterfall order), ADR 0159 (redemption notices).
- **Paper boundary**: untouched. Nothing here submits, sizes beyond the
  allocator's line, or reaches an order path. The three layers (the
  Terraform refusal in `variables.tf`, `AutonomyLevel::deployable` at the
  composition roots, and the `Cell` constructor) are not edited.

## Context

R12-CAPITAL-4090 asks for funding and collateral optimised across
currencies, venues, counterparties and legal entities while the ledger
stays authoritative. `qip-treasury::funding` names a source per requirement
in order and never compares sources by cost.

R12-CAPITAL-4085's society is built, but on every default path nine agents
abstain, the horizon gate budgets the pools from the allocator directly,
and the survival agent has no input.

## Decisions

1. **The optimiser is a pure function in `qip-capital::placement`.**
   `optimise(requirements, sources, ledger)` draws each requirement from
   its eligible sources, cheapest carry first, whole or not at all.
   Eligibility is exact match of currency and legal entity, and venue
   where a source is bound to one. It is greedy in the caller's order: the
   order is the priority between requirements competing for one cheap
   source, and the result is not a global optimum. That limit is accepted
   because a global solve needs a solver dependency, which needs its own
   ADR.
2. **The ledger is an input, never an output.** A source offered above its
   ledger balance, or under an id the ledger does not hold, is refused,
   not clamped (principle 2). Every plan is reconciled against the ledger
   before it is returned, and `reconcile` is public so any plan can be put
   the same question.
3. **No currency is converted.** A rate is a figure nobody supplied. A
   requirement with no same-currency source is left unplaced and says so.
   Cross-currency optimisation is open and needs a rate source (NEEDS_FIGURE).
4. **Carry cost is a caller-supplied figure** (basis points a year). The
   optimiser invents none.
5. **The survival agent reads the released budget.** DECIDE hands
   `CentralPlane::record_survival_budget` the figure the governor actually
   released, so the agent stops abstaining on a default path. The agent
   scales lines pro rata down to it and can never raise one (ADR 0229).
   Provisional and restrictive: the unit of the allocator's risk budget
   and of the released free capital are not asserted to be the same, and a
   released budget smaller than the allocator's lines narrows every line.
   Existing tests that assert unnarrowed allocator figures on a
   `Platform` may move; that is the intended direction.
6. **One path from the allocator to every reader.** `settle_through_society`
   serves both `allocate` and the horizon arming, so the gate's pool
   budgets and the deployed plan are one figure, and every arming records
   all ten agents' plans, which also makes the society run on cycles where
   `plan_pre_positioning` returns early.
7. **The coordinator's settlement is not yet read.** A restrictive reader
   (withholding a manifest whose cell cannot pay a liquidity, collateral
   or funding claim) was written and withdrawn: the existing test
   `competing_proposals_never_sum_past_the_grant` requires the cell to ship
   after its envelope expired, when every claim is cut, and weakening that
   test is not permitted. A reader needs a decision on that test first.
   No production caller supplies the coordinator's claims either: no
   per-cell liquidity, collateral or funding figure exists on a committed
   path.
8. **R12-CAPITAL-4079's route is not built.** An operator-authenticated
   declaration route on `qip-api` would be authorised in shape and
   refused in fact until a per-person credential exists (ADR 0065, ADR
   0076). A route written now would be dead code with a passing test;
   the row is NEEDS_PERSON.

## What would make this wrong

- A solver found worth its dependency would replace the greedy order.
- If the released budget and the allocator's notional are in different
  units, decision 5 narrows by a ratio that means nothing; the first
  pipeline run on `Platform` tests is the evidence.
