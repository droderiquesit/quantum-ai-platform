//! Tests for the arbitrage pipeline.
//!
//! The assertions that matter here are the negative ones. Finding an
//! opportunity in a market that has one is arithmetic; the expensive failures
//! are finding one in a market that does not, and finding one on mid prices
//! that the book will not honour. Both have a test whose only job is to fail
//! loudly if the pipeline ever gets optimistic.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_arbitrage::graph::{ArbitrageGraph, Node, PathKind, SyntheticComponent, VenueFacts};
use qip_arbitrage::liquidity::{LiquiditySource, StaticLiquidity};
use qip_arbitrage::netedge::{EdgeAssumptions, NetEdgeCalculator};
use qip_arbitrage::plan::{LegPlanner, PlanSettings};
use qip_arbitrage::pricing::price_path;
use qip_arbitrage::scan::{OpportunityScanner, RejectionStage, SizePolicy};
use qip_arbitrage::search::{SearchSettings, confirm_exact, search_candidates};
use qip_contracts::edge::{Deduction, DeductionKind, NetEdge};
use qip_contracts::message::BookSide;
use qip_contracts::venue::{VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::time::Duration;
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook};

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("test fixture decimal")
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

fn venue(name: &str) -> VenueId {
    VenueId::new(name)
}

fn node(object_name: &str, venue_name: &str) -> Node {
    Node::new(object(object_name), venue(venue_name))
}

fn book(
    market: &str,
    venue_name: &str,
    bids: &[(&str, &str)],
    asks: &[(&str, &str)],
    stamped: Timestamp,
) -> OrderBook {
    OrderBook::from_levels(
        object(market),
        venue_name,
        stamped,
        bids.iter()
            .map(|(p, s)| BookLevel::new(d(p), d(s)))
            .collect(),
        asks.iter()
            .map(|(p, s)| BookLevel::new(d(p), d(s)))
            .collect(),
    )
}

fn scanner(budget: &str) -> OpportunityScanner {
    OpportunityScanner::new(
        SearchSettings::default(),
        EdgeAssumptions::default(),
        PlanSettings::with_budget(d(budget)),
    )
}

/// One crypto exchange whose ETH/BTC cross is a percent away from the two dollar
/// legs that imply it. A real triangular dislocation, with real spreads.
fn triangular(
    class: VenueClass,
    asks: &[(&str, &str)],
    bids: &[(&str, &str)],
) -> Result<(ArbitrageGraph, StaticLiquidity)> {
    let cx = venue("CX");
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(cx.clone(), VenueFacts::new(class, VenueStatus::Open));

    graph.add_trade(
        node("USDT", "CX"),
        node("ETH", "CX"),
        d("0.000333328"),
        d("0.0004"),
        object("ETHUSDT"),
        BookSide::Ask,
        at(),
        20,
    )?;
    graph.add_trade(
        node("ETH", "CX"),
        node("BTC", "CX"),
        d("0.050505"),
        d("0.0004"),
        object("ETHBTC"),
        BookSide::Bid,
        at(),
        20,
    )?;
    graph.add_trade(
        node("BTC", "CX"),
        node("USDT", "CX"),
        d("60000.5"),
        d("0.0004"),
        object("BTCUSDT"),
        BookSide::Bid,
        at(),
        20,
    )?;

    let depth = StaticLiquidity::new()
        .with_book(
            cx.clone(),
            book("ETHUSDT", "CX", &[("3000.0", "200")], asks, at()),
            20,
        )
        .with_book(
            cx.clone(),
            book("ETHBTC", "CX", bids, &[("0.05051", "200")], at()),
            20,
        )
        .with_book(
            cx,
            book(
                "BTCUSDT",
                "CX",
                &[("60000", "10")],
                &[("60001", "10")],
                at(),
            ),
            20,
        );
    Ok((graph, depth))
}

fn liquid_triangular() -> Result<(ArbitrageGraph, StaticLiquidity)> {
    triangular(
        VenueClass::CryptoExchange,
        &[("3000.1", "200")],
        &[("0.0505", "200")],
    )
}

