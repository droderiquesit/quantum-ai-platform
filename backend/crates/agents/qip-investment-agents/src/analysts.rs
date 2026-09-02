//! The specialist research analysts.
//!
//! Seven agents, each competent in one domain and explicit about where its
//! competence stops. The pattern is the same throughout: read what the
//! manifest permits, compute, and report a direction with a conviction derived
//! from the arithmetic — or report honestly that there was nothing to work
//! with.
//!
//! No analyst asks a language model what it thinks. Where one is available it
//! is used for narrative only, and the numeric guard in `qip-ai` makes that
//! structural rather than a matter of discipline.

use crate::desk::Desk;
use crate::support::{
    FindingBuilder, computed, conviction_from_z, direction_from, no_data, observed_feature,
    out_of_scope, robust_z_score_of_last, z_score_of_last,
};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use qip_financial::asset_class::AssetClass;
use qip_numerics::stats;
use qip_quant::signal;
use std::sync::Arc;

/// How much history a standardisation needs before it means anything.
const MINIMUM_HISTORY: usize = 30;

/// Reading below which a signal is treated as noise rather than a view.
const NOISE_DEAD_ZONE: f64 = 0.5;

// --- macro ------------------------------------------------------------------

/// Reads the macro environment from the world model's feature store.
#[derive(Debug)]
pub struct MacroAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl MacroAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

/// The macro features the analyst reads, and the sign each carries for a risk
/// asset. Declared rather than inferred: a sign convention discovered from the
/// data is a sign convention that flips when the sample changes.
const MACRO_FEATURES: [(&str, f64); 4] = [
    // A rising policy rate raises discount rates and hurts risk assets.
    ("policy_rate", -1.0),
    // Inflation above expectation forces tighter policy.
    ("inflation_yoy", -1.0),
    // Growth above expectation helps earnings.
    ("growth_yoy", 1.0),
    // Wider credit spreads signal tightening financial conditions.
    ("credit_spread_bps", -1.0),
];

impl Agent for MacroAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn accepts(&self, brief: &AgentBrief) -> bool {
        // Macro features are published with a lag measured in weeks; an
        // intraday question is not one this agent can answer.
        brief.horizon >= qip_core::Duration::from_days(5)
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        if !self.accepts(brief) {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                format!(
                    "a {:?} horizon is shorter than the publication lag of macro data",
                    brief.horizon
                ),
            ));
        }

        let world = self.desk.world.get(ctx)?;
        let features = world.features();

        let mut readings = Vec::new();
        let mut missing = Vec::new();
        for (name, sign) in MACRO_FEATURES {
            let history: Vec<f64> = features
                .history(name, "global", brief.as_of)
                .iter()
                .map(|v| v.value)
                .collect();
            match z_score_of_last(&history, MINIMUM_HISTORY) {
                Some(z) => readings.push((name, sign, z, history.len())),
                None => missing.push(format!(
                    "{name}: {} observations, {MINIMUM_HISTORY} needed",
                    history.len()
                )),
            }
        }

        if readings.is_empty() {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!("no macro series had enough history: {}", missing.join("; ")),
            ));
        }

        // Equal-weighted because the relative importance of these four is not
        // stable across regimes, and a weighting fitted to one regime is worse
        // than no weighting at all.
        let composite: f64 =
            readings.iter().map(|(_, sign, z, _)| sign * z).sum::<f64>() / readings.len() as f64;

        let mut builder = FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "the macro environment reads {:+.2} sigma for risk assets across {} series",
                composite,
                readings.len()
            ),
        )
        .direction(
            direction_from(composite, NOISE_DEAD_ZONE),
            conviction_from_z(composite),
        )
        .fact(computed(
            ctx,
            "macro_composite_z",
            composite,
            "sigma",
            &MACRO_FEATURES.map(|(n, _)| n),
        ))
        .evidence(
            readings
                .iter()
                .map(|(name, _, _, _)| format!("feature:{name}@global"))
                .collect(),
        )
        .falsifiers(vec![
            "the next policy decision goes the other way".to_string(),
            "the composite reverts inside one standard deviation within the horizon".to_string(),
        ]);

        for (name, sign, z, _) in &readings {
            builder = builder.fact(computed(
                ctx,
                &format!("{name}_z"),
                sign * z,
                "sigma",
                &[name],
            ));
        }
        if !missing.is_empty() {
            builder = builder.missing(missing);
        }
        builder.build()
    }
}

