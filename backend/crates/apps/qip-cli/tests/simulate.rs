//! `qip simulate --shard k/n` (R12-TICK-9036): n sharded replay runs are
//! the unsharded run, cut along instruments — no fill lost, none doubled.

#![allow(clippy::panic_in_result_fn)]

use qip_cli::simulate::{ReplaySet, Shard, ShardRun, run, split};
use qip_contracts::message::{BookSide, MarketMessage, MessageBody, TradeCondition};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_simulation_engine::MessageBudget;
use qip_simulation_engine::tick_replay::{
    ExecutionModel, LegPlan, MultiLegPlan, OrderSide, ReplayExecution, ReplayOrder, ReplayTick,
    UnreliableSpan,
};
use std::collections::{BTreeMap, BTreeSet};

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_791_676_800_000 + ms)
}

fn d(v: i64) -> Decimal {
    Decimal::from_int(v)
}

fn tick(instrument: &str, sequence: u64, ms: i64, body: MessageBody) -> ReplayTick {
    ReplayTick {
        message: MarketMessage::new(
            ObjectId::from_string(instrument),
            Origin::new(
                VenueId::new("XLON"),
                format!("feed-{instrument}"),
                0,
                sequence,
            ),
            body,
            at(ms),
            at(ms),
        ),
        knowable_at: at(ms),
    }
}

fn level(side: BookSide, price: i64, quantity: i64) -> MessageBody {
    MessageBody::LevelSet {
        side,
        price: d(price),
        quantity: d(quantity),
        order_count: None,
    }
}

fn trade(price: i64, quantity: i64) -> MessageBody {
    MessageBody::Trade {
        price: d(price),
        quantity: d(quantity),
        condition: TradeCondition::Regular,
        aggressor: None,
    }
}

fn model() -> ExecutionModel {
    ExecutionModel {
        latency: Duration::from_millis(10),
        fee_bps: 2.0,
        impact_bps_at_full_depth: 20.0,
        limit_ttl: Duration::from_secs(60),
        message_budget: None,
    }
}

const INSTRUMENTS: [&str; 8] = [
    "obj-ACME", "obj-BOLT", "obj-CRUX", "obj-DUNE", "obj-EPIC", "obj-FERN", "obj-GLOW", "obj-HALO",
];

/// Eight instruments, each with a book, a sweep-shrinking ask, prints at the
/// bid, a market buy that pays the sweep and a resting bid that waits in the
/// queue — the queue-dependent fill being what a sharded run must keep.
fn replay_set() -> ReplaySet {
    let mut ticks = Vec::new();
    let mut orders = Vec::new();
    for (offset, instrument) in INSTRUMENTS.iter().enumerate() {
        let base = i64::try_from(offset).unwrap_or(0) * 3;
        ticks.push(tick(instrument, 1, base, level(BookSide::Bid, 99, 10)));
        ticks.push(tick(instrument, 2, base + 1, level(BookSide::Ask, 101, 5)));
        ticks.push(tick(instrument, 3, base + 2, level(BookSide::Ask, 102, 5)));
        // A one-message gap on every other instrument, so spans are part of
        // what the union must reproduce.
        let next = if offset % 2 == 0 { 4 } else { 5 };
        ticks.push(tick(
            instrument,
            next,
            base + 30,
            level(BookSide::Ask, 101, 3),
        ));
        ticks.push(tick(instrument, next + 1, base + 40, trade(99, 6)));
        ticks.push(tick(instrument, next + 2, base + 50, trade(99, 5)));
        orders.push(ReplayOrder {
            id: format!("mkt-{instrument}"),
            instrument: (*instrument).to_string(),
            side: OrderSide::Buy,
            quantity: d(7),
            limit: None,
            decided_at: at(base + 5),
        });
        orders.push(ReplayOrder {
            id: format!("bid-{instrument}"),
            instrument: (*instrument).to_string(),
            side: OrderSide::Buy,
            quantity: d(2),
            limit: Some(d(99)),
            decided_at: at(base + 5),
        });
    }
    ReplaySet {
        model: model(),
        ticks,
        orders,
        plans: Vec::new(),
    }
}

fn by_order(executions: impl IntoIterator<Item = ReplayExecution>) -> Vec<ReplayExecution> {
    let mut all: Vec<_> = executions.into_iter().collect();
    all.sort_by(|a, b| a.order_id.cmp(&b.order_id));
    all
}

fn by_stream(spans: impl IntoIterator<Item = UnreliableSpan>) -> Vec<UnreliableSpan> {
    let mut all: Vec<_> = spans.into_iter().collect();
    all.sort_by(|a, b| a.stream.cmp(&b.stream));
    all
}

