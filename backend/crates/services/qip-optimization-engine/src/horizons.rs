//! Multi-horizon reconciliation — blueprint §23.4.
//!
//! The blueprint's table says microsecond arbitrage and multi-year private
//! positions coexist by being allocated against *different* capital, and names
//! the four pools: available inventory recycled continuously, deployable
//! capital, capital not reserved for calls, and reserved capital plus the
//! unfunded commitment liability. This module holds the desk to that.
//!
//! The failure it exists to prevent is a single capital number serving four
//! horizons at once. A total that looks unbreached hides a years-horizon
//! commitment funded out of the inventory a market maker needs today, and the
//! discovery happens when the capital call arrives. Four pools that must sum
//! exactly to the total make the double-count arithmetically impossible rather
//! than merely discouraged.
//!
//! # Money and statistics
//!
//! Every capital figure here is [`Decimal`]. Family weights are statistics and
//! arrive as `f64`. **The crossing point is [`FamilyBudget::from_weight`] and
//! nowhere else** — it is the single place a ratio becomes money, it rounds to
//! the nine fractional digits `Decimal` carries, and it refuses a weight it
//! cannot represent. Everything downstream of it is exact integer arithmetic,
//! so a pool comparison needs no tolerance and a rounding residue shows up as
//! [`HorizonReconciliation::unallocated`] rather than as a phantom breach.
//!
//! # The gate
//!
//! [`HorizonReconciliation`] is a report and always available, because an
//! operator needs to see a breach in order to fix it.
//! [`ReconciledPlan`] is the money, and there is no constructor for it other
//! than [`HorizonReconciliation::into_plan`], which refuses while any horizon
//! is over-committed. A limit that cannot fire is a defect; this one can only
//! be passed by balancing.
//!
//! **Including for serde.** That paragraph was a doc comment and not a type:
//! every struct in this module derived `Deserialize` straight onto its private
//! fields, so a document was a second constructor that ran none of the checks
//! above. It was not theoretical. A plan claiming 999,999,999 committed
//! against a total of 4 loaded without passing the gate; a pool set stating
//! 1,000 in each of four pools against a total of 1,000 loaded and the same
//! unit was spent four times with `is_balanced` still true; and a budget of
//! -250 loaded and netted a real breach at that horizon away, past the
//! budgeted-twice guard, because it belonged to a different family. Each type
//! now carries `serde(try_from)` onto its own checked constructor, and
//! [`ReconciledPlan`]'s wire type is [`HorizonReconciliation`] itself — so the
//! only way to a plan really is [`HorizonReconciliation::into_plan`].

use crate::families::{FamilyAssignment, FamilyId};
use qip_core::decimal::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::StrategyId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The four horizon buckets of blueprint §23.4, ordered from most liquid to
/// least.
///
/// Ordered so a `BTreeMap` keyed on a horizon reports in the order the
/// blueprint's table reads, and so "the least liquid horizon in this family"
/// is a `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Horizon {
    /// Arbitrage, market making, microstructure. Immediate liquidity.
    MicrosecondsToMinutes,
    /// Statistical arbitrage, event-driven, reversion. Same or next day.
    HoursToDays,
    /// Trend, carry, volatility. Days to unwind.
    WeeksToMonths,
    /// Private equity, credit, real assets, royalties. Effectively no
    /// liquidity.
    Years,
}

impl Horizon {
    /// Every horizon, most liquid first.
    pub const ALL: [Self; 4] = [
        Self::MicrosecondsToMinutes,
        Self::HoursToDays,
        Self::WeeksToMonths,
        Self::Years,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::MicrosecondsToMinutes => "microseconds_to_minutes",
            Self::HoursToDays => "hours_to_days",
            Self::WeeksToMonths => "weeks_to_months",
            Self::Years => "years",
        }
    }

    /// The blueprint's allocation treatment for this horizon, so a refusal can
    /// say which pool was meant rather than only which one was short.
    pub const fn treatment(&self) -> &'static str {
        match self {
            Self::MicrosecondsToMinutes => "against available inventory, recycled continuously",
            Self::HoursToDays => "against deployable capital",
            Self::WeeksToMonths => "against capital not reserved for calls",
            Self::Years => {
                "against reserved capital, which must also meet the unfunded \
                            commitment liability"
            }
        }
    }
}

