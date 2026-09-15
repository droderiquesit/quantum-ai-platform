//! The ECB's three key interest rates, from the ECB's own data portal.
//!
//! One request returns one SDMX-JSON message carrying three series — the
//! deposit facility rate, the marginal lending facility rate and the main
//! refinancing operations fixed rate — each with its own observations:
//!
//! ```json
//! {"dataSets":[{"series":{"0:0:0:0:0:0:0":{"observations":{"0":[2.25,0,0,null,null]}}}}],
//!  "structure":{"dimensions":{"series":[ ... {"id":"PROVIDER_FM_ID","values":[{"id":"DFR"}]} ... ],
//!                             "observation":[{"id":"TIME_PERIOD","values":[{"id":"2026-09-15"}]}]}}}
//! ```
//!
//! Free, unauthenticated, no signup. The recorded body is
//! `fixtures/ecb-key-interest-rates.json`, fetched on 2026-09-15 at 21:10 UTC.
//!
//! # Why this source exists, and what was refused instead
//!
//! §38.3's reconciliation tolerance is `dust + rate x |expected|`, and the
//! fiat row's interval is "one day's interest accrual". Nothing in this
//! platform held a deposit rate, so every basis was declared at rate zero and
//! the tolerance was the dust floor everywhere. The tempting repair was a
//! constant somebody chose. A tolerance decides whether the books balance, and
//! a constant there is a halt that looks configured while judging real books
//! against an invention — so the rate is fetched from the institution that
//! sets it, carries that institution's name in its provenance, and is refused
//! when it cannot be read.
//!
//! **The deposit facility rate is a euro rate and governs euro balances.** It
//! is published here as an observation and nothing in this file decides what
//! it may be applied to; `qip_capital_fabric::tolerance::SourcedIntervalRate`
//! holds that question, and it refuses a currency the issuer does not set. A
//! euro policy rate applied to a dollar book would be a fabrication with a
//! citation attached, which is worse than the missing number it replaced.
//!
//! # The three instants, again
//!
//! The Governing Council announces a change to these rates in advance, and the
//! data portal carries a daily level for every calendar day. So:
//!
//! * the **event time** is the observation's `TIME_PERIOD` date, at midnight
//!   UTC — the day the level applied;
//! * the **knowable time** is sixteen hours later, from the manifest's
//!   `publication_delay_ms`. That is deliberately *conservative* rather than
//!   exact: a rate decided at a Governing Council meeting is public weeks
//!   before the date it applies to, so the true knowable instant is earlier
//!   than this one. Withholding a record for longer than it needed to be
//!   withheld can never leak; publishing it earlier than the vendor served it
//!   can, and there is no field on this feed that says when the daily row was
//!   written.
//! * the **ingest time** is when this platform fetched it.
//!
//! # Why the response is held to the question
//!
//! The series key in the manifest's path names seven SDMX dimensions, and the
//! third of them is the currency. This connector reads them at construction
//! and compares them against the dimensions the response declares, because the
//! whole point of the source is that a *euro* rate is a euro rate: a response
//! whose `CURRENCY` dimension said something else would mint
//! `POLICY_RATE.EA.DFR` out of a number about another currency, and that key
//! is what the capital fabric keys its own refusal on. Nothing on this hop is
//! authenticated — see the module documentation of
//! [`crate::connectors`] — so what arrived is compared with what was asked
//! for rather than believed.

use crate::adapter::{SensedRecord, bounded_excerpt};
use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_financial::intelligence::MacroObservation;
use qip_financial::quality::{DataQuality, Provenance};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The manifest this connector was written against.
pub const MANIFEST: &str = include_str!("manifests/ecb-key-interest-rates.json");

