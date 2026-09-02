//! The instrument universe: the set of objects the platform knows about.
//!
//! Lookups by object id, symbol and external identifier all have to be fast,
//! because every event that arrives has to be attached to an object before it
//! can mean anything. The registry maintains those indexes and keeps them
//! consistent on insert and removal.

use qip_core::ObjectId;
use qip_core::error::{Error, Result};
use std::collections::{BTreeMap, HashMap};

use crate::asset_class::{AssetClass, InstrumentType};
use crate::identifiers::{IdentifierKind, Identifiers};
use crate::object::FinancialObject;

/// An indexed collection of financial objects.
#[derive(Debug, Default)]
pub struct Universe {
    objects: BTreeMap<String, FinancialObject>,
    by_symbol: HashMap<String, Vec<String>>,
    by_identifier: HashMap<(IdentifierKind, String), String>,
    by_asset_class: BTreeMap<AssetClass, Vec<String>>,
}

impl Universe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Insert or replace an object, rebuilding its index entries.
    pub fn insert(&mut self, object: FinancialObject) -> Result<()> {
        let issues = object.validate();
        if !issues.is_empty() {
            return Err(Error::invalid(format!(
                "refusing to register invalid object {}: {}",
                object.symbol,
                issues.join("; ")
            )));
        }
        let key = object.object_id.as_str().to_string();
        if self.objects.contains_key(&key) {
            self.remove(&object.object_id);
        }

        self.by_symbol
            .entry(normalise_symbol(&object.symbol))
            .or_default()
            .push(key.clone());
        for (kind, value) in object.identifiers.iter() {
            self.by_identifier
                .insert((kind, value.to_ascii_uppercase()), key.clone());
        }
        self.by_asset_class
            .entry(object.asset_class)
            .or_default()
            .push(key.clone());
        self.objects.insert(key, object);
        Ok(())
    }

    pub fn remove(&mut self, id: &ObjectId) -> Option<FinancialObject> {
        let key = id.as_str();
        let object = self.objects.remove(key)?;
        if let Some(list) = self.by_symbol.get_mut(&normalise_symbol(&object.symbol)) {
            list.retain(|k| k != key);
        }
        for (kind, value) in object.identifiers.iter() {
            self.by_identifier
                .remove(&(kind, value.to_ascii_uppercase()));
        }
        if let Some(list) = self.by_asset_class.get_mut(&object.asset_class) {
            list.retain(|k| k != key);
        }
        Some(object)
    }

    pub fn get(&self, id: &ObjectId) -> Option<&FinancialObject> {
        self.objects.get(id.as_str())
    }

    pub fn get_mut(&mut self, id: &ObjectId) -> Option<&mut FinancialObject> {
        self.objects.get_mut(id.as_str())
    }

    /// Look up by object id, returning a descriptive error when absent.
    pub fn require(&self, id: &ObjectId) -> Result<&FinancialObject> {
        self.get(id)
            .ok_or_else(|| Error::not_found(format!("no financial object {id}")))
    }

    /// Objects sharing a symbol. More than one is normal — the same ticker
    /// trades on several venues.
    pub fn by_symbol(&self, symbol: &str) -> Vec<&FinancialObject> {
        self.by_symbol
            .get(&normalise_symbol(symbol))
            .map(|keys| keys.iter().filter_map(|k| self.objects.get(k)).collect())
            .unwrap_or_default()
    }

    /// Look up by an external identifier.
    pub fn by_identifier(&self, kind: IdentifierKind, value: &str) -> Option<&FinancialObject> {
        self.by_identifier
            .get(&(kind, value.to_ascii_uppercase()))
            .and_then(|k| self.objects.get(k))
    }

    /// Resolve against a whole identifier bag, strongest identifier first.
    pub fn resolve(&self, identifiers: &Identifiers) -> Option<&FinancialObject> {
        let mut candidates: Vec<(IdentifierKind, &str)> = identifiers.iter().collect();
        candidates.sort_by(|a, b| {
            b.0.confidence()
                .partial_cmp(&a.0.confidence())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates
            .into_iter()
            .find_map(|(kind, value)| self.by_identifier(kind, value))
    }

    pub fn iter(&self) -> impl Iterator<Item = &FinancialObject> {
        self.objects.values()
    }

    pub fn ids(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.objects
            .keys()
            .map(|k| ObjectId::from_string(k.clone()))
    }

    pub fn of_type(&self, instrument_type: InstrumentType) -> Vec<&FinancialObject> {
        self.objects
            .values()
            .filter(|o| o.instrument_type == instrument_type)
            .collect()
    }

    /// Objects unfit to drive a capital decision, with the reason.
    ///
    /// The kernel logs this at start-up so a degraded universe is visible
    /// before it produces a bad trade rather than after.
    pub fn not_decision_grade(&self) -> Vec<(&FinancialObject, String)> {
        self.objects
            .values()
            .filter(|o| !o.is_decision_grade())
            .map(|o| {
                let reason = if !o.provenance.licensing.allows_production_decisions() {
                    format!(
                        "licensing class {:?} is not production-eligible",
                        o.provenance.licensing
                    )
                } else if !o.price.is_positive() {
                    "price is not positive".to_string()
                } else if !o.risk.is_coherent() {
                    "risk characteristics are incoherent".to_string()
                } else {
                    format!("data quality {:.2} below floor", o.quality.score())
                };
                (o, reason)
            })
            .collect()
    }

    /// Count by asset class, for the platform overview.
    pub fn composition(&self) -> BTreeMap<AssetClass, usize> {
        let mut out = BTreeMap::new();
        for object in self.objects.values() {
            *out.entry(object.asset_class).or_insert(0) += 1;
        }
        out
    }
}

/// Symbols are matched case- and whitespace-insensitively.
fn normalise_symbol(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase()
}
