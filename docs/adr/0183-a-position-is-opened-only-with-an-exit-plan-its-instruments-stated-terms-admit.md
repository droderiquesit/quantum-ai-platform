# ADR 0183: A position is opened only with an exit plan its instrument's stated terms admit

- **Status**: **Accepted**, 2026-10-25, decided under the owner's delegation
  of 2026-10-24, recorded in
  [`docs/blueprint/owner-decisions-2026-10-24.md`](../blueprint/owner-decisions-2026-10-24.md).
  It adds no risk limit, threshold, limit set or capital budget, and changes
  none: it is an admission refusal on the construction path, not a sizing
  rule. A reading that needed a limit or a budget would have been decision
  7's territory and would have stopped here.
- **Date**: 2026-10-25
- **Rows**: R12-ASSET-4069.

## Context

R12-ASSET-4069 reads: "For every position the Asset Brain maintains an exit
plan and attribution. … A position cannot be opened without an exit plan;
its P&L attributes exactly to sources." The attribution half holds on LEARN.
The exit half did not: nothing on the construction path dated an exit, so a
position could be opened whose hypothesis named no horizon, or whose horizon
ended inside a lockup the instrument's own record declares.

A first attempt refused both cases and broke four private-fund tests, each
of which sized a short-horizon thesis on a locked-up fund and expected it to
be admitted. That is a real question: is a lockup a reason to admit a
position with no honourable exit (and plan a secondary sale for later), or
a term the exit plan must respect?

## Decision

1. **An exit plan that cannot be honoured is refused at admission.** Every
   leg `construct_from` sizes gets an exit plan before any capital is held:
   dated at the decision instant plus the horizon of the hypothesis the leg
   expresses (the horizon REASON approved, carried beside the thesis since
   R12-ASSET-4067), routed as a sale, and checked against the instrument's
   lockup by `ExitPlan::new`. A leg whose hypotheses carry no horizon, or
   whose exit falls inside the lockup, refuses the construction naming the
   leg, as every other refusal there does. Nothing is sized on an invented
   date.
2. **A declared lockup is a stated term, not a reason to admit without a
   plan.** The lockup is the catalogue's: a private asset's vintage origin
   plus its stated lockup term. It is read as a constraint the plan must
   satisfy. A secondary sale (`plan_secondary_exit`) does not escape it: the
   secondary route is planned for a position already held, priced off an
   evidenced discount, and starts no earlier than the lockup allows.
3. **The private-fund tests change their fixture, not their purpose.**
   Six tests sized a twenty-day thesis on a fund whose seven-year lockup
   was still running; five of them guard the mark (stale, unmarkable,
   weaker of two, the hedge and obligation statement), so their fixture's
   lockup is shortened to one year, expired before the test instant, and
   each asserts what it did before. The sixth
   (`a_held_position_record_carries_valuation_confidence_horizon_and_expected_liquidity`)
   asserted the lockup's dated return on a held fund; under this record
   that holding cannot be opened, so it now asserts the catalogue's exit
   for an expired lockup, and the lockup's dated return for a fund still
   locked up stays proven where it is computed
   (`valuation_plane.rs`,
   `a_private_fund_inside_its_lockup_is_not_counted_as_exitable_on_the_catalogue_s_word`).
   A new test asserts the refusal.

## Consequences

- A short-horizon thesis on a locked-up fund is refused at DECIDE with a
  reason naming the lockup's expiry. The desk sees it on the stage problems.
- The exit plan dated for each sized object is held beside the mandate and
  horizon records (R12-ASSET-4066/4067) on the same terms: written once the
  proposal's capital is held, bounded by the universe, not rebuilt on a
  restart.

## What it costs

- A construction with one inadmissible leg is refused whole, not trimmed:
  dropping the leg would re-solve a portfolio nobody reviewed.
- The plan is dated from the hypothesis horizon, which is an estimate; the
  plan is a commitment to try, not a guarantee the market will be there.

## What would make this wrong

- If the desk needed to hold locked-up funds through theses shorter than
  the lockup (a deliberate long-dated allocation expressed as a sequence of
  short theses), the horizon the plan is dated from would need to be the
  allocation's, not the thesis's — a different record, and a new ADR.
