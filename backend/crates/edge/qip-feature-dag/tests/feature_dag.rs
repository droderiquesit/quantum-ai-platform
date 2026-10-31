//! Properties the incremental feature DAG has to hold.
//!
//! The load-bearing one is `incremental_evaluation_equals_recomputing_every_feature_from_scratch`.
//! Everything else here is a property that makes it cheap; that one is the
//! property that makes it correct.

use qip_contracts::{
    BookSide, FeatureKey, FeatureValue, MarketMessage, MessageBody, Origin, TradeCondition,
    VenueId, VenueStatus,
};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_feature_dag::definition::{FeatureContext, FeatureDefinition, ValueKind};
use qip_feature_dag::features::{
    BookPressure, CrossVenueSpread, ExponentialMovingAverage, FlowToxicity, LeadLagCorrelation,
    MicropriceDeviation, Mid, OrderFlowImbalance, QueueSide, RealisedVolatility,
    RollingCorrelation, Spread, SpreadPercentile, SweepCost, TimeSinceLastTrade, TouchQueueDepth,
    TradeSignAutocorrelation, standard_suite,
};
use qip_feature_dag::state::MarketReads;
use qip_feature_dag::{FeatureEngine, MarketState};
use qip_market::book::Side;

// --- fixtures ---------------------------------------------------------------

fn object(tag: &str) -> ObjectId {
    ObjectId::from_string(format!("OBJ{tag:0>23}"))
}

fn origin(sequence: u64) -> Origin {
    Origin::new(VenueId::new("XTST"), "primary", 0, sequence)
}

fn message(subject: &ObjectId, body: MessageBody, at: Timestamp, sequence: u64) -> MarketMessage {
    MarketMessage::new(subject.clone(), origin(sequence), body, at, at)
}

fn quote(
    subject: &ObjectId,
    bid: &str,
    ask: &str,
    size: i64,
    at: Timestamp,
    sequence: u64,
) -> MarketMessage {
    let body = MessageBody::Quote {
        bid: Decimal::parse(bid).map(|price| (price, Decimal::from_int(size))),
        ask: Decimal::parse(ask).map(|price| (price, Decimal::from_int(size + 1))),
    };
    message(subject, body, at, sequence)
}

fn trade(subject: &ObjectId, price: &str, at: Timestamp, sequence: u64) -> MarketMessage {
    let body = MessageBody::Trade {
        price: Decimal::parse(price).unwrap_or(Decimal::ZERO),
        quantity: Decimal::from_int(10),
        condition: TradeCondition::Regular,
        aggressor: Some(BookSide::Bid),
    };
    message(subject, body, at, sequence)
}

fn engine_for(subjects: &[ObjectId]) -> Result<FeatureEngine> {
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    for subject in subjects {
        for definition in standard_suite(subject) {
            engine.register(definition)?;
        }
    }
    if let [first, second, ..] = subjects {
        engine.register(Box::new(RollingCorrelation::new(
            first.clone(),
            second.clone(),
            10,
        )))?;
    }
    Ok(engine)
}

/// A definition that exists only to be pointed at another one.
#[derive(Debug)]
struct Echo {
    name: &'static str,
    subject: ObjectId,
    depends_on: Vec<FeatureKey>,
}

impl FeatureDefinition for Echo {
    fn key(&self) -> FeatureKey {
        FeatureKey::new(self.name, self.subject.clone())
    }

    fn dependencies(&self) -> Vec<FeatureKey> {
        self.depends_on.clone()
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Count
    }

    fn compute(&self, _ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        Ok(FeatureValue::Count(1))
    }
}

/// A definition that declares one kind and returns another.
#[derive(Debug)]
struct Liar {
    subject: ObjectId,
}

impl FeatureDefinition for Liar {
    fn key(&self) -> FeatureKey {
        FeatureKey::new("liar", self.subject.clone())
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Exact
    }

    fn compute(&self, _ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        Ok(FeatureValue::Statistic(1.0))
    }
}

// --- sharing and deduplication ---------------------------------------------

#[test]
fn twenty_consumers_of_one_feature_produce_one_node_and_one_computation() {
    let subject = object("AAA");
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    let key = RealisedVolatility::key(&subject, 20);

    let mut ids = Vec::new();
    for _ in 0..20 {
        ids.push(
            engine
                .register(Box::new(RealisedVolatility::new(subject.clone(), 20)))
                .unwrap(),
        );
    }

    assert_eq!(engine.graph().len(), 1, "twenty registrations, one node");
    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "every consumer must be handed the same node"
    );
    assert_eq!(engine.graph().consumers(&key), 20);

    let at = Timestamp::from_secs(1_700_000_000);
    engine
        .ingest(&quote(&subject, "100.00", "100.02", 10, at, 1))
        .unwrap();
    engine.reset_counters();
    engine.evaluate(at).unwrap();
    assert_eq!(
        engine.computations(),
        1,
        "the shared feature is computed once for all twenty consumers"
    );
}

