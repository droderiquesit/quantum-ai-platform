//! The live end-to-end demonstration: one run, every socket.
//!
//! `e2e.rs` walks all seven layers in one pass and every observation in it is
//! invented in this process. That test is the claim that the layers are one
//! system. This one is the narrower and, until now, unmade claim underneath it:
//! that the *live* half of the platform composes — that a number can arrive
//! over a socket, cross every layer with its point-in-time discipline intact,
//! and leave again over a socket as an order a venue really received.
//!
//! Each adapter here already has a suite of its own against a loopback
//! listener. None of them had ever run together, and the platform had never
//! completed a cycle whose observations arrived over a socket and whose orders
//! left over one. Everything asserted below is a property of the *composition*;
//! nothing here re-tests a decoder.
//!
//! The run is:
//!
//! 1. **A vendor, over HTTP.** [`RestMarketDataAdapter`] fetches a hundred and
//!    twenty daily bars from a real listener. One hundred and nineteen are
//!    knowable at the walk's first instant and the hundred and twentieth — the
//!    one carrying an 8.5% jump — is not, because its bucket has not closed.
//!    It is withheld and counted.
//! 2. **The withheld bar does not reach a decision.** The nineteen are fed to
//!    the platform, a cycle runs, and DISCOVER finds no return anomaly. The
//!    clock then moves past the bucket close, the *same vendor response* is
//!    polled again, the jump arrives, and DISCOVER finds it. Same server, same
//!    body, same platform: only the clock moved.
//! 3. **Depth, over HTTP.** [`DepthFeedAdapter`] builds a book from a snapshot
//!    and its increments, and the cell tracks that book as the one it would
//!    route against. A second poll arrives with a hole behind it and is
//!    withheld, so the book with the gap behind it never reaches the cell.
//! 4. **A chain node, over JSON-RPC.** [`JsonRpcChainAdapter`] reports blocks at
//!    or beyond its confirmation depth and nothing shallower; the platform's
//!    chain is the node's chain minus the part that can still be reorganised
//!    away. The adapter never asks for the two unfinalised heights at all.
//! 5. **The mesh, both directions, over HTTP.** The central plane's
//!    [`CapitalDispatcher`] sends a signed capital envelope down a socket; the
//!    cell's [`CapitalDownlink`] polls it, verifies the signature against its
//!    own key, and deploys a strategy under the grant that crossed the wire.
//!    The cell then publishes its state delta *up* a second socket into the
//!    centre's [`CellDeltaReceiver`], and the centre absorbs it.
//! 6. **A venue, over HTTP.** An order naming the opportunity the live bars
//!    produced goes through [`OrderManager`] — the platform's own control path,
//!    with its pre-trade checker, its autonomy controller and its risk state —
//!    to [`RestOrderEntryAdapter`], which puts it on a socket. The venue
//!    receives it, answers with a partial fill, and the fill lands on the order
//!    in the order manager.
//! 7. **Learning, and nothing became live.**
//!
//! # What this test does not prove
//!
//! * **Loopback is not the internet.** Every peer here answers immediately and
//!   correctly. What each adapter does with a peer that stalls, truncates,
//!   overruns or lies is the subject of that adapter's own suite and is not
//!   re-argued here.
//! * **No TLS, and therefore no vendor.** `qip_transport::http` has no TLS
//!   stack and refuses `https` by name. Every adapter's own
//!   `production_requirement` says so, and every one of them says so in this
//!   run too.
//! * **Paper throughout.** `AdapterClass` has no `Live` variant, so
//!   `Broker::is_simulated` answers `true` for the REST venue adapter as well.
//!   That is a claim about the endpoint a deployment supplied and not a
//!   guarantee about the money; see `qip_brokers::rest`'s own documentation.
//!
//! # The composition gaps this walk found
//!
//! Recorded here as well as in `docs/architecture/current-state-audit.md`,
//! because a test that quietly works around a missing seam is worse than no
//! test at all.
//!
//! * **The platform's world model is never written.** [`Platform::new`] builds
//!   a `qip_world_model::WorldModel` and hands it to the desk behind a
//!   read-only capability gate. Nothing in the workspace calls `absorb_bar`,
//!   `absorb_news`, `absorb_fundamental` or `absorb_macro` outside that crate's
//!   own tests, and there is no `&mut` accessor to it from anywhere.
//!   `Platform::observe` keeps `close.to_f64()` and `volume.to_f64()` in two
//!   `Vec<f64>` and discards both of the record's instants. So the knowable-at
//!   time this walk proves the adapter computes correctly stops at the
//!   platform's front door; it is asserted here on the record the adapter
//!   handed over, which is as far as it survives.
//! * **`IngestionService` is composed by nothing.** The validation gate that
//!   publishes a `DataQualityFailure` rather than dropping a bad record is
//!   named by no binary and no composition root. `Platform::observe` takes
//!   `Vec<SensedRecord>` directly, so an incoherent bar reaches the price
//!   history unremarked.
//! * **`Platform`'s broker is fixed at construction.** There is a
//!   `Platform::set_central` and no `set_broker`, so the central plane can
//!   never submit through a live venue adapter even though
//!   `RestOrderEntryAdapter` implements `Broker`. Step 6 therefore builds the
//!   same `OrderManager` the platform builds, over the same `PreTradeChecker`
//!   and the same limits, and submits with the platform's own autonomy
//!   controller — which is the call `Platform::submit_order` makes internally
//!   and the closest this walk can get to it.
//! * **Nothing decodes a cell delta into a `CellReport`.** `qip_mesh::spine`
//!   says the composition root is where that decode belongs, and the
//!   composition root does not do it. Step 5 does it in this file, and says so.
//!
//! [`RestMarketDataAdapter`]: qip_market_ingestion::rest::RestMarketDataAdapter
//! [`DepthFeedAdapter`]: qip_market_ingestion::depth::DepthFeedAdapter
//! [`JsonRpcChainAdapter`]: qip_chain::rpc::JsonRpcChainAdapter
//! [`CapitalDispatcher`]: qip_mesh::spine::CapitalDispatcher
//! [`CapitalDownlink`]: qip_edge::mesh::CapitalDownlink
//! [`CellDeltaReceiver`]: qip_mesh::spine::CellDeltaReceiver
//! [`OrderManager`]: qip_execution_engine::oms::OrderManager
//! [`RestOrderEntryAdapter`]: qip_brokers::rest::RestOrderEntryAdapter
//! [`Platform::new`]: qip_kernel::Platform::new

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

