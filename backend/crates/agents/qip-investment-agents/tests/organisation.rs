//! Tests for the investment organisation.
//!
//! The governance tests are the point of this file. An organisation chart is
//! only a control if something fails when it is violated, so most of what
//! follows tries to violate one.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_agents::budget::Budget;
use qip_agents::capability::Capability;
use qip_agents::finding::{
    AgentBrief, BriefPrecedent, Direction, FindingStatus, NumericProvenance,
};
use qip_agents::governance::Severity;
use qip_agents::manifest::AgentRole;
use qip_agents::memory::{Episode, EpisodeOutcome, ResearchMemory};
use qip_agents::runtime::{AgentHost, RunStatus};
use qip_ai::language::DeterministicModel;
use qip_ai::memory::PrecedentDigest;
use qip_ai::retrieval::{Document, SearchIndex};
use qip_core::error::Result;
use qip_core::ids::{AgentRunId, ObjectId, PortfolioId};
use qip_core::lineage::{CorrelationId, Lineage};
use qip_core::testing::is_exactly_zero;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Currency, Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::constraints::{RegulatoryConstraints, TradingRestriction};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_investment_agents::coverage::Scope;
use qip_investment_agents::desk::{
    BookView, ComplianceView, Desk, ExecutionRecord, ExecutionView, ForecasterHealth, MarketView,
    ModelsView, RiskView, TradeSide,
};
use qip_investment_agents::{Organisation, ids, manifests};
use qip_market::bar::{Bar, Interval};
use qip_market::book::{BookLevel, OrderBook};
use qip_market::snapshot::MarketSnapshot;
use qip_portfolio::portfolio::Portfolio;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_world_model::WorldModel;
use qip_world_model::features::{Feature, FeatureValue};
use qip_world_model::vocabulary::names;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn lineage() -> Lineage {
    Lineage::root(CorrelationId::from_string("cor-1"), "test")
}

fn brief() -> AgentBrief {
    AgentBrief::new(
        "is ACME mispriced given the macro backdrop",
        now(),
        Duration::from_days(30),
    )
    .about_objects(vec![object("ACME")])
    .about_entities(vec!["entity-policy".to_string()])
}

// --- the roster -------------------------------------------------------------

