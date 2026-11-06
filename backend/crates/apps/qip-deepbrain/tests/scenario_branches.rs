//! Conditional scenario branches held across sweeps (R12-AMBIENT-1142),
//! reached from the deep brain's run loop: a risk surprise adds a branch,
//! a later tape re-shocks the same branch, and the branch is retired once
//! its signal expires, each change journaled, with no sweep called by hand.
//!
//! Until this suite the sweep rebuilt its scenarios from scratch every time,
//! so nothing was carried, re-shocked or retired; and the one scenario test
//! called `ResearchLoop::sweep` directly rather than the node loop.

use qip_core::{Clock, Context, Decimal, Duration, ManualClock, ObjectId, Timestamp};
use qip_deepbrain::ambient_journal::{AmbientJournal, AmbientRecord, JournalDestination, records};
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::evolution::{EvolutionConfig, EvolutionEngine};
use qip_deepbrain::node::{self, Loops};
use qip_deepbrain::research::{BranchChange, ResearchConfig, ResearchLoop, ScenarioBranchUpdate};
use qip_deepbrain::status::NodeStatus;
use qip_events::Topic;
use qip_financial::quality::{DataQuality, LicensingClass};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord, SourceDescriptor};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::{ChainArchive, MemoryKeyValueStore};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

const SUBJECT: &str = "obj-N";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn bar(at: Timestamp, close: f64) -> SensedRecord {
    let price = |v: f64| Decimal::from_f64(v).unwrap_or_else(|| panic!("a price"));
    SensedRecord::Bar(Box::new(Bar {
        object_id: ObjectId::from_string(SUBJECT.to_string()),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: price(close),
        high: price(close * 1.001),
        low: price(close * 0.999),
        close: price(close),
        volume: Decimal::from_int(1_000_000),
        trade_count: 5_000,
        vwap: None,
        quality: DataQuality::default(),
    }))
}

/// A feed that hands the node each scripted record once its instant has
/// come: the tape the test moves, and nothing else.
#[derive(Debug)]
struct Scripted {
    pending: Vec<(Timestamp, SensedRecord)>,
}

impl DataAdapter for Scripted {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            name: "scenario-branch-test".to_string(),
            provider: "the test's script".to_string(),
            licensing: LicensingClass::Synthetic,
            topics: vec![Topic::MarketBar],
            expected_latency: Duration::ZERO,
            production_requirement: None,
        }
    }

    fn poll(&mut self, until: Timestamp) -> qip_core::error::Result<Vec<SensedRecord>> {
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|(at, _)| *at <= until);
        self.pending = later;
        Ok(due.into_iter().map(|(_, record)| record).collect())
    }
}

fn updates_of(journaled: &[AmbientRecord]) -> Vec<(u64, ScenarioBranchUpdate)> {
    journaled
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::ScenarioBranch { cycle, update } => Some((*cycle, update.clone())),
            _ => None,
        })
        .collect()
}

