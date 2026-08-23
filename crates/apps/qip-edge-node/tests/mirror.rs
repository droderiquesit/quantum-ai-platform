//! The durability seam: the cell's journal reaching a configured store.
//!
//! The cell's own tests already show that the journal chains. What is tested
//! here is what happens at the boundary — that a shipped batch is readable
//! after the process that wrote it is gone, that a restart appends beside the
//! previous session instead of over it, and that a batch with a hole in it is
//! refused rather than written.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_edge::journal::{Decision, Journal, Mirror, MirrorBatch, ship};
use qip_edge_node::mirror::{StoreMirror, batches};
use qip_storage::kv::KeyValueStore;
use qip_storage::settings::StorageSettings;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> std::path::PathBuf {
    let unique = DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-edge-mirror-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the test fixture directory is creatable");
    dir
}

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_secs(seconds)
}

/// A durable store rooted in a fresh directory, opened the way the node opens
/// it: through the same settings an operator configures.
fn store(root: &std::path::Path) -> Result<Arc<dyn KeyValueStore>> {
    StorageSettings::from_values(Some("engine"), root.to_str())?.key_value("cell-journal")
}

/// A journal holding `count` decisions, each distinct so the chain is real.
fn journal_of(count: usize, from: i64) -> Journal {
    let mut journal = Journal::new();
    for n in 0..count {
        journal.record(
            Decision::Refused {
                gate: format!("gate-{n}"),
                reason: "the test needs a decision that is not a trade".to_string(),
            },
            at(from + n as i64),
        );
    }
    journal
}

// --- the record outlives the process ----------------------------------------

#[test]
fn a_shipped_journal_is_readable_after_the_process_that_wrote_it_is_gone() -> Result<()> {
    let root = temp_dir("outlives");
    let mut journal = journal_of(3, 1_000);

    {
        let mut mirror = StoreMirror::open(store(&root)?, "cell-a", at(1_000))?;
        assert_eq!(
            ship(&mut journal, &mut mirror, "cell-a", Vec::new(), at(1_010))?,
            3
        );
        assert_eq!(mirror.shipped_entries(), 3);
    }

    // A different process, against the same root.
    let recovered = batches(store(&root)?.as_ref())?;
    assert_eq!(recovered.len(), 1, "the batch did not survive the mirror");
    assert_eq!(recovered[0].entries.len(), 3);
    assert_eq!(recovered[0].cell, "cell-a");
    recovered[0]
        .verify_against(Journal::GENESIS)
        .expect("the recovered batch must still verify against the chain it claims");
    Ok(())
}

#[test]
fn a_second_session_appends_beside_the_first_rather_than_over_it() -> Result<()> {
    let root = temp_dir("sessions");

    // The premise: both sessions start their journals from the same genesis
    // digest, so a key scheme that ignored the session would collide. A cell
    // rebuilds its books from the feed on every start, which is exactly why
    // its journal begins again rather than continuing.
    let mut first = journal_of(2, 1_000);
    let mut second = journal_of(2, 2_000);
    assert_eq!(first.entries()[0].sequence, second.entries()[0].sequence);

    {
        let mut mirror = StoreMirror::open(store(&root)?, "cell-a", at(1_000))?;
        ship(&mut first, &mut mirror, "cell-a", Vec::new(), at(1_010))?;
    }
    {
        let mut mirror = StoreMirror::open(store(&root)?, "cell-a", at(2_000))?;
        assert_eq!(
            mirror.retained_sessions()?,
            1,
            "the previous session was not retained"
        );
        ship(&mut second, &mut mirror, "cell-a", Vec::new(), at(2_010))?;
        assert_eq!(mirror.retained_sessions()?, 2);
    }

    let recovered = batches(store(&root)?.as_ref())?;
    assert_eq!(recovered.len(), 2, "the second session overwrote the first");
    let entries: usize = recovered.iter().map(|batch| batch.entries.len()).sum();
    assert_eq!(entries, 4);
    // In session order: the earlier session's batch comes back first.
    assert_eq!(recovered[0].at, at(1_010));
    assert_eq!(recovered[1].at, at(2_010));
    Ok(())
}

#[test]
fn many_batches_in_one_session_come_back_in_the_order_the_cell_made_them() -> Result<()> {
    // Zero-padded batch numbers. Unpadded, batch 10 would sort before batch 9
    // and a reader replaying the record would see the cell's decisions in an
    // order it never made them in.
    let root = temp_dir("ordering");
    let mut mirror = StoreMirror::open(store(&root)?, "cell-a", at(1_000))?;
    let mut journal = Journal::new();
    for n in 0..12i64 {
        journal.record(
            Decision::Refused {
                gate: format!("gate-{n}"),
                reason: "one decision per batch".to_string(),
            },
            at(1_000 + n),
        );
        ship(
            &mut journal,
            &mut mirror,
            "cell-a",
            Vec::new(),
            at(1_100 + n),
        )?;
    }
    assert_eq!(mirror.shipped_batches(), 12);
    // The engine permits one writer per directory, so the session's handle is
    // released before the record is read back — which is also how a reader at
    // the centre would come to it.
    drop(mirror);

    let recovered = batches(store(&root)?.as_ref())?;
    assert_eq!(recovered.len(), 12);
    let sequences: Vec<u64> = recovered
        .iter()
        .flat_map(|batch| batch.entries.iter().map(|entry| entry.sequence))
        .collect();
    assert_eq!(
        sequences,
        (0..12).collect::<Vec<u64>>(),
        "the batches came back out of order"
    );
    Ok(())
}

