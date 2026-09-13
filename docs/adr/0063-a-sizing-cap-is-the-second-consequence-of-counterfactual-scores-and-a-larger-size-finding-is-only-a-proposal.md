# ADR 0063: A sizing cap is the second consequence of counterfactual scores, and a larger-size finding is only a proposal

- **Status**: Proposed
- **Date**: 2026-09-13
- **Supersedes**: nothing
- **Related**: ADR 0005 (confidence as arithmetic), ADR 0055 (a counterfactual result narrows sizing and only narrows it), ADR 0061 (rule regret is a proposal, a defence or a dormancy finding), ADR 0062 (a venue is withdrawn on feasibility evidence)

## Context

Blueprint §12.3's last row reads "alternative sizing consistently better…
the counterfactual says exactly where" → "sizing function adjusted". It has
two halves. ADR 0055 built the declined one: `Platform::counterfactual_
sizing_multiplier` reads the paths a control refused and halves an
instrument's sizing confidence when three in four of a sample of ten were
correctly declined, with no branch that returns more than one. The executed
half — whether the orders a venue actually filled should have been smaller
or larger — was scored by nobody: `Platform::evaluate_alternatives` could
price a placed order through the `OrderPlaced` entry in the outcome capture,
and the twin's `smaller_size` and `larger_size` arms exist for exactly that,
but nothing in production called it for a fill. `docs/DELIVERY-STATUS.md`
said so: "the executed-order half (`smaller_size`/`larger_size`) is not
scored."

The shared foundation of lanes B-2 and B-3 (`dd64c0a`) scores it now.
`score_filled` runs in LEARN under the same per-cycle cap as `score_
declined`, and each `FillScore` carries two bits — whether the smaller or
the larger size beat the twin's own `trade` arm on the same tape — and the
fill error the twin's entry price shows against the venue's. Two facts about
those bits were found by inspection and shaped what follows:

1. **The bits compare size arms with the `trade` arm, not with the realised
   outcome.** An opening fill realises nothing — `capture_submission` books
   it at zero P&L — so the design's `favours_the_alternative` on a size arm
   would read "this direction was profitable" and be true of `smaller_size`
   and `larger_size` at once on every winning fill. A cap armed on the
   first would have armed on the names that made money. Compared with the
   `trade` arm, the two are mutually exclusive except at equality, and a
   size arm that did not fill in simulation favours nothing.
2. **The budget equality makes a bare bound cut infeasible.** The
   constructor pins the gross at `Σw = min(target_gross, position_cap × n)`,
   and `run_cycle` constructs with one name, so a bound cut to half the cap
   under an equality still demanding the whole cap is "no feasible sizing"
   on every capped cycle.

The question this record settles: what may the platform do with the
executed half, given that ADR 0055 already narrows the budget on the
declined half and §12.4 forbids loosening on counterfactual evidence?

## Decision

**A pattern of fills that would mostly have done better smaller arms a cap
on that instrument's weight bound — one half, floored at the minimum
position, with the budget equality lowered by what the cap took — and a
pattern that would mostly have done better larger is a journaled proposal
and nothing else.**

- **A bound, not a second multiplier.** ADR 0055 narrows the *budget* the
  constructor is handed, and a budget cannot name an instrument: a second
  budget multiplier for one name's evidence would narrow every name in the
  construction and leave the proposal unable to say which one the evidence
  was about. `PortfolioConstructor::construct_capped` takes a map of
  object to multiplier in `(0, 1]` and narrows that name's upper bound;
  `construct` delegates to it with no caps, so its callers see no change.
  The proposal's own `compromises` then say which name was narrowed, from
  what to what, and what the gross gave up.
- **Refuse, never clamp.** A cap outside `(0, 1]` or naming an object not in
  the construction is refused: zero is a drop wearing a sizing's clothes,
  above one is the loosening §12.4 forbids, and an absent name is a caller
  that computed the wrong key.
- **The floor.** The narrowed bound is `max(position_cap × cap,
  minimum_position)`, because the drop rule after the solve removes any
  weight under the minimum, and a shrink that became a silent drop is the
  one failure a bound must not have. When the floor binds the compromise
  says so.
