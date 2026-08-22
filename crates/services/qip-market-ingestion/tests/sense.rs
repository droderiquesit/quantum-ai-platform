//! The SENSE stage: synthetic market realism, validation, replay, publication.

use qip_core::testing::approx_eq;
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp, dec};
use qip_events::{EventBus, EventLog, Topic};
use qip_financial::intelligence::DataQualityFailure;
use qip_financial::quality::LicensingClass;
use qip_market::quote::{Quote, Trade, TradeCondition};
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord};
use qip_market_ingestion::synthetic::price::{FactorModel, Regime};
use qip_market_ingestion::synthetic::{EnvironmentConfig, SyntheticEnvironment};
use qip_market_ingestion::{IngestionService, ReplayAdapter, ScriptedEvent, ScriptedEventKind};
use qip_observability::Telemetry;
use std::cell::RefCell;
use std::rc::Rc;

fn start() -> Timestamp {
    // Mid-session so the intraday profile is active.
    Timestamp::parse_rfc3339("2026-08-24T15:00:00Z").unwrap()
}

fn environment(seed: u64) -> SyntheticEnvironment {
    SyntheticEnvironment::demo(
        start(),
        EnvironmentConfig {
            seed,
            ..EnvironmentConfig::default()
        },
    )
}

/// Collect the close prices of one symbol over a run.
fn price_path(environment: &mut SyntheticEnvironment, symbol: &str, steps: usize) -> Vec<f64> {
    let mut path = Vec::with_capacity(steps);
    for _ in 0..steps {
        environment.step();
        if let Some(price) = environment.price_of(symbol) {
            path.push(price);
        }
    }
    path
}

// --- determinism ------------------------------------------------------------

#[test]
fn the_same_seed_reproduces_the_same_market() {
    let a = price_path(&mut environment(11), "NWSC", 200);
    let b = price_path(&mut environment(11), "NWSC", 200);
    assert_eq!(a, b, "a seeded run must be byte-identical");

    let c = price_path(&mut environment(12), "NWSC", 200);
    assert_ne!(a, c, "a different seed must produce a different market");
}

#[test]
fn records_are_reproducible_across_runs() {
    let render = |seed: u64| {
        let mut environment = environment(seed);
        let records = environment.run_until(start().saturating_add(Duration::from_hours(1)));
        records
            .iter()
            .map(|r| format!("{}:{}", r.topic().name(), r.subject()))
            .collect::<Vec<_>>()
    };
    assert_eq!(render(5), render(5));
}

// --- price realism ----------------------------------------------------------

#[test]
fn prices_stay_positive_and_finite_over_a_long_run() {
    let mut environment = environment(3);
    environment.run_until(start().saturating_add(Duration::from_days(30)));
    for instrument in environment.instruments() {
        let price = instrument.state.price;
        assert!(
            price.is_finite() && price > 0.0,
            "{} price {price}",
            instrument.symbol
        );
        assert!(
            price < 1e9,
            "{} price ran away to {price}",
            instrument.symbol
        );
        assert!(instrument.state.variance.is_finite() && instrument.state.variance >= 0.0);
    }
}

#[test]
fn volatility_clusters_rather_than_being_constant() {
    // The defining feature of real returns: today's absolute move predicts
    // tomorrow's. A constant-volatility simulator would fail this, and a risk
    // model tested only against one would be useless.
    let mut environment = environment(7);
    let path = price_path(&mut environment, "NWSC", 3_000);
    let returns = qip_numerics::stats::log_returns(&path);
    let absolute: Vec<f64> = returns.iter().map(|r| r.abs()).collect();

    let clustering = qip_numerics::stats::autocorrelation(&absolute, 1);
    assert!(
        clustering > 0.03,
        "absolute returns should be positively autocorrelated, got {clustering}"
    );
    // Returns themselves should be close to unpredictable.
    let drift = qip_numerics::stats::autocorrelation(&returns, 1).abs();
    assert!(
        drift < 0.25,
        "raw returns should be near-unpredictable, got {drift}"
    );
}

