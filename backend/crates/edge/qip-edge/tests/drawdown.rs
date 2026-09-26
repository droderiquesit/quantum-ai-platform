//! A grant's drawdown limit fires, because the cell books the losses it makes.
//!
//! `CapitalEnvelope::admit` has always refused once `realised_loss` reached
//! the grant's `loss_limit`. Nothing in the cell ever wrote `realised_loss`,
//! so the figure read zero for ever and every grant carried a limit that
//! could not fire (CAPITAL-026) — the `MaxExpectedShortfall` defect in a new
//! place. The contracts test that "proved" the refusal built a `Utilisation`
//! by hand, which is exactly why it never noticed.
//!
//! Every test here drives the cell through `Cell::work` and a placer that
//! fills what it is sent, and reads what the cell refused, journaled and
//! reported. Both seams that move a strategy's book are exercised — venue
//! fills and internal crosses — because a ledger fed from one of them books
//! the other's closing trade as an opening one.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_arbitrage::{
    ArbitrageGraph, EdgeAssumptions, Node, OpportunityScanner, PlanSettings, SearchSettings,
    SizePolicy, VenueFacts,
};
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{BeliefPriors, CausalDigest, EpisodicDigest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Duration, ObjectId, Timestamp};
use qip_edge::arbitrage::ArbitrageDesk;
use qip_edge::cell::{
    Cell, CellConfig, ExecutionReport, Placer, PricingPolicy, QuoteTerms, WorkReport,
};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::collections::BTreeMap;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const ALPHA: &str = "alpha";
const BETA: &str = "beta";
const DESK: &str = "arb-desk";
const CAPITAL: &str = "capital";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-drawdown-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-drawdown-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn acme() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn strategy(id: &str) -> StrategyId {
    StrategyId::new(id)
}

/// A two-sided book on `market`, built through the feed path.
fn book_on(market: &ObjectId, bid: &str, ask: &str, size: &str) -> Result<VenueState> {
    let mut state = VenueState::aggregated(market.clone(), venue(), VenueStatus::Open);
    for (index, (side, price)) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            market.clone(),
            Origin::new(venue(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side,
                price: d(price),
                quantity: d(size),
                order_count: None,
            },
            when,
            when,
        ))?;
    }
    Ok(state)
}

/// Re-quote ACME at `bid` / `ask`, deep enough for every order here.
fn quote(cell: &mut Cell, bid: &str, ask: &str) -> Result<()> {
    cell.track(book_on(&acme(), bid, ask, "1000")?);
    Ok(())
}

/// A strategy whose one rule always holds, so every pass raises the same
/// signal at the same size.
fn firing(id: &str, kind: SignalKind, size: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec =
        StrategySpec::new(strategy(id), acme(), Duration::from_secs(30)).with_rule(Rule::new(
            "always",
            kind,
            Expr::Flag(true),
            Expr::Exact(d(size)),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// A grant for `id` with the given drawdown limit, issued at `granted` and
/// verified by the cell at `granted + 1`.
fn grant(id: &str, loss_limit: &str, granted: i64) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            strategy(id),
            CELL,
            d("1000000"),
            d("100000"),
            d(loss_limit),
            vec![venue()],
            t(granted),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(granted + 1))
}

fn deploy(
    cell: &mut Cell,
    id: &str,
    kind: SignalKind,
    size: &str,
    envelope: &VerifiedEnvelope,
) -> Result<()> {
    let (compiled, program) = firing(id, kind, size)?;
    cell.deploy_with_pricing(
        compiled,
        program,
        envelope.clone(),
        PricingPolicy::Marketable,
    )
}

/// A payload whose sizing slots are fresh, so the multiplier is one and an
/// order is exactly the size its signal asked for — the arithmetic below is
/// then the arithmetic a reader can check by hand.
fn fresh_policy(issued_at: Timestamp) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, issued_at);
    payload.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::new(),
        },
        issued_at,
    );
    payload.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: Vec::new(),
        },
        issued_at,
    );
    payload.episodic_digest = Slot::produced(
        EpisodicDigest {
            digest: "d".to_string(),
            episodes: 0,
        },
        issued_at,
    );
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, issued_at)
}