#[test]
fn the_declared_organisation_passes_its_own_governance() {
    let roster = manifests::roster(now());
    assert_eq!(roster.len(), 24, "the organisation is twenty-four agents");
    let findings = roster.validate(now()).expect("roster must validate");
    let errors: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn every_manifest_states_a_purpose_competencies_and_limitations() {
    // An agent nobody can describe is an agent nobody reviews.
    for manifest in manifests::roster(now()).iter() {
        assert!(
            manifest.purpose.len() > 30,
            "{} has a thin purpose",
            manifest.id
        );
        assert!(
            !manifest.limitations.is_empty(),
            "{} declares no limitations",
            manifest.id
        );
        assert!(!manifest.owner.is_empty(), "{} has no owner", manifest.id);
    }
}

#[test]
fn no_research_side_agent_can_reach_the_market() {
    for manifest in manifests::roster(now()).iter() {
        if !manifest.role.is_research_side() {
            continue;
        }
        for capability in manifest.capabilities.iter() {
            assert!(
                !capability.touches_market(),
                "{} holds {capability}",
                manifest.id
            );
        }
    }
}

#[test]
fn exactly_one_agent_can_submit_orders_and_it_publishes_nothing() {
    let roster = manifests::roster(now());
    let submitters = roster.holding(Capability::SubmitOrder);
    assert_eq!(submitters.len(), 1, "every extra path bypasses a control");
    let trader = submitters[0];
    assert_eq!(trader.id, ids::EXECUTION);
    assert_eq!(trader.role, AgentRole::Execution);
    assert!(!trader.capabilities.contains(Capability::PublishHypothesis));
    assert!(!trader.capabilities.contains(Capability::ProposeTrade));
}

#[test]
fn the_veto_is_held_only_by_control_agents_that_propose_nothing() {
    let roster = manifests::roster(now());
    let vetoers = roster.holding(Capability::VetoProposal);
    assert_eq!(vetoers.len(), 2, "risk and compliance");
    for manifest in vetoers {
        assert_eq!(manifest.role, AgentRole::Control, "{}", manifest.id);
        assert!(!manifest.capabilities.contains(Capability::ProposeTrade));
        assert!(
            !manifest
                .capabilities
                .contains(Capability::PublishHypothesis)
        );
    }
}

#[test]
fn no_agent_can_raise_its_own_autonomy_level() {
    let roster = manifests::roster(now());
    assert!(
        roster.holding(Capability::ChangeAutonomyLevel).is_empty(),
        "changing the autonomy level is an authenticated operator action, not an agent's"
    );
    assert!(
        roster.holding(Capability::OverrideRiskLimit).is_empty(),
        "no agent may move the limit it is checked against"
    );
}

#[test]
fn the_adversarial_reviewer_is_independent_and_publishes_nothing() {
    let roster = manifests::roster(now());
    let reviewer = roster.get(ids::ADVERSARIAL).unwrap();
    assert_eq!(reviewer.role, AgentRole::Adversarial);
    assert!(
        !reviewer
            .capabilities
            .contains(Capability::PublishHypothesis)
    );
    assert!(
        reviewer
            .capabilities
            .contains(Capability::ChallengeHypothesis)
    );

    // It must not report to the desks it reviews.
    for analyst in roster.with_role(AgentRole::Research) {
        assert_ne!(
            reviewer.owner, analyst.owner,
            "the reviewer shares an owner with {}",
            analyst.id
        );
    }
}

#[test]
fn risk_and_trading_are_owned_by_different_teams() {
    let roster = manifests::roster(now());
    let risk = roster.get(ids::RISK).unwrap();
    let trading = roster.get(ids::EXECUTION).unwrap();
    assert_ne!(risk.owner, trading.owner);
}

#[test]
fn the_fast_path_agent_has_no_language_model_and_a_millisecond_budget() {
    let manifest = manifests::microstructure_analyst(now());
    assert!(
        !manifest
            .capabilities
            .contains(Capability::CallLanguageModel)
    );
    assert_eq!(manifest.budget.language_model_calls, 0);
    assert!(manifest.budget.wall_time <= Duration::from_millis(50));
    assert_eq!(manifest.budget, Budget::fast_path());
}

#[test]
fn the_fast_path_agent_cannot_read_the_world_model() {
    // WORLD-065: the fast microstructure analyst reads tick data through the
    // market snapshot, never the world model. The separation is structural: no
    // ReadWorldModel capability is granted.
    let manifest = manifests::microstructure_analyst(now());
    assert!(
        !manifest.capabilities.contains(Capability::ReadWorldModel),
        "microstructure analyst must not read the world model"
    );
}

#[test]
fn compliance_has_no_language_model() {
    // A compliance decision that depends on how a sentence was phrased is not
    // a compliance decision.
    let manifest = manifests::compliance_control(now());
    assert!(
        !manifest
            .capabilities
            .contains(Capability::CallLanguageModel)
    );
}

#[test]
fn the_event_prediction_analyst_is_registered_and_reads_event_markets() {
    let roster = manifests::roster(now());
    let analyst = roster
        .get(ids::EVENT)
        .expect("event analyst must be registered");
    assert_eq!(analyst.role, AgentRole::Research);
    assert!(analyst.capabilities.contains(Capability::PublishHypothesis));
    assert!(analyst.capabilities.contains(Capability::CallLanguageModel));
    assert!(analyst.capabilities.contains(Capability::ReadMarketData));
    assert!(analyst.capabilities.contains(Capability::ReadReferenceData));
    assert!(analyst.capabilities.contains(Capability::ReadWorldModel));
    assert!(!analyst.capabilities.contains(Capability::SubmitOrder));
    assert_eq!(analyst.owner, "quantitative-research");
}

#[test]
fn the_coordinator_decides_nothing() {
    let chief = manifests::chief(now());
    for capability in [
        Capability::PublishHypothesis,
        Capability::ProposeTrade,
        Capability::ApproveProposal,
        Capability::VetoProposal,
        Capability::SubmitOrder,
    ] {
        assert!(
            !chief.capabilities.contains(capability),
            "the coordinator holds {capability}"
        );
    }
}

#[test]
fn an_expired_roster_does_not_start() {
    let roster = manifests::roster(now());
    let late = now().saturating_add(Duration::from_days(120));
    let error = roster.validate(late).unwrap_err();
    assert!(error.message().contains("expired"));
}

// --- the desk ---------------------------------------------------------------

fn equity(symbol: &str, price: &str) -> FinancialObject {
    FinancialObject::builder(
        object(symbol),
        symbol,
        InstrumentType::CommonStock,
        fixture_liquidity(),
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .price(Decimal::parse(price).unwrap())
    .provenance(Provenance::synthetic("test", now()))
    .build(now())
    .expect("valid object")
}

/// A spot commodity, so the commodities analyst's asset-class gate admits
/// the subject rather than deferring it as it does for ACME. Spot rather
/// than a future because the curve the analyst reads is a set of store
/// features keyed on the subject, not a property of the instrument, and a
/// future would need an underlying and a maturity the test never reads.
fn commodity(symbol: &str, price: &str) -> FinancialObject {
    FinancialObject::builder(
        object(symbol),
        symbol,
        InstrumentType::CommoditySpot,
        fixture_liquidity(),
    )
    .venue("XNYM")
    .sector(Sector::Energy)
    .price(Decimal::parse(price).unwrap())
    .provenance(Provenance::synthetic("test", now()))
    .build(now())
    .expect("valid object")
}

/// A spot currency pair for the carry analyst.
fn fx_pair(symbol: &str, price: &str) -> FinancialObject {
    FinancialObject::builder(
        object(symbol),
        symbol,
        InstrumentType::FxSpot,
        fixture_liquidity(),
    )
    .venue("FXALL")
    .price(Decimal::parse(price).unwrap())
    .provenance(Provenance::synthetic("test", now()))
    .build(now())
    .expect("valid object")
}

/// The instant the curve and carry readings describe, and the later instant
/// they became knowable. They differ on purpose: a record id that carried
/// only one of them, or the two swapped, would be indistinguishable from a
/// correct one if both were `now()`.
fn curve_valid_at() -> Timestamp {
    now().saturating_sub(Duration::from_days(1))
}

fn curve_known_at() -> Timestamp {
    now().saturating_sub(Duration::from_hours(12))
}

/// A price series with a mild uptrend and realistic daily noise.
fn bars(symbol: &str, count: usize, drift: f64) -> Vec<Bar> {
    let mut bars = Vec::with_capacity(count);
    let mut price = 100.0_f64;
    for i in 0..count {
        // Deterministic pseudo-noise: a fixed irrational rotation, so the
        // series varies without an RNG and the test stays reproducible.
        let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.02;
        let open = price;
        price *= 1.0 + drift + noise;
        let high = open.max(price) * 1.002;
        let low = open.min(price) * 0.998;
        let at = now().saturating_sub(Duration::from_days((count - i) as i64));
        bars.push(Bar {
            object_id: object(symbol),
            venue: "XNYS".to_string(),
            interval: Interval::Day,
            open_time: at,
            open: Decimal::from_f64(open).unwrap(),
            high: Decimal::from_f64(high).unwrap(),
            low: Decimal::from_f64(low).unwrap(),
            close: Decimal::from_f64(price).unwrap(),
            volume: dec!("1000000"),
            trade_count: 5_000,
            vwap: Decimal::from_f64((open + price) / 2.0),
            quality: qip_financial::quality::DataQuality::default(),
        });
    }
    bars
}

fn book(symbol: &str) -> OrderBook {
    OrderBook::from_levels(
        object(symbol),
        "XNYS",
        now(),
        vec![
            BookLevel::new(dec!("99.95"), dec!("5000")),
            BookLevel::new(dec!("99.90"), dec!("8000")),
            BookLevel::new(dec!("99.85"), dec!("12000")),
        ],
        vec![
            BookLevel::new(dec!("100.05"), dec!("5000")),
            BookLevel::new(dec!("100.10"), dec!("8000")),
            BookLevel::new(dec!("100.15"), dec!("12000")),
        ],
    )
}

/// A desk with enough data for the analysts to say something.
fn populated_desk() -> Arc<Desk> {
    desk_holding_causal_claims(Vec::new())
}

/// [`populated_desk`], with `claims` recorded in its world model's causal
/// graph. With none, the causal analyst has nothing to trace on any desk in
/// this file.
fn desk_holding_causal_claims(claims: Vec<qip_world_model::CausalEdge>) -> Arc<Desk> {
    let mut universe = Universe::new();
    universe.insert(equity("ACME", "100")).unwrap();
    universe.insert(commodity("WTI", "80")).unwrap();
    universe.insert(fx_pair("EURUSD", "1.08")).unwrap();

    let mut snapshot = MarketSnapshot::new(now());
    for bar in bars("ACME", 120, 0.0008) {
        snapshot.apply_bar(bar);
    }
    snapshot.apply_book(book("ACME"));

    let mut world = WorldModel::new();
    {
        let features = world.features_mut();
        // Keyed by the economy the equity's geography names — the builder's
        // default is `US` — because that is the key the macro arm writes and
        // the macro analyst reads. `"global"` was a key nothing ever wrote.
        for (name, base, slope) in [
            (names::POLICY_RATE, 0.03, 0.0002),
            (names::INFLATION_YOY, 0.025, -0.0001),
            (names::GROWTH_YOY, 0.02, 0.0001),
            (names::CREDIT_SPREAD_BPS, 120.0, 0.5),
        ] {
            for i in 0..60 {
                let at = now().saturating_sub(Duration::from_days(60 - i));
                features.record(
                    name,
                    "US",
                    FeatureValue::new(base + slope * i as f64, at, at),
                );
            }
        }
        // Instrument-level features the credit and derivatives analysts need.
        features.define(Feature::new(
            "credit_spread_bps",
            "issuer spread",
            "credit-provider",
        ));
        features.define(Feature::new(
            "effective_duration",
            "modified duration",
            "credit-provider",
        ));
        features.define(Feature::new(
            "implied_volatility",
            "at-the-money implied",
            "options-provider",
        ));
        for i in 0..60 {
            let at = now().saturating_sub(Duration::from_days(60 - i));
            features.record(
                "credit_spread_bps",
                object("ACME").as_str(),
                FeatureValue::new(150.0 + (i as f64 * 0.9), at, at),
            );
        }
        features.record(
            "effective_duration",
            object("ACME").as_str(),
            FeatureValue::new(5.5, now(), now()),
        );
        features.record(
            "implied_volatility",
            object("ACME").as_str(),
            FeatureValue::new(0.35, now(), now()),
        );
        // The curve points the commodities analyst reads, and the two legs
        // and volatility the FX analyst reads. Until these existed both
        // analysts returned no-data on every desk in this file, so their
        // observed stamps compiled without ever being dispatched.
        for (name, description) in [
            ("front_month_price", "front-month settlement"),
            ("deferred_month_price", "deferred-month settlement"),
            ("deferred_tenor_months", "months from front to deferred"),
        ] {
            features.define(Feature::new(name, description, "curve-provider"));
        }
        // Backwardated: the deferred contract is below the front.
        for (name, value) in [
            ("front_month_price", 80.0),
            ("deferred_month_price", 78.0),
            ("deferred_tenor_months", 3.0),
        ] {
            features.record(
                name,
                object("WTI").as_str(),
                FeatureValue::new(value, curve_valid_at(), curve_known_at()),
            );
        }
        for (name, description) in [
            ("base_rate", "base leg policy rate"),
            ("quote_rate", "quote leg policy rate"),
            ("realised_volatility", "annualised realised volatility"),
        ] {
            features.define(Feature::new(name, description, "rates-provider"));
        }
        for (name, value) in [
            ("base_rate", 0.04),
            ("quote_rate", 0.015),
            ("realised_volatility", 0.08),
        ] {
            features.record(
                name,
                object("EURUSD").as_str(),
                FeatureValue::new(value, curve_valid_at(), curve_known_at()),
            );
        }
    }
    for claim in claims {
        world.claim_causal(claim).unwrap();
    }
    // One clean claim in the evidence fabric, so the poisoning detector has
    // something to scan and reports a scan rather than an empty fabric.
    world
        .absorb_evidence(
            qip_world_model::EvidenceRecord::new(
                "ev-acme-margin".into(),
                "https://wire.example/acme".into(),
                now(),
                now(),
                "test".into(),
                "commercial".into(),
                "ACME reported margin compression".into(),
                0.9,
            )
            .unwrap(),
            Some(&"d".repeat(64)),
            now(),
        )
        .unwrap();

    let mut library = SearchIndex::new();
    library.add(Document::new(
        "doc-1",
        "ACME reported margin compression driven by floating rate funding costs",
        now().saturating_sub(Duration::from_days(2)),
    ));

    Arc::new(
        Desk::new(
            MarketView { snapshot, universe },
            world,
            BookView {
                portfolio: Portfolio::new(
                    PortfolioId::from_string("pf-1"),
                    "test",
                    Currency::USD,
                    dec!("10000000"),
                    now(),
                ),
                marks: BTreeMap::from([(object("ACME").as_str().to_string(), dec!("100"))]),
            },
            RiskView {
                state: RiskState::default(),
                limits: LimitSet::conservative_default(),
            },
            ComplianceView::default(),
            ResearchMemory::new(),
            library,
        )
        .with_executions(fixture_executions())
        .with_models(fixture_models()),
    )
}

/// One scored forecaster in good standing and no drift report: a registry
/// the model-failure probe examines and finds nothing in.
fn fixture_models() -> ModelsView {
    ModelsView {
        forecasters: vec![ForecasterHealth {
            name: "detector:momentum".to_string(),
            mean_brier: Some(0.18),
            predicted_failure: 0.2,
        }],
        drifted: Vec::new(),
    }
}

/// Four of the platform's own fills, landing on both sides of arrival with
/// distinct sizes: a flow the self-exploitation probe examines and finds
/// nothing in.
fn fixture_executions() -> ExecutionView {
    let record = |n: i64, side: TradeSide, quantity: &str, fill: &str| ExecutionRecord {
        order_id: format!("ord-{n}"),
        object_id: object("ACME").as_str().to_string(),
        side,
        quantity: Decimal::parse(quantity).unwrap(),
        arrival_price: dec!("100"),
        fill_price: Decimal::parse(fill).unwrap(),
        placed_at: now().saturating_sub(Duration::from_days(5 - n)),
        filled_at: now().saturating_sub(Duration::from_days(5 - n)),
        venue: "XSIM".to_string(),
    };
    ExecutionView {
        executions: vec![
            record(1, TradeSide::Buy, "100", "100.02"),
            record(2, TradeSide::Buy, "150", "99.98"),
            record(3, TradeSide::Sell, "120", "100.03"),
            record(4, TradeSide::Sell, "80", "99.97"),
        ],
    }
}

fn organisation(desk: Arc<Desk>) -> Result<Organisation> {
    Organisation::standard(
        desk,
        now(),
        now(),
        42,
        Some(Arc::new(DeterministicModel::new())),
        vec!["web-traffic".to_string()],
        false,
    )
}

#[test]
fn the_organisation_assembles_and_dispatches() -> Result<()> {
    let mut org = organisation(populated_desk())?;
    assert_eq!(org.len(), 23);

    let report = org.dispatch(&brief(), now(), &lineage());
    assert!(!report.runs.is_empty());
    assert!(report.failed.is_empty(), "runs failed: {:?}", report.failed);
    assert!(
        report.permission_violations().is_empty(),
        "an agent reached for something it was not granted"
    );
    Ok(())
}

#[test]
fn no_agent_reaches_a_facility_its_manifest_does_not_grant() -> Result<()> {
    // The whole point of the gated desk: eighteen agents share one bundle of
    // facilities and each sees only its own slice.
    let mut org = organisation(populated_desk())?;
    let report = org.dispatch(&brief(), now(), &lineage());

    for run in &report.runs {
        let manifest = org
            .roster()
            .get(&run.agent_id)
            .expect("agent is on the roster");
        for capability in run.capabilities_used() {
            assert!(
                manifest.capabilities.contains(capability),
                "{} exercised {capability} without holding it",
                run.agent_id
            );
        }
        assert!(
            run.denied_accesses().is_empty(),
            "{} was denied {:?}",
            run.agent_id,
            run.denied_accesses()
        );
    }
    Ok(())
}

#[test]
fn every_number_in_every_finding_carries_a_provenance() -> Result<()> {
    // The rule that keeps a hallucinated quantity out of a decision. It holds
    // across all eighteen agents or it does not hold at all.
    let mut org = organisation(populated_desk())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let mut checked = 0;
    for finding in &report.findings {
        for fact in &finding.facts {
            fact.validate()?;
            match &fact.provenance {
                NumericProvenance::Computed { by, inputs } => {
                    assert!(
                        !inputs.is_empty(),
                        "{by} computed {} from nothing",
                        fact.label
                    );
                }
                NumericProvenance::Observed { source, .. } => {
                    assert!(!source.trim().is_empty());
                }
            }
            checked += 1;
        }
    }
    assert!(checked > 20, "only {checked} facts were produced");
    Ok(())
}

#[test]
fn a_value_an_agent_reads_off_the_desk_is_recorded_as_observed_with_its_record() -> Result<()> {
    // Premise, then property. Until this test existed every number the
    // eighteen agents reported was stamped computed by the agent — a spread
    // read straight from the feature store was "computed by credit-analyst
    // from [credit_spread_bps]" — so no finding ever named a source, an as-of
    // instant or a record id, and the observed/computed split the security
    // suite protects was, in production, a constant. The support module
    // offered only `computed`, which is why the defect held across all
    // eighteen agents at once.
    let mut org = organisation(populated_desk())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    assert!(report.failed.is_empty(), "runs failed: {:?}", report.failed);

    let expected = [
        (
            ids::CREDIT,
            "credit_spread_bps",
            "feature-store:credit-provider",
        ),
        (
            ids::CREDIT,
            "effective_duration",
            "feature-store:credit-provider",
        ),
        (
            ids::DERIVATIVES,
            "implied_volatility",
            "feature-store:options-provider",
        ),
        (ids::MICROSTRUCTURE, "best_bid", "book:XNYS"),
        (ids::MICROSTRUCTURE, "best_ask", "book:XNYS"),
    ];
    for (agent, label, source) in expected {
        let finding = report
            .findings
            .iter()
            .find(|f| f.agent_id == agent)
            .unwrap_or_else(|| panic!("premise: {agent} produced an informative finding"));
        let fact = finding
            .fact(label)
            .unwrap_or_else(|| panic!("premise: {agent} reported {label}"));
        match &fact.provenance {
            NumericProvenance::Observed {
                source: recorded,
                as_of,
                record_id,
            } => {
                assert_eq!(recorded, source, "{agent}.{label} names the wrong source");
                assert!(
                    !record_id.trim().is_empty(),
                    "{agent}.{label} names no record to re-fetch"
                );
                // The knowable-instant guard at the reporting boundary: a
                // fact observed after the as-of the agent reasoned at is a
                // fact the agent could not have had.
                assert!(
                    *as_of <= finding.as_of,
                    "{agent}.{label} was observed at {as_of}, after the as-of {} the agent reasoned at",
                    finding.as_of
                );
            }
            NumericProvenance::Computed { by, inputs } => panic!(
                "{agent} read {label} off the desk and recorded it as computed by {by} from {inputs:?}"
            ),
        }
    }

    // The split is a split, not a constant the other way: derived numbers
    // are still computed, alongside the observed ones.
    let computed = report
        .findings
        .iter()
        .flat_map(|f| &f.facts)
        .filter(|f| matches!(f.provenance, NumericProvenance::Computed { .. }))
        .count();
    assert!(computed > 0, "no computed fact survived");
    Ok(())
}

/// Asserts that `agent`, dispatched on a brief about `symbol`, produced a
/// finding rather than a refusal, and that each of `labels` is stamped
/// observed from `source` with a record id naming both bitemporal instants.
///
/// The premise comes first and is asserted against the status the analysts
/// actually return when a feature is missing — `NoView`, via `no_data` — so a
/// fixture that quietly stopped supplying a feature fails here with the
/// analyst's own reason rather than as a missing fact three lines later.
fn assert_read_off_the_store(
    org: &mut Organisation,
    symbol: &str,
    agent: &str,
    source: &str,
    labels: &[&str],
) {
    let brief = AgentBrief::new(
        format!("what does the curve or carry say about {symbol}"),
        now(),
        Duration::from_days(30),
    )
    .about_objects(vec![object(symbol)]);
    let report = org.dispatch(&brief, now(), &lineage());
    assert!(report.failed.is_empty(), "runs failed: {:?}", report.failed);

    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == agent)
        .unwrap_or_else(|| panic!("premise: {agent} produced a finding about {symbol}"));
    assert_ne!(
        finding.status,
        FindingStatus::NoView,
        "premise: {agent} returned no-data on {symbol}: {}",
        finding.claim
    );
    assert_ne!(
        finding.status,
        FindingStatus::Deferred,
        "premise: {agent} deferred {symbol}: {}",
        finding.claim
    );

    let subject = object(symbol);
    for label in labels {
        let fact = finding
            .fact(label)
            .unwrap_or_else(|| panic!("premise: {agent} reported {label} for {symbol}"));
        match &fact.provenance {
            NumericProvenance::Observed {
                source: recorded,
                as_of,
                record_id,
            } => {
                assert_eq!(recorded, source, "{agent}.{label} names the wrong source");
                // The exact id `observed_feature` writes, rebuilt from the
                // fixture's two instants: valid-at first, knowable-at second.
                // Equality, not `contains`, so an id that dropped one instant
                // or swapped them cannot pass.
                assert_eq!(
                    record_id,
                    &format!(
                        "feature:{label}@{}:valid={}:known={}",
                        subject.as_str(),
                        curve_valid_at(),
                        curve_known_at()
                    ),
                    "{agent}.{label} does not carry both bitemporal instants"
                );
                assert_eq!(
                    *as_of,
                    curve_valid_at(),
                    "{agent}.{label} is stamped as-of the wrong instant"
                );
                assert!(
                    *as_of <= finding.as_of,
                    "{agent}.{label} was observed after the as-of the agent reasoned at"
                );
            }
            NumericProvenance::Computed { by, inputs } => panic!(
                "{agent} read {label} off the store and recorded it as computed by {by} from {inputs:?}"
            ),
        }
    }
}

#[test]
fn the_commodities_analyst_stamps_each_curve_point_observed_from_the_store() -> Result<()> {
    // Until the WTI fixture existed no test in this file gave the
    // commodities analyst a front price, a deferred price and a tenor, so it
    // returned no-data on every dispatch and its observed stamp was
    // compile-only: a change reverting it to `computed` would have passed
    // the whole suite. The roll yield is the one number the agent itself
    // derives, and the split is checked both ways.
    let mut org = organisation(populated_desk())?;
    assert_read_off_the_store(
        &mut org,
        "WTI",
        ids::COMMODITIES,
        "feature-store:curve-provider",
        &[
            "front_month_price",
            "deferred_month_price",
            "deferred_tenor_months",
        ],
    );
    let brief = AgentBrief::new("is WTI backwardated", now(), Duration::from_days(30))
        .about_objects(vec![object("WTI")]);
    let report = org.dispatch(&brief, now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::COMMODITIES)
        .expect("premise: the commodities analyst reported");
    let roll = finding
        .fact("annualised_roll_yield")
        .expect("premise: the roll yield was reported");
    assert!(
        matches!(&roll.provenance, NumericProvenance::Computed { by, .. } if by == ids::COMMODITIES),
        "the roll yield is the agent's own arithmetic, not a reading: {:?}",
        roll.provenance
    );
    Ok(())
}

#[test]
fn the_fx_analyst_stamps_both_legs_and_the_volatility_observed_from_the_store() -> Result<()> {
    // The same gap as the commodities analyst: no desk in this file carried
    // `base_rate`, `quote_rate` or `realised_volatility`, so the carry path
    // never ran past its no-data refusal and the three observed stamps on it
    // were never asserted. The volatility-adjusted carry is the agent's.
    let mut org = organisation(populated_desk())?;
    assert_read_off_the_store(
        &mut org,
        "EURUSD",
        ids::FX_RATES,
        "feature-store:rates-provider",
        &["base_rate", "quote_rate", "realised_volatility"],
    );
    let brief = AgentBrief::new("does EURUSD carry", now(), Duration::from_days(30))
        .about_objects(vec![object("EURUSD")]);
    let report = org.dispatch(&brief, now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::FX_RATES)
        .expect("premise: the FX analyst reported");
    let carry = finding
        .fact("volatility_adjusted_carry")
        .expect("premise: the carry was reported, so the volatility leg was present");
    assert!(
        matches!(&carry.provenance, NumericProvenance::Computed { by, .. } if by == ids::FX_RATES),
        "the carry is the agent's own arithmetic, not a reading: {:?}",
        carry.provenance
    );
    Ok(())
}

#[test]
fn every_directional_finding_states_what_would_falsify_it() -> Result<()> {
    let mut org = organisation(populated_desk())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    for finding in &report.findings {
        if matches!(finding.direction, Direction::Positive | Direction::Negative) {
            assert!(
                !finding.falsifiers.is_empty(),
                "{} took a view with no falsifier",
                finding.agent_id
            );
        }
    }
    Ok(())
}

#[test]
fn an_empty_desk_produces_honest_no_view_findings_rather_than_failures() -> Result<()> {
    // A cold start is not an error. Every agent must say it has nothing rather
    // than invent something or crash the cycle.
    let mut org = organisation(Arc::new(Desk::empty(now())))?;
    let report = org.dispatch(&brief(), now(), &lineage());

    assert!(report.failed.is_empty(), "runs failed: {:?}", report.failed);
    for finding in &report.findings {
        assert!(
            matches!(
                finding.status,
                FindingStatus::NoView | FindingStatus::Partial | FindingStatus::Complete
            ),
            "{} returned {:?}",
            finding.agent_id,
            finding.status
        );
        if finding.status == FindingStatus::NoView {
            assert!(is_exactly_zero(finding.effective_conviction()));
        }
    }
    Ok(())
}

// --- individual agents ------------------------------------------------------

#[test]
fn the_equity_analyst_declines_an_instrument_outside_its_remit() -> Result<()> {
    use qip_investment_agents::analysts::EquityAnalyst;

    let desk = populated_desk();
    let agent = EquityAnalyst::new(manifests::equity_analyst(now()), desk);
    let host = AgentHost::new(1);
    // A commodity future is not equity; the analyst must defer rather than
    // apply an equity model to it.
    let commodity = AgentBrief::new("is crude cheap", now(), Duration::from_days(30))
        .about_objects(vec![object("CL")]);
    let record = host.run(
        &agent,
        &commodity,
        now(),
        lineage(),
        AgentRunId::from_string("run-1"),
    );
    // No market state for CL, so the honest answer is no-data rather than a view.
    let finding = record.finding.expect("a finding");
    assert!(
        matches!(
            finding.status,
            FindingStatus::NoView | FindingStatus::Deferred
        ),
        "{:?}",
        finding.status
    );
    Ok(())
}

#[test]
fn the_macro_analyst_defers_on_an_intraday_horizon() -> Result<()> {
    use qip_investment_agents::analysts::MacroAnalyst;

    let agent = MacroAnalyst::new(manifests::macro_analyst(now()), populated_desk());
    let intraday = AgentBrief::new(
        "what happens this afternoon",
        now(),
        Duration::from_hours(4),
    );
    let record = AgentHost::new(1).run(
        &agent,
        &intraday,
        now(),
        lineage(),
        AgentRunId::from_string("run-2"),
    );
    let finding = record.finding.expect("a finding");
    assert_eq!(finding.status, FindingStatus::Deferred);
    assert!(finding.claim.contains("publication lag"));
    assert!(is_exactly_zero(finding.effective_conviction()));
    Ok(())
}

#[test]
fn the_credit_analyst_refuses_to_assume_a_duration_it_does_not_have() -> Result<()> {
    use qip_investment_agents::analysts::CreditAnalyst;

    // A spread move cannot be turned into a price move without a duration, and
    // assuming one is how a plausible number reaches a position size.
    let mut world = WorldModel::new();
    {
        let features = world.features_mut();
        features.define(Feature::new("credit_spread_bps", "spread", "provider"));
        for i in 0..60 {
            let at = now().saturating_sub(Duration::from_days(60 - i));
            features.record(
                "credit_spread_bps",
                object("ACME").as_str(),
                FeatureValue::new(150.0 + i as f64, at, at),
            );
        }
    }
    let desk = Arc::new(Desk::new(
        MarketView {
            snapshot: MarketSnapshot::new(now()),
            universe: Universe::new(),
        },
        world,
        BookView {
            portfolio: Portfolio::new(
                PortfolioId::from_string("pf-1"),
                "test",
                Currency::USD,
                dec!("1000000"),
                now(),
            ),
            marks: BTreeMap::new(),
        },
        RiskView {
            state: RiskState::default(),
            limits: LimitSet::conservative_default(),
        },
        ComplianceView::default(),
        ResearchMemory::new(),
        SearchIndex::new(),
    ));

    let agent = CreditAnalyst::new(manifests::credit_analyst(now()), desk);
    let record = AgentHost::new(1).run(
        &agent,
        &brief(),
        now(),
        lineage(),
        AgentRunId::from_string("run-3"),
    );
    let finding = record.finding.expect("a finding");
    assert_eq!(finding.status, FindingStatus::Partial);
    assert_eq!(finding.direction, Direction::Neutral);
    assert!(
        finding
            .missing_inputs
            .iter()
            .any(|m| m.contains("effective_duration")),
        "{:?}",
        finding.missing_inputs
    );
    Ok(())
}

#[test]
fn the_alternative_data_analyst_will_not_use_an_unlicensed_dataset() -> Result<()> {
    use qip_investment_agents::analysts::AlternativeDataAnalyst;

    let mut world = WorldModel::new();
    {
        // Recorded under the world model's own definition, whose producer is
        // the `card-spend` dataset. This fixture once redefined the series
        // with producer `alt-provider`, and the analyst now refuses a
        // licensed name whose series some other producer wrote — which is a
        // different property from the licence question this test asks.
        let features = world.features_mut();
        for i in 0..60 {
            let at = now().saturating_sub(Duration::from_days(60 - i));
            features.record(
                names::CARD_SPEND_INDEX,
                object("ACME").as_str(),
                FeatureValue::new(100.0 + i as f64 * 1.5, at, at),
            );
        }
    }
    let desk = Arc::new(Desk::new(
        MarketView {
            snapshot: MarketSnapshot::new(now()),
            universe: Universe::new(),
        },
        world,
        BookView {
            portfolio: Portfolio::new(
                PortfolioId::from_string("pf-1"),
                "t",
                Currency::USD,
                dec!("1"),
                now(),
            ),
            marks: BTreeMap::new(),
        },
        RiskView {
            state: RiskState::default(),
            limits: LimitSet::conservative_default(),
        },
        ComplianceView::default(),
        ResearchMemory::new(),
        SearchIndex::new(),
    ));

    // The dataset holding the data is not licensed for investment decisions.
    let refusing = AlternativeDataAnalyst::new(
        manifests::alternative_data_analyst(now()),
        desk.clone(),
        vec!["web-traffic".to_string()],
    );
    let record = AgentHost::new(1).run(
        &refusing,
        &brief(),
        now(),
        lineage(),
        AgentRunId::from_string("run-4"),
    );
    let finding = record.finding.expect("a finding");
    assert_eq!(finding.status, FindingStatus::NoView);
    assert!(finding.claim.contains("not licensed"), "{}", finding.claim);

    // With the licence, the same data produces a view.
    let permitted = AlternativeDataAnalyst::new(
        manifests::alternative_data_analyst(now()),
        desk,
        vec!["card-spend".to_string()],
    );
    let record = AgentHost::new(1).run(
        &permitted,
        &brief(),
        now(),
        lineage(),
        AgentRunId::from_string("run-5"),
    );
    let finding = record.finding.expect("a finding");
    assert_ne!(finding.status, FindingStatus::NoView);
    Ok(())
}

#[test]
fn compliance_escalates_a_name_it_has_no_record_for() -> Result<()> {
    use qip_investment_agents::control::ComplianceControl;

    // No record is not permission.
    let desk = populated_desk();
    let agent = ComplianceControl::new(manifests::compliance_control(now()), desk);
    let record = AgentHost::new(1).run(
        &agent,
        &brief(),
        now(),
        lineage(),
        AgentRunId::from_string("run-6"),
    );
    let finding = record.finding.expect("a finding");
    assert_eq!(finding.status, FindingStatus::Partial);
    assert!(finding.claim.contains("escalated"), "{}", finding.claim);
    Ok(())
}

#[test]
fn compliance_blocks_a_restricted_name_with_full_conviction() -> Result<()> {
    use qip_investment_agents::control::ComplianceControl;

    let mut compliance = ComplianceView::default();
    compliance.constraints.insert(
        object("ACME").as_str().to_string(),
        RegulatoryConstraints::unrestricted().with_restriction(
            TradingRestriction::RestrictedList {
                reason: "material non-public information wall".to_string(),
            },
        ),
    );

    let desk = Arc::new(Desk::new(
        MarketView {
            snapshot: MarketSnapshot::new(now()),
            universe: Universe::new(),
        },
        WorldModel::new(),
        BookView {
            portfolio: Portfolio::new(
                PortfolioId::from_string("pf-1"),
                "t",
                Currency::USD,
                dec!("1"),
                now(),
            ),
            marks: BTreeMap::new(),
        },
        RiskView {
            state: RiskState::default(),
            limits: LimitSet::conservative_default(),
        },
        compliance,
        ResearchMemory::new(),
        SearchIndex::new(),
    ));

    let agent = ComplianceControl::new(manifests::compliance_control(now()), desk);
    let record = AgentHost::new(1).run(
        &agent,
        &brief(),
        now(),
        lineage(),
        AgentRunId::from_string("run-7"),
    );
    let finding = record.finding.expect("a finding");
    assert_eq!(finding.direction, Direction::Negative);
    assert!(
        is_exactly_zero(finding.conviction - 1.0),
        "a restriction is a fact, not a judgement"
    );
    Ok(())
}

#[test]
fn the_execution_trader_refuses_to_estimate_a_size_it_was_not_given() -> Result<()> {
    use qip_investment_agents::execution::ExecutionTrader;

    let agent = ExecutionTrader::new(manifests::execution_trader(now()), populated_desk());
    let record = AgentHost::new(1).run(
        &agent,
        &brief(),
        now(),
        lineage(),
        AgentRunId::from_string("run-8"),
    );
    let finding = record.finding.expect("a finding");
    assert_eq!(finding.status, FindingStatus::NoView);
    assert!(finding.claim.contains("no size"), "{}", finding.claim);

    // Given a size, it produces an estimate.
    let sized = brief().asking(vec!["size=50000".to_string()]);
    let record = AgentHost::new(1).run(
        &agent,
        &sized,
        now(),
        lineage(),
        AgentRunId::from_string("run-9"),
    );
    let finding = record.finding.expect("a finding");
    assert!(finding.fact("estimated_impact_bps").is_some());
    assert_eq!(
        finding.direction,
        Direction::Neutral,
        "the trader works orders; it does not have views"
    );
    Ok(())
}

#[test]
fn the_construction_agent_will_not_invent_a_conviction() -> Result<()> {
    use qip_investment_agents::construction::PortfolioConstruction;

    // Construction expresses approved theses. With none recorded it must say
    // so rather than size something on its own reading.
    let agent =
        PortfolioConstruction::new(manifests::portfolio_construction(now()), populated_desk());
    let record = AgentHost::new(1).run(
        &agent,
        &brief(),
        now(),
        lineage(),
        AgentRunId::from_string("run-10"),
    );
    let finding = record.finding.expect("a finding");
    assert_eq!(finding.status, FindingStatus::NoView);
    assert!(
        finding.claim.contains("does not create them"),
        "{}",
        finding.claim
    );
    Ok(())
}

#[test]
fn the_construction_agent_respects_its_concentration_cap() -> Result<()> {
    use qip_investment_agents::construction::PortfolioConstruction;

    let mut universe = Universe::new();
    let mut world = WorldModel::new();
    let mut objects = Vec::new();
    {
        let features = world.features_mut();
        features.define(Feature::new(
            "approved_conviction",
            "conviction of an approved thesis",
            "reasoning-engine",
        ));
        // Three names against an 8% cap cannot reach 90% gross; the cap wins.
        for (i, symbol) in ["AAA", "BBB", "CCC"].iter().enumerate() {
            universe.insert(equity(symbol, "100")).unwrap();
            objects.push(object(symbol));
            features.record(
                "approved_conviction",
                object(symbol).as_str(),
                FeatureValue::new(0.9 - i as f64 * 0.2, now(), now()),
            );
        }
    }

    let desk = Arc::new(Desk::new(
        MarketView {
            snapshot: MarketSnapshot::new(now()),
            universe,
        },
        world,
        BookView {
            portfolio: Portfolio::new(
                PortfolioId::from_string("pf-1"),
                "t",
                Currency::USD,
                dec!("1000000"),
                now(),
            ),
            marks: BTreeMap::new(),
        },
        RiskView {
            state: RiskState::default(),
            limits: LimitSet::conservative_default(),
        },
        ComplianceView::default(),
        ResearchMemory::new(),
        SearchIndex::new(),
    ));

    let agent = PortfolioConstruction::new(manifests::portfolio_construction(now()), desk);
    let record = AgentHost::new(1).run(
        &agent,
        &brief().about_objects(objects),
        now(),
        lineage(),
        AgentRunId::from_string("run-11"),
    );
    let finding = record.finding.expect("a finding");
    let largest = finding.fact("largest_position_weight").unwrap().value;
    assert!(largest <= 0.08 + 1e-9, "cap breached at {largest}");
    let gross = finding.fact("proposed_gross_exposure").unwrap().value;
    assert!(
        gross <= 0.24 + 1e-9,
        "three names at an 8% cap cannot exceed 24%"
    );
    assert!(
        finding.caveats.iter().any(|c| c.contains("under-invested")),
        "the shortfall against the exposure target must be stated: {:?}",
        finding.caveats
    );
    Ok(())
}

#[test]
fn the_quantum_agent_recommends_the_classical_solver_without_a_backend() -> Result<()> {
    use qip_investment_agents::construction::QuantumOptimization;

    let agent = QuantumOptimization::new(
        manifests::quantum_optimization(now()),
        populated_desk(),
        false,
    );
    let large = AgentBrief::new("optimise the book", now(), Duration::from_days(1))
        .about_objects((0..80).map(|i| object(&format!("N{i}"))).collect())
        .asking(vec!["respect a cardinality constraint of 30".to_string()]);
    let record = AgentHost::new(1).run(
        &agent,
        &large,
        now(),
        lineage(),
        AgentRunId::from_string("run-12"),
    );
    let finding = record.finding.expect("a finding");
    assert!(
        finding.claim.contains("classical solver"),
        "{}",
        finding.claim
    );
    assert!(is_exactly_zero(
        finding.fact("quantum_warranted").unwrap().value - 1.0
    ));
    assert!(is_exactly_zero(
        finding.fact("backend_available").unwrap().value
    ));
    assert!(
        finding
            .caveats
            .iter()
            .any(|c| c.contains("classical baseline")),
        "no quantum result may be used without a classical baseline"
    );
    Ok(())
}

#[test]
fn a_small_problem_does_not_warrant_a_quantum_solver() -> Result<()> {
    use qip_investment_agents::construction::QuantumOptimization;

    let agent = QuantumOptimization::new(
        manifests::quantum_optimization(now()),
        populated_desk(),
        true,
    );
    let small = AgentBrief::new("optimise", now(), Duration::from_days(1))
        .about_objects((0..10).map(|i| object(&format!("N{i}"))).collect())
        .asking(vec!["respect a cardinality constraint".to_string()]);
    let record = AgentHost::new(1).run(
        &agent,
        &small,
        now(),
        lineage(),
        AgentRunId::from_string("run-13"),
    );
    let finding = record.finding.expect("a finding");
    assert!(is_exactly_zero(
        finding.fact("quantum_warranted").unwrap().value
    ));
    Ok(())
}

#[test]
fn the_learning_agent_cannot_score_an_unresolved_episode() -> Result<()> {
    use qip_investment_agents::learning::LearningAttribution;

    let mut memory = ResearchMemory::new();
    // Resolved only after the as-of time: scoring it now would grade the
    // thesis on a path it has not finished travelling.
    memory.record_episode(Episode {
        run_id: AgentRunId::from_string("run-past"),
        agent_id: "macro-analyst".to_string(),
        occurred_at: now().saturating_sub(Duration::from_days(10)),
        question: "will spreads widen".to_string(),
        conclusion: "yes".to_string(),
        evidence: vec!["doc-1".to_string()],
        conviction: 0.7,
        outcome: Some(EpisodeOutcome {
            resolved_at: now().saturating_add(Duration::from_days(20)),
            correct: true,
            realised_bps: Some(40.0),
            explanation: "the print landed".to_string(),
        }),
    });

    let desk = Arc::new(Desk::new(
        MarketView {
            snapshot: MarketSnapshot::new(now()),
            universe: Universe::new(),
        },
        WorldModel::new(),
        BookView {
            portfolio: Portfolio::new(
                PortfolioId::from_string("pf-1"),
                "t",
                Currency::USD,
                dec!("1"),
                now(),
            ),
            marks: BTreeMap::new(),
        },
        RiskView {
            state: RiskState::default(),
            limits: LimitSet::conservative_default(),
        },
        ComplianceView::default(),
        memory,
        SearchIndex::new(),
    ));

    let agent = LearningAttribution::new(manifests::learning_attribution(now()), desk);
    let record = AgentHost::new(1).run(
        &agent,
        &brief(),
        now(),
        lineage(),
        AgentRunId::from_string("run-14"),
    );
    let finding = record.finding.expect("a finding");
    assert_eq!(finding.status, FindingStatus::NoView);
    assert!(finding.claim.contains("still open"), "{}", finding.claim);
    Ok(())
}

#[test]
fn the_learning_agent_reports_the_sample_size_behind_each_hit_rate() -> Result<()> {
    use qip_investment_agents::learning::LearningAttribution;

    // A hit rate on its own cannot be told apart from one resting on ten
    // episodes or a thousand. The count was computed and then thrown away
    // (`let _ = count;`), so the finding reported a rate with nothing behind
    // it to weigh the claim against.
    let mut memory = ResearchMemory::new();
    let resolved = 12;
    let correct = 8;
    for i in 0..resolved {
        memory.record_episode(Episode {
            run_id: AgentRunId::from_string(format!("run-{i}")),
            agent_id: "macro-analyst".to_string(),
            occurred_at: now().saturating_sub(Duration::from_days(30 - i)),
            question: "will spreads widen".to_string(),
            conclusion: "yes".to_string(),
            evidence: vec!["doc-1".to_string()],
            conviction: 0.7,
            outcome: Some(EpisodeOutcome {
                resolved_at: now().saturating_sub(Duration::from_days(20 - i)),
                correct: i < correct,
                realised_bps: Some(40.0),
                explanation: "the print landed".to_string(),
            }),
        });
    }

    let desk = Arc::new(Desk::new(
        MarketView {
            snapshot: MarketSnapshot::new(now()),
            universe: Universe::new(),
        },
        WorldModel::new(),
        BookView {
            portfolio: Portfolio::new(
                PortfolioId::from_string("pf-1"),
                "t",
                Currency::USD,
                dec!("1"),
                now(),
            ),
            marks: BTreeMap::new(),
        },
        RiskView {
            state: RiskState::default(),
            limits: LimitSet::conservative_default(),
        },
        ComplianceView::default(),
        memory,
        SearchIndex::new(),
    ));

    let agent = LearningAttribution::new(manifests::learning_attribution(now()), desk);
    let record = AgentHost::new(1).run(
        &agent,
        &brief(),
        now(),
        lineage(),
        AgentRunId::from_string("run-15"),
    );
    let finding = record.finding.expect("a finding");
    // Premise: enough episodes resolved that the agent actually computed a
    // per-agent hit rate rather than withholding it.
    let rate = finding
        .fact("hit_rate_macro-analyst")
        .unwrap_or_else(|| panic!("premise: a hit rate was reported: {:?}", finding.facts));
    assert!(
        (rate.value - (correct as f64 / resolved as f64)).abs() < 1e-9,
        "{}",
        rate.value
    );

    let count = finding
        .fact("hit_rate_macro-analyst_episode_count")
        .expect("the sample size behind the hit rate must be reported alongside it");
    assert!(
        (count.value - resolved as f64).abs() < f64::EPSILON,
        "{} resolved episodes backed the rate, but the fact says {}",
        resolved,
        count.value
    );
    Ok(())
}

// --- determinism ------------------------------------------------------------

#[test]
fn the_same_desk_and_seed_produce_the_same_report() -> Result<()> {
    // Replayability is the property the whole platform rests on: the same
    // inputs must reconstruct the same decision.
    let first = organisation(populated_desk())?.dispatch(&brief(), now(), &lineage());
    let second = organisation(populated_desk())?.dispatch(&brief(), now(), &lineage());

    assert_eq!(first.findings.len(), second.findings.len());
    for (a, b) in first.findings.iter().zip(&second.findings) {
        assert_eq!(a.agent_id, b.agent_id);
        assert_eq!(a.claim, b.claim);
        assert_eq!(a.direction, b.direction);
        assert!(
            (a.conviction - b.conviction).abs() < 1e-15,
            "{} gave {} then {}",
            a.agent_id,
            a.conviction,
            b.conviction
        );
        assert_eq!(a.facts, b.facts);
    }
    Ok(())
}

#[test]
fn the_report_reports_coverage_rather_than_implying_a_consensus() -> Result<()> {
    let mut org = organisation(populated_desk())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    // Coverage is a fraction of the agents that actually answered, so a
    // confident-looking balance from two agents is visibly thin.
    assert!(report.coverage() > 0.0 && report.coverage() <= 1.0);
    assert!(report.balance() >= -1.0 && report.balance() <= 1.0);
    Ok(())
}

#[test]
fn a_run_that_is_refused_still_leaves_a_record() -> Result<()> {
    let mut org = organisation(populated_desk())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    assert_eq!(org.audit().len(), report.runs.len());
    for record in org.audit().records() {
        assert!(!record.manifest_digest.is_empty());
        assert!(!matches!(record.status, RunStatus::Refused { .. }));
    }
    Ok(())
}

// --- the REASON stage, end to end -------------------------------------------

#[test]
fn the_organisation_feeds_the_reasoning_engine() -> Result<()> {
    // The Phase 5 milestone: a question goes to eighteen governed agents, their
    // findings become evidence, the evidence produces a confidence, and the red
    // team attacks the result before anyone could act on it.
    use qip_reasoning_engine::engine::{ReasoningEngine, SynthesisInput};
    use qip_reasoning_engine::evidence::{Evidence, EvidenceKind, EvidenceSet, Stance};
    use qip_reasoning_engine::hypothesis::{CausalChain, CausalStep, Claim, UncertaintyType};
    use qip_reasoning_engine::redteam::ReviewPolicy;
    use qip_world_model::causal::Mechanism;

    let mut org = organisation(populated_desk())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    assert!(
        !report.findings.is_empty(),
        "the organisation produced nothing to reason about"
    );

    let chain = CausalChain::new(vec![
        CausalStep::new(
            "policy-rate",
            "funding-cost",
            Mechanism::CreditConditions,
            "commercial paper rates rise",
            Duration::from_days(2),
            0.85,
        ),
        CausalStep::new(
            "funding-cost",
            object("ACME").as_str(),
            Mechanism::CreditConditions,
            "gross margin compresses in the next quarterly report",
            Duration::from_days(20),
            0.80,
        ),
    ]);

    let direct = EvidenceSet::from_items(vec![
        Evidence::new(
            qip_core::ids::EvidenceId::from_string("ev-filing"),
            EvidenceKind::Filing,
            Stance::Supports,
            "the 10-Q discloses floating-rate debt at 40% of the capital structure",
            "rec-10q",
            "sec-edgar",
            now().saturating_sub(Duration::from_days(5)),
            now().saturating_sub(Duration::from_days(5)),
        )
        .with_diagnosticity(0.8),
        Evidence::new(
            qip_core::ids::EvidenceId::from_string("ev-market"),
            EvidenceKind::MarketObservation,
            Stance::Supports,
            "the issuer's spread has widened 60bp over the quarter",
            "rec-spread",
            "exchange",
            now(),
            now(),
        )
        .with_diagnosticity(0.6),
    ]);

    let mut engine = ReasoningEngine::new(ReviewPolicy::default());
    let outcome = engine.reason(SynthesisInput {
        hypothesis_id: qip_core::ids::HypothesisId::from_string("hyp-e2e"),
        opportunity_id: None,
        as_of: now(),
        now: now(),
        class: "funding-cost-pass-through".to_string(),
        claim: Claim::Overvalued,
        statement: "ACME's guidance does not reflect its floating-rate funding".to_string(),
        subjects: vec![object("ACME")],
        chain,
        findings: report.findings.clone(),
        direct_evidence: direct,
        prior: 0.25,
        uncertainty_type: UncertaintyType::Epistemic,
        falsifiers: vec!["the next quarterly report shows flat gross margin".to_string()],
        leading_alternative:
            "the market already knows the funding structure and has priced the margin path"
                .to_string(),
        horizon: Duration::from_days(60),
        market_priced_in: Some(0.2),
        models: Vec::new(),
    })?;

    // The chain from observation to reviewed thesis is intact and inspectable.
    assert!(!outcome.hypothesis.contributors.is_empty());
    assert!(outcome.hypothesis.confidence > 0.0);
    assert!(outcome.narrate().contains("Review:"));
    assert!(
        outcome.hypothesis.validate().is_ok(),
        "the hypothesis must satisfy its own invariants"
    );

    // And every number in it traces to a computation or an observation.
    for finding in &report.findings {
        for fact in &finding.facts {
            fact.validate()?;
        }
    }
    Ok(())
}

// --- precedent: cited, never counted ----------------------------------------

fn precedent() -> BriefPrecedent {
    BriefPrecedent::new(
        PrecedentDigest {
            nearest: 4,
            resolved: 3,
            agreeing: 1,
            agreement: Some(1.0 / 3.0),
            surprising: 0,
            worst_surprise_bps: None,
        },
        0.91,
        Some(false),
        Duration::from_days(9),
    )
    .expect("a valid precedent")
}

#[test]
fn two_briefs_identical_except_for_the_precedent_produce_identical_convictions_and_confidence()
-> Result<()> {
    // The leak this guards: a precedent that reached the panel as prose in
    // `context` would be substring-matched by the reviewer's lesson matcher
    // and could raise an objection, and an objection is a quarter of a
    // conviction. The typed field lets the panel cite the precedent and
    // nothing more, so every number the panel reports — and the confidence
    // the reasoning engine computes from those numbers — must be the same
    // with the precedent as without it, bit for bit.
    use qip_reasoning_engine::engine::{ReasoningEngine, SynthesisInput};
    use qip_reasoning_engine::evidence::EvidenceSet;
    use qip_reasoning_engine::hypothesis::{CausalChain, CausalStep, Claim, UncertaintyType};
    use qip_reasoning_engine::redteam::ReviewPolicy;
    use qip_world_model::causal::Mechanism;

    let plain = brief().with_context("credit spreads widened 40bp over the month");
    let briefed = plain.clone().with_precedent(precedent());
    assert_ne!(plain, briefed, "premise: the briefs differ");
    assert_eq!(
        plain.context, briefed.context,
        "premise: they differ only in the typed field"
    );

    let without = organisation(populated_desk())?.dispatch(&plain, now(), &lineage());
    let with = organisation(populated_desk())?.dispatch(&briefed, now(), &lineage());
    assert!(
        !with.findings.is_empty(),
        "premise: the organisation produced findings"
    );
    let cites = |report: &qip_investment_agents::OrganisationReport| {
        report
            .findings
            .iter()
            .filter(|f| {
                f.claim.contains("precedent:") || f.caveats.iter().any(|c| c.contains("precedent:"))
            })
            .map(|f| f.agent_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        cites(&with),
        vec![ids::ADVERSARIAL.to_string()],
        "premise: the reviewer cited the precedent in its narrative"
    );
    assert!(
        cites(&without).is_empty(),
        "a brief with no precedent produced a citation of one"
    );

    assert_eq!(with.findings.len(), without.findings.len());
    for (a, b) in with.findings.iter().zip(&without.findings) {
        assert_eq!(a.agent_id, b.agent_id);
        assert_eq!(a.status, b.status, "{}", a.agent_id);
        assert_eq!(a.direction, b.direction, "{}", a.agent_id);
        assert!(
            a.conviction.to_bits() == b.conviction.to_bits(),
            "{} reported conviction {} with a precedent and {} without",
            a.agent_id,
            a.conviction,
            b.conviction
        );
        assert_eq!(a.facts, b.facts, "{}", a.agent_id);
        assert_eq!(
            a.evidence, b.evidence,
            "{} cited the precedent as evidence, which the engine counts",
            a.agent_id
        );
    }

    let synthesis = |findings: Vec<qip_agents::finding::AgentFinding>| SynthesisInput {
        hypothesis_id: qip_core::ids::HypothesisId::from_string("hyp-precedent"),
        opportunity_id: None,
        as_of: now(),
        now: now(),
        class: "funding-cost-pass-through".to_string(),
        claim: Claim::Overvalued,
        statement: "ACME's guidance does not reflect its floating-rate funding".to_string(),
        subjects: vec![object("ACME")],
        chain: CausalChain::new(vec![CausalStep::new(
            "policy-rate",
            object("ACME").as_str(),
            Mechanism::CreditConditions,
            "gross margin compresses",
            Duration::from_days(20),
            0.80,
        )]),
        findings,
        direct_evidence: EvidenceSet::from_items(Vec::new()),
        prior: 0.25,
        uncertainty_type: UncertaintyType::Epistemic,
        falsifiers: vec!["the next quarterly report shows flat gross margin".to_string()],
        leading_alternative: "the market has priced the margin path".to_string(),
        horizon: Duration::from_days(60),
        market_priced_in: None,
        models: Vec::new(),
    };
    let mut engine = ReasoningEngine::new(ReviewPolicy::default());
    let reasoned_with = engine.reason(synthesis(with.findings.clone()))?;
    let mut engine = ReasoningEngine::new(ReviewPolicy::default());
    let reasoned_without = engine.reason(synthesis(without.findings.clone()))?;
    assert!(
        reasoned_with.hypothesis.confidence > 0.0,
        "premise: the findings produced a confidence"
    );
    assert!(
        reasoned_with.hypothesis.effective_confidence().to_bits()
            == reasoned_without.hypothesis.effective_confidence().to_bits(),
        "the precedent moved the confidence: {} with, {} without",
        reasoned_with.hypothesis.effective_confidence(),
        reasoned_without.hypothesis.effective_confidence()
    );
    Ok(())
}

// --- the reduction distinction ----------------------------------------------

/// A desk whose only interesting facility is the risk view.
fn desk_with_risk(state: RiskState, limits: LimitSet) -> Arc<Desk> {
    Arc::new(Desk::new(
        MarketView {
            snapshot: MarketSnapshot::new(now()),
            universe: Universe::new(),
        },
        WorldModel::new(),
        BookView {
            portfolio: Portfolio::new(
                PortfolioId::from_string("pf-1"),
                "test",
                Currency::USD,
                dec!("1000000"),
                now(),
            ),
            marks: BTreeMap::new(),
        },
        RiskView { state, limits },
        ComplianceView::default(),
        ResearchMemory::new(),
        SearchIndex::new(),
    ))
}

/// The claim `RiskControl` makes about a book, through the host that charges
/// and audits the run — not by calling the predicate underneath it.
fn risk_control_claim(state: RiskState, limits: LimitSet, run: &str) -> String {
    use qip_investment_agents::control::RiskControl;

    let agent = RiskControl::new(
        manifests::risk_control(now()),
        desk_with_risk(state, limits),
    );
    let record = AgentHost::new(1).run(
        &agent,
        &brief(),
        now(),
        lineage(),
        AgentRunId::from_string(run),
    );
    record
        .finding
        .expect("the risk control always reaches a finding")
        .claim
}

#[test]
fn a_breach_the_desk_marked_as_forcing_a_reduction_is_reported_as_more_than_a_block() -> Result<()>
{
    // `LimitCheck::requires_reduction` is
    // `any(|b| b.blocks() && b.forces_reduction)`, which implies
    // `LimitCheck::is_blocked` — `any(LimitBreach::blocks)`. `RiskControl` is
    // the only production caller of either, and it asked `is_blocked` first,
    // so the reduction arm was unreachable code. Every
    // `Limit::forcing_reduction()` mark in the platform was therefore inert,
    // including the ones on the shipped `leverage`, `drawdown` and
    // `daily-loss` limits: a desk reading a finding could not tell "stop
    // taking new risk" from "unwind what you are holding".
    //
    // The two limit sets below differ in exactly one byte of intent — the
    // `.forcing_reduction()` call — over the same book, so nothing else can
    // account for a difference in what is reported.
    let breached = || RiskState {
        equity: dec!("1000000"),
        cash: dec!("1000000"),
        gross_exposure: dec!("2000000"),
        ..RiskState::default()
    };
    let plain = LimitSet::new("plain").with(
        Limit::new("leverage", LimitKind::MaxLeverage { limit: 1.5 })
            .with_rationale("gross exposure beyond this cannot be unwound in a stress"),
    );
    let forcing = LimitSet::new("forcing").with(
        Limit::new("leverage", LimitKind::MaxLeverage { limit: 1.5 })
            .forcing_reduction()
            .with_rationale("gross exposure beyond this cannot be unwound in a stress"),
    );

    // Premise: the book breaches both sets and blocks under both, so the
    // difference asserted below is the mark and not the breach.
    for (label, set) in [("plain", &plain), ("forcing", &forcing)] {
        let check = set.check(&breached());
        assert!(
            check.is_blocked(),
            "the premise failed: leverage of 2.0 against a cap of 1.5 did not block \
             under the {label} set"
        );
    }
    assert!(
        !plain.check(&breached()).requires_reduction(),
        "the premise failed: the unmarked set already demands a reduction, so the \
         mark is not what the assertion below measures"
    );

    // Matched on the delimiter, not on a shared prefix: "1 limit(s) block" is
    // a prefix of both sentences, so a bare `contains` would be true of the
    // unmarked set too and the test would pass with the branches back in
    // their unreachable order.
    let plain_claim = risk_control_claim(breached(), plain, "run-reduction-plain");
    assert!(
        plain_claim.starts_with("1 limit(s) block: "),
        "an unmarked blocking breach is no longer reported as a plain block: \
         {plain_claim}"
    );

    let forcing_claim = risk_control_claim(breached(), forcing, "run-reduction-forcing");
    assert!(
        forcing_claim
            .starts_with("1 limit(s) block and 1 of them require the book to be reduced: "),
        "a limit the desk marked as forcing a reduction was reported as an ordinary \
         block, so the mark still changes nothing: {forcing_claim}"
    );
    Ok(())
}

// --- the causal analyst, on a desk that holds causal claims ------------------

/// A claim recorded five days before the brief, from `cause` to `effect`.
fn causal_claim(
    cause: &str,
    effect: &str,
    mechanism: qip_world_model::Mechanism,
    strength: f64,
    evidence: &[&str],
) -> qip_world_model::CausalEdge {
    qip_world_model::CausalEdge::new(
        cause,
        effect,
        mechanism,
        strength,
        Duration::from_days(2),
        now().saturating_sub(Duration::from_days(5)),
    )
    .unwrap()
    .with_evidence(evidence.iter().map(|id| (*id).to_string()).collect())
}

/// What the causal analyst reported for the standard brief on `desk`.
fn causal_finding(desk: Arc<Desk>) -> Result<qip_agents::finding::AgentFinding> {
    let mut org = organisation(desk)?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let run = report
        .runs
        .iter()
        .find(|run| run.agent_id == ids::CAUSAL)
        .expect("the causal analyst accepted a brief naming an entity and an instrument");
    Ok(run
        .finding
        .clone()
        .expect("the causal analyst's run produced a finding"))
}

#[test]
fn the_causal_analyst_cites_the_claim_a_shock_travelled_along_and_the_records_it_rests_on()
-> Result<()> {
    // REASON-006. Two claims join the policy entity to ACME: a weak sentiment
    // link nobody evidenced, and a strong discount-rate link resting on two
    // records. The shock travels the strong one. Until the effect carried the
    // edges it travelled, the analyst looked the pair up again by name, found
    // both claims, and reported the path as unevidenced on the strength of
    // the one the number never came from — while citing none of the records
    // behind the one it did.
    use qip_world_model::Mechanism;
    let origin = "entity-policy";
    let target = object("ACME");
    let weak_and_bare = causal_claim(origin, target.as_str(), Mechanism::Sentiment, 0.2, &[]);
    let strong = |evidence: &[&str]| {
        causal_claim(
            origin,
            target.as_str(),
            Mechanism::DiscountRate,
            0.9,
            evidence,
        )
    };

    let finding = causal_finding(desk_holding_causal_claims(vec![
        weak_and_bare.clone(),
        strong(&["filing-rate-sensitivity", "study-duration"]),
    ]))?;
    // The premise: the analyst traced a path and took a view. Before the
    // floor was read as a fraction of the shock it never could — a one
    // percent reference shock is below an absolute floor of 0.02 before it
    // has crossed a single edge, so every desk got "no data" here.
    assert_eq!(finding.status, FindingStatus::Complete, "{}", finding.claim);
    assert_eq!(finding.direction, Direction::Positive, "{}", finding.claim);

    // The edge travelled, then that edge's own records. Nothing of the other.
    assert_eq!(
        finding.evidence,
        [
            "causal:entity-policy->obj-ACME",
            "filing-rate-sensitivity",
            "study-duration"
        ]
    );
    assert!(
        finding.caveats.iter().all(|c| !c.contains("unevidenced")),
        "the path was called unevidenced because of a claim it did not travel: {:?}",
        finding.caveats
    );
    assert!(
        finding.missing_inputs.is_empty(),
        "{:?}",
        finding.missing_inputs
    );

    // And the caveat still fires when the claim travelled is the bare one,
    // which makes the finding partial: the evidence for that edge is an
    // input the analyst wanted and did not have.
    let bare = causal_finding(desk_holding_causal_claims(vec![weak_and_bare, strong(&[])]))?;
    assert_eq!(bare.status, FindingStatus::Partial, "{}", bare.claim);
    assert_eq!(
        bare.missing_inputs,
        ["evidence for causal edge entity-policy -> obj-ACME"]
    );
    assert_eq!(bare.evidence, ["causal:entity-policy->obj-ACME"]);
    assert!(
        bare.caveats
            .iter()
            .any(|c| c == "unevidenced causal claims on the path: entity-policy -> obj-ACME"),
        "{:?}",
        bare.caveats
    );
    Ok(())
}

#[test]
fn the_causal_analyst_says_whether_a_target_it_could_not_reach_is_downstream_at_all() -> Result<()>
{
    // REASON-006. Propagation stops at four hops and under a floor, so "not
    // reached" used to cover two different findings: the graph holds no
    // mechanism, and the graph holds one too long or too weak to act on. The
    // first asks someone to record a claim; the second must not.
    use qip_world_model::Mechanism;
    let target = object("ACME");
    let hop = |cause: &str, effect: &str| {
        causal_claim(cause, effect, Mechanism::SupplyChain, 0.9, &["note"])
    };

    // Five hops: one more than the analyst will trace.
    let far = causal_finding(desk_holding_causal_claims(vec![
        hop("entity-policy", "a"),
        hop("a", "b"),
        hop("b", "c"),
        hop("c", "d"),
        hop("d", target.as_str()),
    ]))?;
    assert_eq!(far.status, FindingStatus::NoView, "{}", far.claim);
    assert!(
        far.claim
            .starts_with("obj-ACME is downstream of entity-policy along 1 path(s), "),
        "{}",
        far.claim
    );

    // A mechanism out of the entity that leads somewhere else entirely.
    let elsewhere = causal_finding(desk_holding_causal_claims(vec![hop("entity-policy", "a")]))?;
    assert_eq!(
        elsewhere.status,
        FindingStatus::NoView,
        "{}",
        elsewhere.claim
    );
    assert!(
        elsewhere
            .claim
            .starts_with("obj-ACME is not downstream of entity-policy in the causal graph"),
        "{}",
        elsewhere.claim
    );
    Ok(())
}

/// The finding a run produced, refused as an error rather than unwrapped.
fn finding_of(
    record: qip_agents::runtime::AgentRunRecord,
) -> Result<qip_agents::finding::AgentFinding> {
    record.finding.ok_or_else(|| {
        qip_core::error::Error::invalid("the run produced no finding; read its status")
    })
}

#[test]
fn an_analyst_abstains_on_every_desk_instrument_outside_its_asset_class_remit() -> Result<()> {
    use qip_agents::runtime::Agent;
    use qip_investment_agents::analysts::{CommoditiesAnalyst, EquityAnalyst};

    // EXPAND-054. The question text is not what an analyst reads -- it reads
    // the brief's objects -- so a brief about nothing defers for the wrong
    // reason ("needs an instrument") whatever its prose says. This test
    // therefore names a real instrument of the wrong class, walks every such
    // instrument the desk holds, and asserts the remit reason, so it fails if
    // the asset-class check goes and the analyst answers instead.
    let desk = populated_desk();
    let equity: Box<dyn Agent> = Box::new(EquityAnalyst::new(
        manifests::equity_analyst(now()),
        desk.clone(),
    ));
    let commodities: Box<dyn Agent> = Box::new(CommoditiesAnalyst::new(
        manifests::commodities_analyst(now()),
        desk,
    ));
    let cases: [(&dyn Agent, &[&str], &str); 2] = [
        (
            equity.as_ref(),
            &["WTI", "EURUSD"],
            "outside the equity remit",
        ),
        (
            commodities.as_ref(),
            &["ACME", "EURUSD"],
            "outside the commodity remit",
        ),
    ];
    let mut checked = 0;
    for (n, (agent, outside, reason)) in cases.into_iter().enumerate() {
        for symbol in outside {
            let brief = AgentBrief::new("what is the view", now(), Duration::from_hours(24))
                .about_objects(vec![object(symbol)]);
            let finding = finding_of(AgentHost::new(10 + n as u64).run(
                agent,
                &brief,
                now(),
                lineage(),
                AgentRunId::from_string(format!("run-remit-{n}-{symbol}")),
            ))?;
            assert_eq!(
                finding.status,
                FindingStatus::Deferred,
                "{} answered on {symbol}: {}",
                finding.agent_id,
                finding.claim
            );
            assert!(
                finding.claim.ends_with(&format!(" {reason}")),
                "{} deferred on {symbol} for a reason other than its remit: {}",
                finding.agent_id,
                finding.claim
            );
            assert!(is_exactly_zero(finding.effective_conviction()));
            checked += 1;
        }
    }
    // The premise: the walk visited every off-remit instrument named above.
    assert_eq!(checked, 4);
    Ok(())
}

#[test]
fn the_credit_analyst_abstains_on_a_brief_about_no_instrument() -> Result<()> {
    use qip_investment_agents::analysts::CreditAnalyst;

    // The credit analyst's only declared abstention is a brief with no
    // instrument; it has no asset-class remit check (an equity issuer's
    // spread is a legitimate credit question), so this is the whole of its
    // boundary today and the test claims no more than that.
    let agent = CreditAnalyst::new(manifests::credit_analyst(now()), populated_desk());
    let brief = AgentBrief::new(
        "if spreads widen by 50bps, what happens",
        now(),
        Duration::from_hours(24),
    );
    let finding = finding_of(AgentHost::new(3).run(
        &agent,
        &brief,
        now(),
        lineage(),
        AgentRunId::from_string("run-credit-nothing"),
    ))?;
    assert_eq!(finding.status, FindingStatus::Deferred, "{}", finding.claim);
    assert_eq!(
        finding.claim,
        "a credit view needs an instrument to be about"
    );
    assert!(is_exactly_zero(finding.effective_conviction()));
    Ok(())
}

// --- scope coverage: what each specialist alone covers ----------------------

/// The standard organisation with the agent `without` left out, under the
/// standard roster and the same host settings [`organisation`] uses.
fn organisation_without(desk: Arc<Desk>, without: &str) -> Result<Organisation> {
    let agents: Vec<_> = qip_investment_agents::chief::standard_agents(
        desk,
        now(),
        vec!["web-traffic".to_string()],
        false,
    )
    .into_iter()
    .filter(|agent| agent.manifest().id != without)
    .collect();
    Organisation::new(
        manifests::roster(now()),
        agents,
        AgentHost::new(42).with_language_model(Arc::new(DeterministicModel::new())),
        now(),
    )
}

/// Dispatch `brief` to the standard organisation and to the same organisation
/// without `agent`, and return both reports' covered scopes.
fn scopes_with_and_without(
    desk: Arc<Desk>,
    brief: &AgentBrief,
    agent: &str,
) -> Result<(BTreeSet<Scope>, BTreeSet<Scope>)> {
    let mut whole = organisation(desk.clone())?;
    let mut less = organisation_without(desk, agent)?;
    assert_eq!(
        less.len() + 1,
        whole.len(),
        "premise: exactly one agent, {agent}, is left out"
    );
    let with = whole.dispatch(brief, now(), &lineage()).covered_scopes();
    let without = less.dispatch(brief, now(), &lineage()).covered_scopes();
    Ok((with, without))
}

fn fact_value(finding: &qip_agents::finding::AgentFinding, label: &str) -> Option<f64> {
    finding
        .facts
        .iter()
        .find(|fact| fact.label == label)
        .map(|fact| fact.value)
}

/// R12-REASON-3014. The microstructure analyst times the queue at each touch
/// from the tape, and nobody else does: without it the organisation has no
/// view of queue dynamics at all. The figure is checked against the fixture,
/// not merely for presence — a 5,000-unit touch against a daily bar of one
/// million units is 432 seconds of tape — so a depletion computed from the
/// wrong level or the wrong interval fails here.
#[test]
fn removing_the_microstructure_analyst_drops_queue_dynamics_from_coverage() -> Result<()> {
    let desk = populated_desk();
    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::MICROSTRUCTURE)
        .expect("premise: the microstructure analyst reported on ACME");
    for label in ["bid_queue_depletion_secs", "ask_queue_depletion_secs"] {
        let seconds = fact_value(finding, label).expect(label);
        assert!(
            (seconds - 432.0).abs() < 1e-6,
            "{label} is {seconds}, not 5000 units at 1,000,000 units a day"
        );
    }

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::MICROSTRUCTURE)?;
    assert!(with.contains(&Scope::QueueDynamics), "{with:?}");
    assert!(!without.contains(&Scope::QueueDynamics), "{without:?}");
    Ok(())
}

