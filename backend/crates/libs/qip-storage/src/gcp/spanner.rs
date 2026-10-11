//! Cloud Spanner over its JSON REST API: the requests a key-value table
//! needs and nothing more (ADR 0113 §2).
//!
//! ADR 0113 makes Spanner the ledger's store of record and says how it is
//! reached: REST, through the same [`GcpAccess`] the Cloud Storage adapter
//! uses, not a client SDK. This module is that shim — sessions, a strong
//! read, a read inside a read-write transaction, a commit of mutations, a
//! rollback — over one two-column table:
//!
//! ```sql
//! CREATE TABLE ledger_kv (k STRING(MAX) NOT NULL, v STRING(MAX) NOT NULL) PRIMARY KEY (k)
//! ```
//!
//! ([`KV_TABLE_DDL`]). What lives in the table, and what one commit must
//! contain, is the caller's: `qip-ledgerd` composes it into the ledger's
//! store port and commits each delivery's batch as one transaction.
//!
//! # What it does not do
//!
//! The same as the rest of [`crate::gcp`]: it does not retry, does not fall
//! back, and does not hold a connection. One request per call, a refusal
//! classified by status. Two classes are Spanner's own and are named here:
//! an aborted transaction (HTTP 409, `ABORTED`) is
//! [`qip_core::Error::Unavailable`] — the caller's whole transaction may be
//! retried from the start, which is what a ledger worker does with any store
//! error — and a session the service no longer knows (HTTP 404 on a session
//! path) drops the cached session so the next call creates a new one.
//!
//! # The peer is untrusted
//!
//! A session name and a transaction id come back from the peer and go into
//! the next request's path or body. Both are checked before they are used: a
//! session name must sit under this database's own path and contain nothing
//! but the characters Spanner issues, so a misconfigured proxy cannot steer a
//! commit to another database by answering with one.

use super::{GcpAccess, status_refusal};
use qip_core::error::{Error, Result};
use qip_transport::{HttpClient, Method};
use serde_json::{Value, json};
use std::sync::Mutex;

/// The one table [`SpannerKv`] reads and writes.
pub const KV_TABLE_DDL: &str =
    "CREATE TABLE ledger_kv (k STRING(MAX) NOT NULL, v STRING(MAX) NOT NULL) PRIMARY KEY (k)";

/// The default table name, matching [`KV_TABLE_DDL`].
pub const DEFAULT_TABLE: &str = "ledger_kv";

/// Rows one read page asks for.
const DEFAULT_PAGE_ROWS: u32 = 1000;

/// Pages one prefix read follows before it refuses rather than truncates.
const DEFAULT_MAX_PAGES: u32 = 10_000;

/// Which database, named by the three identifiers Google gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpannerDatabase {
    project: String,
    instance: String,
    database: String,
}

fn identifier(what: &str, value: &str) -> Result<String> {
    let value = value.trim();
    let well_formed = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !well_formed {
        return Err(Error::invalid(format!(
            "the Spanner {what} `{value}` is not an identifier (letters, digits, `-` and `_`, at \
             most 128); it goes into a request path and is refused rather than escaped"
        )));
    }
    Ok(value.to_string())
}

impl SpannerDatabase {
    /// Name a database, refusing anything that is not a plain identifier.
    pub fn new(project: &str, instance: &str, database: &str) -> Result<Self> {
        Ok(Self {
            project: identifier("project", project)?,
            instance: identifier("instance", instance)?,
            database: identifier("database", database)?,
        })
    }

    /// `projects/{p}/instances/{i}/databases/{d}`.
    pub fn path(&self) -> String {
        format!(
            "projects/{}/instances/{}/databases/{}",
            self.project, self.instance, self.database
        )
    }
}

/// Everything a [`SpannerKv`] needs.
#[derive(Clone, Debug)]
pub struct SpannerConfig {
    pub database: SpannerDatabase,
    /// The two-column table of [`KV_TABLE_DDL`].
    pub table: String,
    pub access: GcpAccess,
    pub page_rows: u32,
    pub max_pages: u32,
}