/// A cell quoting ACME at 99 / 101 under a fresh policy.
fn cell() -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    cell.apply_policy(fresh_policy(t(5))?, t(5))?;
    quote(&mut cell, "99", "101")?;
    Ok(cell)
}

/// A simulated venue that fills every order it accepts, whole, at the
/// order's own price, and states the listing's quote unit when given one.
#[derive(Debug, Default)]
struct FillingVenue {
    placed: Vec<(String, ObjectId, BookSide, Decimal, Decimal)>,
    reports: Vec<ExecutionReport>,
    quote_unit: Option<Currency>,
}

impl Placer for FillingVenue {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        self.placed.push((
            order_id.to_string(),
            object_id.clone(),
            side,
            quantity,
            price,
        ));
        self.reports.push(ExecutionReport {
            order_id: order_id.to_string(),
            venue: venue.clone(),
            quantity,
            price,
            at,
        });
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.reports)
    }

    fn quote_terms(&self, _object_id: &ObjectId, _venue: &VenueId) -> Option<QuoteTerms> {
        self.quote_unit.map(|quote_unit| QuoteTerms { quote_unit })
    }
}

/// The reasons refused under exactly `gate` — delimited equality, because
/// `capital_reduced` begins with `capital`.
fn refused_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(refused, _)| refused == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

/// A reason split into its words, so a figure or an id is matched whole: a
/// substring test would find `20` inside `200` and `240` alike.
fn tokens(reason: &str) -> Vec<&str> {
    reason
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';' | ':'))
        .map(|token| token.trim_end_matches('.'))
        .filter(|token| !token.is_empty())
        .collect()
}

/// What the cell's delta reports as `id`'s realised loss.
fn reported_loss(cell: &Cell, report: &WorkReport, id: &str, at: Timestamp) -> Result<Decimal> {
    cell.state_delta(report, at)
        .utilisation
        .iter()
        .find(|entry| entry.strategy.as_str() == id)
        .map(|entry| entry.utilisation.realised_loss)
        .ok_or_else(|| Error::not_found(format!("the delta carries no utilisation for {id}")))
}

/// Exactly one order and exactly one fill, and nothing refused on capital:
/// the commitment was admitted and the venue traded it.
fn assert_traded(report: &WorkReport, what: &str) {
    assert_eq!(
        report.orders.len(),
        1,
        "{what}: no order went out: {report:?}"
    );
    assert_eq!(
        report.fills.len(),
        1,
        "{what}: the order did not fill: {report:?}"
    );
    assert!(
        refused_under(report, CAPITAL).is_empty(),
        "{what}: capital refused a commitment the limit had not yet been reached for: {:?}",
        report.refusals
    );
}

const SIZE: &str = "10";

/// One losing round trip for alpha under `envelope`: buy ten at the ask of a
/// 99 / 101 book, then — redeployed as an exit under the same grant, the path
/// a plan that changes one rule takes — sell the ten at the bid of 89 / 91.
/// Loses (101 − 89) × 10 = 120.
fn losing_round_trip(
    cell: &mut Cell,
    gateway: &mut FillingVenue,
    envelope: &VerifiedEnvelope,
    start: i64,
) -> Result<WorkReport> {
    quote(cell, "99", "101")?;
    deploy(cell, ALPHA, SignalKind::Enter, SIZE, envelope)?;
    let bought = cell.work(t(start), gateway)?;
    assert_traded(&bought, "the buy");
    assert_eq!(bought.fills[0].price, d("101"));
    quote(cell, "89", "91")?;
    deploy(cell, ALPHA, SignalKind::Exit, SIZE, envelope)?;
    let sold = cell.work(t(start + 10), gateway)?;
    assert_traded(&sold, "the sell");
    assert_eq!(sold.fills[0].price, d("89"));
    Ok(sold)
}

const LIMIT: &str = "200";
const PER_TRIP: &str = "120";

