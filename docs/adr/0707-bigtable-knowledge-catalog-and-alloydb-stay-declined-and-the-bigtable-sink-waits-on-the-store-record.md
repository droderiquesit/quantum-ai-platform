# ADR 0707: Bigtable, Knowledge Catalog and AlloyDB stay declined, and the Bigtable sink waits on the store record

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15)
- **Date**: 2026-11-16
- **Rows**: R12-DATA-10035, R12-DATA-10037, R12-DATA-10040, R12-DATA-10080

## Context

Four open rows ask for managed stores that no accepted record admits.

R12-DATA-10035 asks for Bigtable holding rebuildable high-rate derived state.
The tree declares the instance with autoscaling, gated off in every
environment, and no Rust client writes it. R12-DATA-10080 asks for a Fabric
sink that writes recent features and replay indexes into it, and it depends on
10035. The provider refuses `StorageTarget::Bigtable` for want of a gRPC
stack. ADR 0109 does not admit a protobuf crate. ADR 0113 states that
Bigtable, AlloyDB and Spanner Graph are not admitted by it.

R12-DATA-10037 asks for Knowledge Catalog. No such resource exists. ADR 0101
row C4 lists it among the stores that each need a storage record.

R12-DATA-10040 asks for AlloyDB holding operational relational metadata. A
cluster is declared and gated off, and no code writes to it. ADR 0113 refuses
AlloyDB, and no Postgres wire client is admitted.

The owner's rulings of 2026-10-31 on these rows were STAY_PARTIAL_DEPLOY,
resting on decision 5 of 2026-10-25: a billed managed store is not created
before a measured need and a stated monthly ceiling exist. The register names
the cost of an empty Bigtable instance at its two-cluster floor and of an
AlloyDB primary billed per vCPU-hour; those figures are the register's, not
this record's. This record turns the rulings into decisions so the rows have
one place that says what reopens them.

## Decision

1. **R12-DATA-10035 (decline).** Bigtable stays declined. `enable_bigtable`
   stays false in every environment, and the row stays PARTIAL. It reopens only
   through a store ADR under ADR 0101 C4 that states a measured need, a monthly
   ceiling and the client it uses. That ADR must show the data is rebuildable
   from retained sources, since a hot store treated as the record makes its
   loss a data loss. The rebuild job, the tfvar flip and a real plan follow
   that record and not before.
2. **R12-DATA-10037 (decline).** Knowledge Catalog stays declined and nothing
   is declared for it in Terraform. The row stays MISSING. It reopens only
   through an ADR that admits it with a stated need, its scope and a monthly
   ceiling. A gated module resource, lineage registration and a plan follow
   that ADR. Until then lineage is carried by the hash-chained event log, which
   is the record this platform already answers to.
3. **R12-DATA-10040 (decline).** AlloyDB stays refused under ADR 0113 and
   decision 5. No cluster is created and no Postgres wire client is admitted.
   Workflow state stays on the journal. The row stays PARTIAL. It reopens only
   through an ADR that supersedes ADR 0113's AlloyDB clause, naming that
   amendment, with a measured need for relational workflow metadata and a
   monthly ceiling, plus a client ADR under ADR 0109.
4. **R12-DATA-10080 (decline, blocked on decision 1).** No Fabric sink is
   written against a store no ADR admits. The row stays MISSING, blocked by
   the Bigtable client needing its own dependency ADR under ADR 0109. It
   depends on the reopening record of decision 1, on that client record, and
   on a running fabricd. A sink then gets an integration test showing rows
   appear, and a plan with `enable_bigtable` and the event fabric broker set.
   Warm-service feature lookups stay on the journal, and the cost of that is
   accepted below.
5. **Common rule.** No figure is chosen here for any ceiling, because none was
   given. Each reopening ADR names its own ceiling, and a ceiling the owner has
   not stated is a figure the owner must supply. Nothing in this record touches
   a risk limit, a budget control, the paper-trading boundary or an Accepted
   record. It restates ADR 0113's refusals and does not amend them.

## What it costs

- Feature and replay-index reads for warm services stay on the journal, which
  the requirement calls too slow. No measurement here says how slow, so the
  cost is real and unquantified.
- There is no catalog view of governance, lineage or quality across the data
  stores. A reader reconstructs lineage from the event log.
- There is no relational store for workflow metadata, so queries across
  workflows stay journal scans.
- Four rows stay open and are not closed by declining. The declared but gated
  Terraform for Bigtable and AlloyDB stays in the tree as unused declarations.

## What would make this wrong

- A measurement showing journal reads miss a warm service's latency need by a
  margin a hot store would fix at a ceiling the owner will pay. That is the
  evidence decision 1 asks for, and it reopens 10035 and 10080 together.
- A decision explained from a data product's lineage that the event log cannot
  answer. That would be the measured need decision 2 asks for.
- Workflow state outgrowing the journal in a way that a relational query would
  fix. That reopens decision 3.
- The owner naming ceilings for these stores and admitting a client. Then the
  declines here become sequencing and the reopening ADRs are the next work.
