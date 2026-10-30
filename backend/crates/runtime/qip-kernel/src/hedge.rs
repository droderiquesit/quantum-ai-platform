//! The Hedge Brain in the cycle (blueprint §14.1; R12-RISK-4105..4109,
//! R12-CAPITAL-7204, R12-RISK-7205).
//!
//! [`crate::hedge_review`] runs the declared, single-beta hedge *policies* —
//! "keep this sector inside its cap with that future" — and stays, because a
//! standing policy a person wrote is the right tool for a standing exposure.
//! This module is the other half §14.1 asks for: once per DECIDE it reads the
//! book on every axis the blueprint lists, scores every declared hedge
//! candidate across every world SIMULATE produced, and returns the cheapest
//! portfolio of one or two candidates that keeps the book above its survival
//! floor in each of them, as a §24.9 [`HedgePlan`] — or the reasons there is
//! none.
//!
//! # What it reads, and from where
//!
//! | Input | Taken from |
//! |---|---|
//! | the exposure survey | the risk state the gate reads, the stress report, the price history and the survival budget — see [`ExposureSurvey`] |
//! | the forecast worlds | every world in `StressReport`: the library, the causal stresses, the adversarial sequence and the tail worlds, each with the book's loss in it |
//! | each candidate's move in a world | its declared loading on each library factor times that scenario's shock — a world with no factor shock (a counterparty failure, a venue outage) is one the candidate does nothing in, never one it is assumed to protect |
//! | contract multiplier and lot | the catalogue record, never configuration ([`crate::hedge_review`]'s rule) |
//! | price | the snapshot's reference price, refused when absent |
//! | the budget | the hedge capacity the survival waterfall withheld this construction |
//! | the floor | the same stress tolerance the survival kernel withholds against |
//!
//! The worlds are weighted equally. SIMULATE produces scenarios, not a
//! calibrated probability over them, and a weight written here would be a
//! number nobody measured; equal weight affects only the reported edge given
//! up, never which worlds must be protected — every world past the floor is
//! protected whatever its weight.
//!
//! # Independent of alpha, and not an enforcement path
//!
//! Nothing here imports a strategy, a signal, a model or an agent; its inputs
//! are measurements the risk lane already holds (R12-CAPITAL-7204, checked by
//! `qip-acceptance`'s `survival_independence`). And it vetoes nothing: the plan
//! is journalled and reported, and a hedge reaches a market only as an order
//! the Risk Gate evaluates against the survival ceiling — which includes the
//! hedge capacity this plan is budgeted inside — exactly like every other
//! order (R12-RISK-7205). A second enforcement path beside the gate is the
//! thing §31.1 forbids, so the type this module returns holds no order.

use crate::platform::StressReport;
use qip_contracts::HedgePlan;
use qip_contracts::control::{
    Exposure as PlanExposure, HedgeInstrument as PlanInstrument, HedgeLiquidity, HedgePlanFields,
    HedgeRollback, HedgeSide as PlanSide,
};
use qip_contracts::validated::Contract;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_events::{EventBody, Topic};
use qip_financial::Universe;
use qip_market::snapshot::MarketSnapshot;
use qip_risk::CAUSAL_DRIVER_AXIS;
use qip_risk::FACTOR_AXIS;
use qip_risk::hedge_search::{
    ForecastWorld, Greeks, HedgeCandidate, HedgeObjective, HedgeSearch, InstrumentClass, LegSide,
    SearchRefusal, WorldMove,
};
use qip_risk::limits::{COUNTERPARTY_AXIS, RiskState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The largest share of stressed daily volume a plan may need to exit.
pub const MAX_VOLUME_SHARE: f64 = 0.10;

/// The longest a plan may take to unwind, in days.
pub const MAX_UNWIND_DAYS: f64 = 5.0;

/// The holding period carry is charged over, in days: a month of sessions.
pub const HOLDING_DAYS: f64 = 21.0;

/// The factor name whose shock is a change in volatility rather than a price
/// move — the scenario library's own spelling.
const VOLATILITY_FACTOR: &str = "volatility";

/// One hedge candidate as a deployment declares it.
///
/// Judgement only: which instrument, at which venue, of which class, with
/// which sensitivities, at what cost and depth. The instrument's contract
/// multiplier and lot come from the catalogue and its price from the
/// snapshot, so neither can drift from the record they describe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HedgeCandidateDeclaration {
    pub instrument: ObjectId,
    pub venue: String,
    pub class: InstrumentClass,
    pub greeks: Greeks,
    pub carry_bps_per_day: f64,
    pub spread_bps: f64,
    pub average_daily_volume: Decimal,
    #[serde(default)]
    pub counterparty_loss_rate: f64,
    pub basis_volatility: f64,
    /// The instrument's loading on each scenario-library factor. Its move in
    /// a world is the sum of loading times shock over that world's factors.
    pub factor_loadings: BTreeMap<String, f64>,
}

