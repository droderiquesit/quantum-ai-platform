//! Typed per-instrument extensions.
//!
//! Only the fields that genuinely apply to an instrument type live here. Code
//! that needs a bond's coupon matches on [`Extension::Bond`] and gets a
//! non-optional value; code that works across the whole portfolio uses the
//! common base fields on [`crate::FinancialObject`] and never sees them.

use qip_core::{Currency, Decimal, Timestamp};
use serde::{Deserialize, Serialize};

use crate::risk_profile::Greeks;

/// Instrument-specific detail, one variant per family of instrument types.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[derive(Default)]
pub enum Extension {
    Equity(EquityDetails),
    Bond(BondDetails),
    Loan(LoanDetails),
    StructuredCredit(StructuredCreditDetails),
    CreditDerivative(CreditDerivativeDetails),
    Rate(RateDetails),
    Repo(RepoDetails),
    Fx(FxDetails),
    Commodity(CommodityDetails),
    Future(FutureDetails),
    Option(OptionDetails),
    Volatility(VolatilityDetails),
    Digital(DigitalAssetDetails),
    Fund(FundDetails),
    PrivateAsset(PrivateAssetDetails),
    RealAsset(RealAssetDetails),
    Index(IndexDetails),
    Cash(CashDetails),
    Structured(StructuredProductDetails),
    /// No extension data is known yet — a legitimate transient state during
    /// ingestion, never a permanent one for a tradable object.
    #[default]
    Unspecified,
}

