//! Physical commerce as a paper research and opportunity source in DISCOVER
//! and SIMULATE (R12-COMMERCE-5090 to -5097).
//!
//! `qip_financial::commerce` and `commerce_resale` held Product Scout, the
//! SKU resolver, all-in feasibility, the purchase executor, the logistics
//! plan, the event-sourced inventory ledger, the simulated marketplace and
//! the resale router — tested, and called by no kernel or app. This module
//! runs them in order:
//!
//! * **DISCOVER** polls the feed, runs [`scout`] over its observations
//!   (5091) and [`Resolver::resolve`] over its listings (5092), and queues
//!   every candidate whose listing resolved to a genuine product. A
//!   candidate whose listing is counterfeit, or a variant a GTIN cannot
//!   reconcile, is dropped and named.
//! * **SIMULATE** assesses each queued candidate's all-in economics (5093);
//!   buys a viable one through the [`PurchaseExecutor`] — account, fraud and
//!   limit controls before the only merchant there is, the simulated one
//!   (5094); plans its logistics, refusing a partial plan (5095); receives
//!   each unit into the event-sourced [`Ledger`] (5096) with its full
//!   twelve-attribute record (5090); routes it to the resale venue with the
//!   best net proceeds after carry, lists it, reprices it while it sits, and
//!   sells and settles it on the simulated marketplace once its venue's
//!   time to sale has passed (5097).
//!
//! # Paper, research-only, licensing first
//!
//! The executor holds a `SimulatedMerchant` by concrete type and the plane a
//! simulated marketplace; no merchant, marketplace or carrier is reachable
//! from this build (ADR 0021, ADR 0099). The feed's licensing posture is
//! decided before it is polled, exactly as [`crate::event_markets`] decides
//! it: a synthetic feed is research-only, any other needs a
//! `LicensingDecision` from `qip-data-finder` naming it and permitting
//! research, and every record says whether it was decision-grade. No commerce
//! figure reaches the desk's sizing or capital.

use std::collections::{BTreeMap, BTreeSet};

use qip_contracts::governance::Usage;
use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, Timestamp};
use qip_data_finder::admission::LicensingDecision;
use qip_events::{EventBody, Topic};
use qip_financial::commerce::{
    Account, Candidate, Channel, CommercePlane, Condition, Consolidation, Credential,
    FeasibilityInput, FraudBook, Insurance, Ledger, LedgerEvent, Listing, LogisticsPlan,
    LogisticsPlanDraft, Observation, PurchaseExecutor, PurchaseRequest, Resolver, UnitState, scout,
};
use qip_financial::commerce_resale::{
    Attr, CycleEvent, ResaleDesk, UnitAttributes, VenueKind, VenueQuote, route_resale,
};
use qip_financial::physical::{LogisticsTerms, Spoilage};
use serde::{Deserialize, Serialize};

use crate::event_markets::SourcePosture;

/// The most candidates queued between DISCOVER and SIMULATE.
pub const MAX_QUEUED_CANDIDATES: usize = 16;

/// The most candidates one SIMULATE pass assesses.
pub const CANDIDATES_PER_PASS: usize = 4;

/// The most units one purchase receives into the ledger.
pub const MAX_UNITS_PER_PURCHASE: u32 = 50;

/// Each resale venue's price as a share of the market reference, in the
/// order [`CommerceResearch::resale_quotes`] quotes them: the marketplace at
/// the market, an auction at 95% and a direct buyer at 85%. Stated, and the
/// same for every SKU, because the synthetic feed observes no resale venue
/// and a per-SKU figure would be one nobody measured.
const RESALE_FACTORS: [Decimal; 3] = [
    Decimal::ONE,
    Decimal::from_raw(950_000_000),
    Decimal::from_raw(850_000_000),
];

/// The most units held listed at once.
pub const MAX_LISTED_UNITS: usize = 256;

/// One batch a commerce feed published.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommerceBatch {
    pub observations: Vec<Observation>,
    pub listings: Vec<Listing>,
}

/// What a commerce feed is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommerceSource {
    pub name: String,
    pub is_synthetic: bool,
}

