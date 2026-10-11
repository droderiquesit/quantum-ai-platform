//! The Market Factory in the cycle (R12-EXEC-5080, -5081; R12-COVERAGE-5107).
//!
//! `qip_execution_engine::market_creation::MarketFactory` creates, lists and
//! seeds a market on the simulated venue and withdraws one that stops being
//! valid. This module is where a deployment's plan reaches it: the plan's
//! permissions are recorded into the three-authority register, each planned
//! market is taken through create, list and seed under its own admission at
//! installation, and the validity monitor runs every SIMULATE stage against
//! the platform's own latest marks. Every step the factory takes is journaled
//! to the event log as a [`MarketFactoryRecorded`] record, drained from the
//! factory in the same call that took it.
//!
//! # Paper only
//!
//! The factory reaches the venue through the desk broker's
//! `Broker::listing_venue`, which only the simulated broker answers; a
//! platform composed over any other broker installs no factory and says so.
//! The factory itself refuses an admission for any venue but the simulated
//! one it is bound to.
//!
//! # Marks
//!
//! A seeded market is marked by the latest observed price of the instrument
//! its plan names as its `mark_source` — the platform's own price history,
//! which is an `f64` statistic, crossed into `Decimal` here and only here,
//! rounded to the venue's nine places. A market whose source has never been
//! observed is unmarked, and the monitor withdraws an unmarked market rather
//! than leave it quoting prices nobody checked.

use std::collections::BTreeMap;

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use qip_events::{EventBody, Topic};
use qip_execution_engine::broker::ListingVenue;
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::market_creation::{
    FactoryRecord, MarketFactory, PricingRule, ValidityFinding,
};
use serde::{Deserialize, Serialize};

/// One recorded permission, as a plan states it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionEntry {
    /// `venue`, `product` or `jurisdiction`.
    pub authority: String,
    /// `create`, `list` or `seed`.
    pub action: String,
    pub venue: String,
    pub product: String,
    pub jurisdiction: String,
    /// The rulebook entry, product approval or legal opinion it rests on.
    pub reference: String,
}

/// One market a plan asks the factory to bring into existence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketEntry {
    pub venue: String,
    pub product: String,
    pub jurisdiction: String,
    pub reference_price: Decimal,
    pub half_spread: Decimal,
    pub level_step: Decimal,
    pub level_size: Decimal,
    pub levels: u32,
    pub tolerance: Decimal,
    /// Inventory committed at seeding.
    pub inventory: Decimal,
    /// The instrument whose latest observed price marks this market.
    pub mark_source: String,
    /// Whether this market is an event contract (R12-EVENT-5087). An event
    /// market is created only where the event plane's access book records a
    /// creation permission for its venue and jurisdiction, on top of the
    /// three authorities every market needs; a plan asking for one without
    /// an event-markets section is refused.
    #[serde(default)]
    pub event_contract: bool,
}

/// The factory section of a deployment's breadth plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactoryPlan {
    pub permissions: Vec<PermissionEntry>,
    pub markets: Vec<MarketEntry>,
}

/// One factory step, as the event log holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketFactoryRecorded {
    pub record: FactoryRecord,
}

impl EventBody for MarketFactoryRecorded {
    /// A governed product decision with its approval references: only this
    /// platform has it, so it rides an irreplaceable topic.
    const TOPIC: Topic = Topic::ComplianceEvaluated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "market-factory:{}:{}",
            self.record.venue, self.record.sequence
        ))
    }
}

/// The factory, its permission register, and where each market is marked.
#[derive(Debug)]
pub struct FactoryStage {
    factory: MarketFactory,
    permissions: CreationPermissions,
    mark_sources: BTreeMap<String, String>,
}

fn authority(name: &str) -> Result<CreationAuthority> {
    CreationAuthority::ALL
        .into_iter()
        .find(|authority| authority.as_str() == name)
        .ok_or_else(|| {
            Error::invalid(format!(
                "configuration: a market-factory permission names authority `{name}`; write \
                 venue, product or jurisdiction"
            ))
        })
}

