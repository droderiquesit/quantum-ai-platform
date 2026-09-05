//! The typed counterparty agents: deterministic flow inside the synthetic
//! market, and the two things the module must never do.
//!
//! First, present itself as calibrated — every run record carries the
//! statement that no real fill was ever used, and a record saying otherwise
//! does not decode. Second, read the future — an agent's information horizon
//! is declared, and a read one step past it is refused. Each behavioural test
//! asserts its premise before its property: a correlation on a path with no
//! trend, or a leak on a path with no planted move, would pass for nothing.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::testing::is_exactly_zero;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_financial::quality::DataQuality;
use qip_market::bar::{Bar, Interval};
use qip_market::book::Side;
use qip_simulation_engine::agents::{
    AgentKind, AgentRecord, CounterpartyAgent, FlowAction, FlowCalibration, FlowRecord,
    NOT_CALIBRATED_STATEMENT, PathObservation, PathWindow,
};
use qip_simulation_engine::conditions::{ConditionSchedule, ConditionWindow, MarketCondition};
use qip_simulation_engine::execution::SimOrder;
use qip_simulation_engine::market::{
    InstrumentSpec, MarketSimulator, MarketView, SimStrategy, SimulationRun,
};

const VENUE: &str = "XSIM";
const SYMBOL: &str = "obj-aaa";
const LOOKBACK: usize = 3;
/// Half a per cent: the trend segments move three times that over a lookback
/// and the flat segment moves nothing.
const THRESHOLD: f64 = 0.005;

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn day(index: usize) -> Timestamp {
    start().saturating_add(Duration::from_days(index as i64))
}

fn spec() -> InstrumentSpec {
    InstrumentSpec::liquid(SYMBOL, dec!("100"))
}

fn bar(index: usize, close: f64) -> Bar {
    let open = close - 0.1;
    Bar {
        object_id: ObjectId::from_string(SYMBOL.to_string()),
        venue: VENUE.to_string(),
        interval: Interval::Day,
        open_time: day(index),
        open: Decimal::from_f64(open).expect("finite"),
        high: Decimal::from_f64(close.max(open) + 0.2).expect("finite"),
        low: Decimal::from_f64(close.min(open) - 0.2).expect("finite"),
        close: Decimal::from_f64(close).expect("finite"),
        volume: Decimal::from_int(1_000_000),
        vwap: None,
        trade_count: 1_000,
        quality: DataQuality::default(),
    }
}

/// Forty flat days, forty rising, forty falling: a path with a trend segment
/// in each direction and a segment with none.
fn trending_closes() -> Vec<f64> {
    (0..120)
        .map(|index| match index {
            0..=39 => 100.0,
            40..=79 => 100.0 + (index - 39) as f64 * 0.5,
            _ => 120.0 - (index - 79) as f64 * 0.5,
        })
        .collect()
}

/// Twenty flat days, thirty falling, seventy rising: a momentum follower
/// sells through the fall and buys through the rise, so a maker on the other
/// side of it is long first and short afterwards.
fn reversing_closes() -> Vec<f64> {
    (0..120)
        .map(|index| match index {
            0..=19 => 100.0,
            20..=49 => 100.0 - (index - 19) as f64 * 0.5,
            _ => 85.0 + (index - 49) as f64 * 0.5,
        })
        .collect()
}

/// Flat at 100 until day `jump`, then 110 from there on: one planted move.
fn planted_closes(jump: usize) -> Vec<f64> {
    (0..80)
        .map(|index| if index < jump { 100.0 } else { 110.0 })
        .collect()
}

fn bars_of(closes: &[f64]) -> Vec<Bar> {
    closes
        .iter()
        .enumerate()
        .map(|(index, close)| bar(index, *close))
        .collect()
}

fn replay(closes: &[f64], seed: u64) -> Result<MarketSimulator> {
    MarketSimulator::replay(bars_of(closes), vec![spec()], vec![VENUE.to_string()], seed)
}

