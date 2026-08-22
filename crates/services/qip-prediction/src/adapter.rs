//! Prediction venue adapter ports.
//!
//! The same pull contract the rest of the platform uses: `poll(until)`, with
//! the caller holding the clock. The synthetic venue is deterministic given
//! its seed, including when it prices a complete set below its payoff, so the
//! arbitrage detector can be tested against a market that genuinely contains
//! one rather than against a hand-written book that happens to.
//!
//! No prediction venue is reachable from this build. [`VenueApiAdapter`] is
//! declared so the shape of a real integration is reviewed, and every call
//! returns [`qip_core::Error::Unavailable`] naming the endpoints and the
//! credential a deployment must supply.

use qip_contracts::{VenueClass, VenueId};
use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook};
use serde::{Deserialize, Serialize};

use crate::market::{EventMarket, FeeSchedule, MarketKind, Outcome, OutcomeId};
use crate::oracle::OracleReport;
use crate::resolution::{
    Comparison, Proposition, ResolutionCriteria, ResolutionSource, SettlementRule, SourceKind,
    UndeterminedRule,
};

/// What an adapter is and what a deployment must supply to make it real.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PredictionSource {
    pub name: String,
    pub venue: VenueId,
    pub class: VenueClass,
    pub expected_latency: Duration,
    pub is_synthetic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production_requirement: Option<String>,
}

/// One thing a prediction venue published.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PredictionUpdate {
    /// A market exists, with its resolution criteria attached. Criteria arrive
    /// with the market rather than being looked up later: a book without the
    /// question it settles on is not tradable information.
    MarketListed(Box<EventMarket>),
    /// Depth for one outcome.
    Book {
        market_id: ObjectId,
        outcome: OutcomeId,
        book: Box<OrderBook>,
    },
    /// The oracle said something.
    Report {
        market_id: ObjectId,
        report: OracleReport,
    },
}

impl PredictionUpdate {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::MarketListed(_) => "market_listed",
            Self::Book { .. } => "book",
            Self::Report { .. } => "report",
        }
    }
}

/// The common prediction adapter contract.
pub trait PredictionAdapter: std::fmt::Debug {
    fn descriptor(&self) -> PredictionSource;

    /// Everything published up to and including `until`.
    fn poll(&mut self, until: Timestamp) -> Result<Vec<PredictionUpdate>>;

    fn start(&mut self, _at: Timestamp) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}

/// How the synthetic venue behaves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyntheticVenueConfig {
    pub venue: VenueId,
    pub seed: u64,
    pub step: Duration,
    /// Half-spread applied around each outcome's fair price.
    pub half_spread: Decimal,
    /// Probability that a step prices the complete set below its payoff.
    pub arbitrage_probability: f64,
    /// How far below the payoff such a step prices the set.
    pub arbitrage_depth: Decimal,
    pub levels: u32,
    pub fees: FeeSchedule,
}

impl SyntheticVenueConfig {
    pub fn demo(seed: u64) -> Result<Self> {
        Ok(Self {
            venue: VenueId::new("SYNTH-PREDICT"),
            seed,
            step: Duration::from_mins(5),
            half_spread: Decimal::parse("0.005").unwrap_or(Decimal::ZERO),
            arbitrage_probability: 0.2,
            arbitrage_depth: Decimal::parse("0.03").unwrap_or(Decimal::ZERO),
            levels: 3,
            fees: FeeSchedule::new(50, 0, 100)?,
        })
    }
}

/// A three-way categorical market on a policy decision, for tests and demos.
///
/// The criteria are structured: a program decides which outcome won by reading
/// one published number, and the three ranges partition it.
pub fn demo_market(venue: VenueId, resolves_at: Timestamp, fees: FeeSchedule) -> Result<EventMarket> {
    let source = ResolutionSource::new(
        "central-bank-statistical-release",
        SourceKind::Official,
        vec!["policy_rate_change_bp".to_string()],
    );
    let outcomes = vec![
        Outcome::new(
            OutcomeId::new("cut"),
            "a cut of at least 25bp",
            ObjectId::from_string("SYNTH-PREDICT-CUT"),
            ResolutionCriteria::Threshold {
                metric: "policy_rate_change_bp".to_string(),
                comparison: Comparison::AtMost,
                value: Decimal::from_int(-25),
            },
        ),
        Outcome::new(
            OutcomeId::new("hold"),
            "no change",
            ObjectId::from_string("SYNTH-PREDICT-HOLD"),
            ResolutionCriteria::Within {
                metric: "policy_rate_change_bp".to_string(),
                lower: Some(Decimal::from_int(-24)),
                upper: Some(Decimal::from_int(25)),
            },
        ),
        Outcome::new(
            OutcomeId::new("hike"),
            "a hike of at least 25bp",
            ObjectId::from_string("SYNTH-PREDICT-HIKE"),
            ResolutionCriteria::Threshold {
                metric: "policy_rate_change_bp".to_string(),
                comparison: Comparison::AtLeast,
                value: Decimal::from_int(25),
            },
        ),
    ];
    let proposition = Proposition::new(
        "the policy rate decision at the next meeting",
        ResolutionCriteria::Any(outcomes.iter().map(|o| o.criteria.clone()).collect()),
        source,
        resolves_at,
        SettlementRule::unit(UndeterminedRule::VoidAndRefund),
        Duration::from_hours(24),
    )?;
    EventMarket::new(
        ObjectId::from_string("SYNTH-PREDICT-MARKET"),
        venue,
        VenueClass::PredictionMarket,
        proposition,
        MarketKind::categorical(outcomes)?,
        fees,
    )
}

