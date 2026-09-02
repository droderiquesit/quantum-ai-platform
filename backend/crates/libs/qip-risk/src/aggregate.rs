//! Hierarchical risk aggregation — the fixed set of counters a risk check
//! reads.
//!
//! A risk check must be O(1) in strategy count. The platform is built to run
//! tens of thousands of strategies at once, and a check that sums their
//! positions to learn the book's gross exposure costs more than everything
//! else in the hot path combined — and gets slower exactly when the book is
//! busiest, which is when the check matters most. So the aggregate is kept
//! *incrementally*: every fill updates a fixed handful of running counters,
//! and the aggregate checks read those counters and nothing else.
//!
//! The split is the point, and it is structural rather than a convention:
//!
//! * [`RiskAggregates::apply_fill`] performs a constant number of counter
//!   updates per fill — the instrument's position, the book's gross and net,
//!   cash, one bucket per exposure axis, and the contributing strategy's own
//!   gross. No update walks the strategy set.
//! * [`LimitSet::check_aggregates`] reads the book-level figures through
//!   [`AggregateFigures`] and never calls [`AggregateFigures::strategies`] or
//!   [`AggregateFigures::strategy_gross`]. Strategy-level budgets are checked
//!   *before* netting, one strategy at a time, by
//!   [`RiskAggregates::admit_contribution`]; every other level is checked on
//!   the net, once.
//!
//! [`AggregateFigures`] is a trait rather than a struct so a test can wrap the
//! aggregate in a probe that counts every figure the check consults. That test
//! runs at two strategy counts and fails the moment a check is rewritten to
//! iterate — which is the only way the property "reads aggregates, never
//! strategy lists" can be held by something stronger than a comment.

use crate::limits::{LimitCheck, LimitSet, RiskState};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The book-level figures an aggregate risk check may consult.
///
/// Everything a check reads comes through here, so a wrapper that counts calls
/// sees every read. The two strategy-level accessors are on the trait too, on
/// purpose: a check *could* call them, and the test proves it does not.
pub trait AggregateFigures {
    fn equity(&self) -> Decimal;
    fn cash(&self) -> Decimal;
    fn gross_exposure(&self) -> Decimal;
    fn net_exposure(&self) -> Decimal;
    /// Current drawdown from the running peak, as a fraction.
    fn drawdown(&self) -> f64;
    /// Signed notional per instrument.
    fn position_notionals(&self) -> &BTreeMap<String, Decimal>;
    /// Gross exposure per bucket, keyed by axis then bucket.
    fn axis_exposures(&self) -> &BTreeMap<String, BTreeMap<String, Decimal>>;
    /// Every strategy that has contributed a fill.
    fn strategies(&self) -> Vec<&str>;
    /// Gross notional one strategy has contributed.
    fn strategy_gross(&self, strategy: &str) -> Decimal;
}

/// The running counters, updated per fill.
///
/// Serialisable so a snapshot can be journalled; iteration is over
/// [`BTreeMap`]s so anything derived from it comes out in the same order on
/// every replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiskAggregates {
    equity: Decimal,
    cash: Decimal,
    gross: Decimal,
    net: Decimal,
    drawdown: f64,
    /// Signed notional per instrument.
    positions: BTreeMap<String, Decimal>,
    /// Gross per bucket, keyed by axis then bucket.
    axis_exposures: BTreeMap<String, BTreeMap<String, Decimal>>,
    /// Signed notional per strategy per instrument, so a strategy's gross can
    /// move by the change in one entry rather than by a recount.
    strategy_positions: BTreeMap<String, BTreeMap<String, Decimal>>,
    /// Gross per strategy.
    strategy_gross: BTreeMap<String, Decimal>,
    /// Fills applied, so a snapshot says how much history it summarises.
    fills: u64,
}

impl RiskAggregates {
    /// Open an aggregate over a book's equity and cash.
    ///
    /// Negative equity is refused rather than floored: it means the caller's
    /// accounting has already gone wrong, and a floor would bury that under
    /// a run of ratio limits reporting infinity.
    pub fn new(equity: Decimal, cash: Decimal) -> Result<Self> {
        if equity.is_negative() {
            return Err(Error::invalid(format!(
                "a risk aggregate cannot open over negative equity ({equity}); reconcile the \
                 book before checking limits against it"
            )));
        }
        Ok(Self {
            equity,
            cash,
            gross: Decimal::ZERO,
            net: Decimal::ZERO,
            drawdown: 0.0,
            positions: BTreeMap::new(),
            axis_exposures: BTreeMap::new(),
            strategy_positions: BTreeMap::new(),
            strategy_gross: BTreeMap::new(),
            fills: 0,
        })
    }

    /// Record a mark: the book's equity and drawdown as of now.
    ///
    /// Marks are the caller's, because equity moves with prices the aggregate
    /// never sees. A drawdown outside `[0, 1]` or not a number is refused —
    /// it is the figure the hard halt reads, and a `NaN` there compares false
    /// against every threshold, which would disarm the halt.
    pub fn mark(&mut self, equity: Decimal, drawdown: f64) -> Result<()> {
        if equity.is_negative() {
            return Err(Error::invalid(format!(
                "a mark cannot set negative equity ({equity})"
            )));
        }
        if !drawdown.is_finite() || !(0.0..=1.0).contains(&drawdown) {
            return Err(Error::invalid(format!(
                "a drawdown must be a fraction in [0, 1], not {drawdown}; a figure the halt \
                 cannot compare is a halt that cannot fire"
            )));
        }
        self.equity = equity;
        self.drawdown = drawdown;
        Ok(())
    }