impl HedgeCandidateDeclaration {
    fn resolve(
        &self,
        universe: &Universe,
        snapshot: &MarketSnapshot,
        library: &BTreeMap<String, Vec<(String, f64)>>,
    ) -> Result<HedgeCandidate> {
        let record = universe.get(&self.instrument).ok_or_else(|| {
            Error::invalid(format!(
                "hedge candidate {} is not in the catalogue this platform was assembled over, so \
                 it has no contract multiplier or lot to size by; add it to the catalogue — the \
                 mechanics are never defaulted",
                self.instrument.as_str()
            ))
        })?;
        let price = snapshot
            .get(&self.instrument)
            .and_then(|state| state.reference_price())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "hedge candidate {} has no reference price in the snapshot; a hedge sized \
                     against a guessed price is a guessed hedge",
                    record.symbol
                ))
            })?;
        let mut moves = BTreeMap::new();
        for (world, shocks) in library {
            let mut step = WorldMove {
                underlying: 0.0,
                volatility: 0.0,
            };
            let mut moved = false;
            for (factor, magnitude) in shocks {
                if factor == VOLATILITY_FACTOR {
                    step.volatility += magnitude;
                    moved = true;
                }
                if let Some(loading) = self.factor_loadings.get(factor) {
                    step.underlying += loading * magnitude;
                    moved = true;
                }
            }
            if moved {
                moves.insert(world.clone(), step);
            }
        }
        Ok(HedgeCandidate {
            object_id: record.object_id.clone(),
            symbol: record.symbol.clone(),
            venue: self.venue.clone(),
            class: self.class,
            price,
            contract_multiplier: record.contract_multiplier,
            lot_size: record.lot_size,
            greeks: self.greeks,
            carry_bps_per_day: self.carry_bps_per_day,
            spread_bps: self.spread_bps,
            average_daily_volume: self.average_daily_volume,
            counterparty_loss_rate: self.counterparty_loss_rate,
            basis_volatility: self.basis_volatility,
            moves,
        })
    }
}

/// The book's Greeks. Delta is the book's net signed notional. Gamma and vega
/// are `Some(0.0)` only when every held position is an instrument with no
/// convexity or volatility sensitivity by construction (see
/// [`is_linear`]); the catalogue carries no per-position option
/// sensitivities, so a book holding anything else reports them `None`, and an
/// unmeasured Greek must not read as a flat one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookGreeks {
    pub delta: Decimal,
    pub gamma: Option<f64>,
    pub vega: Option<f64>,
}

/// The most pairs the survey keeps, so a large book does not journal its
/// whole correlation matrix every cycle.
pub const MAX_PAIR_CORRELATIONS: usize = 16;

/// The correlation of two held positions' returns over their shared window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairCorrelation {
    pub a: String,
    pub b: String,
    pub correlation: f64,
}

/// The book read on every axis §14.1 lists (R12-RISK-4105).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExposureSurvey {
    /// Gross charged to each causal driver.
    pub drivers: BTreeMap<String, Decimal>,
    /// Exposure per risk factor.
    pub factors: BTreeMap<String, Decimal>,
    pub greeks: BookGreeks,
    /// Days to exit per position, from the liquidity ladder.
    pub liquidity: BTreeMap<String, f64>,
    /// Mean pairwise correlation of the held positions' returns over their
    /// shared history; `None` below two measurable positions.
    pub correlation: Option<f64>,
    /// The most strongly correlated held pairs, by absolute correlation, at
    /// most [`MAX_PAIR_CORRELATIONS`] of them: the mean above hides a book
    /// whose risk is two names moving together among many that do not.
    /// Defaulted so a record journaled before the pairs were read decodes.
    #[serde(default)]
    pub pair_correlations: Vec<PairCorrelation>,
    /// Unfunded commitments and margin the survival waterfall withheld.
    pub funding: Decimal,
    /// Gross per counterparty.
    pub counterparty: BTreeMap<String, Decimal>,
    /// Loss as a fraction of equity in every stressed world.
    pub tail: BTreeMap<String, f64>,
}

impl ExposureSurvey {
    /// Every axis the survey reads, in a fixed order.
    pub const AXES: [&'static str; 8] = [
        "drivers",
        "factors",
        "greeks",
        "liquidity",
        "correlation",
        "funding",
        "counterparty",
        "tail",
    ];

    /// The axes on which this survey holds a measured figure. An axis the book
    /// carries nothing on reads as measured-and-empty only where the source
    /// was read; `greeks` counts as measured through delta, with gamma and
    /// vega named in [`Self::unmeasured`].
    pub fn measured(&self) -> Vec<&'static str> {
        let mut axes = vec![
            "drivers",
            "factors",
            "greeks",
            "liquidity",
            "funding",
            "counterparty",
        ];
        if self.correlation.is_some() {
            axes.push("correlation");
        }
        if !self.tail.is_empty() {
            axes.push("tail");
        }
        axes
    }

    /// What the survey could not measure, said rather than read as zero.
    pub fn unmeasured(&self) -> Vec<&'static str> {
        let mut gaps = Vec::new();
        if self.greeks.gamma.is_none() {
            gaps.push("gamma");
        }
        if self.greeks.vega.is_none() {
            gaps.push("vega");
        }
        if self.correlation.is_none() {
            gaps.push("correlation");
        }
        if self.tail.is_empty() {
            gaps.push("tail");
        }
        gaps
    }
}

