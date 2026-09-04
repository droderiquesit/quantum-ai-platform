# data/datasets/

Committed reference datasets. Nothing here is market data and nothing here
is a production store; see `../README.md` for what the data domain holds.

| File | What it is |
|---|---|
| `universe.json` | The instrument catalogue every central composition root reads from `QIP_UNIVERSE_PATH` and refuses to start without. Synthetic instruments mirroring the synthetic exchange's, so a deployment sizes into real exposure buckets. |
| `loop-demonstration-tape.json` | A **synthetic fixture** for demonstrating that the decision loop runs end to end on data with a detectable structure in it. Read by `qip-fastbrain` when `QIP_FASTBRAIN_TAPE_PATH` names it. Not market data: every price is generated from a fixed irrational rotation by `qip-market-ingestion`'s tape tests, so there is no source, no licence and no licensing question; its descriptor reports `LicensingClass::Synthetic`, which the object model bars from any production decision. |

## The demonstration tape

Four of the catalogue's instruments (`NWSC`, `VNTG`, `MRDN`, `ATFB`, all
`XNYS`) over 320 hourly periods from 2025-01-06T21:00Z. Every observation
carries two instants: `at`, when the bar closed, and `known_at`, when it
became knowable (fifteen minutes later). The loader refuses a tape in which
any `known_at` precedes its `at` — that is look-ahead — and a tape whose
`known_at` instants run backwards, rather than sorting it.

Hourly rather than daily because the platform stamps every agent manifest
reviewed at assembly and refuses it ninety days later at tape time; a
320-day daily tape convened its first panel on day 103 with every agent
refused. `qip-fastbrain` now refuses at start-up a tape whose span reaches
the roster's shortest review interval. Three hundred and twenty hours is
thirteen days.

Two structures are planted:

- `NWSC` jumps +8.5% on period 100: one outlier in an ordinary series, aimed
  at the return-anomaly detector. Its claim carries a five-day horizon, which
  is 120 hourly periods, so it resolves on tape at period 220 and the LEARN
  stage scores it.
- `MRDN` drifts +0.6% a period over periods 180–239 against noise of ±0.9%:
  a persistent shift no single period of which is an outlier, aimed at the
  CUSUM structural-break detector. Its ninety-day horizon does not resolve
  on a thirteen-day tape.

`VNTG` and `ATFB` are noise only, so a detector that fires on them is firing
on nothing.

The file is the output of `demonstration_document()` in
`backend/crates/services/qip-market-ingestion/src/tape.rs`, and
`the_committed_demonstration_tape_is_the_generator_output_and_loads` fails
naming the expected file when the two drift. Regenerate with

```
cd backend && cargo test -p qip-market-ingestion demonstration_tape
```

and copy `backend/target/loop-demonstration-tape.expected.json` over this
file. Do not edit it by hand.
