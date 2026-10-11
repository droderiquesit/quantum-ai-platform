//! ADR 0111 §1's managed log on Cloud Spanner, over Spanner's REST API.
//!
//! [`SpannerManagedLog`] implements `qip_streaming`'s [`ManagedLog`] port.
//! It lives here, in the composition root, because this is the process that
//! holds the token and the transport (ADR 0111, "Dependency direction"): the
//! broker core sees only the trait.
//!
//! # Why each transaction is shaped the way it is
//!
//! Every write is one Spanner read-write transaction that **reads the lease
//! row first**. A read-write transaction takes a shared lock on what it reads
//! and Spanner serialises it against any transaction that writes the same
//! row, so a standby's takeover and a deposed leader's commit cannot both
//! succeed: one of them aborts, retries, and on retry reads the other's
//! write. That is "fencing by the database, not by a clock" in concrete
//! terms. Expiry is judged with `CURRENT_TIMESTAMP()` inside the transaction
//! — the database's time — and never with this process's clock.
//!
//! * **Acquire**: read the lease; insert it at epoch 1 if absent, renew it at
//!   the same epoch if this holder has it, take it at epoch + 1 if it has
//!   expired, and otherwise roll back and report the holder.
//! * **Commit**: read the lease and roll back as `Fenced` unless this holder
//!   holds it at the row's epoch; read one past the highest committed offset
//!   and roll back as `Conflict` unless the row is exactly that offset;
//!   insert the row with `PENDING_COMMIT_TIMESTAMP()`; commit.
//!
//! An `ABORTED` commit is retried, whole, a bounded number of times. A
//! transport failure on the commit call itself is returned as an error: the
//! outcome is unknown, and the broker treats it so.
//!
//! # Transport
//!
//! Plain HTTP/1.1 to the TLS-terminating proxy, with a token from the
//! metadata server or a token file — the access the Cloud Storage archive in
//! this binary already uses (`qip_storage::gcp`). ADR 0111 names rustls for
//! this hop; until the rustls client of ADR 0112 exists in-tree the proxy is
//! the only TLS this build has, and the adapter refuses an `https://`
//! endpoint rather than sending a bearer token in clear text.
//!
//! The schema the statements below expect is committed beside the local
//! catalogue: `infrastructure/event-fabric/spanner.ddl`.

use std::sync::{Arc, Mutex};

use qip_core::error::{Error, Result};
use qip_storage::gcp::{GcpAccess, status_refusal};
use qip_streaming::event_fabric::managed::{
    CommitOutcome, CommittedBatch, Lease, LeaseOutcome, ManagedLog, PartitionRef,
};
use qip_transport::{HttpClient, Method};
use serde_json::{Map, Value, json};

/// How many times an `ABORTED` read-write transaction is retried before the
/// call fails. Spanner aborts under contention; contention on one partition's
/// lease is two or three instances, so a handful of attempts is plenty and a
/// loop with no bound would hold the produce path for as long as it liked.
const MAXIMUM_ATTEMPTS: u32 = 4;

/// The most rows one catch-up read returns. A row carries a batch of up to a
/// few mebibytes; this keeps one response well inside the client's 32 MiB
/// body limit. The broker loops until it has caught up, so returning fewer
/// rows than it asked for is the port's contract, not a truncation.
const MAXIMUM_ROWS_PER_READ: u32 = 4;

const SELECT_LEASE: &str = "SELECT holder, epoch, expires_at <= CURRENT_TIMESTAMP() AS expired \
     FROM fabric_leases WHERE region = @region AND stream = @stream AND partition_id = @partition";
const INSERT_LEASE: &str = "INSERT INTO fabric_leases (region, stream, partition_id, holder, \
     epoch, expires_at) VALUES (@region, @stream, @partition, @holder, @epoch, \
     TIMESTAMP_ADD(CURRENT_TIMESTAMP(), INTERVAL @ttl MILLISECOND))";
const UPDATE_LEASE: &str = "UPDATE fabric_leases SET holder = @holder, epoch = @epoch, \
     expires_at = TIMESTAMP_ADD(CURRENT_TIMESTAMP(), INTERVAL @ttl MILLISECOND) \
     WHERE region = @region AND stream = @stream AND partition_id = @partition";
