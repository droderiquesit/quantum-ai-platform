//! The limit engine.
//!
//! Every limit here is a deterministic predicate over a proposed portfolio
//! state. There is no model, no scoring and no judgement: a limit either binds
//! or it does not, the same inputs always produce the same answer, and the
//! answer names which limit bound and by how much.
//!
//! That rigidity is the point. The risk engine holds veto authority over the
//! whole platform (charter section 5), and a veto that can be reasoned with is
//! not a veto. The place for judgement is in *setting* the limits, which is a
//! governance decision recorded in configuration.

use qip_core::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How serious a breach is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Approaching the limit. Reported, not blocking.
    Warning,
    /// The limit is breached. The action is blocked.
    Breach,
    /// Breached badly enough to require intervention beyond blocking one order.
    Critical,
}

impl Severity {
    pub fn blocks(&self) -> bool {
        matches!(self, Self::Breach | Self::Critical)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Breach => "breach",
            Self::Critical => "critical",
        }
    }
}

/// What a limit constrains.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LimitKind {
    /// Maximum notional of a single order.
    MaxOrderNotional { limit: Decimal },
    /// Maximum notional of a single position.
    MaxPositionNotional { limit: Decimal },
    /// Maximum position as a fraction of equity.
    MaxPositionWeight { limit: f64 },
    /// Maximum gross exposure as a multiple of equity.
    MaxLeverage { limit: f64 },
    /// Maximum net exposure as a multiple of equity.
    MaxNetExposure { limit: f64 },
    /// Maximum share of gross exposure in one bucket of a named axis.
    MaxConcentration { axis: String, limit: f64 },
    /// Maximum gross exposure in any one bucket of a named axis, as a
    /// fraction of equity.
    ///
    /// [`LimitKind::MaxBucketExposure`] with the bucket left unnamed: the
    /// same arithmetic over every bucket the axis carries, so a book cannot
    /// concentrate into a bucket nobody thought to write down in advance.
    ///
    /// It exists because [`LimitKind::MaxConcentration`] divides one bucket
    /// by the sum of the buckets, and the first position in an empty book is
    /// the whole of its axis. That cap therefore read 1.0 for the first order
    /// of any size, in any instrument, in any deployment carrying the shipped
    /// defaults — a desk that loaded a real catalogue traded nothing at all.
    /// It is the mirror of the `MaxExpectedShortfall` defect this file
    /// already records: that limit could never fire, this one could never
    /// not, and both read as protection. Equity is the denominator because it
    /// is known before an order exists and the order under check does not
    /// move it; a ratio a pre-trade veto divides by is not allowed to be a
    /// number the order itself creates.
    MaxAxisWeight { axis: String, limit: f64 },
    /// Maximum gross exposure to one named bucket, as a fraction of equity.
    MaxBucketExposure {
        axis: String,
        bucket: String,
        limit: f64,
    },
    /// Maximum portfolio volatility, annualised.
    MaxVolatility { limit: f64 },
    /// Maximum value at risk as a fraction of equity.
    MaxValueAtRisk { confidence: f64, limit: f64 },
    /// Maximum expected shortfall as a fraction of equity.
    MaxExpectedShortfall { confidence: f64, limit: f64 },
    /// Maximum drawdown from the running peak before trading halts.
    MaxDrawdown { limit: f64 },
    /// Maximum loss over a single day, as a fraction of equity.
    MaxDailyLoss { limit: f64 },
    /// Minimum fraction of the portfolio liquidatable within a horizon.
    MinLiquidity { days: f64, fraction: f64 },
    /// Maximum days to exit a single position.
    MaxDaysToLiquidate { limit: f64 },
    /// Maximum gross exposure to one counterparty, as a fraction of equity.
    MaxCounterpartyExposure { limit: f64 },
    /// Minimum cash as a fraction of equity.
    MinCashBuffer { limit: f64 },
}

