# ADR 0709: Batch reprocessing is a Rust Cloud Run Job that reads the archive and the warehouse only, and the Knowledge Catalog, Spanner Graph and Bigtable stay unadmitted

- **Status**: **Proposed 2026-11-16, for the owner.** Decision 2 names a billed
  Cloud Run Job, and decisions 3 and 4 touch billed stores the owner has
  already declined or left unfunded. This record writes no code, adds no
  dependency, touches no paper-trading layer and nothing in it was built or
  tested.
- **Date**: 2026-11-16
- **Rows**: R12-DATA-8128, R12-DATA-8131, R12-DATA-9052, R12-DATA-9053

## Context

Four DATA rows ask for managed stores and engines from the blueprint's durable
knowledge tiers. The owner's rulings of 2026-10-31 already settled the
direction of each: the Knowledge Catalog, Spanner Graph and Bigtable stay
declined under decision 5, and Dataflow and Dataproc stay out because ADR 0011
keeps everything in Rust. What the register lacks is the one record that says
so, states what each row's reopening needs, and gives the batch row its Rust
shape. ADR 0151 decision 1 forbids a producer with no consumer, and ADR 0113
section 1 requires a store record stating a measured need and a monthly
ceiling before a store is admitted. ADR 0109 requires its own ADR for any
client crate. ADR 0638 already makes the warehouse the only evidence source
agents read, through one gated read-only port.

## Decision

1. **R12-DATA-8128 (decline).** The Knowledge Catalog is not admitted. No
   lineage emitter is written for a catalog that does not exist, because a
   producer with no consumer is what ADR 0151 decision 1 forbids. The row
   stays MISSING. It reopens only through an ADR that admits the catalog and
   states its lineage and evidence-contract scope, a measured need and a
   monthly ceiling (**provisional: none chosen here**). A gated Terraform
   resource, an emitter and a real plan follow that ADR and not before.
2. **R12-DATA-8131 (build, a stated way).** Large batch reprocessing is a Rust
   Cloud Run Job. The job reads the durable archive and the warehouse only. It
   never subscribes to the fabric, so freshness is never coupled to a job
   schedule. It runs under the same app composition-root rules as every other
   binary: configuration read in `main.rs`, an explicit timeout on every call
   that leaves the process, secrets as files. It writes nothing to the ledger
   and submits nothing to any broker. Dataflow and Dataproc stay out under ADR
   0011, so the row is scored against its Rust equivalent, as the GKE rows
   were scored against Cloud Run. The Terraform job resource is gated and off
   by default, and is applied only on the owner's approval of a plan. The
   inspection check becomes: the job's inputs are archive and warehouse
   readers, and no fabric subscription is reachable from its crate's
   dependency graph. Any client crate the job needs is a separate ADR under
   ADR 0109.
3. **R12-DATA-9052 (need a figure).** Spanner Graph stays declined. The figure
   needed is the monthly ceiling for a Spanner instance separate from the
   ledger's, plus a measured need; choose none. The `qip-mesh` `GraphStore`
   port stays caller-less and no agent writes the world graph. When the
   Spanner Graph ADR is written it carries the promoted-only write gate on
   `GraphStore::add_edge` with a promotion status on the mesh `Edge`, and a
   unit test whose mutation "admit a scratchpad write" must fail it. A Spanner
   client is admitted separately under ADR 0109. This amends no Accepted
   record: ADR 0113 admitted Spanner for the ledger only, and that stands.
4. **R12-DATA-9053 (owner).** Bigtable stays declined until the owner funds it.
   The register records about USD 950 a month at the autoscaler floor, which
   is the ledger's figure and not a ceiling chosen here. The row stays PARTIAL.
   Once funded, the order is: a store-record ADR under ADR 0113 section 1 with
   a measured need and ceiling; the gRPC client admitted under ADR 0109; a
   `qip-storage` adapter keyed by `rowkey::row_key`; a routing table with a
   test that only the five named high-rate record kinds reach the wide-column
   target, whose mutation routes one kind elsewhere; then the gated
   `enable_bigtable` setting and a real plan. The requirement text is not
   reworded to pass the row, and edge cells keep refusing a wide-column handle.

## What it costs

Three rows stay open on purpose. Without a catalog, lineage for the data
products is not registered, so a reader cannot yet ask the platform where a
figure came from beyond the event log and the warehouse. Without a graph store
and a wide-column store, promoted entities and high-rate ambient state keep
their present homes, and any contention with the ledger that Bigtable would
relieve is unmeasured rather than shown absent. The batch job adds a billed
resource and a Rust binary to maintain when it is built, and until then large
reprocessing has no engine at all. Choosing Rust over Dataflow gives up
Dataflow's autoscaling and windowing library, and the job's parallelism is
whatever the job's own code provides. No cost figure for the job is given,
because nobody supplied one.

## What would make this wrong

A measured workload that shows ledger contention from high-rate state would
make declining Bigtable the wrong call, and the funding ruling should then be
reopened. A committed need for cross-product lineage, from a regulator or from
a reproduction that failed for lack of it, would make declining the catalog
wrong. If reprocessing volume exceeds what a single Rust job can read inside
its timeout, the Rust-only choice fails and ADR 0011's exclusion would need its
own amendment, which this record does not make. If an agent ever needs durable
write access to the world graph before the Spanner Graph ADR exists, the
promoted-only gate would be missing at the moment it matters, so that need
must stop the work and not route around the gate.
