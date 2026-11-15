//! One training-data export of every decision path the LEARN stage priced
//! (R12-MODEL-6002, ADR 0683).
//!
//! The paths were each journaled: declined orders, the legs of cells' refused
//! cycles, orders a venue filled and the sizes not taken. But each lived in
//! its own list inside `CounterfactualJournal`, inside a cycle entry, so a
//! trainer that wanted "every path the platform took or declined to take" had
//! to know four shapes and walk the journal for each. [`TrainingExport`] is the
//! one shape, read from the replayed journal and nothing else, so the export
//! and the log cannot tell two stories.
//!
//! **What it holds.** One [`TrainingRecord`] per declined path, per cell
//! refusal, per fill, and a fill the venue left partly unfilled is a record of
//! its own kind ([`PathKind::PartiallyFilled`]): the unfilled remainder is the
//! path not taken on an order that otherwise went. Every figure about a world
//! that did not happen stays [`Simulated`].
//!
//! **What it does not hold, and why.** The hedge-not-taken path is held by
//! owner decision 7: no hedge policy is declared, so `run_hedge_brain`
//! proposes nothing and there is nothing to record. The agency proposals are
//! the ambient door's. The different-hedge and different-venue arms
//! `Platform::evaluate_alternatives` prices are counted on a `DeclinedScore`
//! (`alternatives`) and their individual figures are not journaled, so they
//! cannot be exported without first being recorded. Each is a named gap, not
//! a silent omission.
//!
//! **Authority.** An export is read by training, never by anything that sizes
//! or loosens a limit: blueprint 12.4 forbids loosening from counterfactual
//! evidence. Nothing here is an order path.

use crate::platform::{
    CellDeclinedScore, CounterfactualJournal, CycleJournalEntry, DeclinedScore, FillScore, Platform,
};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_twin::counterfactual::EstimationMethod;
use qip_twin::value::Simulated;
use serde::{Deserialize, Serialize};

/// Which path a record is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    /// An order a gate refused, priced against what it would have earned.
    Declined,
    /// One leg of a cell's refused cycle, priced the same way.
    CellDeclined,
    /// An order a venue filled, priced for the sizes not taken.
    Filled,
    /// A fill that left part of the order unfilled: the remainder is the path
    /// not taken on an order that otherwise went.
    PartiallyFilled,
}

/// One path as training data. Optional fields are absent exactly where the
/// path has no such fact, never zero-filled: a declined path has no size
/// finding and a fill has no regret.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingRecord {
    /// The cycle whose LEARN stage priced it.
    pub cycle: u64,
    pub path: PathKind,
    pub order_id: String,
    pub object_id: String,
    pub venue: Option<String>,
    /// The control that refused it, for the two declined kinds.
    pub gate: Option<String>,
    /// The rules the refusal is charged to.
    pub rules: Vec<String>,
    /// What the path as proposed would have earned over the twin's horizon.
    pub would_have_earned: Option<Simulated<Decimal>>,
    /// Whether the refused path would have beaten standing aside.
    pub regret: Option<bool>,
    pub smaller_favoured: Option<bool>,
    pub larger_favoured: Option<bool>,
    /// The twin's outcome less the realised one, in basis points.
    pub outcome_error_bps: Option<f64>,
    /// What the venue left unfilled.
    pub unfilled: Option<Decimal>,
    pub method: Option<EstimationMethod>,
    /// When the path was decided, filled or refused.
    pub observed_at: Timestamp,
    pub scored_at: Timestamp,
}

impl TrainingRecord {
    fn declined(cycle: u64, score: &DeclinedScore) -> Self {
        Self {
            cycle,
            path: PathKind::Declined,
            order_id: score.order_id.as_str().to_string(),
            object_id: score.object_id.as_str().to_string(),
            venue: score.venue.clone(),
            gate: Some(score.gate.clone()),
            rules: score.rules.clone(),
            would_have_earned: Some(score.would_have_earned),
            regret: Some(score.regret),
            smaller_favoured: None,
            larger_favoured: None,
            outcome_error_bps: None,
            unfilled: None,
            method: Some(score.method),
            observed_at: score.declined_at,
            scored_at: score.scored_at,
        }
    }