impl LimitKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::MaxOrderNotional { .. } => "max_order_notional",
            Self::MaxPositionNotional { .. } => "max_position_notional",
            Self::MaxPositionWeight { .. } => "max_position_weight",
            Self::MaxLeverage { .. } => "max_leverage",
            Self::MaxNetExposure { .. } => "max_net_exposure",
            Self::MaxConcentration { .. } => "max_concentration",
            Self::MaxAxisWeight { .. } => "max_axis_weight",
            Self::MaxBucketExposure { .. } => "max_bucket_exposure",
            Self::MaxVolatility { .. } => "max_volatility",
            Self::MaxValueAtRisk { .. } => "max_value_at_risk",
            Self::MaxExpectedShortfall { .. } => "max_expected_shortfall",
            Self::MaxDrawdown { .. } => "max_drawdown",
            Self::MaxDailyLoss { .. } => "max_daily_loss",
            Self::MinLiquidity { .. } => "min_liquidity",
            Self::MaxDaysToLiquidate { .. } => "max_days_to_liquidate",
            Self::MaxCounterpartyExposure { .. } => "max_counterparty_exposure",
            Self::MinCashBuffer { .. } => "min_cash_buffer",
        }
    }

    /// Whether the limit is a floor rather than a ceiling.
    pub fn is_minimum(&self) -> bool {
        matches!(self, Self::MinLiquidity { .. } | Self::MinCashBuffer { .. })
    }

    /// Whether the limit's denominator is part of the same state the order
    /// under check changes.
    ///
    /// A pre-trade veto is a question about one order, so its answer must
    /// depend on that order's size. `MaxConcentration` divides a bucket by
    /// the sum of the buckets, so an order that creates the only bucket
    /// creates its own denominator: the observed value is 1.0 at every
    /// non-zero size and the bisection in `PreTradeChecker::largest_permissible`
    /// — which assumes a zero-size order passes and that the predicate is
    /// monotone in size — converges to zero and refuses everything. Nothing
    /// about the threshold could have fixed that.
    ///
    /// The match is exhaustive with no wildcard, so a seventeenth kind cannot
    /// be added without someone answering this question about it. That is the
    /// whole point of the method: the question was never asked of
    /// `MaxConcentration`, and it shipped in every default set.
    pub fn denominator_moves_with_the_order(&self) -> bool {
        match self {
            Self::MaxConcentration { .. } => true,
            Self::MaxOrderNotional { .. }
            | Self::MaxPositionNotional { .. }
            | Self::MaxPositionWeight { .. }
            | Self::MaxLeverage { .. }
            | Self::MaxNetExposure { .. }
            | Self::MaxAxisWeight { .. }
            | Self::MaxBucketExposure { .. }
            | Self::MaxVolatility { .. }
            | Self::MaxValueAtRisk { .. }
            | Self::MaxExpectedShortfall { .. }
            | Self::MaxDrawdown { .. }
            | Self::MaxDailyLoss { .. }
            | Self::MinLiquidity { .. }
            | Self::MaxDaysToLiquidate { .. }
            | Self::MaxCounterpartyExposure { .. }
            | Self::MinCashBuffer { .. } => false,
        }
    }
}

/// A configured limit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    pub name: String,
    pub kind: LimitKind,
    /// Fraction of the limit at which a warning is raised. 0.8 warns at 80%.
    pub warning_threshold: f64,
    /// Multiple of the limit above which the breach is critical.
    pub critical_multiple: f64,
    /// Whether breaching this limit forces liquidation rather than just
    /// blocking new risk.
    pub forces_reduction: bool,
    /// Why the limit exists, so a breach report explains itself.
    pub rationale: String,
}

impl Limit {
    pub fn new(name: impl Into<String>, kind: LimitKind) -> Self {
        Self {
            name: name.into(),
            kind,
            warning_threshold: 0.85,
            critical_multiple: 1.25,
            forces_reduction: false,
            rationale: String::new(),
        }
    }

    pub fn with_rationale(mut self, rationale: impl Into<String>) -> Self {
        self.rationale = rationale.into();
        self
    }

    pub fn forcing_reduction(mut self) -> Self {
        self.forces_reduction = true;
        self
    }