/// R12-REASON-3023, the poisoning half. The detector is constructed in the
/// standard organisation, scans the fabric the desk holds, and is the only
/// agent that covers source poisoning. Its finding is ambiguous rather than
/// negative even when it finds a campaign — see `poisoning.rs` — and here,
/// over one clean claim, it reports a scan that found nothing, which is
/// still coverage: a fabric was examined.
#[test]
fn removing_the_poisoning_detector_drops_source_poisoning_from_coverage() -> Result<()> {
    let desk = populated_desk();
    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::POISONING)
        .expect("premise: the standard organisation runs the poisoning detector");
    assert_eq!(fact_value(finding, "claims_scanned"), Some(1.0));
    assert_eq!(fact_value(finding, "poisoning_findings"), Some(0.0));

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::POISONING)?;
    assert!(with.contains(&Scope::SourcePoisoning), "{with:?}");
    assert!(!without.contains(&Scope::SourcePoisoning), "{without:?}");
    Ok(())
}

#[test]
fn an_agent_outside_the_roster_does_not_assemble() {
    // The roster without the poisoning detector, and the agents with it: the
    // detector's manifest would never have been reviewed.
    let roster = qip_agents::governance::Roster::from_manifests(
        manifests::roster(now())
            .iter()
            .filter(|manifest| manifest.id != ids::POISONING)
            .cloned()
            .collect(),
    );
    let agents =
        qip_investment_agents::chief::standard_agents(populated_desk(), now(), Vec::new(), false);
    assert!(
        agents.iter().any(|a| a.manifest().id == ids::POISONING),
        "premise: the standard agents include the detector"
    );
    let error = Organisation::new(roster, agents, AgentHost::new(1), now()).unwrap_err();
    assert!(
        error
            .message()
            .contains("agent poisoning-detector is not on the roster"),
        "{}",
        error.message()
    );
}

