# Completion plan — how far from done, what is left, in what order, blocked on whom

**Living document.** Scored on branch `claude/algorik-architecture-refactor-pmp0zy`
at `de5d042` (the last commit in the reflog when this was written), with three
agents still editing `qip-edge`, `qip-edge-node`, `qip-arbitrage`,
`qip-contracts`, `qip-api` and the observability rule file. Anything they land
after `de5d042` is not scored here.

This document aggregates the repository's own scorecards; it does not replace
them. Where it disagrees with one of them it says so rather than picking. The
sources, and what each is authoritative for:

| Source | Authoritative for |
|---|---|
| [`../architecture/algorik-blueprint-traceability.md`](../architecture/algorik-blueprint-traceability.md) | Status of every blueprint requirement — the live scorecard (ADR 0022) |
| [`../architecture/integration-truth-pass.md`](../architecture/integration-truth-pass.md) | Whether the seven flows connect, and where each breaks |
| [`../architecture/blueprint-diagram-reconciliation.md`](../architecture/blueprint-diagram-reconciliation.md) | Where the two authoritative references disagree with each other |
| [`gap-matrix.md`](gap-matrix.md), [`current-state.md`](current-state.md) | The ordered work and the measured state against the earlier diagram |
| [`../adr/0020-two-runtime-topologies-and-the-order-to-resolve-them.md`](../adr/0020-two-runtime-topologies-and-the-order-to-resolve-them.md), [`../adr/0022-the-algorik-blueprint-is-the-architecture-of-record.md`](../adr/0022-the-algorik-blueprint-is-the-architecture-of-record.md), [`../adr/0023-real-trading-is-the-destination-and-the-opening-is-gated.md`](../adr/0023-real-trading-is-the-destination-and-the-opening-is-gated.md) | The migration sequence, the reference, and the opening sequence |

**Vocabulary.** MEASURED (runtime evidence exists) · TESTED (a named passing
test) · CONFIGURED (wired in a manifest or tfvars) · IMPLEMENTED-UNVERIFIED
(code exists, no deployable composes it, or no tool here could validate it) ·
PLANNED (backlogged with a phase) · MISSING. The matrix's ALIGNED bar is
implementation plus a passing named test; nothing below is called done on the
strength of a commit.

**One rule for reading this.** "Code exists" is never "done". The four gates in
§2 are empirical claims about real data and real venues, and no amount of code
passes one.

---

## 1. Definition of done, stated twice

These are two finish lines an order of magnitude apart, and every remaining
item in §4 names which one it belongs to.

### 1(a) Alignment-done — the original brief

The programme is aligned when all five hold, each with evidence a person can
check:

1. **The current phase is internally clean.** Every control that exists can
   fire, every test measures something, and no scored document denies a type
   the workspace defines. Checked by `documentation.rs`, `architecture.rs`,
   the gap-matrix risk register's "control that cannot fire" count, and the
   full gate (`make check`).
2. **Every layer and plane carries an evidence-backed disposition** — a
   status from the vocabulary above with a file, test or commit behind it.
   Checked by the traceability matrix having no UNVERIFIED row that a
   composition root could have resolved.
3. **Changed behaviour is tested and mutation-verified.** Checked by each
   slice's mutation report in its commit message or PR body.
4. **Boundaries are enforced structurally.** The three paper layers, the
   dependency direction, the LM and quantum authority boundaries. Checked by
   `security.rs`, `compliance_proof.rs`, `architecture.rs`.
5. **Future phases are gated, not scaffolded.** Nothing exists for a phase the
   roadmap has not reached unless it has a consumer today. Checked by the
   matrix's PLANNED-FUTURE rows being empty crates nowhere in the tree.

Alignment-done says nothing about whether the platform works on real data. A
fully aligned repository has passed zero gates.

### 1(b) Blueprint-done — all twenty phases and four gates

The platform is blueprint-done when Phases 0–19 of blueprint §51 have met
their exit criteria and the four gates at the end of Phases 2, 3, 6 and 8 have
each been passed on real data or a real venue. Phase 19 is "ongoing" by the
blueprint's own table, so blueprint-done is more precisely "Phases 0–18 exited,
four gates passed, Phase 19 operating".

This finish line also requires the two direction decisions ADR 0022 settled to
be executed — no Kubernetes (ADR 0020 step 5) and Leptos (C3) — and the opening
sequence of ADR 0023 to reach step 9. None of those is authorised today.

---

## 2. Where we are

### 2.1 The four gates — zero of four have passed

