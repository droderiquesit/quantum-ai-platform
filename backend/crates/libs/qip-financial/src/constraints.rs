//! Regulatory and policy constraints carried by an instrument.
//!
//! Compliance decisions are deterministic here by design. Section 5 of the
//! charter requires that compliance rules be evaluated by code rather than
//! delegated to a language model's judgement, so these are plain predicates the
//! compliance agent evaluates and cannot argue with.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Regulatory jurisdiction governing an instrument or venue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Jurisdiction {
    UnitedStates,
    EuropeanUnion,
    UnitedKingdom,
    Switzerland,
    Japan,
    HongKong,
    Singapore,
    Australia,
    Canada,
    Other,
}

impl Jurisdiction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UnitedStates => "united_states",
            Self::EuropeanUnion => "european_union",
            Self::UnitedKingdom => "united_kingdom",
            Self::Switzerland => "switzerland",
            Self::Japan => "japan",
            Self::HongKong => "hong_kong",
            Self::Singapore => "singapore",
            Self::Australia => "australia",
            Self::Canada => "canada",
            Self::Other => "other",
        }
    }
}

/// A specific prohibition on trading an instrument.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "restriction", rename_all = "snake_case")]
pub enum TradingRestriction {
    /// Named on a sanctions list; no trading in any direction.
    Sanctioned { authority: String },
    /// On the firm's restricted list, typically for information reasons.
    RestrictedList { reason: String },
    /// Short selling prohibited.
    NoShortSelling,
    /// Only eligible for professional or qualified investors.
    ProfessionalInvestorsOnly,
    /// Blocked in a specific jurisdiction.
    JurisdictionBlocked { jurisdiction: Jurisdiction },
    /// Trading halted by the venue.
    TradingHalt { reason: String },
    /// Cannot be traded during a blackout window.
    BlackoutPeriod { reason: String },
}

impl TradingRestriction {
    /// Whether this restriction blocks buying.
    pub fn blocks_buy(&self) -> bool {
        !matches!(self, Self::NoShortSelling)
    }

    /// Whether this restriction blocks selling short.
    pub fn blocks_short(&self) -> bool {
        true
    }

    /// Whether this restriction blocks closing an existing position.
    ///
    /// Sanctions and halts trap a position; a restricted-list entry or a
    /// short-selling ban does not stop an orderly exit. Getting this wrong in
    /// either direction is a compliance incident, so it is stated explicitly.
    pub fn blocks_liquidation(&self) -> bool {
        matches!(self, Self::Sanctioned { .. } | Self::TradingHalt { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Sanctioned { authority } => format!("sanctioned by {authority}"),
            Self::RestrictedList { reason } => format!("on the restricted list: {reason}"),
            Self::NoShortSelling => "short selling prohibited".into(),
            Self::ProfessionalInvestorsOnly => "professional investors only".into(),
            Self::JurisdictionBlocked { jurisdiction } => {
                format!("blocked in {}", jurisdiction.as_str())
            }
            Self::TradingHalt { reason } => format!("trading halted: {reason}"),
            Self::BlackoutPeriod { reason } => format!("blackout period: {reason}"),
        }
    }
}

/// The full regulatory picture for one instrument.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RegulatoryConstraints {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub jurisdictions: BTreeSet<Jurisdiction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restrictions: Vec<TradingRestriction>,
    /// Venues on which this instrument may be traded. Empty means unrestricted.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub approved_venues: BTreeSet<String>,
    /// Regulatory capital charge as a fraction of notional, where one applies.
    #[serde(default)]
    pub capital_charge: f64,
    /// True when the instrument requires a reporting obligation on trade.
    #[serde(default)]
    pub requires_trade_reporting: bool,
}

impl RegulatoryConstraints {
    pub fn unrestricted() -> Self {
        Self::default()
    }

    pub fn with_jurisdiction(mut self, jurisdiction: Jurisdiction) -> Self {
        self.jurisdictions.insert(jurisdiction);
        self
    }

    pub fn with_restriction(mut self, restriction: TradingRestriction) -> Self {
        self.restrictions.push(restriction);
        self
    }

    pub fn with_approved_venue(mut self, venue: impl Into<String>) -> Self {
        self.approved_venues.insert(venue.into());
        self
    }

    pub fn is_restricted(&self) -> bool {
        !self.restrictions.is_empty()
    }

    /// Restrictions that block opening or increasing a long position.
    pub fn buy_blockers(&self) -> Vec<&TradingRestriction> {
        self.restrictions
            .iter()
            .filter(|r| r.blocks_buy())
            .collect()
    }

    /// Restrictions that block establishing a short position.
    pub fn short_blockers(&self) -> Vec<&TradingRestriction> {
        self.restrictions
            .iter()
            .filter(|r| r.blocks_short())
            .collect()
    }

    /// Restrictions that trap an existing position.
    pub fn liquidation_blockers(&self) -> Vec<&TradingRestriction> {
        self.restrictions
            .iter()
            .filter(|r| r.blocks_liquidation())
            .collect()
    }

    /// Whether `venue` may be used for this instrument.
    pub fn permits_venue(&self, venue: &str) -> bool {
        self.approved_venues.is_empty() || self.approved_venues.contains(venue)
    }
}
