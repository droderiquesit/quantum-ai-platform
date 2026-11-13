//! Systemic risk (R12-RISK-1162): the gross that one causal mechanism reaches
//! **across regions**, so that a shock at that mechanism is contagion rather
//! than a local loss.
//!
//! # What this measures, and what it does not
//!
//! [`crate::shared_cause`] already charges every held position to the causal
//! drivers the world model's graph says move it, and the shipped
//! `causal-driver-concentration` cap bounds any one driver's bucket. That cap
//! cannot tell a driver whose downstream positions all sit in one country —
//! a concentrated local bet the country cap also sees — from one whose
//! downstream positions sit in several. The second is the systemic case: the
//! shock crosses the regional boundary the platform's cells are partitioned
//! on (ADR 0008), and the country cap, which reads each jurisdiction alone,
//! sees nothing above its bound in any of them.
//!
//! So a driver is **systemic** when the held positions it is attributed to
//! span at least [`SYSTEMIC_REGION_SPAN`] distinct regions, and its bucket on
//! [`SYSTEMIC_CONTAGION_AXIS`] is the same weighted gross the causal-driver
//! level charged it — never a second derivation. A driver confined to one
//! region produces no bucket here.
//!
//! Not modelled, and not pretended: crowding across other firms' books (the
//! tree holds no data on anyone else's positions), interbank funding
//! contagion, or market-wide liquidity spirals. Each would be a further
//! source of cross-region attribution feeding the same axis.
//!
//! # A position with no region is counted as a region of its own
//!
//! Fail-closed. A holding whose record names no country could sit anywhere,
//! and reading it as "the same region as its neighbours" would let a
//! catalogue gap hide a cross-border mechanism. Each such holding is counted
//! as one further distinct region, which can only make a driver systemic
//! sooner, never later.
//!
//! # The states
//!
//! | State | How it reads |
//! |---|---|
//! | The causal-driver level was never declared | the axis is absent — nothing was looked for |
//! | Declared, no driver spans two regions | the axis is **present and empty** |
//! | Declared, drivers span regions | one bucket per systemic driver |
//! | The causal-driver level was refused, a second writer exists, or the sum overflowed | [`RiskState::unevaluated`] carries the axis, refusing every order |

use crate::limits::RiskState;
use crate::shared_cause::{CAUSAL_DRIVER_AXIS, SharedCauseExposure};
use qip_core::Decimal;
use std::collections::{BTreeMap, BTreeSet};

/// The exposure axis a cross-region causal driver's gross is charged to. One
/// literal, exported, for the reason `COUNTERPARTY_AXIS` is.
pub const SYSTEMIC_CONTAGION_AXIS: &str = "systemic_contagion";

/// How many distinct regions a driver's held positions must span before the
/// driver is systemic. Two is the definition, not a tuning: one region is a
/// local concentration the country cap already reads.
pub const SYSTEMIC_REGION_SPAN: usize = 2;

