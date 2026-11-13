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
    // Twenty-nine: L2b's four probes, the Portfolio/Asset Brain
    // (R12-ASSET-3019), the Crypto/On-chain Brain (R12-ASSET-3017), the
    // Company/Fundamental Brain (R12-REASON-3013), the Meta Brain
    // (R12-AMBIENT-3024) and the World Model Arbitration Brain
    // (R12-WORLD-3025, ADR 0488).
    assert_eq!(roster.len(), 29, "the organisation is twenty-nine agents");
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
    desk_with(claims, false)
}

/// [`populated_desk`] whose book holds 1,000 ACME, flagged by the lifecycle
/// register as held with no mandate on record (R12-ASSET-3019).
fn desk_holding_an_unmandated_position() -> Arc<Desk> {
    desk_with(Vec::new(), true)
}

fn desk_with(claims: Vec<qip_world_model::CausalEdge>, holding: bool) -> Arc<Desk> {
    desk_with_purpose(
        claims,
        if holding {
            Held::Flagged
        } else {
            Held::Nothing
        },
        qip_investment_agents::desk::BookPurpose::default(),
    )
}

/// What the fixture book holds: nothing, ACME flagged for want of a mandate
/// (R12-ASSET-4066), or ACME being divested (R12-ASSET-1160).
#[derive(Clone, Copy, PartialEq)]
enum Held {
    Nothing,
    Flagged,
    Unwinding,
}

fn desk_with_purpose(
    claims: Vec<qip_world_model::CausalEdge>,
    holding: Held,
    purpose: qip_investment_agents::desk::BookPurpose,
) -> Arc<Desk> {
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
                portfolio: {
                    let mut portfolio = Portfolio::new(
                        PortfolioId::from_string("pf-1"),
                        "test",
                        Currency::USD,
                        dec!("10000000"),
                        now(),
                    );
                    if holding != Held::Nothing {
                        let acme = equity("ACME", "100");
                        portfolio
                            .apply_fill(
                                &acme,
                                dec!("1000"),
                                dec!("100"),
                                Decimal::ZERO,
                                now(),
                                None,
                            )
                            .unwrap();
                        // The lifecycle table has no `Held -> Unwinding`
                        // edge: a deliberate close is flagged first.
                        portfolio.flag_position(&object("ACME")).unwrap();
                        if holding == Held::Unwinding {
                            portfolio.begin_unwind(&object("ACME")).unwrap();
                        }
                    }
                    portfolio
                },
                marks: BTreeMap::from([(object("ACME").as_str().to_string(), dec!("100"))]),
                purpose,
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
    // Twenty-eight dispatched since the Meta Brain joined as the one arbiter
    // (R12-AMBIENT-3024) and the World Model Arbitration Brain joined the
    // panel (R12-WORLD-3025, ADR 0488): twenty-seven on the panel and one
    // after it.
    assert_eq!(org.len(), 28);

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
            purpose: qip_investment_agents::desk::BookPurpose::default(),
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
            purpose: qip_investment_agents::desk::BookPurpose::default(),
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
            purpose: qip_investment_agents::desk::BookPurpose::default(),
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
            purpose: qip_investment_agents::desk::BookPurpose::default(),
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
        regime: None,
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
            purpose: qip_investment_agents::desk::BookPurpose::default(),
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
            regime: None,
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
            purpose: qip_investment_agents::desk::BookPurpose::default(),
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
            purpose: qip_investment_agents::desk::BookPurpose::default(),
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
        desk.clone(),
        now(),
        vec!["web-traffic".to_string()],
        false,
    )
    .into_iter()
    .filter(|agent| agent.manifest().id != without)
    .collect();
    // The second phase too (ADR 0466): an arbiter is left out the same way,
    // so removing the Meta Brain is one call like removing any specialist.
    let arbiters: Vec<_> = qip_investment_agents::chief::standard_arbiters(desk, now())
        .into_iter()
        .filter(|arbiter| arbiter.manifest().id != without)
        .collect();
    Organisation::new(
        manifests::roster(now()),
        agents,
        AgentHost::new(42).with_language_model(Arc::new(DeterministicModel::new())),
        now(),
    )?
    .with_arbiters(arbiters)
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

/// R12-CAPITAL-3020 (ADR 0147 decision 8). No capital specialist is added:
/// a second agent over the same `RiskState` would be a correlated second
/// finding from one origin. The risk control reports the capital facts the
/// state already holds instead, so it alone covers the capital position and
/// removing it drops the scope. A figure the state does not hold is not
/// reported, so an unmeasured funding buffer is never read as zero.
///
/// Mutation: drop the capital facts from `RiskControl::analyse` and the
/// scope is not covered with the control present.
#[test]
fn removing_the_risk_control_drops_capital_position_from_coverage() -> Result<()> {
    let state = RiskState {
        equity: dec!("10000000"),
        cash: dec!("4000000"),
        gross_exposure: dec!("6000000"),
        funding_buffer: Some(0.35),
        margin_utilisation: Some(0.42),
        survival_gross_ceiling: Some(dec!("15000000")),
        ..RiskState::default()
    };
    let desk = desk_with_risk(state, LimitSet::new("t"));

    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::RISK)
        .expect("premise: the standard organisation runs the risk control");
    for (label, value) in [
        ("cash", 4_000_000.0),
        ("funding_buffer", 0.35),
        ("margin_utilisation", 0.42),
        ("survival_gross_ceiling", 15_000_000.0),
    ] {
        let got = fact_value(finding, label).expect(label);
        assert!(
            (got - value).abs() < 1e-6,
            "{label} is {got}, expected {value}"
        );
    }

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::RISK)?;
    assert!(
        with.contains(&Scope::CapitalPosition),
        "capital position not covered: {with:?}"
    );
    assert!(
        !without.contains(&Scope::CapitalPosition),
        "capital position covered without the risk control: {without:?}"
    );

    // Only where present: a state that measured no buffer, margin or
    // ceiling reports none of the three rather than a zero for each.
    let bare = desk_with_risk(
        RiskState {
            equity: dec!("10000000"),
            cash: dec!("10000000"),
            ..RiskState::default()
        },
        LimitSet::new("t"),
    );
    let mut org = organisation(bare)?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::RISK)
        .expect("premise: the standard organisation runs the risk control");
    assert!(fact_value(finding, "cash").is_some(), "cash is always held");
    for label in [
        "funding_buffer",
        "margin_utilisation",
        "survival_gross_ceiling",
    ] {
        assert!(
            fact_value(finding, label).is_none(),
            "{label} was reported though the state holds none"
        );
    }
    Ok(())
}

/// R12-ASSET-3019. The Portfolio/Asset Brain reads the held book, and nobody
/// else reports it: a position the lifecycle register flagged for want of a
/// mandate reaches the chief only through it. The figures are checked
/// against the fixture — 1,000 ACME at 100 against ten million of equity is
/// a gross of one hundredth — so a ratio over the wrong base fails here.
///
/// Mutation: drop the `PortfolioAssetAnalyst` from `standard_agents` — the
/// premise finding is missing; or count `Held` instead of `Flagged` — the
/// flagged count reads zero.
#[test]
fn removing_the_portfolio_asset_brain_drops_portfolio_asset_from_coverage() -> Result<()> {
    let desk = desk_holding_an_unmandated_position();
    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::PORTFOLIO_ASSET)
        .expect("premise: the Portfolio/Asset Brain reported on the held book");
    assert_eq!(fact_value(finding, "positions_held"), Some(1.0));
    assert_eq!(
        fact_value(finding, "positions_flagged_without_mandate"),
        Some(1.0),
        "the register flagged ACME and the brain did not say so"
    );
    assert_eq!(fact_value(finding, "positions_unwinding"), Some(0.0));
    let ratio = fact_value(finding, "book_gross_exposure_ratio").expect("equity is positive");
    assert!(
        (ratio - 0.01).abs() < 1e-3,
        "gross {ratio} is not 100,000 against about ten million of equity"
    );

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::PORTFOLIO_ASSET)?;
    assert!(with.contains(&Scope::PortfolioAsset), "{with:?}");
    assert!(!without.contains(&Scope::PortfolioAsset), "{without:?}");
    Ok(())
}