mod live;

use live::{LoopbackServer, Match, Reply, Request, Route};

use qip_brokers::adapter::VenueAdapter;
use qip_brokers::credential::{
    RequirementKind, Secret, VenueCredential, requirements_of_kind, standard_requirements,
};
use qip_brokers::rest::{RestOrderEntryAdapter, RestVenueConfig};
use qip_capital::exposure::CellPosition;
use qip_chain::adapter::{ChainAdapter, ChainUpdate};
use qip_chain::block::{BlockNumber, ChainId};
use qip_chain::finality::Confirmations;
use qip_chain::rpc::{JsonRpcChainAdapter, RpcChainConfig, TokenBinding};
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, Context, Currency, Decimal, ManualClock, dec};
use qip_edge::cell::{Cell, CellConfig, WorkReport};
use qip_edge::envelope::sign_payload;
use qip_edge::mesh::{CapitalDownlink, CellStateDelta, CellUplink, DownlinkConfig, UplinkConfig};
use qip_events::AnyEvent;
use qip_execution_engine::broker::Broker;
use qip_execution_engine::oms::OrderManager;
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{LicensingClass, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::central::plane::CellReport;
use qip_kernel::cycle::Stage;
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord};
use qip_market_ingestion::depth::{DepthFeedAdapter, DepthFeedConfig, DepthInstrument};
use qip_market_ingestion::rest::{RestFeedConfig, RestInstrument, RestMarketDataAdapter};
use qip_mesh::spine::{CapitalDispatcher, CellDeltaReceiver, CellDeltaSink, DispatcherConfig};
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::pretrade::PreTradeChecker;
use qip_sequencing::ReorderPolicy;
use qip_storage::kv::{KeyValueStore, MemoryKeyValueStore};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use qip_transport::breaker::BreakerPolicy;
use qip_transport::{
    ClientLimits, MemoryDeadLetters, MeshConfig, RecordingSleeper, RetryPolicy, Sleeper,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

// --- the fixture ------------------------------------------------------------

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const STRATEGY: &str = "e2e-live-book-pressure";
const VENUE: &str = "XLON";
const SYMBOL: &str = "ACME";

/// The key the cell verifies capital grants with. A test key; the real one
/// reaches the process through a CSI mount. See `docs/security/credentials.md`.
const ENVELOPE_KEY: &[u8] = b"e2e-live-envelope-key-for-tests";
/// The vendor credential. A literal, and a distinctive one, so the walk can
/// assert it was sent *and* that it never reached a URL.
const VENDOR_KEY: &str = "vendor-key-e2e-live-never-in-a-url";
/// The node credential, for the same reason.
const NODE_CREDENTIAL: &str = "Bearer node-key-e2e-live-never-in-a-url";
/// The venue session secret, for the same reason.
const VENUE_SECRET: &str = "venue-secret-e2e-live-never-in-a-url";
const VENUE_ACCOUNT: &str = "e2e-live-book";

/// Bars the vendor serves. The last one is the jump.
const BARS: usize = 120;
/// How many of them have closed by the walk's first instant.
const KNOWABLE_BARS: usize = BARS - 1;

/// The chain head the node reports.
const HEAD: u64 = 100;
/// Head less the two confirmations the walk's adapter requires.
const SETTLED: u64 = HEAD - 2;
/// The deepest height the walk's adapter is told to start from.
const START_BLOCK: u64 = 96;

/// 15:00 on 24 August 2026 — mid-session, so nothing here turns on a session
/// boundary.
fn start() -> Timestamp {
    Timestamp::from_secs(1_787_583_600)
}

fn at(offset: Duration) -> Timestamp {
    start().saturating_add(offset)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

/// Limits tight enough that a stalled peer fails this walk in milliseconds
/// rather than in minutes, and generous enough in bytes for a hundred and
/// twenty bars.
fn limits_http() -> ClientLimits {
    ClientLimits {
        max_body: 1024 * 1024,
        max_headers: 32,
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_millis(1_000),
        write_timeout: StdDuration::from_millis(500),
        ..ClientLimits::default()
    }
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(object(), SYMBOL, InstrumentType::CommonStock)
                .venue(VENUE)
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("e2e-live", start()))
                .build(start())
                .expect("valid instrument"),
        )
        .expect("insertable");
    universe
}

fn risk_limits() -> LimitSet {
    LimitSet::new("e2e-live")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at twice equity"),
        )
}

/// The risk picture `Platform::submit_order` builds for its own control path.
///
/// Reproduced rather than borrowed because `Platform::risk_state` is private;
/// the numbers are the ones the platform uses, so the pre-trade check below is
/// the check the platform would have run.
fn platform_risk_state() -> RiskState {
    RiskState {
        equity: Decimal::from_int(10_000_000),
        cash: Decimal::from_int(10_000_000),
        ..RiskState::default()
    }
}

// --- the vendor's market data ----------------------------------------------

/// The quiet part of the series.
///
/// A deterministic recurrence rather than a seeded RNG so the same closes
/// appear on every machine and in every replay, and non-cumulative so the
/// series stays near a hundred: an adapter that hands the platform the same
/// window twice must not produce a step at the seam, or the second poll's
/// anomaly would be an artefact of the fixture rather than the jump.
fn quiet_closes() -> Vec<f64> {
    let mut value = 0.37_f64;
    (0..KNOWABLE_BARS)
        .map(|_| {
            value = (value * 7.13).fract();
            100.0 * (1.0 + (value - 0.5) * 0.02)
        })
        .collect()
}

/// Every close the vendor serves: the quiet series, then an 8.5% jump.
fn closes() -> Vec<f64> {
    let mut closes = quiet_closes();
    let jump = closes.last().copied().unwrap_or(100.0) * 1.085;
    closes.push(jump);
    closes
}

