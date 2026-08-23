//! The market as it stood when the decision was made, and nothing later.
//!
//! Every counterfactual in this crate is evaluated against a [`DecisionView`],
//! and a [`DecisionView`] can only be obtained from [`TwinMarket::view_at`].
//! Two properties of that type are what make the leakage the bitemporal
//! apparatus exists to prevent unrepresentable rather than merely discouraged:
//!
//! * **No accessor takes a time.** The view answers "what was the last close",
//!   "how much traded", "what would this cost" — always as of its own instant,
//!   never as of one the caller chose. There is no method to pass a later
//!   timestamp to, so there is no call that can read a price that had not
//!   printed.
//! * **Only one instant is live at a time.** [`TwinMarket::view_at`] takes
//!   `&mut self` because it repositions the underlying simulation clock, so the
//!   borrow checker refuses a second view while the first is alive. Evaluating
//!   an alternative "as of the decision" while holding a view of the horizon is
//!   not a mistake to catch in review; it does not compile.
//!
//! The reads themselves are the simulator's. [`TwinMarket`] holds a
//! [`SimulationClock`] and hands out its [`PointInTimeView`], which keys bars on
//! close time — a daily bar stamped with today's date does not exist until the
//! session ends. Re-deriving that filter here would have meant maintaining a
//! second definition of what "knowable" means, and the two would diverge.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_market::bar::Bar;
use qip_numerics::stats;
use qip_simulation_engine::clock::{ExecutionAssumptions, PointInTimeView, SimulationClock};
use qip_simulation_engine::costs::{CostModel, TradeCost, Unfillable};
use serde::{Deserialize, Serialize};

/// How much the market could absorb, estimated from what was knowable.
///
/// Both figures are statistics and are `f64` accordingly. They exist to feed
/// [`CostModel::cost_of`], which is where the square-root impact law lives, and
/// they are estimated from the same window the backtester uses so a
/// counterfactual and a backtest of the same trade price it the same way.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Liquidity {
    /// Mean volume over the estimation window.
    pub daily_volume_f64: f64,
    /// Standard deviation of the window's bar returns.
    pub daily_volatility_f64: f64,
    /// How many bars the estimate rests on. A liquidity estimate from two bars
    /// and one from two hundred size an order very differently.
    pub observations: usize,
}

/// The market, and the only door onto it.
#[derive(Debug)]
pub struct TwinMarket {
    clock: SimulationClock,
    costs: CostModel,
    impact_window: usize,
}

impl TwinMarket {
    /// Build from history.
    ///
    /// The execution assumptions are the simulator's next-bar default: a
    /// decision cannot fill on the bar whose close produced it. A twin that
    /// permitted same-bar fills would make every alternative look better than
    /// the action taken for the same reason a backtest does.
    pub fn new(bars: Vec<Bar>, costs: CostModel, impact_window: usize) -> Result<Self> {
        costs.validate()?;
        if impact_window == 0 {
            return Err(Error::invalid(
                "a liquidity estimate needs at least one bar of window",
            ));
        }
        Ok(Self {
            clock: SimulationClock::new(bars, ExecutionAssumptions::next_bar())?,
            costs,
            impact_window,
        })
    }

    /// The cost model every fill in this crate goes through.
    ///
    /// Exposed so a caller can see that the twin prices a counterfactual with
    /// the same parameters the backtester would, and so a test can assert it.
    pub const fn costs(&self) -> CostModel {
        self.costs
    }

    pub const fn impact_window(&self) -> usize {
        self.impact_window
    }

    /// The span the twin can evaluate over.
    pub fn span(&self) -> Option<(Timestamp, Timestamp)> {
        self.clock.span()
    }

