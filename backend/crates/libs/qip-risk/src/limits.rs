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

/// The exposure axis [`LimitKind::MaxCounterpartyExposure`] reads.
///
/// One literal, exported, because the producer and the reader are in different
/// crates: `qip-kernel` charges each fill to a bucket under this name and this
/// module looks it up. An axis spelled two ways is a limit that cannot fire,
/// and this cap has already been one — see the field it replaced on
/// [`RiskState`].
pub const COUNTERPARTY_AXIS: &str = "counterparty";

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
    ///
    /// Read from the [`COUNTERPARTY_AXIS`] bucket of [`RiskState::axis_exposures`],
    /// which is the same running per-bucket counter every other axis limit
    /// reads and is maintained by [`crate::aggregate::RiskAggregates::apply_fill`].
    ///
    /// It used to read a `RiskState::counterparty_exposures` map of its own,
    /// and that map had no producer anywhere: the sole writer was
    /// `PreTradeChecker::project`, which added the *change in one instrument's*
    /// exposure to an always-empty starting balance, and both production
    /// callers of the execution engine's submit path named no counterparty at
    /// all. So the cap could not fire — and had the call sites simply started
    /// naming one, it would have fired against a per-order number wearing a
    /// book-level name, which is worse. Two representations of one fact
    /// disagree eventually; there is now one.
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
    /// The match is exhaustive with no wildcard, so an eighteenth kind cannot
    /// be added without someone answering this question about it. There are
    /// seventeen — `grep -c '=> "max_\|=> "min_' src/limits.rs` — and this
    /// sentence said "a seventeenth" after the count had already reached it,
    /// which would have let the next author think the arm they were adding was
    /// the one the rule already covered. That is the
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
    /// Days to liquidate each position, as the caller's liquidity model has
    /// marked it. Supplied, never derived here: a day count comes from average
    /// daily volume and market depth, and this crate holds neither.
    pub days_to_liquidate: BTreeMap<String, f64>,
    /// Fraction of the portfolio liquidatable within a given number of days,
    /// keyed by `{days:.0}` — exactly as [`LimitKind::MinLiquidity`] formats
    /// its own lookup.
    ///
    /// **Filled by one producer, and that producer is not this crate.**
    /// `qip-kernel`'s `Platform::liquidatable_within` sums a
    /// `qip_financial::ladder::LiquidityLadder` whose construction *proved*
    /// that exit cost rises as the ladder descends, and files
    /// [`Self::unevaluated`] when it cannot. This crate held a second
    /// derivation — `RiskState::with_liquidity_horizons`, which refiltered
    /// `days_to_liquidate` into the same ratio — and it is gone rather than
    /// repaired, for two reasons that are worth keeping written down because
    /// the obvious fix was to repair it:
    ///
    /// * It could not file an honest refusal. Handed a book with holdings and
    ///   no marks, it returned the state unchanged, so a reader saw the same
    ///   empty map a passing floor produces. Filing [`Self::unevaluated`]
    ///   instead would have meant *guessing why* somebody else's liquidity
    ///   model produced nothing — "never run" and "run and refused" are the
    ///   caller's facts, not this crate's, and [`Self::with_unevaluated`]
    ///   exists precisely so the producer states its own reason.
    /// * Two derivations of one number disagree eventually, and the louder one
    ///   is wrong. A refilter of a flat map cannot notice that the rung
    ///   classification underneath it contradicts the reference data; the
    ///   ladder refuses on exactly that.
    ///
    /// So a state whose producer computes no liquidity leaves this empty, and
    /// [`LimitKind::MinLiquidity`] records nothing — which is safe only
    /// because that producer files [`Self::unevaluated`] and
    /// `PreTradeChecker::check` refuses on it. Anything that fills this map
    /// without being able to explain its own silence re-opens the gap.
    pub liquidatable_within: BTreeMap<String, f64>,
    // Gross exposure per counterparty used to be a map of its own here. It is
    // now a bucket of `axis_exposures` under `COUNTERPARTY_AXIS`, because the
    // separate map had no producer: `RiskState::from_figures` never filled it,
    // no aggregate counted it, and the only writer was
    // `PreTradeChecker::project` adding one instrument's delta to an empty
    // balance. `MaxCounterpartyExposure` therefore evaluated an empty loop on
    // every book while counting in `LimitCheck::evaluated`. Charging the
    // counterparty as an axis puts it on the same running per-bucket counter
    // every other axis limit already reads, so there is one derivation of the
    // number rather than two.
    /// Notional of the order being checked, when checking one.
    pub order_notional: Option<Decimal>,
    /// Instrument the order concerns.
    pub order_subject: Option<String>,
    /// Figures the producer of this state set out to compute and could not,
    /// keyed by the figure's own name and carrying the refusal that stopped
    /// it.
    ///
    /// **A non-empty map refuses orders.** `PreTradeChecker::check` rejects
    /// every order while one entry stands, and that is the point of the field
    /// rather than a side effect of it. Every limit that reads a keyed figure
    /// takes its `None` arm when the key is absent — [`LimitKind::MinLiquidity`]
    /// looks `liquidatable_within` up by horizon and records nothing when
    /// there is nothing there — so at the venue an unevaluated control and a
    /// control that passed are the same event: no breach. That is not
    /// hypothetical here. `Platform::liquidity_ladder` refuses a book whose
    /// rungs do not get more expensive as they descend, which one listed
    /// instrument quoted wider than 250bps is enough to cause; the refusal
    /// left `liquidatable_within` empty, the shipped `liquidity` floor
    /// abstained, and a book that had just refused ten orders out of ten
    /// accepted ten out of ten on the same universe with one spread changed.
    ///
    /// The message is kept, not just the fact, because "the floor did not run"
    /// and "the floor did not run because the catalogue quotes AAA at 300bps"
    /// are different sentences to the operator who has to fix it.
    ///
    /// Empty by default, and that means *no figure was attempted and failed* —
    /// not "everything is fine". Only a producer knows what it tried, so only
    /// a producer fills this. A state built by a caller that never computes a
    /// liquidity ladder carries no claim about liquidity either way.
    pub unevaluated: BTreeMap<String, String>,
}

