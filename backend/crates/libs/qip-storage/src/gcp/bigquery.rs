//! The research warehouse: stream rows into a BigQuery table, and query it.
//!
//! Two operations, because they are the two
//! [`crate::provider::StorageTarget::BigQuery`] exists for: getting research
//! history in, and getting columnar scans of it back out.
//!
//! # The two ways BigQuery reports failure without failing
//!
//! Both are handled here explicitly, because both look like success to a
//! client that only checks the HTTP status, and both lose data silently when
//! they are missed.
//!
//! **A streaming insert answers HTTP 200 when it rejected rows.** The
//! rejections are in the body, in `insertErrors`, one entry per row index. A
//! client that treats 200 as "inserted" writes a backtest result set that is
//! quietly missing whichever rows had a schema mismatch. [`InsertOutcome`] is
//! `#[must_use]` for this reason and carries every rejection.
//!
//! **A query answers HTTP 200 with `jobComplete: false` when it has not
//! finished.** The `rows` field is then absent — not empty, absent — and a
//! client that decodes it as an empty array concludes the query returned no
//! results. That is the difference between "no strategy breached its limit"
//! and "we did not find out". This adapter never converts an incomplete job
//! into an empty result set; see [`BigQueryWarehouse::query`].

use super::{GcpAccess, percent_encode, status_refusal};
use qip_core::error::{Error, Result};
use qip_transport::{HttpClient, Method};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Rows one `insertAll` request will carry.
///
/// Google's own documented ceiling is 10 000 rows and 10 MB per request, and
/// they recommend far fewer. 500 is a batch that stays well inside the payload
/// limit for realistically-sized research rows.
const DEFAULT_MAX_ROWS_PER_INSERT: usize = 500;

/// Rows one page of query results asks for.
const DEFAULT_PAGE_SIZE: u32 = 10_000;

/// Result pages followed before a query is refused.
const DEFAULT_MAX_RESULT_PAGES: u32 = 100;

/// How many times a still-running query is asked again.
///
/// Each ask is a server-side long poll bounded by
/// [`BigQueryConfig::query_timeout_ms`], so this is a bound on total waiting
/// and not a spin: nothing sleeps on this thread between attempts.
const DEFAULT_MAX_QUERY_POLLS: u32 = 10;

/// Milliseconds BigQuery is asked to hold a request open waiting for a job.
const DEFAULT_QUERY_TIMEOUT_MS: u64 = 10_000;

/// Everything a deployment must decide before the warehouse can be used.
#[derive(Clone, Debug)]
pub struct BigQueryConfig {
    /// The GCP project the job is billed to and the table lives in.
    pub project: String,
    /// The dataset holding the table.
    pub dataset: String,
    /// Rows per `insertAll` request.
    pub max_rows_per_insert: usize,
    /// Rows per page of query results.
    pub page_size: u32,
    /// Result pages followed before a query is refused.
    pub max_result_pages: u32,
    /// Times a still-running job is asked again before the query is reported
    /// incomplete.
    pub max_query_polls: u32,
    /// Milliseconds BigQuery holds each request open waiting for the job.
    ///
    /// Server-side waiting, which is why it is preferred to a client-side
    /// sleep: the platform forbids ambient time, and a thread that sleeps to
    /// poll is exactly what an injected clock exists to avoid.
    pub query_timeout_ms: u64,
    /// Endpoint and credential.
    pub access: GcpAccess,
}

impl BigQueryConfig {
    pub fn new(project: impl Into<String>, dataset: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            dataset: dataset.into(),
            max_rows_per_insert: DEFAULT_MAX_ROWS_PER_INSERT,
            page_size: DEFAULT_PAGE_SIZE,
            max_result_pages: DEFAULT_MAX_RESULT_PAGES,
            max_query_polls: DEFAULT_MAX_QUERY_POLLS,
            query_timeout_ms: DEFAULT_QUERY_TIMEOUT_MS,
            access: GcpAccess::unconfigured(),
        }
    }

    /// Supply the endpoint and credential.
    ///
    /// A deployment's project, dataset and access are resolved by the
    /// composition root through
    /// [`crate::managed::ManagedSettings::big_query_config`]; this crate
    /// reads no environment variable itself.
    pub fn with_access(mut self, access: GcpAccess) -> Self {
        self.access = access;
        self
    }

    pub fn with_max_rows_per_insert(mut self, rows: usize) -> Self {
        self.max_rows_per_insert = rows;
        self
    }

    pub fn with_page_size(mut self, rows: u32) -> Self {
        self.page_size = rows;
        self
    }

    pub fn with_max_query_polls(mut self, polls: u32) -> Self {
        self.max_query_polls = polls;
        self
    }
}