/// R12-AMBIENT-1142: six daily cycles of the node's own loop. A 30% break
/// raises a risk signal, and its branch is added; a further move on a later
/// tape re-shocks that same branch; once the signal has expired the branch
/// is retired. Every change is
/// journaled. Mutation: clear the held branches at the start of each sweep,
/// and the re-shock and retirement never appear.
#[test]
fn a_risk_signals_scenario_branch_is_added_reshocked_on_the_next_tape_and_retired_at_expiry() {
    // A day bar is knowable at its close, so the node's first cycle runs a
    // day after the break opened.
    let manual = Arc::new(ManualClock::new(
        start().saturating_add(Duration::from_days(1)),
    ));
    let clock: Arc<dyn Clock> = manual.clone();
    let config = PlatformConfig::default();
    let seed = config.seed;
    let context = Context::new(clock.clone(), seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    // Thirty calm days, so the break is a surprise and not the tape's habit.
    let calm: Vec<SensedRecord> = (0..30_i64)
        .map(|i| {
            let close = if i % 2 == 0 { 100.2 } else { 99.8 };
            bar(start().saturating_sub(Duration::from_days(30 - i)), close)
        })
        .collect();
    platform.observe(calm);

    // The signal outlives one step and not two: live when the next bar
    // lands, expired by the cycle after.
    let ttl = Duration::from_hours(36);
    let step = Duration::from_days(1);
    // Each bar is handed over at its close, the instant it became knowable:
    // the break, a further move three days later, and one more the day
    // after, which lands while the branch the first two raised is still held.
    let at = |n: i64| start().saturating_add(Duration::from_days(n));
    let feed = Scripted {
        pending: vec![
            (at(1), bar(start(), 130.0)),
            (at(4), bar(at(3), 136.5)),
            (at(5), bar(at(4), 129.0)),
        ],
    };
    let mut engine = EvolutionEngine::new(
        EvolutionConfig {
            every_cycles: 1,
            minimum_bars: 2,
            candidates: 2,
            challengers: 1,
            ..EvolutionConfig::default()
        },
        Box::new(feed),
        seed,
        Universe::new(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let coverage = BTreeMap::from([(
        "Equity".to_string(),
        [SUBJECT.to_string()].into_iter().collect::<BTreeSet<_>>(),
    )]);
    let mut research = ResearchLoop::new(
        ResearchConfig {
            every_cycles: 1,
            signal_ttl: ttl,
            ..ResearchConfig::default()
        },
        coverage,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let mut ambient = AmbientJournal::open(&JournalDestination::InMemory)
        .unwrap_or_else(|e| panic!("the journal opens: {e}"));
    let node_config = DeepBrainConfig {
        cycle_interval: Duration::from_millis(1),
        max_cycles: Some(6),
        ..DeepBrainConfig::default()
    };
    let roster = qip_deepbrain::roster::clear(start()).unwrap_or_else(|e| panic!("{e}"));
    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &roster,
        &node_config,
        start(),
    )));
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
        .unwrap_or_else(|e| panic!("{e}"));
    let summary = node::run_with(
        &mut platform,
        &archive,
        &node_config,
        &status,
        &Arc::new(AtomicBool::new(false)),
        &clock,
        0,
        Some(&mut engine),
        Loops {
            research: Some(&mut research),
            ambient: Some(&mut ambient),
            ..Loops::default()
        },
        // Time passes between cycles; the test asks the loop for nothing.
        |_| manual.advance(step),
    )
    .unwrap_or_else(|e| panic!("the node runs: {e}"));
    assert_eq!(summary.cycles, 6, "premise: the node ran six cycles");

    assert!(
        ambient.log().verify_chain().is_ok(),
        "the journal's chain verifies"
    );
    let journaled = records(ambient.log()).unwrap_or_else(|e| panic!("{e}"));
    let updates = updates_of(&journaled);
    let added: Vec<&(u64, ScenarioBranchUpdate)> = updates
        .iter()
        .filter(|(_, u)| u.change == BranchChange::Added && u.signal_id.ends_with("/risk"))
        .collect();
    let (added_cycle, first) = added
        .first()
        .copied()
        .unwrap_or_else(|| panic!("premise: the break added a risk branch: {updates:?}"));
    let later = |change: BranchChange| {
        updates
            .iter()
            .find(|(cycle, u)| {
                u.branch_id == first.branch_id && u.change == change && cycle > added_cycle
            })
            .cloned()
    };
    let (reshocked_cycle, reshocked) = later(BranchChange::Reshocked).unwrap_or_else(|| {
        panic!("the same branch was not re-shocked on the new tape: {updates:?}")
    });
    assert_ne!(
        reshocked.input_digest, first.input_digest,
        "a re-shock stands on new inputs"
    );
    let (retired_cycle, _) = later(BranchChange::Retired).unwrap_or_else(|| {
        panic!("the branch was not retired at its signal's expiry: {updates:?}")
    });
    assert!(reshocked_cycle < retired_cycle, "{updates:?}");
    assert!(
        !research.scenario_branches().contains_key(&first.branch_id),
        "a retired branch is no longer held"
    );
}