    /// The market as it stood at `as_of`, and no later.
    ///
    /// Takes `&mut self` because it repositions the clock. That is what stops a
    /// caller from holding a view of the decision instant and a view of the
    /// horizon at the same time — the second call will not borrow-check.
    pub fn view_at(&mut self, as_of: Timestamp) -> Result<DecisionView<'_>> {
        let steps = self.position_at(as_of);
        if steps == 0 {
            return Err(Error::not_found(format!(
                "no bar had closed by {as_of}, so there is nothing a decision at that instant could have read"
            )));
        }
        let costs = self.costs;
        let window = self.impact_window;
        let inner = self.clock.view().ok_or_else(|| {
            Error::not_found(format!("the simulation clock has no view at {as_of}"))
        })?;
        Ok(DecisionView {
            inner,
            costs,
            window,
        })
    }

    /// Move the clock to the last step at or before `as_of`, returning how many
    /// steps that is.
    ///
    /// Two passes because the clock advances forward only. Walking back would
    /// mean holding the step list here, which is the one piece of the
    /// simulator's leakage guard worth not duplicating.
    fn position_at(&mut self, as_of: Timestamp) -> usize {
        self.clock.reset();
        let mut steps = 0usize;
        while let Some(now) = self.clock.now() {
            if now > as_of {
                break;
            }
            steps += 1;
            if !self.clock.advance() {
                break;
            }
        }
        self.clock.reset();
        for _ in 1..steps {
            self.clock.advance();
        }
        steps
    }

    /// The price a fill at `at` would have happened at.
    ///
    /// Crate-private and deliberately so. It is the simulator's own
    /// [`SimulationClock::fill_price`] — the open of the bar covering the
    /// instant — and it takes a caller-chosen time, which is exactly the shape
    /// of call the leakage guard exists to prevent. It is reachable only from
    /// settlement, where the instants come from a plan that was already fixed
    /// against a [`DecisionView`] and cannot be influenced by what is read here.
    pub(crate) fn price_at(&self, object_id: &ObjectId, at: Timestamp) -> Option<Decimal> {
        self.clock.fill_price(object_id, at)
    }
}

/// What was knowable at one instant.
///
/// Every accessor answers as of [`DecisionView::as_of`]. None of them takes a
/// timestamp, which is the whole design.
#[derive(Debug)]
pub struct DecisionView<'a> {
    inner: PointInTimeView<'a>,
    costs: CostModel,
    window: usize,
}

impl DecisionView<'_> {
    /// The instant this view describes.
    pub fn as_of(&self) -> Timestamp {
        self.inner.as_of()
    }

    /// The most recent close that had printed.
    pub fn last_close(&self, object_id: &ObjectId) -> Option<Decimal> {
        self.inner.last_close(object_id)
    }

    /// Instruments with at least one closed bar by this instant.
    pub fn available(&self) -> Vec<ObjectId> {
        self.inner.available()
    }

    /// How much the market was absorbing, estimated from the trailing window.
    ///
    /// `None` when nothing had printed: a counterfactual on an instrument with
    /// no history is not a conservative estimate, it is a guess, and the caller
    /// should be told rather than handed a default.
    pub fn liquidity(&self, object_id: &ObjectId) -> Option<Liquidity> {
        let bars = self.inner.bars(object_id);
        if bars.is_empty() {
            return None;
        }
        let window = &bars[bars.len().saturating_sub(self.window)..];
        let volumes: Vec<f64> = window.iter().map(|bar| bar.volume.to_f64()).collect();
        let returns: Vec<f64> = window.iter().map(Bar::return_pct).collect();
        Some(Liquidity {
            daily_volume_f64: stats::mean(&volumes),
            daily_volatility_f64: stats::stddev(&returns),
            observations: window.len(),
        })
    }

    /// What trading `quantity` at `price` would cost, through the simulator's
    /// model.
    ///
    /// Returns [`Unfillable`] rather than a large number when the order is more
    /// of the day's volume than the impact law was calibrated for. That refusal
    /// is the single most important thing this view does for a counterfactual:
    /// without it, "we should have traded ten times the size" always wins.
    pub fn quote_cost(
        &self,
        object_id: &ObjectId,
        quantity: Decimal,
        price: Decimal,
    ) -> std::result::Result<TradeCost, Unfillable> {
        let liquidity = self.liquidity(object_id).ok_or(Unfillable::NoVolume)?;
        self.costs.cost_of(
            quantity,
            price,
            liquidity.daily_volume_f64,
            liquidity.daily_volatility_f64,
        )
    }

    /// The cost model behind [`DecisionView::quote_cost`].
    pub const fn costs(&self) -> CostModel {
        self.costs
    }

    /// Reads served, evidence that a caller went through the guard rather than
    /// around it.
    pub fn reads(&self) -> usize {
        self.inner.read_count()
    }
}
