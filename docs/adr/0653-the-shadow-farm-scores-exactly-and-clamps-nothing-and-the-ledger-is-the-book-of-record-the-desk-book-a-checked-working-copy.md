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

## What it costs

- Two scoring paths now exist side by side: `score_shadow` keeps its clamp at
  10,000 bp and `f64` conversion, `resolve` does not, so the same shadow can
  score differently depending on which one the caller uses.
- A desk must now supply its own list of factors; the farm refuses an empty,
  unit, non-positive or repeated list, and a pack is refused unless every
  shadow has exactly one resolution. The farm supplies no default, so that
  work falls on the caller.
- `linear_pnl` has no market impact, so regret for larger shadows is
  flattered. The pack names the model, which discloses the flaw and does not
  remove it. Storage and compute for packs are unmeasured, and no cycle yet
  produces a plan or resolutions to feed them.

## What would make this wrong

- A reader acts on `regret_bp` from the linear model as though it priced
  impact, and larger-size shadows are consistently preferred in a way a
  measured impact model reverses.
- Unclamped, exact basis points on a tiny actual notional produce values so
  large that downstream consumers cannot use them, which would show that
  refusing small notionals was the better rule than carrying them.
- Desks cannot agree on a factor list, so the farm's refusal to choose one
  leaves it unused.

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

## What it costs

- A break stops new desk orders until an operator records a correction naming
  the break's position, themselves and what was found. That is manual work,
  and a later agreeing verdict deliberately does not lift it.
- A lagging ledger also stops new orders, so a slow or stalled ledger
  throttles the desk. Comparison happens only at an equal position, so an
  ahead ledger yields `Unaligned`, about which nothing can be said.
- The gate covers only the desk's book, not cell positions. The wiring is
  unbuilt: no production path reads the ledger as authority or calls
  `permit_new_orders` yet, so today the cost is the added code and the
  unproven gate. Overhead per acknowledged ship is unmeasured.

## What would make this wrong

- Lag refusals fire often enough in normal operation that operators route
  around the gate, which would show the lag lift rule or the ledger's
  application latency is wrong.
- A correction is recorded that names a break but is later shown to have
  moved a book to match, which would show the correction record is not enough
  evidence.
- A real cash or position disagreement is found between desk book and ledger
  where the ledger was the side in error, which would show it is not fit to be
  the book of record.

## Still open

- The wiring: reading the ledger's balances and applied position over its API
  into a `LedgerStatement`, taking the working copy from the desk book at
  acknowledgement, and calling `permit_new_orders` from the desk's order path.
  Until then no production path reads the ledger as authority.
- Counting breaks as a metric and journaling a correction on the platform's
  chain.
- Cell positions (`qip-edge`) are not covered, only the desk's book.
- Desk shipping is undeployed (no broker, desk grant or token).
