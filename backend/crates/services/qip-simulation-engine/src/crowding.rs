//! Does the edge survive company? (ADR 0053, blueprint §15.3)
//!
//! # Why this exists
//!
//! A tape is a recording, and a recording has no counterparties that respond.
//! A strategy evaluated on one is evaluated in a market where it is the only
//! participant taking its side, so a candidate whose apparent edge consists
//! entirely of that scores exactly as well as one whose edge would survive
//! company — and nothing downstream can tell them apart.
//!
//! [`crate::agents`] builds the five counterparty behaviours and
//! [`crate::market::MarketSimulator::with_agents`] attaches them to the books
//! every fill is priced against. Until this module existed, every caller of
//! that was a test.
//!
//! # What is measured
//!
//! The candidate's **own tape**, replayed twice. Same bars, same book shapes,
//! same seed, same cost model, same strategy program; the only difference is
//! whether the panel is attached. That control is the measurement: anything
//! else varying between the two runs would make the difference attributable to
//! something other than the counterparties.
//!
//! A fresh synthetic path would not do. A candidate's edge is specific to the
//! series it was found on, so measuring it against a different random walk
//! reports its disappearance and blames the counterparties for it.
//!
//! # What it may not do
//!
//! **Refuse a candidate.** The panel is not calibrated, and this crate says so
//! in one place — [`crate::agents::FlowCalibration`] has exactly one arm and
//! every run carrying agent flow carries its sentence. Gating a promotion on it
//! would be a decision made on parameters nobody measured.
//!
//! That is the mirror of the defect this repository names by
//! `MaxExpectedShortfall`: a control that could never fire. A control that
//! fires confidently on invented inputs is worse, because the first fails open
//! and visibly and the second fails closed and silently — strategies discarded,
//! a reason printed, and nothing in the output revealing that the reason was
//! arithmetic over numbers somebody chose. ADR 0053 names what would have to
//! change first.

use crate::agents::{CounterpartyAgent, FlowCalibration};
use crate::backtest::BacktestStrategy;
use crate::clock::{ExecutionAssumptions, SimulationClock};
use crate::execution::SimOrder;
use crate::market::{MarketSimulator, MarketView, SimStrategy};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_market::bar::Bar;
use qip_market::book::Side;

/// The panel's sizes, as fractions of the instrument's own daily volume.
///
/// Stated relative to the instrument rather than absolutely, so a panel
/// attached to a thinly traded name is not the same panel as one attached to a
/// liquid one. **These are proportionate, not calibrated** — see the module
/// header and ADR 0053. They are constants rather than parameters because a
/// caller free to choose them could tune the measurement until it said what the
/// caller wanted, and the whole value of the comparison is that both runs faced
/// the same panel.
const PASSIVE_SHARE: f64 = 0.01;
const AGGRESSOR_SHARE: f64 = 0.02;
const MAKER_SHARE: f64 = 0.01;
const MAKER_INVENTORY_SHARE: f64 = 0.10;

/// The maker's quoted half-spread and inventory skew, in basis points.
const MAKER_HALF_SPREAD_BPS: f64 = 5.0;
const MAKER_SKEW_BPS: f64 = 2.0;

/// How far ahead the informed agent reads, and how far back the reactive ones
/// look, in steps.
const INFORMED_HORIZON: usize = 3;
const REACTIVE_LOOKBACK: usize = 5;

/// The return each reactive agent needs to see before it acts.
const REACTIVE_THRESHOLD: f64 = 0.002;

/// How many consecutive steps the competitor will crowd into before backing off.
const CROWD_LIMIT: usize = 3;

/// The volume the panel is sized against, taken from the tape itself.
///
/// **Not the liquidity profile's `average_daily_volume`, and the distinction is
/// the whole point.** `MarketSimulator::replay` fills the book from the bars'
/// own recorded volume — "the whole reason to replay real history is that its
/// liquidity is real" — so a panel sized off a profile's *claim* about volume
/// trades against a book built from the *record* of it. Two independent claims
/// about one fact will disagree, and the panel is the louder one: sized off a
/// figure ten times the tape's, its aggressors take a fifth of every bar and
/// the comparison measures the mis-sizing rather than the crowding.
///
/// Found exactly that way. A fixture whose profile claimed ten times its bars'
/// volume produced a crowded run losing half the capital against a bare run
/// making a fraction of a percent, which reads as a devastating crowding
/// finding and was an arithmetic error about which number to trust.
///
/// A step, not a day: the agents act per step, and the bars are the steps.
fn observed_step_volume(bars: &[Bar]) -> Result<f64> {
    let volumes: Vec<f64> = bars
        .iter()
        .map(|bar| bar.volume.to_f64())
        .filter(|volume| *volume > 0.0)
        .collect();
    if volumes.is_empty() {
        return Err(Error::invalid(
            "no bar on this tape records a positive volume, so there is no observed liquidity to \
             size a counterparty panel against; a panel sized off anything else would be trading \
             in a market this tape did not record",
        ));
    }
    Ok(volumes.iter().sum::<f64>() / volumes.len() as f64)
}

