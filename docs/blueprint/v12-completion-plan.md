# Blueprint v12.0 / GCP v3.0 — completion plan

**Snapshot 2026-10-11, on `ccr-dc3c4d84-dayxv0`.** Derived from the R12 register
(`requirements/R12-*.json`, `assessment/R12-*.json`, rendered to
`traceability-matrix.md`). The register is the source of truth; this plan is a
reading of it. Recount rather than quote:

```
python3 - <<'EOF'
import json,glob,collections
c=collections.Counter(a['status'] for f in glob.glob('docs/blueprint/assessment/*.json') for a in json.load(open(f)))
print(c, sum(c.values()))
EOF
```

## Where the code stands

1,518 requirements decomposed from v12.0 (M1–M7, 1,080) and GCP v3.0 (G1–G3,
538). Every status was taken from code, test assertions and Terraform, not from
any document.

| Status | Rows | Share |
|---|---:|---:|
| COMPLETE | 133 | 8.8% |
| PARTIAL | 1,116 | 73.5% |
| MISSING | 206 | 13.6% |
| BLOCKED (live capital, external action, or a standing ADR refuses it) | 52 | 3.4% |
| INCORRECT (code contradicts the requirement) | 10 | 0.7% |
| NEEDS-VALIDATION | 1 | 0.1% |