| Gate | What it requires | Status | The specific blocker |
|---|---|---|---|
| End of Phase 2 | A family surviving holdout with honest significance after **cumulative** trial correction, on real data | **NOT PASSED** | No deployment has run on sustained real data (Phase 1 exit unmet — see below). Separately, ADR 0023 "What could not be specified" records that nothing counts trials across runs, so "honest significance" cannot yet be computed even on real data. Two blockers, in series |
| End of Phase 3 | Thirty days live, inside the holdout band, no unexplained break | **CANNOT PASS as the tree stands** | Structurally unreachable while ADR 0003 and ADR 0021 stand — three paper layers refuse it. ADR 0023 sequences the opening at steps 5–8; none is approved. Also: no holdout band exists to be inside of |
| End of Phase 6 | Calibrated probability beating the market's implied on prediction contracts, Brier-scored | **NOT PASSED** | `qip-prediction` has `market.rs`, `oracle.rs`, `pricing.rs`, `resolution.rs`; no Brier comparison against a live venue exists (matrix, gates table) |
| End of Phase 8 | Regime-conditional allocation beating unconditional, out of sample | **NOT PASSED** | Regime detection exists (`qip-cost-router/src/context.rs`, `qip-simulation-engine/src/conditions.rs`); no out-of-sample comparison is computed (matrix, gates table) |

The Phase 1 exit — "7 days stable streaming, statistics converged, no raw
stream retained" — is not a gate but it precedes the first one, and it is
unmet: one real tick was fetched in-session through a TLS-terminating bridge
(`gap-matrix.md` item 6), and no deployment has streamed for any duration,
because no egress proxy runs (§6 below).

### 2.2 Per plane — derived from the traceability matrix

The matrix's plane table has seven rows with one status each. Counting them:

| Status | Rows |
|---|---|
| PARTIAL | 6 — Ingestion, Cognition, Intelligence, Optimisation, Execution, Ledger/wallet/treasury |
| MISSING-CURRENT | 1 — Valuation |
| ALIGNED | 0 |

So **0 of 7 planes are ALIGNED, 6 of 7 are PARTIAL, 1 of 7 is MISSING.** A
fraction per plane below is therefore not a matrix number; the matrix gives
one status per plane. What follows is this plan's own derivation, counting the
*named capabilities* in each plane's matrix row and flow evidence, with the
arithmetic shown so it can be disputed. A capability counts as present only at
the TESTED bar.

| Plane | Capabilities named in the matrix row / flows | Present (TESTED) | Fraction | Evidence |
|---|---|---|---|---|
| 1 Ingestion | absorb records; entity resolution; licensing before use; one live source sustained; deep-web tier | 3 of 5 | 3/5 | `absorption.rs`, `sense.rs`, `qip-fastbrain/src/licensing.rs`; live source is PARTIAL (one tick, no deployment); deep-web tier MISSING |
| 2 Cognition | world model; causal graph; episodic memory; hypotheses; belief stage in the cycle; counterfactuals with a production caller; self-model | 4 of 7 | 4/7 | `understanding.rs`, `reasoning.rs`, `world.rs:41`, `causal.rs:234`; belief SUBSTITUTED (flow 2); `Platform::evaluate_alternatives` called only by tests (observability rule); no self-model |
| 3 Valuation | six engines (§16.1) | 0 of 6 | 0/6 | MISSING-CURRENT; deliberately not scaffolded. Corporate actions are *absorbed* (`platform.rs:1159-1180`) but no engine prices anything |
| 4 Intelligence | statistical gate; champion/challenger; drift detection; training; corridor policy; cumulative trial accounting across runs | 4 of 6 | 4/6 | `lifecycle.rs`, `evolution.rs`, `training.rs`, `qip-deepbrain/src/learning.rs:279`; corridor policy has no subject (Phase 12); cumulative trial count MISSING (ADR 0023) |
| 5 Optimisation | routing gate; classical baseline every time; authority boundary structural; family clustering; multi-horizon reconciliation | 3 of 5 | 3/5 | `optimization.rs`, `architecture.rs` solver tests, ADR 0006 |
| 6 Execution | paper-only cell; envelope admission; intent netting; internal crossing; contributor vector on the uplink; halt reaching a cell; §6.2 narrowing; feasibility gate; per-region reservation; crossing settled to books; leg producer for cycles | 7 of 11 | 7/11 | `cell.rs:143-148`, `qip-edge/tests/cell.rs`, `qip-edge-node/tests/gateway.rs`, `qip-api/tests/mesh.rs::a_cycle_ships_a_signed_payload_the_cell_verifies_and_a_trip_reaches_it`; feasibility MISSING, reservation CONTRADICTS (F6), crossing booked-not-settled (F7), cycle legs — a type landed at `3632932`/`6053935`, unscored in the matrix, IMPLEMENTED-UNVERIFIED here |
| 7 Ledger, wallet, treasury | capital allocation; envelope; two-signature approval; reservation ledger in the kernel; per-user per-strategy ledger; wallet; corridor; transfer gate; destination registry; custody | 4 of 10 | 4/10 | `truth_loop.rs`, `compliance_proof.rs`, `platform.rs::a_second_proposal_is_sized_against_what_the_first_still_holds`; the rest are Phase 12 and bounded by ADR 0021 |

Summed: **25 of 50 named capabilities at the TESTED bar.** That number is
this plan's, not the matrix's, and it double-counts nothing but weights every
capability equally, which flatters nothing and nothing in particular: a
missing valuation plane and a missing feasibility gate are both "one".

### 2.3 Per layer — the matrix carries no status cells for layers

