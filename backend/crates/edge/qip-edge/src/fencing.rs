//! The fencing epoch a cell places under (R12-REFLEX-8069, R12-RES-9166).
//!
//! A region's reflex cell may run as a primary and a warm standby in another
//! zone. Two processes that both believe they own the venue session place
//! every order twice, and nothing downstream can tell which copy was meant:
//! the reconciler sees two well-formed orders, each with a cell id. So the
//! property is "never two active owners", and it has to hold at the one seam
//! a `Placer` is called — not in the process that decides who owns the lease,
//! which can be partitioned from the store, paused by the scheduler, or
//! simply slow.
//!
//! [`Fence`] is that seam's half. Whoever keeps the lease — `qip-edge-node`'s
//! lease keeper, on a thread of its own because it does I/O — writes into it
//! the epoch it holds and the instant its hold lapses; [`Cell::send`] reads it
//! before every order and refuses under [`GATE_FENCED`] when the cell holds
//! no epoch, when the hold has lapsed, or when a higher epoch has been
//! observed. The lapse is what makes this safe against a keeper that cannot
//! reach its store: the hold runs out on the cell's own clock, whether or not
//! anybody tells it, and a standby promotes only after that instant plus a
//! margin, so the two windows in which either may place never overlap.
//!
//! A cell built without a fence places as it always did — that is a cell
//! with no standby, and `qip-edge-node` says so at start. Once installed, the
//! fence starts *unowned*: a process cannot place until its keeper has
//! actually acquired the lease, which is the restrictive default.
//!
//! [`Cell::send`]: crate::cell::Cell

use qip_core::Timestamp;
use std::sync::{Arc, Mutex};

/// The gate a cell refuses under when its fence does not admit an order. A
/// constant, so the `gate` label on `qip_edge_refusals_total` stays bounded.
pub const GATE_FENCED: &str = "fenced";

/// What the fence holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Hold {
    /// The epoch this process holds, if it has ever acquired one.
    epoch: Option<u64>,
    /// The instant the current hold lapses unless renewed.
    valid_until: Option<Timestamp>,
    /// The highest epoch observed held by somebody else.
    fenced_by: Option<u64>,
}

/// Where a fence stands at an instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenceStatus {
    /// No epoch has ever been granted: a standby, or a primary that has not
    /// yet acquired the lease.
    Unowned,
    /// The fence admits orders under `epoch` until `valid_until`.
    Holding { epoch: u64, valid_until: Timestamp },
    /// The hold was not renewed in time; the process may still hold the
    /// lease in the store, and may not place until it renews.
    Lapsed { epoch: u64, lapsed_at: Timestamp },
    /// A higher epoch has been observed: another process owns the session.
    Fenced { epoch: Option<u64>, by: u64 },
}

impl FenceStatus {
    pub fn admits(&self) -> bool {
        matches!(self, Self::Holding { .. })
    }

    /// One sentence for a refusal or a health body.
    pub fn describe(&self) -> String {
        match self {
            Self::Unowned => "this process holds no fencing epoch; it is a standby or has not yet \
                              acquired the lease, and a process that does not own the venue \
                              session places nothing"
                .to_string(),
            Self::Holding { epoch, valid_until } => format!(
                "holding fencing epoch {epoch} until {}",
                valid_until.to_rfc3339()
            ),
            Self::Lapsed { epoch, lapsed_at } => format!(
                "fencing epoch {epoch} lapsed at {} without a renewal; a standby may promote once \
                 the margin past it has run, so this process places nothing until it renews",
                lapsed_at.to_rfc3339()
            ),
            Self::Fenced { epoch, by } => format!(
                "fenced: epoch {by} is held by another process{}; two owners of one venue \
                 session would place every order twice",
                match epoch {
                    Some(own) => format!(" and this process held {own}"),
                    None => String::new(),
                }
            ),
        }
    }
}

/// The shared fencing state between a lease keeper and the cell it guards.
///
/// Cloning shares the state. A mutex rather than atomics because the three
/// fields must be read together: an epoch from one renewal beside a lapse
/// instant from another is a hold nobody granted. A poisoned lock reads as
/// fenced, never as holding.
#[derive(Clone, Debug, Default)]
pub struct Fence {
    hold: Arc<Mutex<Hold>>,
}

