# Target state

The implementation-ready reading of Blueprint v11.6 and GCP v2.1 for this
repository, under [ADR 0099](../adr/0099-blueprint-v11-6-and-gcp-v2-1-are-the-architecture-of-record-in-direction-and-every-standing-decision-they-contradict-keeps-its-force-until-its-own-record.md).
Requirements are cited by ID from [the catalogue](../blueprint/requirements.md);
the per-requirement status is in [the matrix](../blueprint/traceability-matrix.md);
how each current component gets here is in [the migration map](migration-map.md).

This document describes the **target**, not the present. Where a standing
decision refuses part of the target, the part is marked with its ADR 0099
conflict (C1–C8) and the target is described in the form the repository can
reach without that decision.

## The shape, in one picture

```
            SLOW (Lane 3-4, minutes-days)                       WARM (Lane 2, s-min)
 Scout/Evidence ─> Knowledge tiers ─> World Model Federation    Asset / Capital / Risk brains
   (pass-through; manifests only)     + symbolic + ambient ──>  capital grants, risk envelopes,
 Model/Strategy Foundry ─> Evaluation Brain ─> signed packs     policy packs  ──┐
                                                                                │ P0 control
 ═══════════════════ Event & Control Fabric (qip-fabricd, per region) ═════════╪══════════
   P0 control · P1 outcomes · P2 market journal · P3 research · P4 telemetry   │
 ════════════════════════════════════════════════════════════════════════════════════════
      ▲ P1/P2 (async, never waited on)                          ▼ P0 (verified, cached)
 FAST (Lane 0-1, µs-ms)  Regional Reflex Cell (qip-edge-node, one per region, GCE VM)
   feed ─> book ─> features ─> reflex model ─> strategy ─> LOCAL RISK GATE ─> simulated venue
   ◄──── Reflex Mesh (direct peer, ephemeral) ────► other regions' cells          (paper only)
 TRUTH  qip-ledgerd: single-writer double-entry ledger from P1, once in effect
```

## Components

Each row: responsibility · inputs → outputs · runtime and scaling · failure behaviour ·
security boundary. Detailed requirement IDs per component are in the matrix
under the named domain.

