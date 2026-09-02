//! Typed relationships between things in the world.

use serde::{Deserialize, Serialize};

/// The kinds of edge the graph carries.
///
/// A closed set: adding one forces every exhaustive match to acknowledge it.
///
/// A relationship is structure, never a path a shock travels along. The
/// causal layer does not traverse these edges at all: propagation runs over
/// [`crate::causal::CausalEdge`]s, each claimed with a mechanism and a
/// transmission, and an index membership or an officer's seat becomes
/// contagion only when somebody claims the mechanism by which it would. There
/// used to be a `transmits_shock` predicate here that said which kinds could
/// carry a shock; nothing consulted it, because the rule it stated is held
/// structurally by the causal graph, and a bound nothing consults reads as
/// protection and is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    /// A company issues a security.
    Issues,
    /// A person holds an office at a company.
    Officer,
    /// A supplies inputs to B.
    Supplies,
    /// A buys from B. The inverse of `Supplies`.
    Customer,
    /// A and B compete in the same market.
    Competitor,
    /// A is a subsidiary of B.
    SubsidiaryOf,
    /// A belongs to an industry.
    InIndustry,
    /// An industry consumes a commodity.
    ConsumesCommodity,
    /// An industry produces a commodity.
    ProducesCommodity,
    /// A company is domiciled in a country.
    DomiciledIn,
    /// A company has material revenue exposure to a country.
    RevenueExposure,
    /// A country issues a currency.
    IssuesCurrency,
    /// A central bank sets policy for a country or region.
    SetsPolicyFor,
    /// A security is a constituent of an index.
    ConstituentOf,
    /// A derivative references an underlying.
    DerivesFrom,
    /// An event concerns an entity.
    ConcernsEntity,
    /// An asset loads on a risk factor.
    LoadsOnFactor,
    /// A security is held in a portfolio.
    HeldIn,
    /// A strategy expresses a thesis.
    ExpressesThesis,
    /// A piece of evidence supports a thesis.
    SupportsThesis,
    /// A piece of evidence contradicts a thesis.
    ContradictsThesis,
    /// A counterparty relationship, for exposure aggregation.
    CounterpartyOf,
}

impl RelationshipKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Issues => "issues",
            Self::Officer => "officer",
            Self::Supplies => "supplies",
            Self::Customer => "customer",
            Self::Competitor => "competitor",
            Self::SubsidiaryOf => "subsidiary_of",
            Self::InIndustry => "in_industry",
            Self::ConsumesCommodity => "consumes_commodity",
            Self::ProducesCommodity => "produces_commodity",
            Self::DomiciledIn => "domiciled_in",
            Self::RevenueExposure => "revenue_exposure",
            Self::IssuesCurrency => "issues_currency",
            Self::SetsPolicyFor => "sets_policy_for",
            Self::ConstituentOf => "constituent_of",
            Self::DerivesFrom => "derives_from",
            Self::ConcernsEntity => "concerns_entity",
            Self::LoadsOnFactor => "loads_on_factor",
            Self::HeldIn => "held_in",
            Self::ExpressesThesis => "expresses_thesis",
            Self::SupportsThesis => "supports_thesis",
            Self::ContradictsThesis => "contradicts_thesis",
            Self::CounterpartyOf => "counterparty_of",
        }
    }

    /// The edge implied in the opposite direction, where one exists.
    ///
    /// Stored explicitly rather than inferred at query time so traversal in
    /// either direction is a plain lookup.
    pub fn inverse(&self) -> Option<RelationshipKind> {
        match self {
            Self::Supplies => Some(Self::Customer),
            Self::Customer => Some(Self::Supplies),
            Self::Competitor => Some(Self::Competitor),
            Self::CounterpartyOf => Some(Self::CounterpartyOf),
            _ => None,
        }
    }

    /// Whether the relationship is symmetric.
    pub fn is_symmetric(&self) -> bool {
        matches!(self, Self::Competitor | Self::CounterpartyOf)
    }
}

/// An edge in the knowledge graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relationship {
    pub from: String,
    pub to: String,
    pub kind: RelationshipKind,
    /// Strength of the relationship in `[0, 1]`: the fraction of revenue, the
    /// share of supply, the size of the exposure.
    pub weight: f64,
    /// Where the claim came from.
    pub source: String,
}

impl Relationship {
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
        kind: RelationshipKind,
        weight: f64,
        source: impl Into<String>,
    ) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            kind,
            weight: weight.clamp(0.0, 1.0),
            source: source.into(),
        }
    }

    /// The reciprocal edge, where the relationship implies one.
    pub fn inverted(&self) -> Option<Relationship> {
        self.kind.inverse().map(|kind| Relationship {
            from: self.to.clone(),
            to: self.from.clone(),
            kind,
            weight: self.weight,
            source: self.source.clone(),
        })
    }

    /// Stable key, so the same claim from the same source is one fact.
    pub fn key(&self) -> String {
        format!("{}|{}|{}", self.from, self.kind.as_str(), self.to)
    }
}
