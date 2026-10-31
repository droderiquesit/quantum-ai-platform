# Lane-placement register

Blueprint v11.6 organises the platform by time budget: every function sits in the
fastest lane that still meets its correctness requirement, not the fastest lane
available. This file is the register that says which lane each crate is placed in
and what correctness requirement that placement meets. ARCH-013, ARCH-017 and
ARCH-019 inspect it.

`qip-acceptance`'s `lane_placement` suite reads this file and fails when:

* a workspace crate has no row, or a row names a crate that does not exist;
* a row has a lane outside 0 to 4 or an empty requirement;
* the Lane 0 set differs from the reflex pass's closure, defined below;
* a Lane 0 or Lane 1 crate takes a normal dependency on a crate placed in a
  slower lane. That is a remote read or a slow computation placed in a fast lane
  by construction, which is exactly what this register exists to refuse.

**Lane 0 is the reflex pass's closure** (ADR 0116): the normal-dependency closure
of `qip-edge-node`, minus the node itself and the Lane 1 crates it hosts through the
reflex port. That hosted set is exactly `qip-peer-link` and `qip-peer`; the
`lane_placement` suite fixes it as a constant, and it is not to be grown casually —
a node hosting a second coordinator is becoming one, and ADR 0116 says to reopen the
sidecar alternative rather than widen the set. Lane 0 always contains `qip-edge` and
its whole closure. Until ADR 0116 Lane 0 was the node's whole closure, which could
not survive the node hosting the peer link blueprint section 16 requires: the node
would have become Lane 1 and Lane 0 would have had no definition at all.

A crate that composes slower crates is placed in the slowest lane it hosts. So
`qip-edge-node` is Lane 1: it hosts Lane 0 (the cell) and Lane 1 (the peer link),
and its decision loop reaches Lane 1 only through `qip_peer_link::boundary`, whose
port can offer and report and cannot wait.

Lane 1 here means the cell and centre link, the fabric daemon and the peer
coordination protocol and its link. The five functions blueprint section 4 places in
Lane 1 (peer reflex messages, opportunity tokens, distributed reservations,
multi-leg coordination, hedge and unwind commands) are registered as `MeshFunction`,
defined in `qip-peer` and re-exported as `qip_mesh::peer`. `qip-peer-link` carries
them node to node under ADR 0112's mutual TLS (plaintext on loopback only, in a
`local` deployment), on a session thread of its own, which also runs the cycle
itself — origination from the cell's published leg authorities, reservation,
FIRE, paper fill and unwind — so the reflex pass never waits on any of it. `qip-acceptance`'s `mesh_lane` suite compares that
registry with the `qip-mesh` and `qip-peer` rows below and fails (MESH-013) when a
crate in `qip-edge`'s closure or at Lane 0 depends on or names `qip-mesh`,
`qip-peer`, `qip-peer-link` or a coordination type; when the node links any other
Lane 1-or-slower crate; when a node module other than `main.rs` reaches the link
past `qip_peer_link::boundary`; and when the port declares a method beyond
`try_publish` and `status`, or anything in its file can wait.

