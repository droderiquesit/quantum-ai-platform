# GCP blueprint v3.0 against v2.1: the deployment delta

Sources: `source/algorik-gcp-platform-blueprint-v3.0.txt` against `...-v2.1.txt`
(both `pdftotext -layout`; page numbers are the `=== page N ===` markers in the
v3.0 file). Adoption of v3.0 is pending ADR 0101; nothing here authorises
anything (ADR 0099, "What this does not do").

## Method and what the diff actually is

The diff is **additive**. Lines removed from v2.1 are only page headers and
the old title/closing paragraph; no v2.1 table row, region count, service or
rule was deleted or reworded. v3.0 adds roughly 440 lines in nine places. Every
v2.1 deployment element (three execution regions, C4D/C4 Reflex VMs, five
broker VMs per region, regional GKE Standard, Config Sync/Argo CD/Kargo/Rollouts,
Cloud Service Mesh, Spanner Enterprise Plus, Spanner Graph, Bigtable, BigQuery,
AlloyDB, Memorystore, Knowledge Catalog, NCC, `prod` live execution, seven repos)
is carried unchanged, and ADR 0099's register C1-C8 already covers it. This file
lists only what v3.0 adds. Where a v3.0 row merely restates a v2.1 element, the
row says "restated" and points at the existing C-number.

**Cost.** v3.0 contains no dollar figure and no unit price anywhere (grep for
`$`, `USD`, "cost": only §23 FinOps process text and §9 "lowers cost"). So every
"Blueprint cost" cell below is "none stated". No estimate is invented here.

**25 USD/day ceiling.** "Fits" means the element can run at a scale that a
single dev project plausibly holds under that ceiling, judged only from what
the blueprint says about always-on fleets, reservations and accelerators and
from what Terraform contains today. It is a judgement, not a quote, and no price
list was consulted.

## What Terraform holds today (the baseline the conflicts are against)

- One region everywhere: `region = "us-east4"` (dev); every trust zone is
  `us-east4`. `execution_nodes = {}` in all environments; the execution-node
  module accepts only `c3-highcpu-8/22` and `c3d-highcpu-8/16`
  (`modules/execution-node/variables.tf`), not C4/C4D.
- No GKE Standard. The only `google_container_cluster` is the Autopilot
  control-plane cluster in `modules/gitops-control-plane`, behind
  `gitops_enabled` and suspended on cost by ADR 0093.
- `modules/data` has Bigtable, Spanner (`regional-<region>` config, not
  Enterprise Plus multi-region), AlloyDB, Memorystore, BigQuery and Cloud
  Storage resources, each behind an `enable_*` flag that is `false` (dev
  tfvars show `enable_bigquery/cloud_storage/alloydb/bigtable = false`).
- `modules/ai` has a Vertex metadata store and endpoint behind
  `enable_vertex_ai`. No GPU, TPU, accelerator or node-pool resource exists
  anywhere in `infrastructure/` (grep for `gpu|tpu|accelerator` finds only prose
  in `variables.tf`, `main.tf` and some tests).
- No Cloud Run Jobs, Agent Engine, Scheduler, Workflows, Dataflow, Dataproc,
  Service Directory, Cloud Interconnect to a provider (only one
  `google_compute_interconnect_attachment` in `modules/connectivity`), or CAS
  resource.
- Standing decisions: ADR 0024 (no GKE; Cloud Run for warm binaries, one
  execution node per region), ADR 0091 (five binaries, amended to seven by ADR
  0100), ADR 0093 (control-plane cluster suspended on cost), ADR 0099 C2-C8,
  ADR 0069 (no capability without a consumer), paper trading (ADR 0003/0021).

## Delta table

Abbreviations for conflicts: **0024** no GKE; **0091** five-binary composition;
**0093** cost suspension; **C2** dependencies/async runtime/TLS/Raft; **C3**
GKE + GitOps stack; **C4** managed data stores; **C5** Python/JAX/PyTorch/Z3;
**C7** per-brain services; **C8** regions/VMs/cost; **TF** Terraform today.

