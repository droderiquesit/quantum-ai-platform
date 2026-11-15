# ADR 0653: The shadow farm scores exactly and clamps nothing, and the ledger is the book of record with the desk's book a checked working copy (two decisions of lane 26)

- **Status**: **Accepted**, 2026-11-15, wave b52 lane 26, under the owner's
  2026-10-31 delegation. Paper only; nothing here sizes or sends an order.
- **Date**: 2026-11-15
- **Implements**: the library half of R12-CAPITAL-9051.

## Decision

1. `qip_capital::shadow_farm::universe_from_plan` builds, from an
   `AllocationPlan`, one alternate-size shadow per caller-supplied factor per
   positive allocation, and one refused shadow per refusal. The farm chooses
   **no factor**: an empty list, a factor of one, a non-positive or repeated
   factor is refused. Which sizes a desk wants shadowed is the desk's figure.
2. `resolve` scores every shadow from a `Resolution` (simulated and actual
   P&L) into an `OutcomePack`. Money is `Decimal`; `regret = simulated -
   actual`, signed; `regret_bp` is `regret * 10,000 / actual notional`,
   exact and unclamped, and absent for a refused shadow, which has no notional.
   The pack is refused unless every shadow has exactly one resolution.
3. The pack carries the model that priced its alternates and a SHA-256 digest
   over its canonical text (`verify`, `to_canonical_text`).
4. `linear_pnl` scales realised P&L by notional. **Provisional model:** it has
   no market impact, so it flatters larger shadows. The pack names the model
   so a reader knows what the regret assumes.
5. The existing `ShadowPortfolioUniverse::score_shadow` is left as it is: its
   clamp at 10,000 bp and `f64` conversion are asserted by existing tests in
   `capital_shadow_portfolios.rs`. New callers use `resolve`.

## Still open

- The cycle-time allocation pass (R12-CAPITAL-9039): `CentralPlane::allocate`
  is called only from operator paths, so no cycle yet produces a plan to shadow.
- Journaling the pack durably from the cycle, and feeding `Resolution` from the
  twin's fills in LEARN. `to_canonical_text` is the form to store.
- Alternate venue and hedge shadows have no builder; they score if present.

---

# Second decision: the ledger is the book of record (R12-LEDGER-5004)

Both decisions are here because a lane owns two ADR numbers and has four rows.
Implements the owner's 2026-10-31 ruling (decision 14, ADR_THEN_IMPLEMENT),
following the skeptic's text where it corrected the decider's.

## Decision

1. `qip-ledgerd`'s balances are the book of record for cash and positions. The
   kernel's desk book is a working copy, checked after each acknowledged ship.
2. `qip_kernel::ledger_authority::compare` runs only at an equal position: the
   ledger's applied desk-chain position against the acknowledged ship. A ledger
   behind is `Lagging`; ahead is `Unaligned` (nothing can be said); equal is
   `Agrees` or `Disagrees` per unit.
3. `AuthorityGate` refuses new desk orders on lag, lifting on catch-up with no
   break recorded, and on a break, lifting only by `record_correction` naming
   the break's position, the operator and what was found. A later agreeing
   verdict does not lift a break: agreement nobody explained is a book moved to
   match, the overwrite this refuses. Neither side is overwritten.

## Still open

- The wiring: reading the ledger's balances and applied position over its API
  into a `LedgerStatement`, taking the working copy from the desk book at
  acknowledgement, and calling `permit_new_orders` from the desk's order path.
  Until then no production path reads the ledger as authority.
- Counting breaks as a metric and journaling a correction on the platform's
  chain.
- Cell positions (`qip-edge`) are not covered, only the desk's book.
- Desk shipping is undeployed (no broker, desk grant or token).