/// Whether an instrument type has no convexity and no volatility sensitivity
/// by construction, so its gamma and vega are exactly zero rather than
/// unknown. Deliberately a short list: every type not named here (options,
/// variance swaps, convertibles, bonds, swaps, structured credit) is one the
/// catalogue gives no sensitivities for, and stays unmeasured.
fn is_linear(kind: qip_financial::asset_class::InstrumentType) -> bool {
    use qip_financial::asset_class::InstrumentType as T;
    matches!(
        kind,
        T::CommonStock
            | T::PreferredStock
            | T::DepositaryReceipt
            | T::ExchangeTradedFund
            | T::Index
            | T::Future
            | T::Forward
            | T::FxSpot
            | T::FxForward
            | T::CommoditySpot
            | T::CommodityFuture
    )
}

/// The book's (gamma, vega): a measured zero when every held position is a
/// linear instrument in the catalogue, `None` when any is not, or is absent
/// from it and so cannot be known to be.
fn book_option_greeks(held: &[String], universe: &Universe) -> (Option<f64>, Option<f64>) {
    let linear = held.iter().all(|object| {
        universe
            .get(&ObjectId::from_string(object.clone()))
            .is_some_and(|record| is_linear(record.instrument_type))
    });
    if linear {
        (Some(0.0), Some(0.0))
    } else {
        (None, None)
    }
}

/// Mean and per-pair correlation of the held positions' returns over their
/// shared window. The pairs are the strongest by absolute value, ties broken
/// by name, at most [`MAX_PAIR_CORRELATIONS`].
fn correlations(
    held: &[String],
    history: &BTreeMap<String, Vec<f64>>,
) -> (Option<f64>, Vec<PairCorrelation>) {
    let series: Vec<(&String, Vec<f64>)> = held
        .iter()
        .filter_map(|object| history.get(object).map(|prices| (object, prices)))
        .map(|(object, prices)| {
            let returns = prices
                .windows(2)
                .filter(|pair| pair[0] > 0.0 && pair[1].is_finite() && pair[0].is_finite())
                .map(|pair| pair[1] / pair[0] - 1.0)
                .collect::<Vec<f64>>();
            (object, returns)
        })
        .filter(|(_, returns)| returns.len() >= 2)
        .collect();
    let Some(window) = series.iter().map(|(_, r)| r.len()).min() else {
        return (None, Vec::new());
    };
    if series.len() < 2 || window < 2 {
        return (None, Vec::new());
    }
    let tails: Vec<&[f64]> = series.iter().map(|(_, s)| &s[s.len() - window..]).collect();
    let n = window as f64;
    let stats: Vec<(f64, f64)> = tails
        .iter()
        .map(|r| {
            let mean = r.iter().sum::<f64>() / n;
            let sd = (r.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();
            (mean, sd)
        })
        .collect();
    let mut total = 0.0;
    let mut pairs = Vec::new();
    for i in 0..tails.len() {
        for j in (i + 1)..tails.len() {
            let (mi, si) = stats[i];
            let (mj, sj) = stats[j];
            if si <= 0.0 || sj <= 0.0 {
                continue;
            }
            let cov = tails[i]
                .iter()
                .zip(tails[j].iter())
                .map(|(a, b)| (a - mi) * (b - mj))
                .sum::<f64>()
                / (n - 1.0);
            let correlation = cov / (si * sj);
            total += correlation;
            pairs.push(PairCorrelation {
                a: series[i].0.clone(),
                b: series[j].0.clone(),
                correlation,
            });
        }
    }
    if pairs.is_empty() {
        return (None, pairs);
    }
    let mean = total / pairs.len() as f64;
    pairs.sort_by(|x, y| {
        y.correlation
            .abs()
            .total_cmp(&x.correlation.abs())
            .then_with(|| (&x.a, &x.b).cmp(&(&y.a, &y.b)))
    });
    pairs.truncate(MAX_PAIR_CORRELATIONS);
    (Some(mean), pairs)
}

/// Every world in `stress` with the book's loss in it, as the search reads
/// them: the library, the adversarial sequence, every causal stress and the
/// tail worlds.
fn worlds_of(stress: &StressReport, equity: f64) -> Vec<(String, f64)> {
    let mut worlds: BTreeMap<String, f64> = BTreeMap::new();
    let mut add = |name: &str, loss: f64| {
        let loss = if loss.is_nan() { f64::INFINITY } else { loss };
        let entry = worlds.entry(name.to_string()).or_insert(loss);
        *entry = entry.max(loss);
    };
    for result in &stress.scenarios {
        add(&result.scenario, result.loss_fraction);
    }
    for result in &stress.causal {
        add(&result.scenario, result.loss_fraction);
    }
    if let Some(sequence) = &stress.adversarial {
        add(
            qip_simulation_engine::scenario::ADVERSARIAL_SCENARIO_NAME,
            sequence.result.loss_fraction,
        );
    }
    for world in &stress.tail_worlds {
        add(&world.scenario, world.loss_fraction);
    }
    worlds
        .into_iter()
        .map(|(name, loss)| (name, -loss * equity))
        .collect()
}

/// What the Hedge Brain reads, assembled by the platform.
#[derive(Debug)]
pub struct HedgeBrainInputs<'a> {
    pub declarations: &'a [HedgeCandidateDeclaration],
    pub universe: &'a Universe,
    pub snapshot: &'a MarketSnapshot,
    pub state: &'a RiskState,
    pub stress: Option<&'a StressReport>,
    pub price_history: &'a BTreeMap<String, Vec<f64>>,
    pub equity: Decimal,
    /// The hedge capacity the survival waterfall withheld this construction.
    pub hedge_capacity: Decimal,
    /// Margin and unfunded commitments the waterfall withheld.
    pub funding: Decimal,
    /// The stress tolerance the survival kernel withholds against.
    pub tolerance: f64,
    pub cycle: u64,
    pub at: Timestamp,
}

