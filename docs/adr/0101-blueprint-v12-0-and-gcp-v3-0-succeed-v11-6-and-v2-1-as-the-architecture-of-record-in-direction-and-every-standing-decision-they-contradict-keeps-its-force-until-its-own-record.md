# ADR 0101: Blueprint v12.0 and GCP v3.0 succeed v11.6 and v2.1 as the architecture of record in direction, and every standing decision they contradict keeps its force until its own record

- **Status**: Accepted **for the adoption in direction only**, on the owner's
  instruction of 2026-10-04. The owner's words, verbatim and typos included, were "i wave all just peform
  the work". That covers process approvals and the adoption in direction
  (Decision 1-5), and nothing in the register: it lets the work proceed
  without asking at each step. It is not an amendment to ADR 0003, ADR 0021 or any
  rules file, and nothing here treats it as one. **Proposed for everything
  else.** Each conflict in the register below is resolved by its own later
  record, or is not resolved.
- **Date**: 2026-10-04
- **Supersedes**: ADR 0099's choice of *which* blueprint is the reference, and
  nothing else in ADR 0099. Its conflict register, its "What this does not do"
  and its paper-trading section are carried forward below, not replaced.
- **Related**: ADR 0003, ADR 0021. ADR 0023 is unchanged and still opens
  nothing: it records that real trading is the owner's intended destination
  and that the opening is gated, and this record neither advances nor delays
  it. ADR 0001, ADR 0002, ADR 0005, ADR 0006, ADR 0007, ADR 0009, ADR 0011,
  ADR 0012, ADR 0016, ADR 0024, ADR 0036, ADR 0040, ADR 0069, ADR 0083, ADR
  0089, ADR 0091, ADR 0093 (standing decisions the new target contradicts or
  that bite on a row; see the register). ADR 0100 (the in-tree build, a
  consequence of ADR 0002 and ADR 0009 rather than a prohibition). ADR 0098
  (the development factory that carries the programme).

## Context

ADR 0099 made Master Blueprint v11.6 and GCP Platform Blueprint v2.1 the
architecture of record and ended, as ADR 0022 had, with the sentence that
matters here: *a v11.7 or v2.2 arriving does not become the architecture of
record by existing; that takes the owner, again.*

On 2026-10-04 the owner supplied the next versions and directed the work on.
They are held verbatim in `docs/blueprint/source/`, with SHA-256 in
`docs/blueprint/README.md`:

- **Algorik Master Blueprint v12.0**, "Financial Superintelligence +
  Pass-Through Knowledge + Native Rust Event Fabric". 51 pages.
- **Algorik GCP Full Platform Blueprint v3.0**, its companion. 30 pages.
- **Algorik Full Platform Architecture v3.0**, the interactive diagram.