impl std::fmt::Display for Horizon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The four pools of blueprint §23.4, plus the liability the last one carries.
///
/// Fields are private and deserialisation is routed through [`Self::new`] by
/// `serde(try_from)`. The plain derive was a second constructor, and it
/// undid the one guarantee this module claims to make arithmetically
/// impossible: a document stating a total of 1,000 with 1,000 in each of the
/// four pools loaded, and the same unit was then spent four times over, with
/// [`HorizonReconciliation::is_balanced`] reporting true and `unallocated`
/// reading -3,000. Negative pools loaded too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CapitalPoolsWire")]
pub struct CapitalPools {
    total: Decimal,
    available_inventory: Decimal,
    deployable_capital: Decimal,
    capital_not_reserved_for_calls: Decimal,
    reserved_capital: Decimal,
    unfunded_commitments: Decimal,
}

/// The on-disk shape of [`CapitalPools`].
#[derive(Deserialize)]
struct CapitalPoolsWire {
    total: Decimal,
    available_inventory: Decimal,
    deployable_capital: Decimal,
    capital_not_reserved_for_calls: Decimal,
    reserved_capital: Decimal,
    unfunded_commitments: Decimal,
}

impl TryFrom<CapitalPoolsWire> for CapitalPools {
    type Error = Error;

    fn try_from(wire: CapitalPoolsWire) -> Result<Self> {
        Self::new(
            wire.total,
            wire.available_inventory,
            wire.deployable_capital,
            wire.capital_not_reserved_for_calls,
            wire.reserved_capital,
            wire.unfunded_commitments,
        )
    }
}

impl CapitalPools {
    /// Refuses pools that do not sum exactly to the total.
    ///
    /// Exactly, with no tolerance, because these are `Decimal`: the sum is
    /// integer arithmetic and a difference of one unit in the ninth decimal is
    /// a real difference, not a rounding artefact. A tolerance here would be
    /// the seam through which one currency unit gets spent twice, and the
    /// whole point of splitting the total four ways is that it cannot be.
    ///
    /// `unfunded_commitments` is deliberately outside the sum. It is a
    /// liability, not capital in hand — the reserved pool has to be able to
    /// meet it *and* whatever is allocated at the years horizon, which is what
    /// [`reconcile`] charges it for.
    pub fn new(
        total: Decimal,
        available_inventory: Decimal,
        deployable_capital: Decimal,
        capital_not_reserved_for_calls: Decimal,
        reserved_capital: Decimal,
        unfunded_commitments: Decimal,
    ) -> Result<Self> {
        let named = [
            ("total", total),
            ("available_inventory", available_inventory),
            ("deployable_capital", deployable_capital),
            (
                "capital_not_reserved_for_calls",
                capital_not_reserved_for_calls,
            ),
            ("reserved_capital", reserved_capital),
            ("unfunded_commitments", unfunded_commitments),
        ];
        for (label, value) in named {
            if value.is_negative() {
                return Err(Error::invalid(format!(
                    "{label} is {value}; a capital pool cannot be negative — record the shortfall \
                     as a commitment against a pool rather than as a negative pool"
                )));
            }
        }
        if total.is_zero() {
            return Err(Error::invalid(
                "total capital is zero, so there is nothing to reconcile against; supply the \
                 capital the horizons divide",
            ));
        }

        let mut sum = Decimal::ZERO;
        for pool in [
            available_inventory,
            deployable_capital,
            capital_not_reserved_for_calls,
            reserved_capital,
        ] {
            sum = sum.checked_add(pool).ok_or_else(|| {
                Error::numeric("the horizon pools overflow when summed; check the units")
            })?;
        }
        if sum != total {
            let difference = sum.checked_sub(total).unwrap_or(Decimal::ZERO);
            return Err(Error::invalid(format!(
                "the four horizon pools sum to {sum} against a total of {total}, a difference of \
                 {difference}; adjust a pool so they sum exactly — capital that belongs to no \
                 horizon is capital two horizons will both spend"
            )));
        }

        Ok(Self {
            total,
            available_inventory,
            deployable_capital,
            capital_not_reserved_for_calls,
            reserved_capital,
            unfunded_commitments,
        })
    }