/// Losing round trips until alpha's reported loss reaches `LIMIT`. Returns the
/// grant, the cell, the venue, the last report and how many trips it took.
fn breach() -> Result<(VerifiedEnvelope, Cell, FillingVenue, WorkReport, usize)> {
    let envelope = grant(ALPHA, LIMIT, 0)?;
    let mut cell = cell()?;
    let mut gateway = FillingVenue::default();
    let mut last = WorkReport::default();
    let mut trips = 0;
    // Bounded: two trips reach the limit. A loop that never did would be a
    // limit that never fires, and it must fail here rather than spin.
    while trips < 5 {
        let start = 10 + 20 * i64::try_from(trips).map_err(|_| Error::invalid("trip count"))?;
        last = losing_round_trip(&mut cell, &mut gateway, &envelope, start)?;
        trips += 1;
        if reported_loss(&cell, &last, ALPHA, t(start + 11))? >= d(LIMIT) {
            break;
        }
    }
    Ok((envelope, cell, gateway, last, trips))
}

#[test]
fn losses_booked_from_fills_reach_the_grants_drawdown_limit_and_the_next_commitment_is_refused()
-> Result<()> {
    let (_, mut cell, mut gateway, last, trips) = breach()?;
    // Premise: the first trip's loss sat under the limit and the second
    // trip's orders were admitted anyway — every commitment before the
    // breach went out, so the refusal below is the limit and not a cell that
    // refuses everything.
    assert_eq!(
        trips, 2,
        "the loss did not reach the limit in two losing trips"
    );
    let loss = reported_loss(&cell, &last, ALPHA, t(41))?;
    assert_eq!(loss, d(PER_TRIP) + d(PER_TRIP));
    let sent = gateway.placed.len();
    assert_eq!(sent, 4, "the premise is four orders sent, all admitted");

    let next = cell.work(t(60), &mut gateway)?;
    assert_eq!(
        next.signals.len(),
        1,
        "the premise is a strategy that still signals"
    );
    assert!(
        next.orders.is_empty() && gateway.placed.len() == sent,
        "the grant's drawdown limit did not fire: an order went out after a realised loss of \
         {loss} against a {LIMIT} limit: {:?}",
        next.orders
    );
    let capital = refused_under(&next, CAPITAL);
    assert_eq!(
        capital.len(),
        1,
        "the capital gate did not refuse exactly once: {:?}",
        next.refusals
    );
    let words = tokens(capital[0]);
    let (loss, limit) = (loss.to_string(), d(LIMIT).to_string());
    assert!(
        words.contains(&loss.as_str()) && words.contains(&limit.as_str()),
        "the refusal does not name the loss {loss} and the limit {limit}: {}",
        capital[0]
    );
    Ok(())
}

#[test]
fn the_drawdown_breach_is_journaled_with_the_grant_and_the_loss_that_reached_the_limit()
-> Result<()> {
    let (envelope, mut cell, mut gateway, last, _) = breach()?;
    let loss = reported_loss(&cell, &last, ALPHA, t(41))?;
    cell.work(t(60), &mut gateway)?;

    let refused: Vec<&str> = cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::Refused { gate, reason } if gate == CAPITAL => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        refused.len(),
        1,
        "the chain does not hold exactly one capital refusal: {refused:?}"
    );
    let words = tokens(refused[0]);
    for (what, expected) in [
        ("the strategy", ALPHA.to_string()),
        ("the grant's signature", envelope.signature().to_string()),
        ("the realised loss", loss.to_string()),
        ("the limit", d(LIMIT).to_string()),
    ] {
        assert!(
            !expected.is_empty() && words.contains(&expected.as_str()),
            "the journaled breach does not name {what} ({expected}): {}",
            refused[0]
        );
    }
    Ok(())
}

