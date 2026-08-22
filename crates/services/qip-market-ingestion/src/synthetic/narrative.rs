//! Synthetic textual and fundamental intelligence.
//!
//! The point of generating news alongside prices is that the two are *causally
//! linked*: an earnings surprise produces a fundamental record, a story that
//! names the company, and a price move — in that order, with the story
//! arriving before the market has fully absorbed it. Without that linkage the
//! reasoning and world-model stages would have nothing real to connect, and any
//! causal chain they produced would be decoration.
//!
//! Entity mentions deliberately include ambiguous surface forms (a bare
//! surname, a former ticker, a shortened trading name) so identity resolution
//! has genuine work to do.

use qip_core::rng::Rng;
use qip_core::{Decimal, Timestamp};
use qip_financial::intelligence::{
    AlternativeDataPoint, EntityMention, FiscalPeriod, FundamentalUpdate, MacroObservation,
    NewsItem, NewsSource, Sentiment,
};
use qip_financial::quality::{DataQuality, Provenance};

/// A company the synthetic world knows about.
#[derive(Clone, Debug)]
pub struct SyntheticCompany {
    pub entity_id: String,
    pub legal_name: String,
    /// Names the company is referred to by in text, in decreasing formality.
    pub aliases: Vec<String>,
    pub ticker: String,
    pub sector: String,
    pub country: String,
    /// Suppliers and customers, which is what makes second-order effects real.
    pub suppliers: Vec<String>,
    pub customers: Vec<String>,
    pub competitors: Vec<String>,
    /// Quarterly revenue in millions, used as the base for generated figures.
    pub base_revenue: f64,
    pub base_margin: f64,
}

/// Build the entity mentions for a story about `company`.
///
/// Roughly a third of stories use only an informal alias, which is exactly the
/// case identity resolution must handle.
pub fn mentions_for<R: Rng>(company: &SyntheticCompany, rng: &mut R) -> Vec<EntityMention> {
    let use_alias = rng.bernoulli(0.35) && !company.aliases.is_empty();
    let text = if use_alias {
        company.aliases[rng.below(company.aliases.len() as u64) as usize].clone()
    } else {
        company.legal_name.clone()
    };
    vec![EntityMention {
        text,
        entity_id: None,
        // An informal alias is genuinely less certain, and the confidence
        // recorded here should say so rather than flatter the extractor.
        confidence: if use_alias { 0.62 } else { 0.94 },
        is_primary: true,
        sentiment: None,
    }]
}

/// A routine story with no particular information content.
///
/// Included on purpose: a discovery engine that fires on every headline is
/// useless, so the stream has to contain mostly noise.
pub fn routine_story<R: Rng>(company: &SyntheticCompany, at: Timestamp, rng: &mut R) -> NewsItem {
    const TEMPLATES: [(&str, &str); 5] = [
        (
            "{name} to present at industry conference",
            "The company confirmed it will present at an upcoming sector conference.",
        ),
        (
            "{name} announces leadership appointment in {sector}",
            "The appointment takes effect next quarter and does not change reported guidance.",
        ),
        (
            "Analysts maintain rating on {name}",
            "Sell-side coverage was reiterated with no change to estimates.",
        ),
        (
            "{name} extends existing supplier agreement",
            "The renewal is on substantially similar commercial terms.",
        ),
        (
            "{name} publishes annual sustainability report",
            "The report restates previously disclosed operational metrics.",
        ),
    ];
    let (headline, body) = TEMPLATES[rng.below(TEMPLATES.len() as u64) as usize];
    NewsItem {
        item_id: format!("routine-{}-{}", company.ticker, at.as_nanos()),
        headline: headline
            .replace("{name}", &company.legal_name)
            .replace("{sector}", &company.sector),
        body: body.to_string(),
        source: NewsSource::Newswire,
        published_at: at,
        entities: mentions_for(company, rng),
        sentiment: Sentiment {
            polarity: rng.uniform(-0.15, 0.15),
            confidence: 0.4,
            novelty: rng.uniform(0.0, 0.2),
        },
        topics: vec!["corporate".into()],
        provenance: Provenance::synthetic("synthetic-news", at),
        quality: DataQuality::clean(),
    }
}