/// R12-ASSET-3019's remaining scope: horizons, hedges and the factor and
/// causal-driver allocation (ADR 0468). The register's purpose record for
/// the one held position — ACME, 1,000 at 100, sized twenty days ago on a
/// ten-day hypothesis, hedged by one held policy, loading 1.2 on a market
/// factor — must reach the chief as figures checked against the fixture,
/// and a book whose purpose nobody stated must read as *undeclared* and
/// *unattributed*, never as unhedged or as allocated.
///
/// Mutations (each fired, see the lane report): compare `elapsed < span`
/// for past-horizon — the count reads zero; file `Hedged` under the
/// unhedged count — hedged reads zero; drop the `purpose` assignment in
/// the kernel's `brief_the_desk_on_book` — covered by the kernel test.
#[test]
fn the_portfolio_asset_brain_reports_horizons_hedges_and_driver_allocation() -> Result<()> {
    use qip_investment_agents::desk::{BookPurpose, HedgeStanding, HeldHorizon};
    let acme = object("ACME").as_str().to_string();
    let declared = |hedge: Option<HedgeStanding>| {
        let mut purpose = BookPurpose::default();
        purpose.horizons.insert(
            acme.clone(),
            HeldHorizon {
                hypothesis: "hyp-acme-margin".to_string(),
                horizon: Duration::from_days(10),
                sized_at: Some(now().saturating_sub(Duration::from_days(20))),
            },
        );
        if let Some(hedge) = hedge {
            purpose.hedges.insert(acme.clone(), hedge);
        }
        purpose
            .factor_exposure
            .insert("equity_market".to_string(), dec!("120000"));
        purpose
    };
    let finding_for = |purpose: BookPurpose| -> Result<qip_agents::finding::AgentFinding> {
        let mut org = organisation(desk_with_purpose(Vec::new(), Held::Flagged, purpose))?;
        let report = org.dispatch(&brief(), now(), &lineage());
        Ok(report
            .findings
            .iter()
            .find(|f| f.agent_id == ids::PORTFOLIO_ASSET)
            .expect("premise: the Portfolio/Asset Brain reported on the held book")
            .clone())
    };

    let hedged = finding_for(declared(Some(HedgeStanding::Hedged(vec![
        "policy-equity-overlay".to_string(),
    ]))))?;
    assert_eq!(fact_value(&hedged, "positions_held"), Some(1.0), "premise");
    assert_eq!(fact_value(&hedged, "positions_without_horizon"), Some(0.0));
    assert_eq!(
        fact_value(&hedged, "positions_horizon_age_unknown"),
        Some(0.0)
    );
    assert_eq!(
        fact_value(&hedged, "positions_past_horizon"),
        Some(1.0),
        "twenty days held on a ten-day horizon is past it"
    );
    let remaining =
        fact_value(&hedged, "nearest_horizon_remaining_fraction").expect("a dated horizon");
    assert!(
        (remaining + 1.0).abs() < 1e-9,
        "ten days past a ten-day horizon leaves -1 of it, not {remaining}"
    );
    assert_eq!(fact_value(&hedged, "positions_hedged"), Some(1.0));
    assert_eq!(fact_value(&hedged, "positions_unhedged"), Some(0.0));
    assert_eq!(fact_value(&hedged, "positions_hedge_undeclared"), Some(0.0));
    assert_eq!(fact_value(&hedged, "causal_drivers_allocated"), Some(1.0));
    assert_eq!(
        fact_value(&hedged, "largest_causal_driver_gross_share"),
        Some(1.0)
    );
    assert_eq!(fact_value(&hedged, "unattributed_gross_share"), Some(0.0));
    assert_eq!(fact_value(&hedged, "factors_loaded"), Some(1.0));
    let factor = fact_value(&hedged, "largest_factor_exposure_ratio").expect("equity is positive");
    assert!(
        (factor - 0.012).abs() < 1e-3,
        "120,000 of factor exposure against about ten million of equity is not {factor}"
    );

    let unhedged = finding_for(declared(Some(HedgeStanding::Unhedged {
        reason: "no hedge policy is declared for this deployment".to_string(),
    })))?;
    assert_eq!(fact_value(&unhedged, "positions_hedged"), Some(0.0));
    assert_eq!(fact_value(&unhedged, "positions_unhedged"), Some(1.0));
    assert_eq!(
        fact_value(&unhedged, "positions_hedge_undeclared"),
        Some(0.0)
    );

    let undeclared = finding_for(BookPurpose::default())?;
    assert_eq!(
        fact_value(&undeclared, "positions_held"),
        Some(1.0),
        "premise"
    );
    assert_eq!(
        fact_value(&undeclared, "positions_hedge_undeclared"),
        Some(1.0)
    );
    assert_eq!(fact_value(&undeclared, "positions_unhedged"), Some(0.0));
    assert_eq!(
        fact_value(&undeclared, "positions_without_horizon"),
        Some(1.0)
    );
    assert_eq!(fact_value(&undeclared, "positions_past_horizon"), Some(0.0));
    assert_eq!(
        fact_value(&undeclared, "causal_drivers_allocated"),
        Some(0.0)
    );
    assert_eq!(
        fact_value(&undeclared, "unattributed_gross_share"),
        Some(1.0),
        "a position sized on no recorded hypothesis is unattributed, not allocated"
    );
    assert!(
        fact_value(&undeclared, "nearest_horizon_remaining_fraction").is_none(),
        "no horizon is on record, so none can be near"
    );
    assert!(fact_value(&undeclared, "largest_factor_exposure_ratio").is_none());
    Ok(())
}

/// R12-ASSET-3019's divestment clause: a position the register marked
/// unwinding (R12-ASSET-1160, `Platform::begin_deliberate_close` on every
/// desk order that sells the whole long) is counted as divested, and not
/// as flagged. The existing coverage test only ever saw zero here, which a
/// brain that never counted would also report.
///
/// Mutation: count `Held` instead of `Unwinding` — the count reads zero.
#[test]
fn the_portfolio_asset_brain_counts_a_position_being_divested() -> Result<()> {
    let desk = desk_with_purpose(
        Vec::new(),
        Held::Unwinding,
        qip_investment_agents::desk::BookPurpose::default(),
    );
    let mut org = organisation(desk)?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::PORTFOLIO_ASSET)
        .expect("premise: the brain reported on the held book");
    assert_eq!(fact_value(finding, "positions_held"), Some(1.0), "premise");
    assert_eq!(fact_value(finding, "positions_unwinding"), Some(1.0));
    assert_eq!(
        fact_value(finding, "positions_flagged_without_mandate"),
        Some(0.0)
    );
    Ok(())
}

/// An empty book is reported as no data, never as a clean one.
#[test]
fn the_portfolio_asset_brain_reports_an_empty_book_as_no_data() -> Result<()> {
    let mut org = organisation(populated_desk())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::PORTFOLIO_ASSET)
        .expect("premise: the brain answered the brief");
    assert_eq!(
        finding.status,
        qip_agents::finding::FindingStatus::NoView,
        "an empty book is no view, not a clean book"
    );
    assert!(
        fact_value(finding, "positions_held").is_none(),
        "an empty book reported a position count as if it had read one"
    );
    assert!(!report.covered_scopes().contains(&Scope::PortfolioAsset));
    Ok(())
}

/// A desk whose chain slot holds an absorbed reorganisation, as the kernel
/// writes it.
fn desk_with_a_reorganised_chain() -> Arc<Desk> {
    use qip_investment_agents::desk::{ChainView, PoolReading};
    Arc::new(Desk::empty(now()).with_chain(ChainView {
        observed: true,
        confirmed_block: Some(40),
        reorgs: 1,
        deepest_reorg: 3,
        bridged_transfers_failed: 2,
        confirmed_trades: Some(17),
        pools: vec![PoolReading {
            pool: "pool-eth-usdc".to_string(),
            reserve_base: qip_core::dec!("100"),
            reserve_quote: qip_core::dec!("250000"),
            trades: 17,
        }],
    }))
}