impl Fence {
    /// An unowned fence: admits nothing until [`Self::grant`].
    pub fn new() -> Self {
        Self::default()
    }

    /// The keeper acquired or renewed the lease under `epoch`, holding until
    /// `valid_until`.
    ///
    /// Refuses an epoch lower than one already held or already observed held
    /// by another process: a keeper going backwards is a keeper whose store
    /// has lost a write, and admitting it would be two owners.
    pub fn grant(&self, epoch: u64, valid_until: Timestamp) -> qip_core::error::Result<()> {
        let mut hold = self.hold.lock().map_err(|_| {
            qip_core::error::Error::denied("the fence's lock is poisoned; the cell stays fenced")
        })?;
        if hold.epoch.is_some_and(|held| epoch < held)
            || hold.fenced_by.is_some_and(|by| epoch <= by)
        {
            return Err(qip_core::error::Error::denied(format!(
                "fencing epoch {epoch} is not above what this process has held or seen held \
                 ({:?} held, {:?} seen); a lease that goes backwards is not granted",
                hold.epoch, hold.fenced_by
            )));
        }
        hold.epoch = Some(epoch);
        hold.valid_until = Some(valid_until);
        Ok(())
    }

    /// Another process holds `by`. Permanent for every epoch at or below it.
    pub fn fence(&self, by: u64) {
        if let Ok(mut hold) = self.hold.lock() {
            hold.fenced_by = Some(hold.fenced_by.map_or(by, |seen| seen.max(by)));
            hold.valid_until = None;
        }
    }

    /// Where the fence stands at `now`.
    pub fn status(&self, now: Timestamp) -> FenceStatus {
        let Ok(hold) = self.hold.lock() else {
            return FenceStatus::Fenced {
                epoch: None,
                by: u64::MAX,
            };
        };
        match (hold.epoch, hold.fenced_by) {
            (epoch, Some(by)) if epoch.is_none_or(|own| own <= by) => {
                FenceStatus::Fenced { epoch, by }
            }
            (None, _) => FenceStatus::Unowned,
            (Some(epoch), _) => match hold.valid_until {
                Some(valid_until) if now < valid_until => {
                    FenceStatus::Holding { epoch, valid_until }
                }
                Some(lapsed_at) => FenceStatus::Lapsed { epoch, lapsed_at },
                None => FenceStatus::Lapsed {
                    epoch,
                    lapsed_at: now,
                },
            },
        }
    }

    /// The epoch an order at `now` is placed under, or why none is.
    pub fn admits(&self, now: Timestamp) -> Result<u64, FenceStatus> {
        match self.status(now) {
            FenceStatus::Holding { epoch, .. } => Ok(epoch),
            other => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::Duration;

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000 + secs)
    }

    #[test]
    fn a_fence_admits_only_inside_a_granted_hold_and_never_below_an_epoch_seen_elsewhere() {
        let fence = Fence::new();
        assert_eq!(fence.status(t(0)), FenceStatus::Unowned);
        assert!(
            fence.admits(t(0)).is_err(),
            "an unowned fence admits nothing"
        );

        fence
            .grant(1, t(0).saturating_add(Duration::from_secs(3)))
            .expect("first grant");
        assert_eq!(fence.admits(t(1)), Ok(1));
        assert!(
            matches!(fence.status(t(3)), FenceStatus::Lapsed { epoch: 1, .. }),
            "a hold is not valid at its own lapse instant"
        );

        fence.fence(2);
        assert!(matches!(
            fence.status(t(1)),
            FenceStatus::Fenced {
                epoch: Some(1),
                by: 2
            }
        ));
        assert!(
            fence.grant(2, t(10)).is_err(),
            "the epoch another process holds is never granted here"
        );
        fence
            .grant(3, t(10))
            .expect("a higher epoch, acquired later, is granted");
        assert_eq!(fence.admits(t(5)), Ok(3));
        assert!(
            fence.grant(1, t(20)).is_err(),
            "an epoch never goes backwards"
        );
    }
}