- **The equality.** With caps present, `achievable = cap-only gross − Σ
  (position cap − narrowed bound)` over the capped names, floored at zero.
  That reduction is what makes the shortfall *recorded, not reallocated*:
  the other names' own bounds are never touched by another name's cap, and
  because the total the equality demands falls by exactly what each cap
  took, no other name's weight is asked to make up the difference. An
  earlier version of this record and of `construct_capped` computed
  `achievable` as `min(cap-only gross, Σ upper)` instead — the sum of the
  (possibly narrowed) bounds — which is a different number whenever the
  uncapped names have enough headroom to cover the capped name's shortfall
  on their own (`Σ upper ≥ cap-only gross`, the common case with more than
  one name and a binding `target_gross`). In that regime the old formula
  left `achievable` at the pre-cap total, so the equality still demanded it
  and an uncapped name's own bound *did* absorb the capped name's foregone
  weight — the exact reallocation this bullet's first sentence says does not
  happen. An independent code review found this by tracing the shipped
  test's own fixture algebraically (two theses, `wide_mandate()`, one capped
  to half its bound: the old code left the gross at 0.8 and forced the
  uncapped name from ~0.20 to ~0.50) and it is fixed: the reduction now
  applies unconditionally, so the gross itself falls by what the cap took
  and the shortfall the compromise reports is that same number, never zero
  merely because something else happened to have room to hide it.
- **The arithmetic.** `sizing_review::cap_multiplier` returns
  `SIZING_CAP_MULTIPLIER` (one half, one auditable number as ADR 0055's
  discount is) when at least `SIZING_CAP_FAVOUR_FRACTION` of at least
  `SIZING_CAP_MIN_SAMPLE` scored fills on the instrument favoured the
  smaller size — three in four of ten, ADR 0055's bars by reference — and
  one otherwise, including when the larger side clears the same bars. No
  branch returns more than one. `Platform::sizing_cap_multiplier` computes
  it from `fill_scores` on every construction rather than caching it, so
  the bound DECIDE sizes under and the record LEARN wrote cannot disagree.
- **The record.** `review_sizing`, in LEARN after the venue review,
  journals a `SizingCapEntry` under `learning.sizing_reviewed` when a cap
  is armed or released — at the change, keyed on object, state and cycle,
  so a cap standing for a hundred cycles is one record — and a
  `SizingProposal` when the larger side's finding appears or evaporates.
  A journal failure changes no bound.
- **The proposal is the whole response to the loosening direction.**
  `larger_size_finding` returns a record with no multiplier on it, and
  `SizingProposal` carries `direction: "larger"` and an outcome and nothing
  a reader could apply. What a person does with it is a reviewed change to
  the mandate, which is the path a desk already has.

## Compounding, stated

ADR 0055 narrows the budget (`equity` handed to the constructor = free
capital × §6.2 × mark confidence × 0.5); this record narrows the weight
bound (`upper[i] ≥ minimum_position`). The drop rule is on the weight, so
with both active on one instrument the leg's weight is still at or above
the minimum and its position is `0.5 × 0.5 = 0.25` of the un-narrowed one.
The two read disjoint evidence: 0055 the declined scores, this the fill
scores, and the learning test that extends 0055's own holds both numbers
at exactly one half while the valuation-seam test drives four whole cycles
and holds the quarter on the book — as the position sized, budget times
target weight, because the reference cycle's own ACT stage fills its
proposal and the compounded cycle's traded notional nets against that
holding.

## Why the loosening direction cannot reach a bound

`cap_multiplier` has no branch above one; `larger_size_finding` returns a
type with no multiplier; `construct_capped` refuses a cap above one; and
`Platform::sizing_cap_multiplier` is the only producer of the caps map
`construct_from` builds. A mutation that armed the cap on the larger
finding, and one that returned a factor of two for it, each fail a test
that names the guardrail. The guardrail is held by there being no code
path, as ADR 0061 holds it for the rule rows.

## What it costs

- A ninth argument on the constructor's public surface, behind a
  delegating `construct`.
- The one-name limit stated above: the shortfall is a real number only
  when the narrowed bounds sum under the target. `run_cycle` constructs one
  name today, so it is real on every capped cycle.
- A `Decimal → f64` crossing in `construct_from`, once, on a multiplier
  that is exact in both representations, and the comment there says so.
- Two more records on a permanently retained Learn topic per instrument
  per state change — at the change, not per cycle.

## What would make this wrong