/// One row offered to a streaming insert.
#[derive(Clone, Debug)]
pub struct InsertRow {
    /// BigQuery's best-effort deduplication key.
    ///
    /// Supplying one is strongly advised and is why an insert is safe to
    /// retry: BigQuery remembers recently-seen insert ids and drops a repeat.
    /// The promise is **best effort within a time window BigQuery does not
    /// specify** — it is not an idempotency guarantee, and a retry long after
    /// the original will duplicate the row. A caller that cannot tolerate a
    /// duplicate must deduplicate at query time.
    pub insert_id: Option<String>,
    /// The row itself. Its fields must match the table's schema; this adapter
    /// does not know the schema and does not check.
    pub json: serde_json::Value,
}

impl InsertRow {
    /// A row with an explicit deduplication key.
    pub fn with_id(insert_id: impl Into<String>, json: serde_json::Value) -> Self {
        Self {
            insert_id: Some(insert_id.into()),
            json,
        }
    }

    /// A row with no deduplication key, which BigQuery will insert on every
    /// retry. Use [`InsertRow::with_id`] unless duplicates are acceptable.
    pub fn anonymous(json: serde_json::Value) -> Self {
        Self {
            insert_id: None,
            json,
        }
    }
}

/// Why BigQuery rejected one row of an insert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowError {
    /// The row's index in the batch that was submitted.
    pub index: usize,
    /// BigQuery's reason codes, e.g. `invalid`, `stopped`.
    pub reasons: Vec<String>,
    /// BigQuery's human-readable messages.
    pub messages: Vec<String>,
}

impl std::fmt::Display for RowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "row {} rejected ({}): {}",
            self.index,
            self.reasons.join(", "),
            self.messages.join("; ")
        )
    }
}

/// What an insert actually did.
///
/// `#[must_use]` deliberately. BigQuery answers HTTP 200 for a partially
/// failed streaming insert, so a caller that discards this value has written a
/// table that is silently missing rows, and will not find out until somebody
/// queries it and gets an answer that is wrong rather than absent.
/// [`InsertOutcome::into_result`] is the one-line way to treat any rejection
/// as a failure.
#[must_use = "BigQuery answers HTTP 200 even when it rejected rows; discarding this outcome \
              silently loses them. Call `into_result()` to refuse any rejection, or inspect \
              `rejected()`"]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsertOutcome {
    submitted: usize,
    rejected: Vec<RowError>,
}

impl InsertOutcome {
    /// Rows submitted in the request.
    pub fn submitted(&self) -> usize {
        self.submitted
    }

    /// Rows BigQuery accepted: everything submitted that it did not reject.
    pub fn inserted(&self) -> usize {
        self.submitted.saturating_sub(self.rejected.len())
    }

    /// Rows BigQuery refused, with its reason for each.
    pub fn rejected(&self) -> &[RowError] {
        &self.rejected
    }

    /// Whether every submitted row was accepted.
    pub fn is_complete(&self) -> bool {
        self.rejected.is_empty()
    }

    /// Turn any rejection into an error naming every rejected row.
    ///
    /// For the common caller, which has no way to do anything useful with a
    /// partial insert and must not proceed as though it succeeded.
    pub fn into_result(self) -> Result<Self> {
        if self.is_complete() {
            return Ok(self);
        }
        let detail = self
            .rejected
            .iter()
            .map(RowError::to_string)
            .collect::<Vec<_>>()
            .join(" | ");
        Err(Error::schema(format!(
            "BigQuery accepted {} of {} rows and rejected {}: {detail}. The request itself \
             answered HTTP 200 — a streaming insert reports rejected rows in its body, not in \
             its status",
            self.inserted(),
            self.submitted,
            self.rejected.len()
        )))
    }
}

