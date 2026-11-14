//! A unified training-data export derived from the journaled event log
//! (R12-MODEL-6002, owner decision 36).
//!
//! Declined, filled and partially filled paths were each priced in LEARN and
//! journaled on the cycle entry, in three different vectors, and nothing read
//! them back as one set: a model trainer would have had to know three field
//! names and would have silently trained on whichever it remembered. The
//! export reads only the journal, never live platform state, so a record here
//! is reproducible from the log alone and a replayed platform exports the
//! same set.
//!
//! Each cycle entry repeats the retained history of scored paths, so the same
//! order is journaled on many cycles; the export keeps the first occurrence
//! per kind and order, because a duplicate would weight one path many times.
//!
//! Not covered, and stated here rather than implied: hedge paths (held by
//! owner decision 7: no declared hedge policy, so nothing is proposed to
//! record), and quote and agency-plan paths, which LEARN does not journal.

use std::collections::BTreeSet;

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::Decimal;
use qip_twin::value::Simulated;
use serde::{Deserialize, Serialize};

use crate::platform::{CycleJournalEntry, Platform};

/// Which path a training record describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    /// An order the desk's pre-trade gates refused.
    Declined,
    /// A leg of a cycle a regional cell refused whole.
    CellDeclined,
    /// An order a venue filled in full.
    Filled,
    /// An order a venue filled in part.
    PartialFill,
}

/// One path, as a training example.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingRecord {
    pub kind: PathKind,
    pub order_id: String,
    pub object_id: String,
    pub venue: Option<String>,
    /// The control that refused the path; `None` for a fill.
    pub gate: Option<String>,
    pub decided_at: Timestamp,
    pub scored_at: Timestamp,
    /// What a declined path would have earned. `None` for a fill: a fill
    /// carries bits and ratios, never a money figure.
    pub would_have_earned: Option<Simulated<Decimal>>,
    /// Whether the declined path would have beaten standing aside.
    pub regret: Option<bool>,
    /// Whether the twin's smaller size beat the size taken (fills only).
    pub smaller_favoured: Option<bool>,
    /// Whether the twin's larger size beat the size taken (fills only).
    pub larger_favoured: Option<bool>,
    /// The quantity a venue left unfilled (fills only).
    pub unfilled: Option<Decimal>,
}

/// Export every scored path in the journal, in journal order, once each.
pub fn export(platform: &Platform) -> Result<Vec<TrainingRecord>> {
    Ok(from_entries(&platform.journal_entries()?))
}

/// The export over already-decoded journal entries.
pub fn from_entries(entries: &[CycleJournalEntry]) -> Vec<TrainingRecord> {
    let mut seen: BTreeSet<(PathKind, String)> = BTreeSet::new();
    let mut out = Vec::new();
    let mut push = |record: TrainingRecord| {
        if seen.insert((record.kind, record.order_id.clone())) {
            out.push(record);
        }
    };
    for journal in entries.iter().filter_map(|e| e.counterfactuals.as_ref()) {
        for s in &journal.declined_outcomes {
            push(TrainingRecord {
                kind: PathKind::Declined,
                order_id: s.order_id.to_string(),
                object_id: s.object_id.to_string(),
                venue: s.venue.clone(),
                gate: Some(s.gate.clone()),
                decided_at: s.declined_at,
                scored_at: s.scored_at,
                would_have_earned: Some(s.would_have_earned.clone()),
                regret: Some(s.regret),
                smaller_favoured: None,
                larger_favoured: None,
                unfilled: None,
            });
        }
        for s in &journal.cell_declined_outcomes {
            push(TrainingRecord {
                kind: PathKind::CellDeclined,
                order_id: s.order_id.to_string(),
                object_id: s.object_id.to_string(),
                venue: Some(s.venue.clone()),
                gate: Some(s.gate.clone()),
                decided_at: s.declined_at,
                scored_at: s.scored_at,
                would_have_earned: Some(s.would_have_earned.clone()),
                regret: Some(s.regret),
                smaller_favoured: None,
                larger_favoured: None,
                unfilled: None,
            });
        }
        for s in &journal.fill_outcomes {
            push(TrainingRecord {
                kind: if s.is_partial() {
                    PathKind::PartialFill
                } else {
                    PathKind::Filled
                },
                order_id: s.order_id.to_string(),
                object_id: s.object_id.to_string(),
                venue: Some(s.venue.clone()),
                gate: None,
                decided_at: s.filled_at,
                scored_at: s.scored_at,
                would_have_earned: None,
                regret: None,
                smaller_favoured: Some(s.smaller_favoured),
                larger_favoured: Some(s.larger_favoured),
                unfilled: Some(s.unfilled),
            });
        }
    }
    out
}