impl Extension {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Equity(_) => "equity",
            Self::Bond(_) => "bond",
            Self::Loan(_) => "loan",
            Self::StructuredCredit(_) => "structured_credit",
            Self::CreditDerivative(_) => "credit_derivative",
            Self::Rate(_) => "rate",
            Self::Repo(_) => "repo",
            Self::Fx(_) => "fx",
            Self::Commodity(_) => "commodity",
            Self::Future(_) => "future",
            Self::Option(_) => "option",
            Self::Volatility(_) => "volatility",
            Self::Digital(_) => "digital",
            Self::Fund(_) => "fund",
            Self::PrivateAsset(_) => "private_asset",
            Self::RealAsset(_) => "real_asset",
            Self::Index(_) => "index",
            Self::Cash(_) => "cash",
            Self::Structured(_) => "structured",
            Self::Unspecified => "unspecified",
        }
    }

    /// Greeks, where the instrument type has any.
    pub fn greeks(&self) -> Option<&Greeks> {
        match self {
            Self::Option(o) => Some(&o.greeks),
            Self::Volatility(v) => v.greeks.as_ref(),
            Self::Structured(s) => s.greeks.as_ref(),
            _ => None,
        }
    }

    /// Contractual maturity, where one exists.
    pub fn maturity(&self) -> Option<Timestamp> {
        match self {
            Self::Bond(b) => Some(b.maturity),
            Self::Loan(l) => Some(l.maturity),
            Self::StructuredCredit(s) => Some(s.legal_final_maturity),
            Self::CreditDerivative(c) => Some(c.maturity),
            Self::Rate(r) => Some(r.maturity),
            Self::Repo(r) => Some(r.maturity),
            Self::Fx(f) => f.settlement_date,
            Self::Future(f) => Some(f.expiry),
            Self::Option(o) => Some(o.expiry),
            Self::Volatility(v) => v.expiry,
            Self::Structured(s) => Some(s.maturity),
            _ => None,
        }
    }

    /// Modified duration, the first-order rate sensitivity, where defined.
    pub fn modified_duration(&self) -> Option<f64> {
        match self {
            Self::Bond(b) => Some(b.modified_duration),
            Self::Loan(l) => Some(l.modified_duration),
            Self::StructuredCredit(s) => Some(s.effective_duration),
            Self::Rate(r) => Some(r.dv01_per_million / 100.0),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EquityDetails {
    pub shares_outstanding: Decimal,
    pub free_float_shares: Decimal,
    pub dividend_yield: f64,
    pub book_value_per_share: Decimal,
    pub earnings_per_share: Decimal,
    /// Ordinary shares carry 1.0; a preferred line records its own rank.
    pub voting_rights_per_share: f64,
    pub listing_status: ListingStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gics_sub_industry: Option<String>,
}

/// Whether a line is still trading.
///
/// Deliberately has no `Default`. The only variant a default could name is
/// `Listed`, and `Listed` is the permissive one: a suspended or delisted line
/// arriving in a vendor record that omits the field would read as a tradable
/// one. There is no "unknown" variant to fall back to either, and inventing a
/// listing state is worse than declining to have one — so the field has to be
/// stated, and `EquityDetails` accordingly does not derive `Default` and does
/// not mark this `#[serde(default)]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingStatus {
    Listed,
    Suspended,
    Delisted,
    PreIpo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BondDetails {
    pub issuer: String,
    pub coupon_rate: f64,
    pub coupon_frequency: CouponFrequency,
    pub maturity: Timestamp,
    pub issue_date: Timestamp,
    pub face_value: Decimal,
    pub day_count: DayCount,
    pub seniority: Seniority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_rating: Option<CreditRating>,
    /// Yield to maturity as a decimal, e.g. 0.0435.
    pub yield_to_maturity: f64,
    pub modified_duration: f64,
    pub convexity: f64,
    /// Option-adjusted spread over the risk-free curve, in basis points.
    pub option_adjusted_spread_bps: f64,
    pub callable: bool,
    pub puttable: bool,
    /// Set for inflation-linked issues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inflation_index: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CouponFrequency {
    Zero,
    Annual,
    SemiAnnual,
    Quarterly,
    Monthly,
}

impl CouponFrequency {}

/// Day-count convention. Getting this wrong misprices accrued interest, so it
/// is explicit on every instrument rather than assumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DayCount {
    Thirty360,
    ActualActual,
    Actual360,
    Actual365,
}

impl DayCount {
    /// Year fraction between two dates under this convention.
    pub fn year_fraction(&self, from: Timestamp, to: Timestamp) -> f64 {
        let days = to.since(from).as_days_f64();
        match self {
            Self::Actual360 => days / 360.0,
            Self::Actual365 | Self::ActualActual => days / 365.0,
            Self::Thirty360 => {
                let (y1, m1, d1) = from.civil_date();
                let (y2, m2, d2) = to.civil_date();
                let d1 = d1.min(30);
                let d2 = if d1 == 30 { d2.min(30) } else { d2 };
                let counted =
                    360 * (y2 - y1) + 30 * (m2 as i32 - m1 as i32) + (d2 as i32 - d1 as i32);
                f64::from(counted) / 360.0
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Seniority {
    SecuredFirstLien,
    SecuredSecondLien,
    SeniorUnsecured,
    Subordinated,
    JuniorSubordinated,
    Equity,
}

/// Rating on a unified scale, so agency notations can be compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CreditRating {
    Aaa,
    Aa,
    A,
    Baa,
    Ba,
    B,
    Caa,
    Ca,
    C,
    D,
}

impl CreditRating {
    /// Whether the rating is investment grade (Baa/BBB and above).
    pub fn is_investment_grade(&self) -> bool {
        *self <= Self::Baa
    }

    /// Approximate through-the-cycle one-year default rate, used as a prior
    /// where no issuer-specific model is available.
    pub fn indicative_default_probability(&self) -> f64 {
        match self {
            Self::Aaa => 0.0001,
            Self::Aa => 0.0003,
            Self::A => 0.0008,
            Self::Baa => 0.0025,
            Self::Ba => 0.011,
            Self::B => 0.038,
            Self::Caa => 0.120,
            Self::Ca => 0.250,
            Self::C => 0.400,
            Self::D => 1.0,
        }
    }

    /// Parse the common agency notations onto the unified scale.
    ///
    /// Strips both modifier styles: S&P/Fitch use `+`/`-` (BBB+), Moody's uses
    /// numerals (Baa1). Both collapse to the same unified grade.
    pub fn parse(s: &str) -> Option<Self> {
        let normalised: String = s
            .trim()
            .to_ascii_uppercase()
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .collect();
        Some(match normalised.as_str() {
            "AAA" => Self::Aaa,
            "AA" => Self::Aa,
            "A" => Self::A,
            "BBB" | "BAA" => Self::Baa,
            "BB" | "BA" => Self::Ba,
            "B" => Self::B,
            "CCC" | "CAA" => Self::Caa,
            "CC" | "CA" => Self::Ca,
            "C" => Self::C,
            "D" | "SD" | "RD" => Self::D,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoanDetails {
    pub borrower: String,
    pub maturity: Timestamp,
    pub commitment: Decimal,
    pub drawn: Decimal,
    /// Spread over the floating benchmark, in basis points.
    pub spread_bps: f64,
    pub benchmark: String,
    pub seniority: Seniority,
    pub modified_duration: f64,
    pub covenant_lite: bool,
    /// Leverage of the borrower, a primary covenant metric.
    pub net_debt_to_ebitda: f64,
    pub is_amortising: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StructuredCreditDetails {
    pub deal_name: String,
    pub tranche: String,
    /// Subordination below this tranche, as a fraction of the deal.
    pub attachment_point: f64,
    /// Where this tranche is wiped out, as a fraction of the deal.
    pub detachment_point: f64,
    pub weighted_average_life_years: f64,
    pub effective_duration: f64,
    /// Constant prepayment rate assumption used for the quoted analytics.
    pub prepayment_speed: f64,
    pub collateral_type: String,
    pub legal_final_maturity: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_rating: Option<CreditRating>,
}

impl StructuredCreditDetails {
    /// Thickness of the tranche: how much loss it absorbs before wipe-out.
    pub fn tranche_thickness(&self) -> f64 {
        (self.detachment_point - self.attachment_point).max(0.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreditDerivativeDetails {
    pub reference_entity: String,
    pub maturity: Timestamp,
    pub notional: Decimal,
    pub spread_bps: f64,
    pub recovery_assumption: f64,
    /// Positive when the position is long protection (short credit risk).
    pub is_protection_buyer: bool,
    pub seniority: Seniority,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateDetails {
    pub notional: Decimal,
    pub fixed_rate: f64,
    pub floating_index: String,
    pub maturity: Timestamp,
    pub effective_date: Timestamp,
    pub payment_frequency: CouponFrequency,
    pub day_count: DayCount,
    /// Value change for a one-basis-point move, per million of notional.
    pub dv01_per_million: f64,
    /// True when the position pays fixed and receives floating.
    pub pays_fixed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepoDetails {
    pub collateral_object: String,
    pub maturity: Timestamp,
    pub repo_rate: f64,
    pub haircut: f64,
    pub is_reverse: bool,
    pub counterparty: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FxDetails {
    pub base_currency: Currency,
    pub quote_currency: Currency,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_date: Option<Timestamp>,
    /// Forward points in pips over spot; zero for a spot pair.
    pub forward_points: f64,
    /// Interest rate differential implied by covered interest parity.
    pub rate_differential: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommodityDetails {
    pub commodity: String,
    pub grade: String,
    pub delivery_location: String,
    pub unit: String,
    /// Storage cost per unit per year, part of the cost of carry.
    pub storage_cost_rate: f64,
    /// Benefit of holding the physical, the other half of carry.
    pub convenience_yield: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FutureDetails {
    pub underlying_object_id: String,
    pub expiry: Timestamp,
    pub contract_size: Decimal,
    pub tick_size: Decimal,
    pub tick_value: Decimal,
    pub initial_margin: Decimal,
    pub maintenance_margin: Decimal,
    pub settlement: SettlementStyle,
    /// Position in the expiry cycle: 1 is the front month.
    pub contract_month_index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementStyle {
    Cash,
    Physical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OptionDetails {
    pub underlying_object_id: String,
    pub strike: Decimal,
    pub expiry: Timestamp,
    pub option_type: OptionType,
    pub exercise_style: ExerciseStyle,
    pub contract_multiplier: Decimal,
    pub settlement: SettlementStyle,
    pub implied_volatility: f64,
    pub greeks: Greeks,
    #[serde(default)]
    pub open_interest: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionType {
    Call,
    Put,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExerciseStyle {
    European,
    American,
    Bermudan,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VolatilityDetails {
    pub underlying_object_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry: Option<Timestamp>,
    /// Strike expressed in variance terms for a variance swap.
    pub variance_strike: f64,
    pub vega_notional: Decimal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greeks: Option<Greeks>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DigitalAssetDetails {
    pub chain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_address: Option<String>,
    pub decimals: u8,
    pub circulating_supply: Decimal,
    pub max_supply: Option<Decimal>,
    pub staking_yield: f64,
    /// True when the token represents a claim on an off-chain security.
    pub is_tokenized_security: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FundDetails {
    pub strategy: String,
    pub net_asset_value: Decimal,
    pub assets_under_management: Decimal,
    pub management_fee: f64,
    pub performance_fee: f64,
    pub high_water_mark: bool,
    pub redemption_frequency: RedemptionFrequency,
    /// Notice required before a redemption, in days.
    pub redemption_notice_days: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedemptionFrequency {
    Daily,
    Weekly,
    Monthly,
    Quarterly,
    Annual,
    Locked,
}

impl RedemptionFrequency {
    /// Typical days to liquidate, feeding the liquidity limits.
    pub fn days_to_liquidity(&self) -> u32 {
        match self {
            Self::Daily => 1,
            Self::Weekly => 7,
            Self::Monthly => 30,
            Self::Quarterly => 90,
            Self::Annual => 365,
            Self::Locked => 3650,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrivateAssetDetails {
    pub vintage_year: u32,
    pub committed_capital: Decimal,
    pub called_capital: Decimal,
    pub distributed_capital: Decimal,
    pub residual_value: Decimal,
    /// Investment stage or strategy, e.g. `buyout`, `series_b`, `secondary`.
    pub stage: String,
    pub lockup_years: f64,
    /// Time between a capital call and its due date, in days.
    pub capital_call_notice_days: u32,
}

impl PrivateAssetDetails {
    /// Total value to paid-in: `(distributions + residual) / called`.
    pub fn tvpi(&self) -> Option<f64> {
        if self.called_capital.is_zero() {
            return None;
        }
        Some(
            (self.distributed_capital + self.residual_value).to_f64()
                / self.called_capital.to_f64(),
        )
    }

    /// Distributions to paid-in: realised cash returned per unit called.
    pub fn dpi(&self) -> Option<f64> {
        if self.called_capital.is_zero() {
            return None;
        }
        Some(self.distributed_capital.to_f64() / self.called_capital.to_f64())
    }

    /// Capital committed but not yet drawn — a funding obligation, and a real
    /// liquidity risk that portfolio-level cash planning must carry.
    pub fn unfunded_commitment(&self) -> Decimal {
        (self.committed_capital - self.called_capital).max(Decimal::ZERO)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RealAssetDetails {
    pub property_type: String,
    pub location: String,
    /// Net operating income divided by value.
    pub capitalisation_rate: f64,
    pub occupancy_rate: f64,
    pub net_operating_income: Decimal,
    pub loan_to_value: f64,
    pub square_metres: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexDetails {
    pub methodology: String,
    pub constituent_count: u32,
    pub rebalance_frequency: String,
    pub divisor: Decimal,
    pub is_total_return: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CashDetails {
    /// Deposit rate earned on the balance.
    pub deposit_rate: f64,
    /// Days to convert to settled cash; zero for the settlement currency.
    pub days_to_settle: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterparty: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StructuredProductDetails {
    pub payoff_description: String,
    pub maturity: Timestamp,
    pub principal_protected: bool,
    pub participation_rate: f64,
    /// Barrier as a fraction of the initial level, where the payoff has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barrier_level: Option<f64>,
    pub underlying_object_ids: Vec<String>,
    pub issuer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greeks: Option<Greeks>,
}