#[test]
fn a_key_built_independently_by_two_callers_names_the_same_node() {
    let subject = object("AAA");
    let from_definition = RealisedVolatility::new(subject.clone(), 20).key();
    let from_helper = RealisedVolatility::key(&subject, 20);
    assert_eq!(from_definition.canonical(), from_helper.canonical());

    // Parameter order must not matter to identity, or sharing depends on how
    // each caller happened to spell the request.
    let forwards = FeatureKey::new("x", subject.clone())
        .with("window", 20)
        .with("lag", 1);
    let backwards = FeatureKey::new("x", subject)
        .with("lag", 1)
        .with("window", 20);
    assert_eq!(forwards.canonical(), backwards.canonical());
}

// --- incrementality ---------------------------------------------------------

#[test]
fn evaluating_twice_with_no_new_messages_recomputes_nothing() {
    let subject = object("AAA");
    let mut engine = engine_for(std::slice::from_ref(&subject)).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    engine
        .ingest(&quote(&subject, "100.00", "100.02", 10, at, 1))
        .unwrap();
    engine.evaluate(at).unwrap();

    engine.reset_counters();
    engine.evaluate(at).unwrap();
    assert_eq!(
        engine.computations(),
        0,
        "a second pass over an unchanged market must compute nothing"
    );
    assert_eq!(engine.dirty_count(), 0);
}

#[test]
fn only_a_feature_of_time_itself_recomputes_when_the_clock_moves() {
    let subject = object("AAA");
    let mut engine = engine_for(std::slice::from_ref(&subject)).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    engine
        .ingest(&quote(&subject, "100.00", "100.02", 10, at, 1))
        .unwrap();
    engine.ingest(&trade(&subject, "100.01", at, 2)).unwrap();
    engine.evaluate(at).unwrap();

    engine.reset_counters();
    let later = at.saturating_add(Duration::from_secs(1));
    engine.evaluate(later).unwrap();
    assert_eq!(
        engine.computations(),
        1,
        "only the time-since-last-trade reading depends on the instant"
    );

    let elapsed = engine.value(&TimeSinceLastTrade::key(&subject)).unwrap();
    assert_eq!(elapsed, FeatureValue::Count(1_000_000_000));
}

#[test]
fn a_message_about_one_instrument_leaves_another_instruments_features_untouched() {
    let left = object("AAA");
    let right = object("BBB");
    let mut engine = engine_for(&[left.clone(), right.clone()]).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    engine
        .ingest(&quote(&left, "100.00", "100.02", 10, at, 1))
        .unwrap();
    engine
        .ingest(&quote(&right, "50.00", "50.01", 10, at, 2))
        .unwrap();
    engine.evaluate(at).unwrap();

    let watched: Vec<FeatureKey> = vec![
        Mid::key(&right),
        Spread::key(&right),
        RealisedVolatility::key(&right, 20),
        BookPressure::key(&right, 5),
        ExponentialMovingAverage::key(&right, 20),
    ];
    let before: Vec<_> = watched
        .iter()
        .map(|key| engine.revision(key).unwrap())
        .collect();

    let next = at.saturating_add(Duration::from_millis(500));
    engine
        .ingest(&quote(&left, "100.01", "100.03", 12, next, 3))
        .unwrap();
    engine.evaluate(next).unwrap();

    let after: Vec<_> = watched
        .iter()
        .map(|key| engine.revision(key).unwrap())
        .collect();
    assert_eq!(
        before, after,
        "a quote on one instrument must not recompute another's features"
    );
    assert!(
        engine.revision(&Mid::key(&left)).unwrap().get() > 1,
        "the instrument that moved must have been recomputed"
    );
}

#[test]
fn a_print_cannot_dirty_a_feature_that_only_reads_the_top_of_book() {
    let subject = object("AAA");
    let mut engine = engine_for(std::slice::from_ref(&subject)).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    engine
        .ingest(&quote(&subject, "100.00", "100.02", 10, at, 1))
        .unwrap();
    engine.evaluate(at).unwrap();

    let spread_before = engine.revision(&Spread::key(&subject)).unwrap();
    let pressure_before = engine.revision(&BookPressure::key(&subject, 5)).unwrap();

    engine.ingest(&trade(&subject, "100.02", at, 2)).unwrap();
    assert!(
        !engine.is_dirty(&Spread::key(&subject)),
        "a trade cannot move the touch, so it cannot dirty the spread"
    );
    assert!(engine.is_dirty(&TradeSignAutocorrelation::key(&subject, 20, 1)));
    engine.evaluate(at).unwrap();

    assert_eq!(
        spread_before,
        engine.revision(&Spread::key(&subject)).unwrap()
    );
    assert_eq!(
        pressure_before,
        engine.revision(&BookPressure::key(&subject, 5)).unwrap()
    );
}

#[test]
fn dirtiness_reaches_every_feature_computed_from_a_changed_one() {
    let subject = object("AAA");
    let mut engine = engine_for(std::slice::from_ref(&subject)).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    engine
        .ingest(&quote(&subject, "100.00", "100.02", 10, at, 1))
        .unwrap();
    engine.evaluate(at).unwrap();

    // The deviation reads no market state at all; it can only be dirtied
    // through the mid, microprice and spread it is computed from.
    assert!(!engine.is_dirty(&MicropriceDeviation::key(&subject)));
    engine
        .ingest(&quote(
            &subject,
            "100.01",
            "100.03",
            12,
            at.saturating_add(Duration::from_millis(200)),
            2,
        ))
        .unwrap();
    assert!(
        engine.is_dirty(&MicropriceDeviation::key(&subject)),
        "dirtiness must travel from a dependency to its dependents"
    );
}