#[test]
fn the_cells_delta_reports_the_realised_loss_it_booked() -> Result<()> {
    let envelope = grant(ALPHA, "1000", 0)?;
    let mut cell = cell()?;
    let mut gateway = FillingVenue::default();
    let sold = losing_round_trip(&mut cell, &mut gateway, &envelope, 10)?;
    // Premise: one losing round trip, bought at 101 and sold at 89.
    let entry = d("101");
    let exit = d("89");
    assert!(exit < entry);
    let booked = (entry - exit) * d(SIZE);

    let delta = cell.state_delta(&sold, t(21));
    let reported: Vec<Decimal> = delta
        .utilisation
        .iter()
        .filter(|entry| entry.strategy.as_str() == ALPHA)
        .map(|entry| entry.utilisation.realised_loss)
        .collect();
    assert_eq!(
        reported,
        vec![booked],
        "the delta the centre sums (`realised_loss_by_cell`) does not carry the loss the cell \
         booked"
    );
    Ok(())
}

#[test]
fn a_winning_round_trip_books_no_realised_loss() -> Result<()> {
    let envelope = grant(ALPHA, "1000", 0)?;
    let mut cell = cell()?;
    let mut gateway = FillingVenue::default();
    deploy(&mut cell, ALPHA, SignalKind::Enter, SIZE, &envelope)?;
    let bought = cell.work(t(10), &mut gateway)?;
    assert_traded(&bought, "the buy");
    quote(&mut cell, "119", "121")?;
    deploy(&mut cell, ALPHA, SignalKind::Exit, SIZE, &envelope)?;
    let sold = cell.work(t(20), &mut gateway)?;
    assert_traded(&sold, "the sell");
    // Premise: a win, sold above what was paid.
    assert!(
        sold.fills[0].price > bought.fills[0].price,
        "the premise is a winning trip: bought {} sold {}",
        bought.fills[0].price,
        sold.fills[0].price
    );
    assert_eq!(
        reported_loss(&cell, &sold, ALPHA, t(21))?,
        Decimal::ZERO,
        "a gain was booked as a loss"
    );
    Ok(())
}

#[test]
fn a_loss_realised_on_a_position_opened_by_an_internal_cross_reaches_the_drawdown_limit()
-> Result<()> {
    // Alpha buys 20 and beta sells 30 on the same pass: 20 crosses between
    // them at the mid of 100 and only the net 10 reaches the venue, as a
    // sell at 99 split pro rata — 4 to alpha, 6 to beta. Alpha holds 16
    // entered at 100 and has realised (99 − 100) × 4 = −4.
    let limit = "500";
    let alpha = grant(ALPHA, limit, 0)?;
    let beta = grant(BETA, "1000000", 0)?;
    let mut cell = cell()?;
    let mut gateway = FillingVenue::default();
    deploy(&mut cell, ALPHA, SignalKind::Enter, "20", &alpha)?;
    deploy(&mut cell, BETA, SignalKind::Exit, "30", &beta)?;
    let crossed = cell.work(t(10), &mut gateway)?;

    // Premise: a cross, and no venue fill for the crossed quantity.
    assert_eq!(crossed.crosses.len(), 1, "no cross: {crossed:?}");
    let cross = &crossed.crosses[0];
    assert_eq!(cross.quantity, d("20"));
    assert_eq!(cross.price, d("100"));
    assert_eq!(cross.bought, vec![strategy(ALPHA)]);
    assert_eq!(cross.sold, vec![strategy(BETA)]);
    assert_eq!(gateway.placed.len(), 1);
    assert_eq!(
        gateway.placed[0].3,
        d("10"),
        "the venue was sent more than the net, so the crossed 20 did not stay inside the cell"
    );
    let held = cell.strategy_position(&strategy(ALPHA), &venue(), &acme());
    assert_eq!(
        held,
        d("16"),
        "alpha's lot is not the cross less its venue share"
    );

    // Alpha alone now, selling its 16 into a book that has halved.
    cell.withdraw(BETA, t(15))?;
    quote(&mut cell, "49", "51")?;
    deploy(&mut cell, ALPHA, SignalKind::Exit, "16", &alpha)?;
    let closed = cell.work(t(20), &mut gateway)?;
    assert_traded(&closed, "the closing sell");
    assert_eq!(closed.fills[0].price, d("49"));
    // (49 − 100) × 16 on the crossed entry, plus the 4 realised on the pass
    // that crossed.
    let loss = d("820");
    assert!(loss >= d(limit), "the premise is a loss past the limit");

    let next = cell.work(t(30), &mut gateway)?;
    assert_eq!(
        next.signals.len(),
        1,
        "the premise is a strategy that still signals"
    );
    assert!(
        next.orders.is_empty(),
        "an order went out after alpha lost {loss} closing a lot a cross opened: the closing \
         fill was booked as an opening trade: {:?}",
        next.orders
    );
    let capital = refused_under(&next, CAPITAL);
    assert_eq!(capital.len(), 1, "{:?}", next.refusals);
    let loss = loss.to_string();
    assert!(
        tokens(capital[0]).contains(&loss.as_str()),
        "the refusal does not name the loss {loss}: {}",
        capital[0]
    );
    Ok(())
}

