//! The Effective Federal Funds Rate, from the New York Fed's own markets API.
//!
//! One request returns one JSON object carrying an array of daily reference
//! rate rows, newest first:
//!
//! ```json
//! {"refRates":[{"effectiveDate":"2026-09-14","type":"EFFR","percentRate":3.63,
//!               "percentPercentile1":3.60, ... ,"volumeInBillions":91,
//!               "revisionIndicator":""}]}
//! ```
//!
//! Free, unauthenticated, no signup. The recorded body is
//! `fixtures/nyfed-effr.json`, fetched on 2026-09-16 at 01:57 UTC.
//!
//! # Why this source exists, and what it repairs
//!
//! §38.3's reconciliation tolerance is `dust + rate x |expected|`, and the
//! fiat row's interval is "one day's interest accrual". The
//! `ecb-key-interest-rates` connector put a *published* rate behind that arm
//! for the first time — and then ran into the limit the lane recorded
//! honestly: the ECB sets euro-area rates, the one §38.3 class this kernel
//! can attest without being told is the desk's own cash at its broker, and
//! that book is in **dollars**. So no production basis carried a non-zero
//! rate: the machinery worked and had no number for the currency it was asked
//! about. Pointing the euro rate at the dollar book would have been a
//! fabrication with a citation attached. This connector supplies the dollar
//! number from the institution that publishes it.
//!
//! **EFFR is a US dollar rate and governs dollar balances.** It is published
//! here as an observation and nothing in this file decides what it may be
//! applied to; `qip_capital_fabric::tolerance::SourcedIntervalRate` holds that
//! question and refuses a currency the issuer does not set.
//!
//! # The currency is not in the payload, and that is stated rather than papered over
//!
//! The ECB's SDMX message carries an explicit `CURRENCY` dimension, and that
//! connector compares it against the one the manifest asked for. This response
//! carries **no currency field at all**. The dollar is a property of the named
//! rate — the Effective Federal Funds Rate is the rate on overnight federal
//! funds, which are US dollar balances at Federal Reserve Banks — so the
//! currency here is asserted from the rate's identity and *not* verified
//! against the body. What is verified is the identity: the manifest's path
//! names `unsecured/effr`, [`Self::new`] refuses any other, and [`Self::decode`]
//! refuses any row whose `type` is not the one that path asked for. That is
//! the whole of the defence on an unauthenticated hop, and it is narrower than
//! the ECB's by exactly one field, which is why it is written down here.
//!
//! # The three instants
//!
//! The New York Fed computes EFFR from the prior business day's transactions
//! and publishes it on the following business day at approximately 09:00 New
//! York time. So:
//!
//! * the **event time** is the row's `effectiveDate`, at midnight UTC — the
//!   day the transactions it summarises took place;
//! * the **knowable time** is five days later, from the manifest's
//!   `publication_delay_ms`. Five, not the thirty-eight hours a weekday
//!   publication actually takes, because a single scalar has to cover the
//!   worst case or it leaks on that case: the recorded fixture itself shows
//!   the gap, with no row for 2026-09-07 and 2026-09-04's rate therefore not
//!   published until 2026-09-08 — four days and thirteen hours after midnight
//!   UTC on the date it applies to. A December cluster reaches the same. The
//!   cost of being conservative is a rate used later than it had to be; the
//!   cost of being exact-on-average is a backtest reading a Friday rate on
//!   Saturday, which is the leakage
//!   `.claude/rules/domains/data-and-streaming.md` puts first among its
//!   prohibitions.
//! * the **ingest time** is when this platform fetched it.
//!
//! That delay is also why the manifest asks for **ten** rows rather than one.
//! A row is only the newest row for about a day; with a five-day delay, a feed
//! asking for `last/1` would withhold every row it ever saw and would look
//! healthy while publishing nothing for ever. Ten rows span roughly a
//! fortnight of calendar days, so every row is still being served when it
//! becomes knowable.
//!
//! # The licence obligation this connector carries
//!
//! The New York Fed's Terms of Use place a *use restriction* on reference rate
//! data: a notice and disclaimer must accompany any presentation of it. That
//! text is [`NyFedEffrConnector::REFERENCE_RATE_NOTICE`], written once here,
//! evaluated in `qip_data_finder::admission`'s catalogue, and carried into
//! every §38.3 tolerance record derived from this rate by
//! `qip_capital_fabric::tolerance::SourcedIntervalRate::presentation_notice`.
//! A constant nothing carries would be a licence term in a comment.

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
use std::collections::BTreeMap;

