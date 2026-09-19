//! Surface weather observations from one station, served by the United States
//! National Weather Service's own public API.
//!
//! One request returns a GeoJSON feature collection, newest observation first,
//! each feature carrying the station's readings with the publisher's own
//! quality-control grade beside every one:
//!
//! ```json
//! {"features":[{"properties":{
//!    "station":"https://api.weather.gov/stations/KORD",
//!    "timestamp":"2026-09-19T05:00:00+00:00",
//!    "temperature":{"unitCode":"wmoUnit:degC","value":21,"qualityControl":"V"},
//!    "visibility":{"unitCode":"wmoUnit:m","value":16093.44,"qualityControl":"C"},
//!    "seaLevelPressure":{"unitCode":"wmoUnit:Pa","value":null,"qualityControl":"Z"}}}]}
//! ```
//!
//! Free, unauthenticated, no signup. The recorded body is
//! `fixtures/nws-station-observations.json`, fetched on 2026-09-19 at 05:21
//! UTC.
//!
//! # Why this source exists
//!
//! §7.1 names nine source classes. Before this connector the tree had a live
//! connector for three of them — Market, Economic and Resolution — and the
//! platform's own outcomes made a fourth. **Physical** was one of the five
//! with none: the blueprint's row for it reads "Shipping rates, port
//! congestion, weather, satellite, inventories", at an hourly-to-daily
//! cadence, unlocking "Commodities, freight, product arbitrage". Weather is
//! named in that row, and a surface observation is the one member of it
//! published by a body whose licensing posture can be read in an afternoon
//! rather than negotiated.
//!
//! This connector is therefore the fourth source class, and it is deliberately
//! the *narrowest* useful member of its class. It reads one station. A feed of
//! every station in the United States is the same code with a different path
//! and a much larger argument about rate limits, and it is not what a platform
//! with no deployed process needs first.
//!
//! # What this connector refuses to claim
//!
//! [`qip_financial::intelligence::AlternativeDataPoint`] carries three fields
//! about what a reading is a *proxy* for — `proxies_for`, `proxy_correlation`
//! and `lead_days` — and this connector leaves all three at nothing measured.
//! That is the design and not an omission.
//!
//! Nobody here has measured the correlation between the temperature at
//! Chicago O'Hare and any fundamental, so any number written into
//! `proxy_correlation` would be a figure this platform invented about its own
//! data. The consequence is visible and intended:
//! [`qip_financial::intelligence::AlternativeDataPoint::is_actionable`]
//! answers `false` for every record this connector produces, because it
//! requires `proxies_for` to be set. A reading that says "I am an observation
//! and I am not yet evidence for anything" is worth more than one that asserts
//! a relationship nobody computed — this repository's standing example of what
//! not to ship is a control that reads as protection and cannot fire, and an
//! invented proxy correlation is that failure with the sign reversed: a number
//! that always fires and means nothing.
//!
//! # The publisher grades its own readings, and this connector carries the grade
//!
//! Every reading arrives with a `qualityControl` letter — the NWS's own
//! verdict on the value beside it. That is unusual and it is the reason this
//! source is worth having: `DataQuality::default` asserts a *perfect*
//! measurement and clears [`qip_financial::quality::DECISION_QUALITY_FLOOR`],
//! so a connector that dropped the grade would publish a coarse-pass
//! visibility reading and a validated temperature as the same record.
//! [`ObservationGrade`] maps each letter to a confidence, and
//! [`NwsStationObservationsConnector::quality_of`] is where it reaches the
//! envelope.
//!
//! Two different things can be wrong with a grade, and they are answered
//! differently on purpose:
//!
//! * **A grade this connector does not know refuses the whole page.** The
//!   source's vocabulary has changed, which is a finding: the page is
//!   quarantined under `DecodeFailure` with the letter named, and somebody
//!   reads it. Admitting the reading under a guessed confidence would file a
//!   number under a quality nobody established.
//! * **A grade the publisher itself marks bad skips that one reading.** `X`,
//!   `Q` and `B` are the NWS saying its own sensor failed validation. That is
//!   the world, not a protocol violation, and taking the feed down because one
//!   barometer broke would be an outage manufactured out of an ordinary
//!   instrument fault. The same split is the one
//!   [`crate::connectors::frankfurter_rates`] already draws between an
//!   unrequested currency, which refuses the table, and a missing requested
//!   one, which does not.
//!
//! A `null` value is likewise skipped rather than refused. A station that has
//! no sea-level pressure sensor reports the field with a null in it on every
//! observation for ever, and a connector that refused the page for it would
//! never publish anything.
//!
//! # The three instants
//!
//! * the **event time** is the observation's own `timestamp` — when the
//!   instrument read;
//! * the **knowable time** is one hour later, from the manifest's
//!   `publication_delay_ms`. The recorded fixture shows the true lag is far
//!   shorter — the 05:00 observation was served at 05:21 — but a single scalar
//!   has to cover the worst case or it leaks on that case, and a delayed
//!   transmission from a station on a marginal link is the worst case here.
//!   The cost of being conservative is a reading used later than it had to be;
//!   the cost of being exact-on-average is a backtest reading an observation
//!   before the wire carried it, which is the leakage
//!   `.claude/rules/domains/data-and-streaming.md` puts first among its
//!   prohibitions;
//! * the **ingest time** is the caller's horizon, never a clock read, so the
//!   same fetch replayed in a backtest produces the same record.
//!
//! That delay is also why the manifest asks for **twelve** observations rather
//! than one. An ASOS station reports every five minutes, so twelve spans
//! fifty-five minutes — the recorded fixture runs from 04:05 to 05:00 — and an
//! hourly poll therefore never steps over an observation it has not seen. A
//! feed asking for one would withhold each observation until it was knowable
//! and by then be serving a newer one, and would look healthy while publishing
//! nothing for ever. That is the failure
//! [`crate::connectors::nyfed_effr`] records for the same reason.
//!
//! # The offset is checked, because the parser does not
//!
//! `qip_core::Timestamp::parse_rfc3339` discards a numeric UTC offset and
//! reads what is left as UTC. For this feed the offset is always `+00:00`, so
//! the parse is right — but it is right by coincidence rather than by
//! checking, and an event time is a permanent bitemporal stamp. If this
//! endpoint ever served `-05:00`, every observation would be filed five hours
//! early, which is point-in-time leakage that no test of the arithmetic would
//! catch. [`NwsStationObservationsConnector::observed_at`] therefore refuses
//! any timestamp whose offset is not UTC rather than relying on a parser that
//! cannot object.
//!
//! # Unreachable in this build, and it says so
//!
//! `qip_transport::http` has no TLS stack and refuses `https` by name rather
//! than downgrading it, and `api.weather.gov` is HTTPS only. So the manifest
//! ships with **no `base_url`**, which makes
//! [`crate::connector::manifest::SourceManifest::missing_configuration`] name
//! what is missing and
//! [`crate::connector::transport::HttpSourceTransport::connect`] refuse. A
//! deployment supplies the address of a TLS-terminating egress proxy in front
//! of the source. Nothing is applied, so no deployed process of this platform
//! has an outbound path to this host at all.
//!
//! # One obligation this connector cannot discharge itself
//!
//! The NWS asks an API client to identify itself in a `User-Agent`.
//! `qip_transport` writes its own fixed `user-agent: qip-transport/1.1` and a
//! manifest cannot override it — `user-agent` is one of the headers the client
//! owns. So a request from this platform *is* identified, and it carries no
//! contact address, which is what the NWS recommends rather than requires.
//! Recorded here rather than worked around: changing it means changing
//! `qip-transport`, which is a decision about every connector and not about
//! this one.

