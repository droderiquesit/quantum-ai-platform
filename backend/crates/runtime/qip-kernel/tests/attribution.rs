//! The edge contributor vector joined to central attribution (blueprint
//! §27.1, §43.4).
//!
//! A cell nets N intents into one order and ships the order with the vector
//! of strategies behind it. Until this seam existed the centre read the
//! vector and did nothing with it: no strategy book moved, no fill was
//! attributed to anyone but the largest contributor, and an internal cross —
//! the trade between two of the platform's own strategies that §27.1 calls a
//! regulatory expectation — reached the centre as a record nothing settled.
//! Each test here ingests a report and asserts what the books and the exact
//! attribution say afterwards.
//!
//! The centre bills from a report's `fills` and nothing else. Its `orders`
//! are what the cell sent; for one slice the centre read them as fills and
//! attributed, charged and settled orders still resting at the venue. The
//! tests below send the order *and* the fill where a fill is meant, and the
//! order alone where the property is that nothing is billed.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_compliance::incident::HaltScope;
use qip_contracts::intent::Contributor;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_contracts::wire::{CrossRecord, FillRecord, FillShare};
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::central::CellReport;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_mesh::delta::DeltaOrder;
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_risk::limits::{Limit, LimitKind, LimitSet};

// --- fixtures ---------------------------------------------------------------