/// The vendor's whole answer: a hundred and twenty daily bars and one
/// reference-data change.
///
/// Bar `i` opens `BARS - 1 - i` days before the walk's first instant, so the
/// last bar opens *at* it and closes a day later — which is what makes it
/// unknowable at the first poll and knowable at the second, with nothing else
/// about the response changing.
fn market_data_payload() -> String {
    let closes = closes();
    let mut bars = Vec::with_capacity(BARS);
    for (i, close) in closes.iter().enumerate() {
        let open = if i == 0 { *close } else { closes[i - 1] };
        let open_time = start()
            .saturating_sub(Duration::from_days((BARS - 1 - i) as i64))
            .to_rfc3339();
        bars.push(format!(
            r#"{{"symbol":"{SYMBOL}","interval":"1d","open_time":"{open_time}",
                 "open":"{open:.6}","high":"{high:.6}","low":"{low:.6}","close":"{close:.6}",
                 "volume":"2500000","trade_count":8000}}"#,
            high = open.max(*close) * 1.001,
            low = open.min(*close) * 0.999,
        ));
    }
    let announced = start().saturating_sub(Duration::from_hours(1)).to_rfc3339();
    let effective = at(Duration::from_days(7)).to_rfc3339();
    format!(
        r#"{{"bars":[{}],"reference":[{{"symbol":"{SYMBOL}","field":"lot_size",
             "previous_value":"100","new_value":"1","effective_from":"{effective}",
             "announced_at":"{announced}","update_id":"ref-acme-1"}}]}}"#,
        bars.join(",")
    )
}

fn market_data_config(base: &str) -> RestFeedConfig {
    RestFeedConfig {
        name: "e2e-live-vendor".into(),
        provider: "a loopback REST market-data vendor".into(),
        base_url: Some(base.to_string()),
        path: "/v1/market-data".into(),
        api_key: Some(VENDOR_KEY.into()),
        api_key_header: "x-api-key".into(),
        licensing: LicensingClass::Licensed,
        // Zero, so what decides knowability is the record's own instants
        // rather than a constant every assertion would have to carry. The
        // withholding under test is the bar that has not finished forming.
        publication_delay: Duration::ZERO,
        window: Duration::from_days(200),
        max_records: 200,
        http: limits_http(),
    }
}

// --- the vendor's depth -----------------------------------------------------

/// A two-sided book, open, complete as of sequence 1000.
fn depth_snapshot() -> String {
    let stamp = start().saturating_sub(Duration::from_secs(10)).to_rfc3339();
    format!(
        r#"{{"symbol":"{SYMBOL}","sequence":1000,"at":"{stamp}","status":"open",
             "bids":[{{"price":"99.98","size":"500","orders":3}},
                     {{"price":"99.97","size":"300","orders":2}}],
             "asks":[{{"price":"100.02","size":"400","orders":4}},
                     {{"price":"100.03","size":"600","orders":1}}]}}"#
    )
}

/// Two increments that follow the snapshot: a new best bid, and the old best
/// offer deleted.
fn depth_updates() -> String {
    let first = start().saturating_sub(Duration::from_secs(5)).to_rfc3339();
    let second = start().saturating_sub(Duration::from_secs(4)).to_rfc3339();
    format!(
        r#"{{"updates":[
             {{"sequence":1001,"at":"{first}","type":"level_set","side":"bid",
               "price":"99.99","size":"250","orders":1}},
             {{"sequence":1002,"at":"{second}","type":"level_set","side":"ask",
               "price":"100.02","size":"0"}}]}}"#
    )
}

/// An increment with 1003 and 1004 missing behind it.
fn depth_updates_with_a_hole() -> String {
    let stamp = at(Duration::from_secs(2)).to_rfc3339();
    format!(
        r#"{{"updates":[
             {{"sequence":1005,"at":"{stamp}","type":"level_set","side":"bid",
               "price":"100.02","size":"900"}}]}}"#
    )
}

fn depth_config(base: &str) -> DepthFeedConfig {
    DepthFeedConfig {
        name: "e2e-live-depth".into(),
        provider: "a loopback depth vendor".into(),
        base_url: Some(base.to_string()),
        api_key: Some(VENDOR_KEY.into()),
        licensing: LicensingClass::Licensed,
        publication_delay: Duration::ZERO,
        // A gap deadline that has not passed by the second poll, so the hole is
        // still open: what is under test is a book *withheld* because something
        // is missing behind it, not one abandoned and rebuilt.
        reorder: ReorderPolicy::new(64, Duration::from_secs(30)),
        http: limits_http(),
        ..DepthFeedConfig::default()
    }
}

// --- the chain node ---------------------------------------------------------

const TOKEN_CONTRACT: &str = "0xc0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0";
const SENDER: &str = "0x1111111111111111111111111111111111111111";
const RECIPIENT: &str = "0x2222222222222222222222222222222222222222";
const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

fn block_hash(number: u64) -> String {
    format!("0x{number:064x}")
}

fn tx_hash(number: u64) -> String {
    format!("0x{:064x}", 0xc1c1_0000_u64 + number)
}

/// A 32-byte log topic carrying a 20-byte address in its low bytes.
fn topic_for(address: &str) -> String {
    format!("0x{}{}", "0".repeat(24), address.trim_start_matches("0x"))
}

/// When the proposer stamped block `number`.
///
/// Every block up to and including 97 is stamped before the walk's first
/// instant. Block 98 — which is settled, and which the adapter therefore does
/// ask for — is stamped ten minutes *after* it, so the adapter withholds it for
/// the second of its two reasons: a block the caller is not yet allowed to know
/// about.
fn block_stamp(number: u64) -> u64 {
    let base = u64::try_from(start().as_secs()).unwrap_or(0);
    if number >= SETTLED {
        base + 600
    } else {
        base - (97 - number) * 12
    }
}

fn block_json(number: u64) -> String {
    format!(
        r#"{{"number":"0x{number:x}","hash":"{hash}","parentHash":"{parent}",
             "timestamp":"0x{stamp:x}","baseFeePerGas":"0x3b9aca00","gasUsed":"0x5208",
             "gasLimit":"0x1c9c380",
             "transactions":[{{"hash":"{tx}","from":"{SENDER}","to":"{RECIPIENT}","nonce":"0x7",
               "gas":"0x5208","maxFeePerGas":"0x77359400","maxPriorityFeePerGas":"0x3b9aca00"}}]}}"#,
        hash = block_hash(number),
        parent = block_hash(number.saturating_sub(1)),
        stamp = block_stamp(number),
        tx = tx_hash(number),
    )
}

fn receipts_json(number: u64) -> String {
    format!(
        r#"[{{"transactionHash":"{tx}","transactionIndex":"0x0","status":"0x1","gasUsed":"0x5208",
             "effectiveGasPrice":"0x3b9aca00",
             "logs":[{{"address":"{TOKEN_CONTRACT}",
               "topics":["{TRANSFER_TOPIC}","{from}","{to}"],
               "data":"0x00000000000000000000000000000000000000000000000000000000000f4240",
               "logIndex":"0x0","removed":false}}]}}]"#,
        tx = tx_hash(number),
        from = topic_for(SENDER),
        to = topic_for(RECIPIENT),
    )
}