const NEXT_OFFSET: &str = "SELECT IFNULL(MAX(batch_offset) + 1, 0) FROM fabric_batches \
     WHERE region = @region AND stream = @stream AND partition_id = @partition";
const INSERT_BATCH: &str = "INSERT INTO fabric_batches (region, stream, partition_id, \
     batch_offset, lease_epoch, batch_hash, previous_batch_hash, batch, committed_at) VALUES \
     (@region, @stream, @partition, @offset, @epoch, @hash, @previous, @batch, \
     PENDING_COMMIT_TIMESTAMP())";
const READ_BATCHES: &str = "SELECT batch_offset, lease_epoch, batch_hash, previous_batch_hash, \
     batch FROM fabric_batches WHERE region = @region AND stream = @stream AND partition_id = \
     @partition AND batch_offset >= @from ORDER BY batch_offset LIMIT @limit";
const SELECT_CHECKPOINT: &str = "SELECT committed_offset FROM fabric_checkpoints WHERE region = \
     @region AND consumer_group = @group AND stream = @stream AND partition_id = @partition";
const INSERT_CHECKPOINT: &str = "INSERT INTO fabric_checkpoints (region, consumer_group, stream, \
     partition_id, committed_offset) VALUES (@region, @group, @stream, @partition, @offset)";
const UPDATE_CHECKPOINT: &str = "UPDATE fabric_checkpoints SET committed_offset = @offset WHERE \
     region = @region AND consumer_group = @group AND stream = @stream AND partition_id = \
     @partition";

// --- base64 -------------------------------------------------------------------

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// RFC 4648 base64, the encoding Spanner's REST API gives `BYTES`. Written
/// here because no permitted crate in this binary carries it.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(BASE64[(n >> 18) as usize & 63] as char);
        out.push(BASE64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            BASE64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Decode RFC 4648 base64, refusing anything that is not exactly that —
/// a wrong length, a stray character, padding in the middle.
pub fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::schema(
            "a BYTES value from Spanner is not whole base64 quads",
        ));
    }
    let value = |c: u8| -> Option<u32> { BASE64.iter().position(|b| *b == c).map(|p| p as u32) };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, quad) in bytes.chunks(4).enumerate() {
        let last = index + 1 == bytes.len() / 4;
        let pad = quad.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return Err(Error::schema("misplaced base64 padding in a BYTES value"));
        }
        let mut n = 0u32;
        for (i, c) in quad.iter().enumerate() {
            let digit = if i >= 4 - pad {
                0
            } else {
                value(*c).ok_or_else(|| Error::schema("a BYTES value is not base64"))?
            };
            n = (n << 6) | digit;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

// --- the RPC seam -----------------------------------------------------------

/// What one Spanner REST call answered.
#[derive(Clone, Debug, PartialEq)]
pub enum RpcAnswer {
    Ok(Value),
    /// `ABORTED`: the transaction lost a conflict and may be retried whole.
    Aborted,
    /// The session expired server-side; take a new one and retry.
    SessionGone,
}

/// One POST to Spanner's v1 REST API, `path` relative to `/v1/`.
///
/// A seam rather than a direct call so the transaction logic above it is
/// tested against a scripted peer: what is sent in which order, and what is
/// done with each answer, is the part that fences, and it must not need
/// Google to be proven.
pub trait SpannerRpc: Send + Sync + std::fmt::Debug {
    fn post(&self, path: &str, body: &Value) -> Result<RpcAnswer>;
}

/// [`SpannerRpc`] over the in-tree HTTP client and `qip_storage`'s access.
#[derive(Debug)]
pub struct HttpSpannerRpc {
    access: GcpAccess,
    client: HttpClient,
}

impl HttpSpannerRpc {
    pub fn new(access: GcpAccess) -> Result<Self> {
        if !access.is_configured() {
            return Err(Error::unavailable(format!(
                "the Spanner managed log cannot be reached: {}",
                access.missing_configuration().join("; ")
            )));
        }
        let client = HttpClient::new(access.limits());
        Ok(Self { access, client })
    }
}

impl SpannerRpc for HttpSpannerRpc {
    fn post(&self, path: &str, body: &Value) -> Result<RpcAnswer> {
        let request = self
            .access
            .request(Method::Post, &format!("/v1/{path}"))?
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_vec(body)?);
        let response = self.client.send(&request).map_err(Error::from)?;
        if response.is_success() {
            return serde_json::from_slice(&response.body)
                .map(RpcAnswer::Ok)
                .map_err(|e| Error::schema(format!("Spanner answered with non-JSON: {e}")));
        }
        let excerpt = response.body_excerpt();
        if response.status == 409 && excerpt.contains("ABORTED") {
            return Ok(RpcAnswer::Aborted);
        }
        if response.status == 404 && excerpt.contains("Session not found") {
            return Ok(RpcAnswer::SessionGone);
        }
        Err(status_refusal("Spanner", response.status, &excerpt))
    }
}