/// A named query parameter.
///
/// Parameters exist so that a caller never builds SQL by concatenating values
/// into it. This adapter does not parse the SQL it is given and cannot tell a
/// literal from an injected one, so the parameter list is the only defence
/// there is, and it is the caller's job to use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryParameter {
    name: String,
    kind: String,
    value: String,
}

impl QueryParameter {
    /// A parameter of an explicit BigQuery scalar type: `STRING`, `INT64`,
    /// `FLOAT64`, `BOOL`, `TIMESTAMP`, `DATE`, `NUMERIC`, …
    ///
    /// The value is always sent as a string, which is how BigQuery's REST API
    /// takes every parameter value regardless of type; the `kind` is what tells
    /// it how to read that string.
    pub fn new(
        name: impl Into<String>,
        kind: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self> {
        let name = name.into();
        let kind = kind.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "a query parameter needs a name: named parameters are referenced as @name in the \
                 SQL, and an unnamed one cannot be",
            ));
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::invalid(format!(
                "the query parameter name {name:?} is not a BigQuery identifier: ASCII letters, \
                 digits and underscore only"
            )));
        }
        if !kind
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(Error::invalid(format!(
                "the query parameter type {kind:?} is not a BigQuery scalar type name, which are \
                 upper-case identifiers such as STRING, INT64 and TIMESTAMP"
            )));
        }
        Ok(Self {
            name,
            kind,
            value: value.into(),
        })
    }

    pub fn string(name: impl Into<String>, value: impl Into<String>) -> Result<Self> {
        Self::new(name, "STRING", value)
    }

    pub fn bool(name: impl Into<String>, value: bool) -> Result<Self> {
        Self::new(name, "BOOL", value.to_string())
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A query to run, with its parameters.
#[derive(Clone, Debug, Default)]
pub struct QueryRequest {
    sql: String,
    parameters: Vec<QueryParameter>,
}

impl QueryRequest {
    /// GoogleSQL only. Legacy SQL is never enabled by this adapter: the two
    /// dialects differ in what a query *means*, not only in what parses, and a
    /// deployment discovering which one it got from the shape of its results is
    /// a bad afternoon.
    pub fn new(sql: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            parameters: Vec::new(),
        }
    }

    pub fn with_parameter(mut self, parameter: QueryParameter) -> Self {
        self.parameters.push(parameter);
        self
    }

    pub fn sql(&self) -> &str {
        &self.sql
    }

    pub fn parameters(&self) -> &[QueryParameter] {
        &self.parameters
    }
}

/// One row of a result set: column name to value, `None` for SQL `NULL`.
pub type QueryRow = BTreeMap<String, Option<String>>;

/// A complete result set.
///
/// "Complete" is load-bearing: this type is only ever produced for a job that
/// finished and whose pages were all read. A query that did not finish, or
/// whose results did not fit inside
/// [`BigQueryConfig::max_result_pages`], is an error rather than a short
/// [`QueryPage`], because a caller cannot tell a truncated result set from a
/// small one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryPage {
    /// Column names, in the order the schema declares them.
    pub columns: Vec<String>,
    /// The rows, in the order BigQuery returned them. That order is whatever
    /// the SQL's `ORDER BY` made it; BigQuery promises none without one, and
    /// neither does this.
    pub rows: Vec<QueryRow>,
    /// Rows the job produced, as BigQuery counted them. Compared against
    /// `rows.len()` by [`BigQueryWarehouse::query`] so a page count that does
    /// not add up is an error rather than a quiet shortfall.
    pub total_rows: Option<u64>,
    /// Bytes the query scanned, which is what it is billed on.
    pub total_bytes_processed: Option<u64>,
    /// Whether BigQuery served this from its query cache.
    pub cache_hit: bool,
}

