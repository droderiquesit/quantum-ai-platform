# ADR 0099: Blueprint v11.6 and GCP v2.1 are the architecture of record in direction, and every standing decision they contradict keeps its force until its own record

- **Status**: Accepted **for the adoption**, on the owner's instruction of
  2026-09-25. On that date the owner supplied the three documents below and
  directed the repository to be driven "from its current state to the
  blueprint target state." **Proposed for everything else.** Each conflict in
  the register below is resolved by its own later record, or is not resolved.
- **Date**: 2026-09-25
- **Supersedes**: ADR 0022's choice of *which* blueprint is the reference, and
  nothing else in ADR 0022. Its "What this does not do" and its paper-trading
  section are carried forward below, not replaced.
- **Related**: ADR 0003, ADR 0021, ADR 0023 (paper trading; untouched). ADR
  0001, ADR 0002, ADR 0009, ADR 0011, ADR 0016, ADR 0024, ADR 0036, ADR 0081,
  ADR 0091, ADR 0093 (standing decisions the new target contradicts; see the
  register). ADR 0098 (the development factory that carries the programme).

## Context

ADR 0022 made Master Blueprint v10.1 the architecture of record. Under "What
would make this wrong" it said: *"The blueprint being revised or replaced… the
next version does not automatically become the architecture of record — that
takes an owner, here."*

On 2026-09-25 the owner supplied a revised architecture and directed the
repository toward it. It comes as three documents, now held verbatim in
`docs/blueprint/source/`, with their SHA-256 in `docs/blueprint/README.md`:

- **Algorik Master Architecture & Application Blueprint v11.6**, "Pass-Through
  World Data + Native Rust Event Fabric + durable knowledge memory". 45 pages.
  Its running header says v11.5, and its title and body say v11.6. This
  record uses v11.6.
- **Algorik GCP Full Platform Architecture Blueprint v2.1**, the companion
  cloud/platform blueprint. 24 pages. Its running header says v2.0 and
  "companion to v11.5", and its title says v2.1. This record uses v2.1.
- **Algorik Full Platform Architecture v2.1**, an interactive diagram with
  eight views over the same system.

v11.6 describes its own relationship to v10.1 as "architectural rather than
cosmetic" in its §2 correction table. Intelligence becomes a federation. The
regional cells stop being isolated and gain a peer Reflex Mesh. Quantum grows
from an allocation optimiser into a benchmark-gated Quantum Foundry. Research
may leave Rust ("Rust-first execution, not Rust-only research"). A native Rust
Event & Control Fabric replaces Pub/Sub and Kafka as the internal nervous
system. External world data becomes pass-through, and only derived knowledge
is durable.

## Decision

1. **v11.6 and GCP v2.1 are the architecture of record**, together with the
   diagram. Every architectural claim in this repository is scored against
   them. Where the two documents differ, v11.6 governs *what* the system does
   and v2.1 governs *where and how it runs on GCP*. Where v2.1 is more specific
   about a v11.6 requirement, the more specific statement is the requirement.
2. **Traceability is structural, not narrative.** The documents are broken
   into atomic requirements with stable IDs (`DOMAIN-NNN`) in
   `docs/blueprint/requirements/*.json`, rendered to
   `docs/blueprint/requirements.md`. Every ID gets exactly one row in
   `docs/blueprint/traceability-matrix.md`, and a row is COMPLETE only when
   its required behaviour is implemented *and* demonstrated by a named test or
   run. An acceptance test holds the matrix to the catalogue in the same way
   `the_delivery_status_scores_every_numbered_blueprint_section` holds
   `DELIVERY-STATUS.md` to v10.1.
3. **`docs/DELIVERY-STATUS.md` becomes the v10.1 historical register.** It is
   not deleted, it keeps its acceptance test while v10.1's source stays in the
   tree, and it gains a banner pointing at the live matrix. This repository
   collapsed nineteen status documents into one on 2026-09-07. Starting a
   second live register beside it would repeat the failure that collapse
   fixed, so there is one live register at a time, and from this record on it
   is the v11.6 matrix.
