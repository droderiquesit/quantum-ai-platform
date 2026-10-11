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
//! [`survival_worlds`] adds the rest of blueprint §14.3's list — a liquidity
//! freeze that removes depth rather than shocking a factor, a venue outage, a
//! gap move, a correlation inversion over the book's own covariance, data
//! poisoning of the marks, and a simultaneous multi-market shock — and two
//! worlds *generated* each cycle beyond the book's own history (R12-RISK-4116
//! through 4120, 4123, 4124). Every one reaches the survival kernel the same
//! way the two above do.
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

// ---- The §14.3 survival worlds (R12-RISK-4116..4120, 4123, 4124) ----------
//
// The two worlds above answer "what if the counterparty or the model fails".
// The six below answer the rest of §14.3's list, each a world the factor
// library expresses badly or not at all, and two more are *generated* every
// cycle rather than written down. Every one joins `StressReport::tail_worlds`,
// so the survival kernel's worst-world reserve reads it: a world the book does
// not survive withholds capital (`crate::survival::tail_response`), and the
// reserve and the world's name reach DECIDE's detail line. That is the
// "recorded capital response" each requirement asks for, and it is the same
// path for all of them on purpose — six bespoke responses would be six
// enforcement paths beside the Risk Gate, which R12-RISK-7205 forbids.

/// Depth removed from every market the book trades: the exit that normally
/// takes `days_to_liquidate` takes [`FREEZE_HORIZON_MULTIPLE`] times that.
/// Distinct from the library's `liquidity-freeze`, which is a factor shock
/// and never asks how long the book takes to leave.
pub const LIQUIDITY_FREEZE_DEPTH: &str = "liquidity-freeze-depth";

/// The venue the book is held at goes dark: nothing can be exited or hedged.
pub const VENUE_OUTAGE: &str = "venue-outage";

/// Every position gaps against itself in one print, with no price between.
pub const GAP_MOVE: &str = "gap-move";

/// The correlations the book's offsets rely on change sign.
pub const CORRELATION_INVERSION: &str = "correlation-inversion";

/// The marks the book was sized on were poisoned.
pub const DATA_POISONING: &str = "data-poisoning";

/// Every market the book touches is shocked at once, longs and shorts both.
pub const MULTI_MARKET_SHOCK: &str = "multi-market-shock";

/// The generated worlds' names. Fixed literals, so the `scenario` label stays
/// bounded however many cycles generate them.
pub const SYNTHETIC_WORLDS: [&str; 2] = ["synthetic-1", "synthetic-2"];

/// The one-sided 99% normal quantile, for the worlds that turn a volatility
/// into a loss.
pub const TAIL_QUANTILE: f64 = 2.326;

/// How many times longer an exit takes when depth is gone.
pub const FREEZE_HORIZON_MULTIPLE: f64 = 5.0;

/// The longest a frozen exit is charged for: a month of sessions. Past that
/// the world is no longer a freeze but a closed market.
pub const FREEZE_HORIZON_CAP_DAYS: f64 = 21.0;

/// The days-to-liquidate charged to a position the liquidity ladder did not
/// mark: a fortnight, because an unmarked position is the one whose exit
/// nobody has measured.
pub const UNMARKED_LIQUIDATION_DAYS: f64 = 10.0;

/// Spread and impact paid on the way out of a frozen market, as a fraction
/// of notional.
pub const FREEZE_EXIT_COST: f64 = 0.02;

/// How long the venue stays dark, in trading days.
pub const VENUE_OUTAGE_DAYS: f64 = 3.0;

/// A gap, in multiples of the position's own daily volatility.
pub const GAP_MULTIPLE: f64 = 5.0;

/// The horizon the inverted-correlation world is measured over, in days.
pub const CORRELATION_HORIZON_DAYS: f64 = 10.0;

/// The fall every position takes in the simultaneous multi-market world,
/// whichever side it is on.
pub const MULTI_MARKET_MOVE: f64 = 0.15;

/// The daily volatility charged to a position with too little history to
/// measure one: five per cent, the volatility of an instrument nobody has
/// watched long enough to call quiet.
pub const UNMEASURED_DAILY_VOLATILITY: f64 = 0.05;

/// The generated worlds amplify each position's worst retained excursion by
/// a factor drawn from this range — always above one, so every generated
/// world is outside the history the book has actually seen.
pub const SYNTHETIC_AMPLIFICATION: (f64, f64) = (1.25, 2.0);