/// Three instruments whose cross rates agree exactly. There is nothing here.
fn consistent_market() -> Result<(ArbitrageGraph, StaticLiquidity)> {
    let cx = venue("CX");
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        cx.clone(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );

    // ETH at 2500, BTC at 50000, so ETH/BTC is 0.05 and every rate is exact at
    // nine decimal places. A fixture whose consistency depended on rounding
    // would be testing the rounding.
    let rates: [(&str, &str, &str, &str, BookSide); 6] = [
        ("USDT", "ETH", "0.0004", "ETHUSDT", BookSide::Ask),
        ("ETH", "USDT", "2500", "ETHUSDT", BookSide::Bid),
        ("USDT", "BTC", "0.00002", "BTCUSDT", BookSide::Ask),
        ("BTC", "USDT", "50000", "BTCUSDT", BookSide::Bid),
        ("ETH", "BTC", "0.05", "ETHBTC", BookSide::Bid),
        ("BTC", "ETH", "20", "ETHBTC", BookSide::Ask),
    ];
    for (from, to, rate, market, side) in rates {
        graph.add_trade(
            node(from, "CX"),
            node(to, "CX"),
            d(rate),
            Decimal::ZERO,
            object(market),
            side,
            at(),
            32,
        )?;
    }

    let depth = StaticLiquidity::new()
        .with_book(
            cx.clone(),
            book(
                "ETHUSDT",
                "CX",
                &[("2500", "1000")],
                &[("2500", "1000")],
                at(),
            ),
            32,
        )
        .with_book(
            cx.clone(),
            book(
                "BTCUSDT",
                "CX",
                &[("50000", "100")],
                &[("50000", "100")],
                at(),
            ),
            32,
        )
        .with_book(
            cx,
            book(
                "ETHBTC",
                "CX",
                &[("0.05", "1000")],
                &[("0.05", "1000")],
                at(),
            ),
            32,
        );
    Ok((graph, depth))
}

/// Two venues twenty basis points apart on the mid and a spread wider than the
/// gap. Free money until somebody has to cross.
fn mid_only_dislocation() -> Result<(ArbitrageGraph, StaticLiquidity)> {
    let mut graph = ArbitrageGraph::new();
    for name in ["XNAS", "XLON"] {
        graph.register_venue(
            venue(name),
            VenueFacts::new(VenueClass::Exchange, VenueStatus::Open),
        );
    }
    graph.add_trade(
        node("USD", "XNAS"),
        node("TKN", "XNAS"),
        d("0.01"),
        Decimal::ZERO,
        object("TKN.XNAS"),
        BookSide::Ask,
        at(),
        16,
    )?;
    graph.add_transfer(
        object("TKN"),
        venue("XNAS"),
        venue("XLON"),
        Decimal::ZERO,
        at(),
        16,
    )?;
    graph.add_trade(
        node("TKN", "XLON"),
        node("USD", "XLON"),
        d("100.20"),
        Decimal::ZERO,
        object("TKN.XLON"),
        BookSide::Bid,
        at(),
        16,
    )?;
    graph.add_transfer(
        object("USD"),
        venue("XLON"),
        venue("XNAS"),
        Decimal::ZERO,
        at(),
        16,
    )?;

    let depth = StaticLiquidity::new()
        .with_book(
            venue("XNAS"),
            book(
                "TKN.XNAS",
                "XNAS",
                &[("99.50", "1000")],
                &[("100.50", "1000")],
                at(),
            ),
            16,
        )
        .with_book(
            venue("XLON"),
            book(
                "TKN.XLON",
                "XLON",
                &[("99.70", "1000")],
                &[("100.70", "1000")],
                at(),
            ),
            16,
        );
    Ok((graph, depth))
}

