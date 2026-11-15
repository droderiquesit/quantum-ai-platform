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