/// A source of price, availability and listing readings, pulled with the
/// caller holding the clock.
pub trait CommerceFeed: std::fmt::Debug {
    fn descriptor(&self) -> CommerceSource;
    fn poll(&mut self, until: Timestamp) -> Result<CommerceBatch>;
}

/// A deterministic synthetic commerce feed: a few SKUs read at a few venues
/// across the four channels, with discounts drawn from a seeded stream and,
/// now and then, a listing from a seller on the counterfeit registry.
#[derive(Debug)]
pub struct SyntheticCommerceFeed {
    rng: Xoshiro256,
    step: Duration,
    next_at: Timestamp,
    availability: BTreeMap<String, u32>,
}

/// The SKUs the synthetic feed reads: product, brand, model, variant, GTIN,
/// reference price in whole units.
const SYNTHETIC_SKUS: [(&str, &str, &str, &str, &str, i64); 3] = [
    (
        "sku-headphones",
        "acme",
        "h100",
        "black",
        "0000000000017",
        120,
    ),
    ("sku-kettle", "brewco", "k2", "steel", "0000000000024", 60),
    ("sku-drill", "toolworks", "d9", "18v", "0000000000031", 150),
];

/// The venues it reads them at, one per channel.
const SYNTHETIC_VENUES: [(&str, Channel); 4] = [
    ("synthetic-retail", Channel::Retail),
    ("synthetic-auction", Channel::Auction),
    ("synthetic-liquidation", Channel::Liquidation),
    ("synthetic-wholesale", Channel::Wholesale),
];

/// The seller the synthetic feed occasionally lists through, and which the
/// plane's resolver holds on its counterfeit registry.
pub const SYNTHETIC_COUNTERFEIT_SELLER: &str = "synthetic-counterfeit-seller";

impl SyntheticCommerceFeed {
    pub fn new(seed: u64, start: Timestamp) -> Self {
        Self {
            rng: Xoshiro256::seeded(seed).fork("synthetic-commerce"),
            step: Duration::from_hours(1),
            next_at: start,
            availability: BTreeMap::new(),
        }
    }

    /// The venues this feed reads, for a plan's account authority.
    pub fn venues() -> Vec<String> {
        SYNTHETIC_VENUES
            .iter()
            .map(|(venue, _)| (*venue).to_string())
            .collect()
    }

    fn batch_at(&mut self) -> Result<CommerceBatch> {
        let mut batch = CommerceBatch::default();
        for (sku, brand, model, variant, gtin, reference) in SYNTHETIC_SKUS {
            let reference_price = Decimal::from_int(reference);
            for (venue, channel) in SYNTHETIC_VENUES {
                // The crossing into money is here: the draw is a statistic,
                // the price a decimal rounded to cents.
                let discount_f64 = self.rng.uniform(-0.1, 0.4);
                let factor = Decimal::from_f64(1.0 - discount_f64)
                    .ok_or_else(|| Error::numeric("a synthetic discount did not cross"))?
                    .round_dp(4);
                let price = (reference_price * factor).round_dp(2);
                let key = format!("{venue}:{sku}");
                let previous = self.availability.get(&key).copied().unwrap_or(0);
                let units = u32::try_from(self.rng.below(40)).unwrap_or(0) + 1;
                self.availability.insert(key.clone(), units);
                batch.observations.push(Observation {
                    product: sku.to_string(),
                    venue: venue.to_string(),
                    channel,
                    price,
                    reference_price,
                    units_available: units,
                    units_previously_available: previous,
                });
                let counterfeit = self.rng.bernoulli(0.1);
                batch.listings.push(Listing {
                    id: key,
                    brand: brand.to_string(),
                    model: model.to_string(),
                    variant: variant.to_string(),
                    gtin: Some(gtin.to_string()),
                    seller: if counterfeit {
                        SYNTHETIC_COUNTERFEIT_SELLER.to_string()
                    } else {
                        format!("{venue}-seller")
                    },
                });
            }
        }
        Ok(batch)
    }
}

impl CommerceFeed for SyntheticCommerceFeed {
    fn descriptor(&self) -> CommerceSource {
        CommerceSource {
            name: "synthetic-commerce-feed".to_string(),
            is_synthetic: true,
        }
    }