- **The trade-arm comparison.** The bits compare the size arms with the
  twin's re-pricing of the order as taken. If the twin's fill model is
  wrong in a way that scales with size — `qip_venue_fill_error_bps` is the
  series that would show it — the bits inherit the error. The cap is one
  half rather than a curve so that the damage a wrong bit can do is one
  bounded, named number.
- **The bars.** Ten and three in four by reference to ADR 0055; a desk
  whose fills are few will wait a long time for either finding. That
  convention is documented in the learning tests and is the same one ADR
  0055 rests on; if it is ever changed, both records must be re-read.
- **`smaller_favoured` used to be, and to a lesser extent still is, a
  win/loss signal rather than a sizing-efficiency one.** An independent
  code review found that comparing the smaller-size alternative's raw
  simulated P&L against the trade's armed the cap on *every* fill that
  lost money before costs at all, however small the cost paid — a smaller
  loss is trivially closer to zero than a larger one of the same shape,
  regardless of whether the size itself was the problem. A market that
  merely trended against every buy (the risk this section named before the
  fix) would arm the cap on direction, not on sizing.

  The fix: `Platform::score_filled` now also requires that the alternative's
  cost advantage over the trade — recovered from the twin's own cost-model
  breakdown, `gross = simulated_pnl() + simulated_costs()`, per
  `counterfactual.rs`'s own arithmetic, not a separately fitted estimate —
  is at least as large in magnitude as its directional disadvantage before
  `smaller_favoured` counts it. This closes the concrete defect: a pure
  directional loss, with cost a rounding error beside it, no longer arms
  the cap (`a_fill_that_lost_purely_to_an_adverse_price_move_does_not_
  favour_the_smaller_size`).

  What it does **not** do, stated plainly rather than implied by the fix's
  existence: it is not a marginal-cost analysis (it compares the *average*
  cost and gross the twin already computed for each arm, not the derivative
  of cost with respect to size), and it says nothing about whether either
  quantity was *material* — a fill whose cost and directional swing are
  both negligible can still clear the gate on noise the size of a rounding
  error in either direction. It is the minimum fix that removes the
  demonstrated bias, not a rebuilt, unbiased estimator of "was this size
  right for its edge."

  **The fix is asymmetric, and that is deliberate, not an oversight.**
  `larger_favoured` keeps the plain P&L comparison. Applying the identical
  gate to it is mathematically impossible to satisfy except at an exact
  equality: the *larger* alternative's extra cost is always a cost, never a
  saving, so "favoured" (the edge exceeded the extra cost) and "cost
  advantage at least matches the directional disadvantage" (the extra cost
  is at least as large as the edge that beat it) contradict each other for
  every value but one. Gating `larger_favoured` the same way would not
  reduce its bias; it would make the loosening-direction finding
  permanently unreachable — this repository's own standing example of what
  that looks like is `MaxExpectedShortfall`, a limit that could never fire
  and read as protection while being none. `larger_favoured` therefore
  still leans on win/loss more than `smaller_favoured` now does: a fill
  whose edge merely and heavily exceeded its cost will still read as
  "should have been larger," whether or not the size specifically was
  underweight. Two things bound the consequence of that residual bias
  rather than eliminate it: `larger_size_finding` returns a record with no
  multiplier (§12.4's guardrail, held structurally — see "Why the loosening
  direction cannot reach a bound" above), and every consequence of it is a
  proposal a person reads, never an automatic bound. If a future
  change makes `larger_favoured` a production input to anything but a
  proposal, this asymmetry must be resolved first, not inherited quietly.
- **Any code path that reads `larger_favoured` into a number above one.**
  There is none; the day one appears this record is void.

## Consequences

- §12.3's last row moves `PARTIAL → REACHED` for the executed-order half,
  with the loosening direction as a proposal only; the row's other five
  entries are as ADR 0061 and ADR 0062 left them, and the allocator row
  stays absent.
- §11.2 gains a fifth thing confidence-adjacent evidence does to size — a
  *bound* beside the four narrowings — and its row says so.
- §12.4's "never loosened automatically" now covers the sizing row by the
  same structure as the rule rows.
- One topic on the backbone, `learning.sizing_reviewed`; `Topic::ALL` is 76.
- No new metric: the cap is on the proposal's compromises and the DECIDE
  detail, and the records are on the log.