/// A listed venue against a chain, one percent apart, wide enough to survive
/// the spread. The chain leg is the one that can revert.
fn cross_venue_with_a_chain() -> Result<(ArbitrageGraph, StaticLiquidity)> {
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        venue("XNAS"),
        VenueFacts::new(VenueClass::Exchange, VenueStatus::Open),
    );
    graph.register_venue(
        venue("DEX1"),
        VenueFacts::new(VenueClass::DecentralisedExchange, VenueStatus::Open),
    );

    graph.add_trade(
        node("USD", "XNAS"),
        node("TKN", "XNAS"),
        d("0.009995002"),
        d("0.0005"),
        object("TKN.XNAS"),
        BookSide::Ask,
        at(),
        20,
    )?;
    graph.add_transfer(
        object("TKN"),
        venue("XNAS"),
        venue("DEX1"),
        d("0.0002"),
        at(),
        20,
    )?;
    graph.add_trade(
        node("TKN", "DEX1"),
        node("USD", "DEX1"),
        d("101.05"),
        d("0.0030"),
        object("TKN.DEX1"),
        BookSide::Bid,
        at(),
        20,
    )?;
    graph.add_transfer(
        object("USD"),
        venue("DEX1"),
        venue("XNAS"),
        d("0.0001"),
        at(),
        20,
    )?;

    let depth = StaticLiquidity::new()
        .with_book(
            venue("XNAS"),
            book(
                "TKN.XNAS",
                "XNAS",
                &[("100.00", "5000")],
                &[("100.10", "5000")],
                at(),
            ),
            20,
        )
        .with_book(
            venue("DEX1"),
            book(
                "TKN.DEX1",
                "DEX1",
                &[("101.00", "5000")],
                &[("101.10", "5000")],
                at(),
            ),
            20,
        );
    Ok((graph, depth))
}

/// A two-name basket trading rich to the sum of its parts.
fn synthetic_basket() -> Result<(ArbitrageGraph, StaticLiquidity)> {
    let xnas = venue("XNAS");
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        xnas.clone(),
        VenueFacts::new(VenueClass::Exchange, VenueStatus::Open),
    );

    let components = vec![
        SyntheticComponent {
            object: object("ALPHA"),
            venue: xnas.clone(),
            units_per_unit: d("2"),
            unwind_side: BookSide::Bid,
        },
        SyntheticComponent {
            object: object("BETA"),
            venue: xnas.clone(),
            units_per_unit: d("1"),
            unwind_side: BookSide::Bid,
        },
    ];
    graph.add_synthetic(
        node("USD", "XNAS"),
        node("BASKET", "XNAS"),
        d("0.025"),
        d("0.0005"),
        object("BASKET"),
        components,
        at(),
        12,
    )?;
    graph.add_trade(
        node("BASKET", "XNAS"),
        node("USD", "XNAS"),
        d("40.70"),
        d("0.0005"),
        object("BASKET"),
        BookSide::Bid,
        at(),
        12,
    )?;

    let depth = StaticLiquidity::new()
        .with_book(
            xnas.clone(),
            book(
                "ALPHA",
                "XNAS",
                &[("9.95", "10000")],
                &[("10.05", "10000")],
                at(),
            ),
            12,
        )
        .with_book(
            xnas.clone(),
            book(
                "BETA",
                "XNAS",
                &[("19.90", "10000")],
                &[("20.10", "10000")],
                at(),
            ),
            12,
        )
        .with_book(
            xnas,
            book(
                "BASKET",
                "XNAS",
                &[("40.60", "1000")],
                &[("40.80", "1000")],
                at(),
            ),
            12,
        );
    Ok((graph, depth))
}

#[test]
fn triangular_detection_finds_a_genuine_cycle_and_the_book_confirms_it() -> Result<()> {
    let (graph, depth) = liquid_triangular()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    assert_eq!(candidates.len(), 1, "one dislocation, one cycle");
    assert_eq!(candidates[0].kind, PathKind::Triangular);
    assert!(candidates[0].log_gain_f64 > 0.0);

    let confirmation = confirm_exact(&graph, &candidates[0])?;
    assert!(
        confirmation.is_profitable(),
        "exact arithmetic agrees with the search: {}",
        confirmation.multiple
    );

    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    assert!(pricing.is_fully_available());
    assert!(
        pricing.is_profitable_on_book(),
        "the dislocation is wider than the spreads: {} back on 10000",
        pricing.end_quantity
    );
    Ok(())
}

#[test]
fn a_market_with_consistent_cross_rates_offers_nothing() -> Result<()> {
    let (graph, depth) = consistent_market()?;
    assert!(
        search_candidates(&graph, &SearchSettings::default()).is_empty(),
        "a consistent market has no negative cycle to find"
    );

    let report = scanner("1000000").scan(&graph, &depth, &SizePolicy::uniform(d("10000")), at());
    assert!(
        report.opportunities.is_empty(),
        "found {} opportunities in a market that has none",
        report.opportunities.len()
    );
    Ok(())
}