// --- equity -----------------------------------------------------------------

/// Reads a listed equity from its price history and sector context.
#[derive(Debug)]
pub struct EquityAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl EquityAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for EquityAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn accepts(&self, brief: &AgentBrief) -> bool {
        !brief.objects.is_empty()
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let market = self.desk.market.get(ctx)?;
        let Some(subject) = brief.objects.first() else {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                "an equity view needs an instrument to be about",
            ));
        };

        // The instrument must actually be equity. An agent answering outside
        // its competence is worse than one that declines.
        if let Some(object) = market.universe.get(subject)
            && object.asset_class != AssetClass::Equity
        {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                format!(
                    "{} is {}, which is outside the equity remit",
                    subject.as_str(),
                    object.asset_class.as_str()
                ),
            ));
        }

        let Some(state) = market.snapshot.get(subject) else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!("no market state for {}", subject.as_str()),
            ));
        };

        // Point-in-time: only bars that had closed by the as-of time.
        let bars = state.bars.as_of(brief.as_of);
        let closes: Vec<f64> = bars
            .iter()
            .map(|b| b.close.to_f64())
            .filter(|c| c.is_finite() && *c > 0.0)
            .collect();
        if closes.len() < MINIMUM_HISTORY {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "{} closed bars for {} as of {}, {MINIMUM_HISTORY} needed",
                    closes.len(),
                    subject.as_str(),
                    brief.as_of
                ),
            ));
        }

        let momentum = signal::momentum(&closes, 20.min(closes.len() - 1)).unwrap_or(0.0);
        let reversion = signal::mean_reversion(&closes, 10.min(closes.len() - 1)).unwrap_or(0.0);
        let volatility = signal::realised_volatility(&closes, 20.min(closes.len() - 1), 252.0)
            .unwrap_or(f64::NAN);

        // Momentum and reversion measure different horizons and genuinely
        // disagree; combining them without saying so would hide that. The
        // longer-horizon signal leads, and the disagreement is reported.
        let combined = 0.65 * momentum + 0.35 * reversion;
        let disagree = momentum * reversion < 0.0;

        let returns = stats::log_returns(&closes);
        let z = z_score_of_last(&returns, MINIMUM_HISTORY).unwrap_or(0.0);

        let mut caveats = Vec::new();
        if disagree {
            caveats.push(format!(
                "trend ({momentum:+.3}) and reversion ({reversion:+.3}) disagree; the horizons differ"
            ));
        }
        if volatility.is_finite() && volatility > 0.6 {
            caveats.push(format!(
                "realised volatility is {:.0}% annualised; position sizing should reflect it",
                volatility * 100.0
            ));
        }

        FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "{} shows a combined trend and reversion reading of {combined:+.3}",
                subject.as_str()
            ),
        )
        .direction(
            direction_from(combined * 10.0, NOISE_DEAD_ZONE),
            conviction_from_z(combined * 10.0),
        )
        .fact(computed(ctx, "momentum_20d", momentum, "ratio", &["close"]))
        .fact(computed(
            ctx,
            "mean_reversion_10d",
            reversion,
            "ratio",
            &["close"],
        ))
        .fact(computed(
            ctx,
            "realised_volatility_annualised",
            volatility,
            "ratio",
            &["close"],
        ))
        .fact(computed(ctx, "last_return_z", z, "sigma", &["close"]))
        .evidence(vec![format!("bars:{}@{}", subject.as_str(), brief.as_of)])
        .falsifiers(vec![
            format!("{} reverses the move within the horizon", subject.as_str()),
            "realised volatility doubles, making the reading noise".to_string(),
        ])
        .caveats(caveats)
        .build()
    }
}

// --- credit -----------------------------------------------------------------