    pub const fn total(&self) -> Decimal {
        self.total
    }

    pub const fn unfunded_commitments(&self) -> Decimal {
        self.unfunded_commitments
    }

    /// The pool a horizon is allocated against.
    pub const fn pool_for(&self, horizon: Horizon) -> Decimal {
        match horizon {
            Horizon::MicrosecondsToMinutes => self.available_inventory,
            Horizon::HoursToDays => self.deployable_capital,
            Horizon::WeeksToMonths => self.capital_not_reserved_for_calls,
            Horizon::Years => self.reserved_capital,
        }
    }
}

/// One family's claim on one horizon's pool.
///
/// Fields are private and deserialisation is routed through
/// [`Self::from_money`] by `serde(try_from)`. A negative budget is not a small
/// error: [`reconcile`] sums the budgets at a horizon, so one deserialised
/// record of -250 at the horizon another family is over-committed at nets a
/// real breach away and the reconciliation reports balanced. The
/// budgeted-twice guard does not catch it, because the two records are two
/// different families and are each perfectly legitimate on their own. A limit
/// a second record can net away is a limit that cannot fire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FamilyBudgetWire")]
pub struct FamilyBudget {
    family: FamilyId,
    horizon: Horizon,
    money: Decimal,
}

/// The on-disk shape of [`FamilyBudget`].
#[derive(Deserialize)]
struct FamilyBudgetWire {
    family: FamilyId,
    horizon: Horizon,
    money: Decimal,
}

impl TryFrom<FamilyBudgetWire> for FamilyBudget {
    type Error = Error;

    fn try_from(wire: FamilyBudgetWire) -> Result<Self> {
        Self::from_money(wire.family, wire.horizon, wire.money)
    }
}

impl FamilyBudget {
    /// A budget stated directly in money.
    pub fn from_money(family: FamilyId, horizon: Horizon, money: Decimal) -> Result<Self> {
        if money.is_negative() {
            return Err(Error::invalid(format!(
                "family {family} is budgeted {money} at the {horizon} horizon; a negative budget \
                 is a short position, which is sized as an exposure and not as a capital claim"
            )));
        }
        Ok(Self {
            family,
            horizon,
            money,
        })
    }

    /// **The one place a weight becomes money.**
    ///
    /// `weight` is a fraction of `total` and is a statistic; the result is
    /// money. The conversion rounds to the nine fractional digits `Decimal`
    /// carries, and that residue is visible afterwards as
    /// [`HorizonReconciliation::unallocated`] rather than being absorbed
    /// somewhere unnamed.
    ///
    /// Refuses a non-finite weight, a negative one, and one above 1: each is a
    /// caller that has lost track of what its weights mean, and clamping would
    /// let it keep believing it.
    pub fn from_weight(
        family: FamilyId,
        horizon: Horizon,
        weight: f64,
        total: Decimal,
    ) -> Result<Self> {
        if !weight.is_finite() {
            return Err(Error::numeric(format!(
                "family {family} has a non-finite weight at the {horizon} horizon; repair the \
                 allocation upstream — a NaN weight cannot be turned into capital"
            )));
        }
        if weight < 0.0 {
            return Err(Error::invalid(format!(
                "family {family} has a weight of {weight} at the {horizon} horizon; a weight is a \
                 non-negative fraction of capital"
            )));
        }
        if weight > 1.0 {
            return Err(Error::invalid(format!(
                "family {family} has a weight of {weight} at the {horizon} horizon, above the \
                 whole of capital; rescale the allocation rather than letting one family claim \
                 more than exists"
            )));
        }
        // Statistics (f64) become money (Decimal) here and only here.
        let fraction = Decimal::from_f64(weight).ok_or_else(|| {
            Error::numeric(format!(
                "family {family}'s weight of {weight} is outside the range a decimal can carry; \
                 rescale it upstream"
            ))
        })?;
        let money = fraction.checked_mul(total).ok_or_else(|| {
            Error::numeric(format!(
                "family {family}'s weight of {weight} against a total of {total} overflows; check \
                 the units of the total"
            ))
        })?;
        Self::from_money(family, horizon, money)
    }