#[test]
fn returns_have_fatter_tails_than_a_normal_distribution() {
    let mut environment = environment(21);
    let path = price_path(&mut environment, "MRDN", 4_000);
    let returns = qip_numerics::stats::log_returns(&path);
    let kurtosis = qip_numerics::stats::excess_kurtosis(&returns);
    assert!(
        kurtosis > 0.5,
        "jumps and stochastic volatility should produce excess kurtosis, got {kurtosis}"
    );
}

#[test]
fn instruments_co_move_through_the_common_factors() {
    let mut environment = environment(31);
    let mut nwsc = Vec::new();
    let mut vntg = Vec::new();
    for _ in 0..2_000 {
        environment.step();
        nwsc.push(environment.price_of("NWSC").unwrap());
        vntg.push(environment.price_of("VNTG").unwrap());
    }
    let correlation = qip_numerics::stats::correlation(
        &qip_numerics::stats::log_returns(&nwsc),
        &qip_numerics::stats::log_returns(&vntg),
    );
    assert!(
        correlation > 0.15,
        "two equities loading on the market factor should co-move, got {correlation}"
    );
}

#[test]
fn a_bond_and_an_equity_are_far_less_correlated_than_two_equities() {
    let mut environment = environment(41);
    let (mut equity_a, mut equity_b, mut bond) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..2_500 {
        environment.step();
        equity_a.push(environment.price_of("NWSC").unwrap());
        equity_b.push(environment.price_of("VNTG").unwrap());
        bond.push(environment.price_of("UST10Y").unwrap());
    }
    let equity_pair = qip_numerics::stats::correlation(
        &qip_numerics::stats::log_returns(&equity_a),
        &qip_numerics::stats::log_returns(&equity_b),
    );
    let cross_asset = qip_numerics::stats::correlation(
        &qip_numerics::stats::log_returns(&equity_a),
        &qip_numerics::stats::log_returns(&bond),
    );
    assert!(
        equity_pair > cross_asset,
        "equity/equity {equity_pair} should exceed equity/bond {cross_asset}"
    );
}

#[test]
fn the_bond_reverts_toward_par_while_the_equity_does_not() {
    let mut environment = environment(53);
    environment.run_until(start().saturating_add(Duration::from_days(90)));
    let bond = environment.price_of("UST10Y").unwrap();
    assert!(
        (70.0..130.0).contains(&bond),
        "a mean-reverting bond price should stay near par, got {bond}"
    );
}

// --- regimes ----------------------------------------------------------------

#[test]
fn regime_multipliers_are_ordered_by_severity() {
    let regimes = [
        Regime::Calm,
        Regime::Normal,
        Regime::Stressed,
        Regime::Crisis,
    ];
    for pair in regimes.windows(2) {
        assert!(pair[1].volatility_multiplier() > pair[0].volatility_multiplier());
        assert!(pair[1].correlation_multiplier() >= pair[0].correlation_multiplier());
        assert!(pair[1].liquidity_penalty() > pair[0].liquidity_penalty());
        assert!(pair[1].jump_multiplier() > pair[0].jump_multiplier());
    }
}

#[test]
fn regime_transitions_are_a_proper_markov_chain() {
    for regime in [
        Regime::Calm,
        Regime::Normal,
        Regime::Stressed,
        Regime::Crisis,
    ] {
        let total: f64 = regime
            .transition_probabilities()
            .iter()
            .map(|(_, p)| p)
            .sum();
        assert!(
            approx_eq(total, 1.0, 1e-9),
            "{regime:?} probabilities sum to {total}"
        );
        // Every regime is persistent: it is more likely to stay than to move
        // to any single other state.
        let self_probability = regime
            .transition_probabilities()
            .iter()
            .find(|(r, _)| *r == regime)
            .map(|(_, p)| *p)
            .unwrap();
        assert!(
            self_probability > 0.5,
            "{regime:?} should persist, got {self_probability}"
        );
    }
}

