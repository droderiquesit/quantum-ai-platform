//! R12-CAPITAL-4086: `POST /cycle` reads `qip-ledgerd`'s obligations at the
//! cycle's instant and installs them before the cycle runs, so the survival
//! waterfall withholds what the ledger has pledged. The ledger is a stub
//! listener here answering the route's exact shape, `LedgerObligations` as
//! JSON, and recording what it was asked.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::http::{Handler, Method, Request};
use qip_api::ledger_obligations::LedgerObligationsReader;
use qip_api::routes::Api;
use qip_contracts::ledger::{Account, LedgerObligations};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ManualClock, dec};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

const ANALYST_TOKEN: &str = "analyst-token";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// A ledger answering every request with `body`, and the request lines it
/// was sent.
fn stub_ledger(body: String) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buffer = [0u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let text = String::from_utf8_lossy(&buffer[..read]).to_string();
            seen.lock()
                .unwrap()
                .push(text.lines().next().unwrap_or("").to_string());
            let answer = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(answer.as_bytes());
        }
    });
    (format!("http://{address}"), asked)
}

fn api(reader: Option<LedgerObligationsReader>) -> Result<(Api, Arc<Mutex<Platform>>)> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock.clone(), config.seed);
    let platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;
    let platform = Arc::new(Mutex::new(platform));
    let authenticator = Arc::new(Authenticator::new(vec![Credential::from_token(
        "analyst@example.com",
        Role::Analyst,
        ANALYST_TOKEN.to_string(),
        start(),
        start().saturating_add(Duration::from_days(30)),
    )]));
    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(60), 1000));
    let mut api = Api::new(platform.clone(), authenticator, rate_limiter, clock);
    if let Some(reader) = reader {
        api = api.with_ledger_obligations(reader);
    }
    Ok((api, platform))
}

fn cycle(api: &Api) -> u16 {
    let mut headers = BTreeMap::new();
    headers.insert(
        "authorization".to_string(),
        format!("Bearer {ANALYST_TOKEN}"),
    );
    api.handle(&Request {
        method: Method::Post,
        path: "/api/v1/cycle".to_string(),
        query: BTreeMap::new(),
        headers,
        body: Vec::new(),
        peer: "127.0.0.1:1".to_string(),
    })
    .status
}

fn pledged_reading(at: Timestamp) -> Result<LedgerObligations> {
    let balances: BTreeMap<(Account, String), Decimal> = [
        (
            Account::Margin {
                venue: "XNYS".to_string(),
            },
            dec!("100000"),
        ),
        (
            Account::Collateral {
                counterparty: "prime-broker".to_string(),
            },
            dec!("300000"),
        ),
    ]
    .into_iter()
    .map(|(account, amount)| ((account, "USD".to_string()), amount))
    .collect();
    LedgerObligations::from_balances(&balances, "USD", at)
}

/// Mutation: remove the `observe_ledger_obligations` call from the cycle
/// route and the platform holds no reading after the cycle, so the ceiling
/// is not lowered and this fails.
#[test]
fn qip_apis_cycle_reads_qip_ledgerds_obligations_at_the_cycle_instant_and_a_ledger_pledge_lowers_the_ceiling_the_next_order_is_vetoed_against()
-> Result<()> {
    let (without, unread) = api(None)?;
    assert_eq!(cycle(&without), 202, "premise: a cycle runs with no reader");
    let open = unread
        .lock()
        .unwrap()
        .survival_gross_ceiling()
        .expect("premise: the ceiling is seeded");

    let reading = pledged_reading(start())?;
    let (url, asked) = stub_ledger(serde_json::to_string(&reading).unwrap());
    let (with, read) = api(Some(LedgerObligationsReader::new(Some(url))))?;
    assert_eq!(cycle(&with), 202, "the cycle ran");

    let requested = asked.lock().unwrap().clone();
    assert_eq!(
        requested,
        vec![format!(
            "GET /api/v1/ledger/obligations?unit=USD&as_of={} HTTP/1.1",
            start().as_secs()
        )],
        "the route was not asked once, at the cycle's instant, in the settlement currency"
    );
    let platform = read.lock().unwrap();
    assert_eq!(
        platform.ledger_obligations(),
        Some(&reading),
        "the reading was not installed"
    );
    let lowered = platform.survival_gross_ceiling().expect("re-seeded");
    assert!(
        lowered < open,
        "a ledger pledge did not lower the survival ceiling: {lowered} against {open}"
    );
    assert_eq!(
        open - lowered,
        dec!("300000"),
        "margin and the excess pledge are withheld, the covered part once"
    );
    Ok(())
}

/// A ledger that cannot be reached installs no reading, and the cycle still
/// runs and says why: a reading nobody took is not a reading of zero.
#[test]
fn an_unreachable_ledger_installs_no_reading_and_the_cycle_still_runs() -> Result<()> {
    // A port nothing listens on: bound, then dropped.
    let address = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let (with, read) = api(Some(LedgerObligationsReader::new(Some(format!(
        "http://{address}"
    )))))?;
    assert_eq!(cycle(&with), 202, "the cycle ran without the ledger");
    assert!(
        read.lock().unwrap().ledger_obligations().is_none(),
        "a reading was installed from a ledger that never answered"
    );
    Ok(())
}