/// Charge every cross-region causal driver in `exposure` onto
/// [`SYSTEMIC_CONTAGION_AXIS`] and return the state.
///
/// `regions` maps an instrument to the region its record names; an
/// instrument absent from it, or mapped to a blank name, is counted as a
/// region of its own (see the module doc). The buckets are weighted exactly
/// as the causal-driver level weights them, over the positions the state
/// actually holds.
pub fn with_systemic_contagion(
    mut state: RiskState,
    exposure: &SharedCauseExposure,
    regions: &BTreeMap<String, String>,
) -> RiskState {
    if let Some(reason) = exposure.refusal(CAUSAL_DRIVER_AXIS) {
        return state.with_unevaluated(
            SYSTEMIC_CONTAGION_AXIS,
            format!(
                "the systemic level is read from the causal-driver level, which was refused: \
                 {reason}"
            ),
        );
    }
    let Some(drivers) = exposure.attributed(CAUSAL_DRIVER_AXIS) else {
        return state;
    };
    if state.axis_exposures.contains_key(SYSTEMIC_CONTAGION_AXIS) {
        return state.with_unevaluated(
            SYSTEMIC_CONTAGION_AXIS,
            format!(
                "the {SYSTEMIC_CONTAGION_AXIS} level was already charged against this state by \
                 another producer, so this one's {} driver(s) are not merged into it; a level \
                 with two writers cannot say whose number a veto fired on",
                drivers.len()
            ),
        );
    }
    let mut buckets: BTreeMap<String, Decimal> = BTreeMap::new();
    for (driver, members) in drivers {
        let mut span: BTreeSet<String> = BTreeSet::new();
        let mut charged = Decimal::ZERO;
        for (instrument, weight) in members {
            let Some(notional) = state.position_notionals.get(instrument) else {
                continue;
            };
            let region = regions
                .get(instrument)
                .filter(|region| !region.trim().is_empty())
                .map_or_else(|| format!("\u{0}unknown:{instrument}"), Clone::clone);
            span.insert(region);
            let Some(sum) = notional
                .abs()
                .checked_mul(*weight)
                .and_then(|part| charged.checked_add(part))
            else {
                return state.with_unevaluated(
                    SYSTEMIC_CONTAGION_AXIS,
                    format!(
                        "charging driver {driver} across regions left the range a notional can \
                         carry; the systemic level was not computed, and a level nobody \
                         measured is not a level of zero"
                    ),
                );
            };
            charged = sum;
        }
        if span.len() >= SYSTEMIC_REGION_SPAN && charged.is_positive() {
            buckets.insert(driver.clone(), charged);
        }
    }
    state
        .axis_exposures
        .insert(SYSTEMIC_CONTAGION_AXIS.to_string(), buckets);
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::LimitSet;
    use crate::shared_cause::FACTOR_AXIS;

    const CAP: &str = "systemic-contagion";

    fn book() -> RiskState {
        let mut state = RiskState {
            equity: Decimal::from_int(1_000_000),
            cash: Decimal::from_int(600_000),
            gross_exposure: Decimal::from_int(400_000),
            ..RiskState::default()
        };
        for (name, notional) in [("us-bank", 200_000), ("de-bank", 200_000)] {
            state
                .position_notionals
                .insert(name.to_string(), Decimal::from_int(notional));
        }
        state
    }

    fn driver_over(members: &[&str]) -> SharedCauseExposure {
        let mut exposure = SharedCauseExposure::new();
        exposure.declare(CAUSAL_DRIVER_AXIS).expect("declares");
        for member in members {
            exposure
                .attribute(CAUSAL_DRIVER_AXIS, "funding-stress", member, 1.0)
                .expect("attributes");
        }
        exposure
    }

    fn regions(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(i, r)| ((*i).to_string(), (*r).to_string()))
            .collect()
    }

    fn breaches_cap(state: &RiskState) -> bool {
        LimitSet::conservative_default()
            .check(state)
            .breaches
            .iter()
            .any(|b| b.limit_name == CAP && b.blocks())
    }

    #[test]
    fn one_driver_reaching_two_regions_is_systemic_and_breaches_the_shipped_cap() {
        let state = with_systemic_contagion(
            book(),
            &driver_over(&["us-bank", "de-bank"]),
            &regions(&[("us-bank", "US"), ("de-bank", "DE")]),
        );
        assert_eq!(
            state.axis_exposures[SYSTEMIC_CONTAGION_AXIS]["funding-stress"],
            Decimal::from_int(400_000)
        );
        assert!(
            breaches_cap(&state),
            "forty percent of equity behind one cross-border mechanism must block"
        );
    }

    #[test]
    fn the_same_driver_confined_to_one_region_is_not_systemic() {
        let state = with_systemic_contagion(
            book(),
            &driver_over(&["us-bank", "de-bank"]),
            &regions(&[("us-bank", "US"), ("de-bank", "US")]),
        );
        assert!(
            state.axis_exposures[SYSTEMIC_CONTAGION_AXIS].is_empty(),
            "measured and empty: a local concentration is the country cap's to read"
        );
        assert!(!breaches_cap(&state));
    }

    #[test]
    fn a_holding_with_no_region_counts_as_a_region_of_its_own() {
        let state = with_systemic_contagion(
            book(),
            &driver_over(&["us-bank", "de-bank"]),
            &regions(&[("us-bank", "US")]),
        );
        assert!(
            state.axis_exposures[SYSTEMIC_CONTAGION_AXIS].contains_key("funding-stress"),
            "an unknown region could be anywhere and must not hide a cross-border driver"
        );
    }

    #[test]
    fn an_undeclared_causal_level_files_no_systemic_axis_and_a_refused_one_refuses() {
        let state = with_systemic_contagion(book(), &SharedCauseExposure::new(), &BTreeMap::new());
        assert!(!state.axis_exposures.contains_key(SYSTEMIC_CONTAGION_AXIS));
        assert!(state.unevaluated.is_empty());

        let mut refused = SharedCauseExposure::new();
        refused.declare(FACTOR_AXIS).expect("declares");
        refused
            .refuse(CAUSAL_DRIVER_AXIS, "the graph could not be read")
            .expect("refuses");
        let state = with_systemic_contagion(book(), &refused, &BTreeMap::new());
        assert!(state.unevaluated.contains_key(SYSTEMIC_CONTAGION_AXIS));
    }

    #[test]
    fn a_second_writer_of_the_systemic_axis_is_refused_not_merged() {
        let mut state = book();
        state
            .axis_exposures
            .insert(SYSTEMIC_CONTAGION_AXIS.to_string(), BTreeMap::new());
        let state = with_systemic_contagion(
            state,
            &driver_over(&["us-bank", "de-bank"]),
            &regions(&[("us-bank", "US"), ("de-bank", "DE")]),
        );
        assert!(state.unevaluated.contains_key(SYSTEMIC_CONTAGION_AXIS));
    }
}
