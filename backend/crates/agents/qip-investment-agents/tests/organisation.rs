//! Tests for the investment organisation.
//!
//! The governance tests are the point of this file. An organisation chart is
//! only a control if something fails when it is violated, so most of what
//! follows tries to violate one.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_agents::budget::Budget;
use qip_agents::capability::Capability;
use qip_agents::finding::{AgentBrief, Direction, FindingStatus, NumericProvenance};
use qip_agents::governance::Severity;
use qip_agents::manifest::AgentRole;
use qip_agents::memory::{Episode, EpisodeOutcome, ResearchMemory};
use qip_agents::runtime::{AgentHost, RunStatus};
use qip_ai::language::DeterministicModel;
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
use qip_investment_agents::desk::{BookView, ComplianceView, Desk, MarketView, RiskView};
use qip_investment_agents::{Organisation, ids, manifests};
use qip_market::bar::{Bar, Interval};
use qip_market::book::{BookLevel, OrderBook};
use qip_market::snapshot::MarketSnapshot;
use qip_portfolio::portfolio::Portfolio;
use qip_risk::limits::{LimitSet, RiskState};
use qip_world_model::WorldModel;
use qip_world_model::features::{Feature, FeatureValue};
use qip_world_model::vocabulary::names;
use std::collections::BTreeMap;
use std::sync::Arc;

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
    assert_eq!(roster.len(), 18, "the organisation is eighteen agents");
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
    FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
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
    FinancialObject::builder(object(symbol), symbol, InstrumentType::CommoditySpot)
        .venue("XNYM")
        .sector(Sector::Energy)
        .price(Decimal::parse(price).unwrap())
        .provenance(Provenance::synthetic("test", now()))
        .build(now())
        .expect("valid object")
}

/// A spot currency pair for the carry analyst.
fn fx_pair(symbol: &str, price: &str) -> FinancialObject {
    FinancialObject::builder(object(symbol), symbol, InstrumentType::FxSpot)
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

    let mut library = SearchIndex::new();
    library.add(Document::new(
        "doc-1",
        "ACME reported margin compression driven by floating rate funding costs",
        now().saturating_sub(Duration::from_days(2)),
    ));

    Arc::new(Desk::new(
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
    ))
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
    assert_eq!(org.len(), 18);

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
    use qip_reasoning_engine::hypothesis::{CausalChain, CausalStep, Claim};
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
