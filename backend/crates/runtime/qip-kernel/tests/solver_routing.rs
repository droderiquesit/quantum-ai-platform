//! The evidence that ADR 0006's classical baseline was actually computed.
//!
//! `ComputeRouter::solve` has always solved a classical baseline on every
//! construction — that half was never in doubt. What was missing is that the
//! record of the comparison, `ConstructionOutcome::routing`, was read by one
//! test in `qip-portfolio-engine` and by nothing else in the workspace: the
//! kernel took `outcome.proposal` and dropped the decision on the floor. A
//! control whose result nobody keeps is indistinguishable from one that did
//! not run, and ADR 0006 is precisely a rule about being able to show the
//! comparison afterwards. These tests drive a real cycle and assert the
//! comparison reaches the durable journal, so deleting the recording site
//! fails a test rather than leaving an audit that quietly says nothing.
//!
//! # On the fixture, honestly
//!
//! [`REVIEW_FLOOR`] lowers `ReviewPolicy::minimum_surviving_confidence` from
//! the shipped 0.50, for the reason `valuation_seam.rs` already records: the
//! adversarial panel's confidence on a synthetic tape tops out near 0.41 for
//! this fixture, so at the shipped floor no thesis is ever approved,
//! `construct_from` is never called, no solver ever runs, and there is no
//! comparison to journal. The floor is a deployment configuration
//! (`PlatformConfig::review`) and not a safety control; everything asserted
//! here happens strictly downstream of the red team's verdict, and no risk
//! limit, autonomy ceiling or paper-trading layer is touched.
//!
//! **Read that before quoting these tests as evidence about a deployment.**
//! Raise the floor to 0.50 and both tests fail on their own premise — "the
//! cycle sized a proposal and journaled no solver comparison ... no thesis
//! cleared the action bar" — so what they prove is that the comparison is
//! recorded and cleared correctly *when a proposal is sized*, not that a
//! shipped configuration sizes one.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::{CycleJournalEntry, Platform};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

// --- fixtures ---------------------------------------------------------------

/// A liquid listed name on five million units a day, quoted at three basis
/// points. Stated rather than defaulted: `LiquidityProfile` has no `Default`,
/// because `MinLiquidity` and `MaxDaysToLiquidate` read exactly these two
/// figures and a fixture may not inherit a premise nobody wrote down.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
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

/// The red team's confidence floor these tests run under. Stated as a
/// constant so the module doc above can name it, and so a reader who raises it
/// sees both tests fail on their premise rather than on an assertion.
const REVIEW_FLOOR: f64 = 0.10;

