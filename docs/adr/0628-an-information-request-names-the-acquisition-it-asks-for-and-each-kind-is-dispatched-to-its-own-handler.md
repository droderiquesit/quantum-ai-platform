# ADR 0628: An information request names the acquisition it asks for, and each kind is dispatched to its own handler

- **Status**: Accepted 2026-11-15, lane 14 of wave b52. A contract field and a
  routing table. It adds no dependency or figure and touches no
  paper-trading layer.
- **Date**: 2026-11-15
- **Rows**: R12-DATA-3105 (a slice).

## Context

Blueprint §9.10 lets an information request trigger a new source query, an
alternative provider, a microstructure probe, a simulation, a specialist
agent, a public filing fetch or an additional measurement. `InfoRequest` had
no kind, so the research loop turned every launched request into one scout
job whose only handler was the discovery desk. A request to re-measure was
answered by a search for a new source.

## Decision

1. `qip_contracts::ambient::AcquisitionKind` has the seven kinds.
   `InfoRequest` carries one, defaulted and not written when it is the
   default, so a record sealed before the field decodes as a new source query
   — which is what it always meant.
2. `ScoutRequest` carries the kind. A candidate priced as a re-sweep launches
   as an additional measurement; the others as new source queries.
3. `qip_deepbrain::acquisition_dispatch::route` is total over the kinds. The
   node hands the discovery desk only new-source queries and records where
   every launched request went on `ResearchRound::dispatched`.
4. Alternative provider and public filing fetch have no handler: each needs a
   vendor or filing connector no environment configures. They route to
   `Unhandled` with the reason. They are never sent to the discovery desk as
   if a search answered them.

## What it costs

A request of the alternative-provider or public-filing kind is routed to
`Unhandled` with a reason and is not handed to the discovery desk. The
research loop launches a request before the node dispatches it, so such a
request is still counted as launched. A new `AcquisitionKind` variant is a
compile error in `route` until someone assigns it a handler, which is the
intent and also an added edit at every extension. `route` names the handler
for the microstructure, simulation, specialist and re-measurement kinds, but
the node hands the discovery desk only new-source queries and no code outside
`acquisition_dispatch` reads the handler named, so the dispatch records those
kinds on `ResearchRound::dispatched` and executes nothing for them. The node
fills that record only where it holds a discovery desk. The cost of the extra
field and the per-round dispatch record is unmeasured.

## What would make this wrong

A request recorded as `Unhandled` that an operator needed answered, because a
vendor or filing connector was in fact configured, would show the refusal was
withheld work and not an honest gap. A reviewer finding rounds whose
`dispatched` list names a handler for a kind but whose request was never
executed by it would show routing was read as completion. A sealed record
that decodes to the wrong kind because the field default no longer means a
new source query would also make the decision wrong.

## What remains

Handlers for the two unhandled kinds (vendor/connector and egress owner
decisions), and the creation sites that would raise requests of the
microstructure, simulation and specialist kinds: today only the first two
kinds are produced.