// --- values -----------------------------------------------------------------

/// A statement parameter, typed the way Spanner's REST API wants it.
enum Param {
    Str(String),
    Int(i64),
    Bytes(Vec<u8>),
    NullStr,
}

fn int(value: u64, what: &str) -> Result<Param> {
    i64::try_from(value)
        .map(Param::Int)
        .map_err(|_| Error::numeric(format!("{what} {value} does not fit Spanner's INT64")))
}

fn statement(sql: &str, params: Vec<(&str, Param)>) -> Map<String, Value> {
    let mut values = Map::new();
    let mut types = Map::new();
    for (name, param) in params {
        let (value, code) = match param {
            Param::Str(text) => (Value::String(text), "STRING"),
            Param::Int(n) => (Value::String(n.to_string()), "INT64"),
            Param::Bytes(bytes) => (Value::String(base64_encode(&bytes)), "BYTES"),
            Param::NullStr => (Value::Null, "STRING"),
        };
        values.insert(name.to_string(), value);
        types.insert(name.to_string(), json!({ "code": code }));
    }
    let mut body = Map::new();
    body.insert("sql".to_string(), Value::String(sql.to_string()));
    body.insert("params".to_string(), Value::Object(values));
    body.insert("paramTypes".to_string(), Value::Object(types));
    body
}

fn rows(answer: &Value) -> Vec<Vec<Value>> {
    answer
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.as_array().cloned())
                .collect()
        })
        .unwrap_or_default()
}

fn cell<'a>(row: &'a [Value], index: usize, what: &str) -> Result<&'a Value> {
    row.get(index)
        .ok_or_else(|| Error::schema(format!("a Spanner row is missing {what}")))
}

fn as_u64(value: &Value, what: &str) -> Result<u64> {
    value
        .as_str()
        .and_then(|text| text.parse::<u64>().ok())
        .ok_or_else(|| Error::schema(format!("Spanner's {what} is not a non-negative INT64")))
}

fn as_text(value: &Value, what: &str) -> Result<String> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::schema(format!("Spanner's {what} is not a STRING")))
}

// --- the adapter --------------------------------------------------------------

/// What a read-write transaction body decided.
enum Step<T> {
    Commit(T),
    Rollback(T),
}

/// One read-write transaction in progress.
struct Transaction<'a> {
    rpc: &'a dyn SpannerRpc,
    session: String,
    id: Option<String>,
    seqno: u64,
}

impl Transaction<'_> {
    /// Execute one statement inside the transaction, beginning it inline on
    /// the first call.
    fn execute(
        &mut self,
        mut body: Map<String, Value>,
    ) -> Result<std::result::Result<Value, RpcAnswer>> {
        let selector = match &self.id {
            Some(id) => json!({ "id": id }),
            None => json!({ "begin": { "readWrite": {} } }),
        };
        body.insert("transaction".to_string(), selector);
        self.seqno += 1;
        body.insert("seqno".to_string(), Value::String(self.seqno.to_string()));
        match self.rpc.post(
            &format!("{}:executeSql", self.session),
            &Value::Object(body),
        )? {
            RpcAnswer::Ok(answer) => {
                if self.id.is_none() {
                    let id = answer
                        .pointer("/metadata/transaction/id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            Error::schema(
                                "Spanner began a read-write transaction and returned no id",
                            )
                        })?;
                    self.id = Some(id.to_string());
                }
                Ok(Ok(answer))
            }
            other => Ok(Err(other)),
        }
    }

    fn finish(&self, verb: &str) -> Result<RpcAnswer> {
        let Some(id) = &self.id else {
            return Ok(RpcAnswer::Ok(Value::Null));
        };
        self.rpc.post(
            &format!("{}:{verb}", self.session),
            &json!({ "transactionId": id }),
        )
    }
}

