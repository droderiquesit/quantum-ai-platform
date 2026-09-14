//! Blueprint §25.6's cross-margin read, across the seam no crate can see.
//!
//! Three claims, and each of them is about a relationship between two crates
//! rather than about either one.
//!
//! 1. **The read is derived from a book that actually traded.** A real
//!    [`Platform`], driven through its own order path to a real fill, carries
//!    counterparty exposure, and [`qip_kernel::cross_margin::review`] turns
//!    that into a finding naming the venue. `qip-capital`'s own tests cannot
//!    do this — they have no platform — and `qip-kernel`'s unit tests build
//!    the aggregate by hand, which proves the arithmetic and says nothing
//!    about whether a book the loop produced would reach it.
//! 2. **A statement is what closes the finding.** The same book with a
//!    statement handed in through [`Platform::observe_statement`] — the one
//!    production door for the fact — reports the venue covered, and a thinner
//!    statement reports it called. A finding that could not be closed would
//!    be an alarm nobody could act on.
//! 3. **Nothing here reaches a venue.** The cross-margin read is a read. It
//!    creates no order, and the platform it runs against is the paper one.
//!
//! On the paper-trading boundary: nothing in this suite or in the code it
//! covers touches any of the three layers
//! `.claude/rules/01-security-and-safety.md` names. The review takes a
//! holdings map, a risk aggregate and a rate table and returns a struct; it
//! holds no broker, no venue and no order, which claim 3 asserts rather than
//! states.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::margin::MarginModel;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Currency, Decimal, ManualClock, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::cross_margin::{CrossMarginFinding, review};
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::AggregateFigures;
use qip_risk::limits::{COUNTERPARTY_AXIS, Limit, LimitKind, LimitSet};
use std::sync::Arc;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

/// Stated rather than defaulted, for the reason `acceptance.rs` gives:
/// `LiquidityProfile` has no `Default`, because the one it had asserted a
/// ten-basis-point quote for any instrument at all and two vetoes read
/// exactly that figure.
fn liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock, config.seed);
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(object(), "ACME", InstrumentType::CommonStock, liquidity())
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("cross-margin-acceptance", start()))
            .build(start())?,
    )?;
    let limits = LimitSet::new("cross-margin-acceptance").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at twice equity"),
    );
    Platform::new(config, context, Telemetry::silent(), universe, limits)
}

