//! The warm standby and its fencing lease, proven on the node's own seams
//! (R12-REFLEX-8069, R12-REFLEX-9117, R12-RES-9166, R12-RES-9167).
//!
//! The property is "never two active owners". It is asserted two ways: over
//! every interleaving of renewal, promotion, a partitioned primary and the
//! passage of time a small model admits, at most one fence admits an order
//! at any instant; and on two assembled nodes run through `run_pass`
//! against their own simulated venues, a standby places nothing until it
//! has promoted and the primary it replaced places nothing afterwards.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::deployment::Deployment;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, SystemClock, dec};
use qip_edge::cell::{CellConfig, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::fencing::{Fence, FenceStatus, GATE_FENCED};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::standby::{
    FileLeaseStore, LeaseEvent, LeaseKeeper, LeaseRecord, LeaseStore, LeaseTiming,
    MemoryLeaseStore, Role, SimulatedSessions, StandbySettings, UNPAIRED_ROLE, VenueSessions,
};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn timing() -> LeaseTiming {
    LeaseTiming::deployed().expect("the deployed timing is valid")
}

fn venue() -> VenueId {
    VenueId::new("XLON")
}

// --- the model ---------------------------------------------------------------

/// A store one keeper can be cut off from while the other still reaches it:
/// the partition that makes a primary keep running while it cannot renew.
#[derive(Debug)]
struct Partitionable {
    inner: Arc<MemoryLeaseStore>,
    down: Arc<AtomicBool>,
}

impl LeaseStore for Partitionable {
    fn load(&self) -> Result<Option<(LeaseRecord, u64)>> {
        if self.down.load(Ordering::Relaxed) {
            return Err(Error::io("partitioned from the lease store"));
        }
        self.inner.load()
    }
    fn swap(&self, expected: Option<u64>, next: &LeaseRecord) -> Result<bool> {
        if self.down.load(Ordering::Relaxed) {
            return Err(Error::io("partitioned from the lease store"));
        }
        self.inner.swap(expected, next)
    }
}

#[derive(Clone, Copy, Debug)]
enum Step {
    TickPrimary,
    TickStandby,
    /// Less than a ttl: the primary can still renew in time.
    Short,
    /// A whole ttl plus the margin: any hold not renewed meanwhile lapses.
    Long,
    /// Cut the primary off from the store, or reconnect it.
    TogglePartition,
}

const STEPS: [Step; 5] = [
    Step::TickPrimary,
    Step::TickStandby,
    Step::Short,
    Step::Long,
    Step::TogglePartition,
];

/// Run one interleaving; return the first instant at which the property
/// failed, described, or the epoch the store holds at the end.
fn run(sequence: &[Step]) -> std::result::Result<Option<u64>, String> {
    let store = Arc::new(MemoryLeaseStore::new());
    let down = Arc::new(AtomicBool::new(false));
    let primary_fence = Fence::new();
    let standby_fence = Fence::new();
    let mut primary = LeaseKeeper::new(
        "primary",
        Role::Primary,
        timing(),
        Arc::new(Partitionable {
            inner: Arc::clone(&store),
            down: Arc::clone(&down),
        }),
        primary_fence.clone(),
        vec![venue()],
        Box::new(SimulatedSessions),
    )
    .expect("a keeper");
    let mut standby = LeaseKeeper::new(
        "standby",
        Role::Standby,
        timing(),
        Arc::clone(&store) as Arc<dyn LeaseStore>,
        standby_fence.clone(),
        vec![venue()],
        Box::new(SimulatedSessions),
    )
    .expect("a keeper");
    let mut now = t(0);
    for (index, step) in sequence.iter().enumerate() {
        match step {
            Step::TickPrimary => {
                let _ = primary.tick(now);
            }
            Step::TickStandby => {
                let _ = standby.tick(now);
            }
            Step::Short => now = now.saturating_add(Duration::from_secs(1)),
            Step::Long => now = now.saturating_add(Duration::from_secs(5)),
            Step::TogglePartition => {
                let was = down.load(Ordering::Relaxed);
                down.store(!was, Ordering::Relaxed);
            }
        }
        let admitting: Vec<(&str, u64)> =
            [("primary", &primary_fence), ("standby", &standby_fence)]
                .into_iter()
                .filter_map(|(name, fence)| fence.admits(now).ok().map(|epoch| (name, epoch)))
                .collect();
        if admitting.len() > 1 {
            return Err(format!(
                "after step {index} of {sequence:?}, both fences admit: {admitting:?}"
            ));
        }
        if let Some((name, epoch)) = admitting.first() {
            let stored = store.load().ok().flatten().map(|(record, _)| record);
            if stored.as_ref().map(|record| record.epoch) != Some(*epoch)
                || stored.as_ref().map(|record| record.holder.as_str()) != Some(*name)
            {
                return Err(format!(
                    "after step {index} of {sequence:?}, {name} admits under epoch {epoch} and \
                     the store holds {stored:?}"
                ));
            }
        }
    }
    Ok(store.load().ok().flatten().map(|(record, _)| record.epoch))
}

/// A store that lands another process's write between a keeper's read and
/// its swap: the race the interleaving model above cannot produce, because a
/// keeper's tick there loads and swaps with nothing in between.
#[derive(Debug)]
struct Racing {
    inner: Arc<MemoryLeaseStore>,
    interloper: std::sync::Mutex<Option<LeaseRecord>>,
}

impl LeaseStore for Racing {
    fn load(&self) -> Result<Option<(LeaseRecord, u64)>> {
        let read = self.inner.load()?;
        if let Some(next) = self.interloper.lock().expect("unpoisoned").take() {
            let version = read.as_ref().map(|(_, version)| *version);
            assert!(
                self.inner.swap(version, &next)?,
                "premise: the interloper's write landed"
            );
        }
        Ok(read)
    }
    fn swap(&self, expected: Option<u64>, next: &LeaseRecord) -> Result<bool> {
        self.inner.swap(expected, next)
    }
}

/// A keeper whose read went stale before its swap loses the swap and is
/// granted nothing: a holder's renewal does not overwrite the successor that
/// promoted in between, and a standby's promotion does not overwrite a
/// holder that renewed in between. Added by the round-2 mutation audit: the
/// interleaving test's doc named "an unconditional renewal write" as a
/// mutation it caught, and it survived — there is no gap between load and
/// swap in that model. Mutation: swap the renewal (or the promotion) against
/// a freshly re-read version instead of the one the decision was made on,
/// and the stale keeper's write lands over the other's.
#[test]
fn a_keeper_whose_read_went_stale_loses_the_swap_and_is_granted_nothing() {
    let timing = timing();
    // Renewal half.
    let inner = Arc::new(MemoryLeaseStore::new());
    let racing = Arc::new(Racing {
        inner: Arc::clone(&inner),
        interloper: std::sync::Mutex::new(None),
    });
    let fence = Fence::new();
    let mut primary = LeaseKeeper::new(
        "primary",
        Role::Primary,
        timing,
        Arc::clone(&racing) as Arc<dyn LeaseStore>,
        fence.clone(),
        vec![venue()],
        Box::new(SimulatedSessions),
    )
    .expect("a keeper");
    assert_eq!(
        primary.tick(t(0)).expect("a store"),
        LeaseEvent::Acquired { epoch: 1 }
    );
    let successor = LeaseRecord {
        epoch: 2,
        holder: "standby".to_string(),
        renewed_at_ns: t(1).as_nanos(),
    };
    *racing.interloper.lock().expect("unpoisoned") = Some(successor.clone());
    assert_eq!(primary.tick(t(1)).expect("a store"), LeaseEvent::Contended);
    assert_eq!(
        inner.load().expect("readable").map(|(record, _)| record),
        Some(successor),
        "the stale renewal overwrote the successor"
    );
    assert!(
        fence.admits(t(0).saturating_add(timing.ttl)).is_err(),
        "the lost renewal extended the hold"
    );

    // Promotion half: the primary's hold has lapsed past the margin, and it
    // renews between the standby's read and its swap.
    let inner = Arc::new(MemoryLeaseStore::new());
    let holder = LeaseRecord {
        epoch: 1,
        holder: "primary".to_string(),
        renewed_at_ns: t(0).as_nanos(),
    };
    assert!(inner.swap(None, &holder).expect("writable"), "premise");
    let racing = Arc::new(Racing {
        inner: Arc::clone(&inner),
        interloper: std::sync::Mutex::new(None),
    });
    let fence = Fence::new();
    let mut standby = LeaseKeeper::new(
        "standby",
        Role::Standby,
        timing,
        Arc::clone(&racing) as Arc<dyn LeaseStore>,
        fence.clone(),
        vec![venue()],
        Box::new(SimulatedSessions),
    )
    .expect("a keeper");
    let late = t(0)
        .saturating_add(timing.ttl)
        .saturating_add(timing.margin)
        .saturating_add(Duration::from_secs(1));
    let renewed = LeaseRecord {
        renewed_at_ns: late.as_nanos(),
        ..holder
    };
    *racing.interloper.lock().expect("unpoisoned") = Some(renewed.clone());
    assert_eq!(standby.tick(late).expect("a store"), LeaseEvent::Contended);
    assert_eq!(
        inner.load().expect("readable").map(|(record, _)| record),
        Some(renewed),
        "the stale promotion overwrote the renewed holder"
    );
    assert!(
        fence.admits(late).is_err(),
        "the lost promotion was granted"
    );
}

/// For every interleaving of seven steps — renewal, standby ticks, time
/// shorter and longer than a hold, and a primary partitioned from the store
/// while still running — at most one fence admits an order at any instant,
/// and the one that does holds the epoch the store holds.
///
/// Mutation: delete the liveness check (`if now < deadline { … Standing … }`)
/// from `LeaseKeeper::tick` and a standby promotes while the primary's hold
/// is still valid — the first interleaving `TickPrimary, TickStandby` fails.
/// Replacing the renewal's `swap(Some(version), …)` with an unconditional
/// write is *not* caught here — each tick in this model loads and swaps with
/// nothing between, so the version is always current (round-2 mutation
/// audit); `a_keeper_whose_read_went_stale_loses_the_swap_and_is_granted_nothing`
/// is the guard on that compare-and-swap.
#[test]
fn at_most_one_fence_admits_at_any_instant_over_every_interleaving() {
    // Premise: the model can reach both a promotion and a renewal, so the
    // search is over runs in which each side actually held.
    let premise = [
        Step::TickPrimary,
        Step::TickStandby,
        Step::TogglePartition,
        Step::Long,
        Step::TickStandby,
    ];
    assert_eq!(
        run(&premise),
        Ok(Some(2)),
        "premise: a partitioned primary is replaced"
    );
    const LENGTH: usize = 7;
    let mut sequence = [Step::Short; LENGTH];
    let mut checked = 0u64;
    let mut counters = [0usize; LENGTH];
    loop {
        for (slot, counter) in sequence.iter_mut().zip(counters.iter()) {
            *slot = STEPS[*counter];
        }
        if let Err(failure) = run(&sequence) {
            panic!("{failure}");
        }
        checked += 1;
        // Odometer over `STEPS^LENGTH`.
        let mut position = 0;
        loop {
            if position == LENGTH {
                assert_eq!(checked, (STEPS.len() as u64).pow(LENGTH as u32));
                return;
            }
            counters[position] += 1;
            if counters[position] < STEPS.len() {
                break;
            }
            counters[position] = 0;
            position += 1;
        }
    }
}

/// A standby that starts before any primary has held never takes a lease
/// nobody has failed over from.
///
/// Mutation: let the `None` arm of `LeaseKeeper::tick` acquire for a standby
/// and the epoch below is 1.
#[test]
fn a_standby_never_takes_a_lease_no_primary_has_held() -> Result<()> {
    let store: Arc<dyn LeaseStore> = Arc::new(MemoryLeaseStore::new());
    let fence = Fence::new();
    let mut standby = LeaseKeeper::new(
        "standby",
        Role::Standby,
        timing(),
        Arc::clone(&store),
        fence.clone(),
        vec![venue()],
        Box::new(SimulatedSessions),
    )?;
    for second in [0, 10, 100, 1_000] {
        assert_eq!(standby.tick(t(second))?, LeaseEvent::AwaitingPrimary);
    }
    assert!(store.load()?.is_none(), "the standby wrote a lease");
    assert_eq!(fence.status(t(1_000)), FenceStatus::Unowned);
    Ok(())
}

#[derive(Debug)]
struct StillLoggedOn;

impl VenueSessions for StillLoggedOn {
    fn released(&self, _venue: &VenueId, _holder: &str, _now: Timestamp) -> Result<bool> {
        Ok(false)
    }
}

/// Liveness alone is not enough: while a venue still reports the old
/// holder's session, the standby does not promote however long it waits.
///
/// Mutation: skip the venue-session loop in `LeaseKeeper::tick` and the
/// standby promotes to epoch 2.
#[test]
fn a_standby_waits_while_any_venue_still_reports_the_old_holders_session() -> Result<()> {
    let store: Arc<dyn LeaseStore> = Arc::new(MemoryLeaseStore::new());
    let mut primary = LeaseKeeper::new(
        "primary",
        Role::Primary,
        timing(),
        Arc::clone(&store),
        Fence::new(),
        vec![venue()],
        Box::new(SimulatedSessions),
    )?;
    assert_eq!(primary.tick(t(0))?, LeaseEvent::Acquired { epoch: 1 });
    let fence = Fence::new();
    let mut standby = LeaseKeeper::new(
        "standby",
        Role::Standby,
        timing(),
        Arc::clone(&store),
        fence.clone(),
        vec![venue()],
        Box::new(StillLoggedOn),
    )?;
    // Premise: before the deadline it is liveness that holds it back.
    assert!(matches!(standby.tick(t(1))?, LeaseEvent::Standing { .. }));
    let late = standby.tick(t(60))?;
    assert_eq!(
        late,
        LeaseEvent::AwaitingSessions {
            holder: "primary".to_string(),
            venues: vec!["XLON".to_string()],
        }
    );
    assert_eq!(store.load()?.map(|(record, _)| record.epoch), Some(1));
    assert!(fence.admits(t(60)).is_err());
    Ok(())
}

// --- two running nodes -------------------------------------------------------

const ENVELOPE_KEY: &[u8] = b"standby-test-envelope-key";
const STRATEGY: &str = "always-enter";

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn firing_strategy() -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(dec!("10")),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn grant(cell: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            cell,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    // Bounded as the centre's issuer bounds every grant (R12-CAPITAL-4088):
    // the cell `assemble` builds refuses an envelope without exposure bounds.
    let unsigned = build("")?;
    let capital = unsigned.gross_limit();
    let unsigned = unsigned.with_exposure(qip_contracts::capital::ExposureBounds::bounded(
        2_500,
        qip_core::Decimal::ONE,
        capital,
    )?)?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), ENVELOPE_KEY, cell, t(1))
}

