# ADR 0462: Operational and systemic risk are two axes the shipped axis-weight veto reads

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation**,
  session t50-18. It adds two measurements and two shipped caps; it moves no
  existing bound, widens no authority and touches no paper-trading layer.
- **Date**: 2026-11-12
- **Reads**: ADR 0062 (feasibility withdrawal), ADR 0008 (cells decide alone,
  partitioned by region), and the `shared_cause` module's own argument for
  charging an axis rather than adding a limit kind.
- **Rows**: R12-RISK-1162.

## Context

R12-RISK-1162 requires Risk management with twelve scope items: pre-trade,
post-trade, market, liquidity, counterparty, concentration, causal-driver,
settlement, model, data, operational and systemic risk; its check is that
each maps to code reached from a composition root with a named test, and a
missing item fails inspection. Ten had code and tests. Operational and
systemic risk had neither: `grep -rli 'operational.risk'` found no `.rs`
file, and nothing measured a mechanism crossing regions.

## Decision

1. **Operational risk is the gross stranded at a venue the platform has
   withdrawn.** The only standing evidence of a *process* failure in the
   tree is the withdrawn-venue set — written on a cluster of feasibility
   refusals (ADR 0062) and replayed from the event log. A withdrawal stops
   new orders reaching the venue; the gross already there remains exposed
   to the failure and cannot be adjusted through it. `qip_risk::operational::with_operational_failure`
   copies that venue's counterparty-axis gross onto `operational_failure`,
   called from `Platform::risk_state_from`. Fraud, key-person and
   infrastructure-outage probability are **not** modelled: nothing in the
   tree measures them, and each would be a further source feeding this
   axis, never a figure invented for it.
2. **Systemic risk is the gross one causal driver reaches across two or
   more regions.** `qip_risk::systemic::with_systemic_contagion` reads the
   causal-driver attribution `shared_cause` already built (a borrow, not a
   second derivation) and the region each held record names, and charges a
   driver onto `systemic_contagion` when its held positions span at least
   two regions. A holding with no region counts as a region of its own —
   fail-closed, it can only make a driver systemic sooner. Crowding across
   other firms' books is **not** modelled: the tree holds no such data.
3. **Both are axes read by `MaxAxisWeight`, not new limit kinds**, for the
   reason `shared_cause` records: a new kind needs a new `RiskState` field
   and a second producer, which is how `MaxCounterpartyExposure` once read a
   map nothing wrote. Both refuse a second writer of their axis by filing
   `unevaluated`, which refuses every order.
4. **Two caps ship in `LimitSet::conservative_default`, each at a bound the
   set already states — no new figure.** `operational-failure-venue-gross`
   at 0.25, the twin-misfit cap's bound: a venue withdrawn for failing its
   order path is no more trustworthy than one whose twin misprices it.
   `systemic-contagion` at 0.35, `sector-concentration`'s bound: one
   mechanism crossing jurisdictions may carry no more of equity than one
   sector. 0.35 sits inside the causal-driver cap's 0.50, so this cap fires
   on books that cap admits — it is not a limit that cannot fire.
5. **Model and data risk map to existing seams.** Model risk is
   `Platform::venue_misfit` and the `twin-misfit-venue-gross` cap (the
   execution model's measured error), not `qip-compliance`'s model-risk
   file, which no composition root reaches. Data risk is
   `RiskState::unevaluated`: a figure whose inputs could not be read
   refuses every order rather than reading as zero.
6. **An inspection suite holds the map**:
   `qip-acceptance/tests/risk_management_scope.rs` names a seam and a test
   for each of the twelve items, checks each test is under `#[test]`, checks
   the book-level producers run inside `risk_state_from` and that the three
   central roots build a `Platform` and the edge node assembles the cell
   whose pass applies the settlement gate, and checks every axis item is
   capped by the shipped set.

## Consequences

- A desk loading its own limit file must decide whether to carry the two new
  caps; a file without them validates against the shipped set as any
  narrower file does.
- An order anywhere is refused while more than a quarter of equity is
  stranded at a withdrawn venue. That is intended: the book cannot shed that
  gross through the venue, so it must not add risk elsewhere on top of it.
- A cross-region driver is caught only on the standing book, for the reason
  `shared_cause` states: a proposed order names one bucket per axis.

## What it costs

- Two more axes on every risk read: a filter over the counterparty axis and
  one pass over the causal-driver attribution already built, both bounded by
  the venue list and the held book.
- Two more shipped caps, so a desk book can now be refused for stranded gross
  or for a cross-region driver where it was admitted before. That is the
  point, and it is a narrowing a desk will notice.
- The operational figure is only as broad as its evidence: a venue failing in
  a way that never clusters feasibility refusals is not charged.

## What would make this wrong

A measured source of operational failure other than withdrawal (a halted
cell, a failed reconciliation) that the axis ignores; or evidence that region
is the wrong partition for contagion — then this decision is superseded, not
patched.