use crate::adapter::{SensedRecord, bounded_excerpt};
use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_financial::intelligence::AlternativeDataPoint;
use qip_financial::quality::{DataQuality, Provenance};
use serde_json::Value;

/// The manifest this connector was written against.
pub const MANIFEST: &str = include_str!("manifests/nws-station-observations.json");

/// A body recorded from the live endpoint, for tests and for the harness.
///
/// Provenance: fetched on 2026-09-19 at 05:21 UTC from
/// `https://api.weather.gov/stations/KORD/observations?limit=12`, over a TLS
/// connection verified against the session's CA bundle, and recorded byte for
/// byte — the string this fixture embeds is the response as served. The
/// response carries no server-generated instant, so a re-recording differs
/// from this one only where the station has actually reported again.
pub const FIXTURE: &str = include_str!("fixtures/nws-station-observations.json");

/// The publisher's own verdict on one reading.
///
/// The letters are the NWS's, not this platform's, and the confidence beside
/// each is this platform's reading of what the letter means. Kept as an enum
/// rather than as a string so that the set is closed: an unrecognised letter
/// cannot become a new arm at runtime, it becomes a refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObservationGrade {
    /// `V` — validated. The reading passed the publisher's checks.
    Validated,
    /// `S` — subjective good. A human judged it sound.
    SubjectiveGood,
    /// `C` — coarse pass. Screened, but only against a wide band.
    CoarsePass,
    /// `Z` — preliminary: no quality control has been applied at all.
    Preliminary,
    /// `X` — failed validation.
    Failed,
    /// `Q` — questionable.
    Questionable,
    /// `B` — subjective bad. A human judged it wrong.
    SubjectiveBad,
    /// `T` — virtual or trace. A value standing in for "too little to
    /// measure" rather than a measurement.
    Trace,
}