impl SpannerConfig {
    pub fn new(database: SpannerDatabase, access: GcpAccess) -> Self {
        Self {
            database,
            table: DEFAULT_TABLE.to_string(),
            access,
            page_rows: DEFAULT_PAGE_ROWS,
            max_pages: DEFAULT_MAX_PAGES,
        }
    }

    /// Use another table of the same shape.
    pub fn with_table(mut self, table: &str) -> Result<Self> {
        let well_formed = table
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic())
            && table.len() <= 128
            && table
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !well_formed {
            return Err(Error::invalid(format!(
                "the Spanner table `{table}` is not a table name (a letter, then letters, digits \
                 and `_`)"
            )));
        }
        self.table = table.to_string();
        Ok(self)
    }

    pub fn with_page_rows(mut self, rows: u32) -> Self {
        self.page_rows = rows;
        self
    }
}

/// A read-write transaction the service opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    id: String,
}

/// One change to the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mutation {
    /// Insert or overwrite the row at `key`.
    Put { key: String, value: String },
    /// Delete the row at `key`; absent is not an error.
    Delete { key: String },
}

/// A key-value table in one Spanner database.
#[derive(Debug)]
pub struct SpannerKv {
    config: SpannerConfig,
    client: HttpClient,
    session: Mutex<Option<String>>,
}

impl SpannerKv {
    /// A client for `config`. Refuses, opening no connection, when the
    /// access has no endpoint or no credential: a ledger configured for
    /// Spanner that could not reach it must not start (ADR 0113 §1).
    pub fn new(config: SpannerConfig) -> Result<Self> {
        let missing = config.access.missing_configuration();
        if !missing.is_empty() {
            return Err(Error::unavailable(format!(
                "Cloud Spanner database {} cannot be reached and nothing will be written \
                 anywhere else: {}",
                config.database.path(),
                missing.join("; ")
            )));
        }
        if config.page_rows == 0 || config.max_pages == 0 {
            return Err(Error::invalid(
                "a Spanner read needs at least one row per page and one page",
            ));
        }
        let client = HttpClient::new(config.access.limits());
        Ok(Self {
            config,
            client,
            session: Mutex::new(None),
        })
    }

    pub fn database(&self) -> &SpannerDatabase {
        &self.config.database
    }

    fn service(&self) -> String {
        format!("Cloud Spanner database {}", self.config.database.path())
    }

