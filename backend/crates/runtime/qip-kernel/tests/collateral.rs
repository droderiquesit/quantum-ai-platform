//! Blueprint §25.6's cross-margin read, at the seam that now calls it.
//!
//! `qip_kernel::cross_margin::review` and the 1,640-line collateral graph
//! under it in `qip-capital` were complete, refusing and covered by an
//! acceptance suite — and until this seam existed **nothing outside a test
//! called any of it**. A collateral model with no production caller is the
//! shape `MaxExpectedShortfall` shipped in: it reads as a control and cannot
//! fire. `qip-acceptance/tests/cross_margin.rs` proves the arithmetic against
//! a platform that traded; these tests prove the *cycle* runs it, which is a
//! different claim and the one that was missing.
//!
//! Four properties, and the third and fourth are the ones that make the first
//! two worth anything:
//!
//! 1. A book that traded and holds no statement has the venue named on the
//!    LEARN stage and in the hash-chained log — and **not** raised as a stage
//!    problem, because handing in nothing is what every deployment that exists
//!    today has done, and a fault raised every cycle on the ordinary state is
//!    an alarm an operator learns to page past. A margin call on cover
//!    somebody actually read is the fault, and the third test holds that.
//! 2. A statement that covers the book turns that line into a covered reading
//!    and journals nothing, so the finding is one an operator can close.
//! 3. A book that has traded nowhere says **nothing at all** about collateral.
//!    `CrossMarginReview::describe` will happily announce "0 observed against
//!    0 required" on a platform that has never traded, and a reassuring zero
//!    about a question nobody asked is precisely what this stage must not
//!    print.
//! 4. Nothing on this path reaches a venue. The paper-trading boundary is
//!    untouched by anything here: the read takes a holdings map, a risk
//!    aggregate and a rate table and returns a struct. It holds no broker and
//!    no order, and the one order these tests place goes through
//!    `Platform::submit_order` to the simulated broker like every other.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_events::Topic;
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cross_margin::CrossMarginFinding;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::aggregate::AggregateFigures;
use qip_risk::limits::{COUNTERPARTY_AXIS, LimitSet};
use qip_streaming::envelope::StreamEnvelope;

const TRADED: &str = "obj-ACME";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// A liquid listed name, stated rather than inherited.
///
/// `LiquidityProfile` has no `Default` on purpose: the one it had asserted a
/// tight quote and a one-session exit for any instrument at all, and two
/// vetoes read exactly those figures.
fn liquidity() -> LiquidityProfile {
    LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default().with_initial_equity(Decimal::from_int(10_000_000));
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string(TRADED),
            "ACME",
            InstrumentType::CommonStock,
            liquidity(),
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("collateral-seam", start()))
        .build(start())?,
    )?;
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
}

/// A platform that has traded, and the counterparty its fill was charged to.
///
/// The counterparty is read back off the aggregate rather than written down
/// here, because the whole point of the join is that the review reads the same
/// bucket the fill wrote. A fixture that hardcoded the broker's name would
/// keep passing if those two ever stopped agreeing, which is the failure the
/// join exists to make impossible.
fn traded() -> Result<(Platform, VenueId)> {
    let mut platform = platform()?;
    let order = platform.order_from(
        ObjectId::from_string(TRADED),
        Side::Buy,
        dec!("2000"),
        dec!("100"),
        "prop-collateral",
        vec!["hyp-collateral".to_string()],
        start(),
    );
    platform.submit_order(order, start())?;

    let fills = platform.orders().fills();
    assert!(
        !fills.is_empty(),
        "the premise failed: the fixture order did not fill, so no counterparty carries gross"
    );
    for fill in &fills {
        assert!(fill.simulated, "a paper platform produced a live fill");
    }

    let buckets = platform
        .risk_figures()
        .axis_exposures()
        .get(COUNTERPARTY_AXIS)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        buckets.len(),
        1,
        "the premise failed: the fill did not charge exactly one counterparty: {buckets:?}"
    );
    let counterparty = buckets
        .keys()
        .next()
        .cloned()
        .expect("one bucket, just asserted");
    Ok((platform, VenueId::new(counterparty)))
}