Two sentence-level comparisons, recorded in `docs/blueprint/v12-delta-master.md`
and `docs/blueprint/v12-delta-gcp.md`, say what actually changed, and the
answer is smaller than the titles suggest. v12.0 is almost purely additive. It
inserts 23 numbered subsections, a count that includes §24.9 and §31.1 (the NOW Brain, the Temporal Forecast Lattice,
the model tournament and Forecast Market, synthetic futures and digital twins,
active sensing, the Meta-Intelligence Brain, the Cognitive Compiler and the
Forecast Contract in §9.6-9.14; a quantum-classical compute fabric with a
Problem Compiler and a Hybrid Compute Router in §11; a Capital Intelligence
Society, Capital Survival Kernel, Shadow Portfolio Universe, Hedge Brain and
Model-Risk Brain in §13-14; a Forecast Scoreboard in §22.1; §24.9 holds seven control
contracts and §31.1 ten completeness targets). It deletes no section, and its
§3-8, 10, 12, 15-21, 23, 24.1-24.8, 25-30 and 32 match v11.6 apart from the
rename of AGI to superintelligence (the §8 sentence on the "Financial
Superintelligence context window" is one) and page-break shifts. GCP v3.0 likewise removes no v2.1 row; it adds a
"Superintelligence Plane" of about twenty-seven named services, a per-service
catalogue, a connection matrix and build phases 11-16, and it states **no cost
figure anywhere**.

One edit is not additive and is recorded here because it is the kind a reader
misses. v12.0's §33 "Final Definition" **drops** v11.6's sentence that every
action able to move money, alter an external system or communicate publicly is
"bounded by explicit identity, authority, truthfulness, conduct, feasibility,
audit and rollback controls". The controls are still stated in §1.3 and §24.5.
The summary no longer carries them, and a summary is what gets quoted.

## Decision

1. **v12.0 and GCP v3.0 are the architecture of record in direction**, with
   the v3.0 diagram, succeeding v11.6 and v2.1. Every architectural claim in
   this repository is scored against them. v12.0 governs *what* the system
   does and v3.0 *where and how* it runs on GCP, on ADR 0099's precedence
   rules, which are unchanged.
2. **Because v12.0 is additive, the v11.6 requirement catalogue stands.** The
   1,566 requirements under `docs/blueprint/requirements/*.json` and the matrix
   rows against them are not re-derived. v12 additions enter as new
   requirement entries in the same catalogue, in the same `DOMAIN-NNN` scheme,
   IDs never reused. **Those entries use source codes `M12` (Master Blueprint
   v12.0) and `G3` (GCP v3.0)**. `qip-cli`'s `blueprint.rs` `cite()` maps them
   to "v12.0" and "GCP v3.0" only in an **uncommitted working-tree change** on
   this branch (checked 2026-10-04: at the last commit `cite()` maps `M`, `G`
   and `H` alone). Until that change is committed by the lane that owns
   `blueprint.rs`, an entry citing v12.0 renders as nothing, and this record
   does not claim otherwise. An entry citing v12.0 by any other code is a
   defect.
3. **v11.6 and v2.1 are superseded in direction only.** Their sources stay in
   `docs/blueprint/source/`, because the existing requirement IDs cite their
   pages. Standing ADRs decided against them remain in force.
4. **There is still one live register.** It is the matrix, now scored against
   v12.0 and v3.0 where they add or change a requirement. A new `M12` or `G3`
   entry starts `NOT_STARTED` or `BLOCKED`, and is never scored from the
   in-flight `qip-deepbrain` files. `DELIVERY-STATUS.md`
   stays the v10.1 historical register, as ADR 0099 made it.
5. **Where v12.0 is silent, no requirement is invented.** The delta lists
   eleven unknowns (where any of it runs, build order, failure behaviour,
   thresholds, the authority of the new brains, whether synthetic capital is
   ever convertible, where durable state lives, name equivalences, the Solver
   Registry, what a "microstructure probe" is, two unread figures). Each is
   recorded as an open question against its section, not resolved by choosing
   the convenient reading.

## What this does not do

Carried forward from ADR 0099, whose hazard is the same.

**This decision authorises no execution whatsoever.** It settles what the
platform is aiming at. It does not provision, migrate, decommission, add a
dependency, open a path, start a service, or spend, and nothing in it may be
cited as permission to. In particular it does not authorise:

- any of GCP v3.0's roughly twenty-seven new services, on any runtime;
- any accelerator: TPU7x, a GPU pool, Spot GPU or TPU, a reserved slice;
- Vertex AI Agent Engine, its Sessions or Memory Bank, Cloud Batch, Cloud Run
  Jobs for agent fleets, Cloud Scheduler or Workflows, or Cross-Cloud
  Interconnect;
- a new dependency of any class, or a new binary.

**A standing decision is not overridden by a target that contradicts it.**
`.claude/rules/10-product-direction.md` says a standing decision is reopened
by an ADR, not a code change, and adopting a document is not an ADR that
reopens anything. Cloud work still passes through the gates ADR 0040 and ADR
0093 set; `prod` is refused by the workflow and by the rules.

**The owner's 2026-10-04 instruction does not reach the boundary** (Status
states its reach once). Superseding ADR 0003 is a named, owner-made decision
(C1 below).

## The paper-trading boundary, stated separately because it matters most

Everything ADR 0099 listed as assuming real capital is unchanged and
unauthorised: `prod` live bounded execution, autonomous treasury transfer,
custody and settlement, purchase executors, market creation, and public
communication. v12.0 adds **no new live-capital, purchasing, market-creation
or communication path**; its agency text is v11.6's verbatim. It does add
capital-adjacent machinery, and that is where the boundary will be tested:

- the Capital Survival Kernel's funding and collateral movement "across
  currencies, venues, counterparties and legal entities" (§13.2);
- the Hedge Brain's search over "options, futures, FX, rates, commodities,
  volatility, correlated assets, event contracts and cash/collateral actions"
  (§14.1);
