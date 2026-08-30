//! Point-in-time backtesting.
//!
//! The engine walks the [`crate::clock::SimulationClock`] forward, asks the
//! strategy for target weights at each step through a
//! [`crate::clock::PointInTimeView`], and executes the resulting trades at the
//! next executable instant with costs applied.
//!
//! What the result reports is as important as what it computes. A backtest that
//! returns a Sharpe ratio and nothing else invites the reader to believe it.
//! [`BacktestResult`] carries the assumptions used, the orders that could not
//! be filled, the periods where data was missing, and the multiple-testing
//! penalty if the run was one of many — because the number that matters is not
//! the Sharpe ratio, it is the Sharpe ratio after everything that would have
//! reduced it.

use crate::clock::{ExecutionAssumptions, PointInTimeView, SimulationClock};
use crate::costs::{CostModel, TradeCost};
use qip_core::error::{Error, Result};
use qip_core::ids::{ObjectId, PortfolioId};
use qip_core::time::Timestamp;
use qip_core::{Currency, Decimal};
use qip_financial::universe::Universe;
use qip_numerics::stats;
use qip_portfolio::portfolio::Portfolio;
use qip_risk::metrics::RiskMetrics;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a strategy must provide to be backtested.
///
/// The view is the only way in to market data, so a strategy physically cannot
/// read a price that had not printed. Implementations are free to be as
/// careless as they like; the type system is not.
pub trait BacktestStrategy {
    fn name(&self) -> &str;

    /// Target weights as fractions of equity, given what was knowable.
    ///
    /// Returning an empty map means "hold what you have", which is different
    /// from returning zero weights — that means "go to cash".
    fn target_weights(&mut self, view: &PointInTimeView<'_>) -> BTreeMap<String, f64>;

    /// Whether the strategy wants to act at this instant.
    ///
    /// A daily strategy asked at every bar can decline most of them, which
    /// keeps turnover honest.
    fn should_rebalance(&self, view: &PointInTimeView<'_>) -> bool {
        let _ = view;
        true
    }
}

/// One order the backtest could not fill.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RejectedOrder {
    pub at: Timestamp,
    pub object_id: String,
    pub quantity: Decimal,
    pub reason: String,
}

/// One executed trade, with its costs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimulatedFill {
    pub at: Timestamp,
    pub object_id: String,
    pub quantity: Decimal,
    pub price: Decimal,
    pub cost: TradeCost,
}

/// How a backtest was configured.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BacktestConfig {
    pub initial_capital: Decimal,
    pub currency: Currency,
    pub costs: CostModel,
    pub assumptions: ExecutionAssumptions,
    /// Bars used to estimate volume and volatility for the impact model.
    pub impact_window: usize,
    /// Periods per year, for annualising. Inferred from the data when absent.
    pub periods_per_year: Option<f64>,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            initial_capital: Decimal::from_int(10_000_000),
            currency: Currency::USD,
            costs: CostModel::default(),
            assumptions: ExecutionAssumptions::next_bar(),
            impact_window: 20,
            periods_per_year: None,
        }
    }
}

/// What a backtest produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BacktestResult {
    pub strategy: String,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub config: BacktestConfig,
    /// Equity at each step.
    pub equity_curve: Vec<(Timestamp, Decimal)>,
    /// Period returns of the equity curve.
    pub returns: Vec<f64>,
    pub fills: Vec<SimulatedFill>,
    pub rejected: Vec<RejectedOrder>,
    /// Instruments the strategy asked for that had no price yet.
    pub missing_prices: Vec<String>,
    /// Total costs paid, decomposed.
    pub total_commission: f64,
    pub total_spread: f64,
    pub total_impact: f64,
    /// Steps at which the strategy chose to rebalance.
    pub rebalance_count: usize,
    /// Reads served through the point-in-time view, evidence that the strategy
    /// went through the guard rather than around it.
    pub guarded_reads: usize,
    /// Orders replaced by a later decision before they came due.
    ///
    /// Only one decision is held at a time, so a strategy rebalancing faster
    /// than its own decision lag overwrites its pending order every step and
    /// never trades. That is a defensible policy -- the latest signal is the
    /// one you would send -- but it was silent, and silence is what made it
    /// expensive: a run with two thousand rebalances, zero fills and a
    /// perfectly flat equity curve looked exactly like a strategy that chose
    /// not to trade. Counting it is what tells the two apart.
    pub superseded: usize,
}