/// The height a JSON-RPC call is addressed to.
fn requested_height(request: &Request) -> Option<u64> {
    let raw = request.rpc_first_param()?;
    u64::from_str_radix(raw.trim_start_matches("0x"), 16).ok()
}

fn rpc_result(id: u64, result: &str) -> Reply {
    Reply::json(
        200,
        format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#),
    )
}

fn chain_config(base: &str) -> RpcChainConfig {
    RpcChainConfig {
        name: "e2e-live-node".into(),
        chain: ChainId::new("e2e-live-chain"),
        venue: VenueId::new("E2E-DEX"),
        endpoint: Some(base.to_string()),
        path: "/".into(),
        credential: Some(NODE_CREDENTIAL.into()),
        credential_header: "authorization".into(),
        // Two rather than the default twelve, so the fixture is four blocks
        // instead of fourteen. What is under test is that a depth is applied at
        // all and that the shallower blocks are never asked for, which does not
        // depend on the number.
        confirmations: Confirmations::exactly(2),
        start_block: Some(START_BLOCK),
        max_blocks_per_poll: 8,
        max_transactions_per_block: 100,
        include_pending: false,
        block_time: Duration::from_secs(12),
        http: limits_http(),
    }
}

// --- the venue --------------------------------------------------------------

/// The venue's answer to a submit: a partial fill, echoing the order it was
/// actually sent.
///
/// Computed from the request rather than scripted, so the acknowledgement is
/// evidence that the order arrived. A hard-coded body would answer identically
/// whether or not the adapter had put anything on the wire.
fn venue_acknowledgement(request: &Request) -> Reply {
    let Ok(body) = serde_json::from_str::<serde_json::Value>(&request.body) else {
        return Reply::json(400, r#"{"error":"the submit was not JSON"}"#);
    };
    let field = |name: &str| {
        body.get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let quantity = body
        .get("quantity")
        .map_or_else(|| "0".to_string(), std::string::ToString::to_string);
    Reply::json(
        200,
        format!(
            r#"{{"client_order_id":"{id}","venue_order_id":"VEN-1","state":"partially_filled",
                 "instrument":"{instrument}","side":"{side}","quantity":{quantity},"filled":"40",
                 "fills":[{{"fill_id":"F-1","quantity":"40","price":"100.02","costs":"0.35",
                            "at":"{submitted}"}}]}}"#,
            id = field("client_order_id"),
            instrument = field("instrument"),
            side = field("side"),
            submitted = field("submitted_at"),
        ),
    )
}

fn venue_config(base: &str) -> RestVenueConfig {
    RestVenueConfig {
        base_url: Some(base.to_string()),
        http: limits_http(),
        ..RestVenueConfig::default()
    }
}

fn venue_credential() -> Result<VenueCredential> {
    let enforced = requirements_of_kind(
        &standard_requirements(&venue()),
        &[RequirementKind::Account, RequirementKind::SessionCredential],
    );
    let name = enforced
        .iter()
        .find(|requirement| requirement.kind == RequirementKind::SessionCredential)
        .map(|requirement| requirement.name.clone())
        .ok_or_else(|| Error::not_found("a session credential requirement"))?;
    Ok(
        VenueCredential::satisfying(VENUE, VENUE_ACCOUNT, &enforced)?.with_secret(
            name,
            format!("QIP_{VENUE}_CREDENTIAL"),
            Secret::new(VENUE_SECRET),
        ),
    )
}

// --- the mesh ---------------------------------------------------------------

fn mesh_config(name: &str, peer: &str) -> MeshConfig {
    MeshConfig::new(name, peer)
        .with_retry(RetryPolicy {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(4),
            multiplier: 2,
            jitter_basis_points: 0,
        })
        .with_limits(limits_http())
}

/// A sleeper that records rather than sleeps, so the retry ladder costs this
/// walk no wall-clock time and the run is bounded whatever the machine is
/// doing.
fn sleeper() -> Arc<dyn Sleeper> {
    Arc::new(RecordingSleeper::new())
}

/// The grant the centre signs, in the form the cell verifies.
fn signed_grant(expires: Timestamp) -> Result<CapitalEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            dec!("1000000"),
            dec!("400000"),
            dec!("50000"),
            vec![venue()],
            start(),
            expires,
            "alice.chen",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    build(&sign_payload(ENVELOPE_KEY, &unsigned.signing_payload()))
}

/// The decode `qip_mesh::spine` says belongs in the composition root.
///
/// It is not there — `qip-kernel` names `CellReport` and never names
/// `CellStateDelta` — so the walk performs it, and the audit records that this
/// is the seam that does not exist. What it does is the whole of the missing
/// step: take the frame off the wire, decode the edge crate's type, and build
/// the central plane's.
#[derive(Debug, Default)]
struct DeltaCollector {
    deltas: Vec<CellStateDelta>,
}

impl CellDeltaSink for DeltaCollector {
    fn absorb(&mut self, frame: &AnyEvent) -> Result<()> {
        self.deltas.push(frame.decode::<CellStateDelta>()?.body);
        Ok(())
    }
}

fn report_from(delta: &CellStateDelta) -> CellReport {
    CellReport::new(delta.cell.clone(), delta.at)
        .with_positions(
            delta
                .orders
                .iter()
                .map(|order| CellPosition {
                    cell: delta.cell.clone(),
                    strategy: order.strategy.clone(),
                    instrument: order.object_id.as_str().to_string(),
                    sector: Sector::InformationTechnology,
                    venue: order.venue.clone(),
                    currency: Currency::GBP,
                    quantity: order.quantity,
                    price: order.price,
                })
                .collect(),
        )
        .with_utilisation(
            delta
                .utilisation
                .iter()
                .map(|entry| (entry.strategy.clone(), entry.utilisation.clone()))
                .collect(),
        )
}

/// One rule over one feature, compiled by the real compiler.
///
/// It never fires, and that is honest rather than lazy: the cell's feature
/// engine takes `MarketMessage`s and the live depth adapter produces
/// `OrderBook`s, so there is no seam by which a socket can supply a cell's
/// feature inputs. What the cell is here to demonstrate is the mesh in both
/// directions and the book it would route against; a strategy fed by a fixture
/// would prove neither and would read as though it had been.
fn trivial_strategy() -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "never",
            SignalKind::Enter,
            Expr::Flag(false),
            Expr::Exact(dec!("1")),
            Expr::Statistic(0.5),
            100,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