    fn poll(&mut self, until: Timestamp) -> Result<CommerceBatch> {
        let mut out = CommerceBatch::default();
        while self.next_at <= until {
            let batch = self.batch_at()?;
            out.observations.extend(batch.observations);
            out.listings.extend(batch.listings);
            self.next_at = self.next_at.saturating_add(self.step);
            // One batch per poll at most: a feed that caught up a day of
            // hourly readings in one pass would hand DISCOVER a backlog it
            // then reads as one instant.
            if self.next_at <= until {
                self.next_at = until.saturating_add(self.step);
            }
        }
        Ok(out)
    }
}

/// The account the plane buys through, as a plan states it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountEntry {
    pub id: String,
    pub identity_class: String,
    pub merchants: BTreeSet<String>,
    pub per_purchase_limit: Decimal,
}

/// The logistics decisions a plan states once for every purchase.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogisticsEntry {
    pub carrier: String,
    pub warehouse: String,
    /// `None` ships direct; a count consolidates with that many shipments.
    pub consolidate_with: Option<u32>,
    /// Coverage and premium; absent declines insurance with `decline_reason`.
    pub insurance: Option<(Decimal, Decimal)>,
    pub decline_reason: Option<String>,
    pub delivery_risk: Decimal,
}

/// The resale desk's terms.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResaleEntry {
    pub floor_per_unit: Decimal,
    pub markdown_after_days: u32,
    pub markdown_rate: Decimal,
}

/// The commerce section of a deployment's breadth plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommercePlan {
    /// `synthetic` is the only source this build can open.
    pub source: String,
    pub owner: String,
    pub terms: LogisticsTerms,
    pub quantity: Decimal,
    pub days_on_market: u32,
    pub daily_capital_cost: Decimal,
    pub purchase_tax_rate: Decimal,
    pub account: AccountEntry,
    pub total_ceiling: Decimal,
    #[serde(default)]
    pub fraud_merchants: BTreeSet<String>,
    #[serde(default)]
    pub counterfeit_gtins: BTreeSet<String>,
    #[serde(default)]
    pub counterfeit_sellers: BTreeSet<String>,
    pub logistics: LogisticsEntry,
    pub resale: ResaleEntry,
}

/// What one DISCOVER pass did, journaled.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommerceDiscovered {
    pub source: String,
    pub posture: String,
    pub decision_grade: bool,
    pub observations: usize,
    pub candidates: Vec<Candidate>,
    /// Listings dropped as counterfeit, each with its finding.
    pub counterfeit: Vec<String>,
    pub problems: Vec<String>,
}

impl EventBody for CommerceDiscovered {
    const TOPIC: Topic = Topic::OpportunityDetected;
    const SCHEMA_VERSION: u32 = 1;
}

/// One candidate's all-in economics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssessedCandidate {
    pub product: String,
    pub venue: String,
    pub all_in_cost: Decimal,
    pub expected_resale: Decimal,
    pub inventory_carry: Decimal,
    pub capital_lockup: Decimal,
    pub time_to_sale_days: u32,
    pub return_risk: Decimal,
    /// The customs duty and the purchase tax inside `all_in_cost`, journaled
    /// apart (R12-COMMERCE-1093): an all-in figure alone cannot say whether
    /// a candidate died on its freight or on its border, and the two lead to
    /// different research. Defaulted so a record journaled before they were
    /// carried still decodes.
    #[serde(default)]
    pub duty: Decimal,
    #[serde(default)]
    pub purchase_tax: Decimal,
    /// Expected resale exceeds the all-in cost plus carry.
    pub viable: bool,
}

/// A paper purchase the executor admitted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaperPurchase {
    pub sku: String,
    pub merchant: String,
    pub quantity: Decimal,
    pub notional: Decimal,
    pub units: Vec<String>,
}

/// A unit listed or repriced, with where and at what.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PricedUnit {
    pub unit: String,
    pub venue: String,
    pub price: Decimal,
}

/// What one SIMULATE pass did, journaled.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommerceSimulated {
    pub decision_grade: bool,
    pub day: u32,
    pub assessed: Vec<AssessedCandidate>,
    pub purchases: Vec<PaperPurchase>,
    pub refusals: Vec<String>,
    pub listed: Vec<PricedUnit>,
    pub repriced: Vec<PricedUnit>,
    pub sold: Vec<CycleEvent>,
    pub problems: Vec<String>,
}

