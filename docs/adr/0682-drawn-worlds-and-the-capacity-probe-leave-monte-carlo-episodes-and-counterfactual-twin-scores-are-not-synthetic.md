# ADR 0682: Drawn worlds and the capacity probe leave Monte Carlo episodes, and counterfactual twin scores are not synthetic

- **Status**: **Proposed 2026-11-15**, lane 41 of wave b52. Written without a
  build (owner instruction of 2026-11-15); the pipeline builds and tests it.
- **Date**: 2026-11-15
- **Rows**: R12-EXPAND-6033 (stays PARTIAL until the pipeline has run the
  tests), R12-OBS-9140 (decisions 4-6).
- **Amends**: nothing. Extends ADR 0149 decision 18.

## Context

ADR 0149 decision 18 gave `SyntheticEpisode` a stochastic arm,
`SyntheticGenerator::MonteCarlo`, that must carry a seed. SIMULATE journaled
episodes for the library and for causal drivers (both deterministic) and
nothing else, so the stochastic arm had no producer. Three things SIMULATE
generates are drawn from a seed: the two tail worlds `synthetic-1` and
`synthetic-2`, the generated joint world `synthetic-joint`, and the capacity
probe's `MarketSimulator::synthetic(market, seed).with_agents(..)` run. The
in-lane skeptic refused COMPLETE on 2026-10-25 for that omission, and for
the Conditional arm having no test that fails when its push is deleted.

## Decision

1. **The three drawn tail worlds are `MonteCarlo` episodes seeded with the
   cycle number**, the one value `tail.rs` XORs with its stream constants.
   The deterministic tail worlds (counterparty, model failure, the factor
   worlds, the contagion worlds) draw nothing and are not episodes of this
   kind. Which worlds are drawn is `synthetic_worlds::is_drawn_world`, built
   from `SYNTHETIC_WORLDS` and `SYNTHETIC_JOINT` so a third drawn world added
   to those constants is covered without a second list.
2. **The capacity probe is one `MonteCarlo` episode per cycle**, named
   `capacity-probe:<subject>`, seeded with `PlatformConfig::seed`, digesting
   the subject, opening price, size, book shape, volatility, drift, feed delay
   and latency condition as their bits or text.
3. **LEARN's counterfactual twin scores are not synthetic episodes.** The
   twin replays recorded bars; the contract says a synthetic episode is
   "never by a backtester, which replays recorded bars and would label a
   historical replay synthetic". Labelling the twin's scores synthetic would
   be that mislabel. They stay as they are (`CounterfactualScore` records);
   the requirement's "counterfactual worlds" is met by the generated worlds
   above and by the Conditional episodes. This is a decision, not a gap, and
   a reviewer who disagrees should say what the twin generates that is not in
   the recorded bars.
4. (R12-OBS-9140) `qip-ledgerd` adopts `qip-telemetry-drain` as ADR 0156 5.5
   directs.
5. (R12-OBS-9140) The reflex node (`qip-edge-node`) links no exporter. A
   drain in its closure puts a thread that performs network I/O beside the
   decision loop; `qip-telemetry-drain` is a Lane 1 crate and ADR 0116 says a
   second hosted Lane 1 crate reopens the sidecar rather than widening the
   hosted set. The node's OTLP export waits on that record.
6. (R12-OBS-9140) Every binary that has no exporter is named on a committed
   list, `qip_telemetry_drain::EXPORTERLESS_BINARIES`, with the reason, and an
   acceptance-style test in the drain crate holds the list to the binaries
   that really do not call the drain. The row stays open while the list is
   non-empty.

## What it costs

A cycle's `scenario.evaluated` journal grows by one `MonteCarlo` episode for each drawn tail world SIMULATE produced and one for the capacity probe when it ran, and each adds a SHA-256; the added storage and compute are unmeasured. The two digests cover different things: a drawn world's digest is over its name, the seed, the loss in both forms and the unmeasured positions, so it records what the stage computed, while the probe's digest is over its inputs only and the seed is carried beside it, so a replay of the probe must regenerate the run and compare its result separately. A drawn world is seeded with the cycle number and the probe with `PlatformConfig::seed`, so a replay needs both. Counterfactual twin scores get no synthetic label, so a reader who wanted every counterfactual under one episode type will not find them there. Decision 5 leaves the reflex node with no OTLP export, and decision 6 keeps R12-OBS-9140 open while `EXPORTERLESS_BINARIES` is non-empty. Nothing in this ADR was compiled or run, so the cost of the tests is also unmeasured.

## What would make this wrong

A third drawn world is added in `tail.rs` without joining `SYNTHETIC_WORLDS` or `SYNTHETIC_JOINT`, so `is_drawn_world` skips it and its seed is never journaled. A replay of a cycle from its journaled seed regenerates a drawn world whose digest differs from the one recorded, or a probe run that differs under the same digest and seed, which would show that something outside the recorded seed and inputs shaped the run. A reviewer names something the counterfactual twin generates that is not in the recorded bars, which would make the twin scores synthetic after all. Journal growth from these episodes is measured and found to be a retention or latency problem. A second Lane 1 crate has to be hosted by the node, which under ADR 0116 reopens the sidecar and with it decision 5.

## Evidence

Nothing in this ADR was compiled or run. The tests it relies on are named in
`docs/blueprint/lanes/b52-41.json`, each marked "mutation: not run".

## Row R12-OBS-9140: what changed

- `qip-ledgerd` depends on `qip-telemetry-drain`; `src/openobserve.rs` is the
  same thin module the other four roots hold, and `main.rs` spawns the drain
  after `start` returns, handing it the ledger's own `Metrics`. Unset
  `QIP_OPENOBSERVE_URL` exports nothing, as before.
- The earlier draft's unit test set the URL alone and failed: the drain
  refuses a URL with no organisation. The test now supplies both and a third
  pins the refusal.
- `qip-acceptance/tests/otlp_coverage.rs` enumerates `backend/crates/apps/*`
  and fails a binary that neither links the drain nor is named, with a reason,
  on `EXPORTERLESS`. `qip-edge-node`, `qip-web` and `qip-cli` are named; the
  row stays PARTIAL while the list is non-empty.
- No manifest sets a collector URL (ADR 0156 5.4); nothing has been shown to
  ingest.