/// The manifest this connector was written against.
pub const MANIFEST: &str = include_str!("manifests/nyfed-effr.json");

/// A body recorded from the live endpoint, for tests and for the harness.
///
/// Provenance: fetched on 2026-09-16 at 01:57 UTC from
/// `https://markets.newyorkfed.org/api/rates/unsecured/effr/last/10.json`,
/// over a TLS connection verified against the session's CA bundle, and
/// recorded byte for byte — the SHA-256 of the string this file embeds is the
/// SHA-256 of the bytes `curl` wrote. The response carries no server-generated
/// instant, so a re-recording differs from this one only where the published
/// series has actually moved.
pub const FIXTURE: &str = include_str!("fixtures/nyfed-effr.json");

/// The New York Fed's Effective Federal Funds Rate as a macro observation per
/// business day.
///
/// The question and the answer are both held here: the rate family and the
/// rate identifier are read out of the manifest's own path at construction, so
/// [`Self::decode`] can refuse a row whose `type` is not the one that path
/// asked for.
#[derive(Clone, Debug)]
pub struct NyFedEffrConnector {
    manifest: SourceManifest,
    /// The path's own statement of what was asked for.
    question: RateQuestion,
}

/// What the manifest's path asks the markets API for.
///
/// A named struct rather than a positional list for the reason the ECB
/// connector gives about its series key: the fields carry meaning this
/// connector acts on, and a reordering of the path would otherwise go
/// unnoticed.
#[derive(Clone, Debug)]
struct RateQuestion {
    /// `effr`, lowercase as the path spells it.
    rate: String,
    /// How many observations the path asks for. The response may not carry
    /// more.
    observations: usize,
}

impl NyFedEffrConnector {
    /// The manifest's own `source_id`, named as a constant so
    /// [`crate::connector_feed`]'s bridge and the licensing catalogue that
    /// admits this source refer to one string rather than two that can drift.
    pub const SOURCE_ID: &str = "nyfed-effr";

    /// The vendor host the egress proxy dials for this source.
    ///
    /// Not a field the connector reads — the transport is pointed at the
    /// proxy, never at the vendor — but the one place in code the host is
    /// written, so the manifest's `provider` can be held to it by a test
    /// instead of by a reviewer's memory.
    pub const UPSTREAM_HOST: &str = "markets.newyorkfed.org";

    /// The region every observation carries: the United States, because that
    /// is the area this rate is set for and by.
    pub const REGION: &'static str = "US";

    /// The currency federal funds are denominated in.
    ///
    /// Load-bearing rather than decorative, and **asserted rather than read**:
    /// see this module's documentation. The capital fabric refuses to derive
    /// an interval rate for a currency the issuer does not set, and it can
    /// only do that if what arrives is known to be a dollar rate. Here that
    /// knowledge comes from the identity of the rate the path named, because
    /// the payload does not carry one.
    pub const CURRENCY: &'static str = "USD";

    /// The rate family the path must name.
    const FAMILY: &'static str = "unsecured";

    /// The rate identifier this connector will publish, and nothing else.
    ///
    /// A closed set of one, for the reason
    /// [`crate::connector_feed::KNOWN_SOURCES`] is one: the identifier becomes
    /// a permanent feature-store key and a permanent line in the event log. A
    /// manifest repointed at SOFR or OBFR would mint a key nobody named, and —
    /// worse — SOFR's terms are not these: the same Use Restriction section
    /// records that SOFR and BGCR are calculated under a DTCC Solutions
    /// licence with its own liability disclaimer. A second rate is a second
    /// licensing evaluation, not a path edit.
    pub const PUBLISHED_RATE: &'static str = "effr";

    /// The value the response's `type` field must carry, for the one rate this
    /// connector publishes.
    const PUBLISHED_TYPE: &'static str = "EFFR";

