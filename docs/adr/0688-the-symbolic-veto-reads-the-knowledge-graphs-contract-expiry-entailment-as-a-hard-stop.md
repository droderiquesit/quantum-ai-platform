# ADR 0688: The symbolic veto reads the knowledge graph's contract-expiry entailment as a hard stop

- **Status**: **Proposed 2026-11-15, lane 44 of wave b52. Built and not compiled.**
  The owner's instruction of 2026-11-15 forbids local build, test and clippy for
  this wave; the pipeline builds after merge. It sets no figure, adds no
  dependency, relaxes nothing and touches no paper-trading layer.
- **Date**: 2026-11-15
- **Rows**: R12-REASON-3054 (stays PARTIAL; this closes the "advisory only" gap
  for one entailment).

## Context

`KnowledgeDerivation::outlived_by_claim` is computed in every cycle and
replay-checked, and nothing read it. The pack's own rule r70 already rules
against such a hypothesis, but `SymbolicDerivation::veto`, the function the
kernel turns into a refusal, lists only three stops (live authority, lot over
budget, cash short). An approved hypothesis on an expiring contract therefore
produced a derivation that was ineligible and a veto of `None`.

## Decision

1. `veto` adds a fourth stop when the record's own knowledge graph entails the
   claim outlives the contract, and `veto_rules` names it as
   `knowledge/outlived-by-claim`.
2. It is restrictive only. It fires only on a hypothesis already ineligible, and
   a record with no graph (written before it existed) adds no stop.
3. Not decided here: ingesting a physical delivery state, and loading the
   ontology from a governed store. Both remain open on the row.

## Consequences

An approved hypothesis on an expiring contract is vetoed with the entailment
named. The paper boundary is untouched: Terraform refusal, `AutonomyLevel::deployable`
and the `Cell` constructor are not edited.

## What it costs

A refusal now blocks something it did not before: an approved hypothesis on a
dated contract that the graph holds as expiring before the claim can resolve is
vetoed, where before its derivation was ineligible and its veto was `None`. The
stop is only as good as the graph's typing: a subject with no recorded expiry
margin is not typed `dated_contract` and is not stopped here. The graph is
closed from the same expiry margin rule r70 reads, so a wrong margin misleads
both and this stop is no second opinion on it. The decision adds one more name
to `veto_rules` that an operator reading a refusal must learn, and a person
must decide any existing test that expected no veto on such a contract. Each
evaluation of the stop reads the recorded closure up to three times through
`holds`, and `veto_rules` evaluates it again after `veto`; the cost of that is
unmeasured. Physical delivery state and a governed ontology store are left
unbuilt, so the row stays PARTIAL.

## What would make this wrong

If the pipeline shows an existing test that expects `veto()` to be `None` for an
expiring contract, that test asserted the gap and needs a person's decision.
