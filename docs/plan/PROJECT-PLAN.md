# PEOS Quantum AI — Project Plan

**This is the one authoritative plan.** It supersedes, for status, every other
document under `docs/plan/` and the standalone gate/backlog documents listed
below. Those documents are kept as history — they are the record of how each
number in this file was reached, commit by commit, and several of their
internal corrections ("re-scored at …") are the only place the *reasoning*
behind a closed item survives. Read them when you need to check a citation or
see how a number moved; read this file to know where the programme stands
today, 2026-09-04.

Superseded, retained as history (each now carries a one-line pointer back to
this file):

- `docs/plan/completion-plan.md` — the fullest slice-by-slice backlog and the
  four-gates scorecard, last re-scored at `2fd254f`
- `docs/plan/current-state.md` — measured facts (test counts, the full gate,
  crate counts) as of `29ce828`/`851c0ed`
- `docs/plan/blueprint-v10.1-gap-map.md` — an independent structural
  inventory of ~44 blueprint capabilities against the tree
- `docs/plan/gap-matrix.md` — the nine canonical areas, ordered work, risk
  register
- `docs/plan/gate-completion-plan.md`, `docs/plan/wave-7-backlog.md`,
  `docs/plan/wave7-cicd-status-report.md`, `docs/plan/wave7-cycle-conformance-review.md`,
  `docs/plan/algorik-instruction-precedence.md`, `docs/plan/algorik-orchestration-policy.md`
  — wave-scoped working documents, not re-summarised here in full
- `docs/architecture/algorik-blueprint-traceability.md` — the live blueprint
  scorecard by section number, with dated re-score paragraphs appended
  through `2026-09-04`
- `docs/architecture/current-state-audit.md` — the earlier line-count and
  demo-walk audit, last fully re-measured at `237c0f0`, before this branch
- `docs/ops/missing-infrastructure-register.md`, `docs/ops/off-gates-register.md`
  — the infrastructure gap and closed-switch registers this file's
  infrastructure rows are drawn from
- `docs/FINAL-SYSTEM-REPORT.md` — an earlier, narrower snapshot (58 crates,
  2,086 tests) written before this branch's work; its verdict ("not ready to
  run real money") still holds and its subsystem table is superseded here

**None of this changes the verdict any of those documents already state, and
none of it should be read as new evidence of readiness.** This platform does
not run real money, has never been deployed to production, and its
architecture of record (ADR 0022) still expects real trading only through the
ten-step gated sequence in ADR 0023, none of which is authorised. Every
number below is a percentage of a much larger, harder finish line — Phases
0–19 and four empirical gates against real data and real venues (§2 of
`completion-plan.md`) — not a percentage of "done."

---

## 1. Status-by-domain scorecard

Percentages are this document's own arithmetic over named capabilities at the
**TESTED** bar (a named passing test) unless marked otherwise, following the
convention `completion-plan.md` §2.2–§2.3 set: MEASURED (real cloud evidence,
not a repository test), TESTED, CONFIGURED, IMPLEMENTED-UNVERIFIED, PLANNED,
MISSING. A capability wired in code but reached by no deployed process is
still counted TESTED — the caveat is stated once here rather than repeated per
row: **`execution_nodes = {}` in every Terraform environment as of this
writing, so the entire edge/execution plane's TESTED rows are proven in
`cargo test`, not in production.**

| Domain / plane | % | Basis (numerator/denominator) | Evidence | What keeps it below 100 |
|---|---:|---|---|---|
| **Ingestion** (SENSE, licensing, dedup) | 60% | 3/5 named capabilities TESTED | `qip-market-ingestion` absorption (`absorption.rs`), licensing gate (`qip-fastbrain/src/licensing.rs`), Frankfurter registered in the connector bridge (`9f4a557`); `IngestionService` made constructible by a composition root (`26fa4f4`, 2026-09-04) | One live source (Coinbase, keyless) exists and was exercised for one tick in-session through a TLS-terminating bridge, never for a sustained period through a deployed egress proxy (Phase 1 exit unmet); the deep-web source tier does not exist |
| **Cognition** (world model, causal graph, belief, counterfactuals) | 71% | 5/7 TESTED | `qip-world-model`, `qip-reasoning-engine`, `qip-twin`; belief calibration reached from LEARN (`04738ee`, `learning.rs::a_cycle_that_resolves_a_thesis_grades_it_and_moves_the_calibration_series`); counterfactual scoring reached from LEARN (`b9e2242`) | No self-model (`grep -rli "self.model\|CapabilityEstimate"` empty); episodic memory holds one agent's research conclusion, not the blueprint's richer episode vector with ANN retrieval |
| **Valuation** | 0% | 0/6 named engines exist | `blueprint-v10.1-gap-map.md`: term structure BUILT-UNWIRED, credit engine two struct fields only, volatility surface/illiquid valuation/cashflow forecasting/liquidity ladder all ABSENT | Deliberately not scaffolded — a plane the blueprint schedules for Phase 14 |
| **Intelligence & optimisation** (lifecycle gates, cost router, quantum) | 60–86% | Optimisation 3/5 TESTED; Intelligence 6/7 TESTED | Routing gate, classical baseline every run (ADR 0006, `qip-quantum/src/provider.rs`); cumulative trial book durable and quarter-budgeted in every root (`aa66c5d`, `e31aae4`); the ten-model ML stack (`burn`, `linfa`, `polars`) is structurally ABSENT under the two-dependency policy | No family has ever been evaluated on real data, so "the gate" (Phase 2 exit) has never run for real; corridor policy has no subject |
| **Execution** (edge cell, netting, arbitrage, feasibility) | 93% TESTED / **0% MEASURED** | 14/15 TESTED | `qip-edge/tests/cell.rs`, `qip-edge-node/tests/pass.rs` (the node runs `Cell::work` passes when `QIP_VENUE_FEED=simulated`, `6340610`); fills are a venue fact only (`cb79b46`); per-region reservation now wired into the node's composition root and refused if absent (`63e4556`, 2026-09-04 14:18:50Z) | `execution_nodes = {}` in every environment (ADR 0035 authorises exactly one, `us-east4`, shadow-mode, dev-only, and it has not been applied); crossing interval unset (D3); 9 of 12 payload slots still have no producer, 3 of the "buildable now" claim withdrawn on evidence (`a14018d`) |
| **Ledger, wallet, treasury** | 50% | 6/12 TESTED | Fills attributed to contributors with zero residual (`7ef6063`); centre bills from venue fills, not placements, after the B22 wire-billing defect closed (`5290bb9`); a retired strategy's open positions are now dispositioned and the review runs from the LEARN stage in a deployed cycle (`c77877f`, 2026-09-04) | No per-user ledger (§43.3); wallet, corridor, transfer gate, custody are all ABSENT (`blueprint-v10.1-gap-map.md`) |
| **UI / Experience** | 14% (Layer 1) TESTED bar; frontend gates now run and pass | 1/7 | Both frontends' `npm run lint` / `npm run build` ran for the first time and passed 2026-09-04 (`45fc3a8`): portal 36 routes, landing 13 routes, 41 files linted clean; console pages walked against a running `qip-api` and their last simulated placeholders replaced with real routes (`576b9e0`, `e6f9632`, 2026-09-04) | Next.js remains transitional (ADR 0022); Leptos is a proposed record (ADR 0025) with no code; passkeys are one `AuthMethod` among several and `password` is still listed, the opposite of the blueprint's "no passwords, anywhere"; per-account entitlements (`can_invest`, `can_withdraw`) are ABSENT |
| **Infrastructure / cloud (Layer 6)** | 0% TESTED bar; **MEASURED**: partial | 0/7 by this document's own bar | Real plans and applies ran through CI against `algorik-dev`/`dev`: GKE torn down for real (`8194b3b`); `qip-dev-fastbrain` confirmed serving its attested image; portal, landing and OpenObserve confirmed answering publicly by direct `GET` on 2026-09-04 (`missing-infrastructure-register.md` "Observed on 2026-09-04"); `qip-dev-api`/`qip-dev-deepbrain` exist (internal-ingress 404 observed) but their content was not independently verified from outside the project | `terraform validate` has been run and passes (`terraform fmt -check -recursive` exit 0; `validate` "Success!"), but a *plan* is a stronger, different claim than this document's TESTED bar, so the row stays 0/7 by its own rule; ADR 0036 (2026-09-04) reopens the runtime question again — Argo CD/Kargo return on a control-plane cluster, **nothing applied**; the egress sidecar's allowlist still names no market-data vendor |
| **Security** | Structural, not a percentage | 3 paper-trading layers intact | Terraform refusal (`variables.tf`), `AutonomyLevel::deployable` refusal at every composition root, `qip-edge::Cell`'s constructor has no live-ceiling arm — all three unchanged this session. A default-off session gate on the deployed portal was found and closed 2026-09-04 (`28b0e56`): the gateway and proxy both defaulted to open unless `ALGORIK_AUTH_REQUIRED` was exactly `"true"`, so a deployment that forgot the variable proxied every viewer route anonymously with the mounted bearer token | An independent review (`c0316b4`) found the portal deploy did not require a session and would attest an unreviewed tree; both are now refused, but ADR 0033 (OpenObserve authenticated before it holds telemetry) is **decided, not applied** |
| **Observability** | "Not measured" for scrape/ingestion; emission is TESTED | Both planes emit (16+ central sites, edge `CellMetrics`) | `.claude/rules/domains/observability.md`; both brains now drain to the observability sink and the reason no deployment may point it anywhere yet is stated rather than silent (`35f0470`, 2026-09-04); the telemetry console page now says on its face that nothing collects any of it (`f102112`) | `workload_metrics_exist = false` in every environment; A6 (a collector for every emitter) is **refused**, not open — the only published version of the reviewed collector image carries an unfixed CRITICAL CVE (`2fd254f`, 2026-09-04); no scrape has been observed anywhere |
| **Deployment** | dev only; not production | `dev` applied by `infra.yml`; 3 central services + 2 frontends + OpenObserve catalogued | Digests recorded from CI runs 33780092495 and 33891084271 (`a5e82c8`, `3b95f0c`); direct, unauthenticated `GET`s on 2026-09-04 show the portal and landing serving `200` with the correct titles, OpenObserve serving `200`/`401` as designed (anonymous per ADR 0030, its own API still gated), and `qip-dev-api`/`qip-dev-fastbrain`/`qip-dev-deepbrain` answering as services that exist behind internal ingress | No authenticated read of any running revision, digest, secret-mount or attestation has been made from outside the project; `test`, `stage`, `prod` are untouched; ADR 0036's controllers are not applied |