/// A story reporting an earnings result, with tone matching the surprise.
pub fn earnings_story<R: Rng>(
    company: &SyntheticCompany,
    surprise: f64,
    at: Timestamp,
    rng: &mut R,
) -> NewsItem {
    let beat = surprise > 0.0;
    let magnitude = surprise.abs();
    let descriptor = match magnitude {
        m if m > 0.15 => "sharply",
        m if m > 0.06 => "materially",
        _ => "modestly",
    };
    let direction = if beat { "above" } else { "below" };

    NewsItem {
        item_id: format!("earnings-{}-{}", company.ticker, at.as_nanos()),
        headline: format!(
            "{} reports quarterly results {descriptor} {direction} consensus",
            company.legal_name
        ),
        body: format!(
            "{} reported quarterly revenue {descriptor} {direction} analyst estimates, a surprise \
             of {:.1}%. Management attributed the result to demand conditions in {} and did not \
             revise full-year guidance.",
            company.legal_name,
            surprise * 100.0,
            company.sector
        ),
        source: NewsSource::CompanyAnnouncement,
        published_at: at,
        entities: mentions_for(company, rng),
        sentiment: Sentiment {
            polarity: (surprise * 4.0).clamp(-1.0, 1.0),
            confidence: 0.85,
            novelty: magnitude.clamp(0.0, 1.0).max(0.4),
        },
        topics: vec!["earnings".into(), "guidance".into()],
        provenance: Provenance::synthetic("synthetic-news", at),
        quality: DataQuality::clean(),
    }
}

/// A story about a supply-chain disruption, which is what drives second-order
/// effects through the world model's supplier and customer edges.
pub fn supply_chain_story<R: Rng>(
    company: &SyntheticCompany,
    severity: f64,
    at: Timestamp,
    rng: &mut R,
) -> NewsItem {
    let mut entities = mentions_for(company, rng);
    // Name a customer too, so the graph has a second edge to traverse.
    if let Some(customer) = company.customers.first() {
        entities.push(EntityMention {
            text: customer.clone(),
            entity_id: None,
            confidence: 0.8,
            is_primary: false,
            sentiment: None,
        });
    }

    NewsItem {
        item_id: format!("supply-{}-{}", company.ticker, at.as_nanos()),
        headline: format!(
            "{} warns of production disruption at key facility",
            company.legal_name
        ),
        body: format!(
            "{} disclosed a disruption expected to affect output for several weeks. The company \
             indicated that downstream customers, including {}, have been notified. Analysts \
             expect a {:.0}% impact to quarterly volumes.",
            company.legal_name,
            company
                .customers
                .first()
                .map(String::as_str)
                .unwrap_or("major buyers"),
            severity * 100.0
        ),
        source: NewsSource::RegulatoryFiling,
        published_at: at,
        entities,
        sentiment: Sentiment {
            polarity: -(severity * 3.0).clamp(0.0, 1.0),
            confidence: 0.9,
            novelty: 0.85,
        },
        topics: vec!["supply_chain".into(), "operations".into()],
        provenance: Provenance::synthetic("synthetic-news", at),
        quality: DataQuality::clean(),
    }
}

/// A central bank or statistical release.
pub fn macro_story(series_id: &str, region: &str, surprise: f64, at: Timestamp) -> NewsItem {
    let hotter = surprise > 0.0;
    NewsItem {
        item_id: format!("macro-{series_id}-{}", at.as_nanos()),
        headline: format!(
            "{region} {series_id} comes in {} than expected",
            if hotter { "hotter" } else { "softer" }
        ),
        body: format!(
            "The latest {series_id} print for {region} surprised consensus by {surprise:.2}. \
             Rate expectations repriced following the release."
        ),
        source: NewsSource::OfficialStatistics,
        published_at: at,
        entities: vec![EntityMention {
            text: region.to_string(),
            entity_id: None,
            confidence: 0.95,
            is_primary: true,
            sentiment: None,
        }],
        sentiment: Sentiment {
            polarity: -(surprise.clamp(-1.0, 1.0)),
            confidence: 0.9,
            novelty: surprise.abs().clamp(0.3, 1.0),
        },
        topics: vec!["macro".into(), "monetary_policy".into()],
        provenance: Provenance::synthetic("synthetic-macro-news", at),
        quality: DataQuality::clean(),
    }
}

/// A reported fundamental for one company and metric.
pub fn fundamental<R: Rng>(
    company: &SyntheticCompany,
    metric: &str,
    surprise: f64,
    period_end: Timestamp,
    rng: &mut R,
) -> FundamentalUpdate {
    let base = match metric {
        "revenue" => company.base_revenue,
        "operating_margin" => company.base_margin,
        "eps_diluted" => company.base_revenue * company.base_margin / 1_000.0,
        "free_cash_flow" => company.base_revenue * company.base_margin * 0.7,
        _ => company.base_revenue,
    };
    let consensus = base * (1.0 + rng.uniform(-0.02, 0.02));
    let value = consensus * (1.0 + surprise);
    let prior = base * (1.0 + rng.uniform(-0.08, 0.12));

    FundamentalUpdate {
        entity_id: company.entity_id.clone(),
        metric: metric.to_string(),
        value: Decimal::from_f64(value).unwrap_or(Decimal::ZERO),
        unit: if metric == "operating_margin" {
            "ratio".into()
        } else {
            "USD_millions".into()
        },
        period_end,
        period: FiscalPeriod::Quarter,
        consensus: Decimal::from_f64(consensus),
        prior_value: Decimal::from_f64(prior),
        is_restatement: false,
        provenance: Provenance::synthetic("synthetic-fundamentals", period_end),
        quality: DataQuality::clean(),
    }
}

