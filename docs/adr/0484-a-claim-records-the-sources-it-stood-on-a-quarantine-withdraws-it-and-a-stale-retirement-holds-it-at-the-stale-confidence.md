# ADR 0484: A claim records the sources it stood on; a quarantine withdraws it and a stale retirement holds it at the stale confidence

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation**
  (a session decides the in-repository design question it meets, by ADR),
  session t50-29 (wave580). The delegation's limits hold: it sets no figure
  the owner never stated, changes no requirement's wording, reverses no
  accepted refusal, adds no dependency and touches no paper-trading layer.
  The one constant it reuses — `STALE_SOURCE_CONFIDENCE` — is already
  committed in `qip-world-model`.
- **Date**: 2026-11-12
- **Rows**: R12-RES-7097 (stays PARTIAL; this closes one of its three gaps).

## Context

R12-RES-7097 asks that a poisoned or stale source have its evidence
quarantined and the dependent beliefs and models downweighted or invalidated.
Two thirds of that was already true and journaled:
`Platform::assess_sources` retracts a quarantined source's facts through
`WorldModel::quarantine_source`, restates a retired source's at
`STALE_SOURCE_CONFIDENCE`, and the learning engine's data-poisoning critic
refutes a model trained on a quarantined dataset.

What was missing is the half the world model cannot reach. A retraction
narrows what is **read afterwards**. A claim the platform had already made on
the poisoned source's word stayed open, was settled when its horizon passed,
and `Platform::learn_from`'s belief calibration — which is what later theses
are sized against — learnt the platform's own confidence from evidence the
platform had stopped believing. The quarantine stopped the next poisoned fact
and none of the conclusions already drawn from the old ones.

The obvious reading of "beliefs citing it" was the evidence set: withdraw a
claim whose `EvidenceSet` names the source as an origin. That reading does
not work, and the reason is the decision below. A hypothesis's evidence
origin is the **agent or the detector that reported** —
`qip_reasoning_engine`'s own module note says so, and the anomaly path writes
`anomaly.detector` as the origin. No shipped configuration puts a data
source's identifier there. A control keyed on evidence origins alone would
have withdrawn nothing, ever, while reading in the diff as protection: the
`MaxExpectedShortfall` failure in a new place.

## Decision

1. **A claim records what it stood on.** `RecordedPrediction` carries
   `sources: BTreeSet<String>`: the origins of the hypothesis's evidence
   **and** the sources whose facts about the claim's subject the world model
   held when the claim was made (`Platform::graph_sources_for`). The second
   half is what makes the rule able to fire, because instrument-level facts
   carry the identifier of the source that stated them wherever they entered
   through the kernel's knowledge intake.

2. **Attribution is read where the claim is made, not where a source
   moves.** By the time a source is quarantined its facts are retracted, so
   asking the graph then names nobody and every claim survives the poisoning
   that just emptied the ground under it.

3. **A quarantine withdraws the claim; the reason stays on the record.**
   `RecordedPrediction::withdrawn` holds the sentence naming the source and
   why, and `Platform::score_predictions` passes a withdrawn claim over, so
   the calibration never grades the platform against a source it stopped
   believing. Withdrawn rather than deleted: what was claimed, and why it was
   dropped, is the audit trail.

4. **A retirement is not a finding against the evidence.** A feed retired for
   cost or staleness said nothing false, so its claims stand and are held at
   `STALE_SOURCE_CONFIDENCE` (0.5) of their stated confidence — the identical
   rule `WorldModel::mark_source_stale` applies to the source's facts, so the
   fact and the claim formed from it are not held at two different strengths.

5. **Attribution is wide, and matching is exact.** A claim about an
   instrument a poisoned source has been describing is withdrawn whether or
   not the poisoned statement is the one the panel leaned on: the narrow
   reading fails open, this one fails closed. Matching is equality on the
   identifier and never a substring, so `feed-a` cannot withdraw a claim that
   stood on `feed-ab`.

6. **The assessment reports both lists.** `SourceAssessment` carries
   `claims_invalidated` and `claims_downweighted`, merged per entry by
   `DiscoveryDesk`, so one pass's effect on held beliefs is a value a reader
   can check rather than a side effect only a test can see.

## What was rejected

- **Withdrawing queued theses instead.** DECIDE drains `pending_theses` in
  the same cycle REASON fills it, so the queue holds nothing between passes
  and a rule over it could not fire. It was built, measured, and removed for
  that reason.
- **Keying on evidence origins alone.** See the context: it fires in no
  shipped configuration.
- **Re-scoring the hypothesis's confidence arithmetic.** `Hypothesis`
  recomputes confidence from its evidence and `validate` rejects a record
  whose confidence has drifted from its inputs, so rewriting a held
  hypothesis's number would make the record fail its own audit. The claim —
  the thing calibration and sizing read — is where the downweight belongs.

## What it costs

- A `BTreeSet<String>` and an `Option<String>` on every recorded claim,
  bounded by `PREDICTION_HISTORY` as the rest of the working set is.
- One graph read per claim, over the facts believed at the claim's instant,
  filtered to its subject.
- Wide attribution withdraws more than the minimum: a quarantine costs the
  platform claims that did not depend on the poisoned statement, and the
  platform makes them again from the narrower graph on the next cycle.

## What would make this wrong

- If instrument-level facts stopped carrying their stating source's
  identifier, attribution would narrow back to agent names and the rule would
  stop firing. The test that proves it absorbs a fact through
  `Platform::absorb_knowledge` and asserts the claim names the source; if
  that premise ever passes vacuously, the control is gone.
- If a second writer of `RecordedPrediction::withdrawn` appeared outside
  `assess_sources`, a claim could be withdrawn for a reason nobody
  journaled. The invariant to check is that the quarantine arm is the only
  place that sets it.
- If calibration began reading withdrawn claims by another path, the
  withdrawal would be cosmetic. `score_predictions` is the one seam that
  settles a claim today.

## Evidence

`cargo test -p qip-kernel --test source_lifecycle` — `test result: ok. 10
passed; 0 failed; 0 ignored`. Three tests are new:
`quarantining_a_source_withdraws_the_open_claims_that_stood_on_what_it_said`,
`a_withdrawn_claim_is_never_scored_so_calibration_reads_no_poisoned_evidence`
and `a_claim_standing_on_a_source_retired_as_stale_is_held_at_the_stale_confidence`.
Each was mutation-verified and the implementation restored byte-for-byte:
guarding the withdrawal loop with `false` fails the first two; dropping the
`withdrawn.is_some()` arm from `score_predictions` fails the second naming
the claim scored; making the downweight `*= 1.0` fails the third with the
original confidence against the stale fraction.

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D
warnings` are clean.

## Paper trading

Untouched. No execution, credential or autonomy path is in this change, and
all three layers stand as they were: Terraform's refusal of a live ceiling,
`AutonomyLevel::deployable` at the composition roots, and the `Cell` and
`Determinism` types. The change can only withdraw a claim or hold it less
firmly; nothing in it can admit an order.
