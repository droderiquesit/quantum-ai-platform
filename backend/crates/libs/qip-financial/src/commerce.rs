//! The simulated Commerce & Physical Operations plane (blueprint COMMERCE).
//!
//! A price model can value a physical product but cannot find, buy, move,
//! hold or resell one. This module is the paper-only pipeline around
//! [`crate::physical`]'s landed-cost arithmetic: Product Scout, the SKU
//! resolver, feasibility, purchase controls, the logistics plan, the
//! event-sourced inventory ledger and marketplace settlement with returns.
//!
//! **Paper only, structurally.** The only merchant is [`SimulatedMerchant`]
//! and [`PurchaseExecutor`] holds that concrete type, not a trait, so a real
//! merchant adapter cannot be slotted in without changing this file (ADR
//! 0021, ADR 0099). Nothing here performs I/O.

use crate::physical::{Customs, LandedCost, Leg, LogisticsTerms, MarketplaceFees};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

fn mul(what: &str, a: Decimal, b: Decimal) -> Result<Decimal> {
    a.checked_mul(b)
        .ok_or_else(|| Error::numeric(format!("{what} overflowed")))
}

fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

// ---------------------------------------------------------------- Scout

/// Where an observation was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Channel {
    Retail,
    Auction,
    Liquidation,
    Wholesale,
}

/// The five things Product Scout looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum OpportunityKind {
    PriceGap,
    Auction,
    Liquidation,
    Wholesale,
    SupplyShock,
}

/// One price and availability reading for a product at a venue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub product: String,
    pub venue: String,
    pub channel: Channel,
    pub price: Decimal,
    /// What the product ordinarily trades for; a reading at or above it is
    /// not an opportunity.
    pub reference_price: Decimal,
    pub units_available: u32,
    pub units_previously_available: u32,
}

/// A typed opportunity, to be resolved and assessed downstream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub product: String,
    pub venue: String,
    pub kind: OpportunityKind,
    pub price: Decimal,
    pub reference_price: Decimal,
}

/// Emit one candidate per observation that shows a difference.
///
/// A non-retail channel is an opportunity only when it is priced under the
/// reference; a retail reading is a supply shock when availability halved or
/// worse, otherwise a price gap when under the reference. Anything else
/// emits nothing, so a quiet market is not reported as a find.
pub fn scout(observations: &[Observation]) -> Vec<Candidate> {
    observations
        .iter()
        .filter_map(|o| {
            let discounted = o.price < o.reference_price;
            let kind = match o.channel {
                Channel::Auction if discounted => OpportunityKind::Auction,
                Channel::Liquidation if discounted => OpportunityKind::Liquidation,
                Channel::Wholesale if discounted => OpportunityKind::Wholesale,
                Channel::Retail
                    if o.units_previously_available > 0
                        && u64::from(o.units_available) * 2
                            <= u64::from(o.units_previously_available) =>
                {
                    OpportunityKind::SupplyShock
                }
                Channel::Retail if discounted => OpportunityKind::PriceGap,
                _ => return None,
            };
            Some(Candidate {
                product: o.product.clone(),
                venue: o.venue.clone(),
                kind,
                price: o.price,
                reference_price: o.reference_price,
            })
        })
        .collect()
}

// ------------------------------------------------------------- Resolver

/// One retailer, marketplace or regional listing of something.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    /// Unique per listing; carries the retailer/marketplace/region identity.
    pub id: String,
    pub brand: String,
    pub model: String,
    /// A variant (size, colour, capacity) is a different product.
    pub variant: String,
    pub gtin: Option<String>,
    pub seller: String,
}

/// What a listing resolved to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    /// Why the listing is held to be counterfeit; never a confidence.
    pub counterfeit_finding: Option<String>,
}

/// Maps equivalent listings to one identity and keeps variants and
/// counterfeits out of it.
#[derive(Clone, Debug, Default)]
pub struct Resolver {
    counterfeit_gtins: BTreeSet<String>,
    counterfeit_sellers: BTreeSet<String>,
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

impl Resolver {
    /// Registry entries are matched after normalisation (case, spacing).
    pub fn new(counterfeit_gtins: BTreeSet<String>, counterfeit_sellers: BTreeSet<String>) -> Self {
        Self {
            counterfeit_gtins: counterfeit_gtins.iter().map(|s| norm(s)).collect(),
            counterfeit_sellers: counterfeit_sellers.iter().map(|s| norm(s)).collect(),
        }
    }

