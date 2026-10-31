# ADR 0257: The capital-floor member forecasts the book's floor and the settlement member forecasts settlement, each resolved from the ledger's own records, and neither enlarges a size

- **Status**: **Accepted 2026-10-31 under the owner's delegation**. The
  owner's ruling on R12-WORLD-1178 (ADR_THEN_IMPLEMENT) handed the row to
  lane L2 in `docs/blueprint/lanes/wave1031-handoff-2026-10-31.json`, and
  the integration parent relayed it to lane L2-INTEL. The ruling named
  ADR 0194 for this body, but 0194 had already been written for the
  funding-draw member (R12-WORLD-3048). This record therefore takes a new
  number from L2's block (decision 15). It sets no risk number, adds no
  threshold, and touches no paper-trading layer.
- **Date**: 2026-10-31
- **Extends**: the federation stage
  (`backend/crates/runtime/qip-kernel/src/federation_stage.rs`), ADR 0194's
  capital member, and R12-WORLD-3049's counterparty member. It does not
  amend either of them.
- **Rows**: R12-WORLD-1178.

## Context

R12-WORLD-1178 lists the scope a world-model federation must cover. Two of
its items are portfolio/capital and counterparty models. Each already has
one member. ADR 0194's member asks whether the next pass finds less cash.
R12-WORLD-3049's member asks whether the next pass records a feasibility
refusal at a venue. Neither asks the two questions the owner's ruling
named: how close the book is to its floor, and whether its sales have
settled. The kernel already holds both answers. The survival policy
withholds an emergency reserve as a NAV floor, and the platform keeps every
desk sale still in settlement.

## Decision

1. **A capital-floor member, `portfolio.capital-floor.global`** (domain
   `portfolio/capital`, one pass, observational). Its proposition is
   `capital:floor-headroom-narrows-next-pass@{cycle}`. The question is
   whether the next pass finds less equity above the NAV floor than this
   one did. The headroom is equity less the survival policy's
   emergency-reserve fraction of that equity. That is the same policy and
   the same fraction the waterfall withholds as the NAV-floor tier when
   DECIDE sizes. The view is a stated prior, `0.2 + 0.5 × leverage`
   (clamped to `[0, 1]`), because an exposed book is the one a mark can
   move. The member is silent where the floor does not compute.
2. **A settlement member, `counterparty.settlement.global`** (domain
   `counterparty/network`, one pass, observational). Its proposition is
   `settlement:proceeds-outstanding-next-pass@{cycle}`. The question is
   whether the next pass still holds desk sale proceeds in settlement. It
   reads `Platform::unsettled_sale_proceeds`. The view is a stated prior:
   0.7 when proceeds are outstanding now, because a settlement cycle is
   longer than a pass, and 0.1 when none are. A total that overflowed
   reaches the member as NaN, so the member is silent and does not read it
   as zero.
3. **Each is resolved from the ledger's own records.** The next pass's
   headroom and the next pass's unsettled total answer them, from the same
   `CapitalView` the platform builds every cycle. Calibration scores both
   priors like any other member.
4. **Neither can enlarge a size.** The federation's only power over DECIDE
   is `narrowings_of`, and it reads instrument propositions only. Neither
   proposition names an instrument, so neither member can narrow a size,
   let alone widen one.

## Consequences

- `federation_stage.rs::the_floor_and_settlement_members_forecast_from_the_book_and_the_next_pass_answers_them`
  feeds both members, and the next pass answers each of them both ways. A
  book whose floor does not compute leaves the floor member silent and the
  settlement member fed. The test also removes both members' arbitrations
  and asserts that every narrowing is unchanged. Each of these mutations
  fails the test: reversing either resolver, or dropping the
  outstanding term of the settlement prior.
- `qip-kernel/tests/symbolic_federation.rs::every_cycle_journals_one_federation_review_and_each_replays_and_reproduces_its_arbitrations`
  runs the platform's own cycle. Neither member is silent on any cycle, and
  the population is eighteen. Building the view with no headroom fails the
  test, and so does building it with a settlement total that never
  converts.
- **What this does not close.** R12-WORLD-1178 lists fourteen scope items.
  This record adds two members inside two of them. Collateral,
  optionality and capital mobility stay unmet in the capital family, as
  ADR 0194 says. The row stays PARTIAL until each item maps to a member
  reached from a composition root with a named test.

## What it costs

- Two more members feed every pass, so two more propositions are
  arbitrated and journaled per cycle, and the federation review grows by
  that much.
- The floor is proportional to equity, so on the shipped policy the
  headroom narrows exactly when equity at cost falls. The floor member
  therefore forecasts an equity fall stated in the floor's terms, not a
  movement of the floor itself. A floor that moved on its own, such as a
  reserve raised by an operator, would be read as headroom narrowing in
  the pass it moved.
- Both views are priors and not fits. Until calibration has enough
  resolutions to score them, they are numbers somebody chose. They are
  stated here so a reader can see that.

## What would make this wrong

A NAV floor that is not the survival policy's emergency-reserve fraction of
equity. If the waterfall's floor came from another source, such as a
configured absolute floor, this member would forecast a floor DECIDE does
not size under. The headroom in `Platform::capital_view` is the line to
change. Also wrong: a settlement cycle shorter than a pass. Every sale would
then settle before the next pass could see it outstanding, and the
settlement member would forecast a proposition that is always false.
