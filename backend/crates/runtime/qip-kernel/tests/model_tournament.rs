//! LEARN's keyed attribution, the Model Tournament and the verifier society,
//! driven through the platform the binaries run (R12-MODEL-6003, -6021,
//! -7198, -7201; R12-REASON-3108).
//!
//! Each test drives `Platform::run_cycle`, `Platform::submit_order` or
//! `Platform::promote_model` — the calls `qip-api`, `qip-deepbrain` and the
//! CLI make — and asserts what the platform and its log hold afterwards, so
//! deleting a call site fails a test rather than leaving a library nobody
//! calls.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_ai::registry::{ModelRegistry, ModelStage};
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_events::{EventBody, Topic};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::attribution::{ATTRIBUTION_ORIGIN, AttributionRecorded};
use qip_kernel::central::models::register_fit;
use qip_kernel::config::PlatformConfig;
use qip_kernel::model_tournament::{
    ModelVerified, TOURNAMENT_ORIGIN, TournamentReview, VERIFIER_ORIGIN,
};
use qip_kernel::platform::Platform;
use qip_learning_engine::keyed_attribution::{Dimension, UNATTRIBUTED};
use qip_learning_engine::verifier::{CriticKind, SocietyVerdict};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_streaming::envelope::StreamEnvelope;
use qip_training::dataset::TrainingDataset;
use qip_training::job::TrainingSpec;
use qip_training::local::{LocalTrainer, ModelFamily, SkillPolicy, TrainedTeacher};
use qip_training::serve::InTreeProvider;
use serde::de::DeserializeOwned;

// --- fixtures ---------------------------------------------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

/// A liquid listed name on five million units a day, quoted at three basis
/// points — stated, because nothing states it for a fixture any more.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
                    InstrumentType::CommonStock,
                    fixture_liquidity(),
                )
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .expect("valid object"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("kernel-test")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        )
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

fn bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(open).expect("a price"),
        high: Decimal::from_f64(open.max(close) * 1.002).expect("a price"),
        low: Decimal::from_f64(open.min(close) * 0.998).expect("a price"),
        close: Decimal::from_f64(close).expect("a price"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }))
}

/// The kernel's founding shape: noise with a jump two thirds of the way in,
/// so the detectors find something and REASON forms a hypothesis.
fn bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.754_877_666_2) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// A near-silent tape at 100, so the fill's own price is predictable.
fn quiet_bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    (0..count)
        .map(|i| {
            let price = if i % 2 == 0 { 100.3 } else { 99.7 };
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, price, price)
        })
        .collect()
}

/// A flat tape at `level`, every day after `from`.
fn flat_bars_after(symbol: &str, from: Timestamp, days: i64, level: f64) -> Vec<SensedRecord> {
    (1..=days)
        .map(|day| {
            bar(
                symbol,
                from.saturating_add(Duration::from_days(day)),
                level,
                level,
            )
        })
        .collect()
}

/// Every record on `topic` written by `producer`, decoded.
fn records<B: EventBody + DeserializeOwned>(platform: &Platform, producer: &str) -> Result<Vec<B>> {
    platform
        .event_log()
        .records()
        .iter()
        .filter(|record| {
            record.event.topic == B::TOPIC && record.event.lineage.producer == producer
        })
        .map(|record| {
            Ok(StreamEnvelope::from_frame(&record.event)?
                .decode::<B>()?
                .body)
        })
        .collect()
}

// --- W2.6: attribution with real P&L, keyed seven ways ----------------------

