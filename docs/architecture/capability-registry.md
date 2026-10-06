# Capability registry

Blueprint requirement ARCH-007 names six capital-adjacent brains as first-class financial-operating-system capabilities, each with its own domain and contract. This register lists each of the six, names its owning crate(s), states its contract, and tracks its isolation from other domains.

The six capabilities:

| Capability | Owning crate | Contract | Status |
|---|---|---|---|
| Capital management | qip-capital (service) | `qip_capital::Allocator` + `qip_capital::EnvelopeIssuer`; capital-movement decisions, limit checking before order construction, per-user ledger | CRATE |
| Risk management | qip-risk-engine (service) + qip-risk (lib) | `qip_risk_engine::RiskState`; pre-trade deterministic checks, feasibility gates, no model routing | CRATE |
| Execution | qip-execution-engine (service) | `qip_execution_engine::ExecutionDecision`; simulated order placement and fill reporting against paper broker only | CRATE |
| Settlement | qip-capital-fabric (service) + qip-edge (lib) | `qip_capital_fabric::settlement::SettlementLeg` (fabric) + `qip_edge::settlement::Instruction` (cell); bilateral netting, venue settlement mapping, break reconciliation | SPLIT (C7) |
| Ledger/Accounting | qip-capital (service, src/ledger/) + qip-api (app, ledger_views.rs) | `qip_capital::ledger::LedgerEntry`; user-level debit/credit posting, double-entry accounting, no update after finality | SUBMODULE |
| Treasury/Custody | qip-capital-fabric (service) + qip-api (app, treasury_feeds.rs) | `qip_capital_fabric::{forecast, transfer, corridor, gate, wallet}`; movement corridors, transfer gates, custody policy as data, operations refused without signature machinery (ADR 0021) | SPLIT (C1) |
| Asset management | qip-portfolio (lib) + qip-financial (lib) + qip-portfolio-engine (service) | `qip_financial::object::CanonicalObject`; instrument registry, position types, asset classes; no dedicated owning crate | SCATTERED |

## Isolation and the "not your business" rule

ARCH-007's rationale: a defect in one capability can reach another's internals directly if they share a crate or a submodule. The table above marks the isolation level:

- **CRATE**: a dedicated service crate with a published contract type. Defects in one cannot reach another crate's state directly; only through the contract.
- **SPLIT**: functionality split across two crates (one as service, one as library in another domain). Settlement is split between `qip-capital-fabric::settlement` (fabric-side) and `qip-edge::settlement` (cell-side), separated by a message contract. Blocked by C7 (separate brain services).
- **SUBMODULE**: a submodule of a larger crate. Ledger is a module inside qip-capital, so a defect in the ledger can reach capital's other internals. Blocked by C7.
- **SCATTERED**: no dedicated crate. Asset management is scattered across qip-portfolio and qip-financial. Blocked by C7.

## Work to move ARCH-007 from PARTIAL to COMPLETE

1. **Create separate crates for Asset, Treasury/Custody**: Promote the SCATTERED and SPLIT capabilities to dedicated service crates with versioned contract types, under ADR 0074.
2. **Document the "not your business" rule**: Add a test in `qip-acceptance` that verifies each capability's module/crate boundary refuses internal reads from other capabilities, structurally.
3. **Create a capability contract inspection suite**: Add a test that verifies each crate's public API matches the contract named in this register and that no internal state is reachable outside the named contract.

Crates blocked on C7 (separate brain services) cannot be decoupled until the architecture separates the three binaries (qip-api, qip-fastbrain, qip-deepbrain) into independent services with RPC contracts between them. Today they are all the same `Platform` struct instantiated three times.