/// ADR 0111's managed log on a Spanner database. See the module
/// documentation.
#[derive(Debug)]
pub struct SpannerManagedLog {
    rpc: Arc<dyn SpannerRpc>,
    database: String,
    region: String,
    session: Mutex<Option<String>>,
}

impl SpannerManagedLog {
    pub fn new(rpc: Arc<dyn SpannerRpc>, database: &str, region: &str) -> Self {
        Self {
            rpc,
            database: database.to_string(),
            region: region.to_string(),
            session: Mutex::new(None),
        }
    }

    fn session(&self) -> Result<String> {
        let mut held = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(session) = held.as_ref() {
            return Ok(session.clone());
        }
        let answer = match self
            .rpc
            .post(&format!("{}/sessions", self.database), &json!({}))?
        {
            RpcAnswer::Ok(answer) => answer,
            other => {
                return Err(Error::unavailable(format!(
                    "Spanner refused to create a session: {other:?}"
                )));
            }
        };
        let name = answer
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::schema("Spanner created a session and returned no name"))?
            .to_string();
        *held = Some(name.clone());
        Ok(name)
    }

    fn forget_session(&self) {
        *self.session.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn partition_params(&self, partition: &PartitionRef) -> Vec<(&'static str, Param)> {
        vec![
            ("region", Param::Str(self.region.clone())),
            ("stream", Param::Str(partition.stream.clone())),
            ("partition", Param::Int(i64::from(partition.partition))),
        ]
    }

    /// Run `body` in a read-write transaction, retrying the whole of it on
    /// `ABORTED` and on a lost session, at most [`MAXIMUM_ATTEMPTS`] times.
    fn read_write<T>(
        &self,
        what: &str,
        body: impl Fn(&mut Transaction<'_>) -> Result<std::result::Result<Step<T>, RpcAnswer>>,
    ) -> Result<T> {
        for _ in 0..MAXIMUM_ATTEMPTS {
            let session = self.session()?;
            let mut transaction = Transaction {
                rpc: self.rpc.as_ref(),
                session,
                id: None,
                seqno: 0,
            };
            let decided = match body(&mut transaction)? {
                Ok(step) => step,
                Err(RpcAnswer::SessionGone) => {
                    self.forget_session();
                    continue;
                }
                Err(_) => continue,
            };
            let (verb, value) = match decided {
                Step::Commit(value) => ("commit", value),
                Step::Rollback(value) => ("rollback", value),
            };
            match transaction.finish(verb)? {
                RpcAnswer::Ok(_) => return Ok(value),
                RpcAnswer::SessionGone => self.forget_session(),
                RpcAnswer::Aborted => {}
            }
        }
        Err(Error::unavailable(format!(
            "Spanner aborted the {what} transaction {MAXIMUM_ATTEMPTS} times; nothing was \
             committed by this call"
        )))
    }

    /// A strong single-use read.
    fn read(&self, mut body: Map<String, Value>) -> Result<Value> {
        body.insert(
            "transaction".to_string(),
            json!({ "singleUse": { "readOnly": { "strong": true } } }),
        );
        for _ in 0..MAXIMUM_ATTEMPTS {
            let session = self.session()?;
            match self.rpc.post(
                &format!("{session}:executeSql"),
                &Value::Object(body.clone()),
            )? {
                RpcAnswer::Ok(answer) => return Ok(answer),
                RpcAnswer::SessionGone => self.forget_session(),
                RpcAnswer::Aborted => {}
            }
        }
        Err(Error::unavailable(
            "Spanner could not serve a strong read after repeated attempts",
        ))
    }
}

/// `?` for a statement inside a transaction body: an `ABORTED` or lost
/// session ends the attempt with that answer so [`SpannerManagedLog::read_write`]
/// retries it.
macro_rules! attempt {
    ($e:expr) => {
        match $e? {
            Ok(value) => value,
            Err(answer) => return Ok(Err(answer)),
        }
    };
}

impl ManagedLog for SpannerManagedLog {
    fn acquire(&self, partition: &PartitionRef, holder: &str, ttl_ms: u64) -> Result<LeaseOutcome> {
        let ttl = i64::try_from(ttl_ms)
            .map_err(|_| Error::numeric(format!("a lease TTL of {ttl_ms} ms overflows INT64")))?;
        self.read_write("lease", |transaction| {
            let found = attempt!(
                transaction.execute(statement(SELECT_LEASE, self.partition_params(partition)))
            );
            let current = rows(&found).into_iter().next();
            let mut params = self.partition_params(partition);
            params.push(("holder", Param::Str(holder.to_string())));
            params.push(("ttl", Param::Int(ttl)));
            let (sql, epoch) = match current {
                None => (INSERT_LEASE, 1u64),
                Some(row) => {
                    let current_holder = as_text(cell(&row, 0, "the lease holder")?, "holder")?;
                    let epoch = as_u64(cell(&row, 1, "the lease epoch")?, "lease epoch")?;
                    let expired = cell(&row, 2, "the lease expiry")?
                        .as_bool()
                        .ok_or_else(|| Error::schema("Spanner's lease expiry is not a BOOL"))?;
                    if current_holder == holder {
                        (UPDATE_LEASE, epoch)
                    } else if expired {
                        let bumped = epoch
                            .checked_add(1)
                            .ok_or_else(|| Error::numeric("a lease epoch reached u64::MAX"))?;
                        (UPDATE_LEASE, bumped)
                    } else {
                        return Ok(Ok(Step::Rollback(LeaseOutcome::HeldByOther {
                            holder: current_holder,
                            epoch,
                        })));
                    }
                }
            };
            params.push(("epoch", int(epoch, "a lease epoch")?));
            attempt!(transaction.execute(statement(sql, params)));
            Ok(Ok(Step::Commit(LeaseOutcome::Held(Lease {
                holder: holder.to_string(),
                epoch,
            }))))
        })
    }

    fn commit(&self, lease: &Lease, row: &CommittedBatch) -> Result<CommitOutcome> {
        row.verify()?;
        let reference = &row.partition;
        self.read_write("batch commit", |transaction| {
            let found = attempt!(
                transaction.execute(statement(SELECT_LEASE, self.partition_params(reference)))
            );
            let Some(current) = rows(&found).into_iter().next() else {
                return Ok(Ok(Step::Rollback(CommitOutcome::Fenced {
                    current_epoch: 0,
                })));
            };
            let holder = as_text(cell(&current, 0, "the lease holder")?, "holder")?;
            let epoch = as_u64(cell(&current, 1, "the lease epoch")?, "lease epoch")?;
            if holder != lease.holder || epoch != lease.epoch || row.lease_epoch != lease.epoch {
                return Ok(Ok(Step::Rollback(CommitOutcome::Fenced {
                    current_epoch: epoch,
                })));
            }
            let next = attempt!(
                transaction.execute(statement(NEXT_OFFSET, self.partition_params(reference)))
            );
            let committed_through = rows(&next)
                .first()
                .and_then(|r| r.first())
                .map(|v| as_u64(v, "next offset"))
                .transpose()?
                .unwrap_or(0);
            if row.offset != committed_through {
                return Ok(Ok(Step::Rollback(CommitOutcome::Conflict {
                    committed_through,
                })));
            }
            let mut params = self.partition_params(reference);
            params.push(("offset", int(row.offset, "a batch offset")?));
            params.push(("epoch", int(row.lease_epoch, "a lease epoch")?));
            params.push(("hash", Param::Str(row.batch_hash.clone())));
            params.push((
                "previous",
                match &row.previous_batch_hash {
                    Some(hash) => Param::Str(hash.clone()),
                    None => Param::NullStr,
                },
            ));
            params.push(("batch", Param::Bytes(row.bytes.clone())));
            attempt!(transaction.execute(statement(INSERT_BATCH, params)));
            Ok(Ok(Step::Commit(CommitOutcome::Committed)))
        })
    }

    fn read(
        &self,
        partition: &PartitionRef,
        from_offset: u64,
        limit: u32,
    ) -> Result<Vec<CommittedBatch>> {
        let mut params = self.partition_params(partition);
        params.push(("from", int(from_offset, "an offset")?));
        params.push((
            "limit",
            Param::Int(i64::from(limit.min(MAXIMUM_ROWS_PER_READ))),
        ));
        let answer = SpannerManagedLog::read(self, statement(READ_BATCHES, params))?;
        let mut out = Vec::new();
        for row in rows(&answer) {
            let batch = CommittedBatch {
                partition: partition.clone(),
                offset: as_u64(cell(&row, 0, "batch_offset")?, "batch offset")?,
                lease_epoch: as_u64(cell(&row, 1, "lease_epoch")?, "lease epoch")?,
                batch_hash: as_text(cell(&row, 2, "batch_hash")?, "batch hash")?,
                previous_batch_hash: match cell(&row, 3, "previous_batch_hash")? {
                    Value::Null => None,
                    other => Some(as_text(other, "previous batch hash")?),
                },
                bytes: base64_decode(&as_text(cell(&row, 4, "batch")?, "batch")?)?,
            };
            batch.verify()?;
            out.push(batch);
        }
        Ok(out)
    }

    fn committed_through(&self, partition: &PartitionRef) -> Result<u64> {
        let answer = SpannerManagedLog::read(
            self,
            statement(NEXT_OFFSET, self.partition_params(partition)),
        )?;
        rows(&answer)
            .first()
            .and_then(|r| r.first())
            .map(|v| as_u64(v, "next offset"))
            .transpose()
            .map(|n| n.unwrap_or(0))
    }

    fn commit_offset(&self, group: &str, partition: &PartitionRef, offset: u64) -> Result<()> {
        let refused = self.read_write("checkpoint", |transaction| {
            let mut params = self.partition_params(partition);
            params.push(("group", Param::Str(group.to_string())));
            let found = attempt!(transaction.execute(statement(SELECT_CHECKPOINT, params)));
            let held = rows(&found)
                .first()
                .and_then(|r| r.first())
                .map(|v| as_u64(v, "committed offset"))
                .transpose()?;
            if let Some(held) = held
                && offset < held
            {
                return Ok(Ok(Step::Rollback(Some(held))));
            }
            let mut params = self.partition_params(partition);
            params.push(("group", Param::Str(group.to_string())));
            params.push(("offset", int(offset, "a checkpoint")?));
            let sql = if held.is_some() {
                UPDATE_CHECKPOINT
            } else {
                INSERT_CHECKPOINT
            };
            attempt!(transaction.execute(statement(sql, params)));
            Ok(Ok(Step::Commit(None)))
        })?;
        match refused {
            Some(held) => Err(Error::denied(format!(
                "group '{group}' tried to move its managed checkpoint on {}:{} back from {held} \
                 to {offset}",
                partition.stream, partition.partition
            ))),
            None => Ok(()),
        }
    }

    fn committed_offset(&self, group: &str, partition: &PartitionRef) -> Result<Option<u64>> {
        let mut params = self.partition_params(partition);
        params.push(("group", Param::Str(group.to_string())));
        let answer = SpannerManagedLog::read(self, statement(SELECT_CHECKPOINT, params))?;
        rows(&answer)
            .first()
            .and_then(|r| r.first())
            .map(|v| as_u64(v, "committed offset"))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A peer that answers from a script and records every call.
    #[derive(Debug, Default)]
    struct Scripted {
        answers: Mutex<VecDeque<RpcAnswer>>,
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl Scripted {
        fn new(answers: Vec<RpcAnswer>) -> Arc<Self> {
            Arc::new(Self {
                answers: Mutex::new(answers.into()),
                calls: Mutex::new(Vec::new()),
            })
        }
        fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl SpannerRpc for Scripted {
        fn post(&self, path: &str, body: &Value) -> Result<RpcAnswer> {
            self.calls
                .lock()
                .unwrap()
                .push((path.to_string(), body.clone()));
            Ok(self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("the script ran out: the adapter made a call nobody expected"))
        }
    }

    const DB: &str = "projects/p/instances/i/databases/d";

    fn session() -> RpcAnswer {
        RpcAnswer::Ok(json!({ "name": format!("{DB}/sessions/s1") }))
    }

    fn began(rows: Value) -> RpcAnswer {
        RpcAnswer::Ok(json!({ "metadata": { "transaction": { "id": "tx1" } }, "rows": rows }))
    }

    fn row(offset: u64, epoch: u64) -> CommittedBatch {
        let bytes = vec![1, 2, 3, 4, 5];
        CommittedBatch {
            partition: PartitionRef::new("control.test", 0),
            offset,
            lease_epoch: epoch,
            batch_hash: qip_core::hash::sha256_hex(&bytes),
            previous_batch_hash: None,
            bytes,
        }
    }

    fn lease(epoch: u64) -> Lease {
        Lease {
            holder: "fabricd-a#3".to_string(),
            epoch,
        }
    }

    fn sql_of(call: &(String, Value)) -> String {
        call.1
            .get("sql")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// ADR 0111's fencing, in the transaction: a commit whose lease row names
    /// a newer epoch rolls back as `Fenced` and never sends the insert.
    ///
    /// Mutation: drop `|| epoch != lease.epoch` from the lease comparison in
    /// `commit` — the stale commit proceeds to `NEXT_OFFSET`, the script runs
    /// out, and this fails.
    #[test]
    fn a_commit_under_a_superseded_lease_epoch_rolls_back_as_fenced_and_inserts_nothing() {
        let rpc = Scripted::new(vec![
            session(),
            began(json!([["fabricd-a#3", "3", false]])),
            RpcAnswer::Ok(json!({})),
        ]);
        let log = SpannerManagedLog::new(rpc.clone(), DB, "europe-west2");
        let outcome = log.commit(&lease(2), &row(0, 2)).expect("answered");
        assert_eq!(outcome, CommitOutcome::Fenced { current_epoch: 3 });
        let calls = rpc.calls();
        assert!(
            calls.iter().all(|c| !sql_of(c).starts_with("INSERT")),
            "{calls:?}"
        );
        assert!(
            calls.last().is_some_and(|c| c.0.ends_with(":rollback")),
            "premise: the transaction is rolled back, not left open: {calls:?}"
        );
    }

    /// The happy path sends the lease read, the density read and the insert
    /// in one transaction, carrying the bytes as base64 and the commit
    /// timestamp as Spanner's own, and commits.
    ///
    /// Mutation: replace `PENDING_COMMIT_TIMESTAMP()` in `INSERT_BATCH` with
    /// `CURRENT_TIMESTAMP()` — the assertion on the statement fires.
    #[test]
    fn a_commit_under_the_current_lease_inserts_the_batch_at_the_next_offset_and_commits() {
        let rpc = Scripted::new(vec![
            session(),
            began(json!([["fabricd-a#3", "2", false]])),
            RpcAnswer::Ok(json!({ "rows": [["0"]] })),
            RpcAnswer::Ok(json!({})),
            RpcAnswer::Ok(json!({ "commitTimestamp": "2026-10-11T00:00:00Z" })),
        ]);
        let log = SpannerManagedLog::new(rpc.clone(), DB, "europe-west2");
        assert_eq!(
            log.commit(&lease(2), &row(0, 2)).expect("answered"),
            CommitOutcome::Committed
        );
        let calls = rpc.calls();
        let insert = calls
            .iter()
            .find(|c| sql_of(c).starts_with("INSERT INTO fabric_batches"))
            .expect("the batch is inserted");
        assert!(sql_of(insert).contains("PENDING_COMMIT_TIMESTAMP()"));
        assert_eq!(
            insert.1["params"]["batch"],
            json!(base64_encode(&[1, 2, 3, 4, 5]))
        );
        assert_eq!(insert.1["params"]["epoch"], json!("2"));
        assert_eq!(insert.1["transaction"], json!({ "id": "tx1" }));
        assert!(calls.last().is_some_and(|c| c.0.ends_with(":commit")));
    }

    /// A commit that conflicts with a row already at the offset is answered
    /// as a conflict carrying where the log really is, and rolled back.
    ///
    /// Mutation: compare `row.offset > committed_through` instead of `!=` —
    /// an offset behind the log is inserted and this fails.
    #[test]
    fn a_commit_behind_the_committed_log_rolls_back_as_a_conflict() {
        let rpc = Scripted::new(vec![
            session(),
            began(json!([["fabricd-a#3", "2", false]])),
            RpcAnswer::Ok(json!({ "rows": [["5"]] })),
            RpcAnswer::Ok(json!({})),
        ]);
        let log = SpannerManagedLog::new(rpc.clone(), DB, "europe-west2");
        assert_eq!(
            log.commit(&lease(2), &row(3, 2)).expect("answered"),
            CommitOutcome::Conflict {
                committed_through: 5
            }
        );
        assert!(rpc.calls().iter().all(|c| !sql_of(c).starts_with("INSERT")));
    }

    /// An `ABORTED` commit is retried whole: the lease is read again, so a
    /// takeover that won the conflict is seen on the retry.
    ///
    /// Mutation: return the error on `RpcAnswer::Aborted` in `read_write`
    /// instead of retrying — the first attempt's abort is the answer and the
    /// `Fenced` assertion fires.
    #[test]
    fn an_aborted_commit_is_retried_and_the_retry_sees_the_takeover() {
        let rpc = Scripted::new(vec![
            session(),
            began(json!([["fabricd-a#3", "2", false]])),
            RpcAnswer::Ok(json!({ "rows": [["0"]] })),
            RpcAnswer::Ok(json!({})),
            RpcAnswer::Aborted,
            began(json!([["fabricd-b#1", "3", false]])),
            RpcAnswer::Ok(json!({})),
        ]);
        let log = SpannerManagedLog::new(rpc.clone(), DB, "europe-west2");
        assert_eq!(
            log.commit(&lease(2), &row(0, 2)).expect("answered"),
            CommitOutcome::Fenced { current_epoch: 3 }
        );
    }

    /// Taking an expired lease bumps its epoch; another holder's live lease
    /// is reported, not taken.
    ///
    /// Mutation: in `acquire`, take the lease when `!expired` too — the live
    /// lease is taken and the `HeldByOther` assertion fires.
    #[test]
    fn an_expired_lease_is_taken_at_the_next_epoch_and_a_live_one_is_left_alone() {
        let rpc = Scripted::new(vec![
            session(),
            began(json!([["fabricd-b#1", "4", true]])),
            RpcAnswer::Ok(json!({})),
            RpcAnswer::Ok(json!({})),
        ]);
        let log = SpannerManagedLog::new(rpc.clone(), DB, "europe-west2");
        let partition = PartitionRef::new("control.test", 0);
        assert_eq!(
            log.acquire(&partition, "fabricd-a#3", 10_000)
                .expect("answered"),
            LeaseOutcome::Held(Lease {
                holder: "fabricd-a#3".to_string(),
                epoch: 5
            })
        );

        let rpc = Scripted::new(vec![
            session(),
            began(json!([["fabricd-b#1", "4", false]])),
            RpcAnswer::Ok(json!({})),
        ]);
        let log = SpannerManagedLog::new(rpc.clone(), DB, "europe-west2");
        assert_eq!(
            log.acquire(&partition, "fabricd-a#3", 10_000)
                .expect("answered"),
            LeaseOutcome::HeldByOther {
                holder: "fabricd-b#1".to_string(),
                epoch: 4
            }
        );
        assert!(rpc.calls().iter().all(|c| !sql_of(c).starts_with("UPDATE")));
    }

    /// RFC 4648 §10's vectors, both ways, and the refusals.
    ///
    /// Mutation: emit '=' for the third character whenever `chunk.len() < 3`
    /// — "fo" encodes as "Zg==" and this fails.
    #[test]
    fn base64_matches_rfc_4648_and_refuses_what_is_not_base64() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(plain.as_bytes()), encoded);
            assert_eq!(base64_decode(encoded).expect("decodes"), plain.as_bytes());
        }
        for bad in ["Zg=", "Zg=a", "Z===", "Zm9v!A==", "Zg==Zg=="] {
            assert!(base64_decode(bad).is_err(), "{bad}");
        }
    }
}
