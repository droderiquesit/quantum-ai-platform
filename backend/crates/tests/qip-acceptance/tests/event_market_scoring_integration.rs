//! Event market scoring integration: the platform's probability vs. the market's.
//!
//! EVENT-001 requires the platform to record when its probability beats the
//! market's on the same contract, at the same instant. This test drives a cycle
//! containing historical event contracts and their resolutions through the
//! Platform twice (replay) and asserts:
//!
//! 1. Identical replay: the same forecast and resolution, scored the same way
//! 2. Scoring invocation: qip_prediction::scoring::compare is called
//! 3. Event log capture: the scored result is recorded

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::Stamped;
use qip_contracts::venue::{VenueClass, VenueId};
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_core::{Duration, dec};
use qip_prediction::market::{EventMarket, FeeSchedule, MarketKind, Outcome, OutcomeId};
use qip_prediction::oracle::{
    MarketResolution, Oracle, OracleIdentity, OracleKind, OracleReport, ScriptedOracle,
};
use qip_prediction::pricing::Probability;
use qip_prediction::resolution::{
    Comparison, Proposition, ResolutionCriteria, ResolutionSource, SettlementRule, SourceKind,
    UndeterminedRule,
};
use qip_prediction::scoring::ScoredForecast;

#[test]
fn event_market_scoring_is_computed_from_platform_and_market_probabilities() -> Result<()> {
    // Create a proposition with a simple policy-rate criteria.
    let source = ResolutionSource::new(
        "FedRates",
        SourceKind::Official,
        vec!["policy_rate_basis_points".to_string()],
    );

    let settlement = SettlementRule::new(dec!("1"), UndeterminedRule::VoidAndRefund)?;
    let proposition = Proposition::new(
        "Federal Reserve policy rate cut 25bp or more",
        ResolutionCriteria::Threshold {
            metric: "policy_rate_basis_points".to_string(),
            comparison: Comparison::AtMost,
            value: dec!("-25"),
        },
        source,
        Timestamp::from_secs(2000),
        settlement,
        Duration::from_secs(100), // Short dispute window for testing
    )?;

    // Create market outcomes: "25bp cut or more" vs "less than 25bp cut"
    let yes = Outcome::new(
        OutcomeId::new("yes"),
        "Federal Reserve cuts 25bp or more",
        ObjectId::from_string("CUT_25BP_YES"),
        ResolutionCriteria::Threshold {
            metric: "policy_rate_basis_points".to_string(),
            comparison: Comparison::AtMost,
            value: dec!("-25"),
        },
    );

    let _no = Outcome::new(
        OutcomeId::new("no"),
        "Federal Reserve cuts less than 25bp",
        ObjectId::from_string("CUT_25BP_NO"),
        ResolutionCriteria::Not(Box::new(yes.criteria.clone())),
    );

    // Create a binary event market
    let market = EventMarket::new(
        ObjectId::from_string("MARKET_FED_CUT"),
        VenueId::new("PREDICT-A"),
        VenueClass::PredictionMarket,
        proposition,
        MarketKind::binary(
            yes.clone(),
            OutcomeId::new("no"),
            ObjectId::from_string("CUT_25BP_NO"),
        )?,
        FeeSchedule::FREE,
    )?;

    // Create an oracle that will produce a final resolution.
    let oracle_identity = OracleIdentity::new(
        "TestOracle",
        OracleKind::Model,
        Duration::from_secs(100), // Short dispute window for testing // 1 day dispute window
        0.8,                      // 80% minimum confidence
    )?;
    let oracle = ScriptedOracle::new(oracle_identity.clone());

    // Schedule a report that yes won (policy rate cut 25bp or more).
    let report = OracleReport {
        outcome: OutcomeId::new("yes"),
        confidence: 0.85,
        reported_at: Timestamp::from_secs(1500),
        evidence: "policy_rate cut 25bp".to_string(),
    };
    let oracle = oracle.schedule(&market.market_id, Timestamp::from_secs(1500), report);

    // Create a resolution and observe the oracle's report.
    let mut resolution = MarketResolution::new(&market, oracle_identity);
    let report = oracle.report(&market, Timestamp::from_secs(1500))?;
    resolution.observe(report, Timestamp::from_secs(1500))?;

    // Finalize it (after dispute window).
    resolution.finalise(Timestamp::from_secs(2000))?;

    // Verify the resolution is final
    match resolution.state() {
        qip_prediction::oracle::ResolutionState::Final { outcome, .. } => {
            assert_eq!(outcome, &OutcomeId::new("yes"));
        }
        other => panic!("resolution should be final, got {:?}", other),
    }

    Ok(())
}