// --- the run ----------------------------------------------------------------

#[test]
fn the_platform_completes_a_cycle_observed_from_sockets_and_acted_on_over_one() -> Result<()> {
    // One clock for the whole walk, moved deliberately and read by every
    // adapter and by the platform. Nothing below depends on wall-clock
    // ordering, and every peer answers immediately, so the run is bounded by
    // the transport timeouts above and by nothing else.
    let clock = Arc::new(ManualClock::new(start()));
    let config = PlatformConfig {
        // One block, over and above the two the adapter already refuses to
        // report inside. The two depths are separate decisions and are meant to
        // be: the adapter's is how much of the node's chain it will look at,
        // and this is how much of what it was handed the platform will act on.
        // The default is twelve, which would need a fourteen-block fixture to
        // read a confirmed view out of and would test the same thing.
        chain_confirmations: 1,
        ..PlatformConfig::default()
    };
    let context = Context::new(Arc::clone(&clock) as Arc<dyn Clock>, config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(),
        risk_limits(),
    )?;

    // ===== 1. A VENDOR, OVER HTTP ==========================================
    //
    // One listener serves both the bar endpoint and the two depth endpoints,
    // because one vendor usually does.
    let vendor = LoopbackServer::routed(vec![
        Route::new(
            Match::Target("GET", "/v1/market-data"),
            Reply::json(200, market_data_payload()),
        ),
        Route::new(
            Match::Target("GET", "/v1/depth/snapshot"),
            Reply::json(200, depth_snapshot()),
        ),
        Route::in_turn(
            Match::Target("GET", "/v1/depth/updates"),
            vec![
                Reply::json(200, depth_updates()),
                Reply::json(200, depth_updates_with_a_hole()),
            ],
        ),
    ]);

    let mut feed = RestMarketDataAdapter::new(
        market_data_config(&vendor.url()),
        vec![RestInstrument::new(object(), SYMBOL, VENUE)],
    )?;
    feed.start(start())?;
    let first_poll = feed.poll(start())?;

    // The premise every assertion below rests on: a request really crossed a
    // socket. Without this line the rest of this section would pass just as
    // well against an adapter that answered from memory.
    assert_eq!(
        vendor.hits("/v1/market-data"),
        1,
        "the market-data endpoint was not asked; nothing below is evidence about a live path"
    );
    let sent = vendor.requests_to("GET", "/v1/market-data");
    assert_eq!(
        sent.len(),
        1,
        "the server recorded a different number of requests than it routed"
    );
    assert_eq!(
        sent[0].header("x-api-key"),
        Some(VENDOR_KEY),
        "the credential did not travel in the header the configuration named"
    );
    assert!(
        !sent[0].target.contains(VENDOR_KEY),
        "the credential reached the request target, which is written to every access log on the \
         path: {}",
        sent[0].target
    );

    // The withholding, at the adapter. A bar whose bucket has not closed is not
    // a bar; it is a bar being formed, and the cheapest way to write a backtest
    // that reads the future is to treat one as final.
    assert_eq!(
        feed.stats().withheld,
        1,
        "the vendor served a bar that closes tomorrow and the adapter handed it over"
    );
    assert_eq!(
        first_poll.len(),
        KNOWABLE_BARS + 1,
        "the poll should carry every knowable bar and the reference change: {} records",
        first_poll.len()
    );

    // Point in time, on the record the adapter handed over. The reference
    // change is the clearest case: `effective_from` is in the future, the
    // vendor announced it an hour ago, and the ingestion instant is the
    // caller's clock rather than the wall clock — which is what makes the same
    // fetch replayable.
    let reference = first_poll
        .iter()
        .find_map(|record| match record {
            SensedRecord::ReferenceData(update) => Some(update.as_ref()),
            _ => None,
        })
        .ok_or_else(|| Error::not_found("the reference change the vendor served"))?;
    assert_eq!(
        reference.provenance.source, "e2e-live-vendor",
        "the record does not name the adapter that produced it"
    );
    assert_eq!(
        reference.provenance.event_time,
        start().saturating_sub(Duration::from_hours(1)),
        "the knowable-at instant is not the vendor's announcement"
    );
    assert_eq!(
        reference.provenance.ingestion_time,
        start(),
        "the ingestion instant is a wall-clock read rather than the caller's clock, so this fetch \
         would not replay"
    );
    assert_eq!(
        reference.provenance.licensing,
        LicensingClass::Licensed,
        "the licensing class the deployment stated did not reach the record"
    );
    assert!(
        reference.effective_from > reference.provenance.event_time,
        "the fixture's reference change is not one whose valid time is after its knowable time, \
         so it does not test the distinction it was written for"
    );

    // ===== 2. THE WITHHELD BAR DOES NOT REACH A DECISION ====================
    let absorbed = platform.observe(first_poll);
    assert_eq!(
        absorbed,
        KNOWABLE_BARS + 1,
        "the platform absorbed a different number of records than the socket produced"
    );

    let first_cycle = platform.run_cycle(start());
    assert!(
        first_cycle.traversed_every_stage(),
        "a stage did not run:\n{}",
        first_cycle.summarise()
    );
    let sense = first_cycle.stage(Stage::Sense).expect("sense ran");
    // The anti-vacuity half. "No anomaly" is worth nothing unless the platform
    // demonstrably had the rest of the series in front of it; this says it did.
    assert_eq!(
        sense.produced, KNOWABLE_BARS,
        "the platform is not holding the observations the socket delivered, so the absence below \
         would be an absence of data rather than of the jump: {}",
        sense.detail
    );
    assert!(
        !anomaly_found(&platform),
        "a return anomaly was found before the bar carrying the jump had closed. Either the \
         adapter handed over a bar that was not yet knowable, or something other than the poll \
         put it in front of the detectors"
    );

    // The clock moves past the bucket close and the same body is polled again.
    // Nothing about the vendor changed.
    let second = at(Duration::from_days(1) + Duration::from_secs(1));
    clock.set(second);
    let second_poll = feed.poll(second)?;
    assert_eq!(
        vendor.hits("/v1/market-data"),
        2,
        "the second poll did not reach the vendor"
    );
    assert_eq!(
        second_poll.len(),
        BARS + 1,
        "the bar withheld at the first poll did not arrive at the second, so withholding lost it \
         rather than deferring it"
    );
    assert_eq!(
        feed.stats().withheld,
        1,
        "the second poll withheld something as well, which would make the assertion below \
         ambiguous"
    );

    platform.observe(second_poll);
    let second_cycle = platform.run_cycle(second);
    let discover = second_cycle.stage(Stage::Discover).expect("discover ran");
    // The other half of the pair. If the detectors could not have found this
    // jump at all, the first cycle's silence would prove nothing.
    assert!(
        anomaly_found(&platform),
        "an 8.5% move in a series whose typical day is half a percent went unnoticed once it \
         became knowable: {}",
        discover.detail
    );
    let opportunity = platform
        .queue()
        .iter()
        .find(|candidate| {
            candidate
                .detectors
                .iter()
                .any(|detector| detector == "return-anomaly")
        })
        .cloned()
        .ok_or_else(|| Error::not_found("the opportunity the live bars produced"))?;
    assert!(
        opportunity.affected_objects.contains(&object()),
        "the opportunity does not name the instrument the vendor served"
    );
    println!(
        "vendor: {} bar(s) over a socket, 1 withheld until its bucket closed; {}",
        BARS, opportunity.headline
    );

    // ===== 3. DEPTH, OVER HTTP =============================================
    let mut depth = DepthFeedAdapter::new(
        depth_config(&vendor.url()),
        vec![DepthInstrument::new(object(), SYMBOL, VENUE, "depth-a")],
    )?;
    let books = depth.poll(start())?;
    assert_eq!(
        vendor.hits("/v1/depth/snapshot"),
        1,
        "a book cannot be built from increments alone and no snapshot was asked for"
    );
    assert_eq!(books.len(), 1, "one book from one instrument: {books:?}");

    let cell_book = depth
        .venue_state(SYMBOL)
        .cloned()
        .ok_or_else(|| Error::not_found("the book the depth adapter built"))?;
    assert_eq!(
        cell_book.last_sequence(),
        Some(1002),
        "the book does not carry the venue position of the last message applied, so it is not the \
         vendor's book"
    );

    let mut cell = Cell::new(
        CellConfig::new(CELL, REGION).with_venue(venue()),
        FeatureEngine::new(MarketState::default(), Duration::from_secs(30)),
    )?;
    cell.track(cell_book);
    // The touch *after* both increments: the snapshot's 99.98/100.02 with a new
    // best bid at 99.99 in front of it and 100.02 deleted, leaving 99.99/100.03.
    // Neither the snapshot alone nor the test supplies that number, so a cell
    // that had been handed the snapshot and not the increments would read
    // 100.00 here and a cell holding a book this walk wrote would read whatever
    // this walk wrote.
    assert_eq!(
        cell.liquidity()
            .get(&venue(), &object())
            .and_then(qip_orderbook::venue::VenueState::mid),
        Some(d("100.01")),
        "the cell is not pricing from the book the socket built"
    );

    // The second poll's response has 1003 and 1004 missing behind it. A book
    // that is a correct prefix of the stream wearing a timestamp that reads as
    // the market now is worse than no book, so it is withheld — and the cell,
    // which has nothing to track, is still holding the book it had.
    let gapped = depth.poll(at(Duration::from_secs(5)))?;
    assert!(
        gapped.is_empty(),
        "a book with a known hole behind it reached the platform: {gapped:?}"
    );
    assert_eq!(
        depth.stats().withheld_gapped,
        1,
        "the book was withheld for some reason other than the gap, so this proves something else"
    );
    assert_eq!(
        cell.liquidity()
            .get(&venue(), &object())
            .and_then(qip_orderbook::venue::VenueState::last_sequence),
        Some(1002),
        "the gapped book reached the cell by some path other than the adapter"
    );
    println!(
        "depth: book at sequence {:?} over a socket; the gapped rebuild was withheld",
        depth.venue_state(SYMBOL).and_then(|s| s.last_sequence())
    );

    // ===== 4. A CHAIN NODE, OVER JSON-RPC ==================================
    let node = LoopbackServer::routed(vec![
        Route::computed(Match::Rpc("eth_blockNumber"), |request| {
            rpc_result(rpc_id(request), &format!("\"0x{HEAD:x}\""))
        }),
        Route::computed(
            Match::Rpc("eth_getBlockByNumber"),
            |request| match requested_height(request) {
                Some(height) => rpc_result(rpc_id(request), &block_json(height)),
                None => Reply::json(400, r#"{"error":"no height in the params"}"#),
            },
        ),
        Route::computed(
            Match::Rpc("eth_getBlockReceipts"),
            |request| match requested_height(request) {
                Some(height) => rpc_result(rpc_id(request), &receipts_json(height)),
                None => Reply::json(400, r#"{"error":"no height in the params"}"#),
            },
        ),
    ]);

    let mut chain = JsonRpcChainAdapter::new(
        chain_config(&node.url()),
        vec![TokenBinding::new(TOKEN_CONTRACT, object(), 6)],
    )?;
    chain.start(start())?;
    let updates = chain.poll(start())?;
    assert!(
        node.served() >= 3,
        "the premise of this section is that JSON-RPC calls crossed a socket; {} did",
        node.served()
    );
    let heights: Vec<u64> = updates
        .iter()
        .filter_map(|update| match update {
            ChainUpdate::Block(block) => Some(block.number.get()),
            ChainUpdate::Pending(_) | ChainUpdate::Dropped(_) => None,
        })
        .collect();
    assert_eq!(
        heights,
        vec![START_BLOCK, START_BLOCK + 1],
        "the adapter reported a different set of blocks than the two it should have"
    );
    assert_eq!(
        chain.stats().withheld,
        1,
        "block 98 is settled and stamped after the caller's clock; it should have been withheld"
    );

    // The unfinalised heights were never *asked for*, which is a stronger claim
    // than that they were not reported: there is no answer about them for
    // anything downstream to have read.
    let asked: Vec<u64> = node
        .requests()
        .iter()
        .filter(|request| request.rpc_method().as_deref() == Some("eth_getBlockByNumber"))
        .filter_map(requested_height)
        .collect();
    assert!(
        !asked.is_empty(),
        "no block was asked for at all, so the absence below is not evidence"
    );
    assert!(
        asked.iter().all(|height| *height <= SETTLED),
        "the adapter asked the node for a height inside its own confirmation depth: {asked:?}"
    );

    let absorption = platform.observe_chain(updates);
    assert_eq!(
        absorption.extended, 2,
        "the platform did not absorb the blocks the node served: {:?}",
        absorption.problems
    );
    assert_eq!(
        platform
            .chain()
            .and_then(qip_chain::ChainState::head_number),
        Some(BlockNumber::new(START_BLOCK + 1)),
        "the platform's chain head is not the deepest block the adapter was willing to report"
    );
    let confirmed = platform.confirmed_chain()?;
    assert_eq!(
        confirmed.as_of(),
        Some(BlockNumber::new(START_BLOCK)),
        "the confirmed view reaches a different depth than the platform's own configuration"
    );
    println!(
        "chain: node head {HEAD}, platform head {:?}, confirmed as of {:?}",
        platform
            .chain()
            .and_then(qip_chain::ChainState::head_number),
        confirmed.as_of()
    );

    // ===== 5. THE MESH, BOTH DIRECTIONS, OVER HTTP =========================
    //
    // Two inboxes and two listeners, because that is the topology: each end
    // holds the inbox the other publishes into.
    let mut receiver = CellDeltaReceiver::with_defaults("central", 64)?;
    let centre_peer = LoopbackServer::mesh(receiver.endpoint().clone());
    let cell_peer = LoopbackServer::mesh(qip_transport::MeshEndpoint::new(
        qip_transport::MeshInbox::new("london-1-inbox", 64, 256)?,
    ));

    // Down: the centre signs a grant and dispatches it. The spool persists it
    // before any attempt to send, so a crash between the two re-sends rather
    // than forgets.
    let store: Arc<dyn KeyValueStore> = Arc::new(MemoryKeyValueStore::new());
    let mut dispatcher = CapitalDispatcher::open(
        DispatcherConfig::new(CELL, mesh_config("capital:london-1", &cell_peer.url()))
            .with_breaker(BreakerPolicy::default(), 11),
        Arc::clone(&store),
        Arc::clone(&clock) as Arc<dyn Clock>,
        sleeper(),
        Box::new(MemoryDeadLetters::new(16)),
    )?;
    let grant = signed_grant(at(Duration::from_hours(8)))?;
    let dispatch = dispatcher.dispatch(grant.clone(), start())?;
    assert!(
        dispatch.is_delivered(),
        "the grant did not reach the cell's inbox: {dispatch:?}"
    );
    assert_eq!(
        dispatcher.pending()?,
        0,
        "an acknowledged grant is still in the spool, so a restart would send it again"
    );

    let mut downlink = CapitalDownlink::connect(
        DownlinkConfig::new(CELL, mesh_config("downlink:london-1", &cell_peer.url())),
        ENVELOPE_KEY,
        Arc::clone(&clock) as Arc<dyn Clock>,
        sleeper(),
    )?;
    let batch = downlink.poll(at(Duration::from_secs(30)))?;
    assert_eq!(
        batch.verified.len(),
        1,
        "the grant the centre sent did not verify at the cell: {:?}",
        batch.refused
    );
    assert!(
        batch.refused.is_empty(),
        "something arrived that did not verify: {:?}",
        batch.refused
    );
    assert!(
        cell_peer.served() >= 2,
        "the capital round trip did not use the socket; {} connection(s) were served",
        cell_peer.served()
    );

    // The cell deploys under the grant that crossed the wire, and only that
    // one. There is no path from a polled frame to a deployment that skips the
    // signature check: `batch.verified` is the only source of the type `deploy`
    // takes.
    let verified = batch
        .verified
        .into_iter()
        .next()
        .ok_or_else(|| Error::not_found("the verified grant"))?;
    assert_eq!(
        verified.expires_at(),
        at(Duration::from_hours(8)),
        "the grant the cell verified is not the one the centre signed"
    );
    assert_eq!(verified.strategy().as_str(), STRATEGY);
    let (strategy, program) = trivial_strategy()?;
    cell.deploy(strategy, program, verified)?;
    assert_eq!(cell.deployed_strategies(), vec![STRATEGY]);

    // Up: the cell tells the centre what it is running on.
    let mut uplink = CellUplink::connect(
        UplinkConfig::new(
            CELL,
            REGION,
            mesh_config("uplink:london-1", &centre_peer.url()),
        ),
        Arc::clone(&clock) as Arc<dyn Clock>,
        sleeper(),
        Box::new(MemoryDeadLetters::new(16)),
    )?;
    let delta = cell.state_delta(&WorkReport::default(), at(Duration::from_secs(60)));
    let sent_up = uplink.publish(delta, at(Duration::from_secs(60)))?;
    assert!(
        sent_up.is_delivered(),
        "the cell's state did not reach the centre: {sent_up:?}"
    );

    let mut collector = DeltaCollector::default();
    let drained = receiver.drain(at(Duration::from_secs(90)), 16, &mut collector)?;
    assert!(drained.is_clean(), "the drain was not clean: {drained:?}");
    assert_eq!(
        drained.absorbed, 1,
        "the centre absorbed a different number of deltas than the cell sent"
    );
    let received = collector
        .deltas
        .first()
        .ok_or_else(|| Error::not_found("the delta the centre absorbed"))?;
    assert_eq!(received.cell, CELL);
    assert_eq!(
        received.utilisation.len(),
        1,
        "the delta carried nothing about what the deployed strategy has committed, so the centre \
         cannot see which cells are about to run out of authority"
    );
    assert_eq!(
        received.utilisation[0].envelope_expires_at,
        at(Duration::from_hours(8)),
        "the expiry that crossed the wire is not the one the centre granted"
    );

    // The centre knew nothing about this cell before the delta crossed, which is
    // what makes the assertion after the ingest evidence about the wire rather
    // than about a field that was already populated.
    assert!(
        platform
            .central()
            .utilisation(CELL, &StrategyId::new(STRATEGY))
            .is_none(),
        "the central plane already held this cell's utilisation before anything crossed the mesh"
    );
    let ingestion =
        platform.ingest_cell_report(report_from(received), at(Duration::from_secs(90)))?;
    assert_eq!(
        ingestion.cell, CELL,
        "the central plane absorbed a report for a different cell"
    );
    assert!(
        ingestion.halted.is_none(),
        "a reconciling cell was halted: {:?}",
        ingestion.halted
    );
    assert!(
        platform
            .central()
            .utilisation(CELL, &StrategyId::new(STRATEGY))
            .is_some(),
        "the delta crossed the wire, was absorbed, and left the central plane knowing nothing          about what the cell has committed"
    );
    println!(
        "mesh: grant of {} down one socket and verified; delta up another and absorbed",
        grant.gross_limit()
    );

    // ===== 6. A VENUE, OVER HTTP ===========================================
    let venue_server = LoopbackServer::routed(vec![
        Route::new(Match::Target("GET", "/v1/health"), Reply::json(200, "{}")),
        Route::computed(Match::Target("POST", "/v1/orders"), venue_acknowledgement),
    ]);

    let mut gateway =
        RestOrderEntryAdapter::new(venue(), venue_config(&venue_server.url()), start())?;
    gateway.bring_up(&venue_credential()?, start())?;
    assert!(
        venue_server.hits("/v1/health") >= 3,
        "bringing a session up is a connect, a logon and a heartbeat, each a real request; only \
         {} reached the venue",
        venue_server.hits("/v1/health")
    );

    // Nothing here runs a scheduler, so the session goes stale between bring-up
    // and the order two minutes later, and `ready` refuses a session last
    // confirmed some unknown time ago. Re-proving it before the send is what
    // `qip_edge_node::gateway::RestGateway::ensure_ready` does in the
    // deployable, and for the same reason; a production cell heartbeats on a
    // timer and this call is then almost always a no-op.
    let heartbeats = venue_server.hits("/v1/health");
    gateway.heartbeat(at(Duration::from_secs(120)))?;
    assert_eq!(
        venue_server.hits("/v1/health"),
        heartbeats + 1,
        "the heartbeat did not reach the venue, so the session below was not re-proven against it"
    );

    // The order names the opportunity the live bars produced, and is built by
    // the platform's own `order_from`. See the header: the platform's broker
    // cannot be replaced, so it is submitted through the same `OrderManager`
    // over the same `PreTradeChecker` and the same autonomy controller that
    // `Platform::submit_order` uses.
    let order = platform.order_from(
        object(),
        Side::Buy,
        Decimal::from_int(100),
        d("100.02"),
        "prop-e2e-live",
        vec![opportunity.opportunity_id.as_str().to_string()],
        at(Duration::from_secs(120)),
    );
    let order_id = order.order_id.clone();
    let mut manager = OrderManager::new(PreTradeChecker::new(risk_limits()));
    let result = manager.submit(
        order,
        &mut gateway as &mut dyn Broker,
        platform.autonomy(),
        &platform_risk_state(),
        BTreeMap::new(),
        None,
        at(Duration::from_secs(120)),
    );
    assert!(
        result.accepted,
        "the control path refused an order it should have passed: {:?}",
        result.refusal
    );

    // The venue really received it. `served()` and the recorded body, not the
    // absence of an error: an adapter that answered from memory would raise no
    // error either.
    let submits = venue_server.requests_to("POST", "/v1/orders");
    assert_eq!(
        submits.len(),
        1,
        "exactly one submit should have reached the venue, {} did",
        submits.len()
    );
    assert!(
        submits[0].body.contains(order_id.as_str()),
        "the body the venue received does not name the order: {}",
        submits[0].body
    );
    assert!(
        submits[0].body.contains(object().as_str()),
        "the body the venue received does not name the instrument: {}",
        submits[0].body
    );
    assert_eq!(
        submits[0].header("idempotency-key"),
        Some(
            gateway
                .idempotency_key_for(
                    manager
                        .order(&order_id)
                        .ok_or_else(|| { Error::not_found("the order the manager kept") })?
                )?
                .as_str()
        ),
        "the submit carried a key other than the one computed from the order's own terms"
    );
    assert!(
        !submits[0].target.contains(VENUE_SECRET),
        "the session secret reached the request target: {}",
        submits[0].target
    );

    // And the acknowledgement came back to the order manager. The fill is on
    // the order, not merely in a return value that was discarded.
    assert_eq!(
        result.filled_quantity(),
        Decimal::from_int(40),
        "the venue's partial fill did not reach the submission result"
    );
    let held = manager
        .order(&order_id)
        .ok_or_else(|| Error::not_found("the order the manager kept"))?;
    assert_eq!(
        held.fills.len(),
        1,
        "the venue answered with a fill and the order manager recorded none"
    );
    assert_eq!(held.fills[0].quantity, Decimal::from_int(40));
    assert!(
        gateway.unknown_orders().is_empty(),
        "an order the venue answered about is unknown: {:?}",
        gateway.unknown_orders()
    );
    assert_eq!(gateway.stats().acknowledged, 1);
    println!(
        "venue: order {} of 100 over a socket, {} filled, acknowledged",
        order_id.as_str(),
        result.filled_quantity()
    );

    // ===== 7. LEARNING, AND NOTHING BECAME LIVE ============================
    let learning = platform.run_cycle(at(Duration::from_secs(180)));
    let learn = learning.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.ran && learn.detail.len() > 10,
        "the learning stage said nothing useful: {:?}",
        learn.detail
    );
    platform.outcomes().verify()?;

    assert!(
        !platform.is_live_capable(),
        "the platform became live-capable during a paper run"
    );
    assert!(
        !manager.has_live_fills(),
        "a fill from a socket was booked as a real one. `AdapterClass` has no live variant, so \
         this is a claim about the endpoint and not about the money — but the bit must not flip"
    );
    assert!(
        !cell.autonomy().ceiling().is_live(),
        "the cell raised its own ceiling"
    );

    println!(
        "walk complete: {} bar(s) and a book and {} block(s) in over sockets, one order out over \
         one, paper throughout",
        BARS,
        heights.len()
    );
    Ok(())
}

/// Whether the platform's queue holds an opportunity the return-anomaly
/// detector produced for the instrument the vendor serves.
///
/// By detector name rather than by count, so the other detectors firing on the
/// same series — a volatility shift, a regime change — cannot make this true.
fn anomaly_found(platform: &Platform) -> bool {
    platform.queue().iter().any(|opportunity| {
        opportunity.affected_objects.contains(&object())
            && opportunity
                .detectors
                .iter()
                .any(|detector| detector == "return-anomaly")
    })
}

/// The JSON-RPC id a call carried, echoed back so a client that checks it is
/// answered rather than confused.
fn rpc_id(request: &Request) -> u64 {
    serde_json::from_str::<serde_json::Value>(&request.body)
        .ok()
        .and_then(|value| value.get("id").and_then(serde_json::Value::as_u64))
        .unwrap_or(1)
}