    fn key(l: &Listing) -> String {
        format!("{}|{}|{}", norm(&l.brand), norm(&l.model), norm(&l.variant))
    }

    fn finding(&self, l: &Listing) -> Option<String> {
        if let Some(g) = &l.gtin
            && self.counterfeit_gtins.contains(&norm(g))
        {
            return Some(format!("GTIN {g} is on the counterfeit registry"));
        }
        if self.counterfeit_sellers.contains(&norm(&l.seller)) {
            return Some(format!(
                "seller {} is on the counterfeit registry",
                l.seller
            ));
        }
        None
    }

    /// Resolve every listing. Equivalence is "same normalised
    /// brand/model/variant or same GTIN", closed transitively, so the result
    /// is order-independent. A GTIN that links two different variants is
    /// refused: one of the two feeds is wrong and guessing which would
    /// merge a variant into the genuine product.
    pub fn resolve(&self, listings: &[Listing]) -> Result<BTreeMap<String, Identity>> {
        let mut out = BTreeMap::new();
        let mut genuine: Vec<&Listing> = Vec::new();
        for l in listings {
            match self.finding(l) {
                Some(f) => {
                    out.insert(
                        l.id.clone(),
                        Identity {
                            id: format!("counterfeit:{}", Self::key(l)),
                            counterfeit_finding: Some(f),
                        },
                    );
                }
                None => genuine.push(l),
            }
        }
        let mut parent: Vec<usize> = (0..genuine.len()).collect();
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for (i, l) in genuine.iter().enumerate() {
            let mut tokens = vec![format!("key:{}", Self::key(l))];
            if let Some(g) = &l.gtin {
                tokens.push(format!("gtin:{}", norm(g)));
            }
            for t in tokens {
                if let Some(&j) = seen.get(&t) {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    parent[a.max(b)] = a.min(b);
                } else {
                    seen.insert(t, i);
                }
            }
        }
        let mut keys: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
        for (i, l) in genuine.iter().enumerate() {
            let root = find(&mut parent, i);
            keys.entry(root).or_default().insert(Self::key(l));
        }
        if let Some(k) = keys.values().find(|k| k.len() > 1) {
            return Err(Error::invalid(format!(
                "a GTIN links listings of different variants ({}); correct the feed that is wrong \
                 rather than merging a variant into the genuine product",
                k.iter().cloned().collect::<Vec<_>>().join(" / ")
            )));
        }
        for (i, l) in genuine.iter().enumerate() {
            let root = find(&mut parent, i);
            let key = keys
                .get(&root)
                .and_then(|k| k.iter().next())
                .cloned()
                .unwrap_or_default();
            out.insert(
                l.id.clone(),
                Identity {
                    id: format!("product:{key}"),
                    counterfeit_finding: None,
                },
            );
        }
        Ok(out)
    }
}

// ---------------------------------------------------------- Feasibility

/// Everything feasibility needs. `Option` fields are the ones a caller can
/// forget; each one missing is a refusal naming it, never a figure omitted.
#[derive(Clone, Debug)]
pub struct FeasibilityInput {
    pub unit_price: Option<Decimal>,
    pub quantity: Option<Decimal>,
    pub declared_value_per_unit: Option<Decimal>,
    pub terms: Option<LogisticsTerms>,
    /// `(resale price, probability)`; probabilities must sum to one.
    pub resale: Vec<(Decimal, Decimal)>,
    pub days_on_market: Option<u32>,
    /// Cost of capital per day, as a fraction.
    pub daily_capital_cost: Option<Decimal>,
    /// Tax on the purchase (for example import VAT), as a fraction of goods.
    pub purchase_tax_rate: Option<Decimal>,
}

/// The six figures feasibility owes, plus the parts that explain them.
#[derive(Clone, Debug, PartialEq)]
pub struct Assessment {
    pub landed: LandedCost,
    pub purchase_tax: Decimal,
    /// Landed total plus purchase tax.
    pub all_in_cost: Decimal,
    pub expected_resale: Decimal,
    pub time_to_sale_days: u32,
    pub return_risk: Decimal,
    pub inventory_carry: Decimal,
    pub capital_lockup: Decimal,
}

fn need<T: Clone>(v: &Option<T>, what: &str) -> Result<T> {
    v.clone().ok_or_else(|| {
        Error::invalid(format!(
            "feasibility needs {what}; supply it rather than assessing without it"
        ))
    })
}

impl FeasibilityInput {
    pub fn assess(&self) -> Result<Assessment> {
        let unit_price = need(&self.unit_price, "the unit price")?;
        let quantity = need(&self.quantity, "the quantity")?;
        let declared = need(&self.declared_value_per_unit, "the declared value per unit")?;
        let terms = need(&self.terms, "logistics terms")?;
        let days = need(&self.days_on_market, "the expected days on market")?;
        let capital = need(&self.daily_capital_cost, "the daily cost of capital")?;
        let tax_rate = need(&self.purchase_tax_rate, "the purchase tax rate")?;
        if self.resale.is_empty() {
            return Err(Error::invalid(
                "feasibility needs a resale price distribution; supply it rather than assessing without it",
            ));
        }
        let total_p: Decimal = self.resale.iter().map(|(_, p)| *p).sum();
        if total_p != Decimal::ONE
            || self
                .resale
                .iter()
                .any(|(p, q)| p.is_negative() || q.is_negative())
        {
            return Err(Error::invalid(
                "the resale distribution must have non-negative entries and probabilities summing to one",
            ));
        }
        let mut expected_price = Decimal::ZERO;
        for (price, p) in &self.resale {
            expected_price += mul("the expected price", *price, *p)?;
        }
        let arb = terms.arbitrage(quantity, unit_price, declared, expected_price)?;
        let landed = arb.cost;
        let purchase_tax = mul("the purchase tax", landed.goods, tax_rate)?;
        let time_to_sale_days = landed.elapsed_days + days;
        let capital_lockup = landed.goods
            + landed.freight
            + landed.duty
            + landed.clearance
            + landed.storage
            + purchase_tax;
        let carry = mul("the capital carry", capital_lockup, capital)?;
        let inventory_carry = landed.storage
            + mul(
                "the capital carry",
                carry,
                Decimal::from(i64::from(time_to_sale_days)),
            )?;
        Ok(Assessment {
            all_in_cost: landed.total() + purchase_tax,
            expected_resale: arb.proceeds,
            time_to_sale_days,
            return_risk: terms.returns.rate,
            inventory_carry,
            capital_lockup,
            purchase_tax,
            landed,
        })
    }
}

// ------------------------------------------------------ Purchase controls

/// An account the platform buys through, and the authority it was granted.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub id: String,
    pub identity_class: String,
    pub merchants: BTreeSet<String>,
    pub per_purchase_limit: Decimal,
}