    /// POST `body` to `path`, returning the parsed JSON response.
    fn post(&self, path: &str, body: &Value, session_scoped: bool) -> Result<Value> {
        let request = self
            .config
            .access
            .request(Method::Post, path)?
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_vec(body)?);
        let response = self.client.send(&request).map_err(Error::from)?;
        if response.status == 404 && session_scoped {
            // The service forgot the session (idle expiry, a restart):
            // the next call makes a new one.
            *self.session.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return Err(Error::unavailable(format!(
                "{} no longer knows this process's session (HTTP 404); a new one is created on \
                 the next call: {}",
                self.service(),
                response.body_excerpt()
            )));
        }
        if response.status == 409 {
            return Err(Error::unavailable(format!(
                "{} aborted the transaction (HTTP 409); nothing in it was applied and the \
                 caller may retry it from the start: {}",
                self.service(),
                response.body_excerpt()
            )));
        }
        if !response.is_success() {
            return Err(status_refusal(
                &self.service(),
                response.status,
                &response.body_excerpt(),
            ));
        }
        if response.body.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }
        serde_json::from_slice(&response.body).map_err(|e| {
            Error::schema(format!(
                "{} answered with a body that is not JSON: {e}",
                self.service()
            ))
        })
    }

    /// This process's session, created on first use.
    fn session(&self) -> Result<String> {
        if let Some(name) = self
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Ok(name);
        }
        let path = format!("/v1/{}/sessions", self.config.database.path());
        let created = self.post(&path, &json!({}), false)?;
        let name = created.get("name").and_then(Value::as_str).ok_or_else(|| {
            Error::schema(format!(
                "{} created a session and named none",
                self.service()
            ))
        })?;
        let expected = format!("{}/sessions/", self.config.database.path());
        let issued = name.strip_prefix(&expected).unwrap_or_default();
        if issued.is_empty()
            || !issued
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(Error::schema(format!(
                "{} issued a session name that is not under {expected}; refusing to send a \
                 commit wherever the peer pointed",
                self.service()
            )));
        }
        let name = name.to_string();
        *self.session.lock().unwrap_or_else(|e| e.into_inner()) = Some(name.clone());
        Ok(name)
    }

    fn session_call(&self, method: &str, body: &Value) -> Result<Value> {
        let session = self.session()?;
        self.post(&format!("/v1/{session}:{method}"), body, true)
    }

    /// Open a read-write transaction. Its reads take locks, and its commit
    /// fails — changing nothing — if another transaction wrote what it read.
    pub fn begin(&self) -> Result<Transaction> {
        let answer = self.session_call(
            "beginTransaction",
            &json!({ "options": { "readWrite": {} } }),
        )?;
        let id = answer
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                Error::schema(format!(
                    "{} began a transaction and gave it no id",
                    self.service()
                ))
            })?;
        Ok(Transaction { id: id.to_string() })
    }

    /// One row, read inside `transaction` (locking it) or, with `None`, as a
    /// strong single-use read.
    pub fn get(&self, transaction: Option<&Transaction>, key: &str) -> Result<Option<String>> {
        let rows = self.read(transaction, json!({ "keys": [[key]] }), 1)?;
        match rows.as_slice() {
            [] => Ok(None),
            [(found, value)] if found == key => Ok(Some(value.clone())),
            _ => Err(Error::schema(format!(
                "{} answered a read of one key with rows for another",
                self.service()
            ))),
        }
    }

    /// Every row whose key starts with `prefix`, in key order, read strongly
    /// page by page. Refused rather than truncated past the page bound.
    pub fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        let end = prefix_end(prefix)?;
        let mut rows: Vec<(String, String)> = Vec::new();
        for _ in 0..self.config.max_pages {
            let range = match rows.last() {
                None => json!({ "startClosed": [prefix], "endOpen": [end] }),
                Some((last, _)) => json!({ "startOpen": [last], "endOpen": [end] }),
            };
            let page = self.read(None, json!({ "ranges": [range] }), self.config.page_rows)?;
            let full = page.len() >= self.config.page_rows as usize;
            for (key, value) in page {
                if !key.starts_with(prefix) || rows.last().is_some_and(|(last, _)| *last >= key) {
                    return Err(Error::schema(format!(
                        "{} answered a prefix read with a row out of order or out of range",
                        self.service()
                    )));
                }
                rows.push((key, value));
            }
            if !full {
                return Ok(rows);
            }
        }
        Err(Error::guard(format!(
            "{} holds more than {} pages of rows under {prefix:?}; the read is refused rather \
             than returned short",
            self.service(),
            self.config.max_pages
        )))
    }

    fn read(
        &self,
        transaction: Option<&Transaction>,
        key_set: Value,
        limit: u32,
    ) -> Result<Vec<(String, String)>> {
        let selector = match transaction {
            Some(transaction) => json!({ "id": transaction.id }),
            None => json!({ "singleUse": { "readOnly": { "strong": true } } }),
        };
        let answer = self.session_call(
            "read",
            &json!({
                "table": self.config.table,
                "columns": ["k", "v"],
                "keySet": key_set,
                "transaction": selector,
                "limit": limit.to_string(),
            }),
        )?;
        let Some(rows) = answer.get("rows") else {
            return Ok(Vec::new());
        };
        let rows = rows.as_array().ok_or_else(|| {
            Error::schema(format!(
                "{} answered a read whose rows are not a list",
                self.service()
            ))
        })?;
        if rows.len() > limit as usize {
            return Err(Error::schema(format!(
                "{} answered a read limited to {limit} rows with {}",
                self.service(),
                rows.len()
            )));
        }
        rows.iter()
            .map(|row| match row.as_array().map(Vec::as_slice) {
                Some([Value::String(key), Value::String(value)]) => {
                    Ok((key.clone(), value.clone()))
                }
                _ => Err(Error::schema(format!(
                    "{} answered a read with a row that is not two strings",
                    self.service()
                ))),
            })
            .collect()
    }

    /// Commit `mutations` atomically: inside `transaction` if one is given,
    /// otherwise as a single-use read-write transaction.
    pub fn commit(&self, transaction: Option<&Transaction>, mutations: &[Mutation]) -> Result<()> {
        let table = &self.config.table;
        let mutations: Vec<Value> = mutations
            .iter()
            .map(|mutation| match mutation {
                Mutation::Put { key, value } => json!({
                    "insertOrUpdate": {
                        "table": table,
                        "columns": ["k", "v"],
                        "values": [[key, value]],
                    }
                }),
                Mutation::Delete { key } => json!({
                    "delete": { "table": table, "keySet": { "keys": [[key]] } }
                }),
            })
            .collect();
        let body = match transaction {
            Some(transaction) => json!({ "transactionId": transaction.id, "mutations": mutations }),
            None => json!({
                "singleUseTransaction": { "readWrite": {} },
                "mutations": mutations,
            }),
        };
        self.session_call("commit", &body).map(|_| ())
    }

    /// Release `transaction`'s locks without applying anything.
    pub fn rollback(&self, transaction: &Transaction) -> Result<()> {
        self.session_call("rollback", &json!({ "transactionId": transaction.id }))
            .map(|_| ())
    }
}