    fn cell_declined(cycle: u64, score: &CellDeclinedScore) -> Self {
        Self {
            cycle,
            path: PathKind::CellDeclined,
            order_id: score.order_id.as_str().to_string(),
            object_id: score.object_id.as_str().to_string(),
            venue: Some(score.venue.clone()),
            gate: Some(score.gate.clone()),
            rules: Vec::new(),
            would_have_earned: Some(score.would_have_earned),
            regret: Some(score.regret),
            smaller_favoured: None,
            larger_favoured: None,
            outcome_error_bps: None,
            unfilled: None,
            method: Some(score.method),
            observed_at: score.declined_at,
            scored_at: score.scored_at,
        }
    }

    fn filled(cycle: u64, score: &FillScore) -> Self {
        Self {
            cycle,
            path: if score.is_partial() {
                PathKind::PartiallyFilled
            } else {
                PathKind::Filled
            },
            order_id: score.order_id.as_str().to_string(),
            object_id: score.object_id.as_str().to_string(),
            venue: Some(score.venue.clone()),
            gate: None,
            rules: Vec::new(),
            would_have_earned: None,
            regret: None,
            smaller_favoured: Some(score.smaller_favoured),
            larger_favoured: Some(score.larger_favoured),
            outcome_error_bps: score.outcome_error_bps,
            unfilled: Some(score.unfilled),
            method: None,
            observed_at: score.filled_at,
            scored_at: score.scored_at,
        }
    }
}

/// Every priced path, oldest cycle first, and within a cycle declined, then
/// cell-declined, then filled, each in the order the journal holds them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrainingExport {
    pub records: Vec<TrainingRecord>,
}

impl TrainingExport {
    /// The export of `journals`, each paired with the cycle that wrote it.
    pub fn from_counterfactuals<'a>(
        journals: impl IntoIterator<Item = (u64, &'a CounterfactualJournal)>,
    ) -> Self {
        let mut records = Vec::new();
        for (cycle, journal) in journals {
            records.extend(
                journal
                    .declined_outcomes
                    .iter()
                    .map(|score| TrainingRecord::declined(cycle, score)),
            );
            records.extend(
                journal
                    .cell_declined_outcomes
                    .iter()
                    .map(|score| TrainingRecord::cell_declined(cycle, score)),
            );
            records.extend(
                journal
                    .fill_outcomes
                    .iter()
                    .map(|score| TrainingRecord::filled(cycle, score)),
            );
        }
        Self { records }
    }

    /// How many records of `path` the export holds.
    pub fn count(&self, path: PathKind) -> usize {
        self.records
            .iter()
            .filter(|record| record.path == path)
            .count()
    }

    /// One JSON object per line, in export order, for a trainer to stream.
    pub fn to_json_lines(&self) -> Result<Vec<String>> {
        self.records
            .iter()
            .map(|record| {
                serde_json::to_string(record).map_err(|error| {
                    Error::invalid(format!(
                        "a training record could not be written as JSON: {error}"
                    ))
                })
            })
            .collect()
    }
}