**"Not measured" domains, named rather than guessed:** end-to-end latency
(no reproducible benchmark exists — `docs/performance/budgets.md` measures
eight stages in isolation and says so); any of the four blueprint gates
(Phase 2, 3, 6, 8 exits) on real data — Phase 6 and Phase 8's arithmetic now
*exists* (regime-conditioned sizing scored against an unconditional baseline
on a pre-declared split, `4c28391`; probability scored against a venue's
implied one, `d703f4b`, both 2026-09-04) but has never run against real
market or venue data, only synthetic/replayed; whether any secret-mount chain
on Cloud Run has actually served a credential to a running process.

---

## 2. Work-item register

IDs are namespaced by source so a reader can trace back: `ALIGN-*` from
`completion-plan.md` §4(i); `PHASE-*` from §4(ii); `DEC-*` from its owner
decisions §5; `REG-*` from `missing-infrastructure-register.md`'s numbered
gaps; `GATE-*` from `off-gates-register.md`'s numbered switches. Duplicate
findings across documents are merged into one row citing both.

### Alignment work (repository-internal consistency)

| ID | Title | Blueprint ref | Owner role | Status | Closed at (commit, ISO date+time) | Blocked-external need |
|---|---|---|---|---|---|---|
| ALIGN-A1 | Refresh the traceability matrix and truth pass | ADR 0022 | Docs/architecture agent | done | `296e187`, 2026-09-04 (exact time not separately recorded in message) | — |
| ALIGN-A2 | Re-measure the full gate at HEAD | — | Any agent, clean checkout | done | `29ce828` (302 binaries, 3,485 passed, 1 failed, repaired at `397c144`) | — |
| ALIGN-A3 | Wire `Platform::learn_from`, `Platform::evaluate_alternatives`, `Web::record_cycle` | §47, §12 | qip-kernel owner | done | `04738ee`, `b9e2242`, `cf20457` | — |
| ALIGN-A4 | Assert risk's O(1)-in-strategy-count property | §2.2, rule 11 | qip-risk owner | done | `b9e9e7d` | — |
| ALIGN-A5 | Settle `qip-arbitrage` (desk) and `qip-normalization` (delete) | §30, §7.3 | Edge + docs owner | done | Desk: `71f9465`/`584c96b`/`6340610`; normaliser removal applied `0c62a79`, 2026-09-04 14:48:43+0000 | — |
| ALIGN-A6 | A collector for every emitter, an alert for `qip_central_`/`qip_belief_` | §47 | Infra/observability owner | **blocked-external** | not closed | The only published version of the reviewed `cloud-run-gmp-sidecar` image carries an unfixed CRITICAL CVE (CVE beside `run-gmp-entrypoint`/`rungmpcol`, per `2fd254f`, 2026-09-04 12:44:12+0000); needs a patched upstream release before any digest can be pinned |
| ALIGN-A7 | Correct rule files against the tree | — | Docs owner | done | `ecfb0a6`, `132c1b7` | — |

### Phase 0–3 blueprint work

