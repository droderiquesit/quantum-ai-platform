//! The Market Factory: create, list, seed and withdraw a market, on the
//! simulated venue only (R12-EXEC-5080, -5081; R12-COVERAGE-5107).
//!
//! Market creation is EXEC-027: "Creating, listing or seeding a market must
//! go through a governed product workflow with recorded approval. A model or
//! agent may propose a market, but no model or agent output may create, list
//! or seed one directly." Until 2026-10-11 this module's three operations
//! were bodies returning `Ok(())` over an approval struct with public fields
//! that anybody could build, so the workflow was a comment and the market it
//! created existed nowhere.
//!
//! # What gates each step
//!
//! Every operation demands a [`CreationAdmission`] for exactly that action,
//! and an admission has one mint — [`CreationPermissions::admit`], which
//! refuses unless the venue, the product rules and the jurisdiction have
//! each recorded a permission (COVERAGE-5107). There is no other way to
//! hold one: no public constructor, no `Deserialize`. Permission to create
//! is not permission to list, and permission to list is not permission to
//! seed, because seeding commits inventory.
//!
//! # Simulated venue only, twice
//!
//! The factory reaches a venue through [`ListingVenue`], which a broker
//! hands out from [`crate::broker::Broker::listing_venue`] and which only
//! the simulated broker overrides. And every operation refuses a surface
//! whose `is_simulated` is false, or whose name is not the venue the
//! admission names, before anything is written: a market admitted for one
//! venue is not created on another, and no market is created on a real one
//! (ADR 0003, ADR 0021).
//!
//! # What "created" means here
//!
//! * **Create** records the market and publishes its [`PricingRule`] — the
//!   reference price, the spread, the ladder and the tolerance the validity
//!   monitor holds it to.
//! * **List** puts an empty book for the instrument on the venue.
//! * **Seed** commits inventory and rests the rule's ladder on both sides,
//!   refusing inventory that cannot cover the ask side it publishes.
//!
//! Each step is a [`FactoryRecord`] in an append-only journal the composer
//! drains into the event log, so the life of every market is reproducible
//! from the log alone.
//!
//! # The validity monitor (EXEC-5081)
//!
//! [`MarketFactory::review`] re-asks, every time it is called, whether each
//! live market is still legally and economically valid: every permission
//! the market was created, listed and seeded under must still admit; a
//! seeded market must still have a mark, within its rule's tolerance of the
//! published reference; and its book must still rest at the venue. A market
//! that fails any of them is flagged and withdrawn from the venue in the
//! same call — a check that only reported would be a market still quoting
//! after its permission lapsed.

use std::collections::BTreeMap;

use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook};
use serde::{Deserialize, Serialize};

use crate::broker::ListingVenue;
use crate::creation::{CreationAction, CreationAdmission, CreationPermissions, CreationRequest};

/// The most markets one factory will hold, live or withdrawn. A bound, not a
/// target: the journal is drained, and the market table is what the monitor
/// walks every review.
pub const MAX_MARKETS: usize = 64;

/// The most price levels a rule may rest on one side.
pub const MAX_LEVELS: u32 = 20;

/// The pre-trade risk verdict on resting one pricing rule's ladder for one
/// product at one venue.
///
/// No public constructor and no `Deserialize`: the only mint is
/// [`crate::oms::OrderManager::admit_ladder`], so [`MarketFactory::seed`]
/// cannot rest a ladder the desk's limits have not measured. The same shape
/// as [`CreationAdmission`], and for the same reason — a gate that any
/// caller could satisfy by building the struct is a comment.
#[derive(Debug)]
pub struct LadderAdmission {
    pub(crate) product: String,
    pub(crate) venue: String,
    pub(crate) rule: PricingRule,
}

impl LadderAdmission {
    pub fn product(&self) -> &str {
        &self.product
    }

    pub fn venue(&self) -> &str {
        &self.venue
    }
}

/// How a created market is priced, published at creation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingRule {
    reference_price: Decimal,
    half_spread: Decimal,
    level_step: Decimal,
    level_size: Decimal,
    levels: u32,
    tolerance: Decimal,
}