/// The research warehouse.
///
/// # What it does not promise
///
/// **It does not know the table's schema.** Rows are offered as JSON and
/// BigQuery decides whether they fit. A mismatch comes back as a
/// [`RowError`], not as a compile error, and this adapter has no way to catch
/// one earlier.
///
/// **It does not create anything.** No dataset, no table, no schema migration.
/// A table that does not exist produces a 404 that names it. Creating tables
/// from application code is how two deployments end up with the same table name
/// and different columns.
///
/// **It does not coerce values.** BigQuery's REST API returns every value as a
/// string, including `INT64` and `FLOAT64`, and this decoder hands over exactly
/// those strings. Parsing `"1.7976931348623157E308"` into an `f64` here would
/// be inventing a rounding decision on the caller's behalf, in a crate whose
/// whole point is that money is exact.
///
/// **It reads scalar columns only.** A `RECORD` or `REPEATED` column is refused
/// by name rather than flattened into a string that would look like a value.
///
/// **It does not manage jobs.** There is no job listing, no cancellation, and
/// no way to reattach to a query this process started and lost. A query that
/// outlives [`BigQueryConfig::max_query_polls`] is reported incomplete *with
/// its job id*, so an operator can find it in the console.
///
/// **Streaming inserts are not transactional.** Rows become visible
/// individually and there is no rollback. Inserting the two sides of a ledger
/// entry through this adapter does not make them atomic.
#[derive(Debug)]
pub struct BigQueryWarehouse {
    config: BigQueryConfig,
    client: HttpClient,
}

impl BigQueryWarehouse {
    /// What a deployment must supply beyond a working configuration.
    pub const REQUIREMENTS: [&'static str; 4] = [
        "a TLS-terminating proxy at the configured endpoint: `qip_transport::http` has no TLS \
         stack and refuses `https` by name rather than downgrading it, so a bearer token sent \
         straight to `bigquery.googleapis.com` would cross the internet in clear text",
        "a bearer token from a `TokenSource` the deployment keeps fresh — this crate cannot mint \
         one, because that means RS256-signing a JWT and ADR 0009 forbids in-tree cryptography",
        "a service account with `roles/bigquery.dataEditor` on the dataset for inserts and \
         `roles/bigquery.jobUser` on the project for queries; a query is a billed job and the \
         project it is billed to is the one configured here",
        "the dataset and its tables created and their schemas managed outside this code, which \
         creates nothing: a table this adapter is pointed at must already exist with the columns \
         the caller's rows carry",
    ];

    /// Build the warehouse. Succeeds even when nothing is configured: a
    /// warehouse that cannot be reached still has to exist to report why.
    pub fn new(config: BigQueryConfig) -> Result<Self> {
        validate_resource_name("project", &config.project)?;
        validate_resource_name("dataset", &config.dataset)?;
        if config.max_rows_per_insert == 0 {
            return Err(Error::invalid(
                "max_rows_per_insert is zero, which would refuse every insert",
            ));
        }
        if config.page_size == 0 {
            return Err(Error::invalid(
                "page_size is zero: a result page that holds no rows never finishes a result set",
            ));
        }
        if config.max_result_pages == 0 {
            return Err(Error::invalid(
                "max_result_pages is zero, which would refuse every query",
            ));
        }
        let client = HttpClient::new(config.access.limits());
        Ok(Self { config, client })
    }

    pub fn config(&self) -> &BigQueryConfig {
        &self.config
    }

    /// Whether this warehouse can reach BigQuery at all.
    pub fn is_available(&self) -> bool {
        self.config.access.is_configured()
    }

    /// Configuration a deployment has not supplied, each named on its own.
    pub fn missing_configuration(&self) -> Vec<String> {
        self.config.access.missing_configuration()
    }

    /// What is missing now, followed by what is required even when nothing is.
    pub fn requirement(&self) -> String {
        let mut parts = self.missing_configuration();
        parts.extend(Self::REQUIREMENTS.iter().map(|r| (*r).to_string()));
        parts.join("; ")
    }

    /// The refusal every entry point returns when the warehouse is unreachable.
    ///
    /// Never a fallback. A research result set that was written to a local file
    /// because BigQuery was unconfigured would be invisible to every query that
    /// later looked for it.
    fn unavailable(&self) -> Error {
        Error::unavailable(format!(
            "the BigQuery warehouse {}.{} cannot be reached and will not write anywhere else: {}",
            self.config.project,
            self.config.dataset,
            self.requirement()
        ))
    }

    fn service(&self) -> String {
        format!("BigQuery {}.{}", self.config.project, self.config.dataset)
    }

