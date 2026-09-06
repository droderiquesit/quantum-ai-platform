//! The feature-name contract between the absorb arms and the analysts.
//!
//! Every feature an analyst reads and every feature an absorb arm writes is
//! spelled here, once, and nowhere else. The failure this prevents is not
//! hypothetical: the macro analyst read `policy_rate` at `"global"` while the
//! macro arm wrote `macro_level` at the vendor's series id, and the alt-data
//! analyst read `web_traffic_index` while the kernel wrote
//! `alt/web-traffic/web_traffic_index`. Both halves were correct on their own
//! terms and the two never met, so a panel convened on any data reported no
//! data for six analysts and no hypothesis could gather the origins it needed.
//! A name that can only be spelled in one place cannot drift from itself.
//!
//! Three things live here:
//!
//! * [`names`] — the string constants. A reader or a writer that wants a
//!   feature name imports it; a literal in either crate is a defect.
//! * [`MacroSeries`] and [`AltMetric`] — the mappings from what a record
//!   carries (a vendor series id and region; a dataset and metric) to the
//!   name and key the analyst reads. The mapping is a recognition, not a
//!   parse: a series or metric this vocabulary does not name is left as the
//!   raw record it arrived as, never guessed into a vocabulary name.
//! * [`UNWRITTEN`] — the reads no absorb arm can satisfy because no record
//!   kind the platform accepts carries the fact, each with the record kind
//!   that would. Declared rather than discovered, so the acceptance test that
//!   walks every analyst read can refuse a new orphan while admitting the
//!   ones the platform has said it cannot yet feed — and can refuse the
//!   declaration too, the day an arm starts writing one of them.
//!
//! # Keys
//!
//! A feature is keyed by exactly one [`SubjectKind`]. A policy rate is a fact
//! about an economy, not about the world: `"global"` was never a key anything
//! wrote, and had a writer used it two economies' rates would have landed in
//! one series. The macro arm keys by the observation's region, which is the
//! same ISO code the instrument catalogue calls `geography`, and the macro
//! analyst reads by the subject instrument's geography. That is a
//! simplification for a multinational and it is stated as one; it is not a
//! guess, because an instrument with no geography gets no macro view rather
//! than the wrong economy's.

use qip_core::error::{Error, Result};

/// What a feature series is keyed by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SubjectKind {
    /// An ISO country or region code — the catalogue's `geography`, the
    /// macro observation's `region`.
    Economy,
    /// A financial object id.
    Instrument,
    /// A resolved entity id.
    Entity,
}

impl SubjectKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Economy => "economy",
            Self::Instrument => "instrument",
            Self::Entity => "entity",
        }
    }
}

/// One feature as an analyst reads it: the name and what it is keyed by.
///
/// Both halves matter. `credit_spread_bps` keyed by economy is an aggregate
/// the macro arm writes; keyed by instrument it is an issuer spread nothing
/// writes. A contract that compared names alone would call the second one
/// satisfied by the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureRead {
    pub name: &'static str,
    pub keyed_by: SubjectKind,
}

impl FeatureRead {
    pub const fn new(name: &'static str, keyed_by: SubjectKind) -> Self {
        Self { name, keyed_by }
    }
}