/// Reads credit spreads and what a move in them does to a bond's price.
#[derive(Debug)]
pub struct CreditAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl CreditAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for CreditAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let Some(subject) = brief.objects.first() else {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                "a credit view needs an instrument to be about",
            ));
        };
        let world = self.desk.world.get(ctx)?;
        let features = world.features();

        let history = features.history("credit_spread_bps", subject.as_str(), brief.as_of);
        let spreads: Vec<f64> = history.iter().map(|v| v.value).collect();
        // The latest record is what the finding reports as the level, and it
        // is reported as observed from the store: the agent read it, it did
        // not produce it. `history` is at least `MINIMUM_HISTORY` long past
        // this check, so `last` cannot be empty.
        let Some(latest) = history.last().copied() else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!("no spread observations for {}", subject.as_str()),
            ));
        };
        if spreads.len() < MINIMUM_HISTORY {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "{} spread observations for {}, {MINIMUM_HISTORY} needed",
                    spreads.len(),
                    subject.as_str()
                ),
            ));
        }

        // Robust standardisation: credit spreads gap, and one gap in the
        // history would otherwise inflate the denominator enough to hide the
        // next one.
        let Some(z) = robust_z_score_of_last(&spreads, MINIMUM_HISTORY) else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!("{} spread history does not vary", subject.as_str()),
            ));
        };

        let level = spreads[spreads.len() - 1];
        let change = level - spreads[spreads.len() - 2];

        // Duration is required to translate a spread move into a price move.
        // Without it the agent reports the spread and says the translation is
        // missing, rather than assuming a duration.
        let duration = features.value_as_of(
            "effective_duration",
            subject.as_str(),
            brief.as_of,
            brief.as_of,
        );

        let mut builder = FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "{} trades at {level:.0}bp, {z:+.2} sigma against its own history",
                subject.as_str()
            ),
        )
        .fact(observed_feature(
            features,
            "credit_spread_bps",
            subject.as_str(),
            "credit_spread_bps",
            "bps",
            latest,
        ))
        .fact(computed(
            ctx,
            "credit_spread_change_bps",
            change,
            "bps",
            &["credit_spread_bps"],
        ))
        .fact(computed(
            ctx,
            "credit_spread_z",
            z,
            "sigma",
            &["credit_spread_bps"],
        ))
        .evidence(vec![format!(
            "feature:credit_spread_bps@{}",
            subject.as_str()
        )])
        .falsifiers(vec![
            "the spread reverts to its median within the horizon".to_string(),
            "a rating action explains the move, making it fundamental rather than technical"
                .to_string(),
        ]);

        match duration {
            Some(duration) if duration.value.is_finite() && duration.value > 0.0 => {
                // A wide spread that reverts is a price gain, so the direction
                // for the bondholder is the opposite of the spread's sign.
                let price_impact_pct = -duration.value * change / 10_000.0 * 100.0;
                builder = builder
                    .direction(direction_from(-z, NOISE_DEAD_ZONE), conviction_from_z(z))
                    .fact(observed_feature(
                        features,
                        "effective_duration",
                        subject.as_str(),
                        "effective_duration",
                        "years",
                        duration,
                    ))
                    .fact(computed(
                        ctx,
                        "price_impact_pct",
                        price_impact_pct,
                        "percent",
                        &["effective_duration", "credit_spread_change_bps"],
                    ));
            }
            _ => {
                builder = builder
                    .direction(Direction::Neutral, 0.0)
                    .missing(vec![format!(
                        "effective_duration for {}: without it a spread move cannot be translated into a price move",
                        subject.as_str()
                    )]);
            }
        }

        builder.build()
    }
}

// --- derivatives ------------------------------------------------------------