The matrix scores layers as *Current / Keep / Change / Remove / Defer /
Verification*, not with the status vocabulary, so there is nothing to count.
The derivation below maps each layer's matrix row, plus the flow links and
constraint rows that bear on it, to items at the TESTED bar. Same caveat as
§2.2: dispute the item list, not the arithmetic.

| Layer | Items | TESTED | Fraction | Evidence and what is missing |
|---|---|---|---|---|
| 1 Experience | sign-up surface; identity call; passkeys; customer mandate; product entitlements; per-user account; Leptos | 1 of 7 | 1/7 | Flow 1: page TESTED, identity IMPLEMENTED-UNVERIFIED, four MISSING; constraint row §2.1 CONTRADICTS (Next.js). Phase 13 |
| 2 Public edge and identity | one identity store (ADR 0019); sealed sessions; console as VPC viewer (ADR 0018); passkeys | 3 of 4 | 3/4 | `console_route.rs`, `security.rs`; passkeys MISSING (Phase 0 in §51) |
| 3 Application and API | documented endpoints exist; K3's narrower reach is what is built; per-user API; typed-intent surface (§40.9) | 2 of 4 | 2/4 | `documentation.rs::every_documented_endpoint_exists`; `qip-api` composes reads only; 30 desk-wide endpoints (`routes.rs:73-299`), none per-user |
| 4 Domain contracts and control fabric | signed payload down; cell verification; atomic swap; §6.2 narrowing wired; outcome return; twelve producers; two independent halt wires; per-region reservation | 5 of 8 | 5/8 | Flow 3 verdict paragraph and `qip-api/tests/mesh.rs`; 2 of 12 payload slots have producers (PARTIAL); halt is mechanism-independent not wire-independent (flow 6); F6 CONTRADICTS |
| 5 Data and state | source→facts; entity resolution; world event; bitemporal, bounded, hash-chained log; a `Ledger` per §43.3; live source sustained; BigQuery derived series; content-hash manifests | 4 of 8 | 4/8 | Flow 2 links TESTED; `truth_loop.rs`; ledger PARTIAL by naming; the last three deferred |
| 6 Cloud and network | GKE transitional runtime carrying traffic; `cloudrun` module; `execution-node` module; `trust-zones` module; egress proxy deployed; Terraform validated | 0 of 6 | 0/6 | Runtime is CONFIGURED, and ADR 0020 step 1's evidence that any pod ever ran is absent, so not MEASURED; the three modules exist under `infrastructure/terraform/modules/` and are absent from `main.tf`'s seventeen `module` blocks — IMPLEMENTED-UNVERIFIED; proxy `Deployment` commented out (`egress.yaml:835`); `terraform validate` NOT RUN (§6) |
| 7 Security, observability, delivery, reliability | three paper layers; LM/quantum authority; WIF only; central telemetry recorded and served; edge telemetry recorded and served; CSI chain exercised live; scrape observed; OTel spans (§47); edge collector and alert; second halt wire; `qip_central_` alerts | 5 of 11 | 5/11 | `security.rs`, `compliance_proof.rs`, `architecture.rs`, `infrastructure.rs`, `qip-edge/tests/telemetry.rs` (per the corrected observability rule); CSI never exercised live (`current-state.md`); `workload_metrics_exist=false` everywhere; the rest MISSING per the observability rule file |

Summed: **20 of 48 layer items at the TESTED bar.** Layer 6 at zero is the
number to notice — everything in it is either transitional or unvalidated, and
§6 explains why nothing about it can be proven from this environment.

### 2.4 Where the scorecards disagree with each other, or with the tree

Recorded, not resolved. Each is a one-slice matrix refresh (§4, item A1).

| Claim | Where | What the tree says at `de5d042` |
|---|---|---|
| "C4 — still open; the rule file says nothing writes to `Telemetry`" | Matrix, C4 | Closed. `.claude/rules/domains/observability.md` was corrected at `232bc16` and now says both planes emit and names the edge series. The rule file is right; the matrix row is stale |
| "The kernel does not consume cell deltas at all" | Matrix, F7 | `Platform::ingest_cell_report` (`qip-kernel/src/platform.rs:1223`) is called from `qip-api/src/mesh.rs:1149`, and `learn_from_cells` (`:1271`) feeds outcomes back. Whether the *contributor vector* joins central attribution is a separate question this plan could not settle by reading; F7's first gap may be narrower than written |
| F8's footgun — a leg that forgets `as_cycle_leg` nets silently | Matrix, F8 | `3632932` "Make a cycle leg a type that cannot be nettable" and `6053935` "Give the arbitrage scanner a way to emit legs the netting seam cannot mistake" landed after the matrix was scored. IMPLEMENTED-UNVERIFIED here: the tests were not run in this session and the matrix has no row |
| "Tests: 3,308 passing at `fef0c97`" | `current-state.md` | Thirteen commits later; not re-measured. The number is stale by construction and says so |
| `NumericFact::observed` has no production caller | `gap-matrix.md` risk register | `480644d` and `125a7de` add an observed-fact constructor and stamp desk reads with it. Not re-verified here; the register's open count of three is likely two |
| ADR 0023 step 3 "buildable today" | ADR 0023 | The same record's reversal section forbids "execution infrastructure built before the Phase 2 gate passes", and blueprint §51.1 says "Stop. Do not build execution infrastructure". The record is in tension with itself; §5 lists it for the owner |
| Blueprint §48 and rule 77: OpenTofu, Cloud Build, Cloud Deploy | Not scored anywhere | The repository runs Terraform 1.9.8 and GitHub Actions. The matrix has no row for this; it is either CONTRADICTS or NOT-APPLICABLE under the transitional runtime, and that is the matrix owner's call, not this plan's |