/// R12-ASSET-3017. The Crypto/On-chain Brain reads the confirmed chain, and
/// nobody else reports it: a reorganisation under the book and the bridge
/// transfers it failed reach the chief only through it.
///
/// Mutation: drop the `CryptoOnChainAnalyst` from `standard_agents` — the
/// premise finding is missing and the scope is not covered.
#[test]
fn removing_the_crypto_onchain_brain_drops_crypto_onchain_from_coverage() -> Result<()> {
    let desk = desk_with_a_reorganised_chain();
    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::CRYPTO_ONCHAIN)
        .expect("premise: the Crypto/On-chain Brain reported on the chain");
    assert_eq!(fact_value(finding, "chain_reorg_depth"), Some(3.0));
    assert_eq!(fact_value(finding, "bridge_transfers_failed"), Some(2.0));
    assert_eq!(fact_value(finding, "amm_pools_with_liquidity"), Some(1.0));
    assert_eq!(fact_value(finding, "onchain_trades"), Some(17.0));

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::CRYPTO_ONCHAIN)?;
    assert!(with.contains(&Scope::CryptoOnChain), "{with:?}");
    assert!(!without.contains(&Scope::CryptoOnChain), "{without:?}");
    Ok(())
}

/// An unobserved chain is reported as no data, never as a quiet one.
///
/// Mutation: drop the `!chain.observed` early return — an unobserved chain
/// reports zero reorgs as if it had read them and this fails.
#[test]
fn the_crypto_onchain_brain_reports_an_unobserved_chain_as_no_data() -> Result<()> {
    let mut org = organisation(populated_desk())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::CRYPTO_ONCHAIN)
        .expect("premise: the brain answered the brief");
    assert_eq!(
        finding.status,
        qip_agents::finding::FindingStatus::NoView,
        "an unobserved chain is no view, not a quiet chain"
    );
    assert!(fact_value(finding, "chain_reorg_depth").is_none());
    assert!(!report.covered_scopes().contains(&Scope::CryptoOnChain));
    Ok(())
}

// --- the Company/Fundamental Brain (R12-REASON-3013) ----------------------

const ACME_ISSUER: &str = "ent-acme";

fn days(n: i64) -> Duration {
    Duration::from_days(n)
}

/// What the company slot and the world model hold about ACME's issuer.
#[derive(Default)]
struct CompanyFixture {
    /// `(reported, consensus, period_end)` for ACME's revenue, absorbed
    /// through the world model's own fundamental absorber.
    revenue: Option<(&'static str, &'static str, Timestamp)>,
    company: qip_investment_agents::desk::CompanyView,
}

/// ACME's issuer as the kernel links it: an `Issues` edge in the knowledge
/// graph, the revenue absorbed by `WorldModel::absorb_fundamental` after the
/// kernel's definition of the metric and its surprise, and `company` behind
/// the company-reference gate.
fn desk_with_company(fixture: CompanyFixture) -> Arc<Desk> {
    use qip_world_model::relationship::{Relationship, RelationshipKind};
    let mut world = WorldModel::new();
    world.graph_mut().assert_fact(
        qip_world_model::graph::Fact::new(
            Relationship::new(
                ACME_ISSUER,
                object("ACME").as_str(),
                RelationshipKind::Issues,
                1.0,
                "reference",
            ),
            now().saturating_sub(days(400)),
            now().saturating_sub(days(400)),
            1.0,
        )
        .unwrap(),
    );
    if let Some((reported, consensus, period_end)) = fixture.revenue {
        for name in ["revenue", "revenue_surprise"] {
            world.features_mut().define(
                Feature::new(name, "reported fundamental", "fundamentals-vendor")
                    .with_lag(days(30))
                    .with_staleness(days(200)),
            );
        }
        world.absorb_fundamental(&qip_financial::intelligence::FundamentalUpdate {
            entity_id: ACME_ISSUER.to_string(),
            metric: "revenue".to_string(),
            value: Decimal::parse(reported).unwrap(),
            unit: "USD".to_string(),
            period_end,
            period: qip_financial::intelligence::FiscalPeriod::Quarter,
            consensus: Some(Decimal::parse(consensus).unwrap()),
            prior_value: None,
            is_restatement: false,
            provenance: Provenance::synthetic("fundamentals-vendor", now().saturating_sub(days(5))),
            quality: qip_financial::quality::DataQuality::default(),
            evidence_unretrievable: false,
        });
    }
    let mut desk = Desk::empty(now()).with_company(fixture.company);
    desk.world = qip_agents::runtime::Gated::new(world, Capability::ReadWorldModel, "world-model");
    Arc::new(desk)
}

fn guidance(
    low: &str,
    high: &str,
    period_end: Timestamp,
) -> qip_financial::company::ManagementGuidance {
    qip_financial::company::ManagementGuidance::new(
        ACME_ISSUER,
        "revenue",
        Decimal::parse(low).unwrap(),
        Decimal::parse(high).unwrap(),
        period_end,
        now().saturating_sub(days(120)),
        "investor-relations",
    )
    .unwrap()
}

/// Every source populated: revenue 121 against a consensus of 110 and
/// management's 100-120 range, a dividend going ex in ten days, and a
/// balance sheet of 100 debt, 20 cash, 40 EBITDA with 30 due in 100 days and
/// 50 in 800.
fn fully_reported_company() -> CompanyFixture {
    let quarter_end = now().saturating_sub(days(40));
    CompanyFixture {
        revenue: Some(("121", "110", quarter_end)),
        company: qip_investment_agents::desk::CompanyView {
            corporate_actions_observed: true,
            corporate_actions: vec![qip_market::corporate_action::CorporateAction {
                object_id: object("ACME"),
                ex_date: now().saturating_add(days(10)),
                record_date: None,
                payment_date: None,
                kind: qip_market::corporate_action::CorporateActionKind::CashDividend {
                    amount: dec!("0.5"),
                },
                announced_at: now().saturating_sub(days(2)),
            }],
            guidance: vec![guidance("100", "120", quarter_end)],
            capital_structures: vec![
                qip_financial::company::CapitalStructure::new(
                    ACME_ISSUER,
                    quarter_end,
                    now().saturating_sub(days(5)),
                    dec!("100"),
                    dec!("20"),
                    Some(dec!("40")),
                    vec![
                        qip_financial::company::DebtMaturity {
                            due: now().saturating_add(days(100)),
                            amount: dec!("30"),
                        },
                        qip_financial::company::DebtMaturity {
                            due: now().saturating_add(days(800)),
                            amount: dec!("50"),
                        },
                    ],
                    "annual-filing",
                )
                .unwrap(),
            ],
        },
    }
}

const COMPANY_SCOPES: [Scope; 5] = [
    Scope::FinancialStatements,
    Scope::Earnings,
    Scope::CorporateEvents,
    Scope::ManagementGuidance,
    Scope::CapitalStructure,
];

fn company_finding(desk: Arc<Desk>) -> Result<qip_agents::finding::AgentFinding> {
    let mut org = organisation(desk)?;
    let report = org.dispatch(&brief(), now(), &lineage());
    Ok(report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::COMPANY_FUNDAMENTAL)
        .expect("premise: the Company/Fundamental Brain answered the brief")
        .clone())
}