    /// Stream rows into a table.
    ///
    /// Returns what BigQuery actually did. Read [`InsertOutcome`]: a partially
    /// rejected insert answers HTTP 200, so the outcome and not the status is
    /// what says whether the rows are there.
    ///
    /// An empty batch is refused rather than sent: BigQuery rejects a request
    /// with no rows, and a caller that reached here with nothing to insert has
    /// a bug worth surfacing where it happened.
    pub fn insert(&self, table: &str, rows: Vec<InsertRow>) -> Result<InsertOutcome> {
        if !self.is_available() {
            return Err(self.unavailable());
        }
        validate_resource_name("table", table)?;
        if rows.is_empty() {
            return Err(Error::invalid(
                "an insert with no rows: BigQuery refuses an empty batch, and a caller with \
                 nothing to insert should not have reached the network",
            ));
        }
        if rows.len() > self.config.max_rows_per_insert {
            return Err(Error::guard(format!(
                "{} rows in one insert and the batch limit is {}: split the batch. BigQuery's own \
                 ceiling is on payload size, and a request that exceeds it is rejected whole, \
                 losing every row in it",
                rows.len(),
                self.config.max_rows_per_insert
            )));
        }
        for (index, row) in rows.iter().enumerate() {
            if !row.json.is_object() {
                return Err(Error::invalid(format!(
                    "row {index} is not a JSON object: a BigQuery row is a map of column name to \
                     value, and an array or a scalar has no columns to match against the schema"
                )));
            }
            if let Some(id) = &row.insert_id
                && id.trim().is_empty()
            {
                return Err(Error::invalid(format!(
                    "row {index} has a blank insert id: an absent deduplication key is `None`, \
                     not an empty string, which BigQuery would treat as a key that every other \
                     blank-id row also has"
                )));
            }
        }

        let submitted = rows.len();
        let payload = InsertAllRequest {
            kind: "bigquery#tableDataInsertAllRequest",
            // Both false on purpose. `skipInvalidRows` would make BigQuery
            // silently drop a bad row and report success for the rest;
            // `ignoreUnknownValues` would make it silently drop a column whose
            // name this caller got wrong. Either turns a schema mistake into
            // missing data nobody is told about.
            skip_invalid_rows: false,
            ignore_unknown_values: false,
            rows: rows
                .into_iter()
                .map(|row| InsertAllRow {
                    insert_id: row.insert_id,
                    json: row.json,
                })
                .collect(),
        };
        let body = serde_json::to_vec(&payload)?;
        let path = format!(
            "/bigquery/v2/projects/{}/datasets/{}/tables/{}/insertAll",
            percent_encode(&self.config.project),
            percent_encode(&self.config.dataset),
            percent_encode(table)
        );
        let request = self
            .config
            .access
            .request(Method::Post, &path)?
            .with_header("content-type", "application/json; charset=utf-8")
            .with_body(body);
        let response = self.client.send(&request).map_err(Error::from)?;
        if !response.is_success() {
            return Err(status_refusal(
                &self.service(),
                response.status,
                &response.body_excerpt(),
            ));
        }
        let text = response.body_as_str().map_err(Error::from)?;
        let decoded: InsertAllResponse = serde_json::from_str(text).map_err(|error| {
            Error::schema(format!(
                "{} sent an insert response this decoder cannot read: {error}. The first bytes of \
                 it were: {}",
                self.service(),
                response.body_excerpt()
            ))
        })?;
        let rejected = decoded
            .insert_errors
            .into_iter()
            .map(|entry| RowError {
                index: entry.index,
                reasons: entry
                    .errors
                    .iter()
                    .map(|e| e.reason.clone().unwrap_or_else(|| "unspecified".into()))
                    .collect(),
                messages: entry
                    .errors
                    .iter()
                    .map(|e| e.message.clone().unwrap_or_else(|| "no message".into()))
                    .collect(),
            })
            .collect();
        Ok(InsertOutcome {
            submitted,
            rejected,
        })
    }

