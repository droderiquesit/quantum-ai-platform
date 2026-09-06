//! The analysts read what the absorb arms write.
//!
//! Every fixture here feeds the world model through the real arms —
//! `WorldModel::absorb_macro` and `WorldModel::absorb_alternative_data` —
//! and never writes a feature by name. That is the difference from
//! `organisation.rs`, whose desk is populated by hand: a hand-populated desk
//! proves the analyst can read a series, and cannot prove the platform ever
//! writes one. For as long as only the first kind of test existed, the
//! macro analyst read `policy_rate@global` in production and found nothing.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_agents::finding::{AgentBrief, AgentFinding, Direction, FindingStatus};
use qip_agents::memory::ResearchMemory;
use qip_ai::language::DeterministicModel;
use qip_ai::retrieval::SearchIndex;
use qip_core::error::Result;
use qip_core::ids::{ObjectId, PortfolioId};
use qip_core::lineage::{CorrelationId, Lineage};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Currency, Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::intelligence::{AlternativeDataPoint, MacroObservation};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_investment_agents::desk::{BookView, ComplianceView, Desk, MarketView, RiskView};
use qip_investment_agents::vocabulary::structurally_blind;
use qip_investment_agents::{Organisation, ids};
use qip_market::snapshot::MarketSnapshot;
use qip_portfolio::portfolio::Portfolio;
use qip_risk::limits::{LimitSet, RiskState};
use qip_world_model::WorldModel;
use qip_world_model::vocabulary::{AltMetric, MacroSeries};
use std::collections::BTreeMap;
use std::sync::Arc;

const ECONOMY: &str = "US";

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn lineage() -> Lineage {
    Lineage::root(CorrelationId::from_string("cor-1"), "test")
}

fn brief() -> AgentBrief {
    AgentBrief::new(
        "is ACME mispriced given what the platform has absorbed",
        now(),
        Duration::from_days(30),
    )
    .about_objects(vec![object()])
    .about_entities(vec![object().as_str().to_string()])
}

fn equity(geography: &str) -> FinancialObject {
    FinancialObject::builder(object(), "ACME", InstrumentType::CommonStock)
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .geography(geography)
        .price(Decimal::from_int(100))
        .provenance(Provenance::synthetic("test", now()))
        .build(now())
        .expect("valid object")
}

/// Thirty-six monthly releases of one series, the last one `last_z` sigma
/// away from the rest, through the real macro arm.
fn absorb_releases(world: &mut WorldModel, series: MacroSeries, region: &str, last_z: f64) {
    for month in 0..36 {
        // A small deterministic wobble so the series varies, and a final
        // release displaced by the requested number of its own sigmas.
        let wobble = ((f64::from(month) * 0.754_877_666_2) % 1.0 - 0.5) * 0.2;
        let value = if month == 35 {
            2.0 + last_z * 0.057_7 // 0.0577 is the wobble's standard deviation
        } else {
            2.0 + wobble
        };
        let reference = now().saturating_sub(Duration::from_days(30 * (36 - i64::from(month))));
        let published = reference.saturating_add(Duration::from_days(15));
        world.absorb_macro(&MacroObservation {
            series_id: series.series_id(region),
            region: region.to_string(),
            value,
            unit: "percent".to_string(),
            reference_date: reference,
            consensus: None,
            previous: None,
            is_revision: false,
            provenance: Provenance::new("statistics-office", reference, published),
            quality: DataQuality::clean(),
        });
    }
}

/// Forty daily readings of one metric for the subject, the last one displaced
/// upward, through the real alternative-data arm.
fn absorb_readings(world: &mut WorldModel, metric: AltMetric, dataset: &str) -> Result<()> {
    for day in 0..40 {
        let wobble = ((f64::from(day) * 0.754_877_666_2) % 1.0 - 0.5) * 4.0;
        let value = if day == 39 { 130.0 } else { 100.0 + wobble };
        let observed = now().saturating_sub(Duration::from_days(40 - i64::from(day)));
        world.absorb_alternative_data(&AlternativeDataPoint {
            dataset: dataset.to_string(),
            subject_id: object().as_str().to_string(),
            metric: metric.feature().to_string(),
            value,
            unit: "index".to_string(),
            observed_at: observed,
            lead_days: 14.0,
            proxy_correlation: 0.6,
            proxies_for: None,
            provenance: Provenance::new("alt-vendor", observed, observed),
            quality: DataQuality::clean(),
        })?;
    }
    Ok(())
}

