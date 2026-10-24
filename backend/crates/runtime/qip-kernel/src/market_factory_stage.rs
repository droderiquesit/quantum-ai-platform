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
    FactoryRecord, LadderAdmission, MarketFactory, PricingRule, ValidityFinding,
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

/// A permission register from a plan's entries, refused whole on the first
/// entry naming an unknown action or authority or recording nothing.
pub fn register(entries: &[PermissionEntry]) -> Result<CreationPermissions> {
    let mut permissions = CreationPermissions::new();
    for entry in entries {
        let request = CreationRequest::new(
            action(&entry.action)?,
            entry.venue.as_str(),
            entry.product.as_str(),
            entry.jurisdiction.as_str(),
        );
        permissions.record(authority(&entry.authority)?, &request, &entry.reference)?;
    }
    Ok(permissions)
}

impl FactoryStage {
    /// Record the plan's permissions and take every planned market through
    /// create, list and seed on `venue`. Refused whole on the first step any
    /// market fails, naming it: a plan that half-installed would leave the
    /// venue holding markets the plan's reader believes were refused.
    ///
    /// `ladders` measures each market's ladder against the desk's limits and
    /// mints the verdict `MarketFactory::seed` rests it on; the composition
    /// root passes `OrderManager::admit_ladder` over the book as it stands.
    /// A ladder it refuses refuses the plan, like any other failed step.
    pub fn install(
        plan: &FactoryPlan,
        venue: &mut dyn ListingVenue,
        ladders: &mut dyn FnMut(&str, &str, &PricingRule) -> Result<LadderAdmission>,
        now: Timestamp,
    ) -> Result<Self> {
        let permissions = register(&plan.permissions)?;
        let mut factory = MarketFactory::for_venue(venue)?;
        let mut mark_sources = BTreeMap::new();
        // Every book this installation put on the venue, so a refusal part
        // way through takes them all back off it. The venue is the desk
        // broker's and outlives a refused plan: until 2026-10-24 a second
        // market refused at its seed left the first one listed and seeded
        // there, resting a ladder for a plan whose caller was told it was
        // refused, under a factory that was then dropped and so could
        // neither review nor withdraw it.
        let mut placed: Vec<String> = Vec::new();
        let installed = Self::take_through(
            plan,
            &permissions,
            &mut factory,
            &mut mark_sources,
            &mut placed,
            venue,
            ladders,
            now,
        );
        if let Err(error) = installed {
            for product in &placed {
                venue.withdraw_book(product);
            }
            return Err(error);
        }
        Ok(Self {
            factory,
            permissions,
            mark_sources,
        })
    }

    /// Create, list and seed every planned market, recording in `placed`
    /// each product whose book reached the venue before the step that
    /// might fail after it.
    #[allow(clippy::too_many_arguments)]
    fn take_through(
        plan: &FactoryPlan,
        permissions: &CreationPermissions,
        factory: &mut MarketFactory,
        mark_sources: &mut BTreeMap<String, String>,
        placed: &mut Vec<String>,
        venue: &mut dyn ListingVenue,
        ladders: &mut dyn FnMut(&str, &str, &PricingRule) -> Result<LadderAdmission>,
        now: Timestamp,
    ) -> Result<()> {
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
            // A book already resting under this product is not this
            // installation's to replace, and not its to withdraw on a
            // refusal either; refused before anything is written, so the
            // rollback above only ever removes what this plan put there.
            if venue.listed_book(market.product.as_str()).is_some() {
                return Err(named(Error::denied(format!(
                    "{} already rests a book for {}, and a planned market is listed only where                      nothing rests; withdraw it first, or plan the market under an id of its own",
                    venue.venue_name(),
                    market.product
                ))));
            }
            factory
                .create(
                    &step(CreationAction::Create).map_err(named)?,
                    rule.clone(),
                    now,
                )
                .map_err(named)?;
            let listing = step(CreationAction::List).map_err(named)?;
            placed.push(market.product.clone());
            factory.list(&listing, venue, now).map_err(named)?;
            let ladder =
                ladders(market.product.as_str(), market.venue.as_str(), &rule).map_err(named)?;
            factory
                .seed(
                    &step(CreationAction::Seed).map_err(named)?,
                    &ladder,
                    market.inventory,
                    venue,
                    now,
                )
                .map_err(named)?;
            mark_sources.insert(market.product.clone(), market.mark_source.clone());
        }
        Ok(())
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
