# Lane-placement register

Blueprint v11.6 organises the platform by time budget: every function sits in the
fastest lane that still meets its correctness requirement, not the fastest lane
available. This file is the register that says which lane each crate is placed in
and what correctness requirement that placement meets. ARCH-013, ARCH-017 and
ARCH-019 inspect it.

`qip-acceptance`'s `lane_placement` suite reads this file and fails when:

* a workspace crate has no row, or a row names a crate that does not exist;
* a row has a lane outside 0 to 4 or an empty requirement;
* the Lane 0 set differs from the normal-dependency closure of `qip-edge-node`;
* a Lane 0 or Lane 1 crate takes a normal dependency on a crate placed in a
  slower lane. That is a remote read or a slow computation placed in a fast lane
  by construction, which is exactly what this register exists to refuse.

Lane 1 here means the cell and centre link and the fabric daemon only: the
peer-to-peer reflex mesh (ARCH-011) does not exist, so no crate is placed in Lane 1
for peer coordination. A crate that composes slower crates is placed in the slowest
lane it hosts.

| Crate | Lane | Correctness requirement the placement meets |
|---|---|---|
| qip-core | 0 | errors, secrets and hashing every lane shares; no I/O, so no remote dependency |
| qip-numerics | 0 | pure arithmetic; deterministic |
| qip-contracts | 0 | wire and policy vocabulary; pure types with replay-stable encodings |
| qip-events | 0 | hash-chained log written locally; a write never waits on a remote store |
| qip-observability | 0 | in-memory registry behind a mutex; recording never blocks on a scrape |
| qip-market | 0 | canonical market types; pure |
| qip-financial | 0 | canonical financial objects; pure |
| qip-portfolio | 0 | position and jurisdiction types; pure |
| qip-risk | 0 | risk figures; deterministic, never routed to a model |
| qip-quant | 0 | statistics; deterministic |
| qip-storage | 0 | local engine store with explicit timeouts; managed targets refuse at preflight |
| qip-transport | 0 | bounded blocking sockets with explicit timeouts; refuses when full |
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
| qip-edge-node | 0 | the cell's composition root; paper passes without any cognitive process present |
| qip-mesh | 1 | cell and centre exchange of signed deltas and grants; latency accepted, never awaited by a pass |
| qip-streaming | 1 | stream transports with explicit bounds; a refusing port rather than a silent fallback |
| qip-chain | 1 | signed chain records over qip-transport; bounded round trips |
| qip-fabricd | 1 | the in-tree fabric daemon; journal appends acknowledged locally |
| qip-capital | 2 | capital placement and per-user ledger; limits checked before an order exists |
| qip-capital-fabric | 2 | wallet against ledger reconciliation; a break is recorded, never absorbed |
| qip-opportunity-engine | 2 | opportunity valuation over bounded inputs |
| qip-optimization-engine | 2 | portfolio construction; the classical baseline is computed every time and wins ties |
| qip-portfolio-engine | 2 | portfolio construction over the optimiser; refresh within a cycle budget |
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
| qip-learning-engine | 3 | calibration from resolved outcomes |
| qip-prediction | 3 | prediction markets; reached only where a composition root wires it |
| qip-reasoning-engine | 3 | hypotheses and confidence as arithmetic |
| qip-simulation-engine | 3 | scenario simulation; results reproducible from the log |
| qip-training | 3 | training and validation off the order path |
| qip-twin | 3 | replay and counterfactual scoring as of an instant |
| qip-world-model | 3 | the single long-horizon store; bitemporal |
| qip-confidential | 3 | confidential-computing seam; no remote call in the order path |
| qip-kernel | 3 | composes every service into one cycle; hosts the slowest function it composes |
| qip-fastbrain | 3 | millisecond-budget central cycle that runs the full kernel cycle, so it hosts Lane 3 work (ARCH-019) |
| qip-deepbrain | 3 | minute-scale cycle with no ceiling on its length |
| qip-api | 3 | operator API composing the kernel |
| qip-web | 3 | static web server |
| qip-cli | 3 | operator tooling over the other crates |
| qip-acceptance | 3 | cross-cutting tests; never shipped |
| qip-quantum | 4 | benchmarked experiments that never block execution; the classical baseline is a non-optional field |
