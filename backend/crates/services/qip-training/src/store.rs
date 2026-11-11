//! A dataset assembled from the Tick Lake, with a manifest that says exactly
//! which stored bytes it was made from (R12-MODEL-10083).
//!
//! Every fit in the tree was made from rows a caller already held in memory,
//! so a model card could name its dataset and nothing could say which
//! recorded history that name stood for, or rebuild it. This module is the
//! assembly from a store: it reads a history the lake has *pinned*
//! ([`qip_storage::lake::Lake::pin_market_history`]) and returns the dataset
//! together with a [`DatasetManifest`] naming every source segment by key and
//! checksum.
//!
//! Three refusals shape it.
//!
//! * **A row is dated by when it became knowable.** The lake stores a tick's
//!   venue instant and the instant the platform received it. Rows are ordered
//!   and stamped by the second; a feature dated by the venue's clock is
//!   readable before the platform could have read it, which is point-in-time
//!   leakage however good the fit looks.
//! * **A rebuild is the same dataset or it is refused.** [`rebuild`] reads
//!   the same pinned manifest and compares the source snapshots and the
//!   digest of the rows. The lake already fails a segment whose bytes changed;
//!   this catches the other half — the same bytes read under a different
//!   grant, or by an assembly rule that has since changed.
//! * **A withheld segment is named, not skipped.** A segment in the pinned
//!   history that the job's grant does not cover contributes no row and is
//!   listed in the manifest, so two datasets that differ only in what the job
//!   was allowed to read do not share a name and a silence.
//!
//! # What it does not do
//!
//! It reads through [`qip_storage::BlobStore`], the port the Cloud Storage
//! adapter also implements, and it has no caller in a composition root: the
//! deep brain still fits on the bars the platform holds. It does not read
//! BigQuery, Bigtable or Spanner Graph. It derives one feature family —
//! lagged trade-to-trade returns — because that is the one a trade print
//! alone supports; a richer feature layer is the feature layer's.

use crate::dataset::TrainingDataset;
use qip_contracts::message::{MessageBody, TradeCondition};
use qip_core::Timestamp;
use qip_core::canonical::canonical_json;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_storage::lake::Lake;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// What to assemble, from which pinned history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreAssembly {
    /// The dataset's name, as a model card will record it.
    pub dataset: String,
    /// The lake manifest the history was pinned under.
    pub source_manifest: String,
    /// The instrument whose trades are read. Ticks for any other instrument
    /// in the pinned history are left out.
    pub instrument: String,
    /// Entitlements the job is granted. A segment under any other is
    /// withheld and named.
    pub granted: BTreeSet<String>,
    /// How many lagged returns each row carries.
    pub lags: usize,
    /// The most ticks the assembly will hold; a larger history is refused.
    pub max_ticks: usize,
}

/// One stored segment a dataset was read from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    /// The segment's key in the store.
    pub key: String,
    /// SHA-256 of its canonical bytes, as the pinned manifest recorded it.
    pub canonical_sha256: String,
    /// The entitlement it is held under.
    pub entitlement: String,
}

/// The record of what a store-backed dataset was built from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetManifest {
    /// The request the dataset was assembled from.
    pub assembly: StoreAssembly,
    /// Every segment read, in the pinned manifest's order.
    pub snapshots: Vec<SourceSnapshot>,
    /// Segments in the pinned history the grant did not cover. Never read.
    pub withheld: Vec<String>,
    /// Lines in a read segment that decoded as no known tick shape.
    pub unparsed: usize,
    /// Rows in the dataset.
    pub rows: usize,
    /// SHA-256 of the dataset's canonical JSON: names, rows, targets, times.
    pub dataset_sha256: String,
}

/// A dataset and the record of where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct StoreBackedDataset {
    pub dataset: TrainingDataset,
    pub manifest: DatasetManifest,
}

/// The digest a [`DatasetManifest`] records for `dataset`.
pub fn dataset_digest(dataset: &TrainingDataset) -> Result<String> {
    Ok(sha256_hex(
        canonical_json(&serde_json::to_value(dataset)?).as_bytes(),
    ))
}