/// Trailing return at each index, from the closes themselves, so the test's
/// yardstick is not the window under test.
fn trailing_returns(closes: &[f64]) -> Vec<Option<f64>> {
    (0..closes.len())
        .map(|index| {
            index
                .checked_sub(LOOKBACK)
                .map(|earlier| closes[index] / closes[earlier] - 1.0)
        })
        .collect()
}

/// The signed taker flow of one agent at each step, zero where it sent none.
fn signed_flow_by_step(flow: &[FlowRecord], agent: &str, steps: usize) -> Vec<f64> {
    (0..steps)
        .map(|index| {
            flow.iter()
                .filter(|record| record.agent == agent && record.at == day(index + 1))
                .filter_map(FlowRecord::signed_quantity)
                // A statistic from here on: the flow is correlated, not booked.
                .map(Decimal::to_f64)
                .sum()
        })
        .collect()
}

fn pearson(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = ys.iter().sum::<f64>() / n;
    let mut cov = 0.0;
    let mut var_x = 0.0;
    let mut var_y = 0.0;
    for (x, y) in xs.iter().zip(ys) {
        cov += (x - mean_x) * (y - mean_y);
        var_x += (x - mean_x).powi(2);
        var_y += (y - mean_y).powi(2);
    }
    cov / (var_x * var_y).sqrt()
}

/// Correlation between an agent's signed flow and the trailing return, over
/// the steps at which a trailing return exists.
fn flow_correlation(sim: &MarketSimulator, closes: &[f64], agent: &str) -> f64 {
    let trailing = trailing_returns(closes);
    let flow = signed_flow_by_step(sim.counterparty_flow(), agent, closes.len());
    let (xs, ys): (Vec<f64>, Vec<f64>) = trailing
        .iter()
        .zip(&flow)
        .filter_map(|(trailing, flow)| trailing.map(|trailing| (trailing, *flow)))
        .unzip();
    assert!(xs.len() >= 100, "too few steps to correlate: {}", xs.len());
    pearson(&xs, &ys)
}

fn takes(flow: &[FlowRecord], agent: &str) -> usize {
    flow.iter()
        .filter(|record| record.agent == agent)
        .filter(|record| matches!(record.action, FlowAction::Take { .. }))
        .count()
}

/// A strategy that never trades, so a run's record is the agents' alone.
struct Watcher;

impl SimStrategy for Watcher {
    fn name(&self) -> &str {
        "watcher"
    }

    fn on_step(&mut self, _view: &MarketView<'_>) -> Vec<SimOrder> {
        Vec::new()
    }
}

fn five_agents() -> Result<Vec<CounterpartyAgent>> {
    Ok(vec![
        CounterpartyAgent::passive("noise", dec!("200"), 0.5)?,
        CounterpartyAgent::informed("insider", dec!("300"), 5, 0.01)?,
        CounterpartyAgent::momentum("chaser", dec!("400"), LOOKBACK, THRESHOLD)?,
        CounterpartyAgent::competitor("rival", dec!("100"), 2, THRESHOLD, 4)?,
        CounterpartyAgent::maker("quoter", dec!("500"), 3.0, 4.0, dec!("5000"))?,
    ])
}

// ------------------------------------------------------------- the flow rules