impl BacktestResult {
    pub fn final_equity(&self) -> Decimal {
        self.equity_curve
            .last()
            .map(|(_, equity)| *equity)
            .unwrap_or(self.config.initial_capital)
    }

    pub fn total_return(&self) -> f64 {
        let start = self.config.initial_capital.to_f64();
        if start.abs() < 1e-9 {
            return 0.0;
        }
        self.final_equity().to_f64() / start - 1.0
    }

    pub fn total_costs(&self) -> f64 {
        self.total_commission + self.total_spread + self.total_impact
    }

    /// Costs as a fraction of the initial capital.
    ///
    /// Reported next to the return because the interesting comparison is
    /// between the two: a 4% return that paid 3% in costs is a different
    /// strategy from one that paid 0.2%.
    pub fn cost_drag(&self) -> f64 {
        let start = self.config.initial_capital.to_f64();
        if start.abs() < 1e-9 {
            return 0.0;
        }
        self.total_costs() / start
    }

    /// Return metrics over the equity curve.
    ///
    /// The risk-free rate is zero: a backtest's excess return over cash is a
    /// separate question from whether the strategy works, and folding an
    /// assumed rate in here would make the two inseparable.
    pub fn metrics(&self) -> RiskMetrics {
        RiskMetrics::compute(&self.returns, self.periods_per_year(), 0.0)
    }

    /// Periods per year, from the configuration or inferred from the steps.
    pub fn periods_per_year(&self) -> f64 {
        if let Some(periods) = self.config.periods_per_year {
            return periods;
        }
        if self.equity_curve.len() < 2 {
            return 252.0;
        }
        let (first, _) = self.equity_curve[0];
        let (last, _) = self.equity_curve[self.equity_curve.len() - 1];
        let years = last.since(first).as_years_f64();
        if years <= 1e-9 {
            return 252.0;
        }
        (self.equity_curve.len() - 1) as f64 / years
    }

    /// Whether the result rests on assumptions that flatter it.
    ///
    /// Surfaced as a first-class field rather than a footnote: the two ways a
    /// backtest most often lies are zero costs and instantaneous execution,
    /// and both are visible here.
    pub fn caveats(&self) -> Vec<String> {
        let mut caveats = Vec::new();
        if self.config.costs.is_frictionless() {
            caveats.push(
                "costs are zero: this measures the signal's raw content, not an achievable return"
                    .to_string(),
            );
        }
        if self.config.assumptions.is_optimistic() {
            caveats.push(self.config.assumptions.describe());
        }
        if !self.rejected.is_empty() {
            caveats.push(format!(
                "{} order(s) could not be filled; the realised strategy differs from the intended one",
                self.rejected.len()
            ));
        }
        if !self.missing_prices.is_empty() {
            caveats.push(format!(
                "{} instrument(s) were requested before they had a price",
                self.missing_prices.len()
            ));
        }
        if self.returns.len() < 60 {
            caveats.push(format!(
                "{} return observations is too few for the metrics to be stable",
                self.returns.len()
            ));
        }
        caveats
    }

    /// A short account of what happened, for a report.
    pub fn summarise(&self) -> String {
        let metrics = self.metrics();
        format!(
            "{}: {:+.2}% total return over {} periods, Sharpe {:.2} (standard error {:.2}), max drawdown {:.2}%, {:.2}% cost drag across {} fill(s)",
            self.strategy,
            self.total_return() * 100.0,
            self.returns.len(),
            metrics.sharpe_ratio,
            metrics.sharpe_standard_error(),
            metrics.drawdown.max_drawdown * 100.0,
            self.cost_drag() * 100.0,
            self.fills.len()
        )
    }
}

/// Runs a strategy over history.
#[derive(Debug)]
pub struct Backtester {
    config: BacktestConfig,
}

impl Backtester {
    pub fn new(config: BacktestConfig) -> Result<Self> {
        config.costs.validate()?;
        if config.initial_capital <= Decimal::ZERO {
            return Err(Error::invalid("initial capital must be positive"));
        }
        if config.impact_window == 0 {
            return Err(Error::invalid(
                "the impact window must cover at least one bar",
            ));
        }
        Ok(Self { config })
    }

