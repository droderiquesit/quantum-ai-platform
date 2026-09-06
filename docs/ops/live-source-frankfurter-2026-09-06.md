# The ECB reference rates through the loop, live — 2026-09-06

**What this record is.** A dated account of one session in which the shipped
`frankfurter-ecb-reference-rates` connector fetched **real** exchange rates
from `api.frankfurter.dev` and the platform absorbed them: the licensing gate,
the connector runtime, the loop's own adapter bridge, `Platform::observe`, one
`POST /cycle`, and the hash-chained event log. Everything below was run from a
shared build container on 2026-09-06 between 12:07 and 12:28 UTC and the output
is quoted rather than summarised.

**What this record is not.** This is not a deployment. It is not seven days of
streaming. `.claude/rules/domains/data-and-streaming.md` says it in terms — *a
connector proven in a session behind a local bridge is not proven in a
deployment* — and that sentence governs every claim here. The precise boundary
is in the last section, and it should be read before this document is cited
anywhere.

## The source, and its licensing posture

Evaluated **before** use, and not by this session: the entry already exists in
`qip-data-finder`'s catalogue (`backend/crates/services/qip-data-finder/src/admission.rs`,
`source_id: "frankfurter-ecb-reference-rates"`, licence
`ecb-reference-rates-via-frankfurter`), and `ApiFeed::connector_admitted_by_registered`
runs `admission::admit_from_registered` before any transport is constructed.
No gate was relaxed, edited, or bypassed to make this run happen. The decision
the process printed at start-up, verbatim from its own banner:

```
  feed:             connector frankfurter-ecb-reference-rates (Frankfurter (api.frankfurter.dev), which republishes the European Central Bank's daily euro foreign-exchange reference rates. Free, unauthenticated, no signup.), production-grade; licensing: admitted under licence `ecb-reference-rates-via-frankfurter` (class Public) for derive and trade at 2026-09-06T12:10:28.362Z; keyless; no registration needed
```

The source is keyless and needs no account, so the registration gate answered
`keyless; no registration needed` rather than being satisfied by a record
somebody added. No credential was created, mounted, or sent: the bridge
transcript records that every request carried `accept`, `connection`,
`content-length`, `host` and `user-agent` and nothing else.

## The bridge, and why one was needed

`qip_transport::http` speaks plaintext HTTP/1.1 and refuses `https` by name.
Frankfurter is HTTPS-only. The deployed answer is the Envoy egress proxy
(`infrastructure/egress/envoy.yaml`, the `frankfurter` listener) — **which has
never been applied anywhere**. To exercise the connector at all this session
therefore ran a local stand-in: a 100-line Python listener on `127.0.0.1:18081`
that accepts a plain HTTP/1.1 request, forwards it to
`https://api.frankfurter.dev` over TLS verified against the container's CA
bundle, follows no redirect, serves exactly one upstream host, and appends
every exchange to a transcript. It lived in the session scratchpad and is **not
committed**: it is a test fixture standing where an unapplied piece of
infrastructure will go, and committing it would invite somebody to mistake it
for one.

The bridge is a stand-in for the proxy's *shape*, not for the proxy. It does no
allowlisting, no rate limiting, no circuit breaking, no mTLS, and it runs on
the same host as the client.

## Run 1 — the connector runtime against the live endpoint

The opt-in suite that only runs when an egress address is supplied:

```
QIP_LIVE_FRANKFURTER_BASE_URL=http://127.0.0.1:18081 \
  cargo test -p qip-market-ingestion --test live_connectors -- --nocapture --test-threads=1
```

```
test the_frankfurter_rates_connector_fetches_a_live_rate_table_through_a_configured_egress ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.83s
```

The two requests it made, from the bridge transcript, with the bodies the
vendor actually returned:

```
2026-09-06T12:09:59Z GET /v1/latest                            -> 200 in 478 ms   (health probe, 441 bytes)
2026-09-06T12:10:00Z GET /v1/latest?base=EUR&symbols=USD,GBP,JPY -> 200 in 332 ms  (poll, 97 bytes)

{"amount":1.0,"base":"EUR","date":"2026-09-04","rates":{"GBP":0.85898,"JPY":181.59,"USD":1.1622}}
```

The health probe hits the manifest's `health_path`, which carries no query and
so returns all 29 currencies the ECB publishes; the poll carries the manifest's
`base=EUR&symbols=USD,GBP,JPY`. Both are real. The reference date the vendor
stamped is `2026-09-04` — the Friday fixing, because 2026-09-06 is a Sunday and
the ECB publishes on business days.

