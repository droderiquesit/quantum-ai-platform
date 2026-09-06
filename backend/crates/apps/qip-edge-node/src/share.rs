//! The cell's share of its region's grant, as a probe reads it.
//!
//! A node assembled by this crate opens its region table unfunded (ADR 0039)
//! and places nothing until a verified policy payload's grant manifest names
//! grants it holds. From the order count alone that state is
//! indistinguishable from a quiet market, a halted cell, or a plan that
//! deploys nothing — and every one of those has a different remedy. The
//! health body therefore carries the table's state as its own block, with
//! the reason a node that cannot place says nothing, so an operator reading
//! `orders: 0` learns which of the four it is.
//!
//! The block is rendered here, in the library, rather than inline in
//! `main.rs`: `main` cannot be called from a test, and a health field that
//! only a running binary produced would be a field nothing proved.

use qip_core::Decimal;
use qip_edge::cell::Cell;

/// The region table as the cell holds it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionShareStatus {
    /// Whether the table can fund a hold at all: a bound above zero.
    pub funded: bool,
    /// What the cell may commit in total, or `None` for a cell with no
    /// table — a state this root cannot build, kept because the cell's own
    /// accessor is honest about it.
    pub bound: Option<Decimal>,
    /// What is left of the bound.
    pub free: Option<Decimal>,
    /// The operator's ceiling, which no share can raise.
    pub ceiling: Option<Decimal>,
    /// The sequence of the payload whose share the table holds, if any has.
    pub sequence: Option<u64>,
    /// Why the cell places nothing, when it cannot; `None` when it can.
    pub why: Option<String>,
}

impl RegionShareStatus {
    /// Read the table's state off the cell.
    pub fn of(cell: &Cell) -> Self {
        let bound = cell.region_allocation_bound();
        let free = cell.region_allocation_free();
        let ceiling = cell.region_allocation_ceiling();
        let sequence = cell.region_share_sequence();
        let funded = bound.is_some_and(|bound| bound.is_positive());
        let why = match (bound, sequence) {
            (None, _) => Some(
                "this cell holds no region table, so nothing bounds the sum of its strategies"
                    .to_string(),
            ),
            (Some(_), None) => Some(
                "no region share has been applied: this node opened unfunded and places \
                 nothing until the centre's policy payload names grants this cell holds"
                    .to_string(),
            ),
            (Some(bound), Some(sequence)) if !bound.is_positive() => Some(format!(
                "policy sequence {sequence} named no grant this cell holds, so its region \
                 share is nothing; the cell places nothing until a payload names one"
            )),
            (Some(_), Some(_)) => None,
        };
        Self {
            funded,
            bound,
            free,
            ceiling,
            sequence,
            why,
        }
    }

    /// The block as the health body carries it.
    ///
    /// Amounts are quoted strings, as `region_allocation_free` already is,
    /// because a `Decimal` rendered as a JSON number would be re-read as a
    /// float by every probe that parsed it. `why` is escaped the one way a
    /// fixed sentence needs.
    pub fn to_json(&self) -> String {
        let amount = |value: Option<Decimal>| {
            value.map_or_else(|| "null".to_string(), |value| format!("\"{value}\""))
        };
        let sequence = self
            .sequence
            .map_or_else(|| "null".to_string(), |sequence| sequence.to_string());
        let why = self.why.as_ref().map_or_else(
            || "null".to_string(),
            |why| format!("\"{}\"", why.replace('\\', "\\\\").replace('"', "\\\"")),
        );
        format!(
            r#"{{"funded":{},"bound":{},"free":{},"ceiling":{},"sequence":{sequence},"why":{why}}}"#,
            self.funded,
            amount(self.bound),
            amount(self.free),
            amount(self.ceiling),
        )
    }
}