/// R12-REASON-3013. The Company/Fundamental Brain is constructed in the
/// production organisation (`Organisation::standard`), emits a finding for
/// each of its five sub-scopes, and nobody else does: without it the
/// organisation covers none of them. Each figure is checked against the
/// fixture, not merely for presence.
///
/// Mutations: drop the analyst from `standard_agents` — the premise finding
/// is missing; compute the guidance gap from the low instead of the
/// midpoint — `guidance_gap` is 0.21, not 0.1; drop the capital-structure
/// arm — the scope is not covered.
#[test]
fn removing_the_company_fundamental_brain_drops_its_five_scopes_from_coverage() -> Result<()> {
    let desk = desk_with_company(fully_reported_company());
    let finding = company_finding(desk.clone())?;
    let close = |label: &str, want: f64| {
        let got = fact_value(&finding, label).unwrap_or(f64::NAN);
        assert!((got - want).abs() < 1e-9, "{label}: {got} != {want}");
    };
    close("statement_metrics_reported", 1.0);
    close("revenue", 121.0);
    close("earnings_surprise", 0.1);
    close("corporate_actions_pending", 1.0);
    close("next_ex_date_days", 10.0);
    close("guidance_midpoint", 110.0);
    close("guidance_gap", 0.1);
    close("net_debt", 80.0);
    close("net_leverage", 2.0);
    close("debt_due_within_year", 0.3);
    assert!(
        finding.missing_inputs.is_empty(),
        "{:?}",
        finding.missing_inputs
    );

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::COMPANY_FUNDAMENTAL)?;
    for scope in COMPANY_SCOPES {
        assert!(
            with.contains(&scope),
            "{scope:?} not covered with: {with:?}"
        );
        assert!(
            !without.contains(&scope),
            "{scope:?} covered without: {without:?}"
        );
    }
    Ok(())
}

/// A figure the brain read off the store is stamped observed with the
/// store's record, not as the brain's own computation; guidance carries its
/// issuer's record.
///
/// Mutation: stamp the earnings surprise with `computed` — the provenance is
/// no longer observed and this fails.
#[test]
fn the_company_fundamental_brain_stamps_what_it_read_as_observed() -> Result<()> {
    let finding = company_finding(desk_with_company(fully_reported_company()))?;
    let provenance = |label: &str| {
        finding
            .facts
            .iter()
            .find(|f| f.label == label)
            .map(|f| f.provenance.clone())
            .expect("premise: the fact was reported")
    };
    match provenance("earnings_surprise") {
        NumericProvenance::Observed { record_id, .. } => {
            assert!(
                record_id.starts_with("feature:revenue_surprise@ent-acme:"),
                "{record_id}"
            );
        }
        other => panic!("earnings surprise stamped {other:?}"),
    }
    match provenance("guidance_midpoint") {
        NumericProvenance::Observed { source, .. } => assert_eq!(source, "investor-relations"),
        other => panic!("guidance midpoint stamped {other:?}"),
    }
    Ok(())
}

/// What every deployed binary holds today: fundamentals the kernel absorbs,
/// and no corporate-action feed, guidance or balance sheet. The brain covers
/// statements and earnings and names the other three missing — never zero
/// actions, never guidance met.
///
/// Mutation: report `corporate_actions_pending` without checking
/// `corporate_actions_observed` — corporate events is covered by a zero
/// nobody measured and this fails.
#[test]
fn the_company_fundamental_brain_names_a_source_it_lacks_missing_rather_than_zero() -> Result<()> {
    let desk = desk_with_company(CompanyFixture {
        revenue: Some(("121", "110", now().saturating_sub(days(40)))),
        company: Default::default(),
    });
    let finding = company_finding(desk.clone())?;
    assert!(fact_value(&finding, "corporate_actions_pending").is_none());
    assert!(fact_value(&finding, "guidance_gap").is_none());
    assert!(fact_value(&finding, "net_debt").is_none());
    for prefix in [
        "corporate events",
        "management guidance",
        "capital structure",
    ] {
        assert!(
            finding.missing_inputs.iter().any(|m| m.starts_with(prefix)),
            "{prefix} not named missing: {:?}",
            finding.missing_inputs
        );
    }
    let mut org = organisation(desk)?;
    let covered = org.dispatch(&brief(), now(), &lineage()).covered_scopes();
    assert!(covered.contains(&Scope::FinancialStatements));
    assert!(covered.contains(&Scope::Earnings));
    for scope in [
        Scope::CorporateEvents,
        Scope::ManagementGuidance,
        Scope::CapitalStructure,
    ] {
        assert!(!covered.contains(&scope), "{scope:?}: {covered:?}");
    }
    Ok(())
}

/// Guidance is judged only against a figure for the period it guided: a
/// revenue reported for another quarter leaves it outstanding.
///
/// Mutation: drop the `valid_at == period_end` filter — the other quarter's
/// revenue is scored against this guidance and `guidance_gap` appears.
#[test]
fn guidance_is_judged_only_against_the_period_it_guided() -> Result<()> {
    let mut fixture = fully_reported_company();
    fixture.company.guidance = vec![guidance("100", "120", now().saturating_add(days(50)))];
    let finding = company_finding(desk_with_company(fixture))?;
    assert!(
        fact_value(&finding, "guidance_midpoint").is_some(),
        "premise: guidance was read"
    );
    assert!(fact_value(&finding, "guidance_gap").is_none());
    assert_eq!(fact_value(&finding, "guidance_outstanding"), Some(1.0));
    Ok(())
}

/// An instrument with no issuer on record is no data, and says that is why,
/// rather than reading as a company with nothing reported.
///
/// Mutation: resolve a missing issuer to a placeholder instead of returning
/// early — the finding is still no view, but its claim no longer names the
/// missing issuer and this fails.
#[test]
fn the_company_fundamental_brain_reports_an_instrument_with_no_issuer_as_no_data() -> Result<()> {
    let finding = company_finding(populated_desk())?;
    assert_eq!(finding.status, FindingStatus::NoView);
    assert!(finding.facts.is_empty());
    assert!(
        finding
            .claim
            .starts_with("no issuer is on record for obj-ACME"),
        "{}",
        finding.claim
    );
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
                purpose: qip_investment_agents::desk::BookPurpose::default(),
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

/// R12-REASON-3023. The manipulation probe *emits* what each arm finds, not
/// only that it looked. The examination test above counts instruments, and a
/// probe that dropped either arm's findings while still marking the
/// instrument examined passed it, because the fixture holds no manipulation
/// shape: deleting `findings.extend(probe_book(..))` and keeping
/// `looked = true` survived every test in this crate on 2026-11-12. Here the
/// book-only instrument's book is crossed and the tape-only instrument's last
/// bar carries nine times the tape's median volume on an unchanged close, so
/// each arm contributes exactly one finding to the count the probe reports.
///
/// Mutations, both run on 2026-11-12: delete the book arm's
/// `findings.extend(probe_book(..))` line, keeping `looked = true`, and
/// `manipulation_findings` reads 1 where 2 was expected; replace the tape
/// arm's `findings.extend(tape)` with nothing, and likewise. Each restored
/// byte for byte.
#[test]
fn the_manipulation_probe_reports_a_crossed_book_and_a_wash_tape_as_two_findings() -> Result<()> {
    let mut universe = Universe::new();
    universe.insert(equity("WASH", "100")).unwrap();
    universe.insert(equity("CROSS", "100")).unwrap();
    let mut snapshot = MarketSnapshot::new(now());
    let mut tape = bars("WASH", 12, 0.0);
    let last = tape.last().cloned().expect("premise: the tape has bars");
    let mut spike = last.clone();
    spike.open_time = last.open_time.saturating_add(Duration::from_hours(1));
    spike.open = last.close;
    spike.high = last.close;
    spike.low = last.close;
    // The same close as the bar before: a zero-basis-point move.
    spike.volume = dec!("9000000");
    spike.vwap = Some(last.close);
    tape.push(spike);
    for bar in tape {
        snapshot.apply_bar(bar);
    }
    snapshot.apply_book(OrderBook::from_levels(
        object("CROSS"),
        "XNYS",
        now(),
        vec![BookLevel::new(dec!("100.10"), dec!("500"))],
        vec![BookLevel::new(dec!("100.05"), dec!("500"))],
    ));
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
                purpose: qip_investment_agents::desk::BookPurpose::default(),
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
        "premise: each instrument is examined through its one arm: {finding:?}"
    );
    assert_eq!(
        fact_value(finding, "manipulation_findings"),
        Some(2.0),
        "one finding from the crossed book and one from the wash tape: {finding:?}"
    );
    Ok(())
}

