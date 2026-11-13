//! The execution twin refitted from the event log (R12-MODEL-4008).
//!
//! The LEARN stage recalibrates the twin from each fill as it is scored,
//! and that calibration lived only in memory: a restarted platform priced
//! its first counterfactuals with the uncalibrated model, and nobody could
//! ask what the twin would have learned from a stretch of history without
//! re-running the platform over it. The journal already holds every score —
//! each cycle's `CounterfactualJournal::fill_outcomes` carries the twin's
//! entry error per fill and the venue that filled it — so the calibration is
//! a function of the log, and this module is that function.
//!
//! **What the log does not carry**: a fill's participation in its bar's
//! volume. `FillScore` has no field for it, and adding one changes a sealed
//! body's schema. So a refit from the log moves each venue's spread and not
//! its impact slope; it equals the live calibration exactly while the live
//! one has fitted no slope (fewer than two sizes of fill), and differs once
//! it has. That remainder is recorded against the row, not hidden here.

use crate::platform::{CounterfactualJournal, CycleJournalEntry};
use qip_core::error::Result;
use qip_twin::calibration::{FillCalibration, RecordedFill};
use std::collections::BTreeSet;

/// Every fill the journals scored, once each, in the order they were first
/// scored.
///
/// Each cycle's journal carries the *whole* retained list of fill scores,
/// not only that cycle's, so the same fill appears in every later cycle's
/// journal until the retention cap drops it. Read naively the log would
/// replay a fill once per cycle that followed it, and a twin fitted to that
/// learns the first fills a hundred times over. A fill is therefore taken
/// the first time its order id appears and skipped after. A score with no
/// entry error — the twin did not trade it — teaches nothing and is skipped.
pub fn recorded_fills<'a, I>(journals: I) -> Vec<RecordedFill>
where
    I: IntoIterator<Item = &'a CounterfactualJournal>,
{
    let mut seen = BTreeSet::new();
    let mut fills = Vec::new();
    for journal in journals {
        for score in &journal.fill_outcomes {
            if !seen.insert(score.order_id.clone()) {
                continue;
            }
            if let Some(error_bps) = score.trade_error_bps {
                fills.push(RecordedFill {
                    venue: score.venue.clone(),
                    error_bps,
                    participation: None,
                });
            }
        }
    }
    fills
}

/// The twin's fill calibration at `learning_rate`, rebuilt from the cycle
/// journals alone by the rule the LEARN stage applies live. Refuses, absorbing nothing, if any recorded error is not a number
/// (`FillCalibration::replay`).
pub fn refit_from_journal(
    entries: &[CycleJournalEntry],
    learning_rate: f64,
) -> Result<FillCalibration> {
    let fills = recorded_fills(
        entries
            .iter()
            .filter_map(|entry| entry.counterfactuals.as_ref()),
    );
    FillCalibration::from_history(learning_rate, fills)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::FillScore;
    use qip_core::Decimal;
    use qip_core::ObjectId;
    use qip_core::ids::OrderId;
    use qip_core::time::Timestamp;

    fn score(order: &str, venue: &str, error: Option<f64>) -> FillScore {
        FillScore {
            order_id: OrderId::from_string(order),
            object_id: ObjectId::from_string("obj-AAA"),
            venue: venue.to_string(),
            filled_at: Timestamp::from_secs(0),
            scored_at: Timestamp::from_secs(0),
            smaller_favoured: false,
            larger_favoured: false,
            trade_error_bps: error,
            unfilled: Decimal::ZERO,
            realised_slippage_bps: None,
            predicted_slippage_bps: None,
            predicted_outcome_bps: None,
            realised_outcome_bps: None,
            outcome_error_bps: None,
        }
    }

    fn journal(outcomes: Vec<FillScore>) -> CounterfactualJournal {
        CounterfactualJournal {
            scored: 0,
            regrets: 0,
            deferred: 0,
            fills_scored: outcomes.len(),
            fills_deferred: 0,
            declined_outcomes: Vec::new(),
            fill_outcomes: outcomes,
            cell_declined_scored: 0,
            cell_declined_outcomes: Vec::new(),
        }
    }

    /// Each cycle journals the whole retained list of scores, so the second
    /// cycle's journal repeats the first cycle's fill. Replayed naively the
    /// first fill would be learned twice, and over a working day's cycles a
    /// hundred times.
    ///
    /// Mutation: drop the `seen.insert` check — `ord-1` is read from both
    /// journals and the count is three, not two.
    #[test]
    fn a_fill_repeated_in_every_later_cycles_journal_is_replayed_once() {
        let first = journal(vec![score("ord-1", "XLON", Some(-8.0))]);
        let second = journal(vec![
            score("ord-1", "XLON", Some(-8.0)),
            score("ord-2", "XLON", Some(-4.0)),
            score("ord-3", "XLON", None),
        ]);
        let fills = recorded_fills([&first, &second]);
        assert_eq!(
            fills.iter().map(|fill| fill.error_bps).collect::<Vec<_>>(),
            vec![-8.0, -4.0],
            "each scored fill once, in the order first scored, and none without an error"
        );
        assert!(fills.iter().all(|fill| fill.participation.is_none()));
    }
}
