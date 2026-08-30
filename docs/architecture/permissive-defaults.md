# Permissive defaults

## The class

Some `Default` implementations encode the **permissive or optimistic** value:
the class that grants, the measurement that is perfect, the status that trades.
Wherever such a type can be produced by a default rather than by a decision —
`#[serde(default)]`, `unwrap_or_default()`, `..Default::default()`, a `Default`
derive on a struct containing one — **absent input silently becomes a grant or
a claim.**

The failure is not that the default is wrong in the abstract. It is that
absence and assertion become indistinguishable. A vendor that said nothing
produces byte-for-byte the record of a vendor that measured everything, and
nothing downstream can tell them apart, because by the time the value is read
the distinction no longer exists in the data.

The three cases that started this were each found while building something
else, not by looking:

1. **`LicensingClass::default()` is `Internal`**, for which
   `allows_raw_display()` is true. A vendor record arriving with no licensing
   class reads downstream as permission to display licensed text.
2. **`DataQuality::default()`** sets `completeness: 1.0`, `confidence: 1.0`,
   `is_imputed: false` — a perfect, directly observed measurement, which clears
   `DECISION_QUALITY_FLOOR`, the gate deciding whether data may drive a capital
   decision. Imputed vendor data arriving with no quality block presents as
   observed fact.
3. **Venue status defaulting to `Open`**, which lets a crossed book be read as
   continuous-session corruption rather than an auction, or the reverse.

`qip_contracts::VenueStatus` and `qip_contracts::TradeCondition` carry no
`Default` today, which is case 3 already fixed.

## The three outcomes

- **Refuse.** Absence cannot be told from a stated permissive value, and the
  consequence is a grant or a false claim. Say so where the data arrives.
- **Change the default.** The permissive value is simply the wrong one and no
  caller depends on it. Check every construction site first.
- **Leave it, and write down why.** Counters that default to zero, subjects
  that name nothing, types only ever built in-tree. A comment naming why it is
  safe is worth more than silence, because the next reader will wonder.

Refusing at the boundary is preferred over changing a widely used `Default`.
`DataQuality::default()` is *right* as `DataQuality::clean()` — a record that
passed every check. What was wrong was reaching it from silence.

## What was changed

### 1. `qip_market::TradeCondition` — Default removed, absence refused

`backend/crates/libs/qip-market/src/quote.rs`

`TradeCondition::default()` was `Regular`, and `Trade::condition` was
`#[serde(default)]`. `Regular` is the one condition for which
`is_price_forming()` is true. A late report or an off-exchange cross whose
condition a venue omitted decoded as an ordinary continuous-session print and
was admitted to price discovery — a VWAP and a volatility estimate both moved
by a print that never traded on the continuous book.

`qip_contracts::TradeCondition`, the edge's own copy of the same idea, already
had no `Default` for exactly this reason. This is the library half catching up.
The `Default` derive is gone and the field is no longer `#[serde(default)]`, so
a record that does not state how it printed does not decode at all.

Test: `a_trade_that_states_no_condition_does_not_decode`
(`backend/crates/libs/qip-market/tests/structure.rs`).

### 2. The REST adapter's absent trade condition — refused

`backend/crates/services/qip-market-ingestion/src/rest.rs`

`decode_trade` read `None => TradeCondition::Regular`. The module's own
documentation already said what it refuses — "a trade condition this decoder
cannot name" — so an *unreadable* condition was refused while an *absent* one
was promoted to the price-forming value. The decoder cannot tell "the vendor
says this printed normally" from "the vendor said nothing", so it now declines
to guess, and the refusal names both the print and the value it will not fall
back to. The module docs were corrected to match.

Test: `an_absent_trade_condition_is_refused_rather_than_read_as_regular`
(`backend/crates/services/qip-market-ingestion/tests/rest_feed.rs`), which sits beside
the existing `an_unreadable_trade_condition_is_refused_rather_than_defaulted_to_regular`.

### 3. `DataQuality` on the wire types — absence refused

`backend/crates/libs/qip-market/src/{quote.rs,bar.rs}` (`Quote`, `Trade`, `Tick`,
`Bar`), `backend/crates/libs/qip-financial/src/intelligence.rs` (`NewsItem`,
`FundamentalUpdate`, `MacroObservation`, `AlternativeDataPoint`).

Eight types carried `#[serde(default)] pub quality: DataQuality`. This is
case 2 at the type level: any JSON without a `quality` key decoded as a perfect
measurement that clears `DECISION_QUALITY_FLOOR`. The live adapters
(`narrative`, `alternative`) already refuse an unstated quality block, but they
are not the only way these types are built from bytes — the replay adapter, the
event log read back at assembly, the mesh and `qip-storage` all deserialise
them, and each was a separate place for the same silence to become a claim.

`DataQuality::default()` is unchanged: it is `DataQuality::clean()`, it is
correct, and a great deal depends on it. What is removed is the ability to
reach it from an absent field. Serde now refuses, once, for every boundary at
the same time. Nothing serialises these types without `quality` — no
`skip_serializing_if` — so anything the platform wrote can still be read back.