    /// The lowest level this connector will publish, in percent per annum.
    ///
    /// Negative on purpose, and not because EFFR has ever printed negative —
    /// it has not; its floor is the 0.04 of 2011 and 2021. A floor at zero
    /// would take the feed down on the first day a policy nobody has seen was
    /// published, and the Swiss National Bank's -0.75 is the most negative any
    /// major central bank has set, so a floor an order of magnitude below that
    /// admits every policy rate anyone has published while still refusing a
    /// number that is not a rate at all.
    pub const MIN_RATE: f64 = -10.0;

    /// The highest level this connector will publish, in percent per annum.
    ///
    /// The highest daily effective federal funds rate on record is the 22.36
    /// of July 1981. A ceiling of roughly twice that leaves room for a policy
    /// response nobody has seen and still refuses `1e300` — which is finite,
    /// positive, admitted end to end, and infinite the moment anything squares
    /// it.
    pub const MAX_RATE: f64 = 50.0;

    /// The notice and disclaimer the New York Fed's Terms of Use require to
    /// accompany any presentation of reference rate data.
    ///
    /// Quoted from the Use Restrictions section of
    /// `https://www.newyorkfed.org/privacy/termsofuse` (Last Updated 6/9/2023,
    /// read for this connector on 2026-09-16), with the bracketed details the
    /// terms say the publisher must complete filled in: the data is the
    /// Effective Federal Funds Rate (EFFR) and the publisher is this platform.
    ///
    /// # Why it is a constant here rather than a sentence in a document
    ///
    /// The obligation attaches to *presentation* of the figure, not to the act
    /// of fetching it, so the text has to travel with the number rather than
    /// sit beside the code that reads the socket.
    /// `qip_capital_fabric::tolerance::SourcedIntervalRate` carries it into
    /// every §38.3 tolerance record derived from this rate, which is the one
    /// place the derived figure is written down today. What the platform
    /// cannot do is compel a screen that has not been built yet to render it;
    /// see the catalogue entry in `qip_data_finder::admission`, which says so
    /// in the same words the ECB entry uses about acknowledgement.
    pub const REFERENCE_RATE_NOTICE: &'static str = "The Effective Federal Funds Rate (EFFR) is subject to the Terms of Use posted at \
         newyorkfed.org. The New York Fed is not responsible for publication of the EFFR by the \
         PEOS Quantum AI platform, does not sanction or endorse any particular republication, \
         and has no liability for your use.";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        SourceManifest::from_json(MANIFEST)
    }

    /// Build the connector for the rate its manifest asks for.
    ///
    /// Fails when the manifest's path does not spell the markets API's
    /// `last/N` form for a rate family and identifier this build publishes.
    /// That is not bureaucracy: without it [`Self::decode`] would have nothing
    /// to hold the response's `type` against, and the response carries no
    /// currency of its own to fall back on.
    pub fn new(manifest: SourceManifest) -> Result<Self> {
        manifest.validate()?;
        if manifest.publication_delay().is_zero() {
            return Err(Error::invalid(format!(
                "`{}` declares no dissemination delay, so every rate would be knowable at \
                 midnight on the date its transactions took place — a full business day before \
                 the New York Fed computes it. This feed carries no instant saying when the row \
                 was written, so the delay is the only thing standing between a backtest and a \
                 rate it could not have read",
                manifest.source_id
            )));
        }
        let question = Self::question(&manifest)?;
        Ok(Self { manifest, question })
    }

    /// What the manifest's path asks for, checked.
    ///
    /// The markets API spells a reference rate request as
    /// `/api/rates/{family}/{rate}/last/{n}.json`.
    fn question(manifest: &SourceManifest) -> Result<RateQuestion> {
        let path = manifest.endpoint.path.trim();
        let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let shape = ["api", "rates"];
        if segments.len() != 6 || segments[..2] != shape || segments[4] != "last" {
            return Err(Error::invalid(format!(
                "`{}` asks for {}, and this connector reads the markets API's \
                 `/api/rates/<family>/<rate>/last/<n>.json` form. A path this connector cannot \
                 read is a path whose rate identifier it would have to take from the response",
                manifest.source_id,
                bounded_excerpt(path)
            )));
        }
        let family = segments[2];
        if family != Self::FAMILY {
            return Err(Error::invalid(format!(
                "`{}` asks for the {} rate family, and this connector publishes {} — the \
                 unsecured overnight rates. The secured families are calculated under a DTCC \
                 Solutions licence whose terms are not the ones evaluated for this source",
                manifest.source_id,
                bounded_excerpt(family),
                Self::FAMILY
            )));
        }
        let rate = segments[3];
        if rate != Self::PUBLISHED_RATE {
            return Err(Error::invalid(format!(
                "`{}` asks for rate identifier {}, which is not the one this build publishes \
                 ({}). The identifier becomes a permanent series id and carries its own \
                 licensing evaluation, so the set is closed in code rather than taken from \
                 configuration",
                manifest.source_id,
                bounded_excerpt(rate),
                Self::PUBLISHED_RATE
            )));
        }
        let count = segments[5].strip_suffix(".json").unwrap_or(segments[5]);
        let observations = count.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| {
            Error::invalid(format!(
                "`{}` asks for {} observations, which is not a positive count. A feed asking for \
                 none would answer empty for ever and read as a healthy source publishing \
                 nothing",
                manifest.source_id,
                bounded_excerpt(count)
            ))
        })?;
        if observations > manifest.max_events_per_batch {
            return Err(Error::invalid(format!(
                "`{}` asks for {observations} observations against its own declared ceiling of \
                 {}. A manifest that asks for more than it will accept quarantines every \
                 healthy answer",
                manifest.source_id, manifest.max_events_per_batch
            )));
        }
        Ok(RateQuestion {
            rate: rate.to_string(),
            observations,
        })
    }

    /// The rows the response carries, or a refusal naming what arrived.
    fn rows<'a>(&self, payload: &'a Value) -> Result<&'a Vec<Value>> {
        let rows = payload
            .get("refRates")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                Error::schema(
                    "the response carries no `refRates` array, so it declares a shape and \
                     reports no rates",
                )
            })?;
        // Bounded before anything is allocated: a well-formed large answer is
        // exactly what the path's own `last/N` and `max_events_per_batch`
        // exist to refuse.
        if rows.len() > self.question.observations {
            return Err(Error::schema(format!(
                "the response carries {} row(s) and the path asked for the last {}. An answer \
                 larger than the question is either a different query or a vendor change, and \
                 decoding it first is how a well-formed answer becomes an unbounded allocation",
                rows.len(),
                self.question.observations
            )));
        }
        Ok(rows)
    }

    /// A level inside the band, or a refusal naming the value.
    ///
    /// Refused, never clamped. Nobody knows what a level of `1e300` should
    /// have been, and a value silently corrected is a caller bug that survives
    /// into a backtest — and, here, into a reconciliation tolerance.
    fn admissible_rate(&self, date: &str, rate: f64) -> Result<()> {
        if !rate.is_finite() {
            return Err(Error::schema(format!(
                "the level for {} on {date} is {rate}, which is not a number this platform will \
                 publish as a policy rate",
                self.question.rate
            )));
        }
        if !(Self::MIN_RATE..=Self::MAX_RATE).contains(&rate) {
            return Err(Error::schema(format!(
                "the level for {} on {date} is {rate}, outside the {}..={} percent per annum \
                 band an effective federal funds rate can occupy. The highest daily level on \
                 record is 22.36 and the lowest is 0.04, so a value outside this band is not a \
                 rate that moved — it is not a rate",
                self.question.rate,
                Self::MIN_RATE,
                Self::MAX_RATE
            )));
        }
        Ok(())
    }

    /// `POLICY_RATE.US.EFFR` — publisher-neutral, and the same shape the
    /// euro-area key rates use, so a reader of the feature store meets one
    /// naming convention rather than two.
    pub fn series_id() -> String {
        format!("POLICY_RATE.{}.{}", Self::REGION, Self::PUBLISHED_TYPE)
    }

    /// One row's `type`, held to the one the path asked for.
    ///
    /// The narrow half of the defence described in this module's
    /// documentation. Nothing on this hop is authenticated, so a row claiming
    /// to be some other reference rate is refused by name rather than
    /// published under this one's series id.
    fn declared_type<'a>(&self, row: &'a Value, index: usize) -> Result<&'a str> {
        let declared = row.get("type").and_then(Value::as_str).ok_or_else(|| {
            Error::schema(format!(
                "the row at index {index} carries no `type`, so nothing says which reference \
                 rate it is"
            ))
        })?;
        if declared != Self::PUBLISHED_TYPE {
            return Err(Error::schema(format!(
                "the row at index {index} is a {} rate and this connector asked for {}. A row \
                 about a different reference rate published under `{}` is a number filed against \
                 a rate it is not, and the capital fabric keys its own refusal on that id",
                bounded_excerpt(declared),
                Self::PUBLISHED_TYPE,
                Self::series_id()
            )));
        }
        Ok(declared)
    }
}