#[test]
fn a_scripted_crisis_widens_spreads_and_raises_volatility() {
    let measure = |regime: Regime| {
        let mut environment = environment(61);
        environment.schedule(ScriptedEvent::new(
            start().saturating_add(Duration::from_mins(1)),
            ScriptedEventKind::RegimeShift { regime },
        ));
        let mut spreads = Vec::new();
        let mut prices = Vec::new();
        for _ in 0..600 {
            for record in environment.step() {
                if let SensedRecord::Quote(quote) = record
                    && let Some(bps) = quote.spread_bps()
                    && quote.object_id.as_str().ends_with("NWSC")
                {
                    spreads.push(bps);
                }
            }
            prices.push(environment.price_of("NWSC").unwrap());
        }
        let volatility = qip_numerics::stats::stddev(&qip_numerics::stats::log_returns(&prices));
        (qip_numerics::stats::mean(&spreads), volatility)
    };

    let (calm_spread, calm_volatility) = measure(Regime::Calm);
    let (crisis_spread, crisis_volatility) = measure(Regime::Crisis);

    assert!(
        crisis_spread > calm_spread * 2.0,
        "crisis spread {crisis_spread} should far exceed calm {calm_spread}"
    );
    assert!(
        crisis_volatility > calm_volatility * 1.5,
        "crisis volatility {crisis_volatility} should exceed calm {calm_volatility}"
    );
}

#[test]
fn a_correlation_breakdown_can_be_injected() {
    let measure = |coupling: f64| {
        let mut environment = environment(71);
        environment.schedule(ScriptedEvent::new(
            start().saturating_add(Duration::from_mins(1)),
            ScriptedEventKind::CorrelationChange { coupling },
        ));
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for _ in 0..2_000 {
            environment.step();
            a.push(environment.price_of("NWSC").unwrap());
            b.push(environment.price_of("VNTG").unwrap());
        }
        qip_numerics::stats::correlation(
            &qip_numerics::stats::log_returns(&a),
            &qip_numerics::stats::log_returns(&b),
        )
    };
    let coupled = measure(1.0);
    let decoupled = measure(0.0);
    assert!(
        coupled > decoupled + 0.1,
        "coupling 1.0 gave {coupled}, coupling 0.0 gave {decoupled}"
    );
}

// --- scripted events --------------------------------------------------------

#[test]
fn an_earnings_surprise_produces_a_fundamental_a_story_and_a_price_move() {
    let mut environment = environment(83);
    let fire_at = start().saturating_add(Duration::from_mins(5));
    environment.schedule(ScriptedEvent::new(
        fire_at,
        ScriptedEventKind::EarningsSurprise {
            entity_id: "ent-northwind".into(),
            metric: "revenue".into(),
            surprise: 0.12,
        },
    ));

    let before = environment.price_of("NWSC").unwrap();
    let records = environment.run_until(fire_at.saturating_add(Duration::from_mins(2)));
    let after = environment.price_of("NWSC").unwrap();

    let fundamentals: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            SensedRecord::Fundamental(f) => Some(f),
            _ => None,
        })
        .collect();
    assert_eq!(fundamentals.len(), 1, "one fundamental release");
    let fundamental = fundamentals[0];
    assert_eq!(fundamental.entity_id, "ent-northwind");
    let surprise = fundamental.surprise().expect("a consensus was published");
    assert!(approx_eq(surprise, 0.12, 0.005), "surprise {surprise}");
    assert!(fundamental.is_significant_surprise(0.05));

    let stories: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            SensedRecord::News(n) if n.topics.contains(&"earnings".to_string()) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(stories.len(), 1, "one earnings story");
    assert!(
        stories[0].sentiment.polarity > 0.3,
        "a beat should read positive"
    );
    assert!(!stories[0].primary_entities().is_empty());

    assert!(
        after > before * 1.05,
        "price {before} -> {after} should jump on a 12% beat"
    );
    assert_eq!(environment.fired_events().len(), 1);
}

