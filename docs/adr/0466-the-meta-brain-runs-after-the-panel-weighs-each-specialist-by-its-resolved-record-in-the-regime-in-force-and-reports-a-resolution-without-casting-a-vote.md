# ADR 0466: The Meta Brain runs after the panel, weighs each specialist by its resolved record in the regime in force, and reports a resolution without casting a vote

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation**
  (a session decides the in-repository design question it meets, by ADR),
  session t50-20. The delegation's limits hold: it sets no figure the owner
  has not stated, changes no requirement's wording, adds no dependency and
  touches no paper-trading layer. It introduces no threshold: every weight is
  a hit rate the research memory already holds, and nothing sizes, vetoes or
  trades on the result.
- **Date**: 2026-11-12
- **Rows**: R12-AMBIENT-3024.

## Context

R12-AMBIENT-3024: "A Meta Brain exists as a specialist covering: allocating
research attention, measuring which brains/models are reliable in which
regimes, and resolving disagreements." Its check: the brain "is constructed
in the production organisation and emits findings for its scope; removing it
drops its scope from coverage."

Three things stood in the way, each recorded in the row's gap since
2026-10-18:

- No manifest and no agent.
- `Organisation::dispatch` runs every agent on the brief alone, by design: a
  specialist that read its colleagues would be a correlated second finding
  from one origin. A disagreement cannot be resolved by an agent that never
  sees it.
- Research memory, which the kernel writes every resolved analyst call to
  (`Platform::write_research_memory`, R12-WORLD-3032), recorded no regime, so
  the only reliability it could state was one pooled hit rate. An analyst
  right in a calm tape and wrong in a stressed one reads as mediocre in both,
  which is true of neither.

## Decision

1. **A second dispatch phase, for arbiters only.** `chief::Arbiter` is an
   `Agent` that also takes a `PanelView` — the panel's informative findings
   and whether they disagree in direction. `Organisation::with_arbiters`
   admits arbiters; `dispatch` runs every panel agent first, then every
   arbiter once, each through `AgentHost::run` (capability checks, budget,
   audit, backlog) like any run. An arbiter is refused at assembly when the
   roster does not hold it, and when it is also on the panel, since it would
   arbitrate over a finding it had just written. No panel finding is formed
   knowing what an arbiter said, and no arbiter reads another's.
2. **The panel is handed, not gated.** It is the brief's own answer so far,
   produced inside this dispatch; no desk facility holds it and no new
   capability is introduced. What the arbiter may reach beyond it is still
   gated: the Meta Brain's manifest grants `read_research_memory` and
   nothing else.
3. **Research memory carries the regime.** `qip_agents::memory::Episode`
   gains `regime: Option<String>`, keyed `{market}/{volatility}` exactly as
   `Platform::regime_context` keys it; the kernel stamps each charged call
   with the regime its kernel episode was formed in. An episode with no
   regime is left out of `ResearchMemory::reliability_by_regime` rather than
   filed under a regime nobody recorded, and the record is point-in-time in
   both instants (happened by `as_of`, resolved by `as_of`).
4. **The brief carries the regime, typed.** `AgentBrief::regime`, set by
   `stage_reason` from the same first affected object `recall_precedent`
   keys on, and never written into `context`, which the reviewer's lesson
   matcher substring-matches. A brief with no regime is read as pooled over
   every regime, and the finding says so.
5. **Three scopes, one per clause**, in `coverage::Scope`:
   - `RegimeReliability`, evidenced by `regime_reliability_cells` — emitted
     only when memory holds at least one (agent, regime) record, so an empty
     memory does not claim the scope with a zero. Each cell's hit rate is a
     `reliability:{agent}@{regime}` fact.
   - `AttentionAllocation`, evidenced by `attention_allocated`: the
     specialists measured in the regime in force (the panel's authors when
     there is a panel), each given weight in proportion to its hit rate
     there (`attention_weight:{agent}`). A specialist with no resolved call
     in that regime is counted under `attention_unmeasured` and given no
     weight — an even-chance prior would invent a record the memory does not
     hold, the error `ResearchMemory::hit_rate` already refuses. When every
     measured record is zero, proportional attention is undefined and none
     is allocated.
   - `DisagreementResolution`, evidenced by `reliability_weighted_balance`:
     only when the panel is contested, the directional findings re-weighed
     by `effective_conviction × hit rate in the regime`, in `[-1, 1]`. Where
     no directional author has a record, the finding says the panel is
     contested and unresolved, and the label is absent.
6. **A resolution is a fact, never a vote.** The Meta Brain's finding is
   neutral at zero conviction, so `OrganisationReport::balance` and
   `is_contested` are unchanged by it, and it is never a second, correlated
   copy of the panel it read. REASON reads the resolution beside the panel's
   balance; nothing in this record makes it act on either.

## Consequences

- The roster is twenty-eight; the organisation runs twenty-seven agents,
  twenty-six on the panel and one after it. `README.md`,
  `docs/architecture/README.md` and the documentation suite's `CARDINALS`
  move with it.
- The kernel's REASON dispatch now runs the Meta Brain on every convened
  panel, under the regime in force:
  `qip-kernel/tests/episodic.rs::the_reason_stage_runs_the_meta_brain_under_the_regime_in_force`.
- Research episodes written before this change carry no regime and stay out
  of every per-regime record; they still count in the pooled hit rate.

## Not decided here

- Whether REASON should *use* the resolution — to weight the hypothesis, to
  convene a further specialist, or to route attention in the next cycle — is
  a decision about the decision loop, not about the Meta Brain, and is left
  open. Today the resolution is reported and journaled with the run.
- Reliability of *models* (forecasters, production models) by regime: the
  tournament and the deep brain's registry hold no regime key; the
  model-failure probe reads their health un-regimed.

## What would make this wrong

- A second writer of panel findings into an arbiter's input other than
  `dispatch`'s own first phase: an arbiter that could be handed an arbitrary
  panel could be made to "resolve" findings nobody produced.
- An arbiter granted a vote. A Meta Brain that took the direction it
  resolved to would double-count every finding it weighed.