- unwind plans and capital grants distributed to regions (GCP v3.0 §11.7);
- the Cognitive Compiler's executable Reflex packages (§9.13), which move a
  model into a live-capable path;
- active sensing's "market microstructure probe" (§9.10), which is undefined
  and which this record treats as `EXTERNAL_ACTION` until it is defined, since
  a probe of a live venue is not a simulation.

**None of that is authorised.** Two postures are used below and are not
interchangeable. *Shadow* is counterfactual: it is computed and scored and
never reaches the simulator. *Paper* is a proposal admitted by the
deterministic Risk Gate into the simulated broker, with no live venue behind
it. Each item is assigned one in C9; an item with no assignment is shadow. ADR 0021 stands exactly as written and the three layers stay
intact: Terraform's refusal in `infrastructure/terraform/variables.tf`,
`AutonomyLevel::deployable` in the composition roots, and a `Cell` with no
constructor taking a ceiling other than paper trading. A requirement that
cannot be met without live capital or external action carries `LIVE_CAPITAL`
or `EXTERNAL_ACTION`, is scored `BLOCKED` with ADR 0021 as the reason, and is
built no further than the posture C9 assigns it. v12.0 itself says to begin so (§30
"Begin shadow-only"; §25 "unsupported opportunities remain shadow-only").

## The conflict register

C1-C8 are ADR 0099's, carried forward with their force and their resolving
records unchanged, and v12.0 touches C1, C2, C4, C5 and C8 and leaves C3, C6
and C7 as they were (GCP v3.0 does touch C3 and C7: see C9 and C10). C6
remains **declined**: ADR 0016's monorepo stays. Rows C9 onward are new.