/// R12-MODEL-6003's audit finding was that attribution charged commissions
/// and nothing else: every position opened and closed at its own fill price.
/// This buys at the simulated venue, moves the tape to 110, and asserts the
/// fill is credited its marked move — and that it carries all seven keys,
/// stamped at acceptance.
///
/// Mutations that must fail it: restoring `closing_price = fill.price` in
/// `crate::attribution::periods` (the total becomes `-commission`); deleting
/// the `note_release` call in `Platform::submit_order` (the position is
/// unstamped and every key reads `unattributed`); dropping a dimension from
/// `AttributionKeys::values` (the seven-key loop fails).
#[test]
fn a_filled_order_is_attributed_its_marked_pnl_under_all_seven_keys() -> Result<()> {
    let mut platform = platform()?;
    platform.observe(quiet_bars("AAA", 30));
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        "prop-attributed",
        vec!["hyp-attributed".to_string()],
        start(),
    );
    let order_id = order.order_id.clone();
    platform.submit_order(order, start())?;
    let fill = platform
        .orders()
        .fills()
        .into_iter()
        .find(|fill| fill.order_id == order_id)
        .cloned()
        .ok_or_else(|| Error::not_found("premise: the accepted order did not fill"))?;

    platform.observe(flat_bars_after("AAA", start(), 3, 110.0));
    platform.run_cycle(start().saturating_add(Duration::from_days(5)));

    let pass = platform
        .last_attribution()
        .ok_or_else(|| Error::not_found("LEARN attributed nothing"))?;
    let position = pass
        .attribution
        .positions
        .iter()
        .find(|p| p.object_id == order_id.as_str())
        .ok_or_else(|| Error::not_found("the fill is missing from the attribution"))?;
    let expected = fill.quantity * (dec!("110") - fill.price) - fill.costs;
    assert_ne!(
        position.total, -fill.costs,
        "the attribution is commission-only again"
    );
    assert_eq!(position.total, expected, "the fill was not marked to 110");

    let keyed = pass
        .keyed
        .positions
        .iter()
        .find(|p| p.object_id == order_id.as_str())
        .ok_or_else(|| Error::not_found("the fill is missing from the keyed attribution"))?;
    assert!(
        keyed.stamped,
        "the accepted order was not stamped at release"
    );
    for dimension in Dimension::ALL {
        assert!(
            !keyed.keys.values(dimension).is_empty(),
            "{} carries no key",
            dimension.as_str()
        );
    }
    assert_eq!(
        keyed.keys.values(Dimension::Strategy),
        vec!["desk:platform"]
    );
    assert_eq!(keyed.keys.values(Dimension::Belief), vec!["hyp-attributed"]);
    assert_eq!(keyed.keys.values(Dimension::RiskRule), vec!["kernel-test"]);
    let path = keyed.keys.values(Dimension::ExecutionPath);
    assert!(
        path.len() == 1 && path[0].starts_with("desk/") && path[0].ends_with("/simulated/market"),
        "execution path {path:?}"
    );
    assert!(
        keyed.keys.values(Dimension::CapitalPolicy)[0].starts_with("survival(hedge="),
        "capital policy {:?}",
        keyed.keys.values(Dimension::CapitalPolicy)
    );
    // A hypothesis REASON never formed has no remembered forecasters: the
    // model and source keys say so rather than dropping the position.
    assert_eq!(keyed.keys.values(Dimension::Model), vec![UNATTRIBUTED]);

    let journaled = records::<AttributionRecorded>(&platform, ATTRIBUTION_ORIGIN)?;
    let last = journaled
        .last()
        .ok_or_else(|| Error::not_found("the keyed attribution was not journaled"))?;
    assert_eq!(
        last.by_dimension.len(),
        7,
        "a dimension is missing from the record"
    );
    assert_eq!(last.total_pnl, pass.attribution.total_pnl);
    Ok(())
}

// --- W3.3: the tournament through the cycle --------------------------------

/// R12-MODEL-7198 from a composition root's call: a cycle that forms a
/// hypothesis opens a question the detector and every informative analyst
/// answer independently, and a later cycle past the horizon resolves it and
/// scores every forecast. Mutation: deleting the `observe_hypothesis` call in
/// REASON opens no question; deleting the `tournament.learn` call in LEARN
/// leaves it open and journals no resolution.
#[test]
fn a_hypothesis_opens_a_question_many_forecasters_answer_and_a_later_cycle_scores_each()
-> Result<()> {
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());
    let question = platform
        .model_tournament()
        .open_questions()
        .next()
        .cloned()
        .ok_or_else(|| Error::not_found(format!("no question opened:\n{}", first.summarise())))?;
    let forecasters: Vec<String> = question.forecasts().keys().cloned().collect();
    assert!(
        forecasters.iter().any(|f| f.starts_with("detector:")),
        "the detector did not forecast: {forecasters:?}"
    );
    assert!(
        forecasters.len() >= 2,
        "one forecaster is not a tournament: {forecasters:?}"
    );
    let id = question.spec().id.clone();
    let resolves_at = question.spec().resolves_at;

    // A bar that closes past the horizon, ten percent above anything seen.
    platform.observe(vec![bar("AAA", resolves_at, 150.0, 165.0)]);
    platform.run_cycle(resolves_at.saturating_add(Duration::from_days(2)));

    assert!(
        platform.model_tournament().question(&id).is_none(),
        "the question is still open past its horizon"
    );
    let reviews = records::<TournamentReview>(&platform, TOURNAMENT_ORIGIN)?;
    let resolved = reviews
        .iter()
        .flat_map(|review| review.resolved.iter())
        .find(|resolved| resolved.question == id)
        .ok_or_else(|| Error::not_found("the resolution was not journaled"))?;
    assert_eq!(resolved.outcome, "up");
    assert_eq!(
        resolved.scores.len(),
        forecasters.len(),
        "a forecast went unscored"
    );
    for forecaster in &forecasters {
        assert!(
            !platform
                .model_tournament()
                .scorecards(forecaster)
                .is_empty(),
            "{forecaster} has no scorecard"
        );
    }
    Ok(())
}