#[test]
fn a_supply_disruption_names_the_downstream_customer() {
    let mut environment = environment(97);
    let fire_at = start().saturating_add(Duration::from_mins(5));
    environment.schedule(ScriptedEvent::new(
        fire_at,
        ScriptedEventKind::SupplyDisruption {
            entity_id: "ent-northwind".into(),
            severity: 0.25,
        },
    ));
    let before = environment.price_of("NWSC").unwrap();
    let records = environment.run_until(fire_at.saturating_add(Duration::from_mins(2)));
    let after = environment.price_of("NWSC").unwrap();

    let story = records
        .iter()
        .find_map(|r| match r {
            SensedRecord::News(n) if n.topics.contains(&"supply_chain".to_string()) => Some(n),
            _ => None,
        })
        .expect("a supply-chain story");
    assert!(story.sentiment.polarity < 0.0);
    assert!(
        story.entities.len() >= 2,
        "the story must name a customer so the graph has an edge to follow"
    );
    assert!(after < before, "a disruption should hurt the price");
}

#[test]
fn a_macro_surprise_publishes_an_observation_and_a_story() {
    let mut environment = environment(101);
    let fire_at = start().saturating_add(Duration::from_mins(5));
    environment.schedule(ScriptedEvent::new(
        fire_at,
        ScriptedEventKind::MacroSurprise {
            series_id: "US.CPI.YOY".into(),
            region: "US".into(),
            level: 3.4,
            surprise: 0.4,
        },
    ));
    let records = environment.run_until(fire_at.saturating_add(Duration::from_mins(2)));

    let observation = records
        .iter()
        .find_map(|r| match r {
            SensedRecord::Macro(m) => Some(m),
            _ => None,
        })
        .expect("a macro observation");
    assert_eq!(observation.series_id, "US.CPI.YOY");
    assert!(approx_eq(observation.surprise().unwrap(), 0.4, 1e-9));
    assert!(
        records
            .iter()
            .any(|r| matches!(r, SensedRecord::News(n) if n.topics.contains(&"macro".to_string())))
    );
}

#[test]
fn a_liquidity_withdrawal_thins_the_book() {
    let depth_after = |multiplier: f64| {
        let mut environment = environment(113);
        environment.schedule(ScriptedEvent::new(
            start().saturating_add(Duration::from_mins(1)),
            ScriptedEventKind::LiquidityWithdrawal {
                symbol: "NWSC".into(),
                multiplier,
            },
        ));
        let mut depths = Vec::new();
        for _ in 0..20 {
            for record in environment.step() {
                if let SensedRecord::Book(book) = record
                    && book.object_id.as_str().ends_with("NWSC")
                {
                    depths.push(book.total_depth().to_f64());
                }
            }
        }
        qip_numerics::stats::mean(&depths)
    };
    let normal = depth_after(1.0);
    let withdrawn = depth_after(6.0);
    assert!(
        withdrawn < normal * 0.6,
        "normal {normal}, withdrawn {withdrawn}"
    );
}

// --- book and trade realism -------------------------------------------------

#[test]
fn generated_books_are_always_well_formed() {
    let mut environment = environment(127);
    let records = environment.run_until(start().saturating_add(Duration::from_hours(3)));
    let mut checked = 0;
    for record in &records {
        if let SensedRecord::Book(book) = record {
            book.validate()
                .unwrap_or_else(|e| panic!("malformed book: {e}"));
            assert!(book.best_bid().is_some() && book.best_ask().is_some());
            checked += 1;
        }
    }
    assert!(
        checked > 10,
        "expected several book snapshots, saw {checked}"
    );
}

