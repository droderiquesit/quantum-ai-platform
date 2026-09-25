# Gap analysis: the repository against Blueprint v11.6 + GCP v2.1

M2 of the ADR 0099 programme, taken on 2026-09-25 against `3b48f91b`. The live,
per-requirement register is [the traceability matrix](../blueprint/traceability-matrix.md).
This report is the reading of it. When the two disagree, the matrix wins and
this report is stale.

## How it was measured

- **Catalogue.** 1,566 atomic requirements (see
  [the catalogue](../blueprint/requirements.md)). 17 extractors wrote them, 17
  completeness critics added 274 that the first pass missed, and 31 domain
  owners merged the two blueprints' overlap.
- **Assessment.** Each requirement was scored against the code by one of 60
  assessors. An independent skeptic then re-checked every batch. Skeptics
  were told to *refute* COMPLETE claims and to *hunt* for implementations the
  assessor missed. They **downgraded 240 rows and upgraded 135.**
  - A row is COMPLETE only when the behaviour exists in non-test code, a
    composition root reaches it, and a named test exercises that specific
    behaviour.
  - A type existing is never enough.
- **Technical debt.** 179 debt and conflict items were classified in a
  separate pass, in [the debt register](tech-debt.md).

## The numbers

| Measure | Requirements | Share of the 1,559 applicable |
|---|---|---|
| **Blueprint completion** (COMPLETE) | 34 | **2.2%** |
| Implemented (the behaviour exists in code) | 878 | 56.3% |
| Tested (a named test demonstrates it) | 739 | 47.4% |
| Integrated (a composition root reaches it) | 733 | 47.0% |
| Deployable (provisionable from committed config) | 404 | 25.9% |
| **End-to-end demonstrated** | 23 | **1.5%** |

| Status | Requirements |
|---|---|
| COMPLETE | 34 |
| PARTIAL | 757 |
| NEEDS-VALIDATION | 23 |
| MISSING | 525 |
| **INCORRECT** | **64** |
| BLOCKED (ADR 0099 conflict or external) | 156 |
| OBSOLETE (declined: the seven-repo split) | 7 |

**Read the gap between 56% implemented and 2% complete as the finding.**
- The tree holds a great deal of real, tested behaviour. Almost none of it
  meets a whole requirement: implemented, tested and reached from a
  composition root.
- The 2% is not a dismissal of that code. It measures how far behaviour
  stops short of the whole requirement: reached only from tests, or tested but
  not wired, or wired to a path that never runs.
- The 1.5% end to end reflects the governing fact of the current state:
  **no process of this platform runs in any environment.** The edge cell's
  hot path runs only under `cargo test`, because `execution_nodes = {}`
  everywhere. The central binaries' reconciler is suspended (ADR 0093). The
  dev project's billing is disabled, so `deploy.yml` fails at image push.

## The 64 INCORRECT rows: existing code doing what the blueprint forbids

These rank ahead of MISSING work, because each is a defect the platform
carries today, and several break this repository's own principles, not only
the blueprint. Grouped by theme:

**1. The hot path waits on the network.**
- The reflex decision loop is sequenced behind the journal drain, the centre
  mesh exchange and telemetry writes, all on the decision thread
  (FABRIC-002, FABRIC-003, RES-001, RES-005, ARCH-043, OBS-015, OBS-016,
  REFLEX-051).
- Package download, verification and compilation run on the decision thread
  (REFLEX-062).
- Exporting experience to the centre can stall a pass (EXPAND-064).
- **ADR 0100's event-fabric build closes this group by construction.** The
  decision thread only `try_send`s, and a timer drives passes.

**2. Clamping where the rules require refusing.** `01-security-and-safety.md`
principle 2: "Refuse rather than guess… A value silently corrected is a
caller bug that survives."
- `CapitalEnvelope::admit` returns `CapitalGrant::Reduced` and sends a
  smaller order instead of refusing it (CAPITAL-025, AGENCY-024). A test
  currently locks the clamp in.
- `Fact::new` defaults confidence to 1.0 and `CausalEdge::new` to 0.7. Out-of-range or NaN confidences are clamped or passed through (WORLD-008, WORLD-056).

**3. Controls that cannot fire.** `risk-and-execution.md` calls this "a defect,
not a spare part".
- The grant's drawdown limit (CAPITAL-026).
- `DegradationState::halts()` is hard-coded to `false`, so an expired capital
  envelope narrows but never halts (debt register, libs-foundation).

**4. Knowledge without evidence.**
- One extracted news item becomes asserted graph facts and a belief feature
  with no corroboration step (EVID-014, EVID-019).
- Corroboration counts copies of one origin as independent (EVID-006).
- The production causal-precedence writer records uncontrolled lead-lag as
  `Established` (AGENCY-030).

**5. Replay that could not have happened.**
- The strategy backtest and the counterfactual twin fill at a bar's price
  (TICK-006, TICK-050).
- Market ticks collapse event time and receive time (TICK-003).
- Ticks are discarded rather than journaled as a learning corpus (TICK-007,
  TICK-025, TICK-027).
