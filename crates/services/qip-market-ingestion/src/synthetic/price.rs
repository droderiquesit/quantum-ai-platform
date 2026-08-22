//! The synthetic price process.
//!
//! Realistic in the ways that matter for testing an investment platform:
//!
//! * **Stochastic volatility** (Heston-style mean-reverting variance), so
//!   volatility clusters instead of being constant.
//! * **Jumps** (Merton), so tail events actually occur and a risk model built
//!   on a Gaussian assumption is visibly wrong.
//! * **Regime switching**, so correlation, spread and jump intensity all shift
//!   together the way they do in a real stress episode.
//! * **A factor structure**, so instruments co-move for a reason and a
//!   correlation breakdown can be injected as a deliberate scenario.
//!
//! Everything is driven by an injected RNG, so a run is reproducible from its
//! seed.

use qip_core::rng::Rng;
use serde::{Deserialize, Serialize};

/// Market regime. Drives volatility, correlation, spread and jump intensity
/// together, because in practice they move together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Regime {
    #[default]
    Calm,
    Normal,
    Stressed,
    Crisis,
}

impl Regime {
    /// Multiplier applied to instantaneous volatility.
    pub fn volatility_multiplier(&self) -> f64 {
        match self {
            Self::Calm => 0.7,
            Self::Normal => 1.0,
            Self::Stressed => 2.2,
            Self::Crisis => 4.0,
        }
    }

    /// Multiplier on factor loadings. Correlations rise toward one in a crisis,
    /// which is precisely when diversification is being relied on.
    pub fn correlation_multiplier(&self) -> f64 {
        match self {
            Self::Calm => 0.8,
            Self::Normal => 1.0,
            Self::Stressed => 1.5,
            Self::Crisis => 2.0,
        }
    }

    /// Multiplier on quoted spread and on the rate at which depth thins.
    pub fn liquidity_penalty(&self) -> f64 {
        match self {
            Self::Calm => 0.8,
            Self::Normal => 1.0,
            Self::Stressed => 2.5,
            Self::Crisis => 6.0,
        }
    }

    /// Multiplier on jump intensity.
    pub fn jump_multiplier(&self) -> f64 {
        match self {
            Self::Calm => 0.3,
            Self::Normal => 1.0,
            Self::Stressed => 3.0,
            Self::Crisis => 8.0,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Calm => "calm",
            Self::Normal => "normal",
            Self::Stressed => "stressed",
            Self::Crisis => "crisis",
        }
    }

    /// Daily transition probabilities to each regime.
    ///
    /// Calibrated so calm and normal persist for weeks while crisis decays in
    /// days — the empirical asymmetry that makes stress look sudden and
    /// recovery look gradual.
    pub fn transition_probabilities(&self) -> [(Regime, f64); 4] {
        match self {
            Self::Calm => [
                (Self::Calm, 0.94),
                (Self::Normal, 0.055),
                (Self::Stressed, 0.005),
                (Self::Crisis, 0.0),
            ],
            Self::Normal => [
                (Self::Calm, 0.04),
                (Self::Normal, 0.93),
                (Self::Stressed, 0.028),
                (Self::Crisis, 0.002),
            ],
            Self::Stressed => [
                (Self::Calm, 0.005),
                (Self::Normal, 0.20),
                (Self::Stressed, 0.755),
                (Self::Crisis, 0.04),
            ],
            Self::Crisis => [
                (Self::Calm, 0.0),
                (Self::Normal, 0.05),
                (Self::Stressed, 0.35),
                (Self::Crisis, 0.60),
            ],
        }
    }

    /// Draw the next regime.
    pub fn step<R: Rng>(&self, rng: &mut R) -> Regime {
        let transitions = self.transition_probabilities();
        let weights: Vec<f64> = transitions.iter().map(|(_, p)| *p).collect();
        match rng.weighted_index(&weights) {
            Some(index) => transitions[index].0,
            None => *self,
        }
    }
}

/// Speed at which a factor's variance multiplier reverts to one.
const FACTOR_VARIANCE_REVERSION: f64 = 2.0;
/// Volatility of a factor's variance multiplier.
const FACTOR_VOL_OF_VOL: f64 = 3.0;

/// Persistence of the order-flow momentum term between steps.
const MOMENTUM_DECAY: f64 = 0.85;

/// Share of an instrument's volatility that comes from the constant floor
/// rather than the stochastic variance. Kept low so clustering survives.
const IDIOSYNCRATIC_FLOOR_WEIGHT: f64 = 0.25;

