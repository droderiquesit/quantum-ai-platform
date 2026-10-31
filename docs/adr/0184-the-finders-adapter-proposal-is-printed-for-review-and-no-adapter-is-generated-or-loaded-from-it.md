# ADR 0184: The finder's adapter proposal is printed for review, and no adapter is generated or loaded from it

- **Status**: **Accepted**, 2026-10-31, decided under the delegation the
  owner gave the parent on 2026-10-31 for every remaining product decision.
  Conservative and paper-only: nothing below reaches a venue, polls a
  source, or enables anything that was off.
- **Date**: 2026-10-31
- **Amends**: [ADR 0180](0180-connector-health-is-scored-world-documents-are-re-fetched-never-copied-and-the-scout-records-every-first-pass-verdict.md)
  decision 3 for R12-DATA-2002 only. ADR 0157 §4 (no connector is built
  from a registered source) and §5 (no crawl builds candidates) stand
  unchanged, and so does ADR 0180 decision 3 for R12-DATA-1101 and
  R12-DATA-1104.
- **Rows**: R12-DATA-2002.

## Context

R12-DATA-2002 reads: "For a source it approves, the Scout creates, or
proposes for review, a feed adapter that can ingest it. unit: A registered
source yields an ingestion descriptor/adapter plan matching its access
mechanism; fails if registration produces no plan."

The requirement offers two readings, and they are not equally safe. To
*create* an adapter would put a feed in front of the desk whose terms
nobody read; ADR 0157 §4 forbids that, and ADR 0180 decision 3 left this
row PARTIAL for that reason. To *propose for review* is the other reading,
and the code for it now exists: `qip registrations propose --candidates
<path>` assesses each candidate through `Platform::assess_sources` — the
finder, the same seam the deep brain's discovery desk drives — and prints,
for each source it registers, the `IngestionPlan` and `SourceDescriptor`
`qip_data_finder::ingestion` derives from the registration (mechanism,
delivery, poll floor, schema fingerprint, credential needed), and for each
source it does not register, the finder's reason. An independent review
held that path on its own terms and refused the row only because ADR 0180
decision 3 had declined the clause without separating its two readings.

## Decision

1. **"Proposes for review" is met by the finder's printed proposal.** The
   Scout's proposal for an approved source is the plan and descriptor the
   finder derives from that source's registration, printed for a person by
   `qip registrations propose`. The finder decides which sources get one;
   the operator decides when to ask. That is the review the requirement
   names.
2. **Nothing is created, written or loaded from a proposal.** No code reads
   the printed plan back; no manifest, connector or configuration is
   generated from it; the candidate catalogue the deep brain loads never
   reads it; the command assesses on an in-memory event log, so a
   deployment's journal is not touched by a question. A person who wants an
   adapter writes one, reviewed, as every connector is (ADR 0157 §4).
3. **Only an approved source is proposed.** A candidate the finder refuses,
   defers or quarantines yields its reason and no plan, because a plan for a
   source whose terms forbid use would read as permission.

## Consequences

- R12-DATA-2002 is no longer held by ADR 0180 decision 3; it is graded on
  the requirement's proof, which the tests in
  `qip-cli/tests/adapter_proposals.rs` drive for a pull (REST) and a push
  (WebSocket) mechanism.
- R12-DATA-1101 and R12-DATA-1104 (continuous search, generated adapters)
  remain PARTIAL by ADR 0180 decision 3; this record does not touch them.

## What it costs

- An operator must run the command; the proposal is not produced on a
  schedule. That is deliberate: probing third-party hosts is the
  discovery cadence ADR 0180 decision 2 keeps off.

## What would make this wrong

- If an adapter were ever generated from a proposal, or a proposal were
  read back by the platform, decision 2 would be violated and this record
  would need revisiting before the change could land.