#[test]
fn exact_arithmetic_rejects_a_candidate_that_the_log_space_search_proposed() -> Result<()> {
    // These two rates multiply to one part in two billion above one, which
    // rounds away in exact arithmetic and does not in a logarithm. Precisely
    // the disagreement the two-stage design exists to resolve.
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        venue("CX"),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    graph.add_trade(
        node("A", "CX"),
        node("B", "CX"),
        d("1.000022362"),
        Decimal::ZERO,
        object("AB"),
        BookSide::Bid,
        at(),
        8,
    )?;
    graph.add_trade(
        node("B", "CX"),
        node("A", "CX"),
        d("0.999977639"),
        Decimal::ZERO,
        object("AB"),
        BookSide::Ask,
        at(),
        8,
    )?;

    let candidates = search_candidates(&graph, &SearchSettings::default());
    assert_eq!(candidates.len(), 1, "the log-space search proposes it");
    assert!(candidates[0].log_gain_f64 > 0.0);

    let confirmation = confirm_exact(&graph, &candidates[0])?;
    assert_eq!(confirmation.multiple, Decimal::ONE);
    assert!(
        !confirmation.is_profitable(),
        "exact arithmetic disposes of it"
    );
    Ok(())
}

#[test]
fn a_path_that_pays_on_mid_prices_is_refused_once_the_book_is_walked() -> Result<()> {
    let (graph, depth) = mid_only_dislocation()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].kind, PathKind::CrossVenue);
    assert!(
        confirm_exact(&graph, &candidates[0])?.is_profitable(),
        "the quoted mids do pay"
    );

    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    assert!(
        pricing.is_fully_available(),
        "depth is not the problem here"
    );
    assert!(
        pricing.indicative_gross_edge() > Decimal::ZERO,
        "profitable on mids"
    );
    assert!(
        !pricing.is_profitable_on_book(),
        "and unprofitable on the book: {} back on 10000",
        pricing.end_quantity
    );

    let report = scanner("1000000").scan(&graph, &depth, &SizePolicy::uniform(d("10000")), at());
    assert!(report.opportunities.is_empty());
    assert_eq!(
        report.rejected_at(RejectionStage::Book).len(),
        1,
        "and it is refused for that reason and no other"
    );
    Ok(())
}

#[test]
fn sweeping_more_than_the_book_holds_never_reports_a_full_fill() -> Result<()> {
    let (_, depth) = liquid_triangular()?;
    let asked = d("5000");
    let (price, available) = depth
        .sweep_cost(&venue("CX"), &object("ETHUSDT"), BookSide::Ask, asked)
        .expect("the book exists");
    assert!(available < asked, "the book holds 200, not 5000");
    assert_eq!(available, d("200"));
    assert_eq!(price, d("3000.1"), "and prices only what it holds");
    Ok(())
}

#[test]
fn a_path_larger_than_the_book_is_refused_rather_than_extrapolated() -> Result<()> {
    let (graph, depth) = liquid_triangular()?;
    let report =
        scanner("100000000").scan(&graph, &depth, &SizePolicy::uniform(d("10000000")), at());
    assert!(report.opportunities.is_empty());
    assert_eq!(report.rejected_at(RejectionStage::Depth).len(), 1);
    Ok(())
}

#[test]
fn a_net_edge_that_skipped_a_deduction_is_refused() -> Result<()> {
    let mut edge = NetEdge::gross(d("100"), d("10000"))?;
    for kind in DeductionKind::all() {
        if kind == DeductionKind::Uncertainty {
            continue;
        }
        edge = edge.deduct(Deduction::new(kind, d("1"), "test")?);
    }
    let refusal = edge
        .require_complete()
        .expect_err("eight of nine is not complete");
    assert!(refusal.message().contains("uncertainty"));
    Ok(())
}