/// What one cycle's Hedge Brain found.
#[derive(Clone, Debug, PartialEq)]
pub struct HedgeBrainReport {
    /// How many candidates the deployment declared.
    pub declared: usize,
    pub survey: ExposureSurvey,
    /// `None` when nothing was stressed or the inputs were refused.
    pub search: Option<HedgeSearch>,
    /// The search's plan as the §24.9 contract, validated.
    pub plan: Option<HedgePlan>,
    /// Candidates refused before the search, by the catalogue or the snapshot.
    pub unresolved: Vec<String>,
    /// Why no search ran, where one could not.
    pub refused: Option<String>,
}

impl HedgeBrainReport {
    /// The DECIDE line, never empty.
    pub fn summary(&self) -> String {
        let axes = format!(
            "the hedge brain read {} of {} axes",
            self.survey.measured().len(),
            ExposureSurvey::AXES.len()
        );
        if let Some(refused) = &self.refused {
            return format!("{axes} and searched nothing: {refused}");
        }
        match &self.search {
            None => format!("{axes}; the book was not stressed, so no world needs protecting"),
            Some(search) => format!("{axes}; {}", search.summary()),
        }
    }
}

/// The library's shocks by scenario name, as (factor, magnitude).
fn library_shocks() -> BTreeMap<String, Vec<(String, f64)>> {
    qip_simulation_engine::scenario::standard_library()
        .into_iter()
        .map(|scenario| {
            (
                scenario.name,
                scenario
                    .shocks
                    .into_iter()
                    .map(|shock| (shock.factor, shock.magnitude))
                    .collect(),
            )
        })
        .collect()
}

/// Run the Hedge Brain once. Deterministic: the same inputs choose the same
/// plan to the digit, and nothing here reads a clock or a model.
pub fn plan(inputs: &HedgeBrainInputs<'_>) -> HedgeBrainReport {
    let state = inputs.state;
    let axis = |name: &str| -> BTreeMap<String, Decimal> {
        state
            .axis_exposures
            .get(name)
            .map(|buckets| buckets.iter().map(|(k, v)| (k.clone(), *v)).collect())
            .unwrap_or_default()
    };
    let held: Vec<String> = state.position_notionals.keys().cloned().collect();
    let delta = state
        .position_notionals
        .values()
        .fold(Decimal::ZERO, |sum, notional| sum + *notional);
    let equity = inputs.equity.to_f64();
    let tail: BTreeMap<String, f64> = inputs
        .stress
        .map(|stress| {
            worlds_of(stress, 1.0)
                .into_iter()
                .map(|(name, pnl)| (name, -pnl))
                .collect()
        })
        .unwrap_or_default();
    let (gamma, vega) = book_option_greeks(&held, inputs.universe);
    let (correlation, pair_correlations) = correlations(&held, inputs.price_history);
    let survey = ExposureSurvey {
        drivers: axis(CAUSAL_DRIVER_AXIS),
        factors: axis(FACTOR_AXIS),
        greeks: BookGreeks { delta, gamma, vega },
        liquidity: state.days_to_liquidate.clone(),
        correlation,
        pair_correlations,
        funding: inputs.funding,
        counterparty: axis(COUNTERPARTY_AXIS),
        tail,
    };
    let mut report = HedgeBrainReport {
        declared: inputs.declarations.len(),
        survey,
        search: None,
        plan: None,
        unresolved: Vec::new(),
        refused: None,
    };
    let Some(stress) = inputs.stress else {
        return report;
    };
    if !(equity.is_finite() && equity > 0.0) {
        report.refused = Some("the book has no positive equity to measure a floor against".into());
        return report;
    }
    let mut library = library_shocks();
    // The adversarial world is composed from the library's own factors and
    // carries its shocks, so a candidate's move in it is computable. Leaving
    // it out made every candidate `DoesNotProtect` it, and since the search
    // must protect every failing world, a book whose adversarial sequence
    // breached could never be planned a hedge at all.
    if let Some(sequence) = &stress.adversarial {
        library.insert(
            sequence.scenario.name.clone(),
            sequence
                .scenario
                .shocks
                .iter()
                .map(|shock| (shock.factor.clone(), shock.magnitude))
                .collect(),
        );
    }
    let mut candidates = Vec::new();
    for declaration in inputs.declarations {
        match declaration.resolve(inputs.universe, inputs.snapshot, &library) {
            Ok(candidate) => candidates.push(candidate),
            Err(error) => report.unresolved.push(error.message().to_string()),
        }
    }
    let worlds: Vec<ForecastWorld> = worlds_of(stress, equity)
        .into_iter()
        .map(|(name, book_pnl)| ForecastWorld {
            name,
            probability: 1.0,
            book_pnl,
        })
        .collect();
    let objective = HedgeObjective {
        survival_floor: inputs.tolerance * equity,
        budget: inputs.hedge_capacity,
        max_volume_share: MAX_VOLUME_SHARE,
        max_unwind_days: MAX_UNWIND_DAYS,
        holding_days: HOLDING_DAYS,
    };
    let search = match qip_risk::hedge_search::search(&worlds, &candidates, &objective) {
        Ok(search) => search,
        Err(error) => {
            report.refused = Some(error.message().to_string());
            return report;
        }
    };
    if let Some(solution) = &search.plan {
        match contract(solution, &search, inputs) {
            Ok(plan) => report.plan = Some(plan),
            Err(error) => {
                report.refused = Some(format!(
                    "the search chose a plan the HedgePlan contract refuses: {}",
                    error.message()
                ));
            }
        }
    }
    report.search = Some(search);
    report
}

