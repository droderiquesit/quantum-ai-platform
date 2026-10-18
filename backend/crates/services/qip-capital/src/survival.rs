//! Capital Survival Kernel: withholding and releasing capital by priority.
//!
//! The Capital Survival Kernel accounts for NAV, reserved cash, margin,
//! collateral, settlement and funding maturity, and withholds hedge and
//! stressed-liquidity capital before releasing opportunity capital.
//!
//! Capital flows through a priority waterfall:
//! 1. **NAV (Net Asset Value)** — the floor the book cannot trade below
//! 2. **Reserved Cash** — uncommitted liquidity floor (CAPITAL-005)
//! 3. **Margin** — requirements at each venue under its margin regime
//! 4. **Collateral** — posted collateral encumbered to obligations
//! 5. **Settlement** — cash needed for T+settlement obligations
//! 6. **Funding Maturity** — obligations coming due on financing
//! 7. **Hedge Capacity** — capital reserved for hedging positions
//! 8. **Stressed-Liquidity Reserve** — capital for adverse scenarios
//! 9. **Opportunity Capital** — what remains is available for trading
//!
//! Each tier withholds capital from the one below, so a book with zero
//! opportunity capital still holds all its reserves. A refusal at any tier
//! stops release at that tier and all below it.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// One capital accounting across all priority tiers.
///
/// **Fields are private and there is no `Deserialize`.** They were public, so
/// any holder could lower a reserve in place — or deserialize a negative one —
/// after the builder had refused it, and opportunity capital would rise by
/// exactly the amount nobody had approved. A reserve can now only be set
/// through a `with_*` method that refuses a negative figure, which is what
/// makes [`Self::opportunity_capital`] a statement about validated inputs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CapitalSurvivalState {
    /// Total deployable capital at the start of the accounting.
    total_capital: Decimal,
    /// Required Net Asset Value floor: the level below which the book cannot trade.
    nav_floor: Decimal,
    /// Minimum uncommitted liquidity to hold (CAPITAL-005).
    reserved_cash: Decimal,
    /// Capital required as margin at venues.
    margin_required: Decimal,
    /// Capital posted as collateral, encumbered to obligations.
    collateral_posted: Decimal,
    /// Capital needed for settlement of pending transactions.
    settlement_required: Decimal,
    /// Capital tied up in financing obligations coming due soon.
    funding_maturity_required: Decimal,
    /// Capital reserved for hedging positions.
    hedge_capacity: Decimal,
    /// Capital reserved for stressed-liquidity scenarios.
    stressed_liquidity_reserve: Decimal,
}

impl CapitalSurvivalState {
    /// Create a capital accounting with specified reserves.
    ///
    /// All reserves default to zero and are set via builder methods.
    pub fn new(total_capital: Decimal) -> Result<Self> {
        if !total_capital.is_positive() {
            return Err(Error::invalid(format!(
                "total capital must be positive, got {total_capital}"
            )));
        }
        Ok(Self {
            total_capital,
            nav_floor: Decimal::ZERO,
            reserved_cash: Decimal::ZERO,
            margin_required: Decimal::ZERO,
            collateral_posted: Decimal::ZERO,
            settlement_required: Decimal::ZERO,
            funding_maturity_required: Decimal::ZERO,
            hedge_capacity: Decimal::ZERO,
            stressed_liquidity_reserve: Decimal::ZERO,
        })
    }

    /// Set the NAV floor (required minimum equity level).
    pub fn with_nav_floor(mut self, floor: Decimal) -> Result<Self> {
        if floor.is_negative() {
            return Err(Error::invalid(format!(
                "NAV floor must be non-negative, got {floor}"
            )));
        }
        self.nav_floor = floor;
        Ok(self)
    }

    /// Set the reserved cash floor (uncommitted liquidity requirement).
    pub fn with_reserved_cash(mut self, cash: Decimal) -> Result<Self> {
        if cash.is_negative() {
            return Err(Error::invalid(format!(
                "reserved cash must be non-negative, got {cash}"
            )));
        }
        self.reserved_cash = cash;
        Ok(self)
    }