#[test]
fn a_momentum_agents_flow_follows_the_trailing_return_and_a_passive_agents_does_not() -> Result<()>
{
    let closes = trending_closes();

    // Premise: the path has a trend segment in each direction, and a segment
    // with none. A correlation on a path that never crossed the threshold
    // would be a correlation with nothing.
    let trailing = trailing_returns(&closes);
    let rising = trailing
        .iter()
        .flatten()
        .filter(|r| **r > THRESHOLD)
        .count();
    let falling = trailing
        .iter()
        .flatten()
        .filter(|r| **r < -THRESHOLD)
        .count();
    let flat = trailing
        .iter()
        .flatten()
        .filter(|r| r.abs() <= THRESHOLD)
        .count();
    assert!(
        rising >= 30,
        "the path should rise for a segment; {rising} step(s) did"
    );
    assert!(
        falling >= 30,
        "the path should fall for a segment; {falling} step(s) did"
    );
    assert!(
        flat >= 30,
        "the path should hold flat for a segment; {flat} step(s) did"
    );

    let sim = replay(&closes, 0xA6E17)?.with_agents(vec![
        CounterpartyAgent::momentum("chaser", dec!("400"), LOOKBACK, THRESHOLD)?,
        CounterpartyAgent::passive("noise", dec!("200"), 0.5)?,
    ])?;
    let flow = sim.counterparty_flow();

    // Premise: both agents actually traded.
    assert!(
        takes(flow, "chaser") >= 60,
        "the momentum agent sent {} take(s)",
        takes(flow, "chaser")
    );
    assert!(
        takes(flow, "noise") >= 30,
        "the passive agent sent {} take(s)",
        takes(flow, "noise")
    );

    // The momentum agent buys after a rise and sells after a fall; the
    // passive agent's side is a coin and carries no information about either.
    let chaser = flow_correlation(&sim, &closes, "chaser");
    let noise = flow_correlation(&sim, &closes, "noise");
    assert!(
        chaser > 0.8,
        "momentum flow should follow the trailing return; correlation {chaser:.3}"
    );
    assert!(
        noise.abs() < 0.3,
        "passive flow should not follow the trailing return; correlation {noise:.3}"
    );

    // And in the trend the direction is exactly the trend's: no momentum
    // buy on a falling step, no sell on a rising one.
    let momentum_flow = signed_flow_by_step(flow, "chaser", closes.len());
    for (index, trailing) in trailing.iter().enumerate() {
        let Some(trailing) = trailing else { continue };
        if *trailing > THRESHOLD {
            assert!(
                momentum_flow[index] > 0.0,
                "step {index}: rose {trailing:.4}, flow {}",
                momentum_flow[index]
            );
        } else if *trailing < -THRESHOLD {
            assert!(
                momentum_flow[index] < 0.0,
                "step {index}: fell {trailing:.4}, flow {}",
                momentum_flow[index]
            );
        } else {
            assert!(
                is_exactly_zero(momentum_flow[index]),
                "step {index}: flat, yet the momentum agent traded ({})",
                momentum_flow[index]
            );
        }
    }
    Ok(())
}

#[test]
fn a_momentum_agents_take_leaves_a_hole_in_the_book_the_strategy_trades_into() -> Result<()> {
    let closes = trending_closes();
    let calm = replay(&closes, 7)?;
    let with_agent = replay(&closes, 7)?.with_agents(vec![CounterpartyAgent::momentum(
        "chaser",
        dec!("400"),
        LOOKBACK,
        THRESHOLD,
    )?])?;

    // Premise: at this step, in the rising segment, the agent bought.
    let at = day(61);
    let bought = with_agent
        .counterparty_flow()
        .iter()
        .find(|record| record.at == at && record.agent == "chaser")
        .map(|record| record.action);
    assert_eq!(
        bought,
        Some(FlowAction::Take {
            side: Side::Buy,
            quantity: dec!("400")
        })
    );

    // A buy consumes asks: the strategy arriving after it finds exactly the
    // agent's clip gone from the offer, and the bid untouched. A flow that
    // was generated and recorded but never reached the book would leave both
    // depths equal to the calm book's — a control that looks like one.
    let calm_book = calm.book_at(SYMBOL, VENUE, at, 0)?;
    let traded_book = with_agent.book_at(SYMBOL, VENUE, at, 0)?;
    assert_eq!(
        traded_book.depth(Side::Sell),
        calm_book.depth(Side::Sell) - dec!("400")
    );
    assert_eq!(traded_book.depth(Side::Buy), calm_book.depth(Side::Buy));
    Ok(())
}

