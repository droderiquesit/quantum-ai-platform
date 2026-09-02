//! The durable trial book, as a composition root opens it.
//!
//! `StrategyFactory::new` charges holdout evaluations to an in-process book,
//! whose lifetime counts are this process's. Until `Platform::open_trial_book`
//! existed no composition root opened anything else, so every restart forgot
//! every family's lifetime trial count — and a parameter sweep split across
//! two runs was two small sweeps as far as the deflated Sharpe gate could
//! tell, which is the laundering cumulative accounting exists to refuse.
//! These tests pin the three properties the wiring rests on: a book reopened
//! from the same store carries the count forward, a journal that does not
//! verify refuses to open rather than restarting at zero, and a plane
//! swapped in after the book was opened keeps it.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::signal::StrategyId;
use qip_core::Context;
use qip_core::error::{Error, Result};
use qip_core::kv::KeyValueStore;
use qip_core::time::Timestamp;
use qip_financial::universe::Universe;
use qip_kernel::central::CentralPlane;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_lifecycle::trials::{JOURNAL_PREFIX, StrategyFamily, TrialBook};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const STORE: &str = "trial-book";
const FAMILY: &str = "fam-momentum";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

/// The smallest store the port admits, so the book's durability can be
/// exercised without depending on an adapter crate.
#[derive(Debug, Default)]
struct MemoryStore(Mutex<BTreeMap<String, serde_json::Value>>);

impl MemoryStore {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, serde_json::Value>>> {
        self.0
            .lock()
            .map_err(|_| Error::io("the test store's lock is poisoned"))
    }
}

impl KeyValueStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        Ok(self.lock()?.get(key).cloned())
    }

    fn put(&self, key: &str, value: serde_json::Value) -> Result<()> {
        self.lock()?.insert(key.to_string(), value);
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<bool> {
        Ok(self.lock()?.remove(key).is_some())
    }

    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .lock()?
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }

    fn len(&self) -> Result<usize> {
        Ok(self.lock()?.len())
    }
}

fn family() -> Result<StrategyFamily> {
    StrategyFamily::new(FAMILY)
}

/// A store carrying one family with `trials` charged to it, written by a
/// book that is then dropped — what a previous run of the process leaves.
fn store_after_a_run_of(trials: usize) -> Result<Arc<MemoryStore>> {
    let store = Arc::new(MemoryStore::default());
    let mut book = TrialBook::open(store.clone())?;
    let family = family()?;
    book.open_family(&family, start())?;
    let strategy = StrategyId::new("momentum-0001");
    book.enrol(&strategy, &family, start())?;
    let account = book.charge(&strategy, trials, start())?;
    assert_eq!(
        account.lifetime(),
        trials as u64,
        "the fixture's charge did not land"
    );
    Ok(store)
}

/// The lifetime count the platform's factory would correct a gate against.
fn lifetime_known_to(platform: &Platform) -> Result<Option<u64>> {
    Ok(platform
        .central()
        .factory()
        .ledger()
        .trial_book()
        .and_then(|book| book.lifetime_trials(&family().ok()?)))
}

#[test]
fn a_book_reopened_from_the_same_store_carries_the_familys_lifetime_count_forward() -> Result<()> {
    let store = store_after_a_run_of(7)?;
    // Premise: the store really holds a journal, and the platform as built
    // knows nothing of the family — so a count found below came from the
    // store and not from the in-process default.
    assert!(!store.keys_with_prefix(JOURNAL_PREFIX)?.is_empty());
    let mut platform = platform()?;
    assert_eq!(lifetime_known_to(&platform)?, None);

    platform.open_trial_book(store, STORE)?;

    let book = platform
        .central()
        .factory()
        .ledger()
        .trial_book()
        .ok_or_else(|| Error::not_found("the factory's trial book"))?;
    assert!(book.is_durable(), "the book attached is the in-process one");
    assert_eq!(
        lifetime_known_to(&platform)?,
        Some(7),
        "the restart forgot the family's lifetime count"
    );
    Ok(())
}

#[test]
fn a_journal_whose_count_was_lowered_by_hand_refuses_to_open_and_nothing_is_attached() -> Result<()>
{
    let store = store_after_a_run_of(7)?;
    // Lower the last record's lifetime total in place: the hash no longer
    // covers what the record says, which is the one edit that would make a
    // sweep look smaller than it was.
    let keys = store.keys_with_prefix(JOURNAL_PREFIX)?;
    let last = keys
        .last()
        .ok_or_else(|| Error::not_found("a journal record"))?;
    let mut record = store
        .get(last)?
        .ok_or_else(|| Error::not_found("the last journal record"))?;
    let before = record["lifetime_after"].as_u64();
    assert_eq!(
        before,
        Some(7),
        "the premise is a record carrying the charge"
    );
    record["lifetime_after"] = serde_json::Value::from(1_u64);
    store.put(last, record)?;
    // Premise: the tampered store still opens as a plain store, so the
    // refusal below is the chain's and not an I/O failure.
    assert_eq!(store.keys_with_prefix(JOURNAL_PREFIX)?.len(), keys.len());

    let mut platform = platform()?;
    let refused = platform
        .open_trial_book(store, STORE)
        .expect_err("a journal that does not verify was opened");
    let message = refused.message();
    assert!(
        message.contains(&format!("`{STORE}`")),
        "the refusal does not name the store to restore: {message}"
    );
    assert!(
        message.contains(FAMILY),
        "the refusal does not name the family whose journal broke: {message}"
    );
    // And the factory was left with the book it had, not a half-opened one:
    // it still knows nothing of the family.
    assert_eq!(lifetime_known_to(&platform)?, None);
    Ok(())
}

#[test]
fn a_plane_swapped_in_after_the_book_was_opened_keeps_the_durable_book() -> Result<()> {
    let store = store_after_a_run_of(3)?;
    let mut platform = platform()?;
    platform.open_trial_book(store, STORE)?;
    // Premise: the durable book is attached before the swap.
    assert_eq!(lifetime_known_to(&platform)?, Some(3));

    let hardened = CentralPlane::new(
        b"an-operator-key-of-thirty-two-bytes",
        platform.config().central.clone(),
    )?;
    platform.set_central(hardened);

    let book = platform
        .central()
        .factory()
        .ledger()
        .trial_book()
        .ok_or_else(|| Error::not_found("the factory's trial book after the swap"))?;
    assert!(
        book.is_durable(),
        "the swapped-in plane brought the in-process book and the count is this run's again"
    );
    assert_eq!(lifetime_known_to(&platform)?, Some(3));
    Ok(())
}
