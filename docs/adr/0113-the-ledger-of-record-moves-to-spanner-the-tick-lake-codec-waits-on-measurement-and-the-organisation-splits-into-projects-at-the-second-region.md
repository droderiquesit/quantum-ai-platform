# ADR 0113: The ledger of record moves to Spanner, the tick-lake codec waits on measurement, and the organisation splits into projects at the second region

- **Status**: *Proposed*, 2026-10-11. The owner accepts in writing; nothing
  here takes effect by being written or is provisioned by it. **No longer
  blocked on the dependency rule** (ADR 0109, accepted 2026-10-11). Blocked
  on its own acceptance, on ADR 0112 (TLS to Google APIs) and, for every
  apply, on billing being enabled and an owner cost ceiling (ADR 0101 C8, C11).
- **Date**: 2026-10-11
- **Resolves**: owner decisions D5, D6 and D7 in
  `docs/blueprint/v12-completion-plan.md`; ADR 0101 conflict C4 for the
  ledger ("a storage record per store, ledger first").
- **Amends, on acceptance**: ADR 0100 §3–§4's placement of the ledger's
  durable state in a local `DurableStore`; ADR 0105's single `WriteBatch`
  becomes a single Spanner transaction with the same contents.
- **Keeps**: `qip-ledgerd` as the sole writer of the ledger (ADR 0100 §2);
  ADR 0105's chain-continuity dedup; ADR 0106's refusal of `simulated=false`;
  "no second source of truth for a fact the event log already holds".

## Context

`qip-ledgerd` keeps balances, chain tails and consumer offsets in the in-tree
`DurableStore` on one local disk (R12-LEDGER-8007). Terraform declares a gated
`google_spanner_instance.global` (`modules/data/main.tf`) with config
`nam-eur-asea`, 100 processing units and no edition, and a `positions`
database — and no Rust adapter (R12-LEDGER-8132, 8133, 8136, 8137). Bigtable
and AlloyDB are declared and gated, and the Rust provider refuses both targets
(R12-DATA-8016, R12-DATA-8130). Tick-lake segments have no compression codec
(R12-TICK-2057, BLOCKED: "a codec (dependency ADR or in-tree implementation)").
Every environment is one project (`var.project_id`), with no folder, no
per-domain project and no Network Connectivity Center (R12-GCP-8043–8051,
8056–8058).

## Decision

### 1. D5 — the ledger of record is Spanner, written only by `qip-ledgerd`

- `qip-ledgerd` keeps its role and its logic; its `LedgerStore` gains a
  Spanner implementation behind the existing store port. Balances, chain
  tails and consumer offsets commit in **one read-write transaction**, the
  direct translation of ADR 0105's single `WriteBatch`. Dedup by chain
  continuity is a read inside that transaction.
- **Placement**: the ledger lives in a **multi-region** instance (the
  existing `nam-eur-asea` declaration is the candidate), because a global
  book must survive a regional loss and the ledger is off every hot path.
  The fabric's P0/P1 tables (ADR 0111) live in **regional** instances beside
  each region's fabricd, because their commit latency is on the produce path.
  The edition, processing units and `default_leader` are set in the change
  that applies it; this record does not assert which edition a multi-region
  configuration requires — that is checked against Google's documentation at
  plan time.
- The local `DurableStore` remains for tests and for a single-node
  development run, behind the same port. It is not a second ledger in a
  deployment: a deployed `qip-ledgerd` that cannot reach Spanner refuses to
  start rather than falling back.
- **Bigtable, AlloyDB and Spanner Graph are not admitted by this record.**
  Each is a store with its own record under C4 when a workload measures a
  need for it. One managed store first.

### 2. Google APIs are reached by REST, not by an SDK

Spanner, KMS (ADR 0110), CA Service (ADR 0112) and Cloud Storage are called
through the in-tree REST adapter pattern already in `qip-storage/src/gcp/`:
JSON bodies over `qip-transport`'s client and `rustls`, bearer tokens from
the metadata server. Nothing in that path is an auth protocol or a
cryptographic primitive; it fails loudly, so ADR 0012 condition 1 says
in-tree. A Google client SDK is admissible since ADR 0109's Amendment 2 and is
not chosen: the adapter exists, and an SDK's transitive tree is larger than
the problem.

### 3. D6 — the tick-lake codec is admitted on measurement, and `zstd` is the candidate

