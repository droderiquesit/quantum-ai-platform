# Integration and truth pass — seven flows traced through the code

Phase 4 of the Algorik architecture programme. Every claim below was derived by
reading the path named in it. **Where a link could not be traced it is recorded
as untraceable rather than assumed**, and where a flow breaks the seam is named.

Scored against the blueprint (ADR 0022) on branch
`claude/algorik-architecture-refactor-pmp0zy`.

**Status vocabulary.** MEASURED (runtime evidence exists) · TESTED (a named
passing test) · CONFIGURED (wired in a manifest or tfvars) ·
IMPLEMENTED-UNVERIFIED (code exists, no deployable composes it) · PLANNED
(backlogged with a phase) · MISSING.

---

## Flow 1 — public signup/passkey → mandate and entitlements → empty account

**Verdict: BREAKS at the first seam after authentication. The platform has no
concept of a customer account.**

| Link | Status | Evidence |
|---|---|---|
| Sign-up page | TESTED | `frontend/portal/src/app/(auth)/sign-up/page.tsx`; siblings for sign-in, reset, agreements, account-locked |
| Browser → identity | IMPLEMENTED-UNVERIFIED | `(auth)/_lib/api.ts` posts to Next.js `/api/auth/*` with CSRF priming. ADR 0019 makes Identity Platform the only identity store |
| Passkey | MISSING | `grep -rn passkey backend/crates` returns nothing. Blueprint Phase 0 names passkeys; nothing implements them |
| → mandate | MISSING | No customer mandate type. `Mandate` in `runtime/qip-kernel/src/config.rs` and `services/qip-portfolio-engine/src/construction.rs` is a *portfolio* mandate — risk limits for the desk's own book, not a customer agreement |
| → entitlements | MISSING (name collision) | `qip_contracts::governance::Entitlement` (`governance.rs:83-94`) is `Granted{dataset, usage, expires_at}` / `Denied{...}` — **dataset licensing**, not a customer's product rights. Blueprint §40.13 means the latter. Same word, different concept; do not conflate |
| → empty account | MISSING | No account creation anywhere. `qip-api` exposes 30 endpoints (`routes.rs:73-299`) and not one is per-user: `/portfolio`, `/orders`, `/capital`, `/risk` are all desk-wide singletons |

**The seam.** Authentication terminates at Identity Platform and nothing carries
a subject into the Rust platform as an account holder. `qip-api`'s `auth.rs`
resolves a `Principal` with a `Role` (`auth.rs:48-81`) — operator RBAC, not
customer identity.