| # | The target says | The standing decision | Resolved by |
|---|---|---|---|
| C1 | Live bounded execution in `prod`; treasury transfer, custody, purchasing, market creation, public communications. v12.0 adds cross-entity funding and collateral movement (§13.2) and hedge execution including "cash/collateral actions" (§14.1) | ADR 0003, 0021, 0023; `01-security-and-safety.md` | **Not an agent's decision.** Requires the owner to supersede ADR 0003 and amend the rules file. No record written here may do it, and the 2026-10-04 instruction is not that act. |
| C2 | Fabric on Tokio/QUIC/mTLS/prost/BLAKE3/Raft; gRPC clients. v12.0 adds GPU/TPU/accelerator clients and every edge of v3.0's §20.1 matrix assumes this stack | ADR 0002, 0009, 0012 | One dependency record per class, as in ADR 0099. Until then the fabric is built on what the workspace permits, which is what ADR 0100 does in-tree. Z3, OR-Tools, prost and a Raft crate are named in C2 and C5 and are unproven (see Names not verified). |
| C3 | Regional GKE Standard for warm services; Config Sync, Argo CD, Kargo, Rollouts, Cloud Service Mesh. v3.0 puts most of its twenty-seven new services on GKE | ADR 0024, 0036, 0093 | A runtime record, as in ADR 0099. v3.0's population on GKE is a larger instance of the same question, not a new answer. |
| C4 | Spanner, Spanner Graph, Bigtable, BigQuery, AlloyDB, Memorystore, Knowledge Catalog. v12.0/v3.0 add Agent Engine Memory Bank and make these the memory of the intelligence plane | ADR 0002, 0009 (ADR 0089 is a retention-class decision, not a ban on these stores; ADR 0099 cited it the same way and it is not copied forward as a prohibition) | A storage record per store, ledger first. See C13 for the part that is a second source of truth. |
| C5 | Python/JAX/PyTorch research; Z3/OR-Tools. v3.0 adds TPU7x (a name unproven here) and JAX/PyTorch for strategic forecasting and twins | ADR 0001, 0083 | A research-runtime record. The hot lane stays Rust. A distilled Cognitive Compiler package (C9a) is packaging of a trained artifact and is held by this row too. |
| C6 | Seven repositories | ADR 0016 | **Declined**, as in ADR 0099. |
| C7 | Separate warm services per brain. v3.0 states "these services scale independently" of about twenty-seven | ADR 0091 (ADR 0100 applies 0091's own test and raises the binary count from five to seven; that is an application, not an amendment) | A runtime record taken with C3. Independent scaling is asserted, not measured, and ADR 0091 requires measurement. |
| C8 | Three execution regions, C4D/C4 Reflex VMs, five brokers per region | ADR 0093; `execution_nodes = {}`; the execution-node module accepts only C3 and C3D shapes | An owner cost ceiling, then a deployment record. Also blocked externally while billing is disabled on dev. |
| C9 | **Capital and hedge items.** Capital Society, Survival Kernel, Hedge Brain, Shadow Portfolio Universe, grants and unwind plans | ADR 0003, 0021; `risk-and-execution.md` (prohibits "Escalating autonomy from a model output, an agent finding, or a config value" and any live-order path) | **Posture per item, by this record.** *Shadow Portfolio Universe:* shadow; needs nothing beyond paper and is not blocked. *Hedge Brain, Capital Survival Kernel, Capital Society, grants and unwind plans:* proposals only, admitted or refused by the deterministic Risk Gate into the simulator (paper); a simulated hedge order is permitted on that path and no other, and no model output creates an order, raises a grant or changes autonomy. Funding, collateral and cash movement beyond the simulator is C1. |
| C9a | **Cognitive Compiler Reflex packages** (§9.13), which move a distilled model into a fast path | C5 / ADR 0083 (packaging Python-trained artifacts for in-process serving); ADR 0008 (cells decide alone); `Determinism::Required` | **Safe form stated:** a distilled package is a deterministic artifact that a cell loads only through a reviewed release, it is never fetched or swapped by a model at run time, and it can never widen an envelope or a grant. Until that is built and tested, shadow. |
| C10 | **Compute Intelligence Router and Hybrid Compute Router** choosing among Rust/CPU, GPU, TPU, quantum-inspired methods and IBM QPU, learning from "measured quality, calibration, latency, repeatability and cost" (§11.1-11.2; v3.0 §11.6) | C5 (a non-Rust substrate is a research-runtime question); C2 (accelerator clients); ADR 0006 (a classical baseline every time); ADR 0083 | A router that dispatches only among substrates the workspace already has, with the classical baseline computed every time and results verified classically (v12.0 §11.2 says as much), may be designed now. A route to GPU or TPU is C5 and C2 and waits for their records. The router is an Algorik concept, not a GCP product. |
| C11 | **Cost with no budget.** v3.0 states no dollar figure anywhere, yet specifies reserved TPU slices, GPU pools, "massively parallel" counterfactual farms, thousands of concurrent models and "hundreds/thousands" of ephemeral agent workers | ADR 0093's owner cost instruction; the dev billing state; ADR 0069 (no capability without a consumer) | **No owner ceiling is recorded.** The "25 USD/day" in `v12-delta-gcp.md` is that delta's own assumption for judging "fits", written as "a judgement, not a quote"; it is not an owner decision, no file in the tree holds a daily figure, and ADR 0098's reopening condition speaks of a monthly one without stating it. The delta's "fits" column is therefore an assumption. Until the owner records a ceiling (and says whether it is daily or monthly) the budget for the Superintelligence Plane is zero cloud spend, billing being disabled on dev, so every v12 item is in-process shadow or paper work only. No element of the Superintelligence Plane is provisioned against an unstated cost. Elements that fit only as bounded in-process logic in an existing binary are said to, row by row, in the delta, and that is the only form this record contemplates. |
| C12 | **Model-risk authority vs the deterministic Risk Gate.** The Model-Risk Brain makes grants and size "contract automatically" (§14.2) and §31.1 says capital, hedge, model-risk and survival systems "can veto/reduce exposure" independent of alpha. The Confidence Governor is a confidence authority | RISK-020 (a requirement: the Gate enforces and the Risk Brain only forecasts, §14); `10-product-direction.md` and `risk-and-execution.md` (pre-trade checks never route to a model; `Determinism::Required`); `01-security-and-safety.md` layer 3; ADR 0005 (confidence is arithmetic) | The reading this record takes, to be made structural by a design record and proven by a test: (a) the Gate evaluates `min(configured, model_supplied)` and the model-supplied side is typed so it cannot express a raise; (b) shrinking a grant below an open position is a reducing order created by the Gate, deterministically, never by a model, and nothing is auto-unwound by a model; (c) a stale or missing model-risk output leaves the configured limit in force, neither tightening to zero nor widening; (d) this holds for Hedge, Capital and Survival systems (§31.1) as much as the Model-Risk Brain (§14.2); (e) a confidence figure is arithmetic under ADR 0005, not a governor's assertion. A path that bypasses the Gate, or one that can widen anything, is refused. The text does not say this, and a reading that lets a model veto outside the Gate is not adopted. |
| C13 | **State stores vs "no second source of truth".** The NOW Brain, forecast lattice, branch trees, scoreboard and Forecast Market persist state with no stated home (v12.0 §9.6-9.9, §22.1; v3.0 §11.8: Spanner Graph, Bigtable, BigQuery, Agent Engine Memory Bank) | `00-boundaries.md`: no second source of truth for a fact the event log already holds; ADR 0089 (retention class); ADR 0007 (exact attribution, for scoreboard and economic attribution); ADR 0005 (confidence arithmetic, for the Forecast contract and "calibration as SLO"); ADR 0099 C4 | A storage record that says, per kind of state, whether it is derived (and reproducible from the log), or a fact the log does not hold. Resolved forecast outcomes and scores are derived and may not become a rival record. **Synthetic reputation or capital (v12.0 §9.8, §22.1; delta C.6) is never an input to a `CapitalEnvelope` or a capital grant**, and is not convertible to one; the blueprint does not say so and this record does. |
| C14 | **CapitalGrant is defined twice in the blueprint, and two different in-tree types share or approach the name.** §24.9 (p39): amount, strategy, region, venue, risk budget, liquidity reserve, expiry. §27 (p42): owner/mandate, region/strategy, amount, collateral class, leverage, expiry, drawdown and liquidity constraints. §27 was not updated. In the tree: `qip_contracts::capital::CapitalGrant` is an admission outcome (`Full`, `Reduced(Decimal)`, `Refused(String)`) returned by `CapitalEnvelope::admit`, not a grant record; the wire shape under the schema lock is `qip_mesh::spine::CapitalGrantFrame`, which wraps a `CapitalEnvelope` (`event_fabric_schema_lock`) | `qip-contracts::capital`; the schema lock (`event_fabric_schema_lock`) | A contract record choosing one blueprint form and naming how it relates to the in-tree types, which are a third and fourth definition. Until then the locked wire shape is `CapitalGrantFrame` over `CapitalEnvelope`, and neither blueprint form is treated as a wire shape. Adopting the union would change a locked wire form without a record. |
| C15 | **§33 drops v11.6's control sentence.** The Final Definition no longer says every money-moving, system-altering or public action is bounded by "identity, authority, truthfulness, conduct, feasibility, audit and rollback controls" | `01-security-and-safety.md`; v12.0's own §1.3 and §24.5, which still state it | **No requirement is dropped.** The v11.6 sentence is carried forward as the requirement, citing v12.0 p4 and p38, and §33 is read as a shorter summary, not a relaxation. The mechanism: the requirement is carried by a new catalogue entry in the `GOV` domain under source `M12` (ID allocated when the entry is added, restating the v11.6 sentence and citing v12.0 p4 and p38); until that entry exists the sentence is a promise no row carries, and this record says so. If the owner means §33 as a relaxation of the control list, that requires the owner to amend `01-security-and-safety.md`; it is not an agent's call. |

Rows C9-C15 are the ones this record adds. Three of them (C9, C12, C14) are
not resolved by waiting for money or an owner cost ceiling: they are
resolved by a design record, and they can be written now without a boundary
decision.

## Names not verified

Under the existence rule (`HERMES_MISSION.md` s2.4, recorded as U24 in
`docs/SYSTEM_MAP.md`; it is in neither `CLAUDE.md` nor `.claude/rules`, so it
binds by that file and not by the rules corpus), an item cannot enter an ADR
as real until it is **installed or called and version-captured**. A
first-party web page is evidence to look at, not that proof. The following are
**unproven**, and no requirement, Terraform resource or ADR may rely on them
until then:

- **The rule's own items**: Z3, OR-Tools, prost, a Raft crate and a Qiskit
  client version (C2 and C5 name the first four).

- **TPU7x (Ironwood)** as a GA machine or slice type through GKE or Compute
  Engine; **"Gemini Enterprise Agent Platform"** as the Vertex AI branding
  (the blueprint itself calls it unstable); **Vertex AI Agent Engine**
  Sessions, Memory Bank, A2A, bidirectional streaming and sandboxed code
  execution as GA; **"Knowledge Catalog (formerly Dataplex Universal
  Catalog)"**; **Cloud Batch** (used as a product without a precise name);
  **Cross-Cloud Interconnect** reaching IBM Quantum's endpoints.