/// Every cross-margin finding the platform's own log holds.
fn findings(platform: &Platform) -> Vec<CrossMarginFinding> {
    let mut found = Vec::new();
    for record in platform.event_log().by_topic(Topic::RiskEvaluated) {
        if let Ok(envelope) = StreamEnvelope::from_frame(record)
            && let Ok(decoded) = envelope.decode::<CrossMarginFinding>()
        {
            found.push(decoded.body);
        }
    }
    found
}

/// The LEARN stage's sentence for a cycle.
fn learn_detail(platform: &mut Platform, at: Timestamp) -> String {
    let report = platform.run_cycle(at);
    report
        .stage(Stage::Learn)
        .expect("the learn stage ran in the cycle")
        .detail
        .clone()
}

#[test]
fn a_book_that_traded_with_no_statement_has_the_unread_venue_named_by_learn_and_by_the_log()
-> Result<()> {
    let (mut platform, counterparty) = traded()?;
    // The premise, asserted before the property: no statement exists, so the
    // finding below is about an unobserved account rather than an observed
    // and empty one. Those two call for opposite actions and produce the same
    // coverage arithmetic, which is why the review reports them separately
    // and why this assertion is here rather than implied.
    assert!(
        platform.holdings_observed().is_empty(),
        "the premise failed: a statement existed before one was handed in"
    );

    let report = platform.run_cycle(start());
    let learn = report
        .stage(Stage::Learn)
        .expect("the learn stage ran in the cycle");
    // The venue, delimited by the parenthesis the review closes its list
    // with, so this cannot pass on a venue whose name merely contains this
    // one. A substring match on a venue list is how a test in this repository
    // once survived a mutation that deleted the value it protected.
    assert!(
        learn
            .detail
            .contains(&format!("nobody has looked ({counterparty})")),
        "LEARN did not name the venue whose collateral nobody has read: {}",
        learn.detail
    );
    assert!(
        !learn.detail.contains("below maintenance on observed cover"),
        "an unobserved venue was reported as a margin call: {}",
        learn.detail
    );

    // And it is a sentence and **not** a stage problem, which is the half of
    // this property that had to be learned the hard way. Handing in nothing is
    // what every deployment that exists today has done; a fault raised every
    // cycle on the ordinary state is an alarm an operator learns to page past,
    // and `reconcile_wallet`'s caller already argues exactly this about the
    // wallet's own silence. The first draft of this seam raised a problem here
    // and broke `the_learn_stage_retires_a_strategy_whose_cells_realised_
    // sustained_decay_and_journals_its_disposition`, whose book trades and
    // hands in nothing and whose LEARN problem list had been empty since the
    // stage existed. That test was right and this seam was wrong.
    let problems: Vec<String> = report
        .problems()
        .into_iter()
        .filter(|(stage, _)| *stage == Stage::Learn)
        .map(|(_, problem)| problem.to_string())
        .collect();
    assert!(
        !problems.iter().any(
            |problem| problem.contains("nobody has handed in a statement for")
                || problem.contains("collateral has to be posted there")
        ),
        "an unread account was raised as a fault on a book that is entitled to \
         have handed in nothing: {problems:?}"
    );

    // And the record a replay would read. The total tells an operator to go
    // looking; the venue tells them where, and only one of those is an
    // instruction.
    let found = findings(&platform);
    assert_eq!(
        found.len(),
        1,
        "exactly one cross-margin finding is journalled for one cycle"
    );
    assert_eq!(found[0].unobserved_venues, vec![counterparty.to_string()]);
    assert!(
        found[0].called_venues.is_empty(),
        "an unobserved venue reached the log as a margin call: {:?}",
        found[0].called_venues
    );
    assert!(
        found[0].unobserved_gross.is_positive(),
        "the journalled finding carried no gross on a book that traded"
    );
    assert!(
        found[0].unobserved_maintenance.is_positive(),
        "the journalled finding required no maintenance against an unobserved venue"
    );
    Ok(())
}