No codec is admitted by this record. Once W1.13 lands tick capture into
`qip-storage::lake`, a committed measurement of bytes per day per region,
uncompressed, against the archive's cost decides it. If compression is
warranted, `zstd` (the `zstd` crate over C `libzstd`, frame checksums on) is
admitted by ADR 0109's measured-cost route, because the lake is read by tools
outside this repository and a standard frame format is the point; a private
in-tree codec would compress and be unreadable to BigQuery and Dataflow. The
segment header's existing encoding byte (ADR 0100 §1, `PayloadCodec`) carries
the codec, so uncompressed and compressed segments coexist and replay reads
both.

### 4. D7 — a multi-project organisation, at the second region

- **Now**: nothing changes. One project per environment.
- **Trigger**: the change that adds a second execution region, or the first
  real-money preparation under ADR 0108 Stage 1 — whichever is first.
- **Shape at the trigger**: a folder per environment; within it separate
  projects for *security* (KMS key rings, CA pools, the Binary Authorization
  attestor), *data* (Spanner, the archive and lake buckets), *execution*
  (one per region: Compute Engine nodes, regional fabricd), *services* (Cloud
  Run) and *engineering* (Artifact Registry, CI identities, the control-plane
  cluster of ADR 0036). VPC Service Controls perimeters around *security*
  and *data*. Network Connectivity Center joins the per-zone VPCs, keeping
  the acceptance suite's refusal of Shared VPC.
- **Why at the second region and not now**: one region in one project has
  one blast radius already; the split's value is that a compromise in one
  region's execution project cannot read another region's or the ledger's
  data, and that value starts when there is a second region. Doing it now
  doubles the Terraform surface while nothing is deployed.

## Consequences

- An acknowledged posting survives the loss of a disk, a zone or a region.
- The ledger's correctness logic (ADR 0105, ADR 0106) is unchanged and is
  tested against both store implementations.
- The REST adapter becomes the one way the platform calls Google, and the one
  place its timeouts, retries and token refresh are reviewed.

## What it costs

- Spanner multi-region capacity is the largest standing cost this platform
  would carry (unpriced here; `dev` billing disabled).
- A deployed ledger that cannot reach Spanner stops posting; outcomes queue in
  the fabric (P1, never dropped) until it returns.
- The project split, when it comes, is a large Terraform migration with
  `moved`/`import` work across every module.

## Alternatives rejected

- **AlloyDB (PostgreSQL) for the ledger.** Regional with cross-region
  replicas that are asynchronous; a regional loss can lose acknowledged
  postings unless synchronous options are paid for and proven.
- **Keep the local `DurableStore` and replicate it.** That is in-tree
  replication, which ADR 0111 rejects.
- **A Google SDK for Spanner.** See section 2.
- **Split projects now.** See section 4.
- **An in-tree compression codec.** Unreadable by the lake's other readers;
  permitted, and the wrong trade.

## What would make this wrong

- A deployed `qip-ledgerd` posting to the local store, or two writers of the
  ledger.
- A posting acknowledged and later missing after a regional failover.
- `zstd` admitted without the committed measurement.
- A second region deployed in the single project.
- Bigtable or AlloyDB adapters appearing on the strength of this record.

## Dependency direction

The ledger's posting logic stays in `qip-portfolio` (pure double-entry). The
store port is a trait the ledger owns; the Spanner implementation and the
token source live in `apps/qip-ledgerd`, which composes them. The REST shim's
transport is `qip-transport` (the named socket lib); the JSON request types
may live in `qip-storage`'s existing `gcp` module, which already performs its
I/O through `qip-transport`. `zstd`, if admitted, is depended on by
`qip-storage` only — not by any decision-core crate. No lib depends on a
service; no service on the runtime; nothing on an app.

## R12 ids it unblocks

D5: **R12-LEDGER-8007**, **R12-LEDGER-8132**, **8133**, **8134**, **8136**,
**8137**, **8138**, **8139**, **8140**, **R12-DATA-8014** (registry on
Spanner). Not unblocked by decision: **R12-DATA-8015** (Spanner Graph),
**R12-DATA-8016** (Bigtable), **R12-DATA-8130** (AlloyDB) — each needs its
own store record. D6: **R12-TICK-2057**, after its measurement. D7:
**R12-GCP-8043** to **R12-GCP-8051**, **8052**, **8053**, **8056**, **8057**,
**8058**, at the trigger in section 4.

## The paper-trading boundary

Untouched. Moving the ledger's store moves where postings live, not what may
be posted: ADR 0106's refusal of `simulated=false` is in `qip-ledgerd`'s
logic and is tested against the Spanner store as against the local one. The
three original fences and `Determinism::Required` are unchanged; ADR 0108
governs any move beyond paper.

## Gates

None run: no shell in the session that wrote this record.