/// The names. Each is spelled here and imported everywhere else.
pub mod names {
    // Written by the market arms (bars, trades, ticks).
    pub const CLOSE: &str = "close";
    pub const VOLUME: &str = "volume";
    /// Recomputed from `close` by the world model, not absorbed.
    pub const REALISED_VOLATILITY_20D: &str = "realised_volatility_20d";
    // Written by the news arm, keyed by entity.
    pub const SENTIMENT: &str = "sentiment";
    // Written by the fundamentals arm under the update's own metric name;
    // `revenue` is the one the standard definitions declare.
    pub const REVENUE: &str = "revenue";
    pub const REVENUE_SURPRISE: &str = "revenue_surprise";
    // Written by the macro arm for every release, keyed by the vendor's
    // series id — the raw record, kept whatever the vocabulary recognises.
    pub const MACRO_LEVEL: &str = "macro_level";
    pub const MACRO_SURPRISE: &str = "macro_surprise";
    // Written by the macro arm for a recognised series, keyed by economy;
    // read by the macro analyst.
    pub const POLICY_RATE: &str = "policy_rate";
    pub const INFLATION_YOY: &str = "inflation_yoy";
    pub const GROWTH_YOY: &str = "growth_yoy";
    pub const CREDIT_SPREAD_BPS: &str = "credit_spread_bps";
    // Read by the credit analyst keyed by instrument. Unwritten.
    pub const EFFECTIVE_DURATION: &str = "effective_duration";
    // Read by the derivatives analyst keyed by instrument. Unwritten.
    pub const IMPLIED_VOLATILITY: &str = "implied_volatility";
    // Written by the alternative-data arm for a recognised metric, keyed by
    // the reading's subject; read by the alternative-data analyst.
    pub const WEB_TRAFFIC_INDEX: &str = "web_traffic_index";
    pub const CARD_SPEND_INDEX: &str = "card_spend_index";
    pub const JOB_POSTINGS_INDEX: &str = "job_postings_index";
    // Read by the commodities analyst keyed by instrument. Unwritten.
    pub const FRONT_MONTH_PRICE: &str = "front_month_price";
    pub const DEFERRED_MONTH_PRICE: &str = "deferred_month_price";
    pub const DEFERRED_TENOR_MONTHS: &str = "deferred_tenor_months";
    // Read by the FX and rates analyst keyed by instrument. Unwritten.
    pub const BASE_RATE: &str = "base_rate";
    pub const QUOTE_RATE: &str = "quote_rate";
    pub const REALISED_VOLATILITY: &str = "realised_volatility";
}

/// A macro series the vocabulary names, and how a release is recognised as
/// one of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MacroSeries {
    PolicyRate,
    InflationYoy,
    GrowthYoy,
    CreditSpreadBps,
}

impl MacroSeries {
    pub const ALL: [Self; 4] = [
        Self::PolicyRate,
        Self::InflationYoy,
        Self::GrowthYoy,
        Self::CreditSpreadBps,
    ];

    /// The feature the analyst reads.
    pub const fn feature(self) -> &'static str {
        match self {
            Self::PolicyRate => names::POLICY_RATE,
            Self::InflationYoy => names::INFLATION_YOY,
            Self::GrowthYoy => names::GROWTH_YOY,
            Self::CreditSpreadBps => names::CREDIT_SPREAD_BPS,
        }
    }

    /// The series code a release must carry after its region prefix.
    ///
    /// The feature name in the vendor's upper-case spelling — a test holds
    /// the two to that relation so neither can drift from the other.
    pub const fn series_code(self) -> &'static str {
        match self {
            Self::PolicyRate => "POLICY_RATE",
            Self::InflationYoy => "INFLATION_YOY",
            Self::GrowthYoy => "GROWTH_YOY",
            Self::CreditSpreadBps => "CREDIT_SPREAD_BPS",
        }
    }

    pub const fn read(self) -> FeatureRead {
        FeatureRead::new(self.feature(), SubjectKind::Economy)
    }

    /// The series id a release for this series carries in `region`.
    pub fn series_id(self, region: &str) -> String {
        format!("{region}.{}", self.series_code())
    }

    /// Recognise a release, or not.
    ///
    /// The series id must be exactly `{region}.{code}`: delimited on both
    /// sides, so `US.POLICY_RATE_FORECAST` is not the policy rate and a
    /// release stamped region `US` under `EA.POLICY_RATE` is not recognised
    /// either — the two claims about which economy it describes disagree, and
    /// neither is trusted. An unrecognised release is not an error; it is
    /// recorded under [`names::MACRO_LEVEL`] as every release is.
    pub fn recognise(series_id: &str, region: &str) -> Option<Self> {
        // An empty region would let `.POLICY_RATE` pass as the policy rate of
        // no economy and be recorded under an empty subject, which the
        // analyst could then read as a real economy's series. Refused, not
        // defaulted.
        if region.is_empty() {
            return None;
        }
        let code = series_id.strip_prefix(region)?.strip_prefix('.')?;
        Self::ALL
            .into_iter()
            .find(|series| series.series_code() == code)
    }
}

/// An alternative-data metric the vocabulary names, with the dataset it
/// belongs to.
///
/// The dataset is part of the identity because the licence is per dataset:
/// the analyst refuses a dataset it is not licensed for, and a reading from
/// some other dataset landing under a licensed name would be read as licensed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AltMetric {
    WebTrafficIndex,
    CardSpendIndex,
    JobPostingsIndex,
}

impl AltMetric {
    pub const ALL: [Self; 3] = [
        Self::WebTrafficIndex,
        Self::CardSpendIndex,
        Self::JobPostingsIndex,
    ];