#[test]
fn a_correlation_is_dirtied_by_either_instrument_it_reads() {
    let left = object("AAA");
    let right = object("BBB");
    let mut engine = engine_for(&[left.clone(), right.clone()]).unwrap();
    let key = RollingCorrelation::key(&left, &right, 10);
    let at = Timestamp::from_secs(1_700_000_000);

    engine
        .ingest(&quote(&left, "100.00", "100.02", 10, at, 1))
        .unwrap();
    engine.evaluate(at).unwrap();
    assert!(!engine.is_dirty(&key));

    engine
        .ingest(&quote(&right, "50.00", "50.01", 10, at, 2))
        .unwrap();
    assert!(
        engine.is_dirty(&key),
        "the second instrument is declared, so it must dirty the pair"
    );
    engine.evaluate(at).unwrap();

    let unrelated = object("CCC");
    engine
        .ingest(&quote(&unrelated, "10.00", "10.01", 10, at, 3))
        .unwrap();
    assert!(
        !engine.is_dirty(&key),
        "an instrument the pair does not read must leave it alone"
    );
}

// --- the equivalence property ----------------------------------------------

/// A random but reproducible message sequence over three instruments.
fn random_messages(
    rng: &mut Xoshiro256,
    subjects: &[ObjectId],
    count: usize,
) -> Vec<MarketMessage> {
    let mut at = Timestamp::from_secs(1_700_000_000);
    let mut bases: Vec<i64> = subjects.iter().map(|_| 10_000).collect();
    let mut messages = Vec::with_capacity(count);

    for sequence in 0..count {
        let which = rng.below(subjects.len() as u64) as usize;
        at = at.saturating_add(Duration::from_millis(20 + rng.below(180) as i64));
        bases[which] = (bases[which] + rng.below(9) as i64 - 4).max(100);
        let base = bases[which];
        let half = 1 + rng.below(3) as i64;
        let tick = |ticks: i64| Decimal::from_scaled(i128::from(ticks), 2).unwrap_or(Decimal::ZERO);

        let body = match rng.below(10) {
            0..=4 => MessageBody::Quote {
                bid: Some((
                    tick(base - half),
                    Decimal::from_int(1 + rng.below(20) as i64),
                )),
                ask: Some((
                    tick(base + half),
                    Decimal::from_int(1 + rng.below(20) as i64),
                )),
            },
            5..=6 => MessageBody::LevelSet {
                side: if rng.bernoulli(0.5) {
                    BookSide::Bid
                } else {
                    BookSide::Ask
                },
                price: tick(
                    base + if rng.bernoulli(0.5) {
                        half + 2
                    } else {
                        -half - 2
                    },
                ),
                quantity: Decimal::from_int(rng.below(15) as i64),
                order_count: None,
            },
            7..=8 => MessageBody::Trade {
                price: tick(base + rng.below(3) as i64 - 1),
                quantity: Decimal::from_int(1 + rng.below(9) as i64),
                condition: if rng.bernoulli(0.85) {
                    TradeCondition::Regular
                } else {
                    TradeCondition::OddLot
                },
                aggressor: if rng.bernoulli(0.6) {
                    Some(if rng.bernoulli(0.5) {
                        BookSide::Bid
                    } else {
                        BookSide::Ask
                    })
                } else {
                    None
                },
            },
            _ => MessageBody::StatusChange {
                status: if rng.bernoulli(0.9) {
                    VenueStatus::Open
                } else {
                    VenueStatus::Auction
                },
            },
        };
        messages.push(message(&subjects[which], body, at, sequence as u64 + 1));
    }
    messages
}

#[test]
fn incremental_evaluation_equals_recomputing_every_feature_from_scratch() {
    let subjects = [object("AAA"), object("BBB"), object("CCC")];
    let mut rng = Xoshiro256::seeded(0x000F_EA7E_5D46_0001);
    let messages = random_messages(&mut rng, &subjects, 400);

    let mut incremental = engine_for(&subjects).unwrap();
    let mut defined_seen = 0usize;
    let mut compared = 0usize;

    for (index, msg) in messages.iter().enumerate() {
        incremental.ingest(msg).unwrap();
        let as_of = msg.venue_time;
        let live = incremental.evaluate(as_of).unwrap();

        if index % 17 != 0 {
            continue;
        }
        // Rebuild from nothing over exactly the messages seen so far. Any
        // difference means an invalidation was missed, and a cell would be
        // holding a value the market no longer supports.
        let mut rebuilt = engine_for(&subjects).unwrap();
        for replay in &messages[..=index] {
            rebuilt.ingest(replay).unwrap();
        }
        let reference = rebuilt.evaluate(as_of).unwrap();

        assert_eq!(live.len(), reference.len());
        for (key, value, _) in reference.iter() {
            let incremental_value = live.get(key).unwrap();
            assert_eq!(
                incremental_value, value,
                "message {index}: {key} disagreed between incremental and full evaluation"
            );
            if value.is_defined() {
                defined_seen += 1;
            }
            compared += 1;
        }
    }

    assert!(
        compared > 500,
        "the comparison must actually cover the graph"
    );
    assert!(
        defined_seen * 3 > compared,
        "a sequence where almost nothing is computable proves nothing: \
         {defined_seen} defined of {compared}"
    );
}

