//! Catalyst detection properties.
//!
//! The properties that must hold structurally, not merely usually:
//!
//! * no look-ahead through an event — an event known after a move begins can
//!   never be offered as its explanation, and the context refuses events from
//!   the future of the scan outright;
//! * the explained/unexplained distinction survives an event arriving late;
//! * a move is never called *unexplained* when no event stream was watched;
//! * impact statistics are estimated only from event→move pairs where the
//!   temporal ordering held, and a class with fewer observations than the
//!   stated minimum reports insufficient history rather than a number;
//! * detection is deterministic.

use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::approx_eq;
use qip_core::{Context, Duration, Timestamp};
use qip_financial::intelligence::{EntityMention, NewsItem, NewsSource, Sentiment};
use qip_financial::quality::{DataQuality, Provenance};
use qip_opportunity_engine::catalyst::{
    CatalystDetector, ImpactAssessment, ImpactHistory, ImpactScope, KnownEvents, MarketEvent,
};
use qip_opportunity_engine::detector::{AnomalyKind, DetectionContext, Detector};
use qip_opportunity_engine::engine::OpportunityEngine;

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn days(n: i64) -> Duration {
    Duration::from_days(n)
}

/// A calm price path with no anomalies.
fn calm_prices(seed: u64, n: usize) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    let mut price = 100.0;
    (0..n)
        .map(|_| {
            price *= (rng.normal_with(0.0002, 0.006)).exp();
            price
        })
        .collect()
}

/// A calm path ending in a large drop — the move to be explained.
fn shocked_prices(seed: u64, n: usize) -> Vec<f64> {
    let mut prices = calm_prices(seed, n);
    let last = prices[prices.len() - 1];
    prices.push(last * 0.88);
    prices
}

fn event(id: &str, subject: &str, class: &str, known_at: Timestamp) -> MarketEvent {
    MarketEvent::new(id, subject, class, known_at, known_at)
        .with_description(format!("{class} on {subject}"))
}

fn kinds_of(anomalies: &[qip_opportunity_engine::detector::Anomaly]) -> Vec<AnomalyKind> {
    anomalies.iter().map(|a| a.kind).collect()
}

// --- the look-ahead property ------------------------------------------------

#[test]
fn an_event_known_before_the_move_explains_it_with_the_lag_recorded() {
    // The move window is the last bar: it begins one bar interval before
    // as_of. An event known a day before that is a plausible explanation.
    let known_at = now().saturating_sub(days(2));
    let detection = DetectionContext::new(now())
        .with_prices("OBJ-X", shocked_prices(1, 200))
        .with_events(vec![event("ev-1", "OBJ-X", "earnings", known_at)]);

    let anomalies = CatalystDetector::default().detect(&detection);
    assert_eq!(kinds_of(&anomalies), vec![AnomalyKind::Catalyst]);
    let link = anomalies[0].catalyst.as_ref().expect("linkage recorded");
    assert_eq!(link.event_id, "ev-1");
    assert_eq!(link.event_class, "earnings");
    assert_eq!(link.event_known_at, known_at);
    // Known two days before as_of; the move began one day before as_of.
    assert_eq!(link.lag, days(1));
    assert!(anomalies[0].description.contains("earnings"));
}

#[test]
fn an_event_known_after_the_move_begins_cannot_explain_it() {
    // The event becomes knowable twelve hours before as_of — inside the move
    // window, after the move began. It must not be offered as an explanation:
    // the move is unexplained, and the two kinds stay distinct.
    let late = now().saturating_sub(Duration::from_hours(12));
    let detection = DetectionContext::new(now())
        .with_prices("OBJ-X", shocked_prices(1, 200))
        .with_events(vec![event("ev-late", "OBJ-X", "earnings", late)]);

    let anomalies = CatalystDetector::default().detect(&detection);
    assert_eq!(
        kinds_of(&anomalies),
        vec![AnomalyKind::UnexplainedMove],
        "an event known after the move began must not explain it"
    );
    assert!(anomalies[0].catalyst.is_none());
    assert!(anomalies[0].description.contains("no knowable catalyst"));
}