    /// Run a query and read its whole result set.
    ///
    /// # What "whole" means here
    ///
    /// Three ways a result set can be incomplete, and none of them returns
    /// rows:
    ///
    /// * the job has not finished — BigQuery answers `jobComplete: false` with
    ///   no `rows` field at all, which a naive decoder reads as zero rows. This
    ///   asks again, up to [`BigQueryConfig::max_query_polls`] times, each ask
    ///   a server-side wait of [`BigQueryConfig::query_timeout_ms`]; nothing
    ///   sleeps on this thread. Still unfinished after that is an error naming
    ///   the job id.
    /// * the results span more pages than
    ///   [`BigQueryConfig::max_result_pages`] — an error, not a prefix.
    /// * BigQuery's own `totalRows` disagrees with the number of rows actually
    ///   assembled — an error, because one of the two is wrong and a caller has
    ///   no way to tell which.
    pub fn query(&self, request: &QueryRequest) -> Result<QueryPage> {
        if !self.is_available() {
            return Err(self.unavailable());
        }
        if request.sql.trim().is_empty() {
            return Err(Error::invalid("an empty query"));
        }
        let mut seen: BTreeMap<String, ()> = BTreeMap::new();
        for parameter in &request.parameters {
            if seen.insert(parameter.name.clone(), ()).is_some() {
                return Err(Error::invalid(format!(
                    "two query parameters are both named {:?}: BigQuery would bind one of them \
                     and this adapter will not choose which",
                    parameter.name
                )));
            }
        }

        let payload = QueryPayload {
            query: request.sql.clone(),
            use_legacy_sql: false,
            max_results: self.config.page_size,
            timeout_ms: self.config.query_timeout_ms,
            parameter_mode: if request.parameters.is_empty() {
                None
            } else {
                Some("NAMED")
            },
            query_parameters: request
                .parameters
                .iter()
                .map(|p| WireParameter {
                    name: p.name.clone(),
                    parameter_type: WireParameterType {
                        kind: p.kind.clone(),
                    },
                    parameter_value: WireParameterValue {
                        value: p.value.clone(),
                    },
                })
                .collect(),
            default_dataset: DefaultDataset {
                project_id: self.config.project.clone(),
                dataset_id: self.config.dataset.clone(),
            },
        };
        let body = serde_json::to_vec(&payload)?;
        let path = format!(
            "/bigquery/v2/projects/{}/queries",
            percent_encode(&self.config.project)
        );
        let start = self
            .config
            .access
            .request(Method::Post, &path)?
            .with_header("content-type", "application/json; charset=utf-8")
            .with_body(body);
        let mut response = self.send_query(&start)?;

        // Wait for the job, server-side. Each `getQueryResults` with a
        // `timeoutMs` holds the connection open on BigQuery's side until the
        // job finishes or the timeout expires, so waiting costs a round trip
        // rather than a sleeping thread.
        let mut polls = 0;
        while !response.job_complete {
            let Some(job) = response.job_reference.clone() else {
                return Err(Error::schema(format!(
                    "{} reported a query as incomplete without a job reference, so there is \
                     nothing to ask about again",
                    self.service()
                )));
            };
            if polls >= self.config.max_query_polls {
                return Err(Error::timeout(format!(
                    "{} has not finished job {} after {} attempts of {} ms each. The result set \
                     is not returned empty: an unfinished query and a query that found nothing \
                     are different answers, and only one of them is safe to act on",
                    self.service(),
                    job.job_id,
                    self.config.max_query_polls,
                    self.config.query_timeout_ms
                )));
            }
            polls += 1;
            response = self.fetch_results(&job, None)?;
        }

        let columns: Vec<String> = response
            .schema
            .as_ref()
            .map(|schema| schema.fields.iter().map(|f| f.name.clone()).collect())
            .unwrap_or_default();
        let total_rows = response.total_rows.as_deref().and_then(|s| s.parse().ok());
        let total_bytes_processed = response
            .total_bytes_processed
            .as_deref()
            .and_then(|s| s.parse().ok());
        let cache_hit = response.cache_hit.unwrap_or(false);
        let job = response.job_reference.clone();

        let mut rows = Vec::new();
        let mut pages = 0;
        loop {
            pages += 1;
            for row in &response.rows {
                rows.push(decode_row(&columns, row, &self.service())?);
            }
            let next = match response.page_token.as_deref() {
                Some(token) if !token.is_empty() => token.to_string(),
                _ => break,
            };
            if pages >= self.config.max_result_pages {
                return Err(Error::guard(format!(
                    "{} returned more than {} pages of {} rows for this query. The rows already \
                     read are not returned: a caller cannot tell a truncated result set from a \
                     small one, and would act on it as though it were the whole answer",
                    self.service(),
                    self.config.max_result_pages,
                    self.config.page_size
                )));
            }
            let Some(job) = job.clone() else {
                return Err(Error::schema(format!(
                    "{} offered another page of results without a job reference to fetch it with",
                    self.service()
                )));
            };
            response = self.fetch_results(&job, Some(&next))?;
        }

        if let Some(expected) = total_rows
            && expected != rows.len() as u64
        {
            return Err(Error::schema(format!(
                "{} said the query produced {expected} rows and this adapter assembled {}. \
                 Neither number is returned as the answer: one of them is wrong and there is no \
                 way here to tell which",
                self.service(),
                rows.len()
            )));
        }

        Ok(QueryPage {
            columns,
            rows,
            total_rows,
            total_bytes_processed,
            cache_hit,
        })
    }

