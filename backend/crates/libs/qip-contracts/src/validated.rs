//! The one shape every brain-to-brain contract added for blueprint v12.0 §27
//! and §24.9 takes: a plain record of named fields, and a [`Validated`]
//! wrapper that can only be obtained by passing that record's checks.
//!
//! The failure this prevents is the one the register found in most of the
//! contracts that predate it: a type whose constructor refuses bad input but
//! whose `#[derive(Deserialize)]` does not, so every value this process built
//! was checked and every value it *read* was not. Here deserialisation of a
//! [`Validated`] goes through [`Validated::new`], byte for byte the same
//! refusals, and there is no other way in.
//!
//! Three further rules hold for every contract built on this:
//!
//! * **No field is optional on the wire.** No contract record carries an
//!   `Option`, because serde reads a missing `Option` field as `None` and a
//!   missing field would then be indistinguishable from a stated absence.
//!   Where absence is a legitimate value it is an explicit enum variant
//!   (`Genesis`, `Unlocated`, `None`) the producer has to write down.
//! * **Unknown fields are refused** (`deny_unknown_fields` on every record),
//!   so a producer one schema ahead is told so rather than silently truncated.
//! * **Every record carries `schema_version`** and is refused at any version
//!   other than the one this build reads; a migration is explicit or absent.
//!
//! The field records have public fields on purpose: they are the builder.
//! Named fields rather than long positional constructors, because several
//! contracts hold two lists of strings or two basis-point integers side by
//! side and a transposed positional call would compile and record the wrong
//! fact.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;
use std::ops::Deref;

/// The largest probability, confidence or share any contract states, in basis
/// points. Integers rather than `f64` so a record compares exactly and a
/// replay cannot drift on rounding.
pub const MAX_BP: u32 = 10_000;

/// A record of named fields that knows how to refuse itself.
pub trait Contract {
    /// The contract's name as the blueprint gives it, used in every refusal.
    const NAME: &'static str;
    /// The only `schema_version` this build reads or writes.
    const SCHEMA_VERSION: u32;

    /// The version the record claims.
    fn schema_version(&self) -> u32;

    /// Every invariant beyond the version. Refuse, never adjust: a value
    /// silently corrected here is a producer bug that survives.
    fn check(&self) -> Result<()>;
}

/// A contract record that has passed its checks. The only constructor is
/// [`Validated::new`], and deserialisation is routed through it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validated<T>(T);

impl<T: Contract> Validated<T> {
    /// Admit `fields`, refusing a foreign schema version first and then
    /// whatever [`Contract::check`] refuses.
    pub fn new(fields: T) -> Result<Self> {
        if fields.schema_version() != T::SCHEMA_VERSION {
            return Err(Error::schema(format!(
                "{} schema version {} is not {}; migrate the record explicitly rather than \
                 reading it as this version",
                T::NAME,
                fields.schema_version(),
                T::SCHEMA_VERSION
            )));
        }
        fields.check()?;
        Ok(Self(fields))
    }

    /// The admitted fields, read-only.
    pub const fn fields(&self) -> &T {
        &self.0
    }

    /// Give the fields back, to build a successor from. The successor is a
    /// new record and must pass [`Validated::new`] again.
    pub fn into_fields(self) -> T {
        self.0
    }
}

impl<T: Contract + Serialize> Validated<T> {
    /// Canonical `serde_json`, the only serialisation this workspace permits.
    pub fn encode(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&self.0)
            .map_err(|e| Error::invalid(format!("{} does not encode: {e}", T::NAME)))
    }
}

impl<T: Contract + for<'de> Deserialize<'de>> Validated<T> {
    /// Decode and admit, refusing a missing, unknown or malformed field as a
    /// schema error that names the contract.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(|e| {
            Error::schema(format!(
                "not a valid {}: {e}; the producer must state every field",
                T::NAME
            ))
        })
    }
}

impl<T> Deref for Validated<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Serialize> Serialize for Validated<T> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T: Contract + Deserialize<'de>> Deserialize<'de> for Validated<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let fields = T::deserialize(d)?;
        Self::new(fields).map_err(|e| serde::de::Error::custom(e.message().to_string()))
    }
}

// ---- Shared refusals. Each names the contract and the field, and says what
// ---- to state instead, so a refusal read in a log is actionable alone.

pub(crate) fn named(contract: &str, field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::invalid(format!(
            "a {contract} needs a non-blank `{field}`; an unnamed value cannot be traced to \
             what produced it"
        )));
    }
    Ok(())
}

