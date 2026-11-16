# ADR 0638: Agents read evidence only from the BigQuery warehouse, through one gated, read-only port

- **Status**: **Accepted 2026-11-15 under the owner's 2026-10-31 ruling
  ADR_THEN_IMPLEMENT** for R12-AMBIENT-9001 (lane 19, wave b52). An
  in-repository decision. It enables no cloud resource, adds no dependency and
  touches no paper-trading layer.
- **Date**: 2026-11-15
- **Rows**: R12-AMBIENT-9001 (stays PARTIAL; see "What this leaves open").
- **Supersedes**: nothing. Narrows owner decision O02.

## Context

R12-AMBIENT-9001 requires that agents "consume Knowledge Catalog / Spanner
Graph / BigQuery evidence and publish research tasks and results through the
Fabric as typed events." Owner decision O02 was ruled narrowly on 2026-10-31:
BigQuery, already switched on in `dev`, is the one evidence store agents read.
Bigtable, Spanner Graph and Knowledge Catalog stay declined until a measured
need and a monthly ceiling exist (AMBIENT-9001, DATA-8015, DATA-9052,
WORLD-8129). ADR 0113 admitted Spanner for the ledger only, and its section 2
admits in-tree REST adapters.

Until now no agent had a way to read evidence that was not an in-process
facility the platform copied in, so there was nothing to point a warehouse at.

## Decision

1. **One port.** `qip_agents::evidence::EvidenceStore` is the only way an
   agent reads warehouse evidence. It has a `read` method and no write method,
   so the warehouse is read-only from an agent by construction.
2. **One gate.** The port is held behind `Capability::ReadEvidence`
   (sensitivity 0), through the same `Gated` facility every other read uses.
   A run without the grant fails and the refused access is on the run record
   the REASON stage journals whole (ADR 0456).
3. **Queries are validated, never clamped** (principle 2). `EvidenceQuery`
   refuses a blank subject, a dataset that is not a token of letters, digits,
   `_`, `.` and `-` (so fetched text cannot carry a statement), and a limit of
   zero or above `MAX_EVIDENCE_ROWS` (500, **provisional**: nobody gave a
   figure, the choice is restrictive, and a measured need raises it by ADR).
4. **Bitemporal.** `read_evidence` refuses a store that returns a row knowable
   after the run's clock, and one that returns more rows than the limit.
5. **The adapter is not decided here.** A BigQuery REST adapter implements
   `EvidenceStore` in a later slice under ADR 0113 section 2, with an explicit
   timeout, the egress proxy, and Workload Identity Federation. No other
   store is admitted.
6. **Research tasks are already typed records.** `Arbitration::research`
   carries a `ResearchTask`, and the federation review that holds it is
   journaled on `Topic::WorldModelUpdated`. This ADR adds no topic and no
   route; a dedicated research-task topic is a fabric contract change that
   needs its own ADR and is not decided here.

## Evidence

`backend/crates/libs/qip-agents/tests/evidence.rs`: a run holding
`ReadEvidence` reads and the access is recorded; a run holding every other
read capability is denied and the denial is recorded; a future-dated row and
an unbounded or statement-carrying query are refused.

## What it costs

A new capability widens `CapabilitySet::read_only()`: every agent granted "all
read-only capabilities" can now reach the warehouse once a store is mounted.
Manifests that enumerate their grants are unaffected. Until an adapter exists
the port reads nothing in production. When the adapter lands, each read is
billed BigQuery bytes scanned, which is why the row limit is small.

## What would make this wrong

If agents need to join evidence across datasets, a per-dataset token query is
too narrow and the port will be bypassed by a second one, which is the failure
it exists to prevent. If BigQuery latency exceeds an agent's budget line, the
port will be hit on every run and the answer is a cache with a bound, not a
second store. If the 500-row provisional limit proves too low for a measured
study, raise it here by ADR rather than at the call site.

## What this leaves open

The BigQuery REST adapter, its composition-root wiring, a production caller
in the REASON stage, and a decision on whether research tasks deserve their
own fabric topic.
