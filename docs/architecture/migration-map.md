# Migration map: current component → path → target component

How each part of today's tree reaches the [target state](target-state.md).
Migration over rewrite: every row starts from code that exists (see the
[current-state map](current-state.md)), and no row replaces a component
wholesale — the M2 debt pass classed nothing C (replace). Status and
requirement IDs per row are in the [matrix](../blueprint/traceability-matrix.md).

| Current component | Migration path | Target component | Epic | Gated by |
|---|---|---|---|---|
| `qip-edge` `Cell` + `qip-edge-node` (hot path code-complete; tests-only; decision thread blocks on drain, mesh and telemetry) | Timer-driven passes; `try_send` hand-off to ring → segment spool → drain; control compile and wire polling off-thread; tape-seeded venue; populated feature catalogue; journal-pressure halt wire | Regional Reflex Cell (v11.6 §16, v2.1 §6) | E04 | — (slice); GCE VM needs billing + C8 |
| `qip-edge` mesh (cell↔centre hub-and-spoke over plaintext HTTP) | Outcomes move to fabric P1/P2 and control to P0 through the same verify code; the mesh uplink is retired for outcomes (principle 6: one route per fact); peer links added for cross-cell coordination | Reflex Mesh (peer) + fabric (durable) | E03, E11 | C8 for >1 region |
| `qip-events` `EventLog` + `qip-streaming` `DurableLogTransport` + `qip-transport` mesh inbox | Envelope/QoS/codec in `qip-events::event_fabric`; broker core beside `DurableLogTransport`; segments in `qip-storage`; client and moved server in `qip-transport` | Event & Control Fabric (`qip-fabricd`) | E03 | QUIC/mTLS, prost, BLAKE3, Raft, replication: C2 |
| Kernel central plane settle + `qip-capital` per-user ledgers + portfolio stores (three sources of truth; outcomes dead-lettered after bounded retry) | `qip-ledgerd` consumes P1 once in effect; balances read from the ledger's read API; kernel settle points at the fabric | Ledger (v11.6 §15, v2.1 §10) | E05 | Spanner: C4 |
| `CapitalEnvelope::admit` (clamps), grant drawdown (cannot fire), `DegradationState::halts()` (always false) | Refuse whole orders; wire drawdown and halt so they fire; tests that prove each fires | Risk Gate + Capital Brain | E07 | — |
| `qip-market-ingestion` (bar-based; ticks discarded; time collapsed) + `qip-orderbook`/`qip-sequencing`/`qip-protocols` (tests-only decoders) + `qip-simulation-engine`/`qip-twin` (fill at bar price) | Canonical MarketEvent with event/receive/normalised time; P2 capture to segments; book reconstruction on the live path; replay that respects depth and queue | Tick & Replay (v11.6 §8) | E06 | Tick Lake on GCS: C8 |
| `qip-data-finder` (committed source list), `qip-entity-resolution`, `qip-world-model` (single graph; facts without evidence; defaulted confidences) | Evidence gate before any fact/belief; corroboration by independent origin; confidences refused, not defaulted; federation of scoped models with arbitration | Scout/Evidence/Knowledge; World Model Federation | E08, E09 | Spanner Graph/Bigtable: C4 |
| `qip-reasoning-engine`, `qip-investment-agents`, kernel DISCOVER/REASON | Symbolic derivations with proof traces; ambient sentinels under attention budgets | Reasoning fabric; Ambient mesh | E09 | Z3/OR-Tools: C5 |
| `qip-training` (local), `qip-evolution`, `qip-lifecycle`, model promotion (creator judges itself) | Independent Evaluation Brain; signed packs; champion/challenger | Model & Strategy Foundry | E10 | Vertex/Python: C4/C5 |
| `qip-quantum` (disabled by config default) | Async submission, classical baseline recorded per workload | Quantum Foundry | E10 | — |
| `qip-api` + portal (IAP; OpenObserve anonymous ingress) | Close anonymous ingress; relay ledger views from `qip-ledgerd` | API / Portal | E15, E13 | — |
| `qip-observability` (emits; nothing ingests; telemetry on the reflex thread) | Scrape off the decision thread; bounded dropping exporter | Observability | E12 | Cloud Run collector blocked upstream (CVE) |
| GitHub Actions + WIF + Binary Authorization; actions pinned by tag; Argo/Kargo suspended | Pin actions by SHA; plan-then-apply of the saved plan; promote one digest | Supply chain & GitOps | E01, E14 | Cloud Build pools / Argo: C3; billing |
| One VPC; Cloud Run for stateful services; `execution_nodes = {}` | Separate Reflex/Fabric/Service VPCs; GCE for reflex and fabric; one dev node first (ADR 0035) | GCP estate | E14 | billing; C8 cost ceiling |
| Paper-trading layers (Terraform, `AutonomyLevel::deployable`, `Cell` constructor) | **Unchanged.** `qip-ledgerd` adds a fourth fence | — | — | C1 (owner only) |