    /// Run one strategy over one clock.
    pub fn run<S: BacktestStrategy>(
        &self,
        strategy: &mut S,
        clock: &mut SimulationClock,
        universe: &Universe,
    ) -> Result<BacktestResult> {
        clock.reset();
        let Some((started_at, _)) = clock.span() else {
            return Err(Error::invalid("the simulation clock covers no time"));
        };

        let mut portfolio = Portfolio::new(
            PortfolioId::from_string(format!("bt-{}", strategy.name())),
            strategy.name(),
            self.config.currency,
            self.config.initial_capital,
            started_at,
        );

        let mut equity_curve: Vec<(Timestamp, Decimal)> = Vec::new();
        let mut fills = Vec::new();
        let mut rejected = Vec::new();
        let mut missing_prices: Vec<String> = Vec::new();
        let mut pending: Option<(Timestamp, BTreeMap<String, f64>)> = None;
        let mut rebalance_count = 0usize;
        let mut superseded = 0usize;
        let mut guarded_reads = 0usize;
        let mut finished_at = started_at;

        while let Some(now) = clock.now() {
            finished_at = now;

            // Execute anything scheduled for this instant before asking for a
            // new decision. An order placed yesterday fills before today's
            // signal is computed, which is the order events actually occur in.
            if let Some((due, targets)) = pending.take() {
                if due <= now {
                    let marks = self.marks(clock, now);
                    self.rebalance(
                        &mut portfolio,
                        universe,
                        &targets,
                        &marks,
                        clock,
                        now,
                        &mut fills,
                        &mut rejected,
                        &mut missing_prices,
                    );
                } else {
                    pending = Some((due, targets));
                }
            }

            // The decision. Everything the strategy can see is filtered to
            // `now` by the view, and the view cannot outlive this scope.
            let (targets, wants_rebalance) = {
                let Some(view) = clock.view() else { break };
                let wants = strategy.should_rebalance(&view);
                let targets = if wants {
                    Some(strategy.target_weights(&view))
                } else {
                    None
                };
                guarded_reads += view.read_count();
                (targets, wants)
            };

            if wants_rebalance
                && let Some(targets) = targets
                && !targets.is_empty()
            {
                rebalance_count += 1;
                match clock.executable_at() {
                    Some(execute_at) => {
                        if pending.is_some() {
                            superseded += 1;
                        }
                        pending = Some((execute_at, targets));
                    }
                    None => {
                        // The decision lag runs past the end of the data. A
                        // backtest that filled it anyway would award every run
                        // a free trade with perfect hindsight.
                        for object_id in targets.keys() {
                            rejected.push(RejectedOrder {
                                at: now,
                                object_id: object_id.clone(),
                                quantity: Decimal::ZERO,
                                reason:
                                    "the decision lag runs past the end of the data; no fill is possible inside the simulation"
                                        .to_string(),
                            });
                        }
                    }
                }
            }

            let marks = self.marks(clock, now);
            let valuation = portfolio.value(&marks, now);
            equity_curve.push((now, valuation.equity));

            if !clock.advance() {
                break;
            }
        }

        let returns = equity_returns(&equity_curve);
        let total_commission = fills
            .iter()
            .map(|f: &SimulatedFill| f.cost.commission)
            .sum();
        let total_spread = fills.iter().map(|f: &SimulatedFill| f.cost.spread).sum();
        let total_impact = fills.iter().map(|f: &SimulatedFill| f.cost.impact).sum();

        missing_prices.sort();
        missing_prices.dedup();

        Ok(BacktestResult {
            strategy: strategy.name().to_string(),
            started_at,
            finished_at,
            config: self.config.clone(),
            equity_curve,
            returns,
            fills,
            rejected,
            missing_prices,
            total_commission,
            total_spread,
            total_impact,
            rebalance_count,
            superseded,
            guarded_reads,
        })
    }

    /// Marks available at `now`, from bars that had closed.
    fn marks(&self, clock: &SimulationClock, now: Timestamp) -> BTreeMap<String, Decimal> {
        let Some(view) = clock.view() else {
            return BTreeMap::new();
        };
        let _ = now;
        view.available()
            .into_iter()
            .filter_map(|object| {
                view.last_close(&object)
                    .map(|close| (object.as_str().to_string(), close))
            })
            .collect()
    }

