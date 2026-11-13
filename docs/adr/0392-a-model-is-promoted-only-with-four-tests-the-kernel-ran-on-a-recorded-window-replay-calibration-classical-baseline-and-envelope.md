# ADR 0392: A model is promoted only with four tests the kernel ran on a recorded window: replay, calibration, classical baseline and envelope

- **Status**: **Accepted 2026-11-11 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Session l2-d2-01 writes
  it. The limits of the delegation hold:
  - it sets no figure: no threshold, bound or count is introduced;
  - it only adds a refusal, and relaxes no existing one;
  - it touches no paper-trading layer and adds no order path;
  - it adds no dependency and no crate edge (`qip-kernel` already depends
    on `qip-ai` and `qip-training`);
  - `qip-contracts` is not touched: the new types are data on the model
    card in `qip-ai`, and the behaviour is in `qip-kernel`.
- **Date**: 2026-11-11
- **Extends**: ADR 0083 (the promote stage) and ADR 0006 (a classical
  baseline every time). It amends no clause of either. The registry's
  existing refusals are kept and run first.
- **Rows**: R12-MODEL-10202.

## Context

The requirement is an anti-pattern stated as an invariant: no model is
promoted without replay, calibration, classical-baseline and deterministic
risk-envelope compatibility tests. Its acceptance is one sentence: promotion
refuses a candidate missing any of the four.

Before this ADR a card with a passing evaluation was promoted.
`ModelRegistry::promote_artifact` checked the digest, the evaluation, the
feature lineage and, for a self-fitted card, the shadow period. The kernel
added the verifier society and, for a quantum-informed candidate only, a
calibrated baseline. Nothing ran the artifact again over recorded inputs.

Three sessions left the row at a design question. Two decisions were open:
which recorded data a replay runs over, and what a model's output must
respect while no model output feeds a risk limit. A third question followed
from them: who computes the verdicts, since a gate whose evidence the caller
asserts is a gate the caller can pass by assertion.

## Decision

1. **Four named tests, one record.** `qip_ai::registry::PromotionTests`
   holds the artifact digest tested, the recorded window by name, its row
   count, and a result for each of `replay`, `calibration`,
   `classical_baseline` and `risk_envelope`. Each result is a verdict and
   the sentence that says what was measured. The record sits on the model
   card, so it is journaled and restored with the card.

2. **The registry refuses.** `ModelRegistry::promote_artifact` refuses an
   artifact whose card holds no tests, lacks any one of the four, holds a
   failed one, or holds tests run on another digest. The refusal names the
   test. It runs after the card's own refusals and before the card moves.
   The gate is in the registry and not in a producer arm, so every producer
   meets it: `TrainingPipeline`, `SelfFitted` and `QuantumInformed` alike.

3. **The kernel computes the verdicts.**
   `qip_kernel::central::models::run_promotion_tests` takes the artifact,
   the provider that will serve it, and a recorded window, and writes the
   record. The caller supplies the record of what happened and no verdict.
   `Platform::test_model_for_promotion` is the same call through the
   platform's own provider.
   - **Replay.** Two separately served instances score every recorded row.
     The test passes when every pair of outputs is the same bit for bit.
   - **Calibration.** A least-squares scale and offset from the outputs to
     the realised values is measured (`Calibration::fit`). The test passes
     when one can be measured. An output that does not vary has none.
   - **Classical baseline.** The outputs' root-mean-square error must be
     strictly below the zero-move predictor's on the same rows. The
     zero-move predictor is the classical no-skill forecast of a return and
     needs no parameter. The window's own mean is not used: it would be
     computed from the values it is scored on.
   - **Envelope.** Every output must be finite. The largest output and the
     largest recorded move are written beside the verdict as a measurement.

4. **The recorded window is rows the process observed.** For the one
   production producer, the deep brain's learning desk, the window is the
   rows its last shadow round scored: the subject's own bars that arrived
   after the fit, with the move each one's next bar realised. The committed
   synthetic tape `data/datasets/loop-demonstration-tape.json` is not used.
   It is outside the build context of the image, so using it would need a
   new file in the image and a new setting on every service, and a process
   started without it would refuse every promotion. The observed rows are
   already in the process and need neither.

5. **The signed record carries each test.** `ModelPromotion::stages` gains
   one line per test, with what was measured and the window's name. The
   stages are inside the centre's signature, so a test line edited after
   the fact does not verify.

6. **Two arms are declared absent.**
   - *An issued risk envelope.* `qip_contracts::risk_envelope` defines a
     `RiskEnvelope` with a `HealthRequirement` for a model, and nothing in
     the tree issues one. No model output feeds a `RiskLimit` either:
     pre-trade checks are deterministic and never read a model. So there is
     no issued envelope for a candidate to be tested against. The envelope
     test is therefore read as the one property a model must have for any
     deterministic check to carry its output: the output is a finite
     number. A non-finite score compares false against every threshold,
     which reads as a quiet decision and not as a breach.
   - *A bound on the size of an output.* A magnitude bound is a figure, and
     nobody has given one. A bound taken from the window itself (no output
     larger than the largest recorded move) was considered and refused: a
     sound fit on low-noise data predicts a move within a residual of the
     largest one, and the comparison then turns on the sign of that
     residual. The measurement is recorded and judged by nothing.

## What changes and what does not

A promotion that would have succeeded on a passing evaluation alone is now
refused until the four tests are on the card. Rollback is unchanged: it
returns a version at the digest it was first promoted under, and that
version was tested when it was promoted. A card restored from a log written
before this ADR holds no tests. It stays promoted, because the log says it
was, and it is refused if it is published again.

No limit moves. No model gains authority. The verifier society, the shadow
period and the quantum-informed baseline comparison all still run.

## What it costs

Every fixture that promotes a model now runs the tests first. The desk
serves the candidate twice more per promotion and scores each recorded row
twice. A candidate that beat production on its last shadow round and is
still worse than predicting no move is refused, where before it was
promoted.

The calibration test is weak by design. It refuses a model for which no
calibration can be measured, and it does not judge the calibration it
measures, because a tolerance on a scale or an offset is a figure.

## What would make this wrong

- A model output begins to feed a risk limit, or something issues a
  `RiskEnvelope`. The envelope test must then be extended by a new ADR to
  test against that limit or envelope, and the first absent arm closes.
- The owner gives a magnitude bound or a calibration tolerance. Either
  becomes a judged part of its test.
- A second producer appears whose recorded window is not rows the process
  observed. Decision 4 must then say what that producer's window is.

## Evidence

- `backend/crates/libs/qip-ai/tests/model_infrastructure.rs::an_artifact_is_refused_while_any_one_of_its_four_promotion_tests_is_missing_failed_or_on_other_bytes`
- `backend/crates/runtime/qip-kernel/tests/promotion_tests.rs`, seven tests,
  one per verdict and one for the signed record.
- `backend/crates/apps/qip-deepbrain/src/learning.rs`
  `tests::a_self_fitted_candidate_runs_in_shadow_and_is_promoted_only_after_winning_its_rounds`
  asserts the desk's promotion carries the four tests on the subject's own
  window.
- Mutations are listed in the notes of R12-MODEL-10202 in
  `docs/blueprint/assessment/R12-G3.json`.
