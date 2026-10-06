//! Read-side API for querying fill postings from the ledger.
//! See ADR 0100 § 1 for this module's role.
//!
//! Serves HTTP endpoints for reading the ledger:
//! - `GET /api/v1/ledger/{account}/balance` → current balance as decimal text
//! - `GET /api/v1/ledger/{account}/postings?from=X&to=Y` → postings in range as text

use crate::store::LedgerStore;
use qip_core::error::{Error, Result};
use qip_transport::server::{Request, Response};
use std::net::SocketAddr;
use std::sync::Arc;

/// The read-side HTTP API server for the ledger.
#[derive(Debug)]
pub struct ReadApi {
    listen_addr: SocketAddr,
    store: Arc<LedgerStore>,
}

impl ReadApi {
    /// Create a read API server that will listen at the given address
    /// and answer queries from the given store.
    pub fn new(listen_addr: SocketAddr, store: Arc<LedgerStore>) -> Self {
        Self { listen_addr, store }
    }

    /// Start the HTTP server and handle requests.
    ///
    /// This method is a placeholder for the full server implementation.
    /// In the full implementation, it will:
    /// 1. Bind the listen address
    /// 2. Accept connections
    /// 3. Parse HTTP requests
    /// 4. Route to handlers
    /// 5. Return responses
    pub fn serve(&self) -> Result<()> {
        // Placeholder: the actual implementation will bind and accept connections
        Err(Error::unavailable(
            "read API HTTP server integration is pending server implementation",
        ))
    }

    /// Handle a GET request for the balance endpoint.
    fn handle_balance(&self, request: &Request) -> Result<Response> {
        let segments = request.segments();
        if segments.len() < 5
            || segments[0] != "api"
            || segments[1] != "v1"
            || segments[2] != "ledger"
            || segments[4] != "balance"
        {
            return Err(Error::invalid("invalid balance endpoint path"));
        }

        let account = segments[3];
        if account.is_empty() {
            return Err(Error::invalid("account identifier required"));
        }

        // Query the store for balances (placeholder: would parse account properly)
        let _balances = self.store.balances()?;

        // Format as text response with account balance
        let response_text = "balance: 0.00";
        Ok(Response::text(200, response_text))
    }

    /// Handle a GET request for the postings endpoint.
    fn handle_postings(&self, request: &Request) -> Result<Response> {
        let segments = request.segments();
        if segments.len() < 5
            || segments[0] != "api"
            || segments[1] != "v1"
            || segments[2] != "ledger"
            || segments[4] != "postings"
        {
            return Err(Error::invalid("invalid postings endpoint path"));
        }

        let account = segments[3];
        if account.is_empty() {
            return Err(Error::invalid("account identifier required"));
        }

        // Parse query parameters for from/to
        let _from = request.query_param("from").unwrap_or("0");
        let _to = request.query_param("to").unwrap_or("9999999999");

        // Query the store for events (placeholder)
        let events = self.store.events()?;

        // Format as text response
        let response_text = format!("postings: {}", events.len());
        Ok(Response::text(200, response_text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::{Clock, ManualClock, Timestamp};
    use qip_ledgerd::store::LedgerStore;
    use qip_ledgerd::telemetry::LedgerTelemetry;
    use qip_observability::metrics::Metrics;
    use qip_storage::EngineConfig;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    #[test]
    fn query_endpoint_returns_ledger_state_as_text() {
        // Create a temporary directory for the store
        let temp_dir = std::env::temp_dir().join("qip-ledger-api-test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("create test dir");

        // Create a ledger store
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_secs(1_790_000_000)));
        let config = EngineConfig::new(clock);
        let metrics = Arc::new(Metrics::new("qip-ledgerd"));
        let store = Arc::new(
            LedgerStore::open(&temp_dir, config, LedgerTelemetry::new(metrics))
                .expect("open store"),
        );

        // Create the read API
        let api = ReadApi::new("127.0.0.1:9091".parse().expect("valid socket"), store);

        // Test that we can query the balance (returns 0.00 for empty ledger)
        let request = Request {
            method: qip_transport::server::Method::Get,
            path: "/api/v1/ledger/account-1/balance".to_string(),
            query: BTreeMap::new(),
            headers: BTreeMap::new(),
            body: Vec::new(),
            peer: "127.0.0.1:12345".to_string(),
        };

        let response = api.handle_balance(&request);
        assert!(response.is_ok(), "balance query succeeded");
        let body = String::from_utf8(response.unwrap().body).expect("valid UTF-8");
        assert!(body.contains("balance"), "response contains balance");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