/// Compares implied to realised volatility.
#[derive(Debug)]
pub struct DerivativesAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl DerivativesAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for DerivativesAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let Some(subject) = brief.objects.first() else {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                "a volatility view needs an underlying",
            ));
        };

        // The implied volatility is read, not derived, and the fact says so.
        // Built inside the borrow so the finding carries the store's record
        // rather than a number copied out of it.
        let implied = {
            let world = self.desk.world.get(ctx)?;
            let features = world.features();
            features
                .value_as_of(
                    "implied_volatility",
                    subject.as_str(),
                    brief.as_of,
                    brief.as_of,
                )
                .map(|value| {
                    observed_feature(
                        features,
                        "implied_volatility",
                        subject.as_str(),
                        "implied_volatility",
                        "ratio",
                        value,
                    )
                })
        };
        let market = self.desk.market.get(ctx)?;
        let realised = market.snapshot.get(subject).and_then(|state| {
            let closes: Vec<f64> = state
                .bars
                .as_of(brief.as_of)
                .iter()
                .map(|b| b.close.to_f64())
                .collect();
            signal::realised_volatility(&closes, 20, 252.0)
        });

        let implied_missing = implied.is_none();
        let (Some(implied_fact), Some(realised)) = (implied, realised) else {
            let mut missing = Vec::new();
            if implied_missing {
                missing.push(format!("implied_volatility for {}", subject.as_str()));
            }
            if realised.is_none() {
                missing.push(format!(
                    "enough price history to compute realised volatility for {}",
                    subject.as_str()
                ));
            }
            return Ok(no_data(ctx, brief.as_of, missing.join("; ")));
        };
        let implied = implied_fact.value;

        // The variance risk premium: what option buyers pay over what the
        // underlying has actually delivered. Positive means options are rich.
        let premium = implied - realised;
        let ratio = if realised > 1e-9 {
            implied / realised
        } else {
            f64::NAN
        };

        // A positive premium means selling volatility is attractive, so the
        // direction on the *option* is negative.
        FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "{} implies {:.1}% volatility against {:.1}% realised, a {:+.1}pt premium",
                subject.as_str(),
                implied * 100.0,
                realised * 100.0,
                premium * 100.0
            ),
        )
        .direction(
            direction_from(-premium * 20.0, NOISE_DEAD_ZONE),
            conviction_from_z(premium * 20.0),
        )
        .fact(implied_fact)
        .fact(computed(
            ctx,
            "realised_volatility",
            realised,
            "ratio",
            &["close"],
        ))
        .fact(computed(
            ctx,
            "variance_risk_premium",
            premium,
            "ratio",
            &["implied_volatility", "realised_volatility"],
        ))
        .fact(computed(
            ctx,
            "implied_to_realised_ratio",
            ratio,
            "ratio",
            &["implied_volatility", "realised_volatility"],
        ))
        .evidence(vec![
            format!("feature:implied_volatility@{}", subject.as_str()),
            format!("bars:{}@{}", subject.as_str(), brief.as_of),
        ])
        .falsifiers(vec![
            "realised volatility rises to meet the implied level within the horizon".to_string(),
            "a scheduled event explains the premium, in which case it is not a premium".to_string(),
        ])
        .caveats(vec![
            "realised volatility is backward looking; the premium may be compensation for a known future event"
                .to_string(),
        ])
        .build()
    }
}

// --- commodities ------------------------------------------------------------

/// Reads futures curve shape and the roll yield it implies.
#[derive(Debug)]
pub struct CommoditiesAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl CommoditiesAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for CommoditiesAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let Some(subject) = brief.objects.first() else {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                "a commodity view needs an instrument",
            ));
        };
        {
            let market = self.desk.market.get(ctx)?;
            if let Some(object) = market.universe.get(subject)
                && object.asset_class != AssetClass::Commodity
            {
                return Ok(out_of_scope(
                    ctx,
                    brief.as_of,
                    format!(
                        "{} is {}, outside the commodity remit",
                        subject.as_str(),
                        object.asset_class.as_str()
                    ),
                ));
            }
        }

        let world = self.desk.world.get(ctx)?;
        let features = world.features();
        // Each curve point is a reading from the store, and is reported as
        // one. The roll yield is the only number here this agent produces.
        let read = |name: &str, unit: &str| {
            features
                .value_as_of(name, subject.as_str(), brief.as_of, brief.as_of)
                .map(|value| observed_feature(features, name, subject.as_str(), name, unit, value))
        };
        let front = read("front_month_price", "price");
        let deferred = read("deferred_month_price", "price");
        let months = read("deferred_tenor_months", "months");

        let (Some(front), Some(deferred), Some(months)) = (front, deferred, months) else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "the curve for {} needs a front price, a deferred price and its tenor",
                    subject.as_str()
                ),
            ));
        };
        if front.value <= 0.0 || months.value <= 0.0 {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!("{} curve inputs are not usable", subject.as_str()),
            ));
        }

        // Backwardation — a deferred price below the front — pays a positive
        // roll yield to a long position.
        let annualised_roll = (front.value - deferred.value) / front.value * (12.0 / months.value);
        let backwardated = deferred.value < front.value;

        FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "{} is in {} with an annualised roll of {:+.1}%",
                subject.as_str(),
                if backwardated {
                    "backwardation"
                } else {
                    "contango"
                },
                annualised_roll * 100.0
            ),
        )
        .direction(
            direction_from(annualised_roll * 10.0, NOISE_DEAD_ZONE),
            conviction_from_z(annualised_roll * 10.0),
        )
        .fact(computed(
            ctx,
            "annualised_roll_yield",
            annualised_roll,
            "ratio",
            &["front_month_price", "deferred_month_price", "deferred_tenor_months"],
        ))
        .fact(front)
        .fact(deferred)
        .fact(months)
        .evidence(vec![format!("curve:{}@{}", subject.as_str(), brief.as_of)])
        .falsifiers(vec![
            "the curve flattens or inverts within the horizon".to_string(),
            "storage or inventory data explains the shape, making the roll a cost rather than a signal"
                .to_string(),
        ])
        .caveats(vec![
            "roll yield is a carry component, not a directional view on the spot price".to_string(),
        ])
        .build()
    }
}

