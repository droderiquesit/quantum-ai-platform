//! R12-MESH-5053 and -5055 to -5057: a node is more than an instrument at a
//! venue, and an edge carries more than its fee.
//!
//! Every cost test here prices the same triangular dislocation twice — once
//! with no per-conversion cost and once with exactly one — and asserts that
//! the named deduction line equals the hand computation and that the net
//! edge fell. A line that is computed and not charged, or charged and not
//! named, fails one of the two.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_arbitrage::graph::{
    ArbitrageGraph, EdgeCosts, Node, Representation, SettlementStage, VenueFacts,
};
use qip_arbitrage::liquidity::StaticLiquidity;
use qip_arbitrage::netedge::{EdgeAssumptions, NetEdgeCalculator};
use qip_arbitrage::pricing::price_path;
use qip_arbitrage::search::{SearchSettings, search_candidates};
use qip_contracts::edge::{DeductionKind, NetEdge};
use qip_contracts::message::BookSide;
use qip_contracts::venue::{VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
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

fn book(market: &str, bids: &[(&str, &str)], asks: &[(&str, &str)]) -> OrderBook {
    OrderBook::from_levels(
        object(market),
        "CX",
        at(),
        bids.iter()
            .map(|(p, s)| BookLevel::new(d(p), d(s)))
            .collect(),
        asks.iter()
            .map(|(p, s)| BookLevel::new(d(p), d(s)))
            .collect(),
    )
}

/// The same percent-wide ETH/BTC dislocation the pipeline tests use, with
/// edge 0 (USDT to ETH) the one the per-conversion costs are put on.
fn triangular() -> Result<(ArbitrageGraph, StaticLiquidity)> {
    let cx = venue("CX");
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        cx.clone(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
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
            book("ETHUSDT", &[("3000.0", "200")], &[("3000.1", "200")]),
            20,
        )
        .with_book(
            cx.clone(),
            book("ETHBTC", &[("0.0505", "200")], &[("0.05051", "200")]),
            20,
        )
        .with_book(
            cx,
            book("BTCUSDT", &[("60000", "10")], &[("60001", "10")]),
            20,
        );
    Ok((graph, depth))
}

const SIZE: &str = "10000";

/// Price the cycle with `costs` on edge 0 and return its net edge.
fn net_with(costs: EdgeCosts) -> Result<NetEdge> {
    let (mut graph, depth) = triangular()?;
    graph.set_costs(0, costs)?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let candidate = candidates
        .first()
        .ok_or_else(|| qip_core::Error::invalid("no candidate"))?;
    let pricing = price_path(&graph, &depth, candidate, d(SIZE))?;
    NetEdgeCalculator::new(EdgeAssumptions::default()).calculate(&pricing, at())
}

/// The line of `kind` whose basis starts with `prefix`.
fn line(edge: &NetEdge, kind: DeductionKind, prefix: &str) -> Option<Decimal> {
    edge.deductions()
        .iter()
        .find(|deduction| deduction.kind == kind && deduction.basis.starts_with(prefix))
        .map(|deduction| deduction.amount)
}

/// MESH-5053. Mutation: drop `region` (or `settlement`) from `Node`'s derived
/// identity, or delete the zero-cost refusal in `ArbitrageGraph::validate`,
/// and an assertion here fails — the nodes compare equal, intern to one
/// index, or the free edge is admitted.
#[test]
fn two_nodes_differing_only_in_settlement_stage_or_region_are_distinct_and_no_free_edge_joins_them()
-> Result<()> {
    let settled = node("ETH", "CX");
    let pending = settled.clone().with_settlement(SettlementStage::Pending);
    let elsewhere = settled.clone().in_region("eu-west");
    let wrapped = settled.clone().with_representation(Representation::Wrapped);
    assert_ne!(settled, pending);
    assert_ne!(settled, elsewhere);
    assert_ne!(settled, wrapped);
    assert!(settled.differs_only_in_settlement_or_region(&pending));
    assert!(settled.differs_only_in_settlement_or_region(&elsewhere));
    // A different representation is a different asset claim, not a stage.
    assert!(!settled.differs_only_in_settlement_or_region(&wrapped));
    assert_eq!(pending.label(), "ETH@CX[pending]");
    assert_eq!(elsewhere.label(), "ETH@CX[eu-west]");

    let mut graph = ArbitrageGraph::new();
    for (to, what) in [(&pending, "settlement stage"), (&elsewhere, "region")] {
        let refused = graph
            .add_movement(
                to.clone(),
                settled.clone(),
                Decimal::ZERO,
                EdgeCosts::default(),
                at(),
                1,
            )
            .expect_err("a free edge between two stages of one holding was admitted");
        assert!(
            refused
                .message()
                .contains("differing only in settlement stage or region at no cost"),
            "{what}: {}",
            refused.message()
        );
    }
    // Premise of the refusals: the same movement with a cost stated is
    // admitted, and the three holdings are three nodes.
    graph.add_movement(
        pending.clone(),
        settled.clone(),
        Decimal::ZERO,
        EdgeCosts {
            settlement_risk_fraction: d("0.0001"),
            ..EdgeCosts::default()
        },
        at(),
        1,
    )?;
    graph.add_movement(
        elsewhere.clone(),
        settled.clone(),
        d("0.0002"),
        EdgeCosts::default(),
        at(),
        1,
    )?;
    assert_eq!(graph.node_count(), 3);
    assert!(graph.node_index(&pending) != graph.node_index(&settled));
    assert!(graph.node_index(&elsewhere) != graph.node_index(&settled));
    Ok(())
}

/// MESH-5056, tax and duty. Mutation: drop the tax-and-duty `Fees` line from
/// `NetEdgeCalculator::calculate` (or leave it out of `before_fill`) and the
/// line is absent or the net does not fall.
#[test]
fn a_tax_or_duty_on_a_conversion_is_charged_by_name_and_lowers_the_net_edge() -> Result<()> {
    let without = net_with(EdgeCosts::default())?;
    let with = net_with(EdgeCosts {
        tax_duty_fraction: d("0.001"),
        ..EdgeCosts::default()
    })?;
    // Premise: the line exists and is zero when nothing is stated.
    assert_eq!(
        line(&without, DeductionKind::Fees, "tax and duty"),
        Some(Decimal::ZERO)
    );
    // 10,000 × 0.001.
    assert_eq!(
        line(&with, DeductionKind::Fees, "tax and duty"),
        Some(d("10"))
    );
    assert!(with.net() < without.net(), "{}", with.summarise());
    assert!(with.unconsidered().is_empty());

    // The tax is also part of the remainder a missed leg forfeits a share
    // of: a fill shortfall charged on an untaxed remainder overstates what
    // is forfeited. With a fill probability of one the shortfall is zero
    // whatever `before_fill` holds, so the lines above passed with the tax
    // left out of it (round-2 mutation audit); this half fixes a
    // probability below one and asserts the shortfall is charged on the
    // remainder after the tax. Mutation: drop `per_edge.tax_duty` from
    // `before_fill` and the shortfall is 0.05 too large.
    let partial = net_with(EdgeCosts {
        tax_duty_fraction: d("0.001"),
        fill_probability: d("0.995"),
        ..EdgeCosts::default()
    })?;
    assert_eq!(
        line(&partial, DeductionKind::Fees, "tax and duty"),
        Some(d("10")),
        "premise: the tax is charged here too"
    );
    let shortfall = line(&partial, DeductionKind::Slippage, "fill shortfall")
        .ok_or_else(|| qip_core::Error::invalid("no fill-shortfall line"))?;
    let after_tax = partial.gross_edge()
        - partial
            .deductions()
            .iter()
            .filter(|deduction| {
                deduction.kind != DeductionKind::Uncertainty
                    && !deduction.basis.starts_with("fill shortfall")
            })
            .fold(Decimal::ZERO, |sum, deduction| sum + deduction.amount);
    assert!(
        after_tax > Decimal::ZERO,
        "premise: {}",
        partial.summarise()
    );
    assert_eq!(shortfall, after_tax * d("0.005"), "{}", partial.summarise());
    Ok(())
}

/// MESH-5056, bridge and logistics. Mutation: drop the logistics line.
#[test]
fn a_bridge_or_logistics_charge_is_charged_by_name_and_lowers_the_net_edge() -> Result<()> {
    let without = net_with(EdgeCosts::default())?;
    let with = net_with(EdgeCosts {
        logistics_fraction: d("0.0005"),
        ..EdgeCosts::default()
    })?;
    assert_eq!(
        line(&with, DeductionKind::Fees, "transfer, bridge and logistics"),
        Some(d("5"))
    );
    assert!(with.net() < without.net(), "{}", with.summarise());
    Ok(())
}

/// MESH-5057, settlement risk. Mutation: drop the settlement-risk
/// `Collateral` line.
#[test]
fn settlement_risk_is_charged_by_name_as_an_expected_loss() -> Result<()> {
    let without = net_with(EdgeCosts::default())?;
    let with = net_with(EdgeCosts {
        settlement_risk_fraction: d("0.0002"),
        ..EdgeCosts::default()
    })?;
    assert_eq!(
        line(&with, DeductionKind::Collateral, "settlement risk"),
        Some(d("2"))
    );
    assert!(with.net() < without.net(), "{}", with.summarise());
    Ok(())
}

/// MESH-5057, fill probability. The shortfall is `(1 − p)` of what is left
/// after every other cost and before the uncertainty haircut, recomputed
/// here from the edge's own lines. Mutation: charge the shortfall on gross,
/// or drop it, and the equality or the fall fails.
#[test]
fn a_fill_probability_below_one_forfeits_that_share_of_the_remaining_edge() -> Result<()> {
    let without = net_with(EdgeCosts::default())?;
    let with = net_with(EdgeCosts {
        fill_probability: d("0.995"),
        ..EdgeCosts::default()
    })?;
    let shortfall = line(&with, DeductionKind::Slippage, "fill shortfall")
        .ok_or_else(|| qip_core::Error::invalid("no fill-shortfall line"))?;
    let before_fill = with.gross_edge()
        - with
            .deductions()
            .iter()
            .filter(|deduction| {
                deduction.kind != DeductionKind::Uncertainty
                    && !deduction.basis.starts_with("fill shortfall")
            })
            .fold(Decimal::ZERO, |sum, deduction| sum + deduction.amount);
    // Premise: there is a positive remainder to forfeit a share of.
    assert!(before_fill > Decimal::ZERO, "{}", with.summarise());
    assert_eq!(shortfall, before_fill * d("0.005"));
    assert!(shortfall > Decimal::ZERO);
    assert!(with.net() < without.net(), "{}", with.summarise());
    Ok(())
}

/// MESH-5055, measured borrow. Mutation: drop the measured-borrow `Funding`
/// line, or price it over the round trip instead of the holding period.
#[test]
fn a_measured_borrow_rate_is_charged_over_the_holding_period() -> Result<()> {
    let without = net_with(EdgeCosts::default())?;
    let with = net_with(EdgeCosts {
        borrow_rate_annual: d("0.10"),
        ..EdgeCosts::default()
    })?;
    let years = EdgeAssumptions::default().holding_period.as_years_f64();
    let expected = Decimal::from_f64(0.10 * years)
        .ok_or_else(|| qip_core::Error::invalid("fraction"))?
        * d(SIZE);
    assert_eq!(
        line(&with, DeductionKind::Funding, "measured borrow"),
        Some(expected)
    );
    assert!(expected > Decimal::ZERO);
    assert!(with.net() < without.net(), "{}", with.summarise());
    Ok(())
}

/// Refuse rather than clamp. Mutation: delete a range check in
/// `EdgeCosts::validate` and that cost is admitted.
#[test]
fn a_per_conversion_cost_outside_its_range_is_refused_rather_than_clamped() -> Result<()> {
    let (mut graph, _) = triangular()?;
    for (costs, what) in [
        (
            EdgeCosts {
                tax_duty_fraction: d("1"),
                ..EdgeCosts::default()
            },
            "tax and duty",
        ),
        (
            EdgeCosts {
                logistics_fraction: d("-0.1"),
                ..EdgeCosts::default()
            },
            "logistics",
        ),
        (
            EdgeCosts {
                settlement_risk_fraction: d("1.5"),
                ..EdgeCosts::default()
            },
            "settlement risk",
        ),
        (
            EdgeCosts {
                fill_probability: Decimal::ZERO,
                ..EdgeCosts::default()
            },
            "fill probability",
        ),
        (
            EdgeCosts {
                borrow_rate_annual: d("-0.01"),
                ..EdgeCosts::default()
            },
            "borrow rate",
        ),
    ] {
        let refused = graph
            .set_costs(0, costs)
            .expect_err("an out-of-range cost was admitted");
        assert!(refused.message().contains(what), "{}", refused.message());
    }
    // Premise: an in-range set is admitted on the same edge.
    graph.set_costs(
        0,
        EdgeCosts {
            fill_probability: d("0.95"),
            ..EdgeCosts::default()
        },
    )?;
    Ok(())
}

/// The search sees the costs too: the effective rate it searches on falls by
/// exactly the per-conversion multiplier. Mutation: drop
/// `rate_multiplier` from `ConversionEdge::effective_rate` and the rates
/// compare equal.
#[test]
fn the_search_rate_carries_every_per_conversion_cost() -> Result<()> {
    let (mut graph, _) = triangular()?;
    let before = graph
        .edge(0)
        .ok_or_else(|| qip_core::Error::invalid("edge"))?
        .effective_rate()?;
    graph.set_costs(
        0,
        EdgeCosts {
            tax_duty_fraction: d("0.01"),
            fill_probability: d("0.5"),
            ..EdgeCosts::default()
        },
    )?;
    let after = graph
        .edge(0)
        .ok_or_else(|| qip_core::Error::invalid("edge"))?
        .effective_rate()?;
    // The multiplier is 0.5 × (1 − 0.01) = 0.495, applied once.
    assert_eq!(after, before * d("0.495"));
    Ok(())
}

/// R12-MESH-5069: cross-region coordination latency is a priced input. The
/// same cycle, priced under longer and longer waits for a peer region to
/// agree, loses net edge at every step until the scanner's own refusal —
/// `NetEdge::is_positive` — would turn it away. Mutation: price the latency
/// deduction over the round trip alone and the edge does not move.
#[test]
fn raising_cross_region_coordination_latency_lowers_the_net_edge_until_the_cycle_is_refused()
-> Result<()> {
    let (graph, depth) = triangular()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let candidate = candidates
        .first()
        .ok_or_else(|| qip_core::Error::invalid("no candidate"))?;
    let pricing = price_path(&graph, &depth, candidate, d(SIZE))?;
    let priced = |coordination: Duration| {
        NetEdgeCalculator::new(EdgeAssumptions {
            coordination_latency: coordination,
            ..EdgeAssumptions::default()
        })
        .calculate(&pricing, at())
    };

    // Premise: alone in its region the cycle is worth taking, so the refusal
    // below is the coordination wait and not a path that never paid.
    let alone = priced(Duration::from_nanos(0))?;
    assert!(alone.is_positive(), "{}", alone.summarise());

    let mut previous = alone.net();
    let mut refused_at = None;
    for millis in [
        50,
        500,
        5_000,
        60_000,
        3_600_000,
        86_400_000,
        7 * 86_400_000,
        30 * 86_400_000,
        365 * 86_400_000,
    ] {
        let edge = priced(Duration::from_millis(millis))?;
        assert!(
            edge.net() < previous,
            "{millis} ms of coordination did not lower the net edge: {}",
            edge.summarise()
        );
        previous = edge.net();
        if !edge.is_positive() {
            refused_at = Some(millis);
            break;
        }
    }
    assert!(
        refused_at.is_some(),
        "a year of cross-region coordination still left the cycle worth taking at {previous}"
    );
    Ok(())
}
