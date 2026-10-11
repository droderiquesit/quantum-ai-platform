//! The ledger of record on Cloud Spanner (ADR 0113 §1).
//!
//! [`SpannerBackend`] implements the ledger's store port over
//! `qip_storage::gcp::spanner::SpannerKv`. Every [`LedgerBatch`] — one
//! delivery's postings, balances, chain tail, park, journal note and offset,
//! or one operation's — is **one read-write transaction**: the direct
//! translation of ADR 0105's single `WriteBatch`. A refused or aborted
//! commit changes nothing, and the partition worker retries the same
//! delivery from the start, as it does for any store error.
//!
//! # One writer, fenced
//!
//! ADR 0100 §2 makes `qip-ledgerd` the ledger's only writer, and on a local
//! disk that held by construction: one process owns one directory. A
//! database outlives processes, so a replaced machine and the one it
//! replaced can both be running for a while, both deciding on the same chain
//! tail. The fence: [`SpannerBackend::claim`] writes this process's writer
//! id to one row when the ledger opens, and every commit reads that row
//! **inside its own transaction** before writing. Spanner's read-write
//! transactions lock what they read, so a commit either sees its own id and
//! applies, or sees another's and is refused with nothing applied; and a
//! claim that lands between a commit's read and its write aborts that commit.
//! The newest process wins and the old one stops writing — it does not
//! interleave.
//!
//! The decision reads (tail, park, cursor) happen before the transaction,
//! under the store's own writer lock. With the fence, nothing but this
//! process can have changed them since, which is what makes a decision on
//! them a decision on the state the commit applies to.
//!
//! # No fallback
//!
//! A ledger configured for Spanner that cannot reach it refuses to start
//! (`crate::start`), and a commit that cannot reach it is an error the
//! worker retries. Nothing here writes to a local disk instead.

use crate::backend::{BatchOperation, LedgerBackend, LedgerBatch};
use qip_core::error::{Error, Result};
use qip_storage::gcp::spanner::{Mutation, SpannerKv};

/// The row that names the process allowed to write.
pub const KEY_WRITER: &str = "meta/writer";

/// The ledger's rows in one Spanner table.
#[derive(Debug)]
pub struct SpannerBackend {
    kv: SpannerKv,
    writer: String,
}

fn encode(value: &serde_json::Value) -> Result<String> {
    serde_json::to_string(value).map_err(Error::from)
}

fn parse(key: &str, text: &str) -> Result<serde_json::Value> {
    serde_json::from_str(text).map_err(|e| {
        Error::schema(format!(
            "the Spanner ledger row {key:?} is not the JSON this ledger writes: {e}"
        ))
    })
}

impl SpannerBackend {
    /// Open the ledger on `kv` as `writer`, claiming the writer row so any
    /// earlier process stops writing (see the module documentation).
    pub fn claim(kv: SpannerKv, writer: &str) -> Result<Self> {
        if writer.trim().is_empty() {
            return Err(Error::invalid(
                "a Spanner ledger writer needs a non-empty writer id; an anonymous claim fences \
                 nobody out",
            ));
        }
        let transaction = kv.begin()?;
        // Read inside the transaction for the lock the read takes, not for
        // the value: a commit of the previous writer that is in flight
        // either lands first or aborts. Whoever held the row is overwritten
        // regardless — the newest process wins.
        kv.get(Some(&transaction), KEY_WRITER)?;
        kv.commit(
            Some(&transaction),
            &[Mutation::Put {
                key: KEY_WRITER.to_string(),
                value: encode(&serde_json::Value::String(writer.to_string()))?,
            }],
        )?;
        Ok(Self {
            kv,
            writer: writer.to_string(),
        })
    }

    /// The writer id this process claimed with.
    pub fn writer(&self) -> &str {
        &self.writer
    }
}

impl LedgerBackend for SpannerBackend {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        self.kv
            .get(None, key)?
            .map(|text| parse(key, &text))
            .transpose()
    }

    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, serde_json::Value)>> {
        self.kv
            .scan_prefix(prefix)?
            .into_iter()
            .map(|(key, text)| {
                let value = parse(&key, &text)?;
                Ok((key, value))
            })
            .collect()
    }

    fn commit(&self, batch: LedgerBatch) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        let mut mutations = Vec::with_capacity(batch.operations().len());
        for operation in batch.operations() {
            mutations.push(match operation {
                BatchOperation::Put { key, value } => Mutation::Put {
                    key: key.clone(),
                    value: encode(value)?,
                },
                BatchOperation::Delete { key } => Mutation::Delete { key: key.clone() },
            });
        }
        let transaction = self.kv.begin()?;
        let holder = match self.kv.get(Some(&transaction), KEY_WRITER) {
            Ok(holder) => holder,
            Err(error) => {
                // Best effort: the lock is released by the service on expiry
                // whether or not this reaches it, and the read's error is the
                // one worth reporting.
                let _ = self.kv.rollback(&transaction);
                return Err(error);
            }
        };
        let holder = holder
            .map(|text| parse(KEY_WRITER, &text))
            .transpose()?
            .and_then(|value| value.as_str().map(str::to_string));
        if holder.as_deref() != Some(self.writer.as_str()) {
            let _ = self.kv.rollback(&transaction);
            return Err(Error::denied(format!(
                "the Spanner ledger's writer row names {} and this process is {}; another \
                 qip-ledgerd has claimed the ledger, and this one writes nothing more. Stop \
                 this process",
                holder.as_deref().unwrap_or("nobody"),
                self.writer
            )));
        }
        self.kv.commit(Some(&transaction), &mutations)
    }

    fn kind(&self) -> &'static str {
        "spanner"
    }
}
