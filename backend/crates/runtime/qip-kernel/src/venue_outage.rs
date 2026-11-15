//! A venue's reported status as an outage in the historical universe
//! (R12-TICK-2090).
//!
//! `UniverseHistory::open_outage` had one production writer: the platform's
//! own feasibility withdrawal. A venue that told the platform it was down — a
//! `VenueStatus` halt — left no mark, so a backtest over that window read the
//! gap as a quiet market. [`apply_venue_status`] is the writer for a reported
//! status: idempotent, so a venue that repeats "down" every poll opens one
//! outage and not one per poll.

use qip_core::error::Result;
use qip_core::Timestamp;
use qip_financial::history::UniverseHistory;

/// What [`apply_venue_status`] changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutageChange {
    Opened,
    Closed,
    Unchanged,
}

/// Record that `venue` reported itself `up` or down at `at`.
///
/// Down with no open outage opens one; down with one open changes nothing;
/// up with one open closes it at `at`; up with none open changes nothing.
/// Refuses what the history refuses (an outage ending before it began).
pub fn apply_venue_status(
    history: &mut UniverseHistory,
    venue: &str,
    up: bool,
    at: Timestamp,
    reason: &str,
) -> Result<OutageChange> {
    let open = history
        .outages()
        .iter()
        .any(|o| o.venue == venue && o.until == Timestamp::MAX);
    match (up, open) {
        (false, false) => {
            history.open_outage(venue, at, reason)?;
            Ok(OutageChange::Opened)
        }
        (true, true) => {
            history.close_outage(venue, at)?;
            Ok(OutageChange::Closed)
        }
        _ => Ok(OutageChange::Unchanged),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000 + s)
    }

    /// Mutation: not run (nothing was compiled).
    #[test]
    fn a_reported_down_opens_one_outage_and_an_up_closes_it() {
        let mut history = UniverseHistory::new();
        assert_eq!(
            apply_venue_status(&mut history, "XNYS", false, at(10), "halt").unwrap(),
            OutageChange::Opened
        );
        assert_eq!(
            apply_venue_status(&mut history, "XNYS", false, at(20), "halt").unwrap(),
            OutageChange::Unchanged,
            "a repeated down opened a second outage"
        );
        assert_eq!(history.outages().len(), 1);
        assert_eq!(
            apply_venue_status(&mut history, "XNYS", true, at(30), "").unwrap(),
            OutageChange::Closed
        );
        assert_eq!(history.outages()[0].until, at(30));
        assert_eq!(
            apply_venue_status(&mut history, "XNYS", true, at(40), "").unwrap(),
            OutageChange::Unchanged
        );
        assert_eq!(history.outages().len(), 1);
    }
}
