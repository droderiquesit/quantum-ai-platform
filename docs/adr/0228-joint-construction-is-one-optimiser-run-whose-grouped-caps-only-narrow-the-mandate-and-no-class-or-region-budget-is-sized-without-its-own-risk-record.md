# ADR 0228: Joint construction is one optimiser run whose grouped caps only narrow the mandate, and no class or region budget is sized without its own risk record

- **Status**: **Accepted 2026-10-31 under the owner's delegation** (decision
  10 of `docs/blueprint/owner-decisions-2026-10-31.md`, the ruling on
  R12-ASSET-4070). The limits of that delegation hold here: no risk number,
  limit, threshold or limit set changes, nothing new is sized (decision 7 of
  the same file), no billed infrastructure, and no paper-trading layer is
  touched. Tighten-only by construction: a grouped cap above the mandate's
  gross target is refused, so a group can only narrow what the mandate
  already allows.
- **Date**: 2026-10-31
- **Rows**: R12-ASSET-4070.
- **Extends**: ADR 0148 (the reference budget beside the sizing budget).

## Context

R12-ASSET-4070 asks that one construction run respect strategy, asset-class,
region and horizon caps together, rather than sizing each sleeve in its own
pass and adding the results, which lets two sleeves each within their caps
sum past a joint one.

`PortfolioConstructor` already solves one mean-variance problem per cycle
with per-name bounds and a budget equality. It had no way to bound a group
of names. No class, region, strategy or horizon budget is on record anywhere
in the platform with a risk argument behind its figure.

## Decision

1. **Grouped caps live inside the one optimiser run.**
   `PortfolioConstructor::construct_grouped` takes a list of `GroupCaps`,
   each a dimension name, a name-to-group map and a cap per group. Each
   capped group becomes one `at_most` constraint on the summed weight of
   its names, in the same problem as the per-name bounds and the budget.
   `construct_referenced` is `construct_grouped` with no groups, so every
   existing caller is unchanged.
2. **The budget is pinned per dimension, and a breach is refused.** For
   each dimension the reachable gross is the sum over its groups of the
   smaller of the group's cap and its names' own bounds, plus the bounds of
   names in no group, and the budget equality is pinned at the minimum of
   that over every dimension. That minimum is not the joint bound: two
   dimensions can each reach a gross that together they cannot (an
   independent review found a case with a 0.6 budget and a joint maximum of
   0.2), and the solver then returns its best infeasible result. So after
   the solve, at the weights the legs will carry, every capped group is
   checked against its cap, and a breach refuses the construction: no
   proposal is drafted rather than one that breaches a cap. The same check
   covers a discrete path that does not encode the group rows. Pinning the
   budget at the joint bound would need a linear programme of its own and is
   not part of this decision.
3. **A cap only narrows, and must be able to bind.** A grouped cap that is
   not finite, is zero or less, or exceeds the mandate's gross target is
   refused, never clamped. A grouping that names a name in no thesis, or a
   cap on a group with no member, is refused rather than ignored, as a
   sizing cap on an absent name already is.
4. **No figure is invented, and no placeholder group is passed.** No
   class, region, strategy or horizon budget has a risk record, so the
   kernel's DECIDE stage passes no group and calls the constructor exactly
   as before. A class cap at the gross target was tried and is not inert:
   on the fast-brain tape the extra row, which cannot bind, turned the
   solver's "no feasible solution found" into "solved classically" for a
   single thesis, so a row that sizes nothing still changed what the
   default path records. A budget is passed through `construct_grouped`
   only with its own risk record. Strategy, region and horizon are also not
   keys an `ApprovedThesis` carries today, so the kernel could not group
   them yet in any case.

## Consequences

- One run, one budget: a joint cap cannot be breached by two sleeves each
  inside their own. A set of caps that cannot hold together stops the
  cycle's proposal, and the refusal names the group and what it held.
- On every path today the plan is the plan it was, because the kernel
  passes no group.
- A group bounds the *net* summed weight of its names. Under a long-only
  mandate that is the gross; under a mandate that allows shorts, a long and
  a short in one group offset. A gross group cap needs split long and short
  variables in the problem and is not part of this decision.

## What it costs

- One linear constraint per capped group in the problem the router solves.
- The capability is in the constructor and not yet on a default path; the
  requirement is not met in a shipped binary until a budget with a record
  is passed.

## What would make this wrong

- A requirement that caps bind on gross rather than net exposure for a
  mandate that shorts. That needs the split-variable form above.
- A class or region budget arriving as configuration with no risk record.
  That is a new risk number and needs one (decision 7).