| # | v3.0 page / section | Element and statement | Conflicts | Blueprint cost | 25 USD/day |
|---|---|---|---|---|---|
| 1 | p2 §1 (new paragraph) | A "Superintelligence Plane" above the three communication paths: latent-state estimation, forecast lattices, model/agent societies, digital twins, active sensing, meta-intelligence, hybrid CPU/GPU/TPU/QPU routing, capital/hedge intelligence. "These services scale independently." | 0091 (independent scaling asserted, not measured), C7, ADR 0069. Product-direction rule 4: no guarantee weakened. | None stated | Framing only; see rows 2-9. |
| 2 | p12 §11.2 | NOW services: `now-state-coordinator` (GKE persistent + Bigtable/Spanner Graph), `state-filter-workers` (GKE CPU/GPU, HPA by lag), `state-reconciler` (GKE + Spanner Graph, leader with fencing). | 0024, C3, C4 (Bigtable, Spanner Graph), C7, TF: no GKE, `enable_bigtable=false`, no Spanner Graph resource. | None stated | No as specified (persistent GKE + GPU pool + two managed DBs). A library crate in an existing Cloud Run binary on in-tree storage fits. |
| 3 | p12 §11.3 | Forecast lattice: `forecast-router` (GKE), `forecast-micro` (Reflex + warm GKE), `forecast-tactical` (GKE GPU/CPU), `forecast-strategic` (Vertex/GKE/**TPU**), `forecast-calibrator` (GKE + BigQuery). Horizons microsecond to years. | 0024, C3, C4 (Bigtable/BQ/Spanner Graph/GCS), C5 (JAX/PyTorch on TPU), C7. `forecast-micro` on a Reflex VM touches the execution node, whose module allows only C3/C3D. | None stated | No for tactical (GPU) and strategic (TPU); calibrator and router could be in-process. |
| 4 | p12-13 §11.4 | Model and agent society: Specialist Agents on **Vertex AI Agent Engine and/or GKE**; Model Tournament; Forecast Market ("synthetic reputation/capital"); Adversarial/Verifier Society; Curiosity/Active Sensing (InformationRequest to Scout Fabric); Meta-Intelligence (ComputePlan/AgentBudget/ModelWeight). | 0024, C3, C7, ADR 0083 (in-process inference). Agent Engine is a new managed runtime with no Terraform precedent (TF has only `enable_vertex_ai` metadata store/endpoint). "Forecast Market" with synthetic capital must stay synthetic: not a trading venue (non-goal). | None stated | Agent Engine: depends on per-session pricing the blueprint does not give; cannot be confirmed. GKE persistent agents: no. |
| 5 | p13 §11.5 | Digital-twin fabric: `world-branch-manager` (GKE + Bigtable), `market-digital-twin` (GKE Jobs/Batch/GPU), macro/company twins (GKE/Vertex/**TPU**), `rare-event-factory` (**Batch/Spot GPU/TPU**), `counterfactual-portfolio-farm` (GKE Jobs + BQ/GCS, "massively parallel"). | 0024, C3, C4, C5; "Batch" is a new Cloud Batch dependency no ADR covers. The platform already scores counterfactuals in `qip-kernel` LEARN; the farm is a different runtime, not that code. | None stated | No when GPU/TPU or "massively parallel" is taken literally. A bounded CPU replay inside an existing binary fits. |
| 6 | p13 §11.6 + fig. | Hybrid compute fabric: **TPU7x (Ironwood)** via GKE or Compute Engine ("reserved slices/AI zones; JAX/PyTorch"); GPU pools (Vertex custom training + GKE GPU, Spot); CPU/Rust (Compute Engine + GKE; HPA/MIG or fixed reservations); quantum-inspired classical baselines (GKE/Batch CPU/GPU); IBM QPU via quantum gateway; a **Compute Intelligence Router** that "learns which substrate gives the best verified result". | TPU and GPU: no resource exists (TF); C5 (JAX/PyTorch); C3. QPU: consistent with ADR 0006 (classical baseline always) and with the existing IBM path; the gateway over "controlled egress" needs the egress proxy, which is applied nowhere (`data-and-streaming.md`). The router is an Algorik concept, not a GCP product. | None stated | **TPU7x: no.** Reserved slices are the opposite of a daily ceiling and the blueprint says so ("reserved slices/AI zones"). GPU: only a Spot job run on demand, not a pool. QPU via IBM: the cost is IBM's and not stated. |
| 7 | p13-14 §11.7 | Capital/hedge/survival services on regional GKE: `capital-society-coordinator`, `survival-kernel` (+ deterministic policy), `hedge-brain` (GKE CPU/GPU + hybrid solver router), `model-risk/confidence-governor`, `shadow-portfolio-farm`. Authoritative dependency: Spanner capital grants/ledger. | 0024, C3, C4 (Spanner), C7. **Paper boundary:** "CapitalGrant", "capital grants" distributed to regions and "unwind plans" are LIVE_CAPITAL-flagged in ADR 0099 unless they stay shadow/simulated; the survival kernel "reserving cash/margin" is meaningful only against real money. Risk domain rule: no model output escalates autonomy. | None stated | Only as simulated/shadow logic in a binary that already runs; not as GKE services. |
| 8 | p14 §11.8 | Memory boundaries: Spanner Graph (promoted knowledge), Bigtable (ambient state, reputation, scores, branch indexes), BigQuery (resolved forecast outcomes), GCS, and **Agent Engine Memory Bank** or custom memory services. | C4 for each store. Agent Engine Memory Bank is a new managed store. "No second source of truth for a fact the event log already holds" (boundaries rule) applies to forecast outcomes and scores. | None stated | As row 4; Memory Bank price unknown. |
| 9 | p15 §13.1 | Autonomous development "at superintelligence scale": hundreds/thousands of ephemeral agent workers; sandboxes on **Cloud Run Jobs**, GKE Jobs or build workers; Agent Orchestrator DAG; per-role budgets. | 0024 allows Cloud Run, but no Cloud Run Jobs resource exists (TF). Parallel agents share one checkout here (`00-enterprise-governance.md`); the blueprint's worktree isolation matches ADR 0098 but not at that concurrency. Model-gateway "OpenAI/other" is a vendor call needing egress approval and licensing check. | None stated; §23 only says agents "get explicit daily/monthly model/build/tool budgets". | Unbounded by the text; fits only if the agent budget is set by the owner below the ceiling, and the blueprint gives no number. |
| 10 | p20-23 §19 and new §19.0A-B "Service-by-Service Production Architecture" | A second, per-service catalogue. New or newly explicit entries versus v2.1: **Cross-Cloud Interconnect** (p21, "NCC <-> provider/colo/IBM/cloud networks"); **Persistent Disk / local journal disks** for Fabric (p22; v2.1 §7.3 and §19 say Hyperdisk, so v3.0 now names two disk products for the same job); **Vertex AI Agent Engine**; **TPU7x Ironwood**; **GKE GPU pools**; **GKE/Batch CPU pools**; **Quantum Gateway + IBM Quantum**; **Cloud Scheduler/Workflows (peripheral only)** (p23). Rule: "Services that do not provide a distinct capability should not be added." | Each new service needs its own ADR under ADR 0069 / ADR 0099. Cross-Cloud Interconnect to IBM: network contract with a third party, never applied. Scheduler/Workflows is explicitly not the backbone, consistent with ADR 0100. | None stated | Interconnect, TPU, GPU: no. Scheduler/Workflows: tiny but unbuilt; not needed under the ceiling. |
| 11 | p24 §20.1 | Superintelligence connection matrix (10 edges): Scout/Evidence to NOW Brain over Rust Fabric; Forecast Router to Model Society; Meta-Intelligence to Compute Router; **Compute Router to TPU7x/GPU/CPU/IBM QPU over "native service APIs / controlled egress"**; Cognitive Compiler to Artifact Registry/GCS as signed OCI package. | C2 (every Fabric edge assumes QUIC/mTLS/prost; ADR 0100 ships single-node std-TCP). The Cognitive Compiler produces executable Reflex packages: must stay shadow because it moves a model into a live-capable path (paper layers 2 and 3 still bind). | None stated | Not a deployment element on its own. |
| 12 | p25-26 §21.5 | Forecast-to-capital flow (8 steps) ending in bounded CapitalGrant/RiskEnvelope/HedgePlan, Cognitive Compiler distillation, resolved outcomes scoring "compute-routing choices". | Paper boundary for step 6 (see row 7). Compatible with "pre-trade checks never route to a model" only because step 7 keeps live execution local and deterministic; any shortcut from forecast to sizing without the deterministic gate would breach `Determinism::Required`. | None stated | Flow, not a service. |
| 13 | p28 §26.1 | Build phases 11-16 (NOW + Forecast Lattice, Model Society, Digital Twins, Hybrid Compute with "GPU/TPU7x pools", Money Intelligence, Meta-Intelligence with "compute/attention market, recursive specialist creation and cognitive compiler"). | Sequenced after phases 0-10, which in this repository are mostly unbuilt or BLOCKED (C8: billing disabled on dev; `execution_nodes = {}`). Phase 14 is where TPU/GPU appear, so the first accelerator spend is at the end of the plan, not the start. "Recursive specialist creation" has no counterpart control in the rules. | None stated | Phases 11-13 and 15 could start as simulated logic; phase 14 pools cannot. |
| 14 | p29 §27.1 | Seven new acceptance criteria (forecast has horizon/distribution/calibration/disagreement/lineage/expiry; model influence earned from resolved performance; no TPU/GPU/QPU/agent/twin job is a synchronous dependency between tick and order; etc.). | None; every one is compatible with the repository's rules and reproducible-from-log principle. Listed because ADR 0099 scores the "COMPLETE TARGET" lines. | None stated | Not a deployment element. |
| 15 | p30 §29.1 | "Validation" notes: TPU7x (Ironwood) is GA through GKE or Compute Engine; Agent Engine Sessions and Memory Bank are GA with A2A, bidirectional streaming and sandboxed code execution; Spanner Graph with GQL; IBM 2026 roadmap and "quantum-centric supercomputing reference architecture (March 2026)". | See "Products that may not exist" below. | None stated | n/a |

Also changed but not a deployment element: title and companion line (p1),
and the §1 "Data / Intelligence" and §4 `prod/ai` rows are unchanged from v2.1
(both already said "GPU/TPU jobs"), so GPU/TPU as such is **not** new in v3.0.
What is new is **TPU7x by name**, a router over the substrates, and the
services that consume them.

## Elements restated, not new

Same statement as v2.1, same conflicts: three execution regions and exact
regions as a measurement outcome (p4 §3, C8); NCC latency mesh and star hub
(p5 §5, C3/C8); C4D/C4 Reflex cell (p6 §6, C8, TF accepts only C3/C3D);
Rust Fabric components, five brokers per region, RF3, QUIC, Raft, prost
(p7-9, C2, ADR 0100); regional GKE Standard and the Config Sync/Argo/Kargo/Rollouts
split (p9 §8, p16 §14, C3, 0024, 0093); Spanner Enterprise Plus ledger (p10-11,
C4); `prod` "live bounded execution" (p27 §24, C1); seven repositories (p15-16,
C6, declined); Cloud Run as the burst endpoint (p9, allowed by 0024).

## Products and claims that may not exist or cannot be verified

I have no network access in this task and my own knowledge does not cover
late 2026, so these are "unverified", not "false".

1. **TPU7x (Ironwood), "GA ... through GKE or Compute Engine"** (p13, p30).
   The blueprint asserts GA and a "TPU7x" shape name; no Terraform or ADR
   corroborates it. Confirm the machine/slice type name and region availability
   before anything cites it.
2. **"Gemini Enterprise Agent Platform"** (p2, p11, p21, p30) as the branding
   for Vertex AI. The blueprint itself warns the branding "is evolving" and
   says to keep Vertex AI APIs behind adapters; treat the name as unstable and
   cite the API.
3. **"Vertex AI Agent Engine" Sessions, Memory Bank, A2A, "sandboxed code
   execution", bidirectional streaming** (p14, p30). Plausible product family;
   each feature's GA status is the blueprint's claim only.
4. **"Knowledge Catalog (formerly Dataplex Universal Catalog)"** (p30). A
   rename asserted by the document; unverified. (Present in v2.1 too.)
5. **"IBM quantum-centric supercomputing reference architecture (March 2026)"**
   and "IBM 2026 roadmap" (p30). Cannot be checked from the tree.
6. **"Cloud Batch", "Cross-Cloud Interconnect to IBM"** (p13, p21). "Batch" is
   used as a product without a name; Cross-Cloud Interconnect connects to other
   clouds and large providers, and whether IBM Quantum's service endpoints are
   reachable that way is an assumption, not a documented fact.
7. **Not products, Algorik-internal names** the blueprint lists beside GCP
   services: Compute Intelligence Router, Cognitive Compiler, Forecast Market,
   NOW Brain, Quantum Gateway, Reflex Mesh, "compute/attention market". None
   has a GCP API; each is a thing to build, and ADR 0099 C5 applies to any that
   is Python/JAX.

## Top conflicts, in priority order

1. **Accelerators (TPU7x, GPU pools, Spot GPU/TPU)** vs the 25 USD/day ceiling
   and ADR 0093: reserved slices and pools are the opposite of a bounded daily
   spend, and none exists in Terraform (rows 3, 5, 6, 13).
2. **A second and larger warm-service population on GKE** (about 25 named
   services across rows 2-9) vs ADR 0024/0093/0091/C3/C7: GKE Standard
   is retired in this repository, its replacement cluster is itself suspended
   on cost, and ADR 0091 holds that independent scaling must be measured first.
3. **Paper-trading boundary in the capital/hedge/survival services and the
   Cognitive Compiler** (rows 7, 11, 12): capital grants, unwind plans and
   executable Reflex packages are LIVE_CAPITAL-adjacent; they may be built only
   as shadow/simulated logic (ADR 0099, ADR 0021).
4. **Managed data and agent stores as the memory of the intelligence plane**
   (Spanner Graph, Bigtable, BigQuery, Agent Engine Memory Bank; rows 2-5, 8) vs
   C4/ADR 0002/0009 and "no second source of truth for what the event log
   holds": no store here has a record, and Terraform holds them all behind
   `enable_*` flags that are false.
5. **Python/JAX/PyTorch research and Vertex/Agent Engine runtimes** (rows 3-6)
   vs C5/ADR 0001/0083, plus the C2 Fabric stack (Tokio, QUIC, prost, Raft) that
   every edge in the §20.1 matrix assumes while ADR 0100 builds a single-node
   std-TCP broker.

## Counts

- Delta table rows: 15. Rows 2-9 (8) describe deployable service groups; rows
  1, 11-14 (5) are framing, flow, phase or acceptance text with no resource of
  their own; rows 10 and 15 (2) are catalogue and validation notes.
- Named new services: 27 (3 NOW, 5 forecast, 6 society, 5 twin, 5 capital, 3
  cross-cutting: router, compiler, gateway).
- Cost figures stated in v3.0: 0. Rows that fit 25 USD/day as specified: 0;
  that fit only as simulated in-process logic: rows 2, 3 (calibrator/router),
  5 (bounded CPU replay), 7, 13 (phases 11-13, 15).