/// R12-MODEL-6018: every forecaster on a question states quantiles over the
/// realised move, so the scorecard a later cycle writes measures each one's
/// quantile accuracy, not only its direction. Until 2026-10-18 the stage
/// submitted `quantiles: None` for the detector and every analyst alike, and
/// `Scorecard::quantile_loss` read "no forecast here stated quantiles" for
/// every forecaster the platform had ever scored.
///
/// Mutation: submit `quantiles: None` for the detector in
/// `TournamentStage::observe_reasoning` — the detector's quantile loss reads
/// unmeasured and the assertion naming it fails.
#[test]
fn every_forecaster_a_cycle_scored_has_its_quantile_loss_measured() -> Result<()> {
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());
    let question = platform
        .model_tournament()
        .open_questions()
        .next()
        .cloned()
        .ok_or_else(|| Error::not_found(format!("no question opened:\n{}", first.summarise())))?;
    let forecasters: Vec<String> = question.forecasts().keys().cloned().collect();
    assert!(
        forecasters.iter().any(|f| f.starts_with("detector:"))
            && forecasters.iter().any(|f| f.starts_with("analyst:")),
        "premise: a detector and an analyst both forecast: {forecasters:?}"
    );
    let resolves_at = question.spec().resolves_at;
    platform.observe(vec![bar("AAA", resolves_at, 150.0, 165.0)]);
    platform.run_cycle(resolves_at.saturating_add(Duration::from_days(2)));

    for forecaster in &forecasters {
        let cards = platform.model_tournament().scorecards(forecaster);
        assert!(!cards.is_empty(), "premise: {forecaster} was scored");
        for card in cards {
            let loss = card.quantile_loss.value().unwrap_or_else(|| {
                panic!(
                    "{forecaster}'s quantile accuracy was not tracked: {:?}",
                    card.quantile_loss
                )
            });
            assert!(
                loss.is_finite() && loss >= 0.0,
                "{forecaster}'s pinball loss is {loss}"
            );
        }
    }
    Ok(())
}

// --- W3.6: the society blocks a promotion ------------------------------------

const DESK: &str = "tournament-tests/model-desk";

fn platform_serving() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new_serving(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
        Box::new(InTreeProvider),
    )?;
    platform.name_model_desk(DESK)?;
    Ok(platform)
}

/// `y = 0.5 a − 0.25 b + c` with a small residual: a fit that clears the
/// skill bar honestly, so a refusal is about the promotion.
fn teacher(name: &str) -> Result<TrainedTeacher> {
    let mut rows = Vec::new();
    let mut targets = Vec::new();
    let mut times = Vec::new();
    for i in 0..200usize {
        let a = (i as f64 * 0.37).sin();
        let b = (i as f64 * 0.11).cos();
        let residual = ((i * 7919) % 13) as f64 / 13.0 * 0.02 - 0.01;
        rows.push(vec![a, b]);
        targets.push(0.5 * a - 0.25 * b + 0.1 + residual);
        times.push(start().saturating_add(Duration::from_mins(i as i64)));
    }
    let data = TrainingDataset::new(
        format!("{name}-data"),
        vec!["a".to_string(), "b".to_string()],
        rows,
        targets,
        times,
    )?;
    let spec = TrainingSpec::new(
        name,
        "0.1.0",
        "tournament-tests",
        data.name(),
        ModelFamily::Linear { ridge: 1e-6 },
    )
    .with_holdout(0.25);
    LocalTrainer::new().fit(&spec, &data, start())
}

