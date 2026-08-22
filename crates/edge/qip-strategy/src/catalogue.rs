//! What features exist, and what type each one has.
//!
//! The compiler checks every referenced feature against this. An undeclared
//! dependency is not a compile-time inconvenience — it is a lookup that misses
//! inside the latency budget, on a strategy that has already been approved.
//!
//! The catalogue is a plain declaration rather than a handle to the feature
//! graph on purpose. The strategy compiler has no dependency on the DAG
//! implementation and does not want one: it needs the vocabulary, not the
//! machinery. Whatever owns the graph fills this in — one entry per registered
//! node, with the kind that node's definition declares.

use crate::ir::Type;
use qip_contracts::FeatureKey;
use qip_core::error::{Error, Result};
use std::collections::BTreeMap;

/// The features a strategy is allowed to name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FeatureCatalogue {
    entries: BTreeMap<String, (FeatureKey, Type)>,
}

impl FeatureCatalogue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare a feature and the type it produces.
    ///
    /// Redeclaring a key with a different type is refused: two nodes cannot
    /// share a name and disagree about what they are, and letting the last
    /// declaration win would make a compilation depend on registration order.
    pub fn declare(&mut self, key: FeatureKey, value_type: Type) -> Result<()> {
        let canonical = key.canonical();
        if let Some((_, existing)) = self.entries.get(&canonical)
            && *existing != value_type
        {
            return Err(Error::schema(format!(
                "feature {canonical} is already declared as {existing}, not {value_type}"
            )));
        }
        self.entries.insert(canonical, (key, value_type));
        Ok(())
    }

    /// Declare several at once.
    pub fn declaring(mut self, entries: impl IntoIterator<Item = (FeatureKey, Type)>) -> Result<Self> {
        for (key, value_type) in entries {
            self.declare(key, value_type)?;
        }
        Ok(self)
    }

    pub fn type_of(&self, key: &FeatureKey) -> Option<Type> {
        self.entries.get(&key.canonical()).map(|(_, ty)| *ty)
    }

    pub fn contains(&self, key: &FeatureKey) -> bool {
        self.entries.contains_key(&key.canonical())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every declared feature, in canonical order.
    pub fn keys(&self) -> Vec<&FeatureKey> {
        self.entries.values().map(|(key, _)| key).collect()
    }
}