#[test]
fn generated_quotes_and_bars_pass_validation() {
    let mut environment = environment(131);
    let records = environment.run_until(start().saturating_add(Duration::from_hours(4)));
    let mut quotes = 0;
    let mut bars = 0;
    for record in &records {
        assert!(
            record.validate().is_empty(),
            "invalid record: {:?}",
            record.validate()
        );
        match record {
            SensedRecord::Quote(_) => quotes += 1,
            SensedRecord::Bar(_) => bars += 1,
            _ => {}
        }
    }
    assert!(quotes > 100, "expected a stream of quotes, saw {quotes}");
    assert!(bars > 10, "expected bars, saw {bars}");
}

#[test]
fn trading_is_heaviest_at_the_open_and_close() {
    // The U-shaped intraday profile. An execution algorithm scheduled against
    // a flat profile trades too much at the wrong time.
    let open = Timestamp::parse_rfc3339("2026-08-24T14:30:00Z").unwrap();
    let mut environment = SyntheticEnvironment::demo(
        open,
        EnvironmentConfig {
            seed: 137,
            ..EnvironmentConfig::default()
        },
    );

    let mut buckets = [0.0f64; 3];
    for (index, bucket) in buckets.iter_mut().enumerate() {
        let until = open.saturating_add(Duration::from_mins(130 * (index as i64 + 1)));
        for record in environment.run_until(until) {
            if let SensedRecord::Trade(trade) = record {
                *bucket += trade.size.to_f64();
            }
        }
    }
    assert!(
        buckets[0] > buckets[1] && buckets[2] > buckets[1],
        "expected a U shape, got open {} midday {} close {}",
        buckets[0],
        buckets[1],
        buckets[2]
    );
}

#[test]
fn trade_sizes_are_heavy_tailed() {
    let mut environment = environment(139);
    let mut sizes = Vec::new();
    for record in environment.run_until(start().saturating_add(Duration::from_hours(2))) {
        if let SensedRecord::Trade(trade) = record {
            sizes.push(trade.size.to_f64());
        }
    }
    assert!(
        sizes.len() > 200,
        "expected many trades, saw {}",
        sizes.len()
    );
    let median = qip_numerics::stats::median(&sizes);
    let p99 = qip_numerics::stats::quantile(&sizes, 0.99);
    assert!(
        p99 > median * 5.0,
        "median {median}, p99 {p99} — sizes should be heavy tailed"
    );
}

#[test]
fn buying_pressure_shows_up_in_the_order_flow() {
    let mut environment = environment(149);
    let fire_at = start().saturating_add(Duration::from_mins(2));
    environment.schedule(ScriptedEvent::new(
        fire_at,
        ScriptedEventKind::PriceShock {
            symbol: "NWSC".into(),
            log_return: 0.04,
        },
    ));
    let mut buys = 0;
    let mut sells = 0;
    for record in environment.run_until(fire_at.saturating_add(Duration::from_mins(3))) {
        if let SensedRecord::Trade(trade) = record
            && trade.object_id.as_str().ends_with("NWSC")
            && let Some(side) = trade.aggressor
        {
            match side {
                qip_market::book::Side::Buy => buys += 1,
                qip_market::book::Side::Sell => sells += 1,
            }
        }
    }
    assert!(
        buys > sells,
        "a price jump should be bought: {buys} buys vs {sells} sells"
    );
}

// --- factor model -----------------------------------------------------------

#[test]
fn the_factor_model_produces_the_same_shock_for_every_instrument_in_a_step() {
    let mut factors = FactorModel::standard();
    let mut rng = qip_core::rng::Xoshiro256::seeded(1);
    factors.step(1.0 / 252.0, Regime::Normal, 1.0, &mut rng);

    // Two instruments with identical loadings must see an identical
    // contribution; that is what correlation means here.
    let loadings = vec![1.0, 0.0, 0.0];
    let a = factors.contribution(&loadings, Regime::Normal);
    let b = factors.contribution(&loadings, Regime::Normal);
    assert!(approx_eq(a, b, 1e-15));

    // A crisis amplifies the shared component.
    let crisis = factors.contribution(&loadings, Regime::Crisis);
    assert!(
        crisis.abs() > a.abs(),
        "crisis {crisis} should exceed normal {a}"
    );
}