// --- FX and rates -----------------------------------------------------------

/// Reads rate differentials and the carry they imply.
#[derive(Debug)]
pub struct FxRatesAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl FxRatesAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for FxRatesAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let Some(subject) = brief.objects.first() else {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                "a carry view needs a currency pair or rates instrument",
            ));
        };

        let world = self.desk.world.get(ctx)?;
        let features = world.features();
        // The two legs and the volatility are readings, reported as observed
        // from the store; the differential and the carry are this agent's.
        let read = |name: &str| {
            features
                .value_as_of(name, subject.as_str(), brief.as_of, brief.as_of)
                .map(|value| {
                    observed_feature(features, name, subject.as_str(), name, "ratio", value)
                })
        };
        let base_rate = read("base_rate");
        let quote_rate = read("quote_rate");
        let volatility = read("realised_volatility");

        let (base_present, quote_present) = (base_rate.is_some(), quote_rate.is_some());
        let (Some(base_fact), Some(quote_fact)) = (base_rate, quote_rate) else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "carry for {} needs both legs' rates; base {}, quote {}",
                    subject.as_str(),
                    if base_present { "present" } else { "missing" },
                    if quote_present { "present" } else { "missing" },
                ),
            ));
        };
        let base_rate = base_fact.value;
        let quote_rate = quote_fact.value;

        // Carry without a volatility adjustment is the classic way to look
        // good until the currency moves, so an unadjusted number is reported
        // as partial rather than as a view.
        let Some(volatility_fact) = volatility.filter(|v| v.value.is_finite() && v.value > 1e-6)
        else {
            return FindingBuilder::new(
                ctx,
                brief.as_of,
                format!(
                    "{} carries {:+.2}% but the volatility to judge it against is missing",
                    subject.as_str(),
                    (base_rate - quote_rate) * 100.0
                ),
            )
            .direction(Direction::Neutral, 0.0)
            .fact(base_fact)
            .fact(quote_fact)
            .fact(computed(
                ctx,
                "rate_differential",
                base_rate - quote_rate,
                "ratio",
                &["base_rate", "quote_rate"],
            ))
            .missing(vec![format!(
                "realised_volatility for {}: carry unadjusted for volatility is not a signal",
                subject.as_str()
            )])
            .build();
        };

        let volatility = volatility_fact.value;
        let carry = signal::carry(base_rate, quote_rate, volatility).unwrap_or(0.0);

        FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "{} offers {:+.2} of carry per unit of volatility",
                subject.as_str(),
                carry
            ),
        )
        .direction(
            direction_from(carry, NOISE_DEAD_ZONE),
            conviction_from_z(carry),
        )
        .fact(base_fact)
        .fact(quote_fact)
        .fact(volatility_fact)
        .fact(computed(
            ctx,
            "volatility_adjusted_carry",
            carry,
            "ratio",
            &["base_rate", "quote_rate", "realised_volatility"],
        ))
        .fact(computed(
            ctx,
            "rate_differential",
            base_rate - quote_rate,
            "ratio",
            &["base_rate", "quote_rate"],
        ))
        .evidence(vec![format!("feature:base_rate@{}", subject.as_str())])
        .falsifiers(vec![
            "either central bank moves against the differential within the horizon".to_string(),
            "realised volatility rises enough to erase the risk-adjusted carry".to_string(),
        ])
        .caveats(vec![
            "carry trades lose money in the tail; this reading says nothing about the tail"
                .to_string(),
        ])
        .build()
    }
}