    pub const fn family(&self) -> FamilyId {
        self.family
    }

    pub const fn horizon(&self) -> Horizon {
        self.horizon
    }

    pub const fn money(&self) -> Decimal {
        self.money
    }
}

/// One horizon's side of the reconciliation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizonPosition {
    pub horizon: Horizon,
    /// The pool this horizon is allocated against.
    pub pool: Decimal,
    /// What is claimed against it. At [`Horizon::Years`] this includes the
    /// unfunded commitment liability, which no family asked for and the
    /// reserved pool must still meet.
    pub committed: Decimal,
    /// The part of `committed` that is the unfunded commitment liability
    /// rather than a family's budget. Zero everywhere but the years horizon.
    pub liability: Decimal,
    pub families: BTreeSet<FamilyId>,
}

impl HorizonPosition {
    /// Pool less commitment. Negative when the horizon is over-committed.
    pub fn headroom(&self) -> Decimal {
        self.pool
            .checked_sub(self.committed)
            .unwrap_or(Decimal::MIN)
    }

    pub fn is_breached(&self) -> bool {
        self.committed > self.pool
    }
}

/// What the reconciliation found, breaches included.
///
/// Fields are private and deserialisation is routed through [`Self::rederive`]
/// by `serde(try_from)`: a document's pools and budgets are put back through
/// [`CapitalPools::new`], [`FamilyBudget::from_money`] and [`reconcile`], and
/// the result must equal what the document claimed. Checking the fields one by
/// one would be a second implementation of the reconciliation, and the two
/// would drift; re-deriving means there is one.
///
/// Without it, a reconciliation loaded with no positions at all reported
/// `is_balanced` — vacuously, because nothing was over its pool — and handed
/// out a [`ReconciledPlan`] for a total of 4 with 999,999,999 allocated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "HorizonReconciliationWire")]
pub struct HorizonReconciliation {
    positions: BTreeMap<Horizon, HorizonPosition>,
    allocations: BTreeMap<FamilyId, Decimal>,
    total: Decimal,
    allocated: Decimal,
}

/// The on-disk shape of [`HorizonReconciliation`].
#[derive(Deserialize)]
struct HorizonReconciliationWire {
    positions: BTreeMap<Horizon, HorizonPosition>,
    allocations: BTreeMap<FamilyId, Decimal>,
    total: Decimal,
    allocated: Decimal,
}

impl TryFrom<HorizonReconciliationWire> for HorizonReconciliation {
    type Error = Error;

    fn try_from(wire: HorizonReconciliationWire) -> Result<Self> {
        let claimed = Self {
            positions: wire.positions,
            allocations: wire.allocations,
            total: wire.total,
            allocated: wire.allocated,
        };
        let rederived = claimed.rederive()?;
        if rederived != claimed {
            return Err(Error::invalid(format!(
                "the reconciliation does not re-derive from its own parts: it claims {} allocated \
                 over committed {}, and its own budgets against its own pools give {} allocated \
                 over committed {}; the record was edited after it was produced",
                claimed.allocated,
                claimed.committed_summary(),
                rederived.allocated,
                rederived.committed_summary()
            )));
        }
        Ok(rederived)
    }
}

