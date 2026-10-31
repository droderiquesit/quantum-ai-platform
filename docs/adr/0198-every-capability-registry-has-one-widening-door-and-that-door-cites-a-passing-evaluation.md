# 0198. Every capability registry has one widening door, and that door cites a passing evaluation

- Status: accepted 2026-10-31, under the owner's 2026-10-31 delegation ("complete all sessions; finish what you can by morning") relayed by the wave parent, which assigned R12-EXPAND-6023 and R12-EXPAND-6070 to lane L4b-expand
- Supersedes nothing; amends the reading of ADR 0158 decision 8 and ADR 0141 only by citing them
- Rows: R12-EXPAND-6023 (invariant: each capability kind has a registry whose only widening path cites an evaluation), R12-EXPAND-6070 clause 2 (tools, agents, features and schemas have a staged, signed promotion gate or are named as not having one)

## Context

Requirement R12-EXPAND-6023 asks that every new capability — knowledge,
ontology, memory, specialist, tool, model, research — is evaluated,
sandboxed, lineage-tracked and promoted only through explicit gates, checked
by inspection: "each capability kind has a registry whose only widening path
cites an evaluation". R12-EXPAND-6070 asks the same of code, tools, agents,
features, schemas and models, with shadow operation and signed promotion.

An independent skeptic on 2026-10-25 found the invariant refuted, not because
the gates were missing but because each registry had a second door beside its
gate. `OntologyRegistry::register` accepted any higher version directly while
`extend` checked an evaluation nobody had to show. `ModelRegistry::register`
inserted any card, so a card built at `Production` skipped `promote`'s shadow
rule. `CapabilityRegistry::add_capability` was public and cited nothing.
`PromotionPolicy`'s floors were public fields a caller could set to zero, so
`ResearchMemory::promote` could be made a gate that refuses nothing.
`ToolRegistry::promote` took an attestation string, `ResearchRegistry::record`
bypassed the experiment-reference check of `record_concluded`, and
`BrainRegistry::register` cited nothing. A gate with a door beside it reads as
protection and is not.

## Decision

1. **One door per kind.** For every capability kind the expansion engine can
   widen, exactly one registry method widens it, and that method refuses an
   entry that does not carry a passing evaluation, the record of one, or the
   staged evidence the kind's earlier ADR names. Every other method that
   writes the registry either admits only the starting state (version one, a
   read-only sandboxed scope, a shadow stage) or is crate-private.
2. **The doors, by kind, as the code holds them on 2026-10-31.**

   | Kind | Starting state | The one widening door | Evidence it demands |
   |---|---|---|---|
   | Model (self-fitted) | `ModelStage::Shadow` via the learning desk | `ModelRegistry::promote` | a passing evaluation, no lineage leak, a completed shadow of `MIN_SHADOW_ROUNDS` rounds won on fresh rows; `register` refuses a self-fitted card at `Production` without the same completed shadow |
   | Model (pipeline-trained) | registered card | `ModelRegistry::promote` | a passing evaluation and no lineage leak; the image it ships in is CI-signed and attested under Binary Authorization (ADR 0110, `deploy.yml`) |
   | Ontology / schema | version one via `OntologyRegistry::register` | `OntologyRegistry::extend` | a `CitedEvaluation` that passed; `register` refuses any version above one |
   | Capability (verb) | none | `CapabilityRegistry::promote` | a validated `CapabilityEntry` from a concluded, promoted campaign; refuses a duplicate implementation and any eligibility naming live; `add_capability` is crate-private |
   | Memory lesson | episodes recorded | `ResearchMemory::promote` under a `PromotionPolicy` | at least the policy's supporting episodes and smoothed support; the policy's floors are private and `PromotionPolicy::new` refuses fewer than two episodes or a smoothed floor at or under one half |
   | Tool scope | read-only, sandboxed, via `ToolRegistry::register` | `ToolRegistry::promote` | a non-blank evaluation reference (see "What stays open") |
   | Research verdict | `Verdict::Open` | `ResearchRegistry::record_concluded` | the campaign's own experiment references; `record` and `record_version` remain for versioned non-concluded verdicts (see "What stays open") |
   | Specialist / agent | none | none, by decision | ADR 0158 decision 8: a specialist proposal produces a reviewable `BrainSpec` and never an agent; `BrainRegistry::register` runs at start-up from the shipped roster only |
   | Code | a branch | CI: `ci.yml` gate, `deploy.yml` build, scan, sign and attest | Binary Authorization admits only the attested digest (ADR 0110) |
   | Feature definition | `FeatureStore::define` | none, by decision | a feature definition is an ingestion fact (what a source reports), not a capability the platform grants itself; it widens no authority and needs no promotion gate. A model that *uses* a feature is gated as a model |