/// A non-empty list. Use only where the blueprint makes the list mandatory;
/// where an empty list is a statement ("no contradictions"), check entries
/// with [`each_named`] alone.
pub(crate) fn listed<T>(contract: &str, field: &str, values: &[T]) -> Result<()> {
    if values.is_empty() {
        return Err(Error::invalid(format!(
            "a {contract} needs at least one `{field}`; an empty list here is a record that \
             says nothing about what it depends on"
        )));
    }
    Ok(())
}

/// Every entry non-blank and none repeated. A repeat is refused rather than
/// deduplicated, because a producer that wrote one id twice probably meant two.
pub(crate) fn each_named(contract: &str, field: &str, values: &[String]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        named(contract, field, value)?;
        if !seen.insert(value.as_str()) {
            return Err(Error::invalid(format!(
                "a {contract} lists `{value}` twice in `{field}`; state each entry once"
            )));
        }
    }
    Ok(())
}

/// A mandatory, non-blank, duplicate-free list of names.
pub(crate) fn names(contract: &str, field: &str, values: &[String]) -> Result<()> {
    listed(contract, field, values)?;
    each_named(contract, field, values)
}

pub(crate) fn basis_points(contract: &str, field: &str, value: u32) -> Result<()> {
    if value > MAX_BP {
        return Err(Error::invalid(format!(
            "a {contract}'s `{field}` is {value} basis points, above {MAX_BP}; it is refused \
             rather than clamped, so restate it"
        )));
    }
    Ok(())
}

/// A SHA-256 digest as 64 lowercase hex characters — the form
/// [`qip_core::sha256_hex`] writes. Uppercase is refused rather than folded,
/// because two spellings of one digest compare unequal everywhere else.
pub(crate) fn digest(contract: &str, field: &str, value: &str) -> Result<()> {
    let well_formed = value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !well_formed {
        return Err(Error::invalid(format!(
            "a {contract}'s `{field}` is not a SHA-256 digest (64 lowercase hex characters); \
             compute it with qip_core::sha256_hex over the bytes it names"
        )));
    }
    Ok(())
}

/// `later` strictly after `earlier`.
pub(crate) fn after(
    contract: &str,
    later_field: &str,
    later: Timestamp,
    earlier_field: &str,
    earlier: Timestamp,
) -> Result<()> {
    if later <= earlier {
        return Err(Error::invalid(format!(
            "a {contract}'s `{later_field}` must be after its `{earlier_field}`; a record that \
             expires or ends before it begins is never usable"
        )));
    }
    Ok(())
}

pub(crate) fn positive_duration(contract: &str, field: &str, value: Duration) -> Result<()> {
    if value.as_nanos() <= 0 {
        return Err(Error::invalid(format!(
            "a {contract} needs a positive `{field}`; state how long it holds"
        )));
    }
    Ok(())
}

pub(crate) fn non_negative_duration(contract: &str, field: &str, value: Duration) -> Result<()> {
    if value.as_nanos() < 0 {
        return Err(Error::invalid(format!(
            "a {contract}'s `{field}` is negative; a span of time measured backwards is a \
             producer bug"
        )));
    }
    Ok(())
}

pub(crate) fn positive(contract: &str, field: &str, value: Decimal) -> Result<()> {
    if !value.is_positive() {
        return Err(Error::invalid(format!(
            "a {contract} needs a positive `{field}`, not {value}; a zero or negative amount \
             here bounds nothing"
        )));
    }
    Ok(())
}

pub(crate) fn non_negative(contract: &str, field: &str, value: Decimal) -> Result<()> {
    if value.is_negative() {
        return Err(Error::invalid(format!(
            "a {contract}'s `{field}` is {value}; it cannot be negative"
        )));
    }
    Ok(())
}

pub(crate) fn refuse(contract: &str, why: impl std::fmt::Display) -> Result<()> {
    Err(Error::invalid(format!("a {contract} {why}")))
}

/// Probabilities, in basis points, that must each be in range and sum to
/// exactly [`MAX_BP`]. Exactness is the point of integers: a distribution
/// that sums to 9,999 is missing mass somebody should be able to name.
pub(crate) fn probability_mass(contract: &str, field: &str, values: &[u32]) -> Result<()> {
    let mut total: u64 = 0;
    for value in values {
        basis_points(contract, field, *value)?;
        total += u64::from(*value);
    }
    if total != u64::from(MAX_BP) {
        return Err(Error::invalid(format!(
            "a {contract}'s `{field}` sums to {total} basis points, not {MAX_BP}; a \
             distribution that does not sum to one is refused, not renormalised"
        )));
    }
    Ok(())
}
