# ADR 0417: The ambient door gains no causal simulation, and a warm engine host waits on a producer of declared plans

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11**, session fix-c-01. It refuses; it builds
  nothing, widens no authority, reverses no accepted refusal and touches no
  paper-trading layer. What it leaves open is named as the owner's.
- **Date**: 2026-11-12
- **Reads**: [ADR 0145](0145-an-ambient-draft-has-no-identified-effect-so-the-unidentified-effect-is-recorded-and-acts-on-nothing.md)
  decisions 4, 5 and 6, and
  [ADR 0146](0146-the-ambient-agency-door-drafts-under-one-identity-is-a-registered-brain-and-not-an-organisation-agent-and-its-assertions-are-the-claims-its-citations-carry.md)
  decision 11. Amends neither.
- **Rows**: R12-AGENCY-7027 (stays PARTIAL).

## Context

R12-AGENCY-7027 reads: "The Causal Agency/Action Orchestrator runs as
slow/warm services with typed tool adapters, a policy/identity service,
causal simulation access and a durable action journal." Its integration
check — an action passes a typed adapter after policy and identity checks and
is journaled durably — is met by the deep brain's ambient door
(`qip-deepbrain/src/ambient_agency.rs`): the `ActionGuard` over the
`ToolRegistry`, the `DraftingAdapter`, the hash-chained outbox. An
independent check on 2026-11-12 held the row PARTIAL because that door, the
warm service graded as the orchestrator, has no causal simulation: it holds
no `ScenarioSource` and calls no `engine::run`. Simulation exists in
`qip-agency`'s engine (`generate::Structural`, `ScenarioSource`) and is
hosted only by the shadow binary (`qip agency shadow`, `qip-cli/src/agency.rs`
`TwinScenarios`), which runs on demand and is not a warm service.

The spawn that raised this asked whether the door can gain simulation access
without reversing ADR 0145's refusal, for example a scenario source it
consults before a draft that has a plan.

## Decision

1. **The ambient door gains no simulation access.** Every way to give it
   one that does anything reverses an accepted decision:
   - Simulation in `qip-agency` runs a candidate through structural models
     against scenarios. A scenario source alone changes nothing: with no
     model of what a draft moves, "draft" and "do nothing" simulate to the
     same result under every scenario. The model is what ADR 0145
     decision 4 refuses to write ("No structural model of 'a notice to a
     person changes X' is written").
   - Consulting simulation through the engine is `engine::run`, which ADR
     0145 decision 5 refuses on the ambient door and ADR 0146 decision 11's
     composite suite pins absent (`qip-acceptance`
     `ambient_effect_boundary.rs`, `FORBIDDEN_ON_THE_AMBIENT_DOOR`).
   - Scenarios from the world-model federation are ADR 0145 decision 6's
     refused federation-backed simulator.
   - "A draft that has a plan" does not occur: an ambient draft is one tool,
     one step, chosen by the attention router and the promotion policy before
     the door sees the signal (ADR 0145 decision 5). An arm keyed on a plan
     the door never receives would have no producer in the tree, and decision
     28's fifth preference declares such an arm absent rather than building
     it on an invented input.

   A source wired to the door that the door consulted and that changed
   nothing would satisfy a reading of the words and give the requirement
   nothing; it is refused for that reason too.
2. **A second warm door hosting `engine::run` is not built.** It would not
   reverse ADR 0145, whose refusal is of the ambient door, but it has no
   producer: the engine runs a declared `Request` (structural models,
   scenarios, objective, actuation), and the only producer of one is a
   person passing a document to the shadow binary. No warm service receives
   a declared plan, no route or queue carries one, and inventing that channel
   would build a door for input nobody sends. Absent, with this reason.
3. **The row's simulation clause stays unmet, and closing it is the
   owner's.** Two routes exist, and each needs something this delegation
   does not hold:
   - reverse ADR 0145 decisions 4 and 5 (and ADR 0146 decision 11's pin) so
     the ambient door runs the engine over a structural model somebody
     writes — the reversal of an accepted refusal, reserved to the owner by
     decision 28; or
   - name a producer of declared plans that reaches a warm service (who
     writes them, through what channel, under which identity), after which a
     warm host of `engine::run` in shadow is buildable without touching ADR
     0145.

## Consequences

- R12-AGENCY-7027 stays PARTIAL. Its integration check holds and is kept.
- No code changes. The ambient door's boundary pins stay as they are.

## What it costs

The requirement's simulation clause stays open on a warm service, and the
row stays PARTIAL. A desk that wants a planned action simulated still runs
the shadow binary by hand, against declared models and the twin's futures,
and no warm process does it for them.

## What would make this wrong

- If an ambient draft came to carry a declared plan from some producer, the
  plan-keyed arm of decision 1 would have a producer, and the question would
  reopen against ADR 0145 decision 5, not against this record.
- If the requirement's wording were read by the owner as satisfied by the
  shadow binary's on-demand simulation, the row would close by a change of
  reading, which is the owner's (a change to a requirement's wording).

## Evidence

- `grep -n 'ScenarioSource\|Structural\|engine::run' backend/crates/apps/qip-deepbrain/src/ambient_agency.rs`
  prints only the module doc line saying the door runs the guard, not
  `engine::run` (2026-11-12).
- `cargo test -p qip-agency --no-fail-fast`, the row's verification: 8 test
  binaries, 158 passed, 0 failed (2026-11-12).