/// A platform that has traded, and the counterparty it traded through.
///
/// The counterparty is read back off the aggregate rather than assumed,
/// because the whole point of the join is that the review reads the same
/// bucket the fill wrote. A test that hardcoded the broker's name would pass
/// if the two ever stopped agreeing.
fn traded() -> Result<(Platform, VenueId)> {
    let mut platform = platform()?;
    let order = platform.order_from(
        object(),
        Side::Buy,
        dec!("4000"),
        dec!("100"),
        "prop-cross-margin",
        vec!["hyp-cross-margin".to_string()],
        start(),
    );
    platform.submit_order(order, start())?;

    let fills = platform.orders().fills();
    assert!(
        !fills.is_empty(),
        "the premise failed: the order did not fill"
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

#[test]
fn a_platform_that_has_actually_traded_reports_the_counterparty_it_has_never_seen_a_statement_for()
-> Result<()> {
    // Claim 1. The exposure is not a fixture: it is the running gross the
    // platform's own fill wrote onto the counterparty axis, the same bucket
    // `MaxCounterpartyExposure` reads. Two independent claims about where
    // this book traded would eventually disagree, and the louder one would
    // be wrong; there is one bucket and both read it.
    //
    // The finding must be "unobserved", never "under-collateralised". An
    // account nobody has looked at and an account observed and found empty
    // produce the same coverage arithmetic and call for opposite actions.
    let (platform, counterparty) = traded()?;
    assert!(
        platform.holdings_observed().is_empty(),
        "the premise failed: a statement existed before one was handed in"
    );

    let found = review(
        platform.holdings_observed(),
        platform.risk_figures(),
        &MarginModel::default(),
        Currency::USD,
    )?;

    assert!(found.is_finding(), "a book that traded produced no finding");
    assert!(
        found.calls.is_empty(),
        "an unobserved counterparty was reported as a margin call: {:?}",
        found.calls
    );
    assert_eq!(
        found
            .unobserved
            .iter()
            .map(|gap| gap.venue.clone())
            .collect::<Vec<_>>(),
        vec![counterparty.clone()],
        "the review did not name the counterparty the fill was charged to"
    );
    assert!(
        found.unobserved_gross().is_positive(),
        "the unobserved gross was nothing on a book that traded"
    );
    assert!(
        found.unobserved_maintenance().is_positive(),
        "no maintenance was required against an unobserved counterparty"
    );

    // And the record a replay would read names the venue, not only the
    // total: "600,000 unobserved" tells an operator to go looking, and the
    // venue tells them where.
    let record = CrossMarginFinding::of(&found, 1, start()).expect("a finding produces a record");
    assert_eq!(record.unobserved_venues, vec![counterparty.to_string()]);
    Ok(())
}

#[test]
fn a_statement_handed_in_closes_the_finding_and_a_thinner_one_turns_it_into_a_call() -> Result<()> {
    // Claim 2, both halves. Without the closing half the unobserved finding
    // would be an alarm that fires for ever on every venue and that nothing
    // an operator can do would silence — the shape of a control that reads
    // as protection and is noise. Without the calling half it would be a
    // formality that any statement at all satisfies.
    let (mut platform, counterparty) = traded()?;
    let gross = platform
        .risk_figures()
        .axis_exposures()
        .get(COUNTERPARTY_AXIS)
        .and_then(|buckets| buckets.get(counterparty.as_str()))
        .copied()
        .expect("the counterparty's gross");
    assert!(gross.is_positive(), "the premise failed: no gross");

    // Cash at the counterparty, comfortably above the quarter of gross the
    // shipped model maintains. Cash takes no haircut, so this is cover
    // whatever the securities haircut is.
    platform.observe_statement(
        counterparty.clone(),
        Currency::USD.as_str(),
        gross,
        dec!("1"),
        start(),
    )?;
    let covered = review(
        platform.holdings_observed(),
        platform.risk_figures(),
        &MarginModel::default(),
        Currency::USD,
    )?;
    assert!(
        covered.unobserved.is_empty(),
        "a counterparty with a statement was still reported unobserved"
    );
    assert!(
        covered.calls.is_empty(),
        "a well-covered counterparty was called: {}",
        covered.describe()
    );
    assert!(
        !covered.is_finding(),
        "handing in a statement did not close the finding: {}",
        covered.describe()
    );
    assert_eq!(covered.coverage.len(), 1);

    // The same venue, restated at a hundredth of the balance. A statement
    // replaces the venue-asset it names, so this is the same account read
    // again rather than a second one.
    let thin = gross
        .checked_div(dec!("100"))
        .expect("a hundredth of a positive gross");
    platform.observe_statement(
        counterparty.clone(),
        Currency::USD.as_str(),
        thin,
        dec!("1"),
        start(),
    )?;
    let called = review(
        platform.holdings_observed(),
        platform.risk_figures(),
        &MarginModel::default(),
        Currency::USD,
    )?;
    assert!(
        called.unobserved.is_empty(),
        "a counterparty with a thin statement was reported unobserved rather than called"
    );
    assert_eq!(
        called.calls,
        vec![counterparty],
        "a counterparty covering a hundredth of its maintenance was not called: {}",
        called.describe()
    );
    assert!(called.is_finding());
    Ok(())
}

#[test]
fn the_cross_margin_read_reaches_no_broker_no_venue_and_no_order() -> Result<()> {
    // Claim 3, and it is the claim this suite exists to hold rather than
    // `qip-kernel`'s own tests: a module that grew a path to a venue would
    // still be internally consistent, and the property lost would be a
    // property of the relationship. Two independent assertions, because a
    // source scan and a behavioural check fail in different ways.
    //
    // First, behaviourally: a platform that has traded, been reviewed, and
    // been reviewed again after a statement, has the fills it had before and
    // none of them live.
    let (mut platform, counterparty) = traded()?;
    let before = platform.orders().fills().len();
    assert!(
        before > 0,
        "the premise failed: no fills to compare against"
    );

    review(
        platform.holdings_observed(),
        platform.risk_figures(),
        &MarginModel::default(),
        Currency::USD,
    )?;
    platform.observe_statement(
        counterparty,
        Currency::USD.as_str(),
        dec!("1"),
        dec!("1"),
        start(),
    )?;
    review(
        platform.holdings_observed(),
        platform.risk_figures(),
        &MarginModel::default(),
        Currency::USD,
    )?;

    assert_eq!(
        platform.orders().fills().len(),
        before,
        "the cross-margin read produced a fill"
    );
    assert!(!platform.orders().has_live_fills());
    assert!(!platform.is_live_capable());

    // Second, structurally: the module's production source names no broker,
    // no venue adapter and no order type. The premise — that the source was
    // actually read and is not empty — is asserted first, because a helper
    // returning nothing would make every scan below pass forever.
    let source = qip_acceptance::read("backend/crates/runtime/qip-kernel/src/cross_margin.rs");
    let production: Vec<&str> = source
        .lines()
        .take_while(|line| line.trim() != "#[cfg(test)]")
        // The module documentation discusses what the read must not do and so
        // contains the very words being searched for. Prose is not code.
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    assert!(
        production.len() > 100,
        "the production source did not read: {} line(s)",
        production.len()
    );
    let body = production.join("\n");
    for forbidden in ["Broker", "submit_order", "Placer", "OrderManager"] {
        assert!(
            !body.contains(forbidden),
            "the cross-margin module names {forbidden}"
        );
    }
    Ok(())
}