/// Parameters of one instrument's price process.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessParameters {
    /// Annualised drift.
    pub drift: f64,
    /// Long-run variance the process reverts to.
    pub long_run_variance: f64,
    /// Speed of variance mean reversion.
    pub variance_reversion: f64,
    /// Volatility of variance.
    pub variance_of_variance: f64,
    /// Correlation between the price and variance shocks. Negative for
    /// equities: the leverage effect, where price falls raise volatility.
    pub leverage_correlation: f64,
    /// Loadings on the common factors.
    pub factor_loadings: Vec<f64>,
    /// Annualised idiosyncratic volatility.
    pub idiosyncratic_volatility: f64,
    /// Expected jumps per year.
    pub jump_intensity: f64,
    /// Mean log jump size.
    pub jump_mean: f64,
    /// Standard deviation of log jump size.
    pub jump_volatility: f64,
    /// Speed of reversion to `anchor_price`; zero for a pure random walk.
    ///
    /// Non-zero for instruments with an economic anchor — an FX pair around
    /// purchasing-power parity, a bond price pulling to par.
    pub price_reversion: f64,
    /// Level the price reverts toward, when `price_reversion` is non-zero.
    pub anchor_price: f64,
}

impl Default for ProcessParameters {
    fn default() -> Self {
        Self {
            drift: 0.06,
            long_run_variance: 0.04,
            variance_reversion: 3.0,
            variance_of_variance: 0.4,
            leverage_correlation: -0.6,
            factor_loadings: vec![1.0],
            idiosyncratic_volatility: 0.18,
            // Roughly one jump per trading day. Equity returns are visibly
            // non-Gaussian at every horizon, and a simulator that only jumps
            // twice a year would let a Gaussian risk model look correct.
            jump_intensity: 260.0,
            jump_mean: -0.0004,
            jump_volatility: 0.006,
            price_reversion: 0.0,
            anchor_price: 0.0,
        }
    }
}

impl ProcessParameters {
    /// A liquid large-cap equity.
    pub fn equity(beta: f64) -> Self {
        Self {
            factor_loadings: vec![beta, 0.0, 0.0],
            ..Self::default()
        }
    }

    /// A government bond price: low volatility, pulls to par, rate-sensitive.
    pub fn government_bond(duration: f64) -> Self {
        Self {
            drift: 0.01,
            long_run_variance: 0.0009,
            variance_reversion: 5.0,
            variance_of_variance: 0.1,
            leverage_correlation: 0.0,
            // Loads on the rates factor, scaled by duration and signed
            // negative: prices fall when rates rise.
            factor_loadings: vec![0.0, -duration / 10.0, 0.0],
            idiosyncratic_volatility: 0.015,
            jump_intensity: 40.0,
            jump_mean: 0.0,
            jump_volatility: 0.0015,
            price_reversion: 0.3,
            anchor_price: 100.0,
        }
    }

    /// An FX pair: no drift to speak of, mean-reverting, occasional jumps on
    /// policy surprises.
    pub fn fx_pair() -> Self {
        Self {
            drift: 0.0,
            long_run_variance: 0.0081,
            variance_reversion: 4.0,
            variance_of_variance: 0.25,
            leverage_correlation: 0.0,
            factor_loadings: vec![0.0, 0.3, 0.5],
            idiosyncratic_volatility: 0.05,
            jump_intensity: 90.0,
            jump_mean: 0.0,
            jump_volatility: 0.003,
            price_reversion: 0.15,
            anchor_price: 1.10,
        }
    }

    /// A commodity: high volatility, fat tails, weather- and supply-driven.
    pub fn commodity() -> Self {
        Self {
            drift: 0.02,
            long_run_variance: 0.09,
            variance_reversion: 2.0,
            variance_of_variance: 0.6,
            leverage_correlation: 0.2,
            factor_loadings: vec![0.3, 0.0, 0.9],
            idiosyncratic_volatility: 0.30,
            jump_intensity: 400.0,
            jump_mean: 0.0,
            jump_volatility: 0.010,
            price_reversion: 0.1,
            anchor_price: 75.0,
        }
    }

    /// A digital asset: very high volatility, heavy jumps, no anchor.
    pub fn digital_asset() -> Self {
        Self {
            drift: 0.15,
            long_run_variance: 0.36,
            variance_reversion: 1.5,
            variance_of_variance: 1.0,
            leverage_correlation: -0.3,
            factor_loadings: vec![0.6, 0.0, 0.2],
            idiosyncratic_volatility: 0.55,
            jump_intensity: 900.0,
            jump_mean: -0.0005,
            jump_volatility: 0.014,
            price_reversion: 0.0,
            anchor_price: 0.0,
        }
    }
}

