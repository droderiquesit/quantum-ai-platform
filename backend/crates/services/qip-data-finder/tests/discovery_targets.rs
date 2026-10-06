//! DATA-031 discovery starts from gaps and forecast errors; DATA-032 queries
//! carry geography, entity, domain and language.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_data_finder::discovery_targets::{
    DiscoveryTarget, EntityProfile, ForecastError, KnowledgeGap, TargetReason, queries_for,
    targets_from,
};
use std::collections::BTreeSet;

fn covered(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn err(entity: &str, error: f64, baseline: f64) -> ForecastError {
    ForecastError {
        entity: entity.into(),
        error,
        baseline,
    }
}

#[test]
fn a_forecast_error_spike_on_an_entity_no_source_covers_yields_a_target_naming_it() {
    let errors = [
        err("ent-northwind", 9.0, 1.0), // spike, uncovered
        err("ent-covered", 20.0, 1.0),  // spike, but a source exists
        err("ent-quiet", 1.1, 1.0),     // uncovered, within its usual error
    ];
    let covered = covered(&["ent-covered"]);
    // Premise: the covered entity really spiked, so excluding it is the rule at work.
    assert!(errors[1].error / errors[1].baseline >= 3.0);
    let targets = targets_from(&[], &errors, &covered, 3.0).expect("targets");
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].entity, "ent-northwind");
    assert!(
        matches!(targets[0].reason, TargetReason::ForecastErrorSpike { ratio } if (ratio - 9.0).abs() < 1e-9)
    );
}

#[test]
fn a_world_model_gap_is_a_target_unless_covered_and_a_spike_outranks_a_bare_gap() {
    let gaps = [
        KnowledgeGap {
            entity: "ent-b".into(),
        },
        KnowledgeGap {
            entity: "ent-a".into(),
        },
        KnowledgeGap {
            entity: "ent-has-source".into(),
        },
    ];
    let errors = [err("ent-b", 5.0, 1.0)];
    let targets = targets_from(&gaps, &errors, &covered(&["ent-has-source"]), 3.0).expect("t");
    let names: Vec<_> = targets.iter().map(|t| t.entity.as_str()).collect();
    assert_eq!(
        names,
        ["ent-b", "ent-a"],
        "spike first, then gaps, covered dropped"
    );
    assert_eq!(targets[1].reason, TargetReason::KnowledgeGap);
}

#[test]
fn a_baseline_that_would_manufacture_a_spike_is_refused() {
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            targets_from(&[], &[err("e", 1.0, bad)], &covered(&[]), 3.0).is_err(),
            "baseline {bad}"
        );
    }
    assert!(targets_from(&[], &[], &covered(&[]), 1.0).is_err());
}

fn profile() -> EntityProfile {
    EntityProfile {
        entity: "ent-northwind".into(),
        geographies: vec!["TW".into(), "US".into()],
        domains: vec!["regulatory".into(), "supply-chain".into()],
        languages: vec!["en".into(), "zh".into()],
    }
}

#[test]
fn every_generated_query_is_tagged_with_geography_entity_domain_and_language() {
    let target = DiscoveryTarget {
        entity: "ent-northwind".into(),
        reason: TargetReason::KnowledgeGap,
    };
    let queries = queries_for(&target, &profile()).expect("queries");
    assert_eq!(queries.len(), 8, "2 geographies x 2 domains x 2 languages");
    let distinct: BTreeSet<_> = queries.iter().map(|q| q.text.clone()).collect();
    assert_eq!(distinct.len(), 8, "no two queries are the same");
    for q in &queries {
        for (label, v) in [
            ("entity", &q.entity),
            ("geography", &q.geography),
            ("domain", &q.domain),
            ("language", &q.language),
        ] {
            assert!(!v.is_empty(), "{label} missing on {q:?}");
            assert!(
                q.text.contains(v.as_str()),
                "{label} not in the text of {q:?}"
            );
        }
    }
}

#[test]
fn a_profile_missing_any_dimension_produces_no_queries_at_all() {
    let target = DiscoveryTarget {
        entity: "ent-northwind".into(),
        reason: TargetReason::KnowledgeGap,
    };
    for strip in 0..3 {
        let mut p = profile();
        match strip {
            0 => p.geographies.clear(),
            1 => p.domains.clear(),
            _ => p.languages = vec![" ".into()],
        }
        assert!(queries_for(&target, &p).is_err(), "dimension {strip}");
    }
    let mut other = profile();
    other.entity = "ent-other".into();
    assert!(
        queries_for(&target, &other).is_err(),
        "profile for another entity"
    );
}

// --- candidate locations (DATA-032) -----------------------------------------

mod common;

use qip_contracts::governance::Usage;
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::discovery_targets::{CandidateLocation, DiscoveryQuery, locations_for};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::source::{SourceCandidate, SourceIdentity};

