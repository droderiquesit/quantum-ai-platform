//! Operational risk (R12-RISK-1162): the gross the book carries at a venue
//! whose *process* the platform has already found failing.
//!
//! # What "operational" means here, stated no larger than the evidence
//!
//! Market, liquidity and counterparty risk are each a question about the
//! price, the exit or the obligor. Operational risk is a question about the
//! machinery between the desk and the position: a venue whose order path has
//! refused, timed out or diverged often enough that the platform withdrew it.
//! The only evidence of such a failure this tree holds as a *standing fact*
//! is that withdrawal — `qip-kernel`'s withdrawn-venue set, which is written
//! on a cluster of feasibility refusals (ADR 0062) and replayed from the
//! event log. A withdrawal stops new orders reaching the venue; it does
//! nothing about the gross already sitting there, which is exposed to the
//! failure that caused the withdrawal and which the book can no longer
//! adjust through that venue. That stranded gross is the figure.
//!
//! It is deliberately not a model of process loss, of fraud, of key-person
//! risk or of infrastructure outage probabilities: nothing in the tree
//! measures any of them, and a figure invented for one would read as a
//! measurement. Each would be a further *source* of impaired names feeding
//! the same axis, not a new axis.
//!
//! # Why an axis, not a new limit kind
//!
//! For the reason [`crate::shared_cause`] gives: a figure filed onto
//! [`RiskState::axis_exposures`] is read by [`crate::LimitKind::MaxAxisWeight`],
//! the veto already proven to fire, and the shipped
//! `operational-failure-venue-gross` cap names [`OPERATIONAL_FAILURE_AXIS`].
//! A new limit kind would need a new field on [`RiskState`] with a second
//! producer, which is how `MaxCounterpartyExposure` once came to read a map
//! nothing wrote.
//!
//! # The four states
//!
//! | State | How it reads |
//! |---|---|
//! | No impaired venue | the axis is **absent** — no failure, nothing to charge |
//! | An impaired venue the book holds nothing at | the axis is absent for it |
//! | An impaired venue carrying gross | one bucket per such venue, the counterparty axis's own figure |
//! | A second writer already filed the axis | [`RiskState::unevaluated`] carries the axis, which refuses every order |
//!
//! Absent rather than present-and-empty when nothing is impaired, because
//! [`crate::LimitKind::MaxAxisWeight`] reads both the same way and an empty
//! axis on every healthy book would only make the healthy book harder to
//! read. The distinction [`crate::shared_cause`] draws between "nothing
//! looked" and "looked and found nothing" is carried here by the
//! withdrawn-venue set itself, which the kernel replays from the log.

use crate::limits::{COUNTERPARTY_AXIS, RiskState};
use qip_core::Decimal;
use std::collections::{BTreeMap, BTreeSet};

/// The exposure axis gross stranded at an operationally failed venue is
/// charged to. One literal, exported, for the reason
/// [`COUNTERPARTY_AXIS`] is: the producer in `qip-kernel` and the shipped cap
/// here must spell it one way, or the cap cannot fire.
pub const OPERATIONAL_FAILURE_AXIS: &str = "operational_failure";