    /// Set the margin requirement across all venues.
    pub fn with_margin_required(mut self, margin: Decimal) -> Result<Self> {
        if margin.is_negative() {
            return Err(Error::invalid(format!(
                "margin required must be non-negative, got {margin}"
            )));
        }
        self.margin_required = margin;
        Ok(self)
    }

    /// Set the collateral posted as encumbrance.
    pub fn with_collateral_posted(mut self, collateral: Decimal) -> Result<Self> {
        if collateral.is_negative() {
            return Err(Error::invalid(format!(
                "collateral posted must be non-negative, got {collateral}"
            )));
        }
        self.collateral_posted = collateral;
        Ok(self)
    }

    /// Set the settlement requirement for pending transactions.
    pub fn with_settlement_required(mut self, settlement: Decimal) -> Result<Self> {
        if settlement.is_negative() {
            return Err(Error::invalid(format!(
                "settlement required must be non-negative, got {settlement}"
            )));
        }
        self.settlement_required = settlement;
        Ok(self)
    }

    /// Set the funding maturity requirement (obligations coming due soon).
    pub fn with_funding_maturity(mut self, maturity: Decimal) -> Result<Self> {
        if maturity.is_negative() {
            return Err(Error::invalid(format!(
                "funding maturity required must be non-negative, got {maturity}"
            )));
        }
        self.funding_maturity_required = maturity;
        Ok(self)
    }

    /// Set the hedge capacity (capital reserved for hedging).
    pub fn with_hedge_capacity(mut self, hedge: Decimal) -> Result<Self> {
        if hedge.is_negative() {
            return Err(Error::invalid(format!(
                "hedge capacity must be non-negative, got {hedge}"
            )));
        }
        self.hedge_capacity = hedge;
        Ok(self)
    }

    /// Set the stressed-liquidity reserve (capital for adverse scenarios).
    pub fn with_stressed_liquidity_reserve(mut self, reserve: Decimal) -> Result<Self> {
        if reserve.is_negative() {
            return Err(Error::invalid(format!(
                "stressed liquidity reserve must be non-negative, got {reserve}"
            )));
        }
        self.stressed_liquidity_reserve = reserve;
        Ok(self)
    }

    /// The capital the accounting starts from.
    pub fn total_capital(&self) -> Decimal {
        self.total_capital
    }

    /// The hedge-capacity tier: capital withheld so a hedge can be put on.
    pub fn hedge_capacity(&self) -> Decimal {
        self.hedge_capacity
    }

    /// The stressed-liquidity tier: capital withheld against the tail.
    pub fn stressed_liquidity_reserve(&self) -> Decimal {
        self.stressed_liquidity_reserve
    }

    /// Calculate total capital withheld (not available for opportunity).
    ///
    /// Sums all required reserves in priority order. Does not validate that
    /// total withheld <= total_capital; that is the job of [`Self::opportunity_capital`].
    fn total_withheld(&self) -> Decimal {
        self.nav_floor
            + self.reserved_cash
            + self.margin_required
            + self.collateral_posted
            + self.settlement_required
            + self.funding_maturity_required
            + self.hedge_capacity
            + self.stressed_liquidity_reserve
    }

    /// Calculate opportunity capital available after all reserves are withheld.
    ///
    /// Returns an error if total withheld exceeds total capital, meaning the
    /// book does not have enough capital to meet its reserve requirements.
    /// This is the check that fails closed: insufficient reserves refuse
    /// opportunity release rather than reducing reserves to fit.
    pub fn opportunity_capital(&self) -> Result<Decimal> {
        let withheld = self.total_withheld();
        let opportunity = self.total_capital - withheld;
        if opportunity.is_negative() {
            return Err(Error::denied(format!(
                "capital survival check failed: {} total capital is insufficient to withhold {} \
                 (NAV floor {}, reserved cash {}, margin {}, collateral {}, settlement {}, \
                 funding maturity {}, hedge capacity {}, stressed-liquidity reserve {}); \
                 opportunity capital cannot be negative",
                self.total_capital,
                withheld,
                self.nav_floor,
                self.reserved_cash,
                self.margin_required,
                self.collateral_posted,
                self.settlement_required,
                self.funding_maturity_required,
                self.hedge_capacity,
                self.stressed_liquidity_reserve
            )));
        }
        Ok(opportunity)
    }