#[test]
fn an_informed_agent_leans_toward_the_planted_move_only_within_its_horizon() -> Result<()> {
    const JUMP: usize = 40;
    const HORIZON: usize = 5;
    let closes = planted_closes(JUMP);

    // Premise: there is one planted move, at the jump, and nothing before it.
    assert!(closes[JUMP] / closes[JUMP - 1] > 1.05);
    assert!(
        closes[..JUMP]
            .windows(2)
            .all(|pair| is_exactly_zero(pair[0] - pair[1]))
    );
    assert!(
        closes[JUMP..]
            .windows(2)
            .all(|pair| is_exactly_zero(pair[0] - pair[1]))
    );

    let sim = replay(&closes, 3)?.with_agents(vec![CounterpartyAgent::informed(
        "insider",
        dec!("300"),
        HORIZON,
        0.01,
    )?])?;
    let flow = signed_flow_by_step(sim.counterparty_flow(), "insider", closes.len());

    // Premise: the agent traded at all.
    assert!(
        flow.iter().any(|f| *f != 0.0),
        "the informed agent never traded"
    );

    // The bar at `JUMP` is readable from `JUMP - HORIZON` and not one step
    // earlier. Flow before that is flow on a bar the agent could not see.
    for (index, signed) in flow.iter().enumerate() {
        let inside = (JUMP - HORIZON..JUMP).contains(&index);
        if inside {
            assert!(
                *signed > 0.0,
                "step {index}: within the horizon of the move, yet no buy ({signed})"
            );
        } else {
            assert!(
                is_exactly_zero(*signed),
                "step {index}: outside the horizon, yet the agent traded ({signed})"
            );
        }
    }
    Ok(())
}

#[test]
fn an_agent_reading_a_bar_before_its_known_at_is_refused() -> Result<()> {
    let observations: Vec<PathObservation> = (0..10)
        .map(|index| PathObservation::new(day(index), day(index), dec!("100")))
        .collect::<Result<_>>()?;

    // Premise: within the licence the window answers, at the horizon
    // included, and backward reads are unrestricted.
    let window = PathWindow::new(&observations, 2, 3)?;
    assert_eq!(window.price_ahead(3)?, Some(100.0));
    assert_eq!(window.planted_return(3)?, Some(0.0));
    assert_eq!(window.trailing_return(2), Some(0.0));

    // One step past the horizon is a bar knowable after the licence ends.
    let refused = window
        .price_ahead(4)
        .expect_err("a read past the horizon must be refused");
    assert!(
        refused.message().contains("before its known_at"),
        "unexpected refusal: {}",
        refused.message()
    );
    assert!(window.planted_return(4).is_err());

    // The four uninformed behaviours hold a horizon of zero, so for them the
    // very next bar is the refused one.
    let blind = PathWindow::new(&observations, 2, 0)?;
    assert_eq!(blind.price_ahead(0)?, Some(100.0));
    assert!(blind.price_ahead(1).is_err());

    // A bar stamped knowable later than its instant is refused by its
    // knowable instant, not by its position: index 5 sits inside a horizon
    // of three from index 2, and is still not readable.
    let mut late = observations.clone();
    late[5].known_at = day(9);
    let window = PathWindow::new(&late, 2, 3)?;
    assert!(window.price_ahead(3).is_err());
    assert_eq!(window.price_ahead(2)?, Some(100.0));
    Ok(())
}

#[test]
fn an_agent_handed_a_window_wider_than_its_declared_horizon_is_refused() -> Result<()> {
    use qip_core::rng::Xoshiro256;
    use qip_simulation_engine::agents::{AgentState, StepContext};
    use qip_simulation_engine::conditions::Regime;

    let observations: Vec<PathObservation> = (0..10)
        .map(|index| PathObservation::new(day(index), day(index), dec!("100")))
        .collect::<Result<_>>()?;
    let regime = Regime::calm();
    let step = StepContext {
        at: day(2),
        object_id: SYMBOL,
        venue: VENUE,
        price: dec!("100"),
        calm_half_spread_bps: 1.0,
        regime: &regime,
    };
    let agent = CounterpartyAgent::momentum("chaser", dec!("1"), 1, 0.0)?;
    let mut state = AgentState::default();
    let mut rng = Xoshiro256::seeded(1);

    // Premise: with its own horizon the agent acts.
    let own = PathWindow::new(&observations, 2, 0)?;
    assert!(agent.act(&own, &step, &mut state, &mut rng).is_ok());

    // With a wider one it is refused before it reads anything.
    let wider = PathWindow::new(&observations, 2, 3)?;
    let refused = agent
        .act(&wider, &step, &mut state, &mut rng)
        .expect_err("a window wider than the declaration must be refused");
    assert!(refused.message().contains("must match the declaration"));
    Ok(())
}