impl Platform {
    /// The training export of every path this platform's journal records
    /// LEARN as having priced, read back from the event log
    /// ([`Self::journal_entries`]) and not from anything in memory, so it is
    /// what a replay of the log would produce.
    pub fn training_export(&self) -> Result<TrainingExport> {
        let entries: Vec<CycleJournalEntry> = self.journal_entries()?;
        Ok(TrainingExport::from_counterfactuals(
            entries
                .iter()
                .filter_map(|entry| entry.counterfactuals.as_ref().map(|c| (entry.cycle, c))),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::ObjectId;
    use qip_core::ids::OrderId;

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    fn declined(order: &str) -> DeclinedScore {
        DeclinedScore {
            order_id: OrderId::from_string(order),
            object_id: ObjectId::from_string("AAA"),
            gate: "pre-trade".to_string(),
            declined_at: ts(10),
            scored_at: ts(20),
            would_have_earned: Simulated::of(Decimal::from_int(5)),
            regret: true,
            alternatives: 2,
            rules: vec!["max-position".to_string()],
            readings: Vec::new(),
            venue: None,
            method: EstimationMethod::BarOpenReplay,
        }
    }

    fn cell_declined(order: &str) -> CellDeclinedScore {
        CellDeclinedScore {
            order_id: OrderId::from_string(order),
            cell: "cell-1".to_string(),
            gate: "cell-gate".to_string(),
            object_id: ObjectId::from_string("BBB"),
            venue: "XSIM".to_string(),
            declined_at: ts(11),
            scored_at: ts(21),
            would_have_earned: Simulated::of(Decimal::from_int(-1)),
            regret: false,
            method: EstimationMethod::BarOpenReplay,
        }
    }

    fn fill(order: &str, unfilled: i64) -> FillScore {
        FillScore {
            order_id: OrderId::from_string(order),
            object_id: ObjectId::from_string("CCC"),
            venue: "XSIM".to_string(),
            filled_at: ts(12),
            scored_at: ts(22),
            smaller_favoured: true,
            larger_favoured: false,
            trade_error_bps: None,
            unfilled: Decimal::from_int(unfilled),
            realised_slippage_bps: None,
            predicted_slippage_bps: None,
            predicted_outcome_bps: None,
            realised_outcome_bps: Some(1.0),
            outcome_error_bps: Some(0.5),
        }
    }

    fn journal() -> CounterfactualJournal {
        CounterfactualJournal {
            scored: 1,
            regrets: 1,
            deferred: 0,
            fills_scored: 2,
            fills_deferred: 0,
            declined_outcomes: vec![declined("d-1")],
            fill_outcomes: vec![fill("f-1", 0), fill("f-2", 3)],
            cell_declined_scored: 1,
            cell_declined_outcomes: vec![cell_declined("c-1")],
        }
    }

    /// Every path the journal priced is in the export under its own kind, a
    /// partly unfilled fill is a kind of its own, and the facts a kind does
    /// not have are absent rather than zero.
    /// Mutation: not run (nothing was compiled). Dropping the
    /// `declined_outcomes` arm of `from_counterfactuals` fails the first
    /// count; reading `is_partial` as always false fails the third.
    #[test]
    fn every_priced_path_is_exported_under_its_own_kind() {
        let journal = journal();
        let export = TrainingExport::from_counterfactuals([(4, &journal)]);
        assert_eq!(export.records.len(), 4);
        assert_eq!(export.count(PathKind::Declined), 1);
        assert_eq!(export.count(PathKind::CellDeclined), 1);
        assert_eq!(export.count(PathKind::Filled), 1);
        assert_eq!(export.count(PathKind::PartiallyFilled), 1);
        assert!(export.records.iter().all(|record| record.cycle == 4));

        let declined = &export.records[0];
        assert_eq!(declined.order_id, "d-1");
        assert_eq!(declined.regret, Some(true));
        assert!(declined.would_have_earned.is_some());
        assert_eq!(
            declined.smaller_favoured, None,
            "a refusal has no size finding"
        );
        let partial = export
            .records
            .iter()
            .find(|record| record.path == PathKind::PartiallyFilled)
            .expect("the partial fill is exported");
        assert_eq!(partial.order_id, "f-2");
        assert_eq!(partial.unfilled, Some(Decimal::from_int(3)));
        assert_eq!(partial.regret, None, "a fill has no regret");
    }

    /// An empty journal exports nothing, and the lines are one JSON object
    /// per record. Mutation: not run. Writing the lines from a different
    /// list fails the length comparison.
    #[test]
    fn an_export_writes_one_json_line_per_record_and_an_empty_one_writes_none() {
        assert!(
            TrainingExport::from_counterfactuals(std::iter::empty())
                .to_json_lines()
                .expect("an empty export writes")
                .is_empty()
        );
        let journal = journal();
        let export = TrainingExport::from_counterfactuals([(1, &journal)]);
        let lines = export.to_json_lines().expect("records write");
        assert_eq!(lines.len(), export.records.len());
        for line in &lines {
            assert!(line.starts_with('{') && line.ends_with('}'), "{line}");
            assert!(!line.contains('\n'));
        }
    }
}