**The shipped fixture is not stale.** `connectors/fixtures/frankfurter-ecb-reference-rates.json`
records exactly the body above, byte for byte, from its own recording on
2026-09-04.

## Run 2 — the loop's bridge, and the bitemporal stamps (new test)

`live_connectors.rs` had a live `ConnectorFeed::open` test for Coinbase and
none for Frankfurter, and no live test anywhere read the *stamps* on a record
that came off the wire. This session added
`the_connector_feed_fetches_live_reference_rates_that_were_already_knowable_when_they_arrived`
(`backend/crates/services/qip-market-ingestion/tests/live_connectors.rs`),
which opens the source by name through the same egress address and asserts, for
each record released: it is a `SensedRecord::Macro`; its event time is midnight
UTC on a reference *date*; its provenance agrees with it; it carries an upstream
id; and — the assertion the test exists for — `event_time + publication_delay <=
the horizon it was released at`, so nothing was readable before it was knowable.

```
test the_connector_feed_fetches_live_reference_rates_that_were_already_knowable_when_they_arrived ... live evidence: 3 record(s) from frankfurter-ecb-reference-rates (Public) polled at 2026-09-06T12:13:04.349Z: FX.EUR.GBP=0.85898 (GBP per EUR), reference_date 2026-09-04, knowable 2026-09-04T16:00:00.000Z; FX.EUR.JPY=181.59 (JPY per EUR), reference_date 2026-09-04, knowable 2026-09-04T16:00:00.000Z; FX.EUR.USD=1.1622 (USD per EUR), reference_date 2026-09-04, knowable 2026-09-04T16:00:00.000Z
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.76s
```