#[test]
fn a_loss_realised_by_closing_a_venue_position_through_an_internal_cross_is_booked() -> Result<()> {
    let alpha = grant(ALPHA, "1000000", 0)?;
    let beta = grant(BETA, "1000000", 0)?;
    let mut cell = cell()?;
    let mut gateway = FillingVenue::default();
    deploy(&mut cell, ALPHA, SignalKind::Enter, "30", &alpha)?;
    let opened = cell.work(t(10), &mut gateway)?;
    assert_traded(&opened, "the opening buy");
    let entry = opened.fills[0].price;
    assert_eq!(
        cell.strategy_position(&strategy(ALPHA), &venue(), &acme()),
        d("30"),
        "the premise is alpha holding 30 bought at the venue"
    );

    // The book falls to a mid of 90. Alpha exits 30 as beta enters 45: the
    // 30 cross inside the cell at the mid, and only beta's residual reaches
    // the venue.
    quote(&mut cell, "89", "91")?;
    deploy(&mut cell, ALPHA, SignalKind::Exit, "30", &alpha)?;
    deploy(&mut cell, BETA, SignalKind::Enter, "45", &beta)?;
    let crossed = cell.work(t(20), &mut gateway)?;
    assert_eq!(crossed.crosses.len(), 1, "no cross: {crossed:?}");
    let cross = &crossed.crosses[0];
    assert_eq!(
        cross.sold,
        vec![strategy(ALPHA)],
        "alpha did not sell into the cross"
    );
    assert_eq!(cross.quantity, d("30"));
    assert!(
        cross.price < entry,
        "the premise is a cross at a worse mid than the entry"
    );

    assert_eq!(
        reported_loss(&cell, &crossed, ALPHA, t(21))?,
        (entry - cross.price) * cross.quantity,
        "closing a venue position through a cross realised nothing"
    );
    Ok(())
}