    /// The feature the analyst reads, which is also the metric name a
    /// reading must carry.
    pub const fn feature(self) -> &'static str {
        match self {
            Self::WebTrafficIndex => names::WEB_TRAFFIC_INDEX,
            Self::CardSpendIndex => names::CARD_SPEND_INDEX,
            Self::JobPostingsIndex => names::JOB_POSTINGS_INDEX,
        }
    }

    /// The dataset the metric belongs to — the name the licence is held under.
    pub const fn dataset(self) -> &'static str {
        match self {
            Self::WebTrafficIndex => "web-traffic",
            Self::CardSpendIndex => "card-spend",
            Self::JobPostingsIndex => "job-postings",
        }
    }

    pub const fn read(self) -> FeatureRead {
        FeatureRead::new(self.feature(), SubjectKind::Instrument)
    }

    /// Recognise a reading.
    ///
    /// `Ok(Some)` when the metric is in the vocabulary and arrived from its
    /// own dataset. `Ok(None)` for a metric the vocabulary does not name,
    /// which is left as the raw record it is. `Err` for a vocabulary metric
    /// from a different dataset: that reading is refused rather than stored
    /// under the licensed name, because the analyst's licence check is by
    /// dataset and would admit it.
    pub fn recognise(dataset: &str, metric: &str) -> Result<Option<Self>> {
        let Some(found) = Self::ALL.into_iter().find(|m| m.feature() == metric) else {
            return Ok(None);
        };
        if found.dataset() != dataset {
            return Err(Error::invalid(format!(
                "alternative-data metric `{metric}` arrived from dataset `{dataset}`, but the \
                 vocabulary holds it under `{}`; refused rather than recorded under a name the \
                 analyst's licence for `{}` would admit. Publish it under its own metric name, or \
                 add the dataset to the vocabulary and the licensing register together",
                found.dataset(),
                found.dataset()
            )));
        }
        Ok(Some(found))
    }
}

/// A read no absorb arm can satisfy, and the record kind that would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unwritten {
    pub read: FeatureRead,
    /// What would have to be accepted by `SensedRecord` for an arm to write
    /// this. Quoted by the analyst's no-data finding, so the panel says what
    /// would change its answer.
    pub needs: &'static str,
}

/// What an issuer credit view needs and the platform does not accept.
pub const CREDIT_QUOTE_NEEDED: &str = "an issuer credit quote record carrying the spread over its \
                                       benchmark and the effective duration; no record kind the \
                                       platform accepts carries either";

/// What a volatility view needs and the platform does not accept.
pub const OPTION_QUOTE_NEEDED: &str = "an option quote record carrying an implied volatility; no \
                                       record kind the platform accepts carries one";

/// What a curve view needs and the platform does not accept.
pub const FUTURES_CURVE_NEEDED: &str = "a futures curve record carrying the front and deferred \
                                        settlements and the tenor between them; no record kind \
                                        the platform accepts carries one";

/// What a carry view needs and the platform does not accept.
pub const RATE_LEGS_NEEDED: &str = "a rate-leg record for the pair's two currencies and its \
                                    realised volatility; no record kind the platform accepts \
                                    carries one";

/// What a causal trace needs and the platform does not accept. Not a feature
/// read — the causal analyst reads the causal graph — so it stands beside the
/// table rather than in it.
pub const CAUSAL_CLAIM_NEEDED: &str = "a causal-claim record (cause, effect, mechanism, lag, \
                                       evidence); no record kind the platform accepts carries \
                                       one, and the only edges the graph ever holds are the \
                                       demonstration seed's";