/// One position as the generated worlds read it.
#[derive(Clone, Debug, PartialEq)]
pub struct TailPosition {
    pub object: String,
    /// Signed notional at cost, as the two worlds above take it.
    pub notional: f64,
    /// The liquidity ladder's days to exit, where it marked the position.
    pub days_to_liquidate: Option<f64>,
}

/// Everything the generated worlds read, from the platform at SIMULATE.
#[derive(Clone, Copy, Debug)]
pub struct TailBook<'a> {
    pub positions: &'a [TailPosition],
    pub price_history: &'a BTreeMap<String, Vec<f64>>,
    pub equity: f64,
    /// What makes this cycle's synthetic worlds this cycle's: the cycle
    /// number. A replay of the cycle generates the same worlds.
    pub seed: u64,
}

fn usable(prices: &[f64]) -> Vec<f64> {
    prices
        .iter()
        .copied()
        .filter(|p| p.is_finite() && *p > 0.0)
        .collect()
}

fn returns(prices: &[f64]) -> Vec<f64> {
    let usable = usable(prices);
    usable
        .windows(2)
        .map(|pair| pair[1] / pair[0] - 1.0)
        .collect()
}

/// Sample standard deviation of simple returns. `None` below two returns.
fn daily_volatility(prices: &[f64]) -> Option<f64> {
    let returns = returns(prices);
    if returns.len() < 2 {
        return None;
    }
    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
    Some(variance.sqrt())
}

/// The largest single-step move against the position its history contains:
/// the size of spike a poisoned feed could hide inside without looking out of
/// character.
fn largest_adverse_step(prices: &[f64], long: bool) -> Option<f64> {
    let returns = returns(prices);
    if returns.is_empty() {
        return None;
    }
    Some(
        returns
            .iter()
            .map(|r| if long { -r } else { *r })
            .fold(0.0_f64, f64::max),
    )
}

/// A small deterministic generator, so a cycle's synthetic worlds replay.
fn next_unit(state: &mut u64) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    // 53 bits into [0, 1).
    (*state >> 11) as f64 / (1_u64 << 53) as f64
}

fn world(
    scenario: &str,
    at: Timestamp,
    loss: f64,
    equity: f64,
    unmeasured: Vec<String>,
    detail: String,
) -> TailWorld {
    TailWorld {
        scenario: scenario.to_string(),
        at,
        loss_fraction: loss / equity,
        loss,
        unmeasured,
        detail,
    }
}