/// A macroeconomic observation.
pub fn macro_observation<R: Rng>(
    series_id: &str,
    region: &str,
    level: f64,
    surprise: f64,
    reference_date: Timestamp,
    rng: &mut R,
) -> MacroObservation {
    let consensus = level - surprise;
    MacroObservation {
        series_id: series_id.to_string(),
        region: region.to_string(),
        value: level,
        unit: "percent".into(),
        reference_date,
        consensus: Some(consensus),
        previous: Some(consensus + rng.uniform(-0.15, 0.15)),
        is_revision: false,
        provenance: Provenance::synthetic("synthetic-macro", reference_date),
        quality: DataQuality::clean(),
    }
}

/// An alternative-data reading that leads a reported fundamental.
///
/// The `lead_days` and `proxy_correlation` fields carry the reason the dataset
/// is worth paying for; the discovery stage weights it by exactly those.
pub fn alternative_reading<R: Rng>(
    company: &SyntheticCompany,
    underlying_growth: f64,
    observed_at: Timestamp,
    rng: &mut R,
) -> AlternativeDataPoint {
    // The proxy tracks the fundamental imperfectly, with noise proportional to
    // one minus the correlation.
    let correlation = 0.62;
    let noise = rng.normal() * 0.04 * (1.0 - correlation);
    AlternativeDataPoint {
        dataset: "satellite.facility_activity".into(),
        subject_id: company.entity_id.clone(),
        metric: "throughput_index".into(),
        value: 100.0 * (1.0 + underlying_growth * correlation + noise),
        unit: "index".into(),
        observed_at,
        lead_days: 21.0,
        proxy_correlation: correlation,
        proxies_for: Some("revenue".into()),
        provenance: Provenance::synthetic("synthetic-altdata", observed_at),
        quality: DataQuality::clean(),
    }
}

/// The companies the demo world contains.
///
/// A small, connected universe: a chip maker supplying a device maker, an
/// energy producer, a bank and a retailer. The supplier and customer edges are
/// what let a shock in one propagate to another.
pub fn demo_companies() -> Vec<SyntheticCompany> {
    vec![
        SyntheticCompany {
            entity_id: "ent-northwind".into(),
            legal_name: "Northwind Semiconductor Corporation".into(),
            aliases: vec!["Northwind".into(), "Northwind Semi".into(), "NWS".into()],
            ticker: "NWSC".into(),
            sector: "information_technology".into(),
            country: "US".into(),
            suppliers: vec!["ent-kestrel".into()],
            customers: vec!["Vantage Devices".into()],
            competitors: vec!["ent-orion".into()],
            base_revenue: 4_200.0,
            base_margin: 0.31,
        },
        SyntheticCompany {
            entity_id: "ent-vantage".into(),
            legal_name: "Vantage Devices Incorporated".into(),
            aliases: vec!["Vantage".into(), "Vantage Devices".into()],
            ticker: "VNTG".into(),
            sector: "information_technology".into(),
            country: "US".into(),
            suppliers: vec!["ent-northwind".into()],
            customers: Vec::new(),
            competitors: Vec::new(),
            base_revenue: 18_500.0,
            base_margin: 0.24,
        },
        SyntheticCompany {
            entity_id: "ent-kestrel".into(),
            legal_name: "Kestrel Materials PLC".into(),
            aliases: vec!["Kestrel".into()],
            ticker: "KSTL".into(),
            sector: "materials".into(),
            country: "GB".into(),
            suppliers: Vec::new(),
            customers: vec!["Northwind Semiconductor Corporation".into()],
            competitors: Vec::new(),
            base_revenue: 2_100.0,
            base_margin: 0.18,
        },
        SyntheticCompany {
            entity_id: "ent-meridian".into(),
            legal_name: "Meridian Energy Holdings".into(),
            aliases: vec!["Meridian".into(), "Meridian Energy".into()],
            ticker: "MRDN".into(),
            sector: "energy".into(),
            country: "US".into(),
            suppliers: Vec::new(),
            customers: Vec::new(),
            competitors: Vec::new(),
            base_revenue: 9_800.0,
            base_margin: 0.22,
        },
        SyntheticCompany {
            entity_id: "ent-atlas".into(),
            legal_name: "Atlas Federal Bancorp".into(),
            aliases: vec!["Atlas Federal".into(), "Atlas".into()],
            ticker: "ATFB".into(),
            sector: "financials".into(),
            country: "US".into(),
            suppliers: Vec::new(),
            customers: Vec::new(),
            competitors: Vec::new(),
            base_revenue: 6_400.0,
            base_margin: 0.34,
        },
    ]
}