impl ObservationGrade {
    /// The letter as the NWS writes it.
    pub const fn letter(&self) -> &'static str {
        match self {
            Self::Validated => "V",
            Self::SubjectiveGood => "S",
            Self::CoarsePass => "C",
            Self::Preliminary => "Z",
            Self::Failed => "X",
            Self::Questionable => "Q",
            Self::SubjectiveBad => "B",
            Self::Trace => "T",
        }
    }

    /// What the letter means, in the record's own voice.
    pub const fn describe(&self) -> &'static str {
        match self {
            Self::Validated => "validated by the publisher",
            Self::SubjectiveGood => "judged sound by a human observer",
            Self::CoarsePass => "screened only against a coarse band",
            Self::Preliminary => "preliminary: the publisher applied no quality control",
            Self::Failed => "failed the publisher's validation",
            Self::Questionable => "flagged questionable by the publisher",
            Self::SubjectiveBad => "judged wrong by a human observer",
            Self::Trace => "a trace or virtual value rather than a measurement",
        }
    }

    /// Whether the publisher is telling us the reading is wrong.
    ///
    /// A reading this answers `true` for is skipped rather than published.
    /// `Trace` is **not** one of them: a trace is a real statement about the
    /// world — there was precipitation, below the instrument's resolution —
    /// and discarding it would turn "almost none" into "no observation", which
    /// are different facts.
    pub const fn is_repudiated(&self) -> bool {
        matches!(
            self,
            Self::Failed | Self::Questionable | Self::SubjectiveBad
        )
    }

    /// The letter this platform reads, or nothing.
    ///
    /// Returns `Option` rather than defaulting, because the caller's answer to
    /// an unknown letter is to refuse the page and say which letter it was —
    /// a default here would bury that.
    pub fn from_letter(letter: &str) -> Option<Self> {
        match letter {
            "V" => Some(Self::Validated),
            "S" => Some(Self::SubjectiveGood),
            "C" => Some(Self::CoarsePass),
            "Z" => Some(Self::Preliminary),
            "X" => Some(Self::Failed),
            "Q" => Some(Self::Questionable),
            "B" => Some(Self::SubjectiveBad),
            "T" => Some(Self::Trace),
            _ => None,
        }
    }

    /// This reading's quality, as the publisher graded it.
    ///
    /// Each confidence is written out rather than derived by chaining
    /// `DataQuality::with_issue`, and the difference is not cosmetic.
    /// `with_issue` halves the remaining confidence per call, so one issue
    /// lands on 0.5 — just under
    /// [`qip_financial::quality::DECISION_QUALITY_FLOOR`] — and *which* grades
    /// may drive a decision would then be settled by how many times a
    /// constructor happened to be called rather than by a judgement about the
    /// grade. This was written that way first, and it put a coarse-pass
    /// reading below the floor by arithmetic coincidence.
    ///
    /// The boundary the numbers are chosen around is the one that means
    /// something: **screened against something, versus not screened at all.**
    /// `V`, `S`, `C` and `T` are readings the publisher looked at and they
    /// clear the floor; `Z` is the publisher saying it applied no quality
    /// control, and it does not. That is the whole reason the grade is carried
    /// rather than dropped — `DataQuality::default` would assert the opposite.
    ///
    /// `completeness` is 1.0 throughout: the field was present and carried a
    /// value, or this grade would never have been asked for. Confidence is
    /// what the grade speaks to.
    pub fn quality(&self) -> DataQuality {
        let graded = |confidence: f64| DataQuality {
            completeness: 1.0,
            confidence,
            validation_failures: 1,
            issues: vec![format!(
                "the publisher reports the reading is {}",
                self.describe()
            )],
            is_imputed: false,
        };
        match self {
            Self::Validated => DataQuality::clean(),
            // A human judged it sound: short of the instrument's own
            // validation, comfortably above the floor.
            Self::SubjectiveGood => graded(0.9),
            // A trace is a real statement about the world — there was
            // precipitation, below what the instrument resolves — so it is
            // published, and it is less precise than a measured value.
            Self::Trace => graded(0.8),
            // Screened only against a wide band, but screened. Clears the
            // floor.
            Self::CoarsePass => graded(0.75),
            // Below the floor on purpose. The publisher is saying it has not
            // checked this number.
            Self::Preliminary => graded(0.5),
            // Reachable only through a caller that ignored `is_repudiated`.
            // Graded rather than made unreachable so that a future caller
            // which does publish one cannot publish it as clean.
            Self::Failed | Self::Questionable | Self::SubjectiveBad => DataQuality {
                completeness: 1.0,
                confidence: 0.0,
                validation_failures: 2,
                issues: vec![
                    format!("the publisher reports the reading is {}", self.describe()),
                    "the publisher repudiates this value and it should not have been published"
                        .to_string(),
                ],
                is_imputed: false,
            },
        }
    }
}