/// The six named §14.3 worlds and the generated ones, over `book`. Empty for
/// no equity or no position, as [`worlds`] is.
pub fn survival_worlds(book: &TailBook<'_>, at: Timestamp) -> Vec<TailWorld> {
    let equity = book.equity;
    if !(equity.is_finite() && equity > 0.0) || book.positions.is_empty() {
        return Vec::new();
    }
    let held: Vec<&TailPosition> = book
        .positions
        .iter()
        .filter(|position| position.notional != 0.0 && position.notional.is_finite())
        .collect();
    let gross: f64 = held.iter().map(|position| position.notional.abs()).sum();
    let history = |object: &str| book.price_history.get(object).map(Vec::as_slice);

    // Volatility per position, or the unmeasured charge and its name.
    let mut unmeasured_vol = Vec::new();
    let vols: Vec<f64> = held
        .iter()
        .map(
            |position| match history(&position.object).and_then(daily_volatility) {
                Some(vol) => vol,
                None => {
                    unmeasured_vol.push(position.object.clone());
                    UNMEASURED_DAILY_VOLATILITY
                }
            },
        )
        .collect();

    let mut out = Vec::new();

    // Liquidity freeze with depth removed.
    let mut unmarked = Vec::new();
    let freeze: f64 = held
        .iter()
        .zip(&vols)
        .map(|(position, vol)| {
            let days = match position.days_to_liquidate {
                Some(days) if days.is_finite() && days >= 0.0 => days,
                _ => {
                    unmarked.push(position.object.clone());
                    UNMARKED_LIQUIDATION_DAYS
                }
            };
            let frozen = (days.max(1.0) * FREEZE_HORIZON_MULTIPLE).min(FREEZE_HORIZON_CAP_DAYS);
            position.notional.abs() * (FREEZE_EXIT_COST + TAIL_QUANTILE * vol * frozen.sqrt())
        })
        .sum();
    out.push(world(
        LIQUIDITY_FREEZE_DEPTH,
        at,
        freeze,
        equity,
        unmarked.clone(),
        format!(
            "depth leaves every market: each exit takes {FREEZE_HORIZON_MULTIPLE}x its ladder \
             days (at most {FREEZE_HORIZON_CAP_DAYS}), bearing a {TAIL_QUANTILE} sigma move \
             over that horizon and {:.0}% exit cost; {} position(s) without a ladder mark \
             charged {UNMARKED_LIQUIDATION_DAYS} days",
            FREEZE_EXIT_COST * 100.0,
            unmarked.len()
        ),
    ));

    // Venue outage: nothing can be exited or hedged while it lasts.
    let outage: f64 = held
        .iter()
        .zip(&vols)
        .map(|(position, vol)| {
            position.notional.abs() * TAIL_QUANTILE * vol * VENUE_OUTAGE_DAYS.sqrt()
        })
        .sum();
    out.push(world(
        VENUE_OUTAGE,
        at,
        outage,
        equity,
        unmeasured_vol.clone(),
        format!(
            "the venue holding the book is dark for {VENUE_OUTAGE_DAYS} days: nothing exits, no \
             hedge goes on, and each position bears a {TAIL_QUANTILE} sigma move over the \
             outage"
        ),
    ));

    // Gap move: one print, no price in between for a stop to fill at.
    let gap: f64 = held
        .iter()
        .zip(&vols)
        .map(|(position, vol)| {
            let move_ = GAP_MULTIPLE * vol;
            // A long cannot lose more than it holds; a short can.
            let move_ = if position.notional > 0.0 {
                move_.min(1.0)
            } else {
                move_
            };
            position.notional.abs() * move_
        })
        .sum();
    out.push(world(
        GAP_MOVE,
        at,
        gap,
        equity,
        unmeasured_vol.clone(),
        format!(
            "every position gaps {GAP_MULTIPLE} daily sigmas against itself in one print, with \
             no price between for a stop to fill at"
        ),
    ));

    // Correlation inversion over the book's own covariance.
    out.push(correlation_inversion(book, &held, at));

    // Data poisoning: the marks were wrong by the largest spike history hides.
    let mut unpoisonable = Vec::new();
    let poisoned: f64 = held
        .iter()
        .map(|position| {
            let step = history(&position.object)
                .and_then(|prices| largest_adverse_step(prices, position.notional > 0.0));
            let step = match step {
                Some(step) => step,
                None => {
                    unpoisonable.push(position.object.clone());
                    UNMEASURED_EXCURSION
                }
            };
            position.notional.abs() * step
        })
        .sum();
    out.push(world(
        DATA_POISONING,
        at,
        poisoned,
        equity,
        unpoisonable,
        "every mark the book was sized on was off, against the position, by the largest \
         single-step move its own history contains — the spike a poisoned feed can hide in"
            .to_string(),
    ));

    // Simultaneous multi-market shock: no netting, longs and shorts both lose.
    out.push(world(
        MULTI_MARKET_SHOCK,
        at,
        gross * MULTI_MARKET_MOVE,
        equity,
        Vec::new(),
        format!(
            "every market the book touches moves {:.0}% against every position at once; longs \
             fall, shorts squeeze, and nothing nets",
            MULTI_MARKET_MOVE * 100.0
        ),
    ));

    // Generated worlds: each position's worst retained excursion, amplified
    // past anything its history holds, by a draw this cycle's seed fixes.
    let mut state = book.seed ^ 0x9E37_79B9_7F4A_7C15;
    if state == 0 {
        state = 0x2545_F491_4F6C_DD1D;
    }
    let (low, high) = SYNTHETIC_AMPLIFICATION;
    for name in SYNTHETIC_WORLDS {
        let mut unmeasured = Vec::new();
        let mut loss = 0.0;
        for position in &held {
            let amplification = low + (high - low) * next_unit(&mut state);
            let excursion = history(&position.object)
                .and_then(|prices| adverse_excursion(prices, position.notional > 0.0));
            let move_ = match excursion {
                Some(excursion) => excursion * amplification,
                None => {
                    unmeasured.push(position.object.clone());
                    UNMEASURED_EXCURSION
                }
            };
            let move_ = if position.notional > 0.0 {
                move_.min(1.0)
            } else {
                move_
            };
            loss += position.notional.abs() * move_;
        }
        out.push(world(
            name,
            at,
            loss,
            equity,
            unmeasured,
            format!(
                "generated for cycle seed {}: every position's worst retained excursion \
                 amplified by a draw in [{low}, {high}], so the world lies outside the history \
                 the book has seen",
                book.seed
            ),
        ));
    }
    out
}