/// R12-RISK-3021. The risk control forecasts the book's tail, generates a
/// stress day and recommends what to shed, and nothing else on the desk
/// covers any of the three. The book is at 3x against a 2x leverage ceiling,
/// so the recommendation is exactly a third of gross; the stress day is
/// 3 x 20% / sqrt(252) x 3. A fact present with the wrong number fails here
/// as surely as a missing one.
#[test]
fn removing_the_risk_control_drops_forecast_stress_and_recommendation_from_coverage() -> Result<()>
{
    let state = RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        gross_exposure: dec!("30000000"),
        volatility: 0.20,
        value_at_risk: BTreeMap::from([("0.95".to_string(), 0.03), ("0.99".to_string(), 0.05)]),
        expected_shortfall: BTreeMap::from([("0.99".to_string(), 0.07)]),
        ..RiskState::default()
    };
    let limits = LimitSet::new("t").with(Limit::new(
        "leverage",
        LimitKind::MaxLeverage { limit: 2.0 },
    ));
    let desk = desk_with_risk(state, limits);

    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::RISK)
        .expect("premise: the standard organisation runs the risk control");
    let expected = [
        ("forecast_value_at_risk", 0.05),
        ("forecast_expected_shortfall", 0.07),
        ("stress_loss_3sd_day", 3.0 * 0.20 / 252.0_f64.sqrt() * 3.0),
        ("recommended_gross_reduction", 10_000_000.0),
    ];
    for (label, value) in expected {
        let got = fact_value(finding, label).expect(label);
        assert!(
            (got - value).abs() < 1e-6,
            "{label} is {got}, expected {value}"
        );
    }

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::RISK)?;
    for scope in [
        Scope::RiskForecast,
        Scope::StressScenario,
        Scope::RiskRecommendation,
    ] {
        assert!(with.contains(&scope), "{scope:?} not covered: {with:?}");
        assert!(
            !without.contains(&scope),
            "{scope:?} covered without: {without:?}"
        );
    }
    Ok(())
}