    /// Evaluate an observed value against the limit.
    ///
    /// `observed` and `bound` are in the limit's own units.
    fn assess(&self, observed: f64, bound: f64) -> Option<Severity> {
        if self.kind.is_minimum() {
            if observed < bound {
                let shortfall = if bound > 1e-12 {
                    (bound - observed) / bound
                } else {
                    1.0
                };
                return Some(if shortfall > self.critical_multiple - 1.0 {
                    Severity::Critical
                } else {
                    Severity::Breach
                });
            }
            if bound > 1e-12 && observed < bound / self.warning_threshold.max(1e-9) {
                return Some(Severity::Warning);
            }
            return None;
        }

        if observed > bound {
            let ratio = if bound > 1e-12 {
                observed / bound
            } else {
                f64::INFINITY
            };
            return Some(if ratio >= self.critical_multiple {
                Severity::Critical
            } else {
                Severity::Breach
            });
        }
        if bound > 1e-12 && observed > bound * self.warning_threshold {
            return Some(Severity::Warning);
        }
        None
    }
}

/// A limit that bound, and by how much.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitBreach {
    pub limit_name: String,
    pub limit_kind: String,
    pub severity: Severity,
    /// The value that was measured.
    pub observed: f64,
    /// The threshold it was measured against.
    pub bound: f64,
    /// Observed divided by bound. Above one for a ceiling breach.
    pub utilisation: f64,
    /// The bucket or instrument responsible, where the limit is per-bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub detail: String,
    pub forces_reduction: bool,
}

impl LimitBreach {
    pub fn blocks(&self) -> bool {
        self.severity.blocks()
    }
}

/// The state a limit set is evaluated against.
#[derive(Clone, Debug, Default)]
pub struct RiskState {
    pub equity: Decimal,
    pub cash: Decimal,
    pub gross_exposure: Decimal,
    pub net_exposure: Decimal,
    /// Notional per position, keyed by instrument.
    pub position_notionals: BTreeMap<String, Decimal>,
    /// Gross exposure per bucket, keyed by axis then bucket.
    pub axis_exposures: BTreeMap<String, BTreeMap<String, Decimal>>,
    /// Annualised portfolio volatility.
    pub volatility: f64,
    /// Value at risk as a fraction of equity, by confidence.
    pub value_at_risk: BTreeMap<String, f64>,
    /// Expected shortfall as a fraction of equity, by confidence.
    pub expected_shortfall: BTreeMap<String, f64>,
    /// Current drawdown from the running peak.
    pub drawdown: f64,
    /// Loss today as a fraction of equity, positive for a loss.
    pub daily_loss: f64,
    /// Days to liquidate each position.
    pub days_to_liquidate: BTreeMap<String, f64>,
    /// Fraction of the portfolio liquidatable within a given number of days.
    pub liquidatable_within: BTreeMap<String, f64>,
    /// Gross exposure per counterparty.
    pub counterparty_exposures: BTreeMap<String, Decimal>,
    /// Notional of the order being checked, when checking one.
    pub order_notional: Option<Decimal>,
    /// Instrument the order concerns.
    pub order_subject: Option<String>,
}

impl RiskState {
    fn ratio(&self, value: Decimal) -> f64 {
        if !self.equity.is_positive() {
            return f64::INFINITY;
        }
        value.to_f64() / self.equity.to_f64()
    }

