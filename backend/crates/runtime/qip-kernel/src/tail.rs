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
use qip_simulation_engine::scenario::{FactorShock, Scenario};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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

/// The volatility regime flips: every position's own volatility doubles and
/// the offsets the book counted on stop netting (R12-RISK-4116).
pub const REGIME_FLIP: &str = "regime-flip";

/// One position's stress cascades along measured correlations into the rest
/// of the book (R12-RISK-4102).
pub const CONTAGION_CASCADE: &str = "contagion-cascade";

/// How many times a position's measured daily volatility the regime-flip
/// world charges: the top of [`SYNTHETIC_AMPLIFICATION`], so the flipped
/// regime is at least as severe as the harshest world already generated from
/// the book's own history. It is a multiple of a measured figure, never a
/// replacement for one, so the world cannot read milder than the calm
/// regime for any position.
pub const REGIME_FLIP_VOL_MULTIPLE: f64 = 2.0;

/// The fewest shared returns a pair needs before its correlation is
/// believed. Below it the pair is charged as perfectly correlated
/// (`1.0`) in the contagion world: an unmeasurable link is the strictest
/// one, never the absent one.
pub const CONTAGION_MIN_SHARED_RETURNS: usize = 3;

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

/// The generated joint world (R12-RISK-4116): a regime flip and a
/// correlation structure no history supplied, drawn together. Each position's
/// volatility is [`REGIME_FLIP_VOL_MULTIPLE`] times its measured figure times
/// a draw in [`SYNTHETIC_AMPLIFICATION`], and every pair is given a drawn
/// correlation of magnitude in [`JOINT_CORRELATION_MAGNITUDE`] with the sign
/// that hurts the book: positive between two longs or two shorts, negative
/// between a long and a short, so a hedge loses on both legs at once.
pub const SYNTHETIC_JOINT: &str = "synthetic-joint";

/// The range a generated joint world's pairwise correlation magnitude is
/// drawn from. The floor is what keeps the world past the regime flip: with
/// every pair at least this correlated against the book, the joint loss is at
/// least `sqrt(0.75)` of the undiversified sum of the amplified legs, and the
/// amplification floor of 1.25 times `sqrt(0.75)` is above one — so wherever
/// no long's move is capped at its whole notional, this world is worse than
/// [`REGIME_FLIP`] for every book.
pub const JOINT_CORRELATION_MAGNITUDE: (f64, f64) = (0.75, 1.0);

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

    // Regime flip: volatility doubles and nothing nets. Joint, because every
    // position bears its flipped-regime move at once, whichever side it is on.
    let flipped: f64 = held
        .iter()
        .zip(&vols)
        .map(|(position, vol)| {
            let move_ =
                TAIL_QUANTILE * (REGIME_FLIP_VOL_MULTIPLE * vol) * CORRELATION_HORIZON_DAYS.sqrt();
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
        REGIME_FLIP,
        at,
        flipped,
        equity,
        unmeasured_vol.clone(),
        format!(
            "the volatility regime flips: every position's volatility is {REGIME_FLIP_VOL_MULTIPLE}x \
             its measured figure, and its {TAIL_QUANTILE} sigma move over \
             {CORRELATION_HORIZON_DAYS} days is charged against it with no netting"
        ),
    ));

    out.push(contagion_cascade(book, &held, at));

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

/// The generated joint world over `book` ([`SYNTHETIC_JOINT`]), or `None`
/// for no equity or no position, as [`survival_worlds`] is.
///
/// Deliberately **not** one of [`survival_worlds`]: the survival reserve and
/// hedge selection read those, and this world is usually the worst of them,
/// so applying it raises the capital the survival governor withholds. That
/// is a change to a financial risk control and awaits the owner's
/// authorisation; until then the platform computes it, reports it and its
/// would-be reserve as advisory, and applies it only where a composition
/// root has turned on `Platform::set_joint_tail_reserve_applied`, which none
/// does.
pub fn joint_world(book: &TailBook<'_>, at: Timestamp) -> Option<TailWorld> {
    let equity = book.equity;
    if !(equity.is_finite() && equity > 0.0) || book.positions.is_empty() {
        return None;
    }
    let held: Vec<&TailPosition> = book
        .positions
        .iter()
        .filter(|position| position.notional != 0.0 && position.notional.is_finite())
        .collect();
    let mut unmeasured = Vec::new();
    let vols: Vec<f64> = held
        .iter()
        .map(|position| {
            match book
                .price_history
                .get(&position.object)
                .map(Vec::as_slice)
                .and_then(daily_volatility)
            {
                Some(vol) => vol,
                None => {
                    unmeasured.push(position.object.clone());
                    UNMEASURED_DAILY_VOLATILITY
                }
            }
        })
        .collect();
    Some(synthetic_joint(book, &held, &vols, &unmeasured, at))
}