/// A desk holding only what the three probes below read: `snapshot` over
/// `universe`, and the given models and executions. Every other view is
/// empty, so a finding a probe reports comes from the fixture handed to it.
fn adversary_desk(
    snapshot: MarketSnapshot,
    universe: Universe,
    models: ModelsView,
    executions: ExecutionView,
) -> Arc<Desk> {
    Arc::new(
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
                purpose: qip_investment_agents::desk::BookPurpose::default(),
            },
            RiskView {
                state: RiskState::default(),
                limits: LimitSet::conservative_default(),
            },
            ComplianceView::default(),
            ResearchMemory::new(),
            SearchIndex::new(),
        )
        .with_executions(executions)
        .with_models(models),
    )
}

/// The finding `agent` reported when the standard organisation dispatched
/// over `desk`.
fn probe_finding(desk: Arc<Desk>, agent: &str) -> Result<qip_agents::finding::AgentFinding> {
    let mut org = organisation(desk)?;
    let report = org.dispatch(&brief(), now(), &lineage());
    Ok(report
        .findings
        .into_iter()
        .find(|f| f.agent_id == agent)
        .expect("premise: the standard organisation runs the probe"))
}

/// R12-REASON-3023. The regime-break probe *reports* a break, not only that
/// it compared. The coverage test above runs it on a tape whose windows
/// agree, so a probe that compared every instrument and then threw away
/// every break read as working: deleting `breaks.push(..)` in
/// `RegimeBreakProbe::analyse` survived every test in this crate until
/// 2026-11-12. Here one instrument's last ten daily returns swing six times
/// as wide as its prior thirty — a volatility ratio far past the 2.0 the
/// default thresholds break at — so the probe must report one break.
///
/// Mutation, run 2026-11-12: delete the `breaks.push((instrument.to_string(),
/// comparison));` line — `regime_break_findings` reads 0 where 1 was
/// expected. Restored byte for byte.
#[test]
fn the_regime_break_probe_reports_a_tape_whose_recent_volatility_left_its_prior_band() -> Result<()>
{
    let mut universe = Universe::new();
    universe.insert(equity("LOUD", "100")).unwrap();
    let mut snapshot = MarketSnapshot::new(now());
    let mut close = 100.0_f64;
    for i in 0..41_i64 {
        let open = close;
        if i > 0 {
            let wobble = if i % 2 == 0 { 1.0 } else { -1.0 };
            let amplitude = if i <= 30 { 0.005 } else { 0.03 };
            close = open * (1.0 + wobble * amplitude);
        }
        snapshot.apply_bar(Bar {
            object_id: object("LOUD"),
            venue: "XNYS".to_string(),
            interval: Interval::Day,
            open_time: now().saturating_sub(Duration::from_days(41 - i)),
            open: Decimal::from_f64(open).unwrap(),
            high: Decimal::from_f64(open.max(close)).unwrap(),
            low: Decimal::from_f64(open.min(close)).unwrap(),
            close: Decimal::from_f64(close).unwrap(),
            volume: dec!("1000000"),
            trade_count: 5_000,
            vwap: Decimal::from_f64((open + close) / 2.0),
            quality: qip_financial::quality::DataQuality::default(),
        });
    }
    let desk = adversary_desk(snapshot, universe, fixture_models(), fixture_executions());
    let finding = probe_finding(desk, ids::REGIME_BREAK)?;
    assert_eq!(
        fact_value(&finding, "instruments_scanned"),
        Some(1.0),
        "premise: the tape holds the closes two windows need: {finding:?}"
    );
    assert!(
        fact_value(&finding, "max_volatility_ratio").is_some_and(|ratio| ratio >= 2.0),
        "premise: the fixture's recent window is a volatility break: {finding:?}"
    );
    assert_eq!(
        fact_value(&finding, "regime_break_findings"),
        Some(1.0),
        "the volatility break is reported: {finding:?}"
    );
    Ok(())
}

/// R12-REASON-3023. The model-failure probe reports each failing model, not
/// only that it examined the registry: the coverage test above holds one
/// forecaster in good standing and nothing drifted, so a probe that dropped
/// every finding passed it. Here one forecaster scores a mean Brier of 0.31,
/// worse than a coin's 0.25, and one production model is past its drift
/// threshold, so the probe must report two.
///
/// Mutation, run 2026-11-12: in `ModelFailureProbe::analyse`, replace
/// `probe_models(&models, &self.thresholds)` with an empty `Vec` —
/// `model_failure_findings` reads 0 where 2 was expected. Restored byte
/// for byte.
#[test]
fn the_model_failure_probe_reports_a_forecaster_worse_than_chance_and_a_drifted_model() -> Result<()>
{
    let models = ModelsView {
        forecasters: vec![ForecasterHealth {
            name: "detector:momentum".to_string(),
            mean_brier: Some(0.31),
            predicted_failure: 0.2,
        }],
        drifted: vec!["ridge-ACME@3".to_string()],
    };
    let desk = adversary_desk(
        MarketSnapshot::new(now()),
        Universe::new(),
        models,
        fixture_executions(),
    );
    let finding = probe_finding(desk, ids::MODEL_FAILURE)?;
    assert!(
        fact_value(&finding, "models_examined").is_some_and(|examined| examined >= 1.0),
        "premise: the registry was examined: {finding:?}"
    );
    assert_eq!(
        fact_value(&finding, "model_failure_findings"),
        Some(2.0),
        "the forecaster worse than chance and the drifted model are reported: {finding:?}"
    );
    Ok(())
}