// --------------------------------------------------------------- the record

#[test]
fn the_run_record_names_its_agents_and_states_that_the_flow_is_not_calibrated() -> Result<()> {
    let closes = trending_closes();
    let sim = replay(&closes, 11)?.with_agents(five_agents()?)?;
    let run = sim.run(&mut Watcher)?;

    // Premise: the record has flow in it from more than one agent.
    assert!(run.counterparty_flow.len() > 100);
    let kinds: Vec<AgentKind> = run.agents.iter().map(|agent| agent.kind).collect();
    assert_eq!(
        kinds,
        vec![
            AgentKind::Momentum,
            AgentKind::Informed,
            AgentKind::Passive,
            AgentKind::Maker,
            AgentKind::Competitor,
        ],
        "agents are recorded in name order, not declaration order"
    );
    let names: Vec<&str> = run.agents.iter().map(|agent| agent.name.as_str()).collect();
    assert_eq!(names, vec!["chaser", "insider", "noise", "quoter", "rival"]);
    assert!(run.agents.iter().all(|agent| !agent.rule.is_empty()));

    // The statement, on the record, in the summary, and in the encoding.
    assert_eq!(run.flow_calibration, FlowCalibration::NotCalibrated);
    assert_eq!(run.flow_calibration.statement(), NOT_CALIBRATED_STATEMENT);
    assert!(NOT_CALIBRATED_STATEMENT.contains("not calibrated against real fills"));
    assert!(run.summarise().contains(NOT_CALIBRATED_STATEMENT));
    let encoded = serde_json::to_value(&run).expect("serialisable");
    assert_eq!(
        encoded.get("flow_calibration").and_then(|v| v.as_str()),
        Some(NOT_CALIBRATED_STATEMENT)
    );

    // A record that claims anything else about calibration does not decode.
    let mut forged = encoded.clone();
    forged["flow_calibration"] = serde_json::Value::String("calibrated against real fills".into());
    assert!(serde_json::from_value::<SimulationRun>(forged).is_err());
    let decoded: SimulationRun = serde_json::from_value(encoded).expect("a whole record decodes");
    assert_eq!(decoded, run);

    // A run with no agents carries the statement too.
    let alone = replay(&closes, 11)?.run(&mut Watcher)?;
    assert!(alone.agents.is_empty());
    assert_eq!(alone.flow_calibration, FlowCalibration::NotCalibrated);
    Ok(())
}

#[test]
fn the_same_seed_and_agents_produce_the_same_flow_regardless_of_declaration_order() -> Result<()> {
    let closes = trending_closes();
    let forward = replay(&closes, 0xF10)?.with_agents(five_agents()?)?;
    let mut reversed_agents = five_agents()?;
    reversed_agents.reverse();
    let reversed = replay(&closes, 0xF10)?.with_agents(reversed_agents)?;

    // Premise: there is flow, and the passive agent drew from its stream.
    assert!(takes(forward.counterparty_flow(), "noise") > 20);

    assert_eq!(forward.counterparty_flow(), reversed.counterparty_flow());
    let first = forward.run(&mut Watcher)?;
    let second = reversed.run(&mut Watcher)?;
    assert_eq!(first.digest(), second.digest());

    // And a different seed is a different flow.
    let other = replay(&closes, 0xF11)?.with_agents(five_agents()?)?;
    assert_ne!(forward.counterparty_flow(), other.counterparty_flow());
    Ok(())
}

#[test]
fn two_agents_with_one_name_are_refused() -> Result<()> {
    let closes = trending_closes();
    let refused = replay(&closes, 1)?.with_agents(vec![
        CounterpartyAgent::passive("same", dec!("1"), 0.5)?,
        CounterpartyAgent::momentum("same", dec!("1"), 1, 0.0)?,
    ]);
    assert!(refused.is_err());
    Ok(())
}

// ----------------------------------------------------------- under stress