/// See [`SYNTHETIC_JOINT`]. Statistics, so `f64`. The pairwise
/// correlations are drawn independently, so together they need not form a
/// valid correlation matrix: the world is a conservative bound on a joint
/// move, not a coherent market, and every pair term is added as a loss.
/// Like every survival world it is skipped where SIMULATE skips the stress
/// (too little history for the factor model, no equity, no position).
///
/// The generator is seeded
/// from the cycle as the other generated worlds are, but on its own stream,
/// so adding this world did not move the draws behind theirs.
fn synthetic_joint(
    book: &TailBook<'_>,
    held: &[&TailPosition],
    vols: &[f64],
    unmeasured: &[String],
    at: Timestamp,
) -> TailWorld {
    let mut state = book.seed ^ 0xD1B5_4A32_D192_ED03;
    if state == 0 {
        state = 0x2545_F491_4F6C_DD1D;
    }
    let (low, high) = SYNTHETIC_AMPLIFICATION;
    // Each leg's stressed loss on its own, as the regime flip charges it.
    let legs: Vec<f64> = held
        .iter()
        .zip(vols)
        .map(|(position, vol)| {
            let amplification = low + (high - low) * next_unit(&mut state);
            let move_ = TAIL_QUANTILE
                * (REGIME_FLIP_VOL_MULTIPLE * amplification * vol)
                * CORRELATION_HORIZON_DAYS.sqrt();
            let move_ = if position.notional > 0.0 {
                move_.min(1.0)
            } else {
                move_
            };
            position.notional.abs() * move_
        })
        .collect();
    let (floor, ceiling) = JOINT_CORRELATION_MAGNITUDE;
    // Every term is a loss: the drawn correlation takes the sign that makes
    // the pair's co-movement adverse, so the product of two leg losses is
    // added, never netted.
    let mut variance = 0.0;
    for (i, a) in legs.iter().enumerate() {
        variance += a * a;
        for b in &legs[i + 1..] {
            let magnitude = floor + (ceiling - floor) * next_unit(&mut state);
            variance += 2.0 * magnitude * a * b;
        }
    }
    // Fail closed: a variance that overflowed, or that a non-finite input
    // poisoned, is a world too large to measure, not one that costs nothing.
    // It reads as infinite, which `survival::worst_world` treats as the worst
    // world, rather than vanishing from the reserve as zero would.
    let loss = if variance.is_finite() {
        variance.max(0.0).sqrt()
    } else {
        f64::INFINITY
    };
    world(
        SYNTHETIC_JOINT,
        at,
        loss,
        book.equity,
        unmeasured.to_vec(),
        format!(
            "generated for cycle seed {}: volatility {REGIME_FLIP_VOL_MULTIPLE}x times a draw in \
             [{low}, {high}] per position, and every pair correlated against the book at a \
             drawn magnitude in [{floor}, {ceiling}], so hedges lose on both legs in a \
             correlation structure the book's history never showed",
            book.seed
        ),
    )
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

/// Correlation of two return series over their shared trailing window, or
/// `None` when the window is shorter than [`CONTAGION_MIN_SHARED_RETURNS`] or
/// either series is flat over it.
fn trailing_correlation(a: &[f64], b: &[f64]) -> Option<f64> {
    let window = a.len().min(b.len());
    if window < CONTAGION_MIN_SHARED_RETURNS {
        return None;
    }
    let (a, b) = (&a[a.len() - window..], &b[b.len() - window..]);
    let n = window as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut cov, mut va, mut vb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        cov += (x - ma) * (y - mb);
        va += (x - ma).powi(2);
        vb += (y - mb).powi(2);
    }
    let rho = cov / (va * vb).sqrt();
    rho.is_finite().then_some(rho.clamp(-1.0, 1.0))
}