impl PricingRule {
    /// A rule quoting `levels` levels of `level_size` either side of
    /// `reference_price`, the touch `half_spread` away and each further level
    /// `level_step` beyond it. `tolerance` is the fraction a mark may move
    /// from the reference before the market's prices are economically
    /// invalid and the monitor withdraws it.
    ///
    /// Refused rather than clamped: a non-positive figure, a spread or ladder
    /// that would quote a bid at or below zero, a level count outside one to
    /// [`MAX_LEVELS`], or a tolerance outside `(0, 1)`.
    pub fn new(
        reference_price: Decimal,
        half_spread: Decimal,
        level_step: Decimal,
        level_size: Decimal,
        levels: u32,
        tolerance: Decimal,
    ) -> Result<Self> {
        for (value, what) in [
            (reference_price, "reference price"),
            (half_spread, "half-spread"),
            (level_step, "level step"),
            (level_size, "level size"),
        ] {
            if !value.is_positive() {
                return Err(Error::invalid(format!(
                    "a pricing rule's {what} of {value} is not positive; state a positive figure"
                )));
            }
        }
        if levels == 0 || levels > MAX_LEVELS {
            return Err(Error::invalid(format!(
                "a pricing rule resting {levels} levels is outside 1 to {MAX_LEVELS}"
            )));
        }
        if !tolerance.is_positive() || tolerance >= Decimal::ONE {
            return Err(Error::invalid(format!(
                "a tolerance of {tolerance} is outside (0, 1); it is the fraction a mark may move \
                 from the reference before the market is withdrawn"
            )));
        }
        let deepest = level_step
            .checked_mul(Decimal::from_int(i64::from(levels - 1)))
            .and_then(|ladder| ladder.checked_add(half_spread))
            .ok_or_else(|| Error::numeric("the pricing rule's ladder overflowed"))?;
        if deepest >= reference_price {
            return Err(Error::invalid(format!(
                "the deepest bid sits {deepest} below a reference of {reference_price}, at or \
                 below zero; narrow the spread or the ladder"
            )));
        }
        Ok(Self {
            reference_price,
            half_spread,
            level_step,
            level_size,
            levels,
            tolerance,
        })
    }

    pub const fn reference_price(&self) -> Decimal {
        self.reference_price
    }

    pub const fn half_spread(&self) -> Decimal {
        self.half_spread
    }

    pub const fn levels(&self) -> u32 {
        self.levels
    }

    pub const fn level_size(&self) -> Decimal {
        self.level_size
    }

    pub const fn tolerance(&self) -> Decimal {
        self.tolerance
    }

    /// The size the rule rests on the ask side, which seeded inventory must
    /// cover.
    pub fn ask_quantity(&self) -> Result<Decimal> {
        self.level_size
            .checked_mul(Decimal::from_int(i64::from(self.levels)))
            .ok_or_else(|| Error::numeric("the rule's ask quantity overflowed"))
    }

    /// The best bid the rule rests: the reference less the half-spread.
    pub fn top_bid(&self) -> Decimal {
        self.reference_price - self.half_spread
    }

    /// The deepest ask the rule rests, the highest price on the ask side.
    pub fn deepest_ask(&self) -> Result<Decimal> {
        self.level_step
            .checked_mul(Decimal::from_int(i64::from(self.levels - 1)))
            .and_then(|ladder| ladder.checked_add(self.half_spread))
            .and_then(|offset| self.reference_price.checked_add(offset))
            .ok_or_else(|| Error::numeric("the rule's deepest ask overflowed"))
    }

    /// The book the rule rests, both sides.
    pub fn book(&self, object_id: &str, venue: &str, at: Timestamp) -> Result<OrderBook> {
        let mut bids = Vec::new();
        let mut asks = Vec::new();
        for level in 0..self.levels {
            let offset = self
                .level_step
                .checked_mul(Decimal::from_int(i64::from(level)))
                .and_then(|ladder| ladder.checked_add(self.half_spread))
                .ok_or_else(|| Error::numeric("a ladder level overflowed"))?;
            bids.push(BookLevel::new(
                self.reference_price - offset,
                self.level_size,
            ));
            asks.push(BookLevel::new(
                self.reference_price + offset,
                self.level_size,
            ));
        }
        let book = OrderBook::from_levels(ObjectId::from_string(object_id), venue, at, bids, asks);
        book.validate()?;
        Ok(book)
    }

    /// Whether `mark` is within the tolerance of the reference.
    fn admits_mark(&self, mark: Decimal) -> Result<bool> {
        let moved = (mark - self.reference_price).abs();
        let allowed = self
            .reference_price
            .checked_mul(self.tolerance)
            .ok_or_else(|| Error::numeric("the tolerance band overflowed"))?;
        Ok(moved <= allowed)
    }
}

/// Where a created market has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketStage {
    /// Recorded and its pricing rule published; nothing at the venue.
    Created,
    /// An empty book rests at the venue.
    Listed,
    /// Inventory committed and the rule's ladder rests at the venue.
    Seeded,
    /// Taken off the venue by the validity monitor.
    Withdrawn,
}