/// R12-REASON-3023. The self-exploitation probe reports a flow the market
/// could be reading, not only that it scanned the fills: the coverage test
/// above holds a balanced flow, so a probe that dropped every finding passed
/// it. Here twelve buys of distinct sizes each fill eight basis points above
/// arrival — every fill adverse — so the probe must report the slippage.
///
/// Mutation, run 2026-11-12: in `SelfExploitationProbe::analyse`, replace
/// `probe_self_exploitation(..)` with an empty `Vec` —
/// `self_exploitation_findings` reads 0 where 1 was expected. Restored byte
/// for byte.
#[test]
fn the_self_exploitation_probe_reports_fills_that_bled_against_arrival() -> Result<()> {
    let bled = ExecutionView {
        executions: (0..12_i64)
            .map(|n| ExecutionRecord {
                order_id: format!("ord-bled-{n}"),
                object_id: object("ACME").as_str().to_string(),
                side: TradeSide::Buy,
                quantity: Decimal::from_int(100 + n),
                arrival_price: dec!("100"),
                fill_price: dec!("100.08"),
                placed_at: now().saturating_sub(Duration::from_days(12 - n)),
                filled_at: now().saturating_sub(Duration::from_days(12 - n)),
                venue: "XSIM".to_string(),
            })
            .collect(),
    };
    let desk = adversary_desk(
        MarketSnapshot::new(now()),
        Universe::new(),
        fixture_models(),
        bled,
    );
    let finding = probe_finding(desk, ids::SELF_EXPLOITATION)?;
    assert_eq!(
        fact_value(&finding, "fills_scanned"),
        Some(12.0),
        "premise: every fill was scanned: {finding:?}"
    );
    assert_eq!(
        fact_value(&finding, "adverse_fill_share"),
        Some(1.0),
        "premise: every fill landed worse than arrival: {finding:?}"
    );
    assert_eq!(
        fact_value(&finding, "self_exploitation_findings"),
        Some(1.0),
        "the adverse slippage is reported: {finding:?}"
    );
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

// --- the Meta Brain (R12-AMBIENT-3024, ADR 0466) -----------------------------

/// A panel agent that leans one way at a fixed conviction, under a rostered
/// manifest, so a test can hold the panel's shape fixed and vary only the
/// record the Meta Brain reads.
#[derive(Debug)]
struct Leaning {
    manifest: qip_agents::manifest::AgentManifest,
    direction: Direction,
    conviction: f64,
}

impl qip_agents::runtime::Agent for Leaning {
    fn manifest(&self) -> &qip_agents::manifest::AgentManifest {
        &self.manifest
    }

    fn analyse(
        &self,
        ctx: &mut qip_agents::runtime::AgentContext,
        brief: &AgentBrief,
    ) -> Result<qip_agents::finding::AgentFinding> {
        qip_investment_agents::support::FindingBuilder::new(ctx, brief.as_of, "a fixed lean")
            .direction(self.direction, self.conviction)
            .fact(qip_investment_agents::support::computed(
                ctx,
                "fixed_lean",
                self.direction.sign(),
                "sign",
                &["fixture"],
            ))
            .evidence(vec!["fixture".to_string()])
            .falsifiers(vec!["the fixture leans the other way".to_string()])
            .build()
    }
}

const TRENDING: &str = "trending/high";
const CALM: &str = "calm/low";

/// Research memory where the macro analyst was right 9 of 10 times in a
/// trending tape and 1 of 10 in a calm one, and the equity analyst the
/// reverse: pooled, the two are identical; by regime, opposite.
fn regime_memory() -> ResearchMemory {
    let mut memory = ResearchMemory::new();
    let mut n = 0i64;
    for (agent, regime, correct) in [
        (ids::MACRO, TRENDING, 9),
        (ids::MACRO, CALM, 1),
        (ids::EQUITY, TRENDING, 1),
        (ids::EQUITY, CALM, 9),
    ] {
        for i in 0..10 {
            n += 1;
            memory.record_episode(Episode {
                run_id: AgentRunId::from_string(format!("run-meta-{n}")),
                agent_id: agent.to_string(),
                occurred_at: now().saturating_sub(Duration::from_days(60)),
                question: "which way".to_string(),
                conclusion: "positive".to_string(),
                evidence: vec![],
                conviction: 0.5,
                regime: Some(regime.to_string()),
                outcome: Some(EpisodeOutcome {
                    resolved_at: now().saturating_sub(Duration::from_days(30)),
                    correct: i < correct,
                    realised_bps: Some(10.0),
                    explanation: "fixture".to_string(),
                }),
            });
        }
    }
    memory
}

fn meta_desk() -> Arc<Desk> {
    let mut desk = Desk::empty(now());
    desk.memory = qip_agents::runtime::Gated::new(
        regime_memory(),
        desk.memory.required(),
        desk.memory.label(),
    );
    Arc::new(desk)
}

/// Two leaning panel agents — macro positive, equity `equity` — and, when
/// `with_meta`, the standard arbiters after them.
fn leaning_organisation(
    desk: Arc<Desk>,
    equity: Direction,
    with_meta: bool,
) -> Result<Organisation> {
    let agents: Vec<Arc<dyn qip_agents::runtime::Agent>> = vec![
        Arc::new(Leaning {
            manifest: manifests::macro_analyst(now()),
            direction: Direction::Positive,
            conviction: 0.6,
        }),
        Arc::new(Leaning {
            manifest: manifests::equity_analyst(now()),
            direction: equity,
            conviction: 0.6,
        }),
    ];
    let arbiters = if with_meta {
        qip_investment_agents::chief::standard_arbiters(desk, now())
    } else {
        Vec::new()
    };
    Organisation::new(manifests::roster(now()), agents, AgentHost::new(42), now())?
        .with_arbiters(arbiters)
}

fn meta_finding(
    report: &qip_investment_agents::OrganisationReport,
) -> &qip_agents::finding::AgentFinding {
    report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::META)
        .expect("the meta brain reported")
}

/// R12-AMBIENT-3024, the disagreement clause. The panel is one positive and
/// one negative finding at equal conviction, so its own balance is zero. The
/// Meta Brain, run after it with its findings, weighs each by its author's
/// record in the regime the brief names: in a trending tape the macro analyst
/// (9 of 10) outweighs the equity analyst (1 of 10) and the resolution is
/// (0.9 - 0.1) / (0.9 + 0.1) = +0.8; in a calm one it is -0.8. Pooled, both
/// are 10 of 20 and the resolution would be zero — the figure a regime-blind
/// reading gives, and the one this test refuses.
#[test]
fn the_meta_brain_resolves_a_contested_panel_by_each_authors_record_in_the_regime() -> Result<()> {
    for (regime, expected) in [(TRENDING, 0.8), (CALM, -0.8)] {
        let mut org = leaning_organisation(meta_desk(), Direction::Negative, true)?;
        let report = org.dispatch(&brief().in_regime(regime), now(), &lineage());
        assert!(report.is_contested(), "premise: the panel disagrees");
        assert!(
            report.balance().abs() < 1e-9,
            "premise: the panel's own balance is zero, got {}",
            report.balance()
        );
        let meta = meta_finding(&report);
        let balance = fact_value(meta, "reliability_weighted_balance")
            .unwrap_or_else(|| panic!("no resolution in {regime}: {}", meta.claim));
        assert!(
            (balance - expected).abs() < 1e-9,
            "{regime}: resolved to {balance}, expected {expected}"
        );
        assert_eq!(
            fact_value(meta, "disagreement_weighed_findings"),
            Some(2.0),
            "{regime}"
        );
        // Attention follows the same record: 0.9 / 1.0 to whichever analyst
        // has been right in this regime.
        let (strong, weak) = if regime == TRENDING {
            (ids::MACRO, ids::EQUITY)
        } else {
            (ids::EQUITY, ids::MACRO)
        };
        let weight = |agent: &str| fact_value(meta, &format!("attention_weight:{agent}"));
        assert!(
            (weight(strong).expect("strong") - 0.9).abs() < 1e-9,
            "{regime}"
        );
        assert!((weight(weak).expect("weak") - 0.1).abs() < 1e-9, "{regime}");
        // A fact, never a vote: the panel's balance is unchanged by it.
        assert_eq!(meta.direction, Direction::Neutral);
        assert!(is_exactly_zero(meta.effective_conviction()));
    }
    Ok(())
}

/// An uncontested panel has nothing to resolve, and the Meta Brain says
/// nothing about resolution — attention and reliability are still reported.
#[test]
fn the_meta_brain_reports_no_resolution_for_a_panel_that_agrees() -> Result<()> {
    let mut org = leaning_organisation(meta_desk(), Direction::Positive, true)?;
    let report = org.dispatch(&brief().in_regime(TRENDING), now(), &lineage());
    assert!(!report.is_contested(), "premise: the panel agrees");
    let meta = meta_finding(&report);
    assert_eq!(fact_value(meta, "reliability_weighted_balance"), None);
    let scopes = report.covered_scopes();
    assert!(
        !scopes.contains(&Scope::DisagreementResolution),
        "{scopes:?}"
    );
    assert!(scopes.contains(&Scope::AttentionAllocation), "{scopes:?}");
    assert!(scopes.contains(&Scope::RegimeReliability), "{scopes:?}");
    Ok(())
}

/// R12-AMBIENT-3024's integration clause: removing the Meta Brain drops
/// attention allocation, regime reliability and disagreement resolution from
/// coverage, because no other agent evidences any of the three.
#[test]
fn removing_the_meta_brain_drops_its_three_scopes_from_coverage() -> Result<()> {
    let three = [
        Scope::AttentionAllocation,
        Scope::RegimeReliability,
        Scope::DisagreementResolution,
    ];
    let mut whole = leaning_organisation(meta_desk(), Direction::Negative, true)?;
    let mut less = leaning_organisation(meta_desk(), Direction::Negative, false)?;
    assert_eq!(
        less.len() + 1,
        whole.len(),
        "premise: only the meta brain is left out"
    );
    let with = whole
        .dispatch(&brief().in_regime(TRENDING), now(), &lineage())
        .covered_scopes();
    let without = less
        .dispatch(&brief().in_regime(TRENDING), now(), &lineage())
        .covered_scopes();
    for scope in three {
        assert!(
            with.contains(&scope),
            "{scope:?} not covered with it: {with:?}"
        );
        assert!(
            !without.contains(&scope),
            "{scope:?} covered without it: {without:?}"
        );
    }

    // And in the production organisation: `Organisation::standard` builds
    // it, and removing it by id is what drops the reliability scope there.
    let mut standard = organisation(meta_desk())?;
    let mut lacking = organisation_without(meta_desk(), ids::META)?;
    assert_eq!(
        lacking.len() + 1,
        standard.len(),
        "premise: the standard organisation holds it"
    );
    let with = standard
        .dispatch(&brief(), now(), &lineage())
        .covered_scopes();
    let without = lacking
        .dispatch(&brief(), now(), &lineage())
        .covered_scopes();
    for scope in [Scope::AttentionAllocation, Scope::RegimeReliability] {
        assert!(with.contains(&scope), "{scope:?}: {with:?}");
        assert!(!without.contains(&scope), "{scope:?}: {without:?}");
    }
    Ok(())
}