impl EventBody for CommerceSimulated {
    const TOPIC: Topic = Topic::ScenarioEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}

#[derive(Clone, Debug)]
struct HeldUnit {
    sku: String,
    venue: String,
    cost_basis: Decimal,
    listed_day: u32,
    days_to_sale: u32,
    /// The chosen venue's price as a share of the market reference, so a
    /// repricing follows the reference at the venue's own discount.
    factor: Decimal,
}

/// The commerce research plane.
#[derive(Debug)]
pub struct CommerceResearch {
    feed: Box<dyn CommerceFeed + Send + Sync>,
    source: CommerceSource,
    posture: SourcePosture,
    plan: CommercePlan,
    resolver: Resolver,
    executor: PurchaseExecutor,
    plane: CommercePlane,
    resale: ResaleDesk,
    queue: Vec<Candidate>,
    references: BTreeMap<String, Decimal>,
    held: BTreeMap<String, HeldUnit>,
    attributes: BTreeMap<String, UnitAttributes>,
    logistics: Option<LogisticsPlan>,
    day: u32,
    purchases: u32,
    cycles: Vec<CycleEvent>,
    /// Sales settled and buyer returns taken, for the plan's return rate.
    sales: i64,
    returns_taken: i64,
}

impl CommerceResearch {
    /// Open the plane over `feed`, refusing a feed whose licensing posture
    /// has not been evaluated and a plan whose terms do not validate.
    pub fn open(
        feed: Box<dyn CommerceFeed + Send + Sync>,
        licensing: Option<LicensingDecision>,
        plan: CommercePlan,
    ) -> Result<Self> {
        let source = feed.descriptor();
        let posture = if source.is_synthetic {
            SourcePosture::Synthetic
        } else {
            match licensing {
                Some(decision)
                    if decision.source_id == source.name
                        && decision.usages.contains(&Usage::Research) =>
                {
                    SourcePosture::Admitted(Box::new(decision))
                }
                _ => {
                    return Err(Error::denied(format!(
                        "commerce source {} has no licensing evaluation admitting it for \
                         research, so it is refused before it is polled; admit it through \
                         qip-data-finder's licensing gate first",
                        source.name
                    )));
                }
            }
        };
        // The terms deserialised past `Spoilage::new`'s checks; ask them
        // again rather than trust a rate nobody validated.
        Spoilage::new(plan.terms.spoilage.per_day(), plan.terms.spoilage.source())?;
        if !plan.quantity.is_positive() || plan.owner.trim().is_empty() {
            return Err(Error::invalid(
                "a commerce plan needs a positive purchase quantity and a named owner",
            ));
        }
        let executor = PurchaseExecutor::new(
            vec![Account {
                id: plan.account.id.clone(),
                identity_class: plan.account.identity_class.clone(),
                merchants: plan.account.merchants.clone(),
                per_purchase_limit: plan.account.per_purchase_limit,
            }],
            FraudBook {
                merchants: plan.fraud_merchants.clone(),
                listings: BTreeSet::new(),
                payments: BTreeSet::new(),
            },
            plan.total_ceiling,
        );
        let resale = ResaleDesk::new(
            plan.resale.floor_per_unit,
            plan.resale.markdown_after_days,
            plan.resale.markdown_rate,
        )?;
        let logistics = LogisticsPlanDraft {
            route: plan.terms.route.clone(),
            carrier: Some(plan.logistics.carrier.clone()),
            consolidation: Some(match plan.logistics.consolidate_with {
                None => Consolidation::Direct,
                Some(with_shipments) => Consolidation::Consolidate { with_shipments },
            }),
            warehouse: Some(plan.logistics.warehouse.clone()),
            customs: Some(plan.terms.customs.clone()),
            insurance: match (&plan.logistics.insurance, &plan.logistics.decline_reason) {
                (Some((coverage, premium)), _) => Some(Insurance::Insured {
                    coverage: *coverage,
                    premium: *premium,
                }),
                (None, Some(reason)) => Some(Insurance::Declined {
                    reason: reason.clone(),
                }),
                (None, None) => None,
            },
            delivery_risk: Some(plan.logistics.delivery_risk),
        }
        .into_plan()?;
        let resolver = Resolver::new(
            plan.counterfeit_gtins.clone(),
            plan.counterfeit_sellers.clone(),
        );
        let plane = CommercePlane::new(Ledger::default(), plan.terms.fees.clone());
        Ok(Self {
            feed,
            source,
            posture,
            plan,
            resolver,
            executor,
            plane,
            resale,
            queue: Vec::new(),
            references: BTreeMap::new(),
            held: BTreeMap::new(),
            attributes: BTreeMap::new(),
            logistics: Some(logistics),
            day: 0,
            purchases: 0,
            cycles: Vec::new(),
            sales: 0,
            returns_taken: 0,
        })
    }