---

## 3. What landed this session

Grouped by merge. Merge bodies could not be read from this environment (no
shell; git objects are not readable as text), so PR numbers are inferred from
the reflog's five fast-forwards of the target branch and from
`current-state.md`'s citation of "the PR #5 body for `fef0c97`". Commit
subjects are from `.git/logs/HEAD`.

| Merge | Commits | What landed, one line |
|---|---|---|
| PR #1 — `d8b3597` | before this branch | The GitOps cut-over (Argo CD, Kargo) — ADR 0020 names it; this branch was cut from it |
| PR #2 — `19241d8` | `4541923`..`8df1658`, 12 | Solver authority held to the LM rule; the two blueprint conflicts recorded; the traceability matrix; the §6.2 degradation type; ADR 0022; ADR 0023; manifest parser hardened then replaced with Cargo's own; the two credential windows bound; the seven-flow truth pass; the blueprint-vs-diagram reconciliation |
| PR #3 — `acfece3` | `9b8df9b`..`0c91cfa`, 9 | The twelve-item payload's wire shape; a cell that verifies, applies, narrows and halts on it; the reservation ledger, then wired into the kernel; the halt as a signed command; the centre shipping policy; the Deep Brain's reference universe and its own exchange; injective signing strings |
| PR #4 — `7f508cc` | `3be9855`..`db8ce8b`, 21 | One live market source behind the licensing gate, then selectable; `Intent` and netting in the cell, self-trade prevention; the `cloudrun`, `trust-zones` and `execution-node` modules, all unwired; the network module's blueprint notes; GitOps job identity; the console's order ticket deleted; the venue credential refused where the ceiling cannot use it; four unbounded collections bounded; money out of `f64` in risk and execution; the brute-force lockout made able to fire; the safest rung no longer reported live-capable |
| PR #5 — `baffcd8` | `64b765a`..`fef0c97`, 7 | `egress.rs` able to tell a deployed proxy from a described one; contributor attribution and the uplink schema bump; internal crossing at the mid with the forty-percent cap; the rounding remainder returned; the uplink proven; two scored documents corrected |
| Unmerged — this branch | `68b7da6`..`de5d042`, 13 plus in-flight | Edge telemetry parked, then finished and proven site by site; the node hands its cell the scraped registry; the observability rule corrected (C4); three Argo CD Applications that could never sync removed; five documents corrected; the reservation shortfall counted under its registered name; observed-fact constructors for agents; a cycle-leg type that cannot be nettable; the arbitrage scanner's leg emitter; a central reconciliation break and its scoped halt recorded and counted |

Velocity, for what it is worth: sixty-two commits in roughly thirteen hours of
reflog (`4541923` at 1788257119 to `de5d042` at 1788304174), of which roughly
a third are documents and corrections to documents. That
ratio is the programme working as designed — a scorecard that lags the tree
is the failure this repository has already named twice — and it is also the
reason §2's numbers are trustworthy enough to plan on.

---

## 4. The remaining work, as a sequenced backlog

One **slice** = one agent, one review, one PR. Estimates are in slices because
that is the unit this programme runs in; a slice that turns out to need two is
reported as two, not stretched.

Sequencing within (i) and (ii) is by dependency first, consequence second.
Every item names: the blueprint section; the finish line (A = alignment,
P*n* = blueprint phase *n*); dependencies; the blocker if any; the evidence
that closes it; the size.

### (i) Alignment work still open