impl MarketStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Listed => "listed",
            Self::Seeded => "seeded",
            Self::Withdrawn => "withdrawn",
        }
    }
}

/// One market the factory created.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreatedMarket {
    pub product: String,
    pub venue: String,
    pub jurisdiction: String,
    pub rule: PricingRule,
    pub stage: MarketStage,
    /// Inventory committed at seeding; zero before.
    pub inventory: Decimal,
    /// The evidence each authority's permission rested on, per action taken.
    pub approvals: BTreeMap<String, Vec<String>>,
    pub created_at: Timestamp,
}

impl CreatedMarket {
    /// The actions this market has been taken through, in order.
    fn actions_taken(&self) -> Vec<CreationAction> {
        match self.stage {
            MarketStage::Created => vec![CreationAction::Create],
            MarketStage::Listed => vec![CreationAction::Create, CreationAction::List],
            MarketStage::Seeded | MarketStage::Withdrawn => CreationAction::ALL.to_vec(),
        }
    }
}

/// What the factory did, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactoryAction {
    Created,
    Listed,
    Seeded,
    Flagged,
    Withdrawn,
}

impl FactoryAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Listed => "listed",
            Self::Seeded => "seeded",
            Self::Flagged => "flagged",
            Self::Withdrawn => "withdrawn",
        }
    }
}

/// One step in a market's life, journaled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FactoryRecord {
    pub sequence: u64,
    pub at: Timestamp,
    pub product: String,
    pub venue: String,
    pub action: FactoryAction,
    pub detail: String,
    /// The evidence references the step rested on; empty for a flag or a
    /// withdrawal, which rest on the finding instead.
    pub approvals: Vec<String>,
    /// The rule in force when the step was taken, so a reader of the log
    /// sees what was published without replaying the factory.
    pub rule: PricingRule,
}

/// Why a market stopped being valid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidityFailure {
    /// A permission the market rests on no longer admits it.
    PermissionLapsed {
        action: String,
        missing: Vec<String>,
    },
    /// Nothing marks the instrument, so its published prices are unchecked.
    Unmarked,
    /// The mark has moved past the rule's tolerance from its reference.
    MarkDeviated { mark: Decimal, reference: Decimal },
    /// The seeded book no longer rests at the venue.
    LiquidityGone,
}

impl ValidityFailure {
    pub fn describe(&self) -> String {
        match self {
            Self::PermissionLapsed { action, missing } => format!(
                "the permission to {action} lapsed ({} no longer recorded)",
                missing.join(", ")
            ),
            Self::Unmarked => {
                "no mark was supplied, so the published prices cannot be checked".to_string()
            }
            Self::MarkDeviated { mark, reference } => format!(
                "the mark {mark} moved past the rule's tolerance from the reference {reference}"
            ),
            Self::LiquidityGone => "the seeded book no longer rests at the venue".to_string(),
        }
    }

    /// Legal (`true`) or economic (`false`).
    pub const fn is_legal(&self) -> bool {
        matches!(self, Self::PermissionLapsed { .. })
    }
}

/// One market the monitor withdrew, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidityFinding {
    pub product: String,
    pub failure: ValidityFailure,
}

/// The Market Factory, bound to one simulated venue.
#[derive(Clone, Debug)]
pub struct MarketFactory {
    venue: String,
    markets: BTreeMap<String, CreatedMarket>,
    journal: Vec<FactoryRecord>,
    sequence: u64,
}

impl MarketFactory {
    /// A factory for the venue behind `venue`, refusing one that is not
    /// simulated.
    pub fn for_venue(venue: &dyn ListingVenue) -> Result<Self> {
        refuse_live(venue)?;
        Ok(Self {
            venue: venue.venue_name().to_string(),
            markets: BTreeMap::new(),
            journal: Vec::new(),
            sequence: 0,
        })
    }

    pub fn venue(&self) -> &str {
        &self.venue
    }

    pub fn market(&self, product: &str) -> Option<&CreatedMarket> {
        self.markets.get(product)
    }

    pub fn markets(&self) -> impl Iterator<Item = &CreatedMarket> {
        self.markets.values()
    }

    /// The records written since the last drain, oldest first. Drained so
    /// the factory holds no unbounded history: the event log is the record.
    pub fn drain_journal(&mut self) -> Vec<FactoryRecord> {
        std::mem::take(&mut self.journal)
    }