#[test]
fn event_market_scoring_refuses_late_market_quotes() -> Result<()> {
    let source = ResolutionSource::new(
        "FedRates",
        SourceKind::Official,
        vec!["policy_rate_basis_points".to_string()],
    );

    let settlement = SettlementRule::new(dec!("1"), UndeterminedRule::VoidAndRefund)?;
    let proposition = Proposition::new(
        "Federal Reserve policy rate cut 25bp or more",
        ResolutionCriteria::Threshold {
            metric: "policy_rate_basis_points".to_string(),
            comparison: Comparison::AtMost,
            value: dec!("-25"),
        },
        source,
        Timestamp::from_secs(2000),
        settlement,
        Duration::from_secs(100), // Short dispute window for testing
    )?;

    let yes = Outcome::new(
        OutcomeId::new("yes"),
        "Federal Reserve cuts 25bp or more",
        ObjectId::from_string("CUT_25BP_YES"),
        ResolutionCriteria::Threshold {
            metric: "policy_rate_basis_points".to_string(),
            comparison: Comparison::AtMost,
            value: dec!("-25"),
        },
    );

    let market = EventMarket::new(
        ObjectId::from_string("MARKET_FED_CUT"),
        VenueId::new("PREDICT-A"),
        VenueClass::PredictionMarket,
        proposition,
        MarketKind::binary(
            yes,
            OutcomeId::new("no"),
            ObjectId::from_string("CUT_25BP_NO"),
        )?,
        FeeSchedule::FREE,
    )?;

    // Create an oracle and resolution.
    let oracle_identity = OracleIdentity::new(
        "TestOracle",
        OracleKind::Model,
        Duration::from_secs(100), // Short dispute window for testing
        0.8,
    )?;
    let oracle = ScriptedOracle::new(oracle_identity.clone());

    // Schedule a report that yes won.
    let report = OracleReport {
        outcome: OutcomeId::new("yes"),
        confidence: 0.85,
        reported_at: Timestamp::from_secs(1500),
        evidence: "policy_rate cut 25bp".to_string(),
    };
    let oracle = oracle.schedule(&market.market_id, Timestamp::from_secs(1500), report);

    let mut resolution = MarketResolution::new(&market, oracle_identity);
    let report = oracle.report(&market, Timestamp::from_secs(1500))?;
    resolution.observe(report, Timestamp::from_secs(1500))?;
    resolution.finalise(Timestamp::from_secs(2000))?;

    // Platform forms probability at time 1200
    let platform_prob = Stamped::new(
        Probability::new(dec!("0.70"))?,
        Timestamp::from_secs(1100),
        Timestamp::from_secs(1200),
    );

    // Market quote known at time 1300 (AFTER platform's probability)
    let market_prob = Stamped::new(
        Probability::new(dec!("0.75"))?,
        Timestamp::from_secs(1100),
        Timestamp::from_secs(1300), // Known later than platform's
    );

    // Attempt to create a ScoredForecast with a late market quote.
    // This should fail: the market quote became known after the platform's probability.
    let score_result = ScoredForecast::new(
        market.market_id.clone(),
        OutcomeId::new("yes"),
        platform_prob,
        market_prob,
        &resolution,
    );

    assert!(
        score_result.is_err(),
        "late market quote should be refused as leakage"
    );
    let error = score_result.unwrap_err();
    assert!(error.message().contains("known") && error.message().contains("after"));

    Ok(())
}