/// The credential presented with a purchase.
#[derive(Clone, Debug, PartialEq)]
pub struct Credential {
    pub account_id: String,
    pub identity_class: String,
}

/// A request to buy, before any purchase exists.
#[derive(Clone, Debug, PartialEq)]
pub struct PurchaseRequest {
    pub account_id: String,
    pub credential: Credential,
    pub merchant: String,
    pub listing: String,
    pub payment: String,
    pub sku: String,
    pub quantity: Decimal,
    pub unit_price: Decimal,
}

/// A purchase the simulated merchant acknowledged.
#[derive(Clone, Debug, PartialEq)]
pub struct PurchaseRecord {
    pub sequence: u32,
    pub account_id: String,
    pub merchant: String,
    pub sku: String,
    pub quantity: Decimal,
    pub notional: Decimal,
}

/// The only merchant there is: it acknowledges what it is given and counts
/// the calls, so a test can prove a refusal reached nobody.
#[derive(Clone, Debug, Default)]
pub struct SimulatedMerchant {
    calls: u32,
}

impl SimulatedMerchant {
    pub fn calls(&self) -> u32 {
        self.calls
    }

    fn place(
        &mut self,
        account_id: &str,
        r: &PurchaseRequest,
        notional: Decimal,
    ) -> PurchaseRecord {
        self.calls += 1;
        PurchaseRecord {
            sequence: self.calls,
            account_id: account_id.to_string(),
            merchant: r.merchant.clone(),
            sku: r.sku.clone(),
            quantity: r.quantity,
            notional,
        }
    }
}

