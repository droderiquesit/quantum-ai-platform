# ADR 0230: The capital-society coordinator is a kernel step that settles allocation, funding, liquidity and collateral proposals in the survival waterfall's order against the region's grant, and no set of proposals ever sums past it

- **Status**: **Accepted 2026-10-31 under the owner's delegation** (decision
  10 of `docs/blueprint/owner-decisions-2026-10-31.md`, the ruling on
  R12-CAPITAL-9047). The limits of that delegation hold here: no risk
  number, limit, threshold or limit set changes, nothing new is sized
  (decision 7 of the same file), no billed infrastructure, and no
  paper-trading layer is touched. Tighten-only by construction: the grant is
  an input to the settlement and never an output, so no claim can widen it.
- **Date**: 2026-10-31
- **Rows**: R12-CAPITAL-9047.
- **Extends**: ADR 0039 (a cell's share of its region's grant), ADR 0127
  (capital-side brains are crate boundaries the kernel composes), ADR 0229
  (capital agents only propose).

## Context

R12-CAPITAL-9047 asks for a coordinator that combines allocation, funding,
liquidity and collateral proposals against the authoritative capital
grants, because independent proposers over-commit the same capital: each
claim is reasonable alone and their sum is not.

The centre already partitions each region's grant into per-cell shares
(`partition`, ADR 0039) and refuses a plan whose shares exceed a region's
grant. That bounds the allocator's plan. Nothing bounds the allocator's
lines *together with* the other claims on the same share. The durable
ledger is the current store; the Spanner deployment is not funded.

## Decision

1. **A kernel step, not a binary.** The arithmetic is
   `qip_capital::coordinator::settle`, a pure function of one grant and a
   list of proposals. The kernel composes it in `CentralPlane::grant_manifests`,
   immediately after the manifests are decided, so the grant it settles
   against is the share that ships and never a second reading of it.
2. **Waterfall order is the type's order.** `Claim` declares liquidity,
   collateral, funding and allocation in that order and the derived `Ord`
   is the settlement order: a book that cannot meet a margin call or an exit
   sizes nothing else, obligations are paid before opportunity, and
   allocation takes what remains. Within a claim, proposals settle by source
   name, so a replay settles in the same order.
3. **Cut from the bottom.** Each proposal is granted the smaller of its
   amount and what remains. The granted amounts never sum past the grant; a
   proposal the remainder cannot cover is cut to it, and every later one is
   granted nothing. A cell the partition withheld settles against zero, so
   every claim on it is cut.
4. **Refuse, do not read as zero.** A negative grant or claim is refused.
   The plane refuses a negative claim when it is handed over, so a bad
   figure never replaces a good set, and refuses an allocation claim from
   outside, because the plan's own lines are the allocation claims and a
   second source for them would be two figures for one fact.
5. **What the step reads today.** The allocation claims are the plan's
   lines for each cell. The liquidity, collateral and funding claims are
   whatever the plane is handed through `set_coordinator_claims`; no
   composition root hands it any yet, so on every path today the settlement
   grants the plan's lines exactly what the share already holds. The
   settlement is recorded on the plane (`CentralPlane::coordination`) per
   cell. It narrows nothing that ships yet: acting on a cut allocation line
   by narrowing the cell's share is a later change, and it can only narrow.

## Consequences

- With no non-allocation claim handed over, nothing changes on any path.
- Once claims are wired, the record says, per cell, which claims were paid
  in full, which was cut and by how much, and that the total stayed within
  the share.

## What it costs

- One settlement per decided cell per manifest decision, bounded by the
  number of claims on it.
- A reader must know that the settlement is recorded and not yet acted on.

## What would make this wrong

- A requirement that the coordinator *size* claims rather than settle them.
  That sizes capital outside the allocator and needs its own risk record
  (decision 7).
- A claim whose priority is not fixed by its kind, such as a funding
  obligation that must be paid ahead of a collateral top-up. That needs a
  priority on the claim, not a new position in the enum.