impl RiskState {
    fn ratio(&self, value: Decimal) -> f64 {
        if !self.equity.is_positive() {
            return f64::INFINITY;
        }
        value.to_f64() / self.equity.to_f64()
    }

    /// Record that a figure this state was meant to carry could not be
    /// computed, with the refusal that stopped it.
    ///
    /// Deliberately not a setter for the figure itself. A producer that could
    /// not compute a number must not be able to file a substitute for it: a
    /// fabricated zero passes nothing and fails everything, a fabricated one
    /// passes everything, and both read downstream as a measurement. This
    /// records the absence and leaves the number absent.
    ///
    /// `figure` is a fixed name from the producer's own source — it reaches a
    /// metric label — and never anything derived from an instrument, a
    /// strategy or an order.
    pub fn with_unevaluated(
        mut self,
        figure: impl Into<String>,
        refusal: impl Into<String>,
    ) -> Self {
        self.unevaluated.insert(figure.into(), refusal.into());
        self
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
    ///
    /// **This implies [`Self::is_blocked`]** — a breach that forces a
    /// reduction is a breach that blocks — and the implication is the reason
    /// the order of the two questions matters at every call site. The one
    /// production caller, `qip_investment_agents`'s `RiskControl`, asked
    /// `is_blocked()` first and this second, so the second arm was unreachable
    /// and the `forcing_reduction()` marks on the shipped `leverage`,
    /// `drawdown` and `daily-loss` limits changed nothing anywhere. Ask this
    /// one first.
    pub fn requires_reduction(&self) -> bool {
        !self.forcing_reduction().is_empty()
    }

    /// The breaches that block *and* demand the book be brought back inside
    /// the limit, worst first.
    ///
    /// Separate from [`Self::blocking`] because the two answer different
    /// questions for an operator: how many limits stop new risk, and how many
    /// of those the desk declared cannot be left standing. A caller that had
    /// only the first had to report the blocking count on both arms, which is
    /// how the reduction arm came to describe itself with
    /// `blocking.len().max(1)` — a number that is right only when every
    /// blocking breach happens to force a reduction.
    pub fn forcing_reduction(&self) -> Vec<&LimitBreach> {
        self.blocking()
            .into_iter()
            .filter(|breach| breach.forces_reduction)
            .collect()
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
                // An absent axis records nothing, exactly as `MaxAxisWeight`
                // treats an absent axis: a book whose fills were never charged
                // to a counterparty made no statement about counterparty
                // concentration, and inventing a breach for it would refuse
                // orders over reference data rather than over the book.
                let Some(counterparties) = state.axis_exposures.get(COUNTERPARTY_AXIS) else {
                    return out;
                };
                for (counterparty, value) in counterparties {
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