#[test]
fn the_context_refuses_events_from_the_future_of_the_scan() {
    // Structural, not checked-at-use: with_events filters by known-time, so an
    // event knowable only tomorrow never enters the context at all.
    let detection = DetectionContext::new(now())
        .with_prices("OBJ-X", shocked_prices(1, 200))
        .with_events(vec![event(
            "ev-future",
            "OBJ-X",
            "earnings",
            now().saturating_add(days(1)),
        )]);
    assert!(detection.events.is_empty(), "the future was filtered out");
    assert!(detection.events.covers(now()), "but coverage is recorded");

    // And the same directly on the container.
    let filtered = KnownEvents::known_by(
        now(),
        vec![
            event("kept", "OBJ-X", "earnings", now().saturating_sub(days(1))),
            event(
                "dropped",
                "OBJ-X",
                "earnings",
                now().saturating_add(days(1)),
            ),
        ],
    );
    assert_eq!(filtered.len(), 1);
    assert_eq!(
        filtered.iter().next().map(|e| e.event_id.as_str()),
        Some("kept")
    );
}

#[test]
fn even_a_hand_built_container_of_future_events_is_filtered_by_the_detector() {
    // Belt and braces: a container legitimately covering a later time can hold
    // events knowable after this scan's as_of. The detector must still refuse
    // them in both passes.
    let future_event = event(
        "ev-future",
        "OBJ-X",
        "earnings",
        now().saturating_add(days(1)),
    );
    let mut detection = DetectionContext::new(now()).with_prices("OBJ-X", shocked_prices(1, 200));
    detection.events = KnownEvents::known_by(now().saturating_add(days(2)), vec![future_event]);

    let anomalies = CatalystDetector::default().detect(&detection);
    assert!(
        anomalies
            .iter()
            .all(|a| a.catalyst.is_none() && a.kind != AnomalyKind::Catalyst),
        "an event from the scan's future must never be cited"
    );
}

#[test]
fn a_known_time_earlier_than_the_occurrence_is_clamped_forward() {
    // The same discipline as the world model's absorb_bar: a thing cannot be
    // knowable before it happened, so a mis-stamped record is clamped rather
    // than trusted.
    let occurred = now();
    let clamped = MarketEvent::new(
        "ev-1",
        "OBJ-X",
        "earnings",
        occurred,
        occurred.saturating_sub(days(3)),
    );
    assert_eq!(clamped.known_at(), occurred);
    assert_eq!(clamped.occurred_at(), occurred);
}

// --- explained versus unexplained -------------------------------------------

#[test]
fn the_distinction_survives_an_event_arriving_late() {
    // The same happening, stamped twice. Filed before the move: the move is
    // explained. The same filing arriving only after the move began: the move
    // is unexplained, however tempting the story is in hindsight — conflating
    // the two would hide exactly the moves the platform should escalate.
    let prices = shocked_prices(1, 200);
    let run = |known_at: Timestamp| {
        let detection = DetectionContext::new(now())
            .with_prices("OBJ-X", prices.clone())
            .with_events(vec![event("filing-1", "OBJ-X", "guidance", known_at)]);
        CatalystDetector::default().detect(&detection)
    };

    let timely = run(now().saturating_sub(days(2)));
    let late = run(now().saturating_sub(Duration::from_hours(1)));
    assert_eq!(kinds_of(&timely), vec![AnomalyKind::Catalyst]);
    assert_eq!(kinds_of(&late), vec![AnomalyKind::UnexplainedMove]);
    assert!(
        AnomalyKind::UnexplainedMove.base_importance() > AnomalyKind::Catalyst.base_importance(),
        "the unexplained case is the one most worth investigating"
    );
}

