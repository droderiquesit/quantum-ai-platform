# ADR 0468: The Portfolio/Asset Brain allocates the book to causal drivers by the hypothesis each position was sized on, and to factors by the catalogue loadings

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR), session t50-21 (wave580). The delegation's
  limits hold: it sets no figure, changes no requirement's wording, adds no
  dependency, moves no fabric topic to a new schema version and touches no
  paper-trading layer. Nothing it adds decides, sizes or gates.
- **Date**: 2026-11-12
- **Rows**: R12-ASSET-3019.

## Context

R12-ASSET-3019 asks for a Portfolio/Asset Brain "covering: mandates,
horizons, exposures, hedges, divestment, factor/causal-driver allocation".
Since 2026-10-30 the brain read the desk's book slot and reported mandates
(positions the register flagged for want of one), exposure and a divestment
count. It stated nothing about horizons, hedges or allocation, because the
slot (`qip_investment_agents::desk::BookView`) carried only the portfolio
and its marks.

The kernel already holds each of the missing facts, decided with elsewhere:

- `Platform::position_horizons` — the hypothesis and horizon
  `construct_from` recorded when it sized each leg (R12-ASSET-4067);
- `PositionMandate::sized_at` — when that sizing ran (R12-ASSET-4066);
- `Platform::position_obligations` — each held position's hedge standing as
  `Declared<Vec<String>>`, present or not applicable with the reason
  (R12-ASSET-4068);
- `Portfolio::exposures` — the per-factor breakdown from the catalogue's
  factor loadings, already the figure the risk path reads.

What has no settled answer is the *causal-driver* allocation: which kernel
record says what a position is held because of. The candidates were the
world model's causal drivers propagated onto positions (the causal stress
does this per driver, but a position can be reached by many drivers and the
stress says nothing about which one it was bought for), and the hypothesis
the position was sized on.

## Decision

1. **A position's causal driver is the hypothesis it was last sized on**, as
   `position_horizons` records it. The hypothesis is the causal claim the
   desk expressed a thesis on and sized the leg against; it is one per
   position by construction, so the allocation partitions the book rather
   than double-counting it. A position with no recorded hypothesis — resumed
   from the log, or opened by a path other than `construct_from` — is
   **unattributed**, and its gross is reported as such, never spread over
   the drivers that are known.
2. **Factor allocation is the net exposure per factor `Portfolio::exposures`
   computes** from the catalogue loadings. A refusal from that method (a
   loading that is not a number) is carried as its reason, and no factor
   figure is reported.
3. **The kernel writes both, with the horizon and hedge standing, into a
   new `BookPurpose` beside the book** in `brief_the_desk_on_book`, every
   REASON. Each is read from the record the platform already decides with,
   so the brain cannot disagree with it.
4. **"Undeclared" is kept apart from "none".** A held position with no hedge
   entry is *undeclared*; one the register looked at and found nothing
   covering is *unhedged*, with the register's reason. A refusal of the
   whole hedge survey leaves every position undeclared and puts the reason
   on the finding's caveats. Likewise a horizon is either absent, present
   with an unknown sizing instant, or dated.
5. **No threshold is set for "near" a horizon.** No figure for it was
   stated, so the brain reports `nearest_horizon_remaining_fraction` — the
   smallest share of any dated horizon still to run, negative once past —
   and `positions_past_horizon`, and leaves the judgement to the reader.

## Consequences

- The brain's finding now carries `positions_without_horizon`,
  `positions_horizon_age_unknown`, `positions_past_horizon`,
  `nearest_horizon_remaining_fraction`, `positions_hedged`,
  `positions_unhedged`, `positions_hedge_undeclared`,
  `causal_drivers_allocated`, `largest_causal_driver_gross_share`,
  `unattributed_gross_share`, `factors_loaded` and
  `largest_factor_exposure_ratio`, beside its earlier facts.
- On a universe whose records state no factor loading, `factors_loaded` is
  zero and no factor ratio is reported. That is the catalogue's answer, not
  a gap in the brain.
- Nothing reads these figures to decide. A future limit on driver
  concentration would need its own figure from the owner.

## What it costs

- The desk's book slot grows a second structure every REASON: one horizon
  and one hedge entry per held position and one figure per loaded factor,
  bounded by the held book and the catalogue's factor set.
- `book_purpose` runs `position_obligations` and `Portfolio::exposures` a
  second time each cycle, beside their existing callers; both are linear
  in the held book.
- A single-driver attribution is coarser than a weight-split one (below),
  accepted because it partitions the book and needs no figure nobody gave.
- Every `BookView` literal in the tree now names `purpose`, so a test that
  builds a book states what it is held for or says it states nothing.

## What would make this wrong

A position sized on several hypotheses at once is attributed to the first
one `JustifiedHypotheses::horizon` resolves, the same one its horizon and
justification are read from. If the desk began sizing legs whose
hypotheses carry materially different causal claims, a single-driver
attribution would understate the book's driver spread, and the allocation
would need to split by the construction's own weights.
