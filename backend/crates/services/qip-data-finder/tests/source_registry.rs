//! EXPAND-033: the Source Registry's entry carries provenance, cost, licence,
//! freshness and utility together, and a source lacking any one of them never
//! becomes an entry.
//!
//! The failure prevented is a registry that can answer four of the five. A
//! source with no recorded cost, or no utility anybody measured, is still a
//! source the platform is paying for and reasoning from; the registry would
//! list it and say nothing a reader could use to ask whether it should be.
//!
//! Every case goes through the path a deployment registers by: the committed
//! catalogue's loader, then `DataFinder::assess`.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

mod common;

use common::{
    AGENT, QUOTE_PAYLOAD, candidate, endpoint, licensed_for, now, ok_head, permissive_robots,
    probe_for,
};
use qip_contracts::governance::Usage;
use qip_core::error::Result;
use qip_core::{Currency, Decimal, Duration};
use qip_data_finder::catalogue::{CandidateEntry, load};
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::legal::LicensingPosture;
use qip_data_finder::probe::PayloadSample;
use qip_data_finder::quality::SourceCost;
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_data_finder::{DataFinder, DecisionOutcome, FinderConfig, Routing};
use qip_events::Topic;
use qip_financial::asset_class::AssetClass;

const URL: &str = "https://example.com/v1/quotes";
const FOUND_IN: &str = "a curated directory of exchange data vendors";

fn catalogue_of(candidate: SourceCandidate) -> String {
    serde_json::to_string(&vec![CandidateEntry {
        candidate,
        egress_route: "http://127.0.0.1:9105".to_string(),
    }])
    .expect("serialisable")
}

#[test]
fn a_source_lacking_any_one_of_the_five_is_refused_and_a_complete_one_is_stored_with_all_five()
-> Result<()> {
    let licence = licensed_for(&[Usage::Derive])?;
    let text = catalogue_of(candidate("complete", URL, licence.clone(), &["EU0001"])?);

    // The complete source: loaded, assessed, stored, and read back as one
    // entry holding all five.
    let mut finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 7)?);
    let mut probe = probe_for(URL, "example.com", permissive_robots());
    let candidates = load(&text, now())?
        .entries
        .into_iter()
        .map(|entry| entry.candidate)
        .collect();
    let decisions = finder.assess(candidates, &mut probe, now())?;
    assert!(
        decisions[0].is_registered(),
        "premise: the complete source registers: {:?}",
        decisions[0].outcome()
    );
    let stored = finder.registered("complete").expect("the entry is stored");
    let entry = stored.entry();
    assert_eq!(entry.provenance.discovered_from(), FOUND_IN);
    assert_eq!(entry.provenance.discovered_at(), now());
    assert_eq!(entry.cost, &SourceCost::free(Currency::EUR));
    assert_eq!(entry.licence, &licence);
    assert_eq!(entry.freshness, UpdateFrequency::Minutely);
    assert!(entry.utility.class().is_collected());
    assert!(entry.utility.composite() >= Routing::COLD_THRESHOLD);

    // Provenance, cost, licence and freshness are fields of the catalogue
    // record. An entry without any one of them refuses the whole load, by
    // name, so nothing is assessed from a half-described source.
    let whole: serde_json::Value = serde_json::from_str(&text).expect("the catalogue is json");
    for path in [
        vec!["discovered_from"],
        vec!["cost"],
        vec!["declared_licensing"],
        vec!["declared_coverage", "update_frequency"],
    ] {
        let mut lacking = whole.clone();
        let mut at = &mut lacking[0]["candidate"];
        for key in &path[..path.len() - 1] {
            at = &mut at[*key];
        }
        let field = path[path.len() - 1];
        assert!(
            at.as_object_mut()
                .expect("an object")
                .remove(field)
                .is_some(),
            "premise: the catalogue record states `{field}`"
        );
        let refusal =
            load(&lacking.to_string(), now()).expect_err("a source lacking a field loaded");
        assert!(
            refusal
                .message()
                .contains(&format!("missing field `{field}`")),
            "the refusal for `{field}` names something else: {}",
            refusal.message()
        );
    }

    // Stated but empty is the same lack. Provenance left blank is refused
    // where the candidate is built.
    let mut blank = whole.clone();
    blank[0]["candidate"]["discovered_from"] = serde_json::json!("  ");
    assert!(
        load(&blank.to_string(), now())
            .expect_err("a source from nowhere loaded")
            .message()
            .contains("must record where it was discovered")
    );

    // A licence nobody has determined reaches the finder and is not stored.
    let undetermined = finder.assess(
        vec![candidate(
            "unlicensed",
            URL,
            LicensingPosture::Undetermined,
            &["EU0002"],
        )?],
        &mut probe,
        now(),
    )?;
    assert!(!undetermined[0].is_registered());
    assert!(finder.registered("unlicensed").is_none());

    // Utility is measured, not declared, so a source lacks it by scoring
    // below the floor: a paid duplicate of what is already held, over a
    // channel the network can rewrite, with no history and no payload time.
    let worthless_url = "http://mirror.example.net/v1/quotes";
    let worthless = SourceCandidate::new(
        SourceIdentity::new("worthless", "worthless feed", "Example Data Ltd")?,
        endpoint(worthless_url)?,
        SourceCoverage::new(
            [AssetClass::Equity],
            [SourceRegion::Europe],
            ["EU0001".to_string()],
            UpdateFrequency::Minutely,
        )?,
        licence,
        SourceCost::new(
            Decimal::from_int(50_000),
            Decimal::ZERO,
            u64::MAX,
            Currency::EUR,
        )?,
        SourceRegion::Europe,
        [Topic::MarketQuote],
        FOUND_IN,
        now(),
    )?;
    let mut mirror = probe
        .with_robots("mirror.example.net", permissive_robots())
        .with_head(worthless_url, ok_head())
        .with_sample(
            worthless_url,
            PayloadSample {
                body: QUOTE_PAYLOAD.to_string(),
                media_type: "application/json".to_string(),
                payload_at: None,
                latency: Duration::from_millis(55),
            },
        );
    let scored = finder.assess(vec![worthless], &mut mirror, now())?;
    assert!(
        matches!(scored[0].outcome(), DecisionOutcome::Rejected { reason } if reason.contains("is below the floor")),
        "premise: the source is refused on its score: {:?}",
        scored[0].outcome()
    );
    assert!(
        scored[0].scores().expect("it was scored").composite() < Routing::COLD_THRESHOLD,
        "premise: the refusal is the utility floor, not the law"
    );
    assert!(!scored[0].is_registered());
    assert!(finder.registered("worthless").is_none());
    assert_eq!(
        finder.registry().len(),
        1,
        "only the complete source is held"
    );
    Ok(())
}