#[test]
fn a_move_without_event_coverage_is_not_called_unexplained() {
    // No with_events call: nobody watched the event stream. "No events were
    // supplied" is not "no catalyst existed", so the detector says nothing.
    let detection = DetectionContext::new(now()).with_prices("OBJ-X", shocked_prices(1, 200));
    assert!(
        CatalystDetector::default().detect(&detection).is_empty(),
        "no coverage means no claim either way"
    );

    // Watched and empty is different: that supports the claim.
    let watched = DetectionContext::new(now())
        .with_prices("OBJ-X", shocked_prices(1, 200))
        .with_events(Vec::new());
    assert_eq!(
        kinds_of(&CatalystDetector::default().detect(&watched)),
        vec![AnomalyKind::UnexplainedMove]
    );
}

#[test]
fn a_stale_event_does_not_explain_a_fresh_move() {
    // Known well before the move, but beyond the explanation lag: a filing
    // from a month ago does not explain this morning's gap.
    let detection = DetectionContext::new(now())
        .with_prices("OBJ-X", shocked_prices(1, 200))
        .with_events(vec![event(
            "ev-old",
            "OBJ-X",
            "earnings",
            now().saturating_sub(days(30)),
        )]);
    assert_eq!(
        kinds_of(&CatalystDetector::default().detect(&detection)),
        vec![AnomalyKind::UnexplainedMove]
    );
}

#[test]
fn a_calm_series_with_no_events_is_silent() {
    let detection = DetectionContext::new(now())
        .with_prices("OBJ-X", calm_prices(4, 200))
        .with_events(Vec::new());
    assert!(CatalystDetector::default().detect(&detection).is_empty());
}

// --- impact statistics ------------------------------------------------------

#[test]
fn impact_statistics_refuse_pairs_where_the_ordering_did_not_hold() {
    let mut history = ImpactHistory::default();
    let e = event("ev-1", "OBJ-X", "earnings", now());

    // A move that began before the event was knowable is refused, however
    // many times it is offered: recording it would launder look-ahead into a
    // statistic that looks clean.
    for _ in 0..50 {
        assert!(!history.record_outcome(&e, now().saturating_sub(days(1)), -0.05));
    }
    assert_eq!(history.observations_of("earnings"), 0);
    assert!(matches!(
        history.assess("earnings", "OBJ-X"),
        ImpactAssessment::InsufficientHistory {
            observations: 0,
            ..
        }
    ));

    // The same magnitude with the ordering held is accepted.
    assert!(history.record_outcome(&e, now().saturating_add(days(1)), -0.05));
    assert_eq!(history.observations_of("earnings"), 1);
}

#[test]
fn a_thin_class_reports_insufficient_history_not_zero() {
    let mut history = ImpactHistory::default();
    let known = now().saturating_sub(days(10));
    for index in 0..3 {
        let e = event(&format!("ev-{index}"), "OBJ-X", "earnings", known);
        assert!(history.record_outcome(&e, known.saturating_add(days(1)), 0.04));
    }

    match history.assess("earnings", "OBJ-X") {
        ImpactAssessment::InsufficientHistory {
            observations,
            required,
        } => {
            assert_eq!(observations, 3);
            assert_eq!(required, history.minimum_observations());
        }
        ImpactAssessment::Estimated(estimate) => {
            panic!("three observations must not become a statistic: {estimate:?}")
        }
    }

    // A landing event of that class produces no impact anomaly at all: the
    // detector's only added value is the estimate, and it refuses to invent
    // one rather than reporting a zero.
    let detection = DetectionContext::new(now())
        .with_prices("OBJ-X", calm_prices(4, 200))
        .with_events(vec![event(
            "ev-landing",
            "OBJ-X",
            "earnings",
            now().saturating_sub(Duration::from_hours(6)),
        )]);
    let detector = CatalystDetector::with_history(history);
    assert!(
        detector.detect(&detection).is_empty(),
        "no history means no impact statement, not a zero-impact statement"
    );
}