// --- validation gate --------------------------------------------------------

#[test]
fn a_crossed_quote_is_rejected_and_reported_rather_than_published() {
    let now = start();
    let (context, _clock) = Context::deterministic(now, 1);
    let log = Rc::new(RefCell::new(EventLog::in_memory()));
    let mut bus = EventBus::new().with_log(log.clone());
    let mut service = IngestionService::new(Telemetry::silent());

    let crossed = SensedRecord::Quote(Quote {
        object_id: ObjectId::from_string("OBJ0000000000000000000001"),
        venue: "XNYS".into(),
        at: now,
        bid: dec!("100.20"),
        ask: dec!("100.10"),
        bid_size: Decimal::from_int(100),
        ask_size: Decimal::from_int(100),
        quality: Default::default(),
    });
    let good = SensedRecord::Trade(Trade {
        object_id: ObjectId::from_string("OBJ0000000000000000000001"),
        venue: "XNYS".into(),
        at: now,
        price: dec!("100.15"),
        size: Decimal::from_int(50),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: Some("t1".into()),
        quality: Default::default(),
    });

    let published = service
        .publish_records(&context, &mut bus, "test", &[crossed, good])
        .unwrap();
    bus.drain(&context).unwrap();

    assert_eq!(published, 1, "only the valid record should publish");
    assert_eq!(service.stats().rejected, 1);
    assert!(approx_eq(service.stats().acceptance_rate(), 0.5, 1e-12));

    let log = log.borrow();
    assert_eq!(
        log.by_topic(Topic::MarketQuote).len(),
        0,
        "the crossed quote must not publish"
    );
    assert_eq!(log.by_topic(Topic::MarketTrade).len(), 1);

    let failures = log.by_topic(Topic::DataQualityFailed);
    assert_eq!(
        failures.len(),
        1,
        "the rejection must be visible as an event"
    );
    let failure = failures[0].decode::<DataQualityFailure>().unwrap();
    assert!(failure.body.rejected);
    assert_eq!(failure.body.intended_topic, "market.quote");
    assert!(failure.body.issues[0].contains("crossed"));
}

#[test]
fn every_published_record_starts_its_own_lineage_chain() {
    let now = start();
    let (context, _clock) = Context::deterministic(now, 1);
    let log = Rc::new(RefCell::new(EventLog::in_memory()));
    let mut bus = EventBus::new().with_log(log.clone());
    let mut service = IngestionService::new(Telemetry::silent());

    let mut environment = environment(151);
    let records = environment.run_until(now.saturating_add(Duration::from_mins(10)));
    service
        .publish_records(&context, &mut bus, "synthetic", &records)
        .unwrap();
    bus.drain(&context).unwrap();

    let log = log.borrow();
    assert!(
        log.len() > 20,
        "expected a stream of events, got {}",
        log.len()
    );
    for event in log.events() {
        assert!(
            event.lineage.causation_id.is_none(),
            "an observation is a root, not caused by anything"
        );
        assert_eq!(event.lineage.producer, "market-ingestion");
    }
    // Every observation gets its own correlation id, so downstream work on one
    // can be traced without dragging in the rest.
    assert_eq!(log.stats().correlations as usize, log.len());
}

// --- adapters and replay ----------------------------------------------------

#[test]
fn the_synthetic_adapter_declares_itself_unfit_for_production() {
    let environment = environment(1);
    let descriptor = environment.descriptor();
    assert_eq!(descriptor.licensing, LicensingClass::Synthetic);
    assert!(!descriptor.is_production_grade());
    assert!(
        descriptor
            .production_requirement
            .as_ref()
            .is_some_and(|r| r.contains("docs/operations/external-dependencies.md")),
        "the adapter must say what a real deployment needs"
    );
    assert!(!descriptor.topics.is_empty());
}