/// A catalogued candidate declaring `instrument` in `region`.
fn catalogued(id: &str, instrument: &str, region: SourceRegion) -> SourceCandidate {
    SourceCandidate::new(
        SourceIdentity::new(id, format!("{id} feed"), "Example Data Ltd").expect("identity"),
        common::endpoint(&format!("https://{id}.example/data")).expect("endpoint"),
        SourceCoverage::new(
            [qip_financial::asset_class::AssetClass::Equity],
            [region],
            [instrument.to_string()],
            UpdateFrequency::Daily,
        )
        .expect("coverage"),
        common::licensed_for(&[Usage::Derive]).expect("licence"),
        SourceCost::free(qip_core::Currency::EUR),
        region,
        [qip_events::Topic::MarketQuote],
        "a curated directory",
        common::now(),
    )
    .expect("candidate")
}

fn tagged(location: &CandidateLocation) -> bool {
    [
        &location.entity,
        &location.geography,
        &location.domain,
        &location.language,
    ]
    .iter()
    .all(|dimension| !dimension.trim().is_empty())
}

#[test]
fn every_query_and_candidate_location_from_a_gap_carries_all_four_dimensions() {
    // The requirement's own check, from the gap forwards.
    let gaps = [KnowledgeGap {
        entity: "ent-northwind".into(),
    }];
    let targets = targets_from(&gaps, &[], &covered(&[]), 3.0).expect("targets");
    assert_eq!(targets.len(), 1, "the premise: the gap is a target");
    let profile = EntityProfile {
        entity: "ent-northwind".into(),
        geographies: vec!["europe".into(), "apac".into()],
        domains: vec!["filings".into()],
        languages: vec!["de".into(), "ja".into()],
    };
    let catalogue = [
        catalogued("registry-eu", "ent-northwind", SourceRegion::Europe),
        catalogued("wire-global", "ent-northwind", SourceRegion::Global),
        // Declares the entity, serves a geography no query asks about.
        catalogued("registry-us", "ent-northwind", SourceRegion::UsEast),
        // Serves the geography, declares another entity.
        catalogued("other-eu", "ent-elsewhere", SourceRegion::Europe),
    ];

    let queries = queries_for(&targets[0], &profile).expect("queries");
    assert_eq!(queries.len(), 4, "2 geographies x 1 domain x 2 languages");
    let mut found = Vec::new();
    for query in &queries {
        let locations = locations_for(query, &catalogue).expect("locations");
        for location in &locations {
            assert!(tagged(location), "an untagged location: {location:?}");
            assert_eq!(
                (
                    &location.entity,
                    &location.geography,
                    &location.domain,
                    &location.language
                ),
                (
                    &query.entity,
                    &query.geography,
                    &query.domain,
                    &query.language
                ),
                "a location carries tags its query does not"
            );
        }
        found.push((
            query.geography.clone(),
            locations
                .into_iter()
                .map(|location| location.source_id)
                .collect::<Vec<_>>(),
        ));
    }
    // The premise that makes the loop above mean something: locations were
    // produced, and exactly the candidates that declare the entity and serve
    // the geography — the global wire everywhere, the registry only at home.
    let europe: Vec<_> = found.iter().filter(|(g, _)| g == "europe").collect();
    let apac: Vec<_> = found.iter().filter(|(g, _)| g == "apac").collect();
    assert_eq!(europe.len(), 2);
    assert_eq!(apac.len(), 2);
    assert!(
        europe
            .iter()
            .all(|(_, ids)| ids == &["registry-eu", "wire-global"])
    );
    assert!(apac.iter().all(|(_, ids)| ids == &["wire-global"]));
}

#[test]
fn a_query_missing_any_dimension_yields_no_location_at_all() {
    let catalogue = [catalogued(
        "wire-global",
        "ent-northwind",
        SourceRegion::Global,
    )];
    let whole = DiscoveryQuery {
        entity: "ent-northwind".into(),
        geography: "europe".into(),
        domain: "filings".into(),
        language: "de".into(),
        text: "ent-northwind filings europe lang:de".into(),
    };
    // The premise: the whole query has somewhere to look, so each refusal
    // below is the missing dimension's doing.
    assert_eq!(
        locations_for(&whole, &catalogue).expect("locations").len(),
        1
    );

    for missing in ["entity", "geography", "domain", "language"] {
        let mut query = whole.clone();
        match missing {
            "entity" => query.entity = " ".into(),
            "geography" => query.geography = String::new(),
            "domain" => query.domain = String::new(),
            _ => query.language = "\t".into(),
        }
        let error = locations_for(&query, &catalogue)
            .expect_err("a query with a missing dimension produced locations");
        assert!(
            error.message().contains(missing),
            "the refusal does not name the missing {missing}: {}",
            error.message()
        );
    }
}