/// A body recorded from the live endpoint, for tests and for the harness.
///
/// Provenance: fetched on 2026-09-15 at 21:10 UTC from
/// `https://data-api.ecb.europa.eu/service/data/FM/D.U2.EUR.4F.KR.DFR+MLFR+MRR_FR.LEV?format=jsondata&lastNObservations=1`,
/// over a TLS connection verified against the session's CA bundle, and
/// recorded byte for byte. The vendor stamps its own `prepared` instant into
/// the header, so a re-recording differs from this one in that field even when
/// every rate is unchanged; the fixture is the bytes that were served, not a
/// body reduced to the parts this connector reads.
pub const FIXTURE: &str = include_str!("fixtures/ecb-key-interest-rates.json");

/// The ECB's key interest rates as a fan-out of macro observations.
///
/// The question and the answer are both held here: the SDMX series key is read
/// out of the manifest's own path at construction, so [`Self::decode`] compares
/// the frequency, the reference area, the currency and the rate identifiers the
/// response declares against the ones this connector asked for.
#[derive(Clone, Debug)]
pub struct EcbKeyRatesConnector {
    manifest: SourceManifest,
    /// The seven dimension values the manifest's path names, in SDMX order.
    key: SeriesKey,
}

/// The seven dimensions of an `FM` series key, as the manifest's path spells
/// them.
///
/// A named struct rather than a `Vec` because five of the seven are fixed
/// literals and two carry meaning this connector acts on; a positional list
/// would let a reordering of the path go unnoticed, and the position of the
/// currency is the one this whole source exists to be sure of.
#[derive(Clone, Debug)]
struct SeriesKey {
    frequency: String,
    reference_area: String,
    currency: String,
    /// The rate identifiers asked for, `+`-separated in the path.
    rates: BTreeSet<String>,
}

impl EcbKeyRatesConnector {
    /// The manifest's own `source_id`, named as a constant so
    /// [`crate::connector_feed`]'s bridge and the licensing catalogue that
    /// admits this source refer to one string rather than two that can drift.
    pub const SOURCE_ID: &str = "ecb-key-interest-rates";

    /// The vendor host the egress proxy dials for this source.
    ///
    /// Not a field the connector reads — the transport is pointed at the
    /// proxy, never at the vendor — but the one place in code the host is
    /// written, so the manifest's `provider` can be held to it by a test
    /// instead of by a reviewer's memory.
    pub const UPSTREAM_HOST: &str = "data-api.ecb.europa.eu";

    /// The region every observation carries: the euro area, because that is
    /// the area these rates are set for and by.
    pub const REGION: &'static str = "EA";

    /// The SDMX dimension naming which key rate a series is.
    const RATE_DIMENSION: &'static str = "PROVIDER_FM_ID";
    /// The SDMX dimension naming the currency the rate is denominated in.
    const CURRENCY_DIMENSION: &'static str = "CURRENCY";
    /// The SDMX dimension naming the area the rate applies to.
    const AREA_DIMENSION: &'static str = "REF_AREA";
    /// The SDMX observation dimension: the date each level applied on.
    const TIME_DIMENSION: &'static str = "TIME_PERIOD";

    /// How many dot-separated dimensions an `FM` key interest rate series key
    /// carries.
    const KEY_DIMENSIONS: usize = 7;
    /// Where the currency sits in that key.
    const CURRENCY_POSITION: usize = 2;
    /// Where the rate identifier sits in that key.
    const RATE_POSITION: usize = 5;

