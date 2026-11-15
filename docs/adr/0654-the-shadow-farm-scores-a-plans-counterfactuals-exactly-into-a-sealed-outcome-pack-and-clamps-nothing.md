# ADR 0654: The shadow farm scores a plan's counterfactuals exactly into a sealed outcome pack, and clamps nothing

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