fn desk(universe: Universe, world: WorldModel) -> Arc<Desk> {
    Arc::new(Desk::new(
        MarketView {
            snapshot: MarketSnapshot::new(now()),
            universe,
        },
        world,
        BookView {
            portfolio: Portfolio::new(
                PortfolioId::from_string("pf-1"),
                "test",
                Currency::USD,
                dec!("10000000"),
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
    ))
}

fn organisation(desk: Arc<Desk>, licensed: Vec<String>) -> Result<Organisation> {
    Organisation::standard(
        desk,
        now(),
        now(),
        42,
        Some(Arc::new(DeterministicModel::new())),
        licensed,
        false,
    )
}

fn finding_of<'a>(report: &'a [AgentFinding], agent: &str) -> &'a AgentFinding {
    report
        .iter()
        .find(|f| f.agent_id == agent)
        .unwrap_or_else(|| panic!("premise: {agent} produced an informative finding"))
}

#[test]
fn the_macro_analyst_reads_the_series_the_macro_arm_writes_keyed_by_the_subjects_economy()
-> Result<()> {
    let mut world = WorldModel::new();
    // Hawkish across the board: rate and inflation up, growth down, spreads
    // wider. Every sign in the analyst's table points the same way, so the
    // composite is unambiguous and a direction is the only honest answer.
    absorb_releases(&mut world, MacroSeries::PolicyRate, ECONOMY, 3.0);
    absorb_releases(&mut world, MacroSeries::InflationYoy, ECONOMY, 3.0);
    absorb_releases(&mut world, MacroSeries::GrowthYoy, ECONOMY, -3.0);
    absorb_releases(&mut world, MacroSeries::CreditSpreadBps, ECONOMY, 3.0);
    // Premise: the arm wrote the analyst's names at the economy key, and
    // nothing at the key the analyst used to read.
    for series in MacroSeries::ALL {
        assert_eq!(
            world
                .features()
                .history(series.feature(), ECONOMY, now())
                .len(),
            36,
            "{series:?} was not written at {ECONOMY}"
        );
        assert!(
            world
                .features()
                .history(series.feature(), "global", now())
                .is_empty(),
            "{series:?} was written at a key nothing reads"
        );
    }

    let mut universe = Universe::new();
    universe.insert(equity(ECONOMY))?;
    let mut org = organisation(desk(universe, world), Vec::new())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    assert!(report.failed.is_empty(), "runs failed: {:?}", report.failed);

    let finding = finding_of(&report.findings, ids::MACRO);
    assert_eq!(finding.status, FindingStatus::Complete, "{}", finding.claim);
    assert_eq!(
        finding.direction,
        Direction::Negative,
        "a hawkish print reads negative for a risk asset: {}",
        finding.claim
    );
    assert!(
        finding.conviction > 0.5,
        "four three-sigma prints carry conviction {}",
        finding.conviction
    );
    assert!(
        finding
            .evidence
            .iter()
            .all(|e| e.ends_with(&format!("@{ECONOMY}"))),
        "the evidence does not name the economy read: {:?}",
        finding.evidence
    );
    Ok(())
}

#[test]
fn a_subject_the_universe_does_not_place_in_an_economy_gets_no_macro_view_rather_than_anothers()
-> Result<()> {
    let mut world = WorldModel::new();
    for series in MacroSeries::ALL {
        absorb_releases(&mut world, series, ECONOMY, 3.0);
    }
    // The subject is not in the universe at all, so it has no geography. The
    // series exist and would read strongly; the analyst must not reach for
    // them under a default.
    let mut org = organisation(desk(Universe::new(), world), Vec::new())?;
    let report = org.dispatch(&brief(), now(), &lineage());
    let run = report
        .runs
        .iter()
        .find(|r| r.agent_id == ids::MACRO)
        .expect("premise: the macro analyst ran");
    let finding = run
        .finding
        .as_ref()
        .expect("premise: it produced a finding");
    assert_eq!(finding.status, FindingStatus::NoView, "{}", finding.claim);
    assert!(
        finding.claim.contains("no economy"),
        "the refusal does not say why: {}",
        finding.claim
    );
    Ok(())
}