/// Merchants, listings and payment instruments already found fraudulent.
#[derive(Clone, Debug, Default)]
pub struct FraudBook {
    pub merchants: BTreeSet<String>,
    pub listings: BTreeSet<String>,
    pub payments: BTreeSet<String>,
}

/// Account controls, fraud safeguards and limits, in front of the merchant.
#[derive(Clone, Debug)]
pub struct PurchaseExecutor {
    accounts: BTreeMap<String, Account>,
    fraud: FraudBook,
    total_ceiling: Decimal,
    committed: Decimal,
    merchant: SimulatedMerchant,
    records: Vec<PurchaseRecord>,
}

impl PurchaseExecutor {
    pub fn new(accounts: Vec<Account>, fraud: FraudBook, total_ceiling: Decimal) -> Self {
        Self {
            accounts: accounts.into_iter().map(|a| (a.id.clone(), a)).collect(),
            fraud,
            total_ceiling,
            committed: Decimal::ZERO,
            merchant: SimulatedMerchant::default(),
            records: Vec::new(),
        }
    }

    pub fn merchant_calls(&self) -> u32 {
        self.merchant.calls()
    }

    pub fn records(&self) -> &[PurchaseRecord] {
        &self.records
    }

    /// Account authority, then fraud, then limits, then — only if all
    /// three admit — the merchant. Every refusal precedes any purchase
    /// record and any merchant call.
    pub fn purchase(&mut self, r: &PurchaseRequest) -> Result<PurchaseRecord> {
        if !r.quantity.is_positive() || !r.unit_price.is_positive() {
            return Err(Error::invalid(
                "a purchase needs a positive quantity and a positive unit price",
            ));
        }
        let account = self.accounts.get(&r.account_id).ok_or_else(|| {
            Error::denied(format!(
                "account {} is not controlled here; register it first",
                r.account_id
            ))
        })?;
        if r.credential.account_id != account.id
            || r.credential.identity_class != account.identity_class
        {
            return Err(Error::denied(format!(
                "the credential does not belong to account {} and its identity class; present the \
                 account's own credential",
                account.id
            )));
        }
        if !account.merchants.contains(&r.merchant) {
            return Err(Error::denied(format!(
                "account {} has no authority to buy from {}",
                account.id, r.merchant
            )));
        }
        if self.fraud.merchants.contains(&r.merchant)
            || self.fraud.listings.contains(&r.listing)
            || self.fraud.payments.contains(&r.payment)
        {
            return Err(Error::denied(format!(
                "merchant {}, listing {} or payment {} carries a fraud finding; no payment is committed",
                r.merchant, r.listing, r.payment
            )));
        }
        let notional = mul("the notional", r.quantity, r.unit_price)?;
        if notional > account.per_purchase_limit {
            return Err(Error::denied(format!(
                "a notional of {notional} exceeds account {}'s per-purchase limit of {}",
                account.id, account.per_purchase_limit
            )));
        }
        if self.committed + notional > self.total_ceiling {
            return Err(Error::denied(format!(
                "{notional} on top of {} committed exceeds the total ceiling of {}",
                self.committed, self.total_ceiling
            )));
        }
        let account_id = account.id.clone();
        let record = self.merchant.place(&account_id, r, notional);
        self.committed += notional;
        self.records.push(record.clone());
        Ok(record)
    }
}

