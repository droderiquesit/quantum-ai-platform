# ADR 0154: The arbitrage desk trades only the operator's whitelist, sequentially; the centre may only lower a stated fill probability, and the peer frame stays an epoch

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval
  ("I allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). It decides the shape of the desk's inputs. It sets no limit
  value, admits no crate, creates no billed resource, touches no protected path
  and adds no order path.
- **Date**: 2026-10-19
- **Extends**: ADR 0008 (cells decide alone), ADR 0116 (the Lane 1 peer link),
  ADR 0130 (settlement terms).
- **Keeps**: ADR 0003 (paper only), the cell's whitelist as the only source of
  conversions, `LegPlanner::plan`'s least-reversible-first order.

## Context

Nine rows leave the arbitrage desk with questions that all reduce to *who may
widen what the desk searches, prices or runs*. The code already says a lot.
`ArbitrageDesk` searches only the graph `ArbitrageInstaller` builds from the
signed `cycle_whitelist` slot; a cycle is "never built from a book alone".
The centre's `whitelist_for` copies each operator-declared market's optional
`WhitelistedCosts` onto both of its conversions, and the module test pins that
a market with no stated costs ships `costs: None` — so a production conversion
without costs is priced with certain fill and no settlement risk, an optimistic
default nobody chose. `LegPlanner::plan` orders legs least-reversible first and
the send path reads steps by position. `PeerMessage::Token` carries only an
`OpportunityEpoch`. `qip_mesh::rebalance::rebalancing_tasks` has no non-test
caller, and the only holder of `OpportunityEpoch` outside `qip-mesh` is
`qip-peer-link`'s saga, which cannot depend on `qip-mesh` under the
`mesh_lane` and `lane_placement` suites. `EdgeAssumptions::coordination_latency`
is priced and proven to refuse a cycle, and nothing sets it.

Locate them with
`grep -n 'pub struct CycleWhitelist\|pub struct WhitelistedConversion' backend/crates/libs/qip-contracts/src/policy.rs`,
`grep -n 'fn whitelist_for\|costs: market.costs' backend/crates/runtime/qip-kernel/src/central/whitelist.rs`,
`grep -n 'fn plan' backend/crates/edge/qip-arbitrage/src/plan.rs` and
`grep -rn 'coordination_latency' backend/crates --include=*.rs -l`.

## Decision

1. **The whitelist is the permission, and nothing discovers beyond it.** The
   signed policy gains no market reference (R12-MESH-5042) and the centre
   builds no pair list (R12-MESH-5059). A search over every venue book would
   widen what the centre permits through a side door; the operator's
   `ArbitragePolicy` stays the single place a conversion, and the inventory
   target that pre-positions for it, is named. Inventory targets are
   operator-stated, not derived from drift.
2. **Representation and settlement stage are stated per market by the
   operator and carried additively.** `WhitelistedMarket` (and the conversions
   it yields) gain optional `representation` and `settlement_stage`; absent
   reads as `Native`/`Settled` exactly as the desk assumes today, so no record
   written before this changes meaning. The centre does not derive them:
   `qip-kernel` cannot see `qip-edge`'s `SettlementTerms`, and a dependency
   that exists to copy two words is the wrong trade.
3. **Costs are required on a whitelisted market that is not simulated, and
   the centre may only lower a stated fill probability.** An operator's
   `costs: None` on a production market is refused at `validate`, naming the
   market, because "no costs" priced as certain fill is the failure the
   cost model exists to prevent. Where the platform holds a venue's journaled
   fills, `Platform::issue_cycle_whitelist` replaces the stated
   `fill_probability` with `min(stated, lower confidence bound of the measured
   fill rate)`, only past a named minimum sample, and never raises it.
   `settlement_risk_fraction` is not derived: a loss rate needs a
   loss-given-failure that no event measures, so it stays operator-stated.
4. **Own-cell cycles keep zero coordination latency; a cross-cell cycle's
   builder sets it from the measured peer round trip.** A cycle across cells
   exists only through the Lane 1 saga behind the peer switch (ADR 0116), and
   no builder for one exists. Setting a non-zero constant now would price a
   wait that does not happen. The first cross-cell builder carries the peer
   link's measured round trip into `EdgeAssumptions::coordination_latency`
   and refuses to build without a measurement. R12-MESH-5069 stays PARTIAL on
   that builder, stated.
5. **Legs stay sequential.** No `Concurrent` arm. `LegPlan::residual_after`
   assumes sequential completion, and a planner field the send path ignores
   would read as a control that cannot fire. The only latitude is what exists:
   decomposition and passive-first by measured fill time. Concurrent legs,
   if ever wanted, loosen stranded-leg protection and are a RISK matter.
6. **`rebalancing_tasks` and `RebalanceTask` move into `qip-peer`.** `qip-mesh`
   already depends on `qip-peer`, which holds `OpportunityEpoch`; the function
   is pure arithmetic on it. `qip-mesh` re-exports for its callers and
   `qip-peer-link`'s saga calls it when an epoch closes, reporting the tasks
   through the existing `ReflexPort` report path. The node journals them on
   the P2 lane (no ADR 0100 §5 widening: P2 is not the outcome row).
7. **The Lane 1 frame stays an epoch.** `PeerMessage::Token` does not carry the
   contract `OpportunityToken`. The receiving cell re-derives the legs from
   *its own* whitelist by opportunity id; a frame that carried legs, size and
   edge would be a peer-supplied order instruction the receiver must then
   distrust, and `MESH-013` keeps reflex crates from naming `qip-peer`. The
   contract type remains the desk's own record, journalled at the origin.
8. **Pre-positioned directional bands are a separate type from the region's
   share.** A `DirectionalBand` (cell, direction, amount, epoch) is reserved
   against the share and never added to it, so a test can assert the sum, and
   a peer-loss test harness lives in `qip-peer-link`'s test support, not in
   `ReflexPort` (which gains no `stop`: a production port with a kill switch is
   a production hazard added for a test's convenience).

## Consequences

- The desk's search space and pricing inputs are all operator-signed or
  measured-and-lower; none is inferred wider than stated.
- Existing policies stating `costs: None` on non-simulated markets stop
  validating. That is the point, and each affected fixture needs costs.
- The peer wire does not change, so ADR 0116's frame set and the fuzz and
  vector tests stand.

## What it costs

- Operators must state costs and (when not defaults) representation and stage
  for every market; the edge is only as good as those declarations.
- Fill probability is lowered by a conservative bound, so some real
  opportunities price out until the sample grows.
- No cross-cell cycle gets a priced coordination wait until its builder exists.

## What would make this wrong

- If measured fills show the operator's stated fill probability is routinely
  *understated*, lowering-only leaves edge on the table; the fix is a
  reviewed policy change, not a centre that raises it.
- If a second region's cells need cycles the whitelist cannot name in
  advance, decision 1 is revisited in a record that says who signs discovery.
- If the saga needs more than an epoch to act, decision 7 is revisited with
  the receiver's re-derivation shown insufficient.

## Dissent

*Let the centre discover pairs and let cells run legs concurrently; the edge
is in the speed.* Strongest form: a static operator list lags the market, and
sequential legs forfeit the cycles that disappear between legs. It lost
because both widen what a cell may do on authority the cell cannot check —
discovery adds conversions nobody signed; concurrency removes the ordering
that bounds a stranded leg. It would be revived by a measurement of edge
forfeited to sequencing that exceeds the stranded-leg loss the ordering
prevents, argued as a RISK decision.

## Code it constrains

- `ArbitragePolicy` / `WhitelistedMarket` and `CycleWhitelist` in
  `qip-contracts/src/policy.rs`; `whitelist_for` and `validate_costs` in
  `qip-kernel/src/central/whitelist.rs`; `Platform::issue_cycle_whitelist`.
- `graph_from_whitelist_in` in `qip-edge-node/src/arbitrage.rs`.
- `LegPlanner::plan` / `LegPlan::residual_after` in
  `qip-arbitrage/src/plan.rs`; `EdgeAssumptions::coordination_latency` in
  `netedge.rs`.
- `rebalancing_tasks` (`qip-mesh/src/rebalance.rs`) and
  `qip-peer-link`'s saga; `PeerMessage::Token` in `qip-peer/src/lib.rs`.

## Rows unblocked

- R12-MESH-5053 — decision 2.
- R12-MESH-5057 — decision 3 (operator costs required; fill lowered by
  measurement; settlement risk stays stated).
- R12-MESH-5059, R12-MESH-5042 — decision 1: restated, nothing built.
- R12-MESH-5064 — decision 5: restated, nothing built.
- R12-MESH-5069 — decision 4: restated; PARTIAL on the cross-cell builder.
- R12-CAPITAL-5067 — decision 6.
- R12-CONTRACT-7068 — decision 7: restated, no wire change.
- R12-RES-7095 — decision 8.

## Not decided here

- **RISK** — the minimum sample and confidence level of decision 3 are
  statistical constants that only ever lower an edge; they are named in code,
  but if the risk lane considers any fill-probability floor a risk threshold
  it owns the values (R12-MESH-5057). Concurrent legs (decision 5) would be a
  RISK change. The residual-risk budget `PLAN_BUDGET` literal stays as is.
- Nothing is BILLED, PROTECTED or paper-boundary.
