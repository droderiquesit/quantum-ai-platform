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
use qip_world_model::vocabulary::{
    AltMetric, CREDIT_QUOTE_NEEDED, FUTURES_CURVE_NEEDED, MacroSeries, OPTION_QUOTE_NEEDED,
    RATE_LEGS_NEEDED, names,
};
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

/// The macro series the analyst reads, and the sign each carries for a risk
/// asset. Declared rather than inferred: a sign convention discovered from the
/// data is a sign convention that flips when the sample changes. The names
/// are the vocabulary's, so the series read here is the series the macro
/// arm writes — for as long as this platform ran with a literal here, it was
/// not.
pub(crate) const MACRO_SERIES: [(MacroSeries, f64); 4] = [
    // A rising policy rate raises discount rates and hurts risk assets.
    (MacroSeries::PolicyRate, -1.0),
    // Inflation above expectation forces tighter policy.
    (MacroSeries::InflationYoy, -1.0),
    // Growth above expectation helps earnings.
    (MacroSeries::GrowthYoy, 1.0),
    // Wider credit spreads signal tightening financial conditions.
    (MacroSeries::CreditSpreadBps, -1.0),
];

impl Agent for MacroAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn accepts(&self, brief: &AgentBrief) -> bool {
        // Macro features are published with a lag measured in weeks; an
        // intraday question is not one this agent can answer. And a macro
        // view is keyed by an economy, which the analyst takes from the
        // subject instrument — with nothing to be about there is no economy
        // to read, and `"global"` is not one.
        brief.horizon >= qip_core::Duration::from_days(5) && !brief.objects.is_empty()
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        if brief.horizon < qip_core::Duration::from_days(5) {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                format!(
                    "a {:?} horizon is shorter than the publication lag of macro data",
                    brief.horizon
                ),
            ));
        }
        let Some(subject) = brief.objects.first() else {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                "a macro view needs an instrument to place in an economy",
            ));
        };

        // The economy is the instrument's geography, the same ISO code the
        // macro arm keys a release by. Read from the catalogue and never
        // defaulted: a subject the catalogue does not place gets no macro
        // view rather than some other economy's.
        let economy = {
            let market = self.desk.market.get(ctx)?;
            market
                .universe
                .get(subject)
                .map(|object| object.geography.trim().to_string())
        };
        let economy = match economy {
            Some(economy) if !economy.is_empty() => economy,
            _ => {
                return Ok(no_data(
                    ctx,
                    brief.as_of,
                    format!(
                        "{} carries no geography in the universe, so there is no economy to \
                         read the macro series for; a macro view is keyed by the instrument's \
                         economy, never by a global series",
                        subject.as_str()
                    ),
                ));
            }
        };

        let world = self.desk.world.get(ctx)?;
        let features = world.features();

        let mut readings = Vec::new();
        let mut missing = Vec::new();
        for (series, sign) in MACRO_SERIES {
            let name = series.feature();
            let history: Vec<f64> = features
                .history(name, &economy, brief.as_of)
                .iter()
                .map(|v| v.value)
                .collect();
            match z_score_of_last(&history, MINIMUM_HISTORY) {
                Some(z) => readings.push((name, sign, z, history.len())),
                None => missing.push(format!(
                    "{name}@{economy}: {} observations, {MINIMUM_HISTORY} needed",
                    history.len()
                )),
            }
        }

        if readings.is_empty() {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "no macro series for {economy} had enough history: {}",
                    missing.join("; ")
                ),
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
            &MACRO_SERIES.map(|(series, _)| series.feature()),
        ))
        .evidence(
            readings
                .iter()
                .map(|(name, _, _, _)| format!("feature:{name}@{economy}"))
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

        let history = features.history(names::CREDIT_SPREAD_BPS, subject.as_str(), brief.as_of);
        let spreads: Vec<f64> = history.iter().map(|v| v.value).collect();
        // The latest record is what the finding reports as the level, and it
        // is reported as observed from the store: the agent read it, it did
        // not produce it. `history` is at least `MINIMUM_HISTORY` long past
        // this check, so `last` cannot be empty.
        //
        // No absorb arm writes an issuer spread, so on any deployed platform
        // this is the arm taken, and the finding says what record would
        // change that rather than only that the series is empty.
        let Some(latest) = history.last().copied() else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "no spread observations for {}; needs {CREDIT_QUOTE_NEEDED}",
                    subject.as_str()
                ),
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
            names::EFFECTIVE_DURATION,
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
            names::CREDIT_SPREAD_BPS,
            subject.as_str(),
            names::CREDIT_SPREAD_BPS,
            "bps",
            latest,
        ))
        .fact(computed(
            ctx,
            "credit_spread_change_bps",
            change,
            "bps",
            &[names::CREDIT_SPREAD_BPS],
        ))
        .fact(computed(
            ctx,
            "credit_spread_z",
            z,
            "sigma",
            &[names::CREDIT_SPREAD_BPS],
        ))
        .evidence(vec![format!(
            "feature:{}@{}",
            names::CREDIT_SPREAD_BPS,
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
                        names::EFFECTIVE_DURATION,
                        subject.as_str(),
                        names::EFFECTIVE_DURATION,
                        "years",
                        duration,
                    ))
                    .fact(computed(
                        ctx,
                        "price_impact_pct",
                        price_impact_pct,
                        "percent",
                        &[names::EFFECTIVE_DURATION, "credit_spread_change_bps"],
                    ));
            }
            _ => {
                builder = builder
                    .direction(Direction::Neutral, 0.0)
                    .missing(vec![format!(
                        "{} for {}: without it a spread move cannot be translated into a price \
                         move; needs {CREDIT_QUOTE_NEEDED}",
                        names::EFFECTIVE_DURATION,
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
                    names::IMPLIED_VOLATILITY,
                    subject.as_str(),
                    brief.as_of,
                    brief.as_of,
                )
                .map(|value| {
                    observed_feature(
                        features,
                        names::IMPLIED_VOLATILITY,
                        subject.as_str(),
                        names::IMPLIED_VOLATILITY,
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
                // No absorb arm writes an implied volatility, so this is the
                // arm every deployed platform takes; the finding names the
                // record that would change that.
                missing.push(format!(
                    "{} for {}; needs {OPTION_QUOTE_NEEDED}",
                    names::IMPLIED_VOLATILITY,
                    subject.as_str()
                ));
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
            &[names::IMPLIED_VOLATILITY, "realised_volatility"],
        ))
        .fact(computed(
            ctx,
            "implied_to_realised_ratio",
            ratio,
            "ratio",
            &[names::IMPLIED_VOLATILITY, "realised_volatility"],
        ))
        .evidence(vec![
            format!(
                "feature:{}@{}",
                names::IMPLIED_VOLATILITY,
                subject.as_str()
            ),
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
        let front = read(names::FRONT_MONTH_PRICE, "price");
        let deferred = read(names::DEFERRED_MONTH_PRICE, "price");
        let months = read(names::DEFERRED_TENOR_MONTHS, "months");

        let (Some(front), Some(deferred), Some(months)) = (front, deferred, months) else {
            // No absorb arm writes a curve, so this is the arm every deployed
            // platform takes; the finding names the record that would change
            // that.
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "the curve for {} needs a front price, a deferred price and its tenor; \
                     needs {FUTURES_CURVE_NEEDED}",
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
            &[
                names::FRONT_MONTH_PRICE,
                names::DEFERRED_MONTH_PRICE,
                names::DEFERRED_TENOR_MONTHS,
            ],
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
        let base_rate = read(names::BASE_RATE);
        let quote_rate = read(names::QUOTE_RATE);
        let volatility = read(names::REALISED_VOLATILITY);

        let (base_present, quote_present) = (base_rate.is_some(), quote_rate.is_some());
        let (Some(base_fact), Some(quote_fact)) = (base_rate, quote_rate) else {
            // No absorb arm writes a rate leg, so this is the arm every
            // deployed platform takes; the finding names the record that
            // would change that.
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "carry for {} needs both legs' rates; base {}, quote {}; needs \
                     {RATE_LEGS_NEEDED}",
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
                &[names::BASE_RATE, names::QUOTE_RATE],
            ))
            .missing(vec![format!(
                "{} for {}: carry unadjusted for volatility is not a signal; needs \
                 {RATE_LEGS_NEEDED}",
                names::REALISED_VOLATILITY,
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
            &[
                names::BASE_RATE,
                names::QUOTE_RATE,
                names::REALISED_VOLATILITY,
            ],
        ))
        .fact(computed(
            ctx,
            "rate_differential",
            base_rate - quote_rate,
            "ratio",
            &[names::BASE_RATE, names::QUOTE_RATE],
        ))
        .evidence(vec![format!(
            "feature:{}@{}",
            names::BASE_RATE,
            subject.as_str()
        )])
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

/// The alternative-data metrics the analyst reads, with the sign each carries
/// for the subject. The metric and the dataset its licence is held under are
/// the vocabulary's, so the series read here is the series the
/// alternative-data arm writes — until it was, the kernel wrote
/// `alt/{dataset}/{metric}` and this table read the bare metric.
pub(crate) const ALT_METRICS: [(AltMetric, f64); 3] = [
    (AltMetric::WebTrafficIndex, 1.0),
    (AltMetric::CardSpendIndex, 1.0),
    (AltMetric::JobPostingsIndex, 1.0),
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
        for (metric, sign) in ALT_METRICS {
            let (name, dataset) = (metric.feature(), metric.dataset());
            if !self.is_licensed(dataset) {
                unlicensed.push(dataset);
                continue;
            }
            // The licence is per dataset and the check above was by dataset,
            // so the series under this name must be that dataset's: the
            // definition's producer is the dataset the arm wrote it from,
            // and a series produced by anything else is not read.
            if let Some(definition) = features.definition(name)
                && definition.producer != dataset
            {
                missing.push(format!(
                    "{name}: the series is produced by `{}`, not the licensed `{dataset}`",
                    definition.producer
                ));
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