/// The standard counterparty panel, sized to the volume one step actually saw.
///
/// All five behaviours the blueprint names, so a candidate meets a passive
/// flow, someone who knows more than it does, two agents that react to the same
/// moves it reacts to, and a maker whose quotes it has to cross.
///
/// Refuses a non-positive volume rather than substituting one: a panel sized
/// off a volume the tape did not record is a set of numbers with no relation to
/// the market being simulated, and would look exactly like a panel that had.
pub fn standard_panel(step_volume: f64) -> Result<Vec<CounterpartyAgent>> {
    let daily_volume = step_volume;
    if !daily_volume.is_finite() || daily_volume <= 0.0 {
        return Err(Error::invalid(format!(
            "a counterparty panel is sized off the volume the tape recorded, and {daily_volume} \
             is not one; a panel sized off a volume the tape did not record has no relation to \
             the market it would trade in"
        )));
    }
    let share = |fraction: f64| -> Result<Decimal> {
        Decimal::from_f64(daily_volume * fraction)
            .filter(|quantity| quantity.is_positive())
            .ok_or_else(|| {
                Error::numeric(format!(
                    "a daily volume of {daily_volume} gives a panel size that is not a positive \
                     quantity at a {fraction} share"
                ))
            })
    };
    Ok(vec![
        CounterpartyAgent::passive("panel-passive", share(PASSIVE_SHARE)?, 0.5)?,
        CounterpartyAgent::informed(
            "panel-informed",
            share(AGGRESSOR_SHARE)?,
            INFORMED_HORIZON,
            REACTIVE_THRESHOLD,
        )?,
        CounterpartyAgent::momentum(
            "panel-momentum",
            share(AGGRESSOR_SHARE)?,
            REACTIVE_LOOKBACK,
            REACTIVE_THRESHOLD,
        )?,
        CounterpartyAgent::competitor(
            "panel-competitor",
            share(AGGRESSOR_SHARE)?,
            REACTIVE_LOOKBACK,
            REACTIVE_THRESHOLD,
            CROWD_LIMIT,
        )?,
        CounterpartyAgent::maker(
            "panel-maker",
            share(MAKER_SHARE)?,
            MAKER_HALF_SPREAD_BPS,
            MAKER_SKEW_BPS,
            share(MAKER_INVENTORY_SHARE)?,
        )?,
    ])
}

/// Levels published per side in the replayed book.
///
/// The one number here that is neither observed nor derived. Depth beyond the
/// touch is not on a bar and not on a `LiquidityProfile`, so a book shape has
/// to state it, and five is a shape rather than a measurement. Its observable
/// consequence is how far a large order walks before it runs out of book —
/// which matters to the panel's aggressors and to the candidate equally, and
/// is held identical between the two runs, so it cannot bias the comparison.
const REPLAY_LEVELS: usize = 5;

/// A book shape for a replayed instrument, derived from what is observed.
///
/// Everything but [`REPLAY_LEVELS`] comes off the instrument's own liquidity
/// profile and its own bars: the half-spread is half the quoted spread, the
/// levels are a quoted spread apart, the resting size is the recorded
/// top-of-book depth, and the step volatility is the tape's own realised
/// per-bar volatility. `initial_price` and `step_drift` are ignored on replay —
/// the bars supply the path — and are set from the first bar and to zero
/// rather than left to a default that would read as a claim.
///
/// Refuses an empty tape rather than returning a default shape, because a
/// default shape here is a fill price for an instrument nobody measured.
pub fn book_shape(
    object_id: &str,
    profile: &qip_financial::costs::LiquidityProfile,
    bars: &[Bar],
) -> Result<crate::market::InstrumentSpec> {
    let Some(first) = bars.first() else {
        return Err(Error::invalid(format!(
            "a book shape for {object_id} needs at least one bar; a shape guessed for an \
             instrument is a fill price guessed for it"
        )));
    };
    let closes: Vec<f64> = bars.iter().map(|bar| bar.close.to_f64()).collect();
    let returns: Vec<f64> = closes
        .windows(2)
        .filter(|pair| pair[0] > 0.0)
        .map(|pair| (pair[1] - pair[0]) / pair[0])
        .collect();
    let spec = crate::market::InstrumentSpec {
        object_id: object_id.to_string(),
        initial_price: first.close,
        half_spread_bps: profile.typical_spread_bps / 2.0,
        level_spacing_bps: profile.typical_spread_bps,
        level_size: profile.top_of_book_depth,
        levels: REPLAY_LEVELS,
        daily_volume: profile.average_daily_volume.to_f64(),
        // A single-bar tape has no return to take a variance of, and zero is
        // the honest figure for a series that never moved rather than a
        // failure: `validate` refuses a negative one, and the replay reads the
        // path off the bars regardless.
        step_volatility: qip_numerics::stats::variance(&returns).sqrt(),
        step_drift: 0.0,
    };
    spec.validate()?;
    Ok(spec)
}