    /// Ask again for a job's results, optionally at a page token.
    fn fetch_results(&self, job: &JobReference, page_token: Option<&str>) -> Result<QueryResponse> {
        let mut path = format!(
            "/bigquery/v2/projects/{}/queries/{}?maxResults={}&timeoutMs={}",
            percent_encode(&self.config.project),
            percent_encode(&job.job_id),
            self.config.page_size,
            self.config.query_timeout_ms
        );
        // The job's location must travel with the request or BigQuery cannot
        // find a job that ran outside the default region.
        if let Some(location) = &job.location {
            path.push_str("&location=");
            path.push_str(&percent_encode(location));
        }
        if let Some(token) = page_token {
            path.push_str("&pageToken=");
            path.push_str(&percent_encode(token));
        }
        let request = self.config.access.request(Method::Get, &path)?;
        self.send_query(&request)
    }

    /// Send a query request and decode the envelope, without interpreting it.
    fn send_query(&self, request: &qip_transport::HttpRequest) -> Result<QueryResponse> {
        let response = self.client.send(request).map_err(Error::from)?;
        if !response.is_success() {
            return Err(status_refusal(
                &self.service(),
                response.status,
                &response.body_excerpt(),
            ));
        }
        let text = response.body_as_str().map_err(Error::from)?;
        serde_json::from_str(text).map_err(|error| {
            Error::schema(format!(
                "{} sent a query response this decoder cannot read: {error}. The first bytes of \
                 it were: {}",
                self.service(),
                response.body_excerpt()
            ))
        })
    }
}

/// Turn BigQuery's positional row into a map keyed by column name.
///
/// A row whose cell count does not match the schema is refused rather than
/// zipped to the shorter of the two: a short zip would silently shift every
/// subsequent column's value into the wrong name, which produces a result set
/// that is wrong rather than obviously broken.
fn decode_row(columns: &[String], row: &WireRow, service: &str) -> Result<QueryRow> {
    if row.f.len() != columns.len() {
        return Err(Error::schema(format!(
            "{service} returned a row with {} cells for a schema of {} columns; a row that does \
             not match its own schema is refused rather than aligned to the shorter of the two",
            row.f.len(),
            columns.len()
        )));
    }
    let mut out = QueryRow::new();
    for (name, cell) in columns.iter().zip(row.f.iter()) {
        let value = match &cell.v {
            serde_json::Value::Null => None,
            serde_json::Value::String(text) => Some(text.clone()),
            // BigQuery's REST encoding sends every scalar as a string. An array
            // is a REPEATED column and an object is a RECORD, and both are
            // refused: rendering either as a string would hand back something
            // that looks like a value and is not.
            serde_json::Value::Array(_) => {
                return Err(Error::schema(format!(
                    "{service} returned a REPEATED value for column {name:?}; this decoder reads \
                     scalar columns only, and will not flatten a repeated field into a string \
                     that would read as a value"
                )));
            }
            serde_json::Value::Object(_) => {
                return Err(Error::schema(format!(
                    "{service} returned a RECORD value for column {name:?}; this decoder reads \
                     scalar columns only. Select the record's fields individually"
                )));
            }
            other => {
                return Err(Error::schema(format!(
                    "{service} returned {other} for column {name:?}, which is not how BigQuery's \
                     REST API encodes a scalar: every scalar arrives as a JSON string"
                )));
            }
        };
        out.insert(name.clone(), value);
    }
    Ok(out)
}

