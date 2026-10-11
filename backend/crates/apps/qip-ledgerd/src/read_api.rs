//! The read side: balances, postings, parked cells, health and metrics,
//! served from the ledger's own store (ADR 0100 §1, §2).
//!
//! `serve()` used to return an error unconditionally, and `main` returned
//! that error, so the binary exited at start every time. It is now a
//! [`Handler`] for `qip_transport::server::Server`, bound by
//! [`crate::start`] before anything says the process is ready.
//!
//! Routes, all `GET`, all read-only — there is no route that writes, and the
//! store has no method a route could call to change a posting:
//!
//! - `/health` — `200` once storage is proven writable and the listener is
//!   bound, `503` before; the body carries every partition worker's status,
//!   so a partition stalled on a record it will not skip is visible.
//! - `/metrics` — the registry's Prometheus exposition.
//! - `/api/v1/ledger/balance?account=…&unit=…` — one balance as decimal text.
//! - `/api/v1/ledger/postings?account=…&from=…&to=…` — the account's
//!   postings whose delivery offset is in `[from, to)`.
//! - `/api/v1/ledger/parked` — every parked cell and why.
//! - `/api/v1/ledger/tax-lots` — every tax lot opened, with its remaining
//!   quantity and basis (folded from postings) and its acquisition time.
//! - `/api/v1/ledger/trial-balance` — the sum of every balance per unit;
//!   zero in every unit, or the ledger has a defect to name.
//!
//! The account is a query parameter, not a path segment, because a trading
//! account is written `trading:cell/strategy` and a `/` in a path segment is
//! a second segment.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::consumer::Statuses;
use crate::store::LedgerStore;
use qip_contracts::ledger::{Account, Direction};
use qip_core::error::{Error, Result};
use qip_observability::metrics::Metrics;
use qip_transport::server::{Handler, Method, Request, Response};

/// Whether the ledger may say it is ready.
#[derive(Debug, Default)]
pub struct Readiness {
    storage_proven: AtomicBool,
    serving: AtomicBool,
}

impl Readiness {
    pub fn storage_proven(&self) {
        self.storage_proven.store(true, Ordering::SeqCst);
    }

    pub fn set_serving(&self, serving: bool) {
        self.serving.store(serving, Ordering::SeqCst);
    }

    /// Both: the store took a write and gave it back, and the listener is
    /// bound. Never before either.
    pub fn is_ready(&self) -> bool {
        self.storage_proven.load(Ordering::SeqCst) && self.serving.load(Ordering::SeqCst)
    }
}

/// The read-side HTTP handler for the ledger.
#[derive(Debug)]
pub struct ReadApi {
    store: Arc<LedgerStore>,
    readiness: Arc<Readiness>,
    metrics: Arc<Metrics>,
    statuses: Statuses,
}

fn error_response(error: &Error) -> Response {
    let status = match error.code() {
        "invalid" => 400,
        "not_found" => 404,
        _ => 500,
    };
    Response::json(
        status,
        serde_json::json!({ "error": error.to_string() }).to_string(),
    )
}

fn parse_offset(request: &Request, name: &str, default: u64) -> Result<u64> {
    match request.query_param(name) {
        None => Ok(default),
        Some(text) => text.parse::<u64>().map_err(|_| {
            Error::invalid(format!(
                "{name} must be a whole delivery offset, not '{text}'"
            ))
        }),
    }
}

impl ReadApi {
    pub fn new(
        store: Arc<LedgerStore>,
        readiness: Arc<Readiness>,
        metrics: Arc<Metrics>,
        statuses: Statuses,
    ) -> Self {
        Self {
            store,
            readiness,
            metrics,
            statuses,
        }
    }

    /// One balance, as decimal text.
    pub fn handle_balance(&self, account_str: &str, unit: &str) -> Result<String> {
        let account = parse_account(account_str)?;
        if unit.trim().is_empty() {
            return Err(Error::invalid("a balance needs a unit, such as USD"));
        }
        let balance = self.store.balance(&account, unit)?;
        Ok(format!("{balance}"))
    }