- **Qiskit Functions**, the "hybrid quantum-enhanced ensemble classification
  workflow" citation, "IBM quantum-centric supercomputing reference
  architecture (March 2026)", "IBM 2026 roadmap", and the 2024 Physical Review
  Applied survey cited in §32.
- **Blueprint-internal names with no external implementation**: Compute
  Intelligence Router, Problem Compiler, Solver Registry, Cognitive Compiler,
  NOW Brain, Forecast Market, Confidence Governor, Capital Kernel versus
  Capital Survival Kernel, Quantum Gateway. These are things to build, not
  products to find, and where two names may be one thing the delta says so.

## What it costs

- **A larger standing gap again.** v12.0 adds 23 subsections and v3.0 about
  twenty-seven services. By the delta's own approximate count (title greps, and
  including units that are not subsections), roughly eleven of the 23 match no
  existing requirement. The completion percentage will probably drop once the
  matrix is rescored with those entries, though no code got worse.
- **Fifteen open conflicts, and the new ones are not all waiting on money.**
  C1 and C11 need the owner. C9, C12, C13, C14 and C15 need records that can
  be written without one, and that is the work this ADR leaves behind.
- **The target and the loop disagree on scale.** v3.0 imagines thousands of
  models and agent workers, and the programme runs on one desktop within a
  stated cost posture (ADR 0098). The honest position is that the Superintelligence
  Plane is scored far from done, for a long time.