#[test]
fn a_class_with_history_states_magnitude_hit_rate_and_lag() {
    let mut history = ImpactHistory::default();
    let outcomes = [
        0.04, 0.05, -0.03, 0.04, 0.03, -0.05, 0.04, 0.01, -0.005, 0.015,
    ];
    for (index, signed_return) in outcomes.iter().enumerate() {
        let known = now().saturating_sub(days(200 - index as i64));
        let e = event(&format!("ev-{index}"), "OBJ-X", "earnings", known);
        assert!(history.record_outcome(&e, known.saturating_add(days(1)), *signed_return));
    }

    let ImpactAssessment::Estimated(estimate) = history.assess("earnings", "OBJ-X") else {
        panic!("ten observations clear the default minimum");
    };
    assert_eq!(estimate.scope, ImpactScope::Instrument);
    assert_eq!(estimate.observations, 10);
    assert!(approx_eq(estimate.median_abs_return, 0.035, 1e-12));
    assert!(approx_eq(estimate.mean_abs_return, 0.031, 1e-12));
    // Seven of ten cleared the 2% materiality threshold.
    assert!(approx_eq(estimate.hit_rate, 0.7, 1e-12));
    assert_eq!(estimate.typical_lag, days(1));
    assert!(estimate.statement().contains("70%"));

    // An event of the class landing now, with no move yet: the anomaly states
    // the historical impact so REASON gets a distribution, not a headline.
    let detection = DetectionContext::new(now())
        .with_prices("OBJ-X", calm_prices(4, 200))
        .with_events(vec![event(
            "ev-landing",
            "OBJ-X",
            "earnings",
            now().saturating_sub(Duration::from_hours(6)),
        )]);
    let anomalies = CatalystDetector::with_history(history).detect(&detection);
    assert_eq!(kinds_of(&anomalies), vec![AnomalyKind::Catalyst]);
    let anomaly = &anomalies[0];
    assert!(approx_eq(anomaly.observed, 0.035, 1e-12));
    assert_eq!(anomaly.sample_size, 10);
    assert!(anomaly.description.contains("landed"));
    assert!(
        anomaly.description.contains("exceeding 2.0% in 70%"),
        "{}",
        anomaly.description
    );
    let link = anomaly.catalyst.as_ref().expect("landing carries linkage");
    assert_eq!(link.event_id, "ev-landing");
    assert!(link.impact.is_estimated());
}

#[test]
fn per_instrument_history_outranks_the_class_wide_fallback() {
    let mut history = ImpactHistory::default();
    let known = now().saturating_sub(days(50));
    for index in 0..8 {
        let e = event(&format!("x-{index}"), "OBJ-X", "earnings", known);
        assert!(history.record_outcome(&e, known.saturating_add(days(1)), 0.04));
    }
    for index in 0..3 {
        let e = event(&format!("y-{index}"), "OBJ-Y", "earnings", known);
        assert!(history.record_outcome(&e, known.saturating_add(days(2)), 0.08));
    }

    let ImpactAssessment::Estimated(own) = history.assess("earnings", "OBJ-X") else {
        panic!("OBJ-X has enough of its own history");
    };
    assert_eq!(own.scope, ImpactScope::Instrument);
    assert_eq!(own.observations, 8);

    // OBJ-Y alone is thin, but the class across instruments is not; the
    // estimate says which scope it used so the weaker claim is visible.
    let ImpactAssessment::Estimated(fallback) = history.assess("earnings", "OBJ-Y") else {
        panic!("the class-wide fallback has eleven observations");
    };
    assert_eq!(fallback.scope, ImpactScope::Class);
    assert_eq!(fallback.observations, 11);
    assert!(fallback.statement().contains("class-wide"));
}