#[test]
fn the_calculator_considers_every_deduction_kind() -> Result<()> {
    let (graph, depth) = liquid_triangular()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    let net_edge = NetEdgeCalculator::new(EdgeAssumptions::default()).calculate(&pricing, at())?;

    assert!(net_edge.unconsidered().is_empty());
    for kind in DeductionKind::all() {
        assert!(
            net_edge.deductions().iter().any(|dd| dd.kind == kind),
            "{} was not considered",
            kind.as_str()
        );
    }
    assert!(net_edge.gross_edge() > Decimal::ZERO);
    assert!(net_edge.total_deducted() > Decimal::ZERO);
    assert!(
        net_edge.is_positive(),
        "a percent of dislocation survives the deductions: {}",
        net_edge.summarise()
    );
    Ok(())
}

#[test]
fn every_deduction_names_the_reasoning_that_produced_it() -> Result<()> {
    let (graph, depth) = liquid_triangular()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    let net_edge = NetEdgeCalculator::new(EdgeAssumptions::default()).calculate(&pricing, at())?;
    for deduction in net_edge.deductions() {
        assert!(
            !deduction.basis.trim().is_empty(),
            "{} has no stated basis",
            deduction.kind.as_str()
        );
    }
    Ok(())
}

#[test]
fn spread_and_slippage_are_charged_as_separate_costs() -> Result<()> {
    // A thin touch with depth behind it: crossing costs the spread, and the
    // size costs more on top.
    let (graph, depth) = triangular(
        VenueClass::CryptoExchange,
        &[("3000.1", "1"), ("3000.5", "500")],
        &[("0.0505", "200")],
    )?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;

    let swept = pricing
        .legs()
        .find(|leg| leg.object.as_str() == "ETHUSDT")
        .expect("the dollar leg is priced");
    assert!(swept.spread_fraction()? > Decimal::ZERO);
    assert!(
        swept.slippage_fraction()? > Decimal::ZERO,
        "the sweep walked past the touch"
    );
    assert!(swept.executable_price > swept.touch_price);

    let net_edge = NetEdgeCalculator::new(EdgeAssumptions::default()).calculate(&pricing, at())?;
    let amount = |kind: DeductionKind| {
        net_edge
            .deductions()
            .iter()
            .find(|dd| dd.kind == kind)
            .map(|dd| dd.amount)
            .unwrap_or(Decimal::ZERO)
    };
    assert!(amount(DeductionKind::Spread) > Decimal::ZERO);
    assert!(amount(DeductionKind::Slippage) > Decimal::ZERO);
    Ok(())
}

#[test]
fn the_uncertainty_haircut_grows_as_the_inputs_age() -> Result<()> {
    let (graph, depth) = liquid_triangular()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    let calculator = NetEdgeCalculator::new(EdgeAssumptions::default());

    let haircut = |now: Timestamp| -> Result<Decimal> {
        Ok(calculator
            .calculate(&pricing, now)?
            .deductions()
            .iter()
            .find(|dd| dd.kind == DeductionKind::Uncertainty)
            .map(|dd| dd.amount)
            .unwrap_or(Decimal::ZERO))
    };

    let fresh = haircut(at())?;
    let stale = haircut(at().saturating_add(Duration::from_secs(60)))?;
    assert!(stale > fresh, "{stale} should exceed {fresh}");
    Ok(())
}

#[test]
fn the_uncertainty_haircut_grows_as_the_evidence_thins() -> Result<()> {
    let (graph, _) = liquid_triangular()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let calculator = NetEdgeCalculator::new(EdgeAssumptions::default());

    let haircut = |observations: u32| -> Result<Decimal> {
        let cx = venue("CX");
        let depth = StaticLiquidity::new()
            .with_book(
                cx.clone(),
                book(
                    "ETHUSDT",
                    "CX",
                    &[("3000.0", "200")],
                    &[("3000.1", "200")],
                    at(),
                ),
                observations,
            )
            .with_book(
                cx.clone(),
                book(
                    "ETHBTC",
                    "CX",
                    &[("0.0505", "200")],
                    &[("0.05051", "200")],
                    at(),
                ),
                observations,
            )
            .with_book(
                cx,
                book(
                    "BTCUSDT",
                    "CX",
                    &[("60000", "10")],
                    &[("60001", "10")],
                    at(),
                ),
                observations,
            );
        let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
        Ok(calculator
            .calculate(&pricing, at())?
            .deductions()
            .iter()
            .find(|dd| dd.kind == DeductionKind::Uncertainty)
            .map(|dd| dd.amount)
            .unwrap_or(Decimal::ZERO))
    };

    assert!(
        haircut(1)? > haircut(50)?,
        "one observation is worth less than fifty"
    );
    Ok(())
}