/// The correlation-inversion world: the book's variance with every
/// off-diagonal correlation's sign flipped, over the window every measured
/// position shares, taken against the calm variance — whichever is worse —
/// and turned into a loss at [`TAIL_QUANTILE`] over
/// [`CORRELATION_HORIZON_DAYS`]. A book whose offsets rely on correlation
/// (long one, short its twin) is the book this world finds; a long-only book
/// that relies on none reads its calm figure. Positions without enough history
/// are charged [`UNMEASURED_EXCURSION`] beside it, not left out.
fn correlation_inversion(book: &TailBook<'_>, held: &[&TailPosition], at: Timestamp) -> TailWorld {
    let mut unmeasured = Vec::new();
    let mut series: Vec<(f64, Vec<f64>)> = Vec::new();
    for position in held {
        let rets = book
            .price_history
            .get(&position.object)
            .map(|prices| returns(prices))
            .unwrap_or_default();
        if rets.len() < 2 {
            unmeasured.push(position.object.clone());
        } else {
            series.push((position.notional, rets));
        }
    }
    let window = series.iter().map(|(_, rets)| rets.len()).min().unwrap_or(0);
    let mut measured_loss = 0.0;
    if window >= 2 {
        let tails: Vec<(f64, &[f64])> = series
            .iter()
            .map(|(notional, rets)| (*notional, &rets[rets.len() - window..]))
            .collect();
        let n = window as f64;
        let stats: Vec<(f64, f64)> = tails
            .iter()
            .map(|(_, rets)| {
                let mean = rets.iter().sum::<f64>() / n;
                let sd = (rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();
                (mean, sd)
            })
            .collect();
        let mut calm = 0.0;
        let mut inverted = 0.0;
        for (i, (wi, ri)) in tails.iter().enumerate() {
            for (j, (wj, rj)) in tails.iter().enumerate() {
                let (mi, si) = stats[i];
                let (mj, sj) = stats[j];
                let cov = ri
                    .iter()
                    .zip(rj.iter())
                    .map(|(a, b)| (a - mi) * (b - mj))
                    .sum::<f64>()
                    / (n - 1.0);
                calm += wi * wj * cov;
                inverted += if i == j {
                    wi * wj * si * sj
                } else {
                    -wi * wj * cov
                };
            }
        }
        let variance = calm.max(inverted);
        if variance.is_finite() && variance > 0.0 {
            measured_loss = TAIL_QUANTILE * variance.sqrt() * CORRELATION_HORIZON_DAYS.sqrt();
        }
    } else {
        for (notional, _) in &series {
            measured_loss += notional.abs() * UNMEASURED_EXCURSION;
        }
    }
    let unmeasured_loss: f64 = held
        .iter()
        .filter(|position| unmeasured.contains(&position.object))
        .map(|position| position.notional.abs() * UNMEASURED_EXCURSION)
        .sum();
    world(
        CORRELATION_INVERSION,
        at,
        measured_loss + unmeasured_loss,
        book.equity,
        unmeasured,
        format!(
            "every correlation between the book's positions changes sign over a shared window \
             of {window} returns; the book bears a {TAIL_QUANTILE} sigma move of the worse of \
             its calm and inverted variance over {CORRELATION_HORIZON_DAYS} days"
        ),
    )
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

    fn book_of(
        positions: &[TailPosition],
        history: &BTreeMap<String, Vec<f64>>,
        seed: u64,
    ) -> Vec<TailWorld> {
        survival_worlds(
            &TailBook {
                positions,
                price_history: history,
                equity: 1_000_000.0,
                seed,
            },
            at(),
        )
    }

    fn unmeasured(object: &str, notional: f64) -> TailPosition {
        TailPosition {
            object: object.to_string(),
            notional,
            days_to_liquidate: None,
        }
    }

    /// R12-RISK-4117..4120, 4123, 4124: every named world is generated, and
    /// the ones with closed forms carry the figure their constants state.
    /// Mutation: change `VENUE_OUTAGE_DAYS.sqrt()` to `1.0` and the outage
    /// figure fails; drop any `out.push` and its lookup fails.
    #[test]
    fn every_named_survival_world_is_generated_with_the_loss_its_assumptions_state() {
        let positions = [unmeasured("obj-NEW", 100_000.0)];
        let worlds = book_of(&positions, &BTreeMap::new(), 1);
        for name in [
            LIQUIDITY_FREEZE_DEPTH,
            VENUE_OUTAGE,
            GAP_MOVE,
            CORRELATION_INVERSION,
            DATA_POISONING,
            MULTI_MARKET_SHOCK,
        ] {
            let found = world(&worlds, name);
            assert!(
                found.loss > 0.0,
                "{name} charged nothing to an unmeasured book"
            );
        }
        let outage = world(&worlds, VENUE_OUTAGE);
        let expected =
            100_000.0 * TAIL_QUANTILE * UNMEASURED_DAILY_VOLATILITY * VENUE_OUTAGE_DAYS.sqrt();
        assert!((outage.loss - expected).abs() < 1e-6, "{}", outage.loss);
        assert_eq!(outage.unmeasured, vec!["obj-NEW".to_string()]);
        let gap = world(&worlds, GAP_MOVE);
        assert!((gap.loss - 100_000.0 * GAP_MULTIPLE * UNMEASURED_DAILY_VOLATILITY).abs() < 1e-6);
        let shock = world(&worlds, MULTI_MARKET_SHOCK);
        assert!((shock.loss_fraction - 0.1 * MULTI_MARKET_MOVE).abs() < 1e-12);
        let freeze = world(&worlds, LIQUIDITY_FREEZE_DEPTH);
        assert_eq!(
            freeze.unmeasured,
            vec!["obj-NEW".to_string()],
            "unmarked named"
        );
    }

    /// The correlation-inversion world finds the book whose offset relies on
    /// correlation: a long and a short in two instruments that move together
    /// lose far more when the correlation flips than a long-only book of the
    /// same gross. Mutation: take `calm` instead of `calm.max(inverted)` and
    /// the hedged pair reads as nearly riskless.
    #[test]
    fn a_pair_that_relies_on_correlation_is_what_the_inversion_world_finds() {
        let a: Vec<f64> = (0..40)
            .map(|i| 100.0 * (1.0 + 0.02 * ((i as f64) * 1.7).sin()))
            .collect();
        let b: Vec<f64> = a.iter().map(|p| p * 0.5).collect();
        let history = BTreeMap::from([("obj-A".to_string(), a.clone()), ("obj-B".to_string(), b)]);
        let pair = [
            TailPosition {
                object: "obj-A".into(),
                notional: 500_000.0,
                days_to_liquidate: Some(1.0),
            },
            TailPosition {
                object: "obj-B".into(),
                notional: -500_000.0,
                days_to_liquidate: Some(1.0),
            },
        ];
        let inverted = world(&book_of(&pair, &history, 1), CORRELATION_INVERSION).clone();
        let gap = world(&book_of(&pair, &history, 1), GAP_MOVE).clone();
        assert!(inverted.unmeasured.is_empty(), "premise: both measured");
        // Perfectly correlated legs: calm variance is zero, inverted variance
        // is the two legs adding. The world must charge the inverted figure.
        assert!(
            inverted.loss > 0.5 * gap.loss,
            "the inversion world read the hedged pair as safe: {} against a gap of {}",
            inverted.loss,
            gap.loss
        );
    }

    /// R12-RISK-4116: the generated worlds lie beyond the book's own history,
    /// differ from cycle to cycle, and replay exactly for the same cycle.
    /// Mutation: set `SYNTHETIC_AMPLIFICATION` to `(1.0, 1.0)` and the
    /// beyond-history assertion fails; seed the generator with a constant and
    /// the cycles stop differing.
    #[test]
    fn generated_worlds_exceed_the_books_history_and_replay_for_the_same_cycle() {
        let history =
            BTreeMap::from([("obj-AAA".to_string(), vec![100.0, 110.0, 88.0, 95.0, 101.0])]);
        let positions = [TailPosition {
            object: "obj-AAA".into(),
            notional: 500_000.0,
            days_to_liquidate: Some(1.0),
        }];
        let historical = world(
            &worlds(
                &[("obj-AAA".into(), 500_000.0)],
                &history,
                1_000_000.0,
                at(),
            ),
            MODEL_FAILURE,
        )
        .loss;
        let first = book_of(&positions, &history, 7);
        let replay = book_of(&positions, &history, 7);
        let next = book_of(&positions, &history, 8);
        for name in SYNTHETIC_WORLDS {
            let generated = world(&first, name);
            assert!(
                generated.loss > historical,
                "{name} at {} is inside the worst the book has seen ({historical})",
                generated.loss
            );
            assert_eq!(generated, world(&replay, name), "{name} did not replay");
        }
        assert!(
            SYNTHETIC_WORLDS
                .iter()
                .any(|name| (world(&first, name).loss - world(&next, name).loss).abs() > 1e-6),
            "two cycles generated the same worlds"
        );
    }
}