- Drift narrows sizing in place (TICK-048).

**6. The ledger is not the one source of truth.**
- Balances are read from stores the ledger does not feed (LEDGER-001).
- The only route from a cell's outcomes to the centre's ledger dead-letters
  them after bounded retry (LEDGER-033, RES-079).

**7. Security.**
- OpenObserve, an internal telemetry store, admits anonymous internet
  invocation (API-006, SEC-017).
- The address block named "the restricted VIP" is `private.googleapis.com`'s
  range, not `restricted.googleapis.com`'s 199.36.153.4/30 (SEC-072).
- An identity impersonable from any branch push holds grants that could
  reach capital-moving surfaces (SEC-061).
- Human access is bound to individuals, not groups (SEC-039).
- Long-lived keys sit in Kubernetes Secrets (SEC-030, SEC-031).
- The debt register adds: `identity.ts` falls back to a hard-coded HMAC key
  for one-time codes instead of refusing.

**8. Topology divergences.** These are target migrations rather than
defects, and most are gated by ADR 0099's conflicts:
- a single VPC where v2.1 wants separate Reflex, Fabric, Service, Data and
  Engineering VPCs (GCP-009, GCP-024, GCP-051);
- stateful services on Cloud Run (GCP-003, API-010, ARCH-020);
- GitHub-hosted builds instead of Cloud Build private pools (CICD-047,
  CICD-051);
- per-environment rebuilds instead of promoting one digest (CICD-057);
- a regional Spanner where multi-region is specified (GCP-004).

## What is blocked, and by whom

156 rows are BLOCKED under ADR 0099's conflict register. By conflict, alone and
counting rows with a co-blocker:
- **C4, managed data services:** 41 alone, 56 with a co-blocker.
- **C2, new crates:** 33 alone, 45 with a co-blocker.
- **C3, Kubernetes:** 27 alone, 35 with a co-blocker.
- **C8, cost and multi-region:** 22 alone, 28 with a co-blocker.
- **C1, live capital:** 9.
- **C5, a non-Rust research runtime:** 2 alone, 3 with a co-blocker.
- **C7, per-brain services:** 1 alone, 2 with a co-blocker.

On top of the conflicts, **disabled billing blocks every deployment.** The assessors scored deployment rows under C8 rather than as an external blocker, so no row counts it separately.

The owner decisions these rows wait on are C1 (live capital, which is not an
agent's decision), C8's cost ceiling, and the dependency records for C2 and
C4. None of them blocks the first vertical slice: ADR 0100 builds it inside
the current posture.

## Where the open work is, by epic

| Epic | Open requirements | P0 open | Notes |
|---|---|---|---|
| E03 event fabric MVP | 113 | 80 | ADR 0100. On the critical path. |
| E04 reflex cell slice | 90 | 63 | 12 INCORRECT: the blocking hot path. |
| E14 infrastructure & deployment | 171 | 90 | 31 BLOCKED (C3/C4/C8); billing |
| E05 ledger & financial truth | 92 | 41 | `qip-ledgerd` (ADR 0100) |
| E13 security & trust | 58 | 48 | 6 INCORRECT, several cheap to fix |
| E16 expansion, agency (shadow-only), governance | 135 | 47 | mostly P3 |
| E07 risk & capital | 62 | 24 | clamp-not-refuse; controls that cannot fire |
| E06 tick, replay, twin | 92 | 31 | 6 INCORRECT: impossible fills, time collapse |
| E08 scout, evidence, knowledge | 113 | 24 | evidence gate missing |
| E09 world models, reasoning, ambient | 167 | 16 | largest; mostly P1/P2 |
| E10 foundry, evaluation, quantum | 99 | 17 | evaluation independence |
| E11 multi-region mesh & arbitrage | 92 | 22 | peer mesh absent; C8 |
| E01 build/CI hardening | 38 | 25 | action pinning, Makefile gate |
| E12 observability & AIOps | 37 | 21 | nothing ingests; telemetry on the hot path |
| E02 contracts & schemas | 27 | 16 | contract-first, ahead of fan-out |
| E00 programme & traceability | 27 | 9 | |
| E17 event markets, commerce, coverage | 45 | 11 | shadow/paper only |
| E15 frontend & API | 5 | 3 | |
| E99 owner decisions | 62 | 20 | C1–C8 |

## What this means for the order of work

1. **Build blockers:** none. Every gate is green locally and in CI.
2. **Contracts first (E02),** then the **fabric and first slice (E03, E04, E05)**
   under ADR 0100. The slice closes the whole hot-path group of INCORRECT rows
   and is the first end-to-end demonstration.
3. **Cheap INCORRECT fixes in parallel.** Refuse-not-clamp in the capital
   envelope, the confidence defaults, and the drawdown limit and halt that
   cannot fire. Security: the HMAC fallback, OpenObserve, the VIP range. CI:
   action pinning, the Makefile's `--no-fail-fast`. None of these touches the
   fabric's files, so they can run beside it.
4. **Deployment** waits on the owner for billing, and for the C8 cost ceiling
   to run beyond one dev node.