3. **A gap registers a candidate once.** A type the research sweep proposes
   from an ontology gap (R12-EXPAND-6029) enters at version one, as a
   candidate. A repeated gap proposes the next version as an `OntologyPatch`
   and registers nothing; that version enters only through `extend` on a
   passing evaluation, because a gap raised twice is twice the question and
   not once the answer.
4. **The rule is held by tests that mutate the door.** Each door above has a
   test that removes its check and fails: `a_type_above_version_one_enters_the_ontology_only_through_an_evaluated_extension`,
   `a_self_fitted_card_registered_at_production_without_a_completed_shadow_is_refused`,
   `a_promotion_policy_with_a_floor_that_cannot_refuse_is_itself_refused`,
   `a_second_promotion_under_one_verb_merges_its_implementation_and_a_repeat_is_refused`,
   and the shadow and promotion tests of `qip-deepbrain/src/learning.rs`.

## What stays open

Named here so that the row's `gap` and this record agree.

- `ToolRegistry::promote` takes an evaluation *reference*, and its one
  production caller (`qip-deepbrain/src/language.rs`) passes the hosted
  provider's attestation string. That is self-attestation by the caller,
  not a stored evaluation record the registry resolves. Closing it needs an
  evaluation store the registry can read, which is not built.
- `ResearchRegistry::record` and `record_version` write a verdict without
  experiment references. Their production callers (`qip-quantum`'s and
  `qip-optimization-engine`'s registries) record versioned open or routing
  verdicts, not concluded research; a concluded verdict goes through
  `record_concluded`. The two methods are not removed, so the inspection
  clause is met for concluded verdicts only.
- `ResearchMemory::promote` counts a lesson's cited episodes and does not
  resolve them against the episodes it holds. The kernel's LEARN stage cites
  resolution ids that are not the analyst episodes' run ids, so resolving
  them today would refuse every lesson; aligning the two id spaces is a
  kernel change this record does not make.
- Shadow as *live paper operation* (R12-EXPAND-6070 clause 1) and the HSM
  model-pack key of ADR 0110 (clause 3, `artefact_signing` null in every
  environment) are untouched: the first is a design the learning desk does
  not have, the second a deployment.

## What it costs

- A type can no longer be seeded at a version above one. A deployment that
  wants to ship a vocabulary already at version three registers version one
  and extends twice, each time with the evaluation that justified the step;
  the shipped seeds in `symbolic_stage.rs` and `research.rs` all start at one
  and pay nothing.
- A journaled self-fitted card restored at `Production` without its shadow
  record is refused rather than restored. Cards promoted through `promote`
  carry the record; a card journaled before the shadow stage existed would
  have to be re-promoted, and the restore path counts the refusal.
- A caller that built a `PromotionPolicy` with one-episode floors for a test
  now builds it through `new` and is refused below two; four tests in
  `registries.rs` that registered version two directly now go through
  `extend` and show the evaluation.
- Nothing is faster or cheaper: each door does one more comparison.

## What would make this wrong

A capability kind that legitimately needs two writers above its starting
state — for instance a restore path that must re-admit a promoted entry
without re-running its evaluation. The model registry already has that case
and solves it by carrying the evidence on the card (the completed shadow),
so the restore path shows the same evidence the door demands. A kind that
cannot carry its evidence that way needs its own record, not a second door.

## Code it constrains

`qip-agents/src/registries.rs` (`register`, `extend`, `promote`,
`add_capability`), `qip-agents/src/memory.rs` (`PromotionPolicy`),
`qip-ai/src/registry.rs` (`register`, `promote`),
`qip-deepbrain/src/research.rs` (`propose_type`).