/// The second phase admits only a rostered arbiter, and never one also on the
/// panel: it would arbitrate over a finding it had just written.
#[test]
fn an_arbiter_on_the_panel_or_off_the_roster_does_not_assemble() -> Result<()> {
    let desk = meta_desk();
    let meta = qip_investment_agents::chief::standard_arbiters(desk.clone(), now());
    let as_agent: Arc<dyn qip_agents::runtime::Agent> = Arc::new(
        qip_investment_agents::meta_brain::MetaBrain::new(manifests::meta_brain(now()), desk),
    );
    let doubled = Organisation::new(
        manifests::roster(now()),
        vec![as_agent],
        AgentHost::new(1),
        now(),
    )?
    .with_arbiters(meta.clone());
    assert!(doubled.is_err(), "an arbiter also on the panel assembled");
    let unrostered = Organisation::new(
        qip_agents::governance::Roster::from_manifests(
            manifests::roster(now())
                .iter()
                .filter(|manifest| manifest.id != ids::META)
                .cloned()
                .collect(),
        ),
        Vec::new(),
        AgentHost::new(1),
        now(),
    )?
    .with_arbiters(meta);
    assert!(unrostered.is_err(), "an unrostered arbiter assembled");
    Ok(())
}

/// R12-WORLD-3034: an analyst executes its own procedure. Briefed with a
/// procedure whose rule for the brief's subject leans another way than the
/// analyst's present call, the equity analyst's finding records the check as
/// not followed and states the departure as a caveat. Direction and
/// conviction are exactly what the analyst's arithmetic gave with no
/// procedure at all, because a procedure is the record a call is held
/// against and never an input that pulls the call toward past success. A
/// procedure for another agent, or with no rule for the subject, gives no
/// check. The followed arm is `qip-agents`'
/// `a_brief_checks_a_call_against_its_agents_own_procedure_rule`.
///
/// Mutations (2026-11-12, t50-09): `build` in place of `build_against` in
/// `EquityAnalyst::analyse` (no check on the finding); dropping the caveat
/// push in `build_against` (no departure stated).
#[test]
fn an_analyst_checks_its_call_against_its_own_procedure_and_states_a_departure() -> Result<()> {
    use qip_agents::finding::Direction;
    use qip_agents::memory::{Procedure, ProcedureRule, procedure_name_for};
    let analyst = "equity-analyst";
    let equity_finding = |brief: &AgentBrief| -> Result<qip_agents::finding::AgentFinding> {
        let mut org = organisation(populated_desk())?;
        let report = org.dispatch(brief, now(), &lineage());
        Ok(report
            .findings
            .into_iter()
            .find(|f| f.agent_id == analyst)
            .expect("the equity analyst answered"))
    };
    let bare = equity_finding(&brief())?;
    assert!(bare.procedure.is_none(), "premise: no procedure, no check");
    let present = bare.direction;
    let other = if present == Direction::Positive {
        "negative"
    } else {
        "positive"
    };
    let procedure = |owner: &str, subject: &str| Procedure {
        name: procedure_name_for(owner),
        version: 2,
        steps: vec![format!("on {subject}: lean {other} (3 correct call(s))")],
        supporting_episodes: Vec::new(),
        learned_at: now(),
        rules: vec![ProcedureRule {
            subject: subject.to_string(),
            lean: other.to_string(),
            support: 3,
        }],
    };

    let departed = equity_finding(&brief().with_procedures(vec![procedure(analyst, "obj-ACME")]))?;
    let check = departed
        .procedure
        .clone()
        .expect("the analyst checked its procedure");
    assert_eq!(check.name, procedure_name_for(analyst));
    assert_eq!(check.version, 2);
    assert_eq!(check.subject, "obj-ACME");
    assert_eq!(check.procedure_lean, other);
    assert_eq!(check.present_lean, present.as_str());
    assert!(!check.followed, "{check:?}");
    assert!(
        departed.caveats.contains(&check.departure()),
        "no departure stated: {:?}",
        departed.caveats
    );
    assert_eq!(departed.direction, bare.direction);
    assert_eq!(departed.conviction.to_bits(), bare.conviction.to_bits());

    for (owner, subject) in [("macro-analyst", "obj-ACME"), (analyst, "obj-OTHER")] {
        let unchecked = equity_finding(&brief().with_procedures(vec![procedure(owner, subject)]))?;
        assert!(
            unchecked.procedure.is_none(),
            "a procedure of {owner} on {subject} was checked: {:?}",
            unchecked.procedure
        );
        assert_eq!(unchecked.caveats, bare.caveats);
    }
    Ok(())
}

// --- the World Model Arbitration Brain (R12-WORLD-3025, ADR 0488) ---------

fn scored(
    model: &str,
    probability: f64,
    composite: Option<f64>,
    uncertainty: f64,
) -> qip_world_model::federation::ScoredView {
    qip_world_model::federation::ScoredView {
        model: model.to_string(),
        probability,
        evidence_fit: 0.5,
        predictive_performance: composite,
        causal_coherence: 0.5,
        uncertainty,
        composite,
        lineage: qip_world_model::federation::ViewLineage {
            parents: Vec::new(),
            trigger: None,
            depth: 0,
        },
    }
}

fn arbitration(
    proposition: &str,
    decision: qip_world_model::federation::Decision,
    views: Vec<qip_world_model::federation::ScoredView>,
    spread: f64,
) -> qip_world_model::federation::Arbitration {
    qip_world_model::federation::Arbitration {
        proposition: proposition.to_string(),
        at: now(),
        decision,
        reason: "fixture".to_string(),
        disagreement: qip_world_model::federation::Disagreement { views, spread },
        arbitrated_probability: None,
        expired: Vec::new(),
        research: None,
    }
}

/// A desk whose arbitration slot holds a federation review with one
/// arbitration of each kind, as the kernel's UNDERSTAND writes it: each
/// recorded decision is the one the federation's rule reaches on its own
/// views (two close scored views far apart branch; two that agree merge;
/// two too uncertain abstain; one unscored view asks), eight views kept, the
/// widest spread 0.6 on the branch, two members scored (one outside its
/// claimed Brier) and one unscored.
fn federation_review_arbitrations() -> Vec<qip_world_model::federation::Arbitration> {
    use qip_world_model::federation::{Decision, InformationRequest};
    vec![
        arbitration(
            "AAA:next-close-up@1",
            Decision::Branch,
            vec![
                scored("momentum", 0.8, Some(0.60), 0.3),
                scored("reversion", 0.2, Some(0.58), 0.3),
            ],
            0.6,
        ),
        arbitration(
            "BBB:next-close-up@1",
            Decision::Merge,
            vec![
                scored("momentum", 0.55, Some(0.60), 0.3),
                scored("reversion", 0.50, Some(0.55), 0.3),
            ],
            0.05,
        ),
        arbitration(
            "CCC:next-close-up@1",
            Decision::Abstain,
            vec![
                scored("momentum", 0.7, Some(0.5), 0.9),
                scored("reversion", 0.4, Some(0.5), 0.9),
            ],
            0.3,
        ),
        arbitration(
            "DDD:next-close-up@1",
            Decision::RequestMoreInformation(InformationRequest {
                proposition: "DDD:next-close-up@1".to_string(),
                models: vec!["macro".to_string()],
            }),
            vec![
                scored("momentum", 0.7, Some(0.6), 0.3),
                scored("macro", 0.4, None, 0.3),
            ],
            0.3,
        ),
    ]
}