#[test]
fn every_agent_withdraws_under_an_injected_condition() -> Result<()> {
    let closes = trending_closes();

    // Premise: on the calm path every one of the five acts.
    let calm = replay(&closes, 5)?.with_agents(five_agents()?)?;
    for name in ["noise", "insider", "chaser", "rival", "quoter"] {
        assert!(
            calm.counterparty_flow()
                .iter()
                .any(|record| record.agent == name),
            "{name} sent nothing on the calm path"
        );
    }

    // Under a condition in force for the whole run, none of them does — the
    // agents cannot supply what the condition removed.
    let stressed = replay(&closes, 5)?
        .with_agents(five_agents()?)?
        .with_conditions(ConditionSchedule::new().with(ConditionWindow::always(
            MarketCondition::SpreadRegime { multiplier: 3.0 },
        )))?;
    assert!(
        stressed.counterparty_flow().is_empty(),
        "{} record(s) under stress; first: {:?}",
        stressed.counterparty_flow().len(),
        stressed.counterparty_flow().first()
    );

    // Attaching the schedule first and the agents second gives the same
    // answer: the flow is regenerated against the schedule as it stands.
    let reordered = replay(&closes, 5)?
        .with_conditions(ConditionSchedule::new().with(ConditionWindow::always(
            MarketCondition::SpreadRegime { multiplier: 3.0 },
        )))?
        .with_agents(five_agents()?)?;
    assert!(reordered.counterparty_flow().is_empty());
    Ok(())
}

#[test]
fn a_maker_never_quotes_inside_the_calm_touch_and_skews_away_from_its_inventory() -> Result<()> {
    let closes = reversing_closes();
    // An inventory limit the chaser's flow never reaches, so every step has
    // a quote to check; the withdrawal past the limit is a separate rule.
    let sim = replay(&closes, 9)?.with_agents(vec![
        CounterpartyAgent::momentum("chaser", dec!("400"), LOOKBACK, THRESHOLD)?,
        CounterpartyAgent::maker("quoter", dec!("500"), 3.0, 4.0, dec!("40000"))?,
    ])?;

    // The maker's inventory as it stands when it quotes at each step: the
    // negative of the chaser's flow through the *previous* step, since the
    // maker absorbs a step's flow after everyone has acted in it. Built from
    // the chaser's records alone, so the yardstick is not the maker's state.
    let mut inventory_at: std::collections::BTreeMap<Timestamp, Decimal> =
        std::collections::BTreeMap::new();
    let mut cumulative = Decimal::ZERO;
    for index in 0..closes.len() {
        let at = day(index + 1);
        inventory_at.insert(at, -cumulative);
        cumulative += sim
            .counterparty_flow()
            .iter()
            .filter(|record| record.agent == "chaser" && record.at == at)
            .filter_map(FlowRecord::signed_quantity)
            .sum::<Decimal>();
    }
    // Premise: the chaser sold through the fall and bought through the rise,
    // so the maker was long for a stretch and short for another.
    let long_steps = inventory_at.values().filter(|i| i.is_positive()).count();
    let short_steps = inventory_at.values().filter(|i| i.is_negative()).count();
    assert!(
        long_steps > 10,
        "the maker was long on {long_steps} step(s)"
    );
    assert!(
        short_steps > 10,
        "the maker was short on {short_steps} step(s)"
    );
    let quotes: Vec<&FlowRecord> = sim
        .counterparty_flow()
        .iter()
        .filter(|record| record.agent == "quoter")
        .collect();
    assert!(
        quotes.len() > 50,
        "the maker quoted {} time(s)",
        quotes.len()
    );

    let half_spread_bps = spec().half_spread_bps;
    let mut widened_bid = 0usize;
    let mut widened_ask = 0usize;
    for record in &quotes {
        let FlowAction::Quote { bid, ask, .. } = record.action else {
            panic!("a maker only quotes; got {:?}", record.action);
        };
        let calm = sim
            .reference_price(SYMBOL, record.at)
            .expect("a reference at every step");
        let calm_bid = calm - calm.apply_bps(half_spread_bps);
        let calm_ask = calm + calm.apply_bps(half_spread_bps);
        assert!(
            bid <= calm_bid,
            "at {}: bid {bid} inside the calm bid {calm_bid}",
            record.at
        );
        assert!(
            ask >= calm_ask,
            "at {}: ask {ask} inside the calm ask {calm_ask}",
            record.at
        );
        let symmetric_half = calm.apply_bps(3.0);
        let bid_backed_off = bid < calm - symmetric_half;
        let ask_backed_off = ask > calm + symmetric_half;
        // The skew is one-sided and on the side that would add to the
        // inventory: long, the bid backs off and the ask does not; short,
        // the reverse; flat, neither. A skew on the wrong side would still
        // widen *a* side on every step, which is why the sign is checked
        // per quote and not counted.
        let inventory = inventory_at
            .get(&record.at)
            .copied()
            .expect("an inventory at every step");
        assert_eq!(
            (bid_backed_off, ask_backed_off),
            (inventory.is_positive(), inventory.is_negative()),
            "at {}: inventory {inventory}, bid {bid} / ask {ask} about {calm}",
            record.at
        );
        if bid_backed_off {
            widened_bid += 1;
        }
        if ask_backed_off {
            widened_ask += 1;
        }
    }
    // And both skews were exercised, not just permitted.
    assert!(
        widened_bid > 10,
        "the bid never backed off while the maker was long ({widened_bid})"
    );
    assert!(
        widened_ask > 10,
        "the ask never backed off while the maker was short ({widened_ask})"
    );
    Ok(())
}