Tests: `a_market_record_that_states_no_quality_does_not_decode`
(`backend/crates/libs/qip-market/tests/structure.rs`),
`an_intelligence_record_that_states_no_quality_does_not_decode`
(`backend/crates/libs/qip-financial/tests/object_model.rs`).

### 4. `ReplayAdapter` — an undeclared capture is no longer internally licensed

`backend/crates/services/qip-market-ingestion/src/replay.rs`

`ReplayAdapter::open` and `from_records` hardcoded
`licensing: LicensingClass::Internal` — case 1, verbatim, at a boundary. A
capture file is data the platform did not author: it may be a session this
deployment recorded, or a vendor's own extract somebody dropped on a disk, and
nothing in the file says which. `qip-fastbrain` reads that path straight out of
the environment (`QIP_FASTBRAIN_REPLAY_PATH` → `Feed::replay`), so a path in a
config map arrived downstream as permission to show a vendor's raw prices —
`Internal.allows_raw_display()` is true — *and* as production grade, since
`SourceDescriptor::is_production_grade()` is
`licensing.allows_production_decisions()` and that is true for `Internal` too.

The field is now `Option<LicensingClass>`, `None` meaning nobody said, and the
descriptor reports `Restricted` — non-displayable, derived values only — until
`with_licensing` states otherwise. That is the same fallback `narrative` and
`alternative` already use for a feed configured with no class. `Restricted` and
not `Synthetic`: recorded data is not generated data, and a replay of a real
session is exactly what a backtest is supposed to be allowed to reason from.

Test: `a_replay_nobody_declared_a_licence_for_is_not_displayable`
(`backend/crates/services/qip-market-ingestion/tests/sense.rs`).

### 5. `ListingStatus` — Default removed

`backend/crates/libs/qip-financial/src/extensions.rs`

`ListingStatus::default()` was `Listed`. Nothing reached it: `EquityDetails`
does not derive `Default` and the field is not `#[serde(default)]`, so it was a
latent trap rather than a live bug — but it was one edit away from being live,
and the edit is the kind nobody would think twice about. There is no honest
"unknown" variant to change the default *to*, and inventing a listing state is
worse than not having one, so the derive is gone. A suspended or delisted line
now cannot decode as a tradable one by omission.

Test: `an_equity_that_states_no_listing_status_does_not_decode`
(`backend/crates/libs/qip-financial/tests/object_model.rs`).

## What was examined and deliberately left

The value of this section is that it is complete, not that it is short.
Somebody will look at one of these and wonder whether it was missed.