    /// Create a market and publish its pricing rule.
    pub fn create(
        &mut self,
        admission: &CreationAdmission,
        rule: PricingRule,
        now: Timestamp,
    ) -> Result<()> {
        let request = self.admitted_for(admission, CreationAction::Create)?;
        if self.markets.contains_key(&request.product) {
            return Err(Error::invalid(format!(
                "a market in {} already exists at {}; a market is created once",
                request.product, self.venue
            )));
        }
        if self.markets.len() >= MAX_MARKETS {
            return Err(Error::denied(format!(
                "this factory already holds {MAX_MARKETS} markets, its bound; nothing was created"
            )));
        }
        let approvals = references(admission);
        let market = CreatedMarket {
            product: request.product.clone(),
            venue: request.venue.clone(),
            jurisdiction: request.jurisdiction.clone(),
            rule: rule.clone(),
            stage: MarketStage::Created,
            inventory: Decimal::ZERO,
            approvals: BTreeMap::from([(
                CreationAction::Create.as_str().to_string(),
                approvals.clone(),
            )]),
            created_at: now,
        };
        self.markets.insert(request.product.clone(), market);
        self.record(
            &request.product,
            FactoryAction::Created,
            format!(
                "created and published a pricing rule: reference {}, half-spread {}, {} levels \
                 of {}, tolerance {}",
                rule.reference_price,
                rule.half_spread,
                rule.levels,
                rule.level_size,
                rule.tolerance
            ),
            approvals,
            rule,
            now,
        );
        Ok(())
    }

    /// List a created market at the venue, as an empty book.
    pub fn list(
        &mut self,
        admission: &CreationAdmission,
        venue: &mut dyn ListingVenue,
        now: Timestamp,
    ) -> Result<()> {
        let request = self.admitted_for(admission, CreationAction::List)?;
        self.bound_to(venue)?;
        let market = self.stage_of(&request, MarketStage::Created)?;
        let rule = market.rule.clone();
        venue.list_book(OrderBook::new(
            ObjectId::from_string(request.product.as_str()),
            self.venue.as_str(),
            now,
        ));
        let approvals = references(admission);
        if let Some(market) = self.markets.get_mut(&request.product) {
            market.stage = MarketStage::Listed;
            market
                .approvals
                .insert(CreationAction::List.as_str().to_string(), approvals.clone());
        }
        self.record(
            &request.product,
            FactoryAction::Listed,
            "listed at the venue with an empty book".to_string(),
            approvals,
            rule,
            now,
        );
        Ok(())
    }

    /// Commit `inventory` and rest the rule's ladder on both sides.
    ///
    /// `ladder` is the pre-trade verdict on the ladder this rests, and there
    /// is no way to hold one but [`crate::oms::OrderManager::admit_ladder`],
    /// which runs the desk's own limit check over both sides. Seeding rests
    /// orders at a venue, and until round 3 it was the one route to a venue
    /// that passed no Risk Gate (R12-RISK-4093): the creation workflow
    /// admitted the *market*, and nothing measured the *exposure* its ladder
    /// commits. A verdict for another product, venue or rule is refused.
    pub fn seed(
        &mut self,
        admission: &CreationAdmission,
        ladder: &LadderAdmission,
        inventory: Decimal,
        venue: &mut dyn ListingVenue,
        now: Timestamp,
    ) -> Result<()> {
        let request = self.admitted_for(admission, CreationAction::Seed)?;
        self.bound_to(venue)?;
        let market = self.stage_of(&request, MarketStage::Listed)?;
        let rule = market.rule.clone();
        if ladder.product != request.product || ladder.venue != self.venue || ladder.rule != rule {
            return Err(Error::denied(format!(
                "the risk verdict presented to seed {} at {} was given for {} at {}, or for \
                 another pricing rule; a ladder is rested only on the verdict for that ladder",
                request.product, self.venue, ladder.product, ladder.venue
            )));
        }
        let needed = rule.ask_quantity()?;
        if inventory < needed {
            return Err(Error::denied(format!(
                "seeding {} with {inventory} cannot cover the {needed} its rule rests on the ask \
                 side; commit at least that, or narrow the ladder",
                request.product
            )));
        }
        let book = rule.book(&request.product, &self.venue, now)?;
        venue.list_book(book);
        let approvals = references(admission);
        if let Some(market) = self.markets.get_mut(&request.product) {
            market.stage = MarketStage::Seeded;
            market.inventory = inventory;
            market
                .approvals
                .insert(CreationAction::Seed.as_str().to_string(), approvals.clone());
        }
        self.record(
            &request.product,
            FactoryAction::Seeded,
            format!(
                "seeded with {inventory} of inventory; {} levels of {} rest either side",
                rule.levels, rule.level_size
            ),
            approvals,
            rule,
            now,
        );
        Ok(())
    }