#[test]
fn an_agent_with_a_parameter_that_would_generate_nothing_is_refused() {
    assert!(CounterpartyAgent::passive("p", dec!("0"), 0.5).is_err());
    assert!(CounterpartyAgent::passive("p", dec!("1"), 1.5).is_err());
    assert!(CounterpartyAgent::passive("", dec!("1"), 0.5).is_err());
    assert!(CounterpartyAgent::informed("i", dec!("1"), 0, 0.01).is_err());
    assert!(CounterpartyAgent::informed("i", dec!("1"), 1, f64::NAN).is_err());
    assert!(CounterpartyAgent::momentum("m", dec!("1"), 0, 0.01).is_err());
    assert!(CounterpartyAgent::momentum("m", dec!("-1"), 1, 0.01).is_err());
    assert!(CounterpartyAgent::competitor("c", dec!("1"), 1, 0.01, 0).is_err());
    assert!(CounterpartyAgent::maker("k", dec!("1"), 0.0, 1.0, dec!("1")).is_err());
    assert!(CounterpartyAgent::maker("k", dec!("1"), 1.0, -1.0, dec!("1")).is_err());
    assert!(CounterpartyAgent::maker("k", dec!("1"), 1.0, 1.0, dec!("0")).is_err());
}

#[test]
fn two_runs_whose_agent_records_differ_only_in_where_one_string_ends_have_different_digests()
-> Result<()> {
    // The failure this prevents was found by review: the digest concatenated
    // each agent's name, kind and rule with no delimiter, so a run whose
    // first agent's rule ended in "y" beside a second agent named "b" hashed
    // to the same bytes as one whose rule ended in "" beside an agent named
    // "yb". A digest two different runs share is a record that cannot tell
    // them apart, which is the one thing a digest is for.
    let closes = trending_closes();
    let base = replay(&closes, 11)?.run(&mut Watcher)?;
    // Premise: the two agent lists really are different records.
    let mut left = base.clone();
    left.agents = vec![
        AgentRecord {
            name: "a".into(),
            kind: AgentKind::Passive,
            rule: "xy".into(),
        },
        AgentRecord {
            name: "b".into(),
            kind: AgentKind::Passive,
            rule: String::new(),
        },
    ];
    let mut right = base.clone();
    right.agents = vec![
        AgentRecord {
            name: "a".into(),
            kind: AgentKind::Passive,
            rule: "x".into(),
        },
        AgentRecord {
            name: "yb".into(),
            kind: AgentKind::Passive,
            rule: String::new(),
        },
    ];
    assert_ne!(left.agents, right.agents);
    assert_ne!(
        left.digest(),
        right.digest(),
        "two different agent records produced one digest"
    );
    Ok(())
}