/// File the gross the book carries at each venue in `impaired` onto
/// [`OPERATIONAL_FAILURE_AXIS`], and return the state.
///
/// The buckets are the counterparty axis's own running gross, copied and
/// never recomputed, so the cap reads the figure every other counterparty
/// control reads rather than a second count of it. A venue in `impaired`
/// that carries no positive gross produces no bucket. The figure is taken
/// as an absolute value: gross at a venue is exposure to that venue's
/// failure whichever way it points.
///
/// **An axis already present is refused, not overwritten.** A second writer
/// would leave the veto unable to say whose number it fired on; the refusal
/// is filed under [`RiskState::unevaluated`], which refuses every order —
/// the fail-closed direction.
pub fn with_operational_failure(mut state: RiskState, impaired: &BTreeSet<String>) -> RiskState {
    if impaired.is_empty() {
        return state;
    }
    let stranded: BTreeMap<String, Decimal> = state
        .axis_exposures
        .get(COUNTERPARTY_AXIS)
        .into_iter()
        .flatten()
        .filter(|(venue, gross)| impaired.contains(*venue) && gross.abs().is_positive())
        .map(|(venue, gross)| (venue.clone(), gross.abs()))
        .collect();
    if stranded.is_empty() {
        return state;
    }
    if state.axis_exposures.contains_key(OPERATIONAL_FAILURE_AXIS) {
        return state.with_unevaluated(
            OPERATIONAL_FAILURE_AXIS,
            format!(
                "the {OPERATIONAL_FAILURE_AXIS} level was already charged against this state by \
                 another producer, so the gross stranded at {} impaired venue(s) is not merged \
                 into it; a level with two writers cannot say whose number a veto fired on",
                stranded.len()
            ),
        );
    }
    state
        .axis_exposures
        .insert(OPERATIONAL_FAILURE_AXIS.to_string(), stranded);
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::LimitSet;

    fn book() -> RiskState {
        let mut counterparty = BTreeMap::new();
        counterparty.insert("venue-a".to_string(), Decimal::from_int(400_000));
        counterparty.insert("venue-b".to_string(), Decimal::from_int(100_000));
        let mut state = RiskState {
            equity: Decimal::from_int(1_000_000),
            cash: Decimal::from_int(500_000),
            gross_exposure: Decimal::from_int(500_000),
            ..RiskState::default()
        };
        state
            .axis_exposures
            .insert(COUNTERPARTY_AXIS.to_string(), counterparty);
        state
    }

    fn impaired(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    #[test]
    fn gross_stranded_at_a_withdrawn_venue_breaches_the_shipped_operational_cap() {
        let limits = LimitSet::conservative_default();
        let premise = limits.check(&book());
        assert!(
            premise
                .breaches
                .iter()
                .all(|b| b.limit_name != "operational-failure-venue-gross"),
            "premise: a healthy book does not breach the operational cap"
        );
        let state = with_operational_failure(book(), &impaired(&["venue-a"]));
        assert_eq!(
            state.axis_exposures[OPERATIONAL_FAILURE_AXIS]["venue-a"],
            Decimal::from_int(400_000)
        );
        let check = limits.check(&state);
        assert!(
            check
                .breaches
                .iter()
                .any(|b| b.limit_name == "operational-failure-venue-gross" && b.blocks()),
            "forty percent of equity stranded at a failed venue must block: {:?}",
            check.breaches
        );
    }

    #[test]
    fn a_small_stranded_balance_is_charged_but_does_not_breach() {
        let state = with_operational_failure(book(), &impaired(&["venue-b"]));
        assert_eq!(
            state.axis_exposures[OPERATIONAL_FAILURE_AXIS]["venue-b"],
            Decimal::from_int(100_000)
        );
        let check = LimitSet::conservative_default().check(&state);
        assert!(
            check
                .breaches
                .iter()
                .all(|b| b.limit_name != "operational-failure-venue-gross"),
            "a tenth of equity is inside the quarter the cap allows"
        );
    }

    #[test]
    fn no_impaired_venue_or_an_impaired_venue_holding_nothing_files_no_axis() {
        assert!(
            !with_operational_failure(book(), &impaired(&[]))
                .axis_exposures
                .contains_key(OPERATIONAL_FAILURE_AXIS)
        );
        assert!(
            !with_operational_failure(book(), &impaired(&["venue-z"]))
                .axis_exposures
                .contains_key(OPERATIONAL_FAILURE_AXIS)
        );
    }

    #[test]
    fn a_second_writer_of_the_operational_axis_is_refused_not_merged() {
        let mut state = book();
        state
            .axis_exposures
            .insert(OPERATIONAL_FAILURE_AXIS.to_string(), BTreeMap::new());
        let state = with_operational_failure(state, &impaired(&["venue-a"]));
        assert!(state.unevaluated.contains_key(OPERATIONAL_FAILURE_AXIS));
        assert!(state.axis_exposures[OPERATIONAL_FAILURE_AXIS].is_empty());
    }
}
