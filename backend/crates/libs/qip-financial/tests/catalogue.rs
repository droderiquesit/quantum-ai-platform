//! The committed instrument catalogue, read the way a composition root reads it.
//!
//! Every root assembled `Universe::new()`, so the exposure buckets the kernel
//! projects from the universe received nothing in any deployed process and
//! nothing said so. These tests hold the loader to what closes that: the
//! committed file produces a universe with every record in it, a record
//! missing a bucket field refuses the whole file by name, the licensing gate
//! sees a research-only record, and the file's hash is written where a run
//! can be reproduced from.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{KeyValueStore, KeyValueStoreExt, ObjectId, Timestamp};
use qip_financial::catalogue::{self, BY_HASH_PREFIX, CURRENT_KEY, CatalogueManifest};
use qip_financial::quality::LicensingClass;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// The file every central root reads; relative to this crate so the test
/// reads the committed artefact and not a copy of it.
const COMMITTED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../data/datasets/universe.json"
);

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 9, 2)
}

fn committed_text() -> String {
    std::fs::read_to_string(COMMITTED).expect("the committed catalogue is readable")
}

/// The records as written, so a test can assert what the file says before
/// asserting what the loader made of it.
fn committed_records(text: &str) -> Vec<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(text).expect("the catalogue is JSON");
    value["instruments"]
        .as_array()
        .expect("the catalogue has an instruments array")
        .clone()
}

fn field<'a>(record: &'a serde_json::Value, name: &str) -> &'a str {
    record[name]
        .as_str()
        .unwrap_or_else(|| panic!("record has a string `{name}`: {record}"))
}

/// A store with nothing behind it but a map, so the journaling test proves
/// what is written rather than how a particular store persists it.
#[derive(Debug, Default)]
struct MemoryStore {
    values: Mutex<BTreeMap<String, serde_json::Value>>,
}

impl KeyValueStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        Ok(self.values.lock().expect("store lock").get(key).cloned())
    }
    fn put(&self, key: &str, value: serde_json::Value) -> Result<()> {
        self.values
            .lock()
            .expect("store lock")
            .insert(key.to_string(), value);
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<bool> {
        Ok(self
            .values
            .lock()
            .expect("store lock")
            .remove(key)
            .is_some())
    }
    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .values
            .lock()
            .expect("store lock")
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }
    fn len(&self) -> Result<usize> {
        Ok(self.values.lock().expect("store lock").len())
    }
}

#[test]
fn the_committed_catalogue_loads_and_every_record_reaches_the_universe() -> Result<()> {
    let text = committed_text();
    let records = committed_records(&text);
    // Premise: the file lists instruments, and they carry the four bucket
    // fields, so the equalities below compare against something.
    assert!(
        records.len() >= 2,
        "the committed catalogue lists {} instrument(s); a one-record file proves nothing about \
         every record reaching the universe",
        records.len()
    );

    let loaded = catalogue::load(&text, now())?;

    assert_eq!(
        loaded.universe.len(),
        records.len(),
        "the universe holds a different number of instruments than the file lists"
    );
    assert_eq!(loaded.manifest.instruments, records.len());
    for record in &records {
        let id = ObjectId::from_string(field(record, "object_id"));
        let object = loaded.universe.require(&id)?;
        assert_eq!(object.symbol, field(record, "symbol"));
        assert_eq!(object.venue, field(record, "venue"), "{id}: venue");
        assert_eq!(
            object.geography,
            field(record, "geography"),
            "{id}: geography"
        );
        assert_eq!(
            serde_json::to_value(object.sector)?,
            record["sector"],
            "{id}: sector"
        );
        assert_eq!(
            serde_json::to_value(object.asset_class)?,
            serde_json::to_value(object.instrument_type.asset_class())?,
            "{id}: asset class"
        );
        assert_eq!(
            serde_json::to_value(object.provenance.licensing)?,
            record["licensing"],
            "{id}: licensing"
        );
        assert_eq!(object.provenance.source, loaded.manifest.source);
    }
    Ok(())
}

#[test]
fn a_record_missing_a_required_field_refuses_the_whole_catalogue_naming_the_record() -> Result<()> {
    let text = committed_text();
    // Premise: the file loads as committed, and the record the test breaks
    // carries the field it is about to remove — otherwise the refusal below
    // could be about something else entirely.
    assert!(catalogue::load(&text, now()).is_ok());
    let mut value: serde_json::Value = serde_json::from_str(&text)?;
    let broken = 2usize;
    let object_id = value["instruments"][broken]["object_id"]
        .as_str()
        .expect("the record names its object id")
        .to_string();
    assert!(
        value["instruments"][broken]
            .as_object_mut()
            .expect("a record is an object")
            .remove("sector")
            .is_some(),
        "the record had no sector to remove"
    );
    let text = serde_json::to_string(&value)?;

    let error = match catalogue::load(&text, now()) {
        Ok(loaded) => panic!(
            "a catalogue with a sector-less record loaded {} instrument(s)",
            loaded.universe.len()
        ),
        Err(error) => error.message().to_string(),
    };
    assert!(
        error.contains(&format!("record #{}", broken + 1)),
        "the refusal does not name the record's position: {error}"
    );
    assert!(
        error.contains(&format!("`{object_id}`")),
        "the refusal does not name the record's object id: {error}"
    );
    assert!(
        error.contains("`sector`"),
        "the refusal does not name the missing field: {error}"
    );
    Ok(())
}

