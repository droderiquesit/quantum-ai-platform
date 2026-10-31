# 0192. Every capability class names its benchmark and its incumbent before it is admitted

- Status: accepted 2026-10-31. Number claimed by lane L4-quant on 2026-10-24 under the 2026-10-24 delegation; body written by lane L4b-expand under the owner's 2026-10-31 delegation, which moved the row to it
- Rows: R12-EXPAND-1014 (a candidate capability is admitted only after a recorded benchmark beats the incumbent; removing the benchmark gate lets a worse candidate in)
- Related: ADR 0198 (one widening door per registry), ADR 0157 (sources enter by licence record), ADR 0158 decision 8 (specialists never minted), DATA-018 (a redundant, stale source is retired)

## Context

R12-EXPAND-1014 asks that new information domains, ontology, specialist
brains, tools and models be added only through a controlled evaluation
pipeline that promotes measured improvements, and its check is concrete: a
candidate is admitted only after a recorded benchmark beats the incumbent,
and removing the gate lets a worse candidate in. On 2026-10-24 the triage
found that only three classes had such a gate: models through
`require_calibrated_baseline` and the shadow stage, strategies through the
evolution engine's champion, and fill-bias capabilities through the
`IncrementalValue` criterion in `qip-expansion`'s executor. Sources entered by
licence record and score alone, tools on an attestation string, specialists
as gaps, ontology types by version. A worse source than one already held was
admitted at the routing its own composite earned.

## Decision

1. **Each class names its benchmark and its incumbent, in this table, and a
   class with no incumbent says so rather than admitting on nothing.**

   | Class | Incumbent | Recorded benchmark | Gate |
   |---|---|---|---|
   | Model (self-fitted) | every production model of the same purpose, or the zero-return predictor | shadow rounds won on rows after everything the candidate read, `MIN_SHADOW_ROUNDS` of them | `ModelRegistry::promote`, with `register` refusing a self-fitted card at Production without the shadow (ADR 0198) |
   | Strategy | the crowned champion | the evolution engine's out-of-sample score on the synthetic environment | the engine's challenge; a challenger that does not beat the champion is not crowned |
   | Fill-bias capability | the uncorrected twin | shadow-window mean absolute error improvement of at least `CampaignPolicy::min_improvement_bps` | `Criterion::IncrementalValue` in `run_fill_bias_campaign`; `promotion::promote` refuses a failed criterion |
   | Source | the highest-composite registered source whose coverage includes every instrument the candidate covers | the finder's composite score (reliability, freshness, uniqueness, historical value, cost efficiency) | `DataFinder::assess_one`: a candidate that does not beat its incumbent is held to `Cold` routing with the incumbent named (`Routing::held_to_incumbent`); it earns Hot or Warm only by beating it |
   | Tool | none: a tool is a capability the platform grants itself, not one it measures against another | the evaluation reference its promotion cites (ADR 0198 names this as still a string) | `ToolRegistry::promote`; a tool starts read-only and sandboxed |
   | Ontology type | the latest registered version of the same name | a passing `CitedEvaluation` | `OntologyRegistry::extend`; `register` admits version one only (ADR 0198) |
   | Specialist brain | none, by decision | none | ADR 0158 decision 8: a proposal produces a reviewable `BrainSpec`, never an agent; nothing is admitted at runtime |

2. **A source's benchmark holds routing, not admission, and that is
   deliberate.** DATA-018 retires a redundant source once it is stale, and its
   fixtures register a redundant source precisely so the policy can retire it.
   Refusing the redundant source at the door would remove the retirement path
   the platform already proves. Holding it to `Cold` instead keeps it
   collected for history and research only, which is what a source that adds
   nothing over what is held is worth, and leaves DATA-018 to retire it. A
   candidate covering an instrument nothing holds has no incumbent and is
   routed on its own merits: the gate compares like with like or not at all.
3. **The incumbent is named in the record.** Every held routing's basis
   names the incumbent and both composites, so a reviewer reading the
   registration decision can see what the candidate was measured against and
   by how much it fell short.
4. **Removing the gate lets a worse candidate in, and a test says so.**
   `a_worse_candidate_source_is_held_below_its_incumbent_and_a_new_instrument_is_not`
   fails when the `held_to_incumbent` call is deleted. The model, strategy and
   fill-bias gates have their own mutated tests (`learning.rs`,
   `qip-evolution`, `qip-expansion/tests/executor.rs`).

## Consequences

- A second source on instruments the platform already covers is collected
  Cold unless it is better, so a vendor's duplicate feed costs its own fee and
  earns no hot routing.
- The ontology and model doors of ADR 0198 are the benchmark gates for their
  classes; this record names them as such rather than adding a second check.
- Tools and specialists have no incumbent, and this record says so instead of
  inventing a benchmark nobody measures.

## What it costs

- A better source arriving while a worse incumbent holds Hot routing is
  admitted Hot beside it; nothing here demotes the incumbent. The finder's
  monitor and DATA-018 handle the incumbent on its own evidence.
- The composite is the finder's own score, so a candidate scored on a quiet
  probe window may lose to an incumbent scored on a busy one. The basis
  records both figures, which is what makes that argument possible.
- The tool class keeps an attestation string as its evidence; ADR 0198 names
  the stored evaluation record it still lacks.

## What would make this wrong

A class that needs two benchmarks at once, or a source whose instruments are
partly covered: the second case is treated as having no incumbent, which
admits a candidate that is worse on the covered part and better on the
uncovered part. If that becomes a cost in practice, the benchmark narrows to
the covered instruments and this record is amended.

## Code it constrains

`qip-data-finder/src/scoring.rs` (`Routing::held_to_incumbent`),
`qip-data-finder/src/finder.rs` (`best_incumbent_for`, `assess_one`),
`qip-ai/src/registry.rs`, `qip-agents/src/registries.rs`,
`qip-expansion/src/executor.rs`, `qip-deepbrain/src/learning.rs`.
