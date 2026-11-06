//! Resolved outcomes loaded into the research warehouse (R12-DATA-9054,
//! R12-DATA-8127, ADR 0240).
//!
//! The learning stage grades every thesis the platform published and keeps the
//! grades in a bounded window ([`qip_kernel::Platform::evaluations`]). The
//! window is a ring: a grade older than it is gone from memory, and the event
//! log holds the record but not a table a research query can join. The
//! warehouse (`qip_mesh::bigquery`) is that table.
//!
//! # Batch load, never a stream (ADR 0240)
//!
//! Each cycle's new grades are staged as one newline-delimited JSON object
//! under [`STAGING_PREFIX`] in the regional staging bucket and loaded with a
//! BigQuery load job. A batch counts as shipped only when its job reports done
//! without an error: until then its grades are pending and are not offered
//! again, and a failed job releases them to be staged in a new batch. Nothing
//! here calls `insertAll`; `qip-acceptance` refuses an `InsertRow` anywhere in
//! shipped source. The staged object is a transport copy that the bucket's
//! lifecycle deletes; the table and the event log are the records.
//!
//! # Off unless a deployment names a target
//!
//! [`OutcomeSink::from_lookup`] returns `None` when neither
//! `QIP_BIGQUERY_PROJECT` nor `QIP_BIGQUERY_DATASET` is set, which is every
//! environment today (`enable_bigquery = false`). It refuses when only one is
//! set, when no staging bucket is named, and when the target has no endpoint
//! or credential, so a half-written deployment stops the process rather than
//! reading as a warehouse that is quietly not there. There is no fallback
//! sink: a grade the warehouse cannot take is held and offered again, never
//! written somewhere else.
//!
//! # What is sent
//!
//! One row per grade. Only derived facts: the grade's rationale is free text
//! and stays in the event log. The shipped set is pruned to the grades the
//! window still holds, so it is bounded by the window and not by the run; the
//! pending jobs are bounded by the cycles a load takes to finish.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_learning_engine::evaluation::Evaluation;
use qip_mesh::bigquery::{BigQueryConfig, BigQueryWarehouse, LoadState};
use qip_storage::gcp::{
    CloudStorageBlobStore, CloudStorageConfig, DATASET_VARIABLE, PROJECT_VARIABLE,
};
use qip_storage::managed::ManagedSettings;
use qip_storage::{BlobStore, StorageTarget};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The table the grades are loaded into. Created outside this code, with
/// the columns [`row`] writes.
pub const OUTCOMES_TABLE: &str = "resolved_outcomes";

/// Grades one cycle stages at most, so a backlog after an outage drains over
/// cycles instead of holding one cycle on a single large object.
pub const MAX_ROWS_PER_CYCLE: usize = 500;

/// Where staged batches live in the staging bucket. `modules/data` expires
/// this prefix; nothing else may write under it.
pub const STAGING_PREFIX: &str = "warehouse-staging/";

/// The variable naming the bucket batches are staged in.
pub const STAGING_BUCKET_VARIABLE: &str = "QIP_BIGQUERY_STAGING_BUCKET";

/// The variable naming the dataset's location, which every load job is sent
/// with so its state can be read back.
pub const LOCATION_VARIABLE: &str = "QIP_BIGQUERY_LOCATION";

/// Failed loads a grade survives before it is quarantined: held out of every
/// later batch and reported, rather than reloaded every cycle as a billed job
/// that fails the same way.
pub const MAX_LOAD_ATTEMPTS: u32 = 3;

/// Load jobs not yet seen finished, beyond which a cycle stages nothing new:
/// a warehouse that accepts jobs and never finishes them must not grow this
/// map without bound.
pub const MAX_PENDING_JOBS: usize = 16;

/// What one cycle's shipping did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShipRound {
    /// Grades whose load finished this cycle.
    pub sent: usize,
    /// Grades staged and submitted in a load job this cycle.
    pub staged: usize,
    /// Grades whose load failed this cycle and will be staged again.
    pub rejected: usize,
    /// Grades not yet in the table after this cycle, pending ones included.
    pub held: usize,
    /// Grades held out after [`MAX_LOAD_ATTEMPTS`] failed loads.
    pub quarantined: usize,
    /// Pending jobs whose state could not be read this cycle; asked again
    /// next cycle.
    pub unsettled: usize,
}

impl ShipRound {
    pub fn describe(&self) -> String {
        format!(
            "warehouse: {} outcome(s) loaded into {OUTCOMES_TABLE}, {} staged, {} failed and \
             released, {} held, {} quarantined, {} job(s) unsettled",
            self.sent, self.staged, self.rejected, self.held, self.quarantined, self.unsettled
        )
    }
}