/// Wears [`SimStrategy`] over a [`BacktestStrategy`].
///
/// The two traits express a strategy differently on purpose — one trades target
/// weights over bars, the other places orders against a book — and this is the
/// adapter that lets a candidate written for the first meet the conditions only
/// the second can express.
///
/// # The one thing it cannot see
///
/// `SimStrategy::on_step` is handed the marks and returns orders; no fill comes
/// back. So the follower tracks the position it **intended**, not the position
/// it holds, and a step whose order fills partially leaves the two disagreeing.
/// That is visible rather than hidden: the residual is exact in
/// `SimulationRun::residual_quantity`, and a run with a large residual is a run
/// whose weights were never reached. It is not papered over with an estimate,
/// because an estimated position would make the next step's delta wrong in a
/// way nothing in the record could show.
#[derive(Debug)]
pub struct WeightFollower<S> {
    inner: S,
    clock: SimulationClock,
    subject: qip_core::ObjectId,
    venue: String,
    /// The notional a weight of one corresponds to.
    capital: Decimal,
    /// The position the follower has asked for, cumulatively.
    intended: Decimal,
}

impl<S: BacktestStrategy> WeightFollower<S> {
    /// Wrap a strategy over the same bars the simulator is replaying.
    ///
    /// The bars are handed here as well as to the simulator, and they must be
    /// the same ones: the follower reads them point-in-time to decide, and the
    /// simulator replays them to fill. Two different tapes would be a strategy
    /// deciding on one market and trading in another.
    pub fn new(
        inner: S,
        bars: Vec<Bar>,
        subject: qip_core::ObjectId,
        venue: impl Into<String>,
        capital: Decimal,
    ) -> Result<Self> {
        if !capital.is_positive() {
            return Err(Error::invalid(
                "a weight follower needs a positive capital to turn a weight into a quantity",
            ));
        }
        Ok(Self {
            inner,
            // Next-bar execution: the strategy decides on bars that have
            // closed and trades after, which is the assumption the backtester
            // makes for the same candidate. Matching it is what makes the
            // crowded run comparable to the evaluation that preceded it.
            clock: SimulationClock::new(bars, ExecutionAssumptions::next_bar())?,
            subject,
            venue: venue.into(),
            capital,
            intended: Decimal::ZERO,
        })
    }

    /// The strategy underneath, once the run is over.
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: BacktestStrategy> SimStrategy for WeightFollower<S> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn on_step(&mut self, view: &MarketView<'_>) -> Vec<SimOrder> {
        // Walk the clock up to the simulator's instant. The two are driven by
        // the same bars, so this converges; it is a loop rather than a seek
        // because `SimulationClock` advances one step at a time and a strategy
        // that skipped a step would see a different history than the
        // backtester gave it.
        while self.clock.now().is_some_and(|now| now < view.at()) && self.clock.advance() {}
        let Some(price) = view
            .mark(self.subject.as_str(), &self.venue)
            .and_then(|mark| mark.current_price())
        else {
            // No current mark is no decision. Trading off a stale price is
            // exactly what `Mark::is_stale` exists to let a caller refuse, and
            // a follower that ignored it would book fills at a price the venue
            // was not showing.
            return Vec::new();
        };
        // The two fields are borrowed apart so the strategy can be asked while
        // the clock's view is alive; they are disjoint, and `self` as a whole
        // is not.
        let Self { inner, clock, .. } = self;
        let Some(pit) = clock.view() else {
            return Vec::new();
        };
        let weights = inner.target_weights(&pit);
        let Some(weight) = weights.get(self.subject.as_str()).copied() else {
            return Vec::new();
        };
        if !weight.is_finite() {
            return Vec::new();
        }
        let Some(target) = Decimal::from_f64(weight * self.capital.to_f64() / price.to_f64())
        else {
            return Vec::new();
        };
        let delta = target - self.intended;
        if delta.is_zero() {
            return Vec::new();
        }
        self.intended = target;
        let side = if delta.is_positive() {
            Side::Buy
        } else {
            Side::Sell
        };
        vec![SimOrder::market(
            self.subject.as_str(),
            self.venue.clone(),
            side,
            delta.abs(),
        )]
    }
}