#[test]
fn the_alternative_data_analyst_reads_the_metric_the_arm_writes_when_the_dataset_is_licensed()
-> Result<()> {
    let mut world = WorldModel::new();
    let metric = AltMetric::WebTrafficIndex;
    absorb_readings(&mut world, metric, metric.dataset())?;
    // Premise: written under the analyst's name with the dataset as the
    // series' producer, and not under the path the kernel once wrote.
    assert_eq!(
        world
            .features()
            .history(metric.feature(), object().as_str(), now())
            .len(),
        40
    );
    assert_eq!(
        world
            .features()
            .definition(metric.feature())
            .map(|d| d.producer.as_str()),
        Some(metric.dataset())
    );

    let mut universe = Universe::new();
    universe.insert(equity(ECONOMY))?;
    let desk = desk(universe, world);

    // Unlicensed: the same series is not read, and the refusal names the
    // dataset. Licensing is refused by default and this is that default.
    let mut unlicensed = organisation(desk.clone(), Vec::new())?;
    let report = unlicensed.dispatch(&brief(), now(), &lineage());
    let run = report
        .runs
        .iter()
        .find(|r| r.agent_id == ids::ALT_DATA)
        .expect("premise: the analyst ran");
    let finding = run.finding.as_ref().expect("premise: a finding");
    assert_eq!(finding.status, FindingStatus::NoView, "{}", finding.claim);
    assert!(
        finding.claim.contains("not licensed") && finding.claim.contains(metric.dataset()),
        "{}",
        finding.claim
    );

    // Licensed for exactly that dataset: a direction, from the jump.
    let mut licensed = organisation(desk, vec![metric.dataset().to_string()])?;
    let report = licensed.dispatch(&brief(), now(), &lineage());
    let finding = finding_of(&report.findings, ids::ALT_DATA);
    assert_eq!(finding.direction, Direction::Positive, "{}", finding.claim);
    assert!(
        finding.evidence.contains(&format!(
            "feature:{}@{}",
            metric.feature(),
            object().as_str()
        )),
        "{:?}",
        finding.evidence
    );
    Ok(())
}

#[test]
fn the_analysts_no_arm_can_feed_say_which_record_would_change_their_answer() -> Result<()> {
    // The premise: on a desk the platform's own arms populated, these
    // analysts have nothing. The property: each says what it would take,
    // in the finding a panel reads, rather than only that it has nothing.
    let mut universe = Universe::new();
    universe.insert(equity(ECONOMY))?;
    let mut org = organisation(desk(universe, WorldModel::new()), Vec::new())?;
    let report = org.dispatch(&brief(), now(), &lineage());

    let blind = structurally_blind();
    assert!(
        blind.iter().any(|(agent, _)| *agent == ids::CREDIT)
            && blind.iter().any(|(agent, _)| *agent == ids::DERIVATIVES)
            && blind.iter().any(|(agent, _)| *agent == ids::CAUSAL),
        "the declaration does not name the three analysts blind on an equity: {blind:?}"
    );
    let names_its_need = |finding: &AgentFinding, needs: &str| {
        finding.claim.contains(needs) || finding.missing_inputs.iter().any(|m| m.contains(needs))
    };
    for (agent, needs) in &blind {
        let run = report
            .runs
            .iter()
            .find(|r| r.agent_id == *agent)
            .unwrap_or_else(|| panic!("premise: {agent} ran"));
        let finding = run
            .finding
            .as_ref()
            .unwrap_or_else(|| panic!("premise: {agent} produced a finding"));
        if finding.status == FindingStatus::Deferred {
            // Out of remit before any read: the only analyst on the list
            // that declines an equity is the commodities analyst, whose
            // blindness is proven on its own asset class below.
            assert_eq!(
                *agent,
                ids::COMMODITIES,
                "{agent} deferred: {}",
                finding.claim
            );
            continue;
        }
        assert!(
            names_its_need(finding, needs),
            "{agent} reported {:?} without naming the record kind it needs ({needs})",
            finding.claim
        );
    }

    // The commodities analyst, on a commodity, with the same empty store.
    let mut universe = Universe::new();
    let wti = ObjectId::from_string("obj-WTI");
    universe.insert(
        FinancialObject::builder(wti.clone(), "WTI", InstrumentType::CommoditySpot)
            .venue("XNYM")
            .sector(Sector::Energy)
            .price(Decimal::from_int(80))
            .provenance(Provenance::synthetic("test", now()))
            .build(now())?,
    )?;
    let mut org = organisation(desk(universe, WorldModel::new()), Vec::new())?;
    let brief = AgentBrief::new("is WTI backwardated", now(), Duration::from_days(30))
        .about_objects(vec![wti]);
    let report = org.dispatch(&brief, now(), &lineage());
    let finding = report
        .runs
        .iter()
        .find(|r| r.agent_id == ids::COMMODITIES)
        .and_then(|r| r.finding.as_ref())
        .expect("premise: the commodities analyst produced a finding on a commodity");
    assert_eq!(finding.status, FindingStatus::NoView, "{}", finding.claim);
    let (_, needs) = blind
        .iter()
        .find(|(agent, _)| *agent == ids::COMMODITIES)
        .expect("premise: the commodities analyst is declared blind");
    assert!(
        names_its_need(finding, needs),
        "the commodities analyst reported {:?} without naming the record kind it needs",
        finding.claim
    );
    Ok(())
}