const CELL: &str = "cell-lon-1";
const INSTRUMENT: &str = "obj-AAA";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                ObjectId::from_string(INSTRUMENT),
                "AAA",
                InstrumentType::CommonStock,
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid object"),
        )
        .expect("insertable");
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("kernel-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

fn strategy(name: &str) -> StrategyId {
    StrategyId::new(name)
}

fn contributor(name: &str, signed_size: Decimal) -> Contributor {
    Contributor {
        strategy: strategy(name),
        signed_size,
        inputs: vec![(format!("{name}-feature"), 1)],
    }
}

/// One order as the cell ships it, with the contributors it netted.
fn order(
    id: &str,
    side: BookSide,
    quantity: Decimal,
    price: Decimal,
    contributors: Vec<Contributor>,
) -> DeltaOrder {
    let largest = contributors
        .iter()
        .max_by_key(|c| c.signed_size.abs())
        .map_or_else(|| strategy("legacy"), |c| c.strategy.clone());
    DeltaOrder {
        order_id: id.to_string(),
        strategy: largest,
        object_id: ObjectId::from_string(INSTRUMENT),
        venue: VenueId::new("XNYS"),
        side,
        quantity,
        price,
        simulated: true,
        contributors,
    }
}

/// The venue's report on one order, attributed the way the cell attributes
/// it: shares that sum to the fill.
fn fill(
    order_id: &str,
    side: BookSide,
    quantity: Decimal,
    price: Decimal,
    shares: &[(&str, Decimal)],
) -> FillRecord {
    FillRecord {
        order_id: order_id.to_string(),
        object_id: ObjectId::from_string(INSTRUMENT),
        venue: VenueId::new("XNYS"),
        side,
        quantity,
        price,
        simulated: true,
        at: start(),
        shares: shares
            .iter()
            .map(|(name, quantity)| FillShare {
                strategy: strategy(name),
                quantity: *quantity,
            })
            .collect(),
    }
}

fn cross(quantity: Decimal, mid: Decimal, bought: &[&str], sold: &[&str]) -> CrossRecord {
    CrossRecord {
        object_id: ObjectId::from_string(INSTRUMENT),
        venue: VenueId::new("XNYS"),
        quantity,
        price: mid,
        bought: bought.iter().map(|name| strategy(name)).collect(),
        sold: sold.iter().map(|name| strategy(name)).collect(),
    }
}

fn lot(platform: &Platform, name: &str) -> Option<(Decimal, Decimal)> {
    platform
        .central()
        .strategy_lot(CELL, &strategy(name), INSTRUMENT)
        .map(|lot| (lot.quantity, lot.average_price))
}

// --- the join ---------------------------------------------------------------

#[test]
fn a_netted_orders_fill_is_attributed_to_its_contributors_with_zero_residual() -> Result<()> {
    // Three strategies each intended a third of a hundred. A third of a
    // hundred does not exist at nine places, so a split that rounds each
    // share leaves a unit unexplained — and an unexplained unit is exactly
    // where whatever nobody understood is hiding. Premise first: the books
    // are empty and the fixture genuinely does not divide.
    let mut platform = platform()?;
    assert!(lot(&platform, "alpha").is_none());
    let fill = dec!("100");
    assert_ne!(
        (fill / dec!("3")) * dec!("3"),
        fill,
        "the fixture divides exactly"
    );

    let netted = order(
        "ord-1",
        BookSide::Ask,
        fill,
        dec!("50"),
        vec![
            contributor("alpha", dec!("1")),
            contributor("beta", dec!("1")),
            contributor("gamma", dec!("1")),
        ],
    );
    assert_eq!(
        netted.contributors.len(),
        3,
        "the premise is a netted order"
    );
    // The venue filled the whole order and the cell split it the way its
    // `split_fill` does: floor each share, hand the leftover unit to one.
    let shares = [
        ("alpha", dec!("33.333333334")),
        ("beta", dec!("33.333333333")),
        ("gamma", dec!("33.333333333")),
    ];
    assert_eq!(
        shares.iter().map(|(_, share)| *share).sum::<Decimal>(),
        fill,
        "the premise is a split that sums to the fill"
    );
    let filled = self::fill("ord-1", BookSide::Ask, fill, dec!("50"), &shares);
    let report = CellReport::new(CELL, start())
        .with_orders(vec![netted])
        .with_fills(vec![filled]);
    let ingestion = platform.ingest_cell_report(report, start())?;

    let settlement = &ingestion.settlement;
    assert_eq!(settlement.orders_sent, 1);
    assert_eq!(settlement.fills_settled, 1);
    assert_eq!(settlement.fills_attributed, 3, "one share per contributor");
    assert!(
        settlement.refused.is_empty(),
        "nothing should have been refused: {:?}",
        settlement.refused
    );
    let attribution = settlement
        .attribution
        .as_ref()
        .expect("a settled order produces an attribution");
    assert_eq!(attribution.positions.len(), 3);
    assert_eq!(
        attribution.residual(),
        Decimal::ZERO,
        "the decomposition must close"
    );
    assert!(attribution.reconciles());

    // Each book moved by its share, and the shares sum to the fill to the
    // last unit; the leftover unit went to exactly one of them.
    let alpha = lot(&platform, "alpha").expect("alpha holds its share");
    let beta = lot(&platform, "beta").expect("beta holds its share");
    let gamma = lot(&platform, "gamma").expect("gamma holds its share");
    assert_eq!(
        alpha.0 + beta.0 + gamma.0,
        fill,
        "the shares do not sum to the fill"
    );
    assert_eq!(alpha, (dec!("33.333333334"), dec!("50")));
    assert_eq!(beta, (dec!("33.333333333"), dec!("50")));
    assert_eq!(gamma, (dec!("33.333333333"), dec!("50")));

    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(
            names::CENTRAL_FILLS_ATTRIBUTED,
            &labels([("basis", "contributor_vector")])
        ),
        3
    );
    assert_eq!(
        snapshot.counter_total(names::CENTRAL_ATTRIBUTION_FAILURES),
        0,
        "the decomposition closed, so no failure may be counted"
    );
    Ok(())
}

#[test]
fn a_fill_is_booked_to_the_shares_the_cell_attributed_and_not_to_the_orders_contributors()
-> Result<()> {
    // Alpha wanted sixty, beta wanted to sell forty. The cell crossed forty
    // internally and sent a buy of twenty for the rest, and when the venue
    // filled it the cell attributed the fill to alpha alone: beta received
    // its fill in the cross. The centre books the cell's shares as shipped;
    // re-splitting on the order's contributor vector — which still names
    // beta — would fill beta twice.
    let mut platform = platform()?;
    let netted = order(
        "ord-2",
        BookSide::Ask,
        dec!("20"),
        dec!("101"),
        vec![
            contributor("alpha", dec!("60")),
            contributor("beta", dec!("-40")),
        ],
    );
    assert_eq!(netted.contributors.len(), 2, "the premise names both");
    let filled = fill(
        "ord-2",
        BookSide::Ask,
        dec!("20"),
        dec!("101"),
        &[("alpha", dec!("20"))],
    );
    let report = CellReport::new(CELL, start())
        .with_orders(vec![netted])
        .with_fills(vec![filled]);
    let ingestion = platform.ingest_cell_report(report, start())?;
    assert_eq!(ingestion.settlement.fills_attributed, 1);
    assert_eq!(lot(&platform, "alpha"), Some((dec!("20"), dec!("101"))));
    assert!(
        lot(&platform, "beta").is_none(),
        "the seller took a share of a buy fill"
    );
    Ok(())
}