| # | Item | Blueprint | Line | Depends on | Blocker | Evidence that closes it | Slices |
|---|---|---|---|---|---|---|---|
| A1 | **Refresh the matrix and the truth pass for what landed after `fef0c97`** — C4 closed, F7's consumer, F8's type, edge telemetry, `qip_central_` counters, `NumericFact::observed`; and add the missing OpenTofu/Cloud Build row as whatever status the owner assigns | ADR 0022 | A | The three in-flight agents finishing | None | `documentation` suite green; every changed row cites a test name; `no_scored_document_denies_the_existence_of_a_type_the_workspace_defines` passes | 1 |
| A2 | **Re-measure the full gate at HEAD** and correct `current-state.md`'s 3,308 | — | A | A1 | None here beyond a shell | `test result:` lines quoted for every binary under `--no-fail-fast`; clippy zero warnings; `all permitted`; `nothing found` | 1 |
| A3 | **Controls that cannot fire, remaining:** `Platform::learn_from` (belief calibration, no caller), `Platform::evaluate_alternatives` (called only by tests), `Web::record_cycle` (nothing calls it). Either wire a production caller or delete, per the gap-matrix rule | §47 (belief calibration "the single most important metric"), §12 | A | None | None | A production call site above `#[cfg(test)]` plus a test asserting the record; or the removal with the register recounted | 3, one each |
| A4 | **Assert the O(1)-in-strategy-count property of risk** | §2.2, rule 11 | A | None | None | A test at two strategy counts that fails when the aggregate is replaced by an iteration | 1 |
| A5 | **Settle `qip-arbitrage` and `qip-normalization`** — construct each in a composition root or record it research-only and drop the dead dependency edge | §30, §7.3 | A | Owner decision D6 | D6 | For construction: the binary's main constructing it and an acceptance test seeing it; for retirement: `Cargo.toml` edges removed, `architecture.rs` still green | 1 each |
| A6 | **Edge series get a collector and an alert; `qip_central_` descriptors get an alert** — permitted now that they emit | §47 | A | A1 | None (the observability rule's prohibition no longer bites) | The manifest selecting `qip-edge-node`; the policy naming a recorded descriptor; the test binding both halves to the same names extended | 1 |
| A7 | **Correct `.claude/rules/domains/data-and-streaming.md`**, which says a vendor call "needs the egress proxy in front" as though one existed | — | A | None | Owner — it is a rules file | The sentence stating the proxy is described and not deployed, with the manifest line | 1 (owner-approved) |

Alignment-done after (i): **A1–A4 and A6 are unblocked, five to seven slices;
A5 and A7 wait on the owner.** Layer 6 stays at 0/6 regardless — see §6.

### (ii) Phase 0–3 blueprint work

Phase 0 and Phase 1 are where the repository is genuinely behind, however far
ahead it is elsewhere. This is the critical path to the first gate.

| # | Item | Blueprint | Line | Depends on | Blocker | Evidence that closes it | Slices |
|---|---|---|---|---|---|---|---|
| B1 | **Decide and record the egress path** — ADR 0024 (co-located proxy) or the TLS-dependency reversal of ADR 0002, as the design note weighs them | §46.2 network, §45.1 | P1 (exit) and ADR 0020 steps 0/2/5 | — | **D1** | An accepted ADR; if (d), a dependency ADR and the ADR 0009 test relaxed for `qip-transport` only | 1 (the ADR) |
| B2 | **Switch the proxy on in the runtime that exists** — the four prerequisites at `egress.yaml:757-809`, a running `qip-egress` pod, and `egress.rs`'s described-vs-deployed test inverted to assert *on* | §46.2 | P1 (exit) | B1 | **D4**, plus a cluster (§6) | A vendor request in the Envoy access log through the allowlist; the test that today asserts "commented out" flipped and mutation-verified against re-commenting | 2 |
| B3 | **Name the market-data vendor host and record its licensing posture** | §7, rule 40 | P1 | — | **D9** | A `qip-data-finder` posture record and one listener; nothing else in the manifest changes | 1 |
| B4 | **Seven days of stable streaming with statistics converged and no raw stream retained** — the Phase 1 exit | §51 Phase 1, rule 32 | P1 (exit) | B2, B3 | A deployment (§6) | Seven days of a scrape series, the feature store's bound held, the licensing posture in the journal | 1 to observe; 0 to build |
| B5 | **Count trials cumulatively across runs**, per family, for deflated Sharpe | §20.1, rules 24–25; ADR 0023 "what could not be specified" | P2 | None — buildable now | None | `qip-lifecycle` refusing a promotion whose lifetime trial count is unknown; a test that a second run's correction includes the first run's trials | 1–2 |
| B6 | **Define the holdout band as an output of validation**, so step 9 of ADR 0023 has something to be inside of | §20.1, §51 Phase 3 gate | P2 | B5 | None | A band type produced by `validation.rs` and carried on the promotion record | 1 |
| B7 | **Attempt the Phase 2 gate on real data** | §51.1 | P2 (gate) | B4, B5, B6 | Phase 1 evidence (§6) | A family surviving holdout after cumulative correction, recorded in `qip-lifecycle/src/evidence.rs`'s own artefact — or a recorded failure, which the blueprint says is the more likely and the more useful result | 1 to run |
| B8 | **Passkeys** | §51 Phase 0, §40.3 | P0 | None | None known | An authenticator registration and assertion through Identity Platform; `grep -rn passkey backend/crates` non-empty; Playwright for the browser half | 2 |
| B9 | **PQC keys and real signatures for the payload channel** — depends on the crypto decision | §46.2 keys | P0 | — | **D2** | An ADR admitting a vetted crate, or an ADR declining and amending ADR 0002's reversal clause; then KMS-backed signing in place of `hmac_sha256` (`qip-core/src/hash.rs:163`) on the policy and envelope channels | 1 (ADR) + 2 |
| B10 | **Feasibility gate ahead of the profitability filter** | §18.1, rule 23 | P3 | — | Owner reading of ADR 0023 step 3 vs the Phase 2 gate (**D12**) | Passing-and-vetoing fixtures beside the pre-trade path in `qip-execution-engine`; minimum, tick, fee floor, gas, depth | 1–2 |
| B11 | **Join the edge contributor vector to central attribution** and settle a cross to the books (F7's two gaps) | §27.1, §43.4, rule 12 | P3 | A1 (to know how much of the join `ingest_cell_report` already does) | D12 | A netted order's fill attributed per contributor at the centre, `Attribution::residual == 0`; a `CrossedInternally` entry moving both contributors' positions | 2 |
| B12 | **Per-region reservation table** (F6) | §4.2, §26, §33, rule 21 | P3 | B11 | D12 | A disconnected cell refusing its own second proposal against one envelope; the central ledger unchanged; `apps/qip-edge-node/tests/mesh.rs` extended | 2 |
| B13 | **Set the internal-crossing interval** so a full cancellation can cross | §27.1 | P3 | — | **D3** | The cap evaluated per instrument per the chosen interval; a test where two strategies cancelling completely are both filled at the mid | 1 |
| B14 | **Twelve producers for the twelve payload slots** — ten ship unproduced today and narrow the cell | §41.5 | P3 for items 2, 9, 10, 11; later phases for 1, 3–6, 8, 12 | A1 | Most slots have no producing plane yet (belief, episodic, causal digest, self-model are P7–P9) | Per slot: a producer, the cell consuming it, and the §6.2 row it un-narrows | 1 per slot; 4 in P3 |
| B15 | **Second, independent halt wire** — a polled flag beside the broadcast | §46.2 kill switches | P3 | None | None in code; a managed store in deployment (§6) | Flow 6 re-traced with two wires that do not share `qip-transport`'s failure | 1–2 |
| B16 | **ADR 0020 step 1 — establish which GKE workloads have ever run** | §41.4 (as the precondition) | P3 (first C3 node) | — | **D5** and a cluster (§6) | A named cluster, a pod list, a scrape — brought to the owner, not acted on | 1 to gather |
| B17 | **Wire and validate the three unwired modules** (`cloudrun`, `execution-node`, `trust-zones`) | §41.4, §45.1, §46.1 | P3 (node), P16 (regions) | B1, B16 | **D5** per step; a `terraform` binary (§6) | `terraform validate` output; a plan that refuses a bad value and admits a good one; nothing applied without step-named approval | 1 each to validate; apply is not estimated because it is not authorised |

Phase 0–3 total, excluding what is not authorised: **roughly 25 slices, of
which about 8 are unblocked today** (B5, B6, B8, B10–B12 pending D12, B15) and
the rest wait on an owner decision or an environment this session does not
have.

### (iii) Later phases, gated behind Phase 2 and Phase 3

Not sequenced item by item, because estimating a plane that does not exist is
invention. What exists ahead of phase is labelled so it is not read as the
phase being reached.

| Phase | Deliverable | What exists today | Status | Gate above it |
|---|---|---|---|---|
| 4 Counterfactual scoring | Every declined path scored daily | `Platform::evaluate_alternatives`, `qip-twin` — no production caller (A3) | PLANNED, machinery present | Phase 2 gate |
| 5 Ingestion and world model | Entities above confidence; events linked | World model, causal graph, entity resolution all TESTED (flow 2); no deep-web tier; no source discovery | Ahead of phase in part | Phase 2 gate |
| 6 Prediction markets | Brier beating implied — **gate** | `qip-prediction` four modules; no venue, no Brier comparison | PLANNED | Phase 3 gate |
| 7 Episodic and belief | Calibration within tolerance; sizing responds | `qip-agents/src/memory.rs`; `bayes.rs`; no belief stage in the cycle; `learn_from` uncalled | PLANNED; payload slots 3, 4 empty | Phase 3 gate |
| 8 Causal inference | Regime-conditional beating unconditional — **gate** | Causal graph real (`world.rs:41`); regime detection; no out-of-sample comparison | PLANNED; slot 5 empty | Phase 3 gate |
| 9 Self-model and exploration | Value of information measured | Nothing (`grep -rln SelfModel` empty) | MISSING, deliberately | Phase 8 gate |
| 10 Multi-strategy | 500+ strategies; netting ratio above 1.5; attribution exact | Netting at the cell; `qip_edge_netting_ratio` histogram; attribution exact centrally | Ahead of phase; ratio never measured under contention | Phase 3 gate |
| 11 Arbitrage and market making | Path 2 above 93 percent; quoting net positive | `qip-arbitrage` reachable, unconstructed (D6); leg emitter at `6053935`; `qip-orderbook` | Ahead of phase in code, unwired | Phase 3 gate |
| 12 Wallet and treasury | Every holding reconciled; zero unauthorised attempts | Nothing beyond internal placement; refused by ADR 0021; ADR 0023 step 10 | MISSING, bounded | Separate owner decision |
| 13 Web and mobile | Every operational question answerable; kill switch from mobile | Next.js portal and PWA, transitional (C3); Leptos not begun | CONTRADICTS §2.1 | Direction settled, execution unauthorised |
| 14 Valuation plane | Fixed income and options live | Nothing | MISSING, deliberately | Phase 2 gate |
| 15 Optimisation at cadence | Solver delta measured | Routing gate, classical baseline, QAOA adapter | Ahead of phase; no delta measured on real capital | Phase 3 gate |
| 16 Multi-region | Three regions, mirrors live | Three cells in stage tfvars as pods; `execution-node` module unwired; ADR 0020 steps 3–5 | Ahead of phase in tfvars, behind in topology | Phase 3 gate, D5 |
| 17 Illiquid and private | Positions marked with method | Nothing | MISSING | Phase 14 |
| 18 Adversarial and simulation | Simulator calibrated to fills | `qip-simulation-engine` resampling; no adaptive agents | PLANNED | Phase 3 gate |
| 19 Market creation | Per class, on evidence | Nothing | MISSING, and the blueprint says last | Phases 7, 8, 14 |

Order-of-magnitude size: on the (ii) rate of one to two slices per capability
and roughly a hundred named capabilities across the sixteen later phases,
**well over a hundred slices** — and every gate in the column is a place the
blueprint says to stop and possibly not continue.

---

## 5. Owner decisions outstanding

Each verified as still open at `de5d042`. Where a decision has quietly been
taken by a commit, that is said.

| # | Decision | What it blocks | Default if undecided | Verified how |
|---|---|---|---|---|
| D1 | **The egress path** — (a) central Cloud Run proxy, (b) co-located sidecar/systemd unit (the design note's recommendation, needing ADR 0024), (c) managed PSC/SWP (rejected as a substitute, adopted as a complement), (d) a TLS crate in `qip-transport` under ADR 0009's tier, which ADR 0002 names as its own reversal condition | B1, B2, B4, the Phase 1 exit, the Phase 2 gate, ADR 0020 steps 0, 2 and 5 | Nothing outbound runs; every deployed adapter stays inert; `egress.rs:1156` keeps asserting the proxy deploys nothing | No ADR 0024 exists under `docs/adr/`; `qip-transport/src/http.rs:366-367` still refuses `https` |
| D2 | **In-tree HMAC vs ADR 0009** (F3) — admit a vetted crate by ADR, or decline and amend ADR 0002's reversal clause | B9; §46.2's real signatures and PQC; every further use of `hmac_sha256` | The primitive stays; each new caller restates F3 in its diff | `qip-core/src/hash.rs:151,163` carry `sha256` and `hmac_sha256`; no crypto ADR after 0023 |
| D3 | **The internal-crossing cap interval** (§27.1 "per instrument per interval") | B13 — a full cancellation can never cross today (F7) | The cap refuses every full cancellation; safe, and less than the blueprint asks | `grep -i interval` in `qip-contracts/src/intent.rs` returns nothing; `cell.rs:1000-1006` documents the refusal |
| D4 | **Switch the GKE egress proxy on** — a third-party image, a Binary Authorization exemption or a mirrored-and-attested digest, two acceptance exemptions, a `deploy.yml` entry, a service-account map entry | B2, B4 | Off; `qip-egress` Service has no endpoints; four NetworkPolicies select nothing | `egress.yaml:820` and `:835` are `# kind: ServiceAccount` / `# kind: Deployment` in both copies |
| D5 | **ADR 0020 steps 1–5, each by name** — gather step 1's evidence, run one warm service on both, stand up a shadow C3 node, cut over, retire the chart | B16, B17, Phase 16, Layer 6 leaving 0/6 | Nothing migrates; both topologies documented; `main.tf` keeps seventeen module blocks and none of the three new ones | ADR 0020 "What was open, and what still is": "every single step of it" |
| D6 | **`qip-arbitrage` and `qip-normalization`** — construct, or record research-only and drop the edge | A5; Phase 11 (arbitrage); Phase 1 normalisation in the runtime path | Both stay UNVERIFIED at the matrix's ceiling, compiled into binaries that never call them | `qip-kernel/Cargo.toml:39` declares `qip-normalization`; `grep qip_normalization qip-kernel/src` is empty. `qip-edge/Cargo.toml:23` declares `qip-arbitrage`; the only use is `seam.rs:14`'s `LiquiditySource` import |
| D7 | **K3 — what the application zone may reach**: the DOCX's "raise intents only, never a node, venue, QPU or key" or the diagram's wider "reaches Intelligence" | The semantics of `trust-zones` when it is wired (B17); the typed-intent API surface (§40.9) | The narrower reading, which is what is built | `blueprint-diagram-reconciliation.md` K3 unchanged; `qip-api` still composes reads only |
| D8 | ~~C4 — correct the observability rule file~~ | — | — | **Taken.** `232bc16` "Stop telling every agent the edge plane cannot emit" corrected `.claude/rules/domains/observability.md`; the reflog does not record who approved a rules-file edit, and that should be confirmed. What remains is A1 (the matrix row) |
| D9 | **The market-data and chain-RPC hostnames and their licensing posture** | B3, B4 | No listener; the adapters stay inert; the blueprint's Phase 1 cannot start | `egress.yaml:394-499` declares five clusters, none a vendor; the design note §1.6 |
| D10 | **ADR 0023 step 3 versus the Phase 2 gate** — the record says step 3 is "buildable today" and also lists "execution infrastructure built before the Phase 2 gate passes" under what would make it wrong; blueprint §51.1 says stop | B10, B11, B12, B14's Phase 3 slots, B15 — a quarter of (ii) | Ambiguous; the safe reading is the blueprint's, which idles the most-ready work in the backlog | ADR 0023 lines 82–84 against 194–198; unchanged since `e36dbc1` |
| D11 | **Whether the matrix gains rows for §48 / rule 77** (OpenTofu, Cloud Build, Cloud Deploy, third-party source control) and what status they carry | A1's completeness | Unscored; a reader of §48 finds no row and assumes either aligned or ignored | No such row in the matrix's constraint or layer sections |
| D12 | **A2's shell** — someone with a shell must run the full gate, since this plan's author could not | A2; every "not re-measured" cell above | Numbers stay stale by exactly the commits since `fef0c97` | §7 of this document |

---

## 6. Environmental blockers — what can and cannot be proven from here

**No `terraform` binary.** The `cloudrun`, `execution-node` and `trust-zones`
modules (commits `8c73610`, `6cde5d7`, `b6cca79`) and the network module's
change (`2402d2e`) have never been run through `terraform fmt -check`,
`terraform validate` or a plan. What that means for confidence, stated
precisely: the HCL has been read by `infrastructure.rs`, which is a text
scanner, and by people; it has not been read by the `hashicorp/google ~> 6.12`
provider's schema. A misspelt attribute, a wrong block type, or a variable
validation that never compiles would pass every check that has run. The
matrix's Layer 6 row already says "NOT RUN — terraform is not installed"; this
plan scores the three modules IMPLEMENTED-UNVERIFIED for that reason and
nothing about them may be promoted until a validate has been quoted.

**No `helm` binary.** The chart under `infrastructure/helm/qip/` has never
been rendered in this environment; the Argo CD Application removal (`9b51be2`)
and the egress template's state are known from reading YAML, not from
`helm template` output. Same confidence class as above.

**No cluster reachable.** ADR 0020 step 1's evidence (a named cluster, a pod
list, a scrape) cannot be gathered. Consequently: `workload_metrics_exist`
cannot be flipped, the Secret Manager CSI chain stays never-exercised-live,
`infra.yml down` stays never-run, and the proxy cannot be switched on even if
D4 were decided today.

**No live-data deployment.** The Phase 1 exit (seven days streaming) and the
Phase 2 gate are impossible to attempt from here, whatever code lands. One
real tick was fetched in-session (`gap-matrix.md` item 6) through a bridge that
is not the platform's egress path; that is the SENSE half of one cycle and
nothing more.

**No shell in the session that wrote this document.** The documentation gate
for this file was not run by its author (§7). The three in-flight agents have
shells; whoever stages this file must run the gate and quote it.

What *can* be proven from here: everything the Rust workspace asserts about
itself — the paper layers, the authority boundaries, the flow links marked
TESTED, the manifest-to-binary correspondence, the egress manifest's
commented-out state — because those are tests over source and text, and they
run without a cloud. That is the whole of the evidence base above §2.3's
Layer 6, and it is why Layer 6 is the one row at zero.

---

## 7. How far away are we — the honest paragraph, twice

**Alignment-done.** Close, and mostly waiting on decisions rather than on
work. Of the seven open alignment items, five are one-slice jobs an agent can
start today — refreshing the two scorecards for the thirteen commits they have
not seen, re-running the full gate and quoting it, wiring or deleting three
controls that exist but nothing calls, one missing test, and a collector for
metrics that now emit. The other two need the owner to say what becomes of two
crates that are compiled and never constructed, and to approve one sentence in
a rules file. The boundaries are enforced structurally and re-verified this
session; the paper-trading line has three layers and a test on each. What
alignment-done will *not* mean is that the cloud layer is proven: the three
Terraform modules written this session have never been seen by a Terraform
binary, no pod has ever been shown to run, and no scrape has ever been
observed. Call it seven slices from aligned, with one layer that cannot be
scored above zero from this environment.

**Blueprint-done.** Far, by the blueprint's own reckoning, and the distance is
not mainly code. The tree holds capability from Phase 1 to roughly Phase 15 —
netting, a cost router, a quantum adapter with its classical baseline, three
regional cells — and has passed none of the four gates, because every gate is
a question about real data or a real venue and the platform has never streamed
real data for a day. The first thing between here and the first gate is an
egress path that has never been switched on, which is one owner decision and
two slices; after that, seven days of streaming nobody can run from this
environment; after that, the Phase 2 gate, which the blueprint calls the most
important sentence in the document and expects to fail more often than pass.
Phases 0 to 3 are roughly twenty-five slices, a third of them blocked on five
owner decisions. Phases 4 to 19 are well over a hundred more, behind gates
that may say stop. And the two direction decisions already taken — no
Kubernetes, Leptos — have every step still unauthorised. The honest unit is
not weeks; it is gates, and zero of four have passed.

---

## Verification of this document

The gate for this file is
`cd backend && cargo test -p qip-acceptance --test documentation --no-fail-fast`,
which checks every internal link resolves and refuses the overclaims it names.
Run it on every edit and quote the `test result:` line in the commit. This
document does not claim the gate ran; the commit that lands it must.