#[test]
fn replaying_the_same_messages_twice_produces_identical_values() {
    let subjects = [object("AAA"), object("BBB")];
    let mut rng = Xoshiro256::seeded(0x0DE7_E721_1000_0007);
    let messages = random_messages(&mut rng, &subjects, 120);
    let as_of = messages.last().unwrap().venue_time;

    let run = |messages: &[MarketMessage]| {
        let mut engine = engine_for(&subjects).unwrap();
        for msg in messages {
            engine.ingest(msg).unwrap();
        }
        engine.evaluate(as_of)
    };

    assert_eq!(
        run(&messages).unwrap(),
        run(&messages).unwrap(),
        "two runs over one message log must agree bit for bit"
    );
}

// --- refusals ---------------------------------------------------------------

#[test]
fn a_cycle_is_refused_at_registration_and_the_cycle_is_named() {
    let subject = object("AAA");
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));

    let a = FeatureKey::new("a", subject.clone());
    let b = FeatureKey::new("b", subject.clone());
    let c = FeatureKey::new("c", subject.clone());

    engine
        .register(Box::new(Echo {
            name: "a",
            subject: subject.clone(),
            depends_on: vec![b.clone()],
        }))
        .unwrap();
    engine
        .register(Box::new(Echo {
            name: "b",
            subject: subject.clone(),
            depends_on: vec![c.clone()],
        }))
        .unwrap();

    let refused = engine
        .register(Box::new(Echo {
            name: "c",
            subject: subject.clone(),
            depends_on: vec![a.clone()],
        }))
        .unwrap_err();

    let message = refused.to_string();
    assert!(message.contains("cycle"), "{message}");
    for key in [&a, &b, &c] {
        assert!(
            message.contains(&key.canonical()),
            "the cycle must name every feature in it: {message}"
        );
    }
    assert!(
        !engine.graph().is_defined(&c),
        "a refused registration must leave nothing behind"
    );
}

#[test]
fn a_feature_that_depends_on_itself_is_refused() {
    let subject = object("AAA");
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    let self_key = FeatureKey::new("a", subject.clone());
    let refused = engine
        .register(Box::new(Echo {
            name: "a",
            subject,
            depends_on: vec![self_key],
        }))
        .unwrap_err();
    assert!(refused.to_string().contains("cycle"), "{refused}");
}

#[test]
fn a_definition_that_computes_a_different_kind_than_it_declares_is_refused() {
    let subject = object("AAA");
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    engine
        .register(Box::new(Liar {
            subject: subject.clone(),
        }))
        .unwrap();
    let refused = engine.evaluate(Timestamp::from_secs(1)).unwrap_err();
    assert_eq!(refused.code(), "schema", "{refused}");
}

#[test]
fn a_feature_referenced_but_never_registered_is_reported_rather_than_assumed() {
    let subject = object("AAA");
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    let missing = FeatureKey::new("nowhere", subject.clone());
    engine
        .register(Box::new(Echo {
            name: "a",
            subject: subject.clone(),
            depends_on: vec![missing.clone()],
        }))
        .unwrap();

    assert_eq!(engine.graph().unresolved().len(), 1);
    assert!(engine.graph().contains(&missing));
    assert!(!engine.graph().is_defined(&missing));
    let refused = engine.graph().require_complete().unwrap_err();
    assert!(refused.to_string().contains("nowhere"), "{refused}");

    let vector = engine.evaluate(Timestamp::from_secs(1)).unwrap();
    assert_eq!(vector.get(&missing).unwrap(), FeatureValue::Undefined);
}

// --- undefined rather than a plausible number -------------------------------

#[test]
fn insufficient_history_yields_undefined_rather_than_zero() {
    let subject = object("AAA");
    let mut engine = engine_for(std::slice::from_ref(&subject)).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    engine
        .ingest(&quote(&subject, "100.00", "100.02", 10, at, 1))
        .unwrap();
    let vector = engine.evaluate(at).unwrap();

    for key in [
        RealisedVolatility::key(&subject, 20),
        ExponentialMovingAverage::key(&subject, 20),
        OrderFlowImbalance::key(&subject, 10),
        TradeSignAutocorrelation::key(&subject, 20, 1),
        SpreadPercentile::key(&subject, 20),
        TimeSinceLastTrade::key(&subject),
    ] {
        assert_eq!(
            vector.get(&key).unwrap(),
            FeatureValue::Undefined,
            "{key} must be undefined on one observation, not zero"
        );
    }

    // The features that need no history are defined immediately, so the test
    // above is measuring history and not a graph that simply never computes.
    assert!(vector.get(&Mid::key(&subject)).unwrap().is_defined());
    assert!(vector.get(&Spread::key(&subject)).unwrap().is_defined());
}