// ------------------------------------------------------------ Logistics

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Consolidation {
    Direct,
    Consolidate { with_shipments: u32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Insurance {
    Insured { coverage: Decimal, premium: Decimal },
    Declined { reason: String },
}

/// A plan being assembled; every part is optional until it is checked.
#[derive(Clone, Debug, Default)]
pub struct LogisticsPlanDraft {
    pub route: Vec<Leg>,
    pub carrier: Option<String>,
    pub consolidation: Option<Consolidation>,
    pub warehouse: Option<String>,
    pub customs: Option<Customs>,
    pub insurance: Option<Insurance>,
    pub delivery_risk: Option<Decimal>,
}

/// A plan with all seven parts. Only [`LogisticsPlanDraft::into_plan`]
/// builds one.
#[derive(Clone, Debug, PartialEq)]
pub struct LogisticsPlan {
    pub route: Vec<Leg>,
    pub carrier: String,
    pub consolidation: Consolidation,
    pub warehouse: String,
    pub customs: Customs,
    pub insurance: Insurance,
    pub delivery_risk: Decimal,
}

impl LogisticsPlanDraft {
    pub fn into_plan(self) -> Result<LogisticsPlan> {
        let blank = |s: &Option<String>| s.as_deref().is_none_or(|v| v.trim().is_empty());
        let mut missing = Vec::new();
        if self.route.is_empty() {
            missing.push("route");
        }
        if blank(&self.carrier) {
            missing.push("carrier");
        }
        if self.consolidation.is_none() {
            missing.push("consolidation decision");
        }
        if blank(&self.warehouse) {
            missing.push("warehouse");
        }
        if self.customs.is_none() {
            missing.push("customs step");
        }
        if self.insurance.is_none() {
            missing.push("insurance decision");
        }
        if self.delivery_risk.is_none() {
            missing.push("delivery-risk estimate");
        }
        if !missing.is_empty() {
            return Err(Error::invalid(format!(
                "the logistics plan is missing: {}; state each rather than executing a partial plan",
                missing.join(", ")
            )));
        }
        match (
            self.carrier,
            self.consolidation,
            self.warehouse,
            self.customs,
            self.insurance,
            self.delivery_risk,
        ) {
            (
                Some(carrier),
                Some(consolidation),
                Some(warehouse),
                Some(customs),
                Some(insurance),
                Some(delivery_risk),
            ) => {
                if delivery_risk.is_negative() || delivery_risk > Decimal::ONE {
                    return Err(Error::invalid("a delivery risk is a probability in [0, 1]"));
                }
                Ok(LogisticsPlan {
                    route: self.route,
                    carrier,
                    consolidation,
                    warehouse,
                    customs,
                    insurance,
                    delivery_risk,
                })
            }
            _ => Err(Error::invalid("the logistics plan is incomplete")),
        }
    }
}

// --------------------------------------------------------------- Ledger

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Condition {
    New,
    OpenBox,
    Used,
    Damaged,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitState {
    Held,
    Reserved { reservation: String },
    Sold { sale: String },
}

/// What the ledger believes about one physical unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnitRecord {
    pub unit: String,
    pub sku: String,
    pub condition: Condition,
    pub location: String,
    pub cost_basis: Decimal,
    pub owner: String,
    pub state: UnitState,
}

/// The facts the ledger is built from; state is only ever their replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LedgerEvent {
    Received {
        unit: String,
        sku: String,
        condition: Condition,
        location: String,
        cost_basis: Decimal,
        owner: String,
    },
    Moved {
        unit: String,
        to: String,
    },
    Reserved {
        unit: String,
        reservation: String,
    },
    Released {
        unit: String,
        reservation: String,
    },
    Sold {
        unit: String,
        reservation: Option<String>,
        sale: String,
        buyer: String,
    },
    Returned {
        unit: String,
        sale: String,
        condition: Condition,
        location: String,
        owner: String,
    },
}

/// A return, linked to the sale it reverses.
#[derive(Clone, Debug, PartialEq)]
pub struct ReturnRecord {
    pub unit: String,
    pub sale: String,
    pub condition: Condition,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ledger {
    units: BTreeMap<String, UnitRecord>,
    events: Vec<LedgerEvent>,
    returns: Vec<ReturnRecord>,
}

impl Ledger {
    pub fn units(&self) -> &BTreeMap<String, UnitRecord> {
        &self.units
    }

    pub fn events(&self) -> &[LedgerEvent] {
        &self.events
    }

    pub fn returns(&self) -> &[ReturnRecord] {
        &self.returns
    }

    /// Rebuild a ledger from its events; the same refusals apply.
    pub fn replay(events: &[LedgerEvent]) -> Result<Self> {
        let mut l = Self::default();
        for e in events {
            l.apply(e.clone())?;
        }
        Ok(l)
    }

    fn unit_mut(&mut self, unit: &str) -> Result<&mut UnitRecord> {
        self.units.get_mut(unit).ok_or_else(|| {
            Error::not_found(format!(
                "unit {unit} is not in the ledger; receive it first"
            ))
        })
    }

    /// Validate, then mutate. A refused event leaves the ledger exactly as it
    /// was, because every check precedes the first write.
    pub fn apply(&mut self, event: LedgerEvent) -> Result<()> {
        match &event {
            LedgerEvent::Received {
                unit,
                sku,
                condition,
                location,
                cost_basis,
                owner,
            } => {
                if self.units.contains_key(unit) {
                    return Err(Error::invalid(format!(
                        "unit {unit} is already in the ledger"
                    )));
                }
                if [unit, sku, owner, location]
                    .iter()
                    .any(|s| s.trim().is_empty())
                {
                    return Err(Error::invalid(
                        "a unit needs an id, SKU, location and owner",
                    ));
                }
                if cost_basis.is_negative() {
                    return Err(Error::invalid("a cost basis cannot be negative"));
                }
                self.units.insert(
                    unit.clone(),
                    UnitRecord {
                        unit: unit.clone(),
                        sku: sku.clone(),
                        condition: *condition,
                        location: location.clone(),
                        cost_basis: *cost_basis,
                        owner: owner.clone(),
                        state: UnitState::Held,
                    },
                );
            }
            LedgerEvent::Moved { unit, to } => {
                let u = self.unit_mut(unit)?;
                if matches!(u.state, UnitState::Sold { .. }) {
                    return Err(Error::invalid(format!(
                        "unit {unit} is sold; it is no longer ours to move"
                    )));
                }
                u.location = to.clone();
            }
            LedgerEvent::Reserved { unit, reservation } => {
                let u = self.unit_mut(unit)?;
                if u.state != UnitState::Held {
                    return Err(Error::invalid(format!(
                        "unit {unit} is not free ({:?}); release or wait before reserving it again",
                        u.state
                    )));
                }
                u.state = UnitState::Reserved {
                    reservation: reservation.clone(),
                };
            }
            LedgerEvent::Released { unit, reservation } => {
                let u = self.unit_mut(unit)?;
                if u.state
                    != (UnitState::Reserved {
                        reservation: reservation.clone(),
                    })
                {
                    return Err(Error::invalid(format!(
                        "unit {unit} is not reserved under {reservation}; only the holder releases it"
                    )));
                }
                u.state = UnitState::Held;
            }
            LedgerEvent::Sold {
                unit,
                reservation,
                sale,
                buyer,
            } => {
                let u = self.unit_mut(unit)?;
                match (&u.state, reservation) {
                    (UnitState::Held, None) => {}
                    (UnitState::Reserved { reservation: held }, Some(r)) if held == r => {}
                    (state, _) => {
                        return Err(Error::invalid(format!(
                            "unit {unit} cannot be sold from {state:?} with reservation \
                             {reservation:?}; only the holder of a reservation sells a reserved unit"
                        )));
                    }
                }
                u.state = UnitState::Sold { sale: sale.clone() };
                u.owner = buyer.clone();
            }
            LedgerEvent::Returned {
                unit,
                sale,
                condition,
                location,
                owner,
            } => {
                let u = self.unit_mut(unit)?;
                if u.state != (UnitState::Sold { sale: sale.clone() }) {
                    return Err(Error::invalid(format!(
                        "unit {unit} was not sold under {sale}; a return reverses a recorded sale"
                    )));
                }
                u.state = UnitState::Held;
                u.condition = *condition;
                u.location = location.clone();
                u.owner = owner.clone();
                self.returns.push(ReturnRecord {
                    unit: unit.clone(),
                    sale: sale.clone(),
                    condition: *condition,
                });
            }
        }
        self.events.push(event);
        Ok(())
    }
}

// ----------------------------------------------- Marketplace and returns

#[derive(Clone, Debug, PartialEq)]
pub struct Sale {
    pub id: String,
    pub unit: String,
    pub gross: Decimal,
}

/// A sale's proceeds net of fees, and what later adjusted them.
#[derive(Clone, Debug, PartialEq)]
pub struct Settlement {
    pub sale: String,
    pub gross: Decimal,
    pub fee: Decimal,
    pub net: Decimal,
    /// Signed changes to `net`, each naming its cause.
    pub adjustments: Vec<(String, Decimal)>,
}

impl Settlement {
    pub fn net_after_adjustments(&self) -> Decimal {
        self.net + self.adjustments.iter().map(|(_, d)| *d).sum::<Decimal>()
    }
}

/// The ledger and a simulated marketplace, kept consistent: a sale is not
/// settled until its record exists, and a return changes both sides.
#[derive(Clone, Debug)]
pub struct CommercePlane {
    ledger: Ledger,
    fees: MarketplaceFees,
    sales: BTreeMap<String, Sale>,
    settlements: BTreeMap<String, Settlement>,
}

impl CommercePlane {
    pub fn new(ledger: Ledger, fees: MarketplaceFees) -> Self {
        Self {
            ledger,
            fees,
            sales: BTreeMap::new(),
            settlements: BTreeMap::new(),
        }
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn settlement(&self, sale: &str) -> Option<&Settlement> {
        self.settlements.get(sale)
    }

    pub fn is_settled(&self, sale: &str) -> bool {
        self.settlements.contains_key(sale)
    }

    pub fn sell(
        &mut self,
        sale: &str,
        unit: &str,
        buyer: &str,
        gross: Decimal,
        reservation: Option<String>,
    ) -> Result<()> {
        if self.sales.contains_key(sale) {
            return Err(Error::invalid(format!("sale {sale} already exists")));
        }
        if !gross.is_positive() {
            return Err(Error::invalid("a sale needs a positive price"));
        }
        self.ledger.apply(LedgerEvent::Sold {
            unit: unit.to_string(),
            reservation,
            sale: sale.to_string(),
            buyer: buyer.to_string(),
        })?;
        self.sales.insert(
            sale.to_string(),
            Sale {
                id: sale.to_string(),
                unit: unit.to_string(),
                gross,
            },
        );
        Ok(())
    }

    /// Net proceeds are gross less the ad valorem, per-unit and
    /// per-consignment fees; the simulator sells one unit per sale.
    pub fn settle(&mut self, sale: &str) -> Result<&Settlement> {
        let s = self
            .sales
            .get(sale)
            .ok_or_else(|| Error::not_found(format!("sale {sale} does not exist")))?;
        if self.settlements.contains_key(sale) {
            return Err(Error::invalid(format!("sale {sale} is already settled")));
        }
        let fee = mul("the marketplace fee", s.gross, self.fees.ad_valorem)?
            + self.fees.per_unit
            + self.fees.per_consignment;
        let settlement = Settlement {
            sale: sale.to_string(),
            gross: s.gross,
            fee,
            net: s.gross - fee,
            adjustments: Vec::new(),
        };
        Ok(self
            .settlements
            .entry(sale.to_string())
            .or_insert(settlement))
    }

    /// A buyer returns the unit: it goes back to the ledger in its recorded
    /// condition and the settlement is adjusted by the refund, less the
    /// ad valorem fee the marketplace gives back. The per-unit and
    /// per-consignment fees are kept, a stated choice of this simulator.
    pub fn return_unit(
        &mut self,
        sale: &str,
        condition: Condition,
        location: &str,
        owner: &str,
    ) -> Result<()> {
        let sold = self
            .sales
            .get(sale)
            .ok_or_else(|| Error::not_found(format!("sale {sale} does not exist")))?
            .clone();
        self.settlements.get(sale).ok_or_else(|| {
            Error::invalid(format!(
                "sale {sale} is not settled; settle it before accepting a return"
            ))
        })?;
        let refunded_fee = mul("the refunded fee", sold.gross, self.fees.ad_valorem)?;
        self.ledger.apply(LedgerEvent::Returned {
            unit: sold.unit,
            sale: sale.to_string(),
            condition,
            location: location.to_string(),
            owner: owner.to_string(),
        })?;
        if let Some(s) = self.settlements.get_mut(sale) {
            s.adjustments
                .push(("buyer return: refund of the gross".to_string(), -sold.gross));
            s.adjustments.push((
                "buyer return: ad valorem fee refunded".to_string(),
                refunded_fee,
            ));
        }
        Ok(())
    }
}