Those five instants are the whole point of the connector: the rate was **true**
for 2026-09-04, became **knowable** at 2026-09-04T16:00Z (midnight UTC plus the
manifest's 57 600 000 ms dissemination delay), and was **ingested** at
2026-09-06T12:13:04Z. A backtest filtering on the first would have traded
Friday's open on Friday's close.

## Run 3 — the deployed binary, one cycle, and the event log

The real `qip-api` binary, unmodified, with the connector selected the way a
deployment selects it:

```
QIP_API_ADDRESS=127.0.0.1:18090 \
QIP_STORAGE_TARGET=file QIP_STORAGE_ROOT=<scratchpad>/state \
QIP_UNIVERSE_PATH=data/datasets/universe.json \
QIP_CONNECTOR_SOURCE=frankfurter-ecb-reference-rates \
QIP_CONNECTOR_BASE_URL=http://127.0.0.1:18081 \
QIP_TOKEN_OPERATOR=<ephemeral, session-local, never written to the repository> \
  backend/target/debug/qip-api
```

`POST /api/v1/cycle` → `HTTP 202`:

```
{"cycle":1,"correlation_id":"01M1VA1QT8NY3TBXZ6SRA0HZWY","halted":false,"traversed_every_stage":true,"archived":2,
 ...
 "sense":{"source":"frankfurter-ecb-reference-rates","at":"2026-09-06T12:10:49.544Z","released":3,"observed":3,"rejected":0,"rejections":[]}}
```

and the UNDERSTAND stage on the same response:

```
world model holds 0 instrument(s), 0 entity(ies), 0 relationship(s), 0 causal claim(s), 3 readable feature value(s), 0 document(s); 3 knowable event(s) held for the catalyst path
```

Three real ECB rates were released by the runtime, observed by the platform,
rejected none, and are readable as three feature values in the world model.
The cycle was archived to the hash chain on disk:

```
chain/00000000000000000000 digest 1546fcf78b3191c8 prev 0000000000000000 topic reference_data_updated
chain/00000000000000000001 digest 322a5ad62a5ca4b0 prev 1546fcf78b3191c8 topic learning_completed
chain/00000000000000000002 digest 74d4cbc7ce43e744 prev 322a5ad62a5ca4b0 topic learning_completed
```

**A second cycle, twenty minutes later, released nothing** — the vendor served
the identical Friday table and the runtime's dedup on the stable fingerprint
recognised it:

```
cycle 2  correlation_id 01M1VB02VGRT1SDEX42C6FRWGT
{"source": "frankfurter-ecb-reference-rates", "at": "2026-09-06T12:27:23.888Z", "released": 0, "observed": 0, "rejected": 0, "rejections": []}
world model holds ... 3 readable feature value(s) ...
```

Idempotency against a real source, not a scripted one: the same table twice
produced three observations and then none, and the three already held stayed
held.

## One thing worth fixing, found by running this — **fixed 2026-09-06**

The cycle response's SENSE stage reads `"produced":0` with the detail **"no
observations have been fed in; the platform is running blind"** on the very
cycle in which `"sense":{"released":3,"observed":3}`. Two claims about the same
fact, and the louder one is wrong. The stage line describes a different
mechanism from the feed's pre-cycle `observe`, and an operator reading the
stage table would conclude the source was dead.

**Closed on 2026-09-06, after this record was written.** `Platform::stage_sense`
(`backend/crates/runtime/qip-kernel/src/platform.rs`) no longer answers "has
anything been fed in" from the price series. It answers it from
`Platform::observations_absorbed` — the count `Platform::observe` returned to
the same caller that reports `observed` beside the stage, accumulated and
nothing else. One fact, one reading, so the two can no longer disagree. A macro
observation lands in the world model and the catalyst path and touches no price
series at all, which is exactly why the old reading called a platform blind
that had just absorbed three central-bank reference rates: it measured one
absorption arm and concluded about all of them.

The stage now says what it *holds* beside what it took in. Its format string is
`{held} observation(s) held from {absorbed} absorbed: {breakdown}{sourced}`, so
the three-macro-record case this session produced renders as `3 observation(s)
held from 3 absorbed: 3 knowable event(s)`. `produced` deliberately stays what
is held rather than what arrived — every store it counts is bounded, so under
load the two diverge and the divergence is the retention policy working, not a
lost record.

Two kernel tests hold it, both in
`backend/crates/runtime/qip-kernel/tests/kernel.rs` and both run for this
record:

```
$ cargo test -p qip-kernel --test kernel a_cycle_that_absorbed_reference_rates -- --nocapture
test a_cycle_that_absorbed_reference_rates_reports_them_rather_than_calling_itself_blind ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 32 filtered out; finished in 0.01s

$ cargo test -p qip-kernel --test kernel a_cycle_with_no_data_still_runs_every_stage
test a_cycle_with_no_data_still_runs_every_stage_and_says_why_each_was_quiet ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 32 filtered out; finished in 0.00s
```

The first asserts that the stage's `produced` equals the count `observe`
returned and that the detail does not contain "running blind"; the second keeps
the blind detail for the case that genuinely is blind, because a platform that
has absorbed nothing must still say so.

**This closes a reporting defect and moves no boundary.** The repair was made
and tested in-process against three synthetic records in the shape the ECB
connector releases them. It was not re-run against the live vendor, nothing
about it was deployed, and every limit in the next section still stands
unchanged.

## The precise boundary of the claim

What is proven, on this evidence:

* The shipped connector, its manifest, its runtime gates and the loop's adapter
  bridge work against the **real vendor**, today, and the committed fixture
  still matches the source byte for byte.
* The licensing gate ran **before** the socket, admitted a source whose terms
  were evaluated previously, and the run needed no gate weakened.
* Real rates crossed the whole in-process path into the world model and the
  cycle was sealed into the hash-chained event log.
* Knowability and dedup both held against real data.

What is **not** proven, and must not be inferred:

* **Nothing is deployed.** The egress proxy has never been applied; a local
  Python bridge on loopback is not Envoy, has no allowlist, and shares a host
  with its client. `qip-api` has no collector, no ingestion, and this run was a
  process on a build container.
* **This is one poll, twice.** Not a stream, not seven days, not an SLA. The
  freshness SLA (4 days) and the poll interval (1 hour) were never exercised
  over time.
* **The rates are not visible through any research route.** `/markets`,
  `/regimes`, `/correlation` and `/data-sources` all answered `available:false`
  for their own stated reasons. The evidence that the values are the vendor's
  is the connector-level test's printed line, not an API view.
* **No order, no position, no P&L.** The cycle proposed nothing and executed
  nothing; three FX observations are not a trading history.
* **A weekend reference date.** The table fetched was Friday's, already two days
  knowable. A run inside the vendor's publication window (roughly 14:00–16:00
  UTC on a business day) would correctly release nothing, and the new test's
  failure message says so rather than reading as a broken connector.

## Reproducing this

The bridge is not committed, so reproducing run 1 and run 2 needs any
TLS-terminating forward proxy in front of `api.frankfurter.dev` and its
plaintext address in `QIP_LIVE_FRANKFURTER_BASE_URL`. With no such address set,
both live tests print why they skipped and pass, which is how CI stays green
with no egress. Run 3 needs the same address in `QIP_CONNECTOR_BASE_URL` and
the environment block quoted above.