#[test]
fn a_feed_that_has_gone_quiet_makes_its_features_undefined() {
    let subject = object("AAA");
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    engine
        .register(Box::new(Mid::new(subject.clone())))
        .unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    engine
        .ingest(&quote(&subject, "100.00", "100.02", 10, at, 1))
        .unwrap();

    let fresh = engine.evaluate(at).unwrap();
    assert!(fresh.get(&Mid::key(&subject)).unwrap().is_defined());

    let much_later = at.saturating_add(Duration::from_secs(60));
    let stale = engine.evaluate(much_later).unwrap();
    assert_eq!(
        stale.get(&Mid::key(&subject)).unwrap(),
        FeatureValue::Undefined,
        "a price from a feed that stopped an hour ago is not a price"
    );
}

#[test]
fn a_one_sided_or_crossed_book_has_no_mid() {
    let subject = object("AAA");
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    engine
        .register(Box::new(Mid::new(subject.clone())))
        .unwrap();
    let at = Timestamp::from_secs(1_700_000_000);

    engine
        .ingest(&message(
            &subject,
            MessageBody::Quote {
                bid: Some((Decimal::parse("100.00").unwrap(), Decimal::from_int(5))),
                ask: None,
            },
            at,
            1,
        ))
        .unwrap();
    assert_eq!(
        engine
            .evaluate(at)
            .unwrap()
            .get(&Mid::key(&subject))
            .unwrap(),
        FeatureValue::Undefined
    );

    engine
        .ingest(&quote(&subject, "100.05", "100.00", 5, at, 2))
        .unwrap();
    assert_eq!(
        engine
            .evaluate(at)
            .unwrap()
            .get(&Mid::key(&subject))
            .unwrap(),
        FeatureValue::Undefined,
        "a crossed book is a book we know to be wrong"
    );
}

#[test]
fn a_reset_discards_the_history_it_invalidates() {
    let subject = object("AAA");
    let mut engine = engine_for(std::slice::from_ref(&subject)).unwrap();
    let mut at = Timestamp::from_secs(1_700_000_000);
    for sequence in 0..40u64 {
        at = at.saturating_add(Duration::from_millis(200));
        let bid = format!("{:.2}", 100.0 + f64::from(sequence as u32) * 0.01);
        let ask = format!("{:.2}", 100.05 + f64::from(sequence as u32) * 0.01);
        engine
            .ingest(&quote(&subject, &bid, &ask, 10, at, sequence + 1))
            .unwrap();
    }
    let before = engine.evaluate(at).unwrap();
    assert!(
        before
            .get(&RealisedVolatility::key(&subject, 20))
            .unwrap()
            .is_defined()
    );

    engine
        .ingest(&message(
            &subject,
            MessageBody::Reset {
                reason: "sequence gap".into(),
            },
            at,
            100,
        ))
        .unwrap();
    let after = engine.evaluate(at).unwrap();
    assert_eq!(
        after.get(&RealisedVolatility::key(&subject, 20)).unwrap(),
        FeatureValue::Undefined,
        "history built on a book known to be wrong must not survive the reset"
    );
}

// --- the shape of an evaluation ---------------------------------------------

#[test]
fn one_evaluation_produces_one_consistent_view() {
    let subject = object("AAA");
    let mut engine = engine_for(std::slice::from_ref(&subject)).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    engine
        .ingest(&quote(&subject, "100.00", "100.02", 10, at, 1))
        .unwrap();
    let vector = engine.evaluate(at).unwrap();

    assert_eq!(vector.as_of(), Some(at));
    assert_eq!(
        vector.len(),
        engine.graph().len(),
        "the vector must carry every node, so nothing is read from a stale cache"
    );
    for (key, _, revision) in vector.iter() {
        assert_eq!(engine.revision(key), Some(revision));
    }
}

#[test]
fn features_are_evaluated_after_everything_they_are_computed_from() {
    let subject = object("AAA");
    let engine = engine_for(std::slice::from_ref(&subject)).unwrap();
    let order: Vec<String> = engine
        .graph()
        .keys()
        .iter()
        .map(|key| key.canonical())
        .collect();

    for (position, key) in engine.graph().keys().iter().enumerate() {
        for dependency in engine.graph().dependencies_of(key) {
            let at = order
                .iter()
                .position(|name| *name == dependency.canonical())
                .unwrap();
            assert!(
                at < position,
                "{dependency} must be evaluated before {key} that reads it"
            );
        }
    }
}

#[test]
fn a_message_that_changes_nothing_a_feature_reads_dirties_nothing() {
    let subject = object("AAA");
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    engine
        .register(Box::new(Spread::new(subject.clone())))
        .unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    engine
        .ingest(&quote(&subject, "100.00", "100.02", 10, at, 1))
        .unwrap();
    engine.evaluate(at).unwrap();

    engine
        .ingest(&message(
            &subject,
            MessageBody::AuctionUpdate {
                indicative_price: None,
                paired: Decimal::ZERO,
                imbalance: Decimal::ZERO,
                imbalance_side: None,
            },
            at,
            2,
        ))
        .unwrap();
    assert_eq!(engine.dirty_count(), 0);
}