impl HorizonReconciliation {
    /// Rebuild this reconciliation from the pools and budgets it carries.
    ///
    /// Everything a `HorizonReconciliation` asserts is a function of the four
    /// pools, the unfunded commitment liability and the family budgets, all of
    /// which it holds. So the check is to take those back out, put them
    /// through the constructors that refuse a bad one, and run [`reconcile`]
    /// again.
    fn rederive(&self) -> Result<Self> {
        let mut pool: BTreeMap<Horizon, Decimal> = BTreeMap::new();
        let mut liability = Decimal::ZERO;
        for horizon in Horizon::ALL {
            let position = self.positions.get(&horizon).ok_or_else(|| {
                Error::invalid(format!(
                    "the reconciliation carries no position for the {horizon} horizon; all four \
                     are reported whether or not a family was budgeted there, because a horizon \
                     nobody reported is a horizon nobody checked"
                ))
            })?;
            if position.horizon != horizon {
                return Err(Error::invalid(format!(
                    "the position filed under the {horizon} horizon says it is {}; the key and \
                     the record have to name one horizon, or a breach is reported against the \
                     wrong pool ({})",
                    position.horizon,
                    horizon.treatment()
                )));
            }
            if horizon == Horizon::Years {
                liability = position.liability;
            } else if !position.liability.is_zero() {
                return Err(Error::invalid(format!(
                    "the {horizon} horizon carries an unfunded commitment liability of {}; the \
                     liability belongs to the years horizon alone, which is the only pool that \
                     has to meet a capital call",
                    position.liability
                )));
            }
            pool.insert(horizon, position.pool);
        }
        let at = |horizon: Horizon| pool.get(&horizon).copied().unwrap_or(Decimal::MIN);
        // Every horizon is in `pool` by the loop above, so `at` never falls
        // back; the fallback is `MIN` rather than `ZERO` so that if it ever
        // did, `CapitalPools::new` would refuse it as a negative pool instead
        // of admitting a plausible-looking zero.
        let pools = CapitalPools::new(
            self.total,
            at(Horizon::MicrosecondsToMinutes),
            at(Horizon::HoursToDays),
            at(Horizon::WeeksToMonths),
            at(Horizon::Years),
            liability,
        )?;

        let mut budgets: Vec<FamilyBudget> = Vec::new();
        let mut budgeted: BTreeSet<FamilyId> = BTreeSet::new();
        for horizon in Horizon::ALL {
            let families = self
                .positions
                .get(&horizon)
                .map(|position| position.families.clone())
                .unwrap_or_default();
            for family in families {
                let money = self.allocations.get(&family).ok_or_else(|| {
                    Error::invalid(format!(
                        "{family} is listed at the {horizon} horizon with no budget among the \
                         allocations; a family charged to a pool has to say what it was charged"
                    ))
                })?;
                budgets.push(FamilyBudget::from_money(family, horizon, *money)?);
                budgeted.insert(family);
            }
        }
        for (family, money) in &self.allocations {
            if !budgeted.contains(family) {
                return Err(Error::invalid(format!(
                    "{family} holds a budget of {money} at no horizon; capital that belongs to no \
                     horizon is capital two horizons will both spend — file it at the horizon it \
                     is to be allocated against"
                )));
            }
        }

        reconcile(&pools, &budgets)
    }