| Component (domain) | Responsibility | In → out | Runtime / scaling | Failure behaviour | Boundary |
|---|---|---|---|---|---|
| **Reflex Cell** (REFLEX) | Local fast decisions: decode, book, features, reflex inference, strategy predicates, local deterministic risk gate, netting, routing to the simulated venue | venue feed, P0 packs → orders (simulated), P1/P2 journal | `qip-edge-node` under systemd on a dedicated GCE VM per region/shard (v2.1 §6); scale by sharding, never reactively | Continues on last valid pack when fabric/centre/cognition are lost; narrows then halts on freshness expiry; journal pressure is a halt wire (ADR 0100 §6) | Paper only: `Cell` has no live-ceiling constructor; venue typed `SimulatedGateway` |
| **Reflex Mesh** (MESH) | Latency-sensitive peer coordination: opportunity epochs, reservations, multi-leg sagas | peer edge states ↔ peer | direct peer links between cells; bounded fan-out | Disables distributed cycles; local trading continues | C8 (multi-region) for >1 region |
| **Event & Control Fabric** (FABRIC) | Durable async nervous system: typed pub/sub, partitioned journals, replay, backpressure, QoS P0–P4 | producers → partitions → consumers | `qip-fabricd` per region (ADR 0100): RF1 + fsync, producer-retained until archived | Broker loss: producers spool, cells continue, consumers resume from committed offsets | Bearer-token identity + key-scoped ACL; mTLS BLOCKED(C2); replication BLOCKED(C2) |
| **Ledger** (LEDGER) | The one authoritative double-entry record: cash, positions, fees, obligations; reconciliation | P1 outcomes → postings, watermarks (P0), read API | `qip-ledgerd`, single writer, DurableStore (Spanner BLOCKED(C4)) | Ledger down: outcomes accumulate in the fabric; cells narrow on ledger lag; idempotent catch-up | Refuses any fill not marked simulated (fourth paper fence) |
| **Risk Brain + Risk Gate** (RISK) | Brain forecasts and proposes envelopes; the gate enforces deterministically with local state | state, scenarios → signed RiskEnvelope (P0); gate verdicts | Brain: warm service; gate: in the cell and the desk | Gate is local and never routed to a model; a limit that cannot fire is a defect | Refuse, never clamp (CAPITAL-025 INCORRECT today) |
| **Capital Brain / Bank** (CAPITAL) | Grants with capacity, drawdown, expiry; liquidity ladder; placement | allocation → signed CapitalGrant (P0) | warm service | Custody/treasury down: trade only settled capital | Money movement BLOCKED(C1); grants are paper allocations |
| **Asset Brain** (ASSET) | Economic lifecycle of positions, mandates, attribution | ledger, beliefs → targets | warm service | — | — |
| **Tick & Replay** (TICK) | Canonical MarketEvent capture, clock discipline, book reconstruction, Tick Lake, deterministic replay, digital twin | venue events → canonical events → segments → replay | P2 journal + archive; replay jobs | Gaps mark windows unreliable and quarantine them from training | No look-ahead, no impossible fills (TICK-006 INCORRECT today) |
| **Scout / Evidence / Knowledge** (DATA, EVID) | Pass-through world sensing; source manifests; evidence scoring before knowledge | sources → EvidencePacket → KnowledgeDelta | slow-lane workers | Poisoned source quarantined, dependent beliefs downweighted | External content discarded after distillation; licensing evaluated first |
| **World Model Federation, Reasoning, Ambient** (WORLD, REASON, AMBIENT) | Competing world models with arbitration; symbolic reasoners with derivations; always-on forecasts and surprise | knowledge → beliefs, derivations, AmbientSignals | slow lane; bounded attention budgets | Divergence preserved, not collapsed; ambient storms rate-limited | Advisory until promoted; cannot starve reflex/ledger |
| **Model & Strategy Foundry + Evaluation Brain** (MODEL) | Train, validate out-of-time, package, promote; the evaluator independent of the creator | data campaigns → signed ModelPack/StrategyPack | slow lane (non-Rust training BLOCKED(C5)) | Drift falls back to champion/baseline | Promotion never direct to money movement |
| **Quantum Foundry** (QUANT) | Benchmark-gated hybrid optimisation, classical baseline always | problems → candidate solutions | async, never on the hot path | Quantum unavailable → classical path | Advisory; classical verification authoritative |
| **Expansion engine, Agency, Governance** (EXPAND, AGENCY, GOV) | Registries, curriculum, gated self-improvement; causal agency shadow-only | gaps → curriculum; GoalSpec → plans | slow lane | — | External action BLOCKED(C1); shadow form only |
| **API / Portal** (API) | Authenticated console and read APIs; PAPER TRADING shown wherever posture is | ledger/platform views → browser | Cloud Run behind IAP | — | No order-submitting control; no browser access to keys or ledger internals |
| **Observability** (OBS) | Metrics at the seam where facts become known; SLOs per plane | processes → /metrics → collectors | per-process exposition; collection BLOCKED by upstream CVE (Cloud Run) | Telemetry never blocks order processing | No tokens or account ids in logs |
| **Supply chain & GitOps** (CICD, SEC) | Signed, attested, digest-pinned artifacts; plan-then-apply infra | commits → images → environments | GitHub Actions + WIF (Cloud Build pools and Argo/Kargo per C3) | Registry/GitOps down: running system unchanged | No service-account keys; actions pinned by SHA (E01) |
| **GCP estate** (GCP, FINOPS, RES) | Projects, networks, placement, budgets, game days | Terraform → resources | one region (dev) until C8 | per-dependency degradation contracts | Separate Reflex/Fabric/Service VPCs (INCORRECT today: one VPC) |

## Standing divergences the target keeps

- **One repository** (C6, declined): the seven v2.1 repositories map to
  directories.
- **Binaries, not per-brain services** (C7): a brain is a crate in a binary
  until independent scaling is measured to require otherwise (ADR 0091). ADR
  0100 adds the two binaries whose reason is ADR 0091's own.
- **Paper trading** (C1): every live-capital and external-action requirement
  is built in its shadow form and scored BLOCKED.

## The first working system (M5)

ADR 0100 §8: a canonical market-event tape through a real `qip-edge-node`, the
event fabric, `qip-ledgerd` and `qip-api`, with metrics on every process and
deterministic replay, proven by eight real-process tests. The work is the 57
packets in `docs/implementation/packets/slice.json`; order and critical path
are in [the roadmap](../implementation/master-roadmap.md).