#[test]
fn the_read_sets_of_a_message_follow_the_contracts_own_predicate() {
    let trade_reads = MarketReads::of_message(&MessageBody::Trade {
        price: Decimal::ONE,
        quantity: Decimal::ONE,
        condition: TradeCondition::Regular,
        aggressor: None,
    });
    assert!(!trade_reads.intersects(MarketReads::TOUCH));
    assert!(trade_reads.intersects(MarketReads::TRADES));

    let quote_reads = MarketReads::of_message(&MessageBody::Quote {
        bid: None,
        ask: None,
    });
    assert!(quote_reads.intersects(MarketReads::TOUCH));

    let reset_reads = MarketReads::of_message(&MessageBody::Reset {
        reason: String::new(),
    });
    for aspect in [
        MarketReads::TOUCH,
        MarketReads::DEPTH,
        MarketReads::TRADES,
        MarketReads::STATUS,
    ] {
        assert!(
            reset_reads.intersects(aspect),
            "a reset invalidates everything derived from the book"
        );
    }
}

#[test]
fn a_message_that_does_not_move_the_touch_does_not_duplicate_the_spread_series() {
    // `InstrumentState::spreads` is documented as one entry per touch change,
    // matching `flow`. A level added three prices away from the touch still
    // reaches `refresh_touch`, and must not append another copy of the
    // unchanged spread — a percentile computed over that history would then
    // be dominated by duplicates of the current value rather than by genuine
    // history, biasing `SpreadPercentile` toward 1.0 regardless of the real
    // distribution.
    let subject = object("AAA");
    let mut state = qip_feature_dag::MarketState::default();
    let at = Timestamp::from_secs(1_700_000_000);
    state
        .apply(&quote(&subject, "100.00", "100.02", 10, at, 1))
        .unwrap();
    // Assert the premise: one touch-establishing message already produced one
    // spread observation, so the count checked below is a count of
    // duplicates, not of a series that never grew at all.
    assert_eq!(state.instrument(&subject).unwrap().spreads().len(), 1);

    for (sequence, price) in [(2u64, "99.00"), (3, "98.50"), (4, "98.00")] {
        let at_n = at.saturating_add(Duration::from_millis(200 * sequence as i64));
        state
            .apply(&message(
                &subject,
                MessageBody::LevelSet {
                    side: BookSide::Bid,
                    price: Decimal::parse(price).unwrap(),
                    quantity: Decimal::from_int(5),
                    order_count: None,
                },
                at_n,
                sequence,
            ))
            .unwrap();
    }
    assert_eq!(
        state.instrument(&subject).unwrap().spreads().len(),
        1,
        "three messages that never moved the touch must not grow a series \
         documented as one entry per touch change"
    );

    // A message that does move the touch must still be recorded — the fix
    // must gate duplicates, not freeze the series.
    let at_touch = at.saturating_add(Duration::from_secs(1));
    state
        .apply(&quote(&subject, "100.01", "100.03", 10, at_touch, 5))
        .unwrap();
    assert_eq!(
        state.instrument(&subject).unwrap().spreads().len(),
        2,
        "a genuine touch change must still be recorded"
    );
}

// --- the named microstructure families (R12-TICK-2063) ----------------------
//
// Each family the blueprint names has a feature here with a value worked by
// hand from the messages the test sends, so a definition that computes the
// wrong thing fails on the number and not merely on whether one came back.

fn signed_trade(
    subject: &ObjectId,
    quantity: i64,
    aggressor: BookSide,
    at: Timestamp,
    sequence: u64,
) -> MarketMessage {
    let body = MessageBody::Trade {
        price: Decimal::parse("100.01").unwrap_or(Decimal::ZERO),
        quantity: Decimal::from_int(quantity),
        condition: TradeCondition::Regular,
        aggressor: Some(aggressor),
    };
    message(subject, body, at, sequence)
}

fn close(actual: FeatureValue, expected: f64, what: &str) {
    match actual {
        FeatureValue::Statistic(value) => assert!(
            (value - expected).abs() < 1e-9,
            "{what}: {value} against a hand-computed {expected}"
        ),
        other => panic!("{what}: {other:?} where a statistic of {expected} was due"),
    }
}