/// The deep brain's sink for resolved outcomes.
#[derive(Debug)]
pub struct OutcomeSink {
    warehouse: BigQueryWarehouse,
    staging: Box<dyn BlobStore>,
    bucket: String,
    shipped: BTreeSet<String>,
    /// Load job id to the grades it carries. A batch is recorded here before
    /// its upload or load request is sent, so a lost response is found again
    /// by its id next cycle rather than loaded a second time.
    pending: BTreeMap<String, Vec<String>>,
    /// Failed loads per grade, bounded by the window.
    attempts: BTreeMap<String, u32>,
}

fn present(lookup: &dyn Fn(&str) -> Option<String>, name: &str) -> bool {
    lookup(name).is_some_and(|value| !value.trim().is_empty())
}

impl OutcomeSink {
    /// A sink over a warehouse and a staging store the caller built.
    /// `bucket` is the staging store's bucket, which the load names.
    pub fn new(
        warehouse: BigQueryWarehouse,
        staging: Box<dyn BlobStore>,
        bucket: impl Into<String>,
    ) -> Self {
        Self {
            warehouse,
            staging,
            bucket: bucket.into(),
            shipped: BTreeSet::new(),
            pending: BTreeMap::new(),
            attempts: BTreeMap::new(),
        }
    }

    /// The sink a deployment asked for, or `None` when it asked for none.
    pub fn from_lookup(
        lookup: &dyn Fn(&str) -> Option<String>,
        clock: Arc<dyn qip_core::Clock>,
    ) -> Result<Option<Self>> {
        let project = present(lookup, PROJECT_VARIABLE);
        let dataset = present(lookup, DATASET_VARIABLE);
        if !project && !dataset {
            return Ok(None);
        }
        // Either one set is a request for the warehouse, so a missing
        // half, endpoint, credential, location or staging bucket is a refusal that
        // names it.
        let settings = ManagedSettings::from_env(StorageTarget::BigQuery, lookup)?;
        let config = BigQueryConfig::from_managed(&settings, clock)?;
        let access = config.access.clone();
        let reachable = BigQueryWarehouse::new(config.clone())?;
        if !reachable.is_available() {
            return Err(Error::invalid(format!(
                "configuration: the research warehouse is requested but cannot be reached: {}",
                reachable.requirement()
            )));
        }
        let location = lookup(LOCATION_VARIABLE)
            .map(|location| location.trim().to_string())
            .filter(|location| !location.is_empty())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "configuration: the research warehouse is requested and {LOCATION_VARIABLE} \
                     names no dataset location; a load job is sent with its location so that \
                     its state can be read back (ADR 0240)"
                ))
            })?;
        let warehouse = BigQueryWarehouse::new(config.with_location(location))?;
        let bucket = lookup(STAGING_BUCKET_VARIABLE)
            .map(|bucket| bucket.trim().to_string())
            .filter(|bucket| !bucket.is_empty())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "configuration: the research warehouse is requested and \
                     {STAGING_BUCKET_VARIABLE} names no bucket to stage loads in; the warehouse \
                     is fed by batch load (ADR 0240) and has no streaming fallback"
                ))
            })?;
        let mut staging = CloudStorageConfig::new(bucket.clone()).with_access(access);
        staging.content_type = "application/x-ndjson".to_string();
        let staging = CloudStorageBlobStore::new(staging)?;
        Ok(Some(Self::new(warehouse, Box::new(staging), bucket)))
    }

    /// Grades handed in and not yet in the table, pending loads included.
    pub fn pending(&self, evaluations: &[Evaluation]) -> usize {
        evaluations
            .iter()
            .filter(|e| !self.shipped.contains(&e.hypothesis_id))
            .count()
    }

    /// Settle the loads submitted on earlier cycles, then stage and load the
    /// grades in `evaluations` that are neither shipped nor pending.
    ///
    /// A request that fails outright returns the error; a job whose state
    /// could not be read stays pending and is asked about next cycle, and a
    /// batch whose upload or load request failed is not marked pending, so
    /// its grades are offered again.
    pub fn ship(&mut self, evaluations: &[Evaluation], now: Timestamp) -> Result<ShipRound> {
        let window: BTreeSet<&str> = evaluations
            .iter()
            .map(|e| e.hypothesis_id.as_str())
            .collect();
        self.shipped.retain(|id| window.contains(id.as_str()));
        self.attempts.retain(|id, _| window.contains(id.as_str()));
        let mut round = ShipRound::default();

        // Settle what earlier cycles submitted. One job whose state cannot be
        // read is asked again next cycle and holds up nothing else.
        for job in self.pending.keys().cloned().collect::<Vec<_>>() {
            match self.warehouse.load_state(&job) {
                Ok(LoadState::Pending) => {}
                Ok(LoadState::Done { .. }) => {
                    let ids = self.pending.remove(&job).unwrap_or_default();
                    round.sent += ids.len();
                    self.shipped.extend(ids);
                }
                Ok(LoadState::Failed { .. }) => {
                    let ids = self.pending.remove(&job).unwrap_or_default();
                    round.rejected += ids.len();
                    for id in ids {
                        *self.attempts.entry(id).or_insert(0) += 1;
                    }
                }
                // The job was never created: the upload or the load request
                // failed before BigQuery took it. Released, and staged again
                // under the same id, because nothing about it ran.
                Err(Error::NotFound(_)) => {
                    self.pending.remove(&job);
                }
                Err(_) => round.unsettled += 1,
            }
        }

        let quarantined = |id: &str| {
            self.attempts
                .get(id)
                .is_some_and(|failed| *failed >= MAX_LOAD_ATTEMPTS)
        };
        round.quarantined = evaluations
            .iter()
            .filter(|e| !self.shipped.contains(&e.hypothesis_id) && quarantined(&e.hypothesis_id))
            .count();
        let in_flight: BTreeSet<&str> = self
            .pending
            .values()
            .flatten()
            .map(String::as_str)
            .collect();
        let mut fresh: Vec<&Evaluation> = evaluations
            .iter()
            .filter(|e| {
                !self.shipped.contains(&e.hypothesis_id)
                    && !in_flight.contains(e.hypothesis_id.as_str())
                    && !quarantined(&e.hypothesis_id)
            })
            .collect();
        fresh.truncate(MAX_ROWS_PER_CYCLE);
        if !fresh.is_empty() && self.pending.len() < MAX_PENDING_JOBS {
            let ids: Vec<String> = fresh.iter().map(|e| e.hypothesis_id.clone()).collect();
            let attempt = ids
                .iter()
                .filter_map(|id| self.attempts.get(id))
                .max()
                .copied()
                .unwrap_or(0);
            let job = job_id(&ids, attempt);
            let mut body = Vec::new();
            for evaluation in &fresh {
                serde_json::to_writer(&mut body, &row(evaluation, now))?;
                body.push(b'\n');
            }
            let key = format!("{STAGING_PREFIX}{OUTCOMES_TABLE}/{job}.ndjson");
            let uri = format!("gs://{}/{key}", self.bucket);
            // Pending before anything is sent: if either request fails or its
            // answer is lost, the next cycle asks BigQuery about this job id
            // and either finds it or learns it never existed.
            self.pending.insert(job.clone(), ids.clone());
            round.staged = ids.len();
            self.staging.put(&key, body)?;
            match self.warehouse.load(OUTCOMES_TABLE, &uri, &job)? {
                LoadState::Done { .. } => {
                    self.pending.remove(&job);
                    round.sent += ids.len();
                    self.shipped.extend(ids);
                }
                LoadState::Failed { .. } => {
                    self.pending.remove(&job);
                    round.rejected += ids.len();
                    for id in ids {
                        *self.attempts.entry(id).or_insert(0) += 1;
                    }
                }
                LoadState::Pending => {}
            }
        }
        round.held = self.pending(evaluations);
        Ok(round)
    }
}

/// A load job id derived from the batch alone: the grades it carries and how
/// many loads of them have failed. A retried request for this batch is the
/// same job (ADR 0240 decision 2); the same grades after a failed load are a
/// new one.
fn job_id(ids: &[String], attempt: u32) -> String {
    let digest = qip_core::sha256_hex(ids.join("\n").as_bytes());
    format!("qip-outcomes-{}-{attempt}", &digest[..32])
}

/// One grade as a row. Statistics only; money never crosses here as `f64`
/// except where the grade itself already held it as one.
fn row(evaluation: &Evaluation, shipped_at: Timestamp) -> serde_json::Value {
    serde_json::json!({
        "hypothesis_id": evaluation.hypothesis_id,
        "class": evaluation.class,
        "subject": evaluation.subject,
        "verdict": evaluation.verdict.as_str(),
        "expected_move_bps": evaluation.expected_move_bps,
        "realised_move_bps": evaluation.realised_move_bps,
        "magnitude_ratio": evaluation.magnitude_ratio,
        "confidence": evaluation.confidence,
        "realised_pnl": evaluation.realised_pnl,
        "evaluated_at": evaluation.evaluated_at.to_rfc3339(),
        "shipped_at": shipped_at.to_rfc3339(),
    })
}
