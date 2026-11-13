# ADR 0486: A tool scope widens only on a held, passing evaluation record for that tool and grant

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation**,
  session t50-30 (wave580). An in-repository design decision: it narrows the
  one door through which a tool's scope widens. It grants no tool a scope,
  adds no dependency and touches no paper-trading layer.
- **Date**: 2026-11-12
- **Rows**: R12-EXPAND-6023 (stays PARTIAL; see "What this leaves open").

## Context

R12-EXPAND-6023's verification reads: "Each capability kind has a registry
whose only widening path cites an evaluation."

For tools, the widening path was `ToolRegistry::promote(name, grant,
evaluation: &str)` in `qip-agents/src/tools.rs`. It refused a blank
`evaluation` and accepted any other text. "Cites an evaluation" therefore
meant "a caller typed something": the registry held no evaluation, could not
say whether one existed, whether it was of this tool, of this grant, or
whether it passed. The one production caller, `qip-deepbrain`'s
`language::assemble`, passed the provider attestation's description string,
which happened to be meaningful, but nothing in the registry made it so.

## Decision

1. The registry holds evaluations. `ToolEvaluation` is a record: an id, the
   tool, the grant it evaluated, the evidence (what was looked at), and an
   `EvaluationOutcome` of `Passed` or `Failed`. A blank or whitespace-padded
   id and blank evidence are refused at construction.
2. `ToolRegistry::record_evaluation` holds at most one record per tool and
   grant. The tool must already be registered, so an evaluation cannot wait
   to authorise whatever is registered under that name later. A failed
   record may be replaced by a re-evaluation; a passing one may not, because
   it is the evidence a promotion rests on or is about to rest on. Storage
   is bounded by registered tools times the three permissions.
3. `promote(name, grant, evaluation)` resolves `evaluation` against the
   record held for that tool and grant. No record, a record with a different
   id, and a `Failed` record are each refused with `Error::denied`. The
   version a widening mints still cites the id, so `at_version` reads which
   record admitted it.
4. `qip-deepbrain` records the mounted attestation as the hosted endpoint's
   `LeaveSandbox` evaluation (evidence: the attestation's description) and
   promotes citing that record.

## Consequences

- A promotion on free text no longer compiles into a widened scope; it is
  refused at run time with a message naming the record to make first.
- The deep brain's evaluation is still the operator attestation it was
  before. What changed is that the registry now holds it as a record a
  reader can inspect, and the promotion cannot cite anything else.

## What it costs

- One more step for every caller that widens a tool: record the evaluation,
  then promote citing it. The deep brain's assembly gained one call.
- Up to three evaluation records per registered tool held in memory.
- A failed evaluation is replaced, not kept, when the tool is re-evaluated,
  so the registry does not hold a history of failed evaluations; it holds
  the history of widenings, each citing the record that admitted it.

## What would make this wrong

- If an evaluation needs to be produced by something other than the caller
  that promotes (an independent reviewer's signed record, say), this record
  is still supplied by that caller and the door would need to verify who
  made it. The deep brain's evaluation is the operator attestation it read
  from a mounted file; the registry checks it is held, for this tool and
  grant, and passed, and not who wrote it.
- If one tool and grant need several evaluations in force at once (for
  example per-environment), one record per pair is too narrow.

## What this leaves open (2026-11-12)

The row covers seven capability kinds. This decision closes the tool door
only. Still open, as triage recorded them:

- **Research**: `ResearchRegistry::record` and `record_version` still accept
  a concluded `Null`/`Supported` verdict with no experiment reference; only
  `record_concluded` demands one.
- **Memory**: `ResearchMemory::promote` counts `supporting_episodes` without
  resolving them against held episodes.
- **Ontology**: the sweep-6029 candidate cites a gap, not an evaluation.
- The skeptic round the row's notes say never ran has not run.

## Evidence

`cargo test -p qip-agents --test expansion_registries`: three tests added
(`a_promotion_citing_an_evaluation_the_registry_does_not_hold_is_refused`,
`a_held_evaluation_widens_only_its_own_tool_and_grant_and_only_when_it_passed`,
`a_passing_evaluation_is_not_replaced_and_an_unregistered_tool_holds_none`).
Each refusal branch in `promote` and `record_evaluation` was mutated to admit
and the suite failed; restored byte-for-byte and it passed. Deleting the
deep brain's `record_evaluation` call fails
`language::tests::a_pinned_provider_an_attestation_naming_it_and_a_credential_install_the_hosted_model`.
