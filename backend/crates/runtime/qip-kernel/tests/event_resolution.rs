//! R12-EVENT-5083 under ADR 0151 decision 10: an oracle's report never
//! resolves a market by itself.
//!
//! `EventMarketPlane::discover` used to hand every `PredictionUpdate::Report`
//! straight to `MarketResolution::observe`, so whatever outcome the oracle
//! named became the proposed resolution — and, once the dispute window closed
//! quietly, the final one paper positions settled on. The synthetic venue's
//! oracle picked that outcome at random. Nothing compared the claim with the
//! market's own criteria, so a market whose source published "no change"
//! could settle as a cut on the oracle's word alone.
//!
//! These tests drive the plane with a scripted source so that the
//! observations and the report can be made to agree, disagree, or be absent,
//! and each refusal case is set beside the same report with agreeing
//! observations so that "pending" is shown to be a refusal and not a plane
//! that proposes nothing at all.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::VecDeque;

use qip_contracts::{VenueClass, VenueId};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_kernel::event_markets::{EventMarketPlane, EventMarketsDiscovered};
use qip_prediction::access::AccessBook;
use qip_prediction::{
    EventMarket, FeeSchedule, Observation, Observations, OracleIdentity, OracleKind, OracleReport,
    OutcomeId, PredictionAdapter, PredictionSource, PredictionUpdate, ResolutionState, demo_market,
};

const VENUE: &str = "SCRIPTED-PREDICT";
const JURISDICTION: &str = "GB";
/// The metric and source `demo_market` names. Spelled out rather than read
/// back from the market so a rename there breaks these tests loudly.
const METRIC: &str = "policy_rate_change_bp";
const SOURCE: &str = "central-bank-statistical-release";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn resolves_at() -> Timestamp {
    start().saturating_add(Duration::from_days(30))
}

fn market() -> Result<EventMarket> {
    demo_market(
        VenueId::new(VENUE),
        resolves_at(),
        FeeSchedule::new(50, 0, 100)?,
    )
}

/// A source that publishes exactly what the test scripted, once.
#[derive(Debug)]
struct Scripted {
    updates: VecDeque<Vec<PredictionUpdate>>,
}

impl PredictionAdapter for Scripted {
    fn descriptor(&self) -> PredictionSource {
        PredictionSource {
            name: "scripted-prediction-venue".to_string(),
            venue: VenueId::new(VENUE),
            class: VenueClass::PredictionMarket,
            expected_latency: Duration::from_millis(1),
            is_synthetic: true,
            production_requirement: None,
        }
    }

    fn poll(&mut self, _until: Timestamp) -> Result<Vec<PredictionUpdate>> {
        Ok(self.updates.pop_front().unwrap_or_default())
    }
}

/// Every authority permits listing, creating and seeding the market and each
/// of its outcome instruments, so nothing below is refused for a reason this
/// file is not about.
fn permissions(market: &EventMarket) -> Result<CreationPermissions> {
    let mut register = CreationPermissions::new();
    let products: Vec<String> = std::iter::once(market.market_id.as_str().to_string())
        .chain(
            market
                .outcomes()
                .iter()
                .map(|outcome| outcome.object_id.as_str().to_string()),
        )
        .collect();
    for product in products {
        for action in CreationAction::ALL {
            let request = CreationRequest::new(action, VENUE, product.as_str(), JURISDICTION);
            for authority in CreationAuthority::ALL {
                register.record(authority, &request, "test fixture")?;
            }
        }
    }
    Ok(register)
}

fn plane(updates: Vec<PredictionUpdate>) -> Result<EventMarketPlane> {
    let market = market()?;
    EventMarketPlane::open(
        Box::new(Scripted {
            updates: VecDeque::from([updates]),
        }),
        None,
        AccessBook::new(),
        "desk",
        OracleIdentity::new(
            "oracle",
            OracleKind::Optimistic,
            Duration::from_hours(2),
            0.9,
        )?,
        permissions(&market)?,
        JURISDICTION,
    )
}

fn listed() -> Result<PredictionUpdate> {
    Ok(PredictionUpdate::MarketListed(Box::new(market()?)))
}

fn observed(source: &str, change_bp: i64) -> PredictionUpdate {
    PredictionUpdate::Observed {
        market_id: ObjectId::from_string(qip_prediction::DEMO_MARKET_ID),
        source: source.to_string(),
        observations: Observations::at(resolves_at())
            .with(METRIC, Observation::Numeric(Decimal::from_int(change_bp))),
    }
}

fn report(outcome: &str) -> PredictionUpdate {
    PredictionUpdate::Report {
        market_id: ObjectId::from_string(qip_prediction::DEMO_MARKET_ID),
        report: OracleReport {
            outcome: OutcomeId::new(outcome),
            confidence: 0.97,
            reported_at: resolves_at(),
            evidence: "scripted".to_string(),
        },
    }
}

