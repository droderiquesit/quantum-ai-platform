//! The committed instrument catalogue: how a deployed process gets a universe.
//!
//! Every composition root used to assemble `Universe::new()` — an empty
//! universe — so the exposure buckets the kernel projects from the universe
//! at assembly (sector, country, asset class, venue) received nothing in any
//! deployed process, `Universe::not_decision_grade` counted nothing, and two
//! bucket limits in every default set could never fire. Nothing said so: an
//! empty universe looks exactly like a small one from the outside.
//!
//! This module reads a versioned JSON catalogue into a [`Universe`], refusing
//! anything that would leave a record without the fields the buckets and the
//! licensing gate read. The rules, and why each is a refusal rather than a
//! default:
//!
//! - **Every record carries an object id, asset class (through its instrument
//!   type), venue, sector, geography, currency, price and licensing class.**
//!   A record missing one is refused *by name* and takes the whole catalogue
//!   with it: an instrument that reached the universe with `Unclassified`
//!   for a sector would feed the sector bucket a value nobody chose, which is
//!   the defect this module exists to close, rebuilt one record at a time.
//! - **An empty catalogue is refused.** The empty universe is the state that
//!   hid the bucket defect; a catalogue that reproduced it deliberately would
//!   be a way to switch the buckets off from a data file.
//! - **The format carries no maturity and no underlying**, so an instrument
//!   type that needs one is refused rather than given a placeholder. A bond
//!   with an invented maturity is a bond the platform would misprice on
//!   purpose.
//! - **Geography is exactly two upper-case ASCII letters**, the ISO 3166-1
//!   alpha-2 shape the country bucket is keyed on. `us` and `USA` would each
//!   open a second bucket beside `US`, and a concentration limit that reads
//!   two buckets for one country is one that fires late.
//!
//! The catalogue names an instrument, never a data source: there is no host,
//! no endpoint and no vendor identifier in it. The `source` field is the
//! catalogue's own name, recorded as the provenance of every record it
//! produces, and the SHA-256 of the file's bytes is carried on every record's
//! `upstream_id` and in the [`CatalogueManifest`] a root journals at assembly
//! — so a run can say which catalogue it sized against, and two runs that
//! disagree can be told apart by the hash.
//!
//! No I/O here. The caller — a composition root, the only place configuration
//! is read — reads the file and hands the text over, which is what keeps the
//! parser testable without a filesystem and keeps this crate a lib.

use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, KeyValueStore, KeyValueStoreExt, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::asset_class::{InstrumentType, Sector};
use crate::object::FinancialObject;
use crate::quality::{LicensingClass, Provenance};
use crate::universe::{CatalogueOrigin, Universe};

/// The catalogue format this build reads. A file declaring another is
/// refused: a newer schema may carry a field this build would silently
/// drop, and an older one may lack a field this build now requires.
pub const SCHEMA_VERSION: u32 = 1;

/// The key under which the most recently loaded manifest is recorded.
pub const CURRENT_KEY: &str = "current";

/// The key prefix under which every loaded manifest is recorded, by hash.
pub const BY_HASH_PREFIX: &str = "catalogue/";

/// The catalogue file, as written.
///
/// Top-level fields are required by the deserialiser itself: a catalogue
/// with no version is not a record of anything, and serde's own message —
/// which names the missing field — is the right refusal for a file-level
/// omission. Records are read loosely and validated by hand below, because a
/// record-level omission has to name the *record*, which serde cannot.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalogue {
    schema_version: u32,
    /// The catalogue's own version label, chosen by whoever committed it.
    version: String,
    /// When the reference facts in it were true.
    as_of: Timestamp,
    /// The catalogue's own name, recorded as provenance. Not a data source.
    source: String,
    instruments: Vec<serde_json::Value>,
}

/// One record, with every field optional so a missing one is reported by
/// name against the record rather than as a bare deserialisation failure.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecord {
    object_id: Option<String>,
    symbol: Option<String>,
    name: Option<String>,
    instrument_type: Option<InstrumentType>,
    venue: Option<String>,
    sector: Option<Sector>,
    geography: Option<String>,
    currency: Option<String>,
    price: Option<Decimal>,
    lot_size: Option<Decimal>,
    tick_size: Option<Decimal>,
    contract_multiplier: Option<Decimal>,
    issuer: Option<String>,
    licensing: Option<LicensingClass>,
}

/// What a root records about the catalogue it assembled, so the run is
/// reproducible from the record: the file's hash, its version, how many
/// instruments it produced, and which of them the universe itself said may
/// not drive a decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalogueManifest {
    pub schema_version: u32,
    pub version: String,
    pub as_of: Timestamp,
    pub source: String,
    /// SHA-256 of the catalogue text, lowercase hex.
    pub sha256: String,
    pub instruments: usize,
    /// Object id → the reason `Universe::not_decision_grade` gave. A
    /// `BTreeMap` because this reaches the journal, and a replay that
    /// reorders is not a replay.
    pub not_decision_grade: BTreeMap<String, String>,
    /// When the process loaded it — the ingestion instant of every record.
    pub loaded_at: Timestamp,
}