// --- the backlog (R12-AGENCY-10063) ------------------------------------------

/// The standard agents with the macro analyst's manifest reviewed `age` ago
/// instead of now, so the host refuses it once `age` passes the review
/// interval; or with the macro analyst absent.
fn agents_with_macro_analyst(
    desk: Arc<Desk>,
    macro_analyst: Option<Timestamp>,
) -> Vec<Arc<dyn qip_agents::runtime::Agent>> {
    let mut agents: Vec<Arc<dyn qip_agents::runtime::Agent>> =
        qip_investment_agents::chief::standard_agents(
            desk.clone(),
            now(),
            vec!["web-traffic".to_string()],
            false,
        )
        .into_iter()
        .filter(|agent| agent.manifest().id != ids::MACRO)
        .collect();
    if let Some(reviewed_at) = macro_analyst {
        agents.insert(
            1,
            Arc::new(qip_investment_agents::analysts::MacroAnalyst::new(
                manifests::macro_analyst(reviewed_at),
                desk,
            )),
        );
    }
    agents
}

fn organisation_of(agents: Vec<Arc<dyn qip_agents::runtime::Agent>>) -> Result<Organisation> {
    Organisation::new(
        manifests::roster(now()),
        agents,
        AgentHost::new(42).with_language_model(Arc::new(DeterministicModel::new())),
        now(),
    )
}