    /// Re-check every live market and withdraw each that is no longer valid.
    ///
    /// `marks` is the latest independent mark per product. A seeded market
    /// with no mark is withdrawn rather than left quoting unchecked prices.
    /// Returns every finding; each is also journaled as a flag and a
    /// withdrawal.
    pub fn review(
        &mut self,
        permissions: &CreationPermissions,
        marks: &BTreeMap<String, Decimal>,
        venue: &mut dyn ListingVenue,
        now: Timestamp,
    ) -> Result<Vec<ValidityFinding>> {
        self.bound_to(venue)?;
        let mut findings = Vec::new();
        let live: Vec<CreatedMarket> = self
            .markets
            .values()
            .filter(|market| market.stage != MarketStage::Withdrawn)
            .cloned()
            .collect();
        for market in live {
            if let Some(failure) = Self::assess(&market, permissions, marks, venue)? {
                venue.withdraw_book(&market.product);
                if let Some(held) = self.markets.get_mut(&market.product) {
                    held.stage = MarketStage::Withdrawn;
                }
                self.record(
                    &market.product,
                    FactoryAction::Flagged,
                    failure.describe(),
                    Vec::new(),
                    market.rule.clone(),
                    now,
                );
                self.record(
                    &market.product,
                    FactoryAction::Withdrawn,
                    format!(
                        "withdrawn from {} because {}",
                        market.venue,
                        failure.describe()
                    ),
                    Vec::new(),
                    market.rule.clone(),
                    now,
                );
                findings.push(ValidityFinding {
                    product: market.product.clone(),
                    failure,
                });
            }
        }
        Ok(findings)
    }

    /// The first reason a market is no longer valid, legal before economic.
    fn assess(
        market: &CreatedMarket,
        permissions: &CreationPermissions,
        marks: &BTreeMap<String, Decimal>,
        venue: &dyn ListingVenue,
    ) -> Result<Option<ValidityFailure>> {
        for action in market.actions_taken() {
            let request = CreationRequest::new(
                action,
                market.venue.as_str(),
                market.product.as_str(),
                market.jurisdiction.as_str(),
            );
            let missing = permissions.missing(&request);
            if !missing.is_empty() {
                return Ok(Some(ValidityFailure::PermissionLapsed {
                    action: action.as_str().to_string(),
                    missing: missing
                        .iter()
                        .map(|authority| authority.as_str().to_string())
                        .collect(),
                }));
            }
        }
        if market.stage != MarketStage::Seeded {
            return Ok(None);
        }
        let Some(mark) = marks.get(&market.product).copied() else {
            return Ok(Some(ValidityFailure::Unmarked));
        };
        if !market.rule.admits_mark(mark)? {
            return Ok(Some(ValidityFailure::MarkDeviated {
                mark,
                reference: market.rule.reference_price,
            }));
        }
        let rests = venue
            .listed_book(&market.product)
            .is_some_and(|book| book.total_depth().is_positive());
        if !rests {
            return Ok(Some(ValidityFailure::LiquidityGone));
        }
        Ok(None)
    }

    /// The admitted request, refused unless it is for `action` at this
    /// factory's venue.
    fn admitted_for(
        &self,
        admission: &CreationAdmission,
        action: CreationAction,
    ) -> Result<CreationRequest> {
        let request = admission.request().clone();
        if request.action != action {
            return Err(Error::denied(format!(
                "an admission to {} was offered to {}; each step needs its own admission",
                request.action.as_str(),
                action.as_str()
            )));
        }
        if request.venue != self.venue {
            return Err(Error::denied(format!(
                "the admission names venue {}, and this factory creates markets only at the \
                 simulated venue {}; no market is created on any other venue (ADR 0003, ADR 0021)",
                request.venue, self.venue
            )));
        }
        Ok(request)
    }

    /// Refuse a surface that is not this factory's simulated venue.
    fn bound_to(&self, venue: &dyn ListingVenue) -> Result<()> {
        refuse_live(venue)?;
        if venue.venue_name() != self.venue {
            return Err(Error::denied(format!(
                "this factory is bound to {} and was handed {}; a market is listed only where it \
                 was admitted",
                self.venue,
                venue.venue_name()
            )));
        }
        Ok(())
    }