- **The documents are partly inconsistent.** Phase 8 is still named "Financial
  AGI Autonomy", §30 has no v12 phase, §28 has no failure row for any new
  component, and v3.0 names two disk products for one Fabric job. The record
  scores what is written and does not smooth it over.
- **Work in flight moves target.** The in-flight `qip-deepbrain` state
  estimator and forecast lattice fit §9.6 and §9.7 and are unwired. Fitting a
  section is not a requirement being met.

## Alternatives rejected

- **Leave v11.6 as the record and treat v12.0 as a draft.** v12.0 is a strict
  superset by the sentence comparison, so scoring against v11.6 would score
  against a target the owner has already moved past, and the matrix would be
  falsifiable against the wrong document.
- **Re-extract the whole catalogue from v12.0.** Almost every sentence is
  identical, so this would re-issue 1,566 IDs that commits and work items
  cite, for no change in meaning. Additive entries under `M12` and `G3` keep
  the IDs stable.
- **Treat the owner's instruction as covering the boundary.** See Status
  and C1.
- **Adopt GCP v3.0's services as placements.** There is no cost figure to
  place them against, no resource for any of them in Terraform, and none of
  the stores has a record. A placement without a cost is a plan that cannot
  be refused.
- **Resolve C12 by trusting the Model-Risk Brain's veto.** A model that can
  reduce exposure outside the Gate is a model in the pre-trade path, and
  `Determinism::Required` exists to make that unrepresentable.

## Reversal cost

Low, and deliberately so. Adoption in direction creates a catalogue, not
behaviour. Reversing it means marking the `M12` and `G3` requirement entries
`withdrawn` with a reason (never deleting them, since a work item may cite
one) and restoring v11.6 and v2.1 as the reference by a record that says so.
No dependency, resource, binary or contract changes on adoption, so none
unwinds. The cost grows with later records, not with this one: a contract
decision under C14, or a store under C13, would be reversible only through
its own record.

## Validation

How a reader checks that this record did what it says, and not more:

- `every_internal_link_resolves` in `qip-acceptance`'s `documentation.rs`
  holds the index's and this file's links. No test yet checks that the index
  has a row for every ADR number; that guard is not written, and none is
  cited here.
- `sha256sum docs/blueprint/source/*` against `docs/blueprint/README.md` for
  the v12.0, v3.0 and diagram hashes.
- The paper boundary is checked by `paper_boundary` in `qip-acceptance`, and
  by `git diff` showing no change to `infrastructure/terraform/variables.tf`
  or to `AutonomyLevel::deployable` from this record's commits.
- The source-code mapping: `grep -n '"M12"\|"G3"'
  backend/crates/apps/qip-cli/src/blueprint.rs` must print the two `cite()`
  arms, and it does so only once the pending working-tree change to that file
  is committed (see Decision 2). Until then this check fails, by design.
- No dependency and no provisioned resource: `./scripts/check-dependencies.sh`
  must say "all permitted" (the enforced fact is that line, which on
  2026-10-04 read "11 third-party package(s)", not a count of two), and `execution_nodes = {}` must still hold in
  every environment.