/// A deterministic prediction venue.
#[derive(Debug)]
pub struct SyntheticPredictionVenue {
    config: SyntheticVenueConfig,
    market: EventMarket,
    rng: Xoshiro256,
    next_step_at: Timestamp,
    listed: bool,
    reported: bool,
}

impl SyntheticPredictionVenue {
    pub fn new(config: SyntheticVenueConfig, start: Timestamp) -> Result<Self> {
        let market = demo_market(
            config.venue.clone(),
            start.saturating_add(Duration::from_days(30)),
            config.fees,
        )?;
        Ok(Self {
            rng: Xoshiro256::seeded(config.seed),
            config,
            market,
            next_step_at: start,
            listed: false,
            reported: false,
        })
    }

    pub const fn market(&self) -> &EventMarket {
        &self.market
    }

    /// Fair probabilities for one step, normalised to sum to one.
    fn fair_prices(&mut self) -> Vec<Decimal> {
        let count = self.market.outcomes().len();
        let weights: Vec<f64> = (0..count).map(|_| self.rng.uniform(0.2, 1.0)).collect();
        let total: f64 = weights.iter().sum();
        weights
            .iter()
            .map(|weight| {
                Decimal::from_f64(weight / total)
                    .unwrap_or(Decimal::ZERO)
                    .round_dp(3)
            })
            .collect()
    }

    fn books_for(&mut self, at: Timestamp) -> Vec<PredictionUpdate> {
        let prices = self.fair_prices();
        // Occasionally the offers price the whole set below its payoff, which
        // is the state the arbitrage detector exists to find.
        let discount = if self.rng.bernoulli(self.config.arbitrage_probability) {
            self.config.arbitrage_depth
        } else {
            Decimal::ZERO
        };
        let outcomes: Vec<(OutcomeId, ObjectId)> = self
            .market
            .outcomes()
            .iter()
            .map(|outcome| (outcome.id.clone(), outcome.object_id.clone()))
            .collect();

        let mut updates = Vec::with_capacity(outcomes.len());
        for (position, (outcome, object_id)) in outcomes.into_iter().enumerate() {
            let fair = prices.get(position).copied().unwrap_or(Decimal::ZERO);
            let per_leg = discount
                .checked_div(Decimal::from_int(prices.len().max(1) as i64))
                .unwrap_or(Decimal::ZERO);
            let ask_touch = (fair + self.config.half_spread - per_leg)
                .max(Decimal::parse("0.01").unwrap_or(Decimal::ZERO));
            let bid_touch = (fair - self.config.half_spread - per_leg)
                .max(Decimal::parse("0.005").unwrap_or(Decimal::ZERO));

            let mut bids = Vec::new();
            let mut asks = Vec::new();
            let tick = Decimal::parse("0.002").unwrap_or(Decimal::ZERO);
            for level in 0..self.config.levels {
                let step = tick * Decimal::from_int(i64::from(level));
                let size = Decimal::from_int(10 + self.rng.below(40) as i64);
                asks.push(BookLevel::new(ask_touch + step, size));
                let bid_price = bid_touch - step;
                if bid_price.is_positive() {
                    bids.push(BookLevel::new(bid_price, size));
                }
            }
            updates.push(PredictionUpdate::Book {
                market_id: self.market.market_id.clone(),
                outcome,
                book: Box::new(OrderBook::from_levels(
                    object_id,
                    self.config.venue.as_str(),
                    at,
                    bids,
                    asks,
                )),
            });
        }
        updates
    }
}