    /// The rate identifiers this connector will publish, and nothing else.
    ///
    /// A closed set for the same reason [`crate::connector_feed::KNOWN_SOURCES`]
    /// is one: each identifier becomes a permanent feature-store key and a
    /// permanent line in the event log, and each has a meaning a reader has to
    /// be able to look up. A manifest repointed at another `FM` series would
    /// mint a key nobody named; it is refused at construction rather than
    /// published.
    pub const PUBLISHED_RATES: [&'static str; 3] = ["DFR", "MLFR", "MRR_FR"];

    /// The only reference area these series are published for.
    const EURO_AREA: &'static str = "U2";
    /// The only currency the euro area's key rates are set in.
    ///
    /// Load-bearing rather than decorative. The capital fabric refuses to
    /// derive an interval rate for a currency the issuer does not set, and it
    /// can only do that if what arrives is known to be a euro rate.
    pub const CURRENCY: &'static str = "EUR";

    /// The lowest level this connector will publish, in percent per annum.
    ///
    /// Negative on purpose: the ECB's deposit facility rate sat at -0.50 from
    /// 2019 to 2022, and a floor at zero would have taken the feed down for
    /// three years of published policy. The Swiss National Bank's -0.75 is the
    /// most negative any major central bank has set, so a floor an order of
    /// magnitude below that admits every policy rate anyone has published
    /// while still refusing a number that is not a rate at all.
    pub const MIN_RATE: f64 = -10.0;

    /// The highest level this connector will publish, in percent per annum.
    ///
    /// The widest the ECB has ever set is the 1999 marginal lending facility
    /// at 5.75, and the highest a euro-area predecessor set is well inside
    /// this. A ceiling roughly four times the widest the issuer has published
    /// leaves room for a policy response nobody has seen, and still refuses
    /// `1e300` — which is finite, positive, admitted end to end, and infinite
    /// the moment anything squares it.
    pub const MAX_RATE: f64 = 25.0;

    pub fn shipped_manifest() -> Result<SourceManifest> {
        SourceManifest::from_json(MANIFEST)
    }

    /// Build the connector for the series its manifest asks for.
    ///
    /// Fails when the manifest's path does not spell a seven-dimension `FM`
    /// series key naming the euro area, the euro, and rate identifiers this
    /// build publishes. That is not bureaucracy: without it, [`Self::decode`]
    /// would have to take the currency from the response, and the currency is
    /// the fact everything downstream refuses on.
    pub fn new(manifest: SourceManifest) -> Result<Self> {
        manifest.validate()?;
        if manifest.publication_delay().is_zero() {
            return Err(Error::invalid(format!(
                "`{}` declares no dissemination delay, so every level would be knowable at \
                 midnight on the date it applied to. This feed carries no instant saying when \
                 the vendor wrote the row, so the delay is the only thing standing between a \
                 backtest and a rate it could not have read",
                manifest.source_id
            )));
        }
        let key = Self::series_key(&manifest)?;
        Ok(Self { manifest, key })
    }

    /// The seven dimensions the manifest's path spells, checked.
    fn series_key(manifest: &SourceManifest) -> Result<SeriesKey> {
        let last = manifest
            .endpoint
            .path
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .trim();
        let parts: Vec<&str> = last.split('.').collect();
        if parts.len() != Self::KEY_DIMENSIONS {
            return Err(Error::invalid(format!(
                "`{}` ends its path in {}, which is {} dot-separated dimension(s) where an FM \
                 key interest rate series key has {}. A key this connector cannot read is a key \
                 whose currency it would have to take from the response",
                manifest.source_id,
                bounded_excerpt(last),
                parts.len(),
                Self::KEY_DIMENSIONS
            )));
        }
        let currency = parts[Self::CURRENCY_POSITION];
        if currency != Self::CURRENCY {
            return Err(Error::invalid(format!(
                "`{}` asks for a series denominated in {}, and this connector publishes the euro \
                 area's key rates under `POLICY_RATE.{}.*`. A rate in another currency filed \
                 under those keys is a number the capital fabric would take for a euro rate",
                manifest.source_id,
                bounded_excerpt(currency),
                Self::REGION
            )));
        }
        let area = parts[1];
        if area != Self::EURO_AREA {
            return Err(Error::invalid(format!(
                "`{}` asks for reference area {}, and this connector publishes {} — the euro \
                 area. Filing another area's rate under this one's series id would make it look \
                 like a euro-area statistic",
                manifest.source_id,
                bounded_excerpt(area),
                Self::EURO_AREA
            )));
        }
        let mut rates = BTreeSet::new();
        for rate in parts[Self::RATE_POSITION].split('+') {
            let rate = rate.trim();
            if !Self::PUBLISHED_RATES.contains(&rate) {
                return Err(Error::invalid(format!(
                    "`{}` asks for rate identifier {}, which is not one this build publishes \
                     ({}). Each identifier becomes a permanent series id, so the set is closed \
                     in code rather than taken from configuration",
                    manifest.source_id,
                    bounded_excerpt(rate),
                    Self::PUBLISHED_RATES.join(", ")
                )));
            }
            rates.insert(rate.to_string());
        }
        if rates.is_empty() {
            return Err(Error::invalid(format!(
                "`{}` asks for no rate identifiers, so every poll would decode an empty message \
                 and the feed would look healthy while carrying nothing",
                manifest.source_id
            )));
        }
        Ok(SeriesKey {
            frequency: parts[0].to_string(),
            reference_area: area.to_string(),
            currency: currency.to_string(),
            rates,
        })
    }

    /// The declared values of one SDMX dimension, in the order the message
    /// lists them — which is the order the series key's indices refer to.
    fn dimension_values<'a>(payload: &'a Value, group: &str, id: &str) -> Result<Vec<&'a str>> {
        let dimensions = payload
            .pointer("/structure/dimensions")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                Error::schema(
                    "the message declares no `structure.dimensions`, so nothing says what its \
                     series keys index into",
                )
            })?;
        let list = dimensions
            .get(group)
            .and_then(Value::as_array)
            .ok_or_else(|| {
                Error::schema(format!(
                    "the message declares no `structure.dimensions.{group}`"
                ))
            })?;
        let dimension = list
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
            .ok_or_else(|| {
                Error::schema(format!(
                    "the message's `{group}` dimensions do not include `{id}`, so this connector \
                     cannot tell which of them the series key names"
                ))
            })?;
        let values = dimension
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::schema(format!("the `{id}` dimension declares no `values`")))?;
        values
            .iter()
            .map(|value| {
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::schema(format!("a `{id}` value carries no `id`")))
            })
            .collect()
    }

    /// Where a named series dimension sits in the message's own ordering.
    fn dimension_position(payload: &Value, id: &str) -> Result<usize> {
        let list = payload
            .pointer("/structure/dimensions/series")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                Error::schema("the message declares no `structure.dimensions.series`")
            })?;
        list.iter()
            .position(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
            .ok_or_else(|| {
                Error::schema(format!(
                    "the message's series dimensions do not include `{id}`"
                ))
            })
    }

    /// The message describes the series that was asked for, or a refusal
    /// naming what arrived.
    ///
    /// Both directions matter, and the currency most of all. A message about
    /// another currency would publish every level in it under a euro-area
    /// series id, and the capital fabric's refusal keys on that id.
    fn requested_series(&self, payload: &Value) -> Result<()> {
        for (dimension, expected) in [
            (Self::CURRENCY_DIMENSION, self.key.currency.as_str()),
            (Self::AREA_DIMENSION, self.key.reference_area.as_str()),
            ("FREQ", self.key.frequency.as_str()),
        ] {
            let declared = Self::dimension_values(payload, "series", dimension)?;
            if declared != [expected] {
                return Err(Error::schema(format!(
                    "the message's `{dimension}` dimension is {:?} and this connector asked for \
                     [{expected}]. A message about a different {dimension} is an answer to a \
                     question nobody asked, and publishing it would file every level in it under \
                     a series id that names the wrong one",
                    declared
                        .iter()
                        .map(|value| bounded_excerpt(value))
                        .collect::<Vec<_>>()
                )));
            }
        }
        let declared: BTreeSet<String> =
            Self::dimension_values(payload, "series", Self::RATE_DIMENSION)?
                .into_iter()
                .map(str::to_string)
                .collect();
        if declared != self.key.rates {
            return Err(Error::schema(format!(
                "the message carries rate identifiers {:?} and this connector asked for {:?}. \
                 Each unrequested identifier would mint a permanent feature series and a \
                 permanent event-log key from a value the vendor chose",
                declared.iter().collect::<Vec<_>>(),
                self.key.rates.iter().collect::<Vec<_>>()
            )));
        }
        Ok(())
    }

    /// A level inside the band, or a refusal naming the value.
    ///
    /// Refused, never clamped. Nobody knows what a level of `1e300` should
    /// have been, and a value silently corrected is a caller bug that survives
    /// into a backtest — and, here, into a reconciliation tolerance.
    fn admissible_rate(&self, rate_id: &str, rate: f64) -> Result<()> {
        if !rate.is_finite() {
            return Err(Error::schema(format!(
                "the level for {rate_id} is {rate}, which is not a number this platform will \
                 publish as a policy rate"
            )));
        }
        if !(Self::MIN_RATE..=Self::MAX_RATE).contains(&rate) {
            return Err(Error::schema(format!(
                "the level for {rate_id} is {rate}, outside the {}..={} percent per annum band a \
                 euro-area key interest rate can occupy. The widest the ECB has set is 5.75 and \
                 the lowest is -0.50, so a value outside this band is not a rate that moved — it \
                 is not a rate",
                Self::MIN_RATE,
                Self::MAX_RATE
            )));
        }
        Ok(())
    }

    /// `POLICY_RATE.EA.DFR` — publisher-neutral, and stable across a change of
    /// the requested set, so a series is not renamed when a rate is added.
    pub fn series_id(rate_id: &str) -> String {
        format!("POLICY_RATE.{}.{rate_id}", Self::REGION)
    }

    /// The rate identifier a decoded event carries.
    fn rate_id(event: &RawEvent) -> Result<&str> {
        event
            .body
            .get("rate_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::schema("a decoded level lost its rate identifier between decode and map")
            })
    }
}