impl CatalogueManifest {
    /// One line for a start-up banner.
    pub fn describe(&self) -> String {
        format!(
            "{} instrument(s) from catalogue {} (sha256 {}); {} not decision-grade",
            self.instruments,
            self.version,
            self.sha256,
            self.not_decision_grade.len()
        )
    }
}

/// A catalogue read into a universe, with the manifest that describes it.
#[derive(Debug)]
pub struct LoadedCatalogue {
    pub manifest: CatalogueManifest,
    pub universe: Universe,
}

/// Read a catalogue's text into a universe, refusing anything incomplete.
///
/// `now` is the ingestion instant stamped on every record's provenance; the
/// catalogue's own `as_of` is the event instant. A catalogue dated after
/// `now` is refused by the object model's own check ("ingested before it
/// happened"), which is the right outcome for a file from the future.
pub fn load(text: &str, now: Timestamp) -> Result<LoadedCatalogue> {
    let sha256 = qip_core::sha256_hex(text.as_bytes());
    let raw: RawCatalogue = serde_json::from_str(text).map_err(|error| {
        Error::invalid(format!("the instrument catalogue does not parse: {error}"))
    })?;
    if raw.schema_version != SCHEMA_VERSION {
        return Err(Error::invalid(format!(
            "the instrument catalogue declares schema_version {}, and this build reads {}; \
             convert the catalogue rather than letting a field be dropped or defaulted",
            raw.schema_version, SCHEMA_VERSION
        )));
    }
    if raw.version.trim().is_empty() {
        return Err(Error::invalid(
            "the instrument catalogue has an empty version; a catalogue nobody can name is one \
             nobody can reproduce a run against",
        ));
    }
    if raw.source.trim().is_empty() {
        return Err(Error::invalid(
            "the instrument catalogue has an empty source; every record's provenance is read \
             from it",
        ));
    }
    if raw.instruments.is_empty() {
        return Err(Error::invalid(format!(
            "the instrument catalogue {} lists no instruments. An empty universe is the state \
             in which the exposure buckets received nothing and nothing said so; commit at \
             least one instrument or do not start the process",
            raw.version
        )));
    }

    let mut universe = Universe::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (index, value) in raw.instruments.iter().enumerate() {
        let label = record_label(index, value);
        let record: RawRecord = serde_json::from_value(value.clone()).map_err(|error| {
            Error::invalid(format!(
                "instrument catalogue {}: {label} does not parse: {error}",
                raw.version
            ))
        })?;
        let object = build(&raw, &record, &label, &sha256, now)?;
        let key = object.object_id.as_str().to_string();
        if !seen.insert(key.clone()) {
            return Err(Error::invalid(format!(
                "instrument catalogue {}: {label} repeats object_id `{key}`; the second record \
                 would silently replace the first in the universe",
                raw.version
            )));
        }
        universe.insert(object).map_err(|error| {
            Error::invalid(format!(
                "instrument catalogue {}: {label} is refused by the universe: {}",
                raw.version,
                error.message()
            ))
        })?;
    }

    let not_decision_grade: BTreeMap<String, String> = universe
        .not_decision_grade()
        .into_iter()
        .map(|(object, reason)| (object.object_id.as_str().to_string(), reason))
        .collect();
    let manifest = CatalogueManifest {
        schema_version: raw.schema_version,
        version: raw.version,
        as_of: raw.as_of,
        source: raw.source,
        sha256,
        instruments: universe.len(),
        not_decision_grade,
        loaded_at: now,
    };
    // Named after the last insert, because inserting clears it. The same
    // three values the manifest carries, copied rather than recomputed, so
    // the manifest a root banners and the origin the kernel journals cannot
    // name two different catalogues.
    let universe = universe.with_origin(CatalogueOrigin {
        version: manifest.version.clone(),
        sha256: manifest.sha256.clone(),
        source: manifest.source.clone(),
    });
    Ok(LoadedCatalogue { manifest, universe })
}

/// Record the manifest in a store, under its hash and as the current one.
///
/// Two keys on purpose. `current` is what an operator asks for; the hashed
/// key is what a replay asks for, and a process that loaded a different
/// catalogue after a restart leaves both the old and the new record behind
/// rather than overwriting the only copy of what the earlier run used.
pub fn record_manifest(store: &dyn KeyValueStore, manifest: &CatalogueManifest) -> Result<()> {
    store.put_as(&format!("{BY_HASH_PREFIX}{}", manifest.sha256), manifest)?;
    store.put_as(CURRENT_KEY, manifest)
}

