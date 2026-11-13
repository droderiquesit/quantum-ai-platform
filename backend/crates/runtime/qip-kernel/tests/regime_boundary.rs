//! Blueprint §9.4's first handling, end to end: "edges carry a confidence.
//! Low-confidence edges inform exploration, not sizing."
//!
//! The confidence half has shipped for a long while. The exploration half had
//! no feed at all: `ProbeKind::RegimeBoundary` — "enter a regime-boundary
//! trade, to learn how the causal edges behave at the transition" — was
//! declared and left unfed, and `qip_kernel::exploration`'s own module doc
//! said why. `Platform::regime_label` names the regime **in force**, never the
//! instant it changed, so a `RegimeBoundary` candidate would have been "a
//! probe sized against a figure nobody computed".
//!
//! This drives the whole path through the cycle rather than asserting about
//! the marker in isolation: a real temporal-precedence edge is written under
//! one regime, the instrument's quoted spread then blows out, and the DECIDE
//! stage's exploration desk is asked whether it funded a probe of the right
//! kind on the right subject. Both halves of the gate are proven — the pass
//! before the change funds no such probe, and the pass after it does —
//! because a feed that fires on every pass is indistinguishable from a
//! working one when only the firing side is checked.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_kernel::regime_transition::SUBJECT_PREFIX;
use qip_market::bar::{Bar, Interval};
use qip_market::quote::Quote;
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_world_model::causal::ConditionStanding;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// ADR 0392: run the four promotion tests on `teacher`'s artifact over a
/// recorded window of the two features every fixture here is fitted on —
/// sixty later rows of the same process — and assert the premise that it
/// passed them, so a refusal further on is about the promotion.
fn tested(
    registry: &mut qip_ai::registry::ModelRegistry,
    teacher: &qip_training::local::TrainedTeacher,
) -> qip_core::error::Result<()> {
    use qip_training::serve::InTreeProvider;
    let mut rows = Vec::new();
    let mut targets = Vec::new();
    let mut times = Vec::new();
    for i in 200..260usize {
        let a = (i as f64 * 0.37).sin();
        let b = (i as f64 * 0.11).cos();
        let residual = ((i * 7919) % 13) as f64 / 13.0 * 0.02 - 0.01;
        rows.push(vec![a, b]);
        targets.push(0.5 * a - 0.25 * b + 0.1 + residual);
        times.push(start().saturating_add(qip_core::Duration::from_mins(i as i64)));
    }
    let recorded = qip_training::dataset::TrainingDataset::new(
        format!("{}-recorded", teacher.name()),
        teacher.feature_names().to_vec(),
        rows,
        targets,
        times,
    )?;
    let artifact = InTreeProvider::pack(teacher)?;
    let tests = qip_kernel::central::models::run_promotion_tests(
        &InTreeProvider,
        registry,
        &artifact,
        &recorded,
        start(),
    )?;
    assert_eq!(
        tests.gap(&artifact.digest),
        None,
        "premise: the fit passed its four promotion tests"
    );
    // ADR 0436 (R12-MODEL-4022): a training-pipeline candidate is promoted
    // only out of a completed simulation or paper trial, dated before every
    // promotion these tests make, so a refusal further on is not about it.
    if registry
        .get(&artifact.reference)
        .is_some_and(|card| card.trial.is_none())
    {
        for round in 1..=i64::from(qip_ai::registry::MIN_TRIAL_ROUNDS) {
            registry.record_trial_round(
                &artifact.reference,
                qip_ai::registry::TrialVenue::Paper,
                "staged by the test",
                start().saturating_sub(qip_core::Duration::from_secs(
                    i64::from(qip_ai::registry::MIN_TRIAL_ROUNDS) + 1 - round,
                )),
            )?;
        }
    }
    Ok(())
}