/// Refuse a project, dataset or table name that cannot go in a request line.
///
/// Validated rather than trusted because all three are interpolated into the
/// URL path. Percent-encoding follows, so this is defence in depth: what it
/// really catches is a name that is empty or a path fragment somebody meant to
/// be a path.
fn validate_resource_name(what: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::invalid(format!(
            "a BigQuery {what} name is required and there is no default: a default that named a \
             real {what} would be written to successfully"
        )));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return Err(Error::invalid(format!(
            "the BigQuery {what} name {value:?} contains a character this adapter will not put \
             in a request line: ASCII letters, digits, and - _ . : only"
        )));
    }
    Ok(())
}

// --- the wire schema --------------------------------------------------------
//
// What is sent and what is accepted back. Unknown fields in a response are
// ignored, because Google adding one is not a fault and must not stop an
// insert; unknown *values* in a field this decoder reads are refused, because
// those change what the answer means.

#[derive(Debug, Serialize)]
struct InsertAllRequest {
    kind: &'static str,
    #[serde(rename = "skipInvalidRows")]
    skip_invalid_rows: bool,
    #[serde(rename = "ignoreUnknownValues")]
    ignore_unknown_values: bool,
    rows: Vec<InsertAllRow>,
}

#[derive(Debug, Serialize)]
struct InsertAllRow {
    #[serde(rename = "insertId", skip_serializing_if = "Option::is_none")]
    insert_id: Option<String>,
    json: serde_json::Value,
}

#[derive(Debug, Default, Deserialize)]
struct InsertAllResponse {
    #[serde(default, rename = "insertErrors")]
    insert_errors: Vec<InsertErrorEntry>,
}

#[derive(Debug, Deserialize)]
struct InsertErrorEntry {
    index: usize,
    #[serde(default)]
    errors: Vec<InsertErrorDetail>,
}

#[derive(Debug, Deserialize)]
struct InsertErrorDetail {
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct QueryPayload {
    query: String,
    #[serde(rename = "useLegacySql")]
    use_legacy_sql: bool,
    #[serde(rename = "maxResults")]
    max_results: u32,
    #[serde(rename = "timeoutMs")]
    timeout_ms: u64,
    #[serde(rename = "parameterMode", skip_serializing_if = "Option::is_none")]
    parameter_mode: Option<&'static str>,
    #[serde(rename = "queryParameters", skip_serializing_if = "Vec::is_empty")]
    query_parameters: Vec<WireParameter>,
    #[serde(rename = "defaultDataset")]
    default_dataset: DefaultDataset,
}

#[derive(Debug, Serialize)]
struct DefaultDataset {
    #[serde(rename = "projectId")]
    project_id: String,
    #[serde(rename = "datasetId")]
    dataset_id: String,
}

#[derive(Debug, Serialize)]
struct WireParameter {
    name: String,
    #[serde(rename = "parameterType")]
    parameter_type: WireParameterType,
    #[serde(rename = "parameterValue")]
    parameter_value: WireParameterValue,
}

#[derive(Debug, Serialize)]
struct WireParameterType {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Serialize)]
struct WireParameterValue {
    value: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct QueryResponse {
    /// False means the job is still running and `rows` is absent. The single
    /// most important field in this response.
    #[serde(default, rename = "jobComplete")]
    job_complete: bool,
    #[serde(default, rename = "jobReference")]
    job_reference: Option<JobReference>,
    #[serde(default)]
    schema: Option<WireSchema>,
    #[serde(default)]
    rows: Vec<WireRow>,
    #[serde(default, rename = "pageToken")]
    page_token: Option<String>,
    /// A count, sent as a string, because JSON numbers cannot hold an `INT64`.
    #[serde(default, rename = "totalRows")]
    total_rows: Option<String>,
    #[serde(default, rename = "totalBytesProcessed")]
    total_bytes_processed: Option<String>,
    #[serde(default, rename = "cacheHit")]
    cache_hit: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
struct JobReference {
    #[serde(rename = "jobId")]
    job_id: String,
    #[serde(default)]
    location: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct WireSchema {
    #[serde(default)]
    fields: Vec<WireField>,
}

#[derive(Clone, Debug, Deserialize)]
struct WireField {
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
struct WireRow {
    #[serde(default)]
    f: Vec<WireCell>,
}

#[derive(Clone, Debug, Deserialize)]
struct WireCell {
    #[serde(default)]
    v: serde_json::Value,
}
