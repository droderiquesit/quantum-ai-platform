# ADR 0725: An ambient brain covers a scope only through a finding the deep brain composes from its sweep, the brain registry records and does not cover, and the desk's scopes keep their tests

- **Status**: Proposed 2026-11-22, for the owner
- **Date**: 2026-11-22
- **Rows**: R12-AMBIENT-3027 (implemented here); R12-AGENCY-3028 (same rule, not implemented here)

## Context

R12-AMBIENT-3027 states that an Ambient Attention Brain exists as a
specialist covering always-on attention routing. Its check: the brain is
constructed in the production organisation and emits findings for its scope;
removing it drops its scope from coverage.

What existed. `AttentionRouter` is built by `ResearchLoop::new` and runs in
every deep-brain sweep. It is registered as `ambient/attention` over
`attention-routing` in the deep brain's Brain Registry, and
`tests/brains.rs::removing_an_ambient_brain_drops_its_scope_from_the_registry`
proves that removing the registry entry removes the scope from the registry.
That is a statement about a record. `qip_investment_agents::coverage` counts a
scope only from the fact labels of informative findings, the router emitted
attention events and no finding, and so nothing the platform calls coverage
could see it.

The owner ruled on this row on 2026-10-31
(`docs/blueprint/lanes/wave1031-owner-rulings-2026-10-31.json`, class
`ADR_THEN_IMPLEMENT`), and this record is the ADR that ruling asks for. The
ruling, quoted:

> The Brain Registry records a brain; it does not cover a scope. Treating it
> as the production organisation would redefine the requirement to pass the
> row. The attention router stays advisory and off the Desk's roster. Its
> scope is covered by a finding the deep brain composes from each sweep's
> attention events, so removing the router removes the finding and the
> coverage.

and its closure: coverage gains an ambient scope set beside `Scope`, evidenced
only by whole fact labels; `Scope::ALL` and the organisation suite's per-scope
removal tests stay unchanged; `qip-deepbrain` composes one `AgentFinding` per
`ResearchRound` from `AttentionRouter` events carrying an `attention_routing`
fact; the test is
`removing_the_attention_router_drops_attention_routing_from_covered_scopes`;
the mutation is to skip the finding composition.

The ruling's title says "the desk's eight scopes". The figure was a count of
`Scope::ALL` on the day of the ruling and the enum has grown since; this
record's title leaves the figure out rather than carry a number that is
already wrong. Count with
`sed -n '/pub const ALL: \[Scope/,/\];/p' backend/crates/agents/qip-investment-agents/src/coverage.rs | grep -c 'Scope::'`.

ADR 0146 decided that the ambient agency is a registered brain and not an
organisation agent. This record amends nothing in it: the router still has no
seat on the roster. It adds the one thing 0146 left open, which is what
"covers" means for a brain with no seat.

## Decision

1. **A registry entry is a record, not coverage.** Nothing that answers "is
   this scope covered" reads the Brain Registry.
2. **Ambient scopes are a separate set.** `coverage::AmbientScope` sits beside
   `coverage::Scope` with its own `ALL`, its own whole-label rule and its own
   reader, `ambient_covered_by`. `Scope::ALL` is not extended, and no
   organisation test is edited. The two sets do not read each other's labels:
   the attention finding covers no Desk scope.
3. **The deep brain composes the finding.** At the end of every sweep
   `ResearchLoop::sweep_over` composes one `AgentFinding` from the attention
   events the router returned in that sweep
   (`qip_deepbrain::ambient_coverage::attention_finding`). It carries
   `attention_routing` (the number of routing decisions), `attention_activated`
   and one `attention_woke_<pathway>` fact per pathway actually activated, and
   cites the event ids as evidence. It has no direction and no conviction: the
   router decides what wakes and holds no view on a market.
4. **No events, no finding.** A sweep in which the router returned nothing
   composes no finding and covers nothing. A finding stating zero would cover
   the scope on a sweep where the router was absent as well as on one where it
   was quiet.
5. **What reads it.** The sweep publishes
   `qip_ambient_scope_covered{scope}` as 1 or 0 on every sweep, and the
   ambient journal writes the finding as an `attention_finding` record after
   the `attention` records it cites.
6. **R12-AGENCY-3028 takes the same rule** when it is built: a variant of
   `AmbientScope` evidenced by a finding composed from what the causal agency
   routed. It is not built here.

## What it costs

- "Constructed in the production organisation" is read as "constructed by the
  deployed deep brain and reporting through the organisation's coverage
  contract", not as a seat on the Desk's roster. That reading is the owner's,
  and it is a reading: a reviewer who takes the phrase literally will find no
  `ambient/attention` in `standard_agents`.
- Coverage is per sweep. On a quiet platform the series reads 0 while the
  router is present and working. An operator who reads 0 as "the router is
  gone" will look for the wrong fault; the registry is where presence is
  recorded.
- A second scope enum and a second reader, which somebody must remember are
  two when they ask what the platform covers.
- `ResearchLoop::without_attention_router` exists for the removal test and has
  no deployed caller.
- One more record per sweep on the ambient journal, which is in-process unless
  a deployment names a durable destination.
- The new series has no alert policy, and none is proposed until something
  scrapes the deep brain.

## What would make this wrong

- If the owner meant a seat on the roster. Then the router needs a brief it
  can answer, which it does not have, and this record should be replaced
  rather than amended.
- If per-sweep coverage proves too noisy to read and a window is wanted. No
  window length has been given, so none is invented here.
- If a Desk agent ever emits a fact labelled `attention_routing`, the ambient
  scope would be covered by a brain that is not the router. The label is
  matched whole, but nothing structural stops a second emitter.
- If the composed finding is later fed into the chief's synthesis as a vote.
  It carries no conviction for that reason, and averaging it in would let a
  count of routing decisions dilute a market view.