/// What the same tape did to the same strategy with and without company.
#[derive(Clone, Debug, PartialEq)]
pub struct CrowdingOutcome {
    /// P&L with no counterparties on the book.
    pub bare: Decimal,
    /// P&L with the standard panel attached.
    pub crowded: Decimal,
    /// Orders the panel generated. Zero means the panel did not participate,
    /// and a difference of zero then says nothing about the strategy.
    pub counterparty_orders: usize,
    /// The calibration statement the panel's flow carries, verbatim.
    pub calibration: FlowCalibration,
}

impl CrowdingOutcome {
    /// What company cost, positive when the crowded run did worse.
    pub fn cost(&self) -> Decimal {
        self.bare - self.crowded
    }

    /// Whether the edge changed sign under company.
    ///
    /// Reported, never acted on — ADR 0053. A sign inversion is the most
    /// striking thing this measurement can show and the least entitled to
    /// decide anything, because the panel that produced it is uncalibrated.
    pub fn inverted(&self) -> bool {
        self.bare.is_positive() && !self.crowded.is_positive()
    }

    /// Whether the panel's presence changed the outcome at all.
    ///
    /// A panel can place hundreds of orders and still leave the strategy's
    /// fills at exactly the price they had: the takers sweep depth, and if the
    /// book's displayed size at the touch outlasts them, the strategy arrives
    /// to the same quote. That is a real physical outcome and not a fault —
    /// **and it is indistinguishable, in the cost alone, from an edge that
    /// survives company.** So it is a separate question from
    /// [`Self::panel_participated`], which only asks whether the panel showed
    /// up.
    ///
    /// The temptation this exists to resist is enlarging the panel until the
    /// number moves. That would be tuning the measurement until it says
    /// something, and the panel is uncalibrated, so the something it said
    /// would be whatever was chosen.
    pub fn moved_the_book(&self) -> bool {
        self.bare != self.crowded
    }

    /// Whether the panel actually traded.
    ///
    /// A comparison in which the counterparties placed nothing is not evidence
    /// that the strategy is robust; it is evidence that the panel was absent.
    /// Callers report this rather than reading `cost` alone.
    pub fn panel_participated(&self) -> bool {
        self.counterparty_orders > 0
    }

    pub fn summarise(&self) -> String {
        format!(
            "{}: bare {} against crowded {} over {} counterparty order(s), cost {}{}",
            self.calibration.statement(),
            self.bare,
            self.crowded,
            self.counterparty_orders,
            self.cost(),
            if self.inverted() {
                " (the edge inverts under company)"
            } else if !self.moved_the_book() {
                " (the panel traded and the book did not move: this is not evidence the edge \
                 survives company)"
            } else {
                ""
            }
        )
    }
}

/// Replay one tape twice and compare.
///
/// `build` is called once per run rather than the strategy being reused,
/// because a compiled strategy carries state — its runtime, its trace — and a
/// second run over a warmed-up one would not be the same experiment.
///
/// The `seed` is passed to both simulators unchanged. It is what makes the
/// comparison controlled: the same seed forks the same streams, so the panel is
/// the only thing that differs.
pub fn measure_crowding<S: BacktestStrategy>(
    bars: Vec<Bar>,
    instrument: crate::market::InstrumentSpec,
    venue: &str,
    seed: u64,
    capital: Decimal,
    mut build: impl FnMut() -> Result<S>,
) -> Result<CrowdingOutcome> {
    let subject = qip_core::ObjectId::from_string(instrument.object_id.clone());
    // Sized off the tape, never off the spec's `daily_volume`. See
    // `observed_step_volume`: the book is filled from the bars, so a panel
    // sized off the profile's claim about volume would trade against a book
    // built from the record of it, and the difference between the two runs
    // would be the disagreement rather than the crowding.
    let panel = standard_panel(observed_step_volume(&bars)?)?;

    let bare_market = MarketSimulator::replay(
        bars.clone(),
        vec![instrument.clone()],
        vec![venue.to_string()],
        seed,
    )?;
    let mut bare_strategy =
        WeightFollower::new(build()?, bars.clone(), subject.clone(), venue, capital)?;
    let bare = bare_market.run(&mut bare_strategy)?;

    let crowded_market = MarketSimulator::replay(
        bars.clone(),
        vec![instrument],
        vec![venue.to_string()],
        seed,
    )?
    .with_agents(panel)?;
    let counterparty_orders = crowded_market.counterparty_flow().len();
    let mut crowded_strategy = WeightFollower::new(build()?, bars, subject, venue, capital)?;
    let crowded = crowded_market.run(&mut crowded_strategy)?;

    Ok(CrowdingOutcome {
        bare: bare.profit_and_loss,
        crowded: crowded.profit_and_loss,
        counterparty_orders,
        calibration: FlowCalibration::NotCalibrated,
    })
}