/// The failure this prevents: a shard key that is not a partition. If two
/// shards both claim an instrument its orders fill twice across the job and
/// the union books twice the volume; if no shard claims it, its fills
/// vanish. Either way every shard process exits zero.
///
/// Mutation: in `Shard::holds`, also admit the next shard's bucket
/// (`|| bucket(..) == self.index % self.count + 1`), so adjacent shards
/// overlap — the per-instrument ownership assertion fails naming the
/// instrument, and the union carries duplicate executions.
#[test]
fn the_union_of_n_sharded_replay_runs_equals_the_unsharded_run() -> Result<()> {
    let set = replay_set();
    let whole = run(&set, Shard::WHOLE)?;

    // Premise: the unsharded run did fill, queue and flag something.
    assert_eq!(whole.instruments.len(), INSTRUMENTS.len());
    assert_eq!(whole.run.executions.len(), INSTRUMENTS.len() * 2);
    assert!(
        whole
            .run
            .executions
            .iter()
            .any(|e| e.queue_ahead_on_arrival == Some(d(10)) && e.filled == d(1)),
        "premise: a resting bid waited behind the ten lots queued at 99"
    );
    assert!(
        !whole.run.spans.is_empty(),
        "premise: some stream had a gap"
    );

    for count in [2u32, 3, 4] {
        let runs: Vec<ShardRun> = (1..=count)
            .map(|index| run(&set, Shard::new(index, count)?))
            .collect::<Result<_>>()?;

        // Premise: the split actually split — more than one shard did work.
        let working = runs.iter().filter(|r| !r.instruments.is_empty()).count();
        assert!(
            working > 1,
            "premise: with {count} shards only {working} held any instrument"
        );

        // Every instrument is replayed by exactly one shard.
        let mut owners: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
        for shard_run in &runs {
            for instrument in &shard_run.instruments {
                owners
                    .entry(instrument.as_str())
                    .or_default()
                    .push(shard_run.shard.index());
            }
        }
        for instrument in INSTRUMENTS {
            assert_eq!(
                owners.get(instrument).map(Vec::len),
                Some(1),
                "{instrument} is owned by shards {:?} of {count}; it must be exactly one",
                owners.get(instrument)
            );
        }

        assert_eq!(
            runs.iter().map(|r| r.run.steps).sum::<u64>(),
            whole.run.steps,
            "the shards together replayed every tick once ({count} shards)"
        );
        assert_eq!(
            by_order(runs.iter().flat_map(|r| r.run.executions.clone())),
            by_order(whole.run.executions.clone()),
            "the shards' executions are the unsharded run's ({count} shards)"
        );
        assert_eq!(
            by_stream(runs.iter().flat_map(|r| r.run.spans.clone())),
            by_stream(whole.run.spans.clone()),
            "the shards' unreliable spans are the unsharded run's ({count} shards)"
        );
        let mut books = BTreeMap::new();
        for shard_run in &runs {
            for (instrument, book) in &shard_run.run.books {
                assert!(
                    books.insert(instrument.clone(), book.clone()).is_none(),
                    "{instrument}'s final book came from two shards"
                );
            }
        }
        assert_eq!(
            books, whole.run.books,
            "the shards' final books are the unsharded run's ({count} shards)"
        );
    }
    Ok(())
}

/// The failure this prevents: replaying a coupled input in pieces. A venue
/// budget counts orders across instruments and a plan's legs read each
/// other's books, so a shard of either is a different history, not a part
/// of the same one.
///
/// Mutation: delete the `message_budget.is_some()` refusal in `split` — the
/// budgeted set shards without complaint and the first assertion fails.
/// Mutation: delete the cross-shard plan refusal — the second fails.
#[test]
fn a_shard_of_a_set_whose_instruments_are_coupled_is_refused_and_the_whole_set_is_not() -> Result<()>
{
    let mut budgeted = replay_set();
    budgeted.model.message_budget = Some(MessageBudget {
        per_window: 100,
        window: Duration::from_secs(1),
    });
    assert!(
        split(&budgeted, Shard::WHOLE).is_ok(),
        "premise: the budgeted set replays whole"
    );
    let refusal = split(&budgeted, Shard::new(1, 2)?)
        .err()
        .map(|e| e.message().to_string())
        .unwrap_or_default();
    assert!(
        refusal.contains("message budget"),
        "a budgeted set was sharded: {refusal:?}"
    );

    // Two instruments that land in different shards of two.
    let first = INSTRUMENTS[0];
    let probe = Shard::new(1, 2)?;
    let other = INSTRUMENTS
        .iter()
        .copied()
        .find(|i| probe.holds(i) != probe.holds(first))
        .unwrap_or(first);
    assert_ne!(other, first, "premise: two instruments split across shards");
    let mut planned = replay_set();
    let leg = |instrument: &str| LegPlan {
        instrument: instrument.to_string(),
        side: OrderSide::Buy,
        quantity: d(1),
        limit: None,
        region: "europe".to_string(),
    };
    planned.plans.push(MultiLegPlan {
        id: "plan-x".to_string(),
        decided_at: at(5),
        legs: vec![leg(first), leg(other)],
        leg_interval: Duration::from_millis(1),
        max_path_decay_bps: None,
        region_reservations: BTreeMap::new(),
    });
    assert!(
        split(&planned, Shard::WHOLE).is_ok(),
        "premise: the plan replays whole"
    );
    let refusal = split(&planned, Shard::new(1, 2)?)
        .err()
        .map(|e| e.message().to_string())
        .unwrap_or_default();
    assert!(
        refusal.contains("plan-x"),
        "a plan spanning two shards was split: {refusal:?}"
    );
    Ok(())
}

/// Mutation: in `Shard::new`, accept `index == 0` — `0/4` parses and the
/// first assertion fails.
#[test]
fn a_shard_that_names_no_part_of_the_set_is_refused_rather_than_read_as_all_of_it() {
    assert!(Shard::parse("2/4").is_ok(), "premise: a real shard parses");
    let mut seen = BTreeSet::new();
    for text in ["0/4", "5/4", "1/0", "1/4097", "a/4", "1-4", "1/4/2"] {
        assert!(
            Shard::parse(text).is_err(),
            "{text} was accepted as a shard"
        );
        seen.insert(text);
    }
    assert_eq!(seen.len(), 7);
}