/// What a dispatch said, less the agent under test: who found what in which
/// direction, which scopes that covers, who deferred, who failed.
type DispatchShape = (
    Vec<(String, Direction, FindingStatus)>,
    BTreeSet<Scope>,
    Vec<String>,
    Vec<String>,
);

fn shape(report: &qip_investment_agents::OrganisationReport, except: &str) -> DispatchShape {
    (
        report
            .findings
            .iter()
            .filter(|f| f.agent_id != except)
            .map(|f| (f.agent_id.clone(), f.direction, f.status))
            .collect(),
        report.covered_scopes(),
        report
            .deferred
            .iter()
            .filter(|id| id.as_str() != except)
            .cloned()
            .collect(),
        report
            .failed
            .iter()
            .filter(|id| id.as_str() != except)
            .cloned()
            .collect(),
    )
}

/// R12-AGENCY-10063: an agent the runtime cannot run degrades to the
/// organisation's bounded backlog, and the dispatch is otherwise identical
/// to one in which that agent does not exist — no other agent fails, no
/// other finding changes direction, no scope is lost beyond the agent's own.
/// Before this the refused run was a `failed` line and nothing else: the
/// work was dropped, and a reader could not tell a dropped run from a kept
/// one. The backlog is bounded: a run deferred past `max_attempts` is
/// dropped and reported, a backlog at capacity refuses rather than grows,
/// and the dispatch goes on either way.
///
/// Mutation: delete the `self.backlog.defer(..)` arm for `RunStatus::Refused`
/// in `Organisation::dispatch` (leave `failed.push`) — the backlog stays
/// empty and the first backlog assertion fails. Verified and restored.
#[test]
fn a_panel_agent_that_cannot_run_is_deferred_and_the_findings_are_identical_to_its_absence()
-> Result<()> {
    let desk = populated_desk();
    let expired = now().saturating_sub(Duration::from_days(200));
    let mut refused = organisation_of(agents_with_macro_analyst(desk.clone(), Some(expired)))?;
    let mut absent = organisation_of(agents_with_macro_analyst(desk.clone(), None))?;
    // Premise: the macro analyst is in the first organisation, not the
    // second, and on a fresh manifest it would run and find something.
    assert_eq!(refused.len(), absent.len() + 1);
    let mut fresh = organisation_of(agents_with_macro_analyst(desk, Some(now())))?;
    let with_fresh = fresh.dispatch(&brief(), now(), &lineage());
    assert!(
        with_fresh.findings.iter().any(|f| f.agent_id == ids::MACRO),
        "premise: a fresh macro analyst finds something on this brief"
    );
    assert!(
        fresh.backlog().is_empty(),
        "a fresh roster deferred something"
    );

    let report = refused.dispatch(&brief(), now(), &lineage());
    let run = report
        .runs
        .iter()
        .find(|run| run.agent_id == ids::MACRO)
        .expect("the macro analyst was dispatched");
    assert!(
        matches!(run.status, RunStatus::Refused { .. }),
        "the expired manifest was not refused: {:?}",
        run.status
    );
    // Degraded to the backlog, and reported as such.
    assert_eq!(report.backlogged, vec![ids::MACRO.to_string()]);
    assert!(
        report.backlog_refused.is_empty(),
        "{:?}",
        report.backlog_refused
    );
    let pending = refused
        .backlog()
        .pending(ids::MACRO)
        .expect("the refused run is on the backlog");
    assert_eq!(pending.attempts, 1);
    assert_eq!(pending.brief.question, brief().question);
    assert!(
        pending.reason.contains("expired") || pending.reason.contains("review"),
        "{}",
        pending.reason
    );
    assert_eq!(refused.backlog().len(), 1);

    // Identical to the agent's absence.
    let without = absent.dispatch(&brief(), now(), &lineage());
    assert_eq!(
        report.failed,
        vec![ids::MACRO.to_string()],
        "another agent failed"
    );
    assert_eq!(shape(&report, ids::MACRO), shape(&without, ids::MACRO));
    assert!(absent.backlog().is_empty());

    // Bounded. The same question again moves the attempt count, not the
    // length; past the bound the run is dropped and reported.
    let max = refused.backlog().max_attempts();
    for attempt in 2..=max {
        let again = refused.dispatch(&brief(), now(), &lineage());
        assert_eq!(again.backlogged, vec![ids::MACRO.to_string()]);
        assert_eq!(refused.backlog().len(), 1, "a repeat grew the backlog");
        assert_eq!(
            refused
                .backlog()
                .pending(ids::MACRO)
                .map(|run| run.attempts),
            Some(attempt)
        );
    }
    let dropped = refused.dispatch(&brief(), now(), &lineage());
    assert!(dropped.backlogged.is_empty());
    assert_eq!(
        dropped.backlog_refused.len(),
        1,
        "{:?}",
        dropped.backlog_refused
    );
    assert!(dropped.backlog_refused[0].contains("dropped from the backlog"));
    assert!(refused.backlog().pending(ids::MACRO).is_none());
    assert_eq!(refused.backlog().refused(), 1);
    assert_eq!(
        dropped.failed,
        vec![ids::MACRO.to_string()],
        "the drop stopped the dispatch"
    );

    // At capacity, a new question is refused rather than queued.
    let expired_again = now().saturating_sub(Duration::from_days(200));
    let mut tiny = organisation_of(agents_with_macro_analyst(
        populated_desk(),
        Some(expired_again),
    ))?
    .with_backlog(qip_agents::runtime::AgentBacklog::new(1)?);
    let first = tiny.dispatch(&brief(), now(), &lineage());
    assert_eq!(first.backlogged, vec![ids::MACRO.to_string()]);
    let other = AgentBrief::new(
        "is ACME mispriced given the rates path",
        now(),
        Duration::from_days(30),
    )
    .about_objects(vec![object("ACME")]);
    let second = tiny.dispatch(&other, now(), &lineage());
    assert!(second.backlogged.is_empty());
    assert_eq!(second.backlog_refused.len(), 1);
    assert!(
        second.backlog_refused[0].contains("its capacity"),
        "{}",
        second.backlog_refused[0]
    );
    assert_eq!(tiny.backlog().len(), 1);
    assert_eq!(
        second.failed,
        vec![ids::MACRO.to_string()],
        "a refused deferral stopped the rest of the organisation"
    );
    Ok(())
}

