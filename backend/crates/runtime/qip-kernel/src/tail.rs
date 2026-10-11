//! Two tail worlds the scenario library cannot express: the counterparty the
//! book is held at fails, and the model the book was built on fails
//! (R12-RISK-4121, R12-RISK-4122).
//!
//! The standard library shocks *factors*, and a position reaches a factor
//! only through the beta the platform measured for it. That makes both of
//! these worlds invisible to it by construction:
//!
//! * **A counterparty failure moves no price.** The broker holding the book
//!   defaults, and what is lost is a share of what was custodied there,
//!   whatever the market did. No factor shock produces it.
//! * **A model failure is the failure of the betas themselves.** A book the
//!   factor model calls hedged loses nothing in a factor shock, because the
//!   model nets the legs; if the model is wrong the legs do not net. Stressing
//!   the book *through* the model cannot find the world where the model is
//!   what broke.
//!
//! So both are computed off the book directly, with their assumptions as named
//! constants rather than buried in arithmetic, and they join the library's
//! results in the `StressReport` that the survival kernel reads — where a
//! world the book does not survive withholds capital rather than drawing a
//! chart (`crate::survival`).
//!
//! Statistics throughout, so `f64`: notionals cross from `Decimal` at the call
//! site in `Platform::stress_the_book`, once, as the scenario tester's
//! exposures already do beside it.

use qip_core::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The scenario name the counterparty world is reported and charted under.
/// A literal, so the `scenario` label stays bounded by source text.
pub const COUNTERPARTY_FAILURE: &str = "counterparty-failure";

/// The scenario name the model-failure world is reported and charted under.
pub const MODEL_FAILURE: &str = "model-failure";

/// The share of custodied gross lost when the counterparty holding it fails.
///
/// An assumption, stated as one: one fifth of segregated client assets
/// unrecovered at the point of failure, before any later distribution. It is
/// deliberately not the eventual recovery, which arrives years later and does
/// not fund a margin call today. Against the platform's 20% stress tolerance
/// this makes the world bite exactly when gross exceeds equity — an unlevered
/// book survives a broker failure at the tolerance and a levered one does not,
/// which is the property the world exists to test.
pub const COUNTERPARTY_LOSS_GIVEN_DEFAULT: f64 = 0.20;

/// The adverse excursion charged to a position whose history is too short to
/// measure one: half its notional.
///
/// Not zero. A position the model has no history for is the position whose
/// failure the platform has least evidence about, and charging it nothing
/// would make the model-failure world quietest exactly where it should be
/// loudest — the `unmodelled` trap the scenario tester already names.
pub const UNMEASURED_EXCURSION: f64 = 0.50;

/// One generated world and what it would cost the book.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TailWorld {
    /// One of the constants above.
    pub scenario: String,
    pub at: Timestamp,
    /// Loss as a positive fraction of equity.
    pub loss_fraction: f64,
    /// The same loss in the book's currency, as `f64` — a statistic.
    pub loss: f64,
    /// Positions charged the unmeasured excursion rather than a measured one.
    /// Empty for the counterparty world, which needs no history.
    pub unmeasured: Vec<String>,
    /// What the world assumed, for the reader of the report alone.
    pub detail: String,
}

impl TailWorld {
    /// Whether the world takes more than `tolerance` of equity.
    pub fn breaches(&self, tolerance: f64) -> bool {
        self.loss_fraction > tolerance
    }
}

/// The worst move against a position over its retained history, as a positive
/// fraction of its value: the deepest peak-to-trough fall for a long, the
/// highest trough-to-peak rise for a short. `None` for fewer than two usable
/// prices.
fn adverse_excursion(prices: &[f64], long: bool) -> Option<f64> {
    let usable: Vec<f64> = prices
        .iter()
        .copied()
        .filter(|p| p.is_finite() && *p > 0.0)
        .collect();
    if usable.len() < 2 {
        return None;
    }
    let mut worst = 0.0_f64;
    let mut anchor = usable[0];
    for price in &usable[1..] {
        if long {
            anchor = anchor.max(*price);
            worst = worst.max((anchor - price) / anchor);
        } else {
            anchor = anchor.min(*price);
            worst = worst.max((price - anchor) / anchor);
        }
    }
    Some(worst)
}