/// Evolving state of one price process.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriceState {
    pub price: f64,
    /// Instantaneous variance.
    pub variance: f64,
    /// Cumulative jump count, for diagnostics.
    pub jumps: u64,
    /// Realised return over the most recent step.
    pub last_return: f64,
    /// Exponentially-weighted recent return. Order flow follows price action
    /// with persistence, so a single step's return is too short-lived a signal
    /// — and a scripted shock would be overwritten before any trade saw it.
    pub momentum: f64,
    /// Temporary volatility multiplier from a scripted shock, decaying to one.
    pub volatility_shock: f64,
    /// Temporary liquidity multiplier from a scripted shock, decaying to one.
    pub liquidity_shock: f64,
}

impl PriceState {
    pub fn new(price: f64, variance: f64) -> Self {
        Self {
            price,
            variance,
            jumps: 0,
            last_return: 0.0,
            momentum: 0.0,
            volatility_shock: 1.0,
            liquidity_shock: 1.0,
        }
    }

    /// Instantaneous annualised volatility, including any active shock.
    pub fn volatility(&self) -> f64 {
        self.variance.max(0.0).sqrt() * self.volatility_shock
    }
}

/// Common factors driving co-movement.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactorModel {
    pub names: Vec<String>,
    /// Annualised volatility of each factor.
    pub volatilities: Vec<f64>,
    /// Latest realised shock per factor, so instruments in the same step see
    /// the same draw — which is what creates correlation.
    pub shocks: Vec<f64>,
    /// Stochastic variance multiplier per factor.
    ///
    /// Market-wide volatility clusters just as an individual name's does. With
    /// a constant factor volatility, the systematic half of every instrument's
    /// return would be homoscedastic and would wash the clustering out of the
    /// total — which is exactly what a risk model must not be tested against.
    pub variance_state: Vec<f64>,
    /// Scales every loading. A scripted correlation breakdown drops this.
    pub coupling: f64,
}

