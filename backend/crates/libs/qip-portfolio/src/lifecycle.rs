//! The lifecycle of a position, independent of its lots.
//!
//! A position's lot ledger says how much is held; this says what the desk is
//! *doing* about it. The two answers can disagree — a position can be fully
//! hedged and flat in lots while still `Unwinding` in intent — so the state
//! is tracked explicitly rather than inferred from quantity each time
//! something asks. Every move between states is validated by [`transition`],
//! not assigned directly, so a `Closed` position can never be quietly walked
//! back to `Held` by a caller that only checked the happy path.
//!
//! [`transition`]: PositionLifecycle::transition

use qip_core::{Error, Result};
use serde::{Deserialize, Serialize};

/// Where a position sits in its life, from first lot to last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionLifecycle {
    /// The record exists; no lot has been confirmed yet.
    Opened,
    /// At least one lot is confirmed and open.
    Held,
    /// Held, but the desk has raised a concern against it (a limit breach,
    /// a stale mark, a reconciliation break) that has not yet been resolved.
    Flagged,
    /// The desk has decided to reduce the position to flat.
    Unwinding,
    /// The position no longer has a controlling owner or strategy — found in
    /// a reconciliation break rather than opened deliberately — and needs
    /// disposition before it can be unwound or closed.
    Orphaned,
    /// Flat, and done. Terminal: nothing may transition out of it.
    Closed,
}

impl PositionLifecycle {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::Held => "held",
            Self::Flagged => "flagged",
            Self::Unwinding => "unwinding",
            Self::Orphaned => "orphaned",
            Self::Closed => "closed",
        }
    }

    /// Move to `next`, refusing anything not on the legal-edge table.
    ///
    /// Written as one `matches!` so the legal moves read as a table, the same
    /// discipline as `Order::transition`. The refusals matter more than the
    /// permissions: a closed position that can be walked back to held by a
    /// late, mis-ordered report is a books-and-records break waiting to
    /// happen, and there is no reading of "closed" under which that is a
    /// clamp rather than a bug.
    pub fn transition(&self, next: Self) -> Result<Self> {
        let legal = matches!(
            (self, &next),
            (Self::Opened, Self::Held)
                | (Self::Opened, Self::Closed)
                | (Self::Held, Self::Flagged)
                | (Self::Held, Self::Closed)
                | (Self::Flagged, Self::Unwinding)
                | (Self::Flagged, Self::Orphaned)
                | (Self::Flagged, Self::Closed)
                | (Self::Unwinding, Self::Orphaned)
                | (Self::Unwinding, Self::Closed)
                | (Self::Orphaned, Self::Closed)
        );
        if !legal {
            return Err(Error::invalid(format!(
                "position lifecycle cannot move from {} to {}",
                self.as_str(),
                next.as_str()
            )));
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every edge named in `transition`'s table, and nothing else, is legal.
    /// Listed as pairs so a future edit that adds or removes an arm without
    /// updating this list is caught by the count as well as the content.
    const LEGAL_EDGES: &[(PositionLifecycle, PositionLifecycle)] = &[
        (PositionLifecycle::Opened, PositionLifecycle::Held),
        (PositionLifecycle::Opened, PositionLifecycle::Closed),
        (PositionLifecycle::Held, PositionLifecycle::Flagged),
        (PositionLifecycle::Held, PositionLifecycle::Closed),
        (PositionLifecycle::Flagged, PositionLifecycle::Unwinding),
        (PositionLifecycle::Flagged, PositionLifecycle::Orphaned),
        (PositionLifecycle::Flagged, PositionLifecycle::Closed),
        (PositionLifecycle::Unwinding, PositionLifecycle::Orphaned),
        (PositionLifecycle::Unwinding, PositionLifecycle::Closed),
        (PositionLifecycle::Orphaned, PositionLifecycle::Closed),
    ];

    const ALL_STATES: &[PositionLifecycle] = &[
        PositionLifecycle::Opened,
        PositionLifecycle::Held,
        PositionLifecycle::Flagged,
        PositionLifecycle::Unwinding,
        PositionLifecycle::Orphaned,
        PositionLifecycle::Closed,
    ];

    #[test]
    fn every_legal_edge_in_the_table_transitions_and_nothing_else_does() {
        // Assert the premise first: the table is neither empty nor the full
        // cross product, so this test can actually distinguish a table that
        // grew or shrank from one that stayed the same.
        assert!(!LEGAL_EDGES.is_empty());
        assert!(LEGAL_EDGES.len() < ALL_STATES.len() * ALL_STATES.len());

        for &from in ALL_STATES {
            for &to in ALL_STATES {
                let expect_legal = LEGAL_EDGES.contains(&(from, to));
                let outcome = from.transition(to);
                assert_eq!(
                    outcome.is_ok(),
                    expect_legal,
                    "{} -> {} was {:?}, expected legal = {expect_legal}",
                    from.as_str(),
                    to.as_str(),
                    outcome
                );
            }
        }
    }

    #[test]
    fn closing_from_every_reachable_state_is_legal_but_closed_admits_no_further_move() {
        // Premise: Closed is reachable from four distinct states.
        let sources = [
            PositionLifecycle::Opened,
            PositionLifecycle::Held,
            PositionLifecycle::Flagged,
            PositionLifecycle::Unwinding,
            PositionLifecycle::Orphaned,
        ];
        for state in sources {
            assert!(state.transition(PositionLifecycle::Closed).is_ok());
        }
        for &next in ALL_STATES {
            let outcome = PositionLifecycle::Closed.transition(next);
            if next == PositionLifecycle::Closed {
                continue;
            }
            assert!(
                outcome.is_err(),
                "closed refused nothing, but moved to {}",
                next.as_str()
            );
        }
    }

    #[test]
    fn a_refused_transition_names_both_states_in_its_message() {
        let outcome = PositionLifecycle::Closed.transition(PositionLifecycle::Held);
        let Err(err) = outcome else {
            panic!("expected Closed -> Held to be refused, got {outcome:?}");
        };
        let message = format!("{err:?}");
        assert!(message.contains("closed"));
        assert!(message.contains("held"));
    }
}
