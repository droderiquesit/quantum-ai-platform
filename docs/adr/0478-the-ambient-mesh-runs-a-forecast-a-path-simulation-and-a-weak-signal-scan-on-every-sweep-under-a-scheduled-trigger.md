# ADR 0478: The ambient mesh runs a forecast, a path simulation and a weak-signal scan on every sweep under a scheduled trigger

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation**,
  session t50-26 (wave580). An in-repository design decision: it adds three
  signal producers to the deep brain's research sweep. It adds no dependency,
  widens no authority (every output is an `AmbientSignal`, which the attention
  router turns into advisory-only events), touches no order path and no
  paper-trading layer, and adds no limit.
- **Date**: 2026-11-12
- **Rows**: R12-AMBIENT-1180 (graded COMPLETE against its inspection bar; the
  items under "What this leaves open" are outside that bar).

## Context

R12-AMBIENT-1180 asks for an ambient mesh whose always-on models "predict,
detect novelty, monitor assumptions, simulate paths, scan weak signals,
identify precursors and route attention without prompts". Before this
decision every producer the research sweep routed was event-triggered:
`now-surprise` (novelty), `forecast_miss` (an assumption the lattice held,
broken), `precursor_*` (precursors), `limit_approach`, the gap triggers. Each
waits for something else to have happened. `Trigger::Schedule` existed in
`qip-contracts/src/ambient.rs` and nothing in production raised one, so no
attention event could say "a model running on its own schedule woke this".
Nothing predicted forward, nothing walked a path forward, and nothing read
the many sub-threshold readings a book produces together.

## Decision

`qip-deepbrain/src/ambient_models.rs` holds three models. `ResearchLoop::
sweep_over` runs them on every sweep (the sweep runs on the node's cycle
cadence, `ResearchLoop::maybe_sweep`, from `node::run_with`, which
`main.rs` runs), over the same `SWEEP_PRICES` windows the anomaly path reads,
and routes their signals through the same `AttentionRouter` as every other
producer. Each signal carries `Trigger::Schedule(<model>)`, so the router's
`AttentionEvent::originating_models` names the model.

1. **`ambient-forecast`** — an AR(1) on log returns, fitted on the window
   *before* the newest return and scored against it. A miss of
   `forecast_miss_sigmas` (default 4) residual deviations or more is a
   `Surprise` routed to specialist activation. The next step's expected
   return and band are reported in `ResearchRound::ambient_models`. A
   walk-forward holdout rather than a remembered forecast: the prediction is
   reproducible from the price record alone, which a forecast held in
   process memory across sweeps would not be after a restart.
2. **`ambient-path-simulation`** — a bootstrap of the subject's own window
   returns, `path_count` (256) paths of `path_steps` (10) steps, seeded from
   `seed` mixed with the subject's name (FNV-1a), with the in-tree
   `Xoshiro256`. The share of paths whose drawdown from today reaches
   `path_drawdown` (15%) is computed in integer basis points; at
   `path_alarm_bp` (2,500) or more it is a `Risk` routed to risk review,
   with that share as severity. A bootstrap rather than a Gaussian because
   it assumes no distribution the record does not show.
3. **`ambient-weak-signal`** — the newest return of every measurable
   subject in deviations of its own window. Readings in
   `[weak_low_sigmas, weak_high_sigmas)` (1.5, 3.0) — below where the
   anomaly path takes over — leaning the same way, by at least
   `weak_min_subjects` (4) subjects and at least `weak_breadth_bp` (7,500)
   of the measurable book, are one `Opportunity` routed to research.

**Refusal, not clamping.** A non-positive or non-finite price, a history
shorter than the window, or a window with no dispersion is reported in
`AmbientModelReport::unmeasured` by model, subject and reason and raises
nothing. A book with fewer measurable subjects than a breadth is reported
the same way. `AmbientModelConfig::validate` refuses an unusable threshold
(window below 8, a non-finite or non-positive sigma, an empty weak band, no
paths or steps, a drawdown outside (0, 1), a zero alarm, a breadth that is
not a majority), and `ResearchLoop::with_ambient_models` refuses what it
refuses.

**On by default**, like the sentinels; `QIP_DEEPBRAIN_AMBIENT_MODELS=off`
turns them off, anything other than `on`/`off` stops start-up, and with
them off `ResearchRound::ambient_models` is `None`, not an empty report.

The thresholds are conservative engineering defaults; the existing
deep-brain suites pass with the models on. They are not figures the owner
stated, and no calibration against outcomes stands behind them.

## What it costs

- Compute per sweep: one AR(1) fit and one window statistic per subject,
  and 256 × 10 bootstrap draws per subject for the path simulation, over at
  most `SWEEP_PRICES` prices each. Linear in the subjects held; no I/O.
- Attention: the three models spend from the same hourly budget as every
  other producer, so on a busy book they compete with the event-triggered
  signals for activations. A storm is deferred or shed on the record.
- Thresholds nobody has calibrated: until outcomes score them, every
  default is a judgement, and a wrong one either wakes specialists for noise
  or stays silent on a drift it was meant to see.

## Consequences

- An operator reading an attention event can tell a scheduled model's wake
  from an event-triggered one by the trigger arm and the originating model.
- The attention budget now also spends on these signals. They route through
  the same budget, deferral and clustering as everything else, so a storm of
  them is deferred or shed on the record rather than waking every specialist.
- Three more f64 statistics cross into the integer severity scale; each
  crossing is commented where it happens.

## What this leaves open

- The forecast is not yet emitted as a §9.14 `Forecast`
  (`qip-contracts/src/forecast.rs`) record, and its misses do not feed the
  forecast lattice's calibration; it is reported in the round only.
- No model's thresholds are calibrated against realised outcomes; the
  attention events carry `feedback_link`, and nothing scores them yet.
- "Monitor assumptions" is covered by the existing `forecast_miss` sentinel,
  which reads the lattice's misses on every sweep but raises under an event
  trigger, not by a scheduled model of its own.
- Nothing is deployed, so "always-on" holds for the binary's loop and in
  test, not yet in a running process.

## What would make this wrong

A measurement showing these models raise mostly signals the router sheds or
specialists dismiss would say the thresholds are noise generators, and the
defaults should move or the models be switched off pending calibration.