impl PredictionAdapter for SyntheticPredictionVenue {
    fn descriptor(&self) -> PredictionSource {
        PredictionSource {
            name: "synthetic-prediction-venue".to_string(),
            venue: self.config.venue.clone(),
            class: VenueClass::PredictionMarket,
            expected_latency: Duration::from_millis(250),
            is_synthetic: true,
            production_requirement: Some(
                "a venue API; see VenueApiAdapter for the exact endpoints".to_string(),
            ),
        }
    }

    fn poll(&mut self, until: Timestamp) -> Result<Vec<PredictionUpdate>> {
        let mut updates = Vec::new();
        if !self.listed {
            self.listed = true;
            updates.push(PredictionUpdate::MarketListed(Box::new(self.market.clone())));
        }
        while self.next_step_at <= until {
            let at = self.next_step_at;
            updates.extend(self.books_for(at));
            self.next_step_at = self.next_step_at.saturating_add(self.config.step);
        }
        if !self.reported && until >= self.market.proposition.resolves_at {
            self.reported = true;
            let outcomes = self.market.outcomes();
            let chosen = self.rng.below(outcomes.len() as u64) as usize;
            updates.push(PredictionUpdate::Report {
                market_id: self.market.market_id.clone(),
                report: OracleReport {
                    outcome: outcomes[chosen.min(outcomes.len() - 1)].id.clone(),
                    confidence: 0.97,
                    reported_at: self.market.proposition.resolves_at,
                    evidence: "the published policy rate change".to_string(),
                },
            });
        }
        Ok(updates)
    }
}

/// What a real venue integration needs before it can do anything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueApiConfig {
    pub venue: VenueId,
    pub endpoint_env: String,
    pub credential_env: String,
    /// Endpoints the adapter calls, named so an operator can check entitlements
    /// before deploying rather than after.
    pub required_endpoints: Vec<String>,
}

impl VenueApiConfig {
    /// The endpoints this crate's model needs from a prediction venue.
    pub fn standard(venue: VenueId) -> Self {
        Self {
            venue,
            endpoint_env: "QIP_PREDICTION_API_ENDPOINT".to_string(),
            credential_env: "QIP_PREDICTION_API_CREDENTIAL".to_string(),
            required_endpoints: vec![
                "GET /markets".to_string(),
                "GET /markets/{id}/resolution-criteria".to_string(),
                "GET /markets/{id}/orderbook".to_string(),
                "GET /markets/{id}/oracle-reports".to_string(),
                "GET /markets/{id}/disputes".to_string(),
            ],
        }
    }
}

/// The real venue adapter, declared and unavailable.
#[derive(Clone, Debug)]
pub struct VenueApiAdapter {
    config: VenueApiConfig,
    endpoint_present: bool,
    credential_present: bool,
}

impl VenueApiAdapter {
    /// Presence flags come from the composition root, the only layer that
    /// reads the environment.
    pub fn new(config: VenueApiConfig, endpoint_present: bool, credential_present: bool) -> Self {
        Self {
            config,
            endpoint_present,
            credential_present,
        }
    }

    pub const fn config(&self) -> &VenueApiConfig {
        &self.config
    }

    pub const fn is_available(&self) -> bool {
        self.endpoint_present && self.credential_present
    }

    /// Exactly what is missing, named so an operator can act on it.
    pub fn requirement(&self) -> String {
        let mut missing = Vec::new();
        if !self.endpoint_present {
            missing.push(format!(
                "a venue API base URL in the environment variable {}",
                self.config.endpoint_env
            ));
        }
        if !self.credential_present {
            missing.push(format!(
                "a venue API credential in the environment variable {}",
                self.config.credential_env
            ));
        }
        format!(
            "venue {} needs {} serving the endpoints {}",
            self.config.venue,
            if missing.is_empty() {
                "a transport implementation".to_string()
            } else {
                missing.join(" and ")
            },
            self.config.required_endpoints.join(", ")
        )
    }
}

impl PredictionAdapter for VenueApiAdapter {
    fn descriptor(&self) -> PredictionSource {
        PredictionSource {
            name: "venue-api".to_string(),
            venue: self.config.venue.clone(),
            class: VenueClass::PredictionMarket,
            expected_latency: Duration::from_millis(500),
            is_synthetic: false,
            production_requirement: Some(self.requirement()),
        }
    }

    fn poll(&mut self, _until: Timestamp) -> Result<Vec<PredictionUpdate>> {
        Err(Error::unavailable(self.requirement()))
    }

    fn start(&mut self, _at: Timestamp) -> Result<()> {
        Err(Error::unavailable(self.requirement()))
    }
}
