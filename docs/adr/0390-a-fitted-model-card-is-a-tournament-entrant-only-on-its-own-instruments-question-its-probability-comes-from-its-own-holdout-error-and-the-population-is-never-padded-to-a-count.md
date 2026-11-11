# ADR 0390: A fitted model card is a tournament entrant only on its own instrument's question, its probability comes from its own holdout error, and the population is never padded to a count

- **Status**: **Accepted 2026-11-11 under the owner's 2026-11-11 delegation
  to the orchestrator**, batch l2-d1-01. The owner ruled on 2026-10-31 that
  the tournament's population is the learning desk's own fitted models
  (ADR_THEN_IMPLEMENT) and gave this ADR its title. The delegation's limits
  hold:
  - it sets no limit, threshold, rate or cost, and the conversion below
    contains no constant anybody chose;
  - it touches no paper-trading layer and adds no path to an order;
  - it adds no crate edge: `qip-deepbrain` already depends on `qip-kernel`;
  - it puts no behaviour in `qip-contracts`.
- **Date**: 2026-11-11
- **Extends**: ADR 0149 decisions 12 and 13 (the tournament, and a count is
  not a goal), ADR 0347 (a temporary entrant's own small fitted model),
  ADR 0083 (one serve, no second arithmetic).
- **Rows**: R12-MODEL-3089.

## Context

`TournamentStage` admitted a detector per hypothesis class, an analyst per
informative panel finding and at most one temporary specialist per subject.
It admitted no fitted model. `TournamentStage::verify_model` nevertheless
read a `model:<reference>` record when a card was put to the verifier
society, a record nothing could have written.

A fitted card predicts one number: its own instrument's return from the
newest bar to the next. The tournament asks for a probability that the close
is above a reference when a horizon ends. Something has to turn the one into
the other, and every obvious way of doing it invents something. A normal
error assumption invents a shape. A logistic squash invents a scale. A fixed
confidence invents a figure. The holdout RMSE alone does not determine a
probability without one of them.

## Decision

1. **The probability is a count of the card's own held-out errors.** The
   desk scores the card, through the platform's provider, on the round's
   held-out tail and keeps each row's realised error, `target − score`. It
   scores the newest bar's feature row for a point prediction. It counts the
   held-out rows for which `prediction + error > 0`. The offer carries that
   count and the row total and no probability.
   `P(up) = (ups + 1) / (rows + 2)`, derived in the kernel
   (`CardForecast::p_up`). That is the rank of zero in the split-conformal
   predictive distribution of the next return, with the Laplace smoothing
   the temporary specialist already uses (ADR 0347), so a card with few rows
   cannot say 0 or 1. No error distribution is assumed. Exchangeability of
   the held-out rows with the next one is assumed, as
   `qip_training::ConformalInterval` already assumes it, and the
   tournament's Brier score is what tests the assumption.
2. **A card enters under `model:<reference>`** as a
   `ForecasterKind::Specialist`, the id `verify_model` already reads.
3. **A card answers only the question its fit is about.** It is entered on a
   question only where the question is on the subject its card records
   (`central::models::subject_of`), the question's horizon equals the
   spacing of the bars it was fitted on, the question measures from the very
   close the prediction is a return from, and that bar is still the newest.
   Otherwise it is not entered, and the review's `refused` list says which
   card and why. The horizon rule is equality and not the lattice band: a
   five-day question and a one-day bar share a band and are different
   quantities.
4. **Only a card a decision may use is an entrant.**
   `Platform::stage_card_forecasts` refuses a card that is unregistered,
   records another subject, or fails `ModelCard::decision_eligibility` at
   the offer: development-stage, retired, failed, stale or drifted. It
   judges against the desk's registry, because the platform's journal mirror
   skips a drift-only change by design.
5. **The population is exactly what the desk offered last.** An offer
   replaces the subject's whole set, so a card the desk stops offering has
   left; an offer older than its own bar is dropped where it is read. No
   card is entered to reach a number, and an empty offer leaves the subject
   with no card. ADR 0149 decision 12 stands: a count is not a goal.
6. **Nothing new is spent.** The serve is the one promotion and renewal use,
   on the deep brain's existing round. The entry is unmetered (`cost: None`)
   because nothing metered it.
7. **Entry gates nothing by itself.** A card's tournament record reaches a
   decision only where a forecaster's already does: its promotion review by
   the verifier society, which can refuse and cannot enlarge.

## Consequences

- The learning desk's production models compete on the questions the
  platform asks about their instruments, and are scored when those resolve.
- `verify_model` reads a record that can now exist.
- On a subject whose hypotheses carry a horizon other than one bar, no card
  enters, and the review says so each time. That is the honest reading: the
  card did not forecast that question.

## What it costs

- One serve per production card on the round's subject, over the held-out
  rows plus one. No cycle-path cost: the kernel reads counts.
- The offer covers only the round's subject. Another subject's cards stand
  until their own bar ends and are offered again when that subject is next
  fitted.
- For a card fitted in an earlier round the tail scored is this round's, not
  the one on its registration. It lies at or after the card's own tail while
  history only grows; a window that slid backwards would break that.

## What would make this wrong

- A window assembled so that this round's held-out tail overlaps rows an
  older card was fitted on. The count would then include in-sample errors
  and read as more confident than the card has earned. The tournament's
  score would show it, late.
- A second producer of `CardForecast` that offered a probability instead of
  the counts. The type carries no probability field so that cannot be done
  without changing it.
- Relaxing decision 3 to the lattice band to raise the number of entrants.
  That is padding by another name.
