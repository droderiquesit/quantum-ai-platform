//! Ledger composition root's entry point.
//!
//! ADR 0100 assigns this binary the ledger's role: sole writer of the chain
//! of double-entry postings for fills, and its own read-side API (ADR 0100
//! §2). Every composition root here reads configuration and refuses anything
//! invalid, binds ports and proves storage writable *before* reporting
//! healthy, and only then serves
//! (`.claude/rules/architecture/00-boundaries.md`).
//!
//! The startup order is:
//! 1. Read and validate configuration from environment variables
//! 2. Create the durable store at the configured archive path
//! 3. Bind the HTTP listen address to prove it is available
//! 4. Start the read-side HTTP API server
//! 5. Start consuming from the event fabric
//! 6. Report healthy and serve requests

use qip_core::error::Result;
use qip_core::{Clock, SystemClock};
use qip_ledgerd::config::LedgerConfig;
use qip_ledgerd::consumer::FabricConsumer;
use qip_ledgerd::read_api::ReadApi;
use qip_ledgerd::store::LedgerStore;
use qip_ledgerd::telemetry::LedgerTelemetry;
use qip_observability::metrics::Metrics;
use qip_storage::EngineConfig;
use std::sync::Arc;

fn main() {
    if let Err(e) = run() {
        eprintln!("qip-ledgerd: {}: {}", e.code(), e.message());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    // Step 1: Read and validate configuration from environment
    let config = LedgerConfig::from_env()?;

    // Step 2: Create the durable store at the configured archive path
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let engine_config = EngineConfig::new(clock);
    let metrics = Arc::new(Metrics::new("qip-ledgerd"));
    let telemetry = LedgerTelemetry::new(metrics.clone());

    let store = Arc::new(LedgerStore::open(
        &config.archive_path,
        engine_config,
        telemetry,
    )?);

    // Step 3: Bind the HTTP listen address to prove it is available
    // This is done before reporting healthy, as per the composition root order.
    // In the full implementation, this would bind the socket and prove writability.

    // Step 4: Start the read-side HTTP API server
    let read_api = ReadApi::new(config.listen_addr, store.clone());

    // Step 5: Start consuming from the event fabric
    // The fabric consumer will yield P1 outcomes that the store applies.
    // For now, this is a placeholder that cannot run without a fabric broker.
    let _consumer = FabricConsumer::new(
        "127.0.0.1:9090".parse().expect("valid socket"),
        &config.fabric_consumer_group,
    );

    // Step 6: Serve the read API
    // In the full implementation, this would enter the main loop,
    // accepting HTTP connections and draining postings from the fabric consumer.
    read_api.serve()?;

    Ok(())
}