    pub fn source(&self) -> &CommerceSource {
        &self.source
    }

    pub fn posture(&self) -> &SourcePosture {
        &self.posture
    }

    pub fn ledger(&self) -> &Ledger {
        self.plane.ledger()
    }

    /// The ledger and the simulated marketplace together, so a settlement
    /// can be read against the sale it nets.
    pub fn plane(&self) -> &CommercePlane {
        &self.plane
    }

    pub fn executor(&self) -> &PurchaseExecutor {
        &self.executor
    }

    pub fn attributes(&self, unit: &str) -> Option<&UnitAttributes> {
        self.attributes.get(unit)
    }

    pub fn resale(&self) -> &ResaleDesk {
        &self.resale
    }

    pub fn cycles(&self) -> &[CycleEvent] {
        &self.cycles
    }

    pub fn queued(&self) -> &[Candidate] {
        &self.queue
    }

    fn decision_grade(&self) -> bool {
        self.posture.class().allows_production_decisions()
    }

    /// DISCOVER: scout and resolve what the feed published up to `now`.
    pub fn discover(&mut self, now: Timestamp) -> Result<CommerceDiscovered> {
        let batch = self.feed.poll(now)?;
        let mut found = CommerceDiscovered {
            source: self.source.name.clone(),
            posture: self.posture.as_str().to_string(),
            decision_grade: self.decision_grade(),
            observations: batch.observations.len(),
            ..CommerceDiscovered::default()
        };
        for observation in &batch.observations {
            self.references
                .insert(observation.product.clone(), observation.reference_price);
        }
        let identities = match self.resolver.resolve(&batch.listings) {
            Ok(identities) => identities,
            Err(error) => {
                found.problems.push(format!(
                    "the listings could not be resolved, so nothing was queued: {}",
                    error.message()
                ));
                return Ok(found);
            }
        };
        for candidate in scout(&batch.observations) {
            let listing = format!("{}:{}", candidate.venue, candidate.product);
            match identities.get(&listing) {
                Some(identity) => {
                    if let Some(finding) = &identity.counterfeit_finding {
                        found.counterfeit.push(format!("{listing}: {finding}"));
                        continue;
                    }
                }
                None => {
                    found.problems.push(format!(
                        "candidate {listing} has no listing to resolve, so it is not queued"
                    ));
                    continue;
                }
            }
            if self.queue.len() >= MAX_QUEUED_CANDIDATES {
                found.problems.push(format!(
                    "candidate {listing} arrived past the queue bound of \
                     {MAX_QUEUED_CANDIDATES} and was not queued"
                ));
                continue;
            }
            self.queue.push(candidate.clone());
            found.candidates.push(candidate);
        }
        Ok(found)
    }

    /// SIMULATE: assess, buy, plan, receive, route, list, reprice and sell,
    /// all on paper.
    pub fn simulate(&mut self) -> Result<CommerceSimulated> {
        self.day = self.day.saturating_add(1);
        let mut run = CommerceSimulated {
            decision_grade: self.decision_grade(),
            day: self.day,
            ..CommerceSimulated::default()
        };
        self.work_listed(&mut run)?;
        let take = self.queue.len().min(CANDIDATES_PER_PASS);
        let candidates: Vec<Candidate> = self.queue.drain(..take).collect();
        for candidate in candidates {
            if let Err(error) = self.work_candidate(&candidate, &mut run) {
                run.problems.push(format!(
                    "{} at {}: {}",
                    candidate.product,
                    candidate.venue,
                    error.message()
                ));
            }
        }
        Ok(run)
    }