    /// What each horizon claims, for a refusal to show both sides with.
    fn committed_summary(&self) -> String {
        Horizon::ALL
            .iter()
            .map(|horizon| {
                let committed = self.positions.get(horizon).map_or_else(
                    || "absent".to_string(),
                    |position| position.committed.to_string(),
                );
                format!("{horizon}={committed}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn positions(&self) -> &BTreeMap<Horizon, HorizonPosition> {
        &self.positions
    }

    pub fn position(&self, horizon: Horizon) -> Option<&HorizonPosition> {
        self.positions.get(&horizon)
    }

    pub const fn total(&self) -> Decimal {
        self.total
    }

    /// The sum of every family's budget. Excludes the unfunded commitment
    /// liability, which is not an allocation.
    pub const fn allocated(&self) -> Decimal {
        self.allocated
    }

    /// Total less allocated. Negative when the families between them have
    /// claimed more capital than exists, in which case at least one horizon is
    /// breached, since the pools sum to the total.
    pub fn unallocated(&self) -> Decimal {
        self.total
            .checked_sub(self.allocated)
            .unwrap_or(Decimal::MIN)
    }

    /// Every over-committed horizon, most liquid first.
    pub fn breaches(&self) -> Vec<&HorizonPosition> {
        self.positions
            .values()
            .filter(|p| p.is_breached())
            .collect()
    }

    pub fn is_balanced(&self) -> bool {
        !self.positions.values().any(HorizonPosition::is_breached)
    }

    /// The gate. Yields the money only when every horizon fits its pool.
    ///
    /// Refuses rather than trimming the offending horizon. A trim would be the
    /// platform choosing which strategy goes unfunded, silently, at the moment
    /// the desk most needs to make that choice itself.
    pub fn into_plan(self) -> Result<ReconciledPlan> {
        let breaches = self.breaches();
        if !breaches.is_empty() {
            let detail = breaches
                .iter()
                .map(|p| {
                    let over = p.committed.checked_sub(p.pool).unwrap_or(Decimal::ZERO);
                    format!(
                        "{} is over its pool of {} by {} ({})",
                        p.horizon,
                        p.pool,
                        over,
                        p.horizon.treatment()
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(Error::denied(format!(
                "the allocation is not reconciled: {detail}. Lower the budgets at that horizon or \
                 move capital into its pool — this stage will not trim them for you"
            )));
        }
        Ok(ReconciledPlan {
            positions: self.positions,
            allocations: self.allocations,
            total: self.total,
            allocated: self.allocated,
        })
    }
}

/// A reconciled allocation: money per family, every horizon inside its pool.
///
/// Constructible only through [`HorizonReconciliation::into_plan`]. That is
/// the structural half of the guarantee — a caller cannot hold this type and
/// be over-committed, whatever it forgot to check.
///
/// It was a doc comment rather than a type: the derived `Deserialize` wrote
/// straight to these private fields, and a document produced a plan claiming
/// 999,999,999 committed against a total of 4 without passing the gate at all.
/// So serde comes in by the same door, and by no other — `try_from` takes a
/// [`HorizonReconciliation`], which re-derives itself from its own pools and
/// budgets, and then calls [`HorizonReconciliation::into_plan`]. The sentence
/// above is now enforced by the compiler for every route in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "HorizonReconciliation")]
pub struct ReconciledPlan {
    positions: BTreeMap<Horizon, HorizonPosition>,
    allocations: BTreeMap<FamilyId, Decimal>,
    total: Decimal,
    allocated: Decimal,
}

impl TryFrom<HorizonReconciliation> for ReconciledPlan {
    type Error = Error;

    fn try_from(reconciliation: HorizonReconciliation) -> Result<Self> {
        reconciliation.into_plan()
    }
}

impl ReconciledPlan {
    pub fn allocations(&self) -> &BTreeMap<FamilyId, Decimal> {
        &self.allocations
    }

    pub fn allocation_for(&self, family: FamilyId) -> Option<Decimal> {
        self.allocations.get(&family).copied()
    }

    pub fn positions(&self) -> &BTreeMap<Horizon, HorizonPosition> {
        &self.positions
    }

    pub const fn total(&self) -> Decimal {
        self.total
    }

    pub const fn allocated(&self) -> Decimal {
        self.allocated
    }
}

/// Reconcile family budgets against the horizon pools.
///
/// Refuses a family budgeted twice: two claims on the same fact disagree, and
/// summing them silently would fund the family at whichever total the caller
/// did not intend.
pub fn reconcile(pools: &CapitalPools, budgets: &[FamilyBudget]) -> Result<HorizonReconciliation> {
    let mut allocations: BTreeMap<FamilyId, Decimal> = BTreeMap::new();
    let mut by_horizon: BTreeMap<Horizon, (Decimal, BTreeSet<FamilyId>)> = BTreeMap::new();

    for budget in budgets {
        if allocations.contains_key(&budget.family) {
            return Err(Error::invalid(format!(
                "family {} is budgeted twice; give each family one budget at one horizon, \
                 because two claims on the same capital cannot both be the record",
                budget.family
            )));
        }
        allocations.insert(budget.family, budget.money);
        let entry = by_horizon
            .entry(budget.horizon)
            .or_insert((Decimal::ZERO, BTreeSet::new()));
        entry.0 = entry.0.checked_add(budget.money).ok_or_else(|| {
            Error::numeric(format!(
                "the budgets at the {} horizon overflow when summed; check the units",
                budget.horizon
            ))
        })?;
        entry.1.insert(budget.family);
    }

    let mut allocated = Decimal::ZERO;
    for money in allocations.values() {
        allocated = allocated.checked_add(*money).ok_or_else(|| {
            Error::numeric("the family budgets overflow when summed; check the units")
        })?;
    }

    let mut positions: BTreeMap<Horizon, HorizonPosition> = BTreeMap::new();
    for horizon in Horizon::ALL {
        let (claimed, families) = by_horizon
            .remove(&horizon)
            .unwrap_or((Decimal::ZERO, BTreeSet::new()));
        // The years horizon carries the unfunded commitment liability whether
        // or not a family was budgeted there. A commitment nobody allocated
        // against is exactly the one that surprises a desk when it is called.
        let liability = if horizon == Horizon::Years {
            pools.unfunded_commitments()
        } else {
            Decimal::ZERO
        };
        let committed = claimed.checked_add(liability).ok_or_else(|| {
            Error::numeric(
                "the years horizon overflows when the unfunded commitment liability is added",
            )
        })?;
        positions.insert(
            horizon,
            HorizonPosition {
                horizon,
                pool: pools.pool_for(horizon),
                committed,
                liability,
                families,
            },
        );
    }

    Ok(HorizonReconciliation {
        positions,
        allocations,
        total: pools.total(),
        allocated,
    })
}

/// The horizon of each family in an assignment — the blueprint's
/// "horizon × family" seam.
///
/// Refuses a family whose members do not agree on a horizon. Clustering is
/// keyed on stress correlation, which knows nothing about liquidity, so it can
/// and will put a microsecond arbitrage and a multi-year private position in
/// one family when their stress returns move together. Allocating that family
/// against one pool would fund an illiquid position out of inventory that has
/// to be recycled by the close. The refusal names the fix rather than picking
/// a horizon on the caller's behalf.
pub fn family_horizons(
    assignment: &FamilyAssignment,
    of_strategy: &BTreeMap<StrategyId, Horizon>,
) -> Result<BTreeMap<FamilyId, Horizon>> {
    let mut out: BTreeMap<FamilyId, Horizon> = BTreeMap::new();
    for (family, members) in assignment.families() {
        let mut chosen: Option<Horizon> = None;
        for member in members {
            let horizon = of_strategy.get(member).ok_or_else(|| {
                Error::invalid(format!(
                    "strategy {member} is in {family} but declares no horizon; every clustered \
                     strategy needs one, because the horizon decides which capital pool the \
                     family is allocated against"
                ))
            })?;
            match chosen {
                None => chosen = Some(*horizon),
                Some(existing) if existing == *horizon => {}
                Some(existing) => {
                    return Err(Error::invalid(format!(
                        "{family} spans the {existing} and {horizon} horizons; cluster within a \
                         horizon bucket, or assign the family the horizon of its least liquid \
                         member deliberately — a family straddling two pools cannot be \
                         reconciled against either"
                    )));
                }
            }
        }
        let Some(horizon) = chosen else {
            return Err(Error::invalid(format!(
                "{family} has no members; a clustering should not produce an empty family"
            )));
        };
        out.insert(*family, horizon);
    }
    Ok(out)
}