#[test]
fn a_book_older_than_the_staleness_limit_is_refused_rather_than_discounted() -> Result<()> {
    let (graph, depth) = liquid_triangular()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    let calculator = NetEdgeCalculator::new(EdgeAssumptions::default());

    let much_later = at().saturating_add(Duration::from_secs(3_600));
    let refusal = calculator
        .calculate(&pricing, much_later)
        .expect_err("an hour-old book is not tradable at any haircut");
    assert_eq!(refusal.code(), "guard");
    Ok(())
}

#[test]
fn the_least_reversible_leg_is_planned_first() -> Result<()> {
    let (graph, depth) = cross_venue_with_a_chain()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    let planned = LegPlanner::new(PlanSettings::with_budget(d("20000"))).plan(&pricing)?;

    let first = planned.plan.steps().first().expect("the plan has legs");
    assert_eq!(
        first.venue,
        venue("DEX1"),
        "the chain leg cannot be unwound, so it is done while nothing else is committed"
    );
    assert!(
        planned.ordering[0].reversibility_f64 < planned.ordering[1].reversibility_f64,
        "the ordering is by reversibility, ascending"
    );
    assert!(
        planned
            .rationale
            .iter()
            .any(|line| line.contains("hardest leg to undo"))
    );
    Ok(())
}

#[test]
fn a_plan_whose_residual_exceeds_its_budget_is_refused() -> Result<()> {
    let (graph, depth) = cross_venue_with_a_chain()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;

    let generous = LegPlanner::new(PlanSettings::with_budget(d("20000"))).plan(&pricing)?;
    assert!(generous.residual_risk > d("100"));

    let refusal = LegPlanner::new(PlanSettings::with_budget(d("100")))
        .plan(&pricing)
        .expect_err("a plan that cannot be sized is not attempted");
    assert_eq!(refusal.code(), "guard");
    assert!(refusal.message().contains("first leg"));
    Ok(())
}

#[test]
fn the_residual_is_reported_per_instrument_it_is_priced_in() -> Result<()> {
    let (graph, depth) = cross_venue_with_a_chain()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    let planned = LegPlanner::new(PlanSettings::with_budget(d("20000"))).plan(&pricing)?;

    let total: Decimal = planned
        .residual_by_quote
        .iter()
        .map(|(_, amount)| *amount)
        .sum();
    assert_eq!(
        total, planned.residual_risk,
        "the breakdown accounts for the whole residual"
    );
    Ok(())
}

#[test]
fn a_leg_whose_input_comes_from_a_venue_that_can_revert_is_prefunded() -> Result<()> {
    // A chain triangle, ordered so each leg runs after the one that feeds it.
    // Even then the feeding leg can roll back, so the input has to be held.
    let (graph, depth) = triangular(
        VenueClass::DecentralisedExchange,
        &[("3000.1", "1"), ("3000.1", "500")],
        &[("0.0505", "2"), ("0.0505", "500")],
    )?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    let planned = LegPlanner::new(PlanSettings::with_budget(d("1000000"))).plan(&pricing)?;

    let held: Vec<&str> = planned
        .plan
        .prefunded
        .iter()
        .map(|(object_id, _)| object_id.as_str())
        .collect();
    assert!(held.contains(&"USDT"), "the starting capital");
    assert!(held.contains(&"ETH"), "produced by a leg that can revert");
    assert!(held.contains(&"BTC"), "produced by a leg that can revert");
    assert!(
        planned
            .rationale
            .iter()
            .any(|line| line.contains("can revert after this one has landed")),
        "and the reason is stated: {:?}",
        planned.rationale
    );
    Ok(())
}