/// The contagion world: the book's own correlation graph, rather than the
/// driver-only aggregation `qip_risk::shared_cause` offers, carries one
/// position's worst retained excursion to its neighbours.
///
/// Every held position is tried as the origin. It takes its own worst
/// excursion (or [`UNMEASURED_EXCURSION`] with no history); each neighbour
/// takes that move scaled by the absolute correlation of the pair, and a
/// second hop carries the larger of the two readings one step further. The
/// transmission is charged **against** every position whichever side it is
/// on — contagion is the world where hedges stop hedging — and a pair whose
/// correlation cannot be measured is charged as `1.0`, the strictest link.
/// The world's loss is the worst origin's. It is a statistic, so `f64`.
fn contagion_cascade(book: &TailBook<'_>, held: &[&TailPosition], at: Timestamp) -> TailWorld {
    let rets: Vec<Vec<f64>> = held
        .iter()
        .map(|position| {
            book.price_history
                .get(&position.object)
                .map(|prices| returns(prices))
                .unwrap_or_default()
        })
        .collect();
    let mut unmeasured: BTreeSet<String> = BTreeSet::new();
    let n = held.len();
    let mut link = vec![vec![1.0_f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            match trailing_correlation(&rets[i], &rets[j]) {
                Some(rho) => link[i][j] = rho.abs(),
                None => {
                    unmeasured.insert(held[i].object.clone());
                    unmeasured.insert(held[j].object.clone());
                }
            }
        }
    }
    let mut worst = 0.0_f64;
    let mut worst_origin = String::new();
    for (origin, position) in held.iter().enumerate() {
        let seed = book
            .price_history
            .get(&position.object)
            .and_then(|prices| adverse_excursion(prices, position.notional > 0.0))
            .unwrap_or(UNMEASURED_EXCURSION);
        let mut shock = vec![0.0_f64; n];
        shock[origin] = seed;
        // Two relaxation passes: the origin reaches its neighbours, and they
        // reach theirs. Each position keeps the larger of what it holds and
        // what a neighbour transmits, so a second hop never reduces a first.
        for _ in 0..2 {
            let carried = shock.clone();
            for (j, hit) in carried.iter().enumerate() {
                for k in 0..n {
                    if k != j {
                        shock[k] = shock[k].max(hit * link[j][k]);
                    }
                }
            }
        }
        let loss: f64 = held
            .iter()
            .zip(&shock)
            .map(|(held, move_)| {
                let move_ = if held.notional > 0.0 {
                    move_.min(1.0)
                } else {
                    *move_
                };
                held.notional.abs() * move_
            })
            .sum();
        if loss > worst {
            worst = loss;
            worst_origin = position.object.clone();
        }
    }
    world(
        CONTAGION_CASCADE,
        at,
        worst,
        book.equity,
        unmeasured.into_iter().collect(),
        format!(
            "{worst_origin} takes its worst retained excursion and carries it, scaled by \
             absolute return correlation, two hops through the book against every position; a \
             pair with fewer than {CONTAGION_MIN_SHARED_RETURNS} shared returns is charged as \
             perfectly correlated"
        ),
    )
}

/// The name the composed scenario is reported and charted under. A literal,
/// so the `scenario` label stays bounded by source text however the pair it
/// joins changes with the library.
pub const COMPOSED_SCENARIO: &str = "composed-unprecedented";

fn factor_set(scenario: &Scenario) -> BTreeSet<&str> {
    scenario
        .shocks
        .iter()
        .map(|shock| shock.factor.as_str())
        .collect()
}