#[test]
fn a_research_only_instrument_is_reported_as_not_decision_grade() -> Result<()> {
    let text = committed_text();
    let records = committed_records(&text);
    let research_only: Vec<String> = records
        .iter()
        .filter(|record| field(record, "licensing") == "synthetic")
        .map(|record| field(record, "object_id").to_string())
        .collect();
    // Premise: the file carries at least one research-only record and at
    // least one that is not, so the report below is a selection and not the
    // whole file or none of it.
    assert!(
        !research_only.is_empty(),
        "the committed catalogue has no research-only record to report"
    );
    assert!(
        research_only.len() < records.len(),
        "every committed record is research-only; the report would be the whole file"
    );

    let loaded = catalogue::load(&text, now())?;

    let reported: Vec<String> = loaded.manifest.not_decision_grade.keys().cloned().collect();
    assert_eq!(
        reported, research_only,
        "the manifest reports a different set of instruments than the file marks research-only"
    );
    for id in &research_only {
        let reason = &loaded.manifest.not_decision_grade[id];
        assert!(
            reason.contains("Synthetic"),
            "{id} is reported for a reason other than its licensing class: {reason}"
        );
        assert!(
            !loaded
                .universe
                .require(&ObjectId::from_string(id.clone()))?
                .is_decision_grade()
        );
    }
    assert_eq!(
        loaded.universe.not_decision_grade().len(),
        research_only.len(),
        "the universe and the manifest disagree about how many records are unfit"
    );
    // And the class itself is the reason, not a coincidence of the fixture.
    assert!(!LicensingClass::Synthetic.allows_production_decisions());
    Ok(())
}

#[test]
fn the_catalogue_hash_is_journaled_under_its_hash_and_as_current() -> Result<()> {
    let text = committed_text();
    let loaded = catalogue::load(&text, now())?;
    let expected = qip_core::sha256_hex(text.as_bytes());
    let store = MemoryStore::default();
    // Premise: the manifest carries the file's own hash, and the store is
    // empty, so a record found below was written by the call under test.
    assert_eq!(loaded.manifest.sha256, expected);
    assert_eq!(expected.len(), 64, "a SHA-256 renders as 64 hex characters");
    assert!(store.is_empty()?);

    catalogue::record_manifest(&store, &loaded.manifest)?;

    let by_hash: CatalogueManifest = store
        .get_as(&format!("{BY_HASH_PREFIX}{expected}"))?
        .expect("the manifest is recorded under its hash");
    let current: CatalogueManifest = store
        .get_as(CURRENT_KEY)?
        .expect("the manifest is recorded as current");
    assert_eq!(by_hash, loaded.manifest);
    assert_eq!(current, loaded.manifest);
    assert_eq!(by_hash.sha256, expected);
    assert_eq!(by_hash.version, loaded.manifest.version);
    assert_eq!(by_hash.loaded_at, now());
    assert_eq!(store.len()?, 2, "exactly two keys are written");
    Ok(())
}

#[test]
fn an_empty_catalogue_is_refused_rather_than_becoming_an_empty_universe() -> Result<()> {
    let mut value: serde_json::Value = serde_json::from_str(&committed_text())?;
    // Premise: the file was not empty before the test emptied it.
    assert!(!value["instruments"].as_array().expect("array").is_empty());
    value["instruments"] = serde_json::Value::Array(Vec::new());
    let text = serde_json::to_string(&value)?;

    let error = match catalogue::load(&text, now()) {
        Ok(_) => panic!("an empty catalogue produced a universe"),
        Err(error) => error.message().to_string(),
    };
    assert!(
        error.contains("lists no instruments"),
        "the refusal does not say the catalogue is empty: {error}"
    );
    Ok(())
}

#[test]
fn a_geography_that_is_not_an_alpha_2_code_is_refused_rather_than_opening_a_second_bucket()
-> Result<()> {
    let mut value: serde_json::Value = serde_json::from_str(&committed_text())?;
    // Premise: the record is well-formed before the test bends it.
    assert_eq!(value["instruments"][0]["geography"], "US");
    value["instruments"][0]["geography"] = serde_json::Value::from("usa");
    let text = serde_json::to_string(&value)?;

    let error = match catalogue::load(&text, now()) {
        Ok(_) => panic!("a lower-case three-letter geography was accepted"),
        Err(error) => error.message().to_string(),
    };
    assert!(
        error.contains("record #1") && error.contains("`usa`"),
        "the refusal does not name the record and the value: {error}"
    );
    Ok(())
}
