//! The vocabulary a generated strategy is allowed to say anything in.
//!
//! A grammar needs to know two things about every feature it may name: that it
//! exists, and what type it produces. [`qip_strategy::FeatureCatalogue`]
//! already holds exactly that, so the palette is a view over one rather than a
//! second list that can disagree with it — a generator working from its own
//! copy would eventually emit a feature the compiler has never heard of and
//! blame the compiler.
//!
//! The palette does *not* promise that every key in it is registered in the
//! catalogue the compiler will use. It is built from a catalogue, and the
//! compiler may be holding a different one — that is the ordinary case when a
//! feature has been proposed by [`crate::discovery`] and not yet declared into
//! the graph. Such a candidate is refused at compile time with the compiler's
//! own `not_found`, which is the behaviour we want: the discard says which
//! feature is missing, and no pre-check in this crate has to be kept in step
//! with the compiler's.

use qip_contracts::FeatureKey;
use qip_core::ObjectId;
use qip_core::error::{Error, Result};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::ir::Type;
use std::collections::BTreeMap;

/// The features a generated strategy may name, grouped by the type each one
/// produces.
///
/// Ordering is canonical throughout — the catalogue hands its keys back in
/// canonical order and nothing here re-sorts them — so a draw from a seeded
/// stream picks the same feature on every run.
#[derive(Clone, Debug, PartialEq)]
pub struct FeaturePalette {
    subject: ObjectId,
    by_type: BTreeMap<Type, Vec<FeatureKey>>,
}

impl FeaturePalette {
    /// Every feature in `catalogue` whose subject is `subject`.
    ///
    /// Refuses an empty palette rather than returning one. A generator with no
    /// vocabulary produces strategies made entirely of literals, every one of
    /// which compiles and none of which reads the market; that is a silent
    /// waste of a search budget, and the loudest place to catch it is here.
    pub fn from_catalogue(catalogue: &FeatureCatalogue, subject: &ObjectId) -> Result<Self> {
        let mut by_type: BTreeMap<Type, Vec<FeatureKey>> = BTreeMap::new();
        for key in catalogue.keys() {
            if key.subject != *subject {
                continue;
            }
            let Some(value_type) = catalogue.type_of(key) else {
                continue;
            };
            by_type.entry(value_type).or_default().push(key.clone());
        }
        if by_type.is_empty() {
            return Err(Error::not_found(format!(
                "no feature in the catalogue has {} as its subject, so a generator \
                 built on it could only write literals",
                subject.as_str()
            )));
        }
        Ok(Self {
            subject: subject.clone(),
            by_type,
        })
    }

    /// Build a palette directly from keys and their types.
    ///
    /// For the case where the vocabulary is being proposed rather than read —
    /// a discovery run offering features that are not registered yet. The
    /// resulting candidates are refused by the compiler until somebody
    /// declares them, which is the intended friction.
    pub fn from_keys(
        subject: ObjectId,
        entries: impl IntoIterator<Item = (FeatureKey, Type)>,
    ) -> Result<Self> {
        let mut by_type: BTreeMap<Type, Vec<FeatureKey>> = BTreeMap::new();
        for (key, value_type) in entries {
            by_type.entry(value_type).or_default().push(key);
        }
        for keys in by_type.values_mut() {
            keys.sort_by_key(FeatureKey::canonical);
            keys.dedup_by_key(|key| key.canonical());
        }
        if by_type.is_empty() {
            return Err(Error::not_found(format!(
                "an empty palette for {} can only write literals",
                subject.as_str()
            )));
        }
        Ok(Self { subject, by_type })
    }

    /// The instrument every generated strategy will be about.
    pub const fn subject(&self) -> &ObjectId {
        &self.subject
    }

    /// The features of one type, in canonical order. Empty where the palette
    /// has none.
    pub fn of_type(&self, value_type: Type) -> &[FeatureKey] {
        self.by_type
            .get(&value_type)
            .map_or(&[], |keys| keys.as_slice())
    }

    /// The types the palette can offer at all, in a stable order.
    pub fn types(&self) -> Vec<Type> {
        self.by_type.keys().copied().collect()
    }

    /// Types arithmetic and comparison are meaningful on.
    pub fn numeric_types(&self) -> Vec<Type> {
        self.types()
            .into_iter()
            .filter(Type::is_numeric)
            .collect::<Vec<_>>()
    }

    /// The declared type of a key the palette holds, or `None` for one it does
    /// not.
    pub fn type_of(&self, key: &FeatureKey) -> Option<Type> {
        let canonical = key.canonical();
        self.by_type.iter().find_map(|(value_type, keys)| {
            keys.iter()
                .any(|held| held.canonical() == canonical)
                .then_some(*value_type)
        })
    }

    /// How many features the palette holds.
    pub fn len(&self) -> usize {
        self.by_type.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