fn desk_with_review(arbitrations: Vec<qip_world_model::federation::Arbitration>) -> Arc<Desk> {
    use qip_investment_agents::desk::ArbitrationView;
    use qip_world_model::federation::Calibration;
    let calibrations = BTreeMap::from([
        (
            "momentum".to_string(),
            Calibration::Brier {
                score: 0.1,
                resolved: 4,
                within_claim: true,
            },
        ),
        (
            "reversion".to_string(),
            Calibration::Brier {
                score: 0.3,
                resolved: 4,
                within_claim: false,
            },
        ),
        ("macro".to_string(), Calibration::Unscored),
    ]);
    Arc::new(Desk::empty(now()).with_arbitration(ArbitrationView {
        observed: true,
        cycle: 7,
        journal_base: 40,
        arbitrations,
        calibrations,
        uncovered: Vec::new(),
    }))
}

fn desk_with_a_federation_review() -> Arc<Desk> {
    desk_with_review(federation_review_arbitrations())
}

fn arbitration_finding(
    report: &qip_investment_agents::OrganisationReport,
) -> &qip_agents::finding::AgentFinding {
    report
        .findings
        .iter()
        .find(|f| f.agent_id == ids::WORLD_MODEL_ARBITRATION)
        .expect("premise: the World Model Arbitration Brain answered the brief")
}

/// R12-WORLD-3025. The World Model Arbitration Brain is constructed in the
/// production organisation (`standard_agents`, under the standard roster),
/// emits a finding for its scope that names every branch, merge, abstention
/// and request for more data the federation decided, keeps every disagreeing
/// view in the count and states each member's calibration — and removing it
/// drops world-model arbitration from coverage, because nobody else reports
/// the federation's decisions.
///
/// Each figure is checked against the fixture rather than for presence, so a
/// decision counted under the wrong arm, a minority view dropped, or an
/// unscored member averaged in as a zero fails here.
///
/// Mutations: drop the `WorldModelArbitrationAnalyst` from `standard_agents`
/// (the premise finding is missing); count `Decision::Abstain` under
/// `branch` (the branch and abstain figures move); count one view per
/// disagreement (eight becomes four).
#[test]
fn removing_the_world_model_arbitration_brain_drops_world_model_arbitration_from_coverage()
-> Result<()> {
    let desk = desk_with_a_federation_review();
    let mut org = organisation(desk.clone())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = arbitration_finding(&report);
    assert_eq!(finding.status, FindingStatus::Complete, "{}", finding.claim);
    assert_eq!(fact_value(finding, "arbitrations_decided"), Some(4.0));
    assert_eq!(fact_value(finding, "arbitrations_branch"), Some(1.0));
    assert_eq!(fact_value(finding, "arbitrations_merge"), Some(1.0));
    assert_eq!(fact_value(finding, "arbitrations_abstain"), Some(1.0));
    assert_eq!(
        fact_value(finding, "arbitrations_request_more_data"),
        Some(1.0)
    );
    assert_eq!(
        fact_value(finding, "arbitrations_not_reproduced"),
        Some(0.0)
    );
    assert_eq!(fact_value(finding, "disagreeing_views_kept"), Some(8.0));
    assert_eq!(fact_value(finding, "widest_disagreement_spread"), Some(0.6));
    assert_eq!(fact_value(finding, "members_calibrated"), Some(2.0));
    assert_eq!(fact_value(finding, "members_unscored"), Some(1.0));
    assert_eq!(
        fact_value(finding, "members_outside_calibration_claim"),
        Some(1.0)
    );
    let mean = fact_value(finding, "mean_member_brier").expect("two members are scored");
    assert!((mean - 0.2).abs() < 1e-12, "{mean}");
    // The finding names the journaled review it read.
    assert!(
        finding
            .claim
            .contains("federation review of cycle 7 (journal position 40)"),
        "{}",
        finding.claim
    );

    let (with, without) = scopes_with_and_without(desk, &brief(), ids::WORLD_MODEL_ARBITRATION)?;
    assert!(with.contains(&Scope::WorldModelArbitration), "{with:?}");
    assert!(
        !without.contains(&Scope::WorldModelArbitration),
        "{without:?}"
    );
    Ok(())
}

/// No federation review is no data, never a federation that agreed.
///
/// Mutation: drop the `!view.observed` early return — an unobserved slot
/// has no arbitrations and is then reported through the empty-pass arm,
/// whose claim names a cycle that never ran, and the claim check fails.
#[test]
fn the_world_model_arbitration_brain_reports_an_unwritten_review_as_no_data() -> Result<()> {
    let mut org = organisation(populated_desk())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = arbitration_finding(&report);
    assert_eq!(finding.status, FindingStatus::NoView, "{}", finding.claim);
    assert!(
        finding
            .claim
            .contains("no world model federation review has been written"),
        "{}",
        finding.claim
    );
    assert!(fact_value(finding, "arbitrations_decided").is_none());
    assert!(
        !report
            .covered_scopes()
            .contains(&Scope::WorldModelArbitration)
    );
    Ok(())
}

/// A review that arbitrated nothing covers nothing: zero decisions is not a
/// scope covered.
///
/// Mutation: drop the `view.arbitrations.is_empty()` early return — the
/// brain reports `arbitrations_decided = 0` as a complete finding and the
/// scope is covered by an empty record.
#[test]
fn a_federation_review_that_arbitrated_nothing_does_not_cover_world_model_arbitration() -> Result<()>
{
    use qip_investment_agents::desk::ArbitrationView;
    let desk = Arc::new(Desk::empty(now()).with_arbitration(ArbitrationView {
        observed: true,
        cycle: 3,
        ..ArbitrationView::default()
    }));
    let mut org = organisation(desk)?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = arbitration_finding(&report);
    assert_eq!(finding.status, FindingStatus::NoView, "{}", finding.claim);
    assert!(
        finding.claim.contains("cycle 3 arbitrated nothing"),
        "{}",
        finding.claim
    );
    assert!(
        !report
            .covered_scopes()
            .contains(&Scope::WorldModelArbitration)
    );
    Ok(())
}

/// The brain decides each proposition from the views the record preserves,
/// and does not repeat a recorded decision those views do not support. A
/// review whose branch arbitration is recorded as a merge — two close
/// composites far apart, which the federation's rule branches — is counted
/// as a branch, flagged as not reproduced, and named in the finding's missing
/// inputs, so a journal edited or written by a different rule is a finding
/// rather than a figure.
///
/// Mutation: count the recorded `arbitration.decision` instead of the one
/// `decide_on` reaches — the branch count drops to zero and merge rises to
/// two; or drop the `decided != arbitration.decision` comparison — nothing
/// is reported not reproduced.
#[test]
fn the_world_model_arbitration_brain_decides_from_the_preserved_views_and_names_a_record_they_do_not_reproduce()
-> Result<()> {
    let mut arbitrations = federation_review_arbitrations();
    assert_eq!(
        arbitrations[0].decision,
        qip_world_model::federation::Decision::Branch,
        "premise: the first fixture arbitration is the branch"
    );
    arbitrations[0].decision = qip_world_model::federation::Decision::Merge;
    let mut org = organisation(desk_with_review(arbitrations))?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let finding = arbitration_finding(&report);
    assert_eq!(fact_value(finding, "arbitrations_branch"), Some(1.0));
    assert_eq!(fact_value(finding, "arbitrations_merge"), Some(1.0));
    assert_eq!(
        fact_value(finding, "arbitrations_not_reproduced"),
        Some(1.0)
    );
    assert_eq!(finding.status, FindingStatus::Partial, "{}", finding.claim);
    assert!(
        finding
            .missing_inputs
            .iter()
            .any(|m| m.contains("AAA:next-close-up@1")),
        "{:?}",
        finding.missing_inputs
    );
    Ok(())
}
