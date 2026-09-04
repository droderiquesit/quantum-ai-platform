# data/datasets/

Committed reference datasets. Nothing here is market data and nothing here
is a production store; see `../README.md` for what the data domain holds.

| File | What it is |
|---|---|
| `universe.json` | The instrument catalogue every central composition root reads from `QIP_UNIVERSE_PATH` and refuses to start without. Synthetic instruments mirroring the synthetic exchange's, so a deployment sizes into real exposure buckets. |
| `loop-demonstration-tape.json` | A **synthetic fixture** for demonstrating that the decision loop runs end to end on data with a detectable structure in it. Read by `qip-fastbrain` when `QIP_FASTBRAIN_TAPE_PATH` names it and by `qip-api` when `QIP_API_TAPE_PATH` does, through the same `TapeFeed`, so the two roots cannot read one file two ways. Not market data: every price is generated from a fixed irrational rotation by `qip-market-ingestion`'s tape tests, so there is no source, no licence and no licensing question; its descriptor reports `LicensingClass::Synthetic`, which the object model bars from any production decision. |

## The demonstration tape

Four of the catalogue's instruments (`NWSC`, `VNTG`, `MRDN`, `ATFB`, all
`XNYS`) over 600 hourly periods from 2025-01-06T21:00Z, in four sections:
bars, macro releases, alternative-data readings and dividend declarations.
Every record carries two instants: `at`, when the fact was true — the bar
closed, the reference period ended, the reading was observed, the dividend
was declared — and `known_at`, when it became knowable. The loader refuses a
tape in which any `known_at` precedes its `at` — that is look-ahead — a tape
whose `known_at` instants run backwards, rather than sorting it, and a
release, reading or declaration knowable before the first bar: the bars own
the clock, and history already published when the tape starts is stamped
knowable at that first instant.

Hourly rather than daily because the platform stamps every agent manifest
reviewed at assembly and refuses it ninety days later at tape time; a
320-day daily tape convened its first panel on day 103 with every agent
refused. `qip-fastbrain` now refuses at start-up a tape whose span reaches
the roster's shortest review interval. Six hundred hours is twenty-five
days; the tape was 320 hours while the NWSC jump was a bare price move
with a five-day claim, and grew when the jump gained a catalyst and with it
the catalyst detector's twenty-day horizon.

Two structures are planted in the bars:

- `NWSC` jumps +1.5% on period 100 (about +2.4% with that period's own
  noise): one outlier in an ordinary series, aimed at the return-anomaly
  detector and kept under the volatility-shift detector's bar. Thirty hours
  earlier, at period 70, `NWSC` declares a cash dividend: the one record
  kind whose event lands on the instrument's own id, so the catalyst
  detector reads the jump as an explained move rather than an unexplained
  one — and the platform forms no hypothesis about an unexplained move by
  design. The catalyst carries a twenty-day horizon, 480 hourly periods, so
  the claim resolves on tape at period 580 and the LEARN stage scores it.
- `MRDN` drifts +0.6% a period over periods 180–239 against noise of ±0.9%:
  a persistent shift no single period of which is an outlier, aimed at the
  CUSUM structural-break detector. Its ninety-day horizon does not resolve
  on a twenty-five-day tape.

`VNTG` and `ATFB` are noise only, so a detector that fires on them is firing
on nothing.

Two more are planted in the other sections, both leaning the way the jump's
claim leans (a positive jump is claimed overvalued, so a hawkish print and a
collapsing proxy both support it):

- Four US macro series — `US.POLICY_RATE`, `US.INFLATION_YOY`,
  `US.GROWTH_YOY`, `US.CREDIT_SPREAD_BPS`, the codes
  `qip_world_model::vocabulary::MacroSeries` recognises — carry thirty-six
  monthly prints of history knowable at the tape's first instant and a
  December print at period 88 that is hawkish on every series by about 2.5
  sigma of its own history. The macro analyst reads them keyed by `NWSC`'s
  economy (`US`) and needs thirty observations before its standardisation
  means anything, which is what the history is for.
- Daily `web_traffic_index` readings for `NWSC` from the `web-traffic`
  dataset: forty-five days of history at the first instant, then one per
  tape day published at 06:15, the 10 January reading collapsed to 70
  against a level near 100. The alternative-data analyst finds the series
  and refuses it: nothing in this repository licenses the dataset and the
  platform's default licenses none, and a tape cannot grant a licence.

The file is the output of `demonstration_document()` in
`backend/crates/services/qip-market-ingestion/src/tape.rs`, and
`the_committed_demonstration_tape_is_the_generator_output_and_loads` fails
naming the expected file when the two drift. Regenerate with

```
cd backend && cargo test -p qip-market-ingestion demonstration_tape
```

and copy `backend/target/loop-demonstration-tape.expected.json` over this
file. Do not edit it by hand.
