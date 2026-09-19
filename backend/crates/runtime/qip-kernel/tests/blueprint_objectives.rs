//! Blueprint §49.1's objectives, and the one reading of them that must never
//! be produced.
//!
//! `Slo::evaluate(0, 0)` reports `is_met`, deliberately, so that a quiet hour
//! does not page. Every test here exists because that makes an SLO reader one
//! careless line away from announcing fifteen objectives met on a platform
//! that has measured nothing — the `MaxExpectedShortfall` shape, which shipped
//! in this repository once already and read as protection for as long as
//! nobody checked.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_compliance::incident::HaltScope;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance as DataProvenance;
use qip_financial::universe::Universe;
use qip_kernel::blueprint_objectives::{
    FED_OBJECTIVES, NETTING_RATIO, ObjectiveLedger, ObjectiveStanding, RECONCILIATION_BREAKS_ZERO,
    Unmeasured, assess, netting_ratio_of, review, unmeasured,
};
use qip_kernel::central::{BreakOrigin, CellReport, ReconciliationBreak};
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_mesh::delta::DeltaOrder;
use qip_observability::Telemetry;
use qip_observability::slo::blueprint_slos;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

const CELL: &str = "cell-lon-1";
const VENUE: &str = "XNYS";
const INSTRUMENT: &str = "AAA";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    if let Ok(object) = FinancialObject::builder(
        ObjectId::from_string(INSTRUMENT),
        INSTRUMENT,
        InstrumentType::CommonStock,
        LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0),
    )
    .venue(VENUE)
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .provenance(DataProvenance::synthetic("test", start()))
    .build(start())
    {
        let _ = universe.insert(object);
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("objectives-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

/// One order the cell sent, netted from `contributors` gross shares into
/// `quantity` net.
fn order(id: &str, quantity: &str, contributors: &[&str]) -> DeltaOrder {
    DeltaOrder {
        order_id: id.to_string(),
        strategy: StrategyId::new("objectives-strategy"),
        object_id: ObjectId::from_string(INSTRUMENT),
        venue: VenueId::new(VENUE),
        side: BookSide::Bid,
        quantity: qip_core::Decimal::parse(quantity).unwrap_or(qip_core::Decimal::ZERO),
        price: dec!("100"),
        simulated: true,
        contributors: contributors
            .iter()
            .enumerate()
            .map(|(index, size)| qip_contracts::intent::Contributor {
                strategy: StrategyId::new(format!("contributor-{index}")),
                signed_size: qip_core::Decimal::parse(size).unwrap_or(qip_core::Decimal::ZERO),
                inputs: Vec::new(),
            })
            .collect(),
    }
}

fn a_break() -> ReconciliationBreak {
    ReconciliationBreak {
        instrument: INSTRUMENT.to_string(),
        cell_quantity: dec!("10"),
        external_quantity: dec!("4"),
        detail: "six lots the venue has no record of".to_string(),
        origin: BreakOrigin::Book,
    }
}

/// The failure this whole module exists to prevent, asserted directly.
///
/// `Slo::evaluate(0, 0)` returns `is_met: true`. A reader that counted that
/// would report every §49.1 objective achieved on a platform that had never
/// ingested anything, which is exactly how `MaxExpectedShortfall` read as a
/// control while being unable to fire.
#[test]
fn an_objective_nothing_fed_is_reported_unobserved_rather_than_met() {
    // Premise: the objectives exist and `evaluate` really does report met on
    // nothing. Without this the test below would pass on an empty list.
    let objectives = blueprint_slos();
    assert!(
        !objectives.is_empty(),
        "blueprint_slos must declare §49.1's objectives for this to mean anything"
    );
    let on_nothing = objectives[0].evaluate(0, 0);
    assert!(
        on_nothing.is_met,
        "premise: Slo::evaluate(0, 0) reports met, which is the trap this guards"
    );
    assert!(
        !on_nothing.is_observed(),
        "premise: and reports it as unobserved, which is how the trap is avoided"
    );

    let reviewed = assess(&ObjectiveLedger::new(), start());
    assert_eq!(
        reviewed.standings.len(),
        objectives.len(),
        "every objective must get a standing; one missing reads like one that was met"
    );
    assert_eq!(
        reviewed.met(),
        0,
        "nothing was measured, so nothing may be reported met"
    );
    assert_eq!(reviewed.missed(), 0);
    assert_eq!(
        reviewed.unobserved(),
        objectives.len(),
        "all of them are unobserved"
    );
}

/// The count of met and the count of unobserved must appear in the same
/// sentence, so an operator cannot read the first without the second.
#[test]
fn the_review_summary_reports_unobserved_objectives_beside_the_ones_that_were_met() {
    let (summary, problems) = review(&ObjectiveLedger::new(), start());
    let summary = summary.expect("the review always says something; silence reads as unwired");
    assert!(
        summary.contains("0 of 15 objective(s) met"),
        "the summary must state the met count against the total: {summary}"
    );
    assert!(
        summary.contains("15 unobserved"),
        "and the unobserved count beside it: {summary}"
    );
    // Not a problem. Nothing is deployed and six of the figures do not exist;
    // a problem on every cycle of a correctly configured deployment teaches an
    // operator that problems are noise, which `venue_admission` already paid
    // for once.
    assert!(
        problems.is_empty(),
        "an unobserved objective is a sentence, not an alarm: {problems:?}"
    );
}

/// The structural guard on this module rather than on the platform: an
/// objective that is neither fed nor explained must be a problem, so a
/// sixteenth objective cannot quietly join the unobserved pile.
#[test]
fn every_blueprint_objective_is_either_fed_here_or_carries_a_reason_it_cannot_be() {
    let objectives = blueprint_slos();
    assert!(!objectives.is_empty(), "premise: there are objectives");
    for slo in &objectives {
        let fed = FED_OBJECTIVES.contains(&slo.name.as_str());
        let explained = unmeasured(&slo.name).is_some();
        assert!(
            fed || explained,
            "§49.1 objective `{}` is neither fed nor explained; `review` would report it \
             unobserved with no reason",
            slo.name
        );
        assert!(
            !(fed && explained),
            "§49.1 objective `{}` is both fed and excused, which makes the excuse a lie",
            slo.name
        );
    }
    let reviewed = assess(&ObjectiveLedger::new(), start());
    assert!(
        !reviewed
            .standings
            .iter()
            .any(|(_, standing)| matches!(standing, ObjectiveStanding::Unclassified)),
        "no objective may be unclassified"
    );
}

/// The four that need a running system come from
/// `BLUEPRINT_SLOS_NEEDING_A_DEPLOYMENT`, so that list keeps one owner. This
/// asserts the kernel really defers to it rather than keeping a copy.
#[test]
fn the_objectives_that_need_a_deployment_are_classified_from_the_observability_list() {
    let listed = qip_observability::slo::BLUEPRINT_SLOS_NEEDING_A_DEPLOYMENT;
    assert!(
        !listed.is_empty(),
        "premise: the observability crate names the objectives that need a deployment"
    );
    for name in listed {
        assert_eq!(
            unmeasured(name).map(|(reason, _)| reason),
            Some(Unmeasured::NeedsDeployment),
            "`{name}` is on the deployment list and must be classified from it"
        );
    }
}

/// An order carrying no contributor vector is an absence, not a ratio of one.
///
/// `DeltaOrder::contributors` is `#[serde(default)]`, so a delta written
/// before the field decodes as naming nobody. Reading that as "one intent per
/// order, ratio 1.0" would feed the netting objective a number nobody
/// measured — and 1.0 is below the 1.5 floor, so it would also report a miss
/// the platform never had.
#[test]
fn a_report_whose_orders_name_no_contributors_evidences_no_netting_ratio() {
    let with_nobody = order("ord-1", "100", &[]);
    assert!(
        with_nobody.contributors.is_empty(),
        "premise: this order names no contributor"
    );
    assert_eq!(netting_ratio_of(&[with_nobody]), None);
    assert_eq!(netting_ratio_of(&[]), None);

    // And one that does name contributors yields their gross over the net.
    let netted = order("ord-2", "100", &["80", "-20"]);
    assert_eq!(netting_ratio_of(&[netted]), Some(1.0));
    let heavily_netted = order("ord-3", "100", &["300", "-200"]);
    assert_eq!(netting_ratio_of(&[heavily_netted]), Some(5.0));
}

/// A cell report the centre halted must be recorded as a reconciliation
/// *miss*, not as an absence.
#[test]
fn a_cell_report_that_does_not_reconcile_misses_the_reconciliation_objective() -> Result<()> {
    let mut platform = platform()?;
    // Premise: before any report the objective rests on nothing at all.
    let before = platform.blueprint_objectives(start());
    assert!(
        matches!(
            before.standing(RECONCILIATION_BREAKS_ZERO),
            Some(ObjectiveStanding::Quiet(_))
        ),
        "premise: this process feeds the objective and has ingested nothing, so it is quiet — \
         which is a standing of its own and not `Met`"
    );

    let clean = CellReport::new(CELL, start());
    let ingestion = platform.ingest_cell_report(clean, start())?;
    assert_eq!(
        ingestion.halted, None,
        "premise: a report with no break reconciles"
    );
    let after_clean = platform.blueprint_objectives(start());
    assert!(
        matches!(
            after_clean.standing(RECONCILIATION_BREAKS_ZERO),
            Some(ObjectiveStanding::Met(_))
        ),
        "one clean report, and the objective is met on evidence: {:?}",
        after_clean.standing(RECONCILIATION_BREAKS_ZERO)
    );

    let broken = CellReport::new(CELL, start()).with_break(a_break());
    let ingestion = platform.ingest_cell_report(broken, start())?;
    assert_eq!(
        ingestion.halted,
        Some(HaltScope::Cell(CELL.to_string())),
        "premise: a break halts the cell"
    );
    let after_break = platform.blueprint_objectives(start());
    let Some(ObjectiveStanding::Missed(status)) = after_break.standing(RECONCILIATION_BREAKS_ZERO)
    else {
        panic!(
            "§49.1 admits no unexplained reconciliation break; the objective must be missed, \
             not {:?}",
            after_break.standing(RECONCILIATION_BREAKS_ZERO)
        );
    };
    assert_eq!(
        status.observations, 2,
        "both reports are observations; dropping the bad one would make the objective read \
         better the worse things went"
    );
    assert!(
        (status.achieved - 0.5).abs() < 1e-9,
        "one of two reconciled"
    );
    Ok(())
}

/// A netting ratio under §49.1's floor misses the objective; one above it
/// meets it. The floor is read from the objective, so this also proves the
/// kernel is not carrying its own copy of 1.5.
#[test]
fn a_report_netting_less_than_the_floor_misses_the_netting_objective() -> Result<()> {
    let floor = blueprint_slos()
        .into_iter()
        .find(|slo| slo.name == NETTING_RATIO)
        .and_then(|slo| slo.ratio_floor)
        .expect("premise: §49.1's netting objective declares a floor");
    assert!(
        floor > 1.0,
        "premise: the floor is above one, or no order could ever fail it"
    );

    let mut platform = platform()?;
    // Gross 100 + 20 = 120 over a net of 100: 1.2, under the floor.
    let thin = CellReport::new(CELL, start()).with_orders(vec![order(
        "ord-thin",
        "100",
        &["100", "20", "-20"],
    )]);
    platform.ingest_cell_report(thin, start())?;
    let after_thin = platform.blueprint_objectives(start());
    let Some(ObjectiveStanding::Missed(status)) = after_thin.standing(NETTING_RATIO) else {
        panic!(
            "a ratio of 1.2 is under the {floor} floor and must miss, not {:?}",
            after_thin.standing(NETTING_RATIO)
        );
    };
    assert_eq!(status.observations, 1);

    // Gross 300 + 200 = 500 over a net of 100: 5.0, well clear.
    let netted = CellReport::new(CELL, start()).with_orders(vec![order(
        "ord-netted",
        "100",
        &["300", "-200"],
    )]);
    platform.ingest_cell_report(netted, start())?;
    let after_netted = platform.blueprint_objectives(start());
    let Some(ObjectiveStanding::Missed(status)) = after_netted.standing(NETTING_RATIO) else {
        panic!("one of two observations still misses a target of 1.0");
    };
    assert_eq!(status.observations, 2, "both reports carried a ratio");
    assert!(
        (status.achieved - 0.5).abs() < 1e-9,
        "one of the two reached the floor"
    );
    Ok(())
}

/// A report the plane refuses is still evidence about reconciliation, and it
/// is evidence of the bad kind.
///
/// The plane halts the cell *before* the recall step that can error, so
/// counting only the reports that returned `Ok` would drop exactly the ones
/// that halted a cell.
#[test]
fn a_report_the_centre_refuses_counts_against_the_reconciliation_objective() -> Result<()> {
    let mut platform = platform()?;
    // A report that names no cell is refused at the plane's first line.
    // Premise first: it really is refused, or the assertion below would be
    // about a report that was absorbed.
    let nameless = CellReport::new("   ", start());
    let refused = platform.ingest_cell_report(nameless, start());
    let Err(error) = refused else {
        // If the plane ever starts admitting this report, the premise is gone
        // and the test must be rewritten rather than quietly passing.
        panic!("premise: the plane refuses a report it cannot absorb");
    };
    assert!(
        !error.message().is_empty(),
        "a refusal says what to do instead"
    );

    let after = platform.blueprint_objectives(start());
    let Some(ObjectiveStanding::Missed(status)) = after.standing(RECONCILIATION_BREAKS_ZERO) else {
        panic!(
            "a refused report must count against the objective, not vanish from it: {:?}",
            after.standing(RECONCILIATION_BREAKS_ZERO)
        );
    };
    assert_eq!(status.observations, 1);
    assert!(status.achieved.abs() < 1e-9, "nothing reconciled");
    Ok(())
}

/// A fed objective whose window has gone quiet is `Quiet`, not `Met`.
///
/// This is the same trap as the first test at a different seam: the series
/// exists, so `feeds` is true, and the window holds nothing. Reporting that
/// as an achievement is the reading that must not be produced.
#[test]
fn an_objective_whose_window_has_aged_out_is_quiet_rather_than_met() {
    let mut ledger = ObjectiveLedger::new();
    ledger.observe(RECONCILIATION_BREAKS_ZERO, true, start());
    // Premise: at the instant it was taken, the observation counts and the
    // objective is met on it.
    let fresh = assess(&ledger, start());
    assert!(
        matches!(
            fresh.standing(RECONCILIATION_BREAKS_ZERO),
            Some(ObjectiveStanding::Met(_))
        ),
        "premise: the observation is inside its own window"
    );

    // A day is the objective's window; a fortnight later it holds nothing.
    let later = start().saturating_add(qip_core::time::Duration::from_days(14));
    let stale = assess(&ledger, later);
    let Some(standing) = stale.standing(RECONCILIATION_BREAKS_ZERO) else {
        panic!("the objective must still get a standing");
    };
    assert!(
        matches!(standing, ObjectiveStanding::Quiet(_)),
        "a fed objective with an empty window is quiet, not met: {standing:?}"
    );
    assert!(
        !standing.is_observed(),
        "and it rests on no observation, which is what the count must say"
    );
    assert_eq!(stale.met(), 0, "nothing in the window, so nothing met");
}