/// Discover at the resolution instant — not after it, so an unresolved
/// market reads `pending` rather than `overdue` and the assertion names the
/// state the ruling names.
fn discover(updates: Vec<PredictionUpdate>) -> Result<(EventMarketPlane, EventMarketsDiscovered)> {
    let mut plane = plane(updates)?;
    let found = plane.discover(resolves_at())?;
    Ok((plane, found))
}

fn state(plane: &EventMarketPlane) -> Option<ResolutionState> {
    plane
        .resolution(&ObjectId::from_string(qip_prediction::DEMO_MARKET_ID))
        .map(|resolution| resolution.state().clone())
}

/// The premise every refusal below is measured against: the same report of
/// `cut`, with the source publishing a 25bp cut, is proposed.
fn agreeing_report_is_proposed() -> Result<()> {
    let (agreeing, found) = discover(vec![listed()?, observed(SOURCE, -25), report("cut")])?;
    assert!(
        matches!(
            state(&agreeing),
            Some(ResolutionState::Proposed { ref report, .. }) if report.outcome.as_str() == "cut"
        ),
        "premise: a report agreeing with the source's observations must be proposed; {:?} {:?}",
        state(&agreeing),
        found.problems
    );
    assert_eq!(
        found
            .resolutions
            .get(qip_prediction::DEMO_MARKET_ID)
            .map(String::as_str),
        Some("proposed")
    );
    Ok(())
}

/// The ruling's own test. The source published "no change" (0bp), which the
/// market's criteria resolve to `hold`; the oracle reports `cut`. Before this
/// change the plane proposed `cut`, and two quiet hours later settled every
/// paper position on it.
///
/// Mutation: in `EventMarketPlane::corroborate`, return `Ok(())` in place of
/// the `market.evaluate(observations)` match, and the contradicting report is
/// proposed — the `pending` assertion fails.
#[test]
fn an_oracle_report_contradicting_the_markets_own_criteria_is_refused_and_the_market_stays_pending()
-> Result<()> {
    agreeing_report_is_proposed()?;

    let (plane, found) = discover(vec![listed()?, observed(SOURCE, 0), report("cut")])?;
    assert_eq!(
        state(&plane),
        Some(ResolutionState::Pending),
        "a report contradicting the criteria resolved the market: {:?}",
        found.problems
    );
    assert_eq!(
        found
            .resolutions
            .get(qip_prediction::DEMO_MARKET_ID)
            .map(String::as_str),
        Some("pending")
    );
    assert!(
        found.problems.iter().any(|problem| problem.contains(
            "the oracle's report of cut on SYNTH-PREDICT-MARKET was refused and the market was \
             not resolved: it contradicts the market's own criteria, which resolve \
             central-bank-statistical-release's observations to hold"
        )),
        "the refusal must name the report, the criteria and what they resolve to: {:?}",
        found.problems
    );
    Ok(())
}

/// A report with nothing published behind it is the oracle's word alone.
///
/// Mutation: in `EventMarketPlane::corroborate`, return `Ok(())` where no
/// observation is held, and the bare report is proposed — the `pending`
/// assertion fails.
#[test]
fn an_oracle_report_with_no_observation_from_the_markets_source_leaves_the_market_pending()
-> Result<()> {
    agreeing_report_is_proposed()?;

    let (plane, found) = discover(vec![listed()?, report("cut")])?;
    assert_eq!(
        state(&plane),
        Some(ResolutionState::Pending),
        "a bare oracle report resolved the market: {:?}",
        found.problems
    );
    assert!(
        found.problems.iter().any(|problem| problem.contains(
            "central-bank-statistical-release, the market's resolution source, has published no \
             observation of policy_rate_change_bp for it"
        )),
        "the refusal must say no observation was published: {:?}",
        found.problems
    );
    Ok(())
}

/// Observations agreeing with the report but published by someone other
/// than the market's named source are not evidence for it: they are dropped,
/// and the report then stands on the oracle's word alone and is refused.
///
/// Mutation: drop the `source != named` refusal in
/// `EventMarketPlane::take_observations`, and the stranger's -25bp
/// corroborates `cut` — the `pending` assertion fails.
#[test]
fn observations_from_a_source_the_market_does_not_name_cannot_corroborate_a_report() -> Result<()> {
    agreeing_report_is_proposed()?;

    let (plane, found) = discover(vec![
        listed()?,
        observed("a-blog-that-quotes-the-release", -25),
        report("cut"),
    ])?;
    assert_eq!(
        state(&plane),
        Some(ResolutionState::Pending),
        "{:?}",
        found.problems
    );
    assert!(
        plane
            .observations(&ObjectId::from_string(qip_prediction::DEMO_MARKET_ID))
            .is_none(),
        "a stranger's publication was kept as the market's evidence"
    );
    assert!(
        found.problems.iter().any(|problem| problem.contains(
            "observations published by a-blog-that-quotes-the-release cannot resolve it"
        )),
        "{:?}",
        found.problems
    );
    Ok(())
}