#[test]
fn queue_toxicity_and_liquidity_match_their_hand_computed_values() {
    let subject = object("FAM");
    let mut engine = engine_for(std::slice::from_ref(&subject)).unwrap();
    engine
        .register(Box::new(FlowToxicity::new(subject.clone(), 4)))
        .unwrap();
    engine
        .register(Box::new(SweepCost::new(
            subject.clone(),
            Side::Buy,
            Decimal::from_int(12),
        )))
        .unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    // Bid 100.00 for 10, ask 100.02 for 11: mid 100.01.
    engine
        .ingest(&quote(&subject, "100.00", "100.02", 10, at, 1))
        .unwrap();
    // Three buyer-initiated prints of 10 and one seller-initiated of 10.
    for (index, aggressor) in [BookSide::Bid, BookSide::Bid, BookSide::Ask, BookSide::Bid]
        .into_iter()
        .enumerate()
    {
        engine
            .ingest(&signed_trade(&subject, 10, aggressor, at, 2 + index as u64))
            .unwrap();
    }
    let vector = engine.evaluate(at).unwrap();

    // Queue: what rests at each touch.
    assert_eq!(
        vector
            .get(&TouchQueueDepth::key(&subject, QueueSide::Bid))
            .unwrap(),
        FeatureValue::Exact(Decimal::from_int(10))
    );
    assert_eq!(
        vector
            .get(&TouchQueueDepth::key(&subject, QueueSide::Ask))
            .unwrap(),
        FeatureValue::Exact(Decimal::from_int(11))
    );
    // Toxicity: |30 - 10| / 40.
    close(
        vector.get(&FlowToxicity::key(&subject, 4)).unwrap(),
        0.5,
        "flow toxicity",
    );
    // Liquidity: one lot bought at 100.02 against a 100.01 mid is
    // 0.01 / 100.01 of the mid, in basis points; sold at 100.00, the same.
    let one_lot_bps = 0.01 / 100.01 * 10_000.0;
    close(
        vector
            .get(&SweepCost::key(&subject, Side::Buy, Decimal::ONE))
            .unwrap(),
        one_lot_bps,
        "buy sweep cost",
    );
    close(
        vector
            .get(&SweepCost::key(&subject, Side::Sell, Decimal::ONE))
            .unwrap(),
        one_lot_bps,
        "sell sweep cost",
    );
    // Twelve lots against an eleven-lot ask cannot be filled: a limit, not
    // an extrapolated cost.
    assert_eq!(
        vector
            .get(&SweepCost::key(&subject, Side::Buy, Decimal::from_int(12)))
            .unwrap(),
        FeatureValue::Undefined
    );
}

#[test]
fn a_follower_one_sample_behind_its_leader_has_a_lead_lag_correlation_of_one_and_a_cross_venue_spread_in_basis_points()
 {
    let follower = object("FOLLOW");
    let leader = object("LEAD");
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    engine
        .register(Box::new(LeadLagCorrelation::new(
            follower.clone(),
            leader.clone(),
            5,
            1,
        )))
        .unwrap();
    engine
        .register(Box::new(CrossVenueSpread::new(
            follower.clone(),
            leader.clone(),
        )))
        .unwrap();
    // The leader's bids, one per 100 ms sampling bucket; the follower quotes
    // the leader's previous bid, so its every return is the leader's return
    // one step earlier.
    let bids = [
        "100.00", "101.00", "99.00", "102.00", "100.00", "103.00", "98.00", "101.00", "104.00",
        "100.50",
    ];
    let start = Timestamp::from_secs(1_700_000_000);
    let mut sequence = 0;
    for (step, bid) in bids.iter().enumerate() {
        let at = start.saturating_add(Duration::from_millis(100 * step as i64));
        let previous = if step == 0 { bid } else { &bids[step - 1] };
        let ask_of = |bid: &str| {
            (Decimal::parse(bid).unwrap_or(Decimal::ZERO)
                + Decimal::parse("0.02").unwrap_or(Decimal::ZERO))
            .to_string()
        };
        sequence += 1;
        engine
            .ingest(&quote(&leader, bid, &ask_of(bid), 10, at, sequence))
            .unwrap();
        sequence += 1;
        engine
            .ingest(&quote(
                &follower,
                previous,
                &ask_of(previous),
                10,
                at,
                sequence,
            ))
            .unwrap();
    }
    let last = start.saturating_add(Duration::from_millis(100 * (bids.len() as i64 - 1)));
    let vector = engine.evaluate(last).unwrap();

    close(
        vector
            .get(&LeadLagCorrelation::key(&follower, &leader, 5, 1))
            .unwrap(),
        1.0,
        "lead/lag correlation",
    );
    // Cross-venue: at the last step the follower quotes the leader's previous
    // bid, 104.00 (mid 104.01), and the leader bids 100.50 (mid 100.51), so
    // the leader sits (100.51 - 104.01) / 104.01 of the follower's mid away.
    close(
        vector
            .get(&CrossVenueSpread::key(&follower, &leader))
            .unwrap(),
        (100.51 - 104.01) / 104.01 * 10_000.0,
        "cross-venue spread",
    );
}

/// Fails if a named family loses its feature from the standard suite the
/// edge node registers for every strategy subject. The two-subject families
/// (lead/lag, cross-venue) are definitions a strategy registers for a pair,
/// like the rolling correlation, and are proven by the test above.
#[test]
fn the_standard_suite_carries_the_queue_toxicity_and_liquidity_families() {
    let subject = object("SUITE");
    let names: Vec<String> = standard_suite(&subject)
        .iter()
        .map(|definition| definition.key().name)
        .collect();
    for family in [
        TouchQueueDepth::NAME,
        FlowToxicity::NAME,
        SweepCost::NAME,
        Spread::NAME,
        BookPressure::NAME,
        OrderFlowImbalance::NAME,
        RealisedVolatility::NAME,
    ] {
        assert!(
            names.iter().any(|name| name == family),
            "the standard suite has no {family} feature: {names:?}"
        );
    }
}

// --- quarantine (R12-RES-7102) ----------------------------------------------