fn object(symbol: &str) -> qip_core::ObjectId {
    qip_core::ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe(symbols: &[&str]) -> Universe {
    let mut universe = Universe::new();
    for symbol in symbols {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    *symbol,
                    InstrumentType::CommonStock,
                    qip_financial::costs::LiquidityProfile::listed(
                        Decimal::from_int(5_000_000),
                        3.0,
                    ),
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

fn platform(symbols: &[&str]) -> Result<Platform> {
    // A share is set aside deliberately: with none, the desk reports "no
    // budget" and every candidate — right or wrong — is unfundable, so the
    // assertion below would pass on a platform that never computed a
    // boundary at all.
    let config = PlatformConfig::default().with_exploration_share(dec!("0.05"));
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(symbols),
        LimitSet::new("regime-boundary-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        ),
    )
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

fn bars_from_returns(symbol: &str, returns: &[f64], count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let open = price;
            price *= returns[i].exp();
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// A deterministic, bounded pseudo-random sequence — the same fixture shape
/// `causal_precedence.rs` uses, so the edge this test relies on is written by
/// the same production pass and not by a synthetic seed.
fn noise(seed: u64, count: usize, scale: f64) -> Vec<f64> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let unit = (state % 1_000_000) as f64 / 1_000_000.0;
            (unit - 0.5) * 2.0 * scale
        })
        .collect()
}

/// A quote whose half-spread is `bps` basis points of a hundred-dollar mid.
fn quote(symbol: &str, at: Timestamp, bps: f64) -> SensedRecord {
    let half = 100.0 * bps / 20_000.0;
    SensedRecord::Quote(Quote {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        at,
        capture_time: at,
        sequence: None,
        bid: Decimal::from_f64(100.0 - half).expect("a price"),
        ask: Decimal::from_f64(100.0 + half).expect("a price"),
        bid_size: dec!("1000"),
        ask_size: dec!("1000"),
        quality: DataQuality::default(),
    })
}

/// The exploration clause of a report's DECIDE stage.
fn decide_detail(report: &qip_kernel::cycle::CycleReport) -> String {
    let decide = report
        .stage(Stage::Decide)
        .expect("DECIDE runs on every cycle");
    assert!(decide.ran, "DECIDE did not run: {decide:?}");
    decide.detail.clone()
}

#[test]
fn a_subject_whose_regime_turns_becomes_the_regime_boundary_probe_the_desk_funds() -> Result<()> {
    let count = 120;
    let cause_returns = noise(11, count, 0.02);
    let mut effect_returns = vec![0.0; count];
    let effect_noise = noise(22, count, 0.004);
    for t in 1..count {
        effect_returns[t] = 0.8 * cause_returns[t - 1] + effect_noise[t];
    }

    let mut platform = platform(&["AAA", "BBB"])?;
    platform.observe(bars_from_returns("AAA", &cause_returns, count));
    platform.observe(bars_from_returns("BBB", &effect_returns, count));
    // Three tight quotes, so the median the spread ratio divides by is a
    // tight one. Fewer than three and `spread_ratio` answers `None` and the
    // regime could never turn on liquidity at all.
    for day in 0..3 {
        platform.observe(vec![quote(
            "BBB",
            start().saturating_sub(Duration::from_days(3 - day)),
            10.0,
        )]);
    }

    let first = platform.run_cycle(start());
    let settled_regime = platform.regime_context("obj-BBB");

    // Premise one: the production pass wrote an edge into the subject, and
    // conditioned it on the regime then in force. Without an edge there is
    // nothing for a regime-boundary probe to be uncertain about, and the
    // assertion below would be vacuous.
    let edges = platform.world().causal().edges().to_vec();
    let into_subject: Vec<_> = edges.iter().filter(|e| e.effect == "obj-BBB").collect();
    assert!(
        !into_subject.is_empty(),
        "premise: the temporal-precedence pass wrote an edge into obj-BBB, among {edges:?}"
    );
    assert!(
        into_subject
            .iter()
            .all(|e| e.in_regime(&settled_regime) == ConditionStanding::Holds),
        "premise: every edge into the subject was tested in the regime in force ({settled_regime})"
    );

    // Premise two, and the half a one-sided test would miss: a platform whose
    // regime has not turned funds no regime-boundary probe. A feed that
    // proposed one on every pass would satisfy the final assertion and would
    // be worthless.
    let quiet = decide_detail(&first);
    assert!(
        !quiet.contains("regime_boundary on"),
        "a probe of the boundary kind was funded on a pass where nothing crossed: {quiet}"
    );

    // The turn: one quote three hundred basis points wide against a
    // ten-basis-point median. The subject's market regime becomes illiquid,
    // which is a regime this subject's edges have never been tested under.
    platform.observe(vec![quote("BBB", start(), 300.0)]);
    let later = start().saturating_add(Duration::from_secs(60));

    let second = platform.run_cycle(later);
    let turned_regime = platform.regime_context("obj-BBB");
    assert_ne!(
        settled_regime, turned_regime,
        "premise: the subject's regime key actually changed; without that there is no boundary \
         and this test proves nothing"
    );

    let understood = second
        .stage(Stage::Understand)
        .expect("UNDERSTAND runs on every cycle");
    assert!(
        understood
            .detail
            .contains("crossed a regime boundary this pass"),
        "the UNDERSTAND stage did not report the crossing it marked: {}",
        understood.detail
    );

    // And the whole point: the desk funded a probe of the boundary kind, on
    // the subject that crossed. The subject is matched in full, prefix and
    // all, rather than by a bare instrument id — the plan line names every
    // selected probe, and `contains("obj-BBB")` would be true of a capacity
    // probe on the same instrument.
    let turned = decide_detail(&second);
    let expected = format!("regime_boundary on {SUBJECT_PREFIX}obj-BBB");
    assert!(
        turned.contains(&expected),
        "the desk funded no {expected}; §9.4's exploration handling is unfed again: {turned}"
    );
    Ok(())
}

/// `y = 0.5 a − 0.25 b + c` with a small residual: a fit that clears the skill
/// bar honestly, so any refusal below is about the market and not the model.
fn teacher(name: &str) -> Result<qip_training::local::TrainedTeacher> {
    use qip_training::dataset::TrainingDataset;
    use qip_training::job::TrainingSpec;
    use qip_training::local::{LocalTrainer, ModelFamily};
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
        "regime-tests",
        data.name(),
        ModelFamily::Linear { ridge: 1e-6 },
    )
    .with_holdout(0.25);
    LocalTrainer::new().fit(&spec, &data, start())
}

/// A serving platform whose subject `obj-BBB` has had an edge written under a
/// tight regime; with `crossed`, its quoted spread then blows out and the
/// cycle that sees it marks the crossing at `start() + 60s`.
fn serving_platform_after(crossed: bool) -> Result<Platform> {
    let count = 120;
    let cause_returns = noise(11, count, 0.02);
    let mut effect_returns = vec![0.0; count];
    let effect_noise = noise(22, count, 0.004);
    for t in 1..count {
        effect_returns[t] = 0.8 * cause_returns[t - 1] + effect_noise[t];
    }
    let config = PlatformConfig::default().with_exploration_share(dec!("0.05"));
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new_serving(
        config,
        context,
        Telemetry::silent(),
        universe(&["AAA", "BBB"]),
        LimitSet::new("regime-boundary-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        ),
        Box::new(qip_training::serve::InTreeProvider),
    )?;
    platform.name_model_desk("regime-tests/model-desk")?;
    platform.observe(bars_from_returns("AAA", &cause_returns, count));
    platform.observe(bars_from_returns("BBB", &effect_returns, count));
    for day in 0..3 {
        platform.observe(vec![quote(
            "BBB",
            start().saturating_sub(Duration::from_days(3 - day)),
            10.0,
        )]);
    }
    platform.run_cycle(start());
    if crossed {
        let settled = platform.regime_context("obj-BBB");
        platform.observe(vec![quote("BBB", start(), 300.0)]);
        platform.run_cycle(start().saturating_add(Duration::from_secs(60)));
        assert_ne!(
            settled,
            platform.regime_context("obj-BBB"),
            "premise: the subject's regime key changed, so a crossing was marked"
        );
    }
    Ok(platform)
}

/// R12-TICK-2071, ADR 0149 decision on regime drift: the platform measures a
/// regime change as an event (`Platform::latest_regime_crossing`) that the
/// verifier society reads when a model is promoted, not as a second drift
/// column over the same prices. A model last evaluated before the market
/// crossed a boundary was evaluated in a market that no longer exists and is
/// refused by the structural-break critic; the same model with no crossing
/// since, or evaluated after it, is promoted. A gate that refused every
/// promotion would also pass the first half, so both halves are asserted.
///
/// Mutation: make `Platform::latest_regime_crossing` return `None` — the model
/// evaluated before the crossing is promoted and the refusal assertion fails.
#[test]
fn a_model_evaluated_before_the_market_crossed_a_regime_boundary_is_refused_promotion() -> Result<()>
{
    use qip_ai::registry::ModelRegistry;
    use qip_kernel::central::models::register_fit;
    use qip_training::local::SkillPolicy;
    use qip_training::serve::InTreeProvider;

    let teacher = teacher("regime-linear")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let promote = |crossed: bool, evaluated: Timestamp, at: Timestamp| -> Result<Result<()>> {
        let mut platform = serving_platform_after(crossed)?;
        let mut registry = ModelRegistry::new();
        let registration = register_fit(
            &mut registry,
            &teacher,
            &SkillPolicy::default(),
            "regime-tests",
            evaluated,
        )?;
        tested(&mut registry, &teacher)?;
        assert!(
            registration.passed,
            "premise: the fit cleared the skill bar"
        );
        Ok(platform
            .promote_model(&mut registry, &artifact, None, &[], at)
            .map(|_| ()))
    };

    let quiet = promote(
        false,
        start(),
        start().saturating_add(Duration::from_secs(120)),
    )?;
    assert!(
        quiet.is_ok(),
        "premise: with no crossing the model promotes: {quiet:?}"
    );

    let refused = promote(
        true,
        start(),
        start().saturating_add(Duration::from_secs(120)),
    )?
    .expect_err("a model evaluated before a regime crossing was promoted");
    assert!(
        refused.message().contains("structural_break"),
        "refused for another reason: {}",
        refused.message()
    );

    let after = start().saturating_add(Duration::from_secs(3_600));
    let reevaluated = promote(true, after, after.saturating_add(Duration::from_secs(60)))?;
    assert!(
        reevaluated.is_ok(),
        "a model evaluated after the crossing was still refused: {reevaluated:?}"
    );
    Ok(())
}

/// R12-MODEL-10096: the structural-break critic judges a model against its
/// own subject's regime crossings. `obj-BBB`'s spread blows out after the
/// model was evaluated; a model fitted on `obj-AAA`, whose market did not
/// move, is still promoted, and the same model fitted on `obj-BBB` is
/// refused. Before this, the critic read the all-subject latest crossing and
/// so refused every model whenever any watched subject crossed.
///
/// Mutation: make `Platform::regime_crossing_for` return the all-subject
/// `latest_regime_crossing()` whatever the card names — the `obj-AAA` model
/// is refused and the first assertion fails.
#[test]
fn a_model_is_judged_against_its_own_subjects_regime_crossing_and_not_another_subjects()
-> Result<()> {
    use qip_ai::registry::ModelRegistry;
    use qip_kernel::central::models::{record_subject, register_fit, subject_of};
    use qip_training::local::SkillPolicy;
    use qip_training::serve::InTreeProvider;

    let teacher = teacher("regime-subject-linear")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let promote = |subject: &str| -> Result<Result<()>> {
        let mut platform = serving_platform_after(true)?;
        let mut registry = ModelRegistry::new();
        let registration = register_fit(
            &mut registry,
            &teacher,
            &SkillPolicy::default(),
            "regime-tests",
            start(),
        )?;
        tested(&mut registry, &teacher)?;
        assert!(
            registration.passed,
            "premise: the fit cleared the skill bar"
        );
        let card = registry
            .get_mut(&registration.reference)
            .unwrap_or_else(|| panic!("premise: the card is registered"));
        record_subject(card, subject)?;
        assert_eq!(
            subject_of(card),
            Some(subject),
            "premise: the subject is on the card"
        );
        Ok(platform
            .promote_model(
                &mut registry,
                &artifact,
                None,
                &[],
                start().saturating_add(Duration::from_secs(120)),
            )
            .map(|_| ()))
    };

    let untouched = promote("obj-AAA")?;
    assert!(
        untouched.is_ok(),
        "a model on a subject that did not cross was refused for another subject's crossing: \
         {untouched:?}"
    );
    let crossed = promote("obj-BBB")?
        .expect_err("a model on the subject that crossed after its evaluation was promoted");
    assert!(
        crossed.message().contains("structural_break"),
        "refused for another reason: {}",
        crossed.message()
    );
    Ok(())
}

/// R12-MODEL-10096: a production card is due a renewed evaluation exactly
/// when its own subject crossed a regime boundary after it was last
/// evaluated, and stops being due once it is evaluated again. Without the
/// second half a desk that renewed an evaluation would be asked again every
/// round, which is how a re-verification built on 2026-10-30 came to refute
/// every production model for ever.
///
/// Mutations: return `None` unconditionally from `Platform::reevaluation_due`
/// — the first assertion fails; drop the `evaluated_at >= crossing` arm so a
/// crossing is always due — the last assertion fails.
#[test]
fn a_card_is_due_a_renewed_evaluation_only_until_it_is_evaluated_after_its_subjects_crossing()
-> Result<()> {
    use qip_ai::registry::{EvaluationRecord, ModelRegistry};
    use qip_kernel::central::models::{record_subject, register_fit};
    use qip_training::local::SkillPolicy;

    let platform = serving_platform_after(true)?;
    let crossing = start().saturating_add(Duration::from_secs(60));
    let teacher = teacher("regime-due-linear")?;
    let card_on = |subject: &str| -> Result<(ModelRegistry, String)> {
        let mut registry = ModelRegistry::new();
        let registration = register_fit(
            &mut registry,
            &teacher,
            &SkillPolicy::default(),
            "regime-tests",
            start(),
        )?;
        tested(&mut registry, &teacher)?;
        let card = registry
            .get_mut(&registration.reference)
            .unwrap_or_else(|| panic!("premise: the card is registered"));
        record_subject(card, subject)?;
        Ok((registry, registration.reference))
    };

    let (mut crossed, reference) = card_on("obj-BBB")?;
    let card = crossed.get(&reference).expect("the card");
    assert_eq!(platform.reevaluation_due(card), Some(crossing));

    let (untouched, other) = card_on("obj-AAA")?;
    assert_eq!(
        platform.reevaluation_due(untouched.get(&other).expect("the card")),
        None,
        "a card on a subject that did not cross is due nothing"
    );

    crossed.record_evaluation(
        &reference,
        EvaluationRecord {
            evaluated_at: crossing.saturating_add(Duration::from_secs(1)),
            dataset: "after-the-crossing".to_string(),
            metrics: std::collections::BTreeMap::new(),
            passed: true,
        },
    )?;
    assert_eq!(
        platform.reevaluation_due(crossed.get(&reference).expect("the card")),
        None,
        "a card evaluated after the crossing is still due"
    );
    Ok(())
}

/// R12-MODEL-10096: re-verifying a production model puts it in front of the
/// same society a promotion faces and journals the verdict whichever way it
/// goes. A model that stands returns no refutation; the same model with a
/// failed evaluation recorded since is refuted in the society's words; a
/// reference that is not in production is refused rather than judged.
///
/// Mutations: skip `journal_record` in `Platform::reverify_model` — the
/// journal count assertions fail; return `Ok(None)` whatever the verdict —
/// the refutation assertion fails; drop the promoted-set check — the
/// unpromoted reference is judged and the last assertion fails.
#[test]
fn re_verifying_a_production_model_journals_the_verdict_and_reports_a_refutation() -> Result<()> {
    use qip_ai::registry::{EvaluationRecord, ModelRegistry};
    use qip_events::EventBody;
    use qip_kernel::central::models::{record_subject, register_fit};
    use qip_kernel::model_tournament::{ModelVerified, VERIFIER_ORIGIN};
    use qip_training::local::SkillPolicy;
    use qip_training::serve::InTreeProvider;

    let verdicts = |platform: &Platform| -> usize {
        platform
            .event_log()
            .records()
            .iter()
            .filter(|record| {
                record.event.topic == ModelVerified::TOPIC
                    && record.event.lineage.producer == VERIFIER_ORIGIN
            })
            .count()
    };

    let mut platform = serving_platform_after(true)?;
    let teacher = teacher("regime-reverify-linear")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let mut registry = ModelRegistry::new();
    // Registered after the crossing, so the promotion itself is not refused.
    let registered_at = start().saturating_add(Duration::from_secs(3_600));
    let registration = register_fit(
        &mut registry,
        &teacher,
        &SkillPolicy::default(),
        "regime-tests",
        registered_at,
    )?;
    tested(&mut registry, &teacher)?;
    let reference = registration.reference.clone();
    record_subject(registry.get_mut(&reference).expect("the card"), "obj-BBB")?;

    let unpromoted = platform
        .reverify_model(&registry, &reference, registered_at)
        .expect_err("a model that is not in production was re-verified");
    assert!(
        unpromoted.message().contains("not in the promoted set"),
        "refused for another reason: {}",
        unpromoted.message()
    );

    platform.promote_model(
        &mut registry,
        &artifact,
        None,
        &[],
        registered_at.saturating_add(Duration::from_secs(60)),
    )?;
    let after_promotion = verdicts(&platform);
    assert!(after_promotion >= 1, "premise: the promotion was verified");

    let later = registered_at.saturating_add(Duration::from_secs(120));
    assert_eq!(platform.reverify_model(&registry, &reference, later)?, None);
    assert_eq!(verdicts(&platform), after_promotion + 1);

    registry.record_evaluation(
        &reference,
        EvaluationRecord {
            evaluated_at: later,
            dataset: "rows-after-the-crossing".to_string(),
            metrics: std::collections::BTreeMap::new(),
            passed: false,
        },
    )?;
    let refuted = platform
        .reverify_model(
            &registry,
            &reference,
            later.saturating_add(Duration::from_secs(1)),
        )?
        .expect("a model whose renewed evaluation failed was left standing");
    assert!(refuted.contains("did not pass"), "{refuted}");
    assert_eq!(verdicts(&platform), after_promotion + 2);
    Ok(())
}
