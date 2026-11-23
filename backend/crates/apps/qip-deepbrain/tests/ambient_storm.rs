//! An ambient storm against the research loop as the binary builds it
//! (R12-RES-7107): `ResearchLoop::new` on `ResearchConfig::default`, the
//! constructor `main.rs` reaches through `new_for_node`, swept the way
//! `node::run_with` sweeps it between cycles.
//!
//! The storm here is the one a deployment actually meets: a standing
//! coverage gap the sweep re-raises every time it runs. Until 2026-11-23 the
//! deployed router neither clustered such a repeat nor kept a record of its
//! producer, so every re-raise was a fresh activation and a detector that
//! said one thing every sweep spent the hour's attention saying it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::ambient::{AttentionEvent, Disposition};
use qip_core::{Context, Duration, Timestamp};
use qip_deepbrain::research::{ResearchConfig, ResearchLoop, ResearchRound};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::collections::{BTreeMap, BTreeSet};

fn t0() -> Timestamp {
    Timestamp::from_secs(1_800_000_000)
}

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(t0(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

/// A loop whose coverage map names an object nothing covers, so every sweep
/// raises the same coverage gap again.
fn standing_gap_loop() -> ResearchLoop {
    let coverage = BTreeMap::from([("Equity".to_string(), set(&["obj-A"]))]);
    ResearchLoop::new(ResearchConfig::default(), coverage).unwrap()
}

fn sweep(research: &mut ResearchLoop, platform: &Platform, now: Timestamp) -> ResearchRound {
    research
        .sweep(platform, now, &set(&[]), &set(&["US"]))
        .unwrap()
}

/// The producer an event names: the detector its trigger carries.
fn producer(event: &AttentionEvent) -> &str {
    event.originating_models[0].as_str()
}

fn of<'a>(round: &'a ResearchRound, name: &str) -> Vec<&'a AttentionEvent> {
    round
        .attention_events
        .iter()
        .filter(|e| producer(e) == name)
        .collect()
}

/// The first sweep activates the gap; the second raises it again inside its
/// signal's life and the router records it `Clustered`, charging nothing.
/// Mutation: drop `.clustering_duplicates()` from `ResearchLoop::new_holding`
/// — the repeat is `Activated` and the budget spent rises.
#[test]
fn a_standing_gap_raised_again_is_clustered_and_spends_no_attention() {
    let platform = platform();
    let mut research = standing_gap_loop();
    let first = sweep(&mut research, &platform, t0());
    let activated: Vec<&AttentionEvent> = first
        .attention_events
        .iter()
        .filter(|e| e.disposition == Disposition::Activated)
        .collect();
    assert!(
        !activated.is_empty(),
        "premise: the first sweep activates the gap: {:?}",
        first.attention_events
    );
    let name = producer(activated[0]).to_string();
    let spent_after_first = first.attention_events.last().unwrap().budget.spent;

    let second = sweep(
        &mut research,
        &platform,
        t0().saturating_add(Duration::from_mins(1)),
    );
    let repeats = of(&second, &name);
    assert!(
        !repeats.is_empty(),
        "premise: the gap is raised again: {:?}",
        second.attention_events
    );
    for event in &repeats {
        assert_eq!(
            event.disposition,
            Disposition::Clustered,
            "a repeat of a live gap was not clustered: {event:?}"
        );
        assert_eq!(
            event.budget.spent, spent_after_first,
            "a clustered repeat spent attention: {event:?}"
        );
    }
}

/// Each repeat lowers its producer's reputation, and once the producer has
/// raised `producer_quarantine_min_signals` signals with its reputation
/// under `producer_quarantine_floor_bp` it is quarantined: its next signal
/// is recorded `Quarantined` and reaches nothing. The sweeps are ten seconds
/// apart so every repeat falls inside the first signal's one-hour life.
/// Mutations: drop `.quarantining_producers(..)` from
/// `ResearchLoop::new_holding` — no reputation is kept and the first
/// assertion fails; change `producer_quarantine_floor_bp` to 1 in
/// `ResearchConfig::default` — reputation still falls but the producer is
/// never quarantined.
#[test]
fn a_producer_whose_signals_are_all_repeats_loses_reputation_and_is_quarantined() {
    let config = ResearchConfig::default();
    let min_signals = config.producer_quarantine_min_signals;
    let floor_bp = config.producer_quarantine_floor_bp;
    assert!(
        min_signals > 1 && floor_bp > 0,
        "premise: the committed quarantine judges a producer on a record"
    );
    let platform = platform();
    let mut research = standing_gap_loop();
    let first = sweep(&mut research, &platform, t0());
    let name = first
        .attention_events
        .iter()
        .find(|e| e.disposition == Disposition::Activated)
        .map(|e| producer(e).to_string())
        .unwrap_or_else(|| panic!("premise: an activation: {:?}", first.attention_events));
    assert_eq!(of(&first, &name).len(), 1, "premise: one gap per sweep");
    let mut reputation = research
        .producer_reputation_bp(&name)
        .expect("the router keeps a record of the producer");
    assert_eq!(reputation, 10_000, "an activated signal is not waste");

    let mut quarantined_at = None;
    for step in 1..=i64::from(min_signals) + 1 {
        let now = t0().saturating_add(Duration::from_secs(10 * step));
        let round = sweep(&mut research, &platform, now);
        if research.is_producer_quarantined(&name) {
            quarantined_at = Some((step, round));
            break;
        }
        let next = research.producer_reputation_bp(&name).unwrap();
        assert!(
            next < reputation,
            "reputation did not fall on repeat {step}: {reputation} then {next}"
        );
        reputation = next;
    }
    let (step, _) = quarantined_at
        .unwrap_or_else(|| panic!("never quarantined; reputation ended at {reputation}"));
    assert!(
        step + 1 >= i64::from(min_signals),
        "quarantined after {} signals, fewer than the {min_signals} the record needs",
        step + 1
    );
    assert!(
        research.producer_reputation_bp(&name).unwrap() < floor_bp,
        "quarantined above the floor"
    );

    let after = sweep(
        &mut research,
        &platform,
        t0().saturating_add(Duration::from_secs(10 * (step + 1))),
    );
    let stopped = of(&after, &name);
    assert!(!stopped.is_empty(), "premise: the producer raised again");
    assert!(
        stopped
            .iter()
            .all(|e| e.disposition == Disposition::Quarantined),
        "a quarantined producer's signal routed: {stopped:?}"
    );
}
