//! MESH-025: cycles are ranked and accepted on what they pay after costs at
//! their size, never on the spread their quotes show.
//!
//! Quoted spread is the number that makes almost every cycle look good. A
//! scan that ordered its opportunities by it would send the desk's capped
//! capital to whichever cycle hid its costs best: the one whose top of book
//! is a sliver in front of a much worse price. `ScanReport` is sorted by
//! `NetEdge::net`, and until this suite nothing failed if that sort was
//! changed to the search's own log gain.
//!
//! What this suite does not hold, because the engine does not do it: the
//! requirement's executable expected value is weighted by each leg's
//! probability of fill, and no fill probability exists in the tree. The
//! value ranked here is the after-cost value at the cycle's size.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_arbitrage::graph::{ArbitrageGraph, Node, VenueFacts};
use qip_arbitrage::liquidity::StaticLiquidity;
use qip_arbitrage::netedge::EdgeAssumptions;
use qip_arbitrage::plan::PlanSettings;
use qip_arbitrage::scan::{OpportunityScanner, RejectionStage, SizePolicy};
use qip_arbitrage::search::{PathCandidate, SearchSettings, confirm_exact};
use qip_contracts::message::BookSide;
use qip_contracts::venue::{VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook};

const VENUE: &str = "CX";

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("test fixture decimal")
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

fn node(name: &str) -> Node {
    Node::new(object(name), VenueId::new(VENUE))
}

fn levels(levels: &[(Decimal, Decimal)]) -> Vec<BookLevel> {
    levels
        .iter()
        .map(|(price, size)| BookLevel::new(*price, *size))
        .collect()
}

/// A price in ten-thousandths of a percent above 0.05, exact.
fn cross(ticks: u64) -> Decimal {
    d("0.05") + Decimal::from_int(ticks as i64) * d("0.000001")
}

/// One dollar triangle on its own three instruments, `tag` keeping it apart
/// from any other in the same graph: dollars to ETH on the offer, ETH to BTC
/// on the bids of the cross, BTC back to dollars on the bid.
///
/// `cross_bids` is the cross's bid side, best first. The edge's quoted rate
/// is the best bid, which is what the cell's refresh writes; the book is
/// what a fill meets. A sliver at a high best bid in front of a worse level
/// is a cycle whose quote flatters it.
fn add_triangle(
    graph: &mut ArbitrageGraph,
    depth: StaticLiquidity,
    tag: &str,
    cross_bids: &[(Decimal, Decimal)],
) -> Result<StaticLiquidity> {
    let venue = VenueId::new(VENUE);
    let (usd, eth, btc) = (
        format!("USD{tag}"),
        format!("ETH{tag}"),
        format!("BTC{tag}"),
    );
    let (eth_usd, eth_btc, btc_usd) = (
        format!("ETHUSD{tag}"),
        format!("ETHBTC{tag}"),
        format!("BTCUSD{tag}"),
    );
    let fee = d("0.0004");
    graph.add_trade(
        node(&usd),
        node(&eth),
        d("0.000333322"),
        fee,
        object(&eth_usd),
        BookSide::Ask,
        at(),
        20,
    )?;
    graph.add_trade(
        node(&eth),
        node(&btc),
        cross_bids[0].0,
        fee,
        object(&eth_btc),
        BookSide::Bid,
        at(),
        20,
    )?;
    graph.add_trade(
        node(&btc),
        node(&usd),
        d("60000"),
        fee,
        object(&btc_usd),
        BookSide::Bid,
        at(),
        20,
    )?;
    let deep = d("200");
    Ok(depth
        .with_book(
            venue.clone(),
            OrderBook::from_levels(
                object(&eth_usd),
                VENUE,
                at(),
                levels(&[(d("3000.0"), deep)]),
                levels(&[(d("3000.1"), deep)]),
            ),
            20,
        )
        .with_book(
            venue.clone(),
            OrderBook::from_levels(
                object(&eth_btc),
                VENUE,
                at(),
                levels(cross_bids),
                // A tick above the best bid, so the spread charged is the
                // book's own narrow one and what separates the two cycles
                // is the depth behind the best bid, not a wide market.
                levels(&[(cross_bids[0].0 + d("0.00001"), deep)]),
            ),
            20,
        )
        .with_book(
            venue,
            OrderBook::from_levels(
                object(&btc_usd),
                VENUE,
                at(),
                levels(&[(d("60000"), d("10"))]),
                levels(&[(d("60001"), d("10"))]),
            ),
            20,
        ))
}