| Type / site | Default | Why it stays |
| --- | --- | --- |
| `LicensingClass::default()` (`qip-financial`) | `Internal` | The permissive value, but load-bearing: `Provenance::new` and a great deal of in-tree construction use it, and the derive is what the three known refusals *test against*. The fix is to refuse at each boundary, which is what they do. Changing it here would silently reclassify every in-tree record instead. |
| `DataQuality::default()` (`qip-financial`) | perfect measurement | Same reasoning. It is `clean()`, it is correct as a statement, and it is the premise every refusal test asserts. Only the paths that reached it from silence were closed. |
| `Provenance` | no `Default` | `licensing` is a required field with no `serde(default)`. Already correct. |
| `qip_contracts::VenueStatus` | no `Default` | Case 3, already fixed. `Open` would be the permissive value; there is deliberately no derive. |
| `qip_contracts::TradeCondition` | no `Default` | Already correct, and the precedent used for change 1. |
| `qip_data_finder::legal::Legality` | no `Default` | The model the rest of this should follow: three-valued, `Unknown` is not a grant, `is_permitted()` is false for it, and the unanswered question travels with the verdict. |
| `qip_compliance::licensing` entitlements | — | "An unrecorded licence is treated as absent, not as permission." Exemplary; nothing to do. |
| `IdempotencySupport::default()` (`qip-brokers`) | `Absent` | The **conservative** default: under it the adapter refuses to retry a submit whose outcome is unknown. A default that costs something is the shape to aim for. |
| `Durability::default()` (`qip-storage`, `qip-events`) | `Synchronous` | Conservative: `fsync` before returning. The permissive value would be `OsBuffered`, and it is not the default. |
| `EventLogDestination::default()` (`qip-kernel`) | `InMemory` | Documented as the only destination that cannot be wrong about itself. A default *path* would be one nobody chose. |
| `AutonomyLevel::DEFAULT` (`qip-risk-engine`) | `PaperTrading` | Nothing reaches a market. The permissive values are above it, and a separate configured ceiling bounds them independently. |
| `Extension::default()` (`qip-financial`) | `Unspecified` | Claims nothing. Documented as a legitimate transient state during ingestion. |
| `Sector::default()` (`qip-financial`) | `Unclassified` | Claims nothing. Concentration limits work per axis, so unclassified names are limited together rather than escaping a limit. |
| `Subject::default()` (`qip-streaming`) | all `None` | Names nothing — the correct value for a platform-internal event about no instrument. A good example of a Default that is honest about absence. |
| `LotMethod::default()` (`qip-portfolio`) | `FirstInFirstOut` | A jurisdictional convention, and a choice rather than a grant. Whichever lot is consumed, the accounting is exact. |
| `Regime::default()` (`qip-market-ingestion/synthetic`) | `Calm` | Reachable only from the in-tree synthetic generator, which authors its own inputs. Not a boundary. |
| `Environment::default()` (`qip-core`) | `Local` | Documented as controlling "which safety defaults apply", but nothing outside its own module branches on it today. Left as-is and recorded here: the first consumer that gates a safety behaviour on it turns `Local` into the permissive value, and this row is the warning. |
| `WireLog::removed` (`qip-chain`) | `false` | Absence genuinely means "not retracted" in the JSON-RPC shape; nodes state `removed: true` explicitly. Refusing every receipt log from a node that omits an optional field would reject the correct majority, and a retracted log read as live is still caught by the confirmation-depth check. |
| `WireReceipt::status` (`qip-chain`) | `None` | Already refuses rather than reading absence as success — the crate had learned this lesson before this audit. |
| `InsertAllResponse::insert_errors` (`qip-storage/gcp`) | empty | Absence means success in BigQuery's own contract, and the response is a reply to a request this platform made rather than an unsolicited claim. |
| `QueryResponse::job_complete` (`qip-storage/gcp`) | `false` | Conservative: absence reads as *not* complete. |
| `AnyEvent::sequence` (`qip-events`) | `0` | A counter assigned on append. Zero means "not appended yet", which is what it says. |
| `CellStateDelta::refusals_omitted`, `reconciliation_breaks_omitted` (`qip-edge`) | `0` | Optimistic in form — zero claims the list is complete — but the payload is hash-verified against what the sending cell wrote, and the fields exist for schema evolution: making them required would refuse deltas from cells one version behind, during exactly the incident the fields describe. Recorded because it is the closest call on this list. |
| `WireMention::is_primary`, `WireSentiment::novelty` (`qip-market-ingestion`) | `false` / `0.0` | Conservative: a mention is not primary and an item is not novel unless the vendor says so. |
| `ModelReputation::record` (`qip-cost-router`) | zeroed record | Documented: an unseen context returns a coin flip, not the model's average elsewhere. Borrowing a record across contexts is what the module exists to prevent. |
| `DataQuality::clean()` in `rest.rs` decoders | perfect | Not absence becoming a claim — the adapter states it. Prices are observed rather than imputed, and the module deliberately passes vendor incoherence through to `IngestionService`'s validation gate so that bad data becomes a visible `DataQualityFailure` rather than a silently dropped record. Left, but it is the row most worth revisiting if a price vendor ever starts shipping imputed marks. |
| `DataQuality::default()` in `qip-brokers/exchange.rs` | perfect | The simulated exchange, which authors the book it is describing and stamps `simulated: true`. A perfect measurement of a perfectly known book. |
| `ObjectBuilder`'s `DataQuality::clean()` (`qip-financial`) | perfect | A builder is an in-tree decision, and `.quality()` overrides it. `FinancialObject` itself requires both `provenance` and `quality` on the wire. |
| Policy structs — `HoldoutPolicy`, `PaperPolicy`, `ShadowPolicy`, `HealthPolicy`, `Policy` (`qip-confidential`) | thresholds | Each default is the strict end with the reasoning written above it. These are the defaults doing useful work; removing them would replace a documented threshold with an undocumented one at every call site. |

## How to test one of these

A test that only asserts the refusal passes for the wrong reason: it stays
green if the default is later changed to something harmless, and it never
proved there was anything to protect. So **assert the premise first** —
demonstrate the default really is the permissive value — and only then that
absence is refused. The pattern the existing suite already uses:

```rust
assert_eq!(LicensingClass::default(), LicensingClass::Internal);
assert!(
    LicensingClass::default().allows_raw_display(),
    "an unset class is not a neutral one: it is a grant, which is exactly why \
     the adapter refuses rather than falling back to it"
);
```

Where the `Default` has been removed outright, the premise is stated as a
round-trip instead: show that the permissive value is what the field's wire
form produces, then that the field's *absence* does not produce it.

## Where to look next

Work outward from the boundary. Data the platform did not author enters
through the live adapters (`qip-market-ingestion`, `qip-chain/rpc`,
`qip-brokers/rest`, `qip-storage/gcp`, `qip-training/vertex`,
`qip-quantum/provider`), the mesh (`qip-transport`, `qip-edge/mesh`,
`qip-mesh/spine`), persisted state read back (`qip-storage`, `ChainArchive`,
the edge journal, replay captures) and configuration (every `from_env`).

Three greps find most of it:

```
grep -rn '#\[default\]' backend/crates/                     # permissive enum variants
grep -rn -A2 '#\[serde(default)\]' backend/crates/          # fields reachable from silence
grep -rn 'unwrap_or_default()' backend/crates/              # the same thing, spelled out
```

The question to ask at each hit is not "is this default sensible" but: **if
this value arrived because nobody said anything, does it read downstream as
somebody having said yes?**
