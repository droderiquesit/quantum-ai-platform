//! The instrument taxonomy.
//!
//! Two levels: a coarse [`AssetClass`] used for exposure limits and reporting,
//! and a fine [`InstrumentType`] that determines which [`crate::Extension`] an
//! object carries and which pricing and risk models apply.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Coarse classification, used by risk limits and portfolio reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    Equity,
    FixedIncome,
    Credit,
    Rates,
    ForeignExchange,
    Commodity,
    Derivative,
    DigitalAsset,
    Fund,
    PrivateMarket,
    RealAsset,
    Cash,
    StructuredProduct,
}

impl AssetClass {
    pub const ALL: [Self; 13] = [
        Self::Equity,
        Self::FixedIncome,
        Self::Credit,
        Self::Rates,
        Self::ForeignExchange,
        Self::Commodity,
        Self::Derivative,
        Self::DigitalAsset,
        Self::Fund,
        Self::PrivateMarket,
        Self::RealAsset,
        Self::Cash,
        Self::StructuredProduct,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Equity => "equity",
            Self::FixedIncome => "fixed_income",
            Self::Credit => "credit",
            Self::Rates => "rates",
            Self::ForeignExchange => "foreign_exchange",
            Self::Commodity => "commodity",
            Self::Derivative => "derivative",
            Self::DigitalAsset => "digital_asset",
            Self::Fund => "fund",
            Self::PrivateMarket => "private_market",
            Self::RealAsset => "real_asset",
            Self::Cash => "cash",
            Self::StructuredProduct => "structured_product",
        }
    }

    /// Whether positions in this class can normally be exited within a day.
    ///
    /// Drives the liquidity limits and the horizon a risk scenario is run over;
    /// a private-market position cannot be stopped out of at any price.
    pub fn is_typically_liquid(&self) -> bool {
        matches!(
            self,
            Self::Equity
                | Self::Rates
                | Self::ForeignExchange
                | Self::Derivative
                | Self::DigitalAsset
                | Self::Cash
                | Self::Commodity
        )
    }

    /// Whether the class carries embedded leverage, so notional exposure can
    /// exceed the capital committed.
    pub fn is_leveraged_by_construction(&self) -> bool {
        matches!(
            self,
            Self::Derivative | Self::Rates | Self::StructuredProduct
        )
    }
}

impl fmt::Display for AssetClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Fine-grained instrument type. Determines the extension, pricing model and
/// risk treatment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentType {
    // Equity
    CommonStock,
    PreferredStock,
    DepositaryReceipt,
    ExchangeTradedFund,
    Index,
    // Fixed income
    GovernmentBond,
    CorporateBond,
    MunicipalBond,
    InflationLinkedBond,
    ConvertibleBond,
    // Credit
    Loan,
    PrivateCredit,
    StructuredCredit,
    MortgageBackedSecurity,
    AssetBackedSecurity,
    CreditDefaultSwap,
    // Rates
    InterestRateSwap,
    OvernightIndexSwap,
    ForwardRateAgreement,
    YieldCurvePoint,
    Repo,
    // FX
    FxSpot,
    FxForward,
    FxSwap,
    // Commodity
    CommoditySpot,
    CommodityFuture,
    CommoditySwap,
    // Listed derivatives
    Future,
    Forward,
    Option,
    VarianceSwap,
    VolatilityIndexFuture,
    // Digital
    Cryptocurrency,
    TokenizedSecurity,
    // Funds and private markets
    MutualFund,
    HedgeFund,
    PrivateEquityFund,
    VentureCapitalFund,
    InfrastructureFund,
    Secondary,
    // Real assets
    RealEstate,
    // Cash and money markets
    Cash,
    MoneyMarketInstrument,
    // Composite
    StructuredNote,
    SyntheticBasket,
}