    fn work_candidate(&mut self, candidate: &Candidate, run: &mut CommerceSimulated) -> Result<()> {
        let assessment = FeasibilityInput {
            unit_price: Some(candidate.price),
            quantity: Some(self.plan.quantity),
            declared_value_per_unit: Some(candidate.price),
            terms: Some(self.plan.terms.clone()),
            resale: vec![(candidate.reference_price, Decimal::ONE)],
            days_on_market: Some(self.plan.days_on_market),
            daily_capital_cost: Some(self.plan.daily_capital_cost),
            purchase_tax_rate: Some(self.plan.purchase_tax_rate),
        }
        .assess()?;
        let viable =
            assessment.expected_resale > assessment.all_in_cost + assessment.inventory_carry;
        run.assessed.push(AssessedCandidate {
            product: candidate.product.clone(),
            venue: candidate.venue.clone(),
            all_in_cost: assessment.all_in_cost,
            expected_resale: assessment.expected_resale,
            inventory_carry: assessment.inventory_carry,
            capital_lockup: assessment.capital_lockup,
            time_to_sale_days: assessment.time_to_sale_days,
            return_risk: assessment.return_risk,
            duty: assessment.landed.duty,
            purchase_tax: assessment.purchase_tax,
            viable,
        });
        if !viable {
            return Ok(());
        }
        let listing = format!("{}:{}", candidate.venue, candidate.product);
        let request = PurchaseRequest {
            account_id: self.plan.account.id.clone(),
            credential: Credential {
                account_id: self.plan.account.id.clone(),
                identity_class: self.plan.account.identity_class.clone(),
            },
            merchant: candidate.venue.clone(),
            listing,
            payment: "paper-instrument".to_string(),
            sku: candidate.product.clone(),
            quantity: self.plan.quantity,
            unit_price: candidate.price,
        };
        let record = match self.executor.purchase(&request) {
            Ok(record) => record,
            Err(error) => {
                run.refusals.push(format!(
                    "purchase of {} from {}: {}",
                    candidate.product,
                    candidate.venue,
                    error.message()
                ));
                return Ok(());
            }
        };
        self.purchases = self.purchases.saturating_add(1);
        let logistics = self
            .logistics
            .clone()
            .ok_or_else(|| Error::invalid("the logistics plan is missing"))?;
        let unit_count = whole_units(self.plan.quantity)?;
        let cost_basis = assessment
            .all_in_cost
            .checked_div(self.plan.quantity)
            .ok_or_else(|| Error::numeric("the per-unit cost basis is undefined"))?;
        let mut units = Vec::new();
        for index in 0..unit_count {
            if self.held.len() >= MAX_LISTED_UNITS {
                run.problems.push(format!(
                    "the plane holds {MAX_LISTED_UNITS} units, its bound; the rest of purchase \
                     {} were not received",
                    self.purchases
                ));
                break;
            }
            let unit = format!("{}-{}-{index}", candidate.product, self.purchases);
            self.plane_apply(LedgerEvent::Received {
                unit: unit.clone(),
                sku: candidate.product.clone(),
                condition: Condition::New,
                location: logistics.warehouse.clone(),
                cost_basis,
                owner: self.plan.owner.clone(),
            })?;
            self.attributes.insert(
                unit.clone(),
                self.unit_attributes(&unit, candidate, &logistics)?,
            );
            let quotes = self.resale_quotes(candidate.reference_price);
            let choice = route_resale(cost_basis, self.plan.daily_capital_cost, &quotes)?;
            // Listed at the chosen venue's own expected price: the router
            // chose on net proceeds, and the venue takes its fee from the
            // price the unit is offered at, not from the net.
            let (quote, factor) = quotes
                .iter()
                .zip(RESALE_FACTORS)
                .find(|(quote, _)| quote.venue == choice.chosen.venue)
                .map(|(quote, factor)| (quote.clone(), factor))
                .ok_or_else(|| Error::invalid("the router chose a venue it was not quoted"))?;
            let price = self
                .resale
                .list(&self.plane, &unit, quote.expected_price, self.day)?;
            self.held.insert(
                unit.clone(),
                HeldUnit {
                    sku: candidate.product.clone(),
                    venue: choice.chosen.venue.clone(),
                    cost_basis,
                    listed_day: self.day,
                    days_to_sale: quote.days_to_sale,
                    factor,
                },
            );
            run.listed.push(PricedUnit {
                unit: unit.clone(),
                venue: choice.chosen.venue,
                price,
            });
            units.push(unit);
        }
        run.purchases.push(PaperPurchase {
            sku: record.sku,
            merchant: record.merchant,
            quantity: record.quantity,
            notional: record.notional,
            units,
        });
        Ok(())
    }

