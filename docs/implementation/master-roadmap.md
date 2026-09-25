# Master roadmap

The ordered programme from the current state to the [target state](../architecture/target-state.md),
under ADR 0099. Order follows the mission's priority rule — build blockers,
architecture foundations, contracts, data models, core infrastructure, CI/CD,
security foundations, core backend, the first vertical slice, test automation,
observability, scale, remaining capabilities, frontend, optimisation,
hardening — adjusted by what the [gap analysis](../reports/gap-analysis.md) found.

Milestone status and evidence: [milestones.md](milestones.md). The critical
path: [critical-path.md](critical-path.md). Live numbers: [scorecard.md](scorecard.md).

## Stage A — foundations and the first working system (M3 → M5)

| Order | Work | Epic | Packets | Why here |
|---|---|---|---|---|
| A1 | Contracts first: FabricEnvelope, StreamPolicy, QoS, SchemaId, codec, MarketEvent, PassMarker, ReplayManifest, reflex journal contract | E02 | SLICE-02, 03, 05, 06, 07, 12, 14, 49–52 | Parallel work is only safe on stable boundaries |
| A2 | Event fabric MVP: segment log, broker core, groups/QoS, identity and grants, `qip-fabricd`, operator verbs | E03 | SLICE-16, 17, 18, 19, 21, 22, 25, 27, 28, 30, 31, 35, 37 | Critical path; closes the hot-path INCORRECT group |
| A3 | Reflex cell slice: tape-seeded venue, feature catalogue, `try_send` hand-off, spool, drain, journal-pressure wire, timer passes, replay | E04, E06 | SLICE-04, 11, 13, 15, 20, 23, 24, 26, 33, 34, 36, 39, 55, 56 | The first real end-to-end path |
| A4 | Ledger: `qip-ledgerd`, postings once in effect, signed watermarks, API relay | E05 | SLICE-29, 32, 38, 40, 53, 57 | One source of financial truth |
| A5 | Slice suites and integration: the eight real-process tests; `make slice`; runbook | E00, E12 | SLICE-41–48 | M5 exit evidence |
| A6 (parallel) | Cheap INCORRECT fixes that touch no slice file: refuse-not-clamp in the capital envelope, drawdown and halt that fire, confidence refusals, HMAC-key fallback, OpenObserve ingress, restricted-VIP range, action pinning, Makefile `--no-fail-fast` | E07, E09, E13, E01 | to be packetised after rung 1 measures | Defects today; each is small and file-disjoint from Stage A |

## Stage B — core platform (M6)

Multi-asset capital and risk brains over the fabric (grants and envelopes
issued by the central plane on P0, retiring the fixture); tick capture to
segments and depth-respecting replay (TICK INCORRECT rows); evidence gate and
federation (EVID/WORLD INCORRECT rows); independent Evaluation Brain; central
plane consuming P1 instead of the mesh uplink.

## Stage C — deployment and reliability (M7–M8)

Needs the owner: **billing re-enabled** on the dev project, and a **C8 cost
ceiling**. Then one dev execution node (ADR 0035's shadow node) and one
`qip-fabricd` + `qip-ledgerd` on GCE; separate Reflex/Fabric/Service VPCs;
game days (v2.1 §25); SLOs with ingestion proven (OBS).

## Stage D — blueprint breadth (M7)

World-model federation breadth, symbolic reasoning, ambient mesh, quantum
foundry, market making and multi-leg arbitrage in simulation, event markets,
commerce and agency in shadow form. Each P2/P3 domain gets its own packet
DAG when Stage B's contracts are stable.

## Owner decisions on the path

| Decision | Blocks | Recommended |
|---|---|---|
| Re-enable billing on the dev GCP project | every deploy (M7–M9) | Yes — nothing else unblocks deployment |
| C8 cost ceiling | beyond one dev node | Set a monthly ceiling before Stage C |
| C2 dependency records (transport security, consensus, wire schema) | FABRIC replication/mTLS rows | After M5, with measured need |
| C4 storage records (ledger first) | Spanner/Bigtable/BigQuery rows | After M5 |
| C1 live capital | 69 LIVE_CAPITAL rows | Not an agent's decision; the programme reaches M9 in paper |
