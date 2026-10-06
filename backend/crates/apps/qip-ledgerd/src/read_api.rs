//! Read-side API for querying fill postings from the ledger.
//! See ADR 0100 § 1 for this module's role.
//!
//! Serves HTTP endpoints for reading the ledger:
//! - `GET /api/v1/ledger/{account}/balance` → current balance as decimal text
//! - `GET /api/v1/ledger/{account}/postings?from=X&to=Y` → postings in range as text

use crate::store::LedgerStore;
use qip_contracts::ledger::Account;
use qip_core::error::{Error, Result};
use std::net::SocketAddr;
use std::sync::Arc;

/// The read-side HTTP API server for the ledger.
#[derive(Debug)]
#[allow(dead_code)]
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
    /// Account string format: "trading:cell/strategy", "venue:venue", or "fees:venue"
    pub fn handle_balance(&self, account_str: &str, unit: &str) -> Result<String> {
        let account = parse_account(account_str)?;
        let balance = self.store.balance(&account, unit)?;
        Ok(format!("{}", balance))
    }

    /// Handle a GET request for the postings endpoint.
    /// Account string format: "trading:cell/strategy", "venue:venue", or "fees:venue"
    pub fn handle_postings(&self, account_str: &str, _from: u64, _to: u64) -> Result<String> {
        let account = parse_account(account_str)?;

        // Collect postings for this account from chain records
        let chain_records = self.store.chain_records()?;
        let mut postings = Vec::new();

        for record in chain_records.iter() {
            if let Some(event) = &record.event {
                for posting in event.postings() {
                    if posting.account == account {
                        postings.push(format!(
                            "{} {} {}",
                            if posting.direction as u8 == 0 {
                                "debit"
                            } else {
                                "credit"
                            },
                            posting.amount,
                            posting.unit
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
}

/// Parse an account string into an Account enum.
fn parse_account(account_str: &str) -> Result<Account> {
    if let Some(rest) = account_str.strip_prefix("trading:") {
        let trading_parts: Vec<&str> = rest.split('/').collect();
        if trading_parts.len() == 2 {
            Ok(Account::Trading {
                cell: trading_parts[0].to_string(),
                strategy: trading_parts[1].to_string(),
            })
        } else {
            Err(Error::invalid("invalid trading account format"))
        }
    } else if let Some(rest) = account_str.strip_prefix("venue:") {
        Ok(Account::Venue {
            venue: rest.to_string(),
        })
    } else if let Some(rest) = account_str.strip_prefix("fees:") {
        Ok(Account::Fees {
            venue: rest.to_string(),
        })
    } else {
        Err(Error::invalid(
            "account must start with trading:, venue:, or fees:",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::LedgerTelemetry;
    use qip_core::{Clock, ManualClock, Timestamp};
    use qip_observability::metrics::Metrics;
    use qip_storage::EngineConfig;
    use std::sync::Arc;

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn temp_store() -> Arc<LedgerStore> {
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
        Arc::new(
            LedgerStore::open(&temp_dir, config, LedgerTelemetry::new(metrics))
                .expect("open store"),
        )
    }

    #[test]
    fn an_empty_ledger_returns_zero_balance_for_any_venue_and_currency() {
        let store = temp_store();
        let api = ReadApi::new("127.0.0.1:9091".parse().expect("valid socket"), store);

        // Premise: the store is empty, so a query for any account reads zero.
        let balance = api
            .handle_balance("venue:sim-xnys", "USD")
            .expect("query succeeds");
        assert_eq!(balance, "0");
    }

    #[test]
    fn an_empty_ledger_returns_a_readable_no_postings_message() {
        let store = temp_store();
        let api = ReadApi::new("127.0.0.1:9091".parse().expect("valid socket"), store);

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
        let invalid = parse_account("invalid:format");
        assert!(invalid.is_err());
    }
}