4. **v10.1 is superseded in direction only.** Its source stays in
   `docs/architecture/`. ADRs that were decided against v10.1 remain in force
   as descriptions of what runs today and as standing decisions, until a
   record supersedes each one.

## What this does not do

Carried forward from ADR 0022 because the hazard is the same, and it is
larger here: v11.6 specifies more machinery that this platform refuses than
v10.1 did.

**This decision authorises no execution whatsoever.** It settles what the
platform is aiming at. It does not provision, migrate, decommission, add a
dependency or open a path, and nothing in it may be cited as permission to.

- **A standing decision is not overridden by a target that contradicts it.**
  `.claude/rules/10-product-direction.md` says a standing product decision is
  reopened by an ADR, not by a code change. Adopting a document that describes
  a different system is not an ADR that reopens anything. Each conflict below
  gets its own record, with its own costs and reversal conditions, before any
  code acts on it.
- **Cloud work still goes through the gates it went through before.** ADR
  0040's workflow rule and cost posture apply. ADR 0093's owner cost
  instruction applies. `prod` is refused by the workflow and by the rules.

## The paper-trading boundary, stated separately because it matters most

v11.6 and v2.1 assume real capital and real external action:
- a `prod` environment for "live bounded execution" (v2.1 §24);
- autonomous treasury transfer inside signed corridors (v11.6 §13);
- custody and settlement agents (v11.6 §15);
- purchase executors for physical commerce (v11.6 §20);
- market creation (v11.6 §18.2);
- causal-agency action surfaces that publish and conduct outreach (v11.6 §24.4).

**None of that is authorised by this record.** ADR 0021 stands exactly as
written, and the three layers stay intact: Terraform's refusal in
`infrastructure/terraform/variables.tf`, `AutonomyLevel::deployable` in the
composition roots, and a `Cell` with no constructor taking a ceiling other
than paper trading. Every requirement that cannot be met without live capital
or external action carries the `LIVE_CAPITAL` or `EXTERNAL_ACTION` flag. It is
scored `BLOCKED` with ADR 0021 as the reason, and it is built only as far as
its shadow, paper or simulated form, which v11.6 itself asks for first:
"Begin shadow-only" (§30, Phase 10); "Research/simulation may be broader than
executable scope; unsupported opportunities remain shadow-only" (§25).

## The conflict register

Each row names a standing decision the new target contradicts. The standing
decision keeps its force until the record in the last column exists and is
accepted.

| # | The target says | The standing decision | Resolved by |
|---|---|---|---|
| C1 | Live bounded execution in `prod`; treasury transfer, custody, purchasing, market creation, public communications | ADR 0003, 0021, 0023; `01-security-and-safety.md` — paper trading is absolute | **Not an agent's decision.** Requires the owner to supersede ADR 0003 and amend the rules file. No record written here may do it. |
| C2 | Fabric on Tokio with QUIC/mTLS, Protobuf via prost, BLAKE3, a Rust Raft, and gRPC clients for Spanner, Bigtable and BigQuery | ADR 0002, 0009, 0012 — `serde` and `serde_json` only; no async runtime; no in-tree TLS | A dependency record per class (async runtime, transport security, wire schema, consensus, managed-service clients), each weighed against an in-tree or egress-proxy alternative. Until then the Fabric is built on what the workspace permits, and the gap is scored. |
| C3 | Regional GKE Standard for warm services; Config Sync, Argo CD, Kargo, Argo Rollouts, Cloud Service Mesh | ADR 0024 (Kubernetes retired for Cloud Run); ADR 0036 (Argo on a control-plane cluster only); ADR 0093 (that cluster suspended on cost) | A runtime record deciding whether warm services return to GKE, weighed against Cloud Run, which v2.1 itself allows "where stateless burst/serverless economics are beneficial" (v11.6 §26). |
| C4 | Spanner Enterprise Plus ledger, Spanner Graph, Bigtable, BigQuery, AlloyDB, Memorystore, Knowledge Catalog | ADR 0002/0009 (in-tree storage; two dependencies); ADR 0089 (file-backed log) | A storage record per store. The ledger comes first, because v2.1 builds financial truth in its Phase 4. |
| C5 | Python/JAX/PyTorch for research and training; Z3/OR-Tools for symbolic reasoning | ADR 0001 (Rust everywhere); ADR 0083 (in-process in-tree inference) | A research-runtime record. v11.6 keeps the hot lane Rust-first and bounded, so the conflict is confined to the slow lane. |
| C6 | Seven repositories (app, models, infra, platform-config, env, schemas, runbooks) | ADR 0016 — one repository, four domains | **Declined by this record.** v2.1 names the repositories, but nothing in either blueprint depends on the split. The monorepo keeps one gate, one history and one review surface, and each named repository maps to a directory. Recorded as a deliberate divergence. |
| C7 | Separate warm services per brain (Asset, Capital, Risk, Evidence, …) | ADR 0091 — five binaries composed from library crates | A runtime record, taken with C3. v2.1 describes placement, not a process count, and a brain can be a crate in a binary until independent scaling is *measured* to require otherwise. |
| C8 | Three execution regions; dedicated C4D/C4 Reflex VMs; five Fabric brokers per region | ADR 0093's cost instruction; `execution_nodes = {}` in every environment | An owner cost ceiling, then a deployment record. **Also blocked externally today**: billing is disabled on the dev project, and `deploy.yml` fails at image push with "This API method requires billing to be enabled". |