/// Every analyst read no absorb arm writes.
///
/// Adding a line here is an admission, and removing one requires an arm that
/// writes the read — the acceptance test refuses both a read that is neither
/// written nor declared and a declaration an arm has since made false.
pub const UNWRITTEN: [Unwritten; 9] = [
    Unwritten {
        read: FeatureRead::new(names::CREDIT_SPREAD_BPS, SubjectKind::Instrument),
        needs: CREDIT_QUOTE_NEEDED,
    },
    Unwritten {
        read: FeatureRead::new(names::EFFECTIVE_DURATION, SubjectKind::Instrument),
        needs: CREDIT_QUOTE_NEEDED,
    },
    Unwritten {
        read: FeatureRead::new(names::IMPLIED_VOLATILITY, SubjectKind::Instrument),
        needs: OPTION_QUOTE_NEEDED,
    },
    Unwritten {
        read: FeatureRead::new(names::FRONT_MONTH_PRICE, SubjectKind::Instrument),
        needs: FUTURES_CURVE_NEEDED,
    },
    Unwritten {
        read: FeatureRead::new(names::DEFERRED_MONTH_PRICE, SubjectKind::Instrument),
        needs: FUTURES_CURVE_NEEDED,
    },
    Unwritten {
        read: FeatureRead::new(names::DEFERRED_TENOR_MONTHS, SubjectKind::Instrument),
        needs: FUTURES_CURVE_NEEDED,
    },
    Unwritten {
        read: FeatureRead::new(names::BASE_RATE, SubjectKind::Instrument),
        needs: RATE_LEGS_NEEDED,
    },
    Unwritten {
        read: FeatureRead::new(names::QUOTE_RATE, SubjectKind::Instrument),
        needs: RATE_LEGS_NEEDED,
    },
    Unwritten {
        read: FeatureRead::new(names::REALISED_VOLATILITY, SubjectKind::Instrument),
        needs: RATE_LEGS_NEEDED,
    },
];

/// The declaration for a read, if the platform has admitted it cannot feed it.
pub fn unwritten(read: FeatureRead) -> Option<&'static Unwritten> {
    UNWRITTEN.iter().find(|entry| entry.read == read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_series_code_is_the_feature_name_in_the_vendors_spelling_and_nothing_else() {
        // The two spellings are one word. If someone renames a feature and
        // not its code, the release is recorded under the old name's series
        // and the analyst reads the new one: the exact drift this module
        // exists to make impossible.
        for series in MacroSeries::ALL {
            assert_eq!(
                series.series_code().to_ascii_lowercase(),
                series.feature(),
                "{series:?}"
            );
        }
    }

    #[test]
    fn a_release_is_recognised_only_under_its_own_region_and_exact_code() {
        assert_eq!(
            MacroSeries::recognise("US.POLICY_RATE", "US"),
            Some(MacroSeries::PolicyRate)
        );
        assert_eq!(
            MacroSeries::recognise(&MacroSeries::GrowthYoy.series_id("EA"), "EA"),
            Some(MacroSeries::GrowthYoy)
        );
        // Delimited on both sides: a longer code is a different series.
        assert_eq!(
            MacroSeries::recognise("US.POLICY_RATE_FORECAST", "US"),
            None
        );
        // Region and series id disagree about the economy: neither wins.
        assert_eq!(MacroSeries::recognise("EA.POLICY_RATE", "US"), None);
        // The raw vendor spelling the platform already carries is left raw.
        assert_eq!(MacroSeries::recognise("US.CPI.YOY", "US"), None);
        // An empty region is not an economy: `.POLICY_RATE` under region ""
        // would otherwise be recognised and keyed on an empty subject.
        assert_eq!(MacroSeries::recognise(".POLICY_RATE", ""), None);
    }

    #[test]
    fn a_vocabulary_metric_from_the_wrong_dataset_is_refused_not_laundered() {
        assert_eq!(
            AltMetric::recognise("web-traffic", "web_traffic_index").expect("recognised"),
            Some(AltMetric::WebTrafficIndex)
        );
        assert_eq!(
            AltMetric::recognise("satellite.facility_activity", "throughput_index")
                .expect("an unknown metric is left raw"),
            None
        );
        let refused = AltMetric::recognise("scraped-web", "web_traffic_index")
            .expect_err("a licensed name was filled from an unlicensed dataset");
        assert!(
            refused.message().contains("web-traffic") && refused.message().contains("scraped-web"),
            "{}",
            refused.message()
        );
    }

    #[test]
    fn the_unwritten_table_names_a_record_kind_for_every_entry_and_no_written_name() {
        for entry in UNWRITTEN {
            assert!(!entry.needs.trim().is_empty(), "{:?}", entry.read);
        }
        for series in MacroSeries::ALL {
            assert!(unwritten(series.read()).is_none(), "{series:?} is written");
        }
        for metric in AltMetric::ALL {
            assert!(unwritten(metric.read()).is_none(), "{metric:?} is written");
        }
        // The one name that is both: an aggregate spread keyed by economy is
        // written; an issuer spread keyed by instrument is not.
        assert!(
            unwritten(FeatureRead::new(
                names::CREDIT_SPREAD_BPS,
                SubjectKind::Instrument
            ))
            .is_some()
        );
    }
}