    /// Populate the tail figures the given limits will read, from a return
    /// series.
    ///
    /// [`LimitKind::MaxValueAtRisk`] and [`LimitKind::MaxExpectedShortfall`]
    /// look their figure up in a map keyed by confidence and record nothing
    /// when the key is absent. Until anything filled those maps, both limits
    /// shipped in [`LimitSet::conservative_default`] took the `None` arm on
    /// every book, so every deployment believed it held two controls it did
    /// not have. A control that cannot fire reads as protection and is not.
    ///
    /// [`LimitKind::MaxVolatility`] read the same way: no key, just
    /// `RiskState::volatility` itself, which no production caller ever set —
    /// `RiskState::from_figures` does not touch it and `PreTradeChecker::project`
    /// deliberately leaves it alone, so the shipped volatility limit was the
    /// same defect under a different name, just without a map to expose it.
    /// It is filled here because this is the one place a return series and the
    /// limit set that needs it are already both in hand.
    ///
    /// The value-at-risk and expected-shortfall keys are derived from each
    /// configured limit's own confidence, formatted exactly as the limit
    /// formats it when it reads. Computing a fixed set of confidences here
    /// instead would put the key on one side of a rounding boundary and the
    /// lookup on the other — `{:.2}` of 0.975 is `0.97`, and the default
    /// expected-shortfall limit uses 0.975 — and the limit would go on
    /// silently never evaluating.
    ///
    /// `returns` are period returns of the whole book, already in `f64`:
    /// this is the crossing point from the book's [`Decimal`] equity to a
    /// statistic, and the caller makes it by dividing consecutive equity
    /// samples. A series shorter than two leaves the maps empty and the
    /// volatility field untouched rather than recording zero, because a zero
    /// nobody computed would pass every one of these limits and look like
    /// evidence the book has no risk at all.
    pub fn with_tail_risk(mut self, limits: &LimitSet, returns: &[f64]) -> Self {
        if returns.len() < 2 {
            return self;
        }
        for limit in &limits.limits {
            match limit.kind {
                LimitKind::MaxValueAtRisk { confidence, .. } => {
                    self.value_at_risk.insert(
                        format!("{confidence:.2}"),
                        crate::metrics::historical_var(returns, confidence),
                    );
                }
                LimitKind::MaxExpectedShortfall { confidence, .. } => {
                    self.expected_shortfall.insert(
                        format!("{confidence:.2}"),
                        crate::metrics::expected_shortfall(returns, confidence),
                    );
                }
                LimitKind::MaxVolatility { .. } => {
                    self.volatility = crate::metrics::annualised_volatility(returns);
                }
                _ => {}
            }
        }
        self
    }

    /// Populate the fraction of the book liquidatable within each configured
    /// [`LimitKind::MinLiquidity`] horizon, from `days_to_liquidate` and
    /// `position_notionals` this state already carries.
    ///
    /// [`LimitKind::MinLiquidity`] looks its figure up in `liquidatable_within`,
    /// keyed by horizon, and records nothing when the key is absent — the same
    /// shape of failure [`Self::with_tail_risk`] closed for the tail limits.
    /// [`LimitSet::conservative_default`] has shipped a `liquidity` limit since
    /// before this method existed, and nothing filled the map it reads: the
    /// limit took its `None` arm on every book. A control that cannot fire
    /// reads as protection and is not.
    ///
    /// An instrument with no entry in `days_to_liquidate` is **not** treated
    /// as liquidatable — its notional still counts in the denominator, so it
    /// pulls the ratio down rather than being dropped from both sides. A
    /// fraction computed only over the positions with a known exit time would
    /// read more liquid the less anyone had told it, which is the wrong
    /// direction for a floor to fail in; refusing to guess an unknown exit
    /// time is the fail-closed choice.
    ///
    /// The key is `{days:.0}`, formatted exactly as the limit's own lookup
    /// formats it when it reads — one rule, not two copies of the same
    /// format a rounding boundary could separate.
    ///
    /// Leaves `liquidatable_within` untouched when the book holds no
    /// positions: an empty book has nothing to be illiquid, and recording a
    /// fabricated `1.0` for a ratio nobody measured is the same mistake in
    /// the other direction.
    ///
    /// Also leaves it untouched when `days_to_liquidate` itself is empty —
    /// nobody has marked a single instrument, which is a book that has never
    /// been measured, not a book with zero liquid positions. The fail-closed
    /// rule above governs *partial* coverage, once measurement has started;
    /// it does not manufacture a floor-breaching `0.0` for a book a caller
    /// has not marked at all, the same way an all-too-short return series
    /// leaves the tail maps empty in [`Self::with_tail_risk`] rather than
    /// recording a zero nobody computed.
    pub fn with_liquidity_horizons(mut self, limits: &LimitSet) -> Self {
        if self.days_to_liquidate.is_empty() {
            return self;
        }
        let total: Decimal = self.position_notionals.values().map(|v| v.abs()).sum();
        if !total.is_positive() {
            return self;
        }
        for limit in &limits.limits {
            if let LimitKind::MinLiquidity { days, .. } = limit.kind {
                let liquidatable: Decimal = self
                    .position_notionals
                    .iter()
                    .filter(|(instrument, _)| {
                        self.days_to_liquidate
                            .get(instrument.as_str())
                            .is_some_and(|exit_days| *exit_days <= days)
                    })
                    .map(|(_, notional)| notional.abs())
                    .sum();
                self.liquidatable_within
                    .insert(format!("{days:.0}"), liquidatable.to_f64() / total.to_f64());
            }
        }
        self
    }
}

