# ADR 0434: An opportunity sentinel raises a run of sub-threshold scores rising toward a detector's threshold, and never a finished anomaly

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation**,
  session t50-04 (wave580). An in-repository design decision: it adds a
  research signal, changes no limit, no autonomy, no paper-trading layer and
  no capital figure.
- **Date**: 2026-11-12
- **Rows**: R12-AMBIENT-3069 (stays PARTIAL; see "What is not met").

## Context

R12-AMBIENT-3069 reads: "The ambient mesh includes opportunity sentinels that
look for precursors to cross-asset, cross-region, event, physical-product and
multi-leg dislocations before a full trade signal." Its check: "The sentinel
runs in production and its output reaches the attention router; disabling it
removes its signals."

Before this ADR the deep brain's sweep ran `DetectorRegistry::detect_all`
and routed what it found. Every detector there reports a dislocation that
has already happened — a correlation that has broken, a reading already far
from consensus. Nothing in the tree looked for one forming. Triage noted
that what counts as a precursor is a product definition the code cannot
choose by itself; this ADR chooses one.

## Decision

1. **A precursor is a rise, scored on the detector's own statistic.** Each
   family scores the number the matching detector fires on, at that
   detector's threshold. A precursor is `run` (default 3) consecutive sweeps
   of one family and subject, each strictly higher than the last, every one
   below the threshold, the newest at least `floor_share` (default one half)
   of it. "Below threshold" therefore means "the detector has not fired",
   not a second opinion that can disagree with it. The cross-asset family
   shares `detector::correlation_departure` with
   `CorrelationBreakdownDetector`, extracted for this purpose, so the two
   cannot drift apart.
2. **Never a finished signal.** A score at or past its threshold, or a
   subject any detector flagged in the same sweep, raises nothing and its run
   restarts. The anomaly path owns it; raising both would be two claims
   about one fact. A run that raised is cleared, so the next precursor needs
   a new rise rather than repeating every sweep.
3. **The five families.**
   - *cross-asset*: every priced pair, the Fisher-transformed correlation
     departure, at the breakdown detector's 2.5;
   - *cross-region*: configured pairs of listings of one product, the
     latest log price gap's robust z (median and MAD, the observation
     detector's statistic) at 2.5; a regional pair is never also scored as
     cross-asset;
   - *event*: a subject an event became knowable on within `event_window`
     (5 days), the latest log return's robust z;
   - *physical-product*: configured observation names, the latest reading's
     robust z, the observation detector's statistic at its 2.5;
   - *multi-leg*: configured leg sets, the latest `Σ weight × ln price`
     spread's robust z.
4. **Silent until configured is stated, not hidden.** Cross-region,
   physical-product and multi-leg need reference data the sweep's context
   does not carry: which listings are one product, which readings are
   physical, which legs form a structure. `PrecursorConfig` holds them and
   is empty by default; `ResearchLoop::with_precursors` sets them and
   refuses a configuration that could not tell a rise from a reading
   (`run < 2`, a floor outside `(0, 1)`, a non-positive threshold, a leg set
   with fewer than two legs or a zero weight). Event is silent until the
   sweep's context carries events, which today it does not. These are not
   controls that cannot fire: each family's unit test fires it. They are
   sentinels with nothing configured to watch.
5. **One research-task signal per precursor.** `precursor_signal` raises a
   `SignalClass::Opportunity` `AmbientSignal`, `Pathway::ResearchTask`,
   subject `precursor:<family>:<subject>`, severity the newest score's share
   of the threshold, evidence the family and the run of scores. It goes
   through the same `AttentionRouter::route` and budget as every other
   signal and is recorded on `ResearchRound::precursors`. It is advisory:
   it wakes research and decides nothing.
6. **A switch, refused if unclear.** `ResearchConfig::opportunity_sentinel`,
   on by default, read from `QIP_DEEPBRAIN_OPPORTUNITY_SENTINEL` as `on` or
   `off`; anything else stops start-up, because a typo that silently
   disabled the sentinel would be noticed only after the dislocation it
   should have seen. Off, the sweep scores nothing and raises no precursor.
7. **Bounded memory.** The scanner keeps at most `run` scores per family and
   subject present in the current sweep; a subject absent from a sweep is
   forgotten.

## Evidence

- `qip-opportunity-engine/src/precursor.rs` unit tests: a rising run raises
  one precursor and a flat one none; a run reaching the threshold or flagged
  raises none; a run below the floor raises none; a bad configuration is
  refused; each configured family scores the subject it names and a regional
  pair is never cross-asset.
- `qip-deepbrain/tests/research.rs`:
  `a_widening_regional_gap_routes_one_precursor_before_any_anomaly_and_none_when_the_sentinel_is_off`
  drives `ResearchLoop::sweep` over a platform fed bars of two listings
  whose gap widens across three sweeps (scores 1.48, 1.89, 2.29 against
  2.5) and asserts one cross-region precursor reaches an attention event on
  the third, and that the same tape with the switch off raises none;
  `a_steady_regional_gap_raises_no_precursor_and_one_already_past_its_threshold_is_not_one`;
  `the_opportunity_sentinel_switch_reads_on_and_off_and_refuses_anything_else`.
  Each was mutation-verified; the commit message records the mutations.

## What is not met

- **Production.** The check says the sentinel runs in production. It runs
  in every sweep of the `qip-deepbrain` binary's research loop, built in
  `main.rs` from `ResearchConfig::from_lookup`, but no deep brain is shown
  deployed, so that half is not met by anything this repository can do.
- **Families beyond cross-asset in a deployment.** No deployment names
  regional pairs, physical readings or leg sets, and `main.rs` does not yet
  read any; the sweep's context carries no events. Until those are
  supplied, a deployed sentinel watches cross-asset pairs only.
- Only the cross-region family is driven end to end through the sweep in a
  test; the other four are proven at the scoring and scanner seam.

## What it costs

- A pair-wise cross-asset score every sweep: the same O(n²) walk over priced
  pairs the breakdown detector already makes, run a second time.
- Memory of at most three scores per family and subject present in the
  sweep.
- More research-task signals through the attention router's budget. A
  precursor that never becomes a dislocation is research spent on noise;
  the floor and the strict three-sweep rise are what bound that, and
  neither has been calibrated against a real tape.
- Three of five families do nothing until a deployment configures them,
  so the requirement's breadth is promised by the code and delivered by
  configuration nobody has written yet.

## What would make this wrong

A precursor that the anomaly detector would already have flagged in the
same sweep, or one scored on a statistic the detector does not use. Either
would mean the sentinel and the detector disagree about one fact, which is
the failure decision 1 exists to prevent.
