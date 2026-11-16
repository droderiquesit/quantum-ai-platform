# ADR 0708: The archive keeps a class prefix per fabric stream, and the Bigtable, Spanner Graph, Knowledge Catalog and Vertex stores stay declined until each has a measured need and a ceiling

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15)
- **Date**: 2026-11-16
- **Rows**: R12-DATA-8006, R12-DATA-8019, R12-DATA-8122, R12-DATA-8126

## Context

Four open rows in the data plane share one cause. The blueprint names a store
per access pattern, and the owner's ruling of 2026-10-31 funds one of them.

R12-DATA-8006 wants the data plane served by GCS, Bigtable, BigQuery, Knowledge
Catalog, Spanner Graph and Vertex. `modules/data` declares BigQuery, the GCS
archive, AlloyDB, Bigtable, Memorystore and Spanner, each gated false in every
tfvars. Rust adapters exist for Cloud Storage, BigQuery and Memorystore;
`qip-storage`'s `provider.rs` refuses Spanner, Bigtable and AlloyDB by name.
Knowledge Catalog and Spanner Graph appear nowhere. The owner ruled that
BigQuery is the one store switched on, that Cloud Storage waits for its first
writer, and that the others stay declined under decision 5 until an ADR for
each states a measured need and a monthly ceiling.

R12-DATA-8122 wants tier K0 to spill to Bigtable. The bounded in-memory tier
exists; no Bigtable client exists, and ADR 0109 does not admit `prost`.
R12-DATA-8126 wants a multi-cluster Bigtable with routing app profiles fed by
`fabric-sink-bigtable`. The plan clause is met in code (two zonal clusters,
three app profiles, a `.tftest.hcl` for them), the sink does not exist, and
`enable_bigtable` is false everywhere. The owner's recorded floor for the
two-cluster instance is about $950-$1,300 a month.

R12-DATA-8019 wants tick and internal data held as distinct retention classes.
`Topic::retention_class` is exhaustive, the lake writer separates
`RecordClass::Market` and `RecordClass::Internal`, and `Broker::archive_sealed`
attaches entitlement labels. What is missing is that fabric archive keys
(`segments/sha256/`, `manifests/`) fall outside the bucket's
`lake/class=market/` and `lake/class=internal/` lifecycle prefixes, so
archived fabric segments take no class-specific retention. ADR 0239 claimed
that title on 2026-10-31 and has no body; this record carries the decision.

## Decision

1. **R12-DATA-8006 (decline).** The store set stays as the owner ruled: BigQuery
   on, Cloud Storage on its first writer, and Bigtable, Spanner Graph, Knowledge
   Catalog and Vertex declined. Each reopens only by its own ADR that states a
   measured need and a monthly ceiling, after the dev deploy. The requirement
   text stands and is not read down to the stores that are funded; the row stays
   PARTIAL. No adapter is built for a declined store, and the `provider.rs`
   refusals stay, since a refusal by name is what the row's check asks for.
2. **R12-DATA-8019 (build).** Each fabric stream archives under the retention
   class its catalogue entry declares. The archive key gains a class prefix
   inside the bucket's existing `class=market` or `class=internal` lifecycle
   rules. A stream whose declared class disagrees with `Topic::retention_class`
   of any of its topics is refused at catalogue load, never reconciled. No class
   prefix gains a delete rule beyond the two that already exist, and neither
   existing rule is shortened. Entitlement labels are kept on every archived
   object. The bucket stays gated off until the owner's tfvars flip.
3. **R12-DATA-8122 (need a figure).** K0 stays in memory and spills nowhere.
   The figure needed is the horizon beyond which derived state must be kept
   rather than dropped, with the monthly ceiling that would fund it. No figure
   is chosen here. K0 is not restated without Bigtable to pass the row.
4. **R12-DATA-8126 (decline).** Bigtable stays declined. Nothing more is built
   for a store nobody pays for, and `fabric-sink-bigtable` waits with
   R12-DATA-10035. The reopening ADR must name the ceiling against the
   $950-$1,300 floor and the gRPC client dependency, which needs its own ADR
   under ADR 0109. The row stays PARTIAL.

## What it costs

Four rows stay PARTIAL on the data plane's breadth, by choice. Features and
replay indexes have no Bigtable to be resilient on, and K0 state past its
in-memory horizon is dropped. The class prefix in decision 2 changes archive
key layout, so any segment already archived under the old keys would stay
outside the class rules; none is known to exist, because the archive bucket is
gated off, but that is unchecked. Catalogue-load refusal can stop a catalogue
that loads today. This record states no cost for Knowledge Catalog, Spanner
Graph or Vertex, which the owner's ledger lists as unpriced.

## What would make this wrong

If a measured workload shows in-memory K0 dropping state the decision loop needs
before a Bigtable ADR is accepted, decision 3 leaves a real defect and its figure
must be supplied sooner. If a class prefix inside the lifecycle rules proves
unable to carry both an entitlement label and the sealed-segment layout without
breaking replay, decision 2 needs the owner's choice between a second bucket and
a changed key scheme. If the owner funds Bigtable, decisions 1 and 4 lapse for
that store only, through the ADR they name.
