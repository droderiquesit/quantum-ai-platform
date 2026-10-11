//! REFLEX-8071: the operator's rollback request, as the node reads it.
//!
//! The cell already refuses the rollbacks that would ease a control (a
//! centre halt, a return to a halt, an expired payload — `qip-edge`'s
//! `policy_rollback` suite). What these tests hold is the node's half: a
//! request acts only on the sequence it names, once, and a file that is not
//! one sequence is never read as a request.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::policy::PolicyPayload;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::rollback::{RollbackOutcome, RollbackReading, RollbackRequest, RollbackWire};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const CELL: &str = "london-1";
const KEY: &[u8] = b"a-cell-policy-key-for-tests";
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn request_path(label: &str) -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-rollback-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory can be created");
    dir.join("rollback")
}

fn cell_serving(sequences: &[(u64, i64)]) -> Result<Cell> {
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(
        CellConfig::new(CELL, "europe-west2").with_venue(VenueId::new("XLON")),
        features,
    )?;
    for &(sequence, at) in sequences {
        let payload = PolicyPayload::unproduced(sequence, CELL, t(at));
        let verified = VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, t(at))?;
        cell.apply_policy(verified, t(at))?;
    }
    Ok(cell)
}

/// A wire over `path` whose reader has published at least once.
fn wire(path: &std::path::Path) -> Result<RollbackWire> {
    let wire = RollbackWire::spawn(
        RollbackRequest::at(path)?,
        std::time::Duration::from_millis(2),
    )?;
    let waited = std::time::Instant::now();
    while wire.latest().is_none() {
        assert!(waited.elapsed() < PATIENCE, "the reader never published");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    Ok(wire)
}

/// Wait until the reader has published `expected`, so a test about what
/// `apply` does is not a test about thread timing.
fn published(wire: &RollbackWire, expected: &RollbackReading) {
    let waited = std::time::Instant::now();
    while wire.latest().as_ref() != Some(expected) {
        assert!(
            waited.elapsed() < PATIENCE,
            "the reader never published {expected:?}; last {:?}",
            wire.latest()
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Mutation: drop the `cell.policy_sequence() != Some(from)` check in
/// `RollbackWire::apply` and the stale request rolls sequence 3 back to 2 —
/// undoing the very correction the centre shipped after the request.
#[test]
fn a_request_left_on_disk_after_the_centre_moved_on_never_undoes_the_newer_payload() -> Result<()> {
    let path = request_path("stale");
    std::fs::write(&path, "2").expect("the request can be written");
    let mut cell = cell_serving(&[(1, 0), (2, 10), (3, 20)])?;
    let mut wire = wire(&path)?;
    published(&wire, &RollbackReading::AwayFrom(2));
    // Premise: the cell holds a previous payload it could return to.
    assert_eq!(cell.policy_sequence(), Some(3));

    assert_eq!(wire.apply(&mut cell, t(21)), None);
    assert_eq!(cell.policy_sequence(), Some(3));

    // The same wire, now naming the sequence the cell serves, does act.
    std::fs::write(&path, "3").expect("the request can be rewritten");
    published(&wire, &RollbackReading::AwayFrom(3));
    assert_eq!(
        wire.apply(&mut cell, t(22)),
        Some(RollbackOutcome::RolledBack { from: 3, to: 2 })
    );
    assert_eq!(cell.policy_sequence(), Some(2));
    Ok(())
}

/// Mutation: drop the digits-only guard in `rollback::parse` and `"+2"` —
/// which `u64::from_str` accepts — reads as a request for sequence 2.
#[test]
fn a_file_that_is_not_one_sequence_is_never_read_as_a_request() -> Result<()> {
    for (content, label) in [
        ("2 then 3", "two-numbers"),
        ("rollback", "word"),
        ("", "empty"),
        ("-2", "negative"),
        ("+2", "signed"),
    ] {
        let path = request_path(label);
        std::fs::write(&path, content).expect("the request can be written");
        let mut cell = cell_serving(&[(1, 0), (2, 10)])?;
        let mut wire = wire(&path)?;
        let reading = wire.latest().expect("published");
        assert!(
            matches!(reading, RollbackReading::Unreadable(_)),
            "{content:?} read as {reading:?}"
        );
        assert_eq!(wire.apply(&mut cell, t(11)), None, "{content:?} acted");
        assert_eq!(
            cell.policy_sequence(),
            Some(2),
            "{content:?} moved the cell"
        );
    }
    Ok(())
}

/// A refusal is reported once, not on every turn the file stays in place,
/// and it leaves the cell exactly where it was: the wire adds no way around
/// the cell's own refusal to roll back past a halt.
///
/// Mutation: drop `self.acted_on = Some(from)` in `RollbackWire::apply` and
/// the second call reports the refusal again.
#[test]
fn a_refused_rollback_is_reported_once_and_leaves_the_halt_in_place() -> Result<()> {
    let path = request_path("halted");
    std::fs::write(&path, "2").expect("the request can be written");
    let mut cell = cell_serving(&[(1, 0)])?;
    let mut halted = PolicyPayload::unproduced(2, CELL, t(10));
    halted.halted = true;
    cell.apply_policy(
        VerifiedPolicy::verify(halted.signed(KEY)?, KEY, CELL, t(10))?,
        t(10),
    )?;
    assert!(cell.is_halted(), "premise: sequence 2 halts the cell");
    let mut wire = wire(&path)?;
    published(&wire, &RollbackReading::AwayFrom(2));

    assert!(matches!(
        wire.apply(&mut cell, t(11)),
        Some(RollbackOutcome::Refused { from: 2, .. })
    ));
    assert!(cell.is_halted());
    assert_eq!(wire.apply(&mut cell, t(12)), None);
    assert_eq!(cell.policy_sequence(), Some(2));
    Ok(())
}

#[test]
fn a_relative_or_empty_request_path_is_refused_at_configuration() {
    assert!(RollbackRequest::at("").is_err());
    assert!(RollbackRequest::at("relative/rollback").is_err());
    assert!(RollbackRequest::at("/var/run/qip/rollback").is_ok());
}
