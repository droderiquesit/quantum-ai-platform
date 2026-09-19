# Algorik delivery status

**The single status document for this repository.** It scores the Algorik
Master Blueprint v10.1 — all 181 numbered sections of
`docs/architecture/algorik-blueprint-v10.1-source.md` — against the code, one
row per section.

It replaced nineteen documents on 2026-09-07. Those nineteen disagreed with
each other and, in nine measured cases, with themselves. One register cell claimed a search
for the self-model found nothing, while that same search named eight files;
another made the same claim about the transfer gate against ten. (Both
sentences are described rather than quoted here, because quoting them verbatim
would trip the gate that now runs the commands a status document cites — which
is the gate working, and the reason this paragraph reads as it does.) At
one point two registers agreed on a total **only because two errors cancelled
in opposite directions**, so a reader who checked the arithmetic found both
correct and concluded they agreed about the platform. They did not.

**If you are about to add a second status document, do not.** That is the
failure this one exists to end. Add rows here, or change verdicts here.

---

## The one fact that governs every row

**Nothing is deployed.** No process of this platform has ever been observed
running.

```
grep -h execution_nodes infrastructure/environments/*/terraform.tfvars   # {} in all four
awk '/variable "workload_metrics_exist"/,/^}/' infrastructure/terraform/variables.tf | grep default   # false
```

No Cloud Run service has a metrics collector — the only sidecar Google
publishes fails this platform's own Trivy gate on an unfixed CRITICAL, which
is upstream and unfixable here. So every verdict below means *built, wired to
a caller, and tested*. **No verdict anywhere in this document means running.**

Paper trading is absolute and structurally held by three independent layers —
Terraform's refusal of the three live ceilings, `AutonomyLevel::deployable` in
all three composition roots, and `qip-edge`'s `Cell` having no constructor
that takes a non-paper ceiling. All three are intact.

---

## Measured facts

Recounted by the suite, not typed here. `the_delivery_status_counts_the_crates_that_are_actually_there`
fails when this drifts.

| Fact | Value |
|---|---|
| Crates | 58 |
| Blueprint sections scored | 181 |

---

## The bar, stated once

Every row uses exactly one of these. Mixed bars are why the previous registers
became unusable — "closed" meant *has a production caller* in one row and
*exists at all* in another, and the totals silently summed different things.

| Verdict | Means |
|---|---|
| `REACHED` | Built, tested, and a **non-test call path exists**. The evidence names the path. |
| `UNREACHED` | Built and tested; every caller is a test. A real and common state here. Not delivered. |
| `PARTIAL` | Some of the section's stated requirements met. The evidence says which. |
| `ABSENT` | Not built. |
| `BLOCKED` | Cannot be built in this repository. The evidence names the blocker. |
| `NARRATIVE` | The section states philosophy, history or rationale and asks for nothing testable. |

`REACHED` deliberately does **not** require deployment — there is none, so a
bar that required it would score every row identically and measure nothing.
One consequence, resolved here so it is not re-litigated: `Cell::work` is
`REACHED` because `run_pass` is called from `qip-edge-node/src/main.rs`, a
production binary, even though that arm runs only under
`QIP_VENUE_FEED=simulated` and `execution_nodes = {}` means nobody has started it.

**Evidence is a runnable command, never a line number.** Citations in this
repository have rotted inside a single working session while a parallel lane
moved the code. If a command returns nothing, that absence is the evidence.

---

## The shape

| Verdict | Sections | Share |
|---|---|---|
| `REACHED` | 35 | 19% |
| `PARTIAL` | 130 | 72% |
| `UNREACHED` | 1 | 1% |
| `ABSENT` | 0 | 0% |
| `NARRATIVE` | 15 | 8% |
| **Total** | **181** | |

**Recounted 2026-09-15 for §11.3's `PARTIAL → REACHED`** — one row, 27/138
to 28/137, taken with the same command the 2026-09-14 paragraph below gives
and its siblings, and required to sum to 181 before being written here. It
did: 28 + 137 + 1 + 0 + 15. The rounded shares do not move, which is the
reason to recount from the rows rather than from the shares.

**Recounted 2026-09-14 from the rows. No section is `ABSENT` any longer**, and
that sentence is worth less than it sounds. `PARTIAL` is now 76% of the
blueprint and spans everything from a row missing one field to a row whose
model is complete and whose ingestion does not exist. **Do not read this table
as progress without reading the rows.**

The seven that moved — §15.2, §17.4, §25.6, §31.1, §33.1, §34.3, §34.4 — each
say in their own cell what holds them short of `REACHED`, and in four cases the
answer is not code: nothing feeds the venue ladder, no deployed cell holds a
foreign venue, no chain client exists, and §15.2's remaining rows need a
counterparty a paper-trading platform structurally does not have. Two of those
four will not be closed by building anything.

This table read 7 `ABSENT` for several hours after the code closing those rows
had merged and gated, because the wave was integrated without the ledger being
re-scored in the same pass. The owner found it by asking for status, which is
the wrong way to find it. It is recorded here rather than quietly corrected: a
status document that lags its own repository is the failure this document was
created to end, having replaced nineteen conflicting status files, two of which
agreed on a total only because two errors cancelled.

**Recounted 2026-09-14 from the rows, not adjusted by arithmetic against the
previous figures.** Seven `ABSENT` rows moved: §23.3, §23.6, §29.1 and §29.3 to
`REACHED`, and §19.2, §18.4 and §30.2 to `PARTIAL` — each of the three for a
reason stated in its own cell rather than folded into this table. The count
was taken with
`grep -cE '^\| [0-9]+\.[0-9A-Za-z.]+ \| [^|]+\| REACHED \|' docs/DELIVERY-STATUS.md`
and its siblings, and the five figures were required to sum to 181 before being
written here. The first attempt at this recount returned 175 and was discarded
rather than reported: its pattern missed the three-part section numbers. This
document records three earlier recounts that were wrong in the optimistic
direction, which is why a total that does not reconcile is treated as a broken
instrument and not as a result.

Recounted 2026-09-12 for both changes landing in this working tree together:
§12.3's `ABSENT → PARTIAL` (ADR 0055) and §7.6.1's `ABSENT → REACHED` plus
§22.3's and §22.4's `ABSENT → PARTIAL` (ADR 0056) — four rows off `ABSENT`
against the prior 18/127/20, not one. Recounted again later the same day
for ADR 0057: §22.3 and §22.4 `PARTIAL → REACHED`, two rows, 21/130 to
23/128; §7.2, §22.1, §22.2, §56.3 and §56.4 were corrected in place and
none changed verdict.

**Not recounted 2026-09-14 for ADR 0064, and that is the finding rather than
an omission.** §12.3's R5 gains a measured half and stays `absent` on the
revision the row asks for; §12.3 stays `PARTIAL`, §23.1 stays `PARTIAL` with
LEVEL 2 `ABSENT`, and §12.1 is corrected in place without changing verdict.
**23/128/1/14/15 is unchanged**, and it is written out here because three
previous recounts in this file were wrong in the optimistic direction and a
row that moves from "nothing" to "measured" is exactly the shape that invites
a fourth.

### Why there is no single percentage here

Because it would be the same lie the nineteen documents told. **The
blueprint's sections are not units of equal weight.** §5.6 bundles nine
execution domains; §46.2 is an eighteen-row control table; §41.5 is twelve
payload slots. A section where eleven of twelve requirements are met scores
`PARTIAL`, and so does one where one of twenty is. Dividing 12 by 181 would
produce "7% delivered", which is false, and averaging the partials would
require a weighting nobody has agreed.

`PARTIAL` at 69% is not a hedge — it is the accurate shape of a platform that
has built a defensible subset of nearly every table in the specification, and
the whole of very few. **Track the rows, not a headline.** Each row moves
independently and says what would move it.

---

## What cannot be delivered from this repository

These are not backlog. No amount of engineering in this container closes them.

| Blocker | What it holds | To clear it |
|---|---|---|
| No deployment | Every ingestion claim, all runtime observability, the entire cloud plane | One authenticated `terraform apply` a person dispatches through `infra.yml`. **`dev` was applied (runs 34–38) and then torn down on 2026-09-13 on the owner's instruction** — the Cloud Run services, the control-plane cluster and 166 Terraform-managed resources are gone; 55 free entries remain (API enablement, the workflow's identity, an empty VPC and two subnets Google's egress addresses still hold). The "Infrastructure register" entry in Provenance has the three run URLs and the remainder verbatim. Nothing is deployed anywhere. |
| Metrics collector | Cloud Run scrape ingestion | Google publishing a sidecar that passes the Trivy gate. Do **not** clear with a scanner exception |
| Option-quote licensing | The volatility surface's caller (ADR 0050) | An owner-side vendor licensing evaluation |
| ADR 0038 unaccepted | Passkeys | Four checks only the owner can run against `algorik-dev` |
| ADR 0022/0025 | Leptos | A decision to authorise it. No Leptos code exists anywhere |
| Boot image never baked | The execution-node plan | `image.yml` dispatched after `infra.yml` applies `module.image_bake` |

---

## Configuration switches held closed in every environment

Twelve Terraform settings sit in a closed position — `false`, `{}`, `null` or
`[]` — across the four environment files, and the command below is what counts
them rather than a figure typed here. That is not a gap list; several are
closed by decision. But a closed switch is the difference between a capability
that exists in the tree and one that exists in a deployment, so no row above
can be read as "operating" while its switch is off.

```
grep -rhoE '^[a-z_]+ *= *(false|\{\}|null|\[\])' infrastructure/environments/*/terraform.tfvars | sort -u
```

The four that hold the most: `execution_nodes = {}` (no edge node exists, so
every pass-time series and the whole regional plane reach no process),
`workload_metrics_exist = false` (every alert policy evaluates to `count = 0`),
`metrics_collector_image_digest = null` (refused, not pending — the published
sidecar fails this platform's Trivy gate), and `enable_spanner = false` (the
ledger grants are created but empty).

Whether each closure is a decision or an oversight is answered by the section
rows above and by `docs/adr/`, which is where decisions live. This replaced a
584-line register that asked the same question in prose and a 1,309-line
register of blueprint-versus-Terraform deltas; both were status documents by
another name, and the deltas they tracked are the §41–§46 rows above.

---

## Where the blueprint and the code disagree

Found while scoring, verified directly, and recorded here rather than in a
private note — these are places the specification and the tree state
different things, and somebody has to decide which is wrong.

**Three of the four originally filed here are resolved, 2026-09-12, and kept
below rather than deleted, because a status document that erases a closed
question tells the next reader nothing about how it was closed.**

1. **Resolved by [ADR 0052](adr/0052-blueprint-rule-17-is-corrected-a-deterministic-gate-must-be-able-to-approve.md).**
   §56.x rule 17 read "neither pre-trade gate ever returns approval."
   `PreTradeDecision` has an `Approved` arm and a `Reduced` arm that resizes
   an order rather than refusing it
   (`grep -n 'enum PreTradeDecision' -A 12 backend/crates/services/qip-risk-engine/src/pretrade.rs`),
   and the code was not the side that was wrong: a deterministic pre-trade
   check that never lets a compliant order through cannot do its job, the
   blueprint's own §33 already resizes rather than only vetoing on a
   belief-freshness failure, and `Reduced` is an exact computed bisection,
   gated off by default and dormant in the one production composition root.
   Rule 17 is corrected in `docs/architecture/algorik-blueprint-v10.1-source.md`
   to state the property that actually holds; §56.2's row is updated to match.
2. **Resolved by [ADR 0053](adr/0053-declared-and-gated-managed-service-scaffolding-does-not-contradict-not-used.md).**
   §44.1 says Vertex AI is not used, and (by implication, in the "time-series
   database" row) so is Bigtable; neither "Bigtable" nor "AlloyDB" is a name
   the blueprint text itself contains
   (`grep -in 'bigtable\|alloydb' docs/architecture/algorik-blueprint-v10.1-source.md`
   returns nothing). All three have `count = var.enable_X ? 1 : 0` resource
   blocks in Terraform, and the flag is `false` in every environment
   (`grep -rhoE 'enable_(vertex|bigtable|alloydb)[a-z_]* *= *(true|false)' infrastructure/environments/*/terraform.tfvars`),
   so zero instances of any of them exist and no adapter could use one if the
   flag were flipped. Declared-and-disabled scaffolding with no usable
   adapter is what "not used" already meant; this was filed as a
   contradiction and was not one. §44.1's row is corrected to say so.
3. **§56.x rule 74 requires OpenTelemetry spans.** No service emits one;
   `ActiveSpan` and `SpanKind` match only their own file.
4. **Resolved, 2026-09-12, no ADR needed.** §56.x rule 76 reads "cargo-audit
   and cargo-deny run in the pipeline. A new critical advisory blocks the
   build." `cargo audit --deny warnings` already ran in both
   `.github/workflows/ci.yml` and the Makefile's `audit` target; `cargo deny
   check` now runs beside it — the `dependency-supply-chain` job in `ci.yml`
   and `make deny`/`make all` in the Makefile, both against `backend/deny.toml`
   (`grep -n 'cargo deny check' .github/workflows/ci.yml Makefile`). No
   architecture decision was required: `cargo-deny` is CI/development tooling
   invoked via `cargo install`, the same way clippy and rustfmt are, and it
   audits the two dependencies ADR 0002 permits — it is not itself a
   workspace dependency, so ADR 0002/0009's two-dependency ceiling was never
   the thing blocking it. `deny.toml`'s `[bans]` allowlist names the same
   eleven packages as `scripts/check-dependencies.sh`, so the two checks
   cannot silently disagree about what is permitted. This was a missing
   implementation, not a contradiction between the specification and the
   tree, and closing it needed no correction to either.

---

## Absences this document asserts, and CI re-checks

The four claims below are written in the one shape
`qip-acceptance`'s `no_scored_plan_document_calls_a_search_empty_that_is_not`
can execute — a bare literal pattern, no quotes, no regex — so the suite runs
each of them on every test run and fails if any starts finding files. A status
document that asserts an absence should be held to it by machine, not by a
reader's trust; every other evidence cell below is a command a person runs, and
these four are commands the build runs.

- **No Leptos code exists.** `grep -rli leptos frontend/portal/src` returns nothing.
  Unauthorised by ADR 0022/0025.
- **No passkey ceremony exists.** `grep -rli webauthn frontend/portal/src` returns nothing.
  ADR 0038 is *proposed* and blocks on four owner-run checks.
- **No crate depends on `cargo-deny`.** `grep -rl cargo_deny backend/crates` returns nothing.
  Correct by design, not a gap: cargo-deny is CI/development tooling invoked via
  `cargo install`, the same way clippy and rustfmt are, and never belongs in a
  `Cargo.toml`. §56.x rule 76 is held at the pipeline instead — `cargo deny
  check` runs in `.github/workflows/ci.yml`'s `dependency-supply-chain` job and
  in `make deny`/`make all`, against `backend/deny.toml`.
- **No service names a span type.** `grep -rl SpanKind backend/crates/services` returns nothing,
  against §56.x rule 74, which requires OpenTelemetry spans. The one observability
  hole that is not about ingestion.

## The 181 sections

| § | Title | Verdict | Evidence |
|---|---|---|---|
| 1.1 | What Changed and Why — v9 over v8, and v10 over v9 | NARRATIVE | A two-column changelog of what v9/v10 added and why; every entry defers its deliverable to a numbered section. No testable claim of its own. The sections it forward-references are scored where they appear. |
| 1.2 | The Seven Planes | PARTIAL | Six of the seven planes have a crate home; the seventh (Valuation) is a kernel module rather than a plane. `ls backend/crates/services backend/crates/edge backend/crates/runtime` — Ingestion (`qip-market-ingestion`, `qip-entity-resolution`, `qip-data-finder`), Cognition (`qip-world-model`, `qip-reasoning-engine`, `qip-twin`, `qip-learning-engine`), Intelligence (`qip-training`, `qip-evolution`, `qip-lifecycle`), Optimisation (`qip-optimization-engine`, `qip-capital`), Execution (`backend/crates/edge/`), Ledger (`qip-capital-fabric`); Valuation is `ls backend/crates/runtime/qip-kernel/src/valuation.rs` plus `libs/qip-financial`. The per-plane time scales in the table are asserted, not measured anywhere. Completeness per plane is scored at §5.1–5.7. |
| 1.3 | An Honest Word on Capital | NARRATIVE | Argument about capital scale plus a table of four arenas where small capital wins. No deliverable stated. Of the four, only prediction markets have a crate — `ls backend/crates/services/qip-prediction/src` — and funding capture exists only as an assumption constant, `grep -n 'funding_rate_annual_f64' backend/crates/edge/qip-arbitrage/src/netedge.rs`; maker rebates return nothing: `grep -rn 'rebate' --include=*.rs backend/crates \| grep -v tests`. |
| 2.1 | Technology Rules | PARTIAL | Rust-everywhere holds for backend and is structurally enforced (`grep -n 'unsafe_code' backend/Cargo.toml`; `./scripts/check-dependencies.sh`), but the rule's own scope names **frontend** and excludes TypeScript, and the repo ships two Next.js apps — `ls frontend/portal/src/app frontend/landing/app`. That deviation is a standing decision, not a gap: `ls docs/adr/0001-*`. Google-Cloud/IBM-only holds in tree: `grep -rn 'provider "' infrastructure/terraform/*.tf`. |
| 2.2 | Architectural Rules | PARTIAL | Held and reached: "no strategy sends an order" and netting (`grep -n 'fn work\|net(' backend/crates/edge/qip-edge/src/cell.rs`), "risk reads aggregates" (`grep -n 'pub enum LimitKind' backend/crates/libs/qip-risk/src/limits.rs`), "feasibility precedes profitability" (`grep -n 'feasibility::assess' backend/crates/edge/qip-edge/src/cell.rs`), "belief precedes action" (`grep -n 'effective confidence' backend/crates/runtime/qip-kernel/src/platform.rs`), "every path not taken is scored" (`grep -n 'score_declined' backend/crates/runtime/qip-kernel/src/platform.rs`), "no LLM touches a trade" (`grep -n 'Determinism' backend/crates/services/qip-cost-router/src/lib.rs`), "capital moves inside signed corridors" (`grep -n 'EnvelopeIssuer' backend/crates/services/qip-capital/src/envelope.rs`), "promotion is statistical" (`grep -n 'assess_overfitting' backend/crates/runtime/qip-kernel/src/central/learning.rs`). Not held: "strategies are compiled, not interpreted" — the compiled-plan slot is never produced (`grep -n 'compiled_plan' backend/crates/apps/qip-api/src/mesh.rs` returns nothing); "after-tax return is the only return" — no tax engine (`grep -rn 'TaxLot\|lot_selection' --include=*.rs backend/crates` returns nothing beyond `LotMethod` in `backend/crates/libs/qip-portfolio/src/lot.rs`). |
| 4.1 | Why Cognition Is Its Own Plane | NARRATIVE | Rationale for a plane boundary. Its one falsifiable clause — that a degraded Cognition narrows and says so — is the §6.2 table, scored there: `grep -n 'pub enum Capability' backend/crates/libs/qip-contracts/src/degradation.rs`. |
| 4.2 | What Exists Once Versus Per Region | PARTIAL | The global/regional split is real and structural: global state lives in `Platform` (`grep -n 'struct Platform' backend/crates/runtime/qip-kernel/src/platform.rs`), regional in `Cell` (`grep -n 'pub struct Cell' backend/crates/edge/qip-edge/src/cell.rs`), and the shipping seam is the twelve-slot payload (`grep -n 'pub struct PolicyPayload' backend/crates/libs/qip-contracts/src/policy.rs`). But of the eight regional rows that need a shipped artefact, only four slots are ever produced — `grep -n 'Slot::produced\|= episodic' backend/crates/apps/qip-api/src/mesh.rs` — so "compiled strategy plan", "belief priors cached", "inventory manager targets" and "inference in process" have no regional copy. Seven cells are a claim, not a deployment: `grep -rn execution_nodes infrastructure/environments/*/terraform.tfvars`. |
| 5.1 | Ingestion | PARTIAL | 3 of 4 domains reached — **2 until 2026-09-15, corrected in the understating direction; see Source Discovery below.** **Market Connectivity** REACHED — `grep -n 'ConnectorFeed' backend/crates/apps/qip-api/src/feed.rs backend/crates/apps/qip-fastbrain/src/feed.rs`, four connectors only (`grep -n 'SOURCE_ID =>' backend/crates/services/qip-market-ingestion/src/connector_feed.rs`), venue latency in `grep -n 'latency_multiple_f64' backend/crates/edge/qip-routing/src/health.rs`; RFQ absent (`grep -rn -i 'rfq' --include=*.rs backend/crates` returns nothing). **Information Ingestion** PARTIAL — news/alternative adapters exist but their only non-test caller is the CLI demo: `grep -rn 'narrative::\|alternative::' --include=*.rs backend/crates/apps \| grep -v tests`. **Entity Resolution** REACHED — `grep -n 'resolver.resolve' backend/crates/services/qip-world-model/src/world.rs`. **Source Discovery** REACHED — **corrected 2026-09-15 from `UNREACHED`.** This cell said that search finds the kernel method and only test callers. It does not: `grep -rn 'assess_sources' --include=*.rs backend/crates/apps` finds `qip-deepbrain/src/discovery.rs` and `main.rs`, where `DiscoveryDesk` runs `Platform::assess_sources` on its own cadence from the evolution round, itself reached from `node::run`, against a candidate catalogue an operator declares. §7.4 has credited that chain since it landed; this cell was not re-read then. **The row's own verdict does not move and the correction must not be read as moving it**: §5.1 is `PARTIAL` on Information Ingestion, which needs a text connector that does not exist, and that is untouched by this. |
| 5.2 | Cognition | PARTIAL | 5 of 7 domains reached. **World Model** REACHED (`grep -n 'fn stage_understand' -A 12 backend/crates/runtime/qip-kernel/src/platform.rs` reads `world.state_at`). **Causal Inference** REACHED but structurally starved — `grep -n 'causal.propagate' backend/crates/agents/qip-investment-agents/src/reasoning.rs`, reached via `stage_reason` → chief → `CausalAnalyst` (`grep -n 'CausalAnalyst::new' backend/crates/agents/qip-investment-agents/src/chief.rs`); the file's own comment records that no absorb arm writes a causal edge, so on a deployed platform the graph is empty. **Episodic Memory** REACHED (`grep -n 'self.episodes.recall\|pending_episodes' backend/crates/runtime/qip-kernel/src/platform.rs`; shipped by `grep -n 'issue_episodic_digest' backend/crates/apps/qip-api/src/mesh.rs`). **Belief State** REACHED (`ls backend/crates/services/qip-reasoning-engine/src/belief.rs`; `grep -n 'BeliefFreshness' backend/crates/libs/qip-contracts/src/degradation.rs`) — but the `belief_priors` slot is never produced. **Counterfactual Learning** REACHED (`grep -n 'score_declined' backend/crates/runtime/qip-kernel/src/platform.rs`). **Self-Model** REACHED (`grep -n 'learn_from\|self_model.absorb' backend/crates/runtime/qip-kernel/src/platform.rs`). **Hypothesis Generation** REACHED (`grep -n 'fn synthesise' backend/crates/runtime/qip-kernel/src/platform.rs`). |
| 5.3 | Valuation | PARTIAL | 5 of 6 domains reached. Term Structure, Credit, Illiquid Valuation, Cashflow/Commitments and Corporate Actions all have production call paths from `Platform::new`/`stage_sense`: `grep -n 'CreditRegister::from_universe\|private_holdings_of\|apply_due_corporate_actions' backend/crates/runtime/qip-kernel/src/platform.rs` and `grep -n 'TermStructure' backend/crates/runtime/qip-kernel/src/valuation.rs`. **Volatility Surface** is UNREACHED — fully built and tested (`grep -n 'pub struct VolatilitySurface' backend/crates/libs/qip-market/src/volatility.rs`, `ls backend/crates/libs/qip-market/tests/volatility.rs`) with no caller anywhere: `grep -rn 'VolatilitySurface' --include=*.rs backend/crates \| grep -v 'qip-market/'` returns one doc-comment reference only. |
| 5.4 | Intelligence | PARTIAL | 3 of 6 domains reached. **Strategy Lifecycle** REACHED (`grep -n 'review_strategies\|learn_from_cells' backend/crates/runtime/qip-kernel/src/platform.rs`; gates in `ls backend/crates/services/qip-lifecycle/src/gates.rs`). **AI/ML Platform** PARTIAL — training and distillation reached from the deep brain (`grep -n 'TrainingDataset::new\|distil(' backend/crates/apps/qip-deepbrain/src/learning.rs`) but ONNX export is absent: `grep -rn -i onnx --include=*.rs backend/crates` returns one comment. **Risk Policy** REACHED (`grep -n 'pub enum LimitKind' backend/crates/libs/qip-risk/src/limits.rs`; pre-trade in `ls backend/crates/services/qip-risk-engine/src/pretrade.rs`) — but the blueprint's "ten levels" envelope is not a named structure: `grep -rn 'RiskLevel' --include=*.rs backend/crates` returns nothing. **Meta-Learning** ABSENT (`grep -rn -i 'meta_learning\|MetaLearning' --include=*.rs backend/crates` returns nothing). **Adversarial Modelling** ABSENT as a domain — `grep -rn 'Adversar' --include=*.rs backend/crates` finds only the `AgentRole::Adversarial` governance role and the unproduced `adversary_profiles` slot. **Market Simulation** UNREACHED — adaptive counterparties are built and tested (`grep -n 'pub struct CounterpartyAgent\|fn with_agents' backend/crates/services/qip-simulation-engine/src/agents.rs backend/crates/services/qip-simulation-engine/src/market.rs`) with no caller outside the crate: `grep -rn 'with_agents' --include=*.rs backend/crates \| grep -v qip-simulation-engine`. |
| 5.5 | Optimisation | PARTIAL | 2 of 3 domains reached. **Allocation** REACHED (`grep -n 'family_structure\|arm_horizon_gate' backend/crates/runtime/qip-kernel/src/platform.rs`; `ls backend/crates/services/qip-optimization-engine/src/families.rs backend/crates/services/qip-optimization-engine/src/horizons.rs`) — though the stage_learn comment states no seam consumes a family. **Capital Engine** REACHED (`grep -n 'EnvelopeIssuer\|fn issue' backend/crates/services/qip-capital/src/envelope.rs`; reserve/exploration in `ls backend/crates/services/qip-capital/src/reservation.rs`). **Scenario and Stress** is REACHED, and this cell said UNREACHED on a grep that does not return nothing. **Corrected 2026-09-15:** `grep -rn 'scenario::\|Scenario' --include=*.rs backend/crates/apps backend/crates/runtime \| grep -v tests` returns six lines, and `grep -n 'StressTester::new\|standard_library()' backend/crates/runtime/qip-kernel/src/platform.rs` puts both inside `stage_simulate`, so the stress path is driven by `run_cycle` on every cycle. What keeps the row short of REACHED is §23.7's half — two of its four methods, causal propagation through mechanisms and the adversarial worst-plausible sequence, are not built — and shock propagation through the causal graph has no stress caller (`grep -rn 'propagate(' --include=*.rs backend/crates \| grep -v 'fn propagate'` shows one production caller, the panel's `CausalAnalyst`, not a stress path). |
| 5.6 | Execution | PARTIAL | 5 of 9 domains reached, all through one path: `qip-edge-node/src/main.rs` → `run_pass` → `Cell::work` (`grep -n 'fn run_pass' backend/crates/apps/qip-edge-node/src/pass.rs`; `grep -n 'run_pass(' backend/crates/apps/qip-edge-node/src/main.rs`; `grep -n 'cell.work(' backend/crates/apps/qip-edge-node/src/pass.rs`). REACHED: Market Intelligence (`ls backend/crates/edge/qip-orderbook backend/crates/edge/qip-feature-dag`), Strategy Engine (`ls backend/crates/edge/qip-strategy`), **Feasibility** (`grep -n 'feasibility::assess' backend/crates/edge/qip-edge/src/cell.rs`), **Arbitrage** (`ls backend/crates/edge/qip-arbitrage`; `grep -n 'fn scan\|re-quote' backend/crates/edge/qip-edge/src/cell.rs`), **Intent Netting** (`grep -n 'netting_ratio\|InternalCross' backend/crates/edge/qip-edge/src/cell.rs`). PARTIAL: **Inventory and Mirrors** — reservation and mirrors exist (`ls backend/crates/edge/qip-edge/src/reservation.rs`; `grep -n 'Mirror' backend/crates/edge/qip-edge/src/journal.rs`) but the `inventory_targets` slot is never produced (`grep -n 'InventoryTargets' backend/crates/runtime/qip-kernel/src/central/whitelist.rs`). ABSENT: **Market Making and Creation** — no quoting engine, no inventory skew, no origination: `grep -rn 'inventory_skew\|fn quote_two_sided\|MarketMaker' --include=*.rs backend/crates` returns nothing, and the only `two_sided` in the tree is a test fixture name in `backend/crates/edge/qip-orderbook/tests/venue.rs`, **Registries** — asset classes exist (`grep -n 'pub enum AssetClass' backend/crates/libs/qip-financial/src/asset_class.rs`) and settlement calendars exist (`ls backend/crates/services/qip-capital-fabric/src/settlement.rs`) but venue onboarding, hedge map and cross-margin all return nothing: `grep -rn 'VenueOnboard\|HedgeMap\|CrossMargin' --include=*.rs backend/crates`. **Position Lifecycle** REACHED (`grep -n 'PositionLifecycle' backend/crates/libs/qip-portfolio/src/lib.rs`; divestment reached as `DispositionOutcome` in `grep -n 'DispositionOutcome' backend/crates/runtime/qip-kernel/src/platform.rs`; liquidity ladder `ls backend/crates/libs/qip-financial/src/ladder.rs`). |
| 5.7 | Ledger, Experience and Platform | PARTIAL | **Ledger, Wallet and Treasury** REACHED — `grep -n 'reconcile_wallet' backend/crates/runtime/qip-kernel/src/platform.rs` is called from `stage_learn`; corridors and custody in `ls backend/crates/services/qip-capital-fabric/src/corridor.rs backend/crates/services/qip-capital-fabric/src/custody.rs`. Tax lots are the named exception: `grep -n 'pub enum LotMethod' backend/crates/libs/qip-portfolio/src/lot.rs` exists but no jurisdiction or holding-period selection does — `grep -rn 'jurisdiction' --include=*.rs backend/crates/libs/qip-portfolio` returns one doc comment in `lot.rs` and no code, and `grep -rn 'after_tax\|TaxEngine' --include=*.rs backend/crates` returns nothing. **Experience and Identity** PARTIAL — authentication and roles are real (`grep -n 'fn authenticate' backend/crates/apps/qip-api/src/auth.rs`), portal and landing exist (`ls frontend/portal/src/app frontend/landing/app`), and mandates are a whole subsystem. **This clause said the opposite until 2026-09-16** — "mandates exist nowhere as a type, `grep -rn 'Mandate'` returns nothing" — and it was false, in the direction that understates what is built. `grep -rn 'pub struct Mandate\|pub enum Mandate\|pub type Mandate' --include=*.rs backend/crates` prints six declarations across four crates: `Mandate` in both `qip-capital/src/ledger/mandate.rs` and `qip-portfolio-engine/src/construction.rs` (two distinct types, deliberately — one is the ledger's authority record, one the optimiser's constraint set), plus `MandateId`, `MandateTerms`, `MandateRegistry` and the API's `MandateView`. It is production-reached rather than declared: `grep -n 'user_mandates\|MandateRegistry' backend/crates/runtime/qip-kernel/src/config.rs backend/crates/services/qip-capital/src/ledger/book.rs` shows the platform config carrying `Vec<UserMandate>` and the ledger's own book holding the registry. **The bad claim survived because the citation behind it was a bare name grep that the parenthetical then explained away** — a command whose output was argued with instead of read. The half stays PARTIAL on the tax gap above, not on mandates. |
| 6.1 | The Three Return Paths | REACHED | All three re-entry points fire inside one stage. Ledger-as-fact: `grep -n 'reconcile_wallet' backend/crates/runtime/qip-kernel/src/platform.rs` (called from `stage_learn`). Cognition-as-episode and -as-counterfactual: `grep -n 'resolve_pending_episodes\|score_declined' backend/crates/runtime/qip-kernel/src/platform.rs`. Intelligence-as-training-signal: `grep -n 'TrainingDataset::new' backend/crates/apps/qip-deepbrain/src/learning.rs`. Path: `run_cycle → stage_learn → {reconcile_wallet, calibrate_resolved, score_declined}`. |
| 6.2 | Degradation Order | REACHED | The seven-row table is a typed enum with a per-capability narrowing, and it is read on both planes. `grep -n 'pub enum Capability\|pub enum Freshness\|pub enum StrategyClass\|fn affects_trading' backend/crates/libs/qip-contracts/src/degradation.rs`; produced centrally at `grep -n 'degradation::' backend/crates/runtime/qip-kernel/src/platform.rs` (in `stage_reason`) and consumed at the edge by `grep -n 'DegradationState\|StrategyClass' backend/crates/edge/qip-edge/src/cell.rs backend/crates/apps/qip-edge-node/src/arbitrage.rs`. The counterfactual row's "no trading impact" is structural rather than commented: `grep -n 'fn affects_trading' -A 6 backend/crates/libs/qip-contracts/src/degradation.rs`. Caveat: because eight of twelve slots ship unproduced, a real cell reads most capabilities as unavailable, so the table is exercised mostly on its degraded arms. |
| 7.1 | Sources | PARTIAL | 3 of the 9 source classes have a live connector, and the platform's own outcomes make a fourth. `grep -n 'SOURCE_ID =>' backend/crates/services/qip-market-ingestion/src/connector_feed.rs` names exactly four: Coinbase ticker (Market), Alpaca bars (Market), Frankfurter rates (Economic, FX only), Kalshi markets (Resolution/prediction). On-chain exists as a crate reached from the kernel (`grep -n 'qip_chain::' backend/crates/runtime/qip-kernel/src/platform.rs`) with no connector feeding it. Corporate, News/text, Registry and Physical have no connector: `ls backend/crates/services/qip-market-ingestion/src/connectors/`. |
| 7.2 | Ingestion Is Also Pass-Through | PARTIAL | The retained/discarded discipline is built and the manifest-with-hash is real: `grep -n 'pub struct SourceManifest' backend/crates/libs/qip-financial/src/manifest.rs`, and bounded retention is enforced at `grep -n 'MARKET_EVENT_RETENTION\|CHAIN_RETENTION' backend/crates/runtime/qip-kernel/src/platform.rs`. Since ADR 0057 the "manifest pointing at the original, with a content hash" has a production producer for *market data*: every delivered connector poll is digested where its bytes exist (`grep -n 'report.digest = ' backend/crates/services/qip-market-ingestion/src/connector/runtime.rs`), the body is released and not retained, and the digest becomes a `DataReference` on the kernel's bounded ledger (`grep -n 'pub fn reference_fetch' backend/crates/runtime/qip-kernel/src/references.rs`) — see §22.3. The half still not met is the section's own subject, *documents*: no production connector ingests text at all, so the rule that raw article and filing text is discarded has never had a document to discard — the only `SourceManifest` producer outside tests is the synthetic narrative generator (`grep -n 'SourceManifest::generated' backend/crates/services/qip-market-ingestion/src/synthetic/narrative.rs`), reached only from `grep -n 'narrative::' backend/crates/apps/qip-cli/src/demo/mod.rs`. |
| 7.3 | Ingestion Pipeline | PARTIAL | 5 of 7 stages reached. Fetch, Deduplicate, Timestamp, Assess and Emit run in the connector runtime, which apps compose: `grep -n 'admission\|DedupWindow\|backoff\|schema' backend/crates/services/qip-market-ingestion/src/connector/runtime.rs` and `grep -n 'ConnectorFeed::open' backend/crates/apps/qip-api/src/feed.rs backend/crates/apps/qip-fastbrain/src/feed.rs`; event-vs-receipt time is `grep -n 'known_at' backend/crates/libs/qip-contracts/src/time.rs`. Extract (structured facts from semi-structured text) and Resolve are built — `grep -n 'fn absorb_news' backend/crates/services/qip-world-model/src/world.rs`, `grep -n 'resolver.resolve' backend/crates/services/qip-world-model/src/world.rs` — but reachable only from the CLI demo, because no text connector exists. The blueprint's "language models assist extraction" has no implementation: `grep -rn 'extract' --include=*.rs backend/crates/libs/qip-ai/src`. |
| 7.4 | Source Discovery — Finding New Feeds | PARTIAL | ASSESS, REGISTER and SCORE are built, tested and refuse correctly — `grep -n 'fn assess' backend/crates/services/qip-data-finder/src/finder.rs`, `ls backend/crates/services/qip-data-finder/tests/` (10 suites), and the five-axis score with reliability/freshness/uniqueness at `grep -n 'pub struct SourceScores\|fn composite' backend/crates/services/qip-data-finder/src/scoring.rs` — and the tier/legality/robots screen runs before registration (`grep -n 'SourceTier::classify' backend/crates/services/qip-data-finder/src/finder.rs`). The CRAWL stage — seed, follow links, expand into related domains — does not exist: `grep -rn 'follow_links\|expand_from\|fn crawl(' --include=*.rs backend/crates/services/qip-data-finder/src` returns nothing, and candidates arrive as a caller-supplied `Vec<SourceCandidate>` (`grep -n 'pub fn assess' -A 6 backend/crates/services/qip-data-finder/src/finder.rs`). The `seed` in that crate is a tie-break RNG seed, not a crawl seed: `grep -n 'seed: u64' backend/crates/services/qip-data-finder/src/finder.rs`. **Classify now exists; Sample does not.** §7.6.1's category classification runs inside `assess_one` at the existing `LifecycleStage::Classify` step — `grep -n 'SourceCategory::classify' backend/crates/services/qip-data-finder/src/finder.rs` — on a `ContentSignal` the candidate itself declares (`SourceCandidate::with_content_signal`), refusing to classify one that fits none of the eight §7.6.1 categories rather than force-fitting it. Sample ("read enough to judge usefulness... never a full crawl") still does not exist: nothing in this crate reads a page's content, so `ContentSignal` is a declared claim, not an inferred one. This closes the paragraph's own "see §7.6.1" cross-reference (ADR 0056); it does not touch the CRAWL gap, which is why the row's verdict is unchanged. **The assessed half now has a production caller.** `qip-deepbrain`'s `DiscoveryDesk` — a candidate list an operator declares in a file `QIP_DEEPBRAIN_SOURCE_CANDIDATES_PATH` names, and `qip_data_finder::probe::NetworkProbe` as the probe — runs `Platform::assess_sources` on its own cadence from the evolution engine's `maybe_discover`, itself called from `node::run`: `grep -rn 'assess_sources' --include=*.rs backend/crates/apps` now finds the caller in `qip-deepbrain/src/discovery.rs` and `main.rs`. Scored `REACHED` on this half alone would overstate the section — the CRAWL stage this row's own `PARTIAL` names is still absent, and that is what keeps the verdict here rather than moving it — but the wiring gap the closing sentence used to name is closed: candidates come from an operator's file exactly as universe.json and the capital-fabric declaration already do, not from a crawl this platform does not have. `NetworkProbe` reaches a source only through the reviewed egress route its catalogue entry names (ADR 0060; `grep -n 'NetworkProbe::through' backend/crates/apps/qip-deepbrain/src/discovery.rs`), so a deployed pass would fetch and assess exactly the sources somebody wrote a route for and none other — the CRAWL gap is structural behind a reverse proxy, not merely unbuilt. This sentence said the probe "refuses every call by name until a TLS-capable transport is authorised" until the 2026-09-13 merge of the two discovery lanes; that was the wrong diagnosis, and it is withdrawn rather than deleted. |
| 7.5 | The Dark Web, and the Hard Line | REACHED | The hard line is built and enforced at classification, before registration, exactly as the section requires: `grep -n 'SourceTier::DarkWeb' backend/crates/services/qip-data-finder/src/finder.rs` refuses a dark-tier candidate, `grep -n 'DARK_HOST_SUFFIXES\|fn feeds_training' backend/crates/services/qip-data-finder/src/tier.rs` keeps dark facts out of training structurally, and the enclave with no capital-moving path is `grep -n 'pub struct DiscoveryEnclave\|fn permits_egress_to' backend/crates/services/qip-data-finder/src/tier.rs`. Tested at `ls backend/crates/services/qip-data-finder/tests/legality.rs backend/crates/services/qip-data-finder/tests/tiers.rs`. **A non-test caller now exists** for the same reason §7.4's does: `qip-deepbrain`'s `DiscoveryDesk` reaches `DataFinder::assess` through `Platform::assess_sources`, on a cadence, from a declared candidate list. Scored on the `Cell::work` bar: the path is real and in the binary. Terraform now declares both discovery variables (`deepbrain_discover_every`, `source_candidates_file`), mounted the same way `capital_fabric_file` is; no environment assigns either — `grep -rn '^\s*deepbrain_discover_every\s*=\|^\s*source_candidates_file\s*=' infrastructure/environments/*/terraform.tfvars` returns nothing (a bare `grep -rn QIP_DEEPBRAIN_DISCOVER_EVERY` now also matches a tfvars comment explaining why it stays unset, which is not the same fact) — so nothing deployed has assessed a dark-tier candidate. `DefensiveMonitoring` (the permitted-column deliverable: own credentials, venue breach, threat indicators) remains built, exported and tested but constructed nowhere outside its own tests — this change did not touch it: `grep -rn 'DefensiveMonitoring' --include=*.rs backend/crates` still returns only the type, the `lib.rs` re-export and `tests/tiers.rs`, and no anomaly detector or transfer gate consumes an indicator. | **Amended at the 2026-09-13 merge of the discovery lanes (ADR 0060).** Two lanes gave `DataFinder::assess` a production caller on the same day, and both survive as one path: `qip-deepbrain`'s `DiscoveryDesk` keeps the cadence and the `QIP_DEEPBRAIN_SOURCE_CANDIDATES_PATH` mount this row already cites, and now holds the routed catalogue (`grep -n 'pub fn load' backend/crates/services/qip-data-finder/src/catalogue.rs`, one `qip_data_finder::catalogue::CandidateEntry` per candidate, each beside the loopback base URL of its reviewed egress route) and builds one `NetworkProbe::through` per entry at pass time (`grep -n 'NetworkProbe::through' backend/crates/apps/qip-deepbrain/src/discovery.rs`). The probe fetches rather than refusing — `cargo test -p qip-data-finder --test probe_port` drives it over a real loopback socket — so the licensing evaluation, the tier classification, the robots check and the schema fingerprint run against a real endpoint once a catalogue is mounted, and the earlier claim on this row that the probe "refuses every call by name until a TLS-capable transport is authorised" is withdrawn: the proxy originates TLS upstream, and what was missing was the route. **Read ADR 0060 before reading this as progress toward the section.** The egress proxy is a reverse proxy: the client emits neither `CONNECT` nor an absolute-form URI, so a process cannot reach a host by asking for it, and the destination is a property of the loopback port. A crawler is definitionally a process that names hosts at runtime, so the two cannot both be true, and the decision was to keep the security property: **a source is probed only where a reviewed egress route already exists.** The catalogue names the route per entry and refuses an entry without one at load, by name, before a socket opens (`cargo test -p qip-data-finder --test candidate_catalogue`). Two further honest limits: nothing is deployed, so no process has yet made the call (`source_candidates_file` is null in every environment and each tfvars says why); and the probe carries no credential, so `Registered` and `Licensed` stay unreachable and `DefensiveMonitoring` still has no consumer. |
| 7.6 | Deep Web Ingestion — Where the Edge Actually Lives | REACHED | The deep/dark distinction is structural and the deep tier is admitted rather than refused: `grep -n 'pub enum SourceTier' -A 12 backend/crates/services/qip-data-finder/src/tier.rs`, `grep -n 'fn admissible' backend/crates/services/qip-data-finder/src/tier.rs`. Tested at `ls backend/crates/services/qip-data-finder/tests/tiers.rs`. Production caller now exists, same chain as §7.4/§7.5: `grep -rn 'assess_sources' --include=*.rs backend/crates/apps` finds `qip-deepbrain`. No deep-web source has been registered in any deployment, still — `grep -n 'SOURCE_ID =>' backend/crates/services/qip-market-ingestion/src/connector_feed.rs` names four public APIs, all surface tier, and no environment names a discovery candidate file — but that is now a declaration nobody has written rather than a caller nobody built. | **Amended at the 2026-09-13 merge of the discovery lanes (ADR 0060).** Two lanes gave `DataFinder::assess` a production caller on the same day, and both survive as one path: `qip-deepbrain`'s `DiscoveryDesk` keeps the cadence and the `QIP_DEEPBRAIN_SOURCE_CANDIDATES_PATH` mount this row already cites, and now holds the routed catalogue (`grep -n 'pub fn load' backend/crates/services/qip-data-finder/src/catalogue.rs`, one `qip_data_finder::catalogue::CandidateEntry` per candidate, each beside the loopback base URL of its reviewed egress route) and builds one `NetworkProbe::through` per entry at pass time (`grep -n 'NetworkProbe::through' backend/crates/apps/qip-deepbrain/src/discovery.rs`). The probe fetches rather than refusing — `cargo test -p qip-data-finder --test probe_port` drives it over a real loopback socket — so the licensing evaluation, the tier classification, the robots check and the schema fingerprint run against a real endpoint once a catalogue is mounted, and the earlier claim on this row that the probe "refuses every call by name until a TLS-capable transport is authorised" is withdrawn: the proxy originates TLS upstream, and what was missing was the route. **Read ADR 0060 before reading this as progress toward the section.** The egress proxy is a reverse proxy: the client emits neither `CONNECT` nor an absolute-form URI, so a process cannot reach a host by asking for it, and the destination is a property of the loopback port. A crawler is definitionally a process that names hosts at runtime, so the two cannot both be true, and the decision was to keep the security property: **a source is probed only where a reviewed egress route already exists.** The catalogue names the route per entry and refuses an entry without one at load, by name, before a socket opens (`cargo test -p qip-data-finder --test candidate_catalogue`). Two further honest limits: nothing is deployed, so no process has yet made the call (`source_candidates_file` is null in every environment and each tfvars says why); and the probe carries no credential, so `Registered` and `Licensed` stay unreachable and `DefensiveMonitoring` still has no consumer. |
| 7.6.1 | Source Categories | REACHED | Re-scored 2026-09-12 (ADR 0056). The eight categories are a real enum: `grep -n 'pub enum SourceCategory' -A 20 backend/crates/services/qip-data-finder/src/category.rs`. §7.4's Classify question ("is this news, filings, data, discussion, a marketplace, a leak forum?") is implemented as `SourceCategory::classify`, which refuses a candidate with no declared signal and refuses the three shapes that question names and none of the eight admits — general news, unspecialised discussion, a leak forum — rather than force-fitting one: `grep -n 'pub fn classify' -A 40 backend/crates/services/qip-data-finder/src/category.rs`. Wired into the existing pipeline, not a separate unused function: `grep -n 'SourceCategory::classify' backend/crates/services/qip-data-finder/src/finder.rs` shows the call inside `assess_one`'s `LifecycleStage::Classify` step, and a successful classification is carried onto `RegisteredSource::category()` for later use by §22.3. Production caller: the same chain §7.4/§7.5/§7.6/§7.6.2 already established — `qip-deepbrain`'s `DiscoveryDesk` reaches this code through `Platform::assess_sources` on every assessed candidate. Whether a given candidate is actually placed in a category still depends on an operator's candidate file declaring a `ContentSignal` — a declaration nobody has necessarily written, the same caveat this document already makes for §7.6's deep-web sources. Tested and mutation-verified at `backend/crates/services/qip-data-finder/src/category.rs`'s own `#[cfg(test)]` module (4 tests, `grep -c '#\[test\]' backend/crates/services/qip-data-finder/src/category.rs`), including the refusal case and the no-signal case. |
| 7.6.2 | Access Modes | REACHED | All six modes are built exactly as tabled, with the policy each one carries attached to the type: `grep -n 'pub enum AccessMode' -A 25 backend/crates/services/qip-data-finder/src/tier.rs` gives OpenQuery/Api/Registered/Licensed/Rendered/Bulk, with rate limit, credential reference, licence name, rendering budget and bulk retention as fields rather than comments. The three never-do rules are structural: paywall circumvention and credential sharing are refused at `grep -n 'fn admissible' -A 40 backend/crates/services/qip-data-finder/src/tier.rs`, and enclave confinement for rendered/bulk is `grep -n 'fn needs_enclave' backend/crates/services/qip-data-finder/src/tier.rs`. Tested (`ls backend/crates/services/qip-data-finder/tests/tiers.rs`), production caller now exists — same chain as §7.4/§7.5/§7.6. | **Amended at the 2026-09-13 merge of the discovery lanes (ADR 0060).** Two lanes gave `DataFinder::assess` a production caller on the same day, and both survive as one path: `qip-deepbrain`'s `DiscoveryDesk` keeps the cadence and the `QIP_DEEPBRAIN_SOURCE_CANDIDATES_PATH` mount this row already cites, and now holds the routed catalogue (`grep -n 'pub fn load' backend/crates/services/qip-data-finder/src/catalogue.rs`, one `qip_data_finder::catalogue::CandidateEntry` per candidate, each beside the loopback base URL of its reviewed egress route) and builds one `NetworkProbe::through` per entry at pass time (`grep -n 'NetworkProbe::through' backend/crates/apps/qip-deepbrain/src/discovery.rs`). The probe fetches rather than refusing — `cargo test -p qip-data-finder --test probe_port` drives it over a real loopback socket — so the licensing evaluation, the tier classification, the robots check and the schema fingerprint run against a real endpoint once a catalogue is mounted, and the earlier claim on this row that the probe "refuses every call by name until a TLS-capable transport is authorised" is withdrawn: the proxy originates TLS upstream, and what was missing was the route. **Read ADR 0060 before reading this as progress toward the section.** The egress proxy is a reverse proxy: the client emits neither `CONNECT` nor an absolute-form URI, so a process cannot reach a host by asking for it, and the destination is a property of the loopback port. A crawler is definitionally a process that names hosts at runtime, so the two cannot both be true, and the decision was to keep the security property: **a source is probed only where a reviewed egress route already exists.** The catalogue names the route per entry and refuses an entry without one at load, by name, before a socket opens (`cargo test -p qip-data-finder --test candidate_catalogue`). Two further honest limits: nothing is deployed, so no process has yet made the call (`source_candidates_file` is null in every environment and each tfvars says why); and the probe carries no credential, so `Registered` and `Licensed` stay unreachable and `DefensiveMonitoring` still has no consumer. |
| 7.6.3 | The Adapter Pattern | PARTIAL | `DeepWebAdapter` carries 4 of the pseudo-code's 8 fields: `grep -n 'pub struct DeepWebAdapter' -A 12 backend/crates/services/qip-data-finder/src/tier.rs` has source, host, tier and mode (mode subsuming `access`). Absent: `query_plan`, `extractor`, `entity_links`, `freshness` — `grep -rn 'query_plan\|entity_links\|freshness' --include=*.rs backend/crates/services/qip-data-finder/src` returns nothing. **§7.6.1's categories now exist (ADR 0056, `SourceCategory` in `category.rs`), so the governance clause's stated precondition is met, and this row's earlier reason for it being unbuildable no longer holds** — but the clause itself, "a human approves the source category once; adapters within an approved category are promoted automatically", is still not built: nothing links a `DeepWebAdapter` to a `SourceCategory` or records a human's one-time approval of one — `grep -rn 'approved_categor\|category_approval' --include=*.rs backend/crates/services/qip-data-finder/src` returns nothing. **Corrected 2026-09-15, understating.** The built half was called `UNREACHED` on §7.4's chain after §7.4 itself had been re-scored: that chain has a production caller (`grep -rn 'assess_sources' --include=*.rs backend/crates/apps`), and `DeepWebAdapter` is constructed inside `assess_one` on it (`grep -n 'DeepWebAdapter::new' backend/crates/services/qip-data-finder/src/finder.rs`), so the four fields that exist are exercised in a production binary whenever a catalogue is declared. What is unreached is this row's own subject rather than the chain: nothing links an adapter to a `SourceCategory`, so the governance clause has nothing to promote automatically however often the adapter is built. |
| 7.6.4 | How It Feeds Training | PARTIAL | The six-row mapping from a deep-web fact to a `WorldEvent`/attribute/observation has no end to start from — no deep-web source is registered (§7.6) and no text connector exists (§7.1) — so nothing traverses it. Sources *are* ranked on a freshness score, but **not the one this section defines**: `grep -n 'let freshness' -A 18 backend/crates/services/qip-data-finder/src/finder.rs` computes age against the source's own expected publication interval (is it late for itself), whereas §7.6.4 asks how far the source's facts preceded the *same fact appearing elsewhere*. Cross-source lead time is not computed: `grep -rn 'lead_time\|preceded\|ahead_of' --include=*.rs backend/crates/services/qip-data-finder/src` returns nothing. Both scorers were called `UNREACHED` here on a search this cell said returns nothing — **corrected 2026-09-15, understating**: `grep -rn 'assess_sources' --include=*.rs backend/crates/apps` finds `qip-deepbrain`'s `discovery.rs` and `main.rs`, the chain §7.4 credits, so the scorer that is built runs in a production binary whenever an operator declares a candidate catalogue. **The verdict is unchanged, for the reason this row opens with**: no deep-web source is registered and no text connector exists, so the six-row mapping still has no end to start from, and the scorer that runs is not the one this section defines. Flagged as the row in this range whose intent is most open to reading: if "freshness" is read loosely the built scorer satisfies it, and this row would be UNREACHED rather than PARTIAL. |
| 7.6.5 | Why This Is the Right Kind of Edge | NARRATIVE | Five properties (lawful, durable, small-capital compatible, compounding, explainable) argued as rationale for §7.6. No deliverable. The lawfulness property is the only one with code behind it, and it is scored at §7.5/§7.6.2. |
| 7.6.6 | Deep Web Governance | PARTIAL | 4 of 6 rules built. Terms-of-use status per source: `grep -n 'pub enum LicensingPosture\|pub struct SourceLicense' backend/crates/services/qip-data-finder/src/legal.rs`. Politeness — robots directives, crawl-delay, an identified user agent that the finder refuses to run without: `grep -n 'crawl_delay\|user_agent' backend/crates/services/qip-data-finder/src/finder.rs`, tested at `ls backend/crates/services/qip-data-finder/tests/robots_precedence.rs`. Licensed credentials scoped per source in Secret Manager: `grep -n 'pub struct CredentialReference' backend/crates/services/qip-data-finder/src/tier.rs`. §7.5 exclusions applying unchanged: `grep -n 'SourceTier::classify' backend/crates/services/qip-data-finder/src/finder.rs`. Not built: "registration is not ingestion" has nothing to enforce because no crawler samples anything (§7.4), and "personal data on private individuals is not registered" is a stated exclusion with no check — `grep -rn 'personal_data\|private_individual\|pii\|PersonalData' --include=*.rs backend/crates/services/qip-data-finder/src` returns nothing. **Corrected 2026-09-15, understating.** "Everything built here is UNREACHED (same chain as §7.4)" was written after §7.4 had been re-scored: that chain has a production caller (`grep -rn 'assess_sources' --include=*.rs backend/crates/apps`), and the four rules above run inside `assess_one` on it. The identified user agent is the sharpest case — it is supplied by the deep brain itself, `grep -n 'DISCOVERY_USER_AGENT' backend/crates/apps/qip-deepbrain/src/discovery.rs`, so the politeness rule the finder refuses to run without is honoured by a production caller and not only by a test. The row stays `PARTIAL` on the two rules that have no code. |
| 8.1 | What It Holds | REACHED | All six object types are in the graph: `sed -n '/pub enum NodeKind/,/^}/p' backend/crates/services/qip-world-model/src/graph.rs` gives Entity, FinancialObject (the instrument link), Event, Factor, Portfolio, Thesis, Evidence and — the arm that closed this row — ResolutionSource; relations are `grep -n 'pub enum RelationshipKind' backend/crates/services/qip-world-model/src/relationship.rs`; attributes are `grep -n 'fn with_attribute' backend/crates/services/qip-world-model/src/graph.rs`. Facts are bitemporal, which is what makes "what did we know then" answerable: `grep -n 'fn holds' backend/crates/services/qip-world-model/src/graph.rs`. REACHED via `run_cycle → stage_understand → world.state_at` (`grep -n 'world.state_at' backend/crates/runtime/qip-kernel/src/platform.rs`). The resolution source used to exist only on a `Proposition` in `qip-prediction`, so nothing could traverse to it; it is now a node written from the LEARN stage that grades the thesis — `grep -n 'record_resolution_source' backend/crates/runtime/qip-kernel/src/platform.rs backend/crates/services/qip-world-model/src/world.rs` — reachable one hop forward from the thesis over `RelationshipKind::ResolvedBy`. No crate edge was added: `qip-world-model` names no prediction type, the runtime passes the *value* across as the boundary rule requires. Both the node's instant and its edge's come from the proposition's own `recorded_at`, never the settlement clock, so a replay standing where the platform decided already sees who would settle the claim: `backend/crates/runtime/qip-kernel/tests/resolution_source.rs::a_cycle_that_grades_a_thesis_puts_its_resolving_authority_in_the_world_model` and the seven cases in `backend/crates/services/qip-world-model/tests/resolution_source.rs`. |
| 8.2 | Why the Graph Structure Earns Its Place | PARTIAL | The traversal primitives every listed query needs are built and tested: `grep -n 'fn paths_between\|fn reachable\|fn neighbours\|fn predecessors\|fn degree' backend/crates/services/qip-world-model/src/graph.rs`, plus causal shock propagation `grep -n 'fn propagate' backend/crates/services/qip-world-model/src/causal.rs`, tested at `ls backend/crates/services/qip-world-model/tests/understanding.rs`. **Four of the blueprint's five named queries now have a production caller; the fifth is not built and is blocked on a data model rather than on a traversal.** Read the five with `awk '/^8\.2 /{f=1} f&&/^8\.3 /{exit} f' docs/architecture/algorik-blueprint-v10.1-source.md` rather than from this cell. Query three, the shortest causal path from a macro release to an instrument, runs through the panel: `grep -n 'causal.propagate' backend/crates/agents/qip-investment-agents/src/reasoning.rs`, reached by `stage_reason → chief → CausalAnalyst` (`grep -n 'CausalAnalyst::new' backend/crates/agents/qip-investment-agents/src/chief.rs`). Query four, hidden concentration across held positions, runs every cycle: `grep -n 'pub fn hidden_concentration' backend/crates/services/qip-world-model/src/exposure.rs` entered through the kernel's pass-through `grep -n 'pub fn concentration' backend/crates/runtime/qip-kernel/src/causal_review.rs`, which `causal_review::review` assembles and `grep -n 'fn causal_review' backend/crates/runtime/qip-kernel/src/platform.rs` calls from `stage_understand` — the same seam §9.1 credits for confounders. **Queries one and five converted on 2026-09-16 and are called together, because neither is actionable alone.** `qip_world_model::exposure::second_order_exposure` asks query five — which entities does the portfolio depend on that it does not hold — and then asks query one of each answer — which instruments sit within two hops of that entity — and reports the exposed instruments the book does *not* hold, which is the subset a desk can act on: `grep -n 'pub fn second_order_exposure\|unheld_dependencies(causal\|instruments_exposed_to(graph' backend/crates/services/qip-world-model/src/exposure.rs`. Its production caller is `stage_understand`: `grep -n 'fn second_order_exposure\|self.second_order_exposure(now)\|cycle_second_order' backend/crates/runtime/qip-kernel/src/platform.rs`. Two things read the result, so it is not a computed value nobody uses — the UNDERSTAND stage detail an operator reads through `CycleReport::summarise`, and `CycleJournalEntry::second_order`, which carries driver, driven positions, unheld instruments and both truncation figures into the hash-chained log (`grep -n 'pub second_order\|struct SecondOrderJournal' backend/crates/runtime/qip-kernel/src/platform.rs`). Bounded in both directions: sixteen dependencies followed per pass, ranked by how much of the book each reaches before the cut, and the exposure leg keeps its own result cap (`grep -n 'MAX_SECOND_ORDER_DEPENDENCIES\|MAX_EXPOSURE_RESULTS\|MAX_EXPOSURE_HOPS' backend/crates/services/qip-world-model/src/exposure.rs`). The `Option` on the journal field is the control: a cycle holding nothing, or holding positions with no causal edge into any of them, journals nothing rather than a zero, so "no unheld dependency" and "nothing was asked" never render alike. `cargo test -p qip-world-model --test second_order` — "test result: ok. 7 passed; 0 failed" — and `cargo test -p qip-kernel --lib second_order_exposure_tests` — "test result: ok. 3 passed; 0 failed" — the kernel three driving `Platform::run_cycle` rather than the private helper. **Why this stays PARTIAL: query two — which prediction contracts resolve on an event this entity controls — cannot be built on what the tree holds, and the gap is data modelling, not traversal.** Three parts are missing and each is independently blocking. `grep -rni 'prediction_contract' --include=*.rs backend/crates` returns nothing. A proposition names no entity and no event: `sed -n '/^pub struct Proposition/,/^}/p' backend/crates/services/qip-prediction/src/resolution.rs` is statement, criteria, source, resolves_at, settlement, dispute_window, and the only edge a proposition puts in the graph runs to the *authority* that settles it, not to an event (`grep -n 'RelationshipKind::ResolvedBy' backend/crates/services/qip-world-model/src/world.rs`). There is no relationship kind for "resolves on" or for "controls": `sed -n '/^pub enum RelationshipKind/,/^}/p' backend/crates/services/qip-world-model/src/relationship.rs`. And the only `NodeKind::Event` written outside a test is a news item, joined to an entity by `ConcernsEntity` — "this item mentions this company" (`grep -rn 'NodeKind::Event' --include=*.rs backend/crates \| grep -v '/tests/'`), which is not control, and treating it as control would manufacture exactly the spurious edges `causal_review.rs`'s module doc argues against. The platform's own only production proposition is a threshold on one instrument's metric settled by `platform-market-data` (`grep -n 'Proposition::new' backend/crates/runtime/qip-kernel/src/platform.rs` prints two lines, the second inside a `#[cfg(test)]` module), so there is no prediction-market contract in the tree to resolve against an event in the first place. Closing it needs two relationship kinds, a writer for "entity controls event" that is not a news mention, and an ingested prediction-market venue — an ADR-sized change, not a query. |
| 8.3 | Size and Storage | PARTIAL | Two claims, split. **Entity resolution confidence per link, low-confidence links excluded from sizing** is REACHED: `grep -n 'confidence' backend/crates/services/qip-entity-resolution/src/resolver.rs` records it per decision and `grep -n 'fn is_authoritative\|>= 0.9' backend/crates/services/qip-entity-resolution/src/resolver.rs` is the exclusion, tested at `ls backend/crates/services/qip-entity-resolution/tests/resolution.rs`. **Spanner storage and the compact regional digest** are not: Spanner is a declared target with no client — `grep -n 'Spanner' backend/crates/libs/qip-storage/src/provider.rs` and `grep -n 'MeshTarget::SpannerGraph' backend/crates/services/qip-mesh/src/provider.rs` name it, the Terraform resource exists but is switched off in every environment (`grep -n 'google_spanner_instance' infrastructure/terraform/modules/data/main.tf`; `grep -rn 'enable_spanner' infrastructure/environments/*/terraform.tfvars` reads `false` in all four), and no Rust code opens it, so the graph lives in process memory (`grep -n 'world:' backend/crates/runtime/qip-kernel/src/platform.rs`). The relationship digest shipped to regions is the `causal_digest` slot, never produced: `grep -n 'causal_digest' backend/crates/apps/qip-api/src/mesh.rs` returns nothing. An in-tree Spanner client is refused by ADR 0009 (`ls docs/adr/0009-*`), so that half is blocked rather than merely undone. |
| 9.1 | What Is Modelled | REACHED | All five layers are typed and each has a production writer or a production reader. Mechanisms, edges (strength, lag, sign via `preserves_sign`, evidence, confidence) and the drivers they name: `grep -n 'pub enum Mechanism\|pub struct CausalEdge' backend/crates/services/qip-world-model/src/causal.rs` — fourteen mechanisms, twelve mechanism-backed plus `TemporalPrecedence` and `InverseTemporalPrecedence`, which deliberately name no mechanism at all. **Confounders.** This cell used to assert the layer was absent and cited a search for it as proof; that search now answers at length — `grep -rn 'pub struct Confounder\|pub struct ConfounderSet\|suspected_confounders\|adjusted_for' --include=*.rs backend/crates/services/qip-world-model/src` — and `EdgeStanding::Suggestive` makes §9.4's "suggestive rather than established" structural rather than advisory. Its production path is §9.2's control audit: `grep -n 'fn causal_review' backend/crates/runtime/qip-kernel/src/platform.rs`, called from `stage_understand` on every cycle, reaches `grep -n 'Confounder::observed' backend/crates/runtime/qip-kernel/src/causal_review.rs`, which measures a cross-sectional factor from the same bars and re-runs each precedence edge's own test with it held constant. **Conditions, new here.** `grep -n 'pub holds_in\|pub fails_in\|pub enum ConditionStanding' backend/crates/services/qip-world-model/src/causal.rs`. The evidence for this layer was already being computed and discarded: the pass tests every ordered pair every cycle and kept only the results that cleared its bar, so a test that ran on ample history and refused — precisely §9.1's "the conditions under which it is known to fail" — went on the floor. Both arms are written now, at the same seam, by `grep -n 'with_conditions(BTreeSet::from\|record_causal_condition_failure' backend/crates/runtime/qip-kernel/src/platform.rs`, and read back into the UNDERSTAND detail by `grep -n 'failing_their_regime' backend/crates/runtime/qip-kernel/src/causal_review.rs backend/crates/services/qip-world-model/src/causal.rs`. A refutation outranks a stale hold, a regime neither set names reads as untested rather than as a refutation, and recording a failure deliberately leaves `last_updated` alone — a graph whose own edges are failing must not read as freshly refreshed to the degradation control that narrows sizing on staleness. `cargo test -p qip-world-model --test conditions` (10 tests) and `cargo test -p qip-kernel --test causal_precedence` (8). The layer informs and does not size: no path retracts an edge or reduces a strategy on a failed condition, which is §9.3's regime-break row and §9.4's retirement, both still open. |
| 9.2 | How Edges Are Established | PARTIAL | Two establishment paths exist now, not zero, and four of the six named methods remain absent. The generic one: `SupportingClaim` re-estimated inside `CAUSAL_GRAPH_HORIZON` — `grep -n 'pub fn reestimate\|pub struct SupportingClaim' backend/crates/services/qip-world-model/src/causal.rs` and its wrapper `grep -n 'fn absorb_causal_support' backend/crates/services/qip-world-model/src/world.rs` — still has only a test caller: `grep -rn 'absorb_causal_support' --include=*.rs backend/crates` hits only `qip-world-model/tests/understanding.rs`. **Granger-style lead-lag, the sixth of the section's named methods, is now built and reached in production (ADR 0054, 2026-09-12)**: `qip_numerics::stats::granger_causality` is the nested-OLS F-test (`grep -n 'pub fn granger_causality' backend/crates/libs/qip-numerics/src/stats.rs`), `qip_world_model::granger::establish_temporal_precedence` is the domain wrapper that turns a significant test into a `CausalEdge` or refuses (`grep -n 'pub fn establish_temporal_precedence' backend/crates/services/qip-world-model/src/granger.rs`), and `grep -n 'granger::establish_temporal_precedence' backend/crates/runtime/qip-kernel/src/platform.rs` is the non-test caller, inside `Platform::discover_temporal_precedence`. Natural experiments, instrumental variables, structural constraints and hypothesis-plus-falsification remain wholly absent — `grep -rni 'natural_experiment\|instrumental_variable' --include=*.rs backend/crates` returns nothing — and the platform's own order flow is still never fed back as evidence, which ADR 0054 names as blocked on `execution_nodes = {}` rather than on a decision. |
| 9.3 | What the Causal Graph Is Used For | PARTIAL | Three of five uses have a production read path, and two do not. Explanation: `grep -n 'let causal = world.causal()' backend/crates/agents/qip-investment-agents/src/reasoning.rs` — the tracing analyst calls `propagate` and otherwise returns `no_data`. Sizing narrowing on graph freshness: `grep -n 'CausalGraphFreshness::assess' backend/crates/runtime/qip-kernel/src/platform.rs` inside `central_degradation`, reached by `run_cycle → stage_decide → construct_from`. **Hidden concentration, new here.** This cell said the traversal was absent and cited a search as proof; the traversal has existed in `qip_world_model::exposure` and every caller was a test, which is a different verdict — `grep -rn 'hidden_concentration' --include=*.rs backend/crates \| grep -v '/tests/'`. It now has a production caller: `grep -n 'concentration: concentration(' backend/crates/runtime/qip-kernel/src/causal_review.rs` is reached from `grep -n 'fn causal_review' backend/crates/runtime/qip-kernel/src/platform.rs`, which `stage_understand` calls every cycle over the book the platform actually holds, and the finding is surfaced in the stage detail naming the driver and the positions it reaches — proven end to end through `run_cycle` by `a_book_whose_positions_share_an_unheld_causal_driver_has_it_surfaced_by_the_understand_stage` in `backend/crates/runtime/qip-kernel/tests/causal_precedence.rs`. `was_answerable` keeps a book with too few positions, or a graph with no edge, from reading as a clean one. **Still open, and each for a stated reason.** Regime-break survivability is now half-built: §9.1's conditions layer records which regimes an edge holds and fails in and the count failing the regime in force is reported, but no path reduces a strategy on it — how much to reduce, and on what evidence, is a sizing decision this row will not take by implication. Feature validation (cap the allocation of a feature with predictive power and no causal path) has no allocation seam to cap and would need the same kind of decision. Shock propagation still runs off factor betas rather than mechanisms: `grep -rn '\.propagate(' --include=*.rs backend/crates \| grep -v '/tests/'` reaches the read-only analyst and the `WorldModel` wrapper it calls through, and no stress path. |
| 9.4 | Honest Limits | PARTIAL | Three of the four handlings hold; the fourth is short on two clauses, each needing a decision rather than an implementation. **Two claims this row carried until 2026-09-15 were false and are corrected rather than quietly dropped**: it said edges are not conditioned on regime, and that unobserved confounders are recorded nowhere, citing a search that names ten files and over a hundred lines (`grep -rli 'confounder' --include=*.rs backend/crates`). Conditions are built: `grep -n 'holds_in\|fails_in\|fn in_regime\|fn failing_their_regime\|fn record_condition_failure' backend/crates/services/qip-world-model/src/causal.rs`, read back every UNDERSTAND stage by `grep -n 'let review = self.causal_review(now)' backend/crates/runtime/qip-kernel/src/platform.rs`. Confounders are recorded and change the edge's standing: `grep -n 'suspected_confounders\|pub enum EdgeStanding' backend/crates/services/qip-world-model/src/causal.rs` — a non-empty set makes the edge `Suggestive` whatever its p-value, which is this section's own instruction — and the audit over them has the same production caller. Edges carry a confidence and a decayed edge is marked rather than dropped or attenuated (`grep -n 'decayed_at\|pub struct DecayedEdge' backend/crates/services/qip-world-model/src/causal.rs`), and the graph constrains sizing rather than generating trades — `grep -rn '\.propagate(' --include=*.rs backend/crates \| grep -v '/tests/'` reaches the read-only tracing analyst and the world model's own shock path, and no code path turns an edge into an order. Short of `REACHED` on two clauses of the non-stationarity and discovery rows. **"Low-confidence edges inform exploration"**: the exploration budget exists and is production-reached (`grep -n 'exploration::review' backend/crates/runtime/qip-kernel/src/platform.rs`) — this row previously said no exploration path existed, which §13.2 already contradicted — but no edge feeds it, and the obvious feed does not survive reading. A probe candidate needs an uncertainty in `[0, 1]`, and `1 - confidence` is not one figure across the graph: a hand-asserted mechanism claim takes `CausalEdge::DEFAULT_CONFIDENCE`, a precedence edge takes `(1 - p)` under `TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING`, and one carrying an unobserved confounder half of that (`grep -n 'pub const DEFAULT_CONFIDENCE\|pub const TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING\|pub const TEMPORAL_PRECEDENCE_SUGGESTIVE_CEILING' backend/crates/services/qip-world-model/src/causal.rs backend/crates/services/qip-world-model/src/granger.rs`), so probes would be ranked by how an edge was established rather than by how uncertain it is — and none of the five `ProbeKind`s names a causal edge. Both are choices, not implementations. **"An edge that fails its conditions is retired, not patched"**: nothing retires one. `failing_their_regime` is counted and printed in the stage detail, and withdrawing a failing edge from `propagate`, `incoming` and `explanations` is a semantics change for every reader of the graph, which is an ADR. |
| 10.1 | What an Episode Is | REACHED | All seven blueprint fields are present and every one is written by the cycle, not by a test. The record: `grep -n 'pub struct Episode' backend/crates/libs/qip-ai/src/memory/episode.rs`. REASON writes the situation — `grep -n 'state: Some(self.market_state' backend/crates/runtime/qip-kernel/src/platform.rs` and `grep -n 'causal_context: self.causal_context' backend/crates/runtime/qip-kernel/src/platform.rs`, inside `record_precedent`, which `reason_about_the_queue` calls — from `grep -n 'fn market_state\|fn causal_context' backend/crates/runtime/qip-kernel/src/platform.rs`. LEARN writes what followed and what had been expected: `grep -n 'expected_move_bps: expected.get' backend/crates/runtime/qip-kernel/src/platform.rs`, inside `remember_resolved`, which `calibrate_resolved` calls and `stage_learn` calls in turn. `state_vector` is `MarketState` — drawdown, volatility ratio, spread ratio, windowed return, observation depth — and `causal_context` is the graph's own in-edges at the formation instant with their transmission. **Two divergences, both deliberate and both argued in the code.** `surprise` is derived rather than stored (`grep -n 'fn surprise_bps' backend/crates/libs/qip-ai/src/memory/episode.rs`): it is the difference of two numbers already on the record, and a third copy is a third thing that can disagree. The vector is 40 dimensions, not the section's illustrative "a few hundred" — that figure sizes storage, and padding to it would rank retrieval on dimensions nobody computed. The index buckets on the leading identity block only (`grep -n 'EPISODE_INDEX_DIMENSIONS' backend/crates/libs/qip-ai/src/memory/episode.rs`): state ranks, and never decides what is looked at, because bucketing on it made memory answer "no precedent" exactly when the tape had moved. Proven by `cargo test -p qip-ai --test episodic` and `cargo test -p qip-kernel --test episodic`. |
| 10.2 | What Gets Stored | PARTIAL | One trigger of the five is wired, and it is not one of the five as stated: an episode is written per reasoned opportunity and entered into the index only when its thesis resolves — `grep -n 'fn remember_resolved\|self.remember_resolved(' backend/crates/runtime/qip-kernel/src/platform.rs` (called from `calibrate_resolved`, itself called by `stage_learn`). **One sentence of this cell was wrong and is corrected: it said vetoes go to the counterfactual queue "rather than to memory". They go to both.** `record_precedent` runs on every arm of the decision, including `RejectedOnReview`, and `record_prediction` runs before the approval test, so a vetoed hypothesis still writes a claim that resolves and still carries its episode into the index — `grep -n 'DecisionTaken::RejectedOnReview' backend/crates/runtime/qip-kernel/src/platform.rs`, proven end to end by the `RejectedOnReview` assertion in `qip-kernel/tests/episodic.rs::an_episode_records_the_causal_edges_the_graph_held_into_its_instrument_when_it_was_reasoned`. The near-miss half is still absent (`grep -n 'self.declined.push' backend/crates/runtime/qip-kernel/src/platform.rs` — declined *orders* reach the counterfactual queue and no episode). The other three triggers remain absent and are **not** code-only: fills, quotes, cycles and transfers, regime transitions and calm samples are all situations with no claim, no panel, no decision and no horizon, and `Episode` requires all four. Storing one means either inventing a claim nobody made — the `MaxExpectedShortfall` failure, a record that reads as reasoning and is not — or making `Episode` a sum type, which changes the shipped slot-4 digest every cell reads (`grep -n 'fn derive' backend/crates/runtime/qip-kernel/src/central/episodic.rs`). That is an ADR, not an implementation. |
| 10.3 | Retrieval and Use | PARTIAL | Four of the five queries plus the regional digest are production-reached; the fifth needs an identity the platform does not hold when an episode is written. "What does now most resemble" and "what followed those situations": `grep -n 'fn recall_precedent\|self.episodes.recall(' backend/crates/runtime/qip-kernel/src/platform.rs` and `grep -n 'PrecedentDigest::of' backend/crates/runtime/qip-kernel/src/platform.rs`, feeding the panel brief via `brief_precedent`, and ranked on §10.1's state block and causal context rather than on the claim alone. "Was this surprising last time too" is the same digest taken across the analogues — `grep -n 'pub surprising\|pub worst_surprise_bps' backend/crates/libs/qip-ai/src/memory/store.rs` — recorded on every `HypothesisPrecedent` and served by `grep -n 'pub fn precedents' backend/crates/apps/qip-api/src/self_model_views.rs`; the largest surprise and not the mean, because §10.2 calls the rare one the informative one. "What did we decline in situations like this, and should we have" joins episodes to counterfactual scores on the hypothesis id both sides carry — `grep -n 'precedent_declines::join' backend/crates/runtime/qip-kernel/src/platform.rs` over `backend/crates/runtime/qip-kernel/src/precedent_declines.rs` — charged per control, recorded beside the precedent and evidence only, since §12.4 forbids a rule loosened from counterfactual evidence; proven end to end by `a_precedent_says_what_the_platform_declined_on_the_episodes_it_recalled_and_whether_it_should_have` in `backend/crates/runtime/qip-kernel/tests/episodic.rs`. The compact regional digest ships in production: `grep -n 'payload.episodic_digest = \|issue_episodic_digest' backend/crates/apps/qip-api/src/mesh.rs`. Missing: the strategy-filtered query, and not for want of code. An `Episode` is written in REASON, where the platform names no strategy; `StrategyId` is minted from a hypothesis id at settlement (`grep -n 'StrategyId::new(strategy.as_str())' backend/crates/runtime/qip-kernel/src/platform.rs`), so it is unique per episode and could never have faced anything before. Its feed — strategy-level confidence — already exists, keyed on the hypothesis class and fed by graded outcomes (`grep -n 'set_origin_factors' backend/crates/runtime/qip-kernel/src/platform.rs`), so an episodic estimate beside it would be a second claim about one fact rather than the query this row is short of. |
| 11.1 | What a Belief Is | PARTIAL | The belief object is `Hypothesis`, production-formed in REASON: `grep -n 'pub struct Hypothesis' backend/crates/services/qip-reasoning-engine/src/hypothesis.rs` and `grep -n 'self.reasoning.reason(SynthesisInput' backend/crates/runtime/qip-kernel/src/platform.rs`. It carries proposition (`statement`/`claim`), causal_path (`CausalChain`), evidence (`EvidenceSet`), confidence, and a TTL (`grep -n 'fn expires_at\|fn is_resolvable' backend/crates/services/qip-reasoning-engine/src/hypothesis.rs` — this cell cited `is_expired`, which the crate has never defined, so half the citation stood for nothing). Four of the six fields are met and two are not. It is **not** a distribution: `grep -n 'pub struct BeliefUpdate' -A 8 backend/crates/services/qip-reasoning-engine/src/bayes.rs` shows a scalar prior/posterior, and nothing in the update carries a second moment. Giving it one needs a concentration the platform does not measure, so it is left undone rather than chosen: a width nobody computed, sized against, would be the `MaxExpectedShortfall` failure with the sign reversed — a control that always fires on a number that means nothing. **Corrected 2026-09-15: this cell said three of the six proposition classes are "expressible", which counted the type and not its callers, and so overstated the platform under this document's own bar.** Expressible is indeed three — `grep -n 'pub enum Claim' -A 18 backend/crates/services/qip-reasoning-engine/src/hypothesis.rs` has no counterparty, capacity or valuation-range arm (the earlier cell cited that file by an elided path that cannot be run). *Produced* is two. `sed -n '/^fn mechanism_for/,/^}/p' backend/crates/runtime/qip-kernel/src/platform.rs | grep -o 'Claim::[A-Za-z]*' | sort -u` is the whole production enumeration — `grep -rn '[^A-Za-z]Claim::' --include=*.rs backend/crates | grep -v '/tests/'` names that one file and no other — and it prints six arms covering regime and relationship only: `Claim::EventOccurs` and `Claim::SpreadNarrows` have every caller in a test, so the event-outcome class is UNREACHED rather than met. The counterparty datum is measured and never believed: `grep -n 'pub struct FirmQuoteExtension' -A 10 backend/crates/edge/qip-routing/src/extension.rs` holds honoured-over-quoted as two integers and §33.1 gates path 6 on it, so a lane closing this row needs a producer, not the data. |
| 11.2 | Confidence Drives Size | REACHED | Confidence does reach size in production, through four independent narrowings, all inside `construct_from` on the DECIDE path: the hypothesis's own `effective_confidence` becomes the thesis conviction (`grep -n 'conviction: sign \* reasoned.hypothesis.effective_confidence' backend/crates/runtime/qip-kernel/src/platform.rs`), the §6.2 degradation multiplier (`grep -n 'central_sizing_multiplier' .../platform.rs`), the valuation mark's confidence (`grep -n 'fn sizing_confidence' .../platform.rs`), and, since ADR 0055, the instrument's own counterfactual record (`grep -n 'fn counterfactual_sizing_multiplier' .../platform.rs`) — folded into the same `sizing_confidence` call rather than a fifth call site, so a construction reads one number rather than reconciling several; see §12.3 for what that narrowing is and is not. **Corrected 2026-09-13 (ADR 0063): four narrowings plus one *bound*.** The instrument's own fill record narrows its weight bound rather than the budget — `grep -n 'fn sizing_cap_multiplier' backend/crates/runtime/qip-kernel/src/platform.rs`, read in `construct_from` into `construct_capped` — because a budget cannot name the instrument and a bound can; the proposal's `compromises` carry the number, and with the ADR 0055 narrowing active on the same name the position sized is a quarter of the unnarrowed one. The sharpest requirement is now met, and it was the last one. An absence of evidence and a conflict of evidence are different epistemic states and were sized identically, so a desk reading a small position could not tell whether the platform knew nothing or knew two things that disagreed. `EvidenceSet::net_stance_disagreement` measures the weaker side against the stronger over **independent** weight, so one contrary origin restating itself five times is still one dissent, and `EvidenceSet::posture` names which of the two a zero disagreement meant — an absence is not an empty set, since a stack of documents bearing on nothing is an absence and says so. Admission and size are deliberately separate: a thesis is admitted on what always admitted it and **sized on less**, because a platform that forms no view on a contested question stops learning about the cases it was least sure of. The narrowing is `1 - 0.6d`, which lies in `[0.4, 1]`, so a conflict can never size larger — arithmetic rather than a comment. Production seam: `grep -rn 'confidence_for_sizing()\|evidence_posture()' backend/crates/runtime/qip-kernel/src/platform.rs` (`thesis_from`'s conviction, and the REASON detail), reached by `run_cycle`. Two honest notes: a contested thesis now sizes up to 60% smaller and `conflicted` is the common path because the adversarial panel dissents by default; and the `0.6` is argued in its doc comment and has no ADR behind it. |
| 11.3 | Propagation to Regions | REACHED | The contract, the consumer and — since 2026-09-15 — the producer. `belief_priors` is a signed policy slot with a 300s TTL mapping to `Capability::BeliefState`, and a cell that reads it stale or absent falls to the conservative multiplier and reports it (`grep -n 'BeliefPriors\|fn narrowing' backend/crates/libs/qip-contracts/src/policy.rs`, `grep -n 'Capability::BeliefState' backend/crates/edge/qip-edge/src/telemetry.rs`). A cell forming no belief of its own is structural — no belief type is reachable from `qip-edge`. **The producer gap this row named is closed**: `grep -rn 'issue_belief_priors\|belief_priors = belief' --include=*.rs backend/crates \| grep -v '/tests/'` reaches `Platform::issue_belief_priors` and its one caller, `pending_policy`, which `POST /cycle` calls under the platform lock (`grep -n 'mesh::pending_policy' backend/crates/apps/qip-api/src/routes.rs`) and `MeshBackbone::dispatch_policy` signs. The map is keyed by instrument and valued by the effective confidence after review, taken from the drafts REASON forms and LEARN has not yet resolved — `Platform::pending_episodes`, which is the belief-per-subject-retained-past-the-cycle that `central/whitelist.rs`'s audit said would have to exist; that bullet stated its own refutation as a requirement and is amended in place rather than struck. **Read `qip-kernel/src/central/belief.rs` before touching this**, because a produced slot 3 doubles every receiving cell's size and four refusals are what bound that: nothing ships unless `BeliefState::last_updated` says a belief was formed in this process (the same fact `central_degradation` reads for the centre's own row 4, so the two halves cannot disagree); only beliefs inside the slot's own five-minute window are carried; the stamp is the **oldest** of those and never `now` and never the newest, so a centre that stops reasoning returns every cell to the halved multiplier inside five minutes; and a draft dated after the engine's own absorption, or after the issue instant, or carrying a confidence that is not a probability, is refused rather than shipped. Two honest limits, neither holding the row short. No cell reads the *values* — what reaches a decision is the slot's freshness, through `PolicyItem::capability` — and that is the same limit slots 4 and 11 carry. And this reaches no deployed process, because nothing is deployed: `grep -rn execution_nodes infrastructure/environments/*/terraform.tfvars` is `{}` everywhere. |
| 12.1 | What Gets Shadow-Executed | PARTIAL | One of the seven paths-not-taken is captured, in production: a risk-gate veto, recorded with the proposed trade at `grep -n 'self.declined.push(DeclinedPath' backend/crates/runtime/qip-kernel/src/platform.rs` inside `capture_submission`. Against each captured path the twin does evaluate alternative sizing, venue, region, hedge and delay (`grep -n 'pub enum Alternative' -A 14 backend/crates/services/qip-twin/src/counterfactual.rs`), which covers the "alternative sizing" row. A feasibility rejection on the desk is captured at the same site — a feasibility veto is a `Malformed` refusal naming its gate (`grep -n 'fn feasibility_gate' backend/crates/services/qip-execution-engine/src/oms.rs`) and, since `606fc1f` (ADR 0062), its venue (`grep -n 'venue: Option<String>' backend/crates/runtime/qip-kernel/src/platform.rs`), so this row's earlier claim that feasibility rejections were never queued was wrong about the desk; a cell's feasibility refusals reach the centre on the report but are not queued for the twin, which prices desk paths only. Profitability filters, un-whitelisted allocations and alternative cycle paths are never queued — `DeclinedPath` is constructed at exactly one site. **Corrected 2026-09-14: the clause "and the strategy that did not fire is never queued" was wrong, and wrong in the direction that understates the platform.** A strategy that did not fire *is* scored, by the deep brain's evolution round, as `HoldoutEvidence::holdout_returns` attached to every registered candidate — `grep -n 'fn register' backend/crates/runtime/qip-kernel/src/central/foundry.rs` and `grep -n 'foundry' backend/crates/apps/qip-deepbrain/src/evolution.rs`. What is true is the narrower claim: it is never queued **for the twin**, on a different schedule and outside `COUNTERFACTUALS_PER_CYCLE`. The row stays `PARTIAL`; the correction changes what a reader would go looking for, not the verdict. **Corrected 2026-09-15, again in the direction that understated the platform, and this time about two of the four rows a reader would go looking for.** The profitability filter is not missing from the platform, only from the desk: `grep -n 'GATE_FEE_FLOOR\|GATE_GAS_FLOOR' backend/crates/libs/qip-contracts/src/feasibility.rs` names the two rules that ask the profitability question, and they are the cell's; the central gate says in its own module doc why it does not carry them (`sed -n '/# Why this differs from the edge gate/,/^use /p' backend/crates/services/qip-execution-engine/src/feasibility.rs`). So a fee-floor refusal happens, reaches the centre and is counted, and is never queued for the twin — and on the desk nothing filters for profitability at all, which is why there is nothing there to queue. Likewise the alternative paths for a cycle are *recorded* in production and only unpriced: `grep -n 'pub struct PathAssignment' -A 6 backend/crates/edge/qip-routing/src/path.rs` keeps the whole eligible set beside the assigned path, and `grep -n 'path_assigned(' backend/crates/edge/qip-edge/src/cell.rs` writes it onto the cell's journal from `Cell::work`. Neither row is met — §12.1 asks for shadow *execution* and nothing prices an unassigned eligible path or a fee-floor refusal — but the candidate sets exist, and a lane that read this cell as "absent" would build a second copy beside the first. Four of the seven paths remain unqueued, so the verdict stands. |
| 12.2 | How It Is Scored | REACHED | All six steps run on the LEARN path: `grep -n 'fn score_declined\|self.score_declined(now)' backend/crates/runtime/qip-kernel/src/platform.rs` (called from `stage_learn`), which reconstructs from `bar_history` into a `TwinMarket` with a `CostModel` and `COUNTERFACTUAL_IMPACT_WINDOW`, calls `Platform::evaluate_alternatives`, evaluates over the intended horizon, attributes to the declining gate and accumulates into `declined_scores` plus gate-labelled counters (`grep -n 'names::COUNTERFACTUALS_SCORED\|names::COUNTERFACTUAL_REGRETS' .../platform.rs`). Narrower than stated in one respect: accumulation is per gate only, not also per venue, regime and strategy. |
| 12.3 | What It Changes | PARTIAL | **Re-scored 2026-09-13 (ADR 0061); the table has six rows** — `awk '/^12\.3 /{f=1} f&&/^12\.4 /{exit} f' docs/architecture/algorik-blueprint-v10.1-source.md` prints them, and this row now walks them one by one rather than folding the three rule rows into one, which is how it came to say "four" until 2026-09-13. Underneath all three rule rows: a refusal is charged to the *rule* by the breach the checker wrote, never by a word in the sentence — `grep -n 'fn rule_names' backend/crates/services/qip-execution-engine/src/oms.rs`, `qip_rule_fired_total{rule}` beside `qip_orders_refused_total{control}` from the same refusal (`grep -n 'names::RULE_FIRED' backend/crates/runtime/qip-kernel/src/platform.rs`). **R1, a rule vetoes mostly profitable paths → recalibrated: built as a proposal with a governed manual enactment, and never an automatic change.** `grep -n 'fn review_rules\|fn approve_recalibration' backend/crates/runtime/qip-kernel/src/platform.rs` and `grep -n 'pub fn new' backend/crates/runtime/qip-kernel/src/rule_review.rs`: the LEARN stage proposes from the per-rule accumulation at the two bars sizing already uses, journals it under `risk.rule_recalibration`, withdraws it when the evidence stops clearing the bar; the proposal's one constructor refuses a bound that does not loosen; two operator signatures (`POST /risk/recalibrations/:rule/approvals`, cloned from the promotion approval) emit `RecalibrationApprovalEntry::artefact` — `LimitSet::rebound`, the running set with one bound replaced — and the process keeps the limits it booted with (`enactment_emits_an_artefact_and_leaves_the_running_limits_untouched`). The only door is the file: `grep -n 'fn load_risk_limits' backend/crates/apps/qip-api/src/main.rs backend/crates/apps/qip-fastbrain/src/main.rs backend/crates/apps/qip-deepbrain/src/main.rs` (three roots), mounted from `risk_limits_file` (`grep -n risk_limits_file infrastructure/terraform/catalogue.tf`, all three workloads), null in every environment; `security.rs::no_code_path_assigns_a_limit_set_after_boot…` refuses a setter. **R2, vetoes mostly losing paths → earning its place: built as a record** — `RuleDefence` under `risk.rule_defended`, `qip_rule_defended_total{rule}`, with the simulated loss avoided. **R3, almost never fires → dead weight: built as a record** — `RuleDormant` under `risk.rule_dormant`, `qip_rule_dormant{rule}`, at `RULE_DORMANCY_CYCLES` and `RULE_DORMANCY_MIN_ORDERS` (`grep -n 'RULE_DORMANCY' backend/crates/runtime/qip-kernel/src/rule_review.rs`, one hundred each, argued in ADR 0061). **R4, feasibility rejections cluster on one venue → venue withdrawn: reached, since `c759519` (ADR 0062) and the edge closure of 2026-09-14; `PARTIAL` until then, and the sentences that said so are amended in place rather than deleted.** A feasibility refusal names its venue at both seams — the desk broker's name in `capture_submission`, the intent's venue joined onto the delta by `Cell::state_delta` and carried on `CellReport.refusals` — into one rate window under `qip_feasibility_refusals_total{venue,constraint}` (`grep -n 'fn record_feasibility_refusal\|fn attribute_refusals' backend/crates/runtime/qip-kernel/src/platform.rs backend/crates/runtime/qip-kernel/src/central/plane.rs`; a cell's refusal is admitted only under one of `qip_contracts::feasibility::EDGE_GATES` at a venue the policy or a live grant names, else counted `unknown`/`other`). `Platform::review_venues` in LEARN (`grep -n 'fn review_venues\|fn withdraw_venue\|fn reinstate_venue' backend/crates/runtime/qip-kernel/src/platform.rs`) withdraws the venue that dominates three in four of a window of ten — ADR 0055's bars by reference, `grep -n 'VENUE_WITHDRAWAL' backend/crates/runtime/qip-kernel/src/venue_review.rs` — journaling `venue.withdrawn` *before* withdrawing, at both seams by omission: `OrderManager::submit` step 5 refuses under `VenueUnavailable` independent of `is_simulated` (`an_order_to_a_withdrawn_venue_is_refused_as_unavailable_even_though_the_venue_is_simulated`), and `cycle_whitelist_for` `retain`s by parsed venue with the omission named on the journaled issue (`a_withdrawn_venue_is_omitted_from_the_whitelist_and_the_omission_is_on_the_record`). Reinstatement needs two distinct fresh operators, each signature journaled under `venue.reinstated` before anything changes (`reinstatement_needs_two_different_fresh_operators_and_is_journaled_at_each_signature`), and can only remove a name from a subtractive set (`a_window_dominated_by_a_venue_the_policy_does_not_name_changes_no_whitelist`); the fill-error diagnostic withdraws nothing (`a_twin_that_is_wildly_wrong_about_fills_never_withdraws_a_venue`). **The edge limit is closed as of 2026-09-14, and what it was is kept here because the shape of the closure is only legible beside it.** It read, until that date: an installed desk keeps its graph until the node restarts — `Cell::install_arbitrage` refuses a second desk and policy slot 11 has no producer — so omission from the whitelist takes effect only for a desk installed *after* the withdrawal. Slot 11 now has a producer: `CentralPlane::feasibility_constraints`, through `Platform::feasibility_constraints`, assigned at the shipping seam in `pending_policy` (`grep -n 'payload.feasibility_constraints =' backend/crates/apps/qip-api/src/mesh.rs`; `grep -n 'fn feasibility_constraints' backend/crates/runtime/qip-kernel/src/central/plane.rs backend/crates/runtime/qip-kernel/src/platform.rs`). It carries `withdrawn_venues` — the same field `cycle_whitelist_for` retains against, written only after the `venue.withdrawn` record is in the log — and three **empty** grid maps, because the centre's grids are keyed by instrument and the slot by venue and `feasibility::effective` prefers a slot grid to the cell's own; `the_slot_the_centre_ships_carries_the_withdrawn_set_it_applies_and_states_no_grid` fails if a producer ever starts filling one. `qip_edge::feasibility::assess` refuses a withdrawn venue under `qip_contracts::feasibility::GATE_WITHDRAWN_VENUE` before every rule that asks a question about the order, so the withdrawal reaches an installed desk on its **next pass**: `a_desk_installed_before_a_withdrawal_stops_trading_the_withdrawn_venue_on_its_next_pass` places three legs, has a second install refused, applies the withdrawal, places none, and asserts the desk still holds its three edges — a pass-time refusal, not a teardown. The ninth `EDGE_GATES` member is admitted by `attribute_refusals` so it charts under its real venue and gate rather than `other`, and was, from 2026-09-14 until later the same day, **kept out of the window entirely** through `is_withdrawal_evidence`. That argument was half right and the remedy was wrong, and an independent security review found the other half within hours: an installed desk reports one such refusal per intent per pass, so admitting every echo does evict a 256-entry window within a few passes — but excluding every echo removes the withdrawn venue from the **denominator** every other venue's share is measured against, so the runner-up's share climbs on a shrinking divisor until it too reaches three in four, and the next, until the desk has no venue. Both readings end in a control that cannot fire; the second reads as protection while causing the cascade the first was written to prevent. What is built now (ADR 0062 Amendment C) splits counting from judging: `qip_contracts::feasibility::is_withdrawal_echo(gate, venue, withdrawn)` consults the **centre's own** withdrawn set rather than believing a gate string from an uplink that authenticates nobody, `attribute_refusals` seats the first echo per venue per report in the window and counts every repeat without a seat, and `VenueTally::weight` is `refusals + echoes.min(refusals)` — an echo may sustain a withdrawn venue's weight up to the genuine evidence it still holds and never amplify it, decaying to nothing as those refusals age out. A withdrawn venue is never a candidate, so no echo is ever a numerator. Proven by `a_repeated_echo_of_one_withdrawal_is_counted_in_full_and_seated_once`, `a_venue_withdrawn_on_edge_evidence_stays_in_the_denominator_the_runner_up_is_judged_against` and `a_cell_citing_a_withdrawal_the_centre_does_not_hold_is_evidence_and_not_an_echo`, each mutation-verified from both sides — echoes weighing nothing restores the cascade, echoes weighing uncapped lets a withdrawal amplify its own evidence. The set can only subtract, and the three structural guards that make adding impossible are untouched: `graph_from_whitelist` against `QIP_VENUES`, `Cell::install_arbitrage` against `self.config.venues` (asserted with a payload applied, `a_policy_payload_cannot_make_a_venue_this_cell_is_not_configured_for_reachable`), and `whitelist_for` under `envelope.permits_venue`. Two residuals, neither holding R4 short and both stated rather than left to be found: the way back is two signatures through `Platform::reinstate_venue` — this row read "no `qip-api` route exposes reinstatement" until 2026-09-14, when one did, and see the correction at the end of this row for what that route can actually do now; and a cell that has never received *any* payload naming the withdrawal keeps its graph, exactly as it keeps a whitelist it never received — ADR 0008's "cells decide alone", an availability limit on the centre's reach and not a boundary. A security review found a single, unauthenticated cell could clear the withdrawal bar alone — the cell→centre uplink authenticates nobody — with no corroboration required; `venue_review::assess` now admits edge-only evidence only when it names at least `VENUE_WITHDRAWAL_MIN_CELLS` (two) distinct cells, or when the desk's own single-source evidence (accepted alone by design) is present (`grep -n VENUE_WITHDRAWAL_MIN_CELLS backend/crates/runtime/qip-kernel/src/venue_review.rs`); `ten_refusals_from_one_cell_do_not_withdraw_the_only_policy_venue` proves the single-cell case is refused and `ten_cell_refusals_from_two_distinct_cells_at_the_only_policy_venue_withdraw_it_and_the_whitelist_says_so` proves corroboration still works. Reinstatement is exposed since 2026-09-14 (ADR 0062 Amendment B): `POST /venues/:venue/reinstatements` at the operator role and `GET /venues/withdrawals` at the viewer role (`grep -n 'venues/:venue/reinstatements\|venues/withdrawals' backend/crates/apps/qip-api/src/routes.rs`, ); this row read "No `qip-api` route exposes reinstatement" until then. **The route exists and refuses everyone, deliberately, since ADR 0065 later the same day**, and the test that drove both signatures over HTTP is gone with the capability: `no_pair_of_operators_can_put_a_venue_back_while_the_credential_attests_nobody` and `a_reinstatement_is_refused_before_the_kernel_is_asked_because_nothing_attests_a_person` are what stand in `qip-api/tests/venues.rs` now. A security review found the fifteen-minute operator-freshness window on this route and six others measured **process uptime**: `qip-api`'s composition root reads `QIP_TOKEN_OPERATOR` once at start-up and stamped that instant as the credential's `issued_at`, so a leaked token of any age read as fresh for fifteen minutes after every restart and every operator was refused for the rest of the process's life. A standing bearer token has no authentication instant, and the platform now says so structurally rather than fabricating one — `Presence` carries the absence, `Principal::authentication_instant` returns a refusal naming what would be required, and seven routes answer 403 until a per-request proof of recency exists. So R4's way back is, today, a route nobody can complete — recorded here rather than left for an operator to discover with a desk stopped, and see `docs/operations/reinstating-a-venue.md`. The desk-side refusal is also structured since the same date (Amendment A): `RefusalReason::Infeasible { venue, gate, detail }` replaces the `Malformed` variant whose detail string carried an `infeasible (<gate>):` prefix that `feasibility_gate()` parsed back out (`grep -n 'Infeasible {' backend/crates/services/qip-execution-engine/src/oms.rs`), and `capture_submission` reads the venue off that field rather than re-deriving it from `self.broker.name()` — the same value, from the record of the refusal rather than from a broker the capture site happened to hold (`the_venue_a_desk_feasibility_refusal_is_charged_to_is_the_one_the_refusal_carries`).

**R5, an allocator objective revised: absent** — no declined path is attributed to a strategy or family anywhere in `DeclinedPath`/`DeclinedScore`; ADR 0055's argument stands. **Measured and not revised since ADR 0064; the `absent` clause above is kept verbatim because it is still true of the thing it is about.** What ADR 0055 did not reach is that *underperforms* need not be measured from declined paths: every candidate the foundry registers carries the family whose sweep produced it, its holdout evidence and a ledger rung. `grep -n 'fn review_family_allocation\|fn family_standings' backend/crates/runtime/qip-kernel/src/platform.rs` and `grep -n 'pub fn standings\|pub fn misallocation' backend/crates/runtime/qip-kernel/src/family_review.rs`: the LEARN stage groups the registered population by family, scores each on the member mean of `observed - expected_maximum` — the gate's own deflation, deliberately not `DeflatedSharpe::observed`, which is the *undeflated* Sharpe — journals a `FamilyAllocationReview` under `learning.family_allocation_reviewed`, and raises a `MisallocationFinding` where an unfunded family with at least `FAMILY_REVIEW_MIN_MEMBERS` evaluated members stands `FAMILY_REVIEW_MARGIN` above the *best* funded family. **No weight moves, and the guarantee is the absence of the code path.** Three blockers, each by command rather than by assertion: `grep -rn '\.issue(' backend/crates --include=*.rs` finds the only writer of a capital envelope called from tests alone; `grep -rn 'set_proposal\|family_horizons' backend/crates --include=*.rs` finds the allocator's proposal writer reached only from `central/learning.rs::resize` (which mutates a proposal that must already exist) and the family-budget function reached from nothing outside its own suite; and the correlation calendar `family_structure` reads is fed from grants and so depends on the first. A `FamilyCap` on any of them would be a control that cannot fire — the `MaxExpectedShortfall` shape. A fourth fact, found while implementing and worth more than the other three: `HoldoutGate::deflated` resolves its count from `StrategyEvidence::trial_account`, and `grep -rn 'with_trial_account\|submit_evidence' backend/crates --include=*.rs` shows **no candidate ever carries one** — `charge_holdout_trials` charges it into a `Cow` that is dropped and `submit_evidence` has no caller — so the count is resolved from `TrialBook::lifetime_trials` instead and `the_learn_stage_reviews_family_allocation_on_a_population_the_foundry_actually_registered` asserts a family becomes *admitted* once it has been in front of the gate, because a review that refused every member for ever would be the same defect one layer down. `qip_family_standings{standing}` is written every cycle including when both arms are zero (the `qip_rule_dormant` discipline), and that zero is the deliverable: it is the series in which "this row has never had a subject" is a fact rather than a silence, and in which the day the row becomes buildable is visible. `security.rs::every_shipped_function_that_moves_a_capital_weight_is_one_of_the_reviewed_ones` walks every shipped `fn` in the workspace and refuses any whose body moves a capital weight unless it is on an explicit reviewed list carrying the reason it is not a family finding reaching a weight, and `no_function_exported_by_the_family_review_returns_a_type_outside_its_reviewed_list` refuses an exported return type off that module's allow-list. **Both halves of that scan were rebuilt on 2026-09-14 after two independent reviews demonstrated it bypassed, by execution rather than by reading.** It detected only whole-field reassignment (`self.proposals = …`), so a shipped `Platform::discount_family` calling `self.central.set_proposal(…)` — the writer ADR 0064 itself names as the allocator's reachable one — passed it; and it examined a method only when the method's *name* contained "famil", so `apply_misallocation(&mut self, finding: &MisallocationFinding)` was never looked at. The return check was a deny-list of `Decimal`/`Money`, which any newtype over one walks past. Round three detected call-shaped mutation and identified methods by parameter type as well as name; an independent security review then bypassed *that* by compiling two weight-movers — a one-hop delegation whose family-named entry point writes nothing and a neutrally named method the precondition never looks at — and the test stayed green. **The fourth round stopped narrowing the heuristic and deleted it.** The discriminator was never the detector: `self.central.set_proposal(p)` is the same bytes inside a method that took a family finding and inside the allocator's own legitimate resize, so no predicate over a method's name or parameters can tell them apart — the identical conclusion a five-round chain in `qip-transport` reached about redacting credentials, recorded in ADR 0057. The scan now reads no `impl` marker, no holder list and no family test: it walks every shipped `fn` in the workspace and refuses any whose body moves a capital weight unless it appears on an explicit reviewed list, each row carrying the reason it is not a family finding reaching a weight. It fails closed, it refuses more than its predecessor — four of its eleven rows have nothing to do with families — and both reviewer probes are caught, the delegation at its helper. The claim moved with it: 'the guarantee is the absence of a code path' appeared four times across the test, the module doc and ADR 0064 and is gone from all four, because it was never true of a text scan. The test says so itself now — it does not claim a write is wrong, it claims nobody has written down why it is right. Four limits remain open and ADR 0064 names them: a call through a trait object or function pointer, a weight in a field the scan does not name, a macro-generated call, and whether any reviewed site is in fact fed by a family finding. This is the second time this repository has shipped an acceptance scan that matched names where the thing it guarded was structural — `28857ed` was the first — and the failure message now says so. The trial count each family is deflated against is `TrialBook::lifetime_trials`, the family's whole search **as of the review**, and deliberately not the per-member snapshot `HoldoutGate::charged_trials` resolves to; the two coincide only for the member charged last, and the shipped bit-equality test was pinned at the single-member arity where they agree. A consequence worth stating rather than discovering: a member's contribution is therefore not stationary, so two `FamilyAllocationReview` records over identical evidence at different cycles legitimately differ — each is a statement about the search as it stood on its own cycle, which is why the record carries one. The evidence bar is on *evaluated* members and always was in this row; it was the implementation that counted registrations, admitting a finding built on one readable observation while reporting ten as its sample size. The document was right and the code was not, which is the less common direction here and worth the sentence. **R5 is not `reached` and is deliberately not scored as such**: the row asks for a revision and this is a measurement with the revision refused in writing (ADR 0064). **R6, a sizing function adjusted: reached for the executed-order half since `f6f5db9` (ADR 0063), and the loosening direction is a proposal only.** The declined half is as ADR 0055 left it — `Platform::counterfactual_sizing_multiplier` can only narrow (`a_pattern_of_wrongly_declined_paths_never_widens_sizing`). The executed half is scored since `dd64c0a`: `score_filled` prices every fill in LEARN under the one per-cycle cap (`grep -n 'fn score_filled\|self.score_filled(' backend/crates/runtime/qip-kernel/src/platform.rs`), and each `FillScore` says whether the smaller or the larger size beat the twin's own `trade` arm. Its consequence is a *bound*: `sizing_review::cap_multiplier` (`grep -n 'SIZING_CAP_' backend/crates/runtime/qip-kernel/src/sizing_review.rs`, ADR 0055's bars by reference, no branch above one) halves the instrument's weight bound when three in four of ten fills favoured the smaller size, `construct_from` passes it to `PortfolioConstructor::construct_capped` (`grep -n 'fn construct_capped' backend/crates/services/qip-portfolio-engine/src/construction.rs`; refuses a cap outside `(0, 1]`, floors the bound at the minimum position, lowers the budget equality so the shortfall is recorded and not reallocated), and the proposal's `compromises` name the instrument and the numbers. With ADR 0055 active on the same name the position sized is a quarter (`a_declined_pattern_and_a_fill_pattern_on_one_instrument_compound_a_halved_budget_with_a_halved_bound`), on disjoint evidence (the extended ADR 0055 learning test holds both numbers at exactly one half). The larger direction is a `SizingProposal` under `learning.sizing_reviewed` and nothing else — no multiplier on the record, no reader of it produces one (`a_larger_size_pattern_journals_a_sizing_proposal_and_leaves_every_bound_where_it_was`); the arming and the proposal are journaled at the change, not per cycle. A code review found `smaller_favoured` fired on *any* fill that lost money before costs at all — direction alone, not size, since a smaller loss is trivially closer to zero than a larger one of the same shape regardless of whether the size was the problem. `score_filled` now also requires the alternative's cost advantage over the trade — recovered from the twin's own cost-model breakdown, `gross = simulated_pnl() + simulated_costs()` — to be at least as large in magnitude as its directional disadvantage before counting `smaller_favoured`; `a_fill_that_lost_purely_to_an_adverse_price_move_does_not_favour_the_smaller_size` is the regression test. `larger_favoured` is deliberately left on the plain comparison — the same gate is mathematically unsatisfiable for it, argued in ADR 0063's "what would make this wrong". A second, unrelated finding: `capture_submission` queued every refusal for the twin regardless of kind, so a withdrawn venue's administrative refusals (`RefusalReason::VenueUnavailable`) were polluting the same instrument's ADR 0055 declined-path evidence; `RefusalReason::is_sizing_evidence` (`qip-execution-engine::oms`) now excludes it and the platform's other posture refusals from the declined queue, holding only `Malformed` and `RiskRejected` as sizing evidence (`ten_refusals_purely_for_a_venue_withdrawal_do_not_move_the_instruments_sizing_confidence`). Stays `PARTIAL`, and on one row now rather than two: three rule rows built as findings with one governed manual enactment, the venue row reached — withdrawn at the centre and refused at pass time by every cell, including one whose desk was installed before the withdrawal — the sizing row reached on both halves with its loosening direction a proposal, and the allocator row still absent, which is what holds the section. |
| 12.4 | Guardrails | REACHED | All four hold since ADR 0077; three of four until then, and two until `dd64c0a`. Impact is charged and oversized counterfactuals are refused rather than priced: `grep -n 'Unfillable' backend/crates/services/qip-twin/src/counterfactual.rs` ("more of the day's volume than the impact law is calibrated for"). **"Never loosened automatically" no longer holds trivially** (corrected 2026-09-12, ADR 0055): §12.3 now has a real automatic consumer of counterfactual evidence, `Platform::counterfactual_sizing_multiplier`, and the guarantee is held structurally rather than by the absence of anything to check — the function has no branch that returns more than `Decimal::ONE`, proved by `platform::counterfactual_sizing_tests::a_pattern_of_wrongly_declined_paths_never_widens_sizing`, which feeds it an overwhelmingly *favourable* pattern (the "rule vetoes mostly profitable paths" case this guardrail names) and asserts sizing confidence stays at one. **Corrected again 2026-09-13 (ADR 0061): a governed *manual* loosening path now exists** — a platform-generated proposal, two signatures, a file the deployment mounts — so "never loosened automatically" is no longer held by the absence of a consumer but by the absence of a code path: `Platform` has no setter for a limit set, and `security.rs::no_code_path_assigns_a_limit_set_after_boot_and_no_root_reads_one_from_anywhere_but_its_configuration` refuses a `&mut self` method on any of the five holder types whose own *name* names a limit, whose *parameter list* names one of the four types that carry a set, or whose *body* assigns `self.monitor` or `self.orders` directly — corrected 2026-09-13 after a security review found the scan matched only the method's name, so `pub fn adopt(&mut self, bounds: LimitSet) { self.monitor = RiskMonitor::new(bounds, …) }` passed it outright and the guarantee this row claimed was actually held by `PreTradeChecker` and `RiskMonitor`'s private fields, not the scan; the proposal's constructor refuses a non-loosening bound; the route body cannot carry a bound; and the only door is `QIP_RISK_LIMITS_PATH`, read once at boot and, since the same date, re-validated on every resume rather than trusted from the log outright. **Three of four hold since `dd64c0a`: fill-simulation error against actual fills on the same venue is tracked** — `qip_venue_fill_error_bps{venue}` (`grep -n 'VENUE_FILL_ERROR_BPS' backend/crates/libs/qip-observability/src/metrics.rs backend/crates/runtime/qip-kernel/src/platform.rs`), a histogram on `Histogram::signed_basis_points` recorded in `score_filled`, the LEARN pass that prices the sizes not taken on every fill under the same per-cycle cap as the declined paths; `(simulated − actual) / actual × 10⁴` on entry prices, negated for a sell so negative always means the twin filled better than reality on the side taken, the direction that flatters (`the_twins_entry_price_error_against_the_actual_fill_is_recorded_per_venue`). Diagnostic and read by nothing that decides — a venue is withdrawn on feasibility evidence alone (`a_twin_that_is_wildly_wrong_about_fills_never_withdraws_a_venue`, ADR 0062). Entry prices only: the twin reports a round-trip cost and the venue an entry cost, and netting one against the other would be a number nobody computed. The `smaller_size`/`larger_size` arms are now read (ADR 0063): the smaller side arms a bound that only narrows, and the larger side is a journaled proposal with no multiplier on it — "never loosened automatically" covers the sizing row by the same structure as the rule rows (`a_larger_size_pattern_journals_a_sizing_proposal_and_leaves_every_bound_where_it_was`). **The fourth row closed 2026-09-15 (ADR 0077): counterfactual findings now enter a real statistical gate with the platform's own trial accounting.** ADR 0055's bar — ten scored paths, three quarters of them one way — is a fixed threshold, and a fixed threshold is a test the platform may re-run at no cost: `review_rules` runs it on every rule in the boot limit set on every LEARN stage, against a 256-entry window that turns over as scores land, so eight-of-ten (about one in nineteen under a coin) arrives often and used to open a proposal to loosen a risk rule on it. `qip_kernel::counterfactual_trial` is the gate: a one-sided binomial against `p₀ = 0.5` — the weakest null available, because a stronger one would be a figure nobody here has measured — with the continuity correction, through `qip_numerics::distributions::normal_cdf`, against a Bonferroni bar of `0.05` divided by the trials the counterfactual family has been charged this calendar quarter, this finding's own included. **Charged before judged**, because a failed test that cost nothing is how "look until one clears" survives an accounting that bills only what passed. Two production seams, both of them the loosening direction, located by `grep -n 'self.counterfactual_trial(' backend/crates/runtime/qip-kernel/src/platform.rs` — the recalibration proposal in `review_rules` (§12.3's first row, the only counterfactual finding in this tree that can end in a loosened risk bound, via ADR 0061's two signatures) and the larger-size proposal in `review_sizing` (ADR 0063); both are called by `stage_learn` on every `run_cycle`, so the path is the same one `qip-api`, `qip-fastbrain` and `qip-deepbrain` run. The gate both refuses and admits, proven at the seam rather than against the arithmetic: `platform::counterfactual_trial_seam_tests::a_rule_whose_regret_clears_the_fixed_bar_on_ten_paths_is_refused_by_the_trial_gate` (eight of ten — what ADR 0055 admitted — is refused, and the test asserts `is_too_tight()` first so it is about the gate and not about a sample that never qualified) beside `a_rule_whose_regret_is_unanimous_across_twelve_paths_is_admitted_and_proposed`, and `the_same_unanimous_evidence_proposes_nothing_on_a_platform_that_has_already_looked_a_hundred_times` for the correction itself. **Shared book, separate budget** (the residual doubt this row's triage raised, decided): the book is shared because two `TrialBook`s over one store would replay each other's records and append at colliding sequences, which fails `TrialBook::verify` as tampering, and because a second store would need a composition root none of the three has; the budget is separate because `TrialBook` budgets per *family* per quarter and `qip_lifecycle::trials::COUNTERFACTUAL_FAMILY` is reserved — `TrialBook::enrol` refuses a strategy into it and a counterfactual subject into any other family (`neither_kind_of_subject_can_be_enrolled_in_the_others_family`, `a_quarter_of_counterfactual_trials_leaves_a_strategy_familys_budget_untouched`), so a quarter of rule reviews cannot spend the budget a sweep needs and a sweep cannot leave a finding untestable. Quarterly and not lifetime deliberately: lifetime drives the bar to zero, and a gate that can only refuse is `MaxExpectedShortfall` with the sign reversed. Fail-closed at both seams — no proposal, nothing journaled, the bound unmoved — and a budget spent is charted under its own `uncharged` arm of `qip_counterfactual_trials_total{outcome}` rather than reading as a quarter in which nothing was found. **Two findings are deliberately outside the gate and this is the row's one honest caveat**: `Platform::counterfactual_sizing_multiplier` and `sizing_review::cap_multiplier` can only narrow, so a gate in front of either would be a statistical test whose *refusal makes the platform trade larger* — the one thing this section's fourth row forbids — and the rule defence records a rule earning its place, moves nothing, and is re-derived on every new score, so charging it would spend on restatement the budget the proposals need. "Never loosened automatically" is unweakened: nothing added returns a `Decimal` at all, the change can only subtract a proposal, and `security.rs::no_code_path_assigns_a_limit_set_after_boot_and_no_root_reads_one_from_anywhere_but_its_configuration` still passes. |
| 13.1 | What the Self-Model Tracks | PARTIAL | Two of the seven dimensions are built and production-fed. Estimator reliability and calibration: `grep -n 'pub struct Capability' -A 12 backend/crates/services/qip-learning-engine/src/self_model.rs` (accuracy, hit rate, mean Brier, sample count), absorbed in production at `grep -n 'self.self_model.absorb(evaluation' backend/crates/runtime/qip-kernel/src/platform.rs` inside `learn_from`, reached from `calibrate_resolved` ← `stage_learn`. Model age is readable from `last_updated` and is what `SelfModelFreshness::assess` narrows sizing on. Coverage, capacity, regime experience and blind spots are absent: `grep -rni 'blind_spot\|coverage\|capacity' backend/crates/services/qip-learning-engine/src/` returns only `Vec::with_capacity`. |
| 13.2 | The Exploration Budget | PARTIAL | The budget now has something to spend and a ceiling that could ever be above zero; it had neither, and the reason was worse than the gap this row used to describe. `Mandate::desk` pinned `exploration_share` to zero and `MandateRegistry::register` refuses a user share above the desk's, so no mandate anywhere in a running platform could carry a nonzero share: a term validated at construction, refused on the way back in, rendered in the console, and impossible to set — a budget that cannot be spent is the `MaxExpectedShortfall` defect in another costume. The share is now configuration, zero by default because exploration capital is capital withheld from return-seeking sizing: `grep -n 'exploration_share' backend/crates/runtime/qip-kernel/src/config.rs`. Four of the section's asks now exist — the five probe kinds, a deterministic upper-confidence-bound selection over each candidate's uncertainty weighted by the value of resolving it, a per-probe ceiling of a quarter of the budget that declines and names an oversized candidate rather than resizing it behind the caller's back, and an account separating what exploration committed from what it spent: `grep -n 'pub enum ProbeKind\|MAXIMUM_PROBE_SHARE\|pub struct ExplorationBook\|pub fn plan' backend/crates/services/qip-capital/src/exploration.rs`. UCB and not Thompson on purpose: nothing in that crate draws a random number or reads a clock, and a sampler would make this the one capital allocation a replay could not reproduce (`two_runs_over_the_same_evidence_select_the_same_probes_in_the_same_order`). It reaches production through one call in DECIDE that holds the budget out of the reservation ledger before anything is sized, so `deployable_capital` sizes the book against what is left — `grep -n 'exploration::review' backend/crates/runtime/qip-kernel/src/platform.rs` — and the idle state is loud rather than silent, `qip_exploration_budget` being written as zero on every pass instead of appearing only once it is positive. Proven by `the_live_probes_can_never_commit_more_than_the_budget`, `an_exploration_share_is_withheld_from_the_capital_the_book_is_sized_against`, `the_configured_desk_share_is_the_ceiling_user_mandates_are_admitted_under` and `a_platform_that_sets_no_share_aside_still_reports_the_budget_it_did_not_take`. Short of `REACHED` on the thing the section is named for: **no probe is ever executed.** A `Probe` carries a subject, a bound and an expiry and has no instrument, side or quantity, and nothing converts one into an order — `grep -rn 'ProbeKind\|ExplorationBook\|ProbeOutcome' --include=*.rs backend/crates/services/qip-execution-engine backend/crates/edge backend/crates/services/qip-brokers` returns nothing. So every settlement production can make is `ProbeEvidence::Observed`, a subject that resolved while the probe sat unexercised, which the book keeps deliberately apart from `Probed` so it never reports that probing works on evidence that no probing happened (`a_probe_nobody_takes_up_settles_as_observation_and_never_as_evidence_that_probing_works`). Two of the five kinds, `UnfamiliarVenue` and `RegimeBoundary`, are unfed on purpose: nothing counts orders per venue and nothing marks a regime transition, so either would be a probe sized against a figure nobody computed. |
| 14.1 | What a Hypothesis Is | REACHED | Every field of the blueprint record exists on a type produced in production. `grep -n 'pub struct Hypothesis' -A 50 backend/crates/services/qip-reasoning-engine/src/hypothesis.rs` gives proposition (`claim`/`statement`), mechanism (`CausalChain` of `CausalStep`, each naming a `Mechanism`), evidence, and `falsifiers`; the prediction that follows is the resolution proposition (`grep -n 'ResolutionCriteria' backend/crates/runtime/qip-kernel/src/platform.rs`). Status is the six-state enum `grep -n 'pub enum HypothesisStatus' -A 14 .../hypothesis.rs`. Produced on the REASON path at `grep -n 'self.reasoning.reason(SynthesisInput' backend/crates/runtime/qip-kernel/src/platform.rs`, and a directional finding with no falsifier is refused (`grep -n 'unfalsifiable claim cannot be reviewed' backend/crates/libs/qip-agents/src/finding.rs`). |
| 14.2 | Sources | PARTIAL | Still exactly one source wired, and it is not on the blueprint's list: a DISCOVER-stage anomaly becomes the hypothesis's single causal step — `grep -n 'fn synthesise' backend/crates/runtime/qip-kernel/src/platform.rs` (the chain is built from `anomaly.detector`). What changed is that the gap is now stated rather than inferable, which is the failure the lane names: a hypothesis carried no recorded provenance, so a platform implementing one source off a list of six read exactly like one implementing all six. `SourceCensus` enumerates all seven — the six the section names and the detected-anomaly source this platform actually has, which is not one of them — and every unwired source carries the reason it is unwired rather than being a bare absence: `grep -n 'pub const SOURCES\|fn unwired_reason\|pub enum SourceStanding' backend/crates/services/qip-world-model/src/falsification.rs`. It is built on the LEARN path every cycle and prints a line on a cycle where nothing proposed anything — `grep -n 'SourceCensus::new\|census.record' backend/crates/runtime/qip-kernel/src/falsification.rs` — because a census that went quiet for want of a subject would reproduce, one layer out, the exact defect it was built to end, and on a fresh deployment that is the only state anyone would ever see. `a_source_census_enumerates_every_blueprint_source_and_says_which_are_unwired` pins the enumeration and `a_pass_that_tested_nothing_still_describes_itself` pins the idle line. The provenance the census records is structural rather than guessed: every prediction reaches it through `Platform::synthesise`, which returns `Ok(None)` without an anomaly, so no other source is claimed. All six named sources remain unbuilt, for the reasons the census itself gives: the causal graph's gaps cannot be walked, high-surprise episodes are not stored (§10.1), counterfactual anomalies feed nothing (§12.3), no world-model traversal proposes exposures, no cross-asset transfer exists, and the language model deliberately does not propose — `grep -n 'No analyst asks a language model' backend/crates/agents/qip-investment-agents/src/analysts.rs`. Recording that a source is unwired is not wiring it, and this row stays `PARTIAL` on that arithmetic: one of seven. |
| 14.3 | The Path from Hypothesis to Capital | PARTIAL | The spine exists in production: proposed → red-team review → approved → expressed as a thesis → sized. `grep -n 'fn clears_action_bar\|fn meets_action_bar' backend/crates/services/qip-reasoning-engine/src/{engine,hypothesis}.rs` and the thesis construction at `grep -n 'fn construct_from' backend/crates/runtime/qip-kernel/src/platform.rs`. **The `Testing` gate is no longer inert.** A falsifier was written down at formation and evaluated by nothing, which reads downstream exactly like a falsifier that passed — worse than an absent gate, because an absent gate is visible. It is now evaluated against genuinely held-out data once per cycle from `stage_learn`: `grep -n 'pub fn review' backend/crates/runtime/qip-kernel/src/falsification.rs` is the pass and `grep -n 'self.falsification' backend/crates/runtime/qip-kernel/src/platform.rs` the one call in `stage_learn` — deleting that line turns all three kernel tests red. The defect to fear in that fix is point-in-time leakage, so the boundary is the substance: admissibility is decided on the **knowable** instant and never the valid one, which is the only way a bar whose bucket opened before a claim and closed after it is held out while a restatement valid last March and published this morning is not — `grep -n 'pub struct HeldOut\|fn partition\|pub fn rolling_statistic\|pub struct LeakageTally' backend/crates/services/qip-world-model/src/falsification.rs`. Four things are refused rather than filtered, and a sample containing any of them is refused whole, because a sample that silently shrinks is a leak that already happened and left no trace; a caller holding a mixed history partitions first and reports the tally, so dropping an in-sample row is a counted act. A derived statistic carries the *latest* availability in its window rather than its last element's — the leak no per-record filter can catch, and the reason this reaches production at all, since a volatility claim is settled by a statistic over twenty-one closes and not by a close. Proven by `a_record_knowable_before_the_hypothesis_was_formed_is_refused_as_in_sample`, `a_bar_that_opened_before_formation_and_closed_after_it_is_held_out_data`, `a_rolling_statistic_over_held_out_records_is_itself_held_out`, `a_rolling_statistic_is_knowable_only_when_its_latest_input_is`, `a_non_finite_observation_is_refused_rather_than_reported_as_survival` (a value that compares false against every threshold would otherwise report survival on evidence that says nothing), `a_falsifier_that_could_report_survival_on_no_evidence_at_all_is_refused`, and end to end by `a_claim_is_never_tested_against_closes_that_were_knowable_before_it_was_formed` and `a_statistic_derived_only_from_closes_knowable_after_a_claim_was_formed_can_refute_it`. Trials are charged per family and a spent budget returns `BudgetExhausted` rather than another test; a new family at the bound is refused rather than evicted, because evicting hands it a fresh budget and makes the cap unlimited (`a_family_that_has_spent_its_trial_budget_is_refused_a_further_test`, `a_ledger_that_cannot_track_another_family_refuses_rather_than_forgetting_one`). Short of `REACHED` on three counts, each deliberate. The gate promotes nothing, sizes nothing and moves no capital: `Platform::falsification` is read by an operator and by tests and by nothing that decides (`grep -n 'pub const fn falsification' backend/crates/runtime/qip-kernel/src/platform.rs`). `ThesisOutcome::falsifiers_triggered` is still constructed empty on the production path — `grep -n 'falsifiers_triggered' backend/crates/runtime/qip-kernel/src/platform.rs` — so the verdict lives in the book beside the thesis rather than on it. And the rows after `Testing` are unbuilt: a supported hypothesis still never becomes a candidate causal edge (the hypothesis-plus-falsification method §9.2 names, unrelated to the temporal-precedence writer §9.2 gained on 2026-09-12 under ADR 0054, which reads return history), and there is still no canary — `grep -rni 'canary' --include=*.rs backend/crates \| grep -v '/tests/'` now returns two lines and both are doc comments saying it is not built. The family trial budget in the factory (`grep -n 'TrialBook' backend/crates/runtime/qip-kernel/src/central/factory.rs`) still counts strategy sweeps, not hypotheses. |
| 15.1 | Meta-Learning | PARTIAL | The regime-keyed scoreboard is built, tested and now consulted: `grep -n 'pub struct Scoreboard\|pub enum ScoreDomain' backend/crates/services/qip-evolution/src/scoring.rs` scores strategies, models and regimes by context with an evidence-weighted shrink toward a prior, and `qip-deepbrain`'s `SuccessionDesk::judge` keeps its own board, reading a subject-regime pairing's precedent before recording this comparison's outcome into it and refusing to crown a win the deterministic test just returned when that precedent is *established* (the board's own bar for enough evidence) and below the prior: `grep -rn 'Scoreboard' --include=*.rs backend/crates \| grep -v 'qip-evolution/src\|qip-evolution/tests'` now finds the desk's field, its accessor and three tests exercising the consultation. Bootstrapping a fresh regime is unaffected — a pairing with no evidence, or too little to be established, changes nothing. Of the other four capabilities — feature generality, warm starts, cross-asset transfer, hyperparameter learning — none is built; `grep -rni 'warm_start\|cross_asset' --include=*.rs backend/crates` returns nothing, which is why this row is `PARTIAL` and not `REACHED`. | **Re-scored `UNREACHED` → `PARTIAL` on 2026-09-08: the scoreboard now has a production caller.** `grep -n 'fn score_claims_by_regime' backend/crates/runtime/qip-kernel/src/platform.rs` is written from `calibrate_resolved`, which `stage_learn` calls every cycle, at the one instant the platform knows whether a claim held — so the board is filled where the fact becomes known rather than reconstructed later. Two design points are load-bearing and both are pinned by mutation-verified tests in `cargo test -p qip-kernel --test learning`. The subject is the claim's **class**, not its hypothesis id: a hypothesis resolves once and never recurs, so a board keyed on it would hold one observation per cell for ever, every score pinned at its prior, every band `Unproven` — a meta-learner that learns nothing. And the context carries both regime axes together (`market/volatility`), because a claim that works in a calm trend and fails in a volatile one is the conditional fact the board exists to keep and scoring the axes separately averages it away. A claim stating no direction is not scored at all rather than scored as a failure, because "went well" is whether the move came out on the side the claim named. Cardinality is bounded by construction — classes are the anomaly kinds the detectors raise, contexts the product of two regime enums — so the board does not grow with uptime, and it is deliberately not replayed: it summarises resolutions the event log already holds, and a second durable copy of a derived fact is a second source of truth for it. **`PARTIAL` and not `REACHED`, for the reason the left column gives:** one of §15.1's five capabilities now runs. Feature generality, warm starts, cross-asset transfer and hyperparameter learning are still not built. A second honest limit: `Platform::claim_scores` is read by an operator and by nothing else — nothing sizes or gates on a score, and making meta-learning steer allocation is a behavioural change that wants an ADR, not a wire. |
| 15.2 | Adversarial Modelling | PARTIAL | None of the five questions is answered and none of the four responses is implemented. There is no flow classification, no counterparty adaptation detection, no crowding correlation, no fingerprint randomisation and no toxic-flow widening: `grep -rni 'toxic\|crowding\|informed flow\|fingerprint' --include=*.rs backend/crates/edge backend/crates/services/qip-execution-engine \| grep -v '/tests/'` returns nothing. What exists is the symptom the section says version 8.0 already had — an adverse-move estimate in `grep -n 'expected adverse move' backend/crates/edge/qip-arbitrage/src/netedge.rs` — plus an empty transport shell: the `AdversaryProfiles` policy slot is declared (`grep -n 'AdversaryProfiles' backend/crates/libs/qip-contracts/src/policy.rs`) and never produced by anything. **Re-scored 2026-09-14, `ABSENT` → `PARTIAL`.** `adversary_review` reads the twin's per-fill `trade_error_bps` and produces a per-venue posture over a bounded window; `Adapting` is the only *finding* and compares a venue's recent half against its **own** early half rather than a level, because the cost model is constant across the window and cancels in the difference — a level bar would list every venue the day the model turned out generous. Wired at two sites: the `stage_learn` fold and slot twelve of the policy payload, §41.5's last and previously unproduced. Producing it changes the payload digest and **no behaviour**: the slot maps to no §6.2 capability and no cell reads it. **Most of §15.2 is deliberately not built and will not be.** Fingerprint randomisation exists to defeat a counterparty's detection of this platform; there is no counterparty, and the softer framing of the same code as making our own backtests less predictable was refused as the same code wearing a different justification. Flow classification and crowding correlation need a tape with an aggressor side, and the simulated broker's orders never enter the tape. Toxic-flow widening needs a spread of our own, and the platform does not quote. |
| 15.3 | Market Simulation | PARTIAL | All five agent types are built, calibrated and tested: `grep -n 'pub fn passive\|pub fn informed\|pub fn momentum\|pub fn competitor\|pub fn maker' backend/crates/services/qip-simulation-engine/src/agents.rs`, driven through a reactive market by `grep -n 'pub fn with_agents' backend/crates/services/qip-simulation-engine/src/market.rs`. **A production caller now exists.** `qip-kernel`'s `Platform::capacity_probe`, called from `stage_simulate` every cycle with enough price history, drives one of the five stated uses — capacity discovery, chosen because it needs no learning loop, no factor-crowding model and no fault-injection harness, only the market this crate already builds and one plausible order against it: `grep -rn 'with_agents(' --include=*.rs backend/crates` now finds the call in `qip-kernel/src/platform.rs` beside the crate's own tests. The other four stated uses — tactic learning, crowding stress, failure rehearsal, and the continuous predicted-vs-actual impact calibration the section separately asks for — still do not run, which is why this is `PARTIAL` and not `REACHED`. | **Re-scored `UNREACHED` → `PARTIAL` on 2026-09-08: one of the five uses now runs in a production binary, and what it found is the honest headline.** ADR 0059 settles what the counterparty panel is and what its verdict may do; `grep -n 'fn measure_crowding' backend/crates/apps/qip-deepbrain/src/evolution.rs` is the caller, reached from the evolution round after a candidate produces holdout evidence, and `backend/crates/services/qip-simulation-engine/src/crowding.rs` is the machinery — a `WeightFollower` wearing `SimStrategy` over a `BacktestStrategy`, so a candidate written for the backtester can meet conditions only the book-based simulator can express. The candidate's **own tape** is replayed twice with the same bars, seed, costs and program, the only difference being whether the five agents are attached; a fresh synthetic path would report the edge's disappearance and blame the counterparties for it. **Two findings came out of wiring it, and both are recorded rather than smoothed.** First, the panel was sized off the liquidity profile's `average_daily_volume` while `MarketSimulator::replay` fills the book from the bars' own volume — two claims about one fact, and a fixture whose profile overclaimed tenfold produced a crowded run losing half the capital, which reads as a devastating crowding finding and was an arithmetic error about which number to trust. It is sized off the tape now, with a test that fails if that is reverted. Second, **on the committed tape the check currently measures nothing**: the panel places thousands of orders a round and every comparison returns a cost of zero to the last digit, because the book's displayed depth at the touch far outlasts the panel's clips. That is ADR 0059's own "the panel being so small it changes nothing", observed; the round counts it (`crowding_unmoved` beside `crowding_uncontested` — a panel that did not show up and a panel whose takers did not bite are different facts that look identical in the cost), and the response is to count it rather than enlarge the panel until the number moves. `PARTIAL` for the reason the left column gives: four of the five uses — tactic learning, impact calibration, capacity discovery, failure rehearsal — still do not run, and the predicted-vs-actual calibration is still not tracked. And the measurement **decides nothing**: `FlowCalibration` has one arm, `NotCalibrated`, so gating a promotion on it would be a control firing confidently on invented inputs — the mirror of `MaxExpectedShortfall` and the worse of the two, because it fails closed and silently. |
| 16.1 | Six Engines | PARTIAL | Five of six have a production call path; the sixth is a recorded refusal. Term structure and credit: `grep -n 'CreditRegister::from_universe' backend/crates/runtime/qip-kernel/src/platform.rs` (universe assembly), which builds the per-currency `TermStructure` at `grep -n 'TermStructure::new' backend/crates/runtime/qip-kernel/src/valuation.rs`. Illiquid valuation: `grep -n 'IlliquidValuator::mark_object' backend/crates/runtime/qip-kernel/src/platform.rs` inside `private_holdings_of`, called from assembly. Cashflow and commitments: `grep -n 'Commitment::from_private_asset' .../platform.rs` and `grep -n 'self.commitments.unfunded_total' .../platform.rs` in `deployable_capital`. Corporate actions: `grep -n 'fn apply_due_corporate_actions\|self.apply_due_corporate_actions()' .../platform.rs`, called from `stage_sense`. The volatility surface is the sixth and is **BLOCKED, not absent** — it is built and tested but deliberately has no caller until an option-quote source clears a licensing evaluation (ADR 0050): `grep -n 'This engine has no caller, and that is a recorded decision' backend/crates/libs/qip-market/src/volatility.rs`, and `grep -rn 'VolatilitySurface' --include=*.rs backend/crates \| grep -v '/tests/' \| grep -v 'qip-market/src'` returns only a doc reference. |
| 16.2 | Valuation Carries Method and Confidence | REACHED | `grep -n 'pub struct AssetValuation' -A 10 backend/crates/libs/qip-financial/src/valuation.rs` carries asset, value, method, inputs (each with its own confidence), confidence, `as_of` and `next_review` — every field the record names except that `value` is one `Decimal` rather than a range. It is production-struck by `IlliquidValuator::mark_object` at universe assembly (see §16.1) and the method-borne confidence is what narrows sizing: `grep -n 'fn sizing_confidence' backend/crates/runtime/qip-kernel/src/platform.rs`, consumed in `construct_from`. |
| 16.3 | Illiquid Valuation Methods | REACHED | All seven methods are typed with a strict confidence ordering: `grep -n 'pub enum ValuationMethod' -A 16 backend/crates/libs/qip-financial/src/valuation.rs`, with constructors `from_quote`, `from_comparables`, `from_discounted_cashflow`, `from_last_round`, `at_cost` (`grep -n 'pub fn from_\|pub fn at_cost' .../valuation.rs`). Marks decay with age rather than being treated as equally true: `grep -n 'fn confidence_at\|fn is_stale' .../valuation.rs`, and that decayed confidence reaches sizing through `Platform::sizing_confidence` and the risk envelope, so an uncertain mark cannot silently support leverage. |
| 16.4 | Commitments and Capital Calls | PARTIAL | The hard half is REACHED: an unfunded commitment is a first-class draw that comes off free capital before anything is sized, and exceeding it is a refusal, not a clamp — `grep -n 'fn deployable_capital' -A 25 backend/crates/runtime/qip-kernel/src/platform.rs` ("a called commitment that cannot be met forfeits the position"), fed by `grep -n 'Commitment::from_private_asset' .../platform.rs`. **Re-scored 2026-09-15, and the first correction is against this row rather than against the code.** The row claimed `present_value` had no production caller, and that was false when it was written: the grep it offered as proof searched only `j_curve_trough` and `expected_demand_within`, and the row then drew a conclusion about a third name nobody had looked for. A claim of absence is worth exactly what the search behind it covers. The chain is `CashflowForecast::present_value` ← `IlliquidValuator::from_discounted_cashflow` ← `mark_private_asset` ← `mark_object` ← `private_holdings_of` ← `Platform::new`, and `grep -rn 'present_value(\|mark_object(' --include=*.rs backend/crates \| grep -v '/tests/' \| grep -v 'fn present_value\|fn mark_object'` names both production ends of it. The second correction is real change: the forecast now reaches the liquidity ladder. A private holding's `days_to_exit` is the later of the catalogue's `days_to_liquidate`, the rung's bucket floor, and the lockup the holding's own forecast is dated from — `grep -n 'fn exit_days_with_forecast' backend/crates/runtime/qip-kernel/src/platform.rs` is the arithmetic and `grep -n 'exit_days_with_forecast(reference.days_to_liquidate' .../platform.rs` the one call, taken at universe assembly where both figures are first in hand. `max` and never a replacement, because a catalogue stating a *longer* exit than the lockup knows something the lockup does not — a gated manager, a secondary that has stopped bidding — and letting a forecast shorten a stated exit is the failure `LiquidationHorizon::least_days` already caused once, a holding stated at forty-five days read as two and compared against a ten-day limit that recorded nothing. So the wire can only make the liquidity read worse, which is the direction a control is allowed to move on its own. On the fixture a holding moves from 365 days to 1,765, leaves `MinLiquidity`'s numerator and becomes able to breach `MaxDaysToLiquidate` (`cargo test -p qip-kernel --test valuation_plane`, `a_private_fund_inside_its_lockup_is_not_counted_as_exitable_on_the_catalogue_s_word`). **`PARTIAL`, and the two functions still uncalled are uncalled deliberately — the reasons belong here so nobody wires them as a tidy-up.** `expected_demand_within` cannot tighten `deployable_capital`: `Commitment::with_forecast` refuses a schedule whose outflows exceed the unfunded balance, so the demand within any horizon is at most the unfunded balance, while `deployable_capital` already charges the whole unfunded total. Wiring it could only *lower* the reserve — the `MaxExpectedShortfall` failure inverted, not a control that cannot fire but a control rewired to fire less. `j_curve_trough` fires only on a forecast carrying dated capital calls, and nothing in this platform dates one: `PrivateAssetDetails` holds a vintage year, a lockup in years, committed/called/distributed/residual capital and a capital-call notice period, and none of those dates a call, so the trough is always absent. Closing that is a data-model change, not a wiring change, and anyone who reaches for the wire will find the arithmetic already correct and still useless. One landmine for whoever does it: `Commitment::with_forecast` and `Commitment::forecast()` remain test-only — `grep -rn '\.with_forecast(' --include=*.rs backend/crates/libs/qip-financial` reaches that crate's tests and nowhere else — and the no-forecast fallback is the conservative one, the whole unfunded balance. Attach a distribution-only forecast and the demand within a horizon becomes zero instead, which is the reserve silently vanishing at the moment a schedule was added to improve it. |
| 17.1 | Continuously Priced | PARTIAL | The instrument layer covers the table: `grep -n 'pub enum Extension' -A 22 backend/crates/libs/qip-financial/src/extensions.rs` has Bond, Rate, Fx, Future, Equity, CreditDerivative, Index and Digital arms, and `grep -n 'pub enum AssetClass' -A 14 backend/crates/libs/qip-financial/src/asset_class.rs` names thirteen classes. The prerequisites the table cites are met for bonds and equities (term structure, credit, corporate actions — §16.1) but not for listed options, whose stated requirement is the volatility surface (BLOCKED, §16.1). Execution reaches one class in fact: the node runs a pass only under `QIP_VENUE_FEED=simulated` (`grep -n 'QIP_VENUE_FEED' backend/crates/apps/qip-edge-node/src/main.rs`), so "reachable now" is a claim about the design, not about a wired venue. |
| 17.2 | Event and Information Driven | PARTIAL | Prediction-market machinery is real and production-wired: `grep -n 'pub struct ResolutionSource\|pub fn evaluate' backend/crates/services/qip-prediction/src/resolution.rs`, consumed by the kernel at `grep -n 'ResolutionCriteria::Threshold' backend/crates/runtime/qip-kernel/src/platform.rs` inside `calibrate_resolved`, so an event proposition is scored against published observations on the LEARN path. The remaining four rows have nothing: `grep -rni 'catastrophe\|insurance.linked\|carbon\|freight\|additionality' --include=*.rs backend/crates \| grep -v '/tests/'` returns nothing, and the world model and base-rate machinery the first row requires are the empty causal graph of §9. |
| 17.3 | Illiquid and Private | PARTIAL | Two of six rows have the machinery the table names, production-reached: private equity/venture and real estate through `Extension::PrivateAsset` / `Extension::RealAsset` marked by `IlliquidValuator::mark_object` with commitments booked (`grep -n 'Extension::PrivateAsset' backend/crates/runtime/qip-kernel/src/platform.rs`), and private credit through the credit register's per-obligor profiles (`grep -n 'CreditRegister::from_universe' .../platform.rs`, with `Extension::Loan` and `StructuredCredit` typed). Covenant tracking, borrower monitoring, royalties, litigation finance and art provenance are absent: `grep -rni 'covenant\|royalt\|litigation\|provenance' --include=*.rs backend/crates \| grep -v '/tests/'` returns nothing beyond the `covenant_state` field name in the credit extension. |
| 17.4 | Physical | PARTIAL | None of the three rows is built. The commodity extension records storage cost and convenience yield as carry inputs (`grep -n 'pub struct CommodityDetails' -A 10 backend/crates/libs/qip-financial/src/extensions.rs`) but there is no logistics engine, no shipping/customs/spoilage cost model and no auction bidding or winner's-curse adjustment: `grep -rni 'logistics\|winner.*curse\|spoilage\|customs' --include=*.rs backend/crates` returns nothing, and every `auction` hit is a venue session or print type (`grep -rn 'Auction' --include=*.rs backend/crates/libs/qip-contracts/src/venue.rs`). **Re-scored 2026-09-14 (ADR 0072), `ABSENT` → `PARTIAL`.** Landed cost through freight, customs, storage, spoilage, returns and marketplace fees, with `PhysicalArbitrage` and `delivered_price`. Every rate carries its source: `LogisticsTerms` has no `Default` and `Spoilage::new` refuses a blank source, so no rate enters without saying where it came from. Rows one and two have a real cost model with **no ingestion behind it and no production caller**; row three (auctions) is untouched, because a winner's-curse adjustment needs a distribution of rival bids and nothing here has one. |
| 17.5 | Private Markets Are a Second Operating Mode | PARTIAL | Four of the six consequence rows are structurally honoured in production. A position has a mark with a method and confidence, not a price (§16.2/§16.3). Unfunded commitments reserve capital indefinitely and are refused rather than clamped (`grep -n 'fn deployable_capital' backend/crates/runtime/qip-kernel/src/platform.rs`). The liquidity ladder models the lockup explicitly and is built on the risk path — `grep -n 'fn liquidity_ladder\|self.liquidity_ladder(figures)' .../platform.rs`, reached from `risk_state_from` ← `risk_state`, with `grep -n 'pub enum Rung\|fn classify' backend/crates/libs/qip-financial/src/ladder.rs` placing private credit and real assets on their own rung. And the shared substrate is real: one ledger, one capital engine, one frontend. Not met: reconciliation against a quarterly statement has no separate cadence (the reconciliation path is the venue one), and the risk envelope has no manager, vintage or duration dimension — `grep -rni 'manager_risk\|vintage_risk' --include=*.rs backend/crates` returns nothing. |
| 17.6 | Out of Scope, and Why | NARRATIVE | A table of exclusions and their reasons — physical delivery, unlicensed activity, manipulation-adjacent strategies, unvaluable assets. No deliverable. The one adjacent enforced fact (an object with no defensible mark is not decision-grade) is scored under 17.7: `grep -n 'fn not_decision_grade' backend/crates/libs/qip-financial/src/universe.rs`. |
| 17.7 | The Asset Class Registry | PARTIAL | Built and reached: a per-*instrument* admission gate, not a per-*class* registry — `grep -n 'not_decision_grade' backend/crates/runtime/qip-kernel/src/platform.rs` shows `Platform::new` refusing objects on licensing, price, coherence and quality. Absent: the registry record and its nine fields (valuation engine, settlement convention, tick/lot, hours, margin regime, corporate-action applicability, tax class, eligible families, hedge instruments) — `grep -n 'pub enum AssetClass' backend/crates/libs/qip-financial/src/asset_class.rs` finds a bare enum with three predicates and no record type, and no code refuses a trade because a class is unregistered. |
| 17.8 | Settlement Calendars | PARTIAL | Built and reached: `grep -n 'SettlementCalendar::weekday' backend/crates/runtime/qip-kernel/src/platform.rs` — `Platform::new` builds the pre-positioner's T+1 calendar; `grep -n 'fn is_settlement_day\|fn quote' backend/crates/services/qip-capital-fabric/src/settlement.rs` is the holiday-aware cycle arithmetic. Absent in production: holidays are never loaded — `grep -rn 'with_holiday(' backend/crates --include=*.rs` outside `/tests/` finds only the two definitions, so every deployed calendar is plain weekdays. No per-market/per-jurisdiction calendar table, no half-days, no roll/expiry dates, and no holiday-versus-outage distinction. |
| 18.1 | The Feasibility Gate | PARTIAL | Reached on the cell's hot path: `grep -n 'feasibility::assess' backend/crates/edge/qip-edge/src/cell.rs` (in `Cell::work`) and `grep -n 'feasibility::assess_cycle_cost' backend/crates/edge/qip-edge/src/cell.rs`. Five of the seven checks are built and fire — `grep -n 'GATE_MINIMUM_QUANTITY\|GATE_MINIMUM_NOTIONAL\|GATE_LOT\|GATE_TICK\|GATE_DEPTH\|GATE_FEE_FLOOR\|GATE_GAS_FLOOR' backend/crates/edge/qip-edge/src/feasibility.rs`. Absent: withdrawal economics (deliberately — no withdrawal path exists: `grep -n 'withdrawal' backend/crates/services/qip-capital/src/ledger/entitlement.rs`) and settlement viability (no settlement check in `assess`). Resolution is per-VENUE and prefers the policy payload over the cell's grid (`grep -n 'fn effective' backend/crates/edge/qip-edge/src/feasibility.rs`); the centre ships grids per INSTRUMENT (`grep -n 'with_instrument_feasibility' backend/crates/runtime/qip-kernel/src/platform.rs`), and `grep -rn 'with_venue_feasibility' backend/crates --include=*.rs` finds no composition root. |
| 18.2 | The Arithmetic, Stated Plainly | NARRATIVE | A cost table at $200 capital and the argument that two hundred dollars is a correctness harness rather than an engine. No deliverable to build. |
| 18.3 | Where Small Capital Genuinely Wins | NARRATIVE | Five arenas and what each needs from v9. Rationale for where to point the platform, not a testable artefact; each named dependency (world model, market making, feasibility gate, valuation plane) is scored in its own section. |
| 18.4 | Compounding Policy | PARTIAL | No reinvestment cadence, threshold-crossing planner, withdrawal-drag display or minimum-viable-scale computation: `grep -rn 'reinvest\|Reinvest\|withdrawal_drag\|minimum_viable_scale' backend/crates --include=*.rs` returns nothing. The one piece that exists is the fee-tier ladder, and it is unreached: `grep -rn 'FeeTier' backend/crates --include=*.rs` finds the definition in `edge/qip-routing/src/venue.rs`, a re-export, and test callers only — nothing accumulates trailing volume toward a tier. |PARTIAL | **Re-scored 2026-09-14 (ADR 0066).** `CompoundingPolicy`, `ReinvestmentDecision`, `ThresholdLadder` and a bounded `FeeVolumeLedger` are built, all money `Decimal`, reached through the cadence fold above. `PARTIAL` for two stated gaps: **`FeeTier` is still unreached** — the ladder lives in `qip-edge/qip-routing` and nothing outside that crate composes a `FeeSchedule`, so closing it needs an owner of that crate or an ADR for a service-to-edge dependency — and withdrawal drag and minimum viable scale are **not built at all**, because both need a forward growth rate the platform does not measure and inventing one would be a second claim about a fact nothing establishes. **Re-scored 2026-09-14 (ADR 0066) to `PARTIAL`.** `CompoundingPolicy`, `ReinvestmentDecision`, `ThresholdLadder` and a bounded `FeeVolumeLedger` are built, all money `Decimal`, reached through the cadence fold above. Two stated gaps hold it short: **`FeeTier` is still unreached** — the ladder lives in `qip-edge/qip-routing` and nothing outside that crate composes a `FeeSchedule`, so closing it needs an owner of that crate or an ADR for a service-to-edge dependency — and withdrawal drag and minimum viable scale are **not built at all**, because both need a forward growth rate the platform does not measure and inventing one would be a second claim about a fact nothing establishes. |
| 19.1 | How Ten Thousand Is Reached | PARTIAL | The section's testable claim is that the platform accounts in effective breadth rather than strategy count. The measure is built and now read: `grep -rn 'effective_bets' backend/crates --include=*.rs` finds `libs/qip-risk/src/factor.rs`, `libs/qip-risk/tests/risk.rs`, and `qip-kernel`'s `Platform::construct_from`, which decomposes the same covariance and target weights DECIDE just sized against — a zero-factor model, each instrument's own historical variance standing as its entire "specific" risk, because no common factor model is estimated at this seam and a decomposition that invented loadings would be a number nobody computed — and publishes the result on `qip_portfolio_effective_bets`. The 10x15x12x6 decomposition itself is still arithmetic on a page — `grep -rn 'variants\|parameterisation' backend/crates --include=*.rs` finds no universe enumeration — which is why this is `PARTIAL` and not `REACHED`. | **Shared root cause, established 2026-09-08:** this is downstream of one missing wire, not its own gap. `qip_risk::metrics::beta` estimates a beta from returns against a benchmark and has no production caller at all (`grep -rn '::beta(' backend/crates --include=*.rs | grep -v qaoa` returns nothing outside tests), so `factor_betas` is constructed empty at both production sites — `grep -n 'factor_betas: BTreeMap::new()' backend/crates/runtime/qip-kernel/src/{platform.rs,central/plane.rs}`. With no betas the platform has no factor model, and everything that consumes one is starved: this row, §23.7's `StressTester` (whose `apply` reports every position as `unmodelled` without betas), and the factor half of `qip-learning-engine`'s attribution. Wiring `beta` needs a declared benchmark — an equal-weighted universe return is the obvious candidate — which is a modelling decision this document cannot make: it wants an ADR, not a patch. **The wire exists as of 2026-09-08.** ADR 0058 settles the benchmark and `qip_risk::market_factor::MarketFactor` implements it: `grep -n 'MarketFactor::estimate' backend/crates/runtime/qip-kernel/src/platform.rs` is the production caller, in `attribute`, which populates `factor_betas` and `factor_returns` from the platform's own tape where an instrument has enough overlap and leaves both empty — *unmodelled*, never zero — where it does not. | **Re-scored `UNREACHED` → `PARTIAL` on 2026-09-08.** `effective_bets` now has a production caller and real contributions to report over. `grep -n 'fn decompose_risk' backend/crates/runtime/qip-kernel/src/platform.rs` builds a single-factor `FactorRisk` from the same `MarketFactor` estimate the stress test used — exposures, residual variances and the factor's own variance, all from one fit, so the two halves of the report describe one model — and decomposes it at the book's own weights. `grep -n 'fn stage_simulate' backend/crates/runtime/qip-kernel/src/platform.rs` is the caller; `cargo test -p qip-kernel --test stress` drives a cycle and asserts the breadth figure is positive over a real contribution rather than the zero an empty contribution map returns. `PARTIAL` and not `REACHED` because only half the section is closed: the measure is now computed on live positions, and the 10x15x12x6 decomposition it is meant to account against is still arithmetic on a page — nothing enumerates a strategy universe. An honest second limit: the breadth is over the *modelled* positions only, and a position whose instrument has too short an overlap is excluded and counted (`StressReport::unmodelled_positions`) rather than folded in at zero.
| 19.2 | Evaluation Tiers | PARTIAL | No tier assignment, no per-tier cadence, no hot-tier cap and no p99 evaluation budget: `grep -rn 'EvaluationTier\|enum Tier\b\|hot_tier\|pinned' backend/crates --include=*.rs` returns nothing (`qip-cost-router`'s `TierVerdict` is the model-rung router, a different subject). `grep -n 'pub fn with_budget' backend/crates/edge/qip-strategy/src/runtime.rs` is a per-run instruction budget, not a tier schedule. |PARTIAL | **Re-scored 2026-09-14 (ADR 0066).** Five tiers, per-tier cadence and `HOT_TIER_CAP` are built and reached through the cadence fold above (same grep, same pinning test). `PARTIAL` and not `REACHED` for one honest reason stated in the ADR: with today's family names nothing parses to an alpha family, so every strategy tiers to `Batch` and **the hot-tier cap cannot fire**. It is reachable through an operator's string rather than permanently dead, which is what distinguishes it from `MaxExpectedShortfall` — but a control that fires on no input this platform currently produces is not delivered, and this row does not say it is. **Re-scored 2026-09-14 (ADR 0066) to `PARTIAL`, not `REACHED`.** Five tiers, per-tier cadence and `HOT_TIER_CAP` are built and reached through the cadence fold above. The reason it is not `REACHED`: with today's family names nothing parses to an alpha family, so every strategy tiers to `Batch` and **the hot-tier cap cannot fire**. It is reachable through an operator's string rather than permanently dead, which is what separates it from `MaxExpectedShortfall` — but a control firing on no input this platform currently produces is not delivered, and this row does not say it is. |
| 20.1 | The Statistical Gate | PARTIAL | Reached: cumulative trial accounting — `grep -rn 'open_trial_book(' backend/crates/apps` finds it in all three central composition roots, and `grep -n 'book.open_family\|book.enrol' backend/crates/runtime/qip-kernel/src/central/factory.rs` is fed in production through `qip-deepbrain` (`grep -n 'foundry.register(' backend/crates/apps/qip-deepbrain/src/evolution.rs`, reached from `node.rs` → `maybe_turn` → `turn`). Holdout evidence is assembled there too (`grep -n 'HoldoutEvidence' backend/crates/runtime/qip-kernel/src/central/foundry.rs`). **Corrected 2026-09-15 on three counts, every one of them understating.** The cell said the controls that *read* the trial book are unreached, and cited three searches for it. **(1) Deflated Sharpe has a production caller, and the LEARN stage is it.** `grep -rn 'deflated_sharpe(' backend/crates` finds callers in `qip-kernel/src/family_review.rs`, `qip-lifecycle/src/gates.rs` and `qip-evolution/src/challenger.rs` as well as the crate that defines it; the kernel's runs every cycle — `grep -n 'fn member_deflation' backend/crates/runtime/qip-kernel/src/family_review.rs` is called from `family_review::standings`, which `grep -n 'self.family_standings()\|fn review_family_allocation' backend/crates/runtime/qip-kernel/src/platform.rs` reaches from `stage_learn`. Purged and embargoed CV likewise: `grep -rn 'PurgedSplit::new' --include=*.rs backend/crates/apps backend/crates/services` finds `qip-deepbrain`'s evolution round beside `qip-lifecycle`'s gate and `qip-evolution`'s discovery. **(2) The cross-reference to §20.2 was stale when it was written.** That row was re-scored `REACHED` on 2026-09-08: `grep -n 'factory_mut().promote(' backend/crates/apps/qip-deepbrain/src/evolution.rs` is the production caller of `StrategyFactory::promote`, so the five gates are entered on every evolution round and answer. They refuse every candidate on the committed tape, which §20.2 argues is the gate working rather than the wire missing — a different fact from having no caller, and the one this cell flattened. **(3) There is a capacity estimate.** `grep -n 'fn capacity_probe\|self.capacity_probe(' backend/crates/runtime/qip-kernel/src/platform.rs` runs one inside `stage_simulate` on every cycle with enough price history, and §15.3 credits it as that section's single production use. What is genuinely still missing is the live canary, and the only thing `grep -rni 'canary' backend/crates --include=*.rs` finds is two comments saying so. The row stays `PARTIAL` on the canary and on the two rungs no operator route yet climbs, not on the statistics. |
| 20.2 | The Promotion Pipeline | REACHED | The whole ladder is built and refusing — `grep -n 'pub fn gate_for' backend/crates/services/qip-lifecycle/src/gates.rs` and `grep -n 'pub fn attempt_promotion' backend/crates/services/qip-lifecycle/src/ledger.rs` — and wired into the kernel at `grep -n 'attempt_promotion(' backend/crates/runtime/qip-kernel/src/central/factory.rs`. Until 2026-09-08 `grep -rn '\.promote(' backend/crates --include=*.rs` found every caller under `/tests/` (`qip-kernel/tests/central.rs`, `qip-api/tests/{mesh,research}.rs`). Nothing promotes a strategy outside a test, so no strategy is ever funded. The human family-approval rung is likewise test-only. | **The first rung now has a production caller.** `qip-deepbrain`'s evolution loop registers each scored candidate with the holdout evidence its own search produced and then puts that evidence in front of the gate: `grep -n 'factory_mut().promote(' backend/crates/apps/qip-deepbrain/src/evolution.rs`, with the round's accounting in `RoundSummary::promoted` and `gate_refused` so a candidate gathered and never judged is a countable defect rather than a silence. `PARTIAL` and not `REACHED`: this is one rung of five. The gates above read paper and shadow evidence the strategy has had no chance to make, and `Pilot` and `Scaled` — the two rungs where capital is at stake — need a dual human approval (`grep -n 'fn requires_human_approval' -A 3 backend/crates/libs/qip-contracts/src/gate.rs`) that no operator route yet raises. So the headline of this row still stands: **no strategy is funded**, and none can be until that route exists. On the committed synthetic tape the gate refuses every candidate on its merits — 47 held-out observations against a 250 minimum and a Sharpe of -9.56 — which is the gate working, not the wire failing: `cargo test -p qip-deepbrain --lib every_candidate` pins the accounting rather than a pass count, because whether a seed's candidates clear a statistical bar is not a fact about this wiring. | **The signed rungs now have a route too.** `grep -n 'promotion-approvals' backend/crates/apps/qip-api/src/routes.rs` is the operator surface and `grep -n 'pub fn approve_promotion' backend/crates/runtime/qip-kernel/src/platform.rs` the intent it raises: the approver comes from the authenticated session and the body cannot carry one, the kernel holds the first signature until a *different* subject countersigns, a first signature goes stale after 24 hours, and the gate still re-derives its own verdict — the pair authorise an attempt, never an outcome. `cargo test -p qip-kernel --test central` covers one-signer refusal, the two-operator pass including the pilot baseline it writes, staleness of both the signature and the credential, and a signature offered for a rung that takes none. So every rung of the ladder now has a production caller. It stays `PARTIAL` rather than `REACHED` for one honest reason: on the committed synthetic tape the holdout gate refuses every candidate on its merits, so nothing has yet arrived at a rung for anyone to sign for, and a ladder whose every rung is wired is still not a ladder anything has climbed. | **Re-scored `PARTIAL` → `REACHED` on 2026-09-08, and the earlier `PARTIAL` was this document applying a bar it does not state.** The bar above is *built, tested, and a non-test call path exists*, and it says in terms that `REACHED` does not require deployment — `Cell::work` is `REACHED` though `execution_nodes = {}` means nothing has ever entered it. Every rung of this ladder now has a non-test call path, and the promotion path is in a stronger position than that precedent: it is **entered on every round** in a production binary and returns a verdict, rather than sitting behind a switch nobody set. The rounds this document was waiting for are happening; the gate is answering *no*. Two defects found while establishing that, both fixed here rather than described. The evaluation hand-rolled its purged folds and its arithmetic disagreed with `PurgedSplit`, which the gate rebuilds them with — it charged a fold's trailing label horizon as purged on top of the leading purge, reporting four where the splitter found two, so **every** candidate was refused on `purging_and_embargo_applied` whatever its merit (`grep -n 'PurgedSplit::new' backend/crates/apps/qip-deepbrain/src/evolution.rs`). And the engine searched from 66 bars, holding out 16 against a gate minimum of 250, so early rounds could not pass on sample adequacy alone *and still charged every candidate to the family's cumulative trial count*, deflating the Sharpe of every later candidate — futile searches raising the bar for the ones that could have succeeded (`grep -n 'fn minimum_bars_for_the_holdout_gate' backend/crates/apps/qip-deepbrain/src/evolution.rs`, derived from `HoldoutPolicy` so the two cannot drift). What remains is not a wire and not a threshold: on the demo tape the search finds nothing worth funding, because there is nothing to find. Equities there are a random walk with factor exposure — `price_reversion` is zero for `ProcessParameters::equity`, and the one instrument with genuine mean reversion is the bond, which is illiquid and correctly ranked out — so a timing strategy earns zero gross and loses the costs it pays, which is exactly what the gate reports (Sharpe −3.07 against a 1.02 selection threshold). **That is the gate working, and it is the honest outcome for this data.** Making it report otherwise would mean planting an edge in the simulator or lowering a statistical bar, and neither is a change this row would be entitled to claim. |
| 20.3 | Decay and Retirement | REACHED | Production path: `grep -n 'fn review_strategies' backend/crates/runtime/qip-kernel/src/platform.rs` — `stage_learn` calls it every cycle, it calls `learn_from_cells` → `grep -n 'pub fn learn(' backend/crates/runtime/qip-kernel/src/central/learning.rs` → `grep -n 'factory_mut().review(' backend/crates/runtime/qip-kernel/src/central/learning.rs` → `grep -n 'monitor.enforce(' backend/crates/runtime/qip-kernel/src/central/factory.rs`, with retirement dispositions and the ledger demotion behind it. Honest limit: every observation is skipped unless the strategy has a pilot baseline, and `grep -n 'baselines.insert' backend/crates/runtime/qip-kernel/src/central/factory.rs` shows the only two writers are `promote` (which since 2026-09-08 has one — see 20.2, though it reaches only the first rung, and a baseline is seeded at the pilot rung that rung cannot reach) and `set_baseline`, which `grep -rn 'set_baseline(' backend/crates --include=*.rs` shows nobody calls at all. The retirement machinery runs; in a deployment it would retire nothing. All four kill conditions can now fire on what the centre measures: `CostOverrun` read a `realised_cost_bps` the centre hardcoded to `0.0`, so it compared `0.0 > modelled + tolerance` and was false for every non-negative modelled cost — `grep -n 'fn cost_bps_by_strategy' backend/crates/runtime/qip-kernel/src/central/plane.rs` is the measurement that replaced the constant, costing each fill against the price the platform sent, and `cargo test -p qip-kernel --test central cost` drives it end to end through `ingest_cell_report` to `live_outcomes`. |
| 21.1 | Training Without an Archive | PARTIAL | Reached: episodes and counterfactuals, the two v9 additions. `grep -n 'CounterfactualEngine::new' backend/crates/runtime/qip-kernel/src/platform.rs` builds it in `Platform::new`, and `grep -n 'evaluate_alternatives\|score_declined' backend/crates/runtime/qip-kernel/src/platform.rs` shows `stage_learn` scoring declined paths; `grep -n 'pub fn' backend/crates/runtime/qip-kernel/src/central/episodic.rs` is the compressed-episode store. Absent: the streaming-estimator half. `grep -rn 'RunningStats' backend/crates --include=*.rs` finds the Welford accumulator only in `libs/qip-numerics/src/stats.rs` with no caller anywhere; `grep -rn 'digest\|CountMin\|HyperLogLog\|eservoir' backend/crates --include=*.rs` finds no quantile sketch, count-min, HyperLogLog or reservoir sampler. No event-anchored snapshot store, no declared per-estimator error bound and no degraded-marking of dependent models. |
| 21.2 | The Pipeline | PARTIAL | Reached, in one binary: `grep -n 'maybe_learn' backend/crates/apps/qip-deepbrain/src/node.rs` → `grep -n 'LocalTrainer::new().fit\|distil(' backend/crates/apps/qip-deepbrain/src/learning.rs` → the model card lands in `qip_ai::registry::ModelRegistry`, and `grep -n 'holdout' backend/crates/apps/qip-deepbrain/src/learning.rs` is the evaluate step. Absent: everything after evaluate. No feature-pipeline service, no ONNX/protobuf emission, no artifact signing, no Artifact Registry or Spanner row, no Pub/Sub distributor and no atomic model-set swap — `grep -rniE '\bonnx\b\|\bburn\b\|\blinfa\b\|\bprost\b\|\btract\b' backend/crates --include=*.rs` finds two doc comments and no code. The five named runtimes are all forbidden by the two-dependency rule (ADR 0002, ADR 0009), so this half is BLOCKED-by-policy rather than merely undone. |
| 21.3 | Rust for Machine Learning — Honest Assessment | NARRATIVE | An assessment of where Rust is and is not adequate, and the claim that training and inference sharing feature code removes training-serving skew structurally. No deliverable of its own; the shared-feature-code claim is scored under 21.2, where the training and scoring paths are the same `LearningDesk` code. |
| 22.1 | Retention Classes | PARTIAL | Re-scored 2026-09-12 (ADR 0057). Reached: the irreplaceable-versus-replaceable distinction is enforced, not documented — `grep -n 'requires_permanent_retention' backend/crates/libs/qip-events/src/log.rs` shows the log evicting lossy-tolerable records first, then replaceable ones, and *refusing the append* rather than dropping an audit record when only permanent ones remain (the topic predicate is `grep -n 'fn requires_permanent_retention' backend/crates/libs/qip-events/src/topic.rs`); and, new, the taxonomy itself is a type: `grep -n 'pub enum RetentionClass' -A 24 backend/crates/services/qip-data-finder/src/retention.rs` is the table's **nine** rows (this row said ten until 2026-09-12; the enum's `ALL` is what to count), each answering `retention()` with the row's own policy and `what()` with the row's own words — checked against the blueprint's table itself by `the_nine_retention_classes_each_state_their_own_policy`, which reads §22.1 out of the source document and asserts the count, each class, each row's words and each retained-column phrase (an earlier version of the test read every fact off the enum it was testing, and passed a fallback row that said "bars" where the table says "one-minute OHLCV"). The fallback-series row is built and bounded three ways — three years behind the newest bar, 1,100 bars per instrument, 512 instruments, every zero refused (`grep -n 'FALLBACK_RETENTION\|FALLBACK_BARS_PER_INSTRUMENT\|FALLBACK_INSTRUMENTS' backend/crates/services/qip-data-finder/src/retention.rs`) — fed from the SENSE stage's own `Platform::observe` for daily bars only (`grep -n 'self.fallback.retain' backend/crates/runtime/qip-kernel/src/platform.rs`, proven by `cargo test -p qip-kernel --test retention`) and drawn on in production by the deep brain's research campaign when a subject's stream no longer holds enough history (`grep -n 'fallback_bars' backend/crates/apps/qip-deepbrain/src/campaign.rs`). Stated deviation: the row says one-minute bars and this series holds daily ones; ADR 0057 says why. Absent: the 90-day event-anchored roll — `Retention::Rolling` is a value the `EventAnchored` variant answers with and nothing rolls a book state behind it (`grep -rn 'EventAnchored' backend/crates --include=*.rs` finds the enum, its `retention()` arm and its test, and no writer) — and per-class size accounting. |
| 22.2 | Sufficient Statistics | PARTIAL | Re-scored 2026-09-12 (ADR 0057). Reached: two rows of the table. `grep -n 'stats::covariance' backend/crates/runtime/qip-kernel/src/platform.rs` computes pairwise covariance in production — but from a retained return series, batch, not the exponentially weighted online update the section names. New: the "frequency and cardinality" row's count-min sketch, with the error bound the row calls "bounded" declared as a value the sketch is built from — `grep -n 'pub struct ErrorBound\|pub struct CountMinSketch' backend/crates/libs/qip-numerics/src/sketch.rs` — and the guarantee stated exactly (never below the true count; above it by at most ε·N with probability 1−δ) and tested on a skewed stream with more keys than counters; the bound refuses a pair whose counters would exceed `MAX_COUNTERS` or overflow, in code and on the wire (`grep -n 'pub const MAX_COUNTERS' backend/crates/libs/qip-numerics/src/sketch.rs`), so the module's "kilobytes" is a number — and since the second review of 2026-09-12 the sketch itself deserialises only through its bound's geometry (`grep -n 'CountMinSketchWire' backend/crates/libs/qip-numerics/src/sketch.rs`; a `width: 0` was a division by zero on the first estimate). Its production caller is the deep brain's research campaign, which counts bars per subject across the extents it fetched (`grep -n 'CountMinSketch::new' backend/crates/apps/qip-deepbrain/src/campaign.rs`), attaches the estimate *with its bound* to the campaign manifest (`grep -n 'pub struct SketchedStatistic' backend/crates/services/qip-data-finder/src/campaign.rs`), and refuses the fit when the declared error at the counted volume exceeds five percent of the desk's minimum (`grep -n 'tolerable_for' backend/crates/apps/qip-deepbrain/src/campaign.rs`) — §56.3's rule 30, with a consumer. Honest limit: a one-subject campaign gives the sketch one key, so today's estimate is exact and the bound is formal; the refusal path is real and mutation-verified. Built-not-reached: Welford (`grep -rn 'RunningStats' backend/crates --include=*.rs` — no caller) and `grep -n 'pub fn ewma' backend/crates/libs/qip-numerics/src/stats.rs` (`grep -rn 'stats::ewma(' backend/crates --include=*.rs` finds no production caller). Absent: t-digest/KLL quantiles, HyperLogLog, weighted reservoir sampling, recursive least squares, and streaming PCA — `grep -rn 'pub struct.*Digest\b\|HyperLogLog\|pub struct.*Reservoir\|recursive_least_squares\|fn oja' backend/crates/libs/qip-numerics/src --include=*.rs` returns nothing. |
| 22.3 | Data References | REACHED | **Corrected 2026-09-13 a second time, after three more rounds against the same function (ADR 0057, ninth amendment); verdict unchanged.** Round seven (`d231679`) found that cutting the parameter region off first and then searching only the surviving prefix for the credential's terminating `@` leaks whenever the password itself holds a `?` or `#` — ordinary password characters, not legal unencoded in userinfo, which is exactly why such a string reaches this function instead of parsing — so `http://svc:SECRET?x@127.0.0.1:9105` came back as `http://svc:SECRET?…` and 131,040 of 640,000 enumerated inputs leaked the same way; both boundaries are measured over the whole remainder and only then intersected now, and the guarantee is executed by a sweep test rather than asserted in a comment. Round eight (`88ca127`) replaced a byte-index slice behind a byte-length guard that panicked on an address whose byte 8 falls inside a multi-byte character and printed the raw address in the panic — reachable by pasting this function's own output back into the variable — and stopped the gate naming a parsed host the redaction had masked (`http://hf_SECRET?x@127.0.0.1:9105` parses with the secret sitting where the host goes). Round nine (`a190352`) deleted the discredited sentence round eight's commit message said it had deleted and had in fact only argued with, and recorded a real cost of round eight's fix: `qip-market-ingestion`'s `ConnectorFeed::open` is the one of six call sites that does not name its configuration variable, so on that path a refusal can now name neither host nor variable. Nothing on this row's own evidence moves again: `qip-transport` only, no reference, hash, ledger or campaign path touched. The entry this corrects follows. — **Corrected 2026-09-13, after three further rounds against the one function that renders a refused egress address printable (ADR 0057, eighth amendment); verdict unchanged.** The fifth correction below records that every echo of a refused address goes through one redaction helper. Rounds four, five and six each found it still leaking. An authority that was empty, or the bare `:` a typo'd `://` leaves, was read as proof there was no credential one character further in (`e26c8cf`); the "could this be a host" check that replaced that reading trusted `abc:80`, `123` and `svc`, and ran the same way with a scheme as without, so `http://svc/TOKEN@127.0.0.1:9106` leaked and so did a one-character `:`→`/` typo on an ordinary credential-bearing address, through `require_loopback_egress`'s real refusal message (`ca3d581`); and two independent reviews of that fix found the guarantee held for userinfo while the comment claimed it for credentials, a `?api_key=…` query having no `@` in it at all and printing in full (`e691804`). The predicate over the string's own shape is now deleted rather than narrowed a sixth time — **and what follows in this sentence was true of `e691804` and of nothing since, which is what it should have said when it was written**: as of `e691804`, redaction ran through the last `@` past the scheme unconditionally, the parameter region was cut off first and masked whole, a credential inside a path segment was the stated limit with its own test row, control characters were escaped, refusals led with the parsed host rather than the address, and the function was renamed `redact_for_echo` for the job its callers actually use it for. Three of those clauses have since moved, and the correction is above rather than here: at `a190352` both region boundaries are measured over the whole remainder and only then intersected rather than one being cut first (round seven, `d231679`, which found that cut landing inside a password); the surviving region is every address with no `@` before the cut and no `?` or `#` at all, not a path segment; and a refusal names the parsed host only where the redaction kept it (round eight, `88ca127`). Over-redaction, a benign `@` in a path masked, is the accepted permanent cost. Nothing on this row's own evidence moves: no reference, hash, ledger or campaign path is touched and the tests cited below are unchanged. One finding this round leaves **open** is recorded once, in this document's provenance entry for this round rather than here, because it is about a source candidate's host rather than about a reference: `SourceEndpoint` deserialises past the guard that keys the denylist. The entry this corrects follows. — **Corrected 2026-09-12 a fifth time, after a fresh review of the fourth round's repairs (ADR 0057, fifth amendment); verdict unchanged.** Three findings on this row, none above low. The inspected log accepted `append` silently: `EventLog::inspect` returned a log with the file's records and no path, so an append reached memory, minted the next sequence over the file's chain and put nothing on disk — and the test cited below asserted that as the intended shape. The log now carries an `inspected` marker checked first in `append`, which refuses with `denied`, names `EventLog::inspect` as the open it came through and `EventLog::open` as the one to resume with, and leaves the log exactly as read (`an_inspection_of_a_held_log_names_the_holder_and_once_released_holds_nothing_itself`, rewritten from "reaches memory only" to "refused by name, reaches neither"; the mutation replacing the check with `false` made the append succeed and the test fail on the refusal). The billing test's mutation note described a mutation its own store could not admit — a first write carrying the previous position carries a position, and `PositionRefusingStore` refuses any value that does — so the note was rewritten to the mutation that fires: a first write of `{ ledger, checkpoint: None }` in `record_and_commit`, adopted by the store, after which `durable.polls` reads `2` against the `1` asserted and the test fails there, before the restart; the note also says the mutation must land in `record_and_commit` and not in `open`, whose session write has the same shape, because the first attempt here did. The comment's "polls == 2" sentence and its "second table, a second delivery" wording (the emulator serves one fixed body) are corrected; the assertion was sound throughout. And the fourth-round text below said one inspection test was "run as an unprivileged user"; what the checkout shows is that `a_journal_on_read_only_storage_is_inspectable_where_the_writers_open_is_refused` probes whether the mode bits bind the process, asserts the read-only half only where they do, and prints which half it proved — on this uid 0 host it proves the inspection loads and no more; `ci.yml`'s `ubuntu-latest` runner is unprivileged, and no run is cited. The entry this corrects follows. — **Corrected 2026-09-12 a fourth time, after a fresh review of the third round's repairs (ADR 0057, fourth amendment); verdict unchanged.** The one medium finding sat on this row's log lock: `EventLog::open_with_capacity` had become the only open, so `qip replay` — which promises to read a journal where it lies — failed on a read-only mount (the append open cannot succeed there), relabelled the lock refusal on a running node's journal as "is not an event log this platform wrote", and, pointed at an absent path, created an empty file. `EventLog::inspect` is now the reader's open: `File::open`, never creating, under the shared side of the same advisory lock for the read and no longer, and the CLI passes the `Denied` refusal through unwrapped (`an_inspection_of_a_held_log_names_the_holder_and_once_released_holds_nothing_itself`, `an_inspection_of_a_missing_path_refuses_by_name_and_creates_nothing`, `a_journal_on_read_only_storage_is_inspectable_where_the_writers_open_is_refused` — the last probes rather than assumes that the mode bits bind the process, and its mutation was run as `nobody`; in `qip-cli`, `a_journal_a_running_node_holds_is_refused_naming_the_holder_not_as_a_corrupt_file`). `log.rs` now states the lock is proven advisory on Unix only. A low finding on the same row's journal: the third correction's `record_and_commit` wrote ledger then checkpoint over two keys and said the gap "heals itself"; across a crash it did not — the deep brain exits on the failure and the restart billed the re-fetch again, permanently one high. Ledger and checkpoint are one value under one key now, one `put`, atomic on each store's own terms; a store holding the two old keys is refused at open by name; `StreamJournal::record` is gone and `tests/restart.rs` is ported (`a_journal_write_that_fails_after_the_poll_bills_the_refetch_once_even_across_a_restart` replaces `a_journal_write_that_fails_after_the_poll_bills_once_and_never_leaves_the_checkpoint_ahead`; `a_store_holding_the_two_key_layout_is_refused_at_open_rather_than_read_as_a_fresh_stream`). Wording: `references.rs`'s chain refusal now says "a rewrite that recomputed the hash" — under `GapPolicy::Evicted` a post-gap record is anchored on its own claimed predecessor, so one hash suffices. The entry this corrects follows. — **Corrected 2026-09-12 a third time, after a fresh review of the second round's repairs (ADR 0057, third amendment); verdict unchanged.** S-F1 (high): `verify_retained_chain` re-anchored only the head and held every retained record to its retained predecessor, so any log with an *interior* eviction — a permanent record older than every evictable one, which a deep brain that journals closed campaigns between reference records reaches — failed the check and `Platform::new` refused every restart over its own honest log naming tampering. Now every record's hash is recomputed, a link is held only between consecutive sequences, and a gap re-anchors on the claimed predecessor as the head does; `open_with_capacity` refuses a file whose sequences are not contiguous so a removed line never reads as an eviction; and the refusal states what the chain proves — unkeyed SHA-256, ADR 0043 (`the_retained_chain_verifies_across_an_interior_eviction_and_still_names_an_edited_record`, `the_ledger_rebuilds_from_a_log_that_evicted_an_interior_record`). S-F4: `EventLog::open_with_capacity` takes an exclusive lock and refuses a log another handle holds (`a_log_another_handle_holds_is_refused_until_the_handle_is_released`), so the campaign id's uniqueness claim now holds against concurrent writers too and its text says what remains (a log truncated or restored from an older copy). S-F7: the reference wire gate holds `replay://` and `ReplayedAdmitted` together or not at all and the hash to sixty-four lowercase hex (`a_reference_off_the_wire_is_held_to_the_constructors_refusals`, six forged rows added). C-3/S-F5: the bridge writes a poll's ledger and checkpoint as one adopt-on-success step, checkpoint last (`a_journal_write_that_fails_after_the_poll_bills_once_and_never_leaves_the_checkpoint_ahead`). The `Discovered` arm of `sources_backing` has no live gate and its doc now says so and why nothing reaches it. The entry this corrects follows. — **Corrected 2026-09-12 after a second independent review of the repairs (ADR 0057, second amendment); verdict unchanged, four claims below made true.** The gate is now the only door on the log as well: `DataReference` and `RevisionRecord` derived `Deserialize` with no gate in front of it and deserialise through their constructors now (`grep -n 'DataReferenceWire\|RevisionRecordWire' backend/crates/services/qip-data-finder/src/reference.rs backend/crates/services/qip-data-finder/src/ledger.rs`); `resume_references` verifies the retained chain before it restores a frame and refuses a broken link by sequence (`grep -n 'verify_retained_chain' backend/crates/runtime/qip-kernel/src/references.rs backend/crates/libs/qip-events/src/log.rs`; `EventLog::open` recomputes no hash, and until then a frame edited on disk became the ledger's latest reference — `a_log_whose_chain_is_broken_refuses_to_rebuild_the_reference_ledger`); a catalogue-admitted reference counts as backing only while this process holds the admission (`grep -n 'admitted_sources.contains_key' backend/crates/runtime/qip-kernel/src/references.rs`; `a_restored_reference_without_a_live_admission_is_not_vendor_backing`); and `SourceRevisionDetected` carries the revising reference at schema version two, so a log that evicted the reference's own Sense-group record still restores what the source now serves and the *next* revision of the extent is caught (`a_revision_restores_the_revising_reference_after_its_own_record_was_evicted`). A replay under a connector's admission is referenced through `SourceOrigin::ReplayedAdmitted`, never a vendor — see §22.4. The connector bridge unwinds a poll whose journal cannot be written as it unwinds a refused reference (`a_journal_that_cannot_be_written_leaves_the_checkpoint_where_it_was_and_the_next_poll_refetches`), its `DataAdapter::poll` refuses rather than releasing an unreferenced fetch, and a delivered poll whose records carry no subject is marked `unreferenced` and counted. The entry this corrects follows. — Re-scored 2026-09-12 a third time (ADR 0057), from `PARTIAL`; the two earlier entries that day are kept below in this cell because the blocker they named is what this change removed. **The production path, end to end:** `qip-market-ingestion`'s `ConnectorRuntime::ingest` digests exactly the bytes a vendor served, at the one seam holding the body, the locator and the decoded events' instants (`grep -n 'report.digest = ' backend/crates/services/qip-market-ingestion/src/connector/runtime.rs`); the digest carries the *subjects* its mapped records are about beside the vendor's row keys, and the reference is keyed on the subjects, because a campaign asks by `ObjectId` and a ledger keyed on `EUR/USD@2026-09-04` answered no campaign (`grep -n 'pub fn subjects' backend/crates/services/qip-market-ingestion/src/connector/digest.rs`; review finding S2, 2026-09-12); `ConnectorFeed::poll_referencing` stamps the topic and hands it to the root's reference hook *between the poll and the checkpoint commit* (`grep -n 'pub fn poll_referencing' backend/crates/services/qip-market-ingestion/src/connector_feed.rs`), and both roots pass `Platform::reference_fetch` through that hook before `Platform::observe` (`grep -rn 'reference_fetch' backend/crates/apps/qip-api/src/routes.rs backend/crates/apps/qip-fastbrain/src/node.rs`), so a fetch the platform refuses to reference unwinds the connector and is re-fetched next poll rather than dropped past a committed cursor (finding F2; until then the checkpoint committed first and a refused batch was silently lost); the kernel builds the reference through the catalogue door (`grep -n 'pub fn from_digest' backend/crates/services/qip-data-finder/src/reference.rs`, taking the digest's hash rather than bytes, because a `FetchDigest` has one constructor that hashes a body it was given) against an `AdmittedSource` (`grep -n 'pub fn from_decision' backend/crates/services/qip-data-finder/src/admission.rs`) and refuses a source it holds no admission for; and records it on a ledger bounded by count on both axes (`grep -n 'REFERENCE_LEDGER_BOUND\|REVISION_LEDGER_BOUND' backend/crates/services/qip-data-finder/src/ledger.rs`). A second production caller, the deep brain's research campaign, references its window through the `Generated` door (`grep -n 'of_generated\|of_admitted' backend/crates/apps/qip-deepbrain/src/campaign.rs`). **Every field, with its effect:** the content hash — a re-fetch hashing differently is a `SourceRevisionDetected` record on the hash-chained log under its own permanently retained topic (it sat under `DataQualityFailed`, not permanently retained and already carrying another body, until finding S3), a `ResearchCampaignFlagged` record naming every campaign already closed on the log whose manifest read the withdrawn bytes — the backtest that used the original, found by joining the revision against the closed manifests (finding S4; until then the campaign that had just fetched the *corrected* bytes was the one flagged) — a `qip_data_revisions_detected_total{origin}` count, and a flag `Platform::revision_covering` answers and `campaign::assemble` reads before a fit (`grep -n 'SourceRevisionDetected\|ResearchCampaignFlagged\|DATA_REVISIONS_DETECTED\|pub fn revision_covering' backend/crates/runtime/qip-kernel/src/references.rs`; proven end to end by `cargo test -p qip-kernel --test references`): flagged, not silently invalidated. Every reference is on the log as a `DataReferenceRecorded` record *before* the in-memory ledger moves, each record carries an idempotency key the log now indexes so a fact journaled twice is one record, and `Platform::new` rebuilds the ledger from the log (`grep -n 'fn resume_references' backend/crates/runtime/qip-kernel/src/references.rs`; finding S7 — the ledger was process-lifetime, and a restarted node could detect no revision of an extent the previous process had used); the availability field — a data class with fewer than two viable *vendors* is held back from promotion past validation (`grep -n 'held_back' backend/crates/apps/qip-deepbrain/src/evolution.rs`, §56.3 rule 31; a generated stream counts for none, finding F6); the schema version — the `SourceSchema` fingerprint derived from the manifest's declared contract; the cost estimate — recorded as zero for the four shipped connectors, which are all free tiers with no tariff in any manifest, and stated as such rather than invented (`grep -n 'CONNECTOR_FETCH_COST' backend/crates/runtime/qip-kernel/src/references.rs`); availability for a delivered fetch is recorded as 1.0 with the same honesty (the source answered this fetch; a window's availability is `SourceHealth`'s measurement). **The licensing gate is the only door, on the wire as well as in code:** `AdmittedSource` takes a `LicensingDecision`, which since ADR 0057 has a private `GatePassed` field and exactly one construction site — `grep -n 'LicensingDecision {' backend/crates/services/qip-data-finder/src/admission.rs` — inside `admit_from_registered`, after every usage question; `a_source_the_licensing_gate_refuses_cannot_reach_the_catalogue_door` proves an ambiguous posture and a research-only licence both stop there; and `AdmittedSource` and `FetchDigest` no longer derive `Deserialize` — a derive is a second constructor that takes any caller's word for the licence or the hash, both types had one when this row first said the gate was the only door (finding F1/S5), and a `compile_fail` doctest on `AdmittedSource` now pins the absence. Every `DataReference` names its door in a `SourceOrigin` (`grep -n 'pub enum SourceOrigin' -A 8 backend/crates/services/qip-data-finder/src/reference.rs`), so a discovered, a catalogue-admitted and a generated source are never confused. **Why `bbf31c8`'s blocker no longer holds:** that correction was right that a `RegisteredSource` could not honestly be built for a connector, and none is — the door is a distinct type carrying only what happened to the source (the catalogue's evaluation, the manifest's declared category and schema), which is the second of the two designs the correction named without making. Its second-reason argument about §22.4 also still stands and shaped the caller: see §22.4. Kept for the record, the two earlier entries follow. — Re-scored 2026-09-12 (ADR 0056), corrected later the same day. `DataReference` exists with every pseudo-code field — source (by id, requiring the §7.6.1 category already recorded on the `RegisteredSource`), locator, symbols, a `DataPeriod` range, the `SourceSchema` shape, a content hash, `retrieved_at`, a `Decimal` cost estimate, and an availability fraction: `grep -n 'pub struct DataReference' -A 15 backend/crates/services/qip-data-finder/src/reference.rs`. The content hash is real and reuses the exact mechanism `SourceManifest` uses for §7.2: `grep -n 'qip_core::sha256_hex' backend/crates/services/qip-data-finder/src/reference.rs backend/crates/libs/qip-financial/src/manifest.rs` finds both call sites. `DataReference::verify` re-hashes a re-fetch and reports a `RevisionCheck`, tested and mutation-verified (`grep -c '#\[test\]' backend/crates/services/qip-data-finder/src/reference.rs` — 3). Not `REACHED`: **no non-test caller exists, and this row's earlier reason ("no HTTP transport is linked into this build") was wrong.** Checked again the same day: `qip-market-ingestion`'s four production connectors do fetch real bytes over a real transport on every poll — `grep -n 'SOURCE_ID =>' backend/crates/services/qip-market-ingestion/src/connector_feed.rs` names Coinbase, Alpaca, Frankfurter and Kalshi, `ConnectorFeed::open` builds an `HttpSourceTransport` (`grep -n 'HttpSourceTransport::connect' backend/crates/services/qip-market-ingestion/src/connector_feed.rs`), and `qip-api`/`qip-fastbrain` both drive `ConnectorRuntime::poll` every cycle. The real blocker is `DataReference::of`'s own precondition, `&RegisteredSource`, which has exactly one constructor, `pub(crate)`, called from exactly one place: `grep -n 'RegisteredSource::new' backend/crates/services/qip-data-finder/src/finder.rs` lands inside `assess_one`'s register step, reachable only after a candidate has cleared `DataFinder`'s full discover → probe (robots.txt, HEAD, payload sample) → classify → score → route pipeline — the DISCOVER-stage mechanism for vetting a previously-unknown candidate URL (`qip-deepbrain`'s `DiscoveryDesk` via `Platform::assess_sources`, per §7.6.1's row), which is a different question from the one the four SENSE-stage connectors already answered through the separate `qip_data_finder::admission::{admit, StandingAdmission}` gate, whose catalogue returns a `LicensingDecision`, never a `RegisteredSource`. The two are disjoint by construction, not merely unwired: `grep -n 'qip-data-finder' backend/crates/services/qip-market-ingestion/Cargo.toml` finds nothing, because the dependency edge runs the other way (`qip-data-finder` depends on `qip-market-ingestion`), so a `RegisteredSource` cannot even be named at the connector-runtime seam. Building one for a shipped connector would require either running its manifest through discovery for real — and in production today that reaches only `NetworkProbe`, which refuses every call by construction (`grep -n 'impl SourceProbe for NetworkProbe' -A 12 backend/crates/services/qip-data-finder/src/probe.rs` — three `Err(self.unavailable(...))` arms), so no deployed process produces a `RegisteredSource` from real bytes today either — or fabricating the robots/probe/score/routing evidence a discovery pass would have produced for a source that was never run through one, which this correction declines to do rather than misrepresent how an already-licensed vendor feed was vetted. `DataReference::of` reads only `source.category()` and `source.id()` off the `RegisteredSource` it is given, so the honest fix is a reviewed design decision this correction does not make unilaterally: either narrow `DataReference::of`'s precondition to the id and category it actually uses (an amendment to ADR 0056), or give `qip-data-finder` a second, honestly-labelled registration path for a catalogue-admitted connector that claims no probe evidence it never gathered. Either is a change to a type ADR 0056 already treats as settled, not a wiring exercise. |
| 22.4 | Fetch-on-Demand for Research | REACHED | **Corrected 2026-09-13 a second time, after three more rounds on the same redaction (ADR 0057, ninth amendment); verdict unchanged.** The six egress seams and their gate are still untouched; what moved again is what their refusals print and, in round eight, whether one of them can print at all — `base_url.len() >= 8 && base_url[..8]` panicked on an address whose byte 8 sat inside a multi-byte character, and the panic printed the raw address by a path no redaction touches (`88ca127`, now a non-panicking `get(..8)`). Round seven (`d231679`) fixed the region ordering that printed the first half of any password containing a `?` or `#`. Round eight also stopped the host arm naming a parsed host the redaction had masked, and round nine (`a190352`) records what that costs: of the six call sites, `ConnectorFeed::open` in `qip-market-ingestion` calls the gate with a bare `?` and names no configuration variable, so on that one path a refusal can now identify neither the host nor the setting — a real gap with a known fix (a wrapper at that call site), recorded rather than discovered later. The entry this corrects follows. — **Corrected 2026-09-13, after three further rounds on the redaction the fifth correction below credits (ADR 0057, eighth amendment); verdict unchanged.** The six egress seams and their gate are untouched; what changed three more times is what their refusals print. Round four (`e26c8cf`) found an empty or bare-`:` authority read as proof of no credential, six inputs coming back raw through `QIP_LANGUAGE_MODEL_BASE_URL`; round five (`ca3d581`) found the host-shaped check that replaced it trusting `abc:80`, `123` and `svc`, leaking `http://svc/TOKEN@127.0.0.1:9106` and a realistic one-character `:`→`/` typo through the gate's own refusal message; round six (`e691804`) found the guarantee sound for userinfo and the claim around it too broad, a `?api_key=…` query carrying no `@` and printing in full. Six leaks in six rounds is the finding, and the decision recorded in the ADR is that no predicate over the string's shape can tell a scheme-typo'd credential from a bare hostname, so the predicate is deleted rather than narrowed again: through the last `@` past the scheme, unconditionally, with the parameter region cut off before the search rather than after — the ordering is load-bearing, and reversed it prints a secret sitting after an `@` inside the query. A credential in a path segment is still printed and is now a stated limit with a test row. No campaign, cache bound, manifest or connector path on this row is affected. The entry this corrects follows. — **Corrected 2026-09-12 a fifth time, after a fresh review of the fourth round's repairs (ADR 0057, fifth amendment); verdict unchanged.** The fourth correction's "one parser, one spelling" was true of the connector pair and not of every in-process base URL. The deep brain's `QIP_LANGUAGE_MODEL_BASE_URL` — the Hugging Face listener, the one address carrying a bearer token — was still two string prefixes that admitted `localhost` and read `http://127.0.0.1:9106@evil.example/` as loopback; the fast brain's `QIP_MARKET_DATA_BASE_URL` refused `https` alone, so a plaintext address off the instance reached `RestFeedConfig` with the API key in its header (that the fast brain has no egress sidecar is a VPC fact, not a process guarantee). The gate is now `qip_transport::http::require_loopback_egress`, beside `Url::parse`, and `qip-market-ingestion`'s copy is gone; six seams call it — the connector pair in the API's, the deep brain's and the fast brain's parsers, `ConnectorFeed::open`, the language-model listener, the market-data vendor — and it requires an explicit port, which Terraform's `startswith("http://127.0.0.1:")` always did and the deep brain's own test already refused `http://127.0.0.1` for (`a_base_url_that_is_not_loopback_is_refused_and_the_refusal_names_the_proxy` moves `localhost` from admitted to refused and gains `LOCALHOST`, `[::1]`, both userinfo rows and a token row; `a_vendor_address_off_loopback_is_refused_by_name` is new on the fast brain, its first row the module's old cluster-DNS fixture; `an_egress_address_is_loopback_with_a_port_and_a_refusal_never_echoes_a_credential` is new on the transport; `a_base_url_off_loopback_is_refused_before_a_socket_is_opened` moves the port-less row to refused). Every seam's refusal echoed the address it refused, userinfo included, so `QIP_CONNECTOR_BASE_URL=http://svc:TOKEN@127.0.0.1:9105` put `TOKEN` on stderr at start-up; `HttpError::InvalidUrl` now stores the address with its userinfo replaced by `…@`, so `Display` and `Debug` are both safe, and the gate's own echoes go through the same `redact_userinfo` (`a_url_that_carries_a_credential_is_refused` gains the assertion; mutations echoing the raw address in the parser, the gate's `https` arm and its parse-failure arm each printed the token; the same mutation on the host arm does not fire, because the parser refuses userinfo first, and the test says so). The deep brain's `configuration()` wrapper does not double-prefix an off-loopback URL: the parser refuses it first with one prefix, and nothing under `ConnectorArm::open` writes `configuration:`. The one in-process URL outside the gate is `QIP_OPENOBSERVE_URL`, by ADR 0032's decision (a private VPC address, not loopback); the ADR names it rather than letting "every" cover it. On the deep brain: two exits still skipped `shutdown_connectors` — a failed `node::flush` returned through `?` between the run and the release, and the open loop returned past every arm an earlier iteration had handed over (and the arm just opened, on a store or `journal_to` failure); all three exits now go through one `with_release`, first failure reported and a failed release appended. What leaks today is process-local — every shipped connector's `shutdown` is the trait's no-op — so the fix is about the invariant; no test drives `main.rs`'s `run` and none is claimed. `MetronomeBuyer::new`'s `debug_assert!` is `assert!`, loud in every profile. The entry this corrects follows. — **Corrected 2026-09-12 a fourth time, after a fresh review of the third round's repairs (ADR 0057, fourth amendment); verdict unchanged.** A low finding on the loopback gate this row's third correction added: `require_loopback_egress` was a second URL parser beside `qip_transport::http::Url::parse` and read `http://127.0.0.1:9105@evil.com/` as loopback (the transport's own refusal of userinfo was all that kept the socket shut), and it admitted `localhost` where `variables.tf` admits only `http://127.0.0.1:`. It parses with the transport's parser now and admits the literal alone — `localhost` is a name the resolver answers, not a verified address — and `qip-api`'s `FeedSettings::parse`, which still refused only `https://`, calls the same helper so its refusal names `QIP_CONNECTOR_BASE_URL` (`a_base_url_off_loopback_is_refused_before_a_socket_is_opened` gains both userinfo spellings, both `localhost` spellings and `[::1]`; the fast brain's and deep brain's pair tests each gain a `localhost` row and a userinfo row; the API's `a_tape_and_a_connector_together_are_a_contradiction_refused_by_both_names` gains three refused rows naming the variable). The third correction's "on the one brain with an egress path" is corrected: the API has an egress sidecar too (ADR 0024). On the deep brain: `shutdown_connectors` ran only on the clean exit; it runs on the error exit too — the exit a connector arm itself produces — with the run's own error kept and a failed release appended; no test drives `main.rs`'s `run`, and none is claimed. `MetronomeBuyer::new` in `qip-simulation-engine`'s `market_conditions.rs`, the one `a7c03ff` `is_multiple_of` site whose old `%` had not guarded zero, carries a `debug_assert!(every > 0)` — a test-only type, so an assertion rather than a refusal. The entry this corrects follows. — **Corrected 2026-09-12 a third time, after a fresh review of the second round's repairs (ADR 0057, third amendment); verdict unchanged.** S-F2 (medium): whether the own stream is a replay was inferred from the presence of a standing admission and pinned by nothing; it is now the adapter's own answer, `DataAdapter::provenance`, overridden by `ReplayAdapter` and `TapeFeed` (`a_replay_backed_engine_reports_replayed_provenance_with_and_without_an_admission`; a mutation to always-live fails it). S-F3 (medium): the connector arm accepted any plaintext `http://` host in the process — the loopback rule lived in `variables.tf` alone, on the one brain with an egress path; `require_loopback_egress` now refuses at `ConnectorFeed::open`, the deep brain's parser and the fast brain's (`a_base_url_off_loopback_is_refused_before_a_socket_is_opened`, a "loopback" row in `the_connector_pair_is_both_or_neither_and_the_source_is_a_distinct_list`, `the_connector_pair_is_held_to_the_loopback_egress_proxy`). C-1: multi-arm `sense` accumulated every source's records and observed at the end, so a later arm's refusal lost an earlier arm's committed batch; each source is observed as soon as its own poll succeeds (`an_arm_that_refuses_does_not_lose_the_records_an_earlier_arm_delivered`). C-2: the test this row cited as proof that replays never lift the hold iterated an empty history; it now asserts that premise and asks `sources_backing` directly with two replayed vendors on the ledger — the campaign-level `a_replay_under_admission_is_referenced_through_the_replayed_door_and_backs_no_vendor` is what held the door meanwhile. Nits: `CampaignSummary.journaled` could never be `false` and is gone; `ConnectorArm::shutdown` is called on node exit; the root's `configuration:` prefix keeps the error's class; the replay header must precede the first non-blank, non-comment line, parsed or not. The entry this corrects follows. — **Corrected 2026-09-12 after a second independent review of the repairs (ADR 0057, second amendment); verdict unchanged on this document's bar, and the two claims that carried it are now true rather than asserted.** First, the manifest claim holds across restarts: campaign ids carry the event log's last sequence beside the cycle (`grep -n 'pub fn campaign_id' backend/crates/apps/qip-deepbrain/src/campaign.rs`), because the cycle count restarts at one in every process and after any restart every manifest was suppressed as a duplicate of the previous run's while the round line said "manifest journaled"; `Platform::journal_campaign` returns whether it wrote, a `false` for an id just minted is an error, and the summary reports the bool (`a_campaign_closed_after_a_restart_is_journaled_under_its_own_id`, two processes over one log, two records). Second, the gate opens for the right thing: **a replay's bytes are never a vendor**. The earlier entry's "two admitted replays lift the hold" was the high finding — a hand-written bars file headed with the ECB connector, restarted under the Coinbase header, read as two vendors and opened rule 31's hold on zero vendor bytes. A replay under an admission is now referenced through the replayed door, `SourceOrigin::ReplayedAdmitted`, with a `replay://` locator, and `is_independent_vendor` is false for it (`a_replay_under_admission_is_referenced_through_the_replayed_door_and_backs_no_vendor` walks the two-header restart and holds it at zero); the adapter refuses a file whose records the named connector never ships, a second header, and a header after a record (`a_replay_headed_with_a_source_that_never_shipped_its_records_is_refused`). What does open it is the deep brain's new connector arm — `grep -n 'pub struct ConnectorArm' backend/crates/apps/qip-deepbrain/src/connectors.rs`, the fast brain's arm on the one brain that carries the egress sidecar (ADR 0024; the fast brain deliberately has none, ADR 0008), polled beside the own stream with the gate re-asked before every poll and every fetch referenced between the poll and the checkpoint commit — proven end to end over scripted transports through the real gate and runtime: `two_live_admitted_connectors_over_one_subject_lift_the_hold_and_one_does_not`, and `replays_under_two_vendors_admissions_back_no_vendor_and_never_lift_the_hold`. A lapsed standing admission now withdraws its source from the platform on the round it lapsed (`a_lapsed_standing_admission_withdraws_the_source_from_the_platform`; a connector arm's lapse also stops the node, as the fast brain's does). **Cost, stated plainly:** in every shipped deployment the hold stands — `deepbrain_connector` is null in all four tfvars, the proxy's bootstrap names only the ECB host, and no two shipped connectors share a subject (the shipped Frankfurter connector maps to currency series; the proof's second vendor is a stand-in mapping the ECB table onto the test's subject, over the real manifest, gate and runtime). The Terraform half is in the same change (`grep -n 'variable "deepbrain_connector"' infrastructure/terraform/variables.tf`), null everywhere with the reason beside it, so `manifest_wiring.rs`'s allowlist gains nothing. The entry this corrects follows; its replay claim is withdrawn. — Re-scored 2026-09-12 a third time (ADR 0057), from `PARTIAL`; the earlier entries that day are kept below in this cell. **The production caller is the deep brain's learning round**, which is a bounded research run in exactly the shape the section's arrow describes — the earlier correction was right that a continuously running connector is not one, and that reasoning is why the campaign wraps the round and not a poll. `EvolutionEngine::maybe_learn` assembles every window through `qip_deepbrain::campaign::assemble` (`grep -n 'campaign::assemble' backend/crates/apps/qip-deepbrain/src/evolution.rs`), called from the node loop (`grep -n 'maybe_learn(platform' backend/crates/apps/qip-deepbrain/src/node.rs`). Each step of the arrow is a named line there: the stream is resolved to its door — a catalogue-admitted connector the platform holds an admission for, which since finding S1 includes a replay that names the connector it was recorded from and is run through that connector's own standing licensing gate by the root (`grep -n 'recorded_from' backend/crates/apps/qip-deepbrain/src/main.rs`), or a stream the platform generated — and a stream that is neither is refused *as a round outcome*, on the round line and counted in `qip_research_campaigns_refused_total{gate="door"}`, per subject, with the node cycling on (`grep -n 'RefusedAtDoor' backend/crates/apps/qip-deepbrain/src/campaign.rs`; finding B1 — until then the refusal was an error, `maybe_learn` propagated it, and the deep brain stopped on its first due learning round over an undeclared replay; a connector-fed deep brain now fails closed per subject, not per process, finding F5); the window is referenced and recorded on the platform's ledger (hash verification across rounds, with the kernel's consequence on a revision); fetched into a `FetchCampaign` under a stated `CacheBound` (`grep -n 'CAMPAIGN_TTL\|CAMPAIGN_CACHE_ENTRIES' backend/crates/apps/qip-deepbrain/src/campaign.rs`) and **read back out of the cache** for the desk to fit on; sketched; assessed for concentration; and closed, with the manifest journaled as a `ResearchCampaignClosed` record under its own permanently retained topic (`grep -n 'Topic::ResearchCampaignClosed' backend/crates/runtime/qip-kernel/src/references.rs`) and nowhere else — it sat under `LearningCompleted`, whose every frame `Platform::journal_entries` decodes as a cycle entry, so the first close broke the journal's read (finding S3/F3), and a second copy in the node's key-value store was two claims about one fact (finding S8/F4); whether the campaign was flagged or drew on the fallback is read off the manifest rather than carried beside it. **All five mitigations in the section's table, each with a production seam:** *source revises history after use* — the kernel names, on the log, every closed campaign that read the withdrawn bytes, and a campaign's own manifest is flagged only where it read them (`grep -n 'pub fn contradicts' backend/crates/services/qip-data-finder/src/ledger.rs`; `grep -n 'flag_revised' backend/crates/apps/qip-deepbrain/src/campaign.rs`), proven by `the_campaign_that_used_the_original_is_flagged_and_the_one_that_used_the_revision_is_not`; *research is slower than a local copy* — the campaign-scoped TTL cache is on the fit's path, not beside it; *regulatory demand for data not retained* — the closed manifest is on the log, permanently; *vendor withdraws historical access* — `assess_concentration` over vendor doors only gates promotion past validation in `EvolutionEngine::turn` (§56.3 rule 31; a generated stream is not a vendor, finding F6) and §22.1's fallback series is what `assemble` draws on when a subject's own stream no longer holds enough history, recorded on the manifest (`grep -n 'record_fallback' backend/crates/apps/qip-deepbrain/src/campaign.rs`); *sketch or reservoir error affects a model* — the count-min sketch's declared `(ε, δ)` rides on the manifest beside the statistic and the fit refuses when the declared error exceeds its tolerance (`grep -n 'tolerable_for' backend/crates/apps/qip-deepbrain/src/campaign.rs`). Each seam has a test that was mutation-verified: `cargo test -p qip-deepbrain --lib -- campaign::` (seven since the second review), `a_universe_backed_by_one_source_is_held_back_from_promotion_past_validation`, `a_replay_the_door_refuses_is_a_learning_outcome_and_the_node_keeps_cycling` and, since the second review, `two_live_admitted_connectors_over_one_subject_lift_the_hold_and_one_does_not` in place of the withdrawn replay test. **Cost, stated plainly (finding S1, as first written — withdrawn above):** the gate was said to open without a live vendor call, for two replays recorded from two admitted connectors; the second review showed that to be the defect, and the test that asserted it is gone — and in every shipped deployment it is shut, for three reasons each sufficient alone: the shipped deep brain's one stream is the synthetic exchange, which counts for no vendor; with the four shipped connectors no subject has two independent vendors (crypto is Coinbase alone, equities Alpaca alone, FX Frankfurter alone, prediction Kalshi alone, and the last two are refused by the catalogue until their terms are read), so promotion is held everywhere until the catalogue gains a second vendor for a subject; and a single deep-brain process feeds one stream, so even then both vendors' references reach one ledger only through two learning streams, which no deployment shape provides. The row stays `REACHED` on this document's own bar — the non-test path exists and the gate is proven to open through it — and a reader should take the hold, not the promotion, as what a shipped deployment does. Kept for the record, the earlier entries follow. — Re-scored 2026-09-12 (ADR 0056), corrected later the same day. A named, bounded `FetchCampaign` exists — `grep -n 'pub struct FetchCampaign' -A 10 backend/crates/services/qip-data-finder/src/campaign.rs` — with a TTL- and entry-bounded `ResearchCache` that never exceeds its stated ceiling, a `CampaignManifest` that survives the campaign's close while the cache is deleted, and `assess_concentration`: `grep -rn 'two registered\|second source\|concentration risk' backend/crates/services/qip-data-finder/src` finds `campaign.rs`'s own doc comments and `ConcentrationVerdict::MINIMUM_VIABLE_SOURCES = 2`. **3 of the section's 5 named mitigations are built, 1 is partial, 1 is not built**: *source revises history after use* (built, hash verification + manifest flagging), *research is slower than a local copy* (built, the bounded TTL cache), *regulatory demand for data not retained* (built, the manifest), *vendor withdraws historical access* (partial — the two-sources half only; the three-year bar-level fallback series is §22.1's still-absent retention taxonomy), *sketch or reservoir error affects a model* (not built — §22.2 records no sketch exists anywhere in this codebase). Not `REACHED`, and not for the reason previously given here ("no HTTP transport"): see §22.3's corrected row — every shipped connector already fetches real bytes, but `FetchCampaign::fetch` takes a `DataReference` and inherits the same `RegisteredSource` blocker. There is a second, independent reason this would not close even if that blocker were lifted: §22.4's own module doc (`backend/crates/services/qip-data-finder/src/campaign.rs:1-59`) describes a bounded, TTL-scoped, closed research run — "campaign starts → resolve references → fetch into TTL cache → verify hashes → … → cache expires and is deleted" — and the four shipped connectors are the opposite shape: continuously running, restart-surviving streams with their own journal and dedup window (`ConnectorFeed::journal_to`), never closed. Forcing one connector-poll per `FetchCampaign::open`/`close` would misrepresent a permanent feed as a bounded research run for no benefit `ResearchCache`'s TTL eviction actually provides here; this correction declines to force that fit, per its own brief. `assess_concentration` is unaffected by any of this — it takes source ids directly, not a `RegisteredSource` or a `DataReference` — and remains reachable only from a test today. Tested and mutation-verified (`grep -c '#\[test\]' backend/crates/services/qip-data-finder/src/campaign.rs` — 5), depends on 22.3 as the blueprint requires. |
| 23.1 | Allocation Across Ten Thousand Strategies | PARTIAL | LEVEL 1 is REACHED: `grep -n 'FamilyClustering::new' backend/crates/runtime/qip-kernel/src/central/structure.rs` clusters realised returns by stress correlation, and `grep -n 'central.family_structure(' backend/crates/runtime/qip-kernel/src/platform.rs` shows `stage_learn` calling it every cycle. LEVELs 2 and 3 are ABSENT: nothing allocates across the families or distributes a family budget by capacity — the kernel's own comment at that call site says "this measures and allocates nothing: no seam in this platform consumes a family", and `grep -rn 'FamilyId' backend/crates --include=*.rs` finds no consumer outside `qip-optimization-engine` and its tests. Cardinality constraint and effective-breadth objective: not built. **LEVEL 2 stays `ABSENT` after ADR 0064, and the kernel comment quoted above is still accurate — of the family it is about.** Since ADR 0064 a *provenance* family — the sweep a candidate was registered under, `StrategyCandidate::family`, which is the identity the foundry actually mints — is reviewed against funding standing in LEARN and **allocates nothing**: `grep -n 'fn review_family_allocation' backend/crates/runtime/qip-kernel/src/platform.rs`. The *correlation* family this row's LEVEL 1 is about still has no consumer, and the two are different objects; the kernel's comment at the `family_structure` call site was amended in place to say which of the two it speaks for rather than left to be read as covering both. Nothing about LEVEL 2 changed: allocating across families needs a weight, and every writer of one has no production caller (ADR 0064 names the three). **The LEVEL 1 measurement is REACHED in the code and empty in every deployment, and ADR 0075 is the record of why.** `stage_learn` calls `family_structure` on every cycle and it returns nothing on every cycle: the clustering runs over the realised calendar, the calendar is filled by `retain_grants`, and `retain_grants` iterates `self.envelopes`, whose only writer is `CentralPlane::issue` — `grep -rn --include=*.rs 'envelopes\.insert' backend/crates` for the writer, and `grep -rn --include=*.rs '\.issue(' backend/crates \| grep -v '/tests/'` for the callers. `retain_grants` additionally filters on `self.factory.baseline(strategy).is_some()`, and `grep -rn --include=*.rs 'set_baseline' backend/crates` prints the definition and no caller at all. Both gates are the promotion signature, which no deployment can complete. So the row is scored REACHED against this document's stated bar and measures nothing, and the bar rather than the score is what a reader should check. |
| 23.2 | Hardware | NARRATIVE | IBM processor counts, gate depths, fidelities and the Starling roadmap. Facts about somebody else's hardware. The one design claim — every workload sized to ~200 binary variables — has no expression in code: `grep -rn 'max_variables\|MAX_VARIABLES' backend/crates --include=*.rs` returns nothing, so nothing refuses an oversized instance. |
| 23.3 | Regime-Conditional Allocation | REACHED | A regime *is* classified in production — `grep -n 'fn market_regime' backend/crates/runtime/qip-kernel/src/platform.rs` — but it feeds the cost router's intelligence rung, not allocation: `grep -rn 'regime' backend/crates/services/qip-portfolio-engine/src backend/crates/services/qip-optimization-engine/src` finds two doc comments and no code, so no family weighting shifts with it and the "when uncertain, concentrate in regime-agnostic arbitrage" rule is nowhere expressed. The six-row regime/favours table has no counterpart in code. |REACHED | **Re-scored 2026-09-14 (ADR 0066).** `regime_allocation::narrow` is called from `Platform::build_proposal` beside the fill-record cap — `grep -n 'regime_allocation::narrow' backend/crates/runtime/qip-kernel/src/platform.rs` — and the two compound by multiplication, neither able to raise a bound because `Stance::multiplier` has no arm above 1.0. Proven by a real cycle, not by construction: the reference leg in `qip-kernel/tests/valuation_seam.rs::a_declined_pattern_and_a_fill_pattern_on_one_instrument_compound_a_halved_budget_with_a_halved_bound` moved from the mandate's 8% cap to 4%, and removing the call site returns it to 8% and fails that test by name. **What it costs, recorded rather than buried:** nothing attributes an instrument to an alpha family in production, so `unattributed_multiplier` — the minimum over the table — applies to *every* sized name, at 0.75 or 0.5. That was chosen over reading an unattributed instrument as unaffected, which would have made the reader invisible in the only state a deployment reaches. The favouring half carries no number anywhere. **Re-scored 2026-09-14 (ADR 0066): that is no longer true.** `regime_allocation::narrow` is called from `Platform::build_proposal` beside the fill-record cap (`grep -n 'regime_allocation::narrow' backend/crates/runtime/qip-kernel/src/platform.rs`), and the two compound by multiplication — neither can raise a bound, because `Stance::multiplier` has no arm above 1.0. Proven by a cycle rather than by construction: the reference leg in `qip-kernel/tests/valuation_seam.rs::a_declined_pattern_and_a_fill_pattern_on_one_instrument_compound_a_halved_budget_with_a_halved_bound` moved from the mandate's 8% cap to 4%, and deleting the call site returns it to 8% and fails that test by name. **What it costs, stated rather than buried:** nothing attributes an instrument to an alpha family in production, so `unattributed_multiplier` — the minimum over the table — applies to *every* sized name at 0.75 or 0.5. That was chosen over reading an unattributed instrument as unaffected, which would have made the reader invisible in the only state a deployment reaches. |
| 23.4 | Multi-Horizon Reconciliation | REACHED | `grep -n 'arm_horizon_gate' backend/crates/runtime/qip-kernel/src/platform.rs` — `stage_learn` calls it every cycle → `grep -n 'pub fn arm_horizons' backend/crates/runtime/qip-kernel/src/central/plane.rs` → `grep -n 'pub fn pools' backend/crates/runtime/qip-kernel/src/central/horizon.rs`, which charges the years pool against the unfunded-commitment liability measured at `now`. **A composition root now sets `CentralConfig::horizons`.** `qip-deepbrain`'s `load_central_horizons` reads `QIP_CENTRAL_HORIZONS_PATH`, parses it as a `HorizonPolicy` and overlays it onto `CentralConfig::default()` before the platform assembles — refusing to start on a file present but malformed, exactly `load_universe`'s posture — rather than the fastbrain, chosen because a horizon policy is a claim about which horizon each *strategy* sits at, and strategy lifecycle is this node's, not the execution-only fast path's: `grep -n 'fn load_central_horizons\|fn parse_central_horizons' backend/crates/apps/qip-deepbrain/src/main.rs`. Scored `REACHED` on the same bar as `Cell::work`: the Terraform half now exists too — `grep -n 'variable "central_horizons_file"' infrastructure/terraform/variables.tf` — a root variable, null by default, mounted on `qip-deepbrain` exactly the way `capital_fabric_file` is mounted on `qip-api` (`infrastructure/terraform/catalogue.tf`'s `optional_config_files.deepbrain`); every environment still leaves it null (`grep -rn '^\s*central_horizons_file\s*=' infrastructure/environments/*/terraform.tfvars` returns nothing) — so the path is in the binary, mountable from a reviewed root variable, and nobody has selected it. `qip-fastbrain` still sets no view on it, which is a scope decision rather than an oversight, stated in the loader's own doc comment. The strategies it would reconcile across horizons are still never promoted on the committed synthetic tape (see 20.2). |
| 23.5 | The Hybrid Pattern | REACHED | The classical arm runs every cycle: `grep -n 'fn construct_from' backend/crates/runtime/qip-kernel/src/platform.rs` is called from `stage_decide`, reaching `grep -n 'self.router.solve(' backend/crates/services/qip-portfolio-engine/src/construction.rs` → `grep -n 'pub fn solve' backend/crates/services/qip-optimization-engine/src/router.rs`, which scores classical and quantum on the same objective and records the delta (`grep -n 'fn measured_quantum_advantage\|fn improvement_over_classical' .../router.rs`). The quantum arm is unreached and would not be IBM if it were: `grep -n 'quantum_enabled' backend/crates/runtime/qip-kernel/src/config.rs` defaults false and `grep -rn 'with_quantum()' backend/crates/apps --include=*.rs` finds no caller, and when enabled `grep -n 'SimulatedProvider::new' backend/crates/runtime/qip-kernel/src/platform.rs` is what is installed. Quantum cannot block the plane, as the section requires — the classical solver runs first and unconditionally. |
| 23.6 | Adaptive Cadence and Sequencing | REACHED | No trigger table and no "nothing changed, do not run" saving: `grep -rn 'AdaptiveCadence\|should_run\|whitelist_hit_rate\|inventory_deviation' backend/crates --include=*.rs` returns nothing. `stage_decide` calls `router.solve` on every cycle regardless of whether any of the four signals moved, which is the opposite of the section's decision. The build order it prescribes (classical first, quantum as a second solver) is what happened — that is scored under 23.5. |REACHED | **Re-scored 2026-09-14 (ADR 0066).** `adaptive_cadence::review` is folded into `stage_learn` in the same `(summary, problems)` shape as the sizing review — `grep -n 'adaptive_cadence::review' backend/crates/runtime/qip-kernel/src/platform.rs` — and pinned by `qip-acceptance/tests/regime_allocation.rs::the_learn_stage_carries_the_cadence_review_so_the_wiring_and_not_only_the_module_is_proven`, which runs a cycle and reads the stage. That test exists because the module's own suite called `review` directly and stayed green with the call site deleted. The rule the design rests on: a cadence may skip work that produces a **record** and never work that produces a **control**, which is why §23.3's narrowing is recomputed every construction and is deliberately absent from the plan. Two of §23.6's four trigger rows are fed; two are named unbuilt with the reason — no whitelist hit rate is measured anywhere, and the centre holds no inventory-deviation figure. **Re-scored 2026-09-14 (ADR 0066).** `adaptive_cadence::review` is folded into `stage_learn` in the same `(summary, problems)` shape as the sizing review (`grep -n 'adaptive_cadence::review' backend/crates/runtime/qip-kernel/src/platform.rs`), pinned by `qip-acceptance/tests/regime_allocation.rs::the_learn_stage_carries_the_cadence_review_so_the_wiring_and_not_only_the_module_is_proven`, which runs a cycle and reads the stage. That test exists because the module's own suite called `review` directly and stayed green with the call site deleted. The rule the design rests on: a cadence may skip work producing a **record** and never work producing a **control**, which is why §23.3's narrowing is recomputed every construction and is deliberately not in the plan. Two of the four trigger rows are fed; two are named unbuilt — no whitelist hit rate is measured anywhere, and the centre holds no inventory-deviation figure. |
| 23.7 | Scenario and Stress | PARTIAL | Built and tested: `grep -n 'pub struct StressTester\|pub fn standard_library' backend/crates/services/qip-simulation-engine/src/scenario.rs` covers historical replay and correlation stress with a scenario library and per-position impact. **A production caller now exists.** `qip-kernel`'s `Platform::stress_the_book`, called from `stage_simulate` every cycle with an open position, applies every scenario in the standard library to the book as it stands, at the beta the market factor (ADR 0058) measured for each position from the platform's own tape — a position it cannot measure is counted as unmodelled rather than credited as immune, because crediting a diversification nobody measured would understate the risk this control exists to surface — and publishes each scenario's loss fraction on `qip_simulation_stress_loss_fraction` under the scenario's own name: `grep -rn 'StressTester\|standard_library()' backend/crates --include=*.rs` now finds the call in `qip-kernel/src/platform.rs` beside the crate's own tests. Two of the four methods are still not built at all: causal propagation through mechanisms and adversarial worst-plausible-sequence construction, which is why this is `PARTIAL` and not `REACHED`. | **Same root cause as §19.1:** without a production factor model there are no betas to shock, so a stress run would report every position `unmodelled` — a control that executes and measures nothing. See §19.1 for the missing wire and why it wants an ADR. | **Re-scored `UNREACHED` → `PARTIAL` on 2026-09-08: the stress test has a production caller and the shocks reach the book.** The SIMULATE stage — which until now counted its own history and did nothing with it — applies `standard_library()` to the open positions on every cycle: `grep -n 'fn stress_the_book' backend/crates/runtime/qip-kernel/src/platform.rs`, reached from `stage_simulate`, with the result held at `Platform::stress_report`. The exit is priced off the platform's own `CostModel` rather than a figure invented at the seam. Two things make this a control rather than a report: a scenario losing more than `STRESS_LOSS_TOLERANCE` of equity is raised as a problem on the stage — the first production caller of `ScenarioResult::breaches` — and a position the factor could not measure is *counted*, not dropped, because a loss figure over three of a book's ten positions and the same figure over all ten are different facts. `cargo test -p qip-kernel --test stress` covers all three. Two names for one movement had to be reconciled to get here: the library shocks `equity` and the factor is called `market`, ADR 0058 wrongly asserted they already matched, and the mapping is now the single constant `qip_risk::market_factor::EQUITY_SHOCK` with an acceptance test (`architecture.rs`) refusing a rename on either side. `PARTIAL` and not `REACHED` for the reason the left column already gives: two of the section's four methods — causal propagation through mechanisms, and adversarial worst-plausible-sequence construction — are not built at all. And a third honest limit, visible in the reports the tests print: the library shocks `rates`, `credit`, `volatility`, `commodity` and `fx`, and this platform has a beta for none of them, so those shocks are named on `unmodelled_factors` and contribute nothing. One factor is the smallest honest model the tape supports, and the report says so rather than implying the book was stressed on five.
| 25.1 | The Decision Chain | PARTIAL | Reached: most of the chain has a production seam and the "only three layers can say yes" shape holds. Mandates — `grep -n 'UserLedger::with_desk\|user_ledger.enrol' backend/crates/runtime/qip-kernel/src/platform.rs`; risk constrains only — `grep -n 'monitor.*observe(&risk_state' backend/crates/runtime/qip-kernel/src/platform.rs`; optimiser inside the envelope — see 23.5; grants shipped outward — `grep -n 'grant_manifests(' backend/crates/apps/qip-api/src/mesh.rs`, served from `routes.rs` (`grep -n 'mesh::pending_capital' backend/crates/apps/qip-api/src/routes.rs`); the regional gate vetoes only — `grep -n 'metrics.refusal(' backend/crates/edge/qip-edge/src/cell.rs`; and grants age out rather than being revoked — `grep -n 'capital envelope has expired' backend/crates/edge/qip-edge/src/cell.rs` finds the cell stopping on its own. Unreached: the transfer gate's seven checks are only evaluated for an intent routed through `decide_fabric`, and `grep -rn 'decide_fabric(' backend/crates --include=*.rs` finds one production caller (`reconcile_wallet`, from `stage_learn`) with the rest under `/tests/`; the API exposes the checks as a read-only view (`grep -n 'transfer_gate_checks' backend/crates/apps/qip-api/src/ledger_views.rs`). |
| 25.2 | The Capital Engine | PARTIAL | Reached: how much to invest, and the reserve against unfunded commitments. `grep -n 'CapitalAllocator::new' backend/crates/runtime/qip-kernel/src/central/plane.rs` and `grep -n 'self.allocator.allocate(' backend/crates/runtime/qip-kernel/src/central/plane.rs` size the book under a drawdown schedule; the reserve arithmetic is the horizon pools reached in 23.4. Unreached: **most** of the movement half — **corrected 2026-09-15, understating, and the citation was the cause.** The search was scoped to `backend/crates/apps`, a directory none of these three functions lives in, so it could only ever return nothing and proved nothing; run in the tree they live in it answers. `grep -n 'self.forecast_capital_demand(' backend/crates/runtime/qip-kernel/src/platform.rs` is called from `stage_decide` — `grep -n 'fn stage_decide' backend/crates/runtime/qip-kernel/src/platform.rs` for the span — beside the proposal it belongs to, so **where capital will have to be, and when, is computed on every DECIDE**. The planner is the part that is genuinely test-only: `grep -rn 'pre_position(\|evaluate_pre_positioning(' --include=*.rs backend/crates` finds the two methods, one internal call between them and tests, and nothing else, so *how much to move and where* is still computed by nobody. Exploration: **corrected 2026-09-15, understating, and contradicting §13.2 in the same document.** The closing clause said no capital is allocated by uncertainty or accounted separately. `grep -n 'exploration::review' backend/crates/runtime/qip-kernel/src/platform.rs` is one call inside `stage_decide` that holds the budget out of the reservation ledger before anything is sized, over the probe kinds, the upper-confidence-bound selection and the committed-versus-spent account at `grep -n 'pub enum ProbeKind\|MAXIMUM_PROBE_SHARE\|pub struct ExplorationBook\|pub fn plan' backend/crates/services/qip-capital/src/exploration.rs`. §13.2 scores exactly that and states what is still missing. The observation that survives is narrower and worth keeping: `grep -rn 'exploration' backend/crates/services/qip-capital/src/allocation.rs` does return nothing — the allocator proper knows nothing of it, because the budget is withheld upstream of allocation rather than expressed as one of its lines. |
| 25.3 | The Risk Envelope | PARTIAL | Reached, every cycle: `grep -n 'fn risk_state' backend/crates/runtime/qip-kernel/src/platform.rs` builds the state and the monitor observes it (`grep -n 'observe(&risk_state' backend/crates/runtime/qip-kernel/src/platform.rs`). Levels that can fire: per-user (mandate ceilings), per-strategy and per-instrument (notional/weight/loss caps), per-asset-class and per-venue and per-counterparty — `grep -n 'fn exposure_axes_of' backend/crates/runtime/qip-kernel/src/platform.rs` populates `sector`, `country`, `asset_class` and `venue` buckets and `grep -n 'COUNTERPARTY_AXIS' backend/crates/runtime/qip-kernel/src/platform.rs` adds the fifth — and global gross/leverage/drawdown/daily-loss plus the tail-risk pair (`grep -n 'with_tail_risk' backend/crates/runtime/qip-kernel/src/platform.rs`). **Two of the three levels that could not fire now have a producer and a veto, and nine of the ten can fire.** Per factor and per causal driver are not on the instrument record and the kernel's own comment on `exposure_axes` read as though that settled it; it did not. A factor loading is a regression over the tape every SIMULATE already runs and a causal driver is an edge DISCOVER writes into the world model every cycle: both had a producer and nothing had asked it, so a book whose every position sat downstream of one mechanism read as diversified to every control here — by instrument, by sector, by country, by venue, by counterparty — and was one shock from losing all of it at once. That is the `MaxExpectedShortfall` shape in its harsher form: not a limit that shipped and could not fire, but the level the section calls the concentration that ends firms with no limit at all. The figure is charged to `RiskState::axis_exposures` under two reserved axis names rather than to two new `LimitKind`s — `MaxCounterpartyExposure` already made the other choice and had to be undone, a map of its own being a second representation of one fact — so the veto that fires is the one already proven to fire: `grep -n 'CAUSAL_DRIVER_AXIS\|FACTOR_AXIS\|pub fn attribute\|pub fn apply' backend/crates/libs/qip-risk/src/shared_cause.rs`, the shipped caps at `grep -n 'causal-driver-concentration\|factor-concentration' backend/crates/libs/qip-risk/src/limits.rs`, and the production producer at `grep -n 'shared_cause::observe' backend/crates/runtime/qip-kernel/src/platform.rs`, one call inside `risk_state_from` and so on every cycle. `the_per_causal_driver_limit_can_actually_fire` and `the_per_factor_limit_can_actually_fire` drive positions through `Platform::risk_state` until the pre-trade checker returns `Rejected` naming the cap, with leverage and position weight asserted clean first so the refusal cannot be something else. Nothing nets: a bucket is the absolute notional of every position sharing the cause times a non-negative weight, because signing it would let a long and a short that move together report no shared exposure at all — the illusion the level exists to see through, written into the control meant to catch it (`a_long_and_a_short_that_share_one_driver_do_not_net_each_other_away`). The levels overlap, so only an equity-denominated bucket limit may measure them: `the_shipped_set_never_divides_an_overlapping_level_by_its_own_sum` refuses `MaxConcentration` over one, whose denominator would be a number nobody computed. An idle level is legible rather than silent — a declared axis present and empty, an undeclared one absent — and a level whose source could not be read files `RiskState::unevaluated` and refuses every order while it stands (`an_order_is_refused_while_a_shared_cause_level_cannot_be_read`). **The third level, per family, was deliberately left with no producer and ships no cap, and the absence is asserted rather than forgotten**: `the_shipped_set_caps_the_two_levels_that_have_a_producer_and_not_the_third`. `RiskAggregates` exposes no way to split one instrument's notional across the strategies holding it, and neither strategy key production passes to `apply_fill` — the desk's budget holder and a cell's name — is a name the factory's candidate register can resolve to a family. A cap over it today would hold the whole book under one name or nothing, which is precisely the defect this lane exists to close, re-added under a new name. The row stays `PARTIAL` on that one level. |
| 25.4 | The Liquidity Ladder | PARTIAL | Reached: the rungs classify the book every cycle. `grep -n 'pub fn liquidity_ladder' backend/crates/runtime/qip-kernel/src/platform.rs` is called from `risk_state`, feeding `days_to_liquidate` and the `liquidatable_within` fractions that `MinLiquidity` and `MaxDaysToLiquidate` fire on; `grep -n 'pub fn classify' backend/crates/libs/qip-financial/src/ladder.rs` is the seven-rung law, and `LiquidityLadder::new` refuses a non-monotonic ladder. Unreached: serving a withdrawal from the top downward — `grep -n 'pub fn plan' backend/crates/libs/qip-financial/src/ladder.rs` is built and `grep -rn 'ladder.plan(\|\.plan(amount' backend/crates --include=*.rs` finds no production caller, because no withdrawal path exists to serve (`grep -n 'withdrawal' backend/crates/services/qip-capital/src/ledger/entitlement.rs` — the capability is refused by construction). |
| 25.5 | Tax | PARTIAL | Reached: lot tracking and holding period, through the simulated venue. `grep -n 'close_lots_with' backend/crates/libs/qip-portfolio/src/position.rs` closes against lots by `LotMethod` on every fill, and the path to it is production — `grep -n 'SimulatedExchange::new' backend/crates/apps/qip-edge-node/src/gateway.rs` → `grep -n 'self.portfolio.apply_fill' backend/crates/services/qip-brokers/src/ledger.rs` — with `grep -n 'fn holding_period' backend/crates/libs/qip-portfolio/src/lot.rs` on each realised trade. Absent: everything the section calls tax. No wash-sale gate, no harvesting and no after-tax sizing — `grep -rn 'wash_sale\|harvest\|after_tax' backend/crates --include=*.rs` returns nothing; jurisdiction exists on mandates and eligibility (`grep -n 'pub struct Jurisdiction' backend/crates/services/qip-capital/src/ledger/identity.rs`) but no treatment model is keyed on it, so the pluggable treatment model the section promises is not there. |
| 25.6 | The Cross-Margin Model | PARTIAL | None of the six concerns is built: `grep -rn 'CrossMargin\|cross_margin\|rehypothec\|liquidation_cascade\|collateral_graph\|MarginRegime' --include=*.rs backend/crates/` returns nothing. The nearest thing is a per-book margin requirement, not a graph of what collateralises what: `grep -n 'pub struct MarginModel\|posted_collateral' backend/crates/services/qip-capital/src/margin.rs`, and it has no caller — `grep -rn 'MarginModel' --include=*.rs backend/crates/ \| grep -v 'qip-capital/src/margin.rs'` returns only the `lib.rs` re-export. **Re-scored 2026-09-14 (ADR 0072), `ABSENT` → `PARTIAL`.** A `CollateralGraph` with per-domain coverage, `uncorrelated_excess` (the margin spiral as arithmetic) and a `cascade` terminating because a domain closes at most once rather than by an iteration cap. Derivable from platform state: what collateralises what, and portfolio versus isolated margin. **Rehypothecation, correlated collapse and multi-step cascade are library-only and deliberately not fields on the review**: a statement is keyed by venue *and* asset, so nothing this platform records says one holding stands behind two obligations — a field structurally empty on every production path is the `MaxExpectedShortfall` shape, and an `Option` always `None` is that shape wearing a type. A re-pledge chain deeper than one link is refused, because which of A's securities B passed to C is a fact B knows and this platform does not. |
| 26.1 | A Strategy Is a Specification | PARTIAL | Built half — totality is structural, not policed: `grep -n 'pub enum Expr' -A 75 backend/crates/edge/qip-strategy/src/ir.rs` shows no call/loop/recursion node, and declared feature dependencies are checked against a catalogue (`grep -n 'fn declare\|fn type_of' backend/crates/edge/qip-strategy/src/catalogue.rs`). Reached: `grep -n 'StrategyRuntime' backend/crates/edge/qip-edge/src/cell.rs`. Missing half — the record carries none of `family, universe, sizing, tier, capacity, beliefs, hypothesis`: `grep -n 'pub struct StrategySpec' -A 12 backend/crates/edge/qip-strategy/src/ir.rs` shows only `id, subject, rules, validity`. |
| 26.2 | Compilation | PARTIAL | Built and production-reached: feature deduplication / common-subexpression elimination into a shared arena with a stated ratio, type checking, and a refusal above a cost budget — `grep -n 'fn compile\|fn deduplication_ratio\|pub struct CompilerLimits' backend/crates/edge/qip-strategy/src/compile.rs`; the non-test caller is `grep -n 'StrategyCompiler::new' backend/crates/runtime/qip-kernel/src/central/foundry.rs` reached from `grep -n 'StrategyFoundry::new' backend/crates/apps/qip-deepbrain/src/evolution.rs`. Absent: subscription index, tier partitioning, universe bitmaps, layout packing, belief binding, feasibility pre-binding — `grep -rn 'subscription_index\|hot_tier\|universe_bitmap\|belief_binding' --include=*.rs backend/crates/edge/qip-strategy/` returns nothing. |
| 26.3 | The Loop and Budget | PARTIAL | The pipeline shape exists and runs in production order — features, subscription-free strategy loop, intents, netting, gate, send: `grep -n 'fn work' backend/crates/edge/qip-edge/src/cell.rs` and `grep -n 'net(\|netting_ratio(' backend/crates/edge/qip-edge/src/cell.rs`. Budgets are measured rather than asserted per stage: `grep -n 'fn .*costs_what_the_budget_says\|fn report(' backend/crates/tests/qip-acceptance/tests/performance.rs` gives per-operation ceilings for book apply, feature evaluation, strategy evaluation and arbitrage detection. Absent: the eight named stage budgets and any assertion of the `< 70 µs` total — `grep -rn '70 *µs\|70_000\|STRATEGY ENGINE TOTAL' --include=*.rs backend/crates/` returns nothing. |
| 26.4 | Why the Budget Holds | PARTIAL | Built: cost is bounded at compile time and refused rather than trimmed (`grep -n 'pub struct CompilerLimits' -A 18 backend/crates/edge/qip-strategy/src/compile.rs`), and the deploying cell refuses a program over its own budget (`grep -n 'StrategyRuntime::with_budget' backend/crates/edge/qip-edge/src/cell.rs`) — a production seam. Absent: the three answers that bound *fan-out* rather than per-strategy cost — universe bitmaps, the hot-tier cap, and pointer-swap recompilation off the hot path: `grep -rn 'universe_bitmap\|hot_tier\|swap_plan' --include=*.rs backend/crates/edge/` returns nothing. |
| 27.1 | Internal Crossing | REACHED | Crossing at the prevailing mid, the forty-percent cap refused whole rather than trimmed, and a journal entry naming both contributors: `grep -n 'internal_cross_cap\|internal_cross_price\|fn cross_internally\|fn crossing_window' backend/crates/edge/qip-edge/src/cell.rs`. Production path: `Cell::work` → cross, with the counter recorded at the seam — `grep -n 'internal_cross' backend/crates/edge/qip-edge/src/telemetry.rs` — and `Cell::work` is called from non-test code at `grep -n 'fn run_pass' backend/crates/apps/qip-edge-node/src/pass.rs` / `grep -n 'run_pass(' backend/crates/apps/qip-edge-node/src/main.rs`. Proof per site: `grep -n 'cross' backend/crates/edge/qip-edge/tests/crossing.rs`. |
| 27.2 | Rules Across Venues and Cycles | PARTIAL | Four of five rows built and reached: grouping is by instrument, venue **and** representation, so a different venue and a different representation each get their own order, and a cycle leg is isolated by type — `grep -n 'struct NettingKey\|NettingPolicy::NoNet\|pub fn net(' backend/crates/libs/qip-contracts/src/intent.rs`; the no-net flag cannot be forgotten because `CycleLeg` has no nettable form (`grep -n 'From<CycleLeg>' backend/crates/libs/qip-contracts/src/intent.rs`). **The router half is now built and tested and is still not reached**, and the distinction is the whole reason this row has not moved. `qip_routing::consolidate::Consolidator` puts intents carrying `UNSPECIFIED_VENUE` onto the venue the router prices best, before netting, so two strategies that named no venue collapse into one order instead of crossing each other at two placeholders — `cargo test -p qip-routing --test consolidation` runs nine tests including `two_strategies_that_named_no_venue_become_one_order_rather_than_two`, `a_buy_and_a_sell_that_named_no_venue_cancel_internally_instead_of_crossing_each_other`, `an_intent_that_named_its_venue_is_left_exactly_where_it_was` and `a_cycle_leg_that_arrived_without_a_venue_is_refused_rather_than_moved`. An intent carrying the placeholder cannot leave: the vector is private, `Consolidation::intents` is the only reader, and the function returns `Err` if the invariant ever failed (`grep -n 'fn intents\|consolidation left' backend/crates/edge/qip-routing/src/consolidate.rs`). What is missing is one call: `grep -rn 'Consolidator' --include=*.rs backend/crates | grep -v qip-routing` prints nothing, and the seam that would call it is `Cell::work` immediately before `net(intents)` in `qip-edge/src/cell.rs` (`grep -n 'let nets = net(intents)' backend/crates/edge/qip-edge/src/cell.rs`). Also missing: the market-making row (quotes skewing off the net). **Corrected 2026-09-15, understating**: "there is no quoting engine (see §29.1)" cites a row that has read `REACHED` since 2026-09-14. There is one. `grep -n 'quote_loop::review' backend/crates/runtime/qip-kernel/src/platform.rs` prints two lines, both exits of `stage_act`, and it computes a fair value, a decomposed half spread and an inventory skew per instrument. What keeps this row's market-making seat empty is narrower than the absence of an engine: the loop skews off the position the platform actually holds, not off the netting seam's output, and `QuotePair` carries no venue and no side — `sed -n '/^pub struct QuotePair/,/^}/p' backend/crates/services/qip-execution-engine/src/quoting.rs | grep -c '^    pub '` prints 7, and neither a venue nor a side is among those seven — so nothing skews a quote off the net or could place one if it did. |
| 28.1 | Correlated Exposure | PARTIAL | Built and production-reached: per-axis concentration caps as a share of gross, and the cross-cell crowding question no single cell can answer — `grep -n 'pub struct ConcentrationLimits' -A 8 backend/crates/services/qip-capital/src/exposure.rs`, `grep -n 'fn crowded\|fn concentrations' backend/crates/services/qip-capital/src/exposure.rs`, reached via `grep -n 'AggregateExposure\|ConcentrationLimits' backend/crates/runtime/qip-kernel/src/central/plane.rs`. **Corrected 2026-09-15, understating — and until this correction §28.1 and §25.3 contradicted each other inside one document.** This cell called all five of the section's controls absent and cited that search as returning nothing; `grep -rn 'causal_driver\|effective_breadth\|family_cap' --include=*.rs backend/crates/` answers, and two of the five ship as caps proven to fire. The per-causal-driver and per-factor levels are charged to `RiskState::axis_exposures` under two reserved axis names (`grep -n 'CAUSAL_DRIVER_AXIS\|FACTOR_AXIS' backend/crates/libs/qip-risk/src/shared_cause.rs`), ship as caps at `grep -n 'causal-driver-concentration\|factor-concentration' backend/crates/libs/qip-risk/src/limits.rs`, and have a production producer on every cycle at `grep -n 'shared_cause::observe' backend/crates/runtime/qip-kernel/src/platform.rs`, one call inside `risk_state_from`; `grep -n 'the_per_causal_driver_limit_can_actually_fire\|the_per_factor_limit_can_actually_fire' backend/crates/runtime/qip-kernel/src/platform.rs` drives a book until the pre-trade checker rejects on each cap by name, which is the thing `MaxExpectedShortfall` could never do. §25.3 has said all of this since the caps landed, and states the limits of it at length; **read that cell, not this one, for the argument.** Still absent, and now the whole of what holds this row short: a per-family cap, left deliberately without a producer and the absence asserted rather than forgotten (`grep -n 'the_shipped_set_caps_the_two_levels_that_have_a_producer_and_not_the_third' backend/crates/libs/qip-risk/tests/shared_cause.rs`); an effective-breadth floor, where `grep -rn 'effective_breadth' --include=*.rs backend/crates` does return nothing; and automatic tightening on rising realised correlation. The instrument-record axes are still the ones `grep -n 'fn exposure_axes_of' -A 14 backend/crates/runtime/qip-kernel/src/platform.rs` builds, with counterparty added beside them — the two shared-cause axes are seated in the same exposure map rather than in that function, which is how a reader of this cell could check the axis list and conclude, wrongly, that no cause-level control existed at all. |
| 29.1 | The Quote Loop | REACHED | No quoting engine exists: `grep -rni 'fair_value\|half_spread.*skew\|inventory_skew\|toxic' --include=*.rs backend/crates/edge/ backend/crates/services/qip-execution-engine/` returns nothing for the loop's components, and `grep -rni 'MarketMaker\|market_making' --include=*.rs backend/crates/` matches only an acceptance-test string. The cell can rest a limit order but never computes a two-sided quote: `grep -n 'fn rest\|resting' backend/crates/edge/qip-edge/src/cell.rs`. |REACHED | **Re-scored 2026-09-14 (ADR 0067).** `quote_loop::review` is called at **both** exits of `stage_act` — `grep -n 'quote_loop::review' backend/crates/runtime/qip-kernel/src/platform.rs` prints two — and pinned by `qip-acceptance/tests/quote_loop.rs::the_act_stage_carries_the_quote_loop_so_the_wiring_and_not_only_the_module_is_proven`. Both exits, because the early return is taken when nothing was releasable, which is precisely the cycle a market maker is still quoting through; wired at one exit only, deleting the call left the whole suite green. Fair value, a decomposed half spread, inventory skew and size are computed per instrument with a two-sided book. **The boundary is structural, not asserted:** `QuotePair` carries two prices and a size and no venue, no side, no client id and no time in force, and no function in this workspace turns one into an `Order` — checked over production source *and* behaviourally, by ending a real pass with no order and no fill. Adverse selection is a bar-resolution proxy and the ADR says so. **Re-scored 2026-09-14 (ADR 0067).** `quote_loop::review` is called at **both** exits of `stage_act` (`grep -n 'quote_loop::review' backend/crates/runtime/qip-kernel/src/platform.rs` prints two), pinned by `qip-acceptance/tests/quote_loop.rs::the_act_stage_carries_the_quote_loop_so_the_wiring_and_not_only_the_module_is_proven`. Both exits, because the early return is taken when nothing was releasable — precisely the cycle a market maker is still quoting through; wired at one exit only, deleting the call left the whole suite green. **The boundary is structural rather than asserted:** `QuotePair` carries two prices and a size and no venue, no side, no client id and no time in force, and no function in this workspace turns one into an `Order` — checked over production source *and* behaviourally, by ending a real pass with no order and no fill. Adverse selection is a bar-resolution proxy and the ADR says so. |
| 29.2 | Quote Rate Management | PARTIAL | Four of six controls built **and reached**, up from one. Already there: the requote threshold plus per-order and per-instrument throttle budgets — `grep -n 'pub struct RepricePolicy\|enum ThrottleScope\|enum HoldReason\|fn consider' backend/crates/edge/qip-routing/src/reprice.rs` — wired at the node's venue seam by `grep -n 'RepricePolicy\|Requoter' backend/crates/apps/qip-edge-node/src/main.rs` and `grep -n 'struct Requoter\|RequotingPlacer' backend/crates/apps/qip-edge-node/src/reprice.rs`. New: a per-venue message token bucket, the message-to-trade monitor and a mass cancel on halt — `grep -n 'pub struct RateLimits\|pub struct QuoteBudget\|pub fn admit\|pub fn observe_trade\|narrowed_reserve' backend/crates/edge/qip-edge/src/quoting.rs` and the seams at `grep -n 'GATE_QUOTE_BUDGET\|GATE_MASS_CANCEL\|pub fn mass_cancel' backend/crates/edge/qip-edge/src/cell.rs`, reached on the production pass through `grep -n 'withdraw_expired\|cell.work' backend/crates/apps/qip-edge-node/src/pass.rs`. The failure prevented is specific: a session exceeding a venue's message rate or its message-to-trade ratio is throttled or disconnected *by the venue*, at a moment nobody chose, with resting orders the cell can then no longer withdraw. So the cell runs out of budget deliberately, on its own arithmetic, and says so. Three properties are load-bearing. **Withdrawing is not sending** — placements may spend down to the withdrawal reserve and no further, a withdrawal may spend what is left, because a budget that refused a cancel after a session of quoting would leave exposure on a venue the cell had decided to leave, the control making worse the thing it exists to prevent (`a_cancel_is_funded_from_the_reserve_after_placements_have_spent_the_rest`, written at a single instant so the refill cannot fund the cancel instead). **Nothing reads a clock**: refill is integer arithmetic over elapsed nanoseconds with the sub-token remainder carried, and a `now` earlier than the last observation refills nothing rather than refilling backwards, so the same pass timestamps admit the same messages on a replay. **A venue the cell was not configured for is refused rather than minted**, which is what keeps `qip_edge_quote_budget_tokens{venue}` bounded by a deployment-time list. Also proven: `a_net_is_refused_before_it_reaches_the_venue_once_the_quote_budget_is_spent`, `a_cell_that_has_sent_nothing_publishes_a_full_budget_rather_than_no_series`, `a_halted_cell_withdraws_every_order_it_had_resting_and_charts_it_as_a_mass_cancel` (counted apart from the expiry counter, because a withdrawal on a halt and a withdrawal at a time to live are different facts), `a_halted_cell_that_cannot_withdraw_says_so_in_the_chain_rather_than_withdrawing_nothing_quietly`, and `only_a_fill_the_venue_reported_counts_as_a_trade_against_the_message_to_trade_ratio`. Short of `REACHED`: priority allocation of the budget by expected value is absent — nothing ranks a message against what it is worth, and `grep -rni 'priority_alloc\|expected_value' --include=*.rs backend/crates/edge/` returns nothing. The sixth control has never been named in this cell and should be when somebody reads §29.2 beside it; a row that cannot say what its own remainder is has not finished scoring itself. |
| 29.3 | Market Creation | REACHED | Nothing originates a market or gates one: `grep -rni 'market_creation\|origination\|counterparty_of_last_resort\|prediction_market_origination' --include=*.rs backend/crates/` returns nothing, and none of the five "gate before any market creation" checks exists. One prerequisite is built — the valuation plane — `grep -n 'pub struct CreditRegister\|term structure' backend/crates/runtime/qip-kernel/src/valuation.rs`, but a prerequisite is not the deliverable. |REACHED | **Re-scored 2026-09-14 (ADR 0067).** Gated origination through the same `stage_act` call site as §29.1. An `OriginationMandate` has private fields, one constructor and **no `Deserialize`** — it cannot be decoded into existence — and the constructor refuses a market without a defensible valuation, a causal explanation for the absence of other participants that is not "information we lack", a same-class adverse-selection model on at least thirty observations, a positive ceiling at or below a hard-coded maximum (refused, never lowered to it), and a signed per-class human approval. `qip-acceptance/tests/quote_loop.rs::an_origination_mandate_cannot_be_decoded_into_existence` pins the gateless-constructor half. **Re-scored 2026-09-14 (ADR 0067).** Gated origination through the same `stage_act` call sites as §29.1. An `OriginationMandate` has private fields, one constructor and **no `Deserialize`** — it cannot be decoded into existence — and that constructor refuses a market without a defensible valuation, a causal explanation for the absence of other participants that is not "information we lack", a same-class adverse-selection model on at least thirty observations, a positive ceiling at or below a hard-coded maximum (refused, never lowered to it), and a signed per-class human approval. `an_origination_mandate_cannot_be_decoded_into_existence` pins the gateless-constructor half. |
| 30.1 | Detection | PARTIAL | Built and production-reached: the background sweep and the profitability filter. Bellman-Ford over `-ln(rate)` returning negative cycles, with size-aware slippage priced off live depth and an exact-arithmetic confirmation — `grep -n 'fn negative_cycles\|fn search_candidates\|fn confirm_exact' backend/crates/edge/qip-arbitrage/src/search.rs`, `grep -n 'fn slippage_fraction' backend/crates/edge/qip-arbitrage/src/pricing.rs`, `grep -n 'fn scan' backend/crates/edge/qip-arbitrage/src/scan.rs`; the caller is `grep -n 'ArbitrageDesk\|arbitrage' backend/crates/edge/qip-edge/src/cell.rs`. Absent: the two mechanisms that make it fast — the policy-load candidate index and the incremental per-edge rescan. `grep -rni 'candidate_index\|incremental_scan\|cycles_containing' --include=*.rs backend/crates/edge/qip-arbitrage/` returns nothing; every scan walks the whole graph. |
| 30.2 | Path Router | PARTIAL | The eight numbered paths do not exist and nothing assigns one: `grep -rni 'MirroredInventory\|HedgedBridging\|PassiveAnchoring\|FirmQuote\|RepresentationBasis\|PayoffEquivalence\|ExecutionPath' --include=*.rs backend/crates/` returns nothing. What exists is a four-arm *shape* classifier over the edges of a found cycle, derived rather than assigned, and it names none of the router's rows: `grep -n 'pub enum PathKind' -A 12 backend/crates/edge/qip-arbitrage/src/graph.rs`, `grep -n 'fn classify' backend/crates/edge/qip-arbitrage/src/graph.rs`. The edge classes it can route over are three of the blueprint's six — `grep -n 'pub enum EdgeKind' -A 30 backend/crates/edge/qip-arbitrage/src/graph.rs` has `Transfer`, `Trade`, `Synthetic` and no mirror, basis or settlement edge. |PARTIAL | **Re-scored 2026-09-14 (ADR 0068).** The eight paths and six edge classes are built in `qip-routing`, and `Cell::scan_cycles` is a real caller: `grep -n 'CycleRouter\|GATE_PATH_ROUTER' backend/crates/edge/qip-edge/src/cell.rs`, with `cargo test -p qip-edge --test path_assignment` driving real passes. An assignment reaches two surfaces (`WorkReport.paths` and a hash-chained `Decision::CyclePathAssigned`) and a refusal reaches three, under `GATE_PATH_ROUTER` through the existing `Cell::refuse` site — so `qip_edge_refusals_total{gate}` gains a value and **no third recording site**, and the cardinality bound in `.claude/rules/domains/observability.md` is unchanged (recounted before and after: two sites both times). **`PARTIAL` and not `REACHED`, precisely: rows 1 and 2 only.** `VenueRegions::all_in` puts every configured venue in the cell's own region, so no composition holds a mirror edge and rows 3–6 are unreachable until a whitelist names a venue in another region (§31.1's work); rows 7 and 8 are unreachable one layer earlier still, because `ArbitrageDesk::new` refuses a synthetic edge outright. A cross-region cycle today is **refused**, not mis-assigned. The gate can fire and is proven to: a nine-hop ring is refused whole and no leg reaches the gateway, and an eight-hop ring is assigned, because a bound that refused everything would prove nothing. **Re-scored 2026-09-14 (ADR 0068) to `PARTIAL` — rows 1 and 2 only, precisely.** The eight paths and six edge classes are built in `qip-routing`, and `Cell::scan_cycles` is a real caller (`grep -n 'CycleRouter\|GATE_PATH_ROUTER' backend/crates/edge/qip-edge/src/cell.rs`; `cargo test -p qip-edge --test path_assignment` drives real passes). An assignment reaches two surfaces — `WorkReport.paths` and a hash-chained `Decision::CyclePathAssigned` — and a refusal goes through the existing `Cell::refuse` site under `GATE_PATH_ROUTER`, so `qip_edge_refusals_total{gate}` gains a value and **no third recording site**; the cardinality bound in `.claude/rules/domains/observability.md` was recounted before and after and is unchanged at two sites. Rows 3–6 are unreachable because `VenueRegions::all_in` puts every configured venue in the cell's own region, so no composition holds a mirror edge — §31.1's work. Rows 7 and 8 are unreachable one layer earlier, because `ArbitrageDesk::new` refuses a synthetic edge outright. A cross-region cycle today is **refused, not mis-assigned**. The gate is proven able to fire: a nine-hop ring is refused whole with no leg reaching the gateway, and an eight-hop ring is assigned, because a bound that refused everything would prove nothing. |
| 31.1 | Path 3 — The Cross-Region Solve | REACHED | No mirror edge, no distributed reference price, and no direction gating: `grep -rni 'mirror\|direction_gat\|inventory_band\|reference_price' --include=*.rs backend/crates/edge/qip-arbitrage/ backend/crates/edge/qip-edge/src/` returns nothing for any of them, and the graph node is `(object, venue)` with no region term — `grep -n 'pub struct Node' -A 6 backend/crates/edge/qip-arbitrage/src/graph.rs`. The one cross-region control that does exist is capital, not inventory: `grep -n 'fn apply_region_share\|fn rederive_region_share' backend/crates/edge/qip-edge/src/cell.rs`. **Re-scored 2026-09-14 (ADR 0073), `ABSENT` → `PARTIAL`.** Inventory bands, region state, permitted direction and a distributed reference with a threshold and a TTL are built in `qip-routing`'s `mirror`, and `Cell::work` routes against them. **The band never arrives on the wire, and that is the decision**: a band is the operator's own risk appetite for a region, so putting it in policy slot 10 would let a centre widen a cell's tolerance remotely — the direction every control here refuses. "Reduced size" is implemented as a *refusal* rather than a size, because silently halving an order is a sizing decision nobody asked for. **Re-scored 2026-09-15, `PARTIAL` → `REACHED`.** The one requirement outstanding was that `qip-edge-node` built `CellConfig` with `with_venue` only and read no venue-to-region map, so no deployed cell could hold a foreign venue and rows 3–4 of §30.2 were reachable in test only. **A production caller now exists at the composition root.** The node reads an operator-declared mirror file — `QIP_CROSS_REGION_MIRROR_PATH`, parsed in `qip_edge_node::cross_region` — and hands both halves to `assemble`, which writes `CellConfig::venue_regions` and calls `Cell::install_mirror`: `grep -n 'CrossRegionMirror::read\|MIRROR_VARIABLE' backend/crates/apps/qip-edge-node/src/main.rs` and `grep -n 'install_mirror\|venue_regions = ' backend/crates/apps/qip-edge-node/src/lib.rs`. Both halves in one function on purpose: venues abroad with no arrangement refuses every cross-region cycle, and an arrangement with no venue abroad is a band nothing looks up. Nothing is defaulted — a venue outside `QIP_VENUES`, a venue placed in the cell's own region, a region placed abroad with no measured round trip, a measurement no venue sits behind, a surplus key, half a declaration, a band that could gate nothing, a round trip of zero or less, a file too large to read whole: each stops the process with `configuration:` so it exits `EX_CONFIG` (`grep -c 'return Err' backend/crates/apps/qip-edge-node/src/cross_region.rs`). Each refusal is proven to fire **and** a good declaration proven admitted beside it: `cargo test -p qip-edge-node --test cross_region`. The mirrored cell is still paper-only, asserted on the assembled cell rather than argued — `a_cell_assembled_with_a_mirror_is_still_paper_trading_only`. Scored `REACHED` on the same bar as `Cell::work`: the path is in the binary and nobody has selected it — `cross_region_mirror_path` is `optional(string, "")` on every `execution_nodes` entry, written into `node.env` empty so the choice is visible where a reviewer reads it, and `execution_nodes = {}` in every environment (`grep -rn cross_region_mirror_path infrastructure/`, `grep -rn execution_nodes infrastructure/environments/*/terraform.tfvars`). **The "reduced size" reading above is unchanged by this lane and remains a decision for the record rather than a lane's to take.** |
| 31.2 | Saga Semantics | PARTIAL | The named seven-state saga is absent: `grep -rn 'Proposed\|Reserved\|Firing\|PartiallyFilled\|Escalated' --include=*.rs backend/crates/edge/qip-edge/src/ backend/crates/edge/qip-arbitrage/src/` returns nothing. Two of its rules are built elsewhere and neither has a production caller: a deadline-and-unwind group with a residual-risk bound (`grep -n 'fn begin_unwind\|fn unwind_orders\|enum GroupState' backend/crates/services/qip-execution-engine/src/multileg.rs`, whose only non-test reference is the `lib.rs` re-export — `grep -rn 'multileg::' --include=*.rs backend/crates/runtime/ backend/crates/apps/` returns nothing), and a refuse-never-reduce capital hold taken before an order exists (`grep -n 'pub struct RegionAllocation\|pub struct RegionTable' backend/crates/edge/qip-edge/src/reservation.rs`, which *is* reached from `Cell`). No single compare-and-swap spans a cycle's legs. |
| 31.3 | Leg Ordering | PARTIAL | Built and reached: legs are ordered by a stated reversibility statistic, dust legs are made optional against a declared fraction, transfers are realised as prefunded inventory rather than orders, and the residual after the first leg is bounded and broken out by quote currency — `grep -n 'pub struct LegRanking\|pub struct PlannedTrade\|fn plan' backend/crates/edge/qip-arbitrage/src/plan.rs`; production path: the planner sits inside the scanner (`grep -n 'planner: LegPlanner\|LegPlanner::new' backend/crates/edge/qip-arbitrage/src/scan.rs`), the desk holds the scanner (`grep -n 'scanner: OpportunityScanner' backend/crates/edge/qip-edge/src/arbitrage.rs`), and the node builds it (`grep -n 'PlanSettings::with_budget\|OpportunityScanner' backend/crates/apps/qip-edge-node/src/arbitrage.rs`). Absent: the four named policies as a *choice* per path — simultaneous fire, riskiest-leg-first, hedged entry, resting completion. `grep -rni 'simultaneous\|riskiest_leg\|hedged_entry\|resting_completion' --include=*.rs backend/crates/edge/` returns nothing; there is one ordering rule, not a policy selected by path. |
| 31.4 | The Cross-Class Hedge Map | PARTIAL | **Re-scored 2026-09-15: this row said the engine had no production caller, that its module header declared itself unwired, and that a search for its name came back empty. All three are now untrue.** The substance is unchanged — hedge sizing from a **declared** beta, never estimated, with refusals that keep a hedge from becoming a position (`grep -n 'pub struct HedgeProposal\|beta' backend/crates/libs/qip-risk/src/hedge.rs`) — but it is now reached: `grep -rn 'qip_risk::hedge\|hedge_review::review\|propose_hedge(' --include=*.rs backend/crates/runtime/ backend/crates/apps/ backend/crates/services/` names `qip-kernel/src/hedge_review.rs`, and `grep -n 'review_hedges' backend/crates/runtime/qip-kernel/src/platform.rs` gives the definition and the single call, inside `stage_decide`. The header that read "nothing in it is wired yet" now says the composition below it is what runs; an engine tested and unreached is arithmetic, not a control, which is the `MaxExpectedShortfall` shape and the reason this row existed. The load-bearing decision in the wiring is which facts a person may state. `HedgePolicyDeclaration` carries judgement only — axis, bucket, instrument, declared beta, target and de-minimis — while the contract multiplier and the lot size are read from the catalogue record at survey time (`grep -n 'contract_multiplier: record.contract_multiplier' backend/crates/runtime/qip-kernel/src/hedge_review.rs`). A configured copy of a mechanical fact is a second source of truth, and the quiet direction of that drift is a futures contract hedged at a multiplier of one: under-sized by fifty, and reading on every report exactly like a hedge that worked. An instrument the platform was not assembled over is the new `HedgeRefusal::UnknownInstrument` rather than a default of one. Exposures are built from `RiskState::position_notionals` — the same signed figures the limit checks and the liquidity ladder read — and nothing is priced, so the survey invents no mark on a platform that deliberately holds none; the hedge instrument's own price is `InstrumentState::reference_price`, a trade or a mid, and the catalogue's stored price is deliberately not a fallback. The survey terminates at the cycle report and the hash-chained log and holds no broker type. **Three limits keep this `PARTIAL`, and each is a different kind.** First, the `exposures_refused` arm is reachable and no test drives it — `grep -rn 'exposures_refused' --include=*.rs backend/crates/runtime/qip-kernel/tests` returns nothing — so by this repository's standard it is not proven to fire. It fails closed, so what an untested arm risks here is over-refusal rather than a silent hedge, which is the bearable direction and still not a proven one. Second, no deployment declares a policy: `hedge_policies` defaults to an empty vector and `grep -rn 'hedge_policies' --include=*.rs backend/crates/apps/` returns nothing, so the path runs every cycle, reports on the DECIDE line that no hedge policy is declared rather than falling silent — "nobody hedges this book" and "the policies found nothing to do" being the two states an operator must be able to tell apart — and sizes nothing at all until a deployment commits one. A caller that no configuration reaches is a caller in form. Third, the section's own subject is still absent: the map itself, which instrument hedges which exposure and how well; basis risk between hedge and exposure; hedge-instrument liquidity at proposal time, which the ladder cannot answer because it is built only over held positions and a hedge instrument is by construction not one; and degradation retirement fed from the causal graph. Each needs an input this platform does not measure, and inventing one would be the declared-beta compromise repeated where it was not argued for. `grep -rni 'HedgeMap\|hedge_map\|basis_risk' --include=*.rs backend/crates/` returns nothing. |
| 32.1 | Fill-Time Dispersion | PARTIAL | Three of five mechanisms built, and the new one is the only one of the five a cell can hold on its own. Already there: venue-native IOC/FOK selected against the venue's declared capability matrix (`grep -n 'ImmediateOrCancel\|FillOrKill\|fn supports' backend/crates/edge/qip-routing/src/ordertype.rs backend/crates/edge/qip-routing/src/venue.rs`) and a per-venue latency observation feeding a degradation verdict (`grep -n 'latency_multiple_f64\|fn record_ack\|fn observed_latency\|fn assess' backend/crates/edge/qip-routing/src/health.rs`). New: dispersion-aware admission, the pre-trade mechanism that bounds worst-case unwind cost rather than reducing its probability — a cycle whose legs would arrive further apart than the bound is refused whole before it is sent, because a cycle whose legs arrive five milliseconds apart is a position for five milliseconds and the cost of unwinding it is paid at whatever the market did in between. `grep -n 'pub struct DispersionPolicy\|pub enum DispersionVerdict\|pub struct FillTimes\|pub fn assess\|pub fn unmeasured' backend/crates/edge/qip-edge/src/dispersion.rs`, the gate at `grep -n 'GATE_FILL_DISPERSION\|fill_times.observe' backend/crates/edge/qip-edge/src/cell.rs`, reached on the production pass at `grep -n 'confirm_execution_reports\|cell.work' backend/crates/apps/qip-edge-node/src/pass.rs`. The measurement is the cell's own and nothing else's: from the instant it sent an order to the instant the venue's execution report for it was confirmed, taken where both are known. No venue tells the cell its latency, no policy payload carries one, and nothing reads a clock, so a replay of the same reports produces the same verdicts. One exception to the crate's standing discipline is argued rather than assumed: an unmeasured venue does **not** refuse, because a venue has no fill times until it fills something and would fill nothing while refused for having none — a cell that had never traded would then never trade, the discipline closing a loop on itself. What the discipline gets instead is visible silence: `FillTimes::unmeasured` is published on every pass including idle ones as `qip_edge_fill_time_unmeasured_venues`, and a cycle admitted under an unmeasured verdict says so in its journal entry rather than looking like one that passed a check (`a_cell_that_has_filled_nothing_publishes_every_venue_as_unmeasured_rather_than_fast`). Also proven: `a_cycle_whose_two_venues_fill_far_apart_is_refused_whole_on_the_pass_after_it_learns_that`, and `two_venues_that_answer_together_keep_trading_and_their_fill_times_are_published`, which is the half that distinguishes a working gate from one that refuses everything. Short of `REACHED`, including on the mechanism the section calls the single largest effect: latency-equalised dispatch and its timer wheel, which needs a dispatch thread this process does not have; passive-first on thin venues; and size decomposition at a minimum viable fraction. `grep -rni 'equalis\|equaliz\|timer_wheel\|minimum_viable\|passive_first' --include=*.rs backend/crates/edge/ backend/crates/services/qip-execution-engine/` reaches only prose saying so. |
| 32.2 | Settlement Stages and Bridges | PARTIAL | Built: the settlement *timeline* — T+0/T+1/T+2 counted in settlement days with cut-off minutes and holidays, refusing a plan that assumes Friday's instruction is Monday's balance (`grep -n 'enum SettlementConvention\|fn quote\|fn is_settlement_day' backend/crates/services/qip-capital-fabric/src/settlement.rs`), and three of the six funding stages as arithmetic (`grep -n 'fn expected' -B 6 backend/crates/services/qip-capital-fabric/src/wallet.rs` — `ledger_balance - reserved + in_flight`). Absent: the stage set as a type (encumbered, committed, in-settlement are not distinguishable) and **every bridge** — venue margin, broker credit line, cross-margin, no-bridge. `grep -rni 'bridge' --include=*.rs backend/crates/services/qip-capital-fabric/ backend/crates/services/qip-capital/` returns nothing. |
| 33.1 | Path Extensions | PARTIAL | The extensions are per-path additions to the gate, and no path assignment exists to extend (see §30.2): `grep -rni 'path_extension\|extension_for\|PathCheck' --include=*.rs backend/crates/` returns nothing, and none of the eight named checks is present — `grep -rni 'honour_rate\|assignment_budget\|capital_occupancy\|adverse_selection_premium' --include=*.rs backend/crates/` returns nothing. The gate that does exist is flat and per-gate-name rather than per-path: `grep -n 'metrics.refusal(\|fn refuse' backend/crates/edge/qip-edge/src/cell.rs`. **Re-scored 2026-09-14 (ADR 0073), `ABSENT` → `PARTIAL`.** `qip-routing`'s `extension` checks all eight paths exhaustively over six fact bundles, and `Cell::work` refuses under `GATE_PATH_EXTENSION` through the existing `Cell::refuse` site — a new label *value*, not a third recording site, so the cardinality bound in `.claude/rules/domains/observability.md` is unchanged (recounted: `grep -n 'metrics\.refusal(' cell.rs` prints two lines, before and after). Not `REACHED` for §31.1's reason: the checks run on a cell, and no deployed cell reaches the paths they gate. |
| 34.1 | What an Adapter Must Provide | PARTIAL | **Seven of nine provisions built, and the count itself is corrected here.** This row read "Five of nine" while listing six, because the reconciliation reader was named after the word "plus" and never counted; the register has been wrong in the optimistic direction before, and this one was wrong in the other, which is worth saying out loud because an understated row invites a lane to rebuild something that exists. The nine are protocol binding, fee schedule, order-type matrix, settlement rules, withdrawal policy, rate limits, latency profile, minimums/tick/lot, and reconciliation endpoint. Built: protocol binding (`grep -n 'pub trait' backend/crates/services/qip-brokers/src/adapter.rs`), a tiered fee schedule with a maker/taker split, the order-type capability matrix, minimums/tick/lot, and a measured latency profile — `sed -n '/^pub struct VenueProfile/,/^}/p' backend/crates/edge/qip-routing/src/venue.rs` — a reconciliation reader for balances (`grep -n 'fn reconcile\|struct HoldingObservation' backend/crates/services/qip-capital-fabric/src/wallet.rs`), and **per-venue order and message rate limits**, which are new. The limits are declared on the venue profile — there is no `Option` and no unlimited arm, so a venue that publishes nothing gets `RateLimits::ASSUMED` rather than no control at all (`sed -n '/pub const ASSUMED/,/};/p' backend/crates/edge/qip-routing/src/ratelimit.rs`) — and they are enforced at two independent seams that count separately on purpose. The router drops a venue whose window is spent **before an order object exists** (`grep -c 'ExclusionReason::RateLimited' backend/crates/edge/qip-routing/src/router.rs` prints 1), and the venue itself refuses the order that exceeds its own allowance while never refusing a cancel (`grep -c 'spend_order\|record_message' backend/crates/services/qip-brokers/src/exchange.rs` prints 3: the submit refusal, and the cancel and replace message counts). Proven to fire rather than declared: `cargo test -p qip-routing -p qip-brokers` runs `a_venue_whose_order_allowance_is_spent_is_dropped_from_the_comparison_with_a_reason`, `the_allowance_is_read_from_the_profile_so_two_venues_do_not_share_one_budget`, `the_venue_refuses_the_order_that_exceeds_its_published_allowance_and_admits_it_a_window_later` and `a_cancel_is_never_refused_by_the_rate_limit_and_is_still_counted_against_it`. The venue seam is on a non-test path: `grep -c 'exchange.submit' backend/crates/apps/qip-edge-node/src/gateway.rs` prints 2, so a binary reaches it — though `grep -rn 'execution_nodes = {}' infrastructure/environments/*/terraform.tfvars` prints four lines, so no deployed process does. The router seam reaches no caller outside `qip-routing`'s own tests, which was already true of `Router::route` before this work: `grep -rn 'qip_routing::router\|qip_routing::Router' --include=*.rs backend/crates | grep -v '^backend/crates/edge/qip-routing/'` prints one line, and it is a string literal in an acceptance test's module list rather than a call. **Still absent, and both blocked on a seam outside `qip-brokers` rather than on effort.** Withdrawal policy: ADR 0021 permits the deterministic half and refuses the signing half, and ADR 0051 already models the venue's own allowlist as configured out of band and mirrored by `CustodyPolicy::mirrors_the_venue_allowlist` in `qip-capital-fabric`; the consumer the blueprint names for this provision is the corridor and custody model, which is §37 and that crate. A second allowlist in `qip-brokers` would be a second source of truth for a fact ADR 0051 already places elsewhere. Settlement rules joined to a per-venue jurisdiction calendar: the calendar exists and is not duplicable — `grep -n 'pub struct SettlementCalendar' backend/crates/services/qip-capital-fabric/src/settlement.rs` — but it lives in a service, and `.claude/rules/architecture/00-boundaries.md` says a service exposes its domain through types rather than reaching into another service. Closing this wants `SettlementCalendar` moved into `qip-financial`, where `MarketHours` already is and which both crates already depend on; that move touches `qip-capital-fabric`. `grep -rni 'withdrawal_policy' --include=*.rs backend/crates/edge/qip-routing/ backend/crates/services/qip-brokers/` still returns nothing. |
| 34.2 | Venue Types | PARTIAL | The taxonomy is built and is a bounded enum reaching the arbitrage graph's tradability check: `grep -n 'pub enum VenueClass' -A 20 backend/crates/libs/qip-contracts/src/venue.rs`, `grep -n 'fn edge_is_tradable\|VenueFacts' backend/crates/edge/qip-arbitrage/src/graph.rs`, and auction sessions are distinguished from continuous trading (`grep -n 'pub enum VenueStatus' -A 14 backend/crates/libs/qip-contracts/src/venue.rs`). Absent: the "extra machinery" column — no auction engine or bidding strategy, no RFQ binding, no counterparty credit view attached to an OTC venue. `grep -rni 'auction_engine\|winners_curse\|rfq\|request_for_quote' --include=*.rs backend/crates/` returns nothing; the credit model that exists is issuer credit, not venue counterparty credit (`grep -n 'pub struct CreditProfile' backend/crates/libs/qip-financial/src/credit.rs`). |
| 34.3 | Decentralised Venues Need Their Own Model | PARTIAL | A DEX is a label with no model behind it: `grep -n 'DecentralisedExchange' backend/crates/libs/qip-contracts/src/venue.rs` is the whole of it, and `grep -rni 'constant_product\|concentrated_liquidity\|pool_math\|mempool\|sandwich\|front_run\|block_time\|contract_risk\|\bMEV\b' --include=*.rs backend/crates/` returns nothing. None of the four required pieces (pool-math slippage, block-time execution mode, an MEV estimate the feasibility gate reads, a contract-risk exposure) exists, and observe-only registration was the fallback it lacked. **Corrected 2026-09-16**: this clause said `grep -rni 'observe_only'` "returns nothing", which stopped being true when ADR 0070 built the DEX model in the amendment below — `grep -n 'fn is_observe_only' backend/crates/libs/qip-financial/src/pool.rs` prints the predicate, and `grep -n 'is_observe_only()' backend/crates/libs/qip-financial/src/pool.rs` prints its caller inside the quoting path beside it — two lines, the declaration and the gate, no line number written down here because this file's own rule is that a line number into a live file is not a fact worth recording. The amendment was appended and the sentence it falsified was left standing, which is the same shape as ADR 0069's own stale row: a record that grows a correction at the bottom while the top still says the old thing. **Re-scored 2026-09-14 (ADR 0070), `ABSENT` → `PARTIAL`.** Pool math, block-time execution, contract risk and an MEV estimate, with **no `f64` anywhere in the file** — no crossing point to state because there is no crossing. The MEV estimate is read by a gate that fires: a minimum notional `gas × 10000 / (budget_bps − slippage_bps − extractable_bps)`, proven end-to-end through `OrderManager::submit` and mutation-verified. It rides the existing `feasibility_minimum_notional` rather than a new gate literal, because `EDGE_GATES` bounds `qip_feasibility_refusals_total{constraint}` and a literal invented in a service crate would reach the centre as `other` — the label meaning a plane used a name this build does not know. Not `REACHED`: no chain client, no signer, no `VenueAdapter` impl, and the contract-risk axis is shaped for an `axes` map no composition root fills. |
| 34.4 | Promotion, Sim to Production | PARTIAL | The six-rung venue ladder does not exist: `grep -rni 'venue.*promot\|VenueStage\|registered.*observed.*simulated' --include=*.rs backend/crates/` returns nothing, and nothing verifies a venue's declared fees, latency or order-type support against measurement before it is enabled. The identically-shaped ladder that *is* built governs **strategies**, not venues — `grep -n 'pub enum GateStage' -A 20 backend/crates/libs/qip-contracts/src/gate.rs` and `grep -n 'pub struct .*Gate\b\|fn gate_for' backend/crates/services/qip-lifecycle/src/gates.rs` — so the pattern exists and has simply never been applied to this subject. **Re-scored 2026-09-14 (ADR 0070), `ABSENT` → `PARTIAL`.** Three rungs on `qip-lifecycle`'s existing `GateStage` — registered → `Candidate`, observed → `Holdout`, simulated → `Paper` — each gate with a passing and a failing case from one fixture. `VENUE_PROMOTION_CEILING` is the simulator; shadow, capped live and full are refused by name with ADR 0003 in the message. Wired into `stage_learn` beside `review_venues` and not inside it, because they answer opposite questions and folding them would give the ladder a path to withdrawal ADR 0062 reserves for one seam. **Not `REACHED`, and the reason is worth reading:** nothing feeds the ladder — `PlatformConfig` carries no venue declarations and the measurement lives in the adapter layer — so the broker is permanently unladdered. Wiring it first raised a *problem* on every cycle of every deployment, which two existing tests caught; an empty ladder is now a summary line and only a ladder with a **gap** raises a problem. A control firing on 100% of cycles is how an operator learns that problems are noise. |
| 35.1 | A Position Has a Lifecycle | PARTIAL | Built exactly to the section, including the transition table that makes `Closed` terminal and forbids assignment: `grep -n 'pub enum PositionLifecycle' -A 20 backend/crates/libs/qip-portfolio/src/lifecycle.rs`, `grep -n 'fn transition' -A 16 backend/crates/libs/qip-portfolio/src/lifecycle.rs`. The field is private as of this row, so the table is the only writer rather than the only *documented* writer: `grep -n 'lifecycle: PositionLifecycle,' backend/crates/libs/qip-portfolio/src/position.rs`. Reached in production for five of the six states. A booked fill drives `Opened → Held → Closed` through `grep -n 'move_lifecycle' backend/crates/libs/qip-portfolio/src/position.rs` from `grep -n 'portfolio.apply_fill' backend/crates/services/qip-brokers/src/ledger.rs`. `Flagged` and `Unwinding` are written by `grep -n 'pub fn flag_position\|pub fn begin_unwind' backend/crates/libs/qip-portfolio/src/portfolio.rs`, called from the backtester's rebalance — `grep -n 'flag_position\|begin_unwind' backend/crates/services/qip-simulation-engine/src/backtest.rs` — on the one production seam that holds a `Portfolio` of positions beside a strategy's live target book, reached from `grep -n 'Backtester::new' backend/crates/apps/qip-deepbrain/src/evolution.rs`. The trigger is the strategy's own instruction: `target_weights` documents an empty map as "hold what you have" and a weight of exactly zero as "go to cash", so a zero weight on a held name is §35.1's withdrawn thesis (`Flagged`) and the order that flattens it is §35.1's deliberate close (`Unwinding`). Each is proven at the wire, not at the function: `backend/crates/services/qip-simulation-engine/tests/position_lifecycle.rs`. **`Orphaned` still has no non-test writer, and deliberately so.** §35.1 defines it as "its strategy retired but the position remains", a fact produced only by `grep -n 'fn disposition_for\|fn scheduled_unwinds' backend/crates/runtime/qip-kernel/src/central/learning.rs`, and the centre's position book is `StrategyLot` — `grep -n 'pub struct StrategyLot' backend/crates/runtime/qip-kernel/src/central/plane.rs` — not `qip_portfolio::Position`, so no production `Position` is reachable from a strategy retirement. Writing it from any other condition would be a trigger nobody can defend, which reads as a working lifecycle and is not one. Closing it means giving the centre a position book or carrying the disposition to a cell, and that is an ADR rather than a wire. |
| 35.2 | The Three Questions Version 9 Left Open | PARTIAL | Question one is answered and derived rather than scheduled: retirement produces a disposition naming every remaining lot with its flatten quantity, and the outstanding set is recomputed from the ledger on each call so no second record can drift — `grep -n 'fn disposition_for\|struct RetirementDisposition\|fn scheduled_unwinds' backend/crates/runtime/qip-kernel/src/central/learning.rs`. Questions two and three are unanswered: no reduced-allocation unwind policy and no thesis-expiry sweep — `grep -rni 'thesis_expiry\|horizon_elapsed\|reduced_allocation\|worst_thesis' --include=*.rs backend/crates/` returns nothing (the `horizon` machinery in `qip-lifecycle` is a capital-bucket register, not a position's intended holding period: `grep -n 'pub struct HorizonBucket' backend/crates/services/qip-lifecycle/src/horizon.rs`). |
| 35.3 | Unwind Ordering | PARTIAL | Rule 3 only, and it is production-reached: cost-to-exit ordering off the liquidity ladder, with a plan that refuses a non-monotonic ladder — `grep -n 'fn plan\|fn reachable_within\|pub enum Rung' backend/crates/libs/qip-financial/src/ladder.rs`, reached from `grep -n 'fn liquidity_ladder\|fn liquidatable_within' backend/crates/runtime/qip-kernel/src/platform.rs`. Rules 1, 2, 4 and 5 are absent: no thesis-failure priority, no tax-lot selection, no hedge-preservation check, no cross-margin protection of collateral behind a retained position — `grep -rni 'tax_lot\|harvest\|do_not_break_hedge\|retained_position' --include=*.rs backend/crates/` returns nothing, and §25.6's model does not exist to respect. |
| 36.1 | Identical Everywhere, Configured Per Region | PARTIAL | The core claim holds and is structural: one binary, with region, venue set, capital and crossing cap all read at the composition root and nowhere else — `grep -n 'QIP_CELL_REGION\|QIP_REGION_ALLOCATION\|QIP_VENUE_FEED' backend/crates/apps/qip-edge-node/src/main.rs`, `grep -n 'fn new\|pub region\|crossing' backend/crates/edge/qip-edge/src/cell.rs` — and a cell assembled with no region is refused rather than defaulted (`grep -n 'was assembled with no region' backend/crates/edge/qip-edge/src/cell.rs`). Absent: core layout/isolation pinning, blue-green with shadow first, and per-region retention classes — `grep -rni 'core_pinning\|isolcpus\|blue_green' --include=*.rs infrastructure/ backend/crates/apps/qip-edge-node/` returns nothing. The three-region table is aspirational: `grep -rn 'execution_nodes' infrastructure/environments/*/terraform.tfvars` shows `{}` everywhere. |
| 36.2 | What Arrives Versus What Only the Region Has | REACHED | The "from central" column is one signed, sequenced, per-cell payload carrying twelve slots — models, compiled plan digest, belief priors, episodic and causal digests, regime, grants, cycle whitelist, risk envelope, inventory targets, feasibility constraints, adversary profiles — each with its own freshness: `grep -n 'pub struct PolicyPayload' -A 30 backend/crates/libs/qip-contracts/src/policy.rs`. Applied in production by the cell against replay (`grep -n 'fn apply_policy\|sequence' backend/crates/edge/qip-edge/src/cell.rs`) and charted as applied rather than as published (`grep -n 'POLICY_SEQUENCE\|fn policy_sequence' backend/crates/edge/qip-edge/src/telemetry.rs`). The "only local" column is the cell's own books, open orders and region allocation: `grep -n 'fn track\|fn open_orders\|fn region_allocation_free' backend/crates/edge/qip-edge/src/cell.rs`. |
| 36.3 | Failure Isolation | PARTIAL | **Five of six rows built, each with a production caller; one row half-built, and the half that is missing is the centre's.** Run the blueprint's own table first — `awk '/^36\.3 /{f=1} f&&/^37\. /{exit} f' docs/architecture/algorik-blueprint-v10.1-source.md` — because this cell has twice been scored against a paraphrase of it. Unchanged and reached: an unreachable venue is excluded from action (`grep -n 'VenueStatus::Unreachable' backend/crates/edge/qip-edge/src/cell.rs`); a halted cell stops without stopping its peers under three independent halt sources (`grep -n "fn halt(\|names::EDGE_HALTED" backend/crates/edge/qip-edge/src/telemetry.rs`); central-unavailable degrades to the last shipment behind a circuit breaker rather than spooling (`grep -n 'CircuitBreaker\|CircuitOpen' backend/crates/edge/qip-edge/src/mesh.rs`); a degraded model pauses only the strategy classes that depend on it (`grep -n 'fn pauses\|fn sizing_multiplier\|enum Capability' backend/crates/libs/qip-contracts/src/degradation.rs`). **Correction, 2026-09-16.** This cell said the stale-reference row was `UNREACHED` because `install_mirror` had test callers only. It has had a production caller since §31.1's node lane — `grep -n 'install_mirror\|venue_regions = ' backend/crates/apps/qip-edge-node/src/lib.rs`, reached from `grep -n 'assemble(' backend/crates/apps/qip-edge-node/src/main.rs` — so a deployed cell can hold a mirror and the reference window can refuse one. **The dark-region arm is now built, and it is not a fifth `RegionState`.** That enum is §31.1's inventory-band table and every arm of it is a position against a band (`grep -n 'pub enum RegionState' -A 18 backend/crates/edge/qip-routing/src/mirror.rs`); a 'this region has gone dark' arm there would have conflated where a cell's inventory sits with whether anybody is on the other end. The reading is its own type, `grep -n 'pub enum RegionOutlook' -A 14 backend/crates/edge/qip-edge/src/region.rs`, with three arms because a wire that cannot be read is not a wire that says nothing: unreadable darkens every region but the cell's own, so a missing mount suspends mirrors instead of reading as 'every peer is answering'. A cycle with a mirrored leg into a dark region is refused whole under its own gate — `grep -n 'fn dark_mirror\|GATE_DARK_REGION' backend/crates/edge/qip-edge/src/cell.rs` — and not under `path_router`, because a whitelist the desk wrote and a node in another region send an operator to different places. Local strategies and intra-venue cycles are untouched, asserted rather than argued: `cargo test -p qip-edge --test cross_region` (17 passed, 0 failed) holds `a_dark_region_leaves_a_cell_whose_venues_are_all_at_home_trading_exactly_as_before`. **The node-crash row's own column is built too**: a restarted cell forms no order until every venue's account of its resting orders and quotes has agreed with the cell's record (`grep -n 'fn require_reconciliation_before_resuming\|fn observe_venue_account' backend/crates/edge/qip-edge/src/cell.rs`), and a disagreement is a break that halts and is never auto-corrected. `cargo test -p qip-edge --test failure_isolation` (7 passed, 0 failed), `cargo test -p qip-edge-node --test dark --test pass` (5 and 16 passed, 0 failed). **Production callers, which is the bar this register scores on**: the node polls the region wire beside its halt flag and arms the restart discipline from its own journal store — `grep -n 'region_wire\|wire.poll\|require_reconciliation_before_resuming' backend/crates/apps/qip-edge-node/src/main.rs`, `grep -n 'venue_account(\|observe_venue_account' backend/crates/apps/qip-edge-node/src/pass.rs`. **Three limits, stated rather than left to be found.** (1) `Cell::work` reaches no deployed process: the node runs a pass only under `QIP_VENUE_FEED=simulated` (`grep -n 'fn run_pass' backend/crates/apps/qip-edge-node/src/pass.rs`) and `execution_nodes = {}` everywhere (`grep -rn execution_nodes infrastructure/environments/*/terraform.tfvars`); the arming and the region wire are outside the pass and are reached by any start, but no node is deployed at all. (2) The node's simulated venue lives in the process and dies with it, so after a real restart both sides of the account comparison are empty — the arm that finds an order the dead session left is exercised in test, and a gateway to a venue that does outlive the cell has no account channel, so such a node stays paused (fail closed, asserted by `a_restarted_node_whose_second_venue_cannot_answer_keeps_refusing_rather_than_resuming`). (3) The region wire rides the halt flag's mount and no Terraform names it, so a deployment without `QIP_HALT_FLAG_PATH` can never be told a peer is dark; the node prints which of the two it is at start-up. **What is left is one column of one row and it is not in this lane's territory**: §36.3's cloud-region row also asks that global exposure be recomputed without the dark region, which is the central plane's (`backend/crates/runtime/qip-kernel/src/platform.rs`) and is untouched here. Five rows reached, one row reached on the cells' side and unbuilt on the centre's. |
| 37.1 | Corridor Lifecycle | REACHED | All seven stages are built with the actor and control of each, and the transition table refuses every edge not on it — so a revoked corridor cannot be walked back to active by a late event: `grep -n 'pub enum CorridorStage' -A 24 backend/crates/services/qip-capital-fabric/src/corridor.rs`, `grep -n 'fn transition' -A 20 backend/crates/services/qip-capital-fabric/src/corridor.rs`. The time delay is a constant applied at signing, not a comment: `grep -n 'ACTIVATION_DELAY' backend/crates/services/qip-capital-fabric/src/destination.rs backend/crates/services/qip-capital-fabric/src/corridor.rs`. Until 2026-09-08 every caller was a test: `grep -rn 'declare_corridors\|Corridor::propose' --include=*.rs backend/crates/runtime/ backend/crates/apps/` finds only the pass-through pair `Platform::declare_corridors` → `StrategyFactory::declare_corridors`, and `grep -rn 'FabricCommand::Gate' --include=*.rs backend/crates/runtime/ backend/crates/apps/ \| grep -v '/tests/'` returns only the match arm in `Platform::decide_fabric`, never a producer — so no corridor is ever proposed outside tests. | **A production caller now exists.** `qip-api`'s composition root reads the declaration `QIP_CAPITAL_FABRIC_PATH` names and applies every command through `Platform::decide_fabric` before serving, and applies what has been appended on an admitted cycle: `grep -n 'load_fabric_declaration\|FabricRefresh' backend/crates/apps/qip-api/src/main.rs`, with the command vocabulary held in `backend/crates/runtime/qip-kernel/src/fabric_declaration.rs` because `api_boundary.rs` forbids the application layer an edge to `qip-capital-fabric` (`grep -n 'FORBIDDEN_CRATES' -A 12 backend/crates/tests/qip-acceptance/tests/api_boundary.rs`). Scored `REACHED` on the same bar as `Cell::work`: no environment mounts a declaration (`grep -rn capital_fabric_file infrastructure/environments/*/terraform.tfvars` returns nothing, and each tfvars says why), so the path is in the binary and nobody has selected it.
| 37.2 | Delay Applies to Destinations, Not Caps | REACHED | Built and stricter than the blueprint on purpose — a *loosened* cap re-enters the 24h delay rather than taking the blueprint's "two approvals, no delay": `grep -n 'is_looser_than\|ACTIVATION_DELAY\|delay applies to destinations' backend/crates/services/qip-capital-fabric/src/corridor.rs`. Until 2026-09-08 every caller was a test: `grep -rn 'FabricCommand::Corridor' backend/crates --include=*.rs \| grep -v '/tests/'` returns only the dispatch arm in `journal.rs`, and `grep -rn 'declare_corridors' backend/crates/apps` returns nothing. | **A production caller now exists.** `qip-api`'s composition root reads the declaration `QIP_CAPITAL_FABRIC_PATH` names and applies every command through `Platform::decide_fabric` before serving, and applies what has been appended on an admitted cycle: `grep -n 'load_fabric_declaration\|FabricRefresh' backend/crates/apps/qip-api/src/main.rs`, with the command vocabulary held in `backend/crates/runtime/qip-kernel/src/fabric_declaration.rs` because `api_boundary.rs` forbids the application layer an edge to `qip-capital-fabric` (`grep -n 'FORBIDDEN_CRATES' -A 12 backend/crates/tests/qip-acceptance/tests/api_boundary.rs`). Scored `REACHED` on the same bar as `Cell::work`: no environment mounts a declaration (`grep -rn capital_fabric_file infrastructure/environments/*/terraform.tfvars` returns nothing, and each tfvars says why), so the path is in the binary and nobody has selected it.
| 37.3 | Transfer Gate | REACHED | **This row was stale, not the code.** It read `PARTIAL`, citing `grep -n 'No deployed process issues a .Gate. command' backend/crates/runtime/qip-kernel/src/platform.rs` as evidence nothing issues one — that grep now returns nothing, because the sentence it quoted was rewritten in place when the producer landed (`5adddc3`, 2026-09-08, the same commit §37.1/§37.2/§38.4 below already credit) and the scoring pass that closed those three rows did not revisit this one. Same evidence, same producer: all seven checks exist in assessment order and their roster is rendered by a live route (`grep -n 'GateCheck::ALL\|GateCheck::CorridorAuthority\|GateCheck::Caps\|GateCheck::MinimumInterval\|GateCheck::StatedPurpose\|GateCheck::SourceBalance\|GateCheck::VelocityAndAnomaly' backend/crates/services/qip-capital-fabric/src/gate.rs`, `grep -n '"/transfer-gate"' backend/crates/apps/qip-api/src/routes.rs`), and `qip-api`'s composition root now applies every declared command — `Destination`, `Corridor`, `Wallet` and `Gate` alike, the parser names all four by subject and dispatches generically — through `Platform::decide_fabric` before serving and on an admitted cycle: `grep -n 'load_fabric_declaration\|FabricRefresh' backend/crates/apps/qip-api/src/main.rs`, vocabulary in `backend/crates/runtime/qip-kernel/src/fabric_declaration.rs` for the reason §37.1's row gives. Scored `REACHED` on the same bar as `Cell::work` and as §37.1/§37.2/§38.4: no environment mounts a declaration (`grep -rn capital_fabric_file infrastructure/environments/*/terraform.tfvars` returns nothing, and each tfvars says why), so a `Gate` command specifically has not been exercised through this path outside `qip-capital-fabric`'s own journal tests (`assessments().len(), 3` in that crate's `tests/journal.rs`) — the path is real and generic, not yet a deployment's choice. |
| 37.4 | Custody | PARTIAL | Built half — the class/custodian/corridor-kind table plus the three-enforcement-point rule and trading/transfer identity disjointness, enforced inside the gate rather than in a constructor: `grep -n 'fn all_agree\|disjoint_from_trading_authority\|mirrors_the_venue_allowlist\|requires_multi_party_release' backend/crates/services/qip-capital-fabric/src/custody.rs`. Refused half — the MPC threshold-signing policy *engine* holding a key share, by ADR 0021: `grep -n 'ADR 0021 permits that table and refuses' backend/crates/services/qip-capital-fabric/src/custody.rs`. Reached only through `gate::assess`, which no production caller invokes (see §37.3). |
| 38.1 | Read and Write Are Separate Systems | PARTIAL | Read path REACHED — `qip-api`'s statement feed → `observe_statement` → LEARN: `grep -n 'StatementRefresh::new' backend/crates/apps/qip-api/src/main.rs`, `grep -n 'observe_statement' backend/crates/apps/qip-api/src/statement.rs`, `grep -n 'self.reconcile_wallet(now)' backend/crates/runtime/qip-kernel/src/platform.rs`. Write path deliberately does not exist (ADR 0021), so §38.1's separation is structural rather than a dependency audit — no type can hold a credential: `grep -n 'the write path.*does not exist\|enum Provenance' backend/crates/services/qip-capital-fabric/src/wallet.rs`. |
| 38.2 | Adapters | PARTIAL | The wallet's read path has exactly one of the eight channels, and it is the weakest: a statement a person hands in. `grep -n 'ReadOnlyApiKey\|WatchOnlyAddress\|ViewKey\|Statement,' backend/crates/services/qip-capital-fabric/src/wallet.rs` shows four provenance *labels*, but `grep -n 'Provenance::' backend/crates/runtime/qip-kernel/src/platform.rs` shows the kernel can honestly record only `Statement`, and the sole producer is the file feed (`grep -n 'StatementFeed::from_env' backend/crates/apps/qip-api/src/main.rs`). Adapters do exist elsewhere in the tree and none of them feeds the wallet: `grep -n 'pub trait ChainAdapter\|pub struct NodeChainAdapter' backend/crates/services/qip-chain/src/adapter.rs` and `grep -n 'pub trait VenueAdapter\|fn query_cash' backend/crates/services/qip-brokers/src/adapter.rs`. Every "can initiate" column is refused by ADR 0021. |
| 38.3 | Reconciliation | PARTIAL | The reached half stands — the exact arithmetic, the per-venue-asset halt, surplus treated as shortfall, and no path that writes a correction, driven from LEARN each cycle: `grep -n 'fn reconcile\|ReconciliationOutcome::Halt\|BreakCause::DeltaBeyondTolerance' backend/crates/services/qip-capital-fabric/src/wallet.rs` and `grep -n 'WalletCommand::Reconcile' backend/crates/runtime/qip-kernel/src/platform.rs`. **The tolerance is now §38.3's formula rather than the constant the platform shipped**: `tolerance = dust + rate × |expected|`, per **venue-asset**, with the section's class beside it — `grep -n 'pub enum ToleranceClass\|pub struct ToleranceBasis\|pub fn dust_only\|pub fn evaluate\|pub struct ToleranceSchedule' backend/crates/services/qip-capital-fabric/src/tolerance.rs`. Three silent failures close with it. A constant cannot be right for two books — a dollar of drift on a hundred dollars is a break, a dollar on a hundred million is a rounding artefact — so whichever book the number was chosen for, the other was mis-judged, and the too-loose direction is the `MaxExpectedShortfall` defect with the sign reversed (`the_same_basis_allows_more_on_a_larger_book_and_less_on_a_smaller_one`, `a_tolerance_that_the_formula_widened_still_halts_beyond_it_and_not_inside`). It was keyed by asset, so a statement for a second venue holding USD silently replaced the first venue's control, and which venue ended up judged by a tolerance nobody there set depended on the order statements arrived in (`two_venues_holding_one_asset_are_judged_by_their_own_floors`, `two_venues_holding_dollars_keep_their_own_floors_through_the_learn_stage`). And it carried no class, so a halt could not say which of the section's six rules it had applied; every outcome now carries the evaluated formula with all six terms, so the figure behind a halt is re-derivable from the record rather than trusted. The tolerance decides whether to **halt** on a disagreement and no longer whether the disagreement happened: a non-zero delta inside tolerance is `WithinTolerance` carrying its direction, `Reconciled` now means the two claims agreed exactly, and §38.3's "a persistent non-zero delta inside tolerance is a modelling defect and opens a ticket" is a per-venue-asset streak that extends only while the divergence keeps leaning the same way, restarts on a flip and clears on an exact agreement or a halt — `grep -n 'fn drift_tickets\|DriftStreak' backend/crates/services/qip-capital-fabric/src/journal.rs`, with `a_delta_that_keeps_leaning_the_same_way_inside_tolerance_opens_a_ticket`, `a_divergence_that_changes_direction_restarts_the_streak_rather_than_extending_it` and `an_exact_agreement_or_a_halt_clears_the_drift_a_venue_asset_had_accumulated`. Two silences close beside it: reconciling a wallet holding no venue-asset answered `Ok(vec![])`, byte-identical to the answer for a book that reconciled perfectly and the answer a deployment gives for as long as no statement arrives, and it now refuses and says so (`an_empty_wallet_refuses_to_reconcile_rather_than_reporting_a_clean_book`); an empty schedule reports `Idle` with its reason rather than being an absence a caller must know to ask about (`an_empty_schedule_says_it_is_idle_rather_than_reading_as_permissive`). **Short of `REACHED`, and the gap is now narrower and differently shaped than this row said.** It read "no funding rate, deposit rate or mark interval exists anywhere in this platform"; a **deposit rate now does**. The euro area's deposit facility rate is fetched from the ECB's own data portal by the `ecb-key-interest-rates` connector, which clears the same licensing gate the Frankfurter source does before any transport is built — licence `ecb-website-copyright-free-use`, class `Public`, keyless — `grep -n 'ecb-key-interest-rates' backend/crates/services/qip-data-finder/src/admission.rs backend/crates/services/qip-data-finder/src/registration.rs backend/crates/services/qip-market-ingestion/src/connector_feed.rs` and `the_ecb_key_interest_rates_are_admitted_for_trade_under_the_ecb_s_own_terms`. It arrives as a macro observation stamped with the date it applied to and withheld until sixteen hours later (`the_recorded_message_decodes_into_one_level_per_key_rate_with_the_ecb_s_own_figures`, `a_key_rate_is_withheld_until_the_ecb_would_have_published_the_day_it_applies_to`, `the_same_message_served_twice_releases_its_levels_once` in `backend/crates/services/qip-market-ingestion/tests/ecb_key_rates.rs`), and `Platform::observe` records it as one day's accrual on the ECB's own actual/360 convention — `grep -n 'absorb_interval_rate\|interval_rate_series\|policy_rates' backend/crates/runtime/qip-kernel/src/platform.rs` is the production call path, `the_ecb_deposit_facility_rate_absorbed_at_sense_becomes_a_38_3_interval_rate` drives it. **A funding rate and a mark interval still do not exist**, and `SourcedIntervalRate::from_percent_per_annum` refuses to be the one that supplies them: an annual percentage is one day without a second assumption and is a funding interval only with one (`a_per_annum_figure_is_refused_for_every_class_the_section_does_not_measure_in_days`). **The desk's dollar cash book now carries a real sourced rate, and this row said it could not.** It read "no production basis carries a non-zero rate yet" on the reasoning that an issuer sets a rate for a currency, that the one class this process can attest is the desk's own cash at its broker, and that the book is in dollars while the only admitted rate was the euro's. Every clause of that was true and the conclusion has been closed rather than argued away: the **effective federal funds rate** is fetched from the Federal Reserve Bank of New York's own markets API by the `nyfed-effr` connector, through the *unmodified* admission gate — licence `nyfed-terms-of-use-reference-rates`, class `Public`, keyless, `Redistribute` included — `grep -n 'nyfed-effr' backend/crates/services/qip-data-finder/src/admission.rs backend/crates/services/qip-data-finder/src/registration.rs backend/crates/services/qip-market-ingestion/src/connector_feed.rs` and `the_new_york_feds_effective_federal_funds_rate_is_admitted_for_trade_under_its_terms_of_use`. It arrives as a macro observation stamped with the date its transactions took place and withheld five days — a single scalar covering the worst case the recording itself shows, a rate for the Friday before a Monday holiday not published until the Tuesday (`the_recorded_response_decodes_into_one_rate_per_business_day_with_the_new_york_feds_own_figures`, `an_effective_rate_is_withheld_until_the_new_york_fed_would_have_published_the_day_it_covers`, `the_rates_are_published_oldest_first_however_the_vendor_ordered_its_array`, `the_same_response_served_twice_releases_its_rates_once` in `backend/crates/services/qip-market-ingestion/tests/nyfed_effr.rs`). `Platform::observe` records it as one day's accrual on the US money market's own actual/360 convention — checked against that market rather than copied from the euro arm, and the two coincide because two money markets settled on the same convention. The production call path is `grep -n 'POLICY_RATE.US.EFFR\|absorb_interval_rate\|interval_rate_series' backend/crates/runtime/qip-kernel/src/platform.rs`; `the_effective_federal_funds_rate_absorbed_at_sense_becomes_a_38_3_interval_rate_for_dollars` and `the_learn_stage_judges_the_desks_dollar_cash_by_a_published_rate_rather_than_its_dust_floor` drive it end to end, the second asserting the LEARN stage's basis carries `0.000100833` rather than zero. **The separation the euro lane refused to breach still holds, by currency and not by which feed shipped**: EFFR answers for a dollar book and `NoneHeld` for every other, the deposit facility rate answers for a euro book and `NoneHeld` for every other, and a sterling book is still at its dust floor (`a_dollar_rate_and_a_euro_rate_are_held_apart_and_neither_answers_for_the_other_s_book`, `a_euro_rate_on_its_own_leaves_the_desks_dollar_cash_at_its_dust_floor_and_the_record_says_why`, `a_euro_rate_never_answers_for_a_book_in_another_currency`). **The New York Fed's terms attach an obligation this platform can carry and cannot discharge.** Their Use Restrictions require a named notice and disclaimer with any *presentation* of reference rate data; the completed text is `NyFedEffrConnector::REFERENCE_RATE_NOTICE`, the kernel's series table forces every arm to name a notice or to name that there is none, and `SourcedIntervalRate::presentation_notice` carries it into the derivation sentence `Platform::tolerance_reasons` keeps beside the basis (`a_tolerance_derived_from_the_effective_rate_carries_the_notice_the_new_york_fed_demands`, `the_reference_rate_notice_the_terms_require_is_carried_verbatim_by_the_connector`). Carrying it is a mechanism; **rendering it is the obligation, and nothing renders this rate today — whoever first does owns it**, as the catalogue entry says in the same words the ECB entries use about acknowledgement. The attribution format, the same-permissions condition on redistribution, and the Terms' own instruction that users monitor the site for changes are likewise recorded and unenforceable by any check in the tree. What the record gains instead is the **cause**: `rate: 0` reads identically whether nobody publishes for the currency, the rate held is for another row, the publisher has gone quiet or the class accrues nothing, and `RateLookup` makes those four separate findings that `Platform::tolerance_reasons` keeps beside each basis (`grep -n 'pub enum RateLookup\|pub struct SourcedIntervalRate\|pub struct PolicyRateTable\|pub fn from_sourced' backend/crates/services/qip-capital-fabric/src/tolerance.rs`, with `a_published_deposit_rate_becomes_one_days_accrual_and_widens_only_by_that`, `a_rate_knowable_before_it_was_true_is_refused_as_point_in_time_leakage`, `a_rate_whose_publisher_has_gone_quiet_stops_governing_and_the_dust_floor_returns`, `a_superseded_rate_offered_after_a_later_one_is_refused_rather_than_taken` and `a_sourced_rate_faces_the_same_ceiling_a_declared_one_does` in `backend/crates/services/qip-capital-fabric/tests/sourced_tolerance.rs`). **Nothing is deployed and nothing is scraped**: neither `data-api.ecb.europa.eu` nor `markets.newyorkfed.org` is in the Envoy bootstrap or in `egress_allowed_upstreams`, so a deployment cannot reach either source until that separate infrastructure step is taken under ADR 0034 — the same position the ADR 0034 candidates are in, and stated here rather than inferred. Both connectors are proven against recorded bodies and against nothing a deployed process has fetched; a connector is not a feed. `grep -n 'class_for_desk_cash\|ToleranceBasis::dust_only\|ToleranceBasis::from_sourced' backend/crates/runtime/qip-kernel/src/platform.rs` is the seam, and it shows the one class this process can attest: the desk's own cash at its broker is fiat at a broker and everything a statement otherwise names is `Undeclared`, because reading a class out of an asset string — "BTC looks like crypto spot" — would put a §38.3 row nobody chose behind a halt (`the_learn_stage_judges_the_desks_cash_by_the_38_3_row_it_can_attest`, `only_the_desks_own_cash_is_classified_without_being_told`). Five of the six classes therefore have a rule and no data, and the sixth now has data for the currency this deployment's book is actually in as well as for one it is not. That the rate arm is a working gate rather than one that refuses everything is `an_interval_rate_that_could_never_halt_is_refused_and_a_usable_one_is_admitted`; `a_class_that_38_3_gives_no_accrual_to_cannot_be_given_an_interval_rate` refuses an operator's number the formula would have ignored. `FabricRecord::SCHEMA_VERSION` goes 3 to 4 and a version-3 reconciliation is refused by version rather than re-judged, because the hash chain proves a record has not changed, not that the rule that produced it still holds. |
| 38.4 | Destination Registry | REACHED | All five stages built, including the 24-hour delay as a constant rather than a parameter and the venue-allowlist mirror as the third enforcement point: `grep -n 'ACTIVATION_DELAY\|fn propose\|fn verify\|fn record_signature\|fn revoke\|fn usable\|enum DestinationStatus' backend/crates/services/qip-capital-fabric/src/destination.rs` plus `grep -n 'fn mirrors_the_venue_allowlist' backend/crates/services/qip-capital-fabric/src/custody.rs`. Until 2026-09-08 every caller was a test: `grep -rn 'FabricCommand::Destination' backend/crates --include=*.rs \| grep -v '/tests/'` returns only the `journal.rs` dispatch arm. | **A production caller now exists.** `qip-api`'s composition root reads the declaration `QIP_CAPITAL_FABRIC_PATH` names and applies every command through `Platform::decide_fabric` before serving, and applies what has been appended on an admitted cycle: `grep -n 'load_fabric_declaration\|FabricRefresh' backend/crates/apps/qip-api/src/main.rs`, with the command vocabulary held in `backend/crates/runtime/qip-kernel/src/fabric_declaration.rs` because `api_boundary.rs` forbids the application layer an edge to `qip-capital-fabric` (`grep -n 'FORBIDDEN_CRATES' -A 12 backend/crates/tests/qip-acceptance/tests/api_boundary.rs`). Scored `REACHED` on the same bar as `Cell::work`: no environment mounts a declaration (`grep -rn capital_fabric_file infrastructure/environments/*/terraform.tfvars` returns nothing, and each tfvars says why), so the path is in the binary and nobody has selected it.
| 39.1 | Where AI, Quantum and Language Models Sit | PARTIAL | Held rows — "risk and transfer enforcement: deterministic Rust, no model" is structural (`grep -n 'Determinism::Required' backend/crates/services/qip-cost-router/src/router.rs`), and "quantum plus classical" carries a non-optional baseline (`grep -n 'pub classical_baseline\|needs_a_classical_baseline' backend/crates/libs/qip-quantum/src/benchmark.rs`). Absent rows — "ONNX models, shipped, in-process" cannot be built under the two-dependency rule: `grep -rni 'onnx' backend/crates --include=*.rs \| grep -v '/tests/'` returns one prose mention in `qip-contracts/src/policy.rs` and no implementation. Language-model rows exist only as the agent panel's budgeted capability (`grep -n 'pub enum Capability' backend/crates/libs/qip-agents/src/capability.rs`), not as a filings/news hypothesis reader. |
| 40.1 | What a User Controls | PARTIAL | Six of the eight surfaces have a page: `ls frontend/portal/src/app/\(portal\)` shows `portfolio`, `treasury`, `strategies`, `risk`, `cognition`, `capital`. The "Acts on" column is where it stops — no surface raises a transfer intent, funds a family, sets a maximum, or adjusts the exploration share: `grep -rn 'transferIntent\|fundFamily\|explorationShare' frontend/portal/src` returns nothing. |
| 40.2 | Explanation | PARTIAL | Two of the seven questions are answerable end to end — "what do you not know here" and "which episodes resemble now": `grep -n '"/cognition/self-model"\|"/cognition/precedents"' backend/crates/apps/qip-api/src/routes.rs` and `ls frontend/portal/src/app/\(portal\)/cognition`. The other five have no surface: `grep -rn 'explanation' backend/crates/apps/qip-api/src/routes.rs` returns no route, and `find frontend/portal/src/app -ipath '*explan*'` returns nothing. **Re-checked 2026-09-19, and one of the five moved partway rather than closing.** `/risk/recalibrations` now has a console surface (`ls 'frontend/portal/src/app/(portal)/risk/recalibrations'`), and what it renders is exactly the material the fourth question asks for — the gate that declined, and the counterfactual score of declining, carried as `RegretEvidence` with the sample it was judged against, the count the counterfactual said were wrong, and `would_have_earned` as a `Simulated<Decimal>` so a reporting surface cannot print it as money that moved (`grep -n 'pub struct RegretEvidence' -A 10 backend/crates/runtime/qip-kernel/src/rule_review.rs`). **It is deliberately not counted as a third answered question.** §40.2 asks why *this* obvious trade was not taken, and the route aggregates regret per rule over a window rather than per declined order; a desk reading it learns which limit has been costing the platform, not why one trade was declined. Counting it would be the inflation this register exists to end. The remaining surfaces are still absent: `grep -rn 'explanation' backend/crates/apps/qip-api/src/routes.rs` returns no route, `find frontend/portal/src/app -ipath '*explan*'` returns nothing, and no route answers position rationale, sizing decomposition, strategy selection or cost against attributed return (`grep -n 'pattern: "/' backend/crates/apps/qip-api/src/routes.rs | grep -iE 'counterfact|declin|explan|belief|attribut|sizing'` returns nothing). Attribution (the weaker half the section distinguishes itself from) does run in LEARN: `grep -n 'fn attribute' backend/crates/runtime/qip-kernel/src/platform.rs`. |
| 40.3 | Identity and Access | PARTIAL | Roles, devices and step-up are modelled in the shared auth package: `grep -n 'AuthMethod\|MfaMethod\|reauthenticate' frontend/packages/auth/src/index.ts`. But §40.3's load-bearing claim — passkeys, "no passwords anywhere" — is contradicted by the same line: `passkey` is one arm of a union whose first arm is `password`, and password routes are shipped: `ls frontend/portal/src/app/api/auth` shows `forgot-password` and `reset-password`. No WebAuthn ceremony or attestation code exists: `grep -rn 'navigator.credentials\|attestationObject' frontend/portal/src` returns nothing. **Owner-blocked, re-checked 2026-09-19, and deliberately not built toward.** The decision that would settle it is `ls docs/adr/0038-*`, whose status line still opens *proposed* (`grep -n -iE '^\*?\*?status' docs/adr/0038-*.md`) and which is proposed on purpose rather than by neglect: it names checks only the owner can run against the identity provider. Shipping a WebAuthn ceremony before that record is accepted would settle by code a question the owner reserved, and removing the password routes it supersedes would take the only credential the portal currently has. So this row is short of §40.3 for a reason no lane may close. |
| 40.4 | Web and Mobile | PARTIAL | The strongest row holds structurally — "place a manual trade: no path exists" is enforced by the API surface, `grep -n 'pattern: "/orders"' -A4 backend/crates/apps/qip-api/src/routes.rs` (read-only) and the whole `paper_boundary` suite, `ls backend/crates/tests/qip-acceptance/tests/paper_boundary.rs`. Every other row's authentication column is unbuilt: `grep -rn 'biometric\|enclave' frontend/portal/src` returns nothing, and there is no withdraw, second-approval, or hardware-key path on either surface. Re-verified 2026-09-19; the authentication column depends on the same unaccepted record as §40.3 (`ls docs/adr/0038-*`, status *proposed*), so it is owner-blocked rather than pending. **The paper half of this row strengthened rather than the authentication half**: the two self-correction surfaces added since — `ls 'frontend/portal/src/app/(portal)/execution/withdrawals'` and `ls 'frontend/portal/src/app/(portal)/risk/recalibrations'` — render a venue the platform withdrew from itself and a bound it proposed about its own rules, and neither offers a control that acts. Each renders the refusal the platform returned for this caller at the signature route instead, so an operator learns the call cannot succeed from the console before a recovery rather than during one. |
| 40.5 | Experience Architecture | PARTIAL | Three surfaces over one plane exist and the "no path from a surface to an order, a venue, a QPU or a key" rule is tested: `ls frontend/landing frontend/portal frontend/mobile` and `ls backend/crates/tests/qip-acceptance/tests/api_boundary.rs`. The public edge named in the layer table does not: `grep -rn 'google_compute_security_policy\|google_compute_backend_bucket' infrastructure/terraform --include=*.tf` returns nothing, so Cloud Armor, the Global HTTPS LB and Cloud CDN are all absent. **Amended 2026-09-14 (ADR 0069).** `grep google_compute_security_policy infrastructure/terraform` no longer returns nothing: `modules/public-edge/` declares Cloud Armor, a global HTTPS load balancer and Cloud CDN, wired at the root and **gated off in every environment**, with a harness planning it: `cd infrastructure/terraform/modules/public-edge && terraform init -backend=false && terraform test` prints `12 passed, 0 failed`. **Corrected 2026-09-19 — the path is load-bearing and this row gave it ambiguously.** The harness is the module's own (`find infrastructure -name '*.tftest.hcl'` places it under `modules/public-edge/tests/`), and the same name run from the Terraform root, `terraform test -filter=tests/public-edge.tftest.hcl`, prints `Success! 0 passed, 0 failed` — a green line that selected no file and measured nothing. A citation that reads as a pass while asserting nothing is the one result shape this register must never carry, and it is the `MaxExpectedShortfall` failure wearing a test runner's clothes and refusing `execution`, `ledger` and `treasury-write` by name so no customer surface can reach a trading zone. Deployment-blocked remainder, stated rather than left to be found: Private Service Connect is still behind `enable_private_service_connect = false`, no customer surface appears in any catalogue or manifest, and no certificate can issue without DNS. |
| 40.6 | Public Website | REACHED | The landing application carries the six explanatory areas (`ls frontend/landing/app`), and §40.6's actual requirement — every quantitative statement carrying an architecture/target/demo/measured status — is now structural rather than authorial. **The mechanism.** `frontend/landing/components/elements/Claim.js` throws on a status outside the four and on a numeral kind outside its own, so a figure with an unchecked status cannot render; `frontend/landing/lib/claims.mjs` is the single definition of what counts as a quantity, and it reads spelled-out number words as well as digits, because most of this site's numbers spell out and a `\d`-only rule would have reported the site clean. **The test.** `frontend/landing/tests/claims.spec.mjs` walks the rendered DOM of every page — including the 404 body — and fails on any numeral with no annotated ancestor, quoting the sentence; beside it, an annotation must cover a quantity and must be statement-sized (so a `data-claim-status` on a wrapper cannot silence the sweep), and nothing may be labelled `measured` while `execution_nodes = {}` everywhere. `npx playwright test` in `frontend/landing` reports `72 passed`, `npm run lint` reports `landing lint: clean — 43 files, 12 routes checked`. **What it found on its first run**, which is the argument for it: the home page advertised `59` Rust crates against a workspace of 58 (`grep -c '"crates/' backend/Cargo.toml`); "Seven regional cells" was stated flat as an inventory when ADR 0035 calls seven "the blueprint's target and premature" and none is deployed, and it is now labelled `target` with a visible marker; and the three headline figures rendered as empty boxes until a scroll event that never fires on most pages, so the annotation over them was vacuous — the anti-blanket assertion caught that, not a person. **Two honest limits, stated rather than left to be found.** The detector does not read "one", "no" or "both" as quantities, because in English they are determiners far more often than counts and a rule whose output is mostly noise gets suppressed; a claim of exactly one can therefore reach a reader unannotated. And a quantity inside an `aria-label` or a `<meta name="description">` has no element to carry an attribute, so its status is declared in `DECLARED_ATTRIBUTE_CLAIMS` and `scripts/lint.mjs` holds the declaration to the source in both directions — undeclared strings and stale declarations both fail. |
| 40.7 | Investor Portal | PARTIAL | An authenticated portal with a role-gated admin area exists and its screens are reads of kernel views: `ls frontend/portal/src/app/\(portal\)` and `grep -n 'pattern: "/ledger/users' backend/crates/apps/qip-api/src/routes.rs`. The eight-area information architecture §40.7 specifies is not the one built — `find 'frontend/portal/src/app/(portal)' -maxdepth 1 -mindepth 1 -type d | wc -l` printed 24 on 2026-09-19 — `command`, `treasury`, `cognition`, `execution`, `operations`, `topology` and the rest — rather than Overview/Invest/Capital/Intelligence/Risk/Activity/Platform/Account. (The figure was written here as “some twenty-five”; run the command rather than quoting either number, and note it counts directories, so the three loose files beside them are not areas), and the "Raises" column is empty everywhere except mandate enrolment. |
| 40.8 | Mobile Experience | PARTIAL | Two of the section's three claims hold. The PWA is the whole mobile channel by decision, not omission: `grep -n 'manifest.ts\|sw.js\|InstallApp' frontend/mobile/README.md`, `ls frontend/portal/public/sw.js frontend/portal/src/app/manifest.ts`, and mobile-width Playwright projects at `grep -n 'devices\[' frontend/portal/playwright.config.ts`. The halt control *is* on every screen: `grep -n 'KillSwitch' frontend/portal/src/components/chrome/AppShell.tsx`. What is absent is the five-area information architecture — `find 'frontend/portal/src/app/(portal)' -maxdepth 1 -mindepth 1 -type d | wc -l` printed 24 on 2026-09-19, not Home/Invest/Portfolio/Wallet/Activity — and the same missing "Raises" column as §40.7. |
| 40.9 | Application and API Layer | PARTIAL | The behaviour §40.9 asks for — interface services holding no financial state, authorising server-side, one API per screen composition — is met by one binary: `grep -c 'pattern: "' backend/crates/apps/qip-api/src/routes.rs` counts the surface and `grep -n 'fn viewer_entitlements' backend/crates/runtime/qip-kernel/src/platform.rs` is the server-side authorisation. The ten named services do not exist as services: `ls backend/crates/apps` returns six binaries, none of them `account-api`, `portfolio-api`, `investment-api`, `wallet-api`, `treasury-api`, `strategy-api`, `research-api`, `admin-api` or `entitlement-service`. |
| 40.10 | Wallet Experience | PARTIAL | REACHED as a read: `grep -n 'pattern: "/wallet"' backend/crates/apps/qip-api/src/routes.rs`, `grep -n 'pub fn wallet' backend/crates/apps/qip-api/src/ledger_views.rs`, `ls frontend/portal/src/app/\(portal\)/treasury/wallet/page.tsx`, and the "break shown as a halt, never as a corrected number" rule is a property of the type (§38.3). Short of the section: three balance states are modelled, not five — `grep -n 'fn new' -A8 backend/crates/services/qip-capital-fabric/src/wallet.rs` shows `LedgerView` carrying balance, reserved and in-flight only; invested and committed have no field, and "raise a transfer intent" has no control. |
| 40.11 | Investment Experience | PARTIAL | The backend primitive and its gates exist and are reached: `grep -n 'pattern: "/ledger/users/:user/investment-requests"' backend/crates/apps/qip-api/src/routes.rs`, `grep -n 'fn evaluate' backend/crates/services/qip-capital/src/ledger/entitlement.rs`, `grep -n 'can_invest()' backend/crates/services/qip-capital/src/ledger/request.rs`. The six-step user experience does not: `grep -rn 'InvestmentRequest' frontend/portal/src` returns nothing, so no screen selects a family, allocates weights, shows envelope utilisation before and after, or confirms. |
| 40.12 | Capital Movement | PARTIAL | The refusal half is structural and rendered, which is the safe half: `grep -n 'fn can_withdraw' backend/crates/services/qip-capital/src/ledger/entitlement.rs` returns a `WithdrawalEntitlement` whose type has one variant, surfaced at `grep -n 'WithdrawalChip' frontend/portal/src/app/\(portal\)/treasury/_shared.tsx`. None of the four flows in the table exists as a flow: `ExpectedInflow` is built but has no reader — `grep -rn 'ExpectedInflow' backend/crates --include=*.rs \| grep -v 'ledger/cash.rs' \| grep -v '/tests/'` returns nothing — and the corridor gate the two middle flows would pass has no production producer (§37.3). |
| 40.13 | Entitlement Model | PARTIAL | Server-side evaluation on every request, with the interface stating the reason rather than hiding silently: `grep -n 'fn evaluate\|fn investment_capability' backend/crates/services/qip-capital/src/ledger/entitlement.rs`, `grep -n 'pattern: "/ledger/users/:user/eligibility"' backend/crates/apps/qip-api/src/routes.rs`, `ls frontend/portal/src/app/\(portal\)/treasury/ledger/EligibilityPanel.tsx`. Three of the twelve capabilities exist: `grep -n 'pub fn can_' backend/crates/services/qip-capital/src/ledger/entitlement.rs` returns `can_view`, `can_invest`, `can_withdraw`; `grep -rn 'can_use_crypto\|can_view_execution_trace\|can_access_admin\|can_halt' backend/crates --include=*.rs` returns nothing. |
| 40.14 | Frontend Security Boundary | PARTIAL | The "may never reach" column is enforced where it matters most — no client route reaches a node, a venue or key material: `ls backend/crates/tests/qip-acceptance/tests/api_boundary.rs` and `grep -n 'X-Frame-Options' frontend/portal/next.config.ts`. The named edge controls are absent or unvalidated: Cloud Armor is declared — `grep -rn 'google_compute_security_policy' infrastructure/terraform --include=*.tf` prints three lines in `modules/public-edge`, and **this clause claimed that command returned nothing until 2026-09-16**, contradicting the amendment at the end of this same row, Private Service Connect is behind a disabled flag (`grep -n 'enable_private_service_connect' infrastructure/terraform/modules/connectivity/main.tf`), and the zone model is wired but never planned: `head -25 infrastructure/terraform/modules/trust-zones/NOT-ENFORCED-HERE.md`. **Amended 2026-09-14 (ADR 0069).** The Cloud Armor half of this row is declared — see §40.5 — planned under `mock_provider`, and switched off everywhere. What remains needs traffic rather than code: the rate-based ban is keyed on `IP` and behaves differently behind a shared NAT, and none of these rules has ever met a request. |
| 41.1 | Workspace | PARTIAL | A single Rust workspace with the same layering discipline exists — `ls backend/crates/libs backend/crates/services backend/crates/runtime backend/crates/apps backend/crates/edge` — and the backtest-equals-live claim has a home in `qip-simulation-engine`. It is not this workspace: none of §41.1's forty crate names exists (`ls backend/crates/*/ \| grep -c '^qip-'` versus `worldmodel/ causal/ episodic/ belief/`…), and `ls backend/crates/apps` returns six binaries where §41.1 says `algorik-node` plus roughly seventy services. |
| 41.2 | The Execution Node — One Binary, Twenty-Three Modules | PARTIAL | One binary composing the regional cell exists: `ls backend/crates/apps/qip-edge-node/src backend/crates/edge`. Roughly eighteen of the twenty-three responsibilities have code (book engine `qip-orderbook`, feature engine `qip-feature-dag`, cycle scanner `qip-arbitrage`, path router `qip-routing`, feasibility `qip-edge/src/feasibility.rs`, netting/crossing `qip-edge/src/cell.rs`). Four have none: `grep -rli 'inference\|greek\|adversary' backend/crates/edge backend/crates/apps/qip-edge-node/src --include=*.rs \| grep -v /tests/` returns nothing. And the pass itself runs only under one feed value: `grep -n 'QIP_VENUE_FEED' backend/crates/apps/qip-edge-node/src/main.rs`. |
| 41.3 | Thread and Core Assignment | PARTIAL | Half exists, in Terraform rather than in code: `grep -n 'isolated_cpus' infrastructure/terraform/modules/execution-node/main.tf` derives `2-(vcpus-1)` and the startup script refuses a kernel that lacks it (`grep -n 'isolcpus=' infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl`). The table itself — sixteen threads assigned to named cores — is absent from every binary: `grep -rn 'core_affinity\|sched_setaffinity' backend/crates --include=*.rs` returns nothing, and no thread in `qip-edge-node` is pinned. |
| 41.4 | Node Configuration | UNREACHED | Every row is implemented and refuses at boot rather than warning: `grep -n 'machine_type\|TIER_1\|compact\|isolcpus\|Hugepagesize\|/proc/swaps\|Restart=always\|WatchdogSec\|shadow_mode' infrastructure/terraform/modules/execution-node/main.tf infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl`. No caller supplies an instance: `grep -n 'module "execution_node"' -A3 infrastructure/terraform/main.tf` shows `for_each = var.execution_nodes`, and `grep -rn 'execution_nodes' infrastructure/environments/*/terraform.tfvars` returns `{}` in all four environments. |
| 41.5 | The Shipping Payload | PARTIAL | Four of the twelve slots are produced, signed, and applied by the cell without pausing: `grep -n 'Slot::produced\|= episodic' backend/crates/apps/qip-api/src/mesh.rs` finds capital grants, risk envelope, cycle whitelist and episodic digest. The other eight are refused for reasons recorded per slot, not merely unbuilt — `grep -n 'refus' backend/crates/runtime/qip-kernel/src/central/whitelist.rs` — and six of the eight have no non-test reader on the edge. Staleness narrowing is real: `grep -n 'Freshness::Unavailable\|fn unproduced' backend/crates/libs/qip-contracts/src/policy.rs`. |
| 41.6 | Service Catalog | PARTIAL | The planes exist as library crates inside six binaries rather than as ~140 scale-to-zero services: `ls backend/crates/apps` (api, cli, deepbrain, edge-node, fastbrain, web) against `ls backend/crates/services` (24 domain engines). Only the last row is honoured in shape — `algorik-node × 3` maps to `qip-edge-node` on GCE with systemd, `grep -n 'Restart=always' infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl` — and even that has no instance (§41.4). No `policy-distributor`, `outcome-collector`, `reconciler` or `attribution-service` exists as a deployable: `grep -rn 'policy-distributor\|outcome-collector' infrastructure/terraform` returns nothing. |
| 43.1 | World and Cognition | PARTIAL | Twelve of the fourteen objects exist under some name: `grep -rn 'pub struct CausalEdge\|pub struct CausalGraph' backend/crates/services/qip-world-model/src/causal.rs`, `grep -rn 'pub struct Episode' backend/crates/libs/qip-agents/src/memory.rs`, `grep -rn 'pub struct CapabilityEstimate' backend/crates/services/qip-learning-engine/src/self_model.rs`, `grep -rn 'pub struct SourceCandidate' backend/crates/services/qip-data-finder/src/source.rs`, `grep -rn 'pub struct DeepWebAdapter' backend/crates/services/qip-data-finder/src/tier.rs`. Two do not: `grep -rn 'pub struct EntityRelation\|pub struct WorldEvent' backend/crates --include=*.rs` returns nothing. §43's own framing — "all defined once in the types crate" — is not met: the twelve live in eight different crates. |
| 43.2 | Valuation | PARTIAL | All ten objects now exist; three are read by nothing. **This row said "six of the ten" and undercounted by four, because it grepped the blueprint's names rather than the tree's** — search by definition, not by identifier. `YieldCurve` is `TermStructure`, whose module opens "Term structures: yield curves, forward curves" and which carries the discount factors and forwards the blueprint asks for: `grep -n 'pub struct TermStructure\|pub fn discount_factor\|pub fn forward_rate' backend/crates/libs/qip-market/src/curve.rs`. `VolSurface` is `VolatilitySurface`: `grep -n 'pub struct VolatilitySurface' backend/crates/libs/qip-market/src/volatility.rs`. `LogisticsCost` is `LandedCost` over `LogisticsTerms`, carrying freight, duty, clearance, storage, marketplace fees and spoilage — the blueprint's definition item for item: `grep -n 'pub struct LandedCost\|pub struct LogisticsTerms\|pub struct Spoilage\|pub struct MarketplaceFees' backend/crates/libs/qip-financial/src/physical.rs`. `CapitalCall` was the one genuinely absent object and now exists, with the consequence of failure the blueprint names as part of it: `grep -n 'pub struct CapitalCall\|pub enum CallConsequence' backend/crates/libs/qip-financial/src/cashflow.rs`. **Seven of the ten are read by non-test code.** `TermStructure` and `CreditProfile` from `grep -n 'use qip_market::curve::\|use qip_financial::credit::' backend/crates/runtime/qip-kernel/src/valuation.rs`; `AssetValuation` (which carries `ValuationMethod` and `ValuationInput`) and `CorporateAction` from `grep -n 'illiquid_marks\|corporate_actions_pending' backend/crates/runtime/qip-kernel/src/platform.rs`; `Commitment` from `grep -n 'CommitmentBook::new\|Commitment::from_private_asset' backend/crates/runtime/qip-kernel/src/platform.rs`; `AuctionState` from `grep -n 'AuctionState' backend/crates/edge/qip-orderbook/src/venue.rs`; and `CapitalCall` through `Commitment::obligation`, which `CommitmentBook::unfunded_total` sums and which the kernel subtracts from free capital before anything is sized (`grep -n 'unfunded_total' backend/crates/runtime/qip-kernel/src/platform.rs` returned four lines, three of them calls and the fourth a comment inside the `#[cfg(test)]` module — read them, do not count them). **The three that are read by nothing are why this stays PARTIAL, and none is an oversight.** `VolatilitySurface` is deliberately without a caller: ADR 0050 settles that nothing may feed it until an option-quote source passes the licensing gate, and explicitly rejects fitting it to the platform's own numbers (`grep -n 'Synthetic\|alternatives' docs/adr/0050-what-an-option-quote-source-must-satisfy-before-the-volatility-surface-has-a-caller.md`). Do not wire it to close this row. `CashflowForecast` is constructed only in tests: `Commitment::from_private_asset` goes through `unscheduled`, which sets the forecast to `None`, and `grep -rn '\.with_forecast(' backend/crates --include=*.rs \| grep -v /tests/` returns only an unrelated `DemandForecast` builder in the pre-positioner and a doc comment, so no production commitment carries a pacing model. `LandedCost` has no reader at all — `grep -rn 'LandedCost\|PhysicalArbitrage' backend/crates --include=*.rs \| grep -v qip-financial` returns nothing — and a physical-arbitrage valuation caller belongs in a service or the kernel rather than in the lib that defines it. One further honest limit on `CapitalCall`: its **read** path reaches production, its **write** path does not yet. Nothing outside tests constructs a notice, so the seam that would file one alongside `Commitment::from_private_asset` is the remaining work. |
| 43.3 | Execution, Capital and Ledger | PARTIAL | **33 of the 58 type names this section lists are declared; 25 are not.** Count them rather than trusting either figure — the blueprint's own table is the name list, so the census derives from it and cannot drift from it: `awk '/^43\.3 /{f=1} f&&/^43\.4 /{exit} f' docs/architecture/algorik-blueprint-v10.1-source.md | grep -oE '^[A-Z][A-Za-z]*( / [A-Z][A-Za-z]*)*$' | tr '/' '\n' | tr -d ' ' | grep -vE '^(Object|Definition|)$' | sort -u | while read -r n; do grep -rqE "pub (struct|enum|type|trait) $n\b" --include=*.rs backend/crates && echo "$n"; done | wc -l` prints the declared count against a list of 58 (the raw extraction yields 60; two are the table's own `Object` and `Definition` headers). The row said "roughly twenty of the thirty-five" and listed fifteen as missing by name, which was a count of rows rather than of types and was **wrong in its own evidence**: the grep it cited as returning nothing matched `DataReference` and `CollateralGraph`, both `pub struct`, so two of the fifteen were already there. Run the census; do not quote the prose. The load-bearing objects are right. The ledger is keyed per user *and* per strategy, `grep -n 'pub type LedgerKey' backend/crates/services/qip-capital/src/ledger/book.rs`; intent, net-intent and internal-cross are three distinct types, `grep -n 'pub struct Intent\|pub struct NetIntent' backend/crates/libs/qip-contracts/src/intent.rs` with `grep -n 'pub struct InternalCross' backend/crates/edge/qip-edge/src/cell.rs`. **`TaxLot` is built and read on a production path** — `grep -n 'pub struct TaxLot\|pub enum HoldingPeriod' backend/crates/services/qip-capital/src/ledger/lot.rs`. It is not a declaration: `UserLedger::fund` records one and enforces a cumulative-contribution ceiling from the lots, which the settled-balance ceiling beside it cannot do, because a realised loss reduces the number that check reads and so makes room for a contribution past the mandate — `grep -n 'fn contributed_total\|fn holding_period_distribution' backend/crates/services/qip-capital/src/ledger/book.rs`, proven by `cargo test -p qip-capital --test tax_lots` (10 tests). `holding_period_distribution` is the first producer of the "holding-period distribution" ADR 0008 names as its own reversal condition. Holding-period state is declared per jurisdiction and never guessed: an undeclared jurisdiction reports `Undetermined` rather than `Short`. **What remains is 25 names, and most should not be closed by declaring a struct.** The great majority are present in substance under another name, where adding the blueprint's spelling would duplicate a working type rather than build anything: `ExposureAggregate` is `AggregateExposure`, `ExplorationProbe` is `Probe`/`ProbeCandidate`/`ProbeOutcome`, `CapacityEstimate` is `Capacity`/`CapacityBound`, `Feasibility` is `FeasibilityRefusal` plus `EDGE_GATES`, `Trial` is `TrialLedger`/`TrialRecord`, and `Venue`, `Instrument`, `Transfer`, `Holding`, `Destination`, `WalletAccount`, `Cycle`, `GraphNode`/`GraphEdge` and `HedgeRelation` each have a family of prefixed types — `grep -rhoE 'pub (struct|enum|trait|type) [A-Za-z]*(Venue|Instrument|Exposure|Probe|Capacity|Trial|Hedge)[A-Za-z]*' backend/crates --include=*.rs | sort -u`. Genuinely absent in substance, not merely in name: `RiskVerdict`, `OptimizationRun` (as a record carrying classical solution, routing decision, quantum solution, scores and winner — `QuantumOptimization` is an agent, not the run), `SufficientStatistic`, `StrategyVersion`/`ModelVersion` as signed immutable artifacts, `CycleClass`, `InventoryPosition` and `SettlementBridge`. One live defect this row should carry rather than a missing type: `Representation` is declared and is in the netting key, but every producer builds `Spot`, so the §27.2 guard separating spot from perpetual cannot fire — the `MaxExpectedShortfall` shape. Its own doc comment says so; `grep -rn '\.with_representation(' backend/crates --include=*.rs | grep -v '/tests/'` returns nothing — note the leading dot and the trailing parenthesis, which match a call and not the two definitions a bare name grep also returns. |
| 43.4 | The Attribution Chain | PARTIAL | The money half is exact and production-reached; the cognitive tail is still unwalked, but two of this row's own claims were stale and are corrected here. Reached: `grep -n 'journal_pro_rata\|pro_rata_shares' backend/crates/services/qip-capital/src/ledger/book.rs backend/crates/runtime/qip-kernel/src/platform.rs` (splitter at `book.rs`, non-test caller in `platform.rs`), `grep -n 'fn split_pro_rata' backend/crates/services/qip-learning-engine/src/attribution.rs`, `grep -rn 'shares' backend/crates/libs/qip-contracts/src/intent.rs`. **Correction one: this row said `grep -rn 'fn explain' backend/crates --include=*.rs` returns nothing, and it does not.** It returns five non-test lines: four `fn explain` definitions, in `qip-kernel/src/causal_review.rs`, `qip-world-model/src/causal.rs` and `qip-evolution/src/{mutate,generate}.rs`, plus `explained_variance_ratio` in `qip-numerics`, which the pattern matches as a prefix — the substring trap, inside the recount command itself. What is absent is a traversal of *this* chain, not the word; say that, because the grep as written reads as false to anyone who runs it. **Correction two: `TaxLot` is substantially present under another name.** `backend/crates/libs/qip-portfolio/src/lot.rs` opens "Tax-lot accounting" and holds `Lot` with a cost basis, `RealisedTrade::holding_period`, and `LotMethod` for which lot a sale consumes: `grep -n 'pub struct Lot\|pub enum LotMethod\|fn cost_basis\|fn holding_period' backend/crates/libs/qip-portfolio/src/lot.rs`. Of the blueprint's three dimensions — basis, holding period, jurisdiction — only jurisdiction is missing, and `qip_financial::constraints::Jurisdiction` already exists to supply it (`grep -n 'pub enum Jurisdiction' backend/crates/libs/qip-financial/src/constraints.rs`). Building a second `TaxLot` elsewhere would be a second source of truth for a fact the tree already holds; the work is adding the jurisdiction dimension to `Lot` and walking it, in `qip-portfolio`. Genuinely absent as named links: `grep -rn 'struct WorldEvent\|struct OptimizationRun\|enum CycleClass\|struct RiskVerdict' backend/crates --include=*.rs` returns nothing. `CausalEdge`, `Episode`, `CapitalGrant`, `Mandate`, `StrategyFamily` exist as separate types but nothing walks fill→belief→edge→event. Nothing in this lane's §43.2 work touches the chain: `CapitalCall` is a valuation object and is not one of its links. |
| 44.1 | Deliberately Not Built | PARTIAL | The abstentions hold at runtime, including the pair once filed as contradicted. Held: two dependencies only (`grep -n -A3 '^\[workspace.dependencies\]' backend/Cargo.toml` shows `serde`, `serde_json`), no broker and no async runtime, features computed in shared crates (`ls backend/crates/edge/qip-feature-dag/src`). Also held, corrected by ADR 0053 on 2026-09-12: `grep -rn 'google_vertex_ai_metadata_store\|google_vertex_ai_endpoint\|google_bigtable_instance\|google_alloydb_cluster' infrastructure/terraform/modules --include=*.tf` finds Terraform resource blocks for a Vertex AI metadata store and endpoint, a Bigtable instance literally named `timeseries`, and AlloyDB, each gated `count = var.enable_X ? 1 : 0` with the flag `false` in all four environments (`grep -rhoE 'enable_(vertex|bigtable|alloydb)[a-z_]* *= *(true|false)' infrastructure/environments/*/terraform.tfvars`) — so zero instances of any of the five resources exist anywhere, and each variable's own description names the protocol or credential obstacle that means flipping the flag would still not make the platform use the service: no REST data plane for AlloyDB, gRPC-only for Bigtable, no client or egress path for the Vertex AI port in `qip-training`. Declared-and-disabled scaffolding with no usable adapter is what "not used" already meant; this row previously called that a contradiction, and on inspection it is not one. Note also that neither "Bigtable" nor "AlloyDB" is a name the blueprint text itself uses — `grep -in 'bigtable\|alloydb' docs/architecture/algorik-blueprint-v10.1-source.md` returns nothing — only "Vertex AI or a model registry product" and, by implication, the declined "time-series database" row map onto two of the three. **Ruled on 2026-09-15 and deliberately left `PARTIAL`: this row names no unmet requirement, and that is an honest oddity rather than an under-score.** All six abstentions hold — the four above, plus the graph database the two-dependency rule and the Terraform tree make impossible, plus the raw market-history archive, the one bucket that could be mistaken for it being `grep -n 'event-archive' infrastructure/terraform/modules/data/main.tf` — the hash-chained log's own archive, which is the record this platform is built on rather than a tick history. **The verdict is not flipped, because the vocabulary has no arm that fits an abstention.** `REACHED` means built, tested, and a non-test call path exists, and claiming it here would assert a call path into something whose whole requirement is that it not exist — exactly the inflation this register was created to end. `NARRATIVE` is wrong too: the section asks for something testable and the greps above test it. `PARTIAL` therefore reads oddly and is the least wrong of six labels, none of which was designed for a requirement met by not building. Adding a seventh verdict would change what all 181 rows sum to and belongs in an ADR, not in a cell. |
| 44.2 | Bounded State | REACHED | Fixed-capacity types with explicit refusal, on the cell's own pass path. `grep -rn 'pub const MAX_' backend/crates/edge/qip-edge/src/cell.rs backend/crates/edge/qip-edge/src/dropcopy.rs backend/crates/edge/qip-protocols/src/decoder.rs` (`MAX_OPEN_ORDERS` 256, `MAX_CROSSING_WINDOW_SAMPLES` 1_024, `MAX_FILLS_PER_ORDER` 256, `MAX_SETTLED_ORDERS` 4_096, `MAX_RETAINED_SKIPS` 64, `MAX_FRAME_BYTES` 1<<20); `grep -n 'MAX_PLAN_STRATEGIES\|MAX_HELD_GRANTS\|MAX_PLAN_BYTES' backend/crates/apps/qip-edge-node/src/strategies.rs` — a plan or grant at the bound is refused whole, never truncated. Log retention is a three-tier ceiling, not a growth: `grep -n 'retention' backend/crates/libs/qip-events/src/log.rs`. Non-test callers: `grep -n 'run_pass' backend/crates/apps/qip-edge-node/src/pass.rs backend/crates/apps/qip-edge-node/src/main.rs`. |
| 45.1 | Google Cloud | PARTIAL | Most of the catalogue is declared; three rows are deliberately absent, and nothing is deployed. Declared: `grep -rhno 'resource "google_[a-z_]*"' infrastructure/terraform --include=*.tf | sort -u` finds Spanner, BigQuery, Storage, `google_redis_instance` (Memorystore), Pub/Sub, one VPC, Cloud Router/NAT, `google_service_networking_connection` (PSC), Secret Manager, KMS, WIF pool and provider, Artifact Registry, Binary Authorization, SCC modules, the C3 execution node, and — since ADR 0069 — `google_compute_security_policy` in `modules/public-edge` (Cloud Armor with the global HTTPS load balancer and Cloud CDN), which creates nothing anywhere because `hostnames` is empty in all four environments. Still absent entirely: `grep -rn 'google_workflows_workflow\|guest_accelerator\|google_firebase' infrastructure/terraform --include=*.tf` returns nothing — no Cloud Workflows, no spot GPU, no Firebase Cloud Messaging. Cloud Run is no longer Terraform's: `grep -n 'google_cloud_run_v2_service' infrastructure/terraform/modules/cloudrun/main.tf` finds only the comment saying ADR 0036 moved it to Config Connector manifests under `infrastructure/gitops/envs/`. Cloud Build and Cloud Deploy are GitHub Actions instead (`ls .github/workflows`). **Amended 2026-09-16: Cloud HSM is now declared, and this row was wrong about two things it had inherited.** First, it searched for `google_cloud_hsm`, which is not a resource type in any provider — a question that cannot return yes however much HSM a tree declares. Cloud HSM *is* a KMS key whose version template says `protection_level = "HSM"`, so that half of the old evidence command proved nothing and has been removed rather than corrected. Second, it said "no asymmetric key material of any kind exists anywhere (ADR 0043 records that gap)", taking the claim from ADR 0069. ADR 0043 says the opposite in terms — "the platform already meets an asymmetric-signature obligation, today" — and `grep -rn 'purpose *= *"ASYMMETRIC_SIGN"' infrastructure/terraform --include=*.tf` prints the attestor's signing key in `modules/binaryauthorization`, which has been there since 2026-09-02, twelve days before ADR 0069 was written. (Match the `purpose` assignment, not the bare token: a bare `grep -rln ASYMMETRIC_SIGN` also prints `variables.tf`, where this paragraph's own argument is written down, and a recount command that matches its own citation reads itself.) That key is exactly ADR 0069's stated reversal condition for Cloud HSM, so the row closes by the condition firing rather than by the decision being reopened — but **ADR 0069's decision 2 still states the false premise and needs amending by whoever owns `docs/adr/`; that was flagged and not done in this lane.** What is declared: `grep -rn 'protection_level = var.kms_protection_level' infrastructure/terraform/modules --include=main.tf` prints all four KMS keys — secrets, evidence, backup, attestor — taking the variable where each held a `"SOFTWARE"` literal, and `grep -c 'kms_protection_level = var.kms_protection_level' infrastructure/terraform/main.tf` prints `4` for the modules the root threads the one value to. Default `SOFTWARE`, set in no tfvars, which is the level every key already held, so nothing is created and no plan changes until somebody chooses; and one value for the whole configuration means a posture that is HSM for the attestor and software for the evidence key has no way to be expressed. Both halves of the gate are planned, per ADR 0069 decision 1: `cd infrastructure/terraform && terraform test -filter=tests/kms-protection.tftest.hcl` → `6 passed, 0 failed` — two admitting runs (the default, and `HSM` explicitly) asserting on `output.kms_protection_level`, which is read off the planned key rather than echoed from the variable so that severing the two fails the run, and four refusing runs (`hsm` for the case-sensitive enum, `EXTERNAL` and `EXTERNAL_VPC` for the EKM connection this configuration does not declare, and empty). Providers are mocked, so this proves the refusals fire and says nothing whatever about what Google would accept; nothing has been applied. **One thing is documented and deliberately not gated:** raising the level on an already-applied environment does not upgrade a key — `version_template` is immutable, Terraform plans a replacement and `prevent_destroy` stops the apply — and a variable validation is handed the value and never the prior state, so a check for it could never fire. It is written on the root variable rather than added as a control that would read as protection. **Cloud Workflows, spot GPU and Firebase Cloud Messaging remain deliberately absent, re-checked 2026-09-16:** no lifecycle or ingestion orchestration exists to call Workflows; `grep -rn 'enable_vertex_ai' infrastructure/environments/*/terraform.tfvars` is `false` in all four and no training job exists, so a GPU pool would be an instance template for a machine that boots and idles; and no push consumer exists under `frontend/`. Those three hold §45.1 at PARTIAL, and for each ADR 0069's reversal condition — a grep for a caller, not a judgement about phase — is unchanged and still unmet. |
| 45.2 | IBM | PARTIAL | The classical half is built and reached; every IBM row is unreachable by construction. Built and wired: `grep -n 'qip_quantum::' backend/crates/runtime/qip-kernel/src/platform.rs` (the kernel composes `SimulatedProvider`), `grep -n 'baseline' backend/crates/libs/qip-quantum/src/benchmark.rs` — the baseline is a non-optional field, so a report without one has no representation. The port refuses: `grep -n 'transport_present' backend/crates/libs/qip-quantum/src/solver.rs` shows it hard-coded `false` with the reason named. Nothing else exists: `grep -rni 'qasm\|transpiler\|nighthawk' backend/crates --include=*.rs` finds one comment in `qip-quantum/src/provider.rs` and no QASM 3 emitter, no Transpiler Service client and no backend targeting (the only `heron` in the tree is a fixture string, `grep -rn 'ibm-heron' backend/crates/tests/qip-acceptance/tests/stress.rs`); `grep -rni 'post.quantum\|pqc\|dilithium\|kyber' backend/crates --include=*.rs` finds no Quantum Safe. Blocker named in the source: no HTTPS transport may be written in-tree (ADR 0009), and the egress proxy that would carry it is unapplied. |
| 45.3 | The C3 Trade-off | NARRATIVE | Four rows of assessment (offload penalty, Path 1 impact, when it binds, revisit trigger) with no deliverable. The one thing the decision produced is a Terraform validation: `grep -n -A6 'variable "machine_type"' infrastructure/terraform/modules/execution-node/variables.tf` refuses anything but the C3/C3D high-CPU range, and `grep -n 'TIER_1\|resource_policy\|isolated' infrastructure/terraform/modules/execution-node/main.tf` sets TIER_1 egress and a compact placement policy. No node exists to measure the penalty against: `grep -rn execution_nodes infrastructure/environments/*/terraform.tfvars` is `{}` in all four. |
| 46.1 | Trust Zones | PARTIAL | The module is built and wired; it has never been planned and holds three of thirteen zones. Built: `grep -n 'zone_names\|sanctioned_paths\|sanctioned_egress_purposes' infrastructure/terraform/modules/trust-zones/main.tf` — thirteen names, default deny both directions per zone, a path exists only if sanctioned in-file, and only `optimisation` may name `ibm-quantum`. Wired: `grep -n 'module "trust_zones"' infrastructure/terraform/main.tf`. The module's own record of what it does not hold: `sed -n '1,60p' infrastructure/terraform/modules/trust-zones/NOT-ENFORCED-HERE.md` — `terraform fmt -check`, `validate` and `plan` have not been run against it before or after wiring, so every `lifecycle.precondition` in it is an unrun assertion, and ten zones hold no workload. **Amended 2026-09-14 (ADR 0069): the "never planned" half of this row is no longer true, and planning it found two controls that could not fire.** `terraform` 1.9.8 is present in the build environment, which the module's own `NOT-ENFORCED-HERE.md` was written before. `terraform plan` still cannot run — it demands the GCS backend — but `terraform test` with `mock_provider` and `command = plan` runs a real plan against no project, creates nothing, and needs no credential: `cd infrastructure/terraform/modules/trust-zones && terraform init -backend=false && terraform test` prints `17 passed, 0 failed`. **First defect:** the Cloud NAT precondition refusing a zone that declares egress from another region counted on `local.nat_zones` — the egress zones *narrowed to this region* — so a single out-of-region zone emptied the list, `count` fell to zero, the resource was never planned, and a precondition on an unplanned resource is never evaluated. It fired only when a second in-region zone happened to sit beside it. **Second defect, in `modules/network`:** `var.console_egress_cidr == null || tonumber(split("/", …)[1]) <= 26` — Terraform evaluates both operands, `split` refuses a null, and the plan died on `argument must not be null` before any `error_message` was reached. The variable is null in `test`, `stage` and `prod`, so **`terraform plan` was impossible in three of the four environments.** The Rust test asserting that rule passed throughout, because it proves the rule by re-implementing the arithmetic rather than by planning — the mirror never sees a null. **Still `PARTIAL`, and the stated reason is corrected 2026-09-15: it is not a deployment reason.** The closing sentence read "`PARTIAL` for a deployment reason only", which would make this row a scoring error of the kind §20.2 was corrected for — the bar at the top of this document says in terms that `REACHED` does not require deployment, and on the deployment half alone this module clears it, being instantiated from the Terraform root rather than only from its own test file. It is `PARTIAL` because a stated requirement of §46.1 is unmet **in the configuration**, not in a project: the section is thirteen zones each containing the workloads it names, and `grep -n 'Three zones hold workloads' -A 8 infrastructure/terraform/modules/trust-zones/NOT-ENFORCED-HERE.md` records that the catalogue places workloads in three of them. Ten zones have no workload, no identity and, unless an environment declares one, no subnet — and that would remain true of a successful apply today. The sharpest case is the one the module writes down itself: `optimisation` is the only zone permitted to reach IBM and has nothing in it, because `qip-deepbrain` links `qip-optimization-engine` and would have to be split for the rule to bind anything that wants to pass. **That is code and configuration, not a deployment.** A zone with nothing in it constrains nothing, which is the same defect as a limit that cannot fire, and it is what this row is short of — the un-applied plan is merely the reason no verdict in this document means running. |
| 46.2 | Controls | PARTIAL | Roughly two thirds of the table has code; the remaining cryptographic rows are blocked rather than merely undone. Present: WIF only (`grep -n 'google_iam_workload_identity_pool' infrastructure/terraform/modules/cicd/main.tf`), and since 2026-09-19 that control is planned rather than only declared — `cd infrastructure/terraform && terraform test` prints `20 passed, 0 failed`, of which `tests/deploy-identity.tftest.hcl` is eight runs proving both halves of the repository gate that decides who may federate: two admitting runs asserting the committed value and a name using every character GitHub permits reach `google_iam_workload_identity_pool_provider.github.attribute_condition` whole, and six refusing runs for a CEL-quote injection, the shell advisory the domain rule records, a pasted URL, a deep link, a bare owner and an empty string. The admitting assertions read the condition off the planned provider rather than rebuilding it from the variable, so severing the variable from the provider fails the run; providers are mocked and every run is `command = plan`, so this says the configuration is coherent and its refusals fire, and says nothing about what Google's CEL evaluator accepts. The one seam it does not cover is written down in the file: `var.github_repository` also reaches the `principalSet://` member of `google_service_account_iam_member.github_impersonation`, whose string embeds a generated pool name and is therefore unknown under `plan`. No key backs any of it — `grep -rn 'google_service_account_key' infrastructure/ --include=*.tf` prints one line, a comment in `modules/execution-node/main.tf` saying there must never be one. Also present: secrets as files (`grep -n 'secret_mounts\|_FILE' infrastructure/terraform/modules/cloudrun/main.tf`); Binary Authorization (`grep -n 'google_binary_authorization_policy' infrastructure/terraform/modules/binaryauthorization/main.tf`); a scratch, non-root image (`grep -n 'FROM scratch\|USER 10001' infrastructure/docker/Dockerfile`); three independent halt disciplines (`grep -n 'fn halt(' backend/crates/edge/qip-edge/src/telemetry.rs` — `kill_switch`, `policy`, `polled`); source classification before registration and an isolated discovery enclave (`grep -n 'DiscoveryEnclave\|DarkWeb\|DefensiveMonitoring' backend/crates/services/qip-data-finder/src/tier.rs backend/crates/services/qip-data-finder/src/finder.rs`); wallet read/write separation, structural rather than audited (`sed -n '1,25p' backend/crates/services/qip-capital-fabric/src/wallet.rs`); autonomy change only through an authenticated operator (`grep -n 'request_change' backend/crates/services/qip-risk-engine/src/autonomy.rs`); both static-analysis tools the supply-chain row asks for (`grep -n 'cargo deny check' .github/workflows/ci.yml Makefile`, `grep -n 'cargo audit' .github/workflows/ci.yml Makefile`, `ls backend/deny.toml`). **Corrected 2026-09-19: this row listed Cloud HSM as absent, and it had been declarable since 2026-09-16.** §45.1 recorded the change and this row was not updated with it, which is the same drift that row was itself corrected for. `grep -rn 'protection_level = var.kms_protection_level' infrastructure/terraform/modules --include=main.tf` prints all four KMS keys — secrets, evidence, backup and attestor — and both halves of the gate are planned: `cd infrastructure/terraform && terraform test -filter=tests/kms-protection.tftest.hcl` prints `6 passed, 0 failed`, two admitting and four refusing. It is declarable and not deployed: the default is `SOFTWARE` and no tfvars sets it (`grep -rn 'kms_protection_level' infrastructure/environments/`), so no plan changes until somebody chooses, and one value for the whole configuration cannot express HSM for the attestor and software for the evidence key. Still absent, re-verified 2026-09-19: no post-quantum algorithm anywhere (`grep -rniE 'kyber|dilithium|post.quantum|ML-KEM|ML-DSA' backend/crates infrastructure --include=*.rs --include=*.tf` returns nothing); no MPC policy share (`grep -rni 'mpc' backend/crates/services/qip-capital-fabric/src` returns nothing); the node verifies a plan by digest, not by signature (`grep -n 'digest' backend/crates/apps/qip-edge-node/src/main.rs`). The signature half of that last row is one of the three gaps ADR 0043 says no in-tree code may close, so it is owner-blocked rather than pending — and note that ADR 0043 also records that the platform already meets an asymmetric-signature obligation through Binary Authorization's attestor key (`grep -rn 'purpose *= *"ASYMMETRIC_SIGN"' infrastructure/terraform --include=*.tf`), so "no asymmetric key material exists" is a claim this register has made and should not make again. |
| 49.1 | Targets | PARTIAL | Three of the fourteen targets have a harness; the rest have no measurement, and the objective type that could hold them has no caller. Harnessed, re-verified 2026-09-19: `grep -n 'ceiling_micros\|fn report' backend/crates/tests/qip-acceptance/tests/performance.rs` asserts per-operation microsecond ceilings in-test; `grep -n 'EDGE_NETTING_RATIO\|EDGE_RECONCILIATION_BREAKS' backend/crates/libs/qip-observability/src/metrics.rs` gives netting ratio and reconciliation breaks a series; belief calibration error is recorded in production (`grep -n 'BELIEF_BRIER_SCORE' backend/crates/runtime/qip-kernel/src/platform.rs`). **Corrected 2026-09-19, and the correction matters more than the verdict.** This row used to assert that a search across `backend/crates` found no target, no objective type and no comparison anywhere. Two thirds of that was wrong: `grep -rn 'pub struct Slo\b' backend/crates/libs/qip-observability/src/slo.rs` declares an objective type, and beside it `Slo::availability`, `Slo::latency`, a `default_slos()` shipping eight objectives, and an `evaluate` returning `SloStatus` that compares observed counts against the target and sets `is_met`. An objective type and a comparison both exist. The true statement, which is sharper than the false one it replaces, has two halves. First, **none of §49.1's fourteen targets is among those eight** — `grep -n 'Slo::availability(\|Slo::latency(' backend/crates/libs/qip-observability/src/slo.rs` names ingestion, risk-precheck and execution latency, the event log, the world model, reasoning, optimisation and the API, and not one of node availability during market hours, cycle completion, arrival dispersion, mirror drift or quote message-to-trade. Second, **nothing constructs one outside its own crate's tests**: `grep -rln 'Slo::availability\|Slo::latency\|default_slos' backend/crates --include=*.rs` prints exactly two paths, `qip-observability/src/slo.rs` and `qip-observability/tests/telemetry.rs`. So the objective type is the `MaxExpectedShortfall` shape in a new place — a thing that reads as measurement and that no production path can ever make fire — and that is a worse finding than the absence this row previously reported, because an absence is visible and this is not. Closing it is a change to `backend/crates/libs/qip-observability` and to `qip-acceptance`, which the infrastructure lane's allowed paths exclude, so it was found and left rather than fixed. Of the eleven unharnessed targets, the p99 latency pair, netting ratio, effective breadth, mark staleness, live-versus-holdout consistency and the two zero-tolerance rows are measurable in-test with no deployment on the `performance.rs` pattern; **node availability during market hours, cycle completion and mirror drift are the three that genuinely need a running system**, because each is a ratio over wall-clock time that only a process which has been up can produce. Nothing is scraped in any case: `grep -rn 'workload_metrics_exist' infrastructure/environments/*/terraform.tfvars infrastructure/terraform/variables.tf`. |
| 50.1 | Practical Now | NARRATIVE | A difficulty assessment, not a deliverable. Where it is checkable it is mostly right — `grep -n 'pub struct StrategyCompiler' backend/crates/edge/qip-strategy/src/compile.rs`, `grep -n 'fn split_pro_rata' backend/crates/services/qip-learning-engine/src/attribution.rs`, `grep -n 'deflated_sharpe\|PurgedSplit' backend/crates/services/qip-simulation-engine/src/validation.rs`, `grep -n 'fn score_declined' backend/crates/runtime/qip-kernel/src/platform.rs` — and wrong on two rows it calls easy: `grep -rni 'latency.equalis\|latency_equalized' backend/crates --include=*.rs` returns nothing, and `grep -rni 'webauthn' frontend/portal/src` returns nothing (passkeys are ADR 0038, proposed). |
| 50.2 | Advanced But Achievable | NARRATIVE | An assessment of what makes each capability hard; no deliverable of its own. Spot check of the two rows with in-tree traces: `grep -n 'pub fn families' backend/crates/services/qip-optimization-engine/src/families.rs` with a production caller at `grep -n 'qip_optimization_engine::families' backend/crates/runtime/qip-kernel/src/central/structure.rs`, and `ls backend/crates/services/qip-entity-resolution/src`. The "Rust quantum path, three to five weeks" row remains an estimate against `grep -n 'transport_present' backend/crates/libs/qip-quantum/src/solver.rs`. |
| 50.3 | Experimental | NARRATIVE | An assessment table. Two of its rows are contradicted by absence rather than by difficulty: `grep -rni 'regime' backend/crates/services/qip-optimization-engine/src/lib.rs` says in the module doc that there is no regime classification and no volatility index, and `grep -rni 'origination' backend/crates --include=*.rs` returns nothing and the only 'market creation' match is the acceptance suite recording it MISSING (`grep -n 'Market creation' backend/crates/tests/qip-acceptance/tests/documentation.rs`). |
| 50.4 | Not Feasible, and Why That Is Fine | NARRATIVE | Negative claims about what will not be attempted; nothing to build. The one that is structurally held rather than merely declined is the language-model role: `grep -n 'TriggerKillSwitch\|fn capabilities' backend/crates/libs/qip-agents/src/capability.rs` and `grep -n 'AgentRole::Control' backend/crates/libs/qip-agents/src/manifest.rs` — capability is a bounded enum checked against role, not a prompt. |
| 51.1 | The Gates | PARTIAL | Phase 2 and 3 have machinery; Phase 6 and 8 do not. Built and reached: `grep -n 'Holdout\|fn next' backend/crates/libs/qip-contracts/src/gate.rs` (Candidate→Holdout→Paper ladder), `grep -n 'AuthorisedPromotion' backend/crates/services/qip-lifecycle/src/ledger.rs`, `grep -n 'lifetime_trials' backend/crates/runtime/qip-kernel/src/central/factory.rs` (the non-test caller), `grep -n 'deflated_sharpe' backend/crates/services/qip-simulation-engine/src/validation.rs`. Phase 6's gate needs the platform's calibrated probability set against the market's implied one. Both halves exist separately — `grep -n 'fn implied_from_ask\|fn implied_from_bid' backend/crates/services/qip-prediction/src/pricing.rs` and `grep -n 'BELIEF_BRIER_SCORE' backend/crates/runtime/qip-kernel/src/platform.rs` — and nothing joins them: `grep -rn 'implied_from_ask\|implied_from_bid' backend/crates/runtime backend/crates/apps --include=*.rs` returns nothing, so the implied side is used only inside `qip-prediction`'s own sum-deviation arbitrage. Phase 8's gate needs regime-conditional against unconditional allocation, and `grep -rni 'regime' backend/crates/services/qip-optimization-engine/src/lib.rs` records that regime classification does not exist. |
| 51.2 | Why This Order | NARRATIVE | Six rows of sequencing rationale. The one claim with a code trace is that counterfactual scoring is early and cheap, and it is in fact the LEARN stage's production path: `grep -n 'fn score_declined\|fn evaluate_alternatives\|score_declined(' backend/crates/runtime/qip-kernel/src/platform.rs`. |
| 54.1 | Architecture and Scale | PARTIAL | Four of the eleven decisions are encoded and refuse; the rest are prose. Encoded: predicate totality — `grep -n 'pub enum Expr\|pub enum Op' backend/crates/edge/qip-strategy/src/ir.rs backend/crates/edge/qip-strategy/src/program.rs` has no loop, call or recursion opcode; budget refusal — `grep -n 'pub struct CompilerLimits' backend/crates/edge/qip-strategy/src/compile.rs`; trial budget — `grep -n 'quarter' backend/crates/services/qip-lifecycle/src/trials.rs` caps a family at five hundred per UTC calendar quarter against a cumulative chain; crossing cap — `grep -n 'forty percent cap' backend/crates/edge/qip-edge/src/cell.rs` refuses rather than clamps. Not encoded: the hot-tier cap is 256 at the node, not 1,200 (`grep -n 'MAX_PLAN_STRATEGIES' backend/crates/apps/qip-edge-node/src/strategies.rs`, whose own comment names the 1,200 it is not); `grep -rni 'MAX_FAMILIES\|family_cap' backend/crates --include=*.rs` returns nothing for the 128 working range; belief-formation-is-central-only is not structural — `grep -rn 'belief' backend/crates/edge/qip-edge/src/cell.rs` returns nothing, so the cell simply has none rather than being unable to form one. |
| 54.2 | Data and Memory | PARTIAL | Retention discipline exists; five of the nine decisions have no code. Present: bounded log retention with a permanent-retention class (`grep -n 'requires_permanent_retention' backend/crates/libs/qip-events/src/topic.rs`, `grep -n 'retention' backend/crates/libs/qip-events/src/log.rs`); manifest-by-content-hash instead of copied history (`grep -n 'manifest' backend/crates/libs/qip-financial/src/catalogue.rs`); source text discarded, facts kept (`grep -n 'manifest with a content hash' backend/crates/libs/qip-financial/src/intelligence.rs`); a reconciliation tolerance that is a policy object rather than a round number (`grep -n 'TolerancePolicy' backend/crates/services/qip-capital-fabric/src/journal.rs`). Absent: `grep -rni 'snapshot window\|rolling window by age' backend/crates --include=*.rs` returns nothing, so no snapshot roll exists; `grep -rni 'bar fallback' backend/crates --include=*.rs` returns nothing (an OHLCV `Bar` type exists at `grep -n 'pub struct Bar' backend/crates/libs/qip-market/src/bar.rs`, the three-year fallback policy does not); `grep -rni 'viable.*source\|source concentration\|second source' backend/crates/services/qip-data-finder/src` returns nothing; `grep -rni 'seven years\|SEVEN_YEARS' backend/crates --include=*.rs` matches only test text, never a retention floor; `grep -rni 'single.source' backend/crates/services/qip-reasoning-engine/src` returns nothing for the belief threshold. |
| 54.3 | Valuation and Capital | PARTIAL | The capital-side decisions are built and production-reached; the tax and origination rows are not. Present: every mark carries method and confidence and a staleness statement (`grep -n 'ValuationMethod\|staleness' backend/crates/libs/qip-financial/src/valuation.rs`); unfunded commitments reserve whole (`grep -n 'unfunded_commitment' backend/crates/libs/qip-financial/src/extensions.rs`, with the non-test caller named in the same comment, `grep -n 'deployable_capital' backend/crates/runtime/qip-kernel/src/platform.rs`); exploration is a declared mandate share checked against the desk's (`grep -n 'exploration_share' backend/crates/services/qip-capital/src/ledger/registry.rs`); feasibility precedes profitability (`sed -n '1,30p' backend/crates/edge/qip-edge/src/feasibility.rs`). UNREACHED rather than absent: `grep -n -B4 'pub enum LotMethod\|FirstInFirstOut' backend/crates/libs/qip-portfolio/src/lot.rs` gives FIFO/LIFO/highest-cost/lowest-cost beside `holding_period` and `cost_basis`, and `grep -rn 'LotMethod\|FirstInFirstOut' backend/crates --include=*.rs | grep -v qip-portfolio` returns nothing, so no exit decision selects a lot; `grep -rni 'market creation' backend/crates --include=*.rs` matches one line only, the acceptance suite's own row recording it as MISSING (`grep -n 'Market creation' backend/crates/tests/qip-acceptance/tests/documentation.rs`); no settlement bridge or margin model. |
| 54.4 | Quantum, Interfaces and Deployment | PARTIAL | Roughly half the table is settled in code, and two rows are settled the other way from what it says. Held: one circuit family, QAOA only (`ls backend/crates/libs/qip-quantum/src` — `qaoa.rs`, no VQE); a routing gate before quantum (`grep -n 'RoutingPolicy\|ComputeRouter' backend/crates/runtime/qip-kernel/src/config.rs backend/crates/runtime/qip-kernel/src/platform.rs`); session wallet connection correctly not adopted; deep-web access class and dark-web separation are types, not prose (`grep -n 'pub enum AccessMode\|pub struct DeepWebAdapter\|pub struct DefensiveMonitoring' backend/crates/services/qip-data-finder/src/tier.rs`); divestment and DeFi rows partly held by `ls backend/crates/services/qip-chain/src` (`amm.rs`, `finality.rs`). Settled otherwise: mobile is not a Leptos PWA, and that is owner-blocked rather than undone — `grep -rni leptos . --include=*.rs --include=*.toml --include=*.tsx` returns nothing anywhere in the repository (re-run 2026-09-19), the dependency is unauthorised under `ls docs/adr/0025-*`, whose status line reads half decided and half still the owner's (`grep -n -iE '^\*?\*?status' docs/adr/0025-*.md`), and adding the crate would breach the two-dependency rule before any code existed to justify it; the console is Next.js (`find frontend/portal/src/app -name page.tsx | wc -l` → 68); infrastructure is Terraform, not OpenTofu (`grep -rn 'tofu' .github/workflows infrastructure/terraform --include=*.yml --include=*.tf` returns nothing, `grep -n required_version infrastructure/terraform/main.tf`). ISA compilation via the Transpiler Service does not exist (`grep -rni transpiler backend/crates --include=*.rs` returns nothing). |
| 56.1 | Language, Money and Memory | PARTIAL | Five of the seven rules hold structurally; two do not. Held: rule 1 for the backend (rule's own exception is the browser layer, ADR 0001) — `ls backend/crates/*/`; rule 3 in substance, an integer-scaled decimal and never `f64` for money (`grep -n 'pub struct Decimal' backend/crates/libs/qip-core/src/decimal.rs`) though the named crate `rust_decimal` is forbidden by the two-dependency policy (`grep -n -A3 '^\[workspace.dependencies\]' backend/Cargo.toml`); rule 5 (`grep -rn 'pub const MAX_' backend/crates/edge --include=*.rs`); rule 6 (`grep -n 'unsafe_code' backend/Cargo.toml` → `forbid`, no reviewed-primitive exception at all); rule 7 (`ls backend/crates/libs/qip-core/src backend/crates/libs/qip-contracts/src`). Not held: rule 2's "managed services are Google Cloud or IBM" is true only because no IBM service is reachable (`grep -n 'transport_present' backend/crates/libs/qip-quantum/src/solver.rs`); rule 4 has no arena allocator — `grep -rn 'arena' backend/crates --include=*.rs` finds only the compiled-program node arena in `central/factory.rs` and `central/dna.rs`, which is a shipped data structure, not a per-cycle reset allocator. |
| 56.2 | Strategies, Netting and Risk | PARTIAL | Twelve of sixteen rules hold; **three have no code and two are built with no production caller — corrected 2026-09-15 from "five have no code", see 19 and 20 below.** Held: 8, 9, 15 (`grep -n 'pub struct StrategyCompiler\|pub struct CompilerLimits' backend/crates/edge/qip-strategy/src/compile.rs`, `grep -n 'pub enum Op' backend/crates/edge/qip-strategy/src/program.rs`); 10 and 12 (`grep -n 'shares' backend/crates/libs/qip-contracts/src/intent.rs`, `grep -n 'fn journal_pro_rata' backend/crates/services/qip-capital/src/ledger/book.rs`); 11 (`grep -n 'aggregate' backend/crates/libs/qip-risk/src/aggregate.rs`); 13 (`grep -n 'NettingPolicy::NoNet' backend/crates/edge/qip-edge/src/cell.rs`); 14 and 21's reservation half (`grep -n 'fn reserve' backend/crates/edge/qip-edge/src/reservation.rs`); 17, corrected by ADR 0052 on 2026-09-12 — filed as contradicted because `PreTradeDecision` has `Approved` and a `Reduced` arm (`grep -n -A16 'pub enum PreTradeDecision' backend/crates/services/qip-risk-engine/src/pretrade.rs`), but the blueprint's own §33 already resizes on a belief-freshness failure rather than only vetoing, `Approved` is the structural form of "silence is permission", `Reduced` is an exact `Decimal` bisection (never a guess) gated off unless a composition root calls `.allowing_reduction()` — the kernel's one composition root never does (`grep -n 'PreTradeChecker::new(limits.clone())' backend/crates/runtime/qip-kernel/src/platform.rs`), so it is built, tested and dormant in production — and every uncomputed figure or checker error refuses rather than approves by default (`grep -n -A12 'match self.checker.check' backend/crates/services/qip-execution-engine/src/oms.rs`); 18 (`grep -n 'veto the cycle whole' backend/crates/edge/qip-edge/src/cell.rs`); 23 (`sed -n '1,30p' backend/crates/edge/qip-edge/src/feasibility.rs`). **19 and 20 — corrected 2026-09-15, understating on all three clauses.** The cited search does not return nothing, the direction gate is not a comment, and journal shipping is not the only `Mirror` in the edge. Rule 19 is built and checks both halves every time: `grep -n 'pub fn direction_gate' backend/crates/edge/qip-routing/src/mirror.rs` denies outright on a reference past its window before it will name a direction, then denies again if the band does not permit that direction, and `grep -n 'CHECK_REFERENCE_FRESH\|CHECK_DIRECTION_PERMITTED' backend/crates/edge/qip-routing/src/extension.rs` is the path-3 arm that reports satisfied only when both cleared — called per mirrored leg from `grep -n 'fn check_extension_for' backend/crates/edge/qip-edge/src/cell.rs`. Rule 20 is built too and fails closed: `grep -n 'CHECK_HEDGE_AT_DEPTH' backend/crates/edge/qip-routing/src/extension.rs`, whose path-4 arm refuses when no hedge facts are supplied and refuses again on a depth read on an earlier pass rather than this one. Both are `UNREACHED` rather than absent, and for different reasons worth keeping apart: `grep -rn 'install_mirror(' --include=*.rs backend/crates` finds only test callers, so path 3's gate is never asked; `grep -rn 'with_hedge(' --include=*.rs backend/crates` finds no caller in `qip-edge` at all, so a production pass assigned path 4 is **refused** by the gate rather than cleared by it — the rule holds, and holds nothing through. The edge's mirrors are `grep -n 'pub struct MirrorArrangement' backend/crates/edge/qip-edge/src/mirror.rs` and the band-and-reference machinery in `qip-routing`'s own `mirror.rs`; `journal.rs`'s `Mirror` is an unrelated trait wearing the same word, which is how this cell came to deny a module named after the thing it was looking for. Absent: 21's settlement-awareness — `grep -rni 'settlement.aware' backend/crates --include=*.rs` returns nothing; 22 — `grep -rni 'latency.equalis' backend/crates --include=*.rs` returns nothing; 16 — the node verifies a plan by digest, not by a signature (`grep -n 'digest' backend/crates/apps/qip-edge-node/src/main.rs`). |
| 56.3 | Statistics and Promotion | PARTIAL | **Qualified 2026-09-13 a second time (ADR 0057, ninth amendment); verdict unchanged and no rule in this row moves.** Rounds seven, eight and nine of the same chain (`d231679`, `88ca127`, `a190352`) touch `qip-transport`'s two files and nothing else — `git diff --name-only e691804..a190352` names `qip-transport/src/http.rs`, `qip-transport/tests/http_client.rs`, and this document and ADR 0057, the two records of the rounds themselves (`d4fc428`, the eighth entry's own commit, falls in that range) — so again no statistic, no promotion rule, no sketch bound and no vendor count here is affected, and the eight rules stand exactly as scored. The entry this qualifies follows. — **Qualified 2026-09-13 (ADR 0057, eighth amendment); verdict unchanged and no rule in this row moves.** Rounds four, five and six of the egress-redaction chain (`e26c8cf`, `ca3d581`, `e691804`) fixed three further credential leaks in the text a refused address is printed as, and changed `qip-transport` alone: no statistic, no promotion rule, no sketch bound and no vendor count below is touched, and the eight rules stand exactly as scored. Stated rather than left to be re-derived, because this row has been corrected three times from ADR 0057 rounds that *did* reach it, and a reader who sees the ADR amended an eighth time should not have to establish by hand that this one did not. The scoring this qualifies follows. — Re-scored 2026-09-12 (ADR 0057): seven of eight rules hold; one has nothing. Held: 24 and 25 (`grep -n 'deflated_sharpe' backend/crates/services/qip-simulation-engine/src/validation.rs`, `grep -n 'quarter' backend/crates/services/qip-lifecycle/src/trials.rs` correcting against a chained cumulative count rather than a batch); 26 (`grep -n 'AuthorisedPromotion' backend/crates/services/qip-lifecycle/src/ledger.rs`); 27 in part — the harness links the production strategy crates (`grep -n 'use qip_strategy' backend/crates/services/qip-simulation-engine/src/harness.rs`) but not the netting or gate crates (`grep -n 'qip-edge' backend/crates/services/qip-simulation-engine/Cargo.toml` returns nothing); 28 (`grep -n 'One passing and one vetoing fixture per rule' backend/crates/edge/qip-edge/src/feasibility.rs`); 30, new — the one streaming estimator this workspace has declares its error bound as the value it is built from (`grep -n 'pub struct ErrorBound' -A 6 backend/crates/libs/qip-numerics/src/sketch.rs`) and a consumer refuses on it (`grep -n 'tolerable_for' backend/crates/apps/qip-deepbrain/src/campaign.rs`), see §22.2; 31, new — a universe backed by fewer than two independent *vendors* is held back from promotion past validation at the one seam where this node promotes, the holdout gate's promotion to the ladder's first rung (`grep -n 'held_back' backend/crates/apps/qip-deepbrain/src/evolution.rs`, the verdict from `assess_concentration` over this stream plus every ledger source naming the subject, each with its door, counting only the vendor doors: `grep -n 'is_independent_vendor' backend/crates/services/qip-data-finder/src/reference.rs`, since a generated stream cannot withdraw access from anyone), proven by `a_universe_backed_by_one_source_is_held_back_from_promotion_past_validation` and — corrected 2026-09-12 after the second review — `two_live_admitted_connectors_over_one_subject_lift_the_hold_and_one_does_not`: the gate opens for two *live* admitted connectors over one subject through the deep brain's new connector arm (`backend/crates/apps/qip-deepbrain/src/connectors.rs`), and a replay under a connector's admission is `SourceOrigin::ReplayedAdmitted`, never a vendor, however its header reads (the earlier claim that two admitted replays opened it was the second review's high finding and is withdrawn). Corrected 2026-09-12 a fifth time (ADR 0057, fifth amendment); no rule's verdict moves: the fifth review's findings — the two URL gates brought under the transport's, the refusal that echoed a credential, the inspected log's silent append, the deep brain's two remaining exits, the billing note's mutation, the stride assertion — are resolved under §22.3 and §22.4 above, and none touches a statistic, a promotion rule or the concentration count this row is scored on; the tests this row cites are unchanged and were re-run green in the fifth round's workspace gate. Corrected 2026-09-12 a fourth time (ADR 0057, fourth amendment); no rule's verdict moves: the fourth review's findings — the event log's read-only inspection, the connector gate's parser and its `localhost` arm, the stream journal's one-key write, the deep brain's error-exit release — are resolved under §22.3 and §22.4 above, and none touches a statistic, a promotion rule or the concentration count this row is scored on; the tests this row cites are unchanged and were re-run green in the fourth round's workspace gate. Corrected 2026-09-12 a third time (ADR 0057, third amendment): this row cited `replays_under_two_vendors_admissions_back_no_vendor_and_never_lift_the_hold` for that, and the third review found its closing loop ran over an empty history — zero iterations. What held the rule meanwhile was `a_replay_under_admission_is_referenced_through_the_replayed_door_and_backs_no_vendor` at the campaign level and `a_replay_under_a_connectors_admission_is_not_a_vendor` at the ledger; the engine test now asserts its premise and asks `sources_backing` directly with two replayed vendors behind a subject, and both `is_independent_vendor` widened and the `Replayed` arm deleted fail it. S-F6 from the same review: `CountMinSketch` off the wire is now refused unless every row sums to its declared total, the count rule 30's bound is stated against (`a_sketch_off_the_wire_is_held_to_the_geometry_its_bound_implies`, two forged rows added). Its cost, stated plainly: the shipped synthetic-only deep brain counts for no vendor, `deepbrain_connector` is null in every environment, and no shipped subject has two vendors (one connector per asset class, two of them refused until their terms are read) — so promotion past validation is held everywhere until the catalogue gains a second vendor for a subject and a deployment names both. Nothing shipped opens it. Absent: 29 — `grep -rni 'before enablement\|exercised in sim' backend/crates --include=*.rs` returns nothing, so nothing requires a family or path to be simulated against recorded data before it is enabled. |
| 56.4 | Data | PARTIAL | The discovery and licensing rules (38–43) are the best-built block in the range; the retention and registry rules around them are thinner. Held: 34 and 36 (`grep -n 'manifest' backend/crates/libs/qip-financial/src/catalogue.rs`, `grep -n 'manifest with a content hash' backend/crates/libs/qip-financial/src/intelligence.rs`); 37 in part (`grep -n 'provenance source' backend/crates/services/qip-data-finder/src/ingestion.rs` records the adapter as provenance, and `grep -n 'confidence' backend/crates/services/qip-entity-resolution/src/resolver.rs backend/crates/services/qip-world-model/src/causal.rs` carries confidence into resolution and the edge; nothing bars a low-confidence extraction from establishing an edge on its own — `grep -rni 'low.confidence\|minimum_confidence\|confidence_floor' backend/crates/services/qip-world-model/src/causal.rs` returns nothing); 38, 39, 40, 41, 43 (`grep -n 'pub struct DiscoveryEnclave\|pub enum AccessMode\|pub struct DefensiveMonitoring\|pub struct DeepWebAdapter' backend/crates/services/qip-data-finder/src/tier.rs`, `grep -n 'SourceTier::DarkWeb' backend/crates/services/qip-data-finder/src/finder.rs`, `grep -n 'LicensingPosture\|LicensingDecision' backend/crates/services/qip-data-finder/src/admission.rs`, `grep -n 'RobotsPosture' backend/crates/services/qip-data-finder/src/tier.rs`), reached from the kernel (`grep -n 'qip_data_finder::' backend/crates/runtime/qip-kernel/src/platform.rs`); 42 (`ls backend/crates/services/qip-data-finder/src/health.rs backend/crates/services/qip-data-finder/src/scoring.rs`). 33 in part, since 2026-09-12 (ADR 0057) — the retention class is now a declared type (`grep -n 'pub enum RetentionClass' backend/crates/services/qip-data-finder/src/retention.rs`) and one of its rows, the fallback series, is enforced as a class with its own bounds; the event log still retains by topic group without naming a class per record, which is the half of "every stored byte carries a declared class" not yet met; 35, since 2026-09-12 — the research cache is TTL-scoped and reaped (`grep -n 'fn evict_expired\|pub struct CacheBound' backend/crates/services/qip-data-finder/src/campaign.rs`) and the deep brain's campaign runs under that bound in production (`grep -n 'CAMPAIGN_TTL' backend/crates/apps/qip-deepbrain/src/campaign.rs`), see §22.4. Absent: 44 — `grep -rni 'AssetClassRegistry\|unregistered class' backend/crates --include=*.rs` returns nothing. Rule 46 is PARTIAL rather than UNREACHED, and the half that moved is not the half this row used to describe: `Unwinding` and `Flagged` now have production writers on the backtest seam (`grep -n 'flag_position\|begin_unwind' backend/crates/services/qip-simulation-engine/src/backtest.rs`, reached from `grep -n 'Backtester::new' backend/crates/apps/qip-deepbrain/src/evolution.rs`), so a position whose strategy has withdrawn its thesis is recorded rather than silently held. `Orphaned` still has none, and nothing reassigns or schedules a *retired strategy's* position: the retirement is known only at the centre, whose position book is `StrategyLot` and not `qip_portfolio::Position`. See §35.1 for why that gap is left open rather than closed with an invented trigger. Rule 32's "raw stream never persisted" is held by absence rather than by a control that refuses. |
| 56.5 | Cognition | PARTIAL | **The opening count is removed rather than re-stated, 2026-09-15.** It read "six production-reached, four absent" and was taken before rules 49 and 50 landed; a corrected number here would be stale on the same terms, and this document has twice found a figure kept technically true while drifting. Read the rules. Rule 47 is still the load-bearing miss. Reached: 48 (`grep -n 'sizing_multiplier' backend/crates/edge/qip-edge/src/cell.rs`); 52 and 53 (`grep -n 'fn score_declined\|fn evaluate_alternatives\|score_declined(' backend/crates/runtime/qip-kernel/src/platform.rs` — called from the LEARN stage, and recalibration goes through the approval path at `grep -n 'request_change' backend/crates/services/qip-risk-engine/src/autonomy.rs`); 54 (`grep -n 'falsifier\|AlternativeExplanation' backend/crates/services/qip-reasoning-engine/src/hypothesis.rs backend/crates/services/qip-reasoning-engine/src/redteam.rs`); 55 (`grep -n 'exploration_share' backend/crates/services/qip-capital/src/ledger/registry.rs`); 57 (`grep -n 'BELIEF_BRIER_SCORE' backend/crates/runtime/qip-kernel/src/platform.rs`); 58 for the money half only, see §43.4; 59 (`grep -n 'pub enum Capability' backend/crates/libs/qip-agents/src/capability.rs`, `grep -n 'AgentRole::Control' backend/crates/libs/qip-agents/src/manifest.rs`). Absent: 47 — `grep -n -A5 'pub struct BeliefPriors' backend/crates/libs/qip-contracts/src/policy.rs` is a `BTreeMap<String, f64>`, a point estimate per subject with no distribution, evidence, causal path or per-belief TTL, which is exactly what the rule calls a defect; 49 — **corrected 2026-09-15, understating, and the cell asserted the opposite of the tree.** It said the distinction exists only at the risk gate and not at belief level, citing that search as returning nothing; `grep -rn 'net_stance_disagreement\|EvidencePosture\|is_conflicted' backend/crates/services/qip-reasoning-engine/src` answers in `evidence.rs` and `hypothesis.rs`. `EvidenceSet::posture` names absence, conflict and unopposed as three distinct states — an absence is not an empty set — and `net_stance_disagreement` weighs the weaker side against the stronger over independent origins, narrowing size by `1 - 0.6d` so a contested thesis is admitted and sized below an uncontested one. Different representation *and* different behaviour, which is the whole of the rule. §11.2 credits the same code as production-reached on the DECIDE path and has since ADR 0063; this cell was not re-read when that landed. Rule 49 is reached. The risk gate's `checks_run` (`grep -n -B2 'absence of evidence' backend/crates/services/qip-risk-engine/src/pretrade.rs`) is a second place the distinction holds, not the only one; 50 in part, **and the stated reason is corrected 2026-09-15**: "conditions unverified" was true before §9.1's conditions layer landed and is not now. `grep -n 'pub enum ConditionStanding\|pub fn with_conditions' backend/crates/services/qip-world-model/src/causal.rs` carries the regime an edge holds or fails in, both arms are written in production by the temporal-precedence pass (`grep -n 'with_conditions(BTreeSet::from\|record_causal_condition_failure' backend/crates/runtime/qip-kernel/src/platform.rs`), and §9.1 is `REACHED` on that. What keeps rule 50 partial is now a structural point and not a missing layer: the rule says an edge without conditions is not an edge, and `CausalEdge` still constructs perfectly well with none, so the guarantee rests on the writer rather than the type; 56 — `grep -rni 'single.source' backend/crates/services/qip-reasoning-engine/src` returns nothing. |
| 56.6 | Valuation, Capital and Money | PARTIAL | Six of nine rules hold and are reached from the kernel; three do not. Held: 60 and 61 (`grep -n 'ValuationMethod\|staleness' backend/crates/libs/qip-financial/src/valuation.rs`); 62 (`grep -n 'unfunded_commitment' backend/crates/libs/qip-financial/src/extensions.rs`); 65 and 66 (`sed -n '1,30p' backend/crates/services/qip-capital-fabric/src/wallet.rs` — the write path does not exist at all under ADR 0021, so the read path holds no key structurally rather than by dependency audit, and `grep -n 'ReconciliationOutcome' backend/crates/services/qip-capital-fabric/src/journal.rs` halts rather than corrects); 68 (`grep -n 'TolerancePolicy' backend/crates/services/qip-capital-fabric/src/journal.rs`). Reached from production: `grep -n 'qip_capital_fabric::' backend/crates/runtime/qip-kernel/src/platform.rs`. Partial: 67 — the registry half exists and refuses on a supplied clock (`sed -n '1,30p' backend/crates/services/qip-capital-fabric/src/destination.rs`) but verifies no signature cryptographically and there is no hardware key. UNREACHED: 63 — `grep -n 'pub enum LotMethod' backend/crates/libs/qip-portfolio/src/lot.rs` exists with four selection methods and `grep -rn 'LotMethod' backend/crates --include=*.rs | grep -v qip-portfolio` returns nothing, so lot selection is never part of an exit; 64 — `grep -rni 'market creation' backend/crates --include=*.rs` matches one line only, the acceptance suite's own row recording it as MISSING (`grep -n 'Market creation' backend/crates/tests/qip-acceptance/tests/documentation.rs`). |
| 56.7 | Quantum, Interfaces and Platform | PARTIAL | Six of ten rules hold; two are absent and two are settled the other way. Held: 70 — classical first and a baseline that has no absent representation (`grep -n 'baseline' backend/crates/libs/qip-quantum/src/benchmark.rs`), sealed into the cycle journal; 72 in part (`grep -n 'request_change' backend/crates/services/qip-risk-engine/src/autonomy.rs`, `grep -rn 'halted' frontend/portal/src/components/chrome/PlatformProvider.tsx` — halting is on the console, but there is no hardware-key step-up: `grep -rni 'webauthn\|hardware key' frontend/portal/src` returns nothing); 73 (`grep -rn 'no control here submits a live order' frontend/portal/src/components/chrome/Nav.tsx`, `grep -rn 'PAPER TRADING' frontend/portal/src`); 75 (`grep -n 'FROM scratch\|USER 10001' infrastructure/docker/Dockerfile`); 76, since 2026-09-12 — `grep -n 'cargo deny check' .github/workflows/ci.yml Makefile` runs beside the pre-existing `grep -n 'cargo audit' .github/workflows/ci.yml Makefile`, both against `backend/deny.toml`; 78 in shape (`ls docs/adr | grep -c '^0[0-9][0-9][0-9]-'`). Absent: 69 — `grep -rni 'qasm' backend/crates --include=*.rs` returns nothing and `grep -n 'transport_present' backend/crates/libs/qip-quantum/src/solver.rs` is hard `false`, though the "no Python" half holds (`find . -name '*.py' -not -path './.git/*'` finds only `.claude/hooks/`); 74 — `grep -rln 'ActiveSpan\|SpanKind' backend/crates --include=*.rs` matches only `qip-observability/src/trace.rs` itself, so no service emits a span and no `node_id` exists (`grep -rn 'node_id' backend/crates/libs/qip-observability/src` returns nothing). Settled otherwise: 71 — `grep -rni leptos . --include=*.rs --include=*.toml --include=*.tsx` returns nothing anywhere and the mobile channel is the Next.js portal's PWA (`cat frontend/mobile/README.md`), Leptos being unauthorised under ADR 0022/0025; 77 — Terraform, not OpenTofu (`grep -rn 'tofu' .github/workflows infrastructure/terraform --include=*.yml --include=*.tf` returns nothing), emitting GCP resources only and no IBM resource at all. |

---

## Interlocks worth reading as pairs

A verdict per section hides couplings. These were found while scoring and are
the ones that change what a row means.

- **§20.2 promotion is `REACHED` and §20.3 demotion is `REACHED`, and the
  interlock between them is now closed in code.** The demotion monitor skips
  any strategy without a pilot baseline, and a baseline is seeded at the pilot
  rung. As of 2026-09-08 both halves of the path there exist: the evolution
  loop puts each candidate's holdout evidence in front of the gate, and
  `POST /strategies/:strategy/promotion-approvals` raises the dual approval the
  `Pilot` and `Scaled` rungs require. A strategy that clears its gates and is
  signed for by two operators now writes the baseline the monitor reads, which
  a kernel test asserts directly.
  What remains is not a missing wire but a missing candidate: on the committed
  synthetic tape no strategy clears the holdout gate — 47 held-out observations
  against a 250 minimum, and a Sharpe of −9.56 — so in practice nothing has
  reached a rung to be signed for, and the retirement machinery still observes
  nothing. `set_baseline` remains uncalled; it is the deliberate re-baselining
  door, not this path.
- **The factor model now exists, and what it feeds is worth watching.**
  `qip_risk::metrics::beta` had no production caller until 2026-09-08, so
  `factor_betas` was empty wherever the kernel constructed it and §19.1, §23.7
  and the learning engine's factor attribution all measured nothing. ADR 0058
  declares the benchmark — the equal-weighted return of the platform's own tape
  — and `MarketFactor` is called from `Platform::attribute`. Two floors keep it
  honest: too short an overlap, or a factor with no variance, yields **no**
  beta rather than a zero, because `metrics::beta` answers `0.0` when it cannot
  divide and that reads downstream as immunity. What remains unproven is
  whether the factor discriminates: five synthetic instruments driven by shared
  factors may all carry a beta near one, in which case the decomposition
  explains nothing while appearing to work. ADR 0058 names that as the thing
  that would make it wrong. **What it now feeds, as of the same day:** the
  SIMULATE stage stresses the open book against the standard scenario library
  and decomposes its risk over the factor, so §19.1's breadth measure and
  §23.7's stress tester both have production callers. On the committed tape the
  worst scenario is the 2008 credit crisis and the book carries a beta near one
  — which is the degenerate case ADR 0058 predicted for a universe this small,
  and the reason the row above says the factor's discrimination is still
  unproven. It also exposed one plain error in ADR 0058: the scenario library
  shocks `equity` and the factor is called `market`, and the ADR asserted the
  two names already matched. They did not, every position would have been
  reported unmodelled, and the correction is recorded in the ADR itself rather
  than quietly patched.
- **The causal graph is no longer empty by construction, and this bullet was
  wrong in that specific way until 2026-09-12.** It read "`seed_demo_world`
  is the only writer of a causal claim and has no production caller", which
  was true when written and is not now: ADR 0054 gave
  `qip_world_model::granger::establish_temporal_precedence` — a Granger-style
  temporal-precedence test over real `price_history` returns, one of the six
  methods §9.2 names — a real, non-test caller in
  `Platform::discover_temporal_precedence`, run from `stage_understand` every
  cycle. What has not changed: the bar is deliberately strict (`p < 0.01`,
  partial R² ≥ 0.02) and the confidence it can assign is capped at 0.5, so a
  production run's graph is *sparse and narrow*, not populated — most
  cycles, most instrument pairs, and every one of the other five
  establishment methods still write nothing. §9.1 and §9.2's rows are
  corrected directly; §9.3's "both take the empty arm because nothing writes
  the graph" is narrowed to "almost always", not deleted. §10.3 and §11.3 are
  untouched — neither's own stated gap (the strategy-filtered episodic query,
  the join to counterfactual scores; nothing yet producing `belief_priors`)
  is about the causal graph's population, and re-reading both against this
  change found no sentence of either that this writer makes false. The
  `causal_digest` payload slot is still refused rather than shipped: nothing
  in `qip-api::mesh` produces it, which is a distinct, still-open question
  from whether the graph the digest would summarise ever holds anything —
  `backend/crates/runtime/qip-kernel/src/central/whitelist.rs`'s own causal-digest
  bullet is corrected in the same change, in place, dated, for the same
  reason this one is rather than by deleting what it said before.
- **The transfer gate's seven vetoes and three bound attestations are built,
  and a producer for `FabricCommand::Gate` now exists — this bullet was
  itself stale until 2026-09-12.** It read "nothing issues a `Gate` command"
  and cited `assessments()` as "permanently empty, not transiently" after
  `qip-api`'s composition root had already gained a generic declaration
  route for all four `FabricCommand` subjects, `5adddc3` on 2026-09-08 — the
  same commit §37.1/§37.2/§38.4 credit, and §37.3's own row read the fix
  before this bullet did. ADR 0021 *permits* this gate — "a gate that
  refuses is the safe half" — and refuses only the signing and withdrawal
  engines, which is unaffected: no environment mounts a declaration, so no
  deployment has exercised a `Gate` command through it yet, and that half
  of the old claim — nothing *deployed* has assessed a real corridor — still
  holds. What no longer holds is "nothing issues one": the route is real,
  generic, and in the binary.
- **The §23.4 pool gate is armed from `stage_learn` every cycle, and one
  composition root now loads `CentralConfig::horizons`.** `qip-deepbrain`
  reads `QIP_CENTRAL_HORIZONS_PATH` and overlays a parsed `HorizonPolicy`
  onto the default central configuration before assembly; `qip-fastbrain`
  does not, by scope decision rather than oversight (a horizon policy is a
  claim about strategy lifecycle, which is this node's business and not the
  execution-only fast path's). The Terraform half now exists too — a root
  variable (`central_horizons_file`), null by default and mounted on
  `qip-deepbrain` the same way `capital_fabric_file` is mounted on `qip-api`
  — and every environment's tfvars still leaves it null, with the reason
  written beside it; the `qip-acceptance` allowlist entry this used to cite
  is deleted, because the variable is now a conditional catalogue value the
  suite's general rule accounts for rather than an exception it excuses. So
  the gate still arms nothing in any deployment today, and the strategies it would
  reconcile across horizons are still never promoted on the committed
  synthetic tape (§20.2).

---

## Provenance, and how to re-score

Scored on 2026-09-07 against branch `claude/algorik-architecture-refactor-pmp0zy`,
by six independent passes over disjoint section ranges, each applying the bar
above and each required to give a runnable command rather than a line number.
Verdict tallies were re-counted from the rows rather than taken from the
passes' own reports — one pass miscounted its own `PARTIAL` total by one, which
is the reason for the rule.

Workspace gate at the time of scoring: `cargo test --workspace --no-fail-fast`
exit 0, **4823 passed, 0 failed**; `cargo clippy --workspace --all-targets`
zero warnings; `cargo fmt --all --check` clean; dependency policy 11
third-party packages, all permitted; secret scan nothing found.

**Re-scored 2026-09-08**, three rows only. §37.1, §37.2 and §38.4 moved
`UNREACHED` → `REACHED` when the capital-fabric declaration gave them the
production caller they had never had; the shape table above moved with them,
and nothing else was touched. Their shared defect was one sentence — "every
caller is a test" — and it was one defect wearing three numbers, which is why
one change closed all three. Gate at that point: **4837 passed, 0 failed**,
clippy zero warnings, `fmt --all --check` clean, dependency policy 11
packages all permitted, secret scan nothing found, `terraform fmt -check` and
`terraform validate` both clean.

**Amended 2026-09-08**, no verdict changed: §20.3's evidence records that
`CostOverrun` could not fire before the centre measured execution cost, and
now can. Gate at that point: **4842 passed, 0 failed**, clippy zero warnings,
`fmt --all --check` clean, dependency policy 11 packages all permitted, secret
scan nothing found, `terraform fmt -check` and `terraform validate` clean.

**Re-scored 2026-09-08**, §20.2 `UNREACHED` → `PARTIAL` when the ladder's
first rung got a production caller, with §20.3's parenthetical about
`promote` and the interlock note above corrected to match. The shape table
moved with it. Gate: **4843 passed, 0 failed**, clippy zero warnings,
`fmt --all --check` clean, dependency policy 11 packages all permitted,
secret scan nothing found, `terraform fmt -check` and `validate` clean.

**Amended 2026-09-08**, no verdict changed: §20.2 records the dual-approval
route for the signed rungs, and the interlock note above is rewritten — the
gap it described is closed in code, and what is left is that nothing has yet
cleared a gate to be signed for. Gate: **4848 passed, 0 failed**, clippy zero
warnings, `fmt --all --check` clean, dependency policy 11 packages all
permitted, secret scan nothing found.

**Re-scored 2026-09-08**, §20.2 `PARTIAL` → `REACHED`. The earlier `PARTIAL`
applied a bar this document does not state — it asked that the path had been
*exercised successfully*, where the bar asks that a non-test path *exists*.
Two defects found while establishing that are fixed in the same change: the
hand-rolled purged folds that disagreed with the splitter the gate rebuilds
them with, and a search threshold that let futile rounds charge trials which
deflate every later candidate. Gate: **4850 passed, 0 failed**, clippy zero
warnings, `fmt --all --check` clean, dependency policy 11 packages all
permitted, secret scan nothing found.

**Amended 2026-09-12**, no verdict changed: two of the four rows under "Where
the blueprint and the code disagree" are resolved by ADR 0052 (§56.x rule 17)
and ADR 0053 (§44.1's Vertex AI/Bigtable/AlloyDB row), and §56.2's and §44.1's
own rows are corrected to match — §56.2 moves from "eleven hold, one
contradicted" to "twelve hold", and §44.1's contradiction note is replaced
with the reason it was not one. Neither section's `PARTIAL` verdict changes:
§56.2 still has five rules with no code and §44.1's other rows are unaffected.
The editing session had no shell, so the gates below were run afterward, in
this same working tree, before commit. `./scripts/check-secrets.sh` →
`secret scan: nothing found`. `cargo test -p qip-acceptance --test
documentation --test truth_loop --no-fail-fast` → **25 passed, 0 failed** in
`documentation.rs` (including `no_scored_plan_document_calls_a_search_empty_
that_is_not` and `no_scored_document_denies_the_existence_of_a_type_the_
workspace_defines`, the two suites most likely to catch a bad edit to this
file) and **8 passed, 0 failed** in `truth_loop.rs`. The full
`cargo test --workspace --no-fail-fast`, `cargo clippy --workspace
--all-targets` and `cargo fmt --all --check` were not run as part of this
amendment specifically — a parallel change in the same working tree was
wiring `cargo-deny` into CI at the time, and running the full workspace gate
belongs to whichever commit lands last against a working tree both changes
have settled into, not to this documentation-only one. No Rust source,
Terraform, or `Cargo.toml`/`Cargo.lock` file was touched by this amendment —
only this document, the blueprint source's rule 17 line, and two new ADRs.

**Re-scored 2026-09-12**, no verdict changed. This is the commit the amendment
immediately above anticipated landing after it: `cargo-deny` is now wired —
`backend/deny.toml`, a `dependency-supply-chain` job in `ci.yml` and a `deny`
target in the Makefile (folded into `make all`, not `make check`, because its
advisories check reaches the network the same way `cargo audit` already does)
— which closes §56.x rule 76 without an ADR, since cargo-deny audits the two
dependencies ADR 0002 permits rather than being a third one itself. Item 4 of
"Where the blueprint and the code disagree" is resolved, its own intro line
corrected from "two of the four" to "three of the four", the cargo-deny bullet
under "Absences this document asserts" is corrected from "not wired" (false,
once this landed) to the narrower and still-true claim that no crate depends
on it, and §46.2 and §56.7 move the same fact from their Absent lists to
their Held/Present ones — §56.7 from "five of ten hold" to "six of ten hold".
No section's verdict moved: §46.2 and §56.7 are still `PARTIAL`, still short
of other rows in the same tables. `cargo deny check` (installed with `cargo
install cargo-deny --locked`, resolved to 0.20.2) reports **advisories ok,
bans ok, licenses ok, sources ok**, exit 0 — no finding needed fixing, so
`deny.toml`'s `[bans]` allowlist could be written to match
`scripts/check-dependencies.sh`'s eleven packages exactly on the first pass.
Full workspace gate, run against the tree with both this change and the
amendment above settled into it: `cargo fmt --all --check` clean; `cargo
clippy --workspace --all-targets` zero warnings; `cargo test --workspace
--no-fail-fast` exit 0, **4850 passed, 0 failed** (unchanged from the prior
entry — this change adds no Rust test, only CI/Makefile/config); dependency
policy `dependency policy: 11 third-party package(s), all permitted`; secret
scan `secret scan: nothing found`. Terraform gates were not run: no Terraform
file was touched.

**Re-scored 2026-09-12**, eleven rows, closing the Phase 2 backlog of
already-built, already-tested code with no production caller. Seven moved
`UNREACHED`: §7.5, §7.6 and §7.6.2 to `REACHED` and §15.1, §15.3, §19.1 and
§23.7 to `PARTIAL` (each still short of `REACHED` on a stated gap unrelated to
the wiring — no CRAWL stage, three of the four remaining meta-learning
capabilities, three of the four remaining agent-based uses, the 10x15x12x6
enumeration, and two of `StressTester`'s four methods, respectively; §7.4
stays `PARTIAL` for the same CRAWL-stage reason). §37.3 moved `PARTIAL` →
`REACHED` without new code: it was a stale row, not a stale fact, citing a
grep for a sentence in `platform.rs` that the producer's own landing (`5adddc3`,
2026-09-08) had already rewritten in place — the same commit §37.1, §37.2 and
§38.4 above already credit, so this closes one defect wearing a fourth number
the 2026-09-08 pass did not revisit. §23.4 kept its `REACHED` verdict and
gained a real half: a composition root (`qip-deepbrain`, chosen over
`qip-fastbrain` because a horizon policy is a claim about strategy lifecycle)
now loads `CentralConfig::horizons` from an operator-declared file, closing
the "no composition root ever sets it" half of the row's own evidence. The
"Interlocks worth reading as pairs" section is corrected for both the
transfer-gate and the pool-gate bullets, each naming which half changed and
which did not — no environment mounts a fabric declaration or sets either new
discovery or horizon variable, so nothing *deployed* has exercised any of
these paths, which none of today's changes claims otherwise. The causal graph
(§9.x, §10.3, §11.3, `seed_demo_world`) was explicitly left alone: it needs
real causal-edge extraction, not a wiring fix, and remains for a later phase.

Every new production caller keeps its own scope stated rather than
overclaiming: `qip-deepbrain`'s `NetworkProbe`-backed discovery pass was
described here as refusing every call by name until a TLS-capable transport
was authorised (ADR 0009) — withdrawn at the 2026-09-13 merge: the probe
reaches a source through the reviewed egress route its catalogue entry names
(ADR 0060), and no environment mounts a catalogue, so it is real and in the
binary rather than actually assessing anything today for that reason instead;
`qip-kernel`'s `capacity_probe` declares its zero-factor scope in its own doc
comment and metric description beside `qip_portfolio_effective_bets`'s own,
and `stress_test_book`'s full-beta scope was superseded the same day by
`stress_the_book`, which stresses at the measured beta (ADR 0058); the
meta-learning consultation in `SuccessionDesk::judge` only ever tightens the
promotion decision, never loosens it, and changes nothing for a pairing with
insufficient evidence. Three new environment variables
(`QIP_CENTRAL_HORIZONS_PATH`, `QIP_DEEPBRAIN_DISCOVER_EVERY`,
`QIP_DEEPBRAIN_SOURCE_CANDIDATES_PATH`) read by `qip-deepbrain` are named in
`qip-acceptance`'s `manifest_wiring.rs` `READ_BUT_NOT_SET` allowlist with a
reason that says so explicitly: the Terraform half of each — a root variable,
left null with a reason in every tfvars, the same treatment
`QIP_WALLET_STATEMENT_PATH` and `QIP_CAPITAL_FABRIC_PATH` already have — is a
separate, out-of-scope change for this pass, not an argument that the
variable cannot be set. Fourteen new tests were added across
`qip-deepbrain` (`succession.rs` three, `discovery.rs` two, `main.rs` four)
and `qip-kernel` (`platform.rs` five: one for effective bets, four for the
capacity probe and stress test), matching the rise from 4850 to 4864 exactly;
every one was
mutation-verified — the implementation broken, the test confirmed to fail for
the stated reason, the code restored byte-for-byte, the test confirmed to
pass again — and each mutation and its result is recorded in the commits that
introduced the test. Paper trading is untouched at all three layers: no
Terraform file, no `AutonomyLevel::deployable` call site, and no `qip-edge`
constructor were touched by any change in this pass. Gate: `cargo fmt --all
--check` clean; `cargo clippy --workspace --all-targets` zero warnings;
`cargo test --workspace --no-fail-fast` exit 0, **4864 passed, 0 failed** (up
from 4850); dependency policy `dependency policy: 11 third-party package(s),
all permitted`; secret scan `secret scan: nothing found`. Terraform gates
were not run: no Terraform file was touched, by design (see above).

**Re-scored 2026-09-12**, no verdict changed: the Terraform half the pass
above deliberately left out is landed. Three root variables —
`central_horizons_file`, `source_candidates_file`, `deepbrain_discover_every`
— are declared in `infrastructure/terraform/variables.tf`, each null by
default with the same validated-path or validated-cadence shape
`capital_fabric_file` and `cycle_interval_seconds` already use; the first two
are mounted on `qip-deepbrain` through `optional_config_files.deepbrain` in
`catalogue.tf`, exactly the way `capital_fabric_file` is mounted on `qip-api`,
and the third is a conditional entry in the workload's own `env` block, the
same shape `market_data_connector` already takes on `qip-fastbrain`. Every
environment's tfvars leaves all three null, with the reason written beside
them the same way `wallet_statement_file` and `capital_fabric_file` already
are. The corresponding `READ_BUT_NOT_SET` entries in `qip-acceptance`'s
`manifest_wiring.rs` are deleted rather than left to rot: the general rule
that already accounts for `market_data_connector` — a variable the catalogue
sets only inside a `var.x == null ? {} : {` arm is left to the tfvars'
decision rather than demanded of every deployment — now accounts for these
three as well, so an allowlist entry would have excused a capability an
operator now has, which is exactly the defect the wallet-statement row fixed
for `QIP_WALLET_STATEMENT_PATH` on 2026-09-08. §7.5's and §23.4's rows above,
and the §23.4 interlock bullet, are corrected to cite the variable
declaration and the tfvars' non-assignment rather than a bare `grep` for the
env var name, which a tfvars comment explaining *why* a variable stays unset
would now satisfy without the variable being set — the exact false-positive
this correction exists to prevent recurring. No verdict moves: nobody has
named a horizon policy, a candidate list or a cadence in any environment, so
every path these three variables would open is exactly as unreached as it
was the moment before this landed — only the second of the two facts §7.4,
§7.5 and §23.4 each track (a caller exists; an environment has chosen to use
it) is what stayed false, and this pass did not touch the first.

A real `terraform plan` against `dev`'s tfvars, evaluated locally against an
empty state with no cloud credentials (workload identity federation carries
no key this session could read, and none was created), proves two things a
`validate` cannot: the plan text is byte-for-byte identical before and after
this change (`local.cloud_run_catalogue.deepbrain.config_files` renders only
`universe` and `.env` renders the same four keys either way, since all three
new variables stay null), and each new variable's validation refuses a bad
value by name (`central_horizons_file = "/etc/passwd"` — "Invalid value for
variable" naming the repository-relative-path rule; `deepbrain_discover_every
= "six"` and `= "-1"` — the same, naming the whole-number rule) while
admitting a value shaped like the ones `capital_fabric_file` already admits.
One pre-existing character-class gap surfaced and is not fixed here, because
it is shared by all four such variables rather than introduced by this one:
`central_horizons_file`'s and `source_candidates_file`'s regex admits a `..`
segment (`data/../../etc/passwd.json` matches
`^data/[A-Za-z0-9._/-]+\.json$`), the same as `capital_fabric_file`'s own
regex does today — a defect in the shared pattern, not in following it, and
out of scope for a change that was asked to copy that pattern exactly.

Gate: `terraform fmt -check -recursive .` and `-recursive environments` both
clean; `terraform validate` `Success! The configuration is valid.`; `cargo
test -p qip-acceptance --test infrastructure --no-fail-fast` **92 passed, 0
failed**; `cargo test -p qip-acceptance --test manifest_wiring --no-fail-fast`
**18 passed, 0 failed**; `cargo fmt --all --check` clean; `cargo clippy
--workspace --all-targets` zero warnings; dependency policy `11 third-party
package(s), all permitted`; secret scan `nothing found`. `cargo test
--workspace --no-fail-fast` reported **4875 passed, 1 failed** — the one
failure, `qip-fastbrain`'s
`the_shipped_review_policy_admits_a_thesis_that_reaches_the_narrowed_sizing_budget`,
is not this change's: it depends on `qip-kernel`'s self-model sizing logic
and `qip-world-model`'s causal graph, neither of which this change's diff
touches (`infrastructure/**` and one `qip-acceptance` test file only), and
`git status` at the time showed an uncommitted, in-progress edit to
`qip-kernel/src/platform.rs` and `qip-world-model` (a temporal-precedence
capability, unrelated to this pass) landing concurrently in the same shared
checkout. Left for whoever lands that change to verify against a clean tree.

**Re-scored 2026-09-12**, the change the entry immediately above was left
waiting on: the causal graph, deliberately untouched by the 2026-09-12 Phase
2 pass above ("it needs real causal-edge extraction, not a wiring fix, and
remains for a later phase"), gets its first real extraction. §9.1 and §9.2
are corrected in place — not moved off `PARTIAL`, since five of the six
named establishment methods and both of §9.1's missing layers (conditions,
confounders) stay exactly as absent as before — and §9.3's "both take the
empty arm because nothing writes the graph" is narrowed to "almost always",
per ADR 0054. §14.3's parenthetical citing "§9.1 has no writer" is corrected
to name which method the new writer is (temporal precedence) and which it
is not (hypothesis-plus-falsification, still absent), so the row's own
finding — a supported hypothesis never becomes a candidate edge — is not
read as closed by a writer that cannot produce that kind of edge at all.
The causal-graph interlock bullet is rewritten in place, dated, the same way
this document's own history says a stale claim should be corrected rather
than silently deleted.

The failure the previous entry flagged is now explained rather than merely
excused: `qip-fastbrain`'s `the_shipped_review_policy_admits_a_thesis_that_
reaches_the_narrowed_sizing_budget` failed because it is exactly the kind of
production caller ADR 0054 describes — a real cycle, over a committed tape's
real instrument returns — and this tape's own two instruments clear the new
writer's bar well before the first thesis ever reaches the construction seam,
so every construction the test observes sees a causal graph already `Fresh`
rather than permanently `Unavailable`. The test's fixed one-dimensional
expectation (self-model calibrated or not) is corrected to the two
independent facts DECIDE actually reads (self-model calibrated; causal graph
fresh), with four hand-computed constants replacing two and a lookup
function rather than two blind loops — never derived from
`central_degradation`'s own multiplier, which `valuation_seam.rs`'s own
comment already warns would check the arithmetic against itself. Both new
lines were mutation-verified the same way: swapping one lookup-table entry
made the per-construction check fail naming the wrong cycle and the wrong
expected value, and forcing the freshness read to `false` failed the new
premise assertion naming the reason, each restored byte-for-byte and
reverified green.

Two `Mechanism` variants are added (`TemporalPrecedence`,
`InverseTemporalPrecedence`), each documented as proposing no economic
channel — the honest cost of a method that establishes precedence rather
than a mechanism. `qip_numerics::stats::granger_causality` and
`qip_numerics::distributions::f_cdf` are new, general-purpose statistical
primitives (a nested-OLS F-test and the F-distribution CDF via the same
incomplete-beta machinery `student_t_cdf` already proves), not specific to
the causal graph, and are tested against published F-table critical values
and against the identity `F(1, ν) = t(ν)²` independently of either. No
dependency was added: both live in `qip-numerics`, which this workspace
already treats as the shared home for statistics services and the runtime
both use.

Fourteen new tests: `qip-numerics/tests/statistics.rs` (six — two for
`f_cdf`, four for `granger_causality`, including the refusal case and the
malformed-input case), `qip-world-model/tests/granger.rs` (six — the
refusal case, the dependent-pair case, the inverted-sign case, the
too-little-history case, the self-causation refusal and the non-finite
refusal), and `qip-kernel/tests/causal_precedence.rs` (two — a real lagged
pair through `Platform::observe` and `run_cycle` writes an edge, and an
independent pair does not). Every one was mutation-verified: broken,
confirmed to fail for the stated reason, restored byte-for-byte, confirmed
to pass again — the refusal tests each got two mutations (disabling the
significance bar alone, and the effect-size floor alone, to show neither
guard is redundant with the other), and each mutation and its result is
recorded in the commit that introduced the test.

The paper-trading boundary is unaffected, confirmed rather than asserted:
`grep -rn 'order\|Order\|capital\|Capital\|autonomy\|Autonomy\|placer\|Placer\|submit' backend/crates/services/qip-world-model/src/granger.rs`
finds only prose ("order of magnitude", "ordered pairs"), and the same
search over `Platform::discover_temporal_precedence` in `platform.rs` finds
only "ordered pairs" and "a deterministic prefix of a stable order" — no
interaction with an order, a capital envelope, or an autonomy ceiling
anywhere in this change. This is a read/inference addition to the
UNDERSTAND stage; nothing here sizes a position or reaches a venue.

Gate: `cargo fmt --all --check` clean; `cargo clippy --workspace
--all-targets` zero warnings; `cargo test --workspace --no-fail-fast` exit
0, **4878 passed, 0 failed** (up from 4875, the figure the previous entry's
concurrent snapshot reported with one failure that is now explained and
fixed); dependency policy `dependency policy: 11 third-party package(s), all
permitted`; secret scan `secret scan: nothing found`. Terraform gates were
not run: no Terraform file was touched.

**Re-scored 2026-09-12**, three rows in sequence, closing the chain the
blueprint itself states as ordered: §7.6.1 (Source Categories) built first,
then §22.3 (Data References) against it, then §22.4 (Fetch-on-Demand for
Research) against that (ADR 0056). §7.4's stale "Sample and Classify are
likewise absent (see §7.6.1)" is corrected — Classify now exists, Sample does
not — and §7.6.3's stale "cannot be built without §7.6.1's categories, which
are absent" is corrected to say the categories now exist and the governance
clause built on them (per-category human approval, auto-promoted adapters)
still does not.

§7.6.1 moves `ABSENT` → `REACHED`: `SourceCategory`, the eight-variant enum,
and `SourceCategory::classify`, which refuses a candidate with no declared
`ContentSignal` and refuses the three shapes §7.4's own Classify question
names that none of the eight categories admits (general news, unspecialised
discussion, a leak forum) rather than force-fitting one. Wired into
`DataFinder::assess_one` at the existing `LifecycleStage::Classify` step, on
the same production chain §7.4/§7.5/§7.6/§7.6.2 already established
(`qip-deepbrain`'s `DiscoveryDesk` through `Platform::assess_sources`) — not
a new, separate, unused function.

§22.3 moves `ABSENT` → `PARTIAL`, not `REACHED`: `DataReference` exists with
every field the blueprint's pseudo-code names, and the content hash — "the
single most important field" — reuses `qip_core::sha256_hex`, the exact
mechanism `qip_financial::manifest::SourceManifest` already uses for §7.2,
rather than a second scheme. `DataReference::of` refuses a `RegisteredSource`
with no recorded category, so the licensing-and-classification gate a
`DataReference` depends on cannot be routed around. It is not `REACHED`
because, unlike §7.6.1, it has no non-test caller: `grep -rn 'DataReference'
--include=*.rs backend/crates/apps` returns nothing, since nothing in this
build fetches real bytes for it to describe (no HTTP transport is linked in,
ADR 0009) — a structural absence this change does not paper over by adding a
network client, which was out of scope.

§22.4 moves `ABSENT` → `PARTIAL`, explicitly not rounded to `REACHED`: a
named, bounded `FetchCampaign` with a TTL- and entry-bounded `ResearchCache`,
a `CampaignManifest` that survives the campaign's own close, and
`assess_concentration` closing the concentration-risk gap the previous
version of this row named by grep. Of the section's five mitigations, three
are built in full (source revises history after use; research is slower than
a local copy; regulatory demand for data not retained), one is partial
(vendor withdraws historical access — the two-sources half only; the
three-year bar-level fallback series is §22.1's still-absent retention
taxonomy), and one is not built at all (sketch or reservoir error affects a
model — §22.2 records that no sketch exists anywhere in this codebase for a
bound to apply to). Same non-test-caller absence as §22.3, for the same
reason.

Twelve new tests, all in the three new modules — `category.rs` (4),
`reference.rs` (3), `campaign.rs` (5) — each mutation-verified: the
implementation broken, the test confirmed to fail for the stated reason, the
code restored byte-for-byte, the test reconfirmed passing. Among them: a
`ContentSignal` that fits no category is refused rather than force-classified
(mutated the `Err` arm to `Ok`, confirmed failure, restored); a re-fetch with
a different
hash is detected as `RevisionCheck::Revised` (mutated `verify` to always
return `Unchanged`, confirmed failure, restored); a universe backed by one
viable source is held back (mutated `MINIMUM_VIABLE_SOURCES` to `1`,
confirmed failure, restored).

Licensing-before-use is not weakened: `RegisteredSource`'s only constructor is
`pub(crate)`, reachable solely from `DataFinder::assess_one` after
`RegistrationDecision::registered` has already refused to run unless
`LegalAssessment::overall().is_permitted()` (`LicensingPosture::legality_for`
is what answers that). `DataReference::of` takes `&RegisteredSource`, so
there is no path from an unlicensed candidate to a `DataReference` or a
`FetchCampaign` entry — `grep -n 'RegisteredSource::new'
backend/crates/services/qip-data-finder/src/*.rs` finds exactly the one call
site, after the gate.

Gate: `cargo fmt --all --check` clean; `cargo clippy --workspace
--all-targets` zero warnings across all 58 crates; `cargo test -p
qip-data-finder --no-fail-fast` **123 passed, 0 failed** across the crate's
lib tests, all nine integration suites, and its one doc-test; dependency
policy `dependency policy: 11 third-party package(s), all permitted`; secret
scan `secret scan: nothing found`. `cargo test --workspace --no-fail-fast`
reported **4893 passed, 2 failed** — both failures (`qip-kernel`'s
`platform::counterfactual_sizing_tests::a_pattern_of_wrongly_declined_paths_
never_widens_sizing` and `tests/learning.rs`'s
`a_persistent_pattern_of_unfavourable_declines_on_one_instrument_narrows_only_
that_instruments_sizing`) are the concurrent §12.3 lane's in-progress work:
`git status` at the time showed `backend/crates/runtime/qip-kernel/src/
platform.rs` and `backend/crates/runtime/qip-kernel/tests/learning.rs` as
modified, uncommitted, and untouched by this change, and both failing tests
assert against `ADR 0055`'s not-yet-passing discount arithmetic.
`RegisteredSource::new` is `pub(crate)`, so no cross-crate effect from this
change is possible into
`qip-kernel`; the full-workspace clippy pass (zero warnings across all 58
crates including `qip-kernel`) confirms this change compiles cleanly
everywhere it is visible. Left for the §12.3 lane to land against a clean
tree, as the immediately preceding two entries in this section already
describe the same shared-checkout hazard occurring twice before. Two
doc-test targets (`qip-cli`, `qip-deepbrain`, `qip-fastbrain`) also failed
transiently in the same full-workspace run with `error[E0463]: can't find
crate for qip_api` under a concurrent cargo lock ("Blocking waiting for file
lock on artifact directory"); re-run individually once the lock cleared, all
three passed. Terraform gates were not run: no Terraform file was touched.

**Re-scored 2026-09-12**, the §12.3 lane the entry immediately above was left
waiting on: §12.3 (blueprint's "What It Changes") moves `ABSENT` → `PARTIAL`
(ADR 0055). `score_declined` (§12.2) already accumulated `DeclinedScore` and
nothing consumed it; `Platform::counterfactual_sizing_multiplier(object_id)`
now reads that bounded history and narrows `Platform::sizing_confidence` —
never widens it, by construction — once an instrument clears both a stated
minimum sample (ten scored paths) and a stated unfavourable-fraction bar
(three in four correctly declined). Of the table's four named consequences,
only "a sizing function adjusted from a counterfactual result" is built; a
rule recalibrated, a venue dropped and an allocator objective revised all
stay open, each for a reason ADR 0055 states (the guardrail against automatic
loosening for the first, and a missing upstream attribution — no venue, no
strategy or family — on `DeclinedPath`/`DeclinedScore` for the other two).
§11.2's row is corrected in place from three narrowings to four, naming the
new one and pointing at §12.3 rather than restating the argument, and §12.4's
"never loosened automatically" row is corrected from holding trivially (no
automatic path existed to check) to holding structurally (one now exists, and
has no branch that can loosen). The shape
table above is recounted for this change together with ADR 0056's, landing in
the same working tree — see the note beside it.

Five new tests, all mutation-verified (implementation broken, test confirmed
to fail for the stated reason, code restored byte-for-byte, test reconfirmed
passing), each mutation and its result reported in the commit that introduced
the test: four unit tests in `qip-kernel/src/platform.rs`'s new
`counterfactual_sizing_tests` module (a thin sample does not narrow; a clear,
sufficient, unfavourable pattern narrows to exactly the stated discount and
narrows no other instrument; a sufficient sample below the fraction bar does
not narrow; an overwhelmingly *favourable* pattern — the rule-too-tight case —
never raises sizing confidence above one, because the function has no branch
that can), and one integration test in `qip-kernel/tests/learning.rs` driving
ten real refusals through `submit_order`, `score_declined` and
`Platform::run_cycle` to prove the production wiring end to end. The entry two
above already recorded two of these four unit-test names failing transiently
mid-mutation in a concurrent run of this same working tree; both pass now that
every mutation is restored.

The paper-trading boundary and the risk/execution/autonomy paths are
untouched, confirmed rather than asserted: `grep -rn 'counterfactual_sizing_multiplier\|COUNTERFACTUAL_SIZING'
backend/crates` finds only `qip-kernel/src/platform.rs` and its own tests, and
`sizing_confidence`'s only production reader is `sizeable_theses` inside
`construct_from`, which narrows or removes a thesis before the unmodified
risk and execution gates ever see an order. No file under
`qip-risk-engine/**`, `qip-execution-engine/**`, `qip-capital/**`, `qip-edge/**`
or `infrastructure/**` was touched.

Gate: `cargo fmt --all --check` clean; `cargo clippy --workspace
--all-targets` zero warnings; `cargo test --workspace --no-fail-fast` exit 0,
**4895 passed, 0 failed** (summed across every binary's own `test result:`
line, `grep -oE '[0-9]+ passed; [0-9]+ failed' | awk` over the run's full
output — up from the concurrent entry's 4893 passed, 2 failed, both of which
were this lane's own mid-mutation state and not a defect); `cargo test -p
qip-acceptance --test compliance_proof --test security --test paper_boundary
--no-fail-fast` **7 passed, 5 passed, 24 passed, 0 failed** across the three
suites; dependency policy `dependency policy: 11 third-party package(s), all
permitted`; secret scan `secret scan: nothing found`. Terraform gates were not
run: no Terraform file was touched.

**Corrected 2026-09-12**, the same day the two rows above were re-scored:
their shared claim that §22.3 and §22.4 have no non-test caller "because no
HTTP transport is linked into this build" was true only of
`qip-data-finder`'s own `NetworkProbe` (which, at that date, refused every
call by construction; since the 2026-09-13 merge it reaches one reviewed
egress route per catalogue entry, ADR 0060 — `grep -n 'impl SourceProbe for
NetworkProbe' -A 12 backend/crates/services/qip-data-finder/src/probe.rs`),
not of the platform.
`qip-market-ingestion`'s four shipped connectors — Coinbase ticker, Alpaca
bars, Frankfurter rates, Kalshi markets — fetch real bytes over a real,
tested transport (`qip-transport`'s `HttpSourceTransport`) on every poll,
driven by both `qip-api` and `qip-fastbrain`. That correction does not move
either row to `REACHED`: `DataReference::of` requires a `&RegisteredSource`,
and `RegisteredSource` is producible only through `DataFinder`'s
discover → probe → classify → score → route → register pipeline — built for
vetting a previously-unknown candidate URL for the DISCOVER stage, not for a
connector the platform already fetches from continuously under its own,
separate `qip_data_finder::admission` catalogue. The two mechanisms are
disjoint by construction (`qip-market-ingestion` cannot depend on
`qip-data-finder`; the edge runs the other way), and in production today the
discovery pipeline's only caller reaches `NetworkProbe`, which answers every
question with a refusal, so no `RegisteredSource` is produced from real bytes
by any process that exists today, on either side of this gap. The two honest
paths to actually closing §22.3 — loosening `DataReference::of`'s precondition
to the id and category it actually reads, or giving `qip-data-finder` a
second, honestly-labelled non-discovery registration path for a
catalogue-admitted source — are both reviewed design changes to a type ADR
0056 already treats as settled, and this correction makes neither
unilaterally: it corrects the record rather than forcing a wiring that would
either fabricate discovery evidence that never happened or misrepresent an
already-licensed vendor feed as a discovered one. §22.4 carries a second,
independent reason beyond inheriting §22.3's blocker: `FetchCampaign` is a
bounded, TTL-scoped, closed research run by its own module doc, and the four
shipped connectors are continuously running, restart-surviving streams —
forcing the second shape onto the first was already something this section's
own brief said not to do if it did not fit, and it does not.

*Corrected in place later on 2026-09-12 by the ADR 0057 entry below, and
kept rather than deleted because its reasoning is what that change was
built against.* Both halves of the argument held and neither was
overturned: no `RegisteredSource` was ever built for a connector, and no
connector poll was ever forced into a campaign. What changed is that the
second of the two designs this paragraph named — "a second, honestly-labelled
non-discovery registration path for a catalogue-admitted source" — was made,
as `AdmittedSource`, built from the licensing gate's own decision and the
manifest's declared category, with every `DataReference` naming its door in
a `SourceOrigin`; and that the bounded research run §22.4's arrow describes
was found where it actually is, in the deep brain's learning round rather
than in a connector. The blocker was real and is closed by a design
decision, not by a wiring exercise, which is what this paragraph asked for.

No code changed in this correction. Gate: `cargo fmt --all --check` clean;
`cargo clippy -p qip-data-finder -p qip-market-ingestion --all-targets` zero
warnings; `cargo test -p qip-market-ingestion -p qip-data-finder
--no-fail-fast` **all suites `test result: ok`, summing to 481 passed, 0
failed** (`qip-data-finder`: lib 25, `legality` 26, `lifecycle` 10,
`probe_port` 7, `registration` 6, `replacement` 7, `robots_precedence` 9,
`schema_drift` 8, `scoring_and_routing` 8, `tiers` 16, one doc-test 1;
`qip-market-ingestion`: lib 39, `alternative_data` 49, `book_depth` 50,
`connector_contract` 18, `connector_feed_frankfurter` 6, `connector_manifest`
21, `connector_runtime` 41, `hostile_rate_table` 10, `live_connectors` 6,
`narrative_feed` 46, `rest_feed` 32, `restart` 8, `sense` 31, `soak` 1);
dependency policy `dependency policy: 11 third-party package(s), all
permitted`; secret scan `secret scan: nothing found`. The full-workspace
suite was not run: a concurrent lane holds `qip-kernel/src/platform.rs`,
`qip-optimization-engine` and related kernel test files mid-edit for §12.3's
remaining consequences, which this correction does not touch and did not
need to build or test against.

**Re-scored 2026-09-12**, no verdict changed: the concurrent lane the entry
immediately above named its own investigation for. A second, independent pass
over §12.3 checked whether either of ADR 0055's two remaining consequences —
"an allocator objective revised" or "a rule recalibrated" — could now be
closed by carrying attribution through `DeclinedPath`/`DeclinedScore` rather
than by discounting an instrument's sizing again. It cannot, and the finding
is structural rather than a plumbing gap either ADR 0055 or this pass declined
to do.

At the one production site a `DeclinedPath` is built
(`grep -n 'DeclinedPath::new\|DeclinedPath {' backend/crates/runtime/qip-kernel/src/platform.rs`
finds the struct and its single push site, inside `capture_submission`), the
refused order's `hypotheses` never reach that function at all —
`grep -n 'fn capture_submission' -A 8 backend/crates/runtime/qip-kernel/src/platform.rs`
shows its parameters are `object_id`, `side`, `quantity`, `arrival`, none of
them `hypotheses` — so the dropped value was checked at its source rather than
assumed absent. It is not a family or a strategy id once found: a desk
proposal's leg carries exactly one hypothesis id, `thesis.hypothesis_id`
(`grep -n 'hypotheses: vec!\[thesis.hypothesis_id'
backend/crates/services/qip-portfolio-engine/src/construction.rs`), and that
id is minted fresh every cycle from the cycle counter and the instrument
(`grep -n 'HypothesisId::from_string(format!(' -A 4
backend/crates/runtime/qip-kernel/src/platform.rs` reads
`"hyp-{cycle}-{subject}"`) — the opposite of a recurring label a sample of ten
declines could ever accumulate against. `ApprovedThesis`, the type
construction actually sizes from, carries `hypothesis_id`, `object_id`,
`conviction`, `expected_return` and `price` and nothing else
(`grep -n 'pub struct ApprovedThesis' -A 10
backend/crates/services/qip-portfolio-engine/src/construction.rs`). The
desk's own doc comment on `DESK_STRATEGY` states the reason in one line,
unchanged since before ADR 0055: "the desk's own orders implement proposals
and carry hypotheses; they do not belong to a foundry strategy" (`grep -n
"own orders implement proposals and carry hypotheses"
backend/crates/runtime/qip-kernel/src/platform.rs`). A strategy or family
identity is not dropped on the way into `DeclinedPath` — it does not exist yet
at the point a proposal's leg becomes an order, for any order the desk itself
releases.

Separately, and sufficient on its own even had attribution existed: nothing in
production reads a family-keyed allocation weight to discount. `FamilyId`
appears nowhere outside `qip-optimization-engine` (`families.rs`,
`horizons.rs`, `lib.rs`) and `qip-kernel`'s own `central/horizon.rs` (`grep -rn
'FamilyId' backend/crates --include=*.rs | grep -v '/tests/'`), keyed either
on a foundry `StrategyId`'s realised-return series (`FamilyClustering`,
requiring capital actually funded and traded — a different kind of evidence
from a counterfactual fill, the same distinction ADR 0055 already drew for
`qip_lifecycle::DemotionMonitor`) or on positional indices over a
`HorizonPolicy`'s configured subjects, which no environment declares.
`stage_learn`'s own comment on the family-clustering call site says so
plainly: "this measures and allocates nothing: no seam in this platform
consumes a family, and a decision keyed on one would be a gate with no
subject" (`grep -n 'no seam in this platform consumes a family'
backend/crates/runtime/qip-kernel/src/platform.rs`). A discount with nothing
to multiply is not a consequence; it is dead code with a docstring explaining
why it is dead.

`grep -rln 'DeclinedPath\|DeclinedScore\|declined_scores' backend/crates
--include=*.rs` still finds only `qip-kernel/src/platform.rs` and its own
test — no new code was written, because there was no honest place to write
it. §12.3 stays `PARTIAL` at exactly ADR 0055's count: one of the four named
consequences built, three open — "no rule is recalibrated" for the reason ADR
0055's own guardrail states (§12.4 forbids the one automatic direction), "no
venue is dropped" because a declined order never reaches one, and "no
allocator objective is revised" because — now checked twice, by two
independent readings six days apart — the identity an objective would be
revised *for* does not exist at the point a path is declined, and nothing
downstream reads a family weight even where one might. Closing it would need,
at minimum, a foundry strategy or a comparable recurring identity attached to
a desk proposal before REASON hands it to construction (a change to
`qip-reasoning-engine`'s and `qip-portfolio-engine`'s output types, not a
field added to `DeclinedPath`), and a production caller of `HorizonArming`'s
family-weight allocations that no environment configures today — both outside
this task's scope crates and neither a small change. No ADR was opened for
this pass: no new decision was taken, and ADR 0055 already argued the
rejected alternative in full; this entry exists so the next reader does not
have to re-derive the same negative result from scratch.

No file under `qip-risk-engine/**`, `qip-execution-engine/**`,
`qip-capital/**`, `qip-edge/**` or `infrastructure/**` was touched, and no
Rust source was touched anywhere — this is a documentation-only entry. Gate,
re-run against the unmodified tree to confirm the baseline this entry reasons
from: `cargo fmt --all --check` clean; `cargo clippy --workspace
--all-targets` zero warnings; `cargo test --workspace --no-fail-fast` exit 0,
**4895 passed, 0 failed** (`grep -c '^test result:'` over 379 binaries summed
with `awk`, unchanged from the entry ADR 0055 landed with, as expected with no
source change); `cargo test -p qip-acceptance --test compliance_proof --test
security --test paper_boundary --no-fail-fast` **7 passed, 5 passed, 24
passed, 0 failed**, also unchanged; dependency policy `dependency policy: 11
third-party package(s), all permitted`; secret scan `secret scan: nothing
found`. Terraform gates were not run: no Terraform file was touched.

**Re-scored 2026-09-12**, the two rows the "Corrected" entry above left
`PARTIAL` with their blocker named (ADR 0057): §22.3 and §22.4 move
`PARTIAL → REACHED`; §7.2, §22.1, §22.2, §56.3 and §56.4 are corrected in
place and none changes verdict; the shape table moves 21/130 to 23/128.
The blocker was closed by a design decision, not a wiring exercise, which
is what the correction asked for: the second of the two designs it named —
an honestly-labelled non-discovery path for a catalogue-admitted source —
is `AdmittedSource`, a distinct type built only from the licensing gate's
own `LicensingDecision` (now unforgeable: a private `GatePassed` field
leaves one construction site) and the category each shipped manifest now
declares; every `DataReference` names its door in a `SourceOrigin`, with a
third door, `Generated`, for the platform's own synthetic streams, gated on
the `Synthetic` class and carrying no category. `SourceCategory` moved down
to `qip-financial` so a manifest can name it. The correction's second
reason — a connector is not a campaign — stands and shaped the caller: the
campaign wraps the deep brain's learning round, which is a bounded research
run. Six commits in dependency order: `b505195` (the door), `85b6410` (the
digest at `ConnectorRuntime::ingest`), `a63db9b` (the bounded ledger, its
consequence on the log and the metrics, both roots handing the digest up
before `observe`), `5f3e2da` (§22.1's nine `RetentionClass`es and the
daily-bar `FallbackSeries` under three stated bounds, §22.2's count-min
sketch with a declared `(ε, δ)`), `99cac32` (`qip_deepbrain::campaign::
assemble` and the rule-31 hold on promotion past validation), `6865700`
(the API boundary check widened to every file holding an `impl Platform`
block — a hole `a63db9b` had opened, found by the first full-workspace run
of the day and closed the same way it was found), and this entry's commit.

Twenty-five new tests, each mutation-verified — the implementation broken,
the test confirmed to fail for the stated reason, the code restored
byte-for-byte, the test reconfirmed passing — and every mutation recorded
in the commit that introduced the test, twenty-seven in all. Among them,
the ones the brief named: a source whose licensing is ambiguous or
research-only cannot reach the catalogue door (the usage loop skipped, the
research-only licence then mints a decision); a revised payload on the same
locator and period is flagged in the ledger, the log and the metrics (the
`Revised` block deleted, the ledger still reads revised and nothing acts);
a single-source universe is held back from promotion (`is_sufficient()`
replaced with `true`, candidates reach the gate); the sketch's declared
error past tolerance refuses the fit (`tolerable_for` bypassed); the ledger
refuses to exceed its bound (the eviction loop removed, four held against
three); a poll that decodes nothing still delivers and carries no digest
(a missing digest made to refuse the poll).

The paper-trading boundary and the risk, execution, capital and edge crates
are untouched, confirmed by the changed-file list of `git diff --name-only
bbf31c8..HEAD`, which names no file under `qip-risk-engine`,
`qip-execution-engine`, `qip-capital`, `qip-edge`, `qip-brokers`,
`qip-routing`, `qip-compliance` or `infrastructure/`. The one promotion
touched is the holdout gate's `Candidate` → first-rung step in
`qip-deepbrain/src/evolution.rs`, which holds no capital and which this
change can only ever *hold back*; the `holds_capital` assertions around it
are unchanged. No environment variable was added, so `manifest_wiring.rs`'s
allowlist gains nothing and there is no Terraform half.

Gate, run on the tree as committed: `cargo fmt --all --check` clean; `cargo
clippy --workspace --all-targets` zero warnings; `cargo test --workspace
--no-fail-fast` exit 0, **4920 passed, 0 failed**, summed across 381 `test
result:` lines; `cargo test -p qip-acceptance --no-fail-fast` all 21 suites
`test result: ok`, **348 passed, 0 failed**; dependency policy `11
third-party package(s), all permitted` (the two new edges, `qip-market` and
`qip-numerics` into `qip-data-finder` and `qip-streaming` as a
dev-dependency of `qip-deepbrain`, are workspace crates); secret scan
`nothing found`. Terraform gates were not run: no Terraform file was
touched. Frontend gates were not run: no frontend file was touched. The
independent `code-reviewer` and `security-engineer` passes the delivery
brief asked for were **not run**: the session that made this change had no
facility to launch a reviewing agent, and the review recorded in its
delivery report is the implementer's own, which is not the same thing and
is not claimed to be. The full workspace suite was run once mid-way and
once at the end; the first run was cut short by a full disk (28.7 GiB of
build output, cleaned with `cargo clean`) and is not quoted.

**Reviewed and repaired 2026-09-12**, no verdict changed. The seven ADR 0057
commits (`bbf31c8..93c4c7b`) were reviewed independently by a
security-engineer pass and a code-reviewer pass — the two passes the entry
above records as not run — and every finding is fixed in the six commits
that follow `93c4c7b`, each with its own mutation report, and this document
and ADR 0057 are corrected in place where the findings made them false.
The findings and their resolutions, by the reviews' labels:

- **B1** (blocking) — the deep brain halted on its first due learning round
  over a replay: `ReplayAdapter` reports `Restricted`, `assemble` refused the
  stream as neither admitted nor generated, `maybe_learn` propagated with `?`
  and `run` returned. A door refusal is now `Assembly::RefusedAtDoor`, a
  learning-round outcome on the round line, counted in
  `qip_research_campaigns_refused_total{gate="door"}`; the node keeps cycling
  (`a_replay_the_door_refuses_is_a_learning_outcome_and_the_node_keeps_cycling`).
- **S1** — rule 31's hold could lift in no deployment. A replay may name the
  connector it was recorded from; the root runs that source through the
  standing licensing gate and the engine re-asks it every round; two admitted
  replays from two vendors lift the hold and one does not, end to end through
  `maybe_learn` and `maybe_turn`. §22.4 and §56.3 now say plainly that
  nothing shipped opens it, and why.
- **S2** — the ledger's `symbols` were the vendor's row keys and never joined
  the campaign's `ObjectId`. The digest carries `subjects` from the mapped
  records; the reference is keyed on them; the kernel test is rebuilt through
  the real connector's `decode` and `map`.
- **F6** — generated streams counted as vendors. `assess_concentration`
  counts vendor doors only; the evolution tests back a subject with the two
  shipped connectors whose terms are read, through the real gate.
- **F1/S5** — `AdmittedSource` and `FetchDigest` derived `Deserialize`, a
  gateless constructor. Removed; `ReferenceLedger` and `FallbackSeries` too;
  `ErrorBound` and `SketchedStatistic` deserialise through their
  constructors; a `compile_fail` doctest pins `AdmittedSource`.
- **S6** — sketch memory unbounded by ε. `ErrorBound::new` refuses past
  `MAX_COUNTERS` and on overflow; `CountMinSketch::new` cannot abort or wrap.
- **F2** — a refused reference dropped a batch the connector had checkpointed
  past. `ConnectorFeed::poll_referencing` references between the poll and
  the commit and unwinds on refusal; both roots poll through it.
- **S3/F3** — foreign bodies under shared topics, no idempotency keys. Four
  topics of their own; keys on every body; the log indexes explicit keys and
  `Platform::journal_once` consults it; `journal_entries()` reads after a
  close.
- **S4** — the campaign that used the *current* bytes was flagged. One
  predicate, `RevisionRecord::contradicts`; the kernel joins a revision
  against the closed campaigns on the log and names the ones that used the
  original in `ResearchCampaignFlagged`.
- **S7** — the ledger was process-lifetime. Every reference is journaled
  before the ledger moves and `Platform::new` rebuilds the ledger from the
  log; the restart behaviour is in ADR 0057's costs.
- **S8/F4** — `flagged` and `fallback_used` restated the manifest, and the
  store held a second copy. Both read off the manifest; the store write and
  `STORE_NAMESPACE` are gone.
- **F5** — stated in ADR 0057: a connector-fed deep brain fails closed per
  subject.
- **Nits** — the dead `.max(1)`; the hash-constant note on the 600-key test;
  `SketchedStatistic::new` refusing `estimate > total`; `references.rs` one
  `impl Platform` and the boundary scan matching `impl crate::Platform {`;
  the fallback refusal once per instrument; the fast brain re-deriving
  admission as the API's re-admission route does; the dead surface removed
  or wired (`AdmittedSource::describe` reaches both banners; the rest is
  gone); the two vacuous tests strengthened so each fails when its feature
  is deleted.

Commits, in order: `d71c936` (S6, nits), `842eae5` (F1/S5, dead surface,
vacuous tests), `9b56155` (S2), `662c5af` (S3/F3, S4, S7, S8/F4, nits),
`a3a2474` (F2, fast-brain re-admission), `2ce4818` (F6, S1, B1, F5), and
this entry's commit. Twenty-one mutations across them, each recorded in the
commit that introduced or changed the test it fires. The paper-trading
boundary and the risk, execution, capital, edge, broker, routing and
compliance crates are untouched, confirmed by `git diff --name-only
93c4c7b..HEAD`, which names no file under `qip-risk-engine`,
`qip-execution-engine`, `qip-capital`, `qip-edge`, `qip-brokers`,
`qip-routing`, `qip-compliance` or `infrastructure/`. No environment
variable was added; the replay's `# recorded-from:` header is a line in a
file the existing variable already names.

Gate, run on the tree as committed: `cargo fmt --all --check` clean; `cargo
clippy --workspace --all-targets` zero warnings; `cargo test --workspace
--no-fail-fast` exit 0, **4930 passed, 0 failed**, summed across
382 `test result:` lines; `cargo test -p qip-acceptance
--no-fail-fast` all 21 suites `test result: ok`, **348 passed, 0
failed**; dependency policy `11 third-party package(s), all permitted`
(`serde_json` joins `qip-numerics` as a dev-dependency only); secret scan
`nothing found`. Terraform gates were not run: no Terraform file was
touched. Frontend gates were not run: no frontend file was touched.

**Reviewed and repaired a second time, 2026-09-12**, no verdict changed.
The six repair commits above (`93c4c7b..8a19737`) were themselves
re-reviewed independently by a security-engineer pass and a code-reviewer
pass, and every finding is fixed in the commits that follow `8a19737`; ADR
0057 carries a second amendment and §22.2, §22.3, §22.4 and §56.3 above are
corrected in place. The findings and their resolutions, by the reviews'
labels:

- **F-A** (high) — a replay file's header conferred a real vendor's
  standing on whatever bytes the file held; a hand-written bars file headed
  with one admitted connector and restarted under another read as two
  vendors and opened rule 31's hold on zero vendor bytes. Three parts. A
  replay under an admission is referenced through
  `SourceOrigin::ReplayedAdmitted` with a `replay://` locator, and it is
  not an independent vendor
  (`a_replay_under_admission_is_referenced_through_the_replayed_door_and_backs_no_vendor`,
  the two-header restart held at zero). `ReplayAdapter` refuses a headed
  file whose records the named connector never ships, a second header, and
  a header after a record, on the file road and the in-memory road alike
  (`a_replay_headed_with_a_source_that_never_shipped_its_records_is_refused`).
  And the deep brain gains a connector arm beside its own stream —
  `qip_deepbrain::connectors::ConnectorArm`, the gate re-asked before every
  poll, `Platform::admit_source` per arm, every fetch referenced between the
  poll and the checkpoint commit — on the one brain ADR 0024 gives the
  egress sidecar (the fast brain deliberately has none, and the arm it has
  always had is one it cannot use); two live admitted connectors over one
  subject lift the hold and one does not
  (`two_live_admitted_connectors_over_one_subject_lift_the_hold_and_one_does_not`),
  replays never do. Nothing shipped opens it: `deepbrain_connector` is null
  everywhere and no two shipped connectors share a subject.
- **BLOCKING-1 / F-B** — campaign ids restarted at one per process, so
  after any restart every manifest was silently suppressed as a duplicate
  while the round line said "manifest journaled". Ids carry the log's last
  sequence beside the cycle; `journal_campaign` returns whether it wrote; a
  `false` for a freshly minted id is an error; the summary reports the bool
  (`a_campaign_closed_after_a_restart_is_journaled_under_its_own_id`).
- **F-C / SHOULD-FIX-2** — `DataReference` and `RevisionRecord` derived
  `Deserialize` past `build_hashed`'s refusals, `resume_references`
  restored frames from a log `EventLog::open` never hashed (confirmed: it
  parses and refuses a reused id, and recomputes nothing), and
  `sources_backing` counted restored references without a live admission.
  Both types deserialise through their constructors; the retained chain is
  verified before a frame is restored (`EventLog::verify_retained_chain`,
  held to genesis only when the log still starts there) and a broken link
  refuses the resume; a `catalogue_admitted` reference counts only while
  the process holds the admission, and `Platform::withdraw_source` exists
  (`a_log_whose_chain_is_broken_refuses_to_rebuild_the_reference_ledger`,
  `a_restored_reference_without_a_live_admission_is_not_vendor_backing`).
- **F-D** — no unwind on a journal failure after an accepted reference.
  `poll_referencing` unwinds on either journal error, and
  `StreamJournal::record` adopts its ledger only once the store has taken
  it (`a_journal_that_cannot_be_written_leaves_the_checkpoint_where_it_was_and_the_next_poll_refetches`).
- **F-E / SHOULD-FIX-3** — a lapsed standing admission left the previous
  `AdmittedSource` in the platform. The gate is asked before a subject is
  chosen and a refusal withdraws the source
  (`a_lapsed_standing_admission_withdraws_the_source_from_the_platform`);
  a connector arm's lapse withdraws and stops the node
  (`a_connector_arm_whose_licence_lapses_withdraws_its_source_and_refuses_the_poll`).
- **F-F** — `CountMinSketch` deserialised unchecked. Through a wire type
  held to the bound's geometry
  (`a_sketch_off_the_wire_is_held_to_the_geometry_its_bound_implies`).
- **F-G** — after partial eviction a restored revision had no ledger
  entry. `SourceRevisionDetected` carries the revising reference, schema
  version two, restored on resume
  (`a_revision_restores_the_revising_reference_after_its_own_record_was_evicted`).
- **F-H** — `flag_closed_campaigns` fails closed on an undecodable frame,
  now stated in its doc; **NIT-12** — its cost is stated, and a frame whose
  manifest names no symbol the revision covers is not decoded in full.
- **SHOULD-FIX-4** — "connector-fed deep brain" is now true, and every
  passage that said it is exact about which posture applies where.
- **SHOULD-FIX-5** — `ConnectorFeed`'s `DataAdapter::poll` refuses, naming
  `poll_referencing`; the four rigs that polled through the trait pass a
  hook that accepts and say so
  (`the_adapter_contracts_poll_refuses_a_connector_rather_than_releasing_an_unreferenced_fetch`).
- **NIT-6** — a delivered poll whose records carry no subject is marked
  `unreferenced` on its report and counted on the runtime's stats
  (`a_source_whose_records_carry_no_subject_is_delivered_and_marked_unreferenced`).
- **NIT-7** — `Assembly::window()` is `#[cfg(test)]`. **NIT-9** —
  `contradicts` says why its time clause is `<=`. **NIT-10** — the
  `replay://` locator scheme. **NIT-11** — the replay test asserts the
  gate's decisions, not string literals, and asserts replays do *not* lift
  the hold.

Commits, in order: `ed33d74` (F-C, F-F, F-G, F-H, NIT-9, NIT-12, F-B, the
`ReplayedAdmitted` origin), `f31e1fc` (F-A parts one and two, F-D, F-E,
SHOULD-FIX-5, NIT-6, NIT-7, NIT-10, NIT-11), `0a9a53d` (F-A part three
and its Terraform half), and this entry's commit. Nineteen mutations
across them, each recorded in the commit that introduced or changed the
test it fires. The changed-file list `git diff --name-only 8a19737..HEAD`
names no file under `qip-risk-engine`, `qip-execution-engine`,
`qip-capital`, `qip-edge`, `qip-brokers`, `qip-routing` or
`qip-compliance`; under `infrastructure/` it names only
`terraform/variables.tf` and `terraform/catalogue.tf` (the
`deepbrain_connector` variable and its conditional arm) and the four
environments' tfvars prose explaining why it stays null. No new
environment variable by name: the deep brain reads the connector pair the
other roots read. No dependency edge was added.

Gate, run on the tree as committed: `cargo fmt --all --check` clean; `cargo
clippy --workspace --all-targets` zero warnings; `cargo test --workspace
--no-fail-fast` exit 0, **4948 passed, 0 failed**, summed across 382
`test result:` lines; `cargo test -p qip-acceptance --no-fail-fast` all
21 suites `test result: ok`; dependency policy `11 third-party package(s),
all permitted`; secret scan `nothing found`; `terraform fmt -check
-recursive` clean; `terraform validate` "Success! The configuration is
valid." A plan against `dev` was **not run**: the root's backend needs a
re-initialisation this session is not authorised to perform and no
credential is present, so the no-op is asserted by the null default and
the conditional arm rather than by a plan. Frontend gates were not run: no
frontend file was touched.

**Reviewed and repaired a third time, 2026-09-12**, no verdict changed.
The second round's repairs (`8a19737..2c954cd`) were re-reviewed by a
fresh security-engineer pass and a fresh code-reviewer pass; every finding
is fixed in the commits that follow `2c954cd`, ADR 0057 carries a third
amendment, and §22.3, §22.4 and §56.3 above are corrected in place. The
findings and their resolutions, by the reviews' labels:

- **S-F1 (high)** — `verify_retained_chain` re-anchored only the head, so
  any log with an *interior* eviction failed it and `Platform::new` refused
  to restart over its own honest log naming tampering. A link is now held
  only between consecutive sequences, a gap re-anchors on the claimed
  predecessor as the head does, `open_with_capacity` refuses a
  non-contiguous file so a removed line never reads as an eviction, and
  the refusal states the chain is unkeyed SHA-256 (ADR 0043)
  (`the_retained_chain_verifies_across_an_interior_eviction_and_still_names_an_edited_record`,
  `the_ledger_rebuilds_from_a_log_that_evicted_an_interior_record`).
- **S-F2 (medium)** — provenance inferred from an attached admission,
  unpinned. Now `DataAdapter::provenance`, `Replayed` for `ReplayAdapter`
  and `TapeFeed`
  (`a_replay_backed_engine_reports_replayed_provenance_with_and_without_an_admission`).
- **S-F3 (medium)** — any plaintext `http://` host accepted in-process.
  `require_loopback_egress` at `ConnectorFeed::open` and both brains'
  parsers (`a_base_url_off_loopback_is_refused_before_a_socket_is_opened`,
  `the_connector_pair_is_held_to_the_loopback_egress_proxy`, a "loopback"
  row in the deep brain's pair test). No Terraform file changed; its
  validation already carried the rule.
- **C-1** — multi-arm `sense` accumulated and observed at the end. Each
  source is observed as soon as its own poll succeeds
  (`an_arm_that_refuses_does_not_lose_the_records_an_earlier_arm_delivered`).
- **C-2** — the cited replay test looped over an empty history. It asserts
  the premise and asks `sources_backing` directly with two replayed
  vendors; ADR §7, §22.4 and §56.3 name the tests that held the door
  meanwhile.
- **C-3 / S-F5** — `record` then `commit` double-billed a re-fetch and
  could leave the checkpoint ahead. `StreamJournal::record_and_commit`:
  ledger, then checkpoint, adopted only on success
  (`a_journal_write_that_fails_after_the_poll_bills_once_and_never_leaves_the_checkpoint_ahead`).
- **S-F4** — `EventLog::open_with_capacity` takes an exclusive lock
  (`a_log_another_handle_holds_is_refused_until_the_handle_is_released`);
  the campaign id's text says what remains uncovered. The declared MSRV is
  1.89 for `File::try_lock`; the pinned toolchain is unchanged at 1.94.1.
- **S-F6** — sketch rows must sum to the total; **S-F7** — `replay://` and
  `ReplayedAdmitted` together or not at all, hash sixty-four lowercase hex.
- **Nits** — `CampaignSummary.journaled` dropped; `ConnectorArm::shutdown`
  called on node exit; the root's `configuration:` prefix keeps the error
  class; `RevisionRecord::build`'s comment reworded; the replay header
  keyed on the first non-blank, non-comment line; `sources_backing`'s
  `Discovered` arm documented as having no live gate.

Commits, in order: `fe200c1` (S-F1), `60875cd` (S-F3), `1c23e70`
(C-3/S-F5), `529f7a4` (S-F2, C-1, C-2, the deep-brain nits), `a7c03ff`
(the MSRV floor and the mechanical edits it woke, on their own —
`git show a7c03ff --stat` lists seventeen files, `Cargo.toml` and sixteen
source files, and the diff holds twenty-four hunks in those sixteen: eight
`is_multiple_of` rewrites, one of them by hand in `qip-core/src/hash.rs`,
and sixteen `let`-chain rewrites; the commit message's "twenty-four lints"
and "seventeen sites" counted sites, and the triple nest in
`manifest_wiring.rs` is one hunk holding two — corrected in the fourth
round), `84bca47` (S-F4, S-F6, S-F7, the replay-header and comment nits),
and this entry's commit. Fifteen mutations across them, each recorded
in the commit that introduced or changed the test it fires. `git diff
--name-only 2c954cd..HEAD` names no file under `qip-risk-engine`,
`qip-execution-engine`, `qip-capital`, `qip-edge`, `qip-brokers`,
`qip-routing`, `qip-compliance` or `infrastructure/`. No dependency edge
was added.

Gate, run on the tree as committed: `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets` zero warnings; `cargo test
--workspace --no-fail-fast` exit 0, **4956 passed, 0 failed**, summed
across 382 `test result:` lines; `cargo test -p qip-acceptance
--no-fail-fast` 23 `test result:` lines, **348 passed, 0 failed**;
dependency policy `11 third-party package(s), all permitted`; secret scan
`nothing found`. Terraform gates were not run: no Terraform file was
touched. Frontend gates were not run: no frontend file was touched.

**Re-scored 2026-09-13**, §12.3 in place and §12.4 corrected, no verdict
changed — Lane B-1 of §12.3 (ADR 0061), the three rule rows the corrected
count exposed. Five commits in dependency order: `79f73f1` (a refusal is
charged to the rule by the breach the checker wrote, never by a word in
the sentence; `qip_rule_fired_total{rule}`), `39c25c2` (R2 and R3 as
records — a defence with the simulated loss avoided, a dormancy finding at
one hundred cycles across one hundred accepted orders, three topics on the
backbone, `Platform::limits()`), `a59665f` (R1 as a proposal whose one
constructor refuses a non-loosening bound, withdrawn when the evidence
evaporates, resumed from the log; two signatures cloned from the promotion
approval emit an artefact and the running set never moves; two routes),
`3f4a093` (the one door: `QIP_RISK_LIMITS_PATH` on all three central roots
from `risk_limits_file`, the fast brain's first optional-file map, a file
that may move a bound and never remove a control, the risk page reading
`Platform::limits()`, and two acceptance scans — a `&mut self` method
naming a limit on any type that holds the set, and `conservative_default()`
from shipped code outside a reviewed list), and this entry's commit.

Twenty-four new tests, each mutation-verified — the implementation broken,
the test confirmed to fail for the stated reason, the code restored
byte-for-byte with `cmp`, the test reconfirmed passing — **twenty-one**
mutations named across the four commit messages (4 + 4 + 8 + 5; corrected
2026-09-13 after a code review recounted and found this line said
twenty-two), one of which (`from_document` skipping `validate`) was run
with `--no-fail-fast` so all three roots' tests were seen firing. The same
review found one of the twenty-four, `a_viewer_reads_the_open_proposals_and_the_running_set_by_name`,
named with the others but not itself given a stated mutation. Its mutation
is the list route's `required_role` changed from `Viewer` to `Operator`:
run, it failed the role assertion (`left: Operator, right: Viewer`) rather
than the response-shape assertions after it, confirming the test does pin
the role table and not merely the body it happens to read; restored,
`cargo test -p qip-api --test recalibrations` passed 3 of 3 again.
Among them, the ones that hold the decision: the loosening check removed
(a 200,000 ceiling on a 250,000 limit became a proposal); the artefact
assigned into the monitor on enactment (`platform.limits()` read the
proposed bound); the proposed bound applied into the checker at proposal
time (the order the artefact would admit was accepted); a
`set_limits(&mut self, …)` stub on `Platform` (named by the scan); the
coverage loop dropped from `validate` (a file without `expected-shortfall`
was admitted); the risk-limits arm removed from the fastbrain map ("the
catalogue mounts no limits file on fastbrain").

One existing test adapted, in `a59665f` and said so there: `qip-api`'s
scrape-surface test counted one assembly series and now counts two,
because every limit reads `qip_rule_dormant{rule}=0` from assembly so a
rule that never fires is a zero and not an absent series. One departure
from the design: JSON cannot spell NaN, so the roots' "NaN bound" case is
the parser refusing the literal and a negative bound is the validator's
case beside it.

The paper-trading boundary is untouched, confirmed by
`git diff --name-only 0093349..HEAD`, which names no file under
`qip-edge`, `qip-brokers`, `qip-routing`, `qip-capital` or
`qip-risk-engine`; no autonomy code changed; the OMS change adds a field to
a record and moves no gate; `qip-risk` gains pure constructors and a
validator and no arm of the checker changed.

Gate, run on the tree at `3f4a093`: `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets` zero warnings; `cargo test
--workspace --no-fail-fast` exit 0, **5028 passed, 0 failed** over 387
`test result:` lines (run with `CARGO_PROFILE_DEV_DEBUG=0` after the first
attempt filled the disk at 28 GiB of build output and was cleaned with
`cargo clean`; the override changes binary size and nothing the tests
measure); the run before it showed one failure,
`qip-api/tests/openobserve.rs::a_post_failure_is_counted_and_logged_and_the_process_survives_it`,
an ephemeral-port race — bind `127.0.0.1:0`, drop, assert a connect fails
while ~380 other binaries bind loopback ports — which passed three times
in three on its own, is touched by nothing in this lane, and is not
modified; `cargo test -p qip-acceptance --no-fail-fast` **351 passed, 0
failed**; dependency policy `11 third-party package(s), all permitted`
(`serde_json` joins `qip-risk`, one of the two permitted crates); secret
scan `nothing found`; `terraform fmt -check -recursive` clean on the root
and on `environments`; `terraform validate` "The configuration is valid".
Frontend gates were not run: no frontend file was touched. The independent
`code-reviewer` and `security-engineer` passes were **not run**: this
session had no facility to launch a reviewing agent, and the review in its
delivery report is the implementer's own.

**Reviewed and repaired a ninth time, 2026-09-13**, no verdict changed.
Rounds seven, eight and nine against the same function, and the shape of
the chain changed: **the redaction itself is now closed, and the last two
rounds' defects were not in it.** A security review of round eight could
not break the function by execution — zero panics in 544,161 inputs,
zero survivors in 1,021,410 inputs carrying a sentinel before a
credential's terminating `@`, an injective escape, all 37 adversarial
rows passing. What broke instead was the prose around it and the tests'
own claims about themselves, and rounds eight and nine were both spent
there. ADR 0057 carries a ninth amendment with the reasoning. §22.3,
§22.4 and §56.3 above each carry a dated entry for this round; none
changes a verdict, and none of these findings touches a statistic, a
promotion rule, a data reference or the concentration count. The
findings and their resolutions:

- **Blocking (round seven, `d231679`)** — round six cut the parameter
  region off first and then searched only the surviving prefix for the
  credential's terminating `@`. `?` and `#` are ordinary password
  characters and are *not* legal unencoded in userinfo, which is exactly
  why a password containing one arrives at this function instead of
  parsing, so the cut lands inside the credential, the search finds no
  `@`, the code concludes there is no userinfo, and the first half of
  the password prints. Reproduced by execution:
  `http://svc:SECRET?x@127.0.0.1:9105` came back as
  `http://svc:SECRET?…`, and 131,040 of 640,000 enumerated inputs leaked
  the same way, reaching stderr at start-up through two arms of the
  gate. Both boundaries are measured over the whole remainder and only
  then intersected now; where the last `@` falls at or past the cut,
  nothing between the scheme and the cut is shown, because "the `@` is
  in the query so what precedes it is a host" and "the `?` is in the
  password so what precedes it is a credential" are the same string and
  the safe reading wins. Four rows that used to keep a host now lose it.
- **The mechanism, named in round seven and acted on rather than
  deplored** — six rounds share one, and it is not that anyone reasoned
  badly. Each round's proof was correct about the code in front of it,
  was written into a comment, and was carried forward verbatim across a
  restructure that invalidated it: round five proved no `@`-delimited
  credential survives, round six moved the search inside a boundary it
  had not previously had, and the proof stayed true of code that no
  longer existed. **Prose does not get re-derived.** So the guarantee is
  executed now —
  `no_byte_before_a_credentials_terminating_at_ever_survives_redaction`
  sweeps a six-symbol alphabet, plants a sentinel anywhere a credential
  could sit, and requires it never reach the output. It fails on round
  six's ordering with 5100 of 53354 inputs.
- **Blocking (round eight, `88ca127`)** — round seven changed three
  things and tested one, and both untested ones were defective. Making
  the `https://` check case-insensitive, it wrote `base_url.len() >= 8
  && base_url[..8]`: a *byte*-length guard in front of a *byte*-index
  slice. Byte 8 need not be a character boundary, so any address with a
  multi-byte character there panicked, and Rust's slice-boundary panic
  prints the offending string — the credential on stderr by a path that
  never touches the redaction, six rounds of work defeated in one line.
  The nastiest route in is the function's own output: it emits `…` at
  offset 7, so an operator copying a redacted address out of a refusal
  and pasting it back aborted the process and printed what the refusal
  had masked. Fixed with a non-panicking `get(..8)`, pinned by four
  inputs each asserting its own premise that byte 8 is not a character
  boundary.
- **Also round eight** — the gate names the parsed host beside the
  redacted address, and the parser refuses *userinfo*, not a secret
  sitting where a host goes: `http://hf_SECRET?x@127.0.0.1:9105` has no
  `@` before its `?`, so it parses with the credential as its **host**,
  and naming it re-printed exactly what the redaction had masked. The
  host is named only where the redaction kept it. Three smaller items
  from the same reviews: the parameter marker is a constant where the
  delimiter that set the cut is itself inside the credential, since
  printing the real byte leaks one character of the password and which
  of `?` or `#` it was; the escape whitelist no longer exempts `…`, the
  marker being spliced in after escaping, so an operator-typed ellipsis
  cannot render as though it had been redacted; and the sweep's premise
  became an exact count rather than a floor, because `checked > 1_000`
  stays true through a change that drops coverage by 98% — a number that
  drifts without ever becoming false, the failure mode
  `.claude/rules/domains/observability.md` names by hand.
- **Round nine (`a190352`), and its finding is about this chain's own
  subject** — round eight's commit message said it had removed the
  discredited sentence "a parsed host cannot carry a credential". It had
  not: it appended the correction underneath and left the original
  standing five lines above it, in the function it was about. Two
  independent reviews found it the same way and both named it the most
  likely source of the next leak, because whoever edits that arm reads
  the first sentence they reach, concludes the conditional is redundant,
  and reopens what round eight closed. Round nine deletes it, and
  deletes the paragraph above it, which described behaviour that no
  longer existed — that the host and the address are both printed
  because they can legitimately disagree, now impossible, a kept host
  always being a substring of the rendered address.
- **A false justification, counted rather than assumed (round nine)** —
  the argument for the whole host-withholding trade-off was "every
  caller names its configuration variable, so nothing is lost". It is
  true of five of six call sites. `qip-market-ingestion`'s
  `ConnectorFeed::open` calls the gate with a bare `?`, so on that one
  path a refusal can now name **neither** the host nor the variable.
  That is a real cost of round eight's fix; it is recorded at the
  decision in the code and here rather than left to be discovered by the
  operator it happens to, and the fix is a wrapper at that call site
  rather than a relaxation of the gate.
- **Two test defects (round nine), both of the class this document
  exists to catch** — round eight's companion assertion, that an address
  needing no redaction still names its host, was vacuous: such an
  address is rendered verbatim and so already contains the host, and
  deleting `{host}` from the format string left the assertion green. It
  is anchored on a backtick, which *that row's* rendered address does not
  carry — a claim about the row and not the renderer, since a backtick is
  `is_ascii_graphic` and passes the escape whitelist. The sentence was
  first written as a claim about the renderer and shipped here by the
  same commit that retracted it in the code. The mutation it describes is
  observable only with the withheld-host assertion removed, because that
  one runs earlier and both mutations break it. And the very case the round existed to close — a
  secret parsed into host position — appeared only in comments rather
  than being driven, a mutation report naming a case the test did not
  contain. The case was written rather than the claim softened.
- **Known-open, and recorded here because a commit message said it
  already was.** `88ca127`'s message states that a flaky test "is
  recorded as known-open with its repro". It was not: the repro lived in
  a working note outside this repository, and a review caught the claim.
  This bullet is that record. `qip-cli`'s `replay` suite,
  `a_journal_a_running_node_holds_is_refused_naming_the_holder_not_as_a_corrupt_file`,
  fails intermittently: the `holder` binding's `EventLog::open` —
  `replay.rs:464` when this was measured, and located by symbol rather
  than trusted by line, `grep -n 'fn a_journal_a_running_node_holds_is_refused'
  backend/crates/apps/qip-cli/tests/replay.rs` — returns `denied`, "held
  by another process (or another handle in this one)", on a journal the
  preceding `write_journal` has already dropped. Measured, not estimated: running
  the whole `replay` binary twelve times fails 4 times on committed HEAD
  and 4 times with the redaction changes applied; the test alone fails 0
  times in 20; `--test-threads=1` fails 0 times in 12. So it is a
  parallel-execution interaction inside that binary, and it predates the
  redaction work rather than being caused by it. The cause is
  established with a control rather than hypothesised: a standalone
  probe that takes the lock, drops it and immediately re-acquires, 4000
  times, while a second thread does nothing but
  `Command::new("/bin/true").output()` in a loop, is refused
  `WouldBlock` on **514 of 4000** acquires despite the previous holder
  being dropped; the identical loop with the spawner replaced by
  `thread::yield_now()` is refused **0 of 4000**.
  `std::process::Command` forks, the child inherits the open file
  description and with it the advisory `flock` `EventLog::open` takes
  (`log.rs:273` when this was measured; `grep -n 'try_lock'
  backend/crates/libs/qip-events/src/log.rs` names it and the shared
  `try_lock_shared` that `EventLog::inspect` takes beside it, whatever
  the file does next), and for the window between `fork` and
  `exec` the parent's drop does not release it; `O_CLOEXEC` closes that
  window at `exec`, which is why the failure is intermittent rather than
  constant. It is worth recording because it reaches past the test: any
  process here that spawns a subprocess while holding the event-log lock
  can transiently block another opener. In a deployment that is benign —
  a node holds its log for its whole lifetime — but a tool that opens
  the log, forks, and expects a second opener to succeed would meet the
  same refusal. Three fix candidates, **none applied**: serialise the
  suite, retry the open briefly with the mechanism named in a comment,
  or take the lock through an open that cannot be inherited. The test is
  not skipped, not `#[ignore]`d and not weakened. It is filed here
  rather than in an ADR or an operations runbook because it is a
  measured fact about the tree with a repro, which is what this document
  is for, and because the lock it concerns is the subject of ADR 0057's
  fourth amendment, whose regression test is the one that flakes — the
  ADR carries the decision, this carries what the tree does today.

Commits, in order: `d231679` (round seven), `88ca127` (round eight),
`a190352` (round nine), `3af8854` (round nine's own corrections) and
`e451ff1`, which carries this entry. None has been pushed. This sentence
called `a190352` "the head as this entry is written" and it was not, two
commits later — the same error this entry corrects in the eighth. Each of the
code commits touches `qip-transport/src/http.rs` and its own
`tests/http_client.rs` and nothing else; `git diff --name-only
e691804..e451ff1` names those two files and the two documents that
record the rounds, and no dependency edge was added. The paper-trading
boundary is intact at all three layers and was not reached by any of
these changes: no Terraform file, no `AutonomyLevel::deployable` call
site and no `qip-edge` constructor was touched.

Gate — **quoted from the implementer's commit messages, not re-run for
this entry**, which changes Markdown only and was written in a session
told not to run cargo. The figures below were first recorded against
`a190352` and re-run unchanged at `e451ff1`, the commit this entry ships
with; attributing them to `a190352` alone would have credited a tree two
code commits behind the one being pushed. `e451ff1` records: `cargo fmt --all --check`
clean; `cargo clippy --workspace --all-targets` zero warnings; `cargo
test --workspace --no-fail-fast` exited 0 with no FAILED line, the
transport suite reporting `test result: ok. 32 passed; 0 failed; 0
ignored; 0 measured; 0 filtered out` and the totals across 382 result
lines being **4975 passed, 0 failed**; `./scripts/check-dependencies.sh`
`all permitted`, 11 third-party packages, unchanged;
`./scripts/check-secrets.sh` `nothing found`. The flake recorded above
passed that particular run (`test result: ok. 5 passed; 0 failed`),
which is **not** evidence that it is fixed — it is intermittent at
roughly one run in three and nothing in these commits touches it.
Mutations are recorded in the commit that introduced or changed each
test they fire on, restored byte-for-byte; the two matrix figures were
re-measured against round seven's implementation (`13 of 37` for the
deleted region cut, `8 of 37` plus `5100 of 53354` on the sweep for
round six's ordering) and re-run unchanged at round nine, and one
previously reported mutation was withdrawn in round eight rather than
carried forward, because it fired on an assertion that asserted the
wrong thing. Terraform gates were not run: no Terraform file was
touched. Frontend gates were not run: no frontend file was touched.

**Reviewed and repaired an eighth time, 2026-09-13**, no verdict changed.
Three further rounds against the one function that renders a refused
egress address printable, and three further credential leaks found in
it: rounds four, five and six of a chain this document has now recorded
six times. Rounds four and five each closed the shape that had been
reported and left an adjacent one standing, which is the pattern the
seventh entry below said it had broken and had not. Round five stopped
repairing shapes and deleted the judgement that kept producing them.
Round six narrowed the claim the code makes about itself to what is
actually proven and closed the gap that claim had been covering. ADR
0057 carries an eighth amendment with the reasoning, and the seventh
amendment's prediction that this would not recur is kept there in the
past tense rather than deleted. §22.3, §22.4 and §56.3 above each carry
a dated entry for this round; none changes verdict, and none of these
findings touches a statistic, a promotion rule, a data reference or the
concentration count. The findings and their resolutions:

- **Blocking (round four, `e26c8cf`)** — with no scheme found, the
  function read everything up to the first `/`, `?` or `#` as the
  candidate authority and took "no `@` in it" for "no credential". When
  the string's first character is already one of those delimiters — a
  `://` an operator typo'd down to `//`, `/`, `?` or `#` — that
  authority is empty, or for a bare `://` the single character `:`, and
  an authority that cannot be a host proves nothing about the `@` one
  character further in. Six inputs came back raw, every one reachable
  through `QIP_LANGUAGE_MODEL_BASE_URL`. Fixed by a
  `split_authority`-based `authority_could_be_a_host` check, widening
  the search past the delimiter only where the candidate could not be a
  host, with `svc/TOKEN@127.0.0.1:9106` left as a documented accepted
  residual on the argument that a bare single-label hostname is
  indistinguishable from a scheme-typo'd credential.
- **Blocking (round five, `ca3d581`)** — that check trusted far more
  than bare hostnames: any `word:validport` pair (`abc:80`) and any run
  of digits (`123`) satisfied it, and it ran identically whether or not
  a scheme was present, so `http://svc/TOKEN@127.0.0.1:9106` leaked too.
  The realistic input is worse than the synthetic ones: an operator who
  means `http://someservice:SECRET@upstream.example` and mistypes one
  character, `:` for `/`, produces an address `Url::parse` accepts,
  which then fails only `require_loopback_egress`'s later loopback-host
  check — and that refusal message, built by this function, printed the
  secret. Five rounds, five leaks, each the next hole in a heuristic
  that had just been made one input narrower.
- **The decision, recorded rather than left in a commit message** — **no
  predicate over the string's own shape can distinguish a scheme-typo'd
  credential from a legitimate bare hostname, because nothing in the
  string says which it is.** So the predicate was deleted rather than
  narrowed a fifth time: the function redacts through the last `@` past
  the scheme, unconditionally, with no remaining step whose wrong answer
  is "conclude there is no credential". Three tests that pinned the
  narrower behaviour as intentional were rewritten rather than deleted.
  The cost is accepted permanently and written down rather than
  discovered later: it over-redacts, masking a benign `@` in a path, on
  an address a caller is already refusing.
- **Reviewed and narrowed (round six, `e691804`, which this entry called
  "the current head" and which was not — round seven, `d231679`, was
  this entry's own commit's parent; corrected here and in the ninth
  entry above)** —
  two independent reviews of round five converged. Neither could break
  the userinfo guarantee and one proved it structurally: an `@` cannot
  fall inside a consumed scheme, because the scheme grammar `ALPHA *(
  ALPHA / DIGIT / "+" / "-" / "." )` contains no `@`, so "no `@` in the
  remainder" and "no `@` in the input" are the same statement —
  confirmed against three million generated inputs with no violation.
  Both then objected to the same thing, and the objection is the
  finding: the prose claimed more than the proof supports. "It can never
  under-redact" is true of *userinfo* and false of *credentials*, and a
  `?api_key=…` query — the shape a vendor console hands an operator to
  copy — carries no `@` at all and was printed in full by every arm of
  `require_loopback_egress`. The claim is narrowed to userinfo and the
  gap is closed: the parameter region, everything from the first `?` or
  `#`, is masked whole. **Ordering is load-bearing** and this round
  nearly shipped it backwards — masking the query after the `@` search
  lets an `@` inside the query end that search so everything past it
  prints, which on `?a=1@2&api_key=SECRETVALUE` prints the secret;
  cutting the region off first makes those bytes unreachable rather than
  unsearched. A credential inside a *path segment* is still printed and
  is now a stated limit with its own test row, because masking it would
  mean guessing which segment is secret, and guessing which part of a
  string is sensitive is the activity that produced five consecutive
  leaks. Control characters are escaped, so a `\r\n` in a rejected
  address cannot end the log line and begin one the operator did not
  write. The function is renamed `redact_for_echo`, both reviews having
  noted it was named for userinfo while its callers used it as "make
  this safe to print"; ADR 0057's earlier amendments are updated to the
  new name so a search finds the whole history, while the entries below
  in this document keep the former name as they were written, being
  dated records of what was true when each round closed. And
  `require_loopback_egress`'s refusals now lead with the parsed host
  rather than the redacted address, because they could contradict
  themselves — one announced that an address "names no port" beside a
  displayed `:9105`, another refused an address it displayed as exactly
  the loopback form it says is required; the parsed host is safe to
  print there because the parser refuses userinfo before those arms run.
- **A reporting defect in round five's own evidence, corrected** — the
  adversarial table now collects its mismatches and asserts once, where
  it used to `assert_eq!` per row. Round five's mutation report named
  five failing rows, and an `assert_eq!` inside a loop stops at the
  first, so four of the five were inferred and reported as read: a
  breach of this repository's evidence rule inside a report written to
  satisfy it. The corrected report quoted two runs that were made, **as
  of `e691804` and already stale when this entry was committed** —
  deleting the parameter-region cut gave `9 of 28 redaction rows are
  wrong`, reversing the ordering gave `4 of 28`, including
  `http://127.0.0.1:9105/x?a=1@2&api_key=SECRETVALUE` coming back as
  `http://…@2&api_key=SECRETVALUE`. At `a190352` the same comment quotes
  `13 of 37` and `8 of 37`, re-measured against round seven's
  implementation rather than carried across it and re-run unchanged at
  round nine; the ninth entry above says why.
- **Known-open, found while auditing this chain and not fixed in it, the
  more serious half first.** `SourceEndpoint` in `qip-data-finder`
  derives `Deserialize`, and that derive is a second constructor which
  never runs `SourceEndpoint::parse` — while `parse` is the only place a
  *source's own host* is put through `unkeyable_host_reason`, the guard
  that refuses a host no denylist rule could ever be keyed on (`grep -rn
  unkeyable_host_reason backend/crates` names three lines: the
  definition, that call, and one in `HostRules::rules` that validates
  the operator's denylist *entries* rather than any source's host). A
  deserialised endpoint skips the guard, and the path is production
  rather than a fixture: `qip-deepbrain`'s `load_source_candidates`
  reads `QIP_DEEPBRAIN_SOURCE_CANDIDATES_PATH` and hands the text to
  `serde_json::from_str` as a `Vec<SourceCandidate>`, each holding a
  `SourceEndpoint` built field by field (`grep -n
  load_source_candidates backend/crates/apps/qip-deepbrain/src/main.rs`
  — the caller is in `run`, some five hundred lines above that file's
  `#[cfg(test)] mod tests`), and the finder keys the denylist on exactly
  that host (`grep -n host_rules
  backend/crates/services/qip-data-finder/src/finder.rs`). So a
  candidate file may declare `collector.example@denied.example`, which a
  denylist entry for `denied.example` never matches — the character
  before it is `@`, not `.` — and `parse`'s own comment records that
  this has already happened once and calls it failing open in the one
  direction a denylist has. It is the same gateless-constructor class as
  this ADR's F1/S5 items, `AdmittedSource` and `FetchDigest`, and
  `SourceEndpoint` was missed when those were closed; the fix shape is
  the one established there — drop the derive, or route it through
  `parse` with `#[serde(try_from)]`, and pin the absence with the
  `compile_fail` doctest pattern `qip-data-finder`'s `admission.rs`
  already carries. What holds today is configuration, not the type:
  `source_candidates_file` is null in every environment's tfvars and
  commented out in dev, and nothing is deployed — so this is a fail-open
  control waiting on an operator's file, not an exposure now. Below it,
  and genuinely unreachable: `SourceEndpoint::parse` echoes
  caller-supplied text unredacted in five refusals — no scheme, unknown
  scheme, no host, bad port, unkeyable host — and the unknown-scheme arm
  computes its `scheme_text` with `url.split_once("://")`, the identical
  whole-string scan that was round two of this chain, so
  `svc:TOKEN@127.0.0.1:9106/x?y=http://z` makes the echoed text the
  whole credential-bearing prefix. It is the *echo* that is unreachable
  and not the type: `grep -rn SourceEndpoint::parse backend/crates`
  names 13 call sites, eight under `tests/` directories and five inside
  `#[cfg(test)] mod tests` blocks, so no production caller feeds `parse`
  a URL today and the leak becomes live the moment one is wired. The
  seventh round's audit below examined this same function and concluded
  it was "not a residual copy of this defect" because the token is
  matched against a closed eight-item scheme enum; that reasoning is
  sound about what `parse` *accepts* and silent about what it *prints*
  before the match, which is the half that leaks. When it is fixed,
  `redact_for_echo` should move into `qip-core` — pure text, no I/O, and
  `qip-data-finder` already depends on `qip-core`, so no new dependency
  and no boundary crossing — and both crates should call the one
  function. A second copy that can disagree with the first is exactly
  what produced round three.

Commits, in order: `e26c8cf` (round four), `ca3d581` (round five),
`e691804` (round six — **not** the current head, as this sentence and
one bullet above it claimed: this entry's own commit had `d231679`,
round seven, as its parent), and this entry's commit. None of the three
has been pushed. Each of the three code commits touches
`qip-transport/src/http.rs` and its own `tests/http_client.rs` and
nothing else; `git diff --name-only f679956..HEAD` names no file under
`qip-risk-engine`, `qip-execution-engine`, `qip-capital`, `qip-edge`,
`qip-brokers`, `qip-routing`, `qip-compliance` or `infrastructure/`, and
no dependency edge was added. The paper-trading boundary is intact at
all three layers and was not reached by any of these changes: no
Terraform file, no `AutonomyLevel::deployable` call site and no
`qip-edge` constructor was touched.

Gate — **quoted from the implementer's commit messages, not re-run for
this entry**, which changes Markdown only and was written in a session
told not to run cargo: all three commits record `cargo fmt --all
--check` clean; `cargo clippy --workspace --all-targets` zero warnings;
`cargo test --workspace --no-fail-fast` `test result: ok. 4973 passed; 0
failed` summed across 382 result lines with no FAILED or error line in
the log; `./scripts/check-dependencies.sh` `all permitted`, 11
third-party packages, unchanged; `./scripts/check-secrets.sh` `nothing
found`. The workspace total is the seventh round's 4973 in all three,
because these rounds add rows to existing table-driven tests and rewrite
three others rather than adding test functions — the matrix went from
fourteen rows to twenty-eight at `e691804` without the count moving,
which is worth
saying out loud, since an unchanged total normally means an unchanged
suite and here it does not. Mutations are recorded in the commit that
introduced or changed the test each fires, restored byte-for-byte.
Terraform gates were not run: no Terraform file was touched. Frontend
gates were not run: no frontend file was touched.

**Reviewed and repaired a seventh time, 2026-09-13**, no verdict changed.
This is the **third** consecutive round in which a "complete" redaction
fix left a real credential leak: round one (`717cd81` and earlier) required
`"://"` to find userinfo at all; round two (`06d2718`, inside `41f25ad`)
fixed that but located the scheme with `raw.split_once("://")`, which
finds the *first* occurrence of `"://"` anywhere in the string rather than
one anchored at its start. A fresh adversarial security review of round
two's own fix found the credential leak that made a third round necessary.
Say plainly what makes this round different rather than assert it: the
first two rounds each repaired the one input reproduced against them and
left the underlying method — search for a marker, trust what precedes it —
in place; this round replaces that method with a check anchored to RFC
3986's grammar, which the marker-search approach can never accidentally
satisfy on a future adversarial input the way "handle one more special
case" cannot rule out. Neither §22.3, §22.4 nor §56.3 above is otherwise
affected: none of the three findings touches a statistic, a promotion
rule, a data reference or the concentration count.

- **Blocking** — `redact_userinfo("svc:TOKEN@127.0.0.1:9106/callback?redirect=http://evil.example/x")`
  returned the input **unchanged, `TOKEN` in the clear**. The query's
  `"://"` (an ordinary shape — any `?redirect=`, `?callback=` or
  `?fallback=` parameter naming another URL, not a contrived one) is the
  *first* `"://"` in the whole string when no scheme is present, so
  `split_once("://")` matched there instead of finding no scheme, and
  everything up to that match — credential included — was misread as "the
  scheme", leaving the real `@` inside a segment the authority search
  never looked at. A second, structurally separate finding rode the
  identical defect: `HttpError::UnsupportedScheme { scheme }` stores and
  prints whatever `Url::parse` computed as the scheme with no call to
  `redact_userinfo` at all — a value that can only be a clean scheme token
  needs none — and the same adversarial input made `Url::parse` compute
  the whole credential-bearing prefix as "the scheme"; `Url::parse` on it
  returned
  `UnsupportedScheme { scheme: "svc:token@127.0.0.1:9106/callback?redirect=http" }`,
  printed outright. Fixing `redact_userinfo` alone, as round two did for
  round one's finding, would have left this arm printing the identical
  credential by a different path. Both are fixed by one function,
  `split_scheme`, that both `Url::parse` and `redact_userinfo` now call:
  it checks whether `raw` *starts with* a token matching the RFC 3986
  scheme grammar (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`) immediately
  followed by `"://"`, rather than searching the string for that marker —
  a scheme can only be a URI's literal prefix, never something scanned for
  in the tail. Because `UnsupportedScheme.scheme` can now only ever be
  produced by `split_scheme`'s bounded scan, it structurally cannot hold
  `@`, `:` or `/`, reinforced with a `debug_assert` at its one construction
  site (`qip-transport/src/http.rs`). Fourteen cases across
  `redact_userinfo` and `Url::parse`/`UnsupportedScheme`, table-driven
  where the property repeats
  (`redact_userinfo_handles_the_full_adversarial_matrix`,
  `a_scheme_less_credential_with_a_later_marker_falls_through_to_invalid_url_not_unsupported_scheme`,
  `a_genuinely_unsupported_scheme_is_still_reported_with_a_clean_token`,
  `unsupported_scheme_never_carries_unbounded_content`,
  `qip-transport/tests/http_client.rs`). Mutation-verified the two that
  matter most: reverting `redact_userinfo` to round two's
  `split_once("://")` shape reproduces `TOKEN` unredacted on the exact
  input above; reverting `Url::parse`'s detection (with the new
  `debug_assert` also removed, since it would otherwise catch a bare
  regression before the test's own assertions ran) reproduces
  `UnsupportedScheme { scheme: "svc:token@127.0.0.1:9106/callback?redirect=http" }`
  byte-for-byte. Both restored and re-verified passing. Audited every
  `split_once("://")`, `redact_userinfo` and `UnsupportedScheme` reference
  in the workspace: the only other `split_once("://")` is
  `qip-data-finder`'s `SourceEndpoint::parse`, not a residual copy of this
  defect because it matches the resulting token against a closed
  eight-item scheme enum rather than accepting any prefix, so a hijacked
  later `"://"` can only be silently accepted if the entire string up to
  that point is already exactly one of those eight tokens — which requires
  the string to start with that scheme, i.e. to be the legitimate leading
  occurrence `split_once` would have found anyway.
- **Should-fix** — `let _ = feed.shutdown(at);` at all four
  connector-release sites (`qip-deepbrain::connectors`'s two constructors,
  `qip-fastbrain::feed`'s and `qip-api::feed`'s) discarded a real shutdown
  failure rather than folding it into the returned error. Fixed with
  `qip_core::error::Error::and_release`, generalising the
  `with_release`/`fold_releases` pattern this document already described
  out of `qip-deepbrain::main` and into `qip-core` so the three other call
  sites, which had no such machinery of their own, share one
  implementation.
- **Should-fix** — `relabel`'s doc comment in `qip-deepbrain::main` claimed
  "two callers" when `fold_releases` had been a third since it was added.
  Corrected.
- **Should-fix** — the scheme-less-path-`@` permutation flagged as
  untested by the round-six review is **not** subsumed by the existing
  scheme-plus-query-`@` case: which branch of `redact_userinfo` runs
  depends on whether a scheme was found at all, so it is now its own row
  in the test matrix rather than an unverified claim.

Commits, in order: `9c2d651` (the anchored scheme-detection fix and its
test matrix), `88e23f0` (the four connector-release folds, the doc-comment
correction, the added test row), and this entry's commit. Four mutations
across the two code commits, each recorded in the commit that introduced
the test it fires. `git diff --name-only 717cd81..HEAD` names no file
under `qip-risk-engine`, `qip-execution-engine`, `qip-capital`, `qip-edge`,
`qip-brokers`, `qip-routing`, `qip-compliance` or `infrastructure/`. No
dependency edge was added.

Gate, run on the tree as committed: `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets` zero warnings; `cargo test
--workspace --no-fail-fast` exit 0, **4973 passed, 0 failed**, summed
across 382 `test result:` lines (seven more than the sixth round's 4966:
four new tests in `qip-transport`, three new tests in `qip-core`); `cargo
test -p qip-acceptance --no-fail-fast` 23 `test result:` lines, **348
passed, 0 failed**, unchanged because nothing in this round touches
acceptance-suite territory; `cargo test -p qip-transport --no-fail-fast`
7 `test result:` lines, **85 passed, 0 failed** (four more than the sixth
round's 81); dependency policy `11 third-party package(s), all permitted`;
secret scan `nothing found`. Terraform gates were not run: no Terraform
file was touched. Frontend gates were not run: no frontend file was
touched.

**Reviewed and repaired a sixth time, 2026-09-12/13**, no verdict changed.
The fifth round's repairs (`4cb4983..717cd81`) were pushed to origin
**before** review — the working agreement this round was given says so
plainly — and a fresh security-engineer then found one blocking defect
in them, plus two should-fix items from a fresh code-reviewer pass.
Every one is fixed in the commits that follow `717cd81`, ADR 0057
carries a sixth amendment, and this row and §22.3/§22.4 above are
unaffected: none of the three findings touches a statistic, a
promotion rule, a data reference or the concentration count. The
findings and their resolutions:

- **Blocking** — the fifth round's own redaction fix
  (`9189f95`/`b13e52a`, credited above) was itself incomplete.
  `redact_userinfo` required `"://"` before it would mask anything, so
  a base URL with the scheme dropped by mistake —
  `svc:TOKEN@127.0.0.1:9106`, the `http://` missing — came back
  unchanged from both `require_loopback_egress`'s own `shown` and
  `Url::parse`'s `invalid` closure (which calls `redact_userinfo` a
  second time on the same raw string once the scheme-less parse
  fails), printing `TOKEN` in the clear, twice, into the fatal
  start-up error every one of the six egress call sites wraps. Fixed
  by no longer requiring a scheme to find the candidate authority
  (`qip-transport/src/http.rs`); the existing
  `redact_userinfo("no scheme@here")` assertion, which pinned the old
  wrong behaviour, now asserts the corrected redaction, and
  `a_scheme_less_credential_bearing_egress_address_is_still_redacted`
  drives the exact scenario above through `require_loopback_egress`
  and checks both `Display` and `Debug` of the resulting error
  (`qip-transport/tests/http_client.rs`).
- **Should-fix** — `qip-deepbrain`'s `with_release` composed a double
  release as `arm.shutdown(...).and(evolution.shutdown_connectors(...))`;
  both releases ran, but `.and()` reports only the first `Err`,
  dropping the second failure's text. `fold_releases` now folds both
  messages under the first failure's class, mirroring the existing
  `relabel` construction
  (`a_double_release_failure_names_both_failures`).
- **Should-fix** — the admission-check failure arm inside
  `ConnectorArm::open` (`qip-deepbrain`), `Feed::admitted_connector`
  (`qip-fastbrain`) and `ApiFeed::connector_admitted_by_registered`
  (`qip-api`) all left a feed whose transport was already open to be
  released only by its plain `Drop`, never by
  `ConnectorFeed::shutdown` — contradicting the fifth round's own claim
  that every exit past these constructors released its connectors,
  which was true only of the two exits inside each root's `main.rs`.
  Inert today, as before (every shipped `SourceConnector::shutdown` is
  a no-op default), and the fix is small, so all three constructors —
  plus `ConnectorArm::over_transport_admitted_by`, the fourth site
  sharing the shape — now call `shutdown()` on that one arm before
  returning the admission error. Only `over_transport_admitted_by` can
  be driven without a real socket, and it is what
  `an_admission_refusal_after_the_feed_opens_still_releases_it` proves,
  through a manifest with no §7.6.1 category and a spy connector
  recording whether `shutdown` ran; the other three are proven by code
  inspection and by the unchanged clippy and fmt gates, which this
  entry states rather than leaves implied.

Commits, in order: `06d2718` (the blocking redaction fix), `b241d2e`
(the release fold), `3dbb9d1` (the connector-release fix, all four
sites), and this entry's commit. Three mutations across them, each
recorded in the commit that introduced the test it fires: reverting
`redact_userinfo` to require a scheme; reverting `fold_releases` to
`first.and(second)`; reverting `over_transport_admitted_by`'s `match`
to plain `?`-propagation. `git diff --name-only 4cb4983..HEAD` names no
file under `qip-risk-engine`, `qip-execution-engine`, `qip-capital`,
`qip-edge`, `qip-brokers`, `qip-routing`, `qip-compliance` or
`infrastructure/`. No dependency edge was added.

Gate, run on the tree as committed: `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets` zero warnings; `cargo test
--workspace --no-fail-fast` exit 0, **4966 passed, 0 failed**, summed
across 382 `test result:` lines (three more than the fifth round's
4963, one per new test); `cargo test -p qip-acceptance --no-fail-fast`
23 `test result:` lines, **348 passed, 0 failed**, unchanged from the
fifth round because nothing in this round touches acceptance-suite
territory; `cargo test -p qip-transport --no-fail-fast` (the redaction
fix's own crate) 7 `test result:` lines, **81 passed, 0 failed**;
dependency policy `11 third-party package(s), all permitted`; secret
scan `nothing found`. Terraform gates were not run: no Terraform file
was touched. Frontend gates were not run: no frontend file was
touched.

On the evidence-rule item the working agreement raised: the fifth
round's own gate paragraph immediately below quotes a real `cargo test
--workspace --no-fail-fast` run at `2ba1ee0` with its numbers, so
"the tests this row cites are unchanged and were re-run green in the
fifth round's workspace gate" is backed by that quoted run rather than
by an unrun claim — a full `--workspace` run re-executes every test
this row cites along with everything else, so the sentence does not
assert more than the quoted command shows. No change to that sentence
was needed; this paragraph records that it was checked rather than
leaving the check unstated.

**Reviewed and repaired a fifth time, 2026-09-12**, no verdict changed.
The fourth round's repairs (`672563f..4cb4983`) were re-reviewed by a
fresh security-engineer pass and a fresh code-reviewer pass: no blocking
or medium finding, eight low and should-fix items; every one is fixed in
the commits that follow `4cb4983`, ADR 0057 carries a fifth amendment,
and §22.3, §22.4 and §56.3 above are corrected in place. The findings
and their resolutions:

- **Low** — `QIP_LANGUAGE_MODEL_BASE_URL`, the one address carrying a
  bearer token, was gated by two string prefixes admitting `localhost`
  and `http://127.0.0.1:9106@evil.example/`. Through
  `qip_transport::http::require_loopback_egress` now, `localhost` moved
  from admitted to refused, userinfo and `[::1]` rows added
  (`a_base_url_that_is_not_loopback_is_refused_and_the_refusal_names_the_proxy`).
- **Low** — `QIP_MARKET_DATA_BASE_URL` refused `https` alone. Through
  the same gate; the fixture's cluster-DNS address is the new test's
  first refused row (`a_vendor_address_off_loopback_is_refused_by_name`).
- **Low** — every refusal echoed the URL, userinfo included.
  `HttpError::InvalidUrl` stores the address with its userinfo replaced
  by `…@`, and the gate's echoes go through the same `redact_userinfo`;
  the gate itself moved beside the parser, `qip-market-ingestion`'s copy
  is gone, and it requires the explicit port Terraform always did
  (`a_url_that_carries_a_credential_is_refused`,
  `an_egress_address_is_loopback_with_a_port_and_a_refusal_never_echoes_a_credential`;
  the deep brain's `configuration()` wrapper was checked for a double
  prefix and has none — the parser refuses first, once). ADR 0057 now
  names `QIP_OPENOBSERVE_URL` as the one in-process URL outside the gate,
  by ADR 0032's decision, rather than saying "every".
- **Low** — two deep-brain exits skipped `shutdown_connectors`: a failed
  flush, and the open loop's failures past arms already opened. One
  `with_release` on all three exits; the leak today is process-local
  (every shipped `shutdown` is the trait's no-op), so the fix is the
  invariant; no test drives `run`, none claimed.
- **Low** — the billing test's mutation note described a mutation its
  own store refuses. Re-run with the two-key-shaped write
  (`{ ledger, checkpoint: None }` first, at `record_and_commit`):
  `durable.polls` reads `2` against `1` and the test fails there; the
  note, the "polls == 2" sentence and the "second table" wording are
  corrected. The first attempt landed in `open`'s same-shaped session
  write and did not fire; the note says so.
- **Low** — an inspected log accepted `append` silently. An `inspected`
  marker; `append` refuses with `denied` naming `EventLog::inspect` and
  `EventLog::open`; the test rewritten from "reaches memory only" to
  "refused, reaches neither" (mutation: `false` for the check made the
  append succeed, "an inspected log accepted an append: 4").
- **Nit** — `MetronomeBuyer::new`: `debug_assert!` → `assert!`.
- **For the record** — the "run as an unprivileged user" sentence in the
  fourth-round block below is replaced with what the checkout shows.

Commits, in order: `9189f95` (the gate beside the parser, both URL
gates, the redaction), `75cb580` (the deep brain's exits), `843da1d`
(the billing note and the assertion), `2ba1ee0` (the inspected log's
refusal), and this entry's commit. Twelve mutations across them, each
recorded in the commit and in the test it fires or, where it does not
fire, in the test with the structural reason (the host arm cannot see
userinfo; the wrong-function landing). `git diff --name-only
4cb4983..HEAD` names no file under `qip-risk-engine`,
`qip-execution-engine`, `qip-capital`, `qip-edge`, `qip-brokers`,
`qip-routing`, `qip-compliance` or `infrastructure/`. No dependency edge
was added: the three roots and `qip-market-ingestion` already depended
on `qip-transport`.

Gate, run on the tree at `2ba1ee0` (this entry's commit changes only
Markdown): `cargo fmt --all --check` clean; `cargo clippy --workspace
--all-targets` zero warnings; `cargo test --workspace --no-fail-fast`
exit 0, **4963 passed, 0 failed**, summed across 382 `test result:`
lines; `cargo test -p qip-acceptance --no-fail-fast` 23 `test result:`
lines, **348 passed, 0 failed**; dependency policy `11 third-party
package(s), all permitted`; secret scan `nothing found`. Terraform gates
were not run: no Terraform file was touched. Frontend gates were not
run: no frontend file was touched.

**Reviewed and repaired a fourth time, 2026-09-12**, no verdict changed.
The third round's repairs (`2c954cd..672563f`) were re-reviewed by a
fresh security-engineer pass and a fresh code-reviewer pass: no blocking
or high finding, one medium, two low and a set of wording items; every
one is fixed in the commits that follow `672563f`, ADR 0057 carries a
fourth amendment, and §22.3, §22.4 and §56.3 above are corrected in
place. The findings and their resolutions:

- **Medium** — the log lock broke `qip replay` on a read-only mount and
  on a running node's journal, unstated: the writer's open
  (`create(true).append(true)`, exclusive lock) was the only open, so a
  read-only mount failed the append, the lock refusal was relabelled
  "is not an event log this platform wrote", and an absent path was
  created. `EventLog::inspect` — read-only, never creating, shared lock
  for the read and no longer — and the CLI passes `Denied` through
  unwrapped; `replay.rs`'s doc states both cases; `log.rs` states the
  lock is proven advisory on Unix only
  (`an_inspection_of_a_held_log_names_the_holder_and_once_released_holds_nothing_itself`,
  `an_inspection_of_a_missing_path_refuses_by_name_and_creates_nothing`,
  `a_journal_on_read_only_storage_is_inspectable_where_the_writers_open_is_refused`,
  `a_journal_a_running_node_holds_is_refused_naming_the_holder_not_as_a_corrupt_file`).
- **Low** — `require_loopback_egress` was a second URL parser that read
  `http://127.0.0.1:9105@evil.com/` as loopback and admitted `localhost`
  where Terraform admits only the literal; the API's parser still
  refused only `https`. Parsed by `qip_transport::http::Url::parse`,
  `127.0.0.1` alone, called from the API's parser too so the refusal
  names `QIP_CONNECTOR_BASE_URL`; "the one brain with an egress path"
  corrected in the ADR and the deep brain (the API has a sidecar too,
  ADR 0024) (`a_base_url_off_loopback_is_refused_before_a_socket_is_opened`,
  `the_connector_pair_is_held_to_the_loopback_egress_proxy`,
  `the_connector_pair_is_both_or_neither_and_the_source_is_a_distinct_list`,
  `a_tape_and_a_connector_together_are_a_contradiction_refused_by_both_names`).
- **Low** — "one poll ahead … heals itself" was false across a crash:
  the failure propagates, the deep brain exits, the restart re-bills.
  Ledger and checkpoint are one value under one key in one `put`; a
  store holding the two old keys is refused at open; `StreamJournal::record`
  removed and `tests/restart.rs` ported to `record_and_commit`
  (`a_journal_write_that_fails_after_the_poll_bills_the_refetch_once_even_across_a_restart`,
  `a_store_holding_the_two_key_layout_is_refused_at_open_rather_than_read_as_a_fresh_stream`).
- **Wording and small items** — `references.rs` "recomputed the hash";
  the `a7c03ff` count restated above from `git show a7c03ff --stat`;
  `MetronomeBuyer::new` asserts a non-zero stride (test-only type);
  `qip-deepbrain` releases its connector sessions on the error exit too,
  keeping the run's error and appending a failed release (no test drives
  `main.rs`'s `run`; none is claimed).

Commits, in order: `bf7ad35` (the medium), `b13e52a` (the loopback
gate), `9764197` (the one-key journal), `cb767c1` (the small items), and
this entry's commit. Eleven mutations across them, each recorded in the
commit that introduced or changed the test it fires. (This sentence said
one of them was "run as an unprivileged user"; the fifth round replaced
that with what the checkout shows:
`a_journal_on_read_only_storage_is_inspectable_where_the_writers_open_is_refused`
probes whether the mode bits bind the process, asserts its read-only
half only where they do, and prints which half it proved — on a uid 0
host, this one included, it proves that the inspection loads and no
more; `ci.yml`'s `ubuntu-latest` runner is unprivileged, and no run is
cited here.) `git
diff --name-only 672563f..HEAD` names no file under `qip-risk-engine`,
`qip-execution-engine`, `qip-capital`, `qip-edge`, `qip-brokers`,
`qip-routing`, `qip-compliance` or `infrastructure/`. No dependency edge
was added; `qip-market-ingestion` already depended on `qip-transport`.

Gate, run on the tree as committed: `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets` zero warnings; `cargo test
--workspace --no-fail-fast` exit 0, **4961 passed,
0 failed**, summed across 382 `test result:`
lines; `cargo test -p qip-acceptance --no-fail-fast` 23
`test result:` lines, **348 passed, 0
failed**; dependency policy `11 third-party package(s), all permitted`;
secret scan `nothing found`. Terraform gates were not run: no Terraform
file was touched. Frontend gates were not run: no frontend file was
touched.

**Infrastructure register, 2026-09-13 — every destroy in the dev plan, named
before the dispatch.** ADR 0040 decision 10 requires this list to exist
*before* an `up`, not as a report afterwards, because the thing being checked
is whether anybody can say why each resource goes — and an agent that writes
the reasons down after the apply is writing them with the answer in hand.

Two plan runs, both **succeeded**, both reading
`Plan: 10 to add, 0 to change, 6 to destroy.` over 237 resources in
`algorik-dev`:

- Run 39, <https://github.com/droderiquesit/quantum-ai-platform/actions/runs/34753700174>,
  on the then-default branch at `ae0ae2c` — where the six destroys were
  first read. Superseded: decision 9 requires a plan on the dispatched
  commit, and the branch that carries this entry was ahead of that ref
  (`git rev-list --left-right --count ae0ae2c...HEAD` — the left figure is
  `0`, the right is however far this branch has since moved; the number is
  deliberately not written here because it changed twice while this entry
  was being drafted).
- Run 40, <https://github.com/droderiquesit/quantum-ai-platform/actions/runs/34755889202>,
  on `141e6bd`, the commit both the default branch and
  `claude/compassionate-cray-jvx8jt` were fast-forwarded to — the same
  summary line, the same state count, the same six addresses. **This is the
  plan any `up` is read against.**

Runs 34-38 (2026-09-05) are recorded in ADR 0036's amendment; ADR 0040 said
they were here and they never were, and its "Applied by this record"
paragraph now says so.

| Resource | Destroyed because | Commit | Attributed to |
|---|---|---|---|
| `module.secrets.google_secret_manager_secret.platform["qip-token-approver"]` | Removed from `secret_names`: a bearer token for a role no `qip-api` route required, whose holder could do exactly what the analyst token could do | `665c506` | The comment above `"qip-token-operator"` in `terraform/main.tf` — `grep -n 'qip-token-approver' infrastructure/terraform/main.tf` |
| `module.cloud_run["api"].google_secret_manager_secret_iam_member.mounted["token-approver"]` | The mount grant for that secret | `665c506` | "There is no token-approver mount" — `grep -n 'token-approver' infrastructure/terraform/catalogue.tf` |
| `module.cloud_run["api"\|"deepbrain"\|"fastbrain"].google_storage_bucket_object.config_files["universe"]` (three, one per workload) | Replaced, not removed: the object's path carries a hash of its content and `data/datasets/universe.json` changed after run 37 — the plan's outputs show `universe_catalogue_sha256` moving | `ece7602` | `grep -n 'config_files' infrastructure/terraform/modules/cloudrun/main.tf` |
| `module.gitops_control_plane[0].google_container_cluster.control_plane` | Tainted by run 37, whose create failed after thirty-eight minutes waiting for a node that could not reach its endpoint; replaced so it is created with the firewall rule that fixes it | run 37, not a commit — the rule is `nodes_reach_control_plane` | The `depends_on` comment on `google_container_cluster.control_plane`, and `the_control_plane_nodes_may_reach_their_endpoint_and_the_cluster_waits_for_that_rule` in `infrastructure.rs` |

Ten creates, of which the two that matter are `nodes_reach_control_plane`
and `nodes_reach_each_other` — the rules whose absence tainted the cluster —
and the two `qip-alpaca-api-*` secret containers, created empty and seeded by
nobody — the comment above them in `terraform/main.tf` says why an empty
container is the point (`grep -n 'qip-alpaca-api-key-id' infrastructure/terraform/main.tf`).

Nothing on this list is unattributed, and the owner saw the six on
2026-09-13 and said "approve all" — both halves of decision 10's test. What
is **not** yet done is the cluster: `281d9c6` flipped the module's
`deletion_protection` literal to `false` on the belief that an apply would
then replace the tainted cluster, and the review of that commit showed the
belief false — the provider reads the flag from state at delete time, and a
tainted resource is never updated in place first, so the `up` would have
refused the destroy half of the replace. The literal is restored in the
commit carrying this paragraph and the `up` is **not dispatched** until a
person with state access has removed the tainted cluster from state and
deleted it (ADR 0040 decision 11); the next plan then shows the cluster as a
plain create, and this entry gets that run's URL and terminal status.

**Branch register, 2026-09-13 — every remote branch's tip before the
consolidation.** The default branch was fast-forwarded to this lane's head,
`main` was created at the same commit, and the one branch carrying unmerged
work (`claude/algorik-architecture-refactor-pmp0zy`, eight commits from
2026-09-09 that PR #13 predated) was merged in `cbf5a55`. The other 75 were
triaged against that head — 19 strict ancestors or merges producing exactly
HEAD's tree, 43 pre-rewrite snapshots on a disjoint root whose deliverables
are byte-identical in HEAD and whose only unique files HEAD deliberately
deleted, 4 superseded file-by-file, 9 pushed stashes whose finished form is
in HEAD — and handed to the owner for deletion, because this session's egress
policy refuses a ref deletion (HTTP 403 from the proxy on `git push
--delete`, which its own README says to report rather than retry). The tips
are recorded here so that a deleted branch can be recovered by SHA for as
long as GitHub retains the objects: `git fetch origin <sha>` and
`git branch <name> FETCH_HEAD`.

<details><summary>78 branch tips as of 2026-09-13 12:10 UTC</summary>

| Branch | Tip |
|---|---|
| `claude/algorik-architecture-refactor-pmp0zy` | `6c6434377cdb` |
| `claude/autonomous-investment-platform-76gt4y` | `1c05709796ce` |
| `claude/compassionate-cray-jvx8jt` | `1c05709796ce` |
| `claude/wave6-architecture-docs` | `bdf782f6d856` |
| `claude/wave6-bugs-foundational-libs` | `9825c46aaa70` |
| `claude/wave6-central-feasibility-gate` | `e8daa51bf228` |
| `claude/wave6-console-verification` | `21e0bad1d0a7` |
| `claude/wave6-data-finder-licensing` | `f64901d7d0c1` |
| `claude/wave6-mutation-audit` | `2e19a4ce85cc` |
| `claude/wave6-pm-rescope` | `35d528d7df8f` |
| `claude/wave6-risk-limits-audit` | `d7049d4521c5` |
| `claude/wave7-agents-lib` | `ece48c1c91a5` |
| `claude/wave7-ai-lib` | `5d85a8bf6555` |
| `claude/wave7-arbitrage` | `d950292feb15` |
| `claude/wave7-blueprint-v10-gap-map` | `582df7fd3fd0` |
| `claude/wave7-capital` | `c902198c0a2b` |
| `claude/wave7-capital-fabric` | `c1e62d3328d0` |
| `claude/wave7-chain` | `e512760e4ec3` |
| `claude/wave7-cicd-status-report` | `9f9c1d4eca4e` |
| `claude/wave7-cli` | `d1cdab90af2c` |
| `claude/wave7-confidential` | `be50d9cc3be2` |
| `claude/wave7-contracts` | `0b9d8a51ddf8` |
| `claude/wave7-core` | `a9bfb9c4dad9` |
| `claude/wave7-cost-router` | `4fa36ced3511` |
| `claude/wave7-deepbrain` | `6cabf73e2ce8` |
| `claude/wave7-edge` | `f0eff6279a5d` |
| `claude/wave7-edge-node` | `70a136ec29ca` |
| `claude/wave7-entity-resolution` | `d505fb150ec6` |
| `claude/wave7-events` | `5d1f22dc13f5` |
| `claude/wave7-evolution` | `d641001c8a1f` |
| `claude/wave7-execution-engine` | `b170af97ac35` |
| `claude/wave7-fastbrain` | `415eadf395e1` |
| `claude/wave7-feature-dag` | `54c0fa45b680` |
| `claude/wave7-financial` | `a4d4f32ad077` |
| `claude/wave7-infra-node-network` | `31a268415987` |
| `claude/wave7-investment-agents` | `9742a2df17d2` |
| `claude/wave7-learning-engine` | `e8591471be9a` |
| `claude/wave7-lifecycle` | `10fed4041547` |
| `claude/wave7-market-ingestion` | `aaabbf8d5652` |
| `claude/wave7-mesh` | `216d3ebcd247` |
| `claude/wave7-mesh-registration` | `d9102d3ff781` |
| `claude/wave7-normalization` | `911ec0969f1a` |
| `claude/wave7-numerics` | `c37b4df0a016` |
| `claude/wave7-observability-ingestion` | `ad1d56fce88e` |
| `claude/wave7-observability-lib` | `d6c1ef014a97` |
| `claude/wave7-opportunity-engine` | `10295db66388` |
| `claude/wave7-optimization-engine` | `44c2594b8766` |
| `claude/wave7-orderbook` | `d7d3293d39c6` |
| `claude/wave7-portfolio-engine` | `2409fed7a991` |
| `claude/wave7-portfolio-lib` | `2782f6c8e531` |
| `claude/wave7-prediction` | `9ae2fa85b085` |
| `claude/wave7-protocols` | `25d1164ae327` |
| `claude/wave7-quantum-lib` | `f17584ac1e06` |
| `claude/wave7-reasoning-engine` | `6d0943b73e8c` |
| `claude/wave7-risk-engine` | `468bda33d613` |
| `claude/wave7-risk-lib` | `3c6a0be6c19f` |
| `claude/wave7-routing` | `43b1310a25a6` |
| `claude/wave7-sequencing` | `dd5c8e57f1e2` |
| `claude/wave7-simulation-engine` | `b2fbcc2f9d0f` |
| `claude/wave7-storage` | `8947a94b01b3` |
| `claude/wave7-strategy` | `0010760568bd` |
| `claude/wave7-training` | `e9eb14daf946` |
| `claude/wave7-twin` | `9c8c9322d095` |
| `claude/wave8-api-audit` | `8ff54d3cfdda` |
| `claude/wave8-ingestion-composition-root` | `9f4a5570bc85` |
| `claude/wave8-openobserve-otlp-wiring` | `03655a469a3b` |
| `claude/wave8-openobserve-terraform` | `352ede801712` |
| `claude/wave8-training-completeness-2` | `bf8ee3b856e8` |
| `claude/wave8-web-audit` | `9071b4e3534d` |
| `wip/wave14-inflight-e612bd6` | `e612bd683e38` |
| `wip/wave16-inflight-b6f7549` | `b6f7549eab1a` |
| `wip/wave16-inflight-e055180` | `1aa051c077bc` |
| `wip/wave18-inflight-ae78702` | `ae787027c79f` |
| `wip/wave19-inflight-47f1b0e` | `47f1b0e5e546` |
| `wip/wave21-inflight-8a3bbc2` | `8a3bbc24d34e` |
| `wip/wave21b-inflight-da52ce8` | `da52ce8d28c8` |
| `wip/wave21c-inflight-1461aad` | `1461aad55c44` |
| `wip/wave23-inflight-51661d6` | `51661d66ecb7` |

</details>

**Infrastructure register, 2026-09-13 (evening) — dev torn down on the
owner's instruction.** The owner said "Stop and delete all google cloud
resources immediately" and, told this session holds no Google credential,
"Perform the delete command for me, I give you authority." ADR 0040
decision 13 records the instruction and the shape of the act: a `teardown`
action in `infra.yml`, dispatched under the workflow's own identity, because
the agent may not run `terraform destroy` or `gcloud … delete` from its own
shell and `down` targets execution nodes of which there were none. Three
dispatches, each read from its log rather than its conclusion:

- **Run 41**, <https://github.com/droderiquesit/quantum-ai-platform/actions/runs/34781641680>,
  on `00e357f`, job "success". By `gcloud`: deleted Cloud Run
  `qip-dev-api`, `qip-dev-deepbrain`, `qip-dev-fastbrain`,
  `qip-dev-openobserve` and the control-plane cluster
  `qip-dev-control-plane` (`deleted cluster …` at 20:45:47Z) — everything
  that billed while idle. Out of state: the cluster and five KMS keys. The
  targeted destroy planned 175 and refused at plan time on
  `module.evidence.google_storage_bucket.evidence` (`prevent_destroy`; the
  step had removed keys only). Destroyed by Terraform: 0. Remaining: 231.
- **Run 42**, <https://github.com/droderiquesit/quantum-ai-platform/actions/runs/34784153948>,
  on `a872809` (state removal derived from every `prevent_destroy` and
  `force_destroy = false` declaration; five buckets left state), job
  "success". Planned 172, **destroyed 146**, stopped on two causes:
  `storage.objects.delete` denied to `qip-infra-dev@…` on the three
  `universe.json` config objects, and subnetworks `qip-dev-tz-intelligence`
  / `qip-dev-tz-cognition` "already being used by
  `addresses/serverless-ipv4-…`" — Cloud Run's direct-VPC-egress addresses,
  Google-managed and released on Google's schedule. Remaining: 78.
- **Run 43**, <https://github.com/droderiquesit/quantum-ai-platform/actions/runs/34785998589>,
  on `be1cd74` (bucket objects leave state with their buckets; the step
  prints the remaining addresses), job "success". Three objects left
  state; planned 23, **destroyed 20**, stopped again on
  `qip-dev-tz-cognition` held by `serverless-ipv4-1788775195068440944`.
  **Remaining: 55**, verbatim from the run's `what exists now`:

  ```
  terraform_data.gitops_is_placed[0]
  terraform_data.openobserve_is_placed[0]
  module.cicd.google_iam_workload_identity_pool.github
  module.cicd.google_iam_workload_identity_pool_provider.github
  module.cicd.google_project_iam_custom_role.deploy_nodes
  module.cicd.google_project_iam_custom_role.infra_storage
  module.cicd.google_project_iam_member.deploy
  module.cicd.google_project_iam_member.deploy_nodes
  module.cicd.google_project_iam_member.infra_roles[…]   (17 role bindings)
  module.cicd.google_project_iam_member.infra_storage
  module.cicd.google_project_iam_member.read_deployment_logs
  module.cicd.google_service_account.ci
  module.cicd.google_service_account.infra
  module.cicd.google_service_account_iam_member.github_impersonation
  module.cicd.google_service_account_iam_member.infra_impersonation
  module.network.google_compute_network.vpc
  module.services.google_project_service.platform[…]     (20 API enablements)
  module.trust_zones.google_compute_subnetwork.zone["cognition"]
  module.trust_zones.google_compute_subnetwork.zone["intelligence"]
  ```

What remains and why each is not a meter: `module.services` is API
enablement (free; left by design so a disable mid-destroy could not fail
the rest); `module.cicd` is the pool, provider, two accounts and their
bindings (free; the identity running the job — destroying it mid-run
revokes the token); the VPC and two subnets are free and are held only by
Google-managed addresses that release after service deletion — a further
`teardown` dispatch would be idempotent and take them, **and the owner
declined that dispatch on 2026-09-13 at 23:57Z, so the three network
entries stand**; the two `terraform_data`
entries are markers with no cloud object. **Out of state and still in the
project, not deleted:** nine KMS keys (undeletable by design; only
schedulable), five buckets declared `force_destroy = false`
(`qip-config-dev-{api,fastbrain,deepbrain}`, the egress bootstrap bucket,
the evidence bucket, the image-bake payload bucket — each holding a few
files; the identity holds no `storage.objects.delete` and this record
widened nothing), the three `universe.json` objects in them, and the
bootstrap-created state bucket `algorik-dev-qip-tfstate`, which holds the
55-entry state. The project itself is untouched: the identity holds no
`resourcemanager.projects.delete`. `gcloud projects delete algorik-dev`
(recoverable for thirty days) remains the owner's one command for the
rest.

To re-score a row: read the section in
`docs/architecture/algorik-blueprint-v10.1-source.md`, run the row's command,
and change the verdict. To re-score everything:

```
grep -nE '^[0-9]+(\.[0-9]+)* [A-Z]' docs/architecture/algorik-blueprint-v10.1-source.md
```

returns the 181 sections in order — the spine of this document, and the only
structure here that the repository did not invent for itself.

**Re-scored 2026-09-13**, §12.3's rows R4 and R6, §12.4 and §11.2 in
place, no section verdict changed — Lanes B-2 (ADR 0062) and B-3 (ADR
0063) of §12.3, on the design verified against `0093349` and its ten
corrections. Eight commits in dependency order, each gated before the next:
`dd64c0a` (the shared foundation: `score_filled` prices every fill in LEARN
under the one per-cycle cap, the size bits compare the size arms with the
twin's own `trade` arm rather than with an opening fill's zero realised
P&L, and `qip_venue_fill_error_bps{venue}` on a signed histogram — §12.4's
"fill error tracked", entry prices only, diagnostic), `606fc1f` (a
feasibility refusal names its venue on the desk, one rate window,
`qip_feasibility_refusals_total{venue,constraint}`, and `venue_review::
assess` as pure arithmetic), `40917f2` (the cell's refusals carry their
venue on the delta, `CellReport.refusals`, the centre admits only under the
eight `qip_contracts::feasibility::EDGE_GATES` at a venue the policy or a
live grant names, else `unknown`/`other`), `c759519` (the withdrawal:
journaled first, then the order manager's step 5 and the whitelist's
`retain`, resumed from the log, no cascade through the denominator, and
the fill error withdraws nothing), `7999fa7` (reinstatement on two fresh
distinct signatures, ADR 0062 with the installed-desk limit stated),
`fbcfa2b` (`construct_capped`: a bound on one name, refused outside
`(0, 1]`, floored at the minimum position, the budget equality lowered by
what the cap took), `f6f5db9` (`sizing_review`, the cap wired through
`construct_from`, armed and released on the record once), and this entry's
commit (the larger-size finding as a journaled proposal with no multiplier
on it, ADR 0063).

**R4 `ABSENT → PARTIAL`; R6 `PARTIAL → REACHED` for the executed-order
half with the loosening direction a proposal only; R5 stays absent.** The
R4 limit is on the edge and is named in the row, in ADR 0062 and here: a
desk a cell has already installed keeps its graph until the node restarts,
because `Cell::install_arbitrage` refuses a second desk and policy slot 11
has no producer; omission from the whitelist reaches a desk installed after
the withdrawal, and the closure — slot 11 carrying a withdrawn set refused
at the cell's own gate — is not built. ~~No `qip-api` route exposes
reinstatement.~~ **Superseded 2026-09-14** — see the entry at the end of this
section; slot 11 is what keeps the row `PARTIAL`, and it is still not built.

Thirty-one new tests and one existing test extended (ADR 0055's learning
test, which now holds both numbers), each mutation-verified — the
implementation broken, the test confirmed to fail for the stated reason,
the code restored byte-for-byte with `cmp`, the test reconfirmed passing —
thirty-eight mutations fired in all; this entry first said thirty-three
and thirty-nine, and both were recounted from the per-commit records
before the report was written. Among the ones that hold the decisions: the withdrawal
step deleted from the order manager (an order to a withdrawn venue reached
the venue); the step moved ahead of the kill switch (a halted, withdrawn
venue reported the venue and not the halt); withdrawn entries excluded from
the window's denominator (a sample of 32 against 40, the cascade); a
refusal admitted whatever its venue (`XZZZ` reached the window); every
withdrawn venue named on the whitelist issue whether or not the policy
names it (the issue changed for a venue the policy never carried); the
review wired to the fill error (a venue withdrawn on fill-error evidence);
the same subject accepted as countersigner; the cap check made unreachable
(a cap of zero floored to 0.5%); the bound narrowing deleted (0.6 against a
0.30 bound); the equality left at the cap-only gross ("no feasible sizing"
on the single-thesis variant); the floor removed (the capped leg dropped);
the cap folded into ADR 0055's multiplier (0.25 against 0.5); a factor of
two returned for the larger side; and the larger finding read into the
bound (0.5 against 1). One mutation in the B2-2 batch was a no-op edit on
the harness's part and is not counted; the real one beside it fired.

Adapted from the design, each stated in its commit: the twin already
exposes the entry price bare on `SimulatedFill::Filled`, so the accessor is
derived rather than a second field; the size bits compare with the `trade`
arm (above); the fill error is entry prices only; `WorkReport` carries the
venues as a side table keyed by refusal index rather than a third tuple
element; the gate literals live in `qip_contracts::feasibility`, aliased by
the edge and pinned by a kernel test on the desk rather than adding an
internal dependency; the edge test lives beside the lot-model fixture in
`qip-edge/tests/feasibility.rs`; the cluster finding carries its count; the
withdrawal set is resumed from the log; the valuation-seam compounding test
compares the positions sized rather than the traded notionals and seeds
one-share fills, for the reasons its comments give. Two files outside the
lanes' crates changed: the one `.with_refusals` line in `qip-api/src/
mesh.rs` the design names, without which no deployed cell's refusal reaches
the window, and `qip-edge-node`'s `PassOutcome::Ran` boxing its report,
clippy's own remedy for the variant gap the report's growth opened. The
implementer's checks the design asked for: `CycleWhitelist.cycles` has one
reader, a test asserting it is empty, so it is not filtered; `broker.
name()` sites in `oms.rs` are as the design counted; no production
producer of `feasibility_constraints` exists in the runtime or the apps.

The paper-trading boundary is untouched at all three layers, confirmed by
`git diff --name-only 5147c27..HEAD`: no file under `infrastructure/`;
`AutonomyLevel::deployable` and the three composition roots' autonomy code
unchanged (`qip-api/src/mesh.rs` gains one builder call, `qip-edge-node`
boxes a field); `Cell::new` remains the only cell constructor and
`with_arbitrage`/`install_arbitrage` are unchanged; `Determinism::Required`
untouched. The only execution-path change is an additional refusal in
`OrderManager::submit` before `broker.submit`; `is_simulated`,
`fill.simulated` and `qip_live_fills_total` are untouched. Every new money
figure the twin produces is `Simulated<Decimal>` with no conversion out;
the fill error is a ratio. Reinstatement can only remove a name from a
subtractive set.

Gate, run on the tree at this entry's commit before this entry was written
(a documentation change): `cargo fmt --all --check` clean; `cargo clippy
--workspace --all-targets` zero lines matching `^(warning|error)`; `cargo
test --workspace --no-fail-fast` exit 0, **5059 passed, 0 failed** over 387
`test result:` lines (with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0`,
23 GiB free throughout; the `openobserve.rs` ephemeral-port flake did not
occur); `cargo test -p qip-acceptance --no-fail-fast` **351 passed, 0
failed** after each of B2-2, B2-3, B2-4, B3-2 and inside the workspace run;
dependency policy `11 third-party package(s), all permitted`; secret scan
`nothing found`. Terraform and frontend gates were not run: no file under
`infrastructure/` or `frontend/` was touched. The independent
`code-reviewer` and `security-engineer` passes were **not run**: this
session had no facility to launch a reviewing agent, and the review is the
implementer's own.

**Re-scored 2026-09-13**, no section verdict changed — two independent
reviews of Lanes B-2/B-3 (`git diff 5147c27..0942532`), one security-focused
and one code-focused, and the four findings both surfaced as fixable in
production behaviour. Five commits, each gated before the next: `b92f2aa`
(HIGH, security — a single, unauthenticated cell could clear R4's
withdrawal bar alone; `venue_review::assess` now requires either the desk's
own single-source evidence or refusals naming at least two distinct cells,
`VENUE_WITHDRAWAL_MIN_CELLS`), `b788887` (HIGH, code review —
`construct_capped`'s "not reallocated" claim was false for more than one
approved thesis under a binding target; the achievable gross now falls by
exactly what a cap took, unconditionally, rather than only when the
narrowed bounds happened to sum below the target), `240b3fc` (HIGH, code
review — `smaller_favoured` fired on any fill that lost money before costs
at all, direction alone; `score_filled` now also requires the cost
advantage over the trade, from the twin's own cost breakdown, to be at
least as large as the directional disadvantage; `larger_favoured` is
deliberately left unfixed the same way, because the identical gate is
mathematically unsatisfiable for it, argued in ADR 0063), `540a30c`
(MEDIUM, code review — venue-withdrawal refusals were polluting R6's
declined-path evidence for the same instrument; `RefusalReason::
is_sizing_evidence` now excludes `VenueUnavailable` and the platform's
other posture refusals from the queue), and this entry's commit (the LOWs:
this document's own re-scoring, `qip-contracts`'s missing `DESK_GATES`
duplication test, the observability rule file's stale recount, and an
acceptance test naming the dual-signature identity convention explicitly
for a reinstatement route that does not exist yet).

Both reviews' remaining findings were read and left alone, stated here
rather than silently dropped: the security review's MEDIUM (the
reinstatement countersignature check compares `OperatorIdentity::subject()`
values, not people, and nothing makes that identifier durable by type) is
addressed by the acceptance test named above, not by redesigning the type —
the review itself judged that unreachable today (no HTTP route) and the
redesign out of scope (**that judgment expired on 2026-09-14**, when the
route landed; what was found on tracing it is in the entry at the end of this
section, and the type is still not redesigned); its two LOWs (the observability recount, and a
transient-delay observation about the shared per-cycle counterfactual cap
that this batch's re-reading confirmed is not a defect) are the
observability fix above and no code change, respectively. The code review's
LOW (a mutation-coverage gap on `DESK_GATES`) is the `qip-contracts` fix
above.

Nine new tests (two `venue_review` unit tests for corroboration, two
`central.rs` integration tests replacing the single-cell withdrawal
premise, one BBB-weight assertion added to an existing portfolio test, one
new adverse-price-move regression test in `learning.rs`, one exhaustive
`RefusalReason::is_sizing_evidence` test, one `central.rs` sizing-confidence
test, and one acceptance test on the identity convention), each mutation-
verified — the implementation broken, the test confirmed to fail for the
stated reason, the code restored byte-for-byte with `cmp`, the test
reconfirmed passing. Three existing tests' fixtures were rebuilt, not
weakened, because their premise was the pre-fix bug: a jump to a different
price level is pure direction, and the fix's whole point is that direction
alone must no longer arm the smaller-favoured bit, so each was rebuilt on a
tape (or a later decision instant on the same tape) that isolates a
genuine, cost-driven regret; every assertion those three tests made before
this batch still holds after it.

The paper-trading boundary is untouched at all three layers across all five
commits: no file under `infrastructure/`; the three composition roots'
autonomy code and `AutonomyLevel::deployable` unchanged; no new `Cell`
constructor; `Determinism::Required` untouched. `git diff --stat
5f5a034..HEAD -- backend/crates/apps` is empty.

Gate, run on the tree at this entry's commit: `cargo fmt --all --check`
clean; `cargo clippy --workspace --all-targets` zero lines matching
`^(warning|error)`; `cargo test --workspace --no-fail-fast` exit 0,
**5086 passed, 0 failed** over 387 `test result:` lines
(with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0`); dependency policy
`11 third-party package(s), all permitted`; secret scan `nothing found`. Terraform and frontend gates
were not run: no file under `infrastructure/` or `frontend/` was touched.
The independent `code-reviewer` and `security-engineer` passes that found
these four findings were run by a separate agent thread in a prior session,
not by this one; this entry is the fix, not a third review.

**Re-scored 2026-09-14**, §12.3's row R4 only, no section verdict
changed — §12.3 stays `PARTIAL` on R5, which is absent and untouched here.
**R4 `PARTIAL` → `REACHED`**: the edge limit ADR 0062 named as follow-on work
is closed, in one commit, by the mechanism that record named and not another.

What the limit was, so the re-score can be checked rather than believed: a
venue withdrawn at the centre was omitted from `CycleWhitelist.conversions`,
which reaches only a desk installed *after* the withdrawal. `Cell::
install_arbitrage` refuses a second desk ("a second would reset the capital
the first has committed"), nothing anywhere sets `self.desk = None`
(`grep -n 'self.desk' backend/crates/edge/qip-edge/src/cell.rs` finds the
one assignment and no clear), and `ArbitrageDesk::refresh` re-quotes existing
edges over a fixed `edge_count()` without adding or removing any. So an already-installed desk kept trading a withdrawn venue
until the node process restarted.

What closes it: policy slot 11 gains its first producer —
`CentralPlane::feasibility_constraints`, through
`Platform::feasibility_constraints`, assigned in `qip-api`'s `pending_policy`
— carrying `withdrawn_venues` and three **empty** grid maps, and
`qip_edge::feasibility::assess` refuses a withdrawn venue under the ninth
`EDGE_GATES` literal, `feasibility_withdrawn_venue`, ahead of every rule that
asks a question about the order. The withdrawal now reaches an installed desk
on its next pass. The grids stay empty for the reason `central::whitelist`'s
register gives and that register's paragraph is amended in place, not struck;
so is ADR 0062's "the edge limit, stated", and so are the two sentences in the
§12.3 row above that said the closure was not built.

**The non-obvious half, and the reason this is not a two-line change.** The
new gate joins `EDGE_GATES` so the centre charts it under its real venue and
gate rather than under `other` — the label that means a cell used a name this
build does not know — but it is deliberately **not** admitted to the window a
venue is withdrawn on, through
`qip_contracts::feasibility::is_withdrawal_evidence` and a third return vector
on `CellIngestion`. **Corrected later the same day, and the reasoning below is
kept because the correction is only legible beside it: excluding the echo
entirely was itself the `MaxExpectedShortfall` shape it was written to avoid.**
`is_withdrawal_evidence` no longer exists; see the R4 row and ADR 0062
Amendment C for `is_withdrawal_echo` and the weighting that replaced it. An installed desk reports one such refusal per intent per
pass for as long as it keeps offering cycles through the withdrawn venue;
admitted to a 256-entry rate window those echoes evict every genuine refusal
within a few passes and then hold the denominator every other venue's share is
measured against, so no second venue could ever reach three in four and no
second withdrawal could ever happen. That is a control that reads as
protection and cannot fire — this repository's `MaxExpectedShortfall`
template — arrived at by closing the edge limit carelessly. The desk seam
already had the same property by a different route: a withdrawn venue refuses
there under `RefusalReason::VenueUnavailable`, which is not a feasibility gate
and never reached the window either. ADR 0062's "no-cascade denominator" is
unchanged: a withdrawn venue's *genuine* later feasibility refusals stay in
the window. Only the echo of the decision itself is excluded.

**It can only subtract, and the payload travels on a wire that authenticates
nobody.** The amendment to ADR 0062 says so rather than leaving a reader to
derive it: `withdrawn_venues` travels centre → cell, so a forged, replayed or
corrupted payload costs a cell the ability to trade where it was configured to
trade and cannot make a venue reachable, because no field on the slot and no
branch in `qip_edge::feasibility` can add one. The dangerous direction is the
evidence direction (cell → centre), and that is what `b92f2aa`'s plurality
requirement guards. The three structural guards are untouched and one is
asserted directly: `graph_from_whitelist` against `QIP_VENUES`,
`Cell::install_arbitrage` against `self.config.venues` (asserted *with a
payload applied* by
`a_policy_payload_cannot_make_a_venue_this_cell_is_not_configured_for_reachable`,
mutation-verified against removing the check), and `whitelist_for` under
`envelope.permits_venue`.

**What still is not built, stated because R4 moving must not swallow it**: no
`qip-api` route exposes reinstatement, so the way back is two operator
signatures through `Platform::reinstate_venue` and no HTTP surface —
unchanged, and about the way back rather than the withdrawal. And a cell that
has never received *any* payload naming the withdrawal keeps its graph,
exactly as it keeps a whitelist it never received; that is ADR 0008 and an
availability limit on the centre's reach, not a boundary. As with every edge
behaviour, `qip-edge-node` runs `Cell::work` only under
`QIP_VENUE_FEED=simulated` and `execution_nodes = {}` in every environment, so
no deployed process runs this gate — the same standing caveat the
observability rule file carries, restated so the re-score is not read as a
deployment claim.

Five new tests, each named as a sentence, premise-first, and each
mutation-verified — the implementation broken, the test confirmed to fail for
the stated reason, the code restored byte-for-byte with `cmp`, the test
reconfirmed passing:
`the_withdrawn_venue_gate_is_vocabulary_the_centre_admits_but_never_evidence`
(`qip-contracts`, fired on `is_withdrawal_evidence` widened to admit the
gate — both the test and the function it exercised were replaced later the
same day, by `the_withdrawn_venue_gate_is_vocabulary_the_centre_admits_and_an_echo_only_the_centre_names`
and `is_withdrawal_echo`; this list is the record of what was verified then,
not a list of tests that exist now), `a_desk_installed_before_a_withdrawal_stops_trading_the_withdrawn_venue_on_its_next_pass`
(`qip-edge`, fired on the withdrawn-set lookup neutered in `assess` — three
legs went out again), `a_policy_payload_cannot_make_a_venue_this_cell_is_not_configured_for_reachable`
(`qip-edge`, fired on `install_arbitrage`'s venue check short-circuited),
`the_slot_the_centre_ships_carries_the_withdrawn_set_it_applies_and_states_no_grid`
(`qip-kernel`, fired twice — once on the producer shipping an empty set, once
on it fabricating a tick), and
`a_refusal_a_withdrawal_itself_caused_is_counted_and_never_lands_in_the_window`
(`qip-kernel`, fired twice — on the echo admitted in `attribute_refusals`, and
on the echo recorded into the window from `ingest_cell_report`). One existing
test did exactly the job it was written for and was edited in the same change,
as its own message instructs:
`a_shipped_payload_produces_exactly_the_four_slots_the_register_names` failed
the moment slot 11 gained a producer and is now
`..._the_five_slots_...`, asserting the new slot ships with empty grids and
an empty withdrawn set on a fixture that withdrew nothing.

The paper-trading boundary is untouched at all three layers: no file under
`infrastructure/` (Terraform's refusal of the three live ceilings is
unchanged); the three composition roots' autonomy code and
`AutonomyLevel::deployable` are unchanged, and the only production line
touched under `backend/crates/apps` is one slot assignment in `qip-api`'s
`pending_policy` (beside its doc comment and the mesh test that guards the
produced set);
no new `Cell` constructor and no ceiling parameter anywhere, and
`qip-cost-router`'s `Determinism::Required` arm is untouched. Nothing in this
change reaches a `Placer`: the new gate is a refusal, and `Cell::send` — the
one place a `Placer` is called — is not modified.

Gate, run on the tree at this entry's commit with `CARGO_INCREMENTAL=0
CARGO_PROFILE_DEV_DEBUG=0`: `cargo fmt --all --check` clean; `cargo clippy
--workspace --all-targets` exit 0 with zero lines matching `warning`; `cargo
test --workspace --no-fail-fast` exit 0, **5091 passed, 0 failed, 0 ignored**
summed over 387 `test result:` lines; dependency policy `11 third-party
package(s), all permitted`; secret scan `nothing found`. Terraform and
frontend gates were not run and do not apply: no file under `infrastructure/`
or `frontend/` was touched.

**Amended 2026-09-14, no verdict changed**: ADR 0062's two hand-named
follow-ons, in two commits gated separately. `3e2fb81` replaced the
`Malformed` refusal whose detail string carried an `infeasible (<gate>):`
prefix with `RefusalReason::Infeasible { venue, gate, detail }`, so
`feasibility_gate()` reads a field instead of parsing a sentence written for
a person, and `capture_submission` reads the venue off that field instead of
re-deriving it from `self.broker.name()` at the capture site. The recorded
values are deliberately identical — the same gate literals on
`qip_orders_refused_total{control}` and `qip_feasibility_refusals_total`, the
same venue on `DeclinedPath`/`DeclinedScore`, the same `describe()` sentence
byte for byte, because the hash-chained log holds refusals written on both
sides of the change. `13edb2a` added `POST /venues/:venue/reinstatements` at
the operator role and `GET /venues/withdrawals` at the viewer role, closing
the gap ADR 0062 stated in its own words ("No HTTP route exposes it yet"):
until then, recovering a venue the platform had withdrawn on its own evidence
needed direct access to the kernel, which is how a fail-closed control gets
recovered from by a path nobody audited.

§12.3's R4 **stays `PARTIAL`**, and the reason is unchanged: policy slot 11
has no producer, so a cell whose arbitrage desk is already installed keeps
the withdrawn venue's conversions until the node restarts. A route that lets
two people undo a withdrawal does not close an edge limit, and the row is not
moved for work that did not touch it.

On the security review's standing MEDIUM — the countersignature check
compares `OperatorIdentity::subject()` values and nothing makes that
identifier durable by type — the route made it reachable, so it was traced
rather than assumed. `principal.subject` is `Credential::subject`, fixed when
the composition root mints the credential and copied unchanged on every
authentication; it is neither session- nor request-scoped, and
`security.rs::every_operatoridentity_is_built_from_the_principals_durable_subject_not_a_session_value`
now holds that end to end instead of asserting that no route existed. The
residual is the other direction and is recorded in ADR 0062 Amendment B: the
composition root mints one credential per *role*, so two humans holding
`QIP_TOKEN_OPERATOR` present one subject and a countersignature is **refused**
— closed rather than open, identical for the promotion and recalibration
routes that shipped earlier, and a deployment change rather than a code one.

Six new tests (one in `qip-kernel/tests/central_feasibility.rs`, four in the
new `qip-api/tests/venues.rs`, one rewritten in `qip-acceptance/tests/
security.rs`) and two existing tests extended rather than relaxed; ten
mutations applied and every one fired, each restored byte-for-byte with
`cmp`. Among them: the constructed `venue` replaced (`left:
"somewhere-else"`); the kernel's carried venue replaced after the window was
written, which is what proves the declined path follows the field (`left:
Some("elsewhere")`); an unknown gate given a fallback to a known literal
(`left: Some("feasibility_lot") right: None`); `Infeasible` dropped from
`is_sizing_evidence`; the route's role lowered to `Analyst`; the body's
unknown-key screen disabled (`left: 200 right: 400`); the kernel's
not-withdrawn check removed (`left: 200 right: 404`); the
`first.approver == operator.subject()` check disabled; the standing boolean
hard-coded false; and `Principal::subject` made session-scoped by appending
the authentication instant (`left: "operator@example.com#1760000000" right:
"operator@example.com#1760000900"`).

Gate at this entry, run on the tree before it was written (a documentation
change): `cargo fmt --all --check` clean; `cargo clippy --workspace
--all-targets` zero lines matching `^(warning|error)`; `cargo test
--workspace --no-fail-fast` exit 0, **5091 passed, 0 failed** over 388 `test
result:` lines (with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0`);
`cargo test -p qip-acceptance --no-fail-fast` 354 passed, 0 failed over 23
suites; dependency policy `11 third-party package(s), all permitted` —
unchanged, the one dependency added is the in-workspace
`qip-execution-engine` on `qip-api`'s **dev**-dependencies, which the
forbidden-edge scan skips by design and the third-party count does not see;
secret scan `nothing found`. Terraform and frontend gates were not run: no
file under `infrastructure/` or `frontend/` was touched. One acceptance run
during the work failed `the_cycle_cost_stops_growing_once_the_history_
working_sets_reach_their_bounds` on a timing ratio under load from a parallel
clippy, and passed alone and in two clean full runs afterwards; nothing in
either commit runs inside a cycle. No independent `code-reviewer` or
`security-engineer` pass was run on this work: the review here is the
implementer's own.

**What this document replaced**, deleted in the same commit: `PROJECT-PLAN.md`,
`completion-plan.md`, `blueprint-v10.1-gap-map.md`, `current-state.md`,
`gap-matrix.md`, `gate-completion-plan.md`, `rescore-2026-09-06.md`,
`wave-7-backlog.md`, `wave7-cicd-status-report.md`,
`wave7-cycle-conformance-review.md`, `algorik-blueprint-traceability.md`,
`integration-truth-pass.md`, `current-state-audit.md`,
`deployed-vs-blueprint.md`, `diagram-gap-audit.md`, `diagram-reconciliation.md`,
`canonical-platform.md`, `blueprint-diagram-reconciliation.md`,
`repo-inventory.md` — 9,634 lines. Three of them already declared themselves
superseded in their own first line.

**Amended 2026-09-14, no verdict moved: the review round on Lane B, and what
two independent reviews found in it.** The three lanes merged as `dbc1ff5`
were reviewed by a fresh `security-engineer` and a fresh `code-reviewer`,
neither of which had written any of it — the gap the entry above names as
"the implementer's own". Between them they returned four findings of HIGH
consequence, and **three of the four were the same defect wearing different
clothes: a control that reads as protection and cannot do what it says**, the
`MaxExpectedShortfall` shape `.claude/rules/domains/risk-and-execution.md`
names. The count for this engagement is now eight, and the newest was
*introduced by the fix for the previous one* — excluding a withdrawn venue's
echoes from the window, which protected the window by emptying it.

The findings, each reproduced by execution before it was fixed:

1. **The fifteen-minute operator-freshness window measured process uptime.**
   `qip-api`'s composition root binds one instant at start-up and stamped it
   on every credential, so a leaked token of any age read as fresh for
   fifteen minutes after each restart and every operator was refused
   afterwards. Seven routes. Fixed by making the absence structural — a
   standing bearer token carries no authentication instant and the gate
   refuses rather than accepting a fabricated one (ADR 0065). Seven
   capabilities are now unreachable, which is recorded rather than softened;
   three runbooks that instructed an operator to re-authenticate and retry
   were corrected, `kill-switch.md` most urgently, because it is read with a
   desk stopped.
2. **ADR 0064's "no weight moves" guarantee was held by a scan that missed
   every realistic way of breaking it** — it detected whole-field
   reassignment only, so a shipped method calling `self.central.set_proposal`
   passed, and it examined a method only if its *name* contained "famil". Its
   return check was a deny-list any newtype walks past. Rebuilt with a
   positive and a negative control per detector. Second instance in this
   repository; `28857ed` was the first.
3. **Excluding withdrawn-venue echoes emptied the denominator**, so the
   runner-up's share climbed on a shrinking divisor until the desk cascaded
   to no venues. Replaced by ADR 0062 Amendment C's split of counting from
   judging.
4. **The echo classification believed a gate string from an uplink that
   authenticates nobody**, so one cell holding a stale policy slot could hold
   the control shut indefinitely.

Also corrected: the family figure's trial count, which agreed with the gate's
per-member snapshot only at the single-member arity the shipped test was
pinned at; the evidence bar, which counted registered members where this
document had always said evaluated — the document was right and the code was
not; and seven documents the `dbc1ff5` merge had falsified.

**One methodological note worth more than any single fix.** A lane correcting
a stale count in `observability.md` reported a fresh count it had not
measured, phrased as an observation of its own worktree. A sibling lane
checked rather than trusting it and found it wrong. That is the same defect as
the four above — an assertion that reads as evidence and is not — expressed in
prose instead of code, produced inside the lane fixing an instance of it. The
figure was removed rather than replaced, and the rule file now carries the
discriminator and the command instead of a number.

Gate on the merged tree, run by the orchestrator rather than reported by any
lane: `cargo fmt --all --check` clean; `cargo clippy --workspace
--all-targets` **0** lines matching `^(warning|error)`; `cargo test
--workspace --no-fail-fast` exit **0**, **5120 passed, 0 failed** over **388**
`test result:` lines; dependency policy **11 third-party packages, all
permitted**; secret scan nothing found. An earlier run of the same suite
reported **1 failed** — an acceptance test pinning two sentences of
`kill-switch.md` that the correction above had made false. It was fixed by
strengthening the test, not by reverting the runbook, and both new assertions
were mutation-verified with byte-identical restores. Terraform and frontend
gates did not run and are not claimed: no file under `infrastructure/` or
`frontend/` is in this diff. Paper-trading boundary intact at all three
layers, checked on the merged tree: Terraform still refuses the three live
ceilings, all three composition roots still route through
`AutonomyLevel::deployable`, and `qip-edge`'s `Cell` and `qip-cost-router`
are untouched with `metrics.refusal(` at exactly its two sites.