/// R12-TICK-2091: a distillate whose card was trained on licensed data is
/// named in the `trained_models` slot only for a location its licence
/// permits. `ModelCard::authorise_location` was called for the opportunity
/// map alone, so a promoted model trained under a London-only licence was
/// named to every cell the centre served. The licence now rides on the
/// journaled promotion, read off the card, and the manifest withholds it
/// elsewhere — including from a cell whose location was never declared.
///
/// Mutation: drop the licence check in `Platform::model_manifest_for` — the
/// manifest for `us-east4` names the distillate and the refusal assertion
/// fails.
#[test]
fn a_manifest_for_a_region_the_card_does_not_permit_is_refused() -> Result<()> {
    use std::collections::BTreeSet;
    let teacher = teacher("bar-linear-licensed")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let student =
        qip_strategy::model::DistilledModel::linear("bar-linear-licensed", 0.1, vec![0.5, -0.25])?;
    let mut platform = platform_serving()?;
    let mut registry = ModelRegistry::new();
    let registration = register_fit(
        &mut registry,
        &teacher,
        &SkillPolicy::default(),
        "tournament-tests",
        start(),
    )?;
    assert!(
        registration.passed,
        "premise: the fit cleared the skill bar"
    );
    let card = registry
        .get_mut(&registration.reference)
        .ok_or_else(|| Error::not_found("the card"))?;
    *card = card.clone().with_data_entitlements(
        BTreeSet::from(["vendor-ticks-uk".to_string()]),
        BTreeSet::from(["europe-west2".to_string()]),
    )?;
    platform.promote_model(&mut registry, &artifact, Some(&student), &[], start())?;
    let promotion = platform
        .model_promotions()
        .values()
        .next()
        .cloned()
        .ok_or_else(|| Error::not_found("premise: the promotion was recorded"))?;
    assert_eq!(
        promotion.permitted_locations,
        Some(BTreeSet::from(["europe-west2".to_string()])),
        "the licence was not carried onto the journaled promotion"
    );

    let permitted = platform.model_manifest_for(Some("europe-west2"))?;
    assert!(
        permitted
            .manifest()
            .models
            .contains_key("bar-linear-licensed"),
        "premise: the permitted location is told about the distillate: {}",
        permitted.describe()
    );
    let refused = platform
        .model_manifest_for(Some("us-east4"))
        .expect_err("a manifest for a location the licence refuses named the distillate");
    assert!(
        matches!(refused, Error::Denied(_)),
        "refused for another reason: {refused:?}"
    );
    platform
        .model_manifest()
        .expect_err("a cell with no declared location was named a licensed distillate");
    Ok(())
}

/// R12-MODEL-7201: a model whose only evaluation has lapsed passes the
/// registry (which asks for a passing evaluation, not a fresh one) and is
/// refused by the society's model-risk critic, with the verdict journaled;
/// the same model promoted while its evaluation is fresh goes through.
/// Mutation: deleting the society gate in `promote_model_from` promotes the
/// stale model; making the model-risk critic ignore the validity window does
/// too.
#[test]
fn a_model_whose_evaluation_has_lapsed_is_refused_by_the_verifier_society_and_the_verdict_is_journaled()
-> Result<()> {
    let teacher = teacher("bar-linear-stale")?;
    let artifact = InTreeProvider::pack(&teacher)?;

    // Premise: fresh, the promotion goes through and the verdict is on the log.
    let mut fresh = platform_serving()?;
    let mut registry = ModelRegistry::new();
    let registration = register_fit(
        &mut registry,
        &teacher,
        &SkillPolicy::default(),
        "tournament-tests",
        start(),
    )?;
    assert!(
        registration.passed,
        "premise: the fit cleared the skill bar"
    );
    fresh.promote_model(&mut registry, &artifact, None, &[], start())?;
    let verdicts = records::<ModelVerified>(&fresh, VERIFIER_ORIGIN)?;
    assert_eq!(verdicts.len(), 1, "the society's verdict was not journaled");
    assert!(!verdicts[0].verification.verdict.is_refuted());

    // Lapsed: past the card's own validity window.
    let mut stale = platform_serving()?;
    let mut registry = ModelRegistry::new();
    let registration = register_fit(
        &mut registry,
        &teacher,
        &SkillPolicy::default(),
        "tournament-tests",
        start(),
    )?;
    let card = registry
        .get(&registration.reference)
        .ok_or_else(|| Error::not_found("the card"))?;
    let late = start()
        .saturating_add(card.evaluation_validity)
        .saturating_add(Duration::from_days(1));
    let error = stale
        .promote_model(&mut registry, &artifact, None, &[], late)
        .expect_err("a model with a lapsed evaluation was promoted");
    assert!(
        error.message().contains("model_risk"),
        "refused for another reason: {}",
        error.message()
    );
    assert_eq!(
        registry.get(&registration.reference).map(|card| card.stage),
        Some(ModelStage::Development),
        "the registry moved although the promotion was refused"
    );
    assert!(stale.model_promotions().is_empty());
    let verdicts = records::<ModelVerified>(&stale, VERIFIER_ORIGIN)?;
    assert_eq!(verdicts.len(), 1);
    match &verdicts[0].verification.verdict {
        SocietyVerdict::Refuted { by, .. } => assert!(by.contains(&CriticKind::ModelRisk)),
        other => panic!("the journaled verdict is {other:?}"),
    }
    Ok(())
}

/// The record types are on the topics their readers filter on.
#[test]
fn the_tournament_and_verifier_records_sit_on_their_own_topics() {
    assert_eq!(TournamentReview::TOPIC, Topic::HypothesisScored);
    assert_eq!(ModelVerified::TOPIC, Topic::HypothesisChallenged);
    assert_eq!(AttributionRecorded::TOPIC, Topic::OutcomeObserved);
}