/// Both worlds over `positions` — instrument and signed notional at cost —
/// against `equity`. Empty when there is no equity to divide by or no
/// position to stress: the caller reports a book with nothing to stress as
/// that, not as a world that cost nothing.
pub fn worlds(
    positions: &[(String, f64)],
    price_history: &BTreeMap<String, Vec<f64>>,
    equity: f64,
    at: Timestamp,
) -> Vec<TailWorld> {
    if !(equity.is_finite() && equity > 0.0) || positions.is_empty() {
        return Vec::new();
    }
    let gross: f64 = positions.iter().map(|(_, notional)| notional.abs()).sum();
    let counterparty_loss = gross * COUNTERPARTY_LOSS_GIVEN_DEFAULT;

    let mut model_loss = 0.0;
    let mut unmeasured = Vec::new();
    for (object, notional) in positions {
        if *notional == 0.0 {
            continue;
        }
        let excursion = price_history
            .get(object)
            .and_then(|prices| adverse_excursion(prices, *notional > 0.0));
        let excursion = match excursion {
            Some(excursion) => excursion,
            None => {
                unmeasured.push(object.clone());
                UNMEASURED_EXCURSION
            }
        };
        model_loss += notional.abs() * excursion;
    }

    vec![
        TailWorld {
            scenario: COUNTERPARTY_FAILURE.to_string(),
            at,
            loss_fraction: counterparty_loss / equity,
            loss: counterparty_loss,
            unmeasured: Vec::new(),
            detail: format!(
                "the broker custodying {gross:.0} of gross fails and {:.0}% of it is not \
                 recovered at the point of failure",
                COUNTERPARTY_LOSS_GIVEN_DEFAULT * 100.0
            ),
        },
        TailWorld {
            scenario: MODEL_FAILURE.to_string(),
            at,
            loss_fraction: model_loss / equity,
            loss: model_loss,
            detail: format!(
                "every position suffers its own worst retained adverse excursion at once with \
                 no diversification or hedge credit; {} position(s) without history charged \
                 {:.0}%",
                unmeasured.len(),
                UNMEASURED_EXCURSION * 100.0
            ),
            unmeasured,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn world<'a>(worlds: &'a [TailWorld], name: &str) -> &'a TailWorld {
        worlds
            .iter()
            .find(|w| w.scenario == name)
            .unwrap_or_else(|| panic!("no {name} world"))
    }

    #[test]
    fn a_levered_book_does_not_survive_its_broker_and_an_unlevered_one_does() {
        let history = BTreeMap::new();
        let unlevered = worlds(
            &[("obj-AAA".into(), 900_000.0)],
            &history,
            1_000_000.0,
            at(),
        );
        let levered = worlds(
            &[("obj-AAA".into(), 2_000_000.0)],
            &history,
            1_000_000.0,
            at(),
        );
        let calm = world(&unlevered, COUNTERPARTY_FAILURE);
        let stressed = world(&levered, COUNTERPARTY_FAILURE);
        assert!((calm.loss_fraction - 0.18).abs() < 1e-12);
        assert!(!calm.breaches(0.20), "premise: an unlevered book survives");
        assert!((stressed.loss_fraction - 0.40).abs() < 1e-12);
        assert!(stressed.breaches(0.20));
    }

    #[test]
    fn a_hedged_pair_the_factor_model_nets_still_loses_when_the_model_fails() {
        // Long AAA and short BBB at equal notional: a single-factor shock
        // nets the two to nothing. The model-failure world does not net them.
        let history = BTreeMap::from([
            ("obj-AAA".to_string(), vec![100.0, 110.0, 88.0, 95.0]),
            ("obj-BBB".to_string(), vec![50.0, 45.0, 60.0, 55.0]),
        ]);
        let positions = [
            ("obj-AAA".to_string(), 500_000.0),
            ("obj-BBB".to_string(), -500_000.0),
        ];
        let failed = world(
            &worlds(&positions, &history, 1_000_000.0, at()),
            MODEL_FAILURE,
        )
        .clone();
        // AAA: 110 -> 88 is a 20% fall. BBB short: 45 -> 60 is a 33.3% rise.
        let expected = (500_000.0 * 0.20 + 500_000.0 * (15.0 / 45.0)) / 1_000_000.0;
        assert!((failed.loss_fraction - expected).abs() < 1e-9);
        assert!(failed.unmeasured.is_empty());
    }

    #[test]
    fn a_position_without_history_is_charged_rather_than_ignored() {
        let failed = world(
            &worlds(
                &[("obj-NEW".into(), 100_000.0)],
                &BTreeMap::new(),
                1_000_000.0,
                at(),
            ),
            MODEL_FAILURE,
        )
        .clone();
        assert_eq!(failed.unmeasured, vec!["obj-NEW".to_string()]);
        assert!((failed.loss_fraction - 0.05).abs() < 1e-12);
    }

    #[test]
    fn no_equity_or_no_book_produces_no_world() {
        assert!(worlds(&[("obj-AAA".into(), 1.0)], &BTreeMap::new(), 0.0, at()).is_empty());
        assert!(worlds(&[], &BTreeMap::new(), 1.0, at()).is_empty());
    }
}