    /// Apply one fill: a constant number of counter updates.
    ///
    /// `signed_notional` is positive for a buy and negative for a sell. Gross
    /// moves by the change in the instrument's *absolute* position, which is
    /// not the fill's notional when the fill reduces an existing position —
    /// adding the notional regardless would refuse the very trades that fix a
    /// breach. The same arithmetic keeps each axis bucket and the strategy's
    /// own gross exact without a recount.
    pub fn apply_fill(
        &mut self,
        strategy: &str,
        instrument: &str,
        axes: &BTreeMap<String, String>,
        signed_notional: Decimal,
    ) -> Result<()> {
        if strategy.trim().is_empty() {
            return Err(Error::invalid(
                "a fill must name the strategy it belongs to, or its budget cannot be charged",
            ));
        }
        if instrument.trim().is_empty() {
            return Err(Error::invalid("a fill must name an instrument"));
        }
        if signed_notional.is_zero() {
            return Err(Error::invalid(format!(
                "a fill of zero notional in {instrument} is not a fill"
            )));
        }

        let position = self
            .positions
            .entry(instrument.to_string())
            .or_insert(Decimal::ZERO);
        let before = position.abs();
        *position += signed_notional;
        let delta_gross = position.abs() - before;

        self.gross += delta_gross;
        self.net += signed_notional;
        self.cash -= signed_notional;

        for (axis, bucket) in axes {
            *self
                .axis_exposures
                .entry(axis.clone())
                .or_default()
                .entry(bucket.clone())
                .or_insert(Decimal::ZERO) += delta_gross;
        }

        let contribution = self
            .strategy_positions
            .entry(strategy.to_string())
            .or_default()
            .entry(instrument.to_string())
            .or_insert(Decimal::ZERO);
        let before = contribution.abs();
        *contribution += signed_notional;
        let delta_strategy = contribution.abs() - before;
        *self
            .strategy_gross
            .entry(strategy.to_string())
            .or_insert(Decimal::ZERO) += delta_strategy;

        self.fills += 1;
        Ok(())
    }

    /// Fills applied so far.
    pub fn fills(&self) -> u64 {
        self.fills
    }

    /// The strategy-level gate, checked before netting.
    ///
    /// A strategy that has exhausted its budget must not contribute to a net
    /// intent at all, so this is asked per contributor and refuses the whole
    /// contribution rather than trimming it: a trimmed contribution is a
    /// trade nobody reviewed. It reads one strategy's counter, never the set.
    ///
    /// A contribution of nothing is refused as invalid rather than admitted:
    /// admitting it would let a caller "pass" the gate with a zero and then
    /// send something else. There is deliberately no separate rule for a
    /// non-positive budget — the ceiling refuses every non-zero contribution
    /// against one, and a second rule that could only fire on a zero
    /// contribution would be a control that cannot fire.
    pub fn admit_contribution(
        &self,
        strategy: &str,
        additional_notional: Decimal,
        budget: Decimal,
    ) -> Result<()> {
        if additional_notional.is_zero() {
            return Err(Error::invalid(format!(
                "{strategy} offered a contribution of nothing; there is nothing to admit"
            )));
        }
        let held = self.strategy_gross(strategy);
        let after = held + additional_notional.abs();
        if after > budget {
            return Err(Error::denied(format!(
                "{strategy} holds {held} gross and a further {} would take it to {after} \
                 against a budget of {budget}; the contribution is dropped from the net intent",
                additional_notional.abs()
            )));
        }
        Ok(())
    }
}

impl AggregateFigures for RiskAggregates {
    fn equity(&self) -> Decimal {
        self.equity
    }

    fn cash(&self) -> Decimal {
        self.cash
    }

    fn gross_exposure(&self) -> Decimal {
        self.gross
    }

    fn net_exposure(&self) -> Decimal {
        self.net
    }

    fn drawdown(&self) -> f64 {
        self.drawdown
    }

    fn position_notionals(&self) -> &BTreeMap<String, Decimal> {
        &self.positions
    }

    fn axis_exposures(&self) -> &BTreeMap<String, BTreeMap<String, Decimal>> {
        &self.axis_exposures
    }

    fn strategies(&self) -> Vec<&str> {
        self.strategy_gross.keys().map(String::as_str).collect()
    }

    fn strategy_gross(&self, strategy: &str) -> Decimal {
        self.strategy_gross
            .get(strategy)
            .copied()
            .unwrap_or(Decimal::ZERO)
    }
}

impl RiskState {
    /// The state the aggregate checks evaluate, from the book-level figures.
    ///
    /// Reads exactly seven figures and neither strategy-level accessor. The
    /// tail maps are left for [`RiskState::with_tail_risk`], which needs the
    /// return series the aggregate does not hold.
    pub fn from_figures(figures: &impl AggregateFigures) -> Self {
        Self {
            equity: figures.equity(),
            cash: figures.cash(),
            gross_exposure: figures.gross_exposure(),
            net_exposure: figures.net_exposure(),
            position_notionals: figures
                .position_notionals()
                .iter()
                .map(|(instrument, notional)| (instrument.clone(), notional.abs()))
                .collect(),
            axis_exposures: figures.axis_exposures().clone(),
            drawdown: figures.drawdown(),
            ..Self::default()
        }
    }
}

impl LimitSet {
    /// Evaluate every limit against the aggregate — on the net, once.
    ///
    /// `returns` is the book's return series, so the tail limits are filled
    /// in the same call and cannot be forgotten by a caller that builds the
    /// state by hand. Consults a fixed set of book-level figures regardless
    /// of how many strategies contributed; `tests/aggregate.rs` counts them.
    pub fn check_aggregates(&self, figures: &impl AggregateFigures, returns: &[f64]) -> LimitCheck {
        self.check(&RiskState::from_figures(figures).with_tail_risk(self, returns))
    }
}