/// The outcome of checking a state against a limit set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitCheck {
    pub breaches: Vec<LimitBreach>,
    /// Number of limits evaluated, so a check against an empty set is visible.
    pub evaluated: usize,
}

impl LimitCheck {
    /// Whether anything blocks.
    pub fn is_blocked(&self) -> bool {
        self.breaches.iter().any(LimitBreach::blocks)
    }

    /// Whether the state requires reducing risk, not merely stopping.
    pub fn requires_reduction(&self) -> bool {
        self.breaches
            .iter()
            .any(|b| b.blocks() && b.forces_reduction)
    }

    /// Breaches that block, worst first.
    pub fn blocking(&self) -> Vec<&LimitBreach> {
        let mut blocking: Vec<&LimitBreach> = self.breaches.iter().filter(|b| b.blocks()).collect();
        blocking.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then_with(|| {
                    b.utilisation
                        .partial_cmp(&a.utilisation)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.limit_name.cmp(&b.limit_name))
        });
        blocking
    }

    pub fn warnings(&self) -> Vec<&LimitBreach> {
        self.breaches
            .iter()
            .filter(|b| b.severity == Severity::Warning)
            .collect()
    }

    /// A single sentence naming what blocked.
    pub fn reason(&self) -> String {
        match self.blocking().first() {
            None => "within all limits".to_string(),
            Some(worst) => format!(
                "{} ({}): {} against a limit of {} — {}",
                worst.limit_name,
                worst.severity.as_str(),
                format_number(worst.observed),
                format_number(worst.bound),
                worst.detail
            ),
        }
    }
}

fn format_number(value: f64) -> String {
    if value.abs() >= 1000.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.4}")
    }
}

/// A named collection of limits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LimitSet {
    pub name: String,
    pub limits: Vec<Limit>,
}