// --- alternative data -------------------------------------------------------

/// Reads licensed alternative datasets, and refuses those it may not use.
#[derive(Debug)]
pub struct AlternativeDataAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
    /// Datasets whose licence permits use in a production investment decision.
    /// Supplied at construction from the data-licensing register, never
    /// inferred: a licence question answered by an agent is not answered.
    licensed_for_decisions: Vec<String>,
}

impl AlternativeDataAnalyst {
    pub fn new(
        manifest: AgentManifest,
        desk: Arc<Desk>,
        licensed_for_decisions: Vec<String>,
    ) -> Self {
        Self {
            manifest,
            desk,
            licensed_for_decisions,
        }
    }

    fn is_licensed(&self, dataset: &str) -> bool {
        self.licensed_for_decisions.iter().any(|d| d == dataset)
    }
}

/// The alternative-data features the analyst reads, with the dataset each
/// belongs to so the licence can be checked before the value is used.
const ALT_FEATURES: [(&str, &str, f64); 3] = [
    ("web_traffic_index", "web-traffic", 1.0),
    ("card_spend_index", "card-spend", 1.0),
    ("job_postings_index", "job-postings", 1.0),
];

impl Agent for AlternativeDataAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let Some(subject) = brief.objects.first() else {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                "an alternative-data view needs a subject",
            ));
        };
        let world = self.desk.world.get(ctx)?;
        let features = world.features();

        let mut readings = Vec::new();
        let mut missing = Vec::new();
        let mut unlicensed = Vec::new();
        for (name, dataset, sign) in ALT_FEATURES {
            if !self.is_licensed(dataset) {
                unlicensed.push(dataset);
                continue;
            }
            let history: Vec<f64> = features
                .history(name, subject.as_str(), brief.as_of)
                .iter()
                .map(|v| v.value)
                .collect();
            match robust_z_score_of_last(&history, MINIMUM_HISTORY) {
                Some(z) => readings.push((name, sign * z)),
                None => missing.push(format!("{name}: {} observations", history.len())),
            }
        }

        if readings.is_empty() {
            let mut reason = Vec::new();
            if !unlicensed.is_empty() {
                reason.push(format!(
                    "not licensed for investment decisions: {}",
                    unlicensed.join(", ")
                ));
            }
            if !missing.is_empty() {
                reason.push(format!("insufficient history: {}", missing.join("; ")));
            }
            return Ok(no_data(
                ctx,
                brief.as_of,
                if reason.is_empty() {
                    "no alternative data covers this subject".to_string()
                } else {
                    reason.join("; ")
                },
            ));
        }

        let composite: f64 = readings.iter().map(|(_, z)| z).sum::<f64>() / readings.len() as f64;

        let mut caveats = vec![
            "alternative data is a proxy; the relationship to the fundamental it stands for can break without notice"
                .to_string(),
        ];
        if !unlicensed.is_empty() {
            caveats.push(format!("excluded for licensing: {}", unlicensed.join(", ")));
        }

        let mut builder = FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "licensed alternative data reads {composite:+.2} sigma for {} across {} series",
                subject.as_str(),
                readings.len()
            ),
        )
        .direction(
            direction_from(composite, NOISE_DEAD_ZONE),
            // Alternative data is noisy, so its conviction is discounted
            // against the same reading from a primary source.
            conviction_from_z(composite) * 0.7,
        )
        .fact(computed(
            ctx,
            "alt_data_composite_z",
            composite,
            "sigma",
            &readings.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        ))
        .evidence(
            readings
                .iter()
                .map(|(name, _)| format!("feature:{name}@{}", subject.as_str()))
                .collect(),
        )
        .falsifiers(vec![
            "the next reported fundamental contradicts the proxy".to_string(),
            "the composite reverts within one sigma over the horizon".to_string(),
        ])
        .caveats(caveats);

        if !missing.is_empty() {
            builder = builder.missing(missing);
        }
        builder.build()
    }
}