## What it costs

- **A larger standing gap than ADR 0022 created.** v11.6 adds the Scout
  Fabric, Evidence/Truth, the Tick Lake, the World Model Federation, symbolic
  reasoning, the ambient mesh, causal agency, the expansion engine, physical
  commerce and the native Fabric. That is far more target than v10.1 held, and
  the matrix will say so in numbers. The honest completion percentage drops
  the day this lands, even though no code got worse.
- **Eight open conflicts, seven of them large.** Four need owner decisions:
  C1, the cost part of C8, and arguably C2 and C4, because they reopen the
  repository's defining dependency posture. Until those are decided, the
  programme builds each subsystem's semantics on what the workspace permits,
  and it scores the part that needs the refused thing as BLOCKED rather than
  quietly substituting.
- **Work already done is re-scored against a different target.** Some of it
  moves from "aligned" to "requires migration" without having changed.
- **The documents disagree with themselves about their version numbers.**
  Their headers carry v11.5 and v2.0. This record names v11.6 and v2.1 and
  keeps the hashes, so the ambiguity is pinned rather than inherited.

## Alternatives rejected

- **Score v11.6 inside `DELIVERY-STATUS.md`.** That document is tied by an
  acceptance test to v10.1's section numbering, and its rows score a different
  specification. Rewriting it in place would destroy the v10.1 history ADR
  0022 chose to keep, and it would make the test measure a document mid-change.
- **Treat adoption as resolving the conflicts.** That is the reading ADR
  0022's "What this does not do" exists to refuse, and it would let a PDF
  amend `01-security-and-safety.md`.
- **Adopt only v11.6 and treat v2.1 as advisory.** v2.1 carries the concrete
  acceptance criteria (§27), the degradation contracts (§22) and the
  anti-patterns (§28), which are the most checkable statements in either
  document. Leaving them out would make the matrix less falsifiable.
- **Wait for the gap analysis before adopting.** The gap analysis has to
  score against something. Scoring against v11.6 while v10.1 is still
  formally the reference would produce a matrix nothing is held to.

## What would make this wrong

- **Any live-order, live-transfer, purchasing, market-creation or
  public-communication path appearing** on the reasoning that the architecture
  of record calls for it. That inference is invalid here as it was under ADR
  0022.
- **A dependency, cluster or managed service appearing in the tree before its
  record.** That would mean this record was read as the reopening of ADR
  0002, 0009 or 0024, which it explicitly is not.
- **Two live registers.** If the v11.6 matrix and `DELIVERY-STATUS.md` both
  claim to be current, the 2026-09-07 consolidation has been undone.
- **A matrix row marked COMPLETE on the strength of a file existing.** The
  matrix exists to separate "a type is defined" from "the behaviour is
  demonstrated".
- **A v11.7 or v2.2 arriving.** The next version does not become the
  architecture of record by existing. That takes the owner, again.