fn action(name: &str) -> Result<CreationAction> {
    CreationAction::ALL
        .into_iter()
        .find(|action| action.as_str() == name)
        .ok_or_else(|| {
            Error::invalid(format!(
                "configuration: a market-factory permission names action `{name}`; write create, \
                 list or seed"
            ))
        })
}

impl FactoryStage {
    /// Record the plan's permissions and take every planned market through
    /// create, list and seed on `venue`. Refused whole on the first step any
    /// market fails, naming it: a plan that half-installed would leave the
    /// venue holding markets the plan's reader believes were refused.
    pub fn install(
        plan: &FactoryPlan,
        venue: &mut dyn ListingVenue,
        now: Timestamp,
    ) -> Result<Self> {
        let mut permissions = CreationPermissions::new();
        for entry in &plan.permissions {
            let request = CreationRequest::new(
                action(&entry.action)?,
                entry.venue.as_str(),
                entry.product.as_str(),
                entry.jurisdiction.as_str(),
            );
            permissions.record(authority(&entry.authority)?, &request, &entry.reference)?;
        }
        let mut factory = MarketFactory::for_venue(venue)?;
        let mut mark_sources = BTreeMap::new();
        for market in &plan.markets {
            let step = |action: CreationAction| {
                permissions.admit(&CreationRequest::new(
                    action,
                    market.venue.as_str(),
                    market.product.as_str(),
                    market.jurisdiction.as_str(),
                ))
            };
            let rule = PricingRule::new(
                market.reference_price,
                market.half_spread,
                market.level_step,
                market.level_size,
                market.levels,
                market.tolerance,
            )?;
            let named = |error: Error| {
                Error::denied(format!(
                    "market {} at {}: {}",
                    market.product,
                    market.venue,
                    error.message()
                ))
            };
            factory
                .create(&step(CreationAction::Create).map_err(named)?, rule, now)
                .map_err(named)?;
            factory
                .list(&step(CreationAction::List).map_err(named)?, venue, now)
                .map_err(named)?;
            factory
                .seed(
                    &step(CreationAction::Seed).map_err(named)?,
                    market.inventory,
                    venue,
                    now,
                )
                .map_err(named)?;
            mark_sources.insert(market.product.clone(), market.mark_source.clone());
        }
        Ok(Self {
            factory,
            permissions,
            mark_sources,
        })
    }

    pub fn factory(&self) -> &MarketFactory {
        &self.factory
    }

    pub fn permissions(&self) -> &CreationPermissions {
        &self.permissions
    }

    /// Replace the permission register, as an operator revoking or renewing
    /// one does. The next review judges every live market against it.
    pub fn set_permissions(&mut self, permissions: CreationPermissions) {
        self.permissions = permissions;
    }

    /// The marks the monitor reads: each market's source's latest observed
    /// price. The `f64` to `Decimal` crossing is here, rounded to nine
    /// places; a non-finite or unrepresentable observation is no mark.
    pub fn marks(&self, prices: &BTreeMap<String, Vec<f64>>) -> BTreeMap<String, Decimal> {
        self.mark_sources
            .iter()
            .filter_map(|(product, source)| {
                let last = prices.get(source)?.last().copied()?;
                if !last.is_finite() {
                    return None;
                }
                Decimal::from_f64(last).map(|mark| (product.clone(), mark.round_dp(9)))
            })
            .collect()
    }

    /// Run the validity monitor.
    pub fn review(
        &mut self,
        marks: &BTreeMap<String, Decimal>,
        venue: &mut dyn ListingVenue,
        now: Timestamp,
    ) -> Result<Vec<ValidityFinding>> {
        self.factory.review(&self.permissions, marks, venue, now)
    }

    /// Every step taken since the last drain.
    pub fn drain(&mut self) -> Vec<MarketFactoryRecorded> {
        self.factory
            .drain_journal()
            .into_iter()
            .map(|record| MarketFactoryRecorded { record })
            .collect()
    }
}