    /// How the waterfall funds each reserve tier, in priority order, from the
    /// total capital: each tier receives the lesser of what it requires and
    /// what the tiers above it left.
    ///
    /// [`Self::opportunity_capital`] is a sum and says only whether the
    /// reserves fit. It cannot say *which* tier goes short when they do not,
    /// and the module's promise that a refusal at one tier "stops release at
    /// that tier and all below it" was a sentence with no code behind it: a
    /// sum is the same in every order, so reordering the tiers could not fail
    /// any test. This is the ordered accounting. When capital runs out
    /// mid-waterfall the tier it runs out in is funded partly, every tier
    /// below it receives zero, and opportunity capital is the zero it is in
    /// [`Self::opportunity_capital`]'s refusal. It never errors: a shortfall
    /// is the finding.
    pub fn waterfall(&self) -> Vec<TierFunding> {
        let mut remaining = self.total_capital;
        SurvivalTier::PRIORITY
            .into_iter()
            .map(|tier| {
                let required = self.required(tier);
                let funded = required.min(remaining);
                remaining -= funded;
                TierFunding {
                    tier,
                    required,
                    funded,
                }
            })
            .collect()
    }

    fn required(&self, tier: SurvivalTier) -> Decimal {
        match tier {
            SurvivalTier::NavFloor => self.nav_floor,
            SurvivalTier::ReservedCash => self.reserved_cash,
            SurvivalTier::Margin => self.margin_required,
            SurvivalTier::Collateral => self.collateral_posted,
            SurvivalTier::Settlement => self.settlement_required,
            SurvivalTier::FundingMaturity => self.funding_maturity_required,
            SurvivalTier::HedgeCapacity => self.hedge_capacity,
            SurvivalTier::StressedLiquidity => self.stressed_liquidity_reserve,
        }
    }

    /// Report the survival state: all tiers and what remains.
    ///
    /// Useful for auditing, diagnostics and understanding why opportunity
    /// capital is smaller than expected.
    pub fn report(&self) -> Result<SurvivalReport> {
        let opportunity_capital = self.opportunity_capital()?;
        Ok(SurvivalReport {
            total_capital: self.total_capital,
            nav_floor: self.nav_floor,
            reserved_cash: self.reserved_cash,
            margin_required: self.margin_required,
            collateral_posted: self.collateral_posted,
            settlement_required: self.settlement_required,
            funding_maturity_required: self.funding_maturity_required,
            hedge_capacity: self.hedge_capacity,
            stressed_liquidity_reserve: self.stressed_liquidity_reserve,
            opportunity_capital,
        })
    }
}

/// A reserve tier of the survival waterfall.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurvivalTier {
    NavFloor,
    ReservedCash,
    Margin,
    Collateral,
    Settlement,
    FundingMaturity,
    HedgeCapacity,
    StressedLiquidity,
}

impl SurvivalTier {
    /// Every tier in funding priority, highest first: the order of the
    /// module documentation's list.
    pub const PRIORITY: [Self; 8] = [
        Self::NavFloor,
        Self::ReservedCash,
        Self::Margin,
        Self::Collateral,
        Self::Settlement,
        Self::FundingMaturity,
        Self::HedgeCapacity,
        Self::StressedLiquidity,
    ];
}

/// What the waterfall gave one tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierFunding {
    pub tier: SurvivalTier,
    pub required: Decimal,
    pub funded: Decimal,
}