/// The chosen plan as the §24.9 contract, so a reader of the log holds the
/// whole record — exposures, protected scenarios, instruments, cost,
/// liquidity and the way back out — and not a fragment of it.
fn contract(
    solution: &qip_risk::hedge_search::HedgeSolution,
    search: &HedgeSearch,
    inputs: &HedgeBrainInputs<'_>,
) -> Result<HedgePlan> {
    let equity = inputs.equity.to_f64();
    let exposures = search
        .protected
        .iter()
        .filter_map(|world| {
            let loss = inputs
                .stress
                .and_then(|stress| {
                    worlds_of(stress, equity)
                        .into_iter()
                        .find(|(n, _)| n == world)
                })
                .map(|(_, pnl)| pnl)?;
            // The money crossing for the contract's exposure: the world's
            // book P&L is an `f64` statistic, the contract carries money.
            Decimal::from_f64(loss)
                .filter(|amount| !amount.is_zero())
                .map(|amount| PlanExposure {
                    risk_factor: format!("world:{world}"),
                    amount,
                })
        })
        .collect();
    let days = solution.measures.liquidity_decay_days;
    let time_to_exit = Duration::from_secs((days * 86_400.0).ceil().max(1.0) as i64);
    HedgePlan::new(HedgePlanFields {
        schema_version: HedgePlanFields::SCHEMA_VERSION,
        plan_id: format!("hedge-plan-{}", inputs.cycle),
        exposures,
        scenarios_protected: search.protected.clone(),
        instruments: solution
            .legs
            .iter()
            .map(|leg| PlanInstrument {
                instrument: leg.object_id.as_str().to_string(),
                venue: VenueId::new(leg.venue.clone()),
                side: match leg.side {
                    LegSide::Buy => PlanSide::Buy,
                    LegSide::Sell => PlanSide::Sell,
                },
                quantity: leg.quantity,
            })
            .collect(),
        cost: solution.measures.cost,
        liquidity: HedgeLiquidity {
            max_adv_share_bp: (MAX_VOLUME_SHARE * 10_000.0).round() as u32,
            time_to_exit,
        },
        rollback: HedgeRollback {
            trigger: format!(
                "every protected world's unhedged loss is back inside the {:.0}% survival floor",
                inputs.tolerance * 100.0
            ),
            steps: solution
                .legs
                .iter()
                .map(|leg| {
                    format!(
                        "{} {} {} at {}",
                        match leg.side {
                            LegSide::Buy => "sell",
                            LegSide::Sell => "buy back",
                        },
                        leg.quantity,
                        leg.symbol,
                        leg.venue
                    )
                })
                .collect(),
        },
    })
}

/// The journal record of one Hedge Brain run.
///
/// Filed under [`Topic::RiskEvaluated`], as `HedgeSurveyed` is: a plan is a
/// risk evaluation, and nothing refuses or submits an order on it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HedgePlanned {
    pub cycle: u64,
    pub at: Timestamp,
    pub summary: String,
    pub survey: ExposureSurvey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<HedgePlan>,
    #[serde(default)]
    pub protected: Vec<String>,
    #[serde(default)]
    pub refusals: Vec<SearchRefusal>,
    #[serde(default)]
    pub unresolved: Vec<String>,
    /// The six §14.1 measures of the chosen plan (R12-RISK-4109) — cost,
    /// basis risk, convexity, liquidity decay, counterparty risk and unwind
    /// feasibility — as the search computed them. The §24.9 contract carries
    /// only cost and liquidity, so without this a reader of the log could
    /// not check the other four the choice was made on. `None` when no plan
    /// was chosen; omitted then, so a record without one reads as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measures: Option<qip_risk::hedge_search::HedgeMeasures>,
}

impl HedgePlanned {
    /// The record for a report, or `None` when nothing needed protecting and
    /// nothing was refused — a record saying "nothing" every cycle is one
    /// nobody reads, and the stage line says it.
    pub fn of(report: &HedgeBrainReport, cycle: u64, at: Timestamp) -> Option<Self> {
        let protected = report
            .search
            .as_ref()
            .map(|search| search.protected.clone())
            .unwrap_or_default();
        let refusals = report
            .search
            .as_ref()
            .map(|search| search.refusals.clone())
            .unwrap_or_default();
        // A failing world with no candidate declared at all is said on the
        // DECIDE line every cycle and journalled never: it is the standing
        // configuration, not this cycle's finding, and a record repeating it
        // each cycle is one nobody reads. Once a candidate is declared, a
        // failing world is a finding whether or not anything protects it.
        let finding = (report.declared > 0 && !protected.is_empty())
            || report.refused.is_some()
            || !report.unresolved.is_empty();
        finding.then(|| Self {
            cycle,
            at,
            summary: report.summary(),
            survey: report.survey.clone(),
            plan: report.plan.clone(),
            protected,
            refusals,
            unresolved: report.unresolved.clone(),
            measures: report
                .search
                .as_ref()
                .and_then(|search| search.plan.as_ref())
                .map(|solution| solution.measures.clone()),
        })
    }
}

