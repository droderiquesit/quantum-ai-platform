# data/datasets/

Committed reference datasets. Nothing here is market data and nothing here
is a production store; see `../README.md` for what the data domain holds.

| File | What it is |
|---|---|
| `universe.json` | The instrument catalogue every central composition root reads from `QIP_UNIVERSE_PATH` and refuses to start without. Synthetic instruments mirroring the synthetic exchange's, so a deployment sizes into real exposure buckets. |
| `loop-demonstration-tape.json` | A **synthetic fixture** for demonstrating that the decision loop runs end to end on data with a detectable structure in it. Read by `qip-fastbrain` when `QIP_FASTBRAIN_TAPE_PATH` names it and by `qip-api` when `QIP_API_TAPE_PATH` does, through the same `TapeFeed`, so the two roots cannot read one file two ways. Not market data: every price is generated from a fixed irrational rotation by `qip-market-ingestion`'s tape tests, so there is no source, no licence and no licensing question; its descriptor reports `LicensingClass::Synthetic`, which the object model bars from any production decision. |

## The catalogue's liquidity block, and where its figures come from

Every record in `universe.json` states a `liquidity` block, and
`qip_financial::catalogue` refuses a record without one by name. It is
required rather than optional because its absence was not visible: a record
with no block did not arrive without a liquidity profile, it arrived carrying
`LiquidityProfile::default()` — a 10 basis-point quote and a one-day exit —
and `MinLiquidity` and `MaxDaysToLiquidate`, two controls whose job is to veto
trading, were evaluated for every deployed instrument against a figure that
existed in no file, no reviewed diff and under no manifest hash.

The figures now stand exactly where `price` and `tick_size` stand: stated in
the file, read in the diff, covered by the SHA-256 every run journals. Two
different things back them, and the difference is not cosmetic.

- **`NWSC`, `VNTG`, `MRDN` and `ATFB` are backed by the committed tape.**
  `average_daily_volume` is the tape's own total volume over the distinct
  dates it trades on, truncated — 26,423,076 for each, which is what the
  fixed rotation generates. The first and last dates on the tape are partial
  sessions, so this understates the real daily volume, and understating
  capacity trades less rather than more. `typical_spread_bps` is a stated
  figure, but a bounded one: wider than the record's own tick over its own
  price (nothing is quotable tighter than the venue's grid) and narrower than
  the whole high-low range of the tightest bar the tape shows it trading in
  (a quote wider than a bar's entire range is not consistent with that bar
  having traded). `every_committed_record_states_its_own_liquidity_and_the_tape_backs_the_four_it_covers`
  in `backend/crates/libs/qip-financial/tests/costs.rs` holds both bounds.
- **`HLCX` and `RSRC` appear on no tape in this repository, and nothing backs
  them.** This is the point at which a reference file becomes an invented
  number under a different name, so the rule for them is directional rather
  than numerical: a record nothing measured is quoted no tighter, exits no
  faster and is participated in no harder than every record the tape does
  back, and states an average daily volume of zero — which makes
  `LiquidityProfile::days_to_exit` return nothing rather than something fast,
  and makes every volume-based capacity calculation size to zero. The worst a
  stated figure can therefore do is stop the platform trading.
  `a_committed_record_the_tape_does_not_cover_is_quoted_no_tighter_than_every_record_it_does`
  holds that, so the direction cannot be reversed by an edit here.

The exit times put `HLCX` and `RSRC` on `Rung::BondsAndLessLiquidListed` and
the four tape names on `Rung::ListedEquityAndFutures`, and the widest quote on
the upper rung (8bps) is tighter than the tightest on the lower (25bps), so
`prove_quotes_can_coexist` admits the catalogue at `Platform::new`. That is
not a coincidence to preserve by luck: quoting an unmeasured record
conservatively is what keeps the ladder monotone, and quoting one optimistically
is exactly the inversion that stopped the desk before `9f9c92a`.

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