/// A readable report of capital allocation across all survival tiers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurvivalReport {
    pub total_capital: Decimal,
    pub nav_floor: Decimal,
    pub reserved_cash: Decimal,
    pub margin_required: Decimal,
    pub collateral_posted: Decimal,
    pub settlement_required: Decimal,
    pub funding_maturity_required: Decimal,
    pub hedge_capacity: Decimal,
    pub stressed_liquidity_reserve: Decimal,
    pub opportunity_capital: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    #[test]
    fn survival_state_withholds_reserves_before_releasing_opportunity() {
        let state = CapitalSurvivalState::new(dec!("1000"))
            .unwrap()
            .with_nav_floor(dec!("100"))
            .unwrap()
            .with_reserved_cash(dec!("150"))
            .unwrap()
            .with_hedge_capacity(dec!("200"))
            .unwrap()
            .with_stressed_liquidity_reserve(dec!("250"))
            .unwrap();

        let opportunity = state.opportunity_capital().unwrap();
        // 1000 - (100 + 150 + 0 + 0 + 0 + 0 + 200 + 250) = 1000 - 700 = 300
        assert_eq!(opportunity, dec!("300"));
    }

    #[test]
    fn survival_state_refuses_opportunity_when_reserves_exceed_capital() {
        let state = CapitalSurvivalState::new(dec!("1000"))
            .unwrap()
            .with_nav_floor(dec!("400"))
            .unwrap()
            .with_reserved_cash(dec!("300"))
            .unwrap()
            .with_hedge_capacity(dec!("400"))
            .unwrap();

        let result = state.opportunity_capital();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("capital survival check failed"));
    }

    #[test]
    fn survival_state_accepts_zero_opportunity_when_fully_reserved() {
        let state = CapitalSurvivalState::new(dec!("1000"))
            .unwrap()
            .with_nav_floor(dec!("200"))
            .unwrap()
            .with_reserved_cash(dec!("300"))
            .unwrap()
            .with_margin_required(dec!("150"))
            .unwrap()
            .with_collateral_posted(dec!("100"))
            .unwrap()
            .with_settlement_required(dec!("50"))
            .unwrap()
            .with_funding_maturity(dec!("50"))
            .unwrap()
            .with_hedge_capacity(dec!("100"))
            .unwrap()
            .with_stressed_liquidity_reserve(dec!("50"))
            .unwrap();

        let opportunity = state.opportunity_capital().unwrap();
        // 1000 - (200 + 300 + 150 + 100 + 50 + 50 + 100 + 50) = 0
        assert_eq!(opportunity, Decimal::ZERO);
    }

    #[test]
    fn survival_report_shows_all_tiers() {
        let state = CapitalSurvivalState::new(dec!("1000"))
            .unwrap()
            .with_nav_floor(dec!("100"))
            .unwrap()
            .with_reserved_cash(dec!("150"))
            .unwrap()
            .with_hedge_capacity(dec!("200"))
            .unwrap()
            .with_stressed_liquidity_reserve(dec!("250"))
            .unwrap();

        let report = state.report().unwrap();
        assert_eq!(report.total_capital, dec!("1000"));
        assert_eq!(report.nav_floor, dec!("100"));
        assert_eq!(report.reserved_cash, dec!("150"));
        assert_eq!(report.hedge_capacity, dec!("200"));
        assert_eq!(report.stressed_liquidity_reserve, dec!("250"));
        assert_eq!(report.opportunity_capital, dec!("300"));
    }

    #[test]
    fn survival_state_rejects_negative_total_capital() {
        let result = CapitalSurvivalState::new(dec!("-100"));
        assert!(result.is_err());
    }

    #[test]
    fn survival_state_rejects_negative_reserves() {
        assert!(
            CapitalSurvivalState::new(dec!("1000"))
                .unwrap()
                .with_nav_floor(dec!("-10"))
                .is_err()
        );
        assert!(
            CapitalSurvivalState::new(dec!("1000"))
                .unwrap()
                .with_reserved_cash(dec!("-10"))
                .is_err()
        );
        assert!(
            CapitalSurvivalState::new(dec!("1000"))
                .unwrap()
                .with_hedge_capacity(dec!("-10"))
                .is_err()
        );
        assert!(
            CapitalSurvivalState::new(dec!("1000"))
                .unwrap()
                .with_stressed_liquidity_reserve(dec!("-10"))
                .is_err()
        );
    }
}