    /// Trade toward the target weights, applying costs.
    #[allow(clippy::too_many_arguments)]
    fn rebalance(
        &self,
        portfolio: &mut Portfolio,
        universe: &Universe,
        targets: &BTreeMap<String, f64>,
        marks: &BTreeMap<String, Decimal>,
        clock: &SimulationClock,
        at: Timestamp,
        fills: &mut Vec<SimulatedFill>,
        rejected: &mut Vec<RejectedOrder>,
        missing_prices: &mut Vec<String>,
    ) {
        let equity = portfolio.value(marks, at).equity.to_f64();
        if equity <= 0.0 {
            // A blown-up book cannot trade its way out inside the simulation,
            // and pretending it can produces the classic recovery-from-ruin
            // equity curve.
            return;
        }

        for (object_key, weight) in targets {
            let object_id = ObjectId::from_string(object_key.clone());
            // The instrument must be in the universe. Trading something the
            // platform holds no reference data for would mean guessing its
            // contract multiplier, and a guessed multiplier is a wrong P&L.
            let Some(object) = universe.get(&object_id) else {
                rejected.push(RejectedOrder {
                    at,
                    object_id: object_key.clone(),
                    quantity: Decimal::ZERO,
                    reason: "the instrument is not in the universe; its contract terms are unknown"
                        .to_string(),
                });
                continue;
            };
            let Some(fill_price) = clock.fill_price(&object_id, at) else {
                missing_prices.push(object_key.clone());
                continue;
            };
            let price = fill_price.to_f64();
            if price <= 0.0 {
                missing_prices.push(object_key.clone());
                continue;
            }

            let held = portfolio
                .position(&object_id)
                .map(|position| position.quantity().to_f64())
                .unwrap_or(0.0);
            let target_units = weight * equity / price;
            let delta = target_units - held;
            if delta.abs() * price < 1.0 {
                // Below one currency unit of notional: not worth an order, and
                // trading it would inflate the fill count and the commission.
                continue;
            }

            let (volume, volatility) = self.liquidity(clock, &object_id, at);
            let Some(quantity) = Decimal::from_f64(delta) else {
                rejected.push(RejectedOrder {
                    at,
                    object_id: object_key.clone(),
                    quantity: Decimal::ZERO,
                    reason: "the required quantity is not representable".to_string(),
                });
                continue;
            };

            match self
                .config
                .costs
                .cost_of(quantity, fill_price, volume, volatility)
            {
                Ok(cost) => {
                    let costs = Decimal::from_f64(cost.total()).unwrap_or(Decimal::ZERO);
                    portfolio.apply_fill(object, quantity, fill_price, costs, at, None);
                    fills.push(SimulatedFill {
                        at,
                        object_id: object_key.clone(),
                        quantity,
                        price: fill_price,
                        cost,
                    });
                }
                Err(reason) => rejected.push(RejectedOrder {
                    at,
                    object_id: object_key.clone(),
                    quantity,
                    reason: reason.describe(),
                }),
            }
        }
    }

    /// Recent volume and daily volatility for the impact model.
    fn liquidity(
        &self,
        clock: &SimulationClock,
        object_id: &ObjectId,
        at: Timestamp,
    ) -> (f64, f64) {
        let Some(view) = clock.view() else {
            return (0.0, 0.0);
        };
        let _ = at;
        let bars = view.bars(object_id);
        if bars.is_empty() {
            return (0.0, 0.0);
        }
        let window = bars.len().min(self.config.impact_window);
        let recent = &bars[bars.len() - window..];
        let volume = recent.iter().map(|bar| bar.volume.to_f64()).sum::<f64>() / window as f64;
        let closes: Vec<f64> = recent.iter().map(|bar| bar.close.to_f64()).collect();
        let volatility = stats::stddev(&stats::log_returns(&closes));
        (
            volume,
            if volatility.is_finite() {
                volatility
            } else {
                0.0
            },
        )
    }
}

/// Period returns from an equity curve.
fn equity_returns(curve: &[(Timestamp, Decimal)]) -> Vec<f64> {
    curve
        .windows(2)
        .map(|pair| {
            let previous = pair[0].1.to_f64();
            let current = pair[1].1.to_f64();
            if previous.abs() < 1e-12 {
                0.0
            } else {
                current / previous - 1.0
            }
        })
        .collect()
}
