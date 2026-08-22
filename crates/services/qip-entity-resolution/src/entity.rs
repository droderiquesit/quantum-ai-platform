//! The canonical entity model.

use qip_core::{EntityId, Timestamp};
use qip_events::{EventBody, Topic};
use qip_financial::identifiers::Identifiers;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What kind of thing an entity is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Company,
    Person,
    Country,
    Sector,
    Industry,
    Commodity,
    Currency,
    Index,
    Venue,
    CentralBank,
    Regulator,
    Fund,
}

impl EntityKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Company => "company",
            Self::Person => "person",
            Self::Country => "country",
            Self::Sector => "sector",
            Self::Industry => "industry",
            Self::Commodity => "commodity",
            Self::Currency => "currency",
            Self::Index => "index",
            Self::Venue => "venue",
            Self::CentralBank => "central_bank",
            Self::Regulator => "regulator",
            Self::Fund => "fund",
        }
    }

    /// Whether entities of this kind carry legal-name suffixes worth stripping
    /// during comparison.
    pub fn has_legal_form(&self) -> bool {
        matches!(self, Self::Company | Self::Fund)
    }
}

/// A canonical entity: one real-world thing, however many providers describe it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub entity_id: EntityId,
    pub kind: EntityKind,
    /// Preferred name, taken from the most reliable source seen.
    pub canonical_name: String,
    /// Every name the entity has been seen under, including former ones.
    pub aliases: Vec<String>,
    pub identifiers: Identifiers,
    /// ISO country code of domicile, where applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Attributes merged from contributing records.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
    /// Sources that have contributed to this entity.
    pub contributing_sources: Vec<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Confidence that this entity is genuinely one thing, in `[0, 1]`.
    ///
    /// Falls when a merge was made on weak evidence, so downstream consumers
    /// can prefer high-confidence entities for consequential reasoning.
    pub coherence: f64,
}

impl Entity {
    pub fn new(
        entity_id: EntityId,
        kind: EntityKind,
        canonical_name: impl Into<String>,
        at: Timestamp,
    ) -> Self {
        let canonical_name = canonical_name.into();
        Self {
            entity_id,
            kind,
            aliases: vec![canonical_name.clone()],
            canonical_name,
            identifiers: Identifiers::new(),
            country: None,
            attributes: BTreeMap::new(),
            contributing_sources: Vec::new(),
            created_at: at,
            updated_at: at,
            coherence: 1.0,
        }
    }

    pub fn with_identifiers(mut self, identifiers: Identifiers) -> Self {
        self.identifiers = identifiers;
        self
    }

    pub fn with_country(mut self, country: impl Into<String>) -> Self {
        self.country = Some(country.into());
        self
    }

    pub fn with_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// Add an alias if it is genuinely new.
    pub fn add_alias(&mut self, alias: impl Into<String>) {
        let alias = alias.into();
        let normalised = crate::matching::normalise_name(&alias);
        if !self
            .aliases
            .iter()
            .any(|existing| crate::matching::normalise_name(existing) == normalised)
        {
            self.aliases.push(alias);
        }
    }

    /// Absorb a record into this entity.
    pub fn absorb(&mut self, record: &EntityRecord, confidence: f64, at: Timestamp) {
        self.add_alias(&record.name);
        self.identifiers.merge(&record.identifiers);
        if self.country.is_none() {
            self.country = record.country.clone();
        }
        for (key, value) in &record.attributes {
            self.attributes
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        if !self.contributing_sources.contains(&record.source) {
            self.contributing_sources.push(record.source.clone());
        }
        self.updated_at = at;
        // Coherence is the weakest link: a single low-confidence merge caps how
        // much the entity as a whole can be trusted.
        self.coherence = self.coherence.min(confidence);
    }

    /// Whether the entity is coherent enough for consequential reasoning.
    pub fn is_reliable(&self) -> bool {
        self.coherence >= 0.85
    }
}

impl EventBody for Entity {
    const TOPIC: Topic = Topic::EntityUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.entity_id, self.updated_at.as_nanos()))
    }
}

/// One provider's view of an entity, before resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityRecord {
    /// Provider's own key for the record.
    pub record_id: String,
    pub kind: EntityKind,
    pub name: String,
    pub identifiers: Identifiers,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
    /// Adapter the record came from.
    pub source: String,
    pub observed_at: Timestamp,
}

impl EntityRecord {
    pub fn new(
        record_id: impl Into<String>,
        kind: EntityKind,
        name: impl Into<String>,
        source: impl Into<String>,
        observed_at: Timestamp,
    ) -> Self {
        Self {
            record_id: record_id.into(),
            kind,
            name: name.into(),
            identifiers: Identifiers::new(),
            country: None,
            attributes: BTreeMap::new(),
            source: source.into(),
            observed_at,
        }
    }

    pub fn with_identifiers(mut self, identifiers: Identifiers) -> Self {
        self.identifiers = identifiers;
        self
    }

    pub fn with_country(mut self, country: impl Into<String>) -> Self {
        self.country = Some(country.into());
        self
    }

    pub fn with_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }
}

/// Published when a record is linked to a canonical entity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityResolved {
    pub record_id: String,
    pub source: String,
    pub entity_id: EntityId,
    pub confidence: f64,
    /// Why the link was made, in words.
    pub rationale: String,
    /// True when a new entity was created rather than an existing one matched.
    pub created_new: bool,
    pub resolved_at: Timestamp,
}

impl EventBody for EntityResolved {
    const TOPIC: Topic = Topic::EntityResolved;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.source, self.record_id))
    }
}