impl SourceConnector for EcbKeyRatesConnector {
    fn manifest(&self) -> &SourceManifest {
        &self.manifest
    }

    /// One event per rate identifier per observation date.
    ///
    /// The body of each event carries the identifier, the level, the currency
    /// and the date only. It deliberately does *not* carry the whole message:
    /// the fingerprint is taken over the body, and a body containing the
    /// vendor's `prepared` instant would change on every poll, so an unchanged
    /// deposit facility rate would be published as new every hour.
    ///
    /// # Why an unrequested identifier refuses the whole message
    ///
    /// The same reason the reference-rate connector refuses an unrequested
    /// currency: a source answering a question this connector did not ask has
    /// either changed or been answered by someone else, and both are findings.
    /// The refusal quarantines the page with its reason rather than publishing
    /// the rest and recording nothing anybody reads.
    fn decode(&self, payload: &Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
        self.requested_series(payload)?;
        let rates = Self::dimension_values(payload, "series", Self::RATE_DIMENSION)?;
        let rate_position = Self::dimension_position(payload, Self::RATE_DIMENSION)?;
        let dimension_count = payload
            .pointer("/structure/dimensions/series")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        let dates = Self::dimension_values(payload, "observation", Self::TIME_DIMENSION)?;
        // Bounded before anything is allocated: this message is a cross
        // product of series and dates, and a well-formed large answer is
        // exactly what `max_events_per_batch` exists to refuse.
        let most = rates.len().saturating_mul(dates.len());
        if most > self.manifest.max_events_per_batch {
            return Err(Error::schema(format!(
                "the message carries {} series across {} date(s), which is {most} level(s) \
                 against a declared ceiling of {}. A response this large is either a different \
                 query or a vendor change, and decoding it first is how a well-formed answer \
                 becomes an unbounded allocation",
                rates.len(),
                dates.len(),
                self.manifest.max_events_per_batch
            )));
        }
        let series = payload
            .pointer("/dataSets/0/series")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                Error::schema(
                    "the message's first data set carries no `series` object, so it declares a \
                     shape and reports no levels",
                )
            })?;
        // Collected into a map keyed by identifier and date so the events come
        // out in one stable order whatever order the vendor wrote its keys in:
        // a replay that reorders is not a replay.
        let mut levels: BTreeMap<(String, String), f64> = BTreeMap::new();
        for (series_key, body) in series {
            let indices: Vec<&str> = series_key.split(':').collect();
            if indices.len() != dimension_count {
                return Err(Error::schema(format!(
                    "the series key {} names {} dimension(s) and the message declares \
                     {dimension_count}; a key this connector cannot index is a level it cannot \
                     attribute to a rate",
                    bounded_excerpt(series_key),
                    indices.len()
                )));
            }
            let rate_id = indices
                .get(rate_position)
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| rates.get(index))
                .ok_or_else(|| {
                    Error::schema(format!(
                        "the series key {} does not index a declared rate identifier",
                        bounded_excerpt(series_key)
                    ))
                })?;
            let observations = body
                .get("observations")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    Error::schema(format!(
                        "the series {} carries no `observations`",
                        bounded_excerpt(series_key)
                    ))
                })?;
            for (index, observation) in observations {
                let date = index
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| dates.get(index))
                    .ok_or_else(|| {
                        Error::schema(format!(
                            "the observation index {} does not name a declared time period",
                            bounded_excerpt(index)
                        ))
                    })?;
                let rate = observation
                    .as_array()
                    .and_then(|values| values.first())
                    .and_then(Value::as_f64)
                    .ok_or_else(|| {
                        Error::schema(format!(
                            "the observation for {rate_id} on {date} carries no level as its \
                             first value"
                        ))
                    })?;
                self.admissible_rate(rate_id, rate)?;
                levels.insert(((*rate_id).to_string(), (*date).to_string()), rate);
            }
        }
        let mut events = Vec::with_capacity(levels.len());
        for ((rate_id, date), rate) in levels {
            let applied_on = Timestamp::parse_rfc3339(&date).ok_or_else(|| {
                Error::schema(format!(
                    "the time period {} is not a date this platform can read",
                    bounded_excerpt(&date)
                ))
            })?;
            events.push(RawEvent::new(
                format!("{rate_id}@{date}"),
                applied_on,
                serde_json::json!({
                    "rate_id": rate_id,
                    "rate": rate,
                    "currency": self.key.currency,
                    "date": applied_on.to_date_string(),
                }),
            ));
        }
        Ok(events)
    }

    /// The record, and the second place the identifier and the band are
    /// checked.
    ///
    /// This is the seam where the permanent artefacts are minted — the series
    /// id, the unit string, the value a `FeatureValue` is built from. `decode`
    /// is where a bad message is found; `map` is where a bad identifier would
    /// *become* a key, and a connector whose two halves disagree about what it
    /// asked for is a connector where only one of them is the gate.
    fn map(&self, event: &RawEvent, ingest_time: Timestamp) -> Result<SensedRecord> {
        let rate_id = Self::rate_id(event)?;
        if !self.key.rates.contains(rate_id) {
            return Err(Error::schema(format!(
                "a decoded level names rate identifier {}, which is not one of the {} this \
                 connector requested",
                bounded_excerpt(rate_id),
                self.key.rates.len()
            )));
        }
        let rate = event
            .body
            .get("rate")
            .and_then(Value::as_f64)
            .ok_or_else(|| Error::schema("a decoded level lost its rate between decode and map"))?;
        self.admissible_rate(rate_id, rate)?;
        let currency = event
            .body
            .get("currency")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::schema("a decoded level lost its currency between decode and map")
            })?;
        if currency != self.key.currency {
            return Err(Error::schema(format!(
                "a decoded level says it is denominated in {} and this connector asked for {}",
                bounded_excerpt(currency),
                self.key.currency
            )));
        }
        let provenance = Provenance::new(
            self.manifest.source_id.clone(),
            event.event_time,
            // The caller's horizon, not the wall clock: the same fetch
            // replayed in a backtest must produce the same record.
            ingest_time,
        )
        .with_licensing(self.manifest.licensing)
        .with_upstream_id(event.key.clone());
        Ok(SensedRecord::Macro(Box::new(MacroObservation {
            series_id: Self::series_id(rate_id),
            region: Self::REGION.to_string(),
            // A published statistic, not money: `MacroObservation::value` is
            // `f64` because that is what the rest of the platform's statistics
            // are. The crossing into `Decimal` happens once, in
            // `qip_capital_fabric::tolerance::SourcedIntervalRate::from_percent_per_annum`,
            // because from there on it multiplies a balance.
            value: rate,
            // The unit the ECB's own message declares for these series, spelled
            // out rather than abbreviated: a reader who takes `2.25` for a
            // fraction rather than a percentage is out by two orders of
            // magnitude, and this string is the only thing that says which.
            unit: format!("percent per annum, {currency}"),
            reference_date: event.event_time,
            // The ECB publishes no consensus and no prior level on this feed,
            // and inventing either would put a surprise into a signal nobody
            // forecast.
            consensus: None,
            previous: None,
            // A key rate level for a past date is not revised. A source that
            // began revising would show up as a second event with the same key
            // and a different body, which is a new fingerprint — visible as a
            // duplicate key rather than a silent overwrite.
            is_revision: false,
            provenance,
            quality: DataQuality::clean(),
        })))
    }
}