    /// The account's postings whose delivery offset is in `[from, to)`, one
    /// per line as `<offset> <debit|credit> <amount> <unit>`.
    pub fn handle_postings(&self, account_str: &str, from: u64, to: u64) -> Result<String> {
        let account = parse_account(account_str)?;
        if from > to {
            return Err(Error::invalid(format!(
                "from ({from}) is after to ({to}); the range is [from, to)"
            )));
        }
        let mut postings = Vec::new();
        for record in self.store.chain_records()? {
            if record.offset < from || record.offset >= to {
                continue;
            }
            if let Some(event) = &record.event {
                for posting in event.postings() {
                    if posting.account == account {
                        let side = match posting.direction {
                            Direction::Debit => "debit",
                            Direction::Credit => "credit",
                        };
                        postings.push(format!(
                            "{} {side} {} {}",
                            record.offset, posting.amount, posting.unit
                        ));
                    }
                }
            }
        }
        if postings.is_empty() {
            return Ok("no postings found".to_string());
        }
        Ok(postings.join("\n"))
    }

    fn health(&self) -> Response {
        let ready = self.readiness.is_ready();
        let partitions = self
            .statuses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let parked = self.store.parked().map(|p| p.len());
        let body = serde_json::json!({
            "service": "qip-ledgerd",
            "status": if ready { "ready" } else { "not_ready" },
            "store": self.store.backend_kind(),
            "storage_proven": self.readiness.storage_proven.load(Ordering::SeqCst),
            "serving": self.readiness.serving.load(Ordering::SeqCst),
            "parked_cells": parked.as_ref().ok(),
            "parked_error": parked.as_ref().err().map(ToString::to_string),
            "partitions": partitions,
        });
        Response::json(if ready { 200 } else { 503 }, body.to_string())
    }

    fn route(&self, request: &Request) -> Result<Response> {
        match request.path.as_str() {
            "/health" | "/" => Ok(self.health()),
            "/metrics" => Ok(Response::new(
                200,
                "text/plain; version=0.0.4",
                self.metrics.snapshot().to_prometheus().into_bytes(),
            )),
            "/api/v1/ledger/balance" => {
                let account = request
                    .query_param("account")
                    .ok_or_else(|| Error::invalid("balance needs ?account="))?;
                let unit = request
                    .query_param("unit")
                    .ok_or_else(|| Error::invalid("balance needs ?unit="))?;
                Ok(Response::text(200, self.handle_balance(account, unit)?))
            }
            "/api/v1/ledger/postings" => {
                let account = request
                    .query_param("account")
                    .ok_or_else(|| Error::invalid("postings needs ?account="))?;
                let from = parse_offset(request, "from", 0)?;
                let to = parse_offset(request, "to", u64::MAX)?;
                Ok(Response::text(
                    200,
                    self.handle_postings(account, from, to)?,
                ))
            }
            "/api/v1/ledger/parked" => {
                let parked = self.store.parked()?;
                Ok(Response::json(200, serde_json::to_string(&parked)?))
            }
            "/api/v1/ledger/tax-lots" => {
                let lots = self.store.tax_lots()?;
                Ok(Response::json(200, serde_json::to_string(&lots)?))
            }
            "/api/v1/ledger/trial-balance" => {
                let totals: BTreeMap<String, String> = self
                    .store
                    .trial_balance()?
                    .into_iter()
                    .map(|(unit, total)| (unit, total.to_string()))
                    .collect();
                Ok(Response::json(200, serde_json::to_string(&totals)?))
            }
            other => Err(Error::not_found(format!(
                "no ledger route at {other}; see /health and /api/v1/ledger/*"
            ))),
        }
    }
}

impl Handler for ReadApi {
    fn handle(&self, request: &Request) -> Response {
        if !matches!(request.method, Method::Get | Method::Head) {
            return Response::json(
                405,
                r#"{"error":"the ledger read API is read-only; every route is a GET"}"#,
            );
        }
        self.route(request)
            .unwrap_or_else(|error| error_response(&error))
    }
}