impl InstrumentType {
    /// The coarse class this instrument type belongs to.
    pub fn asset_class(&self) -> AssetClass {
        match self {
            Self::CommonStock
            | Self::PreferredStock
            | Self::DepositaryReceipt
            | Self::ExchangeTradedFund
            | Self::Index => AssetClass::Equity,

            Self::GovernmentBond
            | Self::CorporateBond
            | Self::MunicipalBond
            | Self::InflationLinkedBond
            | Self::ConvertibleBond => AssetClass::FixedIncome,

            Self::Loan
            | Self::PrivateCredit
            | Self::StructuredCredit
            | Self::MortgageBackedSecurity
            | Self::AssetBackedSecurity
            | Self::CreditDefaultSwap => AssetClass::Credit,

            Self::InterestRateSwap
            | Self::OvernightIndexSwap
            | Self::ForwardRateAgreement
            | Self::YieldCurvePoint
            | Self::Repo => AssetClass::Rates,

            Self::FxSpot | Self::FxForward | Self::FxSwap => AssetClass::ForeignExchange,

            Self::CommoditySpot | Self::CommodityFuture | Self::CommoditySwap => {
                AssetClass::Commodity
            }

            Self::Future
            | Self::Forward
            | Self::Option
            | Self::VarianceSwap
            | Self::VolatilityIndexFuture => AssetClass::Derivative,

            Self::Cryptocurrency | Self::TokenizedSecurity => AssetClass::DigitalAsset,

            Self::MutualFund | Self::HedgeFund => AssetClass::Fund,

            Self::PrivateEquityFund
            | Self::VentureCapitalFund
            | Self::InfrastructureFund
            | Self::Secondary => AssetClass::PrivateMarket,

            Self::RealEstate => AssetClass::RealAsset,

            Self::Cash | Self::MoneyMarketInstrument => AssetClass::Cash,

            Self::StructuredNote | Self::SyntheticBasket => AssetClass::StructuredProduct,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CommonStock => "common_stock",
            Self::PreferredStock => "preferred_stock",
            Self::DepositaryReceipt => "depositary_receipt",
            Self::ExchangeTradedFund => "exchange_traded_fund",
            Self::Index => "index",
            Self::GovernmentBond => "government_bond",
            Self::CorporateBond => "corporate_bond",
            Self::MunicipalBond => "municipal_bond",
            Self::InflationLinkedBond => "inflation_linked_bond",
            Self::ConvertibleBond => "convertible_bond",
            Self::Loan => "loan",
            Self::PrivateCredit => "private_credit",
            Self::StructuredCredit => "structured_credit",
            Self::MortgageBackedSecurity => "mortgage_backed_security",
            Self::AssetBackedSecurity => "asset_backed_security",
            Self::CreditDefaultSwap => "credit_default_swap",
            Self::InterestRateSwap => "interest_rate_swap",
            Self::OvernightIndexSwap => "overnight_index_swap",
            Self::ForwardRateAgreement => "forward_rate_agreement",
            Self::YieldCurvePoint => "yield_curve_point",
            Self::Repo => "repo",
            Self::FxSpot => "fx_spot",
            Self::FxForward => "fx_forward",
            Self::FxSwap => "fx_swap",
            Self::CommoditySpot => "commodity_spot",
            Self::CommodityFuture => "commodity_future",
            Self::CommoditySwap => "commodity_swap",
            Self::Future => "future",
            Self::Forward => "forward",
            Self::Option => "option",
            Self::VarianceSwap => "variance_swap",
            Self::VolatilityIndexFuture => "volatility_index_future",
            Self::Cryptocurrency => "cryptocurrency",
            Self::TokenizedSecurity => "tokenized_security",
            Self::MutualFund => "mutual_fund",
            Self::HedgeFund => "hedge_fund",
            Self::PrivateEquityFund => "private_equity_fund",
            Self::VentureCapitalFund => "venture_capital_fund",
            Self::InfrastructureFund => "infrastructure_fund",
            Self::Secondary => "secondary",
            Self::RealEstate => "real_estate",
            Self::Cash => "cash",
            Self::MoneyMarketInstrument => "money_market_instrument",
            Self::StructuredNote => "structured_note",
            Self::SyntheticBasket => "synthetic_basket",
        }
    }

    /// Whether the instrument derives its value from another object, which is
    /// what makes an `underlying` mandatory on the object.
    pub fn is_derivative(&self) -> bool {
        matches!(
            self,
            Self::Option
                | Self::Future
                | Self::Forward
                | Self::CommodityFuture
                | Self::VarianceSwap
                | Self::VolatilityIndexFuture
                | Self::CreditDefaultSwap
                | Self::InterestRateSwap
                | Self::OvernightIndexSwap
                | Self::ForwardRateAgreement
                | Self::FxForward
                | Self::FxSwap
                | Self::CommoditySwap
                | Self::ConvertibleBond
        )
    }

    /// Whether the instrument has a contractual redemption date.
    pub fn has_maturity(&self) -> bool {
        matches!(
            self,
            Self::GovernmentBond
                | Self::CorporateBond
                | Self::MunicipalBond
                | Self::InflationLinkedBond
                | Self::ConvertibleBond
                | Self::Loan
                | Self::PrivateCredit
                | Self::StructuredCredit
                | Self::MortgageBackedSecurity
                | Self::AssetBackedSecurity
                | Self::CreditDefaultSwap
                | Self::InterestRateSwap
                | Self::OvernightIndexSwap
                | Self::ForwardRateAgreement
                | Self::Repo
                | Self::FxForward
                | Self::FxSwap
                | Self::Future
                | Self::Forward
                | Self::Option
                | Self::CommodityFuture
                | Self::CommoditySwap
                | Self::VarianceSwap
                | Self::VolatilityIndexFuture
                | Self::StructuredNote
                | Self::MoneyMarketInstrument
        )
    }
}

impl fmt::Display for InstrumentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// GICS-style sector, used for concentration limits and factor attribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Sector {
    Energy,
    Materials,
    Industrials,
    ConsumerDiscretionary,
    ConsumerStaples,
    HealthCare,
    Financials,
    InformationTechnology,
    CommunicationServices,
    Utilities,
    RealEstate,
    Government,
    Diversified,
    #[default]
    Unclassified,
}

impl Sector {
    pub const ALL: [Self; 14] = [
        Self::Energy,
        Self::Materials,
        Self::Industrials,
        Self::ConsumerDiscretionary,
        Self::ConsumerStaples,
        Self::HealthCare,
        Self::Financials,
        Self::InformationTechnology,
        Self::CommunicationServices,
        Self::Utilities,
        Self::RealEstate,
        Self::Government,
        Self::Diversified,
        Self::Unclassified,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Energy => "energy",
            Self::Materials => "materials",
            Self::Industrials => "industrials",
            Self::ConsumerDiscretionary => "consumer_discretionary",
            Self::ConsumerStaples => "consumer_staples",
            Self::HealthCare => "health_care",
            Self::Financials => "financials",
            Self::InformationTechnology => "information_technology",
            Self::CommunicationServices => "communication_services",
            Self::Utilities => "utilities",
            Self::RealEstate => "real_estate",
            Self::Government => "government",
            Self::Diversified => "diversified",
            Self::Unclassified => "unclassified",
        }
    }
}

impl fmt::Display for Sector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