#[test]
fn a_transfer_is_realised_as_held_inventory_rather_than_an_order() -> Result<()> {
    let (graph, depth) = cross_venue_with_a_chain()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    let planned = LegPlanner::new(PlanSettings::with_budget(d("20000"))).plan(&pricing)?;

    assert_eq!(
        planned.plan.len(),
        2,
        "four conversions, two of which are transfers and send no order"
    );
    let held: Vec<&str> = planned
        .plan
        .prefunded
        .iter()
        .map(|(object_id, _)| object_id.as_str())
        .collect();
    assert!(held.contains(&"TKN"), "the transferred inventory is held");
    Ok(())
}

#[test]
fn a_synthetic_against_its_components_is_found_as_a_cross_instrument_path() -> Result<()> {
    let (graph, depth) = synthetic_basket()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].kind, PathKind::CrossInstrument);

    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    assert!(pricing.is_fully_available());
    assert!(pricing.is_profitable_on_book());
    assert_eq!(
        pricing.legs().count(),
        3,
        "two component legs to assemble the basket, one to sell it"
    );

    let report = scanner("100000").scan(&graph, &depth, &SizePolicy::uniform(d("10000")), at());
    assert_eq!(report.opportunities.len(), 1);
    Ok(())
}

#[test]
fn a_halted_venue_never_appears_in_a_candidate() -> Result<()> {
    let (mut graph, depth) = liquid_triangular()?;
    assert_eq!(
        search_candidates(&graph, &SearchSettings::default()).len(),
        1
    );

    graph.register_venue(
        venue("CX"),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Halted),
    );
    assert!(
        search_candidates(&graph, &SearchSettings::default()).is_empty(),
        "a halted venue is not a cheaper venue"
    );
    let report = scanner("1000000").scan(&graph, &depth, &SizePolicy::uniform(d("10000")), at());
    assert!(report.opportunities.is_empty());
    Ok(())
}

#[test]
fn a_venue_with_no_recorded_class_is_treated_as_unusable() -> Result<()> {
    let mut graph = ArbitrageGraph::new();
    graph.add_trade(
        node("A", "UNKNOWN"),
        node("B", "UNKNOWN"),
        d("2"),
        Decimal::ZERO,
        object("AB"),
        BookSide::Bid,
        at(),
        4,
    )?;
    graph.add_trade(
        node("B", "UNKNOWN"),
        node("A", "UNKNOWN"),
        d("2"),
        Decimal::ZERO,
        object("AB"),
        BookSide::Ask,
        at(),
        4,
    )?;
    assert!(
        search_candidates(&graph, &SearchSettings::default()).is_empty(),
        "an unknown venue has unknown settlement, which is not the same as safe"
    );
    Ok(())
}

#[test]
fn a_scan_reports_an_executable_opportunity_with_a_complete_edge_and_a_plan() -> Result<()> {
    let (graph, depth) = liquid_triangular()?;
    let report = scanner("50000").scan(&graph, &depth, &SizePolicy::uniform(d("10000")), at());
    assert_eq!(report.opportunities.len(), 1);

    let opportunity = &report.opportunities[0];
    opportunity.net_edge.require_complete()?;
    assert!(opportunity.net() > Decimal::ZERO);
    assert_eq!(opportunity.planned.plan.len(), 3);
    assert!(opportunity.planned.residual_risk >= Decimal::ZERO);
    assert!(
        opportunity.pricing.end_quantity > opportunity.pricing.start_quantity,
        "the book, not the quote, is what makes it an opportunity"
    );
    Ok(())
}

#[test]
fn a_path_with_no_stated_size_is_refused_rather_than_guessed() -> Result<()> {
    let (graph, depth) = liquid_triangular()?;
    let report = scanner("50000").scan(&graph, &depth, &SizePolicy::default(), at());
    assert!(report.opportunities.is_empty());
    assert_eq!(report.rejected_at(RejectionStage::Unsized).len(), 1);
    Ok(())
}

#[test]
fn scanning_the_same_market_twice_produces_the_same_answer() -> Result<()> {
    let (graph, depth) = liquid_triangular()?;
    let sizes = SizePolicy::uniform(d("10000"));
    let first = scanner("50000").scan(&graph, &depth, &sizes, at());
    let second = scanner("50000").scan(&graph, &depth, &sizes, at());
    assert_eq!(first, second, "a replay must reproduce the run exactly");
    Ok(())
}