fn share(cell: &str, named: String) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, cell, t(5));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec![named],
        },
        t(5),
    );
    VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, cell, t(5))
}

/// One process of the pair: the same cell id, its own simulated venue, a
/// fence installed as `main.rs` installs it.
fn process(fence: &Fence) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    const CELL: &str = "london-1";
    let config = CellConfig::new(CELL, "europe-west2").with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut node = assemble(
        config,
        features,
        Arc::new(SystemClock),
        RegionCapital::read(Some("1000000000"))?,
        None,
    )?;
    node.cell.install_fence(fence.clone());
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let (compiled, program) = firing_strategy()?;
    node.cell.deploy_with_pricing(
        compiled,
        program,
        grant(CELL)?,
        // Marketable, so every order fills against the seeded touch and the
        // strategy forms a fresh one on every pass: a pass that sends
        // nothing is then the fence's doing, not a resting order's.
        PricingPolicy::Marketable,
    )?;
    let named = grant(CELL)?.signature().to_string();
    node.cell.apply_policy(share(CELL, named)?, t(5))?;
    Ok((node, gateway, feed))
}

fn pass(
    node: &mut NodeAssembly,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    at: Timestamp,
) -> Result<PassOutcome> {
    let mut stats = PassStats::default();
    run_pass(&mut node.cell, gateway, feed, None, &mut stats, at)
}