| ID | Title | Blueprint ref | Owner role | Status | Closed at (commit, ISO date+time) | Blocked-external need |
|---|---|---|---|---|---|---|
| PHASE-B1 | Decide and record the egress path | §46.2, §45.1 | Owner + infra | done | `2b7e502` (ADR 0024) | — |
| PHASE-B2 | Plan, apply and observe the egress sidecar | §46.2 | Infra owner | in progress | Plan/apply ran in CI against `algorik-dev`/`dev`; `qip-dev-fastbrain` confirmed serving past the sidecar's probe fix (`32b344d`, 2026-09-03 00:13:53+0000) | Redeployment/observation of `qip-dev-api`/`qip-dev-deepbrain` past the same fix; a vendor host in the allowlist (see DEC-D9) |
| PHASE-B3 | Name the market-data vendor host and its licensing posture | §7, rule 40 | Owner (vendor selection) | **blocked-external** | not closed | ADR 0034 names candidates (Coinbase, Alpaca, Kalshi) but a contract has not been read against a licensing decision; the allowlist (`variables.tf:294-305`) added Frankfurter (`9f4a557`) — FX reference rates, not the equities feed the Phase 2 gate needs |
| PHASE-B4 | Seven days of stable streaming, statistics converged, no raw stream retained (Phase 1 exit) | §51 Phase 1 | Ops (observe only) | **blocked-external** | not closed | Requires PHASE-B2 and PHASE-B3 first; nothing to build, only to run |
| PHASE-B5 | Count trials cumulatively across runs, durable | §20.1 | qip-lifecycle owner | done | `9332bcb`, `94dd7e2`, `aa66c5d` | — |
| PHASE-B6 | Define the holdout band as an output of validation | §20.1, §51 P3 gate | qip-lifecycle owner | done | `d0558b4` | — |
| PHASE-B7 | Attempt the Phase 2 gate on real data | §51.1 | Owner (data + venue) | **blocked-external** | not closed | Depends on PHASE-B4; a family surviving holdout on real data or a recorded failure |
| PHASE-B8 | Passkeys | §51 P0, §40.3 | Frontend/identity owner | open | not closed | None known — unblocked, unclaimed |
| PHASE-B9 | PQC keys / real signatures for the payload channel | §46.2 | Owner (crypto ADR) | **blocked-external** | not closed | DEC-D2: an ADR admitting a vetted crate, or declining and amending ADR 0002's reversal clause |
| PHASE-B10 | Feasibility gate ahead of the profitability filter, edge + central | §18.1, rule 23 | Execution owner | in progress | Edge half done (`95a4932`); central half open; the off-grid central refusal landed `e8daa51`, 2026-09-03 03:00:25+0000, narrowing what remains | — |
| PHASE-B11 | Join edge contributor vector to central attribution, settle crosses | §27.1, §43.4, rule 12 | Kernel owner | done | `7ef6063`, `7d79161` | — |
| PHASE-B12 | Per-region reservation table | §4.2, §26, §33, rule 21 | Edge owner | done | Mechanism `0ca4b92`; installed into the node's composition root and the node refuses to boot without one, `63e4556`, 2026-09-04 14:18:50+0000 | — |
| PHASE-B13 | Set the internal-crossing cap interval | §27.1 | Owner (a number) | **blocked-external** | Code done at `153e429` | DEC-D3: the owner has not chosen an interval; no root sets one |
| PHASE-B14 | Twelve producers for the twelve payload slots | §41.5 | Multiple, per slot | in progress | 3/12 produced (`capital_grants`, `cycle_whitelist`, `risk_envelope`); the "buildable now" claim for slots 2, 10, 11 was withdrawn on evidence (`a14018d`, 2026-09-04 14:18:51+0000) — none has a real source in the workspace | Slots 1, 3–6, 12 wait on later phases that do not exist yet |
| PHASE-B15 | Second, independent halt wire | §46.2 | Edge/node owner | done | `ff86473` | — |
| PHASE-B16 | ADR 0020 step 1 — GKE workload evidence | §41.4 | — | moot | Evidence never gatherable; cluster removed at `808ca32` | — |
| PHASE-B17 | Validate the wired Terraform modules | §41.4, §45.1, §46.1 | Infra owner | in progress | `terraform fmt -check`/`validate` now pass (per `2fd254f`'s re-score); real plans ran in CI | The execution-node module's own fix (`2e19a4c`) has not been re-planned and confirmed clean |
| PHASE-B18 | A producer for the cycle whitelist | §30, §41.5 item 8 | Kernel + API owner | done | `5396679`, `91d20f5`, `73a1694` | — |
| PHASE-B19 | Feed the exposure buckets the bucket limits read | §26, §33 | Kernel owner | done | `588335a`; deployed-root universe assembly `8224509` | — |
| PHASE-B20 | Charge a cell's reported fills into the centre's risk aggregate | §26, §33, rule 11 | Kernel owner | done | `98bc687` | — |
| PHASE-B21 | Budget each family at 500 trials/calendar quarter | §20.1, §54.1 | qip-lifecycle owner | done | `e31aae4` | — |
| PHASE-B22 | Bill the wire's fills, not its placements | §43.4, rule 12 | Kernel + edge owner | done | `5290bb9` (six commits ending here); last test-only reader corrected `095144b` | — |
| PHASE-B23 | Decide what a concentration cap is a share of | §26, §28.1, §33 | Risk owner (ADR) | done | ADR 0027 accepted and applied `eca7ebb`, 2026-09-03 20:13:43+0000 | — |
| PHASE-New1 | Compare regime-conditioned sizing against an unconditional baseline (Phase 8 gate arithmetic) | §51 Phase 8 gate | Kernel/simulation owner | done (arithmetic only, not on real data) | `4c28391`, 2026-09-04 15:26:51+0000 | Real market data to run it on for real (see PHASE-B7-class blocker) |
| PHASE-New2 | Score platform probability against venue-implied probability, refuse late-known quotes (Phase 6 gate arithmetic) | §51 Phase 6 gate | Prediction owner | done (arithmetic only, not on a live venue) | `d703f4b`, 2026-09-04 14:10:40+0000 | A live prediction-market venue |
| PHASE-New3 | Retire a strategy in sustained decay without a human call; disposition its positions from the LEARN stage in a deployed cycle | §20.3, §35 | Kernel owner | done | `3deace8` (2026-09-04 14:29:26+0000); disposition + LEARN wiring `c77877f` (2026-09-04 16:19:45+0000) | Handover to a funded strategy sharing the thesis is not produced — no shared-thesis record exists |

### Owner decisions outstanding

| ID | Decision | Blocks | Status | Blocked-external need |
|---|---|---|---|---|
| DEC-D1 | The egress path | — | taken (sidecar, ADR 0024) | — |
| DEC-D2 | In-tree HMAC vs ADR 0009 | PHASE-B9 | **blocked-external** | An ADR admitting a vetted crypto crate, or declining and amending ADR 0002's reversal clause — owner's call |
| DEC-D3 | The internal-crossing cap interval | PHASE-B13 | **blocked-external** | A number, from the owner |
| DEC-D4 | Switch the GKE egress proxy on | — | moot | — |
| DEC-D5 | ADR 0020 steps 1–5 | PHASE-B17, Layer 6 | taken for code; applied in `dev` | Steps 1–2 (evidence, warm comparison) were never done; amending the ADR text is the owner's |
| DEC-D6 | `qip-arbitrage`/`qip-normalization` disposition | ALIGN-A5 | done | — |
| DEC-D7 | K3 — what the application zone may reach | Typed-intent surface | taken (narrower reading, tested `827a40e`) | — |
| DEC-D8 | Correct the observability rule file | — | done | — |
| DEC-D9 | The market-data/chain-RPC vendor hostnames and licensing posture | PHASE-B3, B2, B4; execution-node venue | **blocked-external** | Owner selects and contracts a vendor; ADR 0034 names candidates only |
| DEC-D10 | ADR 0023 step 3 vs the Phase 2 gate | — | overtaken in practice; ADR text unreconciled | Owner's call to reconcile the record |
| DEC-D11 | Whether the matrix gains rows for §48/rule 77 (OpenTofu, Cloud Build, Cloud Deploy) | ALIGN-A1 completeness | **blocked-external** | Owner's call — now sharper: ADR 0036 (2026-09-04) reintroduces Argo CD/Kargo, so this decision needs re-asking against the new record |
| DEC-D12 | A2's shell | — | taken by circumstance | — |
| DEC-D13 | Concentration semantics (gross vs equity) | PHASE-B19/B23 | done — ADR 0027, option (a), applied `eca7ebb` | — |

### Infrastructure register gaps (`missing-infrastructure-register.md`)

| ID | Title | Status | Closed at (commit, ISO date+time) | Blocked-external need |
|---|---|---|---|---|
| REG-1 | Two Cloud Run services (frontends) existed outside Terraform's admission policy | done in code; unobserved on the running revision | `c643d42`, 2026-09-04 14:18:51+0000 | An authenticated `gcloud run services describe` to confirm the running revision carries the flag |
| REG-2 | The landing ran as the project's default compute identity | done in code; unobserved | `c643d42`, same commit | Same as REG-1 |
| REG-3 | The console-egress subnet had no egress deny | done | `fbb73a7`, 2026-09-04 14:04:57+0000 | — (not observed in the running project) |
| REG-4 | The portal's session secret reached the process as an environment value | done | `c643d42` | Same as REG-1 |
| REG-5 | `ledger_database`/`control_fabric_topic` are unassigned module inputs | open | not closed | F5: wire from `module.data.spanner_database`, or record why it can never be wired |
| REG-6 | §46.1's control fabric (Pub/Sub) has no resource | open | not closed | A decision on whether Pub/Sub is built or the blueprint row is amended |
| REG-7 | `catalogue.tf`'s "deliberately not here" list omits the two frontend workloads | open (COSMETIC) | not closed | One doc edit |
| REG-8 | The §2.1 scorecard claimed "no third-party SaaS at runtime" after OpenObserve deployed | **confirmed by observation, still open** | not closed | The scorecard row needs rewriting; OpenObserve is observed serving anonymously (ADR 0030) |
| REG-9 | LAYER 1 scorecard row disagreed with this repository | open (COSMETIC) | not closed | One doc edit |

### Off-gates register (`off-gates-register.md`) — undocumented gaps only

| ID | Title | Status | Blocked-external need |
|---|---|---|---|
| GATE-1 | `notification_channels = []`, nowhere documented as deliberate | open, BLOCKING-DEPLOY | An owner decision on where alerts go, or a comment saying why none do |

All eleven other off-gates rows are **documented, deliberate** absences
(`execution_nodes = {}`, `workload_metrics_exist = false`, null image
digests, `project_id = "unprovisioned"` ×3, etc.) and are not open work —
they are decisions with a paragraph, which is what the register exists to
distinguish from a gap with none. None of the twelve is BLOCKING-A-GATE
(none stops `make check`).

### Counts

- **Done:** 27 (ALIGN-A1, A2, A3, A4, A5, A7 · PHASE-B1, B5, B6, B11, B12,
  B15, B18, B19, B20, B21, B22, B23, New1, New2, New3 · DEC-D1, D4, D6, D7,
  D8, D12, D13 · REG-3)
- **In progress:** 4 (PHASE-B2, B10, B14, B17)
- **Blocked-external:** 10 (ALIGN-A6 · PHASE-B3, B4, B7, B9, B13 · DEC-D2,
  D3, D9, D11)
- **Open (unblocked, unclaimed):** 6 (PHASE-B8, PHASE-B16 [moot, counted
  separately] · REG-5, REG-6, REG-7, REG-8, REG-9 · GATE-1) — precisely:
  PHASE-B8 (passkeys) is the one genuinely open, unblocked, uncommitted item
  of size in the blueprint backlog; REG-5/6/7/8/9 and GATE-1 are small,
  named documentation or wiring gaps, none BLOCKING-A-GATE.
- PHASE-B16 counted separately as **moot** (1) — the evidence it asked for
  can no longer be gathered because the cluster it concerned is gone.

Total rows in this register: 48 (27 done + 4 in progress + 10 blocked-external
+ 6 open + 1 moot).

---

## 3. Status updates

One entry per commit on this branch since 2026-09-03T00:00:00Z, newest
first, 123 commits. Date and time and the first line are read from
`git log --date=iso`; the evidence sentence is the first sentence of the
commit's own body, quoted rather than paraphrased. Where a commit's body did
not open with a stand-alone verification sentence, its opening context
sentence is quoted instead — read the commit itself for the full account.

| Date/time (UTC) | Commit | What changed | Evidence stated in the commit |
|---|---|---|---|
| 2026-09-04 22:32:13 +0000 | `f8d0c24` | ADR 0036: Argo CD and Kargo return on a control-plane cluster, and the acceptance suite holds the new path | The owner's instruction, given twice today: Argo CD and Kargo are the deployment path. |
| 2026-09-04 17:32:39 +0000 | `819c74e` | Feed the agents' desk from the platform's own observations, and record an expired organisation as an outcome | The finding this closes was proven by arithmetic against the log: no running binary had ever produced an order because the desk handed to the eighteen agents was a cold copy taken at assembly, and nothing fed it. |
| 2026-09-04 16:55:08 +0000 | `e6f9632` | Render predictions, correlation and backtests from the platform, and retire the console's last simulated placeholders | Three pages carried seeded data under a SIMULATED badge because no route served the fact. |
| 2026-09-04 16:41:52 +0000 | `576b9e0` | Serve predictions, correlation and backtests from the platform's own state, and answer regimes and news in its own words | The console, walked page by page against a running qip-api today, rendered labelled placeholders on four pages because no route served the fact. |
| 2026-09-04 16:41:52 +0000 | `b7bcee0` | Drive the brains from a bitemporal replay tape on tape time, and lift every credential read out of the storage library | Two streams that share the brains' configuration files, landed together because neither compiles without the other's lines. |
| 2026-09-04 16:19:45 +0000 | `dcacab0` | Delete two dead deployment scripts, and record what could and could not be observed of dev from outside the project | seed-secret-versions.sh seeded the same six secrets bootstrap-deploy.sh step 7 seeds, was written for a CSI driver and SecretProviderClasses that left with the cluster, and was referenced by nothing but its own usage line. |
| 2026-09-04 16:19:45 +0000 | `c77877f` | Disposition a retired strategy's positions, and run the strategy review from the LEARN stage so retirement reaches a deployed binary | Two gaps the last wave left open, both in the central plane, closed together because the second is what makes the first reachable. |
| 2026-09-04 16:19:45 +0000 | `c0316b4` | Tell the portal deploy to require a session, refuse to attest an unreviewed tree, and say ADR 0033 is not yet applied | Three findings from the independent security review of the merged state. |
| 2026-09-04 16:19:45 +0000 | `28b0e56` | Close the portal's session gate by default, gate the SSE route it never gated, and walk every page against a running backend | An independent review found the console shipped to the internet with its session gate off: the gateway route and the proxy both read ALGORIK_AUTH_REQUIRED and opened unless it was exactly "true", so a deployment that forgot the variable proxied every viewer route of qip-api anonymously with the mounted bearer token. |
| 2026-09-04 15:48:59 +0000 | `3b95f0c` | Record the digests dev now serves, from run 33891084271 | Written by .github/workflows/deploy.yml after each Cloud Run service was moved to the attested digest for bb4711998ecaec49a1b3f78a61bdba7f8ef10e8b and proven serving it. |
| 2026-09-04 15:26:51 +0000 | `4c28391` | Compare regime-conditioned sizing against the unconditional baseline over a split declared before the run | Gate 8 asks whether regime conditioning beats unconditional allocation out of sample, and the tree could not ask the question: the sizing rule existed, the Sharpe deflation existed, and nothing ran the same policy twice with the regime term removed, on the same paths, and scored the two on a holdout. |
| 2026-09-04 14:48:43 +0000 | `0c62a79` | Apply ADR 0029: delete qip-normalization, and correct the four documents that cited it as a control | The ADR was taken and not applied, and every day it stayed that way the tree carried a crate nothing constructed, whose unmapped-symbol guard could not fire, cited by four documents as a control at the ingestion boundary. |
| 2026-09-04 14:29:26 +0000 | `3deace8` | Retire a strategy that stays in decay at the floor, without a human, and say why the threshold is a duration | Blueprint §20.3: "retirement is as automated as promotion." It was not. |
| 2026-09-04 14:18:51 +0000 | `a14018d` | Withdraw B14's "buildable now" for three payload slots, on evidence | The plan said slots 2, 10 and 11 were buildable now. |
| 2026-09-04 14:18:51 +0000 | `c643d42` | Put both frontends under the admission policy, give the landing its own identity, mount the session secret as a file, and pin all four | Register gaps 1, 2 and 4, each BLOCKING-A-GATE, and each a control this repository claimed to hold that did not hold for the two internet-facing services -- because they were deployed by a shell script outside Terraform, where none of modules/cloudrun's guarantees reach. |
| 2026-09-04 14:18:50 +0000 | `63e4556` | Install the per-region allocation in the node's composition root, and refuse to boot a node without one | Per-region capital reservation has been implemented and tested in qip-edge since 0ca4b92 -- RegionAllocation::reserve, Cell::with_region_allocation, hold_region_capital on both capital-committing paths -- and no composition root constructed it. |
| 2026-09-04 14:10:40 +0000 | `d703f4b` | Score the platform's probability against the venue's implied one, and refuse a quote known too late | Gate 6 asks whether calibrated probability beats the market's implied probability on prediction contracts, and nothing computed the comparison: pricing.rs derived the implied probability from bid and ask, resolution.rs settled outcomes, and no arithmetic ever put the two beside a forecast. |
| 2026-09-04 14:05:56 +0000 | `4f765d1` | Correct the register's method claims, and record that A6 was reviewed and refused | The register's gap findings were true and stay. |
| 2026-09-04 14:04:57 +0000 | `fbb73a7` | Give the console-egress subnet the deny every trust zone already has, with the narrowest allow above it | The console-egress subnet was the one subnet in the deployment with no egress deny. |
| 2026-09-04 12:51:04 +0000 | `45fc3a8` | Run the frontend gates for the first time, and correct five documents that were reporting work as undone | Two things, both about the difference between what this platform does and what its own records say it does. |
| 2026-09-04 12:44:12 +0000 | `2fd254f` | The collector review returns a refusal: its only published version carries an unfixed CRITICAL | The last commit uncommented Google's `cloud-run-gmp-sidecar` line and named its digest in dev, on the strength of two checks passing. |
| 2026-09-04 12:35:03 +0000 | `d17f4bd` | Review the collector image, adopt it, and let a tfvars digest be checked against the list rather than forbidden | A6 has been open since the cluster left, and what it was waiting for was not work. |
| 2026-09-04 11:54:40 +0000 | `cdc28a7` | Take the six decisions the gate plan was waiting on, on best practice | The plan written an hour ago ended in a table of six things no engineer could settle. |
| 2026-09-04 11:48:08 +0000 | `0d1cc9a` | Plan the four gates as gates, because five waves of backlog have not moved one of them | The backlog has been the organising unit for five waves and the gate count has been 0 of 4 throughout. |
| 2026-09-04 09:36:40 +0000 | `243f3a1` | Grant the accessor role for a secret that arrives as an environment value, and order it before the revision | ADR 0031 added `secret_env` and added no IAM. |
| 2026-09-04 06:41:10 +0000 | `0882a02` | Withdraw a boundary this workflow never had, and let the kernel fixtures run the set that ships | Two corrections and one deletion, all of the same kind: something in the tree claimed a protection that was not there. |
| 2026-09-04 06:30:14 +0000 | `0e9cdb9` | Seed a credential that has no version, because terraform creates the container and Cloud Run resolves a version | Two applies failed identically -- `Secret .../versions/latest was not found` -- and both times thirteen of fifteen resources landed while the one service that needed a credential did not start. |
| 2026-09-04 06:18:35 +0000 | `d05cfb5` | Let a vendored image take its credential from the environment, because this one cannot read a file | The OpenObserve service has never started. |
| 2026-09-04 02:11:39 +0000 | `f102112` | Say on the telemetry page that nothing collects any of it | The page rendered process counters and an event stream and looked, to anyone opening it, like observability. |
| 2026-09-04 01:54:59 +0000 | `26fa4f4` | Make IngestionService constructible by a root, and hold the console's two absolutes with tests | `IngestionService` was constructed only by its own tests, so the front of the SENSE stage was wired to nothing. |
| 2026-09-04 01:45:48 +0000 | `0ca4b92` | Per-region reservation at the cell, a licensing gate that refuses, bounded streaming, and two suites for the gaps that bit today | Four slices, each complete and verified, on disjoint files. |
| 2026-09-04 01:09:06 +0000 | `35f0470` | Give both brains the drain, and say honestly why no deployment may point it anywhere | `qip-fastbrain` and `qip-deepbrain` construct the same `Telemetry` the API does and serve the same snapshot, and exported nothing, so two thirds of the central plane had no path off the process. |
| 2026-09-04 00:30:26 +0000 | `61f1221` | Encode every OTLP leaf the way the spec names it, and bound what streaming keeps | Two agents' work, landed together because both are complete and verified and neither touches the other's files. |
| 2026-09-04 00:25:28 +0000 | `75804bb` | Stop the invokers description naming the two principals the IAM scanner hunts | A follow-up to 8773982, which I pushed with this test red and should not have: the workspace gate quoted in that message ran before this file's last edit, and the `&&` chain meant to re-check it broke on a wrong relative path, so the commit went out anyway. |
| 2026-09-04 00:19:39 +0000 | `8773982` | Expose OpenObserve anonymously under ADR 0030, replacing the guards rather than deleting them | The owner asked for this four times across one session, in escalating terms, after being shown twice what it costs and what the alternative was. |
| 2026-09-03 20:58:11 +0000 | `13e1a45` | Take the withdrawal's fills by position, because the record is cumulative | `run_pass` spliced the fills a withdrawal turned up by matching order id against the expired list. |
| 2026-09-03 20:50:01 +0000 | `6855146` | Pin the reviewed OpenObserve digest in dev and declare the zone it needs | Three preconditions stood between the vendored image and a service, and all three are met now, so this sets the digest that creates it. |
| 2026-09-03 20:35:42 +0000 | `b86e0e9` | Read the OpenObserve credential from a file, and refuse the values that would fail silently | The drain landed this session reading QIP_OPENOBSERVE_AUTHORIZATION straight from the environment. |
| 2026-09-03 20:13:43 +0000 | `eca7ebb` | Decide ADR 0027: concentration is a share of equity, and the first order is admitted | The default limit set refused the first order of every deployment that fed it a real catalogue. |
| 2026-09-03 19:51:32 +0000 | `ddf5777` | Make deploy.yml write images.tfvars that terraform fmt accepts | The first time the digest-recording step ever succeeded, it turned the branch red. |
| 2026-09-03 18:20:42 +0000 | `62b4abf` | Bound the edge node's health socket, and register every gate that is closed everywhere | Two independent findings from the parallel sweep, landed together because both are complete and verified and neither touches the other's files. |
| 2026-09-03 16:47:22 +0000 | `a5e82c8` | Record the digests dev now serves, from run 33780092495 | Written by .github/workflows/deploy.yml after each Cloud Run service was moved to the attested digest for c3140aff007a629f9fc0d654efde6cfd7e339f5a and proven serving it. |
| 2026-09-03 16:18:43 +0000 | `c3140af` | Make the cloudrun module something terraform will actually validate | ADR 0028's vendored-workload scaffolding shipped in ee09809 with two constructs Terraform refuses outright, and `terraform validate` has failed on every commit carrying them since. |
| 2026-09-03 11:26:02 +0000 | `03655a4` | Wire qip-observability's metrics to OpenObserve over OTLP/JSON (ADR 0028) | ADR 0028 retargets ADR 0026's Option (b) mechanism — a producer records into a bounded structure, a drain thread in a composition root takes it on an interval and POSTs OTLP/JSON to a collector, blocking, with an explicit timeout — at OpenObserve instead of a Google-managed collector. |
| 2026-09-03 09:03:57 +0000 | `bf8ee3b` | Distil the teacher into the linear student the execution path can run | qip_training::distill::distil had the same shape of gap as the two model- governance gaps this branch already closed: fully implemented, well tested from its own crate, and reachable from no running process. |
| 2026-09-03 11:03:41 +0000 | `352ede8` | Wire ADR 0028's remaining infrastructure decisions: modules/cloudrun learns a vendored source, and OpenObserve becomes a real, if not-yet-applied, workload | ADR 0028 decided OpenObserve is adopted, and named three infrastructure consequences this commit implements (decisions 3, 4 and 5); decisions 1 and 2 (the wire protocol and the drain thread) are Rust work this does not touch. |
| 2026-09-03 10:43:09 +0000 | `98df818` | ADR 0028: OpenObserve adopted as a deliberate deviation from §2.1, over OTLP, on ephemeral storage | ADR 0026 already existed as the proposed (not decided) record for this exact question -- what backs the platform's dashboards -- and quotes the blueprint's own §2.1 directly: "managed services are Google Cloud or IBM only... |
| 2026-09-03 09:34:14 +0000 | `8ff54d3` | fix(api): stop the mesh backbone's Debug from printing the trust-root signing key | qip-api's MeshBackbone derived Debug while holding policy_key: Option<Vec<u8>> in plain bytes — the same QIP_CAPITAL_ENVELOPE_KEY the trust module installs and MeshBackbone signs every capital envelope and policy payload down the mesh with. |
| 2026-09-03 09:21:41 +0000 | `9f4a557` | qip-market-ingestion: register Frankfurter in the connector bridge ConnectorFeed already exposes to a composition root | Before this change: SENSE is fed exactly one way in a deployed process — qip-fastbrain's `Feed::open` polls a `DataAdapter` and passes its records straight to `Platform::observe`, in `qip-fastbrain/src/feed.rs`. |
| 2026-09-03 09:12:40 +0000 | `3c1fcdc` | docs: score five requirements the blueprint's full text made scoreable for the first time | docs/architecture/algorik-blueprint-v10.1-source.md, committed earlier this session, is the first time the ~232-section prose of the blueprint (as opposed to the shorter, unnumbered HTML companion) has been available to score docs/architecture/algorik-blueprint-traceability.md against. |
| 2026-09-03 09:09:55 +0000 | `9071b4e` | qip-web: stop shipping the inlined stylesheet as escaped HTML text | Every one of the nine investment surfaces and nine console views embeds one stylesheet through Element::text, which HTML-escapes on the way in. |
| 2026-09-03 05:42:59 +0000 | `8f6713e` | docs: commit the Algorik blueprint's actual full source text, for the first time | Every numbered section this repository already cites -- §6.2, §18.1, §25, §37, §37.4, §40, §41.4, §41.5, §41.6, and others across CLAUDE.md, the ADRs, and .claude/rules/ -- refers to sections of a ~232-section prose document that has never itself been committed to this repository. |
| 2026-09-03 04:50:09 +0000 | `0234d8b` | vendor OpenObserve's image digest; name the two decisions before it deploys | Requested this session ("deploy openobserve"). |
| 2026-09-03 04:34:23 +0000 | `2a6fd43` | qip-kernel: stop a resumed platform's universe-record id colliding with a fresh one | Platform::new's inherited-history integrity depends on qip-events' just-merged duplicate-id refusal, and the full workspace run this merge sequence needed surfaced a genuine collision it was designed to catch: context.ids() is seeded purely from config.seed, so a process resuming an existing log and a from-scratch process starting at the same instant mint the exact same id for the universe record -- deliberate determinism (a documented reproducibility guarantee elsewhere in this codebase) turned into a real collision, because the resumed run's record sits at a different position in a longer chain than a from-scratch run's does at the same wall-clock instant. |
| 2026-09-03 04:20:12 +0000 | `daaa472` | qip-sequencing: restore SequenceTracker::position()'s immediate floor after expecting() | The wave-7 fix to SequenceTracker::expecting (41a5b28, merged this session) moved the "where this stream should start" floor from an eager write into `contiguous` (`first_sequence.checked_sub(1)`) to a separate `expected_start` field, to stop first_sequence == 0 from being indistinguishable from "no expectation at all". |
| 2026-09-03 04:13:48 +0000 | `10fed40` | qip-lifecycle: refuse a trial account borrowed from another strategy | The holdout gate deflates a Sharpe ratio against the lifetime trial count carried on evidence.trial_account, and checked only that the account's this_run figure matched the evidence's own trials count. |
| 2026-09-03 03:55:45 +0000 | `aaabbf8` | Stop a withheld event's time from advancing a connector's cursor | ConnectorRuntime::ingest computed the cursor position it hands to SourceConnector::advance from the newest event *time* across the whole batch, with no distinction between an event that was admitted and one that was withheld because its knowable instant had not yet arrived on a delayed feed. |
| 2026-09-03 03:37:59 +0000 | `d1cdab9` | qip-cli: refuse a non-numeric cycle count instead of silently running one | `qip cycle abc` parsed the count argument with `.and_then(\|n\| n.parse().ok()).unwrap_or(1)`, so any argument that was not a valid u64 — a typo, a stray flag, a negative number — was silently replaced with 1 and the run went ahead as if the operator had asked for exactly that. |
| 2026-09-03 03:36:51 +0000 | `5d1f22d` | qip-events: refuse an event id reused across two log records | EventLog::index keyed by_event_id with a plain map insert and never checked whether the id was already present. |
| 2026-09-03 03:36:33 +0000 | `8947a94` | qip-storage: a checkpoint that cannot complete no longer fails the commit that triggered it | DurableStore::commit_locked propagated a checkpoint error with `?`, so a caller of put/delete/commit could receive Err for a write whose record had already been appended to the write-ahead log and fsynced — durable and already visible through get. |
| 2026-09-03 03:35:55 +0000 | `d641001` | qip-evolution: refuse a cost correction that never earned it, at the one site that applies it | The module documentation for qip-evolution's cost model already promised that a bias which does not survive a walk-forward check is "refused, not quietly applied" -- but nothing enforced it. |
| 2026-09-03 03:34:48 +0000 | `911ec09` | qip-normalization: make the quality-floor a check that can actually fail | Every standard data contract (quote-sanity, trade-sanity, bar-sanity, macro-sanity, news-sanity) carries a minimum_quality field, documented on DataContract as "minimum acceptable data-quality score" — but DataContract::check never read a record's own DataQuality against it. |
| 2026-09-03 03:34:44 +0000 | `c1e62d3` | Refuse a demand forecast fitted on an observation from after its own as-of instant | qip_capital_fabric::forecast::DemandForecaster::forecast() accepted a history slice without checking any entry's timestamp against `as_of`. |
| 2026-09-03 03:34:43 +0000 | `468bda3` | qip-risk-engine: stop counterparty exposure from only ever growing | RiskState::counterparty_exposures is documented in qip-risk as gross exposure per counterparty, read back through abs() by MaxCounterpartyExposure. |
| 2026-09-03 03:31:18 +0000 | `aa92d12` | deploy.yml: retry the digest-bookkeeping push through a fetch-and-rebase loop | Run 33711008893 proved the 63dbe20 detached-HEAD fix works -- the push now correctly names refs/heads/${GITHUB_REF_NAME} instead of failing with "You are not currently on a branch". |
| 2026-09-03 03:30:43 +0000 | `d6c1ef0` | qip-observability: stop counters and histogram buckets from wrapping | Metrics::increment and Histogram::observe both accumulated into u64 fields with a bare += / += 1. |
| 2026-09-03 03:29:53 +0000 | `a9bfb9c` | qip-core: redact the whole subtree of a secret-named config key | Config::redacted() masked a secret-looking key only when its value was a scalar. |
| 2026-09-03 03:24:22 +0000 | `ad1d56f` | docs: stop the ops README claiming Cloud Run has no collector at all | The Terraform side of this gap closed at ce28b16: modules/cloudrun already declares the managed-Prometheus sidecar for both brains, rendered only once metrics_collector_image_digest names a mirrored, attested image. |
| 2026-09-03 03:08:36 +0000 | `e512760` | qip-chain: refuse a block hash reused for different contents | ChainState::apply keyed retained history on block.hash but never checked that a second arrival under an already-seen hash actually matched what was recorded — it just returned Applied::Duplicate. |
| 2026-09-03 03:06:43 +0000 | `0b9d8a5` | qip-contracts: close the Entitlement lower-bound gap and a false clamp report | Entitlement::is_granted checked only the upper bound (expires_at), the same gap already fixed for qip_data_finder::SourceLicense this session with an opt-in effective_from field. |
| 2026-09-03 03:05:35 +0000 | `9c8c932` | qip-twin: refuse a liquidity estimate from fewer than two bars | The audit found that a liquidity estimate's own observation count - Liquidity::observations, with a doc comment saying two bars and two hundred 'size an order very differently' - was computed and then never consulted. |
| 2026-09-03 03:02:35 +0000 | `d9102d3` | qip-mesh: DatasetRegistration::permits gains a lower time-bound | permits(usage, now) checked only that an entitlement had not yet expired, with no floor at all: a query about an instant before the dataset was ever registered into the mesh answered exactly as it would for today. |
| 2026-09-03 03:02:21 +0000 | `be50d9c` | qip-confidential: stop ReleaseGate's derived Debug from printing the seed | The seed is key material by the crate's own documentation: noise_for recovers every release's true value from it, and NOT_DEFENDED_AGAINST already names seed disclosure as a known limit of the mechanism. |
| 2026-09-03 03:02:12 +0000 | `d505fb1` | entity-resolution: stop weak identifiers forcing an identical-name match apart | score_pair's identifier-disagreement check ignored IdentifierKind::confidence entirely: has_shared_identifier_kind fired for *any* shared kind that disagreed, including ExchangeSymbol (0.4) and ProviderKey (0.55), which the crate's own doc comment says are 'reused across venues and reassigned over time' and must not be treated as authoritative. |
| 2026-09-03 03:01:24 +0000 | `9ae2fa8` | qip-prediction: refuse a haircut with nothing to price and a binary market with no distinct no-id | Two constructors in this crate accepted an input their own invariants already rule out for their sibling constructor: |
| 2026-09-03 03:00:25 +0000 | `e8daa51` | Refuse an off-grid order at the central path before it reaches a venue | The edge cell has carried a feasibility gate ahead of its profitability filter since qip-edge's own feasibility.rs (blueprint §18.1): a lot, tick or minimum a venue states is a cheaper question than whether an edge exists, and an order that fails one is inexpressible rather than merely unprofitable. |
| 2026-09-03 02:49:23 +0000 | `b2fbcc2` | Report a scenario shock nothing in the book is exposed to, and carry its stressed correlation through to the result | An audit of qip-simulation-engine's scenario library found two values that were computed or accepted but never reached the caller. |
| 2026-09-03 02:47:43 +0000 | `e9eb14d` | qip-training: refuse a zero minimum leaf or zero candidate splits instead of clamping them | fit_boosted (local.rs) and fit_tree_student (distill.rs) both silently corrected min_samples_leaf: 0 to 1 and candidate_splits: 0 to 2 before fitting. |
| 2026-09-03 02:45:31 +0000 | `6d0943b` | reasoning: make the staleness-limit policy actually govern the staleness check | RedTeam::review computed the staleness limit as a hardcoded horizon * 2 and used ReviewPolicy.staleness_limit only as an on/off switch (> 0.0), discarding the configured multiplier entirely. |
| 2026-09-03 02:42:45 +0000 | `d7049d4` | Feed MaxVolatility the same return series that already fills the tail limits | RiskState::volatility had the exact defect RiskState::expected_shortfall once had, just without a map to expose it: RiskState::from_figures never touches it and PreTradeChecker::project says outright that it leaves volatility as it stands, so the shipped MaxVolatility limit read whatever a caller happened to set by hand and nothing in production ever set it. |
| 2026-09-03 02:41:25 +0000 | `f64901d` | Register a source only after its own decision agreed to, not before | self.registry.insert() ran before RegistrationDecision::registered() was called and checked, so a source reached the finder's own catalogue on the strength of a routing/legality pairing the code had merely assembled, not one the gate had actually confirmed. |
| 2026-09-03 02:30:36 +0000 | `21e0bad` | Verify the console's real data and cover the governance page with Playwright | Wave 5's claimed console features — sent-vs-filled aggregates, reconciliation breaks by direction, the two/three halt wires, the twelve-item payload slot table, and a universe/catalogue exclusion view — do not exist anywhere in frontend/portal, and none of them can be built from qip-api's JSON REST surface: those fields are rendered only by the Rust operator console (qip-api::web / qip-web, mounted at /console) and by the Prometheus text exposition at /metrics, neither of which frontend/portal reads or can reach through its gateway (which only forwards to /api/v1/<path>). |
| 2026-09-03 02:29:47 +0000 | `35d528d` | Re-score the completion plan after wave 5 and the infrastructure-pipeline debugging session | The plan's own §6 said "no project reachable" — true at every prior scoring and false now. |
| 2026-09-03 02:22:16 +0000 | `63dbe20` | Push the deployed-digests commit to a branch instead of a detached HEAD | Run 108 got all the way through: images built, scanned, signed, attested, and 'move every service to the attested digest and prove it serves it' succeeded — all three Cloud Run services proven serving the digest this commit produced. |
| 2026-09-03 02:21:48 +0000 | `c2fe63e` | qip-normalization: stop reporting a timestamp correction that did not happen | clamp_timestamp fell through to a no-op for Bar, CorporateAction, Fundamental, Macro, AlternativeData and ReferenceData records, but the caller incremented NormalizationReport::timestamps_corrected whenever occurred_at() > received_at regardless of record kind. |
| 2026-09-03 02:21:23 +0000 | `b72de63` | Refuse a depth-feed trade with no stated condition instead of reading it as regular | rest.rs already refuses an absent trade condition on a top-of-book trade, with the reasoning that Regular is the one condition that updates the last sale and counts toward volume, so an unstated condition read as Regular lets a late report or an off-exchange cross move a mark it never traded at. |
| 2026-09-03 02:21:05 +0000 | `582df7f` | docs: fresh structural gap map for blueprint v10.1 against the actual tree | Copies the supplied Algorik blueprint v10.1 HTML unmodified into docs/plan/blueprint-v10.1.html for reference, and adds a new docs/plan/blueprint-v10.1-gap-map.md that grep/read-verifies roughly four dozen of the blueprint's named mechanisms across every plane against backend/crates/** and frontend/**, citing file:line for every BUILT+WIRED, BUILT-UNWIRED and PARTIAL claim and naming the search terms tried for every ABSENT one. |
| 2026-09-03 02:18:57 +0000 | `e859147` | learning-engine: refuse a non-finite factor contribution instead of costing it as zero | Attributor::attribute converted the factor P&L term from f64 to Decimal with unwrap_or(Decimal::ZERO): a NaN or out-of-range factor beta/return silently became a zero factor contribution. |
| 2026-09-03 02:18:25 +0000 | `9825c46` | Fix key reuse in DurableDeadLetters after a release and restart | DurableDeadLetters::open resumed key numbering from the *count* of keys currently under its namespace (`keys_with_prefix(...).len()`), not from the highest sequence number ever assigned. |
| 2026-09-03 02:03:38 +0000 | `10295db` | Bound the liquidity detector's baseline to its own window | LiquidityDetector declared a window field but never sliced by it: the baseline median and MAD were computed over the entire spread history handed to the detector, unlike every other windowed detector in this file (ReturnAnomalyDetector, VolatilityShiftDetector, CatalystDetector), which all bound their comparison history to the last self.window observations. |
| 2026-09-03 02:02:57 +0000 | `2409fed` | qip-portfolio-engine refuses a mandate whose risk aversion or turnover cost is negative | Mandate::validate() checked the cap-versus-minimum-position contradiction but let two other nonsensical mandates straight through to construction. |
| 2026-09-03 02:02:31 +0000 | `44c2594` | Remove is_convex_quadratic, a claim about the solver nothing checks | Audited the compute router for a branch that would skip ADR 0006's classical baseline: ComputeRouter::solve unconditionally runs solve_classical before any quantum assessment, pushes it into runs first, and every return path (error, tie, infeasible, win) carries classical_objective from that run. |
| 2026-09-03 01:52:17 +0000 | `4517f10` | Grant the egress bootstrap bucket the same objects.delete exception the state bucket already has | Run 25 got past terraform init (the previous fix) and reached its first real object-level failure: |
| 2026-09-03 01:51:19 +0000 | `216d3eb` | qip-mesh: dispatch no longer races an earlier held capital instruction to the wire | CapitalDispatcher::dispatch persisted a new envelope and always attempted to send it, even when an earlier envelope to the same cell was still sitting in the spool (down cell, open circuit, or exhausted retry ladder). |
| 2026-09-03 01:50:58 +0000 | `31a2684` | Refuse a console egress range Google would refuse anyway, at plan time | An audit of execution-node, network, trust-zones, connectivity and identity against the paper-trading boundary, WIF-only, default-deny and HCL-escape rules found the execution-node regex the prior session fixed (7663a6c) already correct, and one genuine gap: the network module's console_egress_cidr validation checked only that the value parsed as some CIDR (can(cidrnetmask(...))), while the comment beside it claimed "refusing a smaller one here rather than at apply time". |
| 2026-09-03 01:50:15 +0000 | `9f9c1d4` | Record what infra.yml and deploy.yml actually did against dev, from the run logs | infra.yml's last success is a read-only plan (#15) reporting a state that still disagreed with configuration by 135 resources; every up dispatch on this branch has failed, most recently on a storage.objects.delete gap the state-bucket grant never extended to the egress-config bucket. |
| 2026-09-03 01:50:10 +0000 | `35a562d` | Add wave 7 cycle conformance review: stage_simulate does not simulate | Read-only design-conformance trace of Platform::run_cycle's 8 stages against CLAUDE.md's and docs/architecture/README.md's per-stage claims, each verdict cited on both sides. |
| 2026-09-03 01:50:06 +0000 | `4fa36ce` | qip-cost-router: DeterministicRouting's doc overclaimed its own guarantee | DeterministicRouting's comment said Router::select also returns it when a NotRequired decision happens to settle on the DeterministicCode rung. |
| 2026-09-03 01:49:04 +0000 | `3c6a0be` | qip-risk: close the same control-cannot-fire gap for MinLiquidity | The repo's own template for this class of bug was MaxExpectedShortfall: RiskState::expected_shortfall was always empty, so the limit read None on every book and RiskState::with_tail_risk was added to fill it. |
| 2026-09-03 01:48:03 +0000 | `c902198` | qip-capital: prove a reservation cannot be spent or released twice | Audited ReservationLedger for expiry logic, double-spend and double-release: reserve/commit/release/expire_due are correct as written — commit and release each remove the entry from the map on success, so a second call against the same id finds nothing rather than crediting free or committed a second time. |
| 2026-09-03 01:44:54 +0000 | `a4d4f32` | qip-financial: cap breakeven_participation to the domain impact_bps actually prices | TransactionCostModel::breakeven_participation inverted the square-root impact law without the cap impact_bps itself applies: impact_bps caps modelled impact at participation 4.0 (four days of volume in one), "so a pathological participation figure cannot produce a nonsensical cost", but breakeven_participation squared the uncapped inverse regardless of budget. |
| 2026-09-03 01:44:43 +0000 | `f17584a` | qip-quantum: ClassicalValidator::new refuses a NaN or negative tolerance | A validator built with a non-finite tolerance validated every claim unconditionally: qip-quantum/src/benchmark.rs's discrepancy check reads 'discrepancy > self.tolerance', and that comparison is false against a NaN tolerance for any discrepancy, however large. |
| 2026-09-03 01:43:46 +0000 | `5d85a8b` | qip-ai: read hosted-model credentials through the shared secret resolver | RemoteModel::new called std::env::var directly, so a credential the Secret Manager CSI driver mounted only as <VAR>_FILE read as absent — an operator would be sent chasing a variable that was in fact supplied correctly, and the resolver's refusal when both the variable and the file are set was bypassed entirely. |
| 2026-09-03 01:42:47 +0000 | `ece48c1` | qip-agents: a budget exhaustion is a guard trip, not a capability denial | BudgetLedger::exceed raised Error::denied for a run that ran out of tool calls, tokens or wall time. |
| 2026-09-03 01:30:24 +0000 | `0010760` | qip-strategy: give StrategyRuntime::new a budget independent of its program | StrategyRuntime::new derived its default cost ceiling from the program it was handed — program.total_cost(), the sum of every node's cost in the arena. |
| 2026-09-03 01:28:04 +0000 | `415eadf` | fastbrain: refuse overflowing duration and breach-tolerance config instead of clamping | QIP_FASTBRAIN_CYCLE_INTERVAL_MS, QIP_FASTBRAIN_CYCLE_BUDGET_MS, QIP_FASTBRAIN_SHUTDOWN_BUDGET_MS and QIP_FASTBRAIN_MAX_RUNTIME_SECS all went through millis()/a bespoke seconds conversion that silently capped a value too large for i64 at i64::MAX instead of refusing it, and QIP_FASTBRAIN_BREACH_TOLERANCE did the same against u32::MAX. |
| 2026-09-03 01:27:08 +0000 | `6cabf73` | Name the champion a challenge round actually kept, not the instrument itself | The uncrowned branch of ChallengeSummary::describe() read 'succession: OBJ-AAPL holds OBJ-AAPL', which says an instrument holds itself rather than naming any champion. |
| 2026-09-03 01:26:53 +0000 | `2782f6c` | qip-portfolio: a fee-only fill now moves the position's own cost ledger | Position::apply_fill's zero-quantity branch returned -costs as cash flow — correct — but returned before total_costs += costs and before updated_at was set, so a standalone fee (or a requested quantity that rounds to zero at the decimal's fixed 9-digit scale, which the simulation engine can produce from Decimal::from_f64 on a very small delta) debited cash through the caller while leaving the position's own total_costs unmoved. |
| 2026-09-03 01:26:22 +0000 | `b170af9` | Refuse a redelivered fill report instead of double-booking it | Order::apply_fill and LegGroup::record_fill both bounded quantity against what remained, but neither checked the fill's identity. |
| 2026-09-03 01:26:13 +0000 | `c37b4df` | qip-numerics: refuse a zero-asset box projection to a non-zero budget | project_box_sum shortcut-returned Some(vec![]) for an empty box before checking whether the requested total was reachable, so a caller asking to project zero assets onto a non-zero budget got a silent "feasible, sum is zero" answer instead of the None the docstring promises for an infeasible constraint set. |
| 2026-09-03 01:25:42 +0000 | `dd5c8e5` | qip-sequencing: fix two ways a real gap could go undetected | SequenceTracker::expecting(_, _, 0) silently reverted to unconstrained behaviour: contiguous = first_sequence.checked_sub(1) returns None for first_sequence == 0, which is indistinguishable from SequenceTracker::new's "no expectation at all" — a cell resuming a stream that numbers from zero would treat whatever arrived first as the start, hiding exactly the loss `expecting` exists to make visible. |
| 2026-09-03 01:25:01 +0000 | `d950292` | Check every venue a synthetic edge touches, not just its two endpoints | ArbitrageGraph::edge_is_tradable is the only guard search_candidates consults before proposing an edge, and its own module comment promises that "an unusable edge is weighted zero rather than infinite... |
| 2026-09-03 01:24:40 +0000 | `43b1310` | Refuse a health policy whose own thresholds contradict each other | Every sibling policy in qip-routing validates its invariants at construction — RepricePolicy::validate, VenueProfile::validate, FeeSchedule::tiered — but HealthPolicy did not, despite carrying the same class of ordering dependency the crate elsewhere treats as a defect (the RiskState::expected_shortfall precedent named in .claude/rules/domains/risk-and-execution.md: a limit that cannot fire reads as protection and is not). |
| 2026-09-03 01:24:26 +0000 | `70a136e` | Report a policy-poll failure on the mesh as reportable as a capital-poll one | MeshTick::is_quiet() checked poll_error (the capital downlink) but not policy_poll_error (the policy downlink), even though the two fields are set by parallel code and the policy downlink is the one that carries halt commands. |
| 2026-09-03 01:21:55 +0000 | `54c0fa4` | Stop the spread history from duplicating on off-touch messages | InstrumentState::spreads is documented as one entry per touch change, matching the flow series beside it, but refresh_touch pushed unconditionally on every message that reached it — an order added three levels away from the touch, or an OrderReduced on an order that no longer exists, still calls refresh_touch and still appended the current spread again. |
| 2026-09-03 01:21:33 +0000 | `d7d3293` | qip-orderbook: report a resting order whose level cannot be found | Ladder::resize_level, the primitive behind L3Book::reduce and the size-shrinks-in-place branch of L3Book::replace, silently did nothing when the price it was asked to resize held no level. |
| 2026-09-03 01:20:19 +0000 | `25d1164` | qip-protocols: refuse a truncated fractional SendingTime and an unbounded newline frame | Two decoders trusted malformed input instead of refusing it. |
| 2026-09-03 01:19:25 +0000 | `9742a2d` | investment-agents: stop discarding two computed values | Two agents in the organisation computed a value and then threw it away with `let _ = ...;` — exactly the pattern the house rules single out as worse than not computing it at all, because it reads as a control that was never wired up. |
| 2026-09-03 01:12:25 +0000 | `7663a6c` | Escape the node validation's regex the way HCL, not Rust, reads it | Run 24 of infra.yml refused the plan before it reached Cloud Run: |
| 2026-09-03 01:06:20 +0000 | `550496a` | gitignore per-agent worktree scratch directories | .claude/worktrees/ holds one checkout per background agent spawned with isolation:"worktree"; the stop hook flagged it as untracked. |
| 2026-09-03 01:06:15 +0000 | `bdf782f` | Correct two diagram audits against the tree wave 5 and ADR 0024 left behind | Two verifiable overclaims and one stale historical audit, none of them noticed by the prior GKE-only correction passes because they are about the trade path and the identity model, not the runtime substrate. |
| 2026-09-03 00:52:42 +0000 | `2e19a4c` | The node module refused the same bootstrap the other module now admits | Run 23 dispatched the previous commit's fix and never reached the service update — it died at plan time, before any resource was touched: |
| 2026-09-03 00:13:53 +0000 | `32b344d` | Bind the health listener where Cloud Run's probe can actually reach it | The grant landed and the diagnosis finished the job it was built for. |

---

## 4. What is true today

**What runs as a process.** Nothing runs continuously outside a test. In the
`algorik-dev` GCP project, environment `dev`, five Cloud Run services have
been observed answering real HTTP requests as of 2026-09-04: the portal and
landing (public, `200`, correct titles), OpenObserve (public per ADR 0030,
`200`/`401` as designed), and `qip-dev-api`/`qip-dev-fastbrain`/
`qip-dev-deepbrain` exist behind internal ingress (Google Frontend `404`,
which is the shape of "a service exists here," not "no service exists").
`qip-dev-fastbrain` is separately confirmed, from a CI run's own output, to
be serving the attested image built and signed by this branch's pipeline.
None of the five has had its running revision's digest, secret mounts, or
service identity independently confirmed by an authenticated read from
outside the project — every claim above rests on an unauthenticated `GET` or
on CI's own self-report. No execution node exists in any environment
(`execution_nodes = {}` everywhere); ADR 0035 authorises exactly one, in
`us-east4`, shadow mode, dev only, and it has not been applied.

**What is proven only by tests.** The entire edge/execution plane — netting,
feasibility, the arbitrage desk, the per-region reservation table, the
second halt wire, fill-vs-placement billing — is TESTED in `cargo test
--workspace` and reached by no deployed process, because no execution node
exists. The four blueprint gates (Phase 2, 3, 6, 8 exits) now each have the
*arithmetic* to be evaluated — cumulative trial accounting and a holdout
band; a regime-conditioned-vs-unconditional comparison (`4c28391`); a
probability-vs-venue-implied comparison (`d703f4b`) — but every one has run
only against synthetic or replayed data. Zero of four blueprint gates have
passed on real data or a real venue.

**What is deployed in `dev` (from the register and the digest-record
commits).** All three central binaries plus both frontends plus OpenObserve
are catalogued in `infrastructure/terraform/catalogue.tf` and recorded as
moved to an attested digest by `deploy.yml` in run 33780092495 and again in
run 33891084271 (`a5e82c8`, `3b95f0c`). GKE has been torn down for real
(`8194b3b`). `test`, `stage` and `prod` are untouched — `project_id =
"unprovisioned"` in all three, deliberately.

**What is not applied.** The ADR 0036 cluster — Argo CD and Kargo returning
on a GKE Autopilot control-plane cluster per environment that runs no
trading binary, Config Connector `RunService` manifests replacing
`modules/cloudrun`'s service resource — is a decision recorded 2026-09-04
(`f8d0c24`) and **nothing about it has been applied**: no cluster, no Config
Connector, no Argo CD project, no Kargo warehouse exists in any environment
today. The egress sidecar's allowlist still names no market-data vendor. The
observability collector (A6) is refused, not pending, on an unfixed CRITICAL
CVE in its only published version. `workload_metrics_exist = false`
everywhere, correctly, because nothing has been observed to scrape.

**The paper-trading boundary.** Intact, unchanged this session, and checked
at all three layers this document's rule files name: Terraform's
`variables.tf` still refuses `supervised_live`, `limited_autonomous_live`
and `autonomous_live` at plan time; `AutonomyLevel::deployable` still stops
`qip-api`, `qip-fastbrain` and `qip-deepbrain` at start-up on any of the
three; `qip-edge`'s `Cell` still has no constructor taking a live ceiling.
A default-off session gate on the deployed portal — found and closed this
session (`28b0e56`, 2026-09-04) — was a security defect in front of that
boundary, not a breach of it: it would have let an unauthenticated viewer
read the console anonymously, never let one submit an order. The portal
still renders `PAPER TRADING` wherever posture is shown, verified by direct
grep against both frontends' rendered surface (`45fc3a8`), and no control
that could submit a live order exists in either frontend.

---

*The gate for this file, like every scored document in this repository, is*
`cd backend && cargo test -p qip-acceptance --test documentation --no-fail-fast`.
*Run it on every edit and quote the `test result:` line in the commit.*
