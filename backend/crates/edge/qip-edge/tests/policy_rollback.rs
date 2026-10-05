//! REFLEX-065: the previous policy payload stays resident, so a bad one is
//! undone by an assignment and not by waiting for the centre to re-ship.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::policy::Slot;
use qip_contracts::policy::{Dispositions, PolicyPayload};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use std::collections::BTreeMap;

const CELL: &str = "london-1";
const KEY: &[u8] = b"a-cell-policy-key-for-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn cell() -> Result<Cell> {
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    Cell::new(
        CellConfig::new(CELL, "europe-west2").with_venue(VenueId::new("XLON")),
        features,
    )
}

/// A verified payload; `marked` gives it a dispositions slot, the one field
/// this suite can read back from the cell to tell the two payloads apart.
fn payload(sequence: u64, at: Timestamp, marked: bool, halted: bool) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, at);
    payload.halted = halted;
    if marked {
        payload.dispositions = Slot::produced(
            Dispositions {
                unwinds: BTreeMap::new(),
            },
            at,
        );
    }
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, at)
}

fn last_applied_sequence(cell: &Cell) -> Option<u64> {
    cell.journal()
        .entries()
        .iter()
        .rev()
        .find_map(|entry| match &entry.decision {
            Decision::PolicyApplied { sequence, .. } => Some(*sequence),
            _ => None,
        })
}

#[test]
fn a_rollback_restores_the_previous_payload_without_a_fetch_and_the_rolled_away_one_cannot_return()
-> Result<()> {
    let mut cell = cell()?;
    assert!(
        cell.roll_back_policy(t(1)).is_err(),
        "a cell that never applied a payload has nothing to return to"
    );
    cell.apply_policy(payload(1, t(0), false, false)?, t(0))?;
    assert!(
        cell.roll_back_policy(t(1)).is_err(),
        "one payload applied: there is no previous one"
    );
    cell.apply_policy(payload(2, t(10), true, false)?, t(10))?;
    // Premise: the two payloads are distinguishable through the cell.
    assert_eq!(cell.policy_sequence(), Some(2));
    assert!(cell.dispositions().is_some());

    assert_eq!(cell.roll_back_policy(t(11))?, 1);
    assert_eq!(cell.policy_sequence(), Some(1));
    assert!(
        cell.dispositions().is_none(),
        "the cell still serves the payload it was meant to leave"
    );
    assert_eq!(last_applied_sequence(&cell), Some(1));

    // One step deep, and the newer payload is not replayable.
    assert!(cell.roll_back_policy(t(12)).is_err());
    assert!(
        cell.apply_policy(payload(2, t(13), true, false)?, t(13))
            .is_err(),
        "the rolled-away sequence was applied again"
    );
    cell.apply_policy(payload(3, t(14), true, false)?, t(14))?;
    assert_eq!(cell.policy_sequence(), Some(3));
    Ok(())
}

#[test]
fn a_rollback_never_releases_a_halt_returns_to_one_or_serves_an_expired_payload() -> Result<()> {
    // Halted by the newer payload: the older one would release it.
    let mut halted = cell()?;
    halted.apply_policy(payload(1, t(0), false, false)?, t(0))?;
    halted.apply_policy(payload(2, t(10), false, true)?, t(10))?;
    assert!(halted.is_halted(), "premise: the newer payload halts");
    assert!(halted.roll_back_policy(t(11)).is_err());
    assert!(halted.is_halted());
    assert_eq!(halted.policy_sequence(), Some(2));

    // The previous payload is itself a halt.
    let mut into_halt = cell()?;
    into_halt.apply_policy(payload(1, t(0), false, true)?, t(0))?;
    into_halt.apply_policy(payload(2, t(10), false, false)?, t(10))?;
    assert!(into_halt.roll_back_policy(t(11)).is_err());
    assert_eq!(into_halt.policy_sequence(), Some(2));

    // The previous payload is past its validity window (300 s by default).
    let mut stale = cell()?;
    stale.apply_policy(payload(1, t(0), false, false)?, t(0))?;
    stale.apply_policy(payload(2, t(10), false, false)?, t(10))?;
    assert!(stale.roll_back_policy(t(1000)).is_err());
    assert_eq!(stale.policy_sequence(), Some(2));
    Ok(())
}