/// One reading this connector understands, and the unit it must arrive in.
///
/// The unit is half the entry and the more important half. A source that
/// changed `temperature` from Celsius to Fahrenheit would keep the field name,
/// the grade and the shape, and every number would silently mean something
/// else — and unlike a missing field, nothing downstream could ever notice.
/// So the unit is asserted, not read.
struct Reading {
    /// The property name in the payload.
    field: &'static str,
    /// The metric name this platform files it under. Deliberately not the
    /// payload's name: a metric name is a permanent feature-store key, and
    /// `barometricPressure` in a vendor's house style should not become this
    /// platform's vocabulary.
    metric: &'static str,
    /// The `unitCode` the payload must carry for this field.
    unit_code: &'static str,
    /// The unit as this platform writes it in a record.
    unit: &'static str,
}

/// Every reading this connector publishes.
///
/// A field outside this table is ignored rather than refused: the NWS adds
/// derived properties — `heatIndex`, `windChill`, `cloudLayers` — and a
/// connector that refused a page for carrying one would break the first time
/// the publisher added anything. What is *not* ignored is a field in this
/// table arriving in the wrong unit or with an unknown grade.
const READINGS: &[Reading] = &[
    Reading {
        field: "temperature",
        metric: "air_temperature",
        unit_code: "wmoUnit:degC",
        unit: "degrees Celsius",
    },
    Reading {
        field: "dewpoint",
        metric: "dewpoint_temperature",
        unit_code: "wmoUnit:degC",
        unit: "degrees Celsius",
    },
    Reading {
        field: "relativeHumidity",
        metric: "relative_humidity",
        unit_code: "wmoUnit:percent",
        unit: "percent",
    },
    Reading {
        field: "windSpeed",
        metric: "wind_speed",
        unit_code: "wmoUnit:km_h-1",
        unit: "kilometres per hour",
    },
    Reading {
        field: "windGust",
        metric: "wind_gust",
        unit_code: "wmoUnit:km_h-1",
        unit: "kilometres per hour",
    },
    Reading {
        field: "windDirection",
        metric: "wind_direction",
        unit_code: "wmoUnit:degree_(angle)",
        unit: "degrees clockwise from true north",
    },
    Reading {
        field: "barometricPressure",
        metric: "station_pressure",
        unit_code: "wmoUnit:Pa",
        unit: "pascals",
    },
    Reading {
        field: "seaLevelPressure",
        metric: "sea_level_pressure",
        unit_code: "wmoUnit:Pa",
        unit: "pascals",
    },
    Reading {
        field: "visibility",
        metric: "visibility",
        unit_code: "wmoUnit:m",
        unit: "metres",
    },
    Reading {
        field: "precipitationLast3Hours",
        metric: "precipitation_3h",
        unit_code: "wmoUnit:mm",
        unit: "millimetres",
    },
];

/// The reading table entry for a metric name, for [`Self::map`]'s re-check.
fn reading_for_metric(metric: &str) -> Option<&'static Reading> {
    READINGS.iter().find(|entry| entry.metric == metric)
}

/// Surface observations from one NWS station as alternative data points.
#[derive(Clone, Debug)]
pub struct NwsStationObservationsConnector {
    manifest: SourceManifest,
    /// The station the manifest's own path asks for, so [`Self::decode`] can
    /// refuse an observation from anywhere else.
    station: String,
}

impl NwsStationObservationsConnector {
    /// The manifest's own `source_id`, named as a constant so
    /// [`crate::connector_feed`]'s bridge and the licensing catalogue that
    /// admits this source refer to one string rather than two that can drift.
    pub const SOURCE_ID: &str = "nws-station-observations";

    /// The vendor host the egress proxy dials for this source.
    ///
    /// Not a field the connector reads — the transport is pointed at the proxy
    /// — but the one place the hostname is written down, so the allowlist, the
    /// Envoy bootstrap and the licensing catalogue can each be held to it by a
    /// test rather than by a reviewer's memory.
    pub const UPSTREAM_HOST: &str = "api.weather.gov";

    /// The dataset every reading is filed under.
    ///
    /// One dataset for the source rather than one per metric: the metric is
    /// its own field, and a dataset per metric would make the dataset name and
    /// the metric name two places to change one fact.
    pub const DATASET: &str = "weather.surface_observations";

    /// The region every observation carries.
    ///
    /// The United States, because that is who operates the observing network.
    /// The station's own coordinates are in the payload and are deliberately
    /// not used for this: a region derived from a latitude would be this
    /// platform's geocoding rather than the publisher's statement, and a
    /// station just over a border would flip it.
    pub const REGION: &str = "US";

    /// The path segment that precedes the station identifier.
    const STATIONS_SEGMENT: &str = "/stations/";

    /// The path segment that must follow it.
    const OBSERVATIONS_SEGMENT: &str = "/observations";