#[test]
fn a_fill_the_cell_cannot_price_stops_its_owners_next_commitment_until_a_fresh_grant() -> Result<()>
{
    let original = grant(ALPHA, "1000000", 0)?;
    let mut cell = cell()?;
    let mut gateway = FillingVenue {
        quote_unit: Some(Currency::parse("GBP")?),
        ..FillingVenue::default()
    };
    deploy(&mut cell, ALPHA, SignalKind::Enter, SIZE, &original)?;
    assert_traded(
        &cell.work(t(10), &mut gateway)?,
        "the opening buy, in pounds",
    );

    // The listing now states dollars. The next buy is admitted — nothing is
    // wrong yet — and fills in a unit its position was not opened in.
    gateway.quote_unit = Some(Currency::parse("USD")?);
    let added = cell.work(t(20), &mut gateway)?;
    assert_traded(&added, "the second buy");
    let unpriced = added.orders[0].order_id.clone();
    // Premise: the fill was confirmed and journaled, in dollars.
    let journaled = cell.journal().entries().iter().any(|entry| {
        matches!(
            &entry.decision,
            Decision::Filled { order_id, quote_unit, .. }
                if *order_id == unpriced && quote_unit.as_deref() == Some("USD")
        )
    });
    assert!(
        journaled,
        "the premise is a dollar fill in the chain on {unpriced}"
    );

    let stopped = cell.work(t(30), &mut gateway)?;
    assert_eq!(
        stopped.signals.len(),
        1,
        "the premise is a strategy that still signals"
    );
    assert!(
        stopped.orders.is_empty(),
        "an order went out for a strategy holding a fill the cell could not price: its loss \
         was read as zero: {:?}",
        stopped.orders
    );
    let capital = refused_under(&stopped, CAPITAL);
    assert_eq!(capital.len(), 1, "{:?}", stopped.refusals);
    assert!(
        tokens(capital[0]).contains(&unpriced.as_str()),
        "the refusal does not name the fill it could not price ({unpriced}): {}",
        capital[0]
    );

    // A redeploy under the grant in force at the fill is not a clear: it was
    // signed before anybody knew of the fill.
    deploy(&mut cell, ALPHA, SignalKind::Enter, SIZE, &original)?;
    let still = cell.work(t(40), &mut gateway)?;
    assert!(
        still.orders.is_empty(),
        "an unsigned redeploy cleared the latch"
    );
    assert_eq!(refused_under(&still, CAPITAL).len(), 1);

    // A grant issued after the fill is the operator's signed clear.
    let fresh = grant(ALPHA, "1000000", 100)?;
    deploy(&mut cell, ALPHA, SignalKind::Enter, SIZE, &fresh)?;
    let resumed = cell.work(t(110), &mut gateway)?;
    assert_traded(&resumed, "the first buy under the fresh grant");
    Ok(())
}

// --- the arbitrage desk -----------------------------------------------------

fn desk_node(name: &str) -> Node {
    Node::new(ObjectId::from_string(name), venue())
}

/// One conversion, quoted at a placeholder the desk re-quotes from the books
/// before every scan.
fn conversion(
    graph: &mut ArbitrageGraph,
    from: &str,
    to: &str,
    market: &str,
    side: BookSide,
) -> Result<()> {
    graph.add_trade(
        desk_node(from),
        desk_node(to),
        Decimal::ONE,
        d("0.0004"),
        ObjectId::from_string(market),
        side,
        t(0),
        0,
    )?;
    Ok(())
}