// --- refusals ---------------------------------------------------------------

#[test]
fn a_batch_that_does_not_follow_its_predecessor_is_refused_rather_than_written() -> Result<()> {
    let root = temp_dir("gap");
    let store = store(&root)?;
    let mut mirror = StoreMirror::open(store.clone(), "cell-a", at(1_000))?;

    let mut journal = journal_of(4, 1_000);
    ship(&mut journal, &mut mirror, "cell-a", Vec::new(), at(1_010))?;

    // The premise: a batch that *does* chain onto that one is accepted, so
    // what is refused below is the hole and not every second batch.
    journal.record(
        Decision::HaltChanged {
            halted: true,
            reason: "the test needs a fifth entry".to_string(),
        },
        at(1_005),
    );
    ship(&mut journal, &mut mirror, "cell-a", Vec::new(), at(1_020))?;
    assert_eq!(mirror.shipped_batches(), 2);

    // Now a batch claiming to be the first of the session, as it would look if
    // everything between it and the last shipped batch had been lost.
    let orphan = MirrorBatch {
        cell: "cell-a".to_string(),
        at: at(1_030),
        chains_onto: Journal::GENESIS.to_string(),
        entries: journal_of(1, 9_000).entries().to_vec(),
        watermarks: Vec::new(),
    };
    let error = mirror
        .ship(orphan)
        .expect_err("a batch with entries missing before it must not be written");
    assert!(
        error.message().contains("chains onto"),
        "the error should say what did not line up: {}",
        error.message()
    );

    assert_eq!(
        batches(store.as_ref())?.len(),
        2,
        "the refused batch was written anyway"
    );
    Ok(())
}

#[test]
fn a_store_already_holding_another_cells_record_is_refused() -> Result<()> {
    // Two cells sharing one storage root interleave their decisions, and a
    // record that cannot say which cell made a decision is not an audit trail.
    let root = temp_dir("two-cells");
    let mut mirror = StoreMirror::open(store(&root)?, "cell-a", at(1_000))?;
    ship(
        &mut journal_of(2, 1_000),
        &mut mirror,
        "cell-a",
        Vec::new(),
        at(1_010),
    )?;
    drop(mirror);

    // The premise: the same cell reopens the same store without complaint, so
    // the refusal below is about the identity and not about reopening.
    StoreMirror::open(store(&root)?, "cell-a", at(2_000))
        .expect("the same cell must be able to reopen its own record");

    let error = StoreMirror::open(store(&root)?, "cell-b", at(2_000))
        .expect_err("a second cell must not append to the first cell's record");
    assert!(
        error.message().contains("cell-a") && error.message().contains("cell-b"),
        "the error should name both cells: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_batch_from_the_wrong_cell_is_refused_even_when_it_chains() -> Result<()> {
    // The chain check alone would pass this: an empty mirror expects genesis,
    // and this batch chains onto genesis. What is wrong with it is whose it is.
    let root = temp_dir("wrong-cell");
    let mut mirror = StoreMirror::open(store(&root)?, "cell-a", at(1_000))?;
    let mut journal = journal_of(2, 1_000);

    // The premise: shipped under the right cell name it is accepted.
    let mut accepted = journal.clone();
    ship(&mut accepted, &mut mirror, "cell-a", Vec::new(), at(1_010))?;

    let mut fresh = StoreMirror::open(store(&temp_dir("wrong-cell-2"))?, "cell-a", at(1_000))?;
    let error = ship(&mut journal, &mut fresh, "cell-b", Vec::new(), at(1_010))
        .expect_err("a batch labelled with another cell must not be shipped");
    assert!(
        error.message().contains("cell-b") && error.message().contains("cell-a"),
        "{}",
        error.message()
    );
    Ok(())
}

// --- what a probe can see ---------------------------------------------------

#[test]
fn the_mirror_reports_enough_for_a_probe_to_see_the_journal_failing_to_drain() -> Result<()> {
    // The health surface publishes the journal's length beside this count. A
    // cell whose entries climb while nothing ships is a cell whose record is
    // not leaving the process, and that is invisible if only one of the two
    // numbers is published.
    let root = temp_dir("draining");
    let mut mirror = StoreMirror::open(store(&root)?, "cell-a", at(1_000))?;
    assert_eq!(mirror.shipped_entries(), 0);
    assert_eq!(mirror.shipped_batches(), 0);

    let mut journal = journal_of(5, 1_000);
    assert_eq!(journal.len(), 5, "the premise: the journal holds entries");
    assert_eq!(mirror.shipped_entries(), 0, "recording is not shipping");

    ship(&mut journal, &mut mirror, "cell-a", Vec::new(), at(1_010))?;
    assert_eq!(mirror.shipped_entries(), 5);
    assert_eq!(mirror.shipped_batches(), 1);

    // A flush with nothing pending ships nothing and reports nothing new.
    ship(&mut journal, &mut mirror, "cell-a", Vec::new(), at(1_020))?;
    assert_eq!(mirror.shipped_entries(), 5);
    assert_eq!(mirror.shipped_batches(), 1);
    Ok(())
}

#[test]
fn a_mirror_with_a_store_reports_nothing_outstanding() {
    // `required_configuration` is what the node prints as "awaiting". A mirror
    // holding a store that preflighted has nothing left to be given, and
    // saying otherwise would put a permanent warning in a healthy node's log.
    let root = temp_dir("configured");
    let mirror = StoreMirror::open(store(&root).expect("the store opens"), "cell-a", at(1_000))
        .expect("the mirror opens");
    assert!(mirror.required_configuration().is_empty());
}