    /// The attribution the NWS terms require of any copy of this content.
    ///
    /// Written once here, evaluated in `qip_data_finder::admission`'s
    /// catalogue, and carried into every record's provenance as the source
    /// identifier. The NWS terms permit use "without charge for any lawful
    /// purpose" provided a user does not claim the content as their own; this
    /// constant is what stops that condition being a licence term in a
    /// comment.
    pub const ATTRIBUTION: &str = "Observations from the United States National Weather Service (api.weather.gov), a work \
         of the United States Government in the public domain. This platform is not affiliated \
         with and is not endorsed by NOAA or the National Weather Service.";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        SourceManifest::from_json(MANIFEST)
    }

    /// Build the connector for the station its manifest asks for.
    ///
    /// The station is read out of the manifest's own path, the way
    /// [`crate::connectors::nyfed_effr`] reads its rate family, so that
    /// [`Self::decode`] has something to hold the response to. Without it the
    /// station identifier would come from the response body, and on a
    /// plaintext hop that means whoever answers chooses the `subject_id` every
    /// reading is filed under for ever.
    pub fn new(manifest: SourceManifest) -> Result<Self> {
        manifest.validate()?;
        if manifest.publication_delay().is_zero() {
            return Err(Error::invalid(format!(
                "`{}` declares no dissemination delay, so every observation would be knowable at \
                 the instant the instrument read it. A station's report reaches this API after \
                 the wire carries it, and a delay of zero is a backtest that reads the \
                 thermometer through the wall",
                manifest.source_id
            )));
        }
        let station = Self::station_from_path(&manifest.endpoint.path)?;
        Ok(Self { manifest, station })
    }

    /// The station identifier `/stations/KORD/observations` asks for.
    ///
    /// Refuses anything else, including the network-wide `/observations` path.
    /// That one is not a harmless alternative: it serves every station in the
    /// country, which is tens of thousands of readings a poll against a
    /// manifest declaring a batch ceiling of a hundred and twenty-eight, and
    /// the publisher's own guidance asks a client to "request only the data
    /// that you need".
    fn station_from_path(path: &str) -> Result<String> {
        let rest = path.strip_prefix(Self::STATIONS_SEGMENT).ok_or_else(|| {
            Error::invalid(format!(
                "the endpoint path {} does not begin `{}`, so it names no station. This connector \
                 publishes one station's observations and takes the station from its own path; \
                 point the manifest at `/stations/<ID>/observations`",
                bounded_excerpt(path),
                Self::STATIONS_SEGMENT
            ))
        })?;
        let station = rest
            .strip_suffix(Self::OBSERVATIONS_SEGMENT)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "the endpoint path {} does not end `{}`. The station metadata and forecast \
                     paths on this host answer a different shape entirely, and decoding one as a \
                     feature collection of observations would quarantine every poll",
                    bounded_excerpt(path),
                    Self::OBSERVATIONS_SEGMENT
                ))
            })?;
        if station.is_empty() || station.contains('/') {
            return Err(Error::invalid(format!(
                "the endpoint path {} names {} between `{}` and `{}`, which is not a single \
                 station identifier. The network-wide observations path serves every station in \
                 the country, and this connector files each reading under one `subject_id`",
                bounded_excerpt(path),
                bounded_excerpt(station),
                Self::STATIONS_SEGMENT,
                Self::OBSERVATIONS_SEGMENT
            )));
        }
        Ok(station.to_string())
    }

    /// The station this connector asked for.
    pub fn station(&self) -> &str {
        &self.station
    }

    /// The station an observation says it came from, checked against the one
    /// asked for.
    ///
    /// The payload states it as a URL — `https://api.weather.gov/stations/KORD`
    /// — and the last segment is the identifier. A mismatch refuses the page
    /// rather than skipping the observation: a feature collection from a
    /// different station is an answer to a question nobody asked, and
    /// publishing part of it would file readings under a `subject_id` that
    /// names the wrong place.
    fn check_station(&self, properties: &Value) -> Result<()> {
        let station = properties
            .get("station")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::schema(
                    "an observation names no `station`, so nothing says where it was taken and \
                     every reading in it would be filed under the station this connector happened \
                     to ask for",
                )
            })?;
        let named = station.rsplit('/').next().unwrap_or_default();
        if named != self.station {
            return Err(Error::schema(format!(
                "an observation says it came from {}, and this connector asked for {}. A \
                 collection about another station is an answer to a question nobody asked, and \
                 publishing it would file every reading in it under the wrong subject",
                bounded_excerpt(named),
                self.station
            )));
        }
        Ok(())
    }

    /// The instant an observation was taken, refusing a non-UTC offset.
    ///
    /// `qip_core::Timestamp::parse_rfc3339` splits the time at the first `+`
    /// or `-` and reads what precedes it as UTC, so a `-05:00` offset would
    /// parse to a wall-clock reading five hours early and nothing downstream
    /// could tell. An event time is a permanent bitemporal stamp and a
    /// backtest keyed on one that is five hours early is reading the future.
    /// This feed serves `+00:00`, so the check costs nothing today and is the
    /// only thing that would catch the day it stops.
    fn observed_at(stamp: &str) -> Result<Timestamp> {
        let trimmed = stamp.trim();
        let utc = trimmed.ends_with('Z')
            || trimmed.ends_with("+00:00")
            || trimmed.ends_with("-00:00")
            || trimmed.ends_with("+0000");
        if !utc {
            return Err(Error::schema(format!(
                "an observation is stamped {}, whose UTC offset is neither `Z` nor zero. This \
                 platform's timestamp parser discards the offset and reads the rest as UTC, so \
                 admitting this would file the reading at the wrong instant and no later check \
                 could find it. Ask the publisher for UTC, or teach the parser offsets before \
                 admitting this feed",
                bounded_excerpt(trimmed)
            )));
        }
        Timestamp::parse_rfc3339(trimmed).ok_or_else(|| {
            Error::schema(format!(
                "an observation is stamped {}, which is not an instant this platform can read",
                bounded_excerpt(trimmed)
            ))
        })
    }

    /// The grade on one reading, or a refusal naming the letter.
    fn grade_of(field: &str, measurement: &Value) -> Result<ObservationGrade> {
        let letter = measurement
            .get("qualityControl")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::schema(format!(
                    "the `{field}` reading carries no `qualityControl`. Every reading on this feed \
                     is graded by the publisher, and one without a grade would be published as a \
                     clean measurement on the strength of nothing"
                ))
            })?;
        ObservationGrade::from_letter(letter).ok_or_else(|| {
            Error::schema(format!(
                "the `{field}` reading is graded {}, which is not a grade this connector knows. \
                 The publisher's vocabulary has changed, and guessing a confidence for a letter \
                 nobody has read would file a number under a quality nobody established",
                bounded_excerpt(letter)
            ))
        })
    }

    /// The unit on one reading, held to the table's expectation.
    fn check_unit(entry: &Reading, measurement: &Value) -> Result<()> {
        let unit = measurement
            .get("unitCode")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::schema(format!(
                    "the `{}` reading carries no `unitCode`, so its number has no dimension and \
                     this connector would be asserting one",
                    entry.field
                ))
            })?;
        if unit != entry.unit_code {
            return Err(Error::schema(format!(
                "the `{}` reading arrived in {} and this connector expects `{}`. A unit change \
                 keeps the field name and the shape and silently redefines every number in the \
                 series, which nothing downstream can detect — so it refuses here rather than \
                 converting, because nobody knows whether the history should be converted too",
                entry.field,
                bounded_excerpt(unit),
                entry.unit_code
            )));
        }
        Ok(())
    }

    /// `KORD/air_temperature@2026-09-19T05:00:00Z` — the source's own key for
    /// one reading.
    ///
    /// Station, metric and instant together, because the source's key for an
    /// observation identifies the *observation* and this platform publishes
    /// one event per reading within it. A key without the metric would make
    /// ten readings one fingerprint and nine of them would be dropped as
    /// duplicates.
    pub fn event_key(station: &str, metric: &str, at: Timestamp) -> String {
        format!("{station}/{metric}@{}", at.to_rfc3339())
    }
}