#[test]
fn an_internal_cross_moves_both_contributors_books_at_the_mid_and_the_close_out_is_exact()
-> Result<()> {
    // §27.1: both strategies receive their full intended fill at the mid,
    // and every cross is a ledger entry with both contributors and the
    // price. Premise: nothing is held before the cross.
    let mut platform = platform()?;
    assert!(lot(&platform, "entering").is_none());
    assert!(lot(&platform, "leaving").is_none());

    let report = CellReport::new(CELL, start()).with_crosses(vec![cross(
        dec!("40"),
        dec!("101.5"),
        &["entering"],
        &["leaving"],
    )]);
    let ingestion = platform.ingest_cell_report(report, start())?;
    assert_eq!(ingestion.settlement.crosses_settled, 1);
    assert!(ingestion.settlement.refused.is_empty());
    // The entering strategy is the buyer, the leaving one the seller, both
    // at the mid the cell recorded — a price neither side chose.
    assert_eq!(
        lot(&platform, "entering"),
        Some((dec!("40"), dec!("101.5"))),
        "the buyer's book did not move up at the mid"
    );
    assert_eq!(
        lot(&platform, "leaving"),
        Some((dec!("-40"), dec!("101.5"))),
        "the seller's book did not move down at the mid"
    );
    let attribution = ingestion
        .settlement
        .attribution
        .as_ref()
        .expect("a cross is attributed");
    assert_eq!(attribution.residual(), Decimal::ZERO);
    assert_eq!(
        platform
            .telemetry()
            .metrics
            .snapshot()
            .counter_total(names::CENTRAL_CROSSES_SETTLED),
        1
    );

    // The seller later buys back at a lower price. What it earned is
    // exactly the mid it sold at less what it paid, on the quantity — and
    // the attribution says so to the unit, per strategy.
    let close_out = order(
        "ord-3",
        BookSide::Ask,
        dec!("40"),
        dec!("100"),
        vec![contributor("leaving", dec!("40"))],
    );
    let filled = fill(
        "ord-3",
        BookSide::Ask,
        dec!("40"),
        dec!("100"),
        &[("leaving", dec!("40"))],
    );
    let report = CellReport::new(CELL, start())
        .with_orders(vec![close_out])
        .with_fills(vec![filled]);
    let ingestion = platform.ingest_cell_report(report, start())?;
    assert_eq!(
        lot(&platform, "leaving"),
        Some((Decimal::ZERO, Decimal::ZERO))
    );
    let earned = ingestion.settlement.by_strategy();
    assert_eq!(earned.get("leaving").copied(), Some(dec!("60")));
    assert_eq!(
        ingestion
            .settlement
            .attribution
            .as_ref()
            .expect("attributed")
            .residual(),
        Decimal::ZERO
    );
    Ok(())
}

#[test]
fn a_cross_naming_two_buyers_is_refused_rather_than_split_evenly() -> Result<()> {
    // The wire carries who bought and who sold, not how much each. One
    // buyer and one seller is determinable; two buyers is a guess, and a
    // guess in the one record §27.1 calls a regulatory expectation is
    // refused, counted and reported — not halved.
    let mut platform = platform()?;
    let report = CellReport::new(CELL, start()).with_crosses(vec![cross(
        dec!("40"),
        dec!("101.5"),
        &["alpha", "gamma"],
        &["beta"],
    )]);
    let ingestion = platform.ingest_cell_report(report, start())?;
    assert_eq!(ingestion.settlement.crosses_settled, 0);
    assert_eq!(ingestion.settlement.refused.len(), 1);
    assert!(
        ingestion.settlement.refused[0].contains("2 buyer(s)"),
        "the refusal must say what it could not determine: {}",
        ingestion.settlement.refused[0]
    );
    assert!(
        !ingestion.is_quiet(),
        "a refused settlement is not a quiet report"
    );
    for name in ["alpha", "beta", "gamma"] {
        assert!(
            lot(&platform, name).is_none(),
            "{name}'s book moved on a refused cross"
        );
    }
    assert_eq!(
        platform.telemetry().metrics.snapshot().counter(
            names::CENTRAL_SETTLEMENTS_REFUSED,
            &labels([("kind", "cross")])
        ),
        1
    );
    Ok(())
}

