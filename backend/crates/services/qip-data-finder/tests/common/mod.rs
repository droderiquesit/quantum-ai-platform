//! Fixtures shared by the lifecycle tests.
//!
//! Built from constructors rather than from serialised files so that a change
//! to an invariant fails here, at the fixture, rather than producing a value
//! no constructor would accept.
//!
//! Cargo compiles this module separately into every test binary, so a helper
//! used by one test is dead code in the others. The allow is on the module for
//! that reason and no other.

#![allow(dead_code)]

use qip_contracts::governance::Usage;
use qip_core::error::Result;
use qip_core::{Currency, Decimal, Duration, Timestamp};
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
use qip_data_finder::legal::{LicensingPosture, SourceLicense};
use qip_data_finder::probe::{HeadResponse, InMemoryProbe, PayloadSample, RobotsFetch};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_events::Topic;
use qip_financial::asset_class::AssetClass;

pub(crate) const AGENT: &str = "qip-data-finder/1.0";

pub(crate) fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

pub(crate) fn rest() -> AccessMechanism {
    AccessMechanism::Rest {
        auth: AuthRequirement::None,
        incremental_parameter: Some("since".to_string()),
        page_size: 500,
    }
}

pub(crate) fn endpoint(url: &str) -> Result<SourceEndpoint> {
    SourceEndpoint::parse(url, rest())
}

pub(crate) fn coverage(instruments: &[&str], frequency: UpdateFrequency) -> Result<SourceCoverage> {
    Ok(SourceCoverage::new(
        [AssetClass::Equity],
        [SourceRegion::Europe],
        instruments.iter().map(|i| (*i).to_string()),
        frequency,
    )?
    .with_history_from(now().saturating_sub(Duration::from_days(3_650))))
}

pub(crate) fn licensed_for(usages: &[Usage]) -> Result<LicensingPosture> {
    Ok(LicensingPosture::declared(SourceLicense::new(
        "vendor-terms-2026",
        usages.iter().copied(),
    )?))
}

/// A candidate with everything permissive, so a test can vary one thing.
pub(crate) fn candidate(
    id: &str,
    url: &str,
    licensing: LicensingPosture,
    instruments: &[&str],
) -> Result<SourceCandidate> {
    SourceCandidate::new(
        SourceIdentity::new(id, format!("{id} feed"), "Example Data Ltd")?,
        endpoint(url)?,
        coverage(instruments, UpdateFrequency::Minutely)?,
        licensing,
        SourceCost::free(Currency::EUR),
        SourceRegion::Europe,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        now(),
    )
}

pub(crate) fn paid_candidate(
    id: &str,
    url: &str,
    licensing: LicensingPosture,
    monthly: i64,
) -> Result<SourceCandidate> {
    SourceCandidate::new(
        SourceIdentity::new(id, format!("{id} feed"), "Example Data Ltd")?,
        endpoint(url)?,
        coverage(&["EU0001"], UpdateFrequency::Minutely)?,
        licensing,
        SourceCost::new(
            Decimal::from_int(monthly),
            Decimal::ZERO,
            u64::MAX,
            Currency::EUR,
        )?,
        SourceRegion::Europe,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        now(),
    )
}

pub(crate) fn ok_head() -> HeadResponse {
    HeadResponse {
        status: 200,
        content_type: Some("application/json".to_string()),
        content_length: Some(512),
        last_modified: Some(now()),
        latency: Duration::from_millis(40),
    }
}

pub(crate) fn sample(body: &str) -> PayloadSample {
    PayloadSample {
        body: body.to_string(),
        media_type: "application/json".to_string(),
        payload_at: Some(now()),
        latency: Duration::from_millis(55),
    }
}

pub(crate) const QUOTE_PAYLOAD: &str =
    r#"{"symbol":"EU0001","bid":10.25,"ask":10.27,"volume":41000}"#;

/// A probe scripted to serve `robots` for the host and a healthy JSON quote.
pub(crate) fn probe_for(url: &str, host: &str, robots: RobotsFetch) -> InMemoryProbe {
    InMemoryProbe::new()
        .with_robots(host, robots)
        .with_head(url, ok_head())
        .with_sample(url, sample(QUOTE_PAYLOAD))
}

pub(crate) fn robots_served(body: &str) -> RobotsFetch {
    RobotsFetch::Served {
        body: body.to_string(),
        latency: Duration::from_millis(12),
    }
}

pub(crate) fn robots_absent() -> RobotsFetch {
    RobotsFetch::Absent {
        status: 404,
        latency: Duration::from_millis(9),
    }
}

/// A robots.txt that permits everything this crawler asks for.
pub(crate) fn permissive_robots() -> RobotsFetch {
    robots_served("User-agent: *\nAllow: /\n")
}