/// The ETH / BTC / USDT triangle, both ways round: forward buys ETH with
/// dollars, sells it for bitcoin and sells the bitcoin; reverse undoes each.
fn both_ways() -> Result<ArbitrageGraph> {
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        venue(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    conversion(&mut graph, "USDT", "ETH", "ETHUSDT", BookSide::Ask)?;
    conversion(&mut graph, "ETH", "BTC", "ETHBTC", BookSide::Bid)?;
    conversion(&mut graph, "BTC", "USDT", "BTCUSDT", BookSide::Bid)?;
    conversion(&mut graph, "USDT", "BTC", "BTCUSDT", BookSide::Ask)?;
    conversion(&mut graph, "BTC", "ETH", "ETHBTC", BookSide::Ask)?;
    conversion(&mut graph, "ETH", "USDT", "ETHUSDT", BookSide::Bid)?;
    Ok(graph)
}

fn track_desk_books(cell: &mut Cell, eth: (&str, &str), cross: (&str, &str)) -> Result<()> {
    cell.track(book_on(
        &ObjectId::from_string("ETHUSDT"),
        eth.0,
        eth.1,
        "200",
    )?);
    cell.track(book_on(
        &ObjectId::from_string("ETHBTC"),
        cross.0,
        cross.1,
        "200",
    )?);
    cell.track(book_on(
        &ObjectId::from_string("BTCUSDT"),
        "60000",
        "60001",
        "10",
    )?);
    Ok(())
}

#[test]
fn an_arbitrage_desks_losses_reach_its_envelopes_drawdown_limit_too() -> Result<()> {
    let limit = "1000";
    let envelope = grant(DESK, limit, 0)?;
    let desk = ArbitrageDesk::new(
        strategy(DESK),
        OpportunityScanner::new(
            SearchSettings::default(),
            EdgeAssumptions::default(),
            PlanSettings::with_budget(d("50000")),
        ),
        both_ways()?,
        SizePolicy::uniform(d("10000"))
            .with(ObjectId::from_string("ETH"), d("3.3"))
            .with(ObjectId::from_string("BTC"), d("0.16")),
        envelope.clone(),
        4,
        Duration::from_secs(30),
    )?;
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_arbitrage(desk)?;
    cell.apply_policy(fresh_policy(t(5))?, t(5))?;
    let mut gateway = FillingVenue::default();

    // Forward: ETH/BTC a percent rich to what the dollar legs imply. The
    // desk buys ETH at 3000.1 and sells the rest of the triangle.
    track_desk_books(&mut cell, ("3000", "3000.1"), ("0.0505", "0.05051"))?;
    let forward = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        forward.orders.len(),
        3,
        "the forward cycle did not go out: {forward:?}"
    );
    assert_eq!(forward.fills.len(), 3, "the forward cycle did not fill");
    let bought_eth = forward
        .fills
        .iter()
        .find(|fill| fill.object_id.as_str() == "ETHUSDT")
        .ok_or_else(|| Error::not_found("an ETH/USDT fill on the forward cycle"))?;
    assert_eq!(bought_eth.side, BookSide::Ask, "the premise is ETH bought");

    // Reverse, after ETH has fallen by a third: the desk unwinds each leg,
    // selling the ETH it paid 3000.1 for at 2000.
    track_desk_books(&mut cell, ("2000", "2000.1"), ("0.0327", "0.03271"))?;
    let reverse = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        reverse.orders.len(),
        3,
        "the reverse cycle did not go out: {reverse:?}"
    );
    assert_eq!(reverse.fills.len(), 3, "the reverse cycle did not fill");
    assert!(
        refused_under(&reverse, CAPITAL).is_empty(),
        "the premise is a reverse cycle admitted before any loss was booked"
    );
    let sold_eth = reverse
        .fills
        .iter()
        .find(|fill| fill.object_id.as_str() == "ETHUSDT")
        .ok_or_else(|| Error::not_found("an ETH/USDT fill on the reverse cycle"))?;
    assert_eq!(sold_eth.side, BookSide::Bid, "the premise is ETH sold");
    assert!(
        sold_eth.price < bought_eth.price,
        "the premise is ETH sold at a loss"
    );

    let sent = gateway.placed.len();
    track_desk_books(&mut cell, ("2000", "2000.1"), ("0.0327", "0.03271"))?;
    let next = cell.work(t(30), &mut gateway)?;
    assert!(
        next.orders.is_empty() && gateway.placed.len() == sent,
        "the desk sent another leg after its losses passed its envelope's {limit} drawdown \
         limit: {:?}",
        next.orders
    );
    let capital = refused_under(&next, CAPITAL);
    assert!(
        !capital.is_empty(),
        "nothing was refused on capital: {:?}",
        next.refusals
    );

    let loss = cell
        .arbitrage()
        .map(|desk| desk.utilisation().realised_loss)
        .ok_or_else(|| Error::not_found("the desk"))?;
    assert!(
        loss >= d(limit),
        "the desk's booked loss {loss} is under its limit"
    );
    let (loss_word, limit_word) = (loss.to_string(), d(limit).to_string());
    for reason in &capital {
        let words = tokens(reason);
        assert!(
            words.contains(&loss_word.as_str())
                && words.contains(&limit_word.as_str())
                && words.contains(&DESK)
                && words.contains(&envelope.signature()),
            "the desk's refusal does not name itself, its grant, the loss {loss_word} and the \
             limit {limit_word}: {reason}"
        );
    }
    // And the centre hears the same figure the envelope read.
    assert_eq!(
        reported_loss(&cell, &next, DESK, t(31))?,
        loss,
        "the delta does not carry the desk's realised loss"
    );
    Ok(())
}