This is consistent with `CLAUDE.md` ("intended users are the research and risk
desk — not external customers") and **contradicts the blueprint**, which
specifies an investor portal over a per-user ledger. Now that the blueprint is
the architecture of record, this is a real gap rather than a scoping choice.
Blueprint Phase 13. Backlogged; nothing built here.

---

## Flow 2 — the spine

filing/source → facts → entity resolution → world event → belief → strategy
intent → regional execution → fill → ledger → explanation

**Verdict: traceable end to end with two substitutions and one genuine break.**
The loop runs and closes; it is not the blueprint's loop at two points.

| Link | Status | Evidence |
|---|---|---|
| source → facts | TESTED | `platform.rs:1380 observe()`; news at `:1129-1138` → `WorldModel::absorb_news` + `MarketEvent::from_news`; fundamentals `:1140-1153`; macro `:1154-1158`; corporate actions `:1159-1180`; alt `:1181-1203`; reference `:1204-1243`. `absorption.rs` covers most arms |
| live source | PARTIAL — exercised once, not yet sustained | Two paths now: the vendor path (`feed.rs` `Live` arm, needs a keyed aggregator) and the connector path (`Feed::Connector` → `connector_feed.rs`), the latter **exercised against the real Coinbase endpoint in-session** through the full runtime with the licensing catalogue evaluated first. Sustained deployment streaming still unobserved |
| → entity resolution | TESTED | `stage_understand` (`platform.rs:2009`), `qip-entity-resolution` |
| → world event | TESTED | `qip-world-model/src/world.rs`; causal graph is real — `world.rs:41 causal: CausalGraph`, `:192 claim_causal`, `:495` shock propagation, `causal.rs:234` |
| → **belief** | **SUBSTITUTED** | **No belief stage exists.** `grep -n belief runtime/qip-kernel/src/platform.rs` returns one doc-comment line and no code. What runs is `stage_reason` (`platform.rs:2387`) producing *theses*, with Bayesian machinery in `qip-reasoning-engine/src/bayes.rs` (`BeliefUpdate`, `EvidenceStrength`, `attenuate`). Confidence-weighted sizing per blueprint §11.2 is not the mechanism here |
| → **strategy intent** | **SPLIT — built at the edge, absent at the centre** | `Intent` exists (`libs/qip-contracts/src/intent.rs`), and with it netting, internal crossing (§27.1) and the contributor vector. `Cell::work` builds one intent per firing strategy, nets them on instrument/venue/representation, crosses the offsetting part at the book mid and places what survives. **This row said all four of those were absent; that was true when it was written and false from the netting slice onward.** What has not changed is the *central* path: `stage_decide` (`platform.rs:2968`) still sizes theses into `Proposal`s with legs and raises no intent, so §27's mechanism operates in one of the two planes. See the seam below, which is the same seam |
| → **regional execution** | **BREAKS** | `stage_act` (`platform.rs:3048`) runs the central risk monitor and places through the **central** OMS. It does not reach a cell. See the seam below |
| → fill | TESTED | Orders reach the simulated broker via `qip-execution-engine/src/oms.rs`; multi-leg in `multileg.rs` |
| → ledger | PARTIAL / naming | **There is no `Ledger` type.** `grep "pub struct.*Ledger"` finds only `PrivacyLedger` and `BudgetLedger` — neither is money state. Money state is `qip-capital` (allocation, envelope, exposure) plus the hash-chained event log. Blueprint §43.3's per-user, per-strategy authoritative ledger does not exist; per-strategy attribution does |
| → explanation | PARTIAL | Attribution is exact and reconciles (`qip-learning-engine/src/attribution.rs:152 reconciles()`, `:199 by_hypothesis()`), and the cost router records a `rationale` (`platform.rs:564,613`). Blueprint §40.2 explanation — what the system believed and why — has no surface |

**The seam, precisely.** Central and regional execution are two disjoint paths,
not one flow:

- The central plane decides and executes through its own OMS (`stage_act`).
- A cell executes its own deployed strategies under a granted envelope
  (`edge/qip-edge/src/cell.rs work()`).
- The only thing that travels centre → cell is a **signed capital envelope**.
  `CapitalDownlink::absorb` (`edge/qip-edge/src/mesh.rs`) rejects every frame
  whose `frame.topic != CapitalGrantTopic::TOPIC`.

So a decision made centrally is never executed regionally. Policy travelling
down as blueprint §41.5 requires is flow 3, and flow 3 is missing.

---

## Flow 3 — policy generation → signed twelve-item payload → regional verification and atomic swap → outcome return

**Verdict at the original trace: MISSING, with one genuine precursor. Since
closed to PARTIAL by the payload slice** — `qip_contracts::policy` carries
typed slots for all twelve items, the centre builds, signs and ships one per
cell each cycle (`qip-api/src/mesh.rs::pending_policy`, `::dispatch_policy`),
the cell verifies into `VerifiedPolicy` and applies by one-assignment swap
with sequence discipline (`qip-edge/src/cell.rs::apply_policy`), and §6.2
narrowing runs through `DegradationState` at last. Two of twelve slots have
real producers (the grant manifest and the risk envelope as the enforced
`LimitSet`); the other ten ship unproduced and read as unavailable, which
narrows the cell — fail-closed, not omission. End-to-end over real sockets:
`qip-api/tests/mesh.rs::a_cycle_ships_a_signed_payload_the_cell_verifies_and_a_trip_reaches_it`.
The table below records the state as originally found.

| Link | Status | Evidence |
|---|---|---|
| Twelve-item payload | MISSING | No type carries the twelve items of §41.5. Nothing to grep for; the concept is absent |
| Signed policy down | **PARTIAL — one item of twelve** | Item 7, "family budgets and capital grants", exists and is genuinely signed: `qip_contracts::capital::CapitalEnvelope::signing_payload` (`capital.rs:128`), `signature()` (`:108`), verified cell-side into `VerifiedEnvelope` (`mesh.rs`) |
| Regional verification | TESTED (for that one item) | `CapitalDownlink::poll`/`absorb` (`mesh.rs:664+`) verifies against the cell's own key, refuses and de-duplicates by grant key |
| **Atomic swap** | MISSING | Envelopes are applied incrementally per grant. No pointer swap, no all-or-nothing application of a payload |
| **Stale-item narrowing (§6.2)** | PLANNED | `qip_contracts::degradation` types the narrowing (added this programme) but **has no production caller** and no payload to attach to |
| Outcome return | TESTED | `CellUplink::publish` (`mesh.rs:418`) sends `CellStateDelta` up — utilisation, halt flag, refusals, orders |

**Backlogged, not built.** Blueprint Phase 16 for the full payload. The capital
envelope is the pattern to generalise: it already proves the sign-verify-refuse
path works.

---

## Flow 4 — investment request → capital engine → grant → regional policy

**Verdict: the most complete flow in the platform.** Traceable end to end.

| Link | Status | Evidence |
|---|---|---|
| Request | TESTED | `qip_compliance::approval::CapitalRequest` with two-signature `ApprovalChain` (`approval.rs`), fresh-credential requirement (`:37`, `:402`) |
| Capital engine | TESTED | `qip-capital/src/allocation.rs` — `StrategyProposal`, `AllocationLimits` with per-cell (`:128`) and per-venue (`:134`) caps, `DrawdownSchedule` (`:169`) |
| Grant | TESTED | `CapitalEnvelope` / `CapitalGrant` in `qip-contracts/src/capital.rs`, signed |
| → regional policy | TESTED | `CapitalDownlink` verifies, refuses unsigned, de-duplicates; `RefusedGrant` records why |
| Cell cannot self-grant | TESTED | No edge crate reaches `qip-capital` or `qip-lifecycle` (`architecture.rs::no_edge_cell_can_issue_its_own_capital_or_promote_its_own_strategy`) |

**Known gap, pre-existing:** capital reservation. A proposal that passes a
capital check does not hold the capital, so two concurrent proposals can pass
against the same free balance (`docs/plan/gap-matrix.md` item 10).

---

## Flow 5 — deposit/withdrawal/internal move → expected inflow or transfer intent → corridor → transfer gate → custody → reconciliation

**Verdict: MISSING almost entirely. Correctly so — Phase 12, and bounded by ADR 0021.**

| Link | Status | Evidence |
|---|---|---|
| Deposit / withdrawal | MISSING | `qip-portfolio/src/portfolio.rs:131` has "deposit or withdraw cash" — a simulated book operation, not a money movement |
| Transfer intent | PARTIAL | `qip-capital-fabric/src/transfer.rs`, `settlement.rs`, `plan.rs`, `location.rs` — internal capital placement, no external boundary |
| Corridor | MISSING | `grep -rn corridor --include=*.rs` returns **nothing** |
| Transfer gate | MISSING | `grep -rn TransferGate` returns **nothing** |
| Custody | MISSING | Only `VenueClass::Custodian` (`venue.rs:53-54`) as a venue kind |
| Destination registry | MISSING | Nothing |
| Reconciliation | PARTIAL, and real where it exists | The cell reconciles fills against the venue drop-copy and **self-halts on disagreement** (`cell.rs:774-786`) |

**Deliberately not built.** ADR 0021 permits the deterministic half and refuses
signing and withdrawal; ADR 0023 sequences this as step 10, separate from and
later than order submission. `security.rs::no_signing_or_withdrawal_path_exists_for_capital_to_leave_the_platform`
enforces the refusal.

---

## Flow 6 — halt/kill switch through both independent paths

**Verdict at the original trace: the halt worked where checked, there were
not two independent paths, and a central halt could not stop a regional cell.
Since partially closed:** `POST /api/v1/kill-switch` now also broadcasts a
signed engage-only `HaltCommand` to every cell inbox, a cell that hears it
halts, and release requires a strictly newer signed payload issued after the
halt's barrier — stopping easy, resuming a fresh decision. What remains open,
honestly: both paths share `qip-transport`, so this is mechanism independence
rather than the blueprint's two independent *wires*; the managed-store second
wire stays backlogged. ADR 0008 is intact — an unreachable cell keeps trading
its envelope, and the guarantee added is only that a reachable one obeys. The
sections below record the state as originally found.

### What works

| Property | Status | Evidence |
|---|---|---|
| Central halt enforced pre-trade | TESTED | `qip-execution-engine/src/oms.rs:248` — `if autonomy.kill_switch().is_halted(&order.scope)` refuses |
| Autonomy cannot be raised while halted | TESTED | `autonomy.rs:587` |
| Cell halt enforced | TESTED | `cell.rs:364-372` — `work()` refuses with `"kill_switch"` and keeps absorbing books, so a halted cell can still tell why |
| Cell self-halts on reconciliation break | TESTED | `cell.rs:774-786` — fills disagreeing with the venue's own account trip the cell's switch and journal a `HaltChanged` |
| Tripping needs no authority, clearing does | TESTED | `autonomy.rs:318-347` vs `:429`; mirrored in `qip-compliance/src/incident.rs` |
| The two credential windows agree | **TESTED — closed this pass** | `compliance_proof.rs::the_two_credential_windows_that_claim_to_be_the_same_window_agree_on_the_same_credential` |

### What does not work

**There is one mechanism, reached by two user interfaces — not two independent
paths.** Blueprint §46.2 requires "two independent paths — Spanner flag polled
and Pub/Sub broadcast. Either halts trading, quoting and transfers."

- `POST /api/v1/kill-switch` → `platform.autonomy_mut().kill_switch_mut().trip_global(...)` (`routes.rs:635-647`)
- Console `POST` → `platform.autonomy_mut().kill_switch_mut().trip_global(...)` (`console.rs:123`)

Both mutate the **same in-process object**. A process that is wedged, a
partition, or a lost API is one failure that takes both. Independence is the
entire point of the control and it is absent.

**And a central halt cannot reach a cell.** The cell's `is_halted()`
(`cell.rs:199-200`) reads its *own* `AutonomyController::new()` switch. The
downlink accepts only `CapitalGrantTopic` frames, so no halt command can arrive.
The mesh delta carries `halted` **upward** (`mesh.rs:173`, `delta.rs:100`) —
the cell reporting its own state, not receiving one.

Consequence, stated plainly: **an operator tripping the kill switch halts the
central plane and leaves every regional cell trading.** Each cell can only be
halted by its own local trip or by its own reconciliation self-trip.

**Why this is backlogged rather than fixed here.** The downward halt is a policy
item travelling centre → cell, which is exactly flow 3's twelve-item payload —
§41.5 item 9 is the risk envelope and §6.2 defines stale-item narrowing.
Building a bespoke halt topic now would be a second policy channel beside the
one the blueprint specifies, which is the process proliferation the programme's
own rules forbid. It belongs with the payload, and it is the strongest argument
for doing that work.

**Mitigating, and not a substitute:** a cell is bounded by a signed envelope
with an expiry, so an unhaltable cell can only spend what was already approved,
for as long as it has left (ADR 0008).

---

## Flow 7 — failure and degradation paths

| Dependency lost | Blueprint expects | Repository | Status |
|---|---|---|---|
| Central plane | Region narrows, never halts on that alone | `CellUplink`/`CapitalDownlink` circuit breakers (`mesh.rs:265,516`); cell keeps trading its envelope | TESTED — `apps/qip-edge-node/tests/mesh.rs:376` |
| Source / ingestion | World model ages; price-only strategies continue | §6.2 row typed in `qip_contracts::degradation`, **no caller**. What exists is mechanism-level: a stale book supplies nothing (`edge/qip-edge/src/seam.rs:53-61,108-111`) | PLANNED |
| Belief | Fixed conservative multiplier; nothing halts | Typed, no caller. No belief state to go stale | PLANNED |
| Ledger | — | No ledger type; event log is append-only and hash-chained | PARTIAL |
| Node / cell | Failure isolation per region | Venue health and degradation in `edge/qip-routing/src/health.rs`; `resilience.rs`, `chaos.rs` | TESTED |
| Venue | Refuse, do not guess | `qip-brokers/src/connection.rs` degradation; `oms.rs` refusals recorded | TESTED |
| IBM / quantum | Classical baseline always | ADR 0006; `qip-optimization-engine/src/router.rs` computes the baseline every time, so a QPU outage narrows nothing | TESTED |

**The pattern.** Degradation is implemented thoroughly at the *mechanism* level
— one book, one venue, one peer — and not at all at the *capability* level that
§6.2 defines. The type exists; its consumer does not.

---

## What this pass changed

One defect found and fixed: two credential windows that each documented
agreement with the other, in crates that cannot see each other. Bound by a
behavioural test, two mutations fired.

Everything else found is either a missing capability whose phase has not
arrived (flows 1, 3, 5) or a structural gap that belongs with the twelve-item
payload (flow 6's downward halt, flow 7's capability degradation). None of it
was built, per the no-speculative-scaffolding rule.

**The single highest-value next slice remains the twelve-item payload.** Flow 3
is missing, flow 2 breaks at the centre→region seam for want of it, flow 6's
independence gap needs it, and flow 7's §6.2 narrowing is already typed and
waiting for it. Four of the seven flows converge on one piece of work.
