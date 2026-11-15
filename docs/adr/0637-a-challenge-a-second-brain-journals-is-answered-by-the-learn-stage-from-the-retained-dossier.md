# ADR 0637: A challenge a second brain journals is answered by the LEARN stage from the retained dossier

- **Status**: Proposed by wave b52 lane 18, 2026-11-15. A slice of
  R12-REASON-9032. It adds no dependency, figure or topic and touches none of
  the three paper-trading layers.
- **Date**: 2026-11-15
- **Rows**: R12-REASON-9032 (stays PARTIAL).

## Context

The verifier society attacks every important prediction as the cycle forms
it and journals the §27 pair on the tournament review.
`VerifierSociety::challenge_prediction`, the entry point for a request a
second brain raises, had no caller outside tests. The row's own remedy, a
fabric topic with a subscriber and a reply path, adds a `Topic` variant, and
every exhaustive match in the platform must then move.

## Decision

1. A request is journaled as `ChallengeRecord::Requested` under the existing
   `Topic::HypothesisChallenged`, producer `challenge-desk`, by
   `Platform::submit_challenge`. A repeated request id is refused.
2. The tournament retains the dossier of each prediction the society attacks
   (256, oldest dropped). `stage_learn` calls `Platform::answer_challenges`,
   at most 16 per cycle, oldest first; each is answered by
   `challenge_prediction` against the retained dossier.
3. A request for a prediction with no retained dossier is journaled as
   `Refused` with the reason, never attacked from a reconstruction.
4. A `Refuted` verdict is journaled with `rejected: true`;
   `Platform::rejected_by_challenge` reads it from the log.

## What remains

Nothing outside the kernel reads `rejected_by_challenge`, and no deep-brain
process submits a request: the fabric subscriber and reply path stay open, as
does making REASON honour a late rejection.