#[test]
fn a_quarantined_instrument_withholds_every_feature_computed_from_it_and_nothing_else() {
    // The failure this prevents: a feature of another instrument computed
    // from an unreliable one's mid would carry the bad value out under a
    // healthy subject, if the quarantine were keyed on the subject alone and
    // not on what the feature was computed from.
    let a = object("AAA");
    let b = object("BBB");
    let at = Timestamp::from_secs(1_700_000_000);
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    engine.register(Box::new(Mid::new(a.clone()))).unwrap();
    engine.register(Box::new(Mid::new(b.clone()))).unwrap();
    let echo = Echo {
        name: "echo_of_a",
        subject: b.clone(),
        depends_on: vec![Mid::key(&a)],
    };
    let echo_key = echo.key();
    engine.register(Box::new(echo)).unwrap();
    engine.ingest(&quote(&a, "99", "101", 10, at, 1)).unwrap();
    engine.ingest(&quote(&b, "49", "51", 10, at, 2)).unwrap();

    // Premise: nothing is quarantined and every value is defined.
    let clean = engine.evaluate(at).unwrap();
    assert!(engine.quarantined_features().is_empty());
    assert!(clean.is_complete(), "{:?}", clean.undefined());

    engine.quarantine(&a, at);
    let held = engine.evaluate(at).unwrap();
    assert_eq!(
        engine.quarantined_features(),
        [Mid::key(&a).canonical(), echo_key.canonical()]
            .into_iter()
            .collect(),
        "the quarantine did not reach exactly the instrument's features and their dependents"
    );
    assert_eq!(held.get(&Mid::key(&a)), Some(FeatureValue::Undefined));
    assert_eq!(held.get(&echo_key), Some(FeatureValue::Undefined));
    assert_eq!(
        held.get(&Mid::key(&b)),
        clean.get(&Mid::key(&b)),
        "an instrument nobody quarantined lost its feature"
    );

    assert_eq!(engine.lift_quarantine(&a), Some(at));
    let lifted = engine.evaluate(at).unwrap();
    assert!(engine.quarantined_features().is_empty());
    assert_eq!(lifted.get(&echo_key), clean.get(&echo_key));
}

/// R12-TICK-2062: a lead/lag correlation asserts that one instrument's move
/// preceded another's. Where the two venues' clocks are doubted by more than
/// the gap between the samples, that order is one the clocks cannot support,
/// and the feature is undefined rather than computed off the bucket grid.
/// The same tape under a doubt smaller than the gap computes as before.
///
/// Mutation: make the order test in `LeadLagCorrelation::compute` accept
/// every pair (treat every alignment as `Before`) — the doubted engine
/// computes a correlation and the first assertion fails.
#[test]
fn cross_instrument_samples_within_combined_clock_uncertainty_yield_no_aligned_pair() {
    let follower = object("FOLU");
    let leader = object("LEDU");
    let bids = [
        "100.00", "101.00", "99.00", "102.00", "100.00", "103.00", "98.00", "101.00", "104.00",
        "100.50",
    ];
    let start = Timestamp::from_secs(1_700_000_000);
    let ask_of = |bid: &str| {
        (Decimal::parse(bid).unwrap_or(Decimal::ZERO)
            + Decimal::parse("0.02").unwrap_or(Decimal::ZERO))
        .to_string()
    };
    // The tape is a follower that copies the leader one step (100 ms) late.
    let run = |uncertainty: Duration| {
        let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
        engine
            .register(Box::new(LeadLagCorrelation::new(
                follower.clone(),
                leader.clone(),
                5,
                1,
            )))
            .unwrap();
        let mut sequence = 0;
        for (step, bid) in bids.iter().enumerate() {
            let at = start.saturating_add(Duration::from_millis(100 * step as i64));
            let previous = if step == 0 { bid } else { &bids[step - 1] };
            sequence += 1;
            engine
                .ingest_placed(
                    &quote(&leader, bid, &ask_of(bid), 10, at, sequence),
                    uncertainty,
                )
                .unwrap();
            sequence += 1;
            engine
                .ingest_placed(
                    &quote(&follower, previous, &ask_of(previous), 10, at, sequence),
                    uncertainty,
                )
                .unwrap();
        }
        let last = start.saturating_add(Duration::from_millis(100 * (bids.len() as i64 - 1)));
        let vector = engine.evaluate(last).unwrap();
        vector.get(&LeadLagCorrelation::key(&follower, &leader, 5, 1))
    };

    // Each venue's stamp doubted by 60 ms: the 100 ms lead is inside the
    // 120 ms combined doubt, so no pair is ordered and nothing is asserted.
    let doubted = run(Duration::from_millis(60));
    assert!(
        matches!(
            doubted,
            Some(qip_contracts::feature::FeatureValue::Undefined)
        ),
        "a lead/lag correlation was asserted across samples the clocks cannot order, or the \
         feature was not evaluated at all: {doubted:?}"
    );

    // Doubted by 10 ms each: the lead stands clear of the 20 ms combined
    // doubt, and the feature computes the correlation the tape carries.
    let clear = run(Duration::from_millis(10)).expect("the feature is defined");
    close(clear, 1.0, "lead/lag correlation under a small clock doubt");
}