fn platform() -> Result<Platform> {
    let config = PlatformConfig {
        review: ReviewPolicy {
            minimum_surviving_confidence: REVIEW_FLOOR,
            ..ReviewPolicy::default()
        },
        ..PlatformConfig::default()
    };
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

/// A price series with a jump partway through, so the detectors have something
/// real to find — the same shape the kernel's founding test feeds.
fn bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// The journal entry for one cycle, replayed from the durable log rather than
/// read off the platform in memory. The log is the record; a field asserted
/// only in memory would pass on a platform that computed the comparison and
/// never sealed it.
fn entry_for(platform: &Platform, cycle: u64) -> Result<CycleJournalEntry> {
    let entries = platform.journal_entries()?;
    assert!(
        !entries.is_empty(),
        "no cycle was journaled at all, so nothing below is about the solver routing"
    );
    entries
        .into_iter()
        .find(|entry| entry.cycle == cycle)
        .ok_or_else(|| {
            qip_core::error::Error::not_found(format!("no journal entry for cycle {cycle}"))
        })
}

// --- the comparison reaches the log -----------------------------------------

#[test]
fn a_cycle_that_sizes_a_proposal_journals_the_solver_and_the_classical_baseline_it_was_measured_against()
-> Result<()> {
    let mut platform = platform()?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());

    // Premise, first: this cycle actually constructed something. A cycle that
    // sized nothing would journal no routing for an honest reason, and every
    // assertion below would then be about the wrong absence.
    let entry = entry_for(&platform, 1)?;
    let routing = entry.solver_routing.clone().unwrap_or_else(|| {
        panic!(
            "the cycle sized a proposal and journaled no solver comparison:\n{}",
            report.summarise()
        )
    });

    // What ADR 0006 actually requires kept: a baseline, a chosen answer, and
    // the distance between them. The baseline must be a number — not merely
    // present — because a NaN or an infinity would satisfy `is_some()` and
    // tell an operator nothing about whether the quantum arm was worth taking.
    assert!(
        routing.classical_objective.is_finite(),
        "the classical baseline journaled as {}, which is not a measurement",
        routing.classical_objective
    );
    assert!(
        routing.objective.is_finite(),
        "the chosen objective journaled as {}, which is not a measurement",
        routing.objective
    );
    assert!(
        routing.improvement_over_classical.is_finite(),
        "the improvement over the baseline journaled as {}",
        routing.improvement_over_classical
    );

    // The chosen solver is one of the solvers that ran. These are two
    // separately recorded facts and a journal in which they disagreed would be
    // a record of a choice made among answers nobody computed.
    assert!(
        !routing.ran.is_empty(),
        "the journal names a chosen solver and no solver that ran"
    );
    assert!(
        routing.ran.contains(&routing.chosen),
        "the journal says {} was chosen and it is not among the solvers that ran ({:?})",
        routing.chosen,
        routing.ran
    );

    // A deployment with no quantum provider configured — which is every
    // deployment, `quantum_enabled` being off by default — must claim no
    // advantage at all. `None` rather than `0.0`: a zero would read as an
    // advantage measured and found to be nothing, which is a different fact
    // from a quantum path that never ran, and it is the fabrication this slot
    // of the record exists to refuse.
    assert!(
        !routing.chosen.contains("quantum"),
        "a platform with no quantum provider chose {}",
        routing.chosen
    );
    assert_eq!(
        routing.measured_quantum_advantage, None,
        "no quantum answer was used, so there is no advantage to report"
    );
    assert!(
        routing
            .describe()
            .contains("no quantum answer was used, so no advantage is claimed"),
        "{}",
        routing.describe()
    );
    // And the operator line carries the baseline itself rather than only the
    // verdict, because "a baseline was computed" is the claim ADR 0006 refuses
    // to accept without the number.
    assert!(
        routing
            .describe()
            .contains(&format!("{:.6}", routing.classical_objective)),
        "{}",
        routing.describe()
    );
    Ok(())
}

#[test]
fn a_cycle_that_reaches_no_construction_journals_no_solver_comparison_at_all() -> Result<()> {
    // The failure this closes: an entry written unconditionally — a zeroed
    // comparison on a cycle where no solver ran — would tell an operator that
    // ADR 0006's baseline was computed and scored nothing, which is a
    // different and false claim from "this cycle never got as far as solving".
    // A fabricated zero is worse than an absence, because a chart of it is
    // indistinguishable from a real result.
    //
    // Which cycle is which, measured rather than assumed. A cycle whose DECIDE
    // stage reports "no thesis cleared the action bar" has still constructed —
    // the solver ran on a degenerate problem and the action bar rejected the
    // result — and it journals a real comparison. The cycle that reaches no
    // construction at all is one on a platform that has observed nothing, so
    // REASON approves no thesis and `construct_from` refuses before any solver
    // runs.
    let mut platform = platform()?;
    let bare = platform.run_cycle(start());
    let first = entry_for(&platform, 1)?;
    assert!(
        first.solver_routing.is_none(),
        "a cycle that observed nothing journaled a solver comparison: {:?}\n{}",
        first.solver_routing,
        bare.summarise()
    );

    // Premise, and it is the half that stops this test passing forever against
    // a field nothing ever sets: the same platform, once it has something to
    // reason about, does journal one on the very next cycle.
    platform.observe(bars("AAA", 120));
    let fed = platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let second = entry_for(&platform, 2)?;
    assert!(
        second.solver_routing.is_some(),
        "the fed cycle journaled no comparison, so the absence above proves nothing:\n{}",
        fed.summarise()
    );
    Ok(())
}
