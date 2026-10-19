# ADR 0151: What enters UNDERSTAND — each tick-plane and federation input has a named producer and consumer, or is not built, and an outside prior may only narrow REASON

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval
  ("I allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). It decides which inputs the cycle's understanding stage takes
  and which it declines. It admits no crate, creates no billed resource and
  touches no protected path; the one number-shaped consequence is listed under
  RISK.
- **Date**: 2026-10-19
- **Extends**: ADR 0125 (an advisory stage may veto or narrow, never enlarge),
  ADR 0132 (one admission header; a tick reaches the fabric only inside a
  bounded feature digest), ADR 0133 (what crosses between centre and reflex
  lane).
- **Keeps**: ADR 0001/0003, the hash-chained `OutcomeRecord`, the cost router's
  `Determinism::Required` arm.

## Context

Twenty-two rows ask the cycle to consume something: a `MarketStatePack`, a
`WorldModelState`, a `SharedEpisode`, a fused tick posterior, typed world
drivers, signed order flow, an event-market resolution, a belief price level.
They have one thing in common that the registers keep rediscovering: the
*producer* exists or is easy, and the *receiver* is "a decision somebody has not
made". A receiver invented to satisfy a row is a decode-and-drop that proves
nothing, and a producer with no receiver is a record that reads as a
capability. The platform has been bitten by exactly this: `MaxExpectedShortfall`
shipped in every limit set and could never fire.

Facts the decisions rest on (all by symbol; recount, do not trust):
`MarketStatePack` is built nowhere outside `qip-contracts`; the NOW stage holds
instrument readings, not `MarketEvent`s; `Platform::stage_simulate` grows no
`FutureTree` and `FutureTree` has no caller outside `qip-twin` tests and
`qip-cli`'s agency; `LatticeStage::set_signals` has no caller; the bar tape is
OHLCV with no aggressor side; `KnowledgeDeltaRecord` is the one record that
crosses a brain boundary; `OutcomeRecord`'s `JournalEntry` is a hash-chained
versioned body; the central `Tick` is built in about ninety-six places with no
clock discipline behind it.

Locate them with
`grep -rn 'MarketStatePack' backend/crates --include=*.rs -l`,
`grep -rn 'FutureTree' backend/crates --include=*.rs -l`,
`grep -rn 'set_signals' backend/crates --include=*.rs`,
`grep -n 'pub struct OutcomeRecord' backend/crates/libs/qip-contracts/src/reflex.rs` and
`grep -n 'fn reason_about_the_queue' backend/crates/runtime/qip-kernel/src/platform.rs`.

## Decision

1. **Consumer first.** An advisory record is built only when its producer and
   its consumer are both named in the change that adds it. A record whose
   consumer is replay (reproducibility) says so; a record with neither is not
   built. The rows below are each decided by this rule.
2. **`MarketStatePack` is an integrity annotation, built over events that were
   drained, and consumed only as a downgrade.** The producer is the edge
   capture drain: per interval it builds a pack over the `MarketEvent`s it
   actually applied, and never over zero events (an empty interval records "no
   tape", not a clean one). Interval and horizon are the pass interval. The
   pack rides inside the pass's `FeatureSnapshot` P2 record (ADR 0132). The
   consumer is the tick desk's fusion (`fusion.rs`): an instrument whose pack
   carries an integrity flag has its input weight removed from the fused
   state. It is not a regime input and never raises a weight. An acceptance
   test pins that the cognitive layer has no raw-tick input type.
3. **A fused posterior never raises REASON's confidence and may only narrow
   it.** `reason_about_the_queue` gains an optional outside-prior seam
   (`OutsidePrior`) that can multiply the hypothesis's confidence by
   `m = 1 − max(0, 2p − 1)`, where `p` is the posterior's own probability
   against the hypothesis's direction. `m` is in `[0,1]`, derived from the
   posterior and containing no constant, compounds with ADR 0125's caps, and
   with `p ≤ 0.5` is exactly 1. The tick desk stays off by default, so this
   is inert until it is on; the test removes either input and asserts the
   decision changes.
4. **`WorldModelState` has replay as its consumer and no brain receiver.** The
   record exists so a pass reproduces. A cross-brain receiver is not built
   until a brain names a decision it feeds; the register wording says
   "emitter and replay, no receiver, by decision". `SharedEpisode` is not
   built: `KnowledgeDeltaRecord` is the cross-brain record, and the deep
   brain reads it from `world_model.updated`.
5. **`FutureTree` is not grown in SIMULATE.** The compute bound of the
   decision-path stage is not spent on trees; `to_world_branch` is called
   where trees are grown (the tick desk and `qip-cli`), and a `WorldBranch` is
   journaled there. `WorldDriver` gets a typed `DriverKind`
   (`Policy`, `Weather`, `Default`, `Supply`, `OwnAction`, `Price`) so a
   declaration is typed, but only `OwnAction` and `Price` have a producer; the
   others need data this repository does not hold (ADR 0157).
6. **Signed order flow has no source, so no flow member is built.** A
   microstructure member fed from bars would be the regional member again.
   `LatticeStage::set_signals` stays uncalled (calling it with an empty map
   changes nothing). The source is a licensed tape with aggressor side
   (BILLED, below).
7. **The sealed outcome record gains no field.** `OutcomeRecord`'s
   `JournalEntry` is hash-chained and versioned; adding `queue_estimate` or
   `hedge_leg` needs a chain version, a codec record and a v2 verifier policy
   for entries that lack it, for fields with no producer on the desk path.
   If a producer appears, the field rides a separate non-chained `DeskDiagnostic`
   record, not the sealed one.
8. **Raw wire frames are captured by a separate writer, only where a licence
   permits raw retention.** The applied record stays the decoded event; the
   frame goes to a raw-capture writer on the feed side in the `Transient`
   retention class, per dataset, refused when the licence record (ADR 0157)
   does not permit raw retention. Not built until one does.
9. **The central `Tick` claims no normalised time.** It states `received_at`
   only, because the centre has no clock discipline to certify an observed
   instant or an uncertainty; the edge lake line already carries both.
10. **Federation members need a typed proposition, a source and a resolver.**
    `FundingDraw` and `EventResolution` propositions are not added: no
    per-instrument funding event exists to wait on (R12-WORLD-3048), and the
    event plane opens only with a licensed source, none in the catalogue
    (R12-WORLD-3047). The event plane's outcome comes from a resolver reading
    named observations from a named source; the synthetic venue's random
    outcome stays test-only (R12-EVENT-5083), and arrival likelihoods have no
    producer, so the plane keeps normalising book midpoints and labels them
    "venue-implied, not evidence-estimated" (R12-EVENT-5084).
11. **The central lattice plus `ReflexForecast` satisfy "milliseconds to
    years".** The microsecond row is `PromotionTarget::LocalReflexOnly` by
    design (ADR 0133); the central cycle issues every other horizon.
12. **Detector inputs that need an undeclared map or an unmeasured source are
    not built.** The kernel retains a bounded list of contested `ClaimKey`s
    (bounded by the evidence fabric's capacity); the missing-link detector
    waits for an operator-declared variable-to-subject map and is not built
    on a guessed one (R12-EXPAND-6011). Coverage dimensions stay asset class,
    venue and geography; language and causal-driver inventories need a
    measured source (R12-EXPAND-6065). The kernel exposes no belief "price
    level", because beliefs are probabilities; the persistence detector over
    price-versus-belief is not built (R12-EXPAND-6067).
13. **Self-model health scales evidence, in shape only.** Operational health
    (halt, stale feed) enters `SelfModel` as a multiplier in `(0,1]` on the
    evidence a sample contributes, 1 when healthy and never above 1, and
    capability estimates are keyed per asset class so equity evidence never
    counts toward crypto. The multiplier values are not decided here (RISK).
14. **Landed cost is not futures carry.** `qip_financial::physical` models
    commerce legs; a commodity curve's carry is read from the curve
    (`TermStructure`), not mapped from landed-cost fields (R12-COVERAGE-5101).

## Consequences

- The tick plane has one tested consumer (fusion) and one narrowing seam into
  REASON, both inert by default.
- Seven registers' receivers are recorded as absent *by decision*, so "not
  built" can be told from "not yet built".
- No sealed wire body, no topic and no contract changes except the additive
  `DriverKind`, `OutsidePrior` and `contested_keys`.

## What it costs

- Integrity annotations only downgrade, so a clean pack adds no confidence.
- Without a flow source the flow member and propagation outputs stay absent.
- No cross-brain world-state receiver means a second brain cannot yet read the
  centre's state as a contract.

## What would make this wrong

- If fusion's removal of a flagged instrument measurably worsens calibration,
  the downgrade is too blunt and a pack needs a graded weight; that is a
  follow-on record, not a regime input.
- If a brain names a decision `WorldModelState` would feed, decision 4's
  "no receiver" is superseded for that brain only.
- If a licensed tape with aggressor side is admitted (ADR 0157), decision 6 is
  satisfied by building the member, not by this record.

## Dissent

*Build the receivers now so the seams are proven end to end.* Strongest form:
an emitter with no receiver is untested at the one place it matters, and the
register will report the seam as open for ever. It lost because a receiver
without a decision behind it can only decode and drop, which proves the codec
and nothing else, and it is precisely the shape of control that cannot fire.
It would be revived by a named consumer decision for any one of them.

## Code it constrains

- `MarketStatePack` in `qip-contracts/src/market_state.rs`; the edge drain in
  `qip-edge-node/src/drain.rs`; `fusion.rs` in the tick desk.
- `Platform::reason_about_the_queue` and `OutsidePrior` (new) in `qip-kernel`.
- `Platform::stage_simulate` (unchanged: grows no tree); `Future::to_world_branch`
  and `WorldDriver`/`DriverKind` in `qip-twin/src/futures.rs`.
- `OutcomeRecord`/`JournalEntry` in `qip-contracts/src/reflex.rs` (unchanged).
- `Tick` in `qip-market/src/quote.rs` (unchanged).
- `EvidenceIntake` in `qip-kernel/src/evidence_intake.rs`
  (`contested_keys`); `SelfModel` in `qip-learning-engine/src/self_model.rs`.

## Rows unblocked

- R12-TICK-2041, R12-CONTRACT-7072 — decision 2.
- R12-TICK-1002 — decision 3.
- R12-CONTRACT-7075, R12-EXPAND-6039 — decision 4: restated.
- R12-CONTRACT-6110, R12-TICK-3097 — decision 5.
- R12-WORLD-3085, R12-WORLD-3046 — decision 6: restated; BILLED source.
- R12-TICK-2047 — decision 7: restated.
- R12-TICK-2050 — decision 8: restated; gated on a licence record.
- R12-TICK-2051 — decision 9.
- R12-WORLD-3047, R12-WORLD-3048, R12-EVENT-5083, R12-EVENT-5084 —
  decision 10: restated.
- R12-AMBIENT-3066 — decision 11: restated.
- R12-EXPAND-6011, R12-EXPAND-6065, R12-EXPAND-6067 — decision 12.
- R12-WORLD-3036 — decision 13; values RISK.
- R12-COVERAGE-5101 — decision 14: restated.

## Not decided here

- **BILLED** — a licensed trade tape with aggressor side
  (R12-WORLD-3085, R12-WORLD-3046); a licensed event source for the event
  plane (R12-WORLD-3047); a licence permitting raw frame retention
  (R12-TICK-2050).
- **RISK** — the health-multiplier values of decision 13 (R12-WORLD-3036),
  because they change the sizing bound; any floor under the decision-3
  multiplier (R12-TICK-1002).
- Nothing is PROTECTED or paper-boundary.