    /// Reprice what is still sitting, and sell what has sat as long as its
    /// venue takes.
    fn work_listed(&mut self, run: &mut CommerceSimulated) -> Result<()> {
        let held: Vec<(String, HeldUnit)> = self
            .held
            .iter()
            .map(|(unit, held)| (unit.clone(), held.clone()))
            .collect();
        for (unit, held) in held {
            let state = self
                .plane
                .ledger()
                .units()
                .get(&unit)
                .map(|u| u.state.clone());
            if !matches!(state, Some(UnitState::Held)) {
                continue;
            }
            let elapsed = self.day.saturating_sub(held.listed_day);
            let Some(listed) = self.resale.listed(&unit).cloned() else {
                continue;
            };
            if elapsed >= held.days_to_sale {
                let sale = format!("sale-{unit}");
                self.plane
                    .sell(&sale, &unit, "paper-buyer", listed.price, None)?;
                self.plane.settle(&sale)?;
                let returned = self.take_return_owed(&sale)?;
                let settlement = self
                    .plane
                    .settlement(&sale)
                    .ok_or_else(|| Error::invalid(format!("sale {sale} settled and vanished")))?;
                let event = CycleEvent {
                    sku: held.sku.clone(),
                    realised_cost: held.cost_basis,
                    resale_price: settlement.net_after_adjustments(),
                    days_to_sale: elapsed,
                    returned,
                };
                self.cycles.push(event.clone());
                run.sold.push(event);
                self.held.remove(&unit);
                continue;
            }
            let market = self
                .references
                .get(&held.sku)
                .map_or(listed.market_price, |reference| {
                    (*reference * held.factor).round_dp(2)
                });
            match self.resale.reprice(&self.plane, &unit, market, self.day) {
                Ok(Some(price)) => run.repriced.push(PricedUnit {
                    unit: unit.clone(),
                    venue: held.venue.clone(),
                    price,
                }),
                Ok(None) => {}
                Err(error) => run
                    .refusals
                    .push(format!("repricing {unit}: {}", error.message())),
            }
        }
        Ok(())
    }

    /// Take a buyer return on `sale` when the plan's return rate says one is
    /// owed (R12-COMMERCE-1092). Counted, not drawn: the k-th return is
    /// taken at the sale where `floor(sales × rate)` first reaches k, so the
    /// realised rate tracks the stated one exactly and a replay of the log
    /// takes the same returns. Before this the plan's rate priced
    /// feasibility's return risk and no sale was ever returned, so the
    /// settlement adjustment and the ledger's return record had no caller.
    /// The unit goes back into the warehouse open-box and is not relisted by
    /// this pass: a returned unit's resale is a decision about its
    /// condition this plane has no inspection for.
    fn take_return_owed(&mut self, sale: &str) -> Result<bool> {
        self.sales = self.sales.saturating_add(1);
        let owed = Decimal::from_int(self.sales)
            .checked_mul(self.plan.terms.returns.rate)
            .ok_or_else(|| Error::numeric("the returns owed overflowed"))?
            .truncate_dp(0);
        if owed <= Decimal::from_int(self.returns_taken) {
            return Ok(false);
        }
        let warehouse = self
            .logistics
            .as_ref()
            .map(|plan| plan.warehouse.clone())
            .ok_or_else(|| Error::invalid("the logistics plan is missing"))?;
        self.plane
            .return_unit(sale, Condition::OpenBox, &warehouse, &self.plan.owner)?;
        self.returns_taken = self.returns_taken.saturating_add(1);
        Ok(true)
    }