    /// The market, refused unless it stands at `stage` and matches the
    /// admitted request's jurisdiction.
    fn stage_of(&self, request: &CreationRequest, stage: MarketStage) -> Result<&CreatedMarket> {
        let market = self.markets.get(&request.product).ok_or_else(|| {
            Error::not_found(format!(
                "no market in {} has been created at {}; create it first",
                request.product, self.venue
            ))
        })?;
        if market.jurisdiction != request.jurisdiction {
            return Err(Error::denied(format!(
                "the market in {} was created for {} and the admission is for {}",
                request.product, market.jurisdiction, request.jurisdiction
            )));
        }
        if market.stage != stage {
            return Err(Error::invalid(format!(
                "the market in {} is {}, and this step needs it {}",
                request.product,
                market.stage.as_str(),
                stage.as_str()
            )));
        }
        Ok(market)
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        product: &str,
        action: FactoryAction,
        detail: String,
        approvals: Vec<String>,
        rule: PricingRule,
        at: Timestamp,
    ) {
        self.sequence += 1;
        self.journal.push(FactoryRecord {
            sequence: self.sequence,
            at,
            product: product.to_string(),
            venue: self.venue.clone(),
            action,
            detail,
            approvals,
            rule,
        });
    }
}

fn refuse_live(venue: &dyn ListingVenue) -> Result<()> {
    if !venue.is_simulated() {
        return Err(Error::denied(format!(
            "venue {} is not simulated, and the Market Factory creates, lists and seeds markets \
             on the simulated venue only (ADR 0003, ADR 0021); nothing was sent to it",
            venue.venue_name()
        )));
    }
    Ok(())
}