/// Assemble the dataset `assembly` describes from the lake.
pub fn assemble(lake: &Lake<'_>, assembly: &StoreAssembly) -> Result<StoreBackedDataset> {
    if assembly.lags == 0 {
        return Err(Error::invalid(
            "a store-backed dataset needs at least one lagged return; with none there is no \
             feature to fit",
        ));
    }
    if assembly.instrument.trim().is_empty() {
        return Err(Error::invalid(
            "a store-backed dataset names the instrument it reads; an unnamed one would mix \
             every instrument in the pinned history into one price series",
        ));
    }
    let pinned = lake.manifest(&assembly.source_manifest)?;
    let history = lake.read_market_history(
        &assembly.source_manifest,
        &assembly.granted,
        assembly.max_ticks,
    )?;

    let snapshots: Vec<SourceSnapshot> = pinned
        .segments
        .iter()
        .filter(|record| assembly.granted.contains(&record.partition.entitlement.id))
        .map(|record| SourceSnapshot {
            key: record.key.clone(),
            canonical_sha256: record.canonical_sha256.clone(),
            entitlement: record.partition.entitlement.id.clone(),
        })
        .collect();

    // (knowable instant, price). Money becomes a statistic here: a return is
    // a ratio the fit consumes as `f64`, and nothing downstream is a price.
    let mut prints: Vec<(Timestamp, f64)> = Vec::new();
    for tick in &history.ticks {
        if tick.message.object_id.as_str() != assembly.instrument {
            continue;
        }
        let MessageBody::Trade {
            price, condition, ..
        } = &tick.message.body
        else {
            continue;
        };
        // A correction restates an earlier print and an odd lot does not move
        // the last sale; reading either as the next price invents a return.
        if matches!(
            condition,
            TradeCondition::Correction | TradeCondition::OddLot
        ) {
            continue;
        }
        let value = price.to_f64();
        if !value.is_finite() || value <= 0.0 {
            return Err(Error::numeric(format!(
                "segment {} holds a trade for {} at a price of {price}; a return over a \
                 non-positive price is not a return",
                tick.segment, assembly.instrument
            )));
        }
        prints.push((tick.knowable_at, value));
    }
    // Storage order is segment order, not market order. Stable, so two prints
    // received at the same instant keep the order the store holds them in.
    prints.sort_by_key(|(knowable_at, _)| *knowable_at);

    // `lags` returns of features and one more for the target need
    // `lags + 2` prints.
    if prints.len() < assembly.lags + 2 {
        return Err(Error::invalid(format!(
            "manifest {} holds {} usable trade(s) for {} under the grant; {} lagged return(s) \
             and a target need at least {}",
            assembly.source_manifest,
            prints.len(),
            assembly.instrument,
            assembly.lags,
            assembly.lags + 2
        )));
    }
    let returns: Vec<f64> = prints
        .windows(2)
        .map(|pair| pair[1].1 / pair[0].1 - 1.0)
        .collect();

    let mut rows = Vec::new();
    let mut targets = Vec::new();
    let mut times = Vec::new();
    // Row `i` reads returns `i..i + lags`, the last of which closed at print
    // `i + lags`; its target is the return to print `i + lags + 1`.
    for start in 0..returns.len() - assembly.lags {
        let row: Vec<f64> = (0..assembly.lags)
            .map(|lag| returns[start + assembly.lags - 1 - lag])
            .collect();
        rows.push(row);
        targets.push(returns[start + assembly.lags]);
        times.push(prints[start + assembly.lags].0);
    }
    let names: Vec<String> = (1..=assembly.lags)
        .map(|lag| format!("return_lag_{lag}"))
        .collect();
    let dataset = TrainingDataset::new(assembly.dataset.clone(), names, rows, targets, times)?;
    let manifest = DatasetManifest {
        assembly: assembly.clone(),
        snapshots,
        withheld: history.withheld,
        unparsed: history.unparsed,
        rows: dataset.len(),
        dataset_sha256: dataset_digest(&dataset)?,
    };
    Ok(StoreBackedDataset { dataset, manifest })
}

/// Rebuild the dataset `manifest` records, refusing anything that is not it.
///
/// The lake refuses a segment whose bytes no longer match the pinned
/// checksum. What is checked here is the rest: that the segments read are the
/// ones the manifest names, and that the rows come out with the digest it
/// recorded. A rebuild that returned a different dataset under the same name
/// would let a model card point at data its model was never fitted on.
pub fn rebuild(lake: &Lake<'_>, manifest: &DatasetManifest) -> Result<TrainingDataset> {
    let again = assemble(lake, &manifest.assembly)?;
    if again.manifest.snapshots != manifest.snapshots {
        return Err(Error::invalid(format!(
            "dataset {} was built from {} source snapshot(s) and the rebuild read {}, or \
             different ones; pin the history again under a new name rather than rebuilding \
             this one",
            manifest.assembly.dataset,
            manifest.snapshots.len(),
            again.manifest.snapshots.len()
        )));
    }
    if again.manifest.dataset_sha256 != manifest.dataset_sha256 {
        return Err(Error::invalid(format!(
            "dataset {} rebuilt with digest {} against a recorded {}; the same source bytes \
             produced different rows, so the assembly rule changed since the manifest was \
             written",
            manifest.assembly.dataset, again.manifest.dataset_sha256, manifest.dataset_sha256
        )));
    }
    Ok(again.dataset)
}