/// Parse an account string into an Account enum: the contract's own
/// parser, the exact inverse of how an account renders, for every subledger.
/// This used to be a hand parser that knew three of the four kinds and
/// accepted `venue:` with an empty name.
fn parse_account(account_str: &str) -> Result<Account> {
    account_str.parse::<Account>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::LedgerTelemetry;
    use qip_core::{Clock, ManualClock, Timestamp};
    use qip_storage::EngineConfig;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn api() -> ReadApi {
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp_dir = std::env::temp_dir().join(format!(
            "qip-ledger-api-test-{}-{}",
            std::process::id(),
            unique
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("create test dir");

        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_secs(1_790_000_000)));
        let config = EngineConfig::new(clock);
        let metrics = Arc::new(Metrics::new("qip-ledgerd"));
        let store = Arc::new(
            LedgerStore::open(&temp_dir, config, LedgerTelemetry::new(metrics.clone()))
                .expect("open store"),
        );
        ReadApi::new(
            store,
            Arc::new(Readiness::default()),
            metrics,
            Arc::new(Mutex::new(BTreeMap::new())),
        )
    }

    fn get(path: &str, query: &[(&str, &str)]) -> Request {
        Request {
            method: Method::Get,
            path: path.to_string(),
            query: query
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            headers: BTreeMap::new(),
            body: Vec::new(),
            peer: "127.0.0.1:1".to_string(),
        }
    }

    #[test]
    fn an_empty_ledger_returns_zero_balance_for_any_venue_and_currency() {
        let api = api();
        // Premise: the store is empty, so a query for any account reads zero.
        let balance = api
            .handle_balance("venue:sim-xnys", "USD")
            .expect("query succeeds");
        assert_eq!(balance, "0");
    }

    #[test]
    fn an_empty_ledger_returns_a_readable_no_postings_message() {
        let api = api();
        // Premise: the store is empty, so a query for postings reads the message
        // that tells an operator nothing has been recorded, not an empty list.
        let postings = api
            .handle_postings("venue:sim-xnys", 0, u64::MAX)
            .expect("query succeeds");
        assert_eq!(postings, "no postings found");
    }

    #[test]
    fn the_account_parser_accepts_all_three_formats_and_rejects_malformed_ones() {
        // Premise: each format can be parsed successfully, so the assertion below
        // is about the parsing and not about the fixture.
        let trading = parse_account("trading:cell-a/strategy-b").expect("parses trading");
        assert!(matches!(
            trading,
            Account::Trading {
                cell,
                strategy
            } if cell == "cell-a" && strategy == "strategy-b"
        ));

        let venue = parse_account("venue:sim-xnys").expect("parses venue");
        assert!(matches!(
            venue,
            Account::Venue { venue: v } if v == "sim-xnys"
        ));

        let fees = parse_account("fees:sim-xnys").expect("parses fees");
        assert!(matches!(
            fees,
            Account::Fees { venue: v } if v == "sim-xnys"
        ));

        // The other half: a malformed format is rejected.
        assert!(parse_account("invalid:format").is_err());
        assert!(parse_account("trading:cell-only").is_err());
    }

    /// Readiness is earned, never assumed: `/health` answers 503 until the
    /// store has been proven writable *and* the listener is serving, and a
    /// write method is refused on every path. Mutation: make `is_ready`
    /// return `self.serving` alone — the second assertion sees 200 and fails.
    #[test]
    fn health_is_not_ready_until_storage_is_proven_and_writes_are_refused_everywhere() {
        let api = api();
        assert_eq!(api.handle(&get("/health", &[])).status, 503);
        api.readiness.set_serving(true);
        assert_eq!(
            api.handle(&get("/health", &[])).status,
            503,
            "serving alone is not ready"
        );
        api.readiness.storage_proven();
        assert_eq!(api.handle(&get("/health", &[])).status, 200);

        let balance = api.handle(&get(
            "/api/v1/ledger/balance",
            &[("account", "venue:sim-xnys"), ("unit", "USD")],
        ));
        assert_eq!(balance.status, 200);
        assert_eq!(balance.body, b"0");
        assert_eq!(
            api.handle(&get("/api/v1/ledger/balance", &[("unit", "USD")]))
                .status,
            400
        );

        let mut post = get("/api/v1/ledger/balance", &[]);
        post.method = Method::Post;
        assert_eq!(api.handle(&post).status, 405);
    }
}