| Measure | Rows |
|---|---:|
| Implemented (behaviour exists in code) | 1,250 (82%) |
| Tested (a named test asserts it) | 1,134 (75%) |
| Integrated (reached from a binary's composition root) | 659 (43%) |
| Deployable from committed config | 184 (12%) |
| Demonstrated end to end | 0 |

**The one-line reading:** the libraries are largely written and tested; half of
them are not called by any running binary; nothing is deployed. The shortest
path to a large COMPLETE count is *wiring and deploying what exists*, not
writing new subsystems. The v12.0 additions (NOW Brain, Forecast Lattice,
Tournament, VOI, adversarial verification, Meta-Intelligence, Cognitive
Compiler) are the genuinely new build, and all ten §31.1 targets are OPEN.

By priority: P0 411 rows, of which 87 COMPLETE, 294 PARTIAL, 19 MISSING,
5 INCORRECT, 6 BLOCKED. P0 is where the plan starts.

The old (v11.6) register said 24.9% COMPLETE. This one says 8.8%. The drop is
the stricter test — COMPLETE now needs integration, and a test whose
assertions would pass with the feature deleted no longer counts — plus the
v12.0 additions. It is not lost work.

## How far to trust this register

_Spot-check pending; see the section at the end._

## Decisions only the owner can take

Each blocks a cluster below. None is a code change; each needs an ADR or an
explicit "no".

| # | Decision | Blocks | Standing position |
|---|---|---|---|
| D1 | Fabric replication: in-tree Raft (or another consensus) for multi-broker RF3 and majority ack | W5.1 (12 rows incl. 3 INCORRECT) | ADR 0100 §3 / conflict C2 refuses RF≠1 |
| D2 | QUIC / mTLS transport and workload certificates | FABRIC-7046/7048, 8083, 8100, 8113, 8119 | ADR 0009 forbids an in-tree TLS stack |
| D3 | Asymmetric signing of policy frames and model packs (KMS) | W1.4 | ADR 0043 (proposed) |
| D4 | The GKE cluster that exists for the GitOps control plane (`modules/gitops-control-plane/main.tf:131`) vs "no Kubernetes" | GKE-placed G1/G2/G3 rows | ADR 0024 / ADR 0036 disagree; CLAUDE.md says none |
| D5 | Managed data stores (Spanner, Bigtable, AlloyDB) and their clients | W4.4 (14 rows) | ADR 0002 — a client needs a dependency ADR |
| D6 | Compression codec for the tick lake | TICK-2057 | ADR 0002/0009 |
| D7 | Multi-project org/folder model and NCC | W4.5 (12 rows) | one project per environment today |
| D8 | External-action surfaces (§24.4: publishing, outreach, pricing, operations) | 6 AGENCY rows | refused (EXTERNAL_ACTION); recommend keep refused |
| D9 | Live capital (Capital Bank money movement, financing, custodian reconciliation, live prod) | ~10 rows | ADR 0003 — **recommend keep refused**; these stay BLOCKED by design |

## The plan

Six waves in dependency order. Each work item names its R12 ids, crates, size
(S < 1 day, M days, L 1–2 weeks, XL more) and the gate that proves it. A work
item is done when its rows move to COMPLETE in the register with evidence, not
when code merges.

### Wave 0 — Correct what is wrong (do first; small; P0-heavy)

| Item | Rows | What | Crates | Size |
|---|---|---|---|---|
| W0.1 Conduct checks that check nothing | AGENCY-1060, 1061 (INCORRECT), 1056–1069 | `check_market_conduct` ignores its rules and rejects only "guaranteed"; `check_audience_permissions` ignores the allow-list; `PreActionGate` check 6 and the action check match "wash"/"spoof" by substring. Make them evaluate the rules given; put `ConductGate` in front of every adapter call. | qip-agency, qip-contracts | M |
| W0.2 Quantum on the cycle thread | QUANT-4029 (INCORRECT), 4046–4053 | The router calls the provider synchronously inside the cycle; `admit_job` has no production caller; default QAOA is unreachable (≥50 assets vs 24 qubits). Submit-and-poll with the classical answer standing. | qip-quantum, qip-cost-router, qip-kernel | M |
| W0.3 Build once, promote the digest | CICD-9097, 9112 (INCORRECT) | Per-environment registries and `SOURCE_DATE_EPOCH=$(date +%s)`. One registry, commit-time epoch, promote by digest; add replay and paper stages. | deploy.yml, Dockerfile, Kargo | L |
| W0.4 Contracts that are wrong or duplicated | CONTRACT-7082 (INCORRECT); AGENCY-6079, 6091, 6104 | `ActionIntent` lacks §27 fields; two `GoalSpec`, two tool registries, two `EffectAttribution`, two `ModelReputation`, two `StreamPolicy`, two `FabricEnvelope`. One of each. | qip-contracts, qip-agency, qip-events, qip-cost-router | M |
| W0.5 Curiosity ranking | AMBIENT-3106 (INCORRECT) | Additive score where §9.10 is multiplicative. | qip-contracts | S |
| W0.6 Fabric "quorum" that is one broker | FABRIC-8086 (INCORRECT) | Rename the profile to what it is until D1 lands. | qip-streaming | S |
| W0.7 Tests that assert nothing | see list below | Rewrite each to drive production code; mutation-verify. | qip-acceptance, qip-market, qip-data-finder | M |
| W0.8 Unbounded metric label | OBS-3118 | `qip_belief_brier_score_by_horizon` labels with a runtime duration; bucket to an enum. | qip-kernel, qip-observability | S |
| W0.9 Edge journal grows without bound | REFLEX (M5) | The node never enables journal trimming. | qip-edge-node | S |

Tests found to pass with their feature deleted (W0.7):
`qip-market/tests/tick_time_fields.rs`; `qip-acceptance/tests/tick_hot_replay_indexes.rs`;
`qip-data-finder/tests/sandbox_lifecycle.rs`; `qip-investment-agents` poisoning tests;
`qip-acceptance` `tick_replay` (one-packet gap replays as fully reliable);
`arch_033_quantum_path.rs`; `arch_004_regional_cell_autonomy.rs`; `arch_001` (Cargo.toml by
substring); `qip-data-finder/src/references.rs:625`; `event_fabric_transport_and_integrity.rs`,
`event_fabric_delivery_semantics.rs`, `event_fabric_infrastructure.rs`, `event_fabric_stream_classes.rs`,
`event_fabric_scaling.rs` (fabric_003/005/010/036/055/063/066/084/085/086/090/095/100/106);
`model_reflex_evaluation.rs`; `stress.rs:441` (loss fraction up to 1.5);
`cost_anomalies_are_not_yet_journaled_to_fabric_tasks`.

### Wave 1 — Wire what is already built (largest COMPLETE yield)

591 rows are implemented but not integrated. These are the big ones.

| Item | Rows | What | Crates | Size |
|---|---|---|---|---|
| W1.1 Ledger writer that starts | LEDGER-5001, 5013, 5014, 8024, 8134, 8139; FABRIC-8092; REFLEX-5047; MESH-5066 | `qip-ledgerd` exits at start: `ReadApi::serve()` always errs, the consumer is a placeholder on 127.0.0.1:9090. Start the consumer, serve reads, add the Terraform. | qip-ledgerd, infrastructure | M |
| W1.2 Edge node → fabric | REFLEX-5031/5033/5034/7059/8020/8067; FABRIC-7031/7058/8076; RES-7113; ARCH-8021 | Spool, pressure gauge and non-blocking writer are tested but `main` never builds them; flush runs on the decision thread. | qip-edge-node | M |
| W1.3 Hot path off the decision thread | G3 acc_no_sync, ap_observability | Store flush, mesh exchange and scrape render share `run_pass`'s thread. | qip-edge-node | M |
| W1.4 Signed packages | MODEL-4023, 9009; REFLEX-9011; SEC-9109, 9137 | HMAC, a seed-derived central key, `ModelPack` checks only non-empty. Needs D3. | qip-kernel, qip-edge, qip-contracts | L |
| W1.5 World Model Federation in the kernel | WORLD-3034–3038; ASSET-3017 | Build `Federation`, feed views, journal arbitrations. | qip-world-model, qip-kernel | L |
| W1.6 Symbolic reasoners in the cycle | WORLD-3050–3052; REASON-3053–3060 | Rules, temporal, Bayesian, constraint, `abduce`/`intervene` are tested and uncalled. | qip-reasoning-engine, qip-kernel | L |
| W1.7 Survival kernel, confidence governor | CAPITAL-4086–4089; RISK-4111, 4112, 4114, 4115 | `CapitalSurvivalState` unused; only stale evidence shrinks size. Feed calibration error, disagreement, OOD and regime mismatch into sizing as contractions only. | qip-capital, qip-kernel, qip-risk-engine | L |
| W1.8 Tail scenarios that act | RISK-4116–4124 | The stress report is displayed and changes nothing; add counterparty- and model-failure scenarios. | qip-risk-engine, qip-kernel | L |
| W1.9 Risk Gate on every order route | RISK (M4, 4093) | Desk path runs `PreTradeChecker`; edge cells apply envelope and feasibility only. One test enumerating every place an order reaches a placer. | qip-edge, qip-execution-engine, qip-acceptance | M |
| W1.10 Expansion engine | EXPAND-6011, 6025, 6046–6051, 6063–6069 | Library-only; real gap detectors and a `ResearchQueue` caller in deep brain. | qip-expansion, qip-deepbrain | L |
| W1.11 Agency seams | AGENCY-6091, 6103, 6107, 6108; GOV-6123, 6124, 6127 | Tool registry, conduct gate, approval chain, outcome memory have no production caller. | qip-agency, qip-contracts | M |
| W1.12 Ambient mesh and attention router | REASON-3061, 3062; WORLD-3075 | `AttentionRouter` built only by tests; nothing emits an `AmbientSignal`. | qip-contracts, qip-deepbrain | L |
| W1.13 Tick lake capture and time model | TICK-2042, 2050, 2055, 2056, 2049, 2051, 2087, 2089; DATA-2004 | Market events never reach `qip-storage::lake`; ticks lack sequence numbers; `ClockDiscipline` uncalled. | qip-edge-node, qip-storage, qip-market, qip-sequencing | M |
| W1.14 Fabric archive to GCS | FABRIC-8079, 8101; DATA-8124 | `qip-fabricd` archives to a local directory; GCS store and reader exist unwired. | qip-fabricd | M |
| W1.15 AIOps loop | OBS-7115, 7121, 7122 | Remediator, timeline, graph have no caller; no SLOs for ledger commit, serving, quantum success. | qip-observability, apps | M |

### Wave 2 — Contracts and data spine (unblocks Waves 3–4)

| Item | Rows | What | Size |
|---|---|---|---|
| W2.1 §27 contracts | CONTRACT (M7): EvidencePacket, StrategyPack, OpportunityToken, MicrostructureDelta, WorldModelState, SymbolicDerivation, KnowledgeDelta, ControlPackAnnouncement, MirrorCheckpoint | One typed, versioned contract each in `qip-contracts`. | L |
| W2.2 §24.9 control contracts | CONTRACT-6109–6115 | Seven superintelligence control contracts; `CapitalGrant` name collision. | M |
| W2.3 Forecast contract and calibration SLO | CONTRACT-3117; OBS-3118 | A `Forecast` type and an alert policy on Brier gauges. | M |
| W2.4 Evidence fabric | EVID-2031–2039; DATA-2012 | One `EvidenceRecord` built on the ingestion path; provenance graph; authenticity, contradiction, poisoning checks. | L–XL |
| W2.5 Pass-through manifest filled | DATA-1033–1037, 1039, 1041; AMBIENT-1040 | `DataReference` fields exist, nothing fills them; re-fetch driver; parser lineage. | M |
| W2.6 Outcome attribution | MODEL-6003, 6021 | Open = close = fill price, P&L zero; attribute by model, source, policy, path. | L |

### Wave 3 — The v12.0 additions (the new build; all §31.1 targets)

| Item | Rows | Target | Size |
|---|---|---|---|
| W3.1 NOW Brain | AMBIENT-3072/3073; WORLD-3075–3078; GCP §11.2 rows 9016–9027 | §31.1 #1 (WORLD-7196) | XL |
| W3.2 Temporal Forecast Lattice | WORLD (§9.7), GCP §11.3 | #2 (WORLD-7197) | XL |
| W3.3 Model Tournament, Forecast Market, scoreboard | WORLD-3086–3088; MODEL-3089–3092, 6018–6022; REASON-3108 | #3 (MODEL-7198) | L–XL |
| W3.4 Synthetic futures, replay over history, fusion | TICK-2059, 2061, 2067, 2076, 2078, 2081–2086; TICK-7199 | #4 | XL |
| W3.5 Active sensing / VOI | TICK-3101, 3102; RISK-3103; AMBIENT-3104, 7200 | #5 | M |
| W3.6 Adversarial verifier society | MODEL-7201; §9.11 rows | #6 | L |
| W3.7 Meta-Intelligence attention and compute market | AMBIENT-7202; §9.12 rows | #7 | L |
| W3.8 Cognitive Compiler | REASON-7203; §9.13 rows | #8 | L |
| W3.9 Capital/hedge/model-risk independence | CAPITAL-7204; RISK-7205; hedge RISK-4105–4109 | #9 — veto as a limit the Risk Gate evaluates, never a second enforcement path | L |
| W3.10 Hybrid compute fabric and problem compiler | QUANT-7206, 7016; QUANT (§11.1–11.5) | #10 | XL |
| W3.11 Tick learning loop | TICK-2065, 2068, 2069, 2080, 2091 | — | L |

### Wave 4 — Deploy (turns integrated into deployable and e2e)

| Item | Rows | What | Size |
|---|---|---|---|
| W4.1 Keyless deploy works | deploy.yml run 255 | Every image job fails at "authenticate without a key": WIF pool/provider not applied. Owner runs the dev bootstrap; then build, scan, sign, push. | S (owner) |
| W4.2 Dev apply with the fabric broker and ledgerd | G1/G3 rows | Instantiate `event-fabric-broker` (no root uses it) and `qip-ledgerd`. Plan shown, owner applies. | M |
| W4.3 One execution node in dev | REFLEX/RES rows; `execution_nodes = {}` | First edge node, simulated feed. | M |
| W4.4 Managed stores | LEDGER-8007, 8132–8140; DATA-8014–8016, 8130 | Needs D5. | XL |
| W4.5 Project/folder model, NCC | GCP-8043–8051, 8056–8058 | Needs D7. | L |
| W4.6 Security services | SEC-9123, 9128, 9129, 9134; MFA mandatory | PAM, CA Service, VPC-SC, SDP; Identity Platform MFA defaults to optional. | M |
| W4.7 Prove ingestion, then flip `workload_metrics_exist` | OBS rows | Nothing is shown to scrape. | M |

### Wave 5 — Scale and resilience

| Item | Rows | What | Size |
|---|---|---|---|
| W5.1 Fabric replication and mirror | FABRIC-7035, 7037, 7038, 8075, 8085, 8086, 8096, 8102; RES-7112, 7114, 8103; CONTRACT-7092 | Needs D1. | XL |
| W5.2 Consumer groups and broker admission | FABRIC-7033, 7036, 7056 | `group.rs` and `admission.rs` are stubs. | L |
| W5.3 Reflex standby and fencing | REFLEX-9117, 8069; RES-9166, 9167 | Terraform declares a standby; no fencing epoch in code. | L |
| W5.4 Peer reflex mesh transport | MESH-5035, 5046, 5050, 5060–5065, 5070 | Protocol tested, no sockets; edge node does not depend on `qip-mesh`. | L |
| W5.5 Game days and RTO/RPO | RES (G3 §25) | No schedule or harness. | L |

### Wave 6 — Breadth

| Item | Rows | Size |
|---|---|---|
| W6.1 Ledger scope (treasury, custody, inventory, collateral, margin, tax lots) | LEDGER-5005–5011, 5016–5021 | L |
| W6.2 Arbitrage graph costs (tax/duty, fill probability, settlement risk) | MESH-5053, 5055–5057 | M |
| W6.3 Market factory (create/list/seed are no-ops) | EXEC-5080, 5081; COVERAGE-5107 | L |
| W6.4 Event markets and physical commerce wired (paper only) | EVENT-5082–5089; COMMERCE-5090–5097 | L |
| W6.5 FX/cash and digital-asset coverage | COVERAGE-5098, 5103 | M |
| W6.6 Governance breadth (venue/product/strategy eligibility; envelope leverage bounds; decision records carry code/policy version) | GOV-6116, 6118, 6120, 6121 | M |
| W6.7 Development-agent controls | GOV-9067; SEC-9068, 9077, 9078, 9084; CICD-9076, 9082, 9083 | L |

## Sequencing and parallelism

```
W0 ──► W1 ──► W2 ──► W3
          └──► W4 (W4.1 is the owner's, can start today) ──► W5
W6 runs beside W3 once W2.1 lands.
```

W0 and W4.1 have no dependencies and should start now. Within W1, items touch
different crates and can run in parallel lanes (edge-node: W1.2, W1.3, W1.13;
kernel: W1.5–W1.9; deep brain: W1.10–W1.12; apps/infra: W1.1, W1.14, W1.15).
Rough total, one engineer-equivalent: W0 ≈ 3–4 weeks, W1 ≈ 3–4 months, W2 ≈ 2
months, W3 ≈ 6–9 months, W4 ≈ 1–2 months plus owner time, W5 ≈ 3–4 months,
W6 ≈ 3 months. These are sums of the register's S/M/L/XL sizes, not a
commitment.

## Out of scope by design

52 BLOCKED rows stay blocked unless the owner decides otherwise: live capital
and money movement (ADR 0003), external action surfaces (§24.4), GKE workload
clusters and service mesh (ADR 0024), seven repositories (one-repo rule).
Paper trading is not a phase.

## Definition of done for each item

The rows it names reach COMPLETE in `assessment/*.json` with path:line
evidence; `qip blueprint render` re-run and committed; the house gates run and
quoted (fmt, clippy -D warnings, `cargo test --workspace --no-fail-fast`,
dependencies, secrets); every new test mutation-verified.