impl SourceConnector for NyFedEffrConnector {
    fn manifest(&self) -> &SourceManifest {
        &self.manifest
    }

    /// One event per effective date.
    ///
    /// The body of each event carries the rate, the date, the currency and
    /// whether the vendor flagged the row as a revision, and nothing else. It
    /// deliberately does not carry the whole row: the fingerprint is taken
    /// over the body, and the percentile and volume fields move independently
    /// of the rate this platform reads, so an unchanged rate would be
    /// published as new whenever the volume was restated.
    ///
    /// # Why a revision is a body field rather than a dropped row
    ///
    /// The New York Fed's terms reserve the right to alter "rate revision
    /// practices ... at any time without prior notice", and the response
    /// carries a `revisionIndicator` for exactly that. A revised row is a
    /// different fingerprint, so it arrives as a new event on the same date
    /// rather than silently replacing one — which is what lets
    /// `PolicyRateTable::record` see it at all.
    fn decode(&self, payload: &Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
        let rows = self.rows(payload)?;
        // Collected into a map keyed by date so the events come out oldest
        // first whatever order the vendor wrote its array in. The vendor
        // serves newest first, and `PolicyRateTable::record` refuses a figure
        // stamped earlier than the one it holds — so publishing in the
        // vendor's own order would record the newest rate and then refuse
        // every older one as a superseded replay. A replay that reorders is
        // not a replay, and here it is also a feed that fills the capture
        // problems on a healthy poll.
        let mut levels: BTreeMap<String, (f64, bool)> = BTreeMap::new();
        for (index, row) in rows.iter().enumerate() {
            self.declared_type(row, index)?;
            let date = row
                .get("effectiveDate")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    Error::schema(format!(
                        "the row at index {index} carries no `effectiveDate`, so nothing says \
                         which day its rate applied to"
                    ))
                })?;
            let rate = row
                .get("percentRate")
                .and_then(Value::as_f64)
                .ok_or_else(|| {
                    Error::schema(format!(
                        "the row for {} carries no `percentRate` as a number",
                        bounded_excerpt(date)
                    ))
                })?;
            self.admissible_rate(date, rate)?;
            // Absent is not the same as empty, and both mean "not revised":
            // the vendor writes `""` today and a missing field would be a
            // shape change rather than a revision, so neither is read as one.
            let revised = row
                .get("revisionIndicator")
                .and_then(Value::as_str)
                .is_some_and(|flag| !flag.trim().is_empty());
            levels.insert(date.to_string(), (rate, revised));
        }
        let mut events = Vec::with_capacity(levels.len());
        for (date, (rate, revised)) in levels {
            let applied_on = Timestamp::parse_rfc3339(&date).ok_or_else(|| {
                Error::schema(format!(
                    "the effective date {} is not a date this platform can read",
                    bounded_excerpt(&date)
                ))
            })?;
            events.push(RawEvent::new(
                format!("{}@{date}", Self::PUBLISHED_TYPE),
                applied_on,
                serde_json::json!({
                    "rate": rate,
                    "currency": Self::CURRENCY,
                    "date": applied_on.to_date_string(),
                    "revised": revised,
                }),
            ));
        }
        Ok(events)
    }

    /// The record, and the second place the band is checked.
    ///
    /// This is the seam where the permanent artefacts are minted — the series
    /// id, the unit string, the value a `FeatureValue` is built from.
    /// [`Self::decode`] is where a bad response is found; `map` is where a bad
    /// level would *become* a key, and a connector whose two halves disagree
    /// about what it asked for is a connector where only one of them is the
    /// gate.
    fn map(&self, event: &RawEvent, ingest_time: Timestamp) -> Result<SensedRecord> {
        let rate = event
            .body
            .get("rate")
            .and_then(Value::as_f64)
            .ok_or_else(|| Error::schema("a decoded level lost its rate between decode and map"))?;
        self.admissible_rate(&event.event_time.to_date_string(), rate)?;
        let currency = event
            .body
            .get("currency")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::schema("a decoded level lost its currency between decode and map")
            })?;
        if currency != Self::CURRENCY {
            return Err(Error::schema(format!(
                "a decoded level says it is denominated in {} and federal funds are {}",
                bounded_excerpt(currency),
                Self::CURRENCY
            )));
        }
        let revised = event
            .body
            .get("revised")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                Error::schema("a decoded level lost its revision flag between decode and map")
            })?;
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
            series_id: Self::series_id(),
            region: Self::REGION.to_string(),
            // A published statistic, not money: `MacroObservation::value` is
            // `f64` because that is what the rest of the platform's statistics
            // are. The crossing into `Decimal` happens once, in
            // `qip_capital_fabric::tolerance::SourcedIntervalRate::from_percent_per_annum`,
            // because from there on it multiplies a balance.
            value: rate,
            // The unit the New York Fed publishes this series in, spelled out
            // rather than abbreviated: a reader who takes `3.63` for a
            // fraction rather than a percentage is out by two orders of
            // magnitude, and this string is the only thing that says which.
            unit: format!("percent per annum, {currency}"),
            reference_date: event.event_time,
            // The markets API publishes no consensus and no prior level on
            // this endpoint, and inventing either would put a surprise into a
            // signal nobody forecast.
            consensus: None,
            previous: None,
            // The vendor's own flag, not this platform's inference. A revised
            // rate for a past date is a fact the terms explicitly reserve the
            // right to produce, and a record that hid it would make the
            // republished figure indistinguishable from the original.
            is_revision: revised,
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
        let manifest = NyFedEffrConnector::shipped_manifest()?;
        // Premise: the constant is a bare hostname, not a URL or a path.
        assert!(
            !NyFedEffrConnector::UPSTREAM_HOST.contains('/')
                && NyFedEffrConnector::UPSTREAM_HOST.contains('.'),
            "UPSTREAM_HOST is {:?}, which is not a bare hostname",
            NyFedEffrConnector::UPSTREAM_HOST
        );
        let named = format!("({})", NyFedEffrConnector::UPSTREAM_HOST);
        assert!(
            manifest.provider.contains(&named),
            "the shipped manifest's provider is {:?} and does not name {named}",
            manifest.provider
        );
        Ok(())
    }

    /// The rate identifier is what everything downstream keys on, and the
    /// secured families carry a different licence entirely — SOFR and BGCR are
    /// calculated under a DTCC Solutions licence the catalogue entry for this
    /// source did not evaluate. A manifest repointed at one must not build a
    /// connector at all.
    #[test]
    fn a_manifest_pointed_at_another_reference_rate_does_not_build_a_connector() -> Result<()> {
        let shipped = NyFedEffrConnector::shipped_manifest()?;
        // Premise: the shipped manifest does build one, so the refusal below
        // is about the edit and not about the rest of the manifest.
        NyFedEffrConnector::new(shipped.clone())?;

        let mut secured = shipped.clone();
        secured.endpoint.path = "/api/rates/secured/sofr/last/10.json".to_string();
        let refused = NyFedEffrConnector::new(secured)
            .expect_err("a secured reference rate built an EFFR connector");
        assert!(
            refused.message().contains("rate family"),
            "the refusal is not about the family: {}",
            refused.message()
        );

        let mut sibling = shipped;
        sibling.endpoint.path = "/api/rates/unsecured/obfr/last/10.json".to_string();
        let refused = NyFedEffrConnector::new(sibling)
            .expect_err("the overnight bank funding rate built an EFFR connector");
        assert!(
            refused.message().contains("rate identifier"),
            "the refusal is not about the identifier: {}",
            refused.message()
        );
        Ok(())
    }
}
