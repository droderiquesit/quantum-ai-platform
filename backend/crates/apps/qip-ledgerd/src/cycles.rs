//! A multi-region cycle's legs, grouped and judged from the order register
//! (R12-LEDGER-10114, ADR 0652).
//!
//! The register (ADR 0490) holds one row per order; since ADR 0652 a cycle
//! leg's row names its cycle. This module is the replay that reads those rows
//! and says what became of each cycle, so a cycle that filled one leg and lost
//! the other is a finding instead of two unrelated orders. It reads the
//! register only and moves no balance: a verdict is a report, never a posting.

use crate::store::{OrderRow, OrderState};
use qip_core::Decimal;
use std::collections::BTreeMap;

/// What became of one cycle, judged from its legs' register rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleVerdict {
    /// At least one leg is still open: no verdict yet.
    Open,
    /// Every leg filled in full.
    Complete,
    /// Something filled and something did not: the books hold a leg whose
    /// offset never arrived. The finding a replay exists to raise.
    Partial,
    /// Nothing filled and every leg closed: the cycle was lost whole.
    Failed,
}

/// One cycle as the ledger saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleReport {
    pub cell: String,
    pub cycle_id: String,
    /// Order ids of the legs the ledger saw sent, in order-id order.
    pub orders: Vec<String>,
    pub verdict: CycleVerdict,
}

use serde::{Deserialize, Serialize};

/// Group `rows` by `(cell, cycle_id)` and judge each group. Rows naming no
/// cycle are not a cycle's and are left out.
///
/// A cycle whose other legs were never sent shows only the legs that were,
/// so a lone filled leg reads `Complete` here; completeness against the
/// cycle's intended leg count needs `CycleCommitted`, which stays on P2
/// (ADR 0153 decision 6) and is why ADR 0652 records that as open.
pub fn evaluate(rows: &[OrderRow]) -> Vec<CycleReport> {
    let mut groups: BTreeMap<(String, String), Vec<&OrderRow>> = BTreeMap::new();
    for row in rows {
        if let Some(cycle) = &row.cycle_id {
            groups
                .entry((row.cell.clone(), cycle.clone()))
                .or_default()
                .push(row);
        }
    }
    groups
        .into_iter()
        .map(|((cell, cycle_id), mut legs)| {
            legs.sort_by(|a, b| a.order_id.cmp(&b.order_id));
            let verdict = if legs.iter().any(|l| l.state == OrderState::Open) {
                CycleVerdict::Open
            } else if legs.iter().all(|l| l.state == OrderState::Filled) {
                CycleVerdict::Complete
            } else if legs.iter().any(|l| l.filled > Decimal::ZERO) {
                CycleVerdict::Partial
            } else {
                CycleVerdict::Failed
            };
            CycleReport {
                cell,
                cycle_id,
                orders: legs.iter().map(|l| l.order_id.clone()).collect(),
                verdict,
            }
        })
        .collect()
}