## What would make this wrong

- **Any live-order, live-transfer, purchasing, market-creation or
  public-communication path appearing** on the reasoning that v12.0 calls for
  one, or that the owner said "just perform the work". Both inferences are
  invalid here.
- **A capital, hedge or compiler item built beyond shadow form** (C9), or a
  "microstructure probe" that touches a live venue.
- **A model-risk output that can widen anything, or act without the Gate**
  (C12). That would be `MaxExpectedShortfall` in a new place, a control that
  reads as protection and is not.
- **A store, accelerator, agent runtime or cluster appearing in the tree
  before its record.** That would mean this record was read as reopening ADR
  0002, 0009 or 0024.
- **A cost appearing without an owner ceiling** (C11).
- **Two live registers.**
- **A `M12` or `G3` entry marked COMPLETE on the strength of a file existing.**
- **A v12.1 or v3.1 arriving.** The next version does not become the
  architecture of record by existing. That takes the owner, again.

## Review disposition

Reviews: `docs/blueprint/reviews/0101-review-a.md` (A1-A14) and
`docs/blueprint/reviews/0101-review-b.md` (B1-B6). Each claim was re-checked against
the tree on 2026-10-04 before editing.

| Item | Disposition | Why |
|---|---|---|
| A1 | applied | `CapitalGrant` in `capital.rs` is an admission outcome; `CapitalGrantFrame` wraps `CapitalEnvelope`. C14 rewritten. |
| A2 | applied | `grep -rn the_adr_index backend` is empty; only `every_internal_link_resolves` exists. Validation says the index guard is unwritten. |
| A3 | applied | Shadow and paper defined; C9 assigns one per item. |
| A4 | applied | Compiler moved to C9a under C5, ADR 0083, ADR 0008, `Determinism::Required`; `risk-and-execution.md` line quoted. |
| A5 | applied | C12 expanded (a)-(e); RISK-020 is a requirement (`RISK.json`), now labelled so; ADR 0005 and the correct rule files named. |
| A6 | applied | The delta records the §8 rename and says "apart from page-break shifts". Count wording fixed (23 includes §24.9 and §31.1: 9+5+3+3+1+1+1). |
| A7 | applied | "roughly" in Context and cost; the delta says approximate. |
| A8 | applied | Rule cited to `HERMES_MISSION.md` s2.4 and U24, with its stronger wording; Z3, OR-Tools, prost, Raft, Qiskit version added; TPU7x marked unproven in C5. The suggestion to move the ~25-name list to the delta files is **rejected**: the list is the guard an ADR reader sees. |
| A9 | applied | `v12-delta-gcp.md` line 25 calls 25 USD/day a judgement; stated as the delta's assumption, not an owner figure. |
| A10 | applied | Relaxation is a rules-file amendment, not C1; carrying mechanism named (an `M12` entry; its ID is not invented here). |
| A11 | applied | ADR 0007 and 0005 added to C13; synthetic capital never feeds an envelope or grant. Initial status of `M12`/`G3` entries added to Decision 4. |
| A12 | applied | ADR 0099's ADR 0089 slip is not copied; 0100 moved out of the prohibition column; C7 reworded (ADR 0100 itself says it applies 0091's test). |
| A13 | applied | Related list reconciled with the register; ADR 0023 described as it reads. ADR 0081 dropped from the register-driven list (no row uses it). |
| A14 | applied | Reach stated once in Status; the two restatements trimmed. |
| B1 | applied in part | The reviewer's grep is wrong for this working tree: `blueprint.rs` lines 365-366 map `M12` and `G3`. But that is an uncommitted modification (`git status`: ` M`), so "already maps" was false at the last commit. Decision 2 and Validation now say so; the lane that owns `blueprint.rs` still has to commit it, and this record did not touch the file. |
| B2 | applied | Same as A2. |
| B3 | **rejected** | `docs/adr/README.md` line 141 already holds a 0101 row in this tree (`grep -n 0101` finds it). The row's wording was brought into line with this revision. |
| B4 | applied | See A9; the monthly/daily mismatch with ADR 0098 stated. |
| B5 | applied | C11 states zero cloud spend until an owner ceiling exists. |
| B6 | applied | Validation says the enforced fact is `all permitted` (11 third-party packages on 2026-10-04). |

Counts: 20 items; 19 applied (B1 in part), 1 rejected (B3), plus one rejected sub-suggestion inside A8.