impl FactorModel {
    /// The three factors the synthetic environment ships with: a broad market
    /// factor, a rates factor and a commodity factor.
    pub fn standard() -> Self {
        Self {
            names: vec!["market".into(), "rates".into(), "commodity".into()],
            volatilities: vec![0.16, 0.06, 0.28],
            shocks: vec![0.0; 3],
            variance_state: vec![1.0; 3],
            coupling: 1.0,
        }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Draw this step's factor shocks, advancing each factor's own variance.
    /// `volatility_scale` carries the time-of-day seasonality, which applies to
    /// systematic risk as much as to idiosyncratic risk.
    pub fn step<R: Rng>(
        &mut self,
        dt_years: f64,
        regime: Regime,
        volatility_scale: f64,
        rng: &mut R,
    ) {
        let scale = regime.volatility_multiplier() * volatility_scale;
        let sqrt_dt = dt_years.max(0.0).sqrt();
        for index in 0..self.volatilities.len() {
            // Mean-reverting log-variance, so the multiplier stays positive and
            // wanders on a horizon of weeks rather than minutes.
            let state = self.variance_state[index].max(1e-6);
            let drift = FACTOR_VARIANCE_REVERSION * (1.0 - state) * dt_years;
            let shock = FACTOR_VOL_OF_VOL * state * sqrt_dt * rng.normal();
            self.variance_state[index] = (state + drift + shock).clamp(0.15, 6.0);

            self.shocks[index] = rng.normal()
                * self.volatilities[index]
                * self.variance_state[index].sqrt()
                * scale
                * sqrt_dt;
        }
    }

    /// Combined factor contribution for a set of loadings.
    pub fn contribution(&self, loadings: &[f64], regime: Regime) -> f64 {
        let scale = self.coupling * regime.correlation_multiplier();
        loadings
            .iter()
            .zip(&self.shocks)
            .map(|(loading, shock)| loading * shock * scale)
            .sum()
    }
}

/// Advance a price process by `dt_years`.
///
/// Returns the realised log return over the step.
pub fn step_process<R: Rng>(
    state: &mut PriceState,
    parameters: &ProcessParameters,
    factors: &FactorModel,
    regime: Regime,
    dt_years: f64,
    volatility_scale: f64,
    rng: &mut R,
) -> f64 {
    if dt_years <= 0.0 || !state.price.is_finite() || state.price <= 0.0 {
        return 0.0;
    }
    let sqrt_dt = dt_years.sqrt();

    // Variance follows a mean-reverting square-root process, with a full
    // truncation scheme so a negative Euler step cannot produce a NaN.
    let variance_shock = rng.normal();
    let current_variance = state.variance.max(0.0);
    let next_variance = current_variance
        + parameters.variance_reversion
            * (parameters.long_run_variance - current_variance)
            * dt_years
        + parameters.variance_of_variance * current_variance.sqrt() * sqrt_dt * variance_shock;
    state.variance = next_variance.max(1e-8);

    // The price shock is correlated with the variance shock (leverage effect).
    let independent = rng.normal();
    let rho = parameters.leverage_correlation.clamp(-0.999, 0.999);
    let correlated = rho * variance_shock + (1.0 - rho * rho).sqrt() * independent;

    // The instrument's own volatility is the stochastic variance, so periods of
    // high variance produce large moves and periods of low variance small ones.
    // That persistence is what makes volatility cluster; a constant term added
    // alongside it would dilute the effect until it disappeared.
    let instantaneous_volatility =
        current_variance.sqrt() * regime.volatility_multiplier() * state.volatility_shock;
    let stochastic = instantaneous_volatility * sqrt_dt * correlated;

    // A small constant floor keeps the process from going completely quiet when
    // variance approaches zero. Deliberately a minority of total variance.
    let floor_noise = parameters.idiosyncratic_volatility
        * IDIOSYNCRATIC_FLOOR_WEIGHT
        * regime.volatility_multiplier()
        * state.volatility_shock
        * volatility_scale
        * sqrt_dt
        * rng.normal();

    let factor_component = factors.contribution(&parameters.factor_loadings, regime);

    // Mean reversion toward an economic anchor, where the instrument has one.
    let reversion = if parameters.price_reversion > 0.0 && parameters.anchor_price > 0.0 {
        parameters.price_reversion * (parameters.anchor_price / state.price).ln() * dt_years
    } else {
        0.0
    };

    // Jumps: Poisson arrivals with lognormal sizes.
    let intensity =
        parameters.jump_intensity * regime.jump_multiplier() * volatility_scale * dt_years;
    let jump_count = rng.poisson(intensity.max(0.0));
    let mut jump_component = 0.0;
    for _ in 0..jump_count {
        jump_component += parameters.jump_mean + parameters.jump_volatility * rng.normal();
    }
    state.jumps += jump_count;

    let log_return = (parameters.drift - 0.5 * current_variance) * dt_years
        + factor_component
        + stochastic
        + floor_noise
        + reversion
        + jump_component;

    // Bound a single step so a pathological draw cannot produce a price of
    // zero or infinity and silently corrupt everything downstream.
    let bounded = log_return.clamp(-0.5, 0.5);
    state.price = (state.price * bounded.exp()).max(1e-6);
    state.last_return = bounded;
    state.momentum = state.momentum * MOMENTUM_DECAY + bounded;

    // Scripted shocks decay back toward normal.
    state.volatility_shock = 1.0 + (state.volatility_shock - 1.0) * 0.97;
    state.liquidity_shock = 1.0 + (state.liquidity_shock - 1.0) * 0.95;

    bounded
}

/// Position within the regular session, or `None` when the venue is closed.
///
/// Sessions are hard-coded to 14:30-21:00 UTC, the US equity session the demo
/// world trades in. A venue-aware version would consult the trading calendar.
pub fn session_position(minutes_utc: f64) -> Option<f64> {
    const OPEN: f64 = 14.0 * 60.0 + 30.0;
    const CLOSE: f64 = 21.0 * 60.0;
    if !(OPEN..CLOSE).contains(&minutes_utc) {
        return None;
    }
    Some((minutes_utc - OPEN) / (CLOSE - OPEN))
}

/// Relative trading intensity, as a fraction of the day's average.
///
/// The familiar U shape: heavy at the open and the close, quiet at midday, thin
/// outside the session. Execution algorithms that schedule against a flat
/// profile trade too much at the wrong time, so the simulator has to reproduce
/// it.
pub fn intraday_intensity(position: Option<f64>) -> f64 {
    let Some(t) = position else {
        // After hours: a trickle, not nothing.
        return 0.05;
    };
    let curve = 3.2 * (t - 0.5).powi(2);
    let open_burst = 0.30 * (-(t / 0.10).powi(2)).exp();
    let close_burst = 0.35 * (-((t - 1.0) / 0.12).powi(2)).exp();
    0.45 + curve + open_burst + close_burst
}

/// Multiplier applied to instantaneous volatility by time of day.
///
/// Volatility is seasonal within the session — high at the open while overnight
/// information is absorbed, low through midday, rising into the close. This
/// deterministic cycle is a large part of why short-horizon absolute returns are
/// autocorrelated in real data, and a simulator without it produces returns that
/// look far more independent than anything a risk model will actually meet.
pub fn intraday_volatility_scale(position: Option<f64>) -> f64 {
    let Some(t) = position else {
        // Overnight: thin and quiet, but news still arrives.
        return 0.30;
    };
    let open_effect = 1.10 * (-(t / 0.14).powi(2)).exp();
    let close_effect = 0.60 * (-((t - 1.0) / 0.16).powi(2)).exp();
    0.70 + open_effect + close_effect
}