impl SourceConnector for NwsStationObservationsConnector {
    fn manifest(&self) -> &SourceManifest {
        &self.manifest
    }

    /// One event per graded, present reading of every observation in the page.
    ///
    /// The body of each event carries that one reading only, and deliberately
    /// not the whole observation: the fingerprint is taken over the body, so a
    /// body holding every other field would change whenever any of them moved
    /// — and an unchanged temperature would fingerprint differently each time
    /// the wind did and be republished as new.
    fn decode(&self, payload: &Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
        let features = payload
            .get("features")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                Error::schema(
                    "the observation collection's `features` is not an array, so nothing in this \
                     response is an observation",
                )
            })?;
        let mut events = Vec::new();
        for feature in features {
            let properties = feature.get("properties").ok_or_else(|| {
                Error::schema(
                    "a feature in the observation collection carries no `properties`, so it holds \
                     no readings and no instant",
                )
            })?;
            self.check_station(properties)?;
            let stamp = properties
                .get("timestamp")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    Error::schema(
                        "an observation carries no `timestamp`, so nothing says when the \
                         instrument read and the reading cannot be stamped with an event time",
                    )
                })?;
            let observed_at = Self::observed_at(stamp)?;
            for entry in READINGS {
                let Some(measurement) = properties.get(entry.field) else {
                    // A station that does not report this reading omits the
                    // field. Ordinary, and not a finding.
                    continue;
                };
                // Checked before the value, so a unit change is caught on the
                // observations where the field is null too — which for a
                // rarely-reported reading may be all of them for days.
                Self::check_unit(entry, measurement)?;
                let grade = Self::grade_of(entry.field, measurement)?;
                let Some(value) = measurement.get("value").and_then(Value::as_f64) else {
                    // Either absent or `null`: the station reported no value
                    // for this reading. Skipped, never imputed — a carried
                    // forward temperature is a measurement this platform
                    // invented.
                    continue;
                };
                if !value.is_finite() {
                    return Err(Error::schema(format!(
                        "the `{}` reading is {value}, which is not a number an instrument can \
                         report. Refused rather than dropped, because a non-finite value in a \
                         numeric field means the encoder produced something this connector does \
                         not understand",
                        entry.field
                    )));
                }
                if grade.is_repudiated() {
                    // The publisher says its own sensor was wrong. That is the
                    // world rather than a protocol violation, so it costs this
                    // reading and not the page.
                    continue;
                }
                events.push(RawEvent::new(
                    Self::event_key(&self.station, entry.metric, observed_at),
                    observed_at,
                    serde_json::json!({
                        "station": self.station,
                        "metric": entry.metric,
                        "value": value,
                        "grade": grade.letter(),
                        "observed_at": observed_at.to_rfc3339(),
                    }),
                ));
            }
        }
        Ok(events)
    }

    /// The record, and the second place the metric and the grade are checked.
    ///
    /// Not belt and braces: `decode` is where a bad page is found, but `map`
    /// is where the permanent artefacts are minted — the dataset name, the
    /// metric name, the unit string a reader interprets the number by. A
    /// connector whose two halves could disagree about the metric is one where
    /// only one of them is the gate.
    fn map(&self, event: &RawEvent, ingest_time: Timestamp) -> Result<SensedRecord> {
        let metric = event
            .body
            .get("metric")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::schema("a decoded reading lost its metric between decode and map")
            })?;
        let entry = reading_for_metric(metric).ok_or_else(|| {
            Error::schema(format!(
                "a decoded reading names the metric {}, which is not one this connector publishes",
                bounded_excerpt(metric)
            ))
        })?;
        let station = event
            .body
            .get("station")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::schema("a decoded reading lost its station between decode and map")
            })?;
        if station != self.station {
            return Err(Error::schema(format!(
                "a decoded reading is for station {} and this connector publishes {}",
                bounded_excerpt(station),
                self.station
            )));
        }
        let value = event
            .body
            .get("value")
            .and_then(Value::as_f64)
            .ok_or_else(|| {
                Error::schema("a decoded reading lost its value between decode and map")
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
        Ok(SensedRecord::AlternativeData(Box::new(
            AlternativeDataPoint {
                dataset: Self::DATASET.to_string(),
                subject_id: station.to_string(),
                metric: entry.metric.to_string(),
                // The crossing point between money and statistics does not
                // arise here: a temperature is a measurement and never a
                // price, so `f64` is the right type all the way through and
                // no `Decimal` is involved.
                value,
                unit: entry.unit.to_string(),
                observed_at: event.event_time,
                // All three left at nothing measured. See the module doc: a
                // proxy correlation this platform invented would make
                // `is_actionable` answer `true` on the strength of a number
                // nobody computed.
                lead_days: 0.0,
                proxy_correlation: 0.0,
                proxies_for: None,
                provenance,
                quality: self.quality_of(event),
            },
        )))
    }

    /// The publisher's grade, carried rather than assumed.
    ///
    /// The default for this trait is a clean measurement, which for this feed
    /// would assert that a reading the NWS itself marked unchecked had passed
    /// every check.
    fn quality_of(&self, event: &RawEvent) -> DataQuality {
        let grade = event
            .body
            .get("grade")
            .and_then(Value::as_str)
            .and_then(ObservationGrade::from_letter);
        match grade {
            Some(grade) => grade.quality(),
            // A body whose grade did not survive `decode` is not a clean
            // measurement. It is a record this connector can no longer vouch
            // for, and it says so rather than defaulting to perfect.
            None => DataQuality::clean()
                .with_issue("the publisher's quality grade did not survive decoding"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> Result<SourceManifest> {
        NwsStationObservationsConnector::shipped_manifest()
    }

    #[test]
    fn the_shipped_manifest_names_one_station_and_the_connector_reads_it() -> Result<()> {
        let connector = NwsStationObservationsConnector::new(shipped()?)?;
        assert_eq!(connector.station(), "KORD");
        Ok(())
    }

    #[test]
    fn the_network_wide_observations_path_is_refused_rather_than_read_as_a_station() -> Result<()> {
        // `/observations` serves every station in the United States. Admitting
        // it would file tens of thousands of readings under one `subject_id`
        // taken from a path that names none.
        let mut manifest = shipped()?;
        manifest.endpoint.path = "/observations".to_string();
        manifest.endpoint.health_path = Some("/observations".to_string());
        let refused = NwsStationObservationsConnector::new(manifest)
            .expect_err("the network-wide path built a single-station connector");
        assert!(
            refused.message().contains("names no station"),
            "the refusal must say the path names no station, got {}",
            refused.message()
        );
        Ok(())
    }

    #[test]
    fn a_station_metadata_path_is_refused_because_it_answers_a_different_shape() -> Result<()> {
        let mut manifest = shipped()?;
        manifest.endpoint.path = "/stations/KORD".to_string();
        manifest.endpoint.health_path = Some("/stations/KORD".to_string());
        let refused = NwsStationObservationsConnector::new(manifest)
            .expect_err("a station metadata path built an observations connector");
        assert!(
            refused.message().contains("/observations"),
            "the refusal must name the segment that is missing, got {}",
            refused.message()
        );
        Ok(())
    }

    #[test]
    fn a_manifest_declaring_no_dissemination_delay_is_refused() -> Result<()> {
        let mut manifest = shipped()?;
        manifest.publication_delay_ms = 0;
        let refused = NwsStationObservationsConnector::new(manifest)
            .expect_err("a zero dissemination delay built a connector");
        assert!(
            refused.message().contains("dissemination delay"),
            "the refusal must name the delay, got {}",
            refused.message()
        );
        Ok(())
    }

    #[test]
    fn every_grade_letter_round_trips_and_only_the_repudiated_ones_are_skipped() {
        // The enumeration is closed on purpose: an unknown letter must not be
        // able to become a new arm at runtime.
        for grade in [
            ObservationGrade::Validated,
            ObservationGrade::SubjectiveGood,
            ObservationGrade::CoarsePass,
            ObservationGrade::Preliminary,
            ObservationGrade::Failed,
            ObservationGrade::Questionable,
            ObservationGrade::SubjectiveBad,
            ObservationGrade::Trace,
        ] {
            assert_eq!(
                ObservationGrade::from_letter(grade.letter()),
                Some(grade),
                "{} did not round-trip through its own letter",
                grade.letter()
            );
        }
        assert_eq!(ObservationGrade::from_letter("W"), None);
        // A trace is a statement about the world and is published; a
        // repudiated reading is the publisher saying its sensor was wrong.
        assert!(!ObservationGrade::Trace.is_repudiated());
        assert!(ObservationGrade::Failed.is_repudiated());
        assert!(ObservationGrade::Questionable.is_repudiated());
        assert!(ObservationGrade::SubjectiveBad.is_repudiated());
        assert!(!ObservationGrade::Validated.is_repudiated());
        assert!(!ObservationGrade::Preliminary.is_repudiated());
    }

    #[test]
    fn a_preliminary_reading_cannot_drive_a_decision_and_a_validated_one_can() {
        // The whole reason the grade is carried. `DataQuality::default` is a
        // perfect measurement and clears the floor, so a connector that
        // dropped the grade would publish an unchecked value as decision
        // grade.
        let validated = ObservationGrade::Validated.quality();
        assert!(
            validated.meets(qip_financial::quality::DECISION_QUALITY_FLOOR),
            "a validated reading must clear the decision floor, scored {}",
            validated.score()
        );
        let preliminary = ObservationGrade::Preliminary.quality();
        assert!(
            !preliminary.meets(qip_financial::quality::DECISION_QUALITY_FLOOR),
            "a reading the publisher never checked must not clear the decision floor, scored {}",
            preliminary.score()
        );
        // And the ordering between the two admitted-but-imperfect grades is
        // the publisher's own: screened against a coarse band beats not
        // screened at all.
        assert!(
            ObservationGrade::CoarsePass.quality().score()
                > ObservationGrade::Preliminary.quality().score(),
            "a coarse-pass reading must outrank one with no quality control"
        );
    }

    #[test]
    fn a_timestamp_with_a_non_utc_offset_is_refused_rather_than_read_as_utc() -> Result<()> {
        // The parser discards the offset, so `-05:00` would be filed five
        // hours early and no later check could find it. This is the
        // point-in-time leakage the domain rules put first.
        let refused = NwsStationObservationsConnector::observed_at("2026-09-19T05:00:00-05:00")
            .expect_err("a non-UTC offset was read as UTC");
        assert!(
            refused.message().contains("offset"),
            "the refusal must name the offset, got {}",
            refused.message()
        );
        // The premise: the same instant in UTC is admitted, so the check is a
        // gate and not a blanket refusal.
        let admitted = NwsStationObservationsConnector::observed_at("2026-09-19T05:00:00+00:00")?;
        assert_eq!(
            admitted,
            Timestamp::parse_rfc3339("2026-09-19T05:00:00Z").unwrap_or_default()
        );
        Ok(())
    }
}