| Crate | Lane | Correctness requirement the placement meets |
|---|---|---|
| qip-core | 0 | errors, secrets and hashing every lane shares; no I/O, so no remote dependency |
| qip-numerics | 0 | pure arithmetic; deterministic |
| qip-contracts | 0 | wire and policy vocabulary; pure types with replay-stable encodings; holds no peer coordination type (ADR 0116) |
| qip-events | 0 | hash-chained log written locally; a write never waits on a remote store |
| qip-observability | 0 | in-memory registry behind a mutex; recording never blocks on a scrape |
| qip-market | 0 | canonical market types; pure |
| qip-financial | 0 | canonical financial objects; pure |
| qip-portfolio | 0 | position and jurisdiction types; pure |
| qip-risk | 0 | risk figures; deterministic, never routed to a model |
| qip-quant | 0 | statistics; deterministic |
| qip-storage | 0 | local engine store with explicit timeouts; managed targets refuse at preflight |
| qip-transport | 0 | bounded blocking sockets with explicit timeouts; refuses when full |
| qip-tls | 0 | the mutual-TLS 1.3 policy on the `ring` provider, loaded from files the root names or from a Certificate Authority Service issuance (the settings, names and in-memory key in `qip_tls::cas`; the REST client and renewal loop stay in `qip_transport::cas`) and refusing a missing identity; opens no socket, so its callers own every timeout; the one pin of `ring`, `rustls` and `rustls-webpki` |
| qip-protocols | 0 | venue message codecs; local decode, no remote read |
| qip-orderbook | 0 | local book update; no remote read |
| qip-sequencing | 0 | local ordering and dedup; no remote read |
| qip-feature-dag | 0 | local feature computation from the local book |
| qip-strategy | 0 | strategy predicates over local features |
| qip-arbitrage | 0 | local cross-venue arithmetic over granted venues |
| qip-routing | 0 | local venue selection and child orders |
| qip-risk-engine | 0 | pre-trade deterministic checks before an order exists |
| qip-execution-engine | 0 | simulated execution against the simulator only; paper boundary |
| qip-brokers | 0 | simulated broker and provider sandboxes only |
| qip-edge | 0 | the reflex cell; decides from its last signed packages with the centre unreachable |
| qip-edge-node | 1 | the cell's composition root. Hosts Lane 0 (the cell) and Lane 1 (the peer link), and its decision loop reaches Lane 1 only through `qip_peer_link::boundary`; paper passes without any cognitive process present |
| qip-mesh | 1 | cell and centre exchange of signed deltas and grants, and the host of the peer coordination protocol's five functions, re-exported as `qip_mesh::peer`; latency accepted, never awaited by a pass |
| qip-peer | 1 | the peer coordination protocol, pure: frames in, state and refusals out; depends on qip-core, serde and serde_json alone, so no warehouse client is reachable from it (DATA-058) |
| qip-peer-link | 1 | the peer wire (mutual TLS 1.3 through `qip-tls`, not `qip-transport`, so the fabric's wire is unreachable from it) and session on threads of their own, with explicit connect, write and handshake timeouts; runs the peer cycle on the session thread; reached from the reflex node only through a port that offers with `try_send` and reports from atomics |
| qip-streaming | 1 | stream transports with explicit bounds; a refusing port rather than a silent fallback |
| qip-telemetry-drain | 1 | the one OTLP/JSON exporter: a thread of its own with explicit connect, read and write timeouts, backing off on a dead collector and never awaited by anything; takes its configuration as a lookup so it reads no environment (ADR 0156 §5) |
| qip-chain | 1 | signed chain records over qip-transport; bounded round trips |
| qip-fabricd | 1 | the in-tree fabric daemon; journal appends acknowledged locally |
| qip-capital | 2 | capital placement and per-user ledger; limits checked before an order exists |
| qip-capital-fabric | 2 | wallet against ledger reconciliation; a break is recorded, never absorbed |
| qip-opportunity-engine | 2 | opportunity valuation over bounded inputs |
| qip-optimization-engine | 2 | portfolio construction; the classical baseline is computed every time and wins ties |
| qip-portfolio-engine | 2 | portfolio construction over the optimiser; refresh within a cycle budget |
| qip-evidence | 2 | evidence records and the fabric that checks them; pure, no I/O, so ingestion can check before release without linking the slow-lane world model |
| qip-market-ingestion | 2 | connector feeds with timeouts; licensing posture evaluated first |
| qip-ledgerd | 2 | ledger daemon appending to the local log |
| qip-ai | 3 | model rungs and the cost router's vocabulary; no order-path caller |
| qip-agents | 3 | agent contracts; the panel is convened off the order path |
| qip-investment-agents | 3 | the specialist panel; findings never escalate autonomy |
| qip-cost-router | 3 | rung selection with a recorded rationale; Determinism::Required cannot name a rung |
| qip-compliance | 3 | licensing and policy evaluation before a source is used |
| qip-lifecycle | 3 | evidence and promotion gates; a candidate is refused, not clamped |
| qip-data-finder | 3 | discovery against a committed candidate list; licence posture before use |
| qip-entity-resolution | 3 | entity resolution over evidence |
| qip-evolution | 3 | challenger and promotion; shadow before live |
| qip-agency | 3 | goal specs, tool registry and bounded intervention plans as contracts; slow-plane cognition that depends only on lane 0 and is reached only where a composition root wires it |
| qip-expansion | 3 | gap classification and the ranked, budgeted research queue; records only, above the brains, and held by no crate below a composition root |
| qip-learning-engine | 3 | calibration from resolved outcomes |
| qip-prediction | 3 | prediction markets; reached only where a composition root wires it |
| qip-reasoning-engine | 3 | hypotheses and confidence as arithmetic |
| qip-language-providers | 3 | a hosted generalist model, linked by the deep brain alone (ADR 0138) |
| qip-simulation-engine | 3 | scenario simulation; results reproducible from the log |
| qip-training | 3 | training and validation off the order path |
| qip-twin | 3 | replay and counterfactual scoring as of an instant |
| qip-world-model | 3 | the single long-horizon store; bitemporal |
| qip-confidential | 3 | confidential-computing seam; no remote call in the order path |
| qip-kernel | 3 | composes every service into one cycle; hosts the slowest function it composes |
| qip-desk-ship | 3 | ships a desk's ledger records to qip-ledgerd off the cycle, on its own worker thread; drains a `qip_kernel::Platform`, so it sits with the kernel it reads |
| qip-fastbrain | 3 | millisecond-budget central cycle that runs the full kernel cycle, so it hosts Lane 3 work (ARCH-019) |
| qip-deepbrain | 3 | minute-scale cycle with no ceiling on its length |
| qip-api | 3 | operator API composing the kernel |
| qip-web | 3 | static web server |
| qip-cli | 3 | operator tooling over the other crates |
| qip-acceptance | 3 | cross-cutting tests; never shipped |
| qip-quantum | 4 | benchmarked experiments that never block execution; the classical baseline is a non-optional field |

## Lane 2 functions (ARCH-017)

The six named Lane 2 functions that operate on a seconds-to-minutes refresh budget:

1. **Portfolio/capital placement** — `qip_kernel::central::regions` (region share allocation), `qip_kernel::central::horizon` (capital pool accounting), `qip_kernel::pre_positioning` (movement planning). Reached from production via `Platform::stage_reason`. Structurally excluded from Lane 0 because `qip-edge` cannot depend on `qip-capital`, which both `qip-kernel` and `qip-capital-fabric` depend on.

2. **Regime shifts** — `qip_kernel::regime_allocation::RegimeAllocationEngine` and `qip_kernel::regime_transition::RegimeTransition`. Narrows sizing bounds within a Lane 2 budget; no dedicated market-making regime engine exists yet (ARCH-024).

3. **Treasury moves** — `qip_capital_fabric::{forecast, transfer, corridor, gate, wallet}` (movement corridors, transfer gates) composed with `qip_api::treasury_feeds`. Appends to the ledger and wallet state within the Lane 2 budget. Execution refused under ADR 0021 (paper-trading boundary).

4. **Repricing** — `qip_simulation_engine::resolution` and `qip_prediction` modules. Re-evaluates instrument prices against market data and forward models within the Lane 2 budget.

5. **Cross-region inventory rebalancing** — `qip_edge::cross_region` (execution-side mirroring logic) composed with `qip_kernel::central::regions` (centre-side allocation). The inventory_targets policy slot is currently unproduced by qip-api, so cells mirror against operator-declared bands with no centre-issued targets.

6. **Market-making position refresh** — `qip_kernel::quote_loop` within a Lane 2 budget (currently marked 'sent nowhere'; market-making execution is blocked by ADR 0021).

All six functions are structurally excluded from the reflex hot path by crate dependency direction: no Lane 0 or Lane 1 crate can depend on `qip-capital`, `qip-capital-fabric`, or the cognitive binaries that compose them, enforced by `qip-acceptance`'s `lane_placement` test suite.