fn fenced_refusals(node: &NodeAssembly) -> usize {
    node.cell
        .journal()
        .entries()
        .iter()
        .filter(|entry| {
            matches!(&entry.decision, Decision::Refused { gate, .. } if gate == GATE_FENCED)
        })
        .count()
}

/// The drill §14.3 and §17 describe, on two assembled nodes: the standby
/// runs warm and places nothing; the primary stops renewing; the standby
/// promotes only once the primary's hold has lapsed by the margin, records
/// the per-venue recovery it measured, and places; the old primary, back,
/// finds itself fenced and places nothing.
///
/// Mutation: delete `self.fence_admit(now)?;` from `Cell::send` and the
/// standby's venue receives an order on its first pass.
#[test]
fn a_standby_places_nothing_until_it_promotes_and_the_primary_it_replaced_places_nothing_after()
-> Result<()> {
    let store: Arc<dyn LeaseStore> = Arc::new(MemoryLeaseStore::new());
    let (primary_fence, standby_fence) = (Fence::new(), Fence::new());
    let mut primary_keeper = LeaseKeeper::new(
        "primary",
        Role::Primary,
        timing(),
        Arc::clone(&store),
        primary_fence.clone(),
        vec![venue()],
        Box::new(SimulatedSessions),
    )?;
    let mut standby_keeper = LeaseKeeper::new(
        "standby",
        Role::Standby,
        timing(),
        Arc::clone(&store),
        standby_fence.clone(),
        vec![venue()],
        Box::new(SimulatedSessions),
    )?;
    let (mut primary, mut primary_venue, mut primary_feed) = process(&primary_fence)?;
    let (mut standby, mut standby_venue, mut standby_feed) = process(&standby_fence)?;

    assert_eq!(
        primary_keeper.tick(t(10))?,
        LeaseEvent::Acquired { epoch: 1 }
    );
    assert!(matches!(
        standby_keeper.tick(t(10))?,
        LeaseEvent::Standing { epoch: 1, .. }
    ));

    // Premise: the primary places under the same code the standby runs.
    let placed = pass(&mut primary, &mut primary_venue, &mut primary_feed, t(10))?;
    let PassOutcome::Ran { report, .. } = placed else {
        panic!("premise: the primary's pass ran");
    };
    assert_eq!(report.orders.len(), 1, "premise: the primary placed");
    assert_eq!(primary_venue.submitted_count(), 1);

    // The standby runs warm and places nothing.
    let warm = pass(&mut standby, &mut standby_venue, &mut standby_feed, t(10));
    assert!(warm.is_err(), "a fenced order fails the pass: {warm:?}");
    assert_eq!(
        standby_venue.submitted_count(),
        0,
        "a standby holding no epoch placed an order"
    );
    assert_eq!(fenced_refusals(&standby), 1);

    // The primary stops renewing at t(10). Its hold lapses at t(13); the
    // standby may promote at t(15) and not before.
    assert!(matches!(
        standby_keeper.tick(t(14))?,
        LeaseEvent::Standing { .. }
    ));
    let LeaseEvent::Promoted(promotion) = standby_keeper.tick(t(15))? else {
        panic!("the standby did not promote once the hold had lapsed by the margin");
    };
    assert_eq!(promotion.epoch, 2);
    assert_eq!(promotion.from, "primary");
    assert_eq!(promotion.lapsed_at, t(13));
    assert_eq!(
        promotion.recovery.get("XLON"),
        Some(&Duration::from_secs(2)),
        "recovery is measured per venue from the lapse"
    );

    let promoted = pass(&mut standby, &mut standby_venue, &mut standby_feed, t(16))?;
    let PassOutcome::Ran { report, .. } = promoted else {
        panic!("the promoted standby's pass did not run");
    };
    assert_eq!(report.orders.len(), 1, "the promoted standby places");
    assert_eq!(standby_venue.submitted_count(), 1);

    // The old primary returns. Its hold lapsed on its own clock, so it
    // places nothing even before its keeper reads the store…
    let before = primary_venue.submitted_count();
    let refused_before = fenced_refusals(&primary);
    let _ = pass(&mut primary, &mut primary_venue, &mut primary_feed, t(16));
    assert_eq!(
        fenced_refusals(&primary),
        refused_before + 1,
        "the lapsed primary formed an order and the fence did not refuse it"
    );
    // …and once it has, it is fenced for good.
    assert_eq!(primary_keeper.tick(t(16))?, LeaseEvent::Fenced { by: 2 });
    assert!(matches!(
        primary_fence.status(t(17)),
        FenceStatus::Fenced { by: 2, .. }
    ));
    let _ = pass(&mut primary, &mut primary_venue, &mut primary_feed, t(17));
    assert_eq!(
        fenced_refusals(&primary),
        refused_before + 2,
        "the fenced primary formed an order and the fence did not refuse it"
    );
    assert_eq!(
        primary_venue.submitted_count(),
        before,
        "a fenced primary placed an order"
    );
    Ok(())
}