fn scanner() -> OpportunityScanner {
    OpportunityScanner::new(
        SearchSettings::default(),
        EdgeAssumptions::default(),
        PlanSettings::with_budget(d("50000")),
    )
}

/// The quoted spread of a candidate: the exact product of its quoted,
/// cost-adjusted rates.
fn quoted(graph: &ArbitrageGraph, candidate: &PathCandidate) -> Result<Decimal> {
    Ok(confirm_exact(graph, candidate)?.multiple)
}

#[test]
fn when_spread_and_executable_value_disagree_the_cycle_worth_more_after_costs_ranks_first_and_one_that_loses_is_never_accepted()
-> Result<()> {
    let mut rng = Xoshiro256::seeded(25);
    let venue = VenueId::new(VENUE);
    let sizes = SizePolicy::uniform(d("10000"));
    let (mut disagreements, mut refused_despite_quote, mut pairs_accepted) = (0u32, 0u32, 0u32);

    for _ in 0..400 {
        // Flattered: a best bid well above fair, a sliver deep, in front of
        // a level somewhere between slightly and badly worse.
        let flattered_top = 700 + rng.below(700);
        let sliver = Decimal::from_int(1 + rng.below(9) as i64) * d("0.1");
        let behind = 300 + rng.below(350);
        // Honest: a best bid above fair by less than the flattered one's,
        // and the whole size available at it.
        let honest_top = 500 + rng.below(flattered_top - 550);

        let mut graph = ArbitrageGraph::new();
        graph.register_venue(
            venue.clone(),
            VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
        );
        let depth = add_triangle(
            &mut graph,
            StaticLiquidity::new(),
            "f",
            &[(cross(flattered_top), sliver), (cross(behind), d("200"))],
        )?;
        let depth = add_triangle(&mut graph, depth, "h", &[(cross(honest_top), d("200"))])?;

        let report = scanner().scan(&graph, &depth, &sizes, at());

        // Never accepted at a loss, whatever the quote said.
        for opportunity in &report.opportunities {
            assert!(opportunity.net() > Decimal::ZERO);
            assert!(opportunity.pricing.is_profitable_on_book());
        }
        for rejection in &report.rejections {
            if matches!(
                rejection.stage,
                RejectionStage::Book | RejectionStage::NetEdge
            ) && quoted(&graph, &rejection.candidate)? > Decimal::ONE
            {
                refused_despite_quote += 1;
            }
        }

        // Ranked by what is kept, always.
        for pair in report.opportunities.windows(2) {
            assert!(
                pair[0].net() >= pair[1].net(),
                "an opportunity netting {} was ranked above one netting {}",
                pair[1].net(),
                pair[0].net()
            );
        }

        if let [first, second] = report.opportunities.as_slice() {
            pairs_accepted += 1;
            let (first_quote, second_quote) = (
                quoted(&graph, &first.candidate)?,
                quoted(&graph, &second.candidate)?,
            );
            if first.net() > second.net() && first_quote < second_quote {
                // The two orders disagree, and the one worth more after
                // costs is first even though its quote is the worse one.
                disagreements += 1;
                assert!(first.candidate.log_gain_f64 < second.candidate.log_gain_f64);
            }
            assert!(
                first.net() >= second.net(),
                "the cycle with the higher quoted spread outranked the one worth more"
            );
        }
    }

    // Premise, about the run as a whole: the generator really produced
    // pairs that were both accepted, really produced pairs the two orders
    // disagree about, and really produced cycles a quote liked and the book
    // or the deductions refused. Without these every assertion above could
    // have passed over reports with one opportunity in them.
    assert!(pairs_accepted > 100, "only {pairs_accepted} pairs accepted");
    assert!(
        disagreements > 50,
        "spread and executable value disagreed in only {disagreements} generated pairs"
    );
    assert!(
        refused_despite_quote > 20,
        "only {refused_despite_quote} cycles were quoted profitable and refused"
    );
    Ok(())
}
