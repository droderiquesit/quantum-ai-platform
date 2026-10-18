# Blueprint v12.0 / GCP v3.0 — completion plan

**Snapshot 2026-10-11 (the branch it named, `ccr-dc3c4d84-dayxv0`, is absent from this repository; figures below were recounted 2026-10-17).** Derived from the R12 register
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

1,518 requirements decomposed from v12.0 (M1–M7) and GCP v3.0 (G1–G3).
Per-file row counts under `docs/blueprint/requirements/R12-*.json` summed to
980 for M1–M7 and 538 for G1–G3 on 2026-10-17 (the plan said 1,080 for M1–M7,
which with 538 does not make 1,518); recount with `python3 -I -c` over
`json.load(open(f))` lengths. Every status was taken from code, test assertions and Terraform, not from
any document.

Status counts are not restated as a drifting table; run the snippet above.
Recounted **2026-10-18** with `python3 -I` over `docs/blueprint/assessment/*.json`:
COMPLETE 249, PARTIAL 1122, MISSING 114, BLOCKED 31, INCORRECT 1,
NEEDS-VALIDATION 1 (= 1518). On 2026-10-17 it was COMPLETE 172, PARTIAL 1154,
MISSING 159: eleven independent graders read 980 rows (ran nothing), 88
were proposed COMPLETE, eleven were held back (nine TICK rows reachable only
with `QIP_DEEPBRAIN_TICK_EVERY` set, which needs a licensing decision;
OBS-3118, whose alert policy is gated off; AMBIENT-1055, fenced partly by a
source scan), and the other 77 were promoted after every named test passed in
a full workspace run and five production calls were mutated. 35 MISSING rows
became PARTIAL and 218 rows had text corrected. Earlier on 2026-10-17 it was COMPLETE 161, PARTIAL 1165 earlier the
same day; nine rows (AMBIENT-1026, -1140, -1144, -7025, EXPAND-1113, -1114,
-1120, -6045, WORLD-9004) were promoted after an independent re-grade found
their text older than the code and the named tests were run and mutated;
LEDGER-5002 and LEDGER-5012 followed once their missing refusal tests landed. Of the 177 COMPLETE rows on that date, a W0.10
independent re-audit confirmed 156 and re-graded 21 to PARTIAL (11.9%; reasons
in each row's `notes`). A later independent grading of six rows this work
touched (RISK-3022, QUANT-4028, CONTRACT-7089, AGENCY-7029, COVERAGE-7190,
CICD-7120) completed QUANT-4028 and AGENCY-7029 and left the other four PARTIAL.
A third grading, of 28 rows the tick-to-ledger integration touched, completed
LEDGER-5015, LEDGER-10082 and FABRIC-8023 (their gap text predated a running
ledger writer) and left 23 PARTIAL with the unmet clause named in each gap. It
also found no register row for instrument-universe membership, order-id
idempotency or DECIDE release: those safety properties are tested and enforced
but not yet requirements the register can score. The 2026-10-11 table (124 / 1,125 / 207 / 52 / 9 / 1)
was superseded by the register's later rounds. BLOCKED means live capital, an
external action, or a standing ADR refuses it; INCORRECT means the code
contradicts the requirement.

The measure table below and the P0 line after it are 2026-10-11 figures not
recounted on 2026-10-17; treat them as stale.

| Measure | Rows |
|---|---:|
| Implemented (behaviour exists in code) | 1,250 (82%) |
| Tested (a named test asserts it) | 1,134 (75%) |
| Integrated (reached from a binary's composition root) | 656 (43%) |
| Deployable from committed config | 184 (12%) |
| Demonstrated end to end | 0 |

**The one-line reading:** the libraries are largely written and tested; half of
them are not called by any running binary; nothing is deployed. The shortest
path to a large COMPLETE count is *wiring and deploying what exists*, not
writing new subsystems. The v12.0 additions (NOW Brain, Forecast Lattice,
Tournament, VOI, adversarial verification, Meta-Intelligence, Cognitive
Compiler) are the genuinely new build, and all ten §31.1 targets are OPEN.

By priority: P0 411 rows, of which 85 COMPLETE, 296 PARTIAL, 20 MISSING,
4 INCORRECT, 6 BLOCKED. P0 is where the plan starts.

The old (v11.6) register said 24.9% COMPLETE. This one says 8.2%. The drop is
the stricter test — COMPLETE now needs integration, and a test whose
assertions would pass with the feature deleted no longer counts — plus the
v12.0 additions. It is not lost work.

## How far to trust this register

An independent reviewer that wrote none of the verdicts checked a random
sample of 30 COMPLETE and all 10 INCORRECT rows against the code, running ten
of the cited tests (all passed).

- **COMPLETE: 21 of 30 confirmed (70%).** The nine others were re-graded to
  PARTIAL in the register (ARCH-1185, CICD-9105, DATA-2020, DATA-2024,
  FABRIC-8010, GCP-10008, LEDGER-5002, MODEL-4001, QUANT-1076), each with the
  reviewer's reason in `gap` and `notes`.
- **INCORRECT: 9 of 10 confirmed.** CONTRACT-7082 became MISSING: the code
  is absent, not contradictory.
- **The errors all run one way: optimistic.** Composition-root reach
  asserted rather than traced; cited test files not opened; code that exists
  read as code that runs; guards that pass vacuously accepted; multi-clause
  requirements credited on their strongest clause.

So read COMPLETE as an upper bound. At the sampled rate, roughly 30 of the
remaining 94 unchecked COMPLETE rows are likely PARTIAL; the true figure is
probably nearer 6% than 8%. PARTIAL and MISSING were not sampled; the bias
suggests some PARTIAL rows are MISSING too. **W0.10** below re-audits every
COMPLETE row with the same method before anyone quotes a percentage
externally.

## Round 1 — what landed (2026-10-11)

Twelve parallel lanes implemented W0.1–W0.9, most of Wave 1, W2.1–W2.5,
W3.5/W3.9/W3.10 starts, W4.2/W4.6 and W5.2 (merged as `3ab3595c` …
`7d3be4e7`), then a gate pass fixed 33 failures (`0a233b40` … `6bbf5b88`).
Gate on the final tree: `cargo clippy --workspace --all-targets -D warnings`
exit 0; `cargo test --workspace --no-fail-fast` 761 binaries, **8,685
passed, 0 failed, 0 ignored**; fmt, dependency policy (11 packages, all
permitted) and secret scan clean.

Two independent auditors then re-graded the 186 rows the round touched,
strictly (a test that fails without the behaviour, and a traced path from a
binary's composition root). 64 moved. The register now reads **154
COMPLETE, 1,126 PARTIAL, 181 MISSING, 53 BLOCKED, 3 INCORRECT, 1
NEEDS-VALIDATION**; integrated 689, deployable 184, end-to-end 0.

The decisions table below is resolved: ADRs 0109–0115 were accepted on
2026-10-11 (audited crates from an allow-list, tokio permitted, KMS signing,
fabric P0/P1 replication on Spanner, mTLS through rustls, Spanner ledger,
GKE controllers-only, external action refused, single build registry).
Paper trading is unchanged (ADR 0003, 0108).

**Round 2 starts with what the audit found the lanes had not finished** —
each is a library that works and that no binary yet calls, or a claim the
code does not bear out:

| Item | Rows | What |
|---|---|---|
| R2.1 Install the Risk Gate in the edge node | RISK-4093, 7205 | `Cell::with_risk_gate` exists and is tested, but no app installs a `CellRiskGate`; the node sends orders without it. `order_route_risk_gate.rs` checks source text, not behaviour — make it behavioural. The repricer's replacement order also bypasses the gate. **P0.** |
| R2.2 Ledger state import | LEDGER-5001 (now INCORRECT) | `qip-ledgerd` `store.rs:907` `import_state` writes balances directly, outside the event history, and silently skips every account key containing more than one colon. Remove or confine it to replay. **P0.** |
| R2.3 Wire the conduct gate into a binary | AGENCY-1056…1069 | The only binary caller (`qip-cli shadow`) runs with no policy and an optional guard. |
| R2.4 Forward evidence | EVID-2031…2039 | `drain_evidence` / `absorb_evidence` are never called, so the world model and poisoning detector read an empty fabric. |
| R2.5 Adopt the §27 contracts | CONTRACT rows | 21 `Validated<…>` contracts with no production producer or consumer. |
| R2.6 Turn quantum on somewhere | QUANT-4052, 7206, 1076 | No app sets `quantum_enabled`; the async gateway never runs in production. |
| R2.7 Survival policy and curiosity rank in a binary | CAPITAL-4089, AMBIENT-3106 | `set_survival_policy` and `rank_acquisitions` have no caller. |
| R2.8 SAST that can block | CICD-9105 | Deploy waits for CodeQL to finish, not for it to find nothing. |
| R2.9 Catalogue says what it means | FABRIC-8086 | `streams.local.json` still declares `"quorum"`; replace with `archived` and stop accepting the alias silently. |
| R2.10 Private build pool | CICD-9097 | Builds run on GitHub-hosted runners, not a Cloud Build private pool. |

## Round 2 — what landed (2026-10-11)

Thirteen lanes merged (quant, model, NOW lattice, ledger on Spanner, fabric
P0/P1 managed path, edge Risk Gate, agency conduct gate, deep research,
tick lake, breadth, capital and hedge, CI/CD and infrastructure, signing and
mTLS), then 38 cross-lane test failures were fixed — five of them real
defects (a restart that re-minted an event id, an mTLS server that dropped
its own refusal, a CLI that told a caller how to repair a smuggled guard, a
ledger that accepted contradictory deployment settings, an API holding the
whole central plane) — and ADR 0116 resolved the one deliberate failure,
MESH-013, by moving the peer protocol and wire to Lane 1.

Gate on `d49b3899`: `cargo fmt --all --check` exit 0; `cargo clippy
--workspace --all-targets -- -D warnings` exit 0; `cargo test --workspace
--no-fail-fast` **9054 passed, 0 failed, 0 ignored** over 811 result lines;
`check-dependencies.sh` "42 third-party package(s), all permitted";
`check-secrets.sh` "nothing found".

Five independent auditors re-graded the 159 rows round 2 touched: the
register stood at 177 COMPLETE, 1133 PARTIAL, 151 MISSING, 55 BLOCKED,
1 INCORRECT after round 2 (these sum to 1517, not 1518; the 2026-10-17 recount
above is COMPLETE 161, PARTIAL 1165, MISSING 159, BLOCKED 31, INCORRECT 1,
NEEDS-VALIDATION 1 = 1518, after the W0.10 re-audit of 2026-10-17). Their defects and an independent security review's
seven findings are assigned to round-3 sessions (capital exposure, signed
envelope bounds, trust infrastructure, agent controls, evidence and data,
mesh, mutation verification). Round 4 runs ten sessions over the 469 rows
whose code is built but unwired or untested.

What code alone cannot close: 193 rows need a real deployment, drill or
plan against a cloud project (owner approval for a dev apply), 22 need an
owner decision, 55 are blocked by accepted ADRs.

## Decisions only the owner can take

*Resolved 2026-10-11 by ADRs 0109–0115; kept for the record.* Each blocked
a cluster below. None was a code change; each needed an ADR or an explicit
"no".

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
| W0.4 Contracts that are wrong or duplicated | CONTRACT-7082 (MISSING); AGENCY-6079, 6091, 6104 | `ActionIntent` lacks §27 fields; two `GoalSpec`, two tool registries, two `EffectAttribution`, two `ModelReputation`, two `StreamPolicy`, two `FabricEnvelope`. One of each. | qip-contracts, qip-agency, qip-events, qip-cost-router | M |
| W0.5 Curiosity ranking | AMBIENT-3106 (INCORRECT) | Additive score where §9.10 is multiplicative. | qip-contracts | S |
| W0.6 Fabric "quorum" that is one broker | FABRIC-8086 (INCORRECT) | Rename the profile to what it is until D1 lands. | qip-streaming | S |
| W0.7 Tests that assert nothing | see list below | Rewrite each to drive production code; mutation-verify. | qip-acceptance, qip-market, qip-data-finder | M |
| W0.8 Unbounded metric label | OBS-3118 | `qip_belief_brier_score_by_horizon` labels with a runtime duration; bucket to an enum. | qip-kernel, qip-observability | S |
| W0.9 Edge journal grows without bound | REFLEX (M5) | The node never enables journal trimming. | qip-edge-node | S |
| W0.10 Re-audit every COMPLETE row | the 94 COMPLETE rows the spot-check did not sample | Trace the binary reach, open the cited test, run it; re-grade. | register | M |

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

594 rows are implemented but not integrated. These are the big ones.

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

## Corrections 2026-10-17

Each item below was checked by opening the cited lines on 2026-10-17. They
record what the code shows, not that a gate passed.

Already fixed in code (do not re-plan as open):

- **W0.3 / CICD-9097.** `.github/workflows/deploy.yml:87-97` builds each commit
  once in `BUILD_ENVIRONMENT: dev`; `:707-716` takes `SOURCE_DATE_EPOCH` from
  the commit timestamp, not the clock.
- **CICD-9105.** `deploy.yml:271-305` reads the ci run's jobs and refuses unless
  at least one `sast / ...` job exists and every one succeeded.
- **R12-REFLEX-8002 refuted.**
  `infrastructure/terraform/modules/execution-node/variables.tf:159-167`
  validation admits only the C4/C4D high-CPU shapes.
- **R2.1 risk gate installed.** `backend/crates/apps/qip-edge-node/src/lib.rs:247`
  (`.with_risk_gate(risk_gate)`), assembled via `assemble_with_risk` at
  `main.rs:592`.
- **W1.1 ledgerd serves.** `backend/crates/apps/qip-ledgerd/src/lib.rs:366-371`
  sets serving and spawns `server.serve()`.
- **R2.2 `import_state` removed.** `qip-ledgerd/src/store.rs:76` documents the
  removal; `grep -n import_state` finds only that comment.
- **W5.2.** `qip-streaming/src/event_fabric/group.rs` (509 lines) and
  `admission.rs` (152 lines) are real implementations, not stubs.
- **W0.5 product score.** `qip-contracts/src/ambient.rs:696-706`
  (`net_value_bp`, integer product, floored).
- **W0.6 rename.** `qip-events/src/event_fabric/catalogue.rs:240-246` maps the
  old `quorum` spelling to `AckProfile::ManagedMajority` or refuses it. The
  plan's path `qip-events catalogue.rs` is in the `event_fabric` subdirectory.
- **W0.8 `HorizonBucket`.** `qip-kernel/src/confidence.rs:485-500`, a five-bucket
  enum replacing a runtime-duration label.
- **W0.9 journal trim.** `qip-edge-node/src/main.rs:556-560`
  (`with_journal_trimmed_on_ship().with_journal_wire()`).

Flagged conflicts, not resolved here:

- **ADR 0043 is binding in code but Proposed on paper.** `docs/adr/0043-*.md`
  says "Status: proposed. Nothing in this record is applied", while
  `qip-tls/src/cas.rs:108` and `qip-contracts/src/trust.rs:4` cite it as a
  decision. Owner to accept it or the citations need a different authority.
- **`std::env` in `qip-core`.** `libs/qip-core/src/config.rs:103`
  (`std::env::vars()`) and `secret.rs:59-60` (`std::env::var`) read the
  environment from a lib, and `.claude/rules/domains/core-rust.md` prohibits
  `std::env` outside `backend/crates/apps/**`. `secret.rs` is the sanctioned
  `_FILE` reader; no exception is written down. Open question for the owner;
  none is invented here.
- **Prod refusal.** See "Known gap" in `.claude/rules/domains/infrastructure.md`:
  `deploy.yml` lists `prod` as a dispatch option and refuses it only for
  non-dispatch events.
