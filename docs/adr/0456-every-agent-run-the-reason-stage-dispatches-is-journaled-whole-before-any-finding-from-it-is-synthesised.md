# ADR 0456: Every agent run the REASON stage dispatches is journaled whole before any finding from it is synthesised

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation**,
  session t50-15 (wave580). An in-repository design decision: it adds a
  record to the event log and a fail-closed refusal to the REASON stage. It
  widens no authority, grants no agent a capability, adds no dependency and
  touches no paper-trading layer.
- **Date**: 2026-11-12
- **Rows**: R12-AMBIENT-9029 (stays PARTIAL; see "What this leaves open").

## Context

R12-AMBIENT-9029's verification reads: "An agent can invoke only tools in
its explicit grant and its outputs are journaled." The first half already
held. `AgentContext::authorise` (`qip-agents/src/runtime.rs`) is the single
enforcement point, records every attempt with whether it was granted, and
refuses what the manifest does not name; `ToolRegistry::authorise` does the
same for registered tools.

The second half did not. `Platform::stage_reason` dispatched the panel and
kept two counters (`qip_agent_runs_total`, `qip_permission_denials_total`)
and a problem string per violating agent. The full `AgentRunRecord` — the
manifest digest the run executed under, the brief, every access and its
grant, the spend, the finding — lived only in the organisation's in-memory
`AuditTrail`. A restart erased it. A hypothesis built from the findings
could not be reproduced from the log, and an agent that probed for a
capability its manifest withholds left a number, not evidence.

`Topic::AgentRunCompleted` already existed and nothing in production wrote
it.

## Decision

1. **Each run is journaled whole.** After `Organisation::dispatch` returns,
   the REASON stage journals one `qip_kernel::agent_journal::AgentRunJournal`
   per run — the cycle number and the unabridged `AgentRunRecord` — on
   `Topic::AgentRunCompleted` through `Platform::journal_record`, so the same
   frame reaches the hash-chained event log and the journal. Denied accesses
   are part of the record, so a refused attempt is on the log beside the
   output the agent returned.
2. **Not a summary.** A summary would be a second claim about the run, and
   two claims about one fact disagree. The record the log holds equals the
   record the audit trail holds, and the test asserts that equality.
3. **Keyed on the run id.** The idempotency key is `agent-run:<run_id>`. The
   organisation mints a run id once per dispatch, so a re-delivered record is
   suppressed. The prefix also separates an agent run from any other body on
   the same topic (the acceptance suite seals an `AttentionEvent` there).
4. **Fail-closed.** If any run cannot be journaled, the stage forms no
   hypothesis that cycle and reports each unjournaled run as a problem. A
   hypothesis resting on findings the log never kept is one nobody can
   reproduce. The runs that did reach the log stay there.

## Evidence

`qip-kernel/src/agent_journal.rs` tests, each mutation-verified on
2026-11-12:

- `every_run_the_panel_makes_is_on_the_log_with_the_finding_it_returned` —
  fails when the journal write is dropped (log empty).
- `an_agent_reaching_past_its_grant_is_refused_and_the_refusal_is_on_the_log`
  — seats a rostered research agent that reaches for `RunSimulation`; fails
  when the journal write is dropped and when `AgentContext::authorise`
  grants everything.
- `a_panel_whose_runs_the_log_refused_forms_no_hypothesis` — fails when the
  fail-closed return is removed.

## What this leaves open

The requirement text also names Fabric task/result interfaces, knowledge
APIs and multimodal reasoning. None is built here:

- No Fabric task topic or result envelope carries a brief to a specialist;
  the panel is still dispatched in-process by `Organisation::dispatch`.
- No knowledge-API seam exists for research agents beyond the `Desk` views.
- Multimodal reasoning has no code in the tree.

R12-AMBIENT-9029 therefore stays PARTIAL.

## What it costs

- One record per agent run per convened cycle on the log, carrying the
  brief and the finding, so a convened cycle writes as many frames as the
  roster ran. The roster is fixed by committed manifests, so the growth is
  bounded by the roster and the cycle cadence, not by input volume.
- A log that refuses an append now stops REASON forming a hypothesis that
  cycle. Before, the panel's output still reached synthesis. That is a
  cycle with no thesis rather than a thesis with no record, chosen on
  purpose (principle 3, fail closed).

## Consequences

- A replay can now answer which specialist said what, under which manifest,
  and what it was refused, without the process that ran it.