/// The smallest key greater than every key starting with `prefix`: the
/// prefix with its last byte incremented. Keys here are ASCII, and a prefix
/// that is empty or ends in DEL has no such key and is refused.
pub fn prefix_end(prefix: &str) -> Result<String> {
    let mut bytes = prefix.as_bytes().to_vec();
    match bytes.last_mut() {
        Some(last) if last.is_ascii() && *last < 0x7f => {
            *last += 1;
            String::from_utf8(bytes)
                .map_err(|_| Error::invalid(format!("prefix {prefix:?} is not ASCII")))
        }
        _ => Err(Error::invalid(format!(
            "prefix {prefix:?} has no ASCII successor; a prefix read needs a non-empty ASCII \
             prefix"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A prefix read is a half-open range ending at the prefix's successor;
    /// an end one too far would read the next prefix's rows as this one's.
    /// Mutation: add 2 instead of 1 — `balance/` ends at `balance1` and the
    /// first assertion fails.
    #[test]
    fn a_prefix_range_ends_at_the_prefixs_immediate_successor_and_an_unbounded_one_is_refused() {
        assert_eq!(prefix_end("balance/").expect("ascii"), "balance0");
        assert_eq!(prefix_end("a").expect("ascii"), "b");
        assert!(prefix_end("").is_err());
        assert!(prefix_end("x\u{7f}").is_err());
    }

    /// Identifiers go into a request path unescaped, so anything that could
    /// change the path is refused rather than escaped. Mutation: drop the
    /// character check in `identifier` — the `../` database is admitted.
    #[test]
    fn a_database_named_with_anything_but_identifier_characters_is_refused() {
        let database = SpannerDatabase::new("proj-1", "ledger", "positions").expect("plain");
        assert_eq!(
            database.path(),
            "projects/proj-1/instances/ledger/databases/positions"
        );
        assert!(SpannerDatabase::new("proj", "ledger", "../other").is_err());
        assert!(SpannerDatabase::new("proj", "", "positions").is_err());
        assert!(SpannerDatabase::new("proj", "led ger", "positions").is_err());
    }
}