impl EventBody for HedgePlanned {
    const TOPIC: Topic = Topic::RiskEvaluated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("hedge-plan:{}", self.cycle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tail::TailWorld;
    use qip_core::dec;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn stress(worlds: &[(&str, f64)]) -> StressReport {
        StressReport {
            at: at(),
            scenarios: Vec::new(),
            modelled_positions: 0,
            unmodelled_positions: 0,
            decomposition: None,
            adversarial: None,
            causal: Vec::new(),
            causal_drivers: 0,
            causal_drivers_unconnected: 0,
            episodes: Vec::new(),
            tail_worlds: worlds
                .iter()
                .map(|(name, loss)| TailWorld {
                    scenario: (*name).to_string(),
                    at: at(),
                    loss_fraction: *loss,
                    loss: loss * 1_000_000.0,
                    unmeasured: Vec::new(),
                    detail: String::new(),
                })
                .collect(),
            advisory_worlds: Vec::new(),
            advisory_reserve: None,
        }
    }

    fn state() -> RiskState {
        let mut state = RiskState {
            equity: dec!("1000000"),
            ..RiskState::default()
        };
        state
            .position_notionals
            .insert("obj-AAA".to_string(), dec!("800000"));
        state
            .position_notionals
            .insert("obj-BBB".to_string(), dec!("-200000"));
        state.days_to_liquidate.insert("obj-AAA".to_string(), 2.0);
        state
    }

    /// R12-RISK-4105: the survey reads every axis §14.1 lists, and names the
    /// two Greeks it cannot measure rather than reporting them flat.
    /// Mutation: drop the `correlation` assignment (set `None`) and the
    /// measured list shrinks.
    #[test]
    fn the_survey_reads_every_listed_axis_and_names_what_it_cannot_measure() {
        let history = BTreeMap::from([
            (
                "obj-AAA".to_string(),
                vec![100.0, 101.0, 99.0, 102.0, 103.0],
            ),
            ("obj-BBB".to_string(), vec![50.0, 50.4, 49.6, 51.0, 51.2]),
        ]);
        let stress = stress(&[("venue-outage", 0.12)]);
        let report = plan(&HedgeBrainInputs {
            declarations: &[],
            universe: &Universe::new(),
            snapshot: &MarketSnapshot::new(at()),
            state: &state(),
            stress: Some(&stress),
            price_history: &history,
            equity: dec!("1000000"),
            hedge_capacity: dec!("50000"),
            funding: dec!("25000"),
            tolerance: 0.20,
            cycle: 7,
            at: at(),
        });
        assert_eq!(report.survey.measured().len(), ExposureSurvey::AXES.len());
        assert_eq!(report.survey.greeks.delta, dec!("600000"));
        assert!(report.survey.correlation.is_some());
        assert_eq!(report.survey.unmeasured(), vec!["gamma", "vega"]);
        assert_eq!(report.survey.tail.get("venue-outage"), Some(&0.12));
        assert_eq!(report.survey.funding, dec!("25000"));
        // The book survives the one world at the 20% floor: nothing to plan.
        assert!(
            report
                .search
                .as_ref()
                .is_some_and(|s| s.protected.is_empty())
        );
        assert!(report.plan.is_none());
    }

    fn catalogued(symbols: &[(&str, qip_financial::asset_class::InstrumentType)]) -> Universe {
        use qip_financial::asset_class::Sector;
        use qip_financial::costs::LiquidityProfile;
        use qip_financial::quality::Provenance;
        let mut universe = Universe::new();
        for (symbol, kind) in symbols {
            universe
                .insert(
                    qip_financial::object::FinancialObject::builder(
                        ObjectId::from_string(format!("obj-{symbol}")),
                        *symbol,
                        *kind,
                        LiquidityProfile::listed(Decimal::from_int(1_000_000), 5.0),
                    )
                    .venue("XNAS")
                    .geography("US")
                    .sector(Sector::Financials)
                    .provenance(Provenance::synthetic("catalogue", at()))
                    .build(at())
                    .expect("a record"),
                )
                .expect("a catalogue entry");
        }
        universe
    }

    fn survey_over(universe: &Universe, history: &BTreeMap<String, Vec<f64>>) -> ExposureSurvey {
        let stress = stress(&[("venue-outage", 0.12)]);
        plan(&HedgeBrainInputs {
            declarations: &[],
            universe,
            snapshot: &MarketSnapshot::new(at()),
            state: &state(),
            stress: Some(&stress),
            price_history: history,
            equity: dec!("1000000"),
            hedge_capacity: dec!("50000"),
            funding: dec!("25000"),
            tolerance: 0.20,
            cycle: 7,
            at: at(),
        })
        .survey
    }

    /// R12-RISK-4105: a book held entirely in linear instruments has a
    /// measured zero gamma and vega; one holding anything the catalogue gives
    /// no sensitivities for, or an instrument the catalogue does not know,
    /// has neither, so a flat reading is never offered for convexity nobody
    /// measured. Mutations: report `None` for a linear book (the first
    /// assertion fails); admit `Option` in `is_linear` (the second fails);
    /// skip the catalogue lookup (the unknown-instrument assertion fails).
    #[test]
    fn a_linear_book_has_a_measured_zero_gamma_and_vega_and_anything_else_has_none() {
        use qip_financial::asset_class::InstrumentType;
        let history = BTreeMap::new();
        let linear = catalogued(&[
            ("AAA", InstrumentType::CommonStock),
            ("BBB", InstrumentType::ExchangeTradedFund),
        ]);
        let survey = survey_over(&linear, &history);
        assert_eq!(survey.greeks.gamma, Some(0.0));
        assert_eq!(survey.greeks.vega, Some(0.0));
        assert!(
            survey
                .unmeasured()
                .iter()
                .all(|g| *g != "gamma" && *g != "vega")
        );

        for convex in [
            InstrumentType::Option,
            InstrumentType::VarianceSwap,
            InstrumentType::VolatilityIndexFuture,
            InstrumentType::ConvertibleBond,
            InstrumentType::HedgeFund,
        ] {
            assert!(!is_linear(convex), "{convex:?} is not known to be linear");
        }
        assert!(is_linear(InstrumentType::CommonStock), "premise");

        // A catalogued hedge-fund holding is something the catalogue gives no
        // sensitivities for: known to it and still not known to be linear.
        let swap = catalogued(&[
            ("AAA", InstrumentType::CommonStock),
            ("BBB", InstrumentType::HedgeFund),
        ]);
        let survey = survey_over(&swap, &history);
        assert_eq!(
            survey.greeks.gamma, None,
            "a hedge-fund holding is not known to be linear"
        );
        assert_eq!(survey.greeks.vega, None);

        let unknown = catalogued(&[("AAA", InstrumentType::CommonStock)]);
        let survey = survey_over(&unknown, &history);
        assert_eq!(survey.greeks.gamma, None, "obj-BBB is not in the catalogue");
        assert_eq!(survey.greeks.vega, None);
        assert_eq!(survey.unmeasured()[..2], ["gamma", "vega"]);
    }

    /// R12-RISK-4105: the survey names which pair is correlated, not only the
    /// mean. Three held names, two moving together and one against them: the
    /// mean sits between the pairs and names no one, the pair list puts the
    /// co-moving pair first at +1 and the opposed pairs at -1. Mutations:
    /// leave `pair_correlations` empty (the premise fails); sort ascending
    /// (the strongest pair is no longer first).
    #[test]
    fn the_survey_reports_each_correlated_pair_and_not_only_their_mean() {
        let up = |scale: f64| -> Vec<f64> {
            [100.0, 101.0, 99.0, 102.0, 100.0, 103.0]
                .iter()
                .map(|p| p * scale)
                .collect()
        };
        let down: Vec<f64> = [100.0, 99.0, 101.0, 98.0, 100.0, 97.0].to_vec();
        let history = BTreeMap::from([
            ("obj-AAA".to_string(), up(1.0)),
            ("obj-BBB".to_string(), up(0.5)),
            ("obj-CCC".to_string(), down),
        ]);
        let mut book = state();
        book.position_notionals
            .insert("obj-CCC".to_string(), dec!("100000"));
        let stress = stress(&[("venue-outage", 0.12)]);
        let survey = plan(&HedgeBrainInputs {
            declarations: &[],
            universe: &Universe::new(),
            snapshot: &MarketSnapshot::new(at()),
            state: &book,
            stress: Some(&stress),
            price_history: &history,
            equity: dec!("1000000"),
            hedge_capacity: dec!("50000"),
            funding: dec!("25000"),
            tolerance: 0.20,
            cycle: 7,
            at: at(),
        })
        .survey;
        assert_eq!(survey.pair_correlations.len(), 3, "premise: three pairs");
        let first = &survey.pair_correlations[0];
        assert_eq!((first.a.as_str(), first.b.as_str()), ("obj-AAA", "obj-BBB"));
        assert!((first.correlation - 1.0).abs() < 1e-9, "{first:?}");
        assert!(
            survey.pair_correlations[1..]
                .iter()
                .all(|pair| pair.correlation < -0.5),
            "{:?}",
            survey.pair_correlations
        );
        let mean = survey.correlation.expect("a mean over three pairs");
        assert!(mean.abs() < 0.5, "the mean {mean} must hide the pairs");
    }

    fn hedge_record() -> qip_financial::object::FinancialObject {
        use qip_financial::asset_class::{InstrumentType, Sector};
        use qip_financial::costs::LiquidityProfile;
        use qip_financial::quality::Provenance;
        qip_financial::object::FinancialObject::builder(
            ObjectId::from_string("obj-HDG"),
            "HDG",
            InstrumentType::CommonStock,
            LiquidityProfile::listed(Decimal::from_int(1_000_000), 5.0),
        )
        .venue("XNAS")
        .geography("US")
        .sector(Sector::Financials)
        .provenance(Provenance::synthetic("catalogue", at()))
        .build(at())
        .expect("a listed record")
    }

    fn priced() -> MarketSnapshot {
        use qip_market::quote::{Trade, TradeCondition};
        let mut snapshot = MarketSnapshot::new(at());
        snapshot.apply_trade(Trade {
            object_id: ObjectId::from_string("obj-HDG"),
            venue: "XNAS".into(),
            at: at(),
            capture_time: at(),
            sequence: None,
            price: dec!("100"),
            size: Decimal::from_int(100),
            aggressor: None,
            condition: TradeCondition::Regular,
            trade_id: None,
            quality: Default::default(),
        });
        snapshot
    }

    fn declaration() -> HedgeCandidateDeclaration {
        HedgeCandidateDeclaration {
            instrument: ObjectId::from_string("obj-HDG"),
            venue: "SIM-XNAS".to_string(),
            class: InstrumentClass::Future,
            greeks: Greeks::linear(),
            carry_bps_per_day: 0.5,
            spread_bps: 2.0,
            average_daily_volume: Decimal::from_int(1_000_000),
            counterparty_loss_rate: 0.0,
            basis_volatility: 0.004,
            factor_loadings: BTreeMap::from([("equity".to_string(), 1.0)]),
        }
    }

    /// R12-RISK-4106/4108/4109 through the kernel: a book that fails the
    /// library's 1987 world is planned a hedge whose size comes from that
    /// world's own equity shock — not a declared beta — inside the hedge
    /// capacity, and the plan reaches the log as a validated §24.9
    /// `HedgePlan`. Mutation: resolve the candidate with no `moves` (the
    /// factor loop skipped) and the search refuses it as `DoesNotProtect`.
    #[test]
    fn a_book_failing_a_library_world_is_planned_a_validated_hedge_sized_by_that_world() {
        let mut universe = Universe::new();
        universe.insert(hedge_record()).expect("insertable");
        let stress = stress(&[("equity-crash-1987", 0.35)]);
        let report = plan(&HedgeBrainInputs {
            declarations: &[declaration()],
            universe: &universe,
            snapshot: &priced(),
            state: &state(),
            stress: Some(&stress),
            price_history: &BTreeMap::new(),
            equity: dec!("1000000"),
            hedge_capacity: dec!("100000"),
            funding: Decimal::ZERO,
            tolerance: 0.20,
            cycle: 11,
            at: at(),
        });
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        assert!(report.refused.is_none(), "{:?}", report.refused);
        let plan = report.plan.as_ref().expect("a plan");
        assert_eq!(
            plan.scenarios_protected,
            vec!["equity-crash-1987".to_string()]
        );
        assert_eq!(plan.instruments.len(), 1);
        assert_eq!(plan.instruments[0].side, PlanSide::Sell);
        // 150,000 to recover at 100 x 0.22 a unit: 6,819 units.
        assert_eq!(plan.instruments[0].quantity, dec!("6819"));
        assert!(plan.cost.is_positive());
        let record = HedgePlanned::of(&report, 11, at()).expect("journalled");
        assert!(record.plan.is_some());

        // The same book with no hedge capacity withheld plans nothing: the
        // budget is the survival waterfall's, not the search's own.
        let starved = super::plan(&HedgeBrainInputs {
            declarations: &[declaration()],
            universe: &universe,
            snapshot: &priced(),
            state: &state(),
            stress: Some(&stress),
            price_history: &BTreeMap::new(),
            equity: dec!("1000000"),
            hedge_capacity: Decimal::ZERO,
            funding: Decimal::ZERO,
            tolerance: 0.20,
            cycle: 11,
            at: at(),
        });
        assert!(starved.plan.is_none());
    }

    /// A world the book does not survive and no candidate to protect it: the
    /// DECIDE line says so rather than reading as a hedged book, and the
    /// standing absence of candidates is not journalled every cycle; a
    /// declared candidate the catalogue cannot resolve is. Mutation: drop the
    /// `report.declared > 0` guard and the first `is_none` fails.
    #[test]
    fn a_failing_world_with_no_candidate_is_a_recorded_finding_not_silence() {
        let stress = stress(&[("gap-move", 0.35)]);
        let report = plan(&HedgeBrainInputs {
            declarations: &[],
            universe: &Universe::new(),
            snapshot: &MarketSnapshot::new(at()),
            state: &state(),
            stress: Some(&stress),
            price_history: &BTreeMap::new(),
            equity: dec!("1000000"),
            hedge_capacity: dec!("50000"),
            funding: Decimal::ZERO,
            tolerance: 0.20,
            cycle: 3,
            at: at(),
        });
        let search = report.search.as_ref().expect("searched");
        assert_eq!(search.protected, vec!["gap-move".to_string()]);
        assert!(report.plan.is_none());
        assert!(report.summary().contains("no candidate or pair protects"));
        assert!(
            HedgePlanned::of(&report, 3, at()).is_none(),
            "an undeclared hedge universe was journalled as this cycle's finding"
        );

        let unresolvable = plan(&HedgeBrainInputs {
            declarations: &[declaration()],
            universe: &Universe::new(),
            snapshot: &MarketSnapshot::new(at()),
            state: &state(),
            stress: Some(&stress),
            price_history: &BTreeMap::new(),
            equity: dec!("1000000"),
            hedge_capacity: dec!("50000"),
            funding: Decimal::ZERO,
            tolerance: 0.20,
            cycle: 3,
            at: at(),
        });
        assert_eq!(
            unresolvable.unresolved.len(),
            1,
            "premise: not in the catalogue"
        );
        let record = HedgePlanned::of(&unresolvable, 3, at()).expect("a finding is journalled");
        assert_eq!(record.protected, vec!["gap-move".to_string()]);
    }
}