/// How a record is named in a refusal: its position, and whatever identity
/// it managed to state — so a record missing its object id is still findable.
fn record_label(index: usize, value: &serde_json::Value) -> String {
    let field = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    match (field("object_id"), field("symbol")) {
        (Some(id), Some(symbol)) => format!(
            "record #{} (object_id `{id}`, symbol `{symbol}`)",
            index + 1
        ),
        (Some(id), None) => format!("record #{} (object_id `{id}`)", index + 1),
        (None, Some(symbol)) => format!("record #{} (symbol `{symbol}`)", index + 1),
        (None, None) => format!("record #{} (no object_id and no symbol)", index + 1),
    }
}

fn missing(catalogue: &str, label: &str, field: &str, why: &str) -> Error {
    Error::invalid(format!(
        "instrument catalogue {catalogue}: {label} has no `{field}`; {why}"
    ))
}

fn build(
    catalogue: &RawCatalogue,
    record: &RawRecord,
    label: &str,
    sha256: &str,
    now: Timestamp,
) -> Result<FinancialObject> {
    let version = catalogue.version.as_str();
    let required_text = |field: &str, value: &Option<String>, why: &str| -> Result<String> {
        match value {
            Some(text) if !text.trim().is_empty() => Ok(text.trim().to_string()),
            _ => Err(missing(version, label, field, why)),
        }
    };

    let object_id = required_text(
        "object_id",
        &record.object_id,
        "every fill is charged to buckets keyed by object id",
    )?;
    let symbol = required_text("symbol", &record.symbol, "a record must be nameable")?;
    let instrument_type = record.instrument_type.ok_or_else(|| {
        missing(
            version,
            label,
            "instrument_type",
            "the asset-class bucket is derived from it",
        )
    })?;
    if instrument_type.is_derivative() {
        return Err(Error::invalid(format!(
            "instrument catalogue {version}: {label} is a {instrument_type}, which needs an \
             underlying, and this catalogue format carries none; a derivative is registered \
             through ingestion, not the reference catalogue"
        )));
    }
    if instrument_type.has_maturity() {
        return Err(Error::invalid(format!(
            "instrument catalogue {version}: {label} is a {instrument_type}, which needs a \
             maturity, and this catalogue format carries none; an invented maturity would be \
             a mispricing on purpose"
        )));
    }
    let venue = required_text(
        "venue",
        &record.venue,
        "the venue bucket is keyed on it, and a blank one feeds no bucket",
    )?;
    let sector = record.sector.ok_or_else(|| {
        missing(
            version,
            label,
            "sector",
            "the sector bucket is keyed on it; `unclassified` must be written, not assumed",
        )
    })?;
    let geography = required_text(
        "geography",
        &record.geography,
        "the country bucket is keyed on it",
    )?;
    if geography.len() != 2 || !geography.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(Error::invalid(format!(
            "instrument catalogue {version}: {label} has geography `{geography}`, which is not \
             a two-letter upper-case country code; the country bucket is keyed on the exact \
             string, and a variant spelling would open a second bucket for the same country"
        )));
    }
    let currency = required_text("currency", &record.currency, "the price is quoted in it")?;
    let currency = Currency::parse(&currency).map_err(|error| {
        Error::invalid(format!(
            "instrument catalogue {version}: {label}: {}",
            error.message()
        ))
    })?;
    let price = record.price.ok_or_else(|| {
        missing(
            version,
            label,
            "price",
            "notional is sized from it and an unpriced record is not decision-grade",
        )
    })?;
    if !price.is_positive() {
        return Err(Error::invalid(format!(
            "instrument catalogue {version}: {label} has price {price}, which is not positive; \
             a zero price is a missing price written as a number"
        )));
    }
    let licensing = record.licensing.ok_or_else(|| {
        missing(
            version,
            label,
            "licensing",
            "licensing posture is evaluated before an instrument is used, not after",
        )
    })?;

    let provenance = Provenance::new(catalogue.source.clone(), catalogue.as_of, now)
        .with_licensing(licensing)
        .with_upstream_id(format!("{version}@{sha256}"));

    let mut builder =
        FinancialObject::builder(ObjectId::from_string(object_id), symbol, instrument_type)
            .venue(venue)
            .sector(sector)
            .geography(geography)
            .currency(currency)
            .price(price)
            .provenance(provenance)
            .metadata("catalogue_version", version)
            .metadata("catalogue_sha256", sha256);
    if let Some(name) = &record.name {
        builder = builder.name(name.clone());
    }
    if let Some(issuer) = &record.issuer {
        builder = builder.issuer(issuer.clone());
    }
    if let Some(lot) = record.lot_size {
        builder = builder.lot_size(lot);
    }
    if let Some(tick) = record.tick_size {
        builder = builder.tick_size(tick);
    }
    if let Some(multiplier) = record.contract_multiplier {
        builder = builder.contract_multiplier(multiplier);
    }
    builder.build(now).map_err(|error| {
        Error::invalid(format!(
            "instrument catalogue {version}: {label}: {}",
            error.message()
        ))
    })
}
