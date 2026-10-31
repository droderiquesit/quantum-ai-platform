# 0199. Three expansion clauses stay declined under the 2026-10-31 delegation: specialists are never minted, cross-brain episodes wait on a named consumer, and the quantum research families stay parked

- Status: accepted 2026-10-31 under the owner's 2026-10-31 delegation ("complete all sessions; finish what you can by morning"), relayed by the wave parent to lane L4b-expand, which holds the number
- Confirms: ADR 0158 decision 8, ADR 0151 decision 4, ADR 0150 amendment A2 and ADR 0190
- Rows: R12-EXPAND-6030 (BrainSpec / SpecialistPack from unresolved-question clusters), R12-EXPAND-1174 (the collective intelligence fabric's exchange of episodes, residuals, lessons and outcomes between brains), R12-EXPAND-6036 (QuantumExperiment / Benchmark from quantum-assisted research)

## Context

Three register rows ask for something an accepted decision already declines,
and the 2026-10-24 triage marked each DECIDE_ADR: the delegation lets a lane
supersede the earlier decision or confirm it, and either way the row needs a
record so that "declined by decision" can be told from "nobody looked".

- R12-EXPAND-6030 asks that repeatedly unresolved domain questions
  instantiate a new specialist with its own tools, corpus, evaluations and
  authority scope. ADR 0158 decision 8 says a specialist proposal produces a
  reviewable `BrainSpec` and never an agent, because an agent minted by a
  process that found a gap is an agent nobody reviewed holding authority
  nobody granted.
- R12-EXPAND-1174 asks that brains, evaluators and reflex cells exchange
  compressed knowledge, episodes, residuals, lessons and outcomes via typed
  contracts and lineage. ADR 0151 decision 4 builds no `SharedEpisode` and no
  cross-brain receiver until a brain names a decision the record feeds;
  `KnowledgeDeltaRecord` is the one cross-brain record, and a cell's
  `RegionalEpisodes` stay in cell memory, neither reported nor journaled.
- R12-EXPAND-6036 asks that quantum resources be used for benchmarked
  optimisation, graph search, feature and ensemble selection and variational
  experiments where they add measurable value. ADR 0150 amendment A2 admits a
  new quantum family only after a committed note shows the posed problem
  sizes sit above the exact limit and at or below `MAX_QUBITS`; no such note
  exists, the platform poses no such sizes, and ADR 0190 confirmed on
  2026-10-24 that the gates stand and the research families stay parked.

## Decision

1. **Specialists are never minted (6030): ADR 0158 decision 8 is
   confirmed.** `MetaIntelligence::propose_specialists` keeps producing
   research observations, and ADR 0197's `BrainSpec` proposal is the most a
   gap produces: a reviewable spec with the gap as lineage, registered by a
   person through the roster, never by the sweep. The requirement's
   "instantiate" clause is declined; the row records that and stays PARTIAL
   until the owner changes the requirement text.
2. **Cross-brain episodes wait on a named consumer (1174): ADR 0151
   decision 4 is confirmed, with the consumer this record does name.** The
   deep brain's research sweep now reads the kernel's `KnowledgeDeltaRecord`
   off the log (R12-EXPAND-6039, 2026-10-25); that is the one cross-brain
   exchange that has both a producer and a reader. A cell's
   `RegionalEpisodes` are carried on `CellReport` only when a central
   consumer names the decision they feed, in a frame designed for it (ADR
   0128 admits the spine alone); none has, so none is built. Residuals and
   lessons travel as they do today: residuals as fill scores the sweep reads,
   lessons as the LEARN stage's own memory. The row stays PARTIAL with the
   unbuilt items named.
3. **The quantum research families stay parked (6036): ADR 0150 A2 and ADR
   0190 are confirmed.** No `FeatureEnsembleSelection` family, no new
   `QuantumExperiment` producer, and the parked `l4-quant-research-families`
   patch stays parked. The one existing family (portfolio allocation) keeps
   its benchmark against the classical baseline (ADR 0006). The row stays
   PARTIAL, citing A2 and the note it waits on.
4. **These are confirmations, not new gates.** No test, guard or default
   changes under this record; what changes is that each row's gap now cites
   the decision that declines its clause rather than describing an absence.

## Consequences

- The three rows carry a decision date and a record to reopen, which is the
  difference between a clause declined and a clause forgotten.
- Lane L4-quant's parked patch and ADR 0197's `BrainSpec` proposal remain
  the next steps if the owner reverses either decision.

## What it costs

- Nothing in code. The cost is in the register: three rows that read PARTIAL
  for a reason the owner may one day want to change, each naming that reason.
- A reader of the requirement text who expects specialists to be minted or
  quantum families to grow finds the platform declining both, and this record
  rather than a gap says why.

## What would make this wrong

An owner decision changing the requirement text of any of the three rows, or
a committed note under ADR 0150 A2 showing a posed problem size in the
quantum-relevant band. Either reopens the clause it names and supersedes the
corresponding paragraph here; the other two stand.

## Code it constrains

None changed. `qip-expansion/src/meta.rs` (`propose_specialists`),
`qip-deepbrain/src/brains.rs` (`BrainRegistry::register` at start-up only),
`qip-edge/src/cell.rs` (`RegionalEpisodes` in cell memory), and the quantum
family declarations of `qip-optimization-engine` keep their current shape.
