# ADR 0182: A simulation lesson is a promoted claim, an embedding is a fifth model form, and event arrival and the warehouse write path stay as decided

- **Status**: **Accepted**, 2026-10-25, decided under the owner's delegation
  of 2026-10-24, recorded in
  [`docs/blueprint/owner-decisions-2026-10-24.md`](../blueprint/owner-decisions-2026-10-24.md).
  No new dependency, billed service or risk limit is decided here.
- **Date**: 2026-10-25
- **Amends**: [ADR 0083](0083-a-trained-model-is-served-in-process-by-an-in-tree-provider-for-the-forms-the-platform-trains.md) (the
  closed list of model forms) by adding one. Leaves
  [ADR 0151](0151-what-enters-understand-each-tick-plane-and-federation-input-has-a-named-producer-and-consumer-or-is-not-built-and-an-outside-prior-may-only-narrow-reason.md)
  decision 10 standing.
- **Rows**: R12-TICK-9038, R12-TICK-2064, R12-EVENT-5084, R12-DATA-10081,
  R12-TICK-10172.

## Context

- **TICK-9038** asks for a rare-event factory whose outputs are ephemeral,
  with only promoted lessons retained. Nothing defines what a promoted
  simulation lesson is or where it persists. The kernel's lessons cover
  claim classes only.
- **TICK-2064** asks for a learned, versioned embedding of market state
  evaluated against a baseline. ADR 0083 serves four model forms "until an
  ADR says five" and names an embedding as refused at the wire.
- **EVENT-5084** asks for outcome distributions estimated from evidence. ADR
  0151 decision 10 keeps the event plane's distribution venue-implied
  because arrival likelihoods have no producer, and the event plane opens
  only with a licensed source.
- **DATA-10081** asks for a fabric sink to BigQuery through the Storage Write
  API. That API is gRPC-only; no gRPC crate is admitted (ADR 0109), and
  `qip_mesh::bigquery::BigQueryWarehouse` uses `tabledata.insertAll`.
- **TICK-10172** asks for phase 13's exit test, which nobody has defined.

## Decision

1. **A simulation lesson is a promoted claim (TICK-9038).** A scenario the
   generator produces, including one beyond the historical envelope, lives in
   memory for the round that grew it and is never journaled. What persists is
   a lesson: a statement of the form "under conditions C, the book's response
   R breached bound B", raised as a `LessonCandidate` with the scenario's
   digest as its evidence, and journaled only when the existing lesson
   promotion admits it. A restarted process rebuilds promoted lessons from
   the journal and has no scenario. The default-path generator is the
   kernel's stress stage; a beyond-history move is a scaled historical shock
   with its scale recorded.
2. **An embedding is the fifth model form (TICK-2064; amends ADR 0083).** A
   market-state embedding is admitted as a model form: a fixed-dimension
   vector produced by a linear projection fitted in-tree (no new crate), with
   a `ModelCard` version and a baseline it must beat on a stated retrieval or
   prediction task before the card is issued. It is refused at the wire for
   every use except retrieval and features, the same discipline the four
   forms have.
3. **Event arrival stays venue-implied (EVENT-5084).** ADR 0151 decision 10
   stands: there is no licensed evidence source for arrival likelihoods, and
   estimating them from nothing would be a number nobody computed. The row
   stays PARTIAL, citing this record.
4. **The warehouse write path stays `insertAll` (DATA-10081).** The
   requirement's Storage Write API needs a gRPC stack, and admitting one
   needs its own dependency ADR with a security review. `insertAll` meets the
   requirement's purpose: rows land and a lagging sink affects no producer.
   The row stays PARTIAL on the API clause until the owner either restates it
   or admits a gRPC crate. The remaining code is a fabricd consumer driving
   the warehouse, with a test that a stalled sink never blocks a producer.
5. **Phase 13's exit test (TICK-10172)** is: a future tree grown on the
   default path; at least one twin beyond the per-instrument market twin; a
   rare-event lesson promoted and surviving a restart per decision 1; and a
   portfolio outcome distribution produced from the tree for the platform's
   own book. Macro and company twins need licensed data under ADR 0157, so
   the phase cannot exit until that data exists.

## Consequences

- TICK-9038 and TICK-2064 become closable by code.
- EVENT-5084 stays PARTIAL by decision. DATA-10081 stays PARTIAL on the API
  clause and has code left. TICK-10172 has a defined exit and waits on data.

## What it costs

- A fifth model form widens the wire's closed list by one.
- Promoting only lessons discards scenarios a later reader might have wanted
  to inspect; their digests survive in the lesson's evidence.

## What would make this wrong

- If a licensed source for event-arrival evidence entered the catalogue,
  decision 3 should be revisited.
- If `insertAll` quotas or costs became binding at real volume, decision 4
  should give way to a gRPC dependency ADR.
