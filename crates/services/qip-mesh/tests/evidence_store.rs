//! The L0 immutable evidence layer.
//!
//! The tests try to revise the record: overwriting a key, and rewriting one
//! through the file-backed adapter after a restart. The one thing that must
//! work is the honest retry — writing identical bytes again after an
//! ambiguous failure.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_mesh::ports::EvidenceStore;
use qip_mesh::{FileEvidence, MemoryEvidence};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A directory of this test's own, since these tests write real files.
fn scratch(label: &str) -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "qip-mesh-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    path
}

const KEY: &str = "decisions/2026-08-22/d-4711";

#[test]
fn a_second_write_of_different_bytes_is_refused_and_names_the_conflict() -> Result<()> {
    let evidence = MemoryEvidence::new();
    let first = evidence.put(KEY, b"the decision as taken".to_vec(), now())?;

    let error = evidence
        .put(KEY, b"the decision as we would prefer it".to_vec(), now())
        .expect_err("evidence is write-once");

    // The refusal has to identify the key and both digests, because the whole
    // value of the layer is being able to say what was there and what somebody
    // tried to replace it with.
    assert!(error.message().contains(KEY));
    assert!(error.message().contains(&first.digest[..16]));
    assert!(error.message().contains("write-once"));

    // And the original is untouched.
    let stored = evidence.get(KEY, now())?;
    assert_eq!(stored.value().as_deref(), Some(b"the decision as taken".as_slice()));
    Ok(())
}

#[test]
fn writing_identical_bytes_again_is_idempotent() -> Result<()> {
    // The retry-after-an-ambiguous-failure case. If this were an error every
    // writer would need a read-before-write, and the race that opens is worse
    // than the duplicate it prevents.
    let evidence = MemoryEvidence::new();
    let first = evidence.put(KEY, b"the decision".to_vec(), now())?;
    let second = evidence.put(KEY, b"the decision".to_vec(), now())?;

    assert_eq!(first, second);
    assert_eq!(evidence.keys("decisions/", now())?.value().len(), 1);
    Ok(())
}

#[test]
fn a_retry_at_a_later_time_keeps_the_original_write_time() -> Result<()> {
    // The receipt says when the evidence was created, not when somebody last
    // pressed the button. An incident timeline built from the later time would
    // be wrong by however long the retry took.
    let evidence = MemoryEvidence::new();
    let first = evidence.put(KEY, b"the decision".to_vec(), now())?;
    let retry = evidence.put(
        KEY,
        b"the decision".to_vec(),
        now().saturating_add(Duration::from_hours(2)),
    )?;

    assert_eq!(retry.written_at, first.written_at);
    Ok(())
}

#[test]
fn the_file_backed_store_is_still_write_once_after_a_restart() -> Result<()> {
    // The interesting case: the process that wrote the evidence is gone, and
    // the one that comes back must not be able to revise it either.
    let root = scratch("evidence");
    std::fs::create_dir_all(&root)?;
    let path = root.join("evidence.json");

    {
        let evidence = FileEvidence::open(&path)?;
        evidence.put(KEY, b"the decision as taken".to_vec(), now())?;
    }

    let reopened = FileEvidence::open(&path)?;
    let recovered = reopened.get(KEY, now())?;
    assert_eq!(
        recovered.value().as_deref(),
        Some(b"the decision as taken".as_slice())
    );

    let error = reopened
        .put(KEY, b"a revised account".to_vec(), now())
        .expect_err("a restart does not reset write-once");
    assert!(error.message().contains(KEY));

    // The honest retry still works across the restart.
    reopened.put(KEY, b"the decision as taken".to_vec(), now())?;

    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn a_receipt_proves_what_was_written_without_reading_it_back() -> Result<()> {
    let evidence = MemoryEvidence::new();
    let bytes = b"a long piece of evidence".to_vec();
    let receipt = evidence.put(KEY, bytes.clone(), now())?;

    assert_eq!(receipt.key, KEY);
    assert_eq!(receipt.size_bytes, bytes.len());
    assert_eq!(receipt.digest, qip_core::sha256_hex(&bytes));

    let read_back = evidence.receipt(KEY, now())?;
    assert_eq!(read_back.value().as_ref(), Some(&receipt));
    Ok(())
}

#[test]
fn keys_are_listed_by_prefix_and_only_those_already_written() -> Result<()> {
    let evidence = MemoryEvidence::new();
    let later = now().saturating_add(Duration::from_hours(1));
    evidence.put("decisions/a", b"a".to_vec(), now())?;
    evidence.put("decisions/b", b"b".to_vec(), later)?;
    evidence.put("models/c", b"c".to_vec(), now())?;

    assert_eq!(evidence.keys("decisions/", now())?.value(), &["decisions/a"]);
    assert_eq!(
        evidence.keys("decisions/", later)?.value(),
        &["decisions/a", "decisions/b"]
    );
    assert_eq!(evidence.keys("", later)?.value().len(), 3);
    Ok(())
}

#[test]
fn evidence_written_under_no_key_is_refused() {
    let evidence = MemoryEvidence::new();
    assert!(evidence.put("   ", b"orphan".to_vec(), now()).is_err());
}