/// The file store is a compare-and-swap: of two writers acting on the same
/// reading, one lands.
///
/// Mutation: drop the `current != expected` comparison in
/// `FileLeaseStore::swap` and the second `swap(None, …)` lands too.
#[test]
fn the_file_lease_store_lands_exactly_one_of_two_writes_from_one_reading() -> Result<()> {
    let dir = std::env::temp_dir().join(format!(
        "qip-lease-{}-{}",
        std::process::id(),
        SystemClock.now().as_nanos()
    ));
    std::fs::create_dir_all(&dir).map_err(|error| Error::io(error.to_string()))?;
    let path = dir.join("lease.json");
    let first = FileLeaseStore::at(&path)?;
    let second = FileLeaseStore::at(&path)?;
    let record = |holder: &str| LeaseRecord {
        epoch: 1,
        holder: holder.to_string(),
        renewed_at_ns: t(0).as_nanos(),
    };
    assert!(first.load()?.is_none(), "premise: an empty store");
    assert!(first.swap(None, &record("a"))?);
    assert!(
        !second.swap(None, &record("b"))?,
        "a second write on the same reading landed"
    );
    let (held, version) = second
        .load()?
        .expect("the first write is visible to the second");
    assert_eq!(held.holder, "a");
    assert!(second.swap(Some(version), &record("b"))?);
    assert!(!first.swap(Some(version), &record("a"))?);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn a_role_or_name_without_a_lease_is_refused_rather_than_run_as_a_second_primary() {
    assert!(matches!(
        StandbySettings::read(None, None, None, Deployment::Local),
        Ok(None)
    ));
    for (role, name) in [
        (Some("standby"), None),
        (None, Some("b")),
        (Some("standby"), Some("b")),
    ] {
        let error = StandbySettings::read(None, role, name, Deployment::Local)
            .expect_err("no lease is refused");
        assert!(
            error.message().starts_with("configuration:"),
            "{}",
            error.message()
        );
    }
    assert!(
        StandbySettings::read(Some("/l"), Some("leader"), Some("a"), Deployment::Local).is_err()
    );
    assert!(StandbySettings::read(Some("/l"), None, Some("a"), Deployment::Local).is_err());
    assert!(StandbySettings::read(Some("/l"), Some("primary"), None, Deployment::Local).is_err());
    let settings = StandbySettings::read(Some("/l"), Some("standby"), Some("b"), Deployment::Local)
        .expect("well formed")
        .expect("configured");
    assert_eq!(settings.role, Role::Standby);
}

/// The failure this prevents: with `standby_enabled`, the execution-node
/// module boots the standby from the same startup script as the primary,
/// with no `QIP_FENCING_LEASE` and no role — indistinguishable from a lone
/// primary, so it traded as a second one and every order went out twice.
/// Mutation: drop the deployed-and-no-role refusal in
/// `StandbySettings::read` and the role-less deployed node is admitted as
/// alone; this fails.
#[test]
fn a_deployed_node_that_does_not_say_it_runs_alone_is_refused_rather_than_run_as_a_second_primary()
{
    // Premise: on a developer's machine absence still means alone.
    assert!(matches!(
        StandbySettings::read(None, None, None, Deployment::Local),
        Ok(None)
    ));
    for deployment in [
        Deployment::Dev,
        Deployment::Test,
        Deployment::Stage,
        Deployment::Prod,
    ] {
        let error = StandbySettings::read(None, None, None, deployment)
            .expect_err("a deployed node with no role was run as alone");
        assert!(
            error.message().contains("QIP_NODE_ROLE is not set")
                && error.message().contains(UNPAIRED_ROLE),
            "the refusal does not name the variable and the value to set: {}",
            error.message()
        );
        // Saying so is enough for a node with no standby …
        assert!(matches!(
            StandbySettings::read(None, Some(UNPAIRED_ROLE), None, deployment),
            Ok(None)
        ));
        // … and a pair still needs the lease on both nodes.
        assert!(StandbySettings::read(None, Some("standby"), Some("b"), deployment).is_err());
        assert!(
            StandbySettings::read(Some("/l"), Some("standby"), Some("b"), deployment)
                .expect("a configured standby")
                .is_some()
        );
    }
    // `unpaired` beside a lease is a contradiction, refused rather than
    // resolved in either direction.
    assert!(
        StandbySettings::read(Some("/l"), Some(UNPAIRED_ROLE), Some("a"), Deployment::Prod)
            .is_err()
    );
    assert!(StandbySettings::read(None, Some(UNPAIRED_ROLE), Some("a"), Deployment::Prod).is_err());
}