    fn plane_apply(&mut self, event: LedgerEvent) -> Result<()> {
        // Through the plane, so the ledger it settles sales against is the
        // ledger that received the unit.
        self.plane.receive(event)
    }

    fn unit_attributes(
        &self,
        unit: &str,
        candidate: &Candidate,
        logistics: &LogisticsPlan,
    ) -> Result<UnitAttributes> {
        let terms = &self.plan.terms;
        let value = |text: String| Attr::Value(text);
        let attrs: BTreeMap<String, Attr> = [
            ("sku_identity", value(candidate.product.clone())),
            ("condition", value("new".to_string())),
            ("location", value(logistics.warehouse.clone())),
            ("inventory_state", value("held".to_string())),
            (
                "shipping",
                value(format!(
                    "{} via {}",
                    logistics.route.len(),
                    logistics.carrier
                )),
            ),
            (
                "insurance",
                value(match &logistics.insurance {
                    Insurance::Insured { coverage, premium } => {
                        format!("insured for {coverage} at {premium}")
                    }
                    Insurance::Declined { reason } => format!("declined: {reason}"),
                }),
            ),
            (
                "customs_duties",
                value(format!("duty rate {}", terms.customs.duty_rate)),
            ),
            (
                "tax",
                value(format!("purchase tax {}", self.plan.purchase_tax_rate)),
            ),
            (
                "marketplace_fees",
                value(format!(
                    "{} ad valorem, {} per unit",
                    terms.fees.ad_valorem, terms.fees.per_unit
                )),
            ),
            (
                "payment_chargeback_risk",
                Attr::Absent {
                    reason: "a simulated merchant takes no payment network, so no chargeback \
                             can be raised"
                        .to_string(),
                },
            ),
            (
                "storage",
                value(format!(
                    "{} at {} per unit per day",
                    logistics.warehouse, terms.storage_per_unit_per_day
                )),
            ),
            (
                "returns",
                value(format!("return rate {}", terms.returns.rate)),
            ),
        ]
        .into_iter()
        .map(|(name, attr)| (name.to_string(), attr))
        .collect();
        UnitAttributes::new(unit, attrs)
    }

    /// What each resale venue offers for a unit whose market price is
    /// `reference`: the marketplace at the market less its fees, an auction
    /// slightly under it but faster, a direct buyer under that and faster
    /// still.
    fn resale_quotes(&self, reference: Decimal) -> Vec<VenueQuote> {
        let fees = &self.plan.terms.fees;
        vec![
            VenueQuote {
                kind: VenueKind::Marketplace,
                venue: "simulated-marketplace".to_string(),
                expected_price: (reference * RESALE_FACTORS[0]).round_dp(2),
                fee_rate: fees.ad_valorem,
                fixed_fee: fees.per_unit,
                days_to_sale: self.plan.days_on_market,
            },
            VenueQuote {
                kind: VenueKind::Auction,
                venue: "simulated-auction".to_string(),
                expected_price: (reference * RESALE_FACTORS[1]).round_dp(2),
                fee_rate: fees.ad_valorem,
                fixed_fee: Decimal::ZERO,
                days_to_sale: (self.plan.days_on_market / 2).max(1),
            },
            VenueQuote {
                kind: VenueKind::Direct,
                venue: "simulated-direct".to_string(),
                expected_price: (reference * RESALE_FACTORS[2]).round_dp(2),
                fee_rate: Decimal::ZERO,
                fixed_fee: Decimal::ZERO,
                days_to_sale: 1,
            },
        ]
    }
}

/// The whole units a purchase quantity receives, refused when fractional or
/// past the bound rather than rounded.
fn whole_units(quantity: Decimal) -> Result<u32> {
    let whole = quantity.round_dp(0);
    if whole != quantity || !whole.is_positive() {
        return Err(Error::invalid(format!(
            "a purchase of {quantity} is not a whole number of physical units"
        )));
    }
    let count = u32::try_from(whole.raw() / qip_core::decimal::SCALE)
        .ok()
        .filter(|count| *count <= MAX_UNITS_PER_PURCHASE)
        .ok_or_else(|| {
            Error::invalid(format!(
                "a purchase of {quantity} units is past the bound of {MAX_UNITS_PER_PURCHASE}"
            ))
        })?;
    Ok(count)
}