#[test]
fn an_explained_move_of_a_thin_class_says_so_in_its_linkage() {
    // The move is explained — the event is real and prior — but the class has
    // no history, and the linkage records the refusal instead of a number.
    let detection = DetectionContext::new(now())
        .with_prices("OBJ-X", shocked_prices(1, 200))
        .with_events(vec![event(
            "ev-1",
            "OBJ-X",
            "spinoff",
            now().saturating_sub(days(2)),
        )]);
    let anomalies = CatalystDetector::default().detect(&detection);
    assert_eq!(kinds_of(&anomalies), vec![AnomalyKind::Catalyst]);
    let link = anomalies[0].catalyst.as_ref().expect("linkage recorded");
    assert!(matches!(
        link.impact,
        ImpactAssessment::InsufficientHistory {
            observations: 0,
            ..
        }
    ));
    assert!(anomalies[0].description.contains("insufficient history"));
}

// --- construction from intelligence records ---------------------------------

#[test]
fn a_news_item_stamped_before_publication_is_clamped_to_publication() {
    let published = now().saturating_sub(days(1));
    let item = NewsItem {
        item_id: "wire-1".to_string(),
        headline: "Northwind cuts guidance".to_string(),
        body: String::new(),
        source: NewsSource::Newswire,
        published_at: published,
        entities: vec![EntityMention {
            text: "Northwind".to_string(),
            entity_id: Some("ent-northwind".to_string()),
            confidence: 0.95,
            is_primary: true,
            sentiment: None,
        }],
        sentiment: Sentiment {
            polarity: -0.8,
            confidence: 0.9,
            novelty: 0.7,
        },
        topics: vec!["guidance".to_string()],
        // A mis-stamped feed claiming ingestion before publication.
        provenance: Provenance::new("wire", published, published.saturating_sub(days(2))),
        quality: DataQuality::default(),
    };

    let events = MarketEvent::from_news(&item);
    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(e.subject, "ent-northwind");
    assert_eq!(e.class, "guidance");
    assert_eq!(
        e.known_at(),
        published,
        "a story cannot have been actionable before it was published"
    );
    assert!(e.direction < 0.0, "sentiment carries the direction");
}

// --- determinism and engine integration -------------------------------------

#[test]
fn detection_is_deterministic() {
    let run = || {
        let mut history = ImpactHistory::default();
        let known = now().saturating_sub(days(40));
        for index in 0..9 {
            let e = event(&format!("h-{index}"), "OBJ-X", "earnings", known);
            assert!(history.record_outcome(
                &e,
                known.saturating_add(days(1)),
                if index % 2 == 0 { 0.04 } else { -0.03 },
            ));
        }
        let detection = DetectionContext::new(now())
            .with_prices("OBJ-X", shocked_prices(1, 200))
            .with_prices("OBJ-Y", calm_prices(4, 200))
            .with_events(vec![
                event("ev-1", "OBJ-X", "earnings", now().saturating_sub(days(2))),
                event(
                    "ev-2",
                    "OBJ-Y",
                    "earnings",
                    now().saturating_sub(Duration::from_hours(3)),
                ),
            ]);
        let anomalies = CatalystDetector::with_history(history).detect(&detection);
        serde_json::to_string(&anomalies).expect("anomalies serialize")
    };
    assert_eq!(run(), run());
}

#[test]
fn the_engine_carries_the_event_into_the_opportunity_evidence() {
    let (context, _clock) = Context::deterministic(now(), 11);
    let detection = DetectionContext::new(now())
        .with_prices("OBJ-X", shocked_prices(1, 250))
        .with_events(vec![event(
            "ev-1",
            "OBJ-X",
            "earnings",
            now().saturating_sub(days(2)),
        )]);

    let mut engine = OpportunityEngine::default();
    let opportunities = engine.scan(&detection, &context);
    assert_eq!(opportunities.len(), 1);
    let opportunity = &opportunities[0];
    assert!(
        opportunity
            .anomalies
            .iter()
            .any(|a| a.kind == AnomalyKind::Catalyst),
        "the standard registry runs the catalyst detector"
    );
    assert!(
        opportunity
            .evidence
            .iter()
            .any(|entry| entry == "event:ev-1"),
        "the explaining event is cited as evidence: {:?}",
        opportunity.evidence
    );
}
