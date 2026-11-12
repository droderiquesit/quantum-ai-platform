//! The centre's producer of [`HedgeNarrowing`]s (ADR 0407, ADR 0126
//! decision 4, R12-CAPITAL-10098).
//!
//! A [`HedgePlan`] states trades — buy or sell this quantity of this
//! instrument at this venue — and a cell can be bound only by a target net
//! between what it holds and zero. This module is the one place the first is
//! turned into the second, and the rule it applies is the narrowest one that
//! states anything:
//!
//! 1. The plan's instruments are netted per venue and instrument, buys
//!    positive and sells negative, so a plan that names one instrument twice
//!    states one trade and every cell receives at most one target for it.
//! 2. A net sell reduces the cells that are long it at that venue, a net buy
//!    the cells that are short it. A cell flat or on the same side is not
//!    touched: binding it would mean asking it to open a position, which a
//!    narrowing cannot express (ADR 0126, "What would make this wrong").
//! 3. The quantity is shared pro rata across those cells by the size of each
//!    one's holding: each keeps `holding × (total − quantity) / total`, and
//!    zero once the quantity reaches the total. No cell is preferred, so no
//!    order of cells is a figure somebody has to choose, and no target ever
//!    passes zero. The part of the quantity beyond what the cells hold, and
//!    every instrument nobody holds the opposite side of, is not sent and is
//!    said not to be sent.
//! 4. A target is stated only at a venue one of the cell's live envelopes
//!    grants, because the cell refuses a whole frame naming a venue no live
//!    envelope there grants, and one stray target would cost the cell every
//!    other target in the frame.
//!
//! The holdings are each cell's stated book (ADR 0208), read only where it
//! agrees with the centre's settled book for that instrument: the settled
//! book stands, and a target computed from a statement the ledger
//! contradicts would be a target about a position nobody has established.
//! An instrument on which the two disagree is left out at that cell and the
//! line says so.
//!
//! Nothing here signs, sends or reaches a venue. The plane signs what this
//! returns with the envelope issuer and the API's mesh backbone sends it; a
//! cell may refuse it whole, and a cell that never hears it is exactly as
//! bounded as its envelope leaves it.

use qip_contracts::HedgePlan;
use qip_contracts::control::HedgeSide;
use qip_contracts::policy::{HedgeNarrowing, HedgeTarget};
use qip_contracts::venue::VenueId;
use qip_core::{Decimal, ObjectId, Timestamp};
use std::collections::{BTreeMap, BTreeSet};

/// One cell's holdings as the producer reads them: the stated positions on
/// which the cell and the centre's settled book agree, summed per venue and
/// instrument, and the venues its live envelopes grant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CellHoldings {
    /// `(venue, instrument) → signed net`, bought positive.
    pub held: BTreeMap<(String, String), Decimal>,
    pub granted_venues: BTreeSet<String>,
    /// Instruments left out because the statement and the settled book
    /// disagree on them, for the line.
    pub disputed: BTreeSet<String>,
}

/// What one derivation produced: the unsigned narrowings, one per cell that
/// is bound by anything, and a line for every part of the plan that was not
/// stated to any cell.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HedgeDistribution {
    pub narrowings: Vec<HedgeNarrowing>,
    pub lines: Vec<String>,
}