/// A scenario no library entry holds: two library scenarios' shocks joined
/// into one event (R12-RISK-3103).
///
/// The generated worlds above amplify each position's *own* history, so they
/// never produce a combination of factors the library lacks. The library
/// itself shocks the factors one scenario at a time, so a book whose
/// exposures sit on, say, rates and commodity and fx together is stressed by
/// no entry that moves all three. This joins the pair whose union of shocked
/// factors is widest and equals no single entry's factor set. Where both
/// shock one factor the worse reading is kept — the larger magnitude over the
/// shorter window — so the join is never milder than either parent. The
/// liquidity multiplier and stressed correlation are the higher of the two
/// for the same reason. Pairs are walked in library order and a tie keeps the
/// first, so a replay composes the same event.
///
/// `None` when no pair yields a factor set the library lacks, rather than a
/// copy of an entry that would read as a new test.
pub fn composed_scenario(library: &[Scenario]) -> Option<Scenario> {
    let sets: Vec<BTreeSet<&str>> = library.iter().map(factor_set).collect();
    let mut best: Option<(usize, usize, usize)> = None;
    for (i, a) in sets.iter().enumerate() {
        for (j, b) in sets.iter().enumerate().skip(i + 1) {
            let union: BTreeSet<&str> = a.union(b).copied().collect();
            if sets.contains(&union) {
                continue;
            }
            if best.is_none_or(|(_, _, width)| union.len() > width) {
                best = Some((i, j, union.len()));
            }
        }
    }
    let (i, j, _) = best?;
    let (a, b) = (&library[i], &library[j]);
    let mut shocks: BTreeMap<&str, FactorShock> = BTreeMap::new();
    for shock in a.shocks.iter().chain(&b.shocks) {
        match shocks.get(shock.factor.as_str()) {
            Some(held) => {
                let worse = shock.magnitude.abs() > held.magnitude.abs();
                let magnitude = if worse {
                    shock.magnitude
                } else {
                    held.magnitude
                };
                let over_days = held.over_days.min(shock.over_days);
                shocks.insert(
                    shock.factor.as_str(),
                    FactorShock::new(shock.factor.clone(), magnitude, over_days),
                );
            }
            None => {
                shocks.insert(shock.factor.as_str(), shock.clone());
            }
        }
    }
    let scenario = Scenario {
        name: COMPOSED_SCENARIO.to_string(),
        description: format!(
            "Composed, never observed: {} and {} as one event, joining factor sets no library              entry shocks together",
            a.name, b.name
        ),
        shocks: shocks.into_values().collect(),
        stressed_correlation: match (a.stressed_correlation, b.stressed_correlation) {
            (Some(x), Some(y)) => Some(x.max(y)),
            (one, other) => one.or(other),
        },
        liquidity_multiplier: a.liquidity_multiplier.max(b.liquidity_multiplier),
        historical: false,
    };
    // A malformed join is no scenario at all, never a silently repaired one.
    scenario.validate().ok()?;
    Some(scenario)
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

    fn joint_of(
        positions: &[TailPosition],
        history: &BTreeMap<String, Vec<f64>>,
        seed: u64,
    ) -> TailWorld {
        joint_world(
            &TailBook {
                positions,
                price_history: history,
                equity: 1_000_000.0,
                seed,
            },
            at(),
        )
        .expect("premise: a held book with equity has a joint world")
    }

    fn series(start: f64, moves: &[f64]) -> Vec<f64> {
        let mut prices = vec![start];
        for move_ in moves {
            let last = *prices.last().expect("seeded");
            prices.push(last * (1.0 + move_));
        }
        prices
    }

    fn held(object: &str, notional: f64) -> TailPosition {
        TailPosition {
            object: object.to_string(),
            notional,
            days_to_liquidate: Some(1.0),
        }
    }

    /// R12-RISK-4116. The regime-flip world fires on a book that survives
    /// every calm reading and is admitted on a quiet one. Mutation: change
    /// `REGIME_FLIP_VOL_MULTIPLE * vol` to `vol` and the closed form fails;
    /// delete the push and the lookup panics.
    #[test]
    fn the_regime_flip_world_breaches_a_book_of_unwatched_names_and_spares_a_quiet_one() {
        // Premise: an unmeasured position is charged the unmeasured 5% daily
        // volatility, so 400,000 of 1,000,000 flips to a closed-form loss.
        let worlds = book_of(&[unmeasured("obj-NEW", 400_000.0)], &BTreeMap::new(), 1);
        let flipped = world(&worlds, REGIME_FLIP);
        let expected = 400_000.0
            * TAIL_QUANTILE
            * (REGIME_FLIP_VOL_MULTIPLE * UNMEASURED_DAILY_VOLATILITY)
            * CORRELATION_HORIZON_DAYS.sqrt();
        assert!((flipped.loss - expected).abs() < 1e-6, "{}", flipped.loss);
        assert!(flipped.breaches(0.20), "{}", flipped.loss_fraction);
        assert_eq!(flipped.unmeasured, vec!["obj-NEW".to_string()]);
        // Never milder than the calm regime's own outage reading.
        assert!(flipped.loss >= world(&worlds, VENUE_OUTAGE).loss);

        // A measured, quiet name at a small size is not breached.
        let quiet = series(100.0, &[0.001, -0.001, 0.001, -0.001, 0.001, -0.001]);
        let history = BTreeMap::from([("obj-Q".to_string(), quiet)]);
        let worlds = book_of(&[held("obj-Q", 50_000.0)], &history, 1);
        let flipped = world(&worlds, REGIME_FLIP);
        assert!(flipped.unmeasured.is_empty());
        assert!(!flipped.breaches(0.20), "{}", flipped.loss_fraction);
    }

    /// R12-RISK-4102. Contagion follows the measured correlation graph.
    /// Mutation: replace `link[origin][j]` with `0.0` and the correlated
    /// pair reads as only its origin's loss.
    #[test]
    fn the_contagion_world_carries_a_shock_along_correlation_and_not_across_independence() {
        let moves = [0.10, -0.10, 0.10, -0.10, 0.10, -0.10];
        let a = series(100.0, &moves);
        let b: Vec<f64> = a.iter().map(|p| p * 0.5).collect();
        // Returns alternating in pairs: exactly uncorrelated with `moves`.
        let c = series(100.0, &[0.10, 0.10, -0.10, -0.10, 0.10, 0.10]);
        let excursion = adverse_excursion(&a, true).expect("measurable");
        assert!(excursion > 0.0, "premise: the origin has an excursion");
        let rho = trailing_correlation(&returns(&a), &returns(&c)).expect("measurable");
        assert!(rho.abs() < 1e-9, "premise: independent returns, got {rho}");

        let both = BTreeMap::from([("obj-A".to_string(), a.clone()), ("obj-B".to_string(), b)]);
        let correlated = book_of(
            &[held("obj-A", 300_000.0), held("obj-B", 300_000.0)],
            &both,
            1,
        );
        let loss = world(&correlated, CONTAGION_CASCADE).loss;
        assert!((loss - 600_000.0 * excursion).abs() < 1e-6, "{loss}");

        let apart = BTreeMap::from([("obj-A".to_string(), a), ("obj-C".to_string(), c)]);
        let independent = book_of(
            &[held("obj-A", 300_000.0), held("obj-C", 300_000.0)],
            &apart,
            1,
        );
        let alone = world(&independent, CONTAGION_CASCADE).loss;
        assert!(
            alone < loss,
            "independence carried contagion: {alone} vs {loss}"
        );
    }

    /// R12-RISK-4102. A pair that cannot be measured is the strictest link,
    /// never the absent one. Mutation: initialise `link` to `0.0`.
    #[test]
    fn an_unmeasurable_pair_is_charged_as_perfectly_correlated() {
        let a = series(100.0, &[0.10, -0.10, 0.10, -0.10]);
        let history = BTreeMap::from([("obj-A".to_string(), a)]);
        let worlds = book_of(
            &[held("obj-A", 200_000.0), unmeasured("obj-NEW", 200_000.0)],
            &history,
            1,
        );
        let cascade = world(&worlds, CONTAGION_CASCADE);
        // obj-NEW has no history: its seed is the unmeasured excursion and
        // it reaches obj-A at full strength.
        assert!(
            (cascade.loss - 400_000.0 * UNMEASURED_EXCURSION).abs() < 1e-6,
            "{}",
            cascade.loss
        );
        assert_eq!(
            cascade.unmeasured,
            vec!["obj-A".to_string(), "obj-NEW".to_string()]
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

    /// R12-RISK-4116: for a hedged pair whose history says the legs move
    /// together, the generated joint world is worse than every named world
    /// the same stress produces — the regime flip, the correlation inversion
    /// and the contagion cascade included — and than the counterparty and
    /// model-failure worlds, so SIMULATE stresses the book outside the
    /// library's bounds. It replays for the same cycle and differs across
    /// cycles.
    ///
    /// Mutation: give the pair a calm, history-signed correlation instead of
    /// the adverse one (`variance -= 2.0 * magnitude * a * b`) and the hedge
    /// nets, so the joint world falls inside the library.
    #[test]
    fn a_generated_joint_world_lies_outside_every_library_world_for_a_hedged_pair() {
        let a = series(
            100.0,
            &[0.03, -0.025, 0.02, -0.03, 0.025, -0.02, 0.03, -0.025],
        );
        let b = series(
            50.0,
            &[0.028, -0.024, 0.021, -0.029, 0.026, -0.019, 0.031, -0.024],
        );
        let history = BTreeMap::from([("obj-AAA".to_string(), a), ("obj-BBB".to_string(), b)]);
        let positions = [held("obj-AAA", 500_000.0), held("obj-BBB", -500_000.0)];
        let generated = book_of(&positions, &history, 11);
        let joint = joint_of(&positions, &history, 11);
        let library = worlds(
            &[
                ("obj-AAA".into(), 500_000.0),
                ("obj-BBB".into(), -500_000.0),
            ],
            &history,
            1_000_000.0,
            at(),
        );
        let others: Vec<&TailWorld> = generated
            .iter()
            .chain(library.iter())
            .filter(|w| !SYNTHETIC_WORLDS.contains(&w.scenario.as_str()))
            .collect();
        assert!(
            others.iter().any(|w| w.scenario == REGIME_FLIP)
                && others.iter().any(|w| w.scenario == CORRELATION_INVERSION)
                && others.iter().any(|w| w.scenario == CONTAGION_CASCADE)
                && others.iter().any(|w| w.scenario == MODEL_FAILURE),
            "premise: the library worlds were generated: {:?}",
            others.iter().map(|w| &w.scenario).collect::<Vec<_>>()
        );
        assert!(
            joint.unmeasured.is_empty(),
            "premise: both legs are measured"
        );
        for other in &others {
            assert!(
                joint.loss > other.loss,
                "the joint world ({}) is inside {} ({})",
                joint.loss,
                other.scenario,
                other.loss
            );
        }
        assert!(
            generated.iter().all(|w| w.scenario != SYNTHETIC_JOINT),
            "the joint world is advisory and never one of the applied survival worlds"
        );
        assert_eq!(joint, joint_of(&positions, &history, 11));
        assert!(
            (joint.loss - joint_of(&positions, &history, 12).loss).abs() > 1e-6,
            "two cycles generated the same joint world"
        );
    }

    /// R12-RISK-4116, the bound `JOINT_CORRELATION_MAGNITUDE` documents: on a
    /// book where no long's move is capped, the joint world is at least
    /// `1.25 * sqrt(0.75)` times the regime flip, because every pair is at
    /// least that correlated against the book. Eight independent longs are
    /// where a diversified correlation would undercut it most — at zero
    /// correlation the joint loss is about `sqrt(8)/8` of the summed legs —
    /// so this is the book that tells the drawn correlation from none.
    ///
    /// Mutation: set `JOINT_CORRELATION_MAGNITUDE` to `(0.0, 0.0)` and the
    /// joint world falls below the bound.
    #[test]
    fn the_joint_world_stays_past_the_regime_flip_by_its_correlation_floor_on_a_diversified_book() {
        let names: Vec<String> = (0..8).map(|i| format!("obj-L{i}")).collect();
        let history: BTreeMap<String, Vec<f64>> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
                let moves: Vec<f64> = (0..8)
                    .map(|k| sign * if k % 2 == 0 { 0.01 } else { -0.008 })
                    .collect();
                (name.clone(), series(100.0, &moves))
            })
            .collect();
        let positions: Vec<TailPosition> = names.iter().map(|name| held(name, 100_000.0)).collect();
        for seed in [3, 17, 99] {
            let generated = book_of(&positions, &history, seed);
            let flip = world(&generated, REGIME_FLIP).loss;
            let joint = joint_of(&positions, &history, seed).loss;
            assert!(flip > 0.0, "premise: the regime flip charges the book");
            // The floor written out, not read from the constants it checks.
            let bound = 1.25 * 0.75_f64.sqrt() * flip;
            assert!(
                joint >= bound - 1e-6,
                "seed {seed}: the joint world {joint} is below its documented floor {bound}"
            );
        }
    }

    /// A book so large its joint variance overflows reads as an infinite
    /// loss, the worst world, rather than a free one.
    ///
    /// Mutation: return `0.0` for a non-finite variance and the world reads
    /// as costing nothing.
    #[test]
    fn a_joint_variance_that_overflows_is_an_infinite_loss_not_a_free_world() {
        let history = BTreeMap::from([
            ("obj-AAA".to_string(), series(100.0, &[0.03, -0.02, 0.025])),
            ("obj-BBB".to_string(), series(100.0, &[-0.02, 0.03, -0.025])),
        ]);
        let positions = [held("obj-AAA", -1.0e200), held("obj-BBB", -1.0e200)];
        let joint = joint_of(&positions, &history, 5);
        assert!(
            joint.loss.is_infinite() && joint.loss > 0.0,
            "an overflowed joint world read as {}",
            joint.loss
        );
    }

    /// R12-RISK-3103: the composed scenario shocks a factor set the library
    /// holds nowhere, and is never milder than either parent on a shared
    /// factor. Mutation: return the first library entry unchanged and the
    /// absence assertion fails; keep the shorter magnitude on a shared factor
    /// and the severity assertion fails.
    #[test]
    fn the_composed_scenario_shocks_a_factor_set_no_library_entry_holds() {
        let library = qip_simulation_engine::scenario::standard_library();
        let composed = composed_scenario(&library).expect("the library admits a composition");
        let set = |scenario: &Scenario| -> BTreeSet<String> {
            scenario.shocks.iter().map(|s| s.factor.clone()).collect()
        };
        let joined = set(&composed);
        assert!(
            library.iter().all(|entry| set(entry) != joined),
            "premise: the composed set {joined:?} must differ from every entry"
        );
        assert!(
            library.iter().any(|entry| set(entry).is_subset(&joined)),
            "the join must contain whole library entries, not invented factors"
        );
        let widest = library.iter().map(|e| set(e).len()).max().unwrap_or(0);
        assert!(joined.len() > widest, "no wider than a single entry");
        assert_eq!(composed.name, COMPOSED_SCENARIO);
        assert!(!composed.historical);
        assert!(composed.validate().is_ok());
        // Never milder than a named parent on a factor it shares.
        let parents: Vec<&Scenario> = library
            .iter()
            .filter(|entry| composed.description.contains(&format!("{} ", entry.name)))
            .collect();
        assert_eq!(
            parents.len(),
            2,
            "premise: the description names two parents"
        );
        for shock in &composed.shocks {
            for entry in &parents {
                if let Some(parent) = entry.shocks.iter().find(|s| s.factor == shock.factor) {
                    assert!(
                        shock.magnitude.abs() >= parent.magnitude.abs() - 1e-12,
                        "{} softened against {}",
                        shock.factor,
                        entry.name
                    );
                }
            }
        }
        assert_eq!(composed_scenario(&library), Some(composed), "no replay");
    }

    #[test]
    fn a_library_whose_every_union_already_exists_composes_nothing() {
        let entry = |name: &str, factor: &str| Scenario {
            name: name.to_string(),
            description: "a library entry with stated provenance".to_string(),
            shocks: vec![FactorShock::new(factor, -0.1, 1.0)],
            stressed_correlation: None,
            liquidity_multiplier: 1.0,
            historical: false,
        };
        let mut both = entry("both", "equity");
        both.shocks.push(FactorShock::new("rates", -0.1, 1.0));
        let library = [entry("a", "equity"), entry("b", "rates"), both];
        assert!(composed_scenario(&library).is_none());
        assert!(composed_scenario(&[]).is_none());
    }
}
