//! R12-DATA-2002: for a source the finder approves, `qip registrations
//! propose` proposes the adapter a person would build, matching the
//! source's access mechanism; for one it refuses, it proposes nothing.
//!
//! `plan_for` and `descriptor_for` existed and were called only in tests, so
//! an approved source ended in a decision nobody could act on. The proposal
//! runs through `Platform::assess_sources`, the deep brain's discovery seam,
//! so the refusals are the finder's own.

#![allow(clippy::panic_in_result_fn)]

use qip_cli::adapter_proposals::{Proposal, propose};
use qip_contracts::governance::Usage;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Currency};
use qip_data_finder::catalogue::CandidateEntry;
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
use qip_data_finder::legal::{LicensingPosture, SourceLicense};
use qip_data_finder::probe::{HeadResponse, InMemoryProbe, PayloadSample, RobotsFetch};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_events::Topic;
use qip_financial::asset_class::AssetClass;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

const OPEN_URL: &str = "https://open.example/quotes";
const CLOSED_URL: &str = "https://closed.example/quotes";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

fn entry(id: &str, url: &str) -> Result<CandidateEntry> {
    let candidate = SourceCandidate::new(
        SourceIdentity::new(id, format!("{id} feed"), "Example Data Ltd")?,
        SourceEndpoint::parse(
            url,
            AccessMechanism::Rest {
                auth: AuthRequirement::None,
                incremental_parameter: None,
                page_size: 100,
            },
        )?,
        SourceCoverage::new(
            [AssetClass::Equity],
            [SourceRegion::UsEast],
            [format!("ent-{id}")],
            UpdateFrequency::Minutely,
        )?,
        LicensingPosture::declared(SourceLicense::new(
            "vendor-terms-2026",
            [Usage::Research, Usage::Derive, Usage::Trade],
        )?),
        SourceCost::free(Currency::USD),
        SourceRegion::UsEast,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        start(),
    )?;
    Ok(CandidateEntry {
        candidate,
        egress_route: "http://127.0.0.1:1".to_string(),
    })
}

fn served(probe: InMemoryProbe, host: &str, url: &str, robots: &str) -> InMemoryProbe {
    probe
        .with_robots(
            host,
            RobotsFetch::Served {
                body: robots.to_string(),
                latency: Duration::from_millis(12),
            },
        )
        .with_head(
            url,
            HeadResponse {
                status: 200,
                content_type: Some("application/json".to_string()),
                content_length: Some(64),
                last_modified: Some(start()),
                latency: Duration::from_millis(40),
            },
        )
        .with_sample(
            url,
            PayloadSample {
                body: qip_core::untrusted::Untrusted::new(
                    "a scripted sample",
                    r#"{"symbol":"X","bid":10.25,"ask":10.27}"#.to_string(),
                ),
                media_type: "application/json".to_string(),
                payload_at: Some(start()),
                latency: Duration::from_millis(55),
            },
        )
}

/// Mutation: in `propose`, build an `Adapter` for a `Rejected` decision
/// (plan from any registered source) — the refused candidate then carries a
/// proposal and the last assertion fails.
#[test]
fn an_approved_candidate_yields_an_adapter_proposal_matching_its_access_mechanism_and_a_refused_one_yields_none()
-> Result<()> {
    let mut platform = platform()?;
    let probe = served(
        InMemoryProbe::new(),
        "open.example",
        OPEN_URL,
        "User-agent: *\nAllow: /\n",
    );
    let mut probe = served(
        probe,
        "closed.example",
        CLOSED_URL,
        "User-agent: *\nDisallow: /\n",
    );
    let entries = vec![entry("open", OPEN_URL)?, entry("closed", CLOSED_URL)?];
    let proposals = propose(&mut platform, &entries, Some(&mut probe), start())?;
    assert_eq!(
        proposals.len(),
        2,
        "one answer per candidate: {proposals:?}"
    );

    let open = proposals
        .iter()
        .find(|proposal| proposal.source() == "open")
        .expect("the open candidate was assessed");
    let Proposal::Adapter {
        plan, descriptor, ..
    } = open
    else {
        panic!("premise: the finder approves a source that allows crawling: {open:?}");
    };
    assert_eq!(
        plan.mechanism, "rest",
        "the plan follows the REST mechanism"
    );
    assert_eq!(plan.url, OPEN_URL);
    assert!(!plan.requires_buffering(), "a pull source needs no buffer");
    assert_eq!(plan.credential_required, None, "the source needs no key");
    assert_eq!(descriptor.name, "open");
    assert_eq!(descriptor.provider, "Example Data Ltd");
    assert!(
        open.lines()[0].contains("PROPOSED adapter `open`"),
        "{:?}",
        open.lines()
    );

    let closed = proposals
        .iter()
        .find(|proposal| proposal.source() == "closed")
        .expect("the closed candidate was assessed");
    assert!(
        matches!(closed, Proposal::None { reason, .. } if reason.starts_with("refused")),
        "a source whose robots rules forbid crawling yields no adapter: {closed:?}"
    );
    Ok(())
}