impl LimitSet {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            limits: Vec::new(),
        }
    }

    pub fn with(mut self, limit: Limit) -> Self {
        self.limits.push(limit);
        self
    }

    pub fn len(&self) -> usize {
        self.limits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.limits.is_empty()
    }

    /// Evaluate every limit against a state.
    pub fn check(&self, state: &RiskState) -> LimitCheck {
        let mut breaches = Vec::new();
        for limit in &self.limits {
            breaches.extend(self.evaluate(limit, state));
        }
        LimitCheck {
            breaches,
            evaluated: self.limits.len(),
        }
    }

    fn evaluate(&self, limit: &Limit, state: &RiskState) -> Vec<LimitBreach> {
        let mut out = Vec::new();
        let mut record = |observed: f64, bound: f64, subject: Option<String>, detail: String| {
            if let Some(severity) = limit.assess(observed, bound) {
                out.push(LimitBreach {
                    limit_name: limit.name.clone(),
                    limit_kind: limit.kind.label().to_string(),
                    severity,
                    observed,
                    bound,
                    utilisation: if bound.abs() > 1e-12 {
                        observed / bound
                    } else {
                        f64::INFINITY
                    },
                    subject,
                    detail,
                    forces_reduction: limit.forces_reduction,
                });
            }
        };

        match &limit.kind {
            LimitKind::MaxOrderNotional { limit: bound } => {
                if let Some(notional) = state.order_notional {
                    record(
                        notional.abs().to_f64(),
                        bound.to_f64(),
                        state.order_subject.clone(),
                        "single order notional".into(),
                    );
                }
            }
            LimitKind::MaxPositionNotional { limit: bound } => {
                for (instrument, notional) in &state.position_notionals {
                    record(
                        notional.abs().to_f64(),
                        bound.to_f64(),
                        Some(instrument.clone()),
                        format!("position notional in {instrument}"),
                    );
                }
            }
            LimitKind::MaxPositionWeight { limit: bound } => {
                for (instrument, notional) in &state.position_notionals {
                    record(
                        state.ratio(notional.abs()),
                        *bound,
                        Some(instrument.clone()),
                        format!("{instrument} as a fraction of equity"),
                    );
                }
            }
            LimitKind::MaxLeverage { limit: bound } => {
                record(
                    state.ratio(state.gross_exposure),
                    *bound,
                    None,
                    "gross exposure over equity".into(),
                );
            }
            LimitKind::MaxNetExposure { limit: bound } => {
                record(
                    state.ratio(state.net_exposure.abs()),
                    *bound,
                    None,
                    "net exposure over equity".into(),
                );
            }
            LimitKind::MaxConcentration { axis, limit: bound } => {
                let Some(buckets) = state.axis_exposures.get(axis) else {
                    return out;
                };
                let total: Decimal = buckets.values().map(|v| v.abs()).sum();
                if !total.is_positive() {
                    return out;
                }
                for (bucket, value) in buckets {
                    record(
                        value.abs().to_f64() / total.to_f64(),
                        *bound,
                        Some(bucket.clone()),
                        format!("share of gross exposure in {axis} bucket {bucket}"),
                    );
                }
            }
            LimitKind::MaxAxisWeight { axis, limit: bound } => {
                let Some(buckets) = state.axis_exposures.get(axis) else {
                    // An instrument the catalogue holds no record for reaches
                    // no bucket at all (`qip-kernel`'s `exposure_axes`), so an
                    // absent axis is a fact about the reference data and not a
                    // concentration. Refusing here would refuse an order for
                    // something the book did not do.
                    return out;
                };
                for (bucket, value) in buckets {
                    // No guard on a zero denominator and no early return:
                    // `RiskState::ratio` answers `f64::INFINITY` on
                    // non-positive equity, so a book with no equity fails
                    // every weight limit instead of silently skipping them.
                    // The share-of-gross arm above returns early on a zero
                    // axis total, and that early return is the fail-open half
                    // of the same defect.
                    record(
                        state.ratio(value.abs()),
                        *bound,
                        Some(bucket.clone()),
                        format!("{axis} bucket {bucket} as a fraction of equity"),
                    );
                }
            }
            LimitKind::MaxBucketExposure {
                axis,
                bucket,
                limit: bound,
            } => {
                let value = state
                    .axis_exposures
                    .get(axis)
                    .and_then(|b| b.get(bucket))
                    .copied()
                    .unwrap_or(Decimal::ZERO);
                record(
                    state.ratio(value.abs()),
                    *bound,
                    Some(bucket.clone()),
                    format!("exposure to {axis} bucket {bucket}"),
                );
            }
            LimitKind::MaxVolatility { limit: bound } => {
                record(
                    state.volatility,
                    *bound,
                    None,
                    "annualised portfolio volatility".into(),
                );
            }
            LimitKind::MaxValueAtRisk {
                confidence,
                limit: bound,
            } => {
                let key = format!("{confidence:.2}");
                if let Some(value) = state.value_at_risk.get(&key) {
                    record(
                        *value,
                        *bound,
                        None,
                        format!("value at risk at {confidence:.0}%"),
                    );
                }
            }
            LimitKind::MaxExpectedShortfall {
                confidence,
                limit: bound,
            } => {
                let key = format!("{confidence:.2}");
                if let Some(value) = state.expected_shortfall.get(&key) {
                    record(
                        *value,
                        *bound,
                        None,
                        format!("expected shortfall at {confidence:.0}%"),
                    );
                }
            }
            LimitKind::MaxDrawdown { limit: bound } => {
                record(
                    state.drawdown,
                    *bound,
                    None,
                    "drawdown from the running peak".into(),
                );
            }
            LimitKind::MaxDailyLoss { limit: bound } => {
                record(
                    state.daily_loss,
                    *bound,
                    None,
                    "loss today over equity".into(),
                );
            }
            LimitKind::MinLiquidity { days, fraction } => {
                let key = format!("{days:.0}");
                if let Some(value) = state.liquidatable_within.get(&key) {
                    record(
                        *value,
                        *fraction,
                        None,
                        format!("fraction liquidatable within {days:.0} days"),
                    );
                }
            }
            LimitKind::MaxDaysToLiquidate { limit: bound } => {
                for (instrument, days) in &state.days_to_liquidate {
                    record(
                        *days,
                        *bound,
                        Some(instrument.clone()),
                        format!("days to exit {instrument}"),
                    );
                }
            }
            LimitKind::MaxCounterpartyExposure { limit: bound } => {
                for (counterparty, value) in &state.counterparty_exposures {
                    record(
                        state.ratio(value.abs()),
                        *bound,
                        Some(counterparty.clone()),
                        format!("exposure to {counterparty}"),
                    );
                }
            }
            LimitKind::MinCashBuffer { limit: bound } => {
                record(
                    state.ratio(state.cash),
                    *bound,
                    None,
                    "cash over equity".into(),
                );
            }
        }
        out
    }

    /// The limit set the platform ships with for paper trading.
    ///
    /// Deliberately conservative. These are the defaults a deployment starts
    /// from and tightens; they are not calibrated to any particular mandate.
    ///
    /// The two per-axis caps measure a bucket against **equity**, not against
    /// gross exposure. They were share-of-gross until ADR 0027, and a share of
    /// gross is 100% for the first position in an empty book, so the set
    /// refused the first order of every deployment that fed it a real
    /// catalogue. The bounds are unchanged at 0.35 and 0.60 so that the
    /// denominator is the only thing this change moved; against a
    /// `position-weight` of 0.10 they bind at four names in one sector and six
    /// in one country, which is a real control and not a calibrated one. The
    /// numbers are the desk's.
    pub fn conservative_default() -> Self {
        Self::new("conservative-paper")
            .with(
                Limit::new(
                    "order-notional",
                    LimitKind::MaxOrderNotional {
                        limit: Decimal::from_int(250_000),
                    },
                )
                .with_rationale("bounds the damage of a single mis-sized order"),
            )
            .with(
                Limit::new(
                    "position-weight",
                    LimitKind::MaxPositionWeight { limit: 0.10 },
                )
                .with_rationale("no single name may dominate the book"),
            )
            .with(
                Limit::new("leverage", LimitKind::MaxLeverage { limit: 1.5 })
                    .forcing_reduction()
                    .with_rationale("gross exposure beyond this cannot be unwound in a stress"),
            )
            .with(
                Limit::new(
                    "sector-concentration",
                    LimitKind::MaxAxisWeight {
                        axis: "sector".into(),
                        limit: 0.35,
                    },
                )
                .with_rationale(
                    "a sector bet must be deliberate, not accumulated: no sector may hold more \
                     than this share of equity",
                ),
            )
            .with(
                Limit::new(
                    "country-concentration",
                    LimitKind::MaxAxisWeight {
                        axis: "country".into(),
                        limit: 0.60,
                    },
                )
                .with_rationale("bounds single-jurisdiction political and currency risk"),
            )
            .with(
                Limit::new("volatility", LimitKind::MaxVolatility { limit: 0.25 })
                    .with_rationale("keeps the book inside its stated risk profile"),
            )
            .with(
                Limit::new(
                    "value-at-risk",
                    LimitKind::MaxValueAtRisk {
                        confidence: 0.99,
                        limit: 0.05,
                    },
                )
                .with_rationale("a one-in-a-hundred day must not cost more than this"),
            )
            .with(
                Limit::new(
                    "expected-shortfall",
                    LimitKind::MaxExpectedShortfall {
                        confidence: 0.975,
                        limit: 0.08,
                    },
                )
                .with_rationale("bounds the average loss beyond the value-at-risk point"),
            )
            .with(
                Limit::new("drawdown", LimitKind::MaxDrawdown { limit: 0.15 })
                    .forcing_reduction()
                    .with_rationale("halts trading before a drawdown becomes unrecoverable"),
            )
            .with(
                Limit::new("daily-loss", LimitKind::MaxDailyLoss { limit: 0.04 })
                    .forcing_reduction()
                    .with_rationale("stops a bad day from becoming a bad quarter"),
            )
            .with(
                Limit::new(
                    "liquidity",
                    LimitKind::MinLiquidity {
                        days: 5.0,
                        fraction: 0.80,
                    },
                )
                .with_rationale("most of the book must be exitable within a week"),
            )
            .with(
                Limit::new("cash-buffer", LimitKind::MinCashBuffer { limit: 0.02 })
                    .with_rationale("settlement and margin need headroom"),
            )
    }
}