/// R12-REASON-3023. The self-exploitation probe reads the platform's own
/// fills, and nobody else does: without it the organisation has no view of
/// strategies that exploit the platform itself. The fixture flow lands on
/// both sides of arrival with distinct sizes, so the probe examines four
/// fills and finds nothing — which is still coverage: the flow was probed.
///
/// Mutation: rename the `self_exploitation_findings` fact in
/// `SelfExploitationProbe::analyse` — the fact assertion on it fails first
/// (organisation.rs, `fact_value` returns `None`), and the scope is no
/// longer evidenced so `with.contains` would fail next. Verified and
/// restored.
#[test]
fn removing_the_self_exploitation_probe_drops_self_exploitation_from_coverage() -> Result<()> {
    let desk = populated_desk();
    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::SELF_EXPLOITATION)
        .expect("premise: the standard organisation runs the self-exploitation probe");
    assert_eq!(fact_value(finding, "fills_scanned"), Some(4.0));
    assert_eq!(fact_value(finding, "adverse_fill_share"), Some(0.5));
    assert_eq!(fact_value(finding, "self_exploitation_findings"), Some(0.0));

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::SELF_EXPLOITATION)?;
    assert!(with.contains(&Scope::SelfExploitation), "{with:?}");
    assert!(!without.contains(&Scope::SelfExploitation), "{without:?}");
    Ok(())
}