#[test]
fn a_statement_covering_the_book_closes_the_finding_and_journals_nothing() -> Result<()> {
    // Without this property the unobserved finding would be an alarm that
    // fires for ever at every venue and that nothing an operator can do would
    // silence. A control nobody can close is noise wearing a control's name.
    let (mut platform, counterparty) = traded()?;
    let gross = platform
        .risk_figures()
        .axis_exposures()
        .get(COUNTERPARTY_AXIS)
        .and_then(|axis| axis.get(counterparty.as_str()))
        .copied()
        .unwrap_or(Decimal::ZERO);
    assert!(
        gross.is_positive(),
        "the premise failed: the counterparty carries no gross"
    );

    // Settlement cash at the venue that carries the exposure, comfortably
    // over the quarter of gross the default maintenance rate asks for. Cash
    // rather than a security because cash is the asset the review takes no
    // haircut on, and the point here is the closing arm rather than the
    // haircut.
    platform.observe_statement(counterparty.clone(), "USD", gross, dec!("1"), start())?;
    assert_eq!(
        platform.holdings_observed().len(),
        1,
        "the premise failed: the statement was not taken"
    );

    let detail = learn_detail(&mut platform, start());
    assert!(
        detail.contains("every venue carrying exposure has a statement"),
        "LEARN did not report the book covered after a statement closed the gap: {detail}"
    );
    assert!(
        !detail.contains("nobody has looked"),
        "LEARN still reported unread collateral after a statement was handed in: {detail}"
    );
    assert!(
        findings(&platform).is_empty(),
        "a covered book journalled a finding, so the record says nothing distinguishes it \
         from a book with a gap"
    );
    Ok(())
}

#[test]
fn a_thin_statement_is_a_margin_call_naming_its_own_venue_rather_than_an_unread_one() -> Result<()>
{
    // The other half of the closing arm. Without it, any statement at all
    // would satisfy the finding and the read would be a formality: hand in a
    // dollar, and a venue holding a dollar against a million of maintenance
    // would report as covered.
    let (mut platform, counterparty) = traded()?;
    platform.observe_statement(counterparty.clone(), "USD", dec!("1"), dec!("1"), start())?;

    let report = platform.run_cycle(start());
    let learn = report
        .stage(Stage::Learn)
        .expect("the learn stage ran in the cycle");
    assert!(
        learn.detail.contains(&format!(
            "below maintenance on observed cover ({counterparty})"
        )),
        "LEARN did not report the thinly covered venue as a call: {}",
        learn.detail
    );
    assert!(
        !learn.detail.contains("nobody has looked"),
        "an observed venue was also reported as unread: {}",
        learn.detail
    );

    let problems: Vec<String> = report
        .problems()
        .into_iter()
        .filter(|(stage, _)| *stage == Stage::Learn)
        .map(|(_, problem)| problem.to_string())
        .collect();
    assert!(
        problems.iter().any(|problem| problem
            .contains("below the maintenance its own exposure requires")
            && problem.contains(counterparty.as_str())),
        "no stage problem named the venue collateral has to be posted at: {problems:?}"
    );

    let found = findings(&platform);
    assert_eq!(found.len(), 1, "one finding for one cycle");
    assert_eq!(found[0].called_venues, vec![counterparty.to_string()]);
    assert!(
        found[0].unobserved_venues.is_empty(),
        "an observed venue reached the log as unobserved: {:?}",
        found[0].unobserved_venues
    );
    Ok(())
}

#[test]
fn a_book_that_has_traded_nowhere_says_nothing_about_collateral_rather_than_a_reassuring_zero()
-> Result<()> {
    // The arm that makes the other three mean something. `describe` states
    // "every venue carrying exposure has a statement, and each covers its own
    // maintenance: 0 observed against 0 required" for a book with no exposure
    // and no statement — a sentence that is true, reassuring, and about
    // nothing. Printing it every cycle on a platform that has never traded
    // would make the covered reading above worthless, because it would be the
    // reading a platform with no collateral model at all also produced.
    let mut platform = platform()?;
    assert!(
        platform
            .risk_figures()
            .axis_exposures()
            .get(COUNTERPARTY_AXIS)
            .is_none_or(|axis| axis.is_empty()),
        "the premise failed: the book carried counterparty exposure before it traded"
    );
    assert!(
        platform.holdings_observed().is_empty(),
        "the premise failed: a statement existed on an untraded book"
    );

    let detail = learn_detail(&mut platform, start());
    assert!(
        !detail.contains("observed against"),
        "LEARN reported a coverage figure for a book that requires nothing anywhere: {detail}"
    );
    assert!(
        !detail.contains("nobody has looked"),
        "LEARN reported unread collateral on a book that has traded nowhere: {detail}"
    );
    assert!(
        findings(&platform).is_empty(),
        "a book with nothing to say journalled a cross-margin finding"
    );
    Ok(())
}