fn references(admission: &CreationAdmission) -> Vec<String> {
    admission
        .references()
        .iter()
        .map(|(authority, reference)| format!("{}: {reference}", authority.as_str()))
        .collect()
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use crate::broker::{SimulatedBroker, SimulationSettings};
    use crate::creation::CreationAuthority;

    const VENUE: &str = "simulated-venue";
    const PRODUCT: &str = "TEST/USD";
    const PLACE: &str = "GB";

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn d(text: &str) -> Decimal {
        Decimal::parse(text).unwrap_or(Decimal::ZERO)
    }

    fn rule() -> Result<PricingRule> {
        PricingRule::new(d("100"), d("0.5"), d("0.25"), d("10"), 3, d("0.05"))
    }

    /// The risk verdict on `rule`'s ladder under `limits`, minted the only
    /// way one can be: by the order manager's check, against a book of a
    /// million.
    fn ladder_under(
        limits: qip_risk::limits::LimitSet,
        rule: &PricingRule,
    ) -> Result<LadderAdmission> {
        let mut manager =
            crate::oms::OrderManager::new(qip_risk_engine::pretrade::PreTradeChecker::new(limits));
        let state = qip_risk::limits::RiskState {
            equity: d("1000000"),
            cash: d("1000000"),
            ..qip_risk::limits::RiskState::default()
        };
        manager.admit_ladder(
            PRODUCT,
            VENUE,
            rule,
            "market-factory",
            &qip_risk_engine::autonomy::AutonomyController::new(),
            &state,
            at(),
        )
    }

    /// `rule()`'s ladder under a 2x leverage limit it is far inside.
    fn ladder() -> Result<LadderAdmission> {
        use qip_risk::limits::{Limit, LimitKind, LimitSet};
        ladder_under(
            LimitSet::new("factory-test").with(Limit::new(
                "leverage",
                LimitKind::MaxLeverage { limit: 2.0 },
            )),
            &rule()?,
        )
    }

    fn notional_cap(limit: &str) -> qip_risk::limits::LimitSet {
        use qip_risk::limits::{Limit, LimitKind, LimitSet};
        LimitSet::new("factory-test").with(Limit::new(
            "order-notional",
            LimitKind::MaxOrderNotional { limit: d(limit) },
        ))
    }

    /// R12-RISK-4093: a seeded ladder is orders at a venue, and it is
    /// measured against the desk's limits like one. `rule()` rests 30 a side
    /// with a best bid of 99.5 and a deepest ask of 101: some 3,000 of
    /// notional each way. A per-order cap of 2,000 refuses it, naming the
    /// side; a cap of 5,000 admits it, so the refusal is the cap and not the
    /// mint refusing everything.
    /// Mutation: make `admit_ladder` skip its check loop and the first
    /// `expect_err` fails.
    #[test]
    fn a_ladder_past_a_desk_limit_gets_no_verdict_to_seed_on() -> Result<()> {
        assert!(ladder_under(notional_cap("5000"), &rule()?).is_ok());
        let refused = ladder_under(notional_cap("2000"), &rule()?)
            .expect_err("a ladder past the per-order cap must not be admitted");
        assert!(
            refused.message().contains("the bid side of the ladder"),
            "the refusal names the side that breached: {}",
            refused.message()
        );
        Ok(())
    }

    /// A verdict is for one ladder: one measured for a different rule does
    /// not seed this market, and nothing rests.
    /// Mutation: drop the `ladder.rule != rule` comparison from `seed` and
    /// the seed succeeds.
    #[test]
    fn a_verdict_for_another_rule_does_not_seed_the_market() -> Result<()> {
        let permissions = permitted()?;
        let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 7);
        let mut factory = MarketFactory::for_venue(&broker)?;
        factory.create(&admit(&permissions, CreationAction::Create)?, rule()?, at())?;
        factory.list(
            &admit(&permissions, CreationAction::List)?,
            &mut broker,
            at(),
        )?;
        let narrower = PricingRule::new(d("100"), d("0.5"), d("0.25"), d("1"), 1, d("0.05"))?;
        let other = ladder_under(notional_cap("5000"), &narrower)?;
        assert!(
            factory
                .seed(
                    &admit(&permissions, CreationAction::Seed)?,
                    &other,
                    d("30"),
                    &mut broker,
                    at(),
                )
                .is_err(),
            "a ladder was seeded on a verdict given for another rule"
        );
        assert!(broker.listed_book(PRODUCT).is_some_and(OrderBook::is_empty));
        // Contrast: the verdict for this rule seeds it.
        factory.seed(
            &admit(&permissions, CreationAction::Seed)?,
            &ladder()?,
            d("30"),
            &mut broker,
            at(),
        )?;
        Ok(())
    }

    fn permitted() -> Result<CreationPermissions> {
        let mut permissions = CreationPermissions::new();
        for action in CreationAction::ALL {
            let request = CreationRequest::new(action, VENUE, PRODUCT, PLACE);
            for authority in CreationAuthority::ALL {
                permissions.record(authority, &request, "rulebook 4.2")?;
            }
        }
        Ok(permissions)
    }

    fn admit(
        permissions: &CreationPermissions,
        action: CreationAction,
    ) -> Result<CreationAdmission> {
        permissions.admit(&CreationRequest::new(action, VENUE, PRODUCT, PLACE))
    }

    /// The whole factory, end to end: an approved creation lists the
    /// instrument at the simulated venue, seeds resting liquidity on both
    /// sides, and publishes its pricing rule in the journal.
    ///
    /// Mutation: make `seed` skip `venue.list_book(book)` and the venue's book
    /// is the empty one `list` left — the depth assertion fails.
    #[test]
    fn an_approved_creation_lists_seeds_and_publishes_its_rule_at_the_simulated_venue() -> Result<()>
    {
        let permissions = permitted()?;
        let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 7);
        let mut factory = MarketFactory::for_venue(&broker)?;
        factory.create(&admit(&permissions, CreationAction::Create)?, rule()?, at())?;
        // Premise: nothing rests before listing and seeding.
        assert!(broker.listed_book(PRODUCT).is_none());
        factory.list(
            &admit(&permissions, CreationAction::List)?,
            &mut broker,
            at(),
        )?;
        assert!(broker.listed_book(PRODUCT).is_some_and(OrderBook::is_empty));
        factory.seed(
            &admit(&permissions, CreationAction::Seed)?,
            &ladder()?,
            d("30"),
            &mut broker,
            at(),
        )?;
        let book = broker
            .listed_book(PRODUCT)
            .ok_or_else(|| Error::invalid("no book after seeding"))?;
        assert_eq!(book.best_bid().map(|level| level.price), Some(d("99.5")));
        assert_eq!(book.best_ask().map(|level| level.price), Some(d("100.5")));
        assert_eq!(book.total_depth(), d("60"));
        let journal = factory.drain_journal();
        let actions: Vec<FactoryAction> = journal.iter().map(|record| record.action).collect();
        assert_eq!(
            actions,
            vec![
                FactoryAction::Created,
                FactoryAction::Listed,
                FactoryAction::Seeded
            ]
        );
        assert_eq!(journal[0].rule.reference_price(), d("100"));
        assert!(
            journal[0]
                .approvals
                .iter()
                .any(|a| a.contains("rulebook 4.2"))
        );
        assert_eq!(
            factory.market(PRODUCT).map(|market| market.stage),
            Some(MarketStage::Seeded)
        );
        Ok(())
    }

    /// COVERAGE-5107: without all three permissions there is no admission,
    /// and with an admission for another venue the factory refuses.
    /// Mutation: drop the venue check in `admitted_for` and the second
    /// creation succeeds.
    #[test]
    fn a_creation_without_every_permission_or_for_another_venue_is_refused() -> Result<()> {
        let mut permissions = CreationPermissions::new();
        let request = CreationRequest::new(CreationAction::Create, VENUE, PRODUCT, PLACE);
        permissions.record(CreationAuthority::Venue, &request, "rulebook 4.2")?;
        permissions.record(CreationAuthority::Product, &request, "product approval 9")?;
        assert!(
            permissions.admit(&request).is_err(),
            "jurisdiction is missing"
        );

        let elsewhere = CreationRequest::new(CreationAction::Create, "XNYS", PRODUCT, PLACE);
        for authority in CreationAuthority::ALL {
            permissions.record(authority, &elsewhere, "opinion 3")?;
        }
        let broker = SimulatedBroker::new(SimulationSettings::frictionless(), 7);
        let mut factory = MarketFactory::for_venue(&broker)?;
        let refused = factory
            .create(&permissions.admit(&elsewhere)?, rule()?, at())
            .expect_err("a market admitted for a real venue was created at the simulator");
        assert!(refused.message().contains("only at the simulated venue"));
        assert!(factory.market(PRODUCT).is_none());
        Ok(())
    }

    /// EXEC-5081: a market whose permission lapses is flagged and withdrawn,
    /// and so is one whose mark moves past tolerance; a market still valid
    /// stays. Mutation: make `review` skip the withdrawal and the book still
    /// rests; skip `assess`'s permission loop and the lapsed market stays.
    #[test]
    fn a_market_whose_permission_lapses_or_whose_economics_fail_is_flagged_and_withdrawn()
    -> Result<()> {
        let permissions = permitted()?;
        let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 7);
        let mut factory = MarketFactory::for_venue(&broker)?;
        factory.create(&admit(&permissions, CreationAction::Create)?, rule()?, at())?;
        factory.list(
            &admit(&permissions, CreationAction::List)?,
            &mut broker,
            at(),
        )?;
        factory.seed(
            &admit(&permissions, CreationAction::Seed)?,
            &ladder()?,
            d("30"),
            &mut broker,
            at(),
        )?;
        let marks = BTreeMap::from([(PRODUCT.to_string(), d("101"))]);

        // Premise: valid under every permission and a mark inside tolerance.
        assert!(
            factory
                .review(&permissions, &marks, &mut broker, at())?
                .is_empty()
        );
        assert!(broker.listed_book(PRODUCT).is_some());

        // The mark moves 10% against a 5% tolerance: economics fail.
        let moved = BTreeMap::from([(PRODUCT.to_string(), d("110"))]);
        let findings = factory.review(&permissions, &moved, &mut broker, at())?;
        assert_eq!(findings.len(), 1);
        assert!(matches!(
            findings[0].failure,
            ValidityFailure::MarkDeviated { .. }
        ));
        assert!(
            broker.listed_book(PRODUCT).is_none(),
            "the book still rests"
        );
        assert_eq!(
            factory.market(PRODUCT).map(|market| market.stage),
            Some(MarketStage::Withdrawn)
        );

        // A second market whose jurisdiction permission is then lapsed.
        let mut second = SimulatedBroker::new(SimulationSettings::frictionless(), 8);
        let mut factory = MarketFactory::for_venue(&second)?;
        factory.create(&admit(&permissions, CreationAction::Create)?, rule()?, at())?;
        factory.list(
            &admit(&permissions, CreationAction::List)?,
            &mut second,
            at(),
        )?;
        let lapsed = CreationPermissions::new();
        let findings = factory.review(&lapsed, &marks, &mut second, at())?;
        assert!(findings.iter().any(|finding| finding.failure.is_legal()));
        assert!(second.listed_book(PRODUCT).is_none());
        let journal = factory.drain_journal();
        assert!(
            journal
                .iter()
                .any(|record| record.action == FactoryAction::Withdrawn)
        );
        Ok(())
    }

    #[test]
    fn seeding_with_inventory_that_cannot_cover_the_ask_side_is_refused() -> Result<()> {
        let permissions = permitted()?;
        let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 7);
        let mut factory = MarketFactory::for_venue(&broker)?;
        factory.create(&admit(&permissions, CreationAction::Create)?, rule()?, at())?;
        factory.list(
            &admit(&permissions, CreationAction::List)?,
            &mut broker,
            at(),
        )?;
        // Premise: the rule rests 30 on the ask side.
        assert_eq!(rule()?.ask_quantity()?, d("30"));
        assert!(
            factory
                .seed(
                    &admit(&permissions, CreationAction::Seed)?,
                    &ladder()?,
                    d("29"),
                    &mut broker,
                    at()
                )
                .is_err()
        );
        assert!(broker.listed_book(PRODUCT).is_some_and(OrderBook::is_empty));
        Ok(())
    }
}