/// R12-REASON-3023. The regime-break probe compares each instrument's recent
/// window against its prior, and nobody else does. The fixture tape's noise
/// is a fixed rotation with a small drift, so the two windows agree and the
/// probe reports one instrument compared and no break.
///
/// Mutation: rename the `regime_break_findings` fact in
/// `RegimeBreakProbe::analyse` — the fact assertion on it fails first, and
/// the scope is no longer evidenced so `with.contains` would fail next.
/// Verified and restored.
#[test]
fn removing_the_regime_break_probe_drops_regime_breaks_from_coverage() -> Result<()> {
    let desk = populated_desk();
    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::REGIME_BREAK)
        .expect("premise: the standard organisation runs the regime-break probe");
    assert_eq!(fact_value(finding, "instruments_scanned"), Some(1.0));
    assert_eq!(fact_value(finding, "regime_break_findings"), Some(0.0));

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::REGIME_BREAK)?;
    assert!(with.contains(&Scope::RegimeBreak), "{with:?}");
    assert!(!without.contains(&Scope::RegimeBreak), "{without:?}");
    Ok(())
}

/// R12-REASON-3023. The manipulation probe reads each instrument's book and
/// tape for the shapes spoofing, layering and wash trading leave, and nobody
/// else does. The fixture book is plain and its tape's noise carries a move
/// on every bar, so the probe examines the one instrument and finds nothing,
/// which is still coverage: the book and the tape were examined.
///
/// Mutation: rename the `manipulation_findings` fact in
/// `ManipulationProbe::analyse` — the fact assertion fails first and the
/// scope is no longer evidenced. Verified and restored.
#[test]
fn removing_the_manipulation_probe_drops_market_manipulation_from_coverage() -> Result<()> {
    let desk = populated_desk();
    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::MANIPULATION)
        .expect("premise: the standard organisation runs the manipulation probe");
    assert_eq!(fact_value(finding, "instruments_examined"), Some(1.0));
    assert_eq!(fact_value(finding, "manipulation_findings"), Some(0.0));

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::MANIPULATION)?;
    assert!(with.contains(&Scope::MarketManipulation), "{with:?}");
    assert!(!without.contains(&Scope::MarketManipulation), "{without:?}");
    Ok(())
}

/// R12-REASON-3023. The model-failure probe reads the tournament's forecaster
/// records and the drift report, and nobody else does. The fixture holds one
/// scored forecaster in good standing, so the probe examines one model and
/// finds nothing.
///
/// Mutation: rename the `model_failure_findings` fact in
/// `ModelFailureProbe::analyse` — the fact assertion fails first and the
/// scope is no longer evidenced. Verified and restored.
#[test]
fn removing_the_model_failure_probe_drops_model_failure_from_coverage() -> Result<()> {
    let desk = populated_desk();
    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::MODEL_FAILURE)
        .expect("premise: the standard organisation runs the model-failure probe");
    assert_eq!(fact_value(finding, "models_examined"), Some(1.0));
    assert_eq!(fact_value(finding, "model_failure_findings"), Some(0.0));

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::MODEL_FAILURE)?;
    assert!(with.contains(&Scope::ModelFailure), "{with:?}");
    assert!(!without.contains(&Scope::ModelFailure), "{without:?}");
    Ok(())
}

/// R12-REASON-3023. The manipulation probe examines an instrument through
/// either arm: a tape-only instrument through `probe_tape`, a book-only one
/// through `probe_book`. On the standard fixture one instrument has both,
/// so a probe that had lost either arm still counted it examined; here each
/// arm is the only way its instrument counts.
///
/// Mutations, both run: delete the `probe_tape` call in
/// `ManipulationProbe::analyse` — `instruments_examined` reads 1 and the
/// assertion fails naming the tape-only instrument; delete the `probe_book`
/// call — likewise for the book-only one. Each restored byte for byte.
#[test]
fn the_manipulation_probe_examines_a_tape_only_and_a_book_only_instrument() -> Result<()> {
    let mut universe = Universe::new();
    universe.insert(equity("TAPE", "100")).unwrap();
    universe.insert(equity("BOOK", "100")).unwrap();
    let mut snapshot = MarketSnapshot::new(now());
    for bar in bars("TAPE", 12, 0.0) {
        snapshot.apply_bar(bar);
    }
    snapshot.apply_book(book("BOOK"));
    let desk = Arc::new(
        Desk::new(
            MarketView { snapshot, universe },
            WorldModel::new(),
            BookView {
                portfolio: Portfolio::new(
                    PortfolioId::from_string("pf-probe"),
                    "probe",
                    Currency::USD,
                    Decimal::from_int(1_000_000),
                    now(),
                ),
                marks: BTreeMap::new(),
            },
            RiskView {
                state: RiskState::default(),
                limits: LimitSet::conservative_default(),
            },
            ComplianceView::default(),
            ResearchMemory::new(),
            SearchIndex::new(),
        )
        .with_executions(fixture_executions())
        .with_models(fixture_models()),
    );
    let mut org = organisation(desk)?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::MANIPULATION)
        .expect("premise: the manipulation probe ran");
    assert_eq!(
        fact_value(finding, "instruments_examined"),
        Some(2.0),
        "one instrument through the tape arm and one through the book arm: {finding:?}"
    );
    assert_eq!(fact_value(finding, "manipulation_findings"), Some(0.0));
    Ok(())
}

/// R12-REASON-3012. The macro analyst reads the world model's geopolitical
/// claims as well as the macro series, and nobody else does: without it the
/// organisation has no view of geopolitical exposure at all. On a desk
/// holding a sanctions claim landing on ACME the exposure is the claim's
/// strength weighted by its confidence, so a read of the wrong mechanism or
/// the wrong subject reads zero here.
///
/// Mutation: drop the `edge.mechanism == Mechanism::Geopolitical` filter's
/// companion read — replace the `geopolitical` vector in
/// `MacroAnalyst::analyse` with `Vec::new()` — the exposure reads zero and
/// the first assertion fails; the scope is then absent too. Verified and
/// restored.
#[test]
fn removing_the_macro_analyst_drops_geopolitical_risk_from_coverage() -> Result<()> {
    use qip_world_model::Mechanism;
    let desk = desk_holding_causal_claims(vec![
        causal_claim(
            "sanctions:EU",
            object("ACME").as_str(),
            Mechanism::Geopolitical,
            0.6,
            &["ev-sanctions"],
        )
        .with_confidence(0.5)
        .unwrap(),
        // A supply-chain claim on the same subject is not geopolitical and
        // must not count.
        causal_claim(
            "WTI",
            object("ACME").as_str(),
            Mechanism::SupplyChain,
            0.9,
            &["ev-oil"],
        ),
    ]);
    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::MACRO)
        .expect("premise: the standard organisation runs the macro analyst");
    let exposure = fact_value(finding, "geopolitical_risk").expect("the fact is stated");
    assert!(
        (exposure - 0.3).abs() < 1e-9,
        "0.6 strength at 0.5 confidence is 0.3 of exposure, not {exposure}"
    );
    assert_eq!(fact_value(finding, "geopolitical_claims"), Some(1.0));

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::MACRO)?;
    assert!(with.contains(&Scope::GeopoliticalRisk), "{with:?}");
    assert!(!without.contains(&Scope::GeopoliticalRisk), "{without:?}");
    Ok(())
}

/// R12-REASON-3016. The commodities analyst reads the world model's
/// logistics and weather claims and the supplier links behind a commodity,
/// and nobody else does: without it the organisation has no view of
/// supply-chain exposure. On a desk holding a freight claim landing on WTI
/// the exposure is the claim's strength weighted by its confidence; a
/// demand-linkage claim on the same commodity does not count.
///
/// Mutation: replace the `supply_claims` vector in
/// `CommoditiesAnalyst::analyse` with `Vec::new()` — the exposure reads zero
/// and the first assertion fails. Verified and restored.
#[test]
fn removing_the_commodities_analyst_drops_supply_chain_from_coverage() -> Result<()> {
    use qip_world_model::Mechanism;
    let desk = desk_holding_causal_claims(vec![
        causal_claim(
            "freight:baltic-dry",
            object("WTI").as_str(),
            Mechanism::Logistics,
            0.5,
            &["ev-freight"],
        )
        .with_confidence(0.8)
        .unwrap(),
        causal_claim(
            "ACME",
            object("WTI").as_str(),
            Mechanism::DemandLinkage,
            0.9,
            &["ev-demand"],
        ),
    ]);
    let brief = AgentBrief::new("is WTI backwardated", now(), Duration::from_days(30))
        .about_objects(vec![object("WTI")]);
    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief, now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::COMMODITIES)
        .expect("premise: the standard organisation runs the commodities analyst on a commodity");
    let exposure = fact_value(finding, "supply_chain_exposure").expect("the fact is stated");
    assert!(
        (exposure - 0.4).abs() < 1e-9,
        "0.5 strength at 0.8 confidence is 0.4 of exposure, not {exposure}"
    );
    assert_eq!(fact_value(finding, "supplier_links"), Some(0.0));

    let (with, without) = scopes_with_and_without(desk, &brief, ids::COMMODITIES)?;
    assert!(with.contains(&Scope::SupplyChain), "{with:?}");
    assert!(!without.contains(&Scope::SupplyChain), "{without:?}");
    Ok(())
}