#[test]
fn production_licensing_enforcement_refuses_synthetic_sources() {
    let now = start();
    let (context, _clock) = Context::deterministic(now, 1);
    let mut bus = EventBus::new();
    let mut service =
        IngestionService::new(Telemetry::silent()).enforcing_production_licensing(true);
    service.register(Box::new(environment(1)));

    assert!(!service.is_production_ready());
    assert_eq!(service.non_production_sources().len(), 1);

    let published = service
        .poll_and_publish(
            &context,
            &mut bus,
            now.saturating_add(Duration::from_hours(1)),
        )
        .unwrap();
    assert_eq!(
        published, 0,
        "a synthetic source must be refused under enforcement"
    );
}

#[test]
fn records_round_trip_through_the_replay_adapter() {
    let mut environment = environment(163);
    let original = environment.run_until(start().saturating_add(Duration::from_mins(30)));
    assert!(!original.is_empty());

    let dir = std::env::temp_dir().join(format!("qip-replay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("capture.jsonl");
    let written = ReplayAdapter::write(&path, &original).unwrap();
    assert_eq!(written, original.len());

    let mut replay = ReplayAdapter::open("capture", &path).unwrap();
    assert!(replay.skipped().is_empty(), "nothing should fail to parse");
    assert_eq!(replay.len(), original.len());

    let replayed = replay.poll(Timestamp::MAX).unwrap();
    assert_eq!(replayed.len(), original.len());

    // Replay is in event order, which the original already was.
    let original_topics: Vec<&str> = original.iter().map(|r| r.topic().name()).collect();
    let replayed_topics: Vec<&str> = replayed.iter().map(|r| r.topic().name()).collect();
    assert_eq!(original_topics, replayed_topics);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replay_respects_the_callers_clock() {
    let mut environment = environment(167);
    let records = environment.run_until(start().saturating_add(Duration::from_hours(2)));
    let mut replay = ReplayAdapter::from_records("memory", records);
    let total = replay.len();

    let first_half = replay
        .poll(start().saturating_add(Duration::from_hours(1)))
        .unwrap();
    assert!(!first_half.is_empty() && first_half.len() < total);
    assert!(
        first_half
            .iter()
            .all(|r| r.occurred_at() <= start().saturating_add(Duration::from_hours(1)))
    );

    let rest = replay.poll(Timestamp::MAX).unwrap();
    assert_eq!(first_half.len() + rest.len(), total);
    assert_eq!(replay.remaining(), 0);
}

#[test]
fn a_malformed_replay_line_is_reported_not_silently_skipped() {
    let dir = std::env::temp_dir().join(format!("qip-replay-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.jsonl");
    std::fs::write(&path, "{\"record\":\"nonsense\"}\n# a comment\n\n").unwrap();

    let replay = ReplayAdapter::open("bad", &path).unwrap();
    assert_eq!(replay.len(), 0);
    assert_eq!(replay.skipped().len(), 1, "the bad line must be reported");
    assert!(replay.skipped()[0].starts_with("line 1"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ingestion_reports_what_it_published_by_topic() {
    let now = start();
    let (context, _clock) = Context::deterministic(now, 1);
    let mut bus = EventBus::new();
    let mut service = IngestionService::new(Telemetry::silent());
    service.register(Box::new(environment(173)));
    assert_eq!(service.adapter_count(), 1);
    service.start(now).unwrap();

    service
        .poll_and_publish(
            &context,
            &mut bus,
            now.saturating_add(Duration::from_hours(1)),
        )
        .unwrap();
    service.stop().unwrap();

    let stats = service.stats();
    assert!(stats.published > 50, "published {}", stats.published);
    assert!(stats.by_topic.contains_key("market.quote"));
    assert!(stats.by_topic.contains_key("market.tick"));
    assert_eq!(
        stats.rejected, 0,
        "the simulator should not emit invalid records"
    );
}