/// Derive the per-cell narrowings of `plan`, decided at `planned_at`, from
/// each cell's holdings (ADR 0407).
///
/// `issued_at` is the plan's instant, not the moment of sending: a cell
/// keeps the newest by that instant and refuses one at or behind it, so a
/// plan re-derived against a book its own targets already moved is refused
/// at the cell rather than compounded. The deadline is the plan's own time
/// to exit — the one figure in the plan that says how long taking the hedge
/// on or off is expected to take — and nothing is derived at or past it.
pub fn derive(
    plan: &HedgePlan,
    planned_at: Timestamp,
    cells: &BTreeMap<String, CellHoldings>,
    now: Timestamp,
) -> HedgeDistribution {
    let mut out = HedgeDistribution::default();
    let deadline = planned_at.saturating_add(plan.liquidity.time_to_exit);
    if deadline <= planned_at {
        out.lines.push(format!(
            "{}: its time to exit does not land after the plan's instant; nothing is sent",
            plan.plan_id
        ));
        return out;
    }
    if now >= deadline {
        out.lines.push(format!(
            "{}: its time to exit ended at {deadline}; a narrowing past its deadline binds \
             nothing and is not sent",
            plan.plan_id
        ));
        return out;
    }
    for (cell, holdings) in cells {
        for instrument in &holdings.disputed {
            out.lines.push(format!(
                "{}: {cell}'s stated book disagrees with the settled book on {instrument}, so no \
                 target is stated for it there",
                plan.plan_id
            ));
        }
    }
    let mut trades: BTreeMap<(String, String), Decimal> = BTreeMap::new();
    for leg in &plan.instruments {
        let signed = match leg.side {
            HedgeSide::Buy => leg.quantity,
            HedgeSide::Sell => -leg.quantity,
        };
        let entry = trades
            .entry((leg.venue.as_str().to_string(), leg.instrument.clone()))
            .or_insert(Decimal::ZERO);
        match entry.checked_add(signed) {
            Some(sum) => *entry = sum,
            None => {
                out.lines.push(format!(
                    "{}: the quantities of {} at {} do not sum; nothing is stated for it",
                    plan.plan_id,
                    leg.instrument,
                    leg.venue.as_str()
                ));
                *entry = Decimal::ZERO;
            }
        }
    }
    let mut targets: BTreeMap<String, Vec<HedgeTarget>> = BTreeMap::new();
    for ((venue, instrument), trade) in &trades {
        if trade.is_zero() {
            continue;
        }
        // A sell reduces longs, a buy reduces shorts: the holders are the
        // cells on the side opposite the trade.
        let reduces = |held: Decimal| {
            if trade.is_negative() {
                held.is_positive() && !held.is_zero()
            } else {
                held.is_negative() && !held.is_zero()
            }
        };
        let holders: Vec<(&String, Decimal)> = cells
            .iter()
            .filter(|(_, holdings)| holdings.granted_venues.contains(venue))
            .filter_map(|(cell, holdings)| {
                holdings
                    .held
                    .get(&(venue.clone(), instrument.clone()))
                    .copied()
                    .filter(|held| reduces(*held))
                    .map(|held| (cell, held))
            })
            .collect();
        let quantity = trade.abs();
        let Some(total) = holders
            .iter()
            .try_fold(Decimal::ZERO, |sum, (_, held)| sum.checked_add(held.abs()))
        else {
            out.lines.push(format!(
                "{}: the holdings of {instrument} at {venue} do not sum; nothing is stated for it",
                plan.plan_id
            ));
            continue;
        };
        if total.is_zero() {
            out.lines.push(format!(
                "{}: no cell granted {venue} holds the side of {instrument} the plan's {trade} \
                 reduces; the trade would open exposure, which a narrowing cannot state and an \
                 envelope must (ADR 0126)",
                plan.plan_id
            ));
            continue;
        }
        let remaining = if quantity >= total {
            if quantity > total {
                out.lines.push(format!(
                    "{}: the plan's {trade} of {instrument} at {venue} exceeds the {total} the \
                     cells hold against it; every holder is narrowed to zero and the excess, \
                     which would open exposure, is not sent",
                    plan.plan_id
                ));
            }
            Decimal::ZERO
        } else {
            match total.checked_sub(quantity) {
                Some(remaining) => remaining,
                None => continue,
            }
        };
        for (cell, held) in holders {
            let Some(target) = held
                .checked_mul(remaining)
                .and_then(|scaled| scaled.checked_div(total))
            else {
                out.lines.push(format!(
                    "{}: {cell}'s share of {instrument} at {venue} is not representable; no \
                     target is stated for it",
                    plan.plan_id
                ));
                continue;
            };
            // A target equal to the holding binds nothing; one past it on
            // either side would be refused by the cell. Neither is sent.
            if target == held || !between_zero_and(target, held) {
                continue;
            }
            targets.entry(cell.clone()).or_default().push(HedgeTarget {
                venue: VenueId::new(venue.clone()),
                object_id: ObjectId::from_string(instrument.clone()),
                target_net: target,
            });
        }
    }
    for (cell, targets) in targets {
        out.narrowings.push(HedgeNarrowing::new(
            cell,
            plan.plan_id.clone(),
            planned_at,
            deadline,
            targets,
        ));
    }
    out
}

fn between_zero_and(value: Decimal, held: Decimal) -> bool {
    let (low, high) = if held.is_negative() {
        (held, Decimal::ZERO)
    } else {
        (Decimal::ZERO, held)
    };
    low <= value && value <= high
}
