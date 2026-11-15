//! The node's one reader of the peer link's rebalancing tasks (R12-CAPITAL-5067).
//!
//! `PeerStatus::rebalance_new` is the one field of the status report that a
//! read consumes. Before this module the scheduler read `port.status()` twice
//! a turn, once for telemetry and once for the health body, and each read
//! drained the tasks the previous read left, so a task raised by a closed
//! epoch with drift reached a caller that never looked at it and was gone.
//! The scheduler now reads the status once a turn and hands the tasks here;
//! everything else reads the same copy.
//!
//! The keeper is bounded like the port's own queue: a task past the bound is
//! counted, never held, so a node whose journal consumer is slow cannot grow
//! without limit on a drifting book's account.

use qip_peer_link::boundary::RebalanceTask;
use std::collections::VecDeque;

/// Tasks kept before the oldest is released. The same figure as the port's
/// own task queue (`qip-peer-link`'s `TASK_QUEUE`), so the keeper holds what
/// the port could have held rather than a second, unrelated limit.
pub const KEPT_TASKS: usize = 64;

/// Rebalancing tasks the node has read and not yet handed on.
#[derive(Debug, Default)]
pub struct RebalanceKept {
    kept: VecDeque<RebalanceTask>,
    absorbed: u64,
    released_unread: u64,
}

impl RebalanceKept {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the tasks one status read delivered, oldest first. When the keeper
    /// is full the oldest kept task is released and counted: the newest drift
    /// is the one still true.
    pub fn absorb(&mut self, tasks: Vec<RebalanceTask>) {
        for task in tasks {
            self.absorbed = self.absorbed.saturating_add(1);
            if self.kept.len() == KEPT_TASKS {
                self.kept.pop_front();
                self.released_unread = self.released_unread.saturating_add(1);
            }
            self.kept.push_back(task);
        }
    }

    /// Hand every kept task on, oldest first, leaving none.
    pub fn take(&mut self) -> Vec<RebalanceTask> {
        self.kept.drain(..).collect()
    }

    /// Tasks waiting to be handed on.
    pub fn pending(&self) -> usize {
        self.kept.len()
    }

    /// Tasks ever absorbed, kept or not.
    pub fn absorbed(&self) -> u64 {
        self.absorbed
    }

    /// Tasks released because the keeper was full.
    pub fn released_unread(&self) -> u64 {
        self.released_unread
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::Decimal;

    fn task(place: &str, sequence: u64) -> RebalanceTask {
        RebalanceTask {
            place: place.to_string(),
            amount: Decimal::from_int(1),
            opportunity: "opp".to_string(),
            epoch_sequence: sequence,
        }
    }

    /// A task read once is kept until taken, and taken exactly once: the
    /// failure is a second reader that finds nothing because the first one
    /// consumed it.
    #[test]
    fn a_task_is_kept_until_taken_and_taken_once() {
        let mut kept = RebalanceKept::new();
        kept.absorb(vec![task("a", 1), task("b", 1)]);
        assert_eq!(kept.pending(), 2);
        let taken = kept.take();
        assert_eq!(
            taken.iter().map(|t| t.place.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(kept.take().is_empty(), "a task was delivered twice");
        assert_eq!(kept.absorbed(), 2);
    }

    /// The bound holds: one past it releases the oldest and counts it.
    #[test]
    fn the_keeper_is_bounded_and_counts_what_it_releases() {
        let mut kept = RebalanceKept::new();
        let over = (KEPT_TASKS as u64) + 3;
        kept.absorb((0..over).map(|n| task("a", n)).collect());
        assert_eq!(kept.pending(), KEPT_TASKS);
        assert_eq!(kept.released_unread(), 3);
        let taken = kept.take();
        assert_eq!(
            taken[0].epoch_sequence, 3,
            "the oldest was not the one released"
        );
        assert_eq!(taken.last().map(|t| t.epoch_sequence), Some(over - 1));
    }
}