// The workspace denies `panic_in_result_fn` for production code; a test that
// returns `Result` so it can use `?` on the manifest loader still has to
// assert, and the abort is the reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    /// The manifest is the document a reviewer and the acceptance suite read;
    /// the constant is what the code says. The reference-rate connector had
    /// the two drift apart when its vendor moved hosts and nothing compared
    /// them. The token is matched inside the parentheses the provider
    /// convention uses, because a bare `contains` would also pass for a longer
    /// hostname that merely ends in this one.
    #[test]
    fn the_shipped_manifest_names_the_same_upstream_host_as_the_code() -> Result<()> {
        let manifest = EcbKeyRatesConnector::shipped_manifest()?;
        // Premise: the constant is a bare hostname, not a URL or a path.
        assert!(
            !EcbKeyRatesConnector::UPSTREAM_HOST.contains('/')
                && EcbKeyRatesConnector::UPSTREAM_HOST.contains('.'),
            "UPSTREAM_HOST is {:?}, which is not a bare hostname",
            EcbKeyRatesConnector::UPSTREAM_HOST
        );
        let named = format!("({})", EcbKeyRatesConnector::UPSTREAM_HOST);
        assert!(
            manifest.provider.contains(&named),
            "the shipped manifest's provider is {:?} and does not name {named}",
            manifest.provider
        );
        Ok(())
    }

    /// The currency is the fact the capital fabric refuses on, so a manifest
    /// repointed at a series in another currency must not build a connector
    /// at all. This is the check that stops a euro rate's series id being
    /// minted out of a number about something else.
    #[test]
    fn a_manifest_pointed_at_a_series_in_another_currency_does_not_build_a_connector() -> Result<()>
    {
        let shipped = EcbKeyRatesConnector::shipped_manifest()?;
        // Premise: the shipped manifest does build one, so the refusal below
        // is about the edit and not about the rest of the manifest.
        EcbKeyRatesConnector::new(shipped.clone())?;
        let mut repointed = shipped;
        repointed.endpoint.path = "/service/data/FM/D.U2.USD.4F.KR.DFR.LEV".to_string();
        let refused = EcbKeyRatesConnector::new(repointed)
            .expect_err("a series in another currency built a euro-area key-rate connector");
        assert!(
            refused.message().contains("denominated in"),
            "the refusal is not about the currency: {}",
            refused.message()
        );
        Ok(())
    }
}