#[test]
fn a_report_from_a_cell_older_than_the_fill_record_is_counted_sent_and_settles_nothing()
-> Result<()> {
    // The event log is sealed and a report written before `fills` existed
    // still replays. It carried orders — what was sent — and no fills, and
    // the honest reading of it is that nothing was confirmed: the centre of
    // its day billed the orders, and replaying that reading would put the
    // same resting orders back on the books. Premise first: the serialised
    // report genuinely lacks the field, and names one sent order.
    let mut platform = platform()?;
    let mut legacy = order("ord-4", BookSide::Bid, dec!("10"), dec!("99"), Vec::new());
    legacy.strategy = strategy("legacy");
    let mut payload =
        serde_json::to_value(CellReport::new(CELL, start()).with_orders(vec![legacy]))?;
    let Some(object) = payload.as_object_mut() else {
        return Err(qip_core::error::Error::invalid(
            "the report is not an object",
        ));
    };
    assert!(
        object.remove("fills").is_some(),
        "the field was not there to remove"
    );
    assert!(
        payload.get("fills").is_none(),
        "the premise is an older wire"
    );
    assert_eq!(payload["orders"].as_array().map(Vec::len), Some(1));

    let report: CellReport = serde_json::from_value(payload)?;
    let ingestion = platform.ingest_cell_report(report, start())?;
    assert_eq!(
        ingestion.settlement.orders_sent, 1,
        "the order was not registered as sent"
    );
    assert_eq!(ingestion.settlement.fills_settled, 0);
    assert_eq!(ingestion.settlement.fills_attributed, 0);
    assert!(
        ingestion.settlement.absorbed.is_empty(),
        "a sent order was charged"
    );
    assert!(ingestion.settlement.attribution.is_none());
    assert!(
        ingestion.settlement.refused.is_empty(),
        "{:?}",
        ingestion.settlement.refused
    );
    assert!(ingestion.halted.is_none(), "an open order is not a break");
    assert!(
        lot(&platform, "legacy").is_none(),
        "a sent order was booked"
    );
    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(snapshot.counter_total(names::CENTRAL_ORDERS_SENT), 1);
    assert_eq!(snapshot.counter_total(names::CENTRAL_FILLS_ATTRIBUTED), 0);
    Ok(())
}

#[test]
fn a_fill_on_an_order_the_centre_never_saw_sent_halts_the_cell_and_books_nothing() -> Result<()> {
    // A venue claim with no order of the platform's behind it. The cell's
    // own reconciler halts on this when its drop copy finds it; the centre
    // must do the same when the uplink carries it, because a position the
    // centre cannot trace to an order is a position nobody authorised.
    // Premise first: the plane is fresh, so it has seen no order sent.
    let mut platform = platform()?;
    assert!(lot(&platform, "alpha").is_none());
    let ghost = fill(
        "ord-ghost",
        BookSide::Ask,
        dec!("10"),
        dec!("100"),
        &[("alpha", dec!("10"))],
    );
    let report = CellReport::new(CELL, start()).with_fills(vec![ghost]);
    assert!(
        report.orders.is_empty(),
        "the premise is a fill with no order"
    );
    assert!(report.reconciles(), "the cell itself reported no break");

    let ingestion = platform.ingest_cell_report(report, start())?;
    assert_eq!(
        ingestion.halted,
        Some(HaltScope::Cell(CELL.to_string())),
        "the cell was not halted for a fill it has no order for"
    );
    assert_eq!(ingestion.settlement.breaks.len(), 1);
    assert_eq!(
        ingestion.settlement.breaks[0].direction(),
        qip_kernel::central::BreakDirection::UnsentFill
    );
    assert!(
        ingestion.settlement.breaks[0]
            .detail
            .contains("never saw that order sent"),
        "the break does not say what is missing: {}",
        ingestion.settlement.breaks[0].detail
    );
    assert_eq!(ingestion.settlement.fills_settled, 0);
    assert!(
        ingestion.settlement.absorbed.is_empty(),
        "an unsent fill was charged"
    );
    assert!(
        lot(&platform, "alpha").is_none(),
        "an unsent fill was booked"
    );
    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(
            names::CENTRAL_RECONCILIATION_BREAKS,
            &labels([("direction", "unsent_fill")])
        ),
        1
    );
    assert_eq!(
        snapshot.counter(
            names::CENTRAL_CELL_HALTS,
            &labels([("cause", "reconciliation")])
        ),
        1
    );
    Ok(())
}

