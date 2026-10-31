# ADR 0226: A concluded microstructure delta is read by the centre's divergence hold; a residual may keep a venue's hold but never lift one or size anything

- **Status**: **Accepted 2026-10-31 under the owner's delegation**: decision 10
  of `docs/blueprint/lanes/wave1031-owner-rulings-2026-10-31.json`, relayed
  by the integration parent, which assigned R12-CONTRACT-7074 to lane
  L2-INTEL. This record sets no risk number and adds no threshold. It reuses
  R12-RES-7104's `REPLAY_DIVERGENCE_THRESHOLD_BPS` and
  `REPLAY_DIVERGENCE_MIN_SAMPLE` unchanged, and touches no paper-trading
  layer.
- **Date**: 2026-10-31
- **Extends**: the replay-divergence hold
  (`backend/crates/runtime/qip-kernel/src/replay_divergence.rs`,
  R12-RES-7104), which narrows the grant of every strategy at a venue whose
  twin diverged and releases it only on a re-promotion.
- **Rows**: R12-CONTRACT-7074.

## Context

R12-CONTRACT-7074 asks for a `MicrostructureDelta`: what a cell saw a venue
do, set against what its model said the venue would do, sent back to the slow
brain as residuals. The contract exists in `qip-contracts`. The deep brain's
tick campaigns conclude deltas, and its ambient journal records each one as
`AmbientRecord::MicrostructureDelta`. Nothing read them. A residual that
reaches a journal and moves nothing is the histogram it replaced, under a
better name.

One control in the platform asks the question a delta answers: is the
twin's picture of this venue still wrong? That control is the replay-divergence
hold. It is raised from the twin's own fill errors. It is released when the
strategy is promoted again, and that promotion is argued on replays the same
twin produced. A strategy can therefore win back its full size while the
venue's own residuals still say the model prices some other market.

## Decision

1. **The receiver is the centre's divergence hold, and only its release.**
   `Platform::admit_microstructure_delta` re-admits each delta through
   `MicrostructureDelta::new`, because the fields come from another
   process's journal and a delta that does not validate is not evidence. The
   platform keeps the latest delta per venue, ordered by the end of its
   window. An older campaign read late never displaces a newer one, and the
   set is bounded at `MAX_DELTA_VENUES`.
2. **A residual may keep a hold.** A re-promotion releases a venue's hold
   only when that venue's latest delta passes `residual_keeps_hold`. The
   check fails when any slippage or impact residual has `|observed -
   modelled|` past the threshold on at least the minimum sample. Those two
   families are the ones stated in basis points, the twin's own unit. The
   kept hold is named on the LEARN summary, with the delta, the residual and
   its window.
3. **A residual never lifts a hold, never raises one, and never sizes
   anything.** A delta inside the threshold leaves the release to the
   re-promotion exactly as before. A delta for a venue with no hold does
   nothing. No branch uses a residual to make a grant larger.

## Consequences

- `qip-kernel/tests/central.rs::a_concluded_tick_campaigns_delta_keeps_the_venues_divergence_hold`
  re-promotes a held strategy. While a delta's slippage residual is 150 bps
  off on a full sample, the hold stands. A later delta inside the threshold
  lets the same re-promotion release it. Dropping the lookup in
  `review_replay_divergence` fails the test.
- **What is not built, and why.** The ruling names qip-api's central plane
  as the reader of the deep brain's ambient journal. Two facts stop that
  wire from being written honestly today. First, `EventLog::open` takes an
  exclusive advisory lock, so qip-api cannot open the journal while the deep
  brain holds it. Second, the two run as separate Cloud Run services with
  no shared file. No transport carries a delta from the producer's process
  to the process whose Platform holds cell-fed grants. Until one does,
  `admit_microstructure_delta` has no production caller, and
  R12-CONTRACT-7074 stays PARTIAL on that clause. The transport is the
  remaining work: a read-only frame reader over a shipped journal, or the
  delta riding an existing mesh or policy payload. Choosing between them
  needs its own record, because either one moves a contract across a
  process boundary.

## What it costs

- A strategy can stay held to half its grant after a re-promotion that its
  gate signed. That is paper capital the desk could have deployed, kept
  idle on one campaign's residuals. The release still follows the
  re-promotion already on the ledger: the first LEARN after an in-threshold
  delta releases it, with no second approval.
- A cell whose residuals are themselves miscalibrated keeps holds the twin
  no longer deserves. The rule trusts the residual's own sample count and
  nothing else about it.
- The latest delta per venue is held only in memory. A restart forgets it,
  so in the window before the producer's next delta arrives, a re-promotion
  can release a hold that the forgotten delta would have kept. That is the
  unsafe direction, and it is stated rather than hidden. The hold itself is
  resumed from the log, so a restart never releases a hold that has not
  been re-promoted. The transport that gives this seam a caller must replay
  the latest delta at start-up, or journal admitted deltas, before it is
  wired.

## What would make this wrong

A residual family stated in basis points that is not a fill-cost measure.
In that case the rule would keep holds on evidence unrelated to the twin's
pricing, and the family list in `residual_keeps_hold` is the line to narrow.
