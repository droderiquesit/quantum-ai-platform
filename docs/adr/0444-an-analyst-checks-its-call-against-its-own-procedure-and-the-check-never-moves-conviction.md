# ADR 0444: An analyst checks its call against its own procedure, and the check never moves conviction

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11**, session t50-09. An in-repository design
  decision: it widens no authority, adds no limit, touches no paper-trading
  layer and states no figure the owner did not.
- **Date**: 2026-11-12
- **Rows**: R12-WORLD-3034 (stays PARTIAL).

## Context

R12-WORLD-3034 asks for procedural memory — research procedures, strategy
templates, data adapters, playbooks and remediation patterns — and its
check is that "a procedure is stored, versioned and retrieved for reuse".
Until this decision an analyst's procedure was distilled on LEARN and
delivered on the brief at REASON, and nothing read it. Its steps were the
analyst's past calls as prose ("which way AAPL would move on an undervalued
claim: lean positive"), which no agent could execute.

## Decision

1. **A procedure is a method of rules.** `qip_agents::memory::ProcedureRule`
   is one executable step — on `subject`, lean `positive` or `negative`,
   supported by `support` correct calls. `ResearchMemory::record_method`
   records them under every refusal `record_procedure` already made, and
   refuses no rule, a blank or repeated subject, any other lean and zero
   support; the prose steps are rendered from the rules. The kernel's
   `distil_procedure` keys the rules on the `subject:` evidence each analyst
   research episode now carries (the majority lean of the analyst's recent
   correct calls on that subject, ties to the latest). Episodes without that
   evidence still give the old prose-only procedure, which holds no rule and
   so is checked by nobody.
2. **The procedure name has one spelling.** `procedure_name_for` lives in
   `qip-agents`; the kernel writes under it and the brief reads under it.
3. **An analyst executes its rule and the finding records it.**
   `AgentBrief::check_procedure` takes the agent's own procedure, its rule
   for the brief's first object, and the agent's present direction, giving a
   `ProcedureCheck`. Every analyst in `qip-investment-agents/src/analysts.rs`
   builds through `FindingBuilder::build_against(brief)`, which sets
   `AgentFinding::procedure` and states a departure as a caveat. The kernel
   reads the checks off the panel's findings into
   `Platform::procedures_followed`.
4. **The check never moves direction or conviction.** A procedure is the
   desk's record of what was right; feeding it back into the call would make
   past success manufacture more of the same view, and the precedent field
   (`AgentBrief::precedent`) is already held to the same rule. Caveats are
   read by nothing that computes. The test
   `an_analyst_checks_its_call_against_its_own_procedure_and_states_a_departure`
   asserts conviction is bit-identical with and without the procedure.
5. **Strategy templates, data adapters, playbooks and remediation patterns
   are not procedural memory yet.** Each would be a different record with a
   different consumer — a template is read by strategy construction, an
   adapter by ingestion's licensing gate, a playbook and a remediation
   pattern by operations — and none of those consumers reads memory today.
   Storing them with nothing to reuse them would repeat the defect this
   decision closes for procedures. They stay open on the row.

## Consequences

- The procedure is reused: delivered, executed by the agent, and recorded.
- Still open on R12-WORLD-3034, dated 2026-11-12: the four other kinds
  (decision 5); no test drives LEARN from a resolved cycle through to the
  next REASON — the kernel test still seeds memory through
  `write_research_memory`; `MAX_PROCEDURES` has no supersede path; and the
  chief and non-analyst agents build findings without `build_against`.

## What it costs

- A finding grows an optional field and an analyst a caveat line where it
  departs; serialisation is unchanged where no procedure applies.
- Rules key on the brief's first object only. A brief about several
  instruments is checked against one of them.
- A procedure that cannot move a call cannot improve one. If the desk later
  wants its procedures to inform conviction, that is a new decision with its
  own calibration evidence, not an edit to `build_against`.

## What would make this wrong

- Evidence that analysts' departures from their procedures resolve correct
  as often as their followed calls: the check would then be recording noise,
  and the procedure is not a method worth holding a call against.
- A consumer for strategy templates, data adapters, playbooks or
  remediation patterns reading memory: decision 5's reason for not storing
  them would no longer hold.