#[test]
fn a_fill_beyond_the_quantity_sent_is_the_same_break() -> Result<()> {
    // Ten were sent and six filled; a further five is one more than the
    // platform asked for, and it halts exactly as a fill on an unknown
    // order does. The order is still open when the excess arrives — four
    // remain — so this is the excess check and not the unknown-order one:
    // a first version filled all ten, which closed the order and sent the
    // second fill down the unknown-order path, and disabling the excess
    // check left that version green. Premise: the six settle cleanly.
    let mut platform = platform()?;
    let sent = order(
        "ord-5",
        BookSide::Ask,
        dec!("10"),
        dec!("100"),
        vec![contributor("alpha", dec!("10"))],
    );
    let part = fill(
        "ord-5",
        BookSide::Ask,
        dec!("6"),
        dec!("100"),
        &[("alpha", dec!("6"))],
    );
    let report = CellReport::new(CELL, start())
        .with_orders(vec![sent])
        .with_fills(vec![part]);
    let ingestion = platform.ingest_cell_report(report, start())?;
    assert!(
        ingestion.halted.is_none(),
        "{:?}",
        ingestion.settlement.breaks
    );
    assert_eq!(lot(&platform, "alpha"), Some((dec!("6"), dec!("100"))));

    let excess = fill(
        "ord-5",
        BookSide::Ask,
        dec!("5"),
        dec!("100"),
        &[("alpha", dec!("5"))],
    );
    assert!(
        excess.quantity > dec!("10") - dec!("6"),
        "the premise is a fill beyond what remains"
    );
    let report = CellReport::new(CELL, start()).with_fills(vec![excess]);
    let ingestion = platform.ingest_cell_report(report, start())?;
    assert_eq!(ingestion.halted, Some(HaltScope::Cell(CELL.to_string())));
    assert_eq!(
        ingestion.settlement.breaks[0].direction(),
        qip_kernel::central::BreakDirection::UnsentFill
    );
    assert!(
        ingestion.settlement.breaks[0]
            .detail
            .contains("the excess was never sent"),
        "{}",
        ingestion.settlement.breaks[0].detail
    );
    assert_eq!(
        lot(&platform, "alpha"),
        Some((dec!("6"), dec!("100"))),
        "the excess was booked"
    );
    Ok(())
}

#[test]
fn a_fill_whose_shares_do_not_sum_to_it_is_refused_rather_than_booked_short() -> Result<()> {
    // The cell's shares are the attribution. Booking nineteen of a twenty
    // fill would leave one unit nobody holds and a position the aggregate
    // charges that no strategy book explains. Refused, counted, and the
    // cell is not halted — its order was sent and its fill is real; what
    // is wrong is the arithmetic on the record.
    let mut platform = platform()?;
    let sent = order(
        "ord-6",
        BookSide::Ask,
        dec!("20"),
        dec!("100"),
        vec![contributor("alpha", dec!("20"))],
    );
    let short = fill(
        "ord-6",
        BookSide::Ask,
        dec!("20"),
        dec!("100"),
        &[("alpha", dec!("19"))],
    );
    assert_ne!(
        short.shares[0].quantity, short.quantity,
        "the premise is a short split"
    );
    let report = CellReport::new(CELL, start())
        .with_orders(vec![sent])
        .with_fills(vec![short]);
    let ingestion = platform.ingest_cell_report(report, start())?;
    assert!(ingestion.halted.is_none());
    assert_eq!(ingestion.settlement.fills_settled, 0);
    assert_eq!(
        ingestion.settlement.refused.len(),
        1,
        "{:?}",
        ingestion.settlement.refused
    );
    assert!(ingestion.settlement.refused[0].contains("sum to the fill exactly"));
    assert!(ingestion.settlement.absorbed.is_empty());
    assert!(
        lot(&platform, "alpha").is_none(),
        "a short split was booked"
    );
    assert_eq!(
        platform.telemetry().metrics.snapshot().counter(
            names::CENTRAL_SETTLEMENTS_REFUSED,
            &labels([("kind", "fill")])
        ),
        1
    );
    Ok(())
}
