//! What the API keeps, and what a restart takes away.
//!
//! The platform's event log lives in memory and starts again at every launch,
//! so the only thing that makes two runs of `qip-api` one audit trail is the
//! archive. These tests exercise it through the route an operator actually
//! calls, because an archive that works when driven directly and not when
//! driven through `POST /cycle` is an archive that does nothing.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::http::{Handler, Method, Request};
use qip_api::routes::Api;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, ManualClock};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::ChainArchive;
use qip_storage::settings::StorageSettings;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> std::path::PathBuf {
    let unique = DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-api-persist-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the test fixture directory is creatable");
    dir
}

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn operator_credentials() -> Vec<Credential> {
    vec![Credential::from_token(
        "operator@example.com",
        Role::Operator,
        "operator-token".to_string(),
        now(),
        now().saturating_add(Duration::from_days(30)),
    )]
}

/// An `Api` assembled the way `main` assembles one, minus the HTTP server.
fn api(archive: Option<Arc<ChainArchive>>) -> Result<Arc<Api>> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(now()));
    let context = Context::new(clock.clone(), config.seed);
    let platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;
    let api = Api::new(
        Arc::new(Mutex::new(platform)),
        Arc::new(Authenticator::new(operator_credentials())),
        Arc::new(RateLimiter::new(Duration::from_secs(60), 1000)),
        clock,
    );
    Ok(Arc::new(match archive {
        Some(archive) => api.with_archive(archive),
        None => api,
    }))
}

fn run_cycle(api: &Api) -> String {
    let response = api.handle(&Request {
        method: Method::Post,
        path: "/api/v1/cycle".to_string(),
        query: BTreeMap::new(),
        headers: BTreeMap::from([(
            "authorization".to_string(),
            "Bearer operator-token".to_string(),
        )]),
        body: Vec::new(),
        peer: "127.0.0.1:1".to_string(),
    });
    assert_eq!(response.status, 202, "the cycle route did not run a cycle");
    String::from_utf8_lossy(&response.body).to_string()
}

fn get(api: &Api, path: &str) -> String {
    let response = api.handle(&Request {
        method: Method::Get,
        path: path.to_string(),
        query: BTreeMap::new(),
        headers: BTreeMap::from([(
            "authorization".to_string(),
            "Bearer operator-token".to_string(),
        )]),
        body: Vec::new(),
        peer: "127.0.0.1:1".to_string(),
    });
    assert_eq!(response.status, 200, "{path} did not answer");
    String::from_utf8_lossy(&response.body).to_string()
}

fn archive_at(root: &std::path::Path) -> Result<ChainArchive> {
    ChainArchive::open(
        StorageSettings::from_values(Some("engine"), root.to_str())?.key_value("event-log")?,
    )
}

// --- the audit trail spans the process --------------------------------------

#[test]
fn a_cycle_run_through_the_api_leaves_a_chain_that_outlives_the_api() -> Result<()> {
    let root = temp_dir("survives");
    let archive = Arc::new(archive_at(&root)?);
    let api = api(Some(archive.clone()))?;

    let body = run_cycle(&api);
    assert!(
        body.contains(r#""archived":"#) && !body.contains("archive_error"),
        "the response did not report what was archived: {body}"
    );

    let archived = archive.len()?;
    assert!(
        archived > 0,
        "a cycle produced no archived records; the wiring is inert"
    );
    drop(api);
    drop(archive);

    // A different process against the same root.
    let reopened = archive_at(&root)?;
    assert_eq!(
        reopened.len()?,
        archived,
        "the chain did not survive the process that wrote it"
    );
    assert_eq!(
        reopened.first_broken_position()?,
        None,
        "the recovered chain does not verify"
    );
    Ok(())
}

#[test]
fn a_restarted_api_appends_to_the_chain_rather_than_starting_it_again() -> Result<()> {
    // The failure this guards: a fresh platform's event log begins its
    // sequences at one, so an archive keyed by the source sequence would write
    // the second run over the first and still verify afterwards.
    let root = temp_dir("restart");

    let first = {
        let archive = Arc::new(archive_at(&root)?);
        let api = api(Some(archive.clone()))?;
        run_cycle(&api);
        archive.len()?
    };
    assert!(first > 0, "the premise: the first run archived something");

    let total = {
        let archive = Arc::new(archive_at(&root)?);
        let api = api(Some(archive.clone()))?;
        run_cycle(&api);
        archive.len()?
    };

    assert!(
        total > first,
        "the second run left the archive at {total} record(s) having found {first}; it \
         overwrote the first run instead of appending to it"
    );
    assert_eq!(archive_at(&root)?.first_broken_position()?, None);
    Ok(())
}

#[test]
fn running_two_cycles_archives_each_of_them_once() -> Result<()> {
    // The watermark, through the route. The handler hands the log's whole
    // record slice over every time, so without one the second cycle would
    // re-archive the first cycle's records.
    let root = temp_dir("twice");
    let archive = Arc::new(archive_at(&root)?);
    let api = api(Some(archive.clone()))?;

    run_cycle(&api);
    let after_one = archive.len()?;
    assert!(
        after_one > 0,
        "the premise: the first cycle archived records"
    );

    run_cycle(&api);
    let after_two = archive.len()?;
    assert!(
        after_two > after_one,
        "the second cycle archived nothing new"
    );
    assert!(
        after_two < after_one * 2 + 1 || after_one == 0,
        "the second cycle re-archived the first cycle's records: {after_one} then {after_two}"
    );
    assert_eq!(archive.first_broken_position()?, None);
    Ok(())
}

// --- an unconfigured API says so --------------------------------------------

#[test]
fn an_api_with_no_archive_says_it_archived_nothing_rather_than_claiming_a_number() -> Result<()> {
    // The premise: the same route with an archive reports a count, so `null`
    // here is the honest answer to "what was kept" and not a route that never
    // reports one. A caller cannot tell a durable deployment from an ephemeral
    // one by the status code, so the body has to say.
    let root = temp_dir("unconfigured");
    let configured = api(Some(Arc::new(archive_at(&root)?)))?;
    assert!(run_cycle(&configured).contains(r#""archived":"#));

    let unconfigured = api(None)?;
    let body = run_cycle(&unconfigured);
    assert!(
        body.contains(r#""archived":null"#),
        "an API with nowhere to write must say so: {body}"
    );
    Ok(())
}

// --- what a running deployment can be asked -----------------------------------

#[test]
fn the_status_endpoint_reports_what_survives_the_process_and_not_only_what_is_in_memory()
-> Result<()> {
    // The start-up banner scrolls away. An operator asking a *running* server
    // whether it is keeping anything has only this, and an event count on its
    // own cannot distinguish a durable deployment from one keeping none of it.
    let root = temp_dir("status");
    let archive = Arc::new(archive_at(&root)?);
    let configured = api(Some(archive.clone()))?;

    // Before any cycle: configured, and honestly empty.
    let body = get(&configured, "/api/v1/system/status");
    assert!(body.contains(r#""archived":0"#), "{body}");

    run_cycle(&configured);
    let body = get(&configured, "/api/v1/system/status");
    let archived = archive.records_archived();
    assert!(archived > 0, "the premise: the cycle archived records");
    assert!(
        body.contains(&format!(r#""archived":{archived}"#)),
        "the status endpoint disagrees with the archive it was given: {body}"
    );

    // The premise for reading `null` as meaningful: a configured archive
    // reports a number, so `null` is "nothing is configured" and not "this
    // field is never populated". Zero would have read as "configured, empty",
    // which is the one answer that would let an ephemeral deployment pass for
    // a durable one that has not run yet.
    let unconfigured = api(None)?;
    let body = get(&unconfigured, "/api/v1/system/status");
    assert!(
        body.contains(r#""archived":null"#),
        "an API keeping nothing must say so rather than report a zero: {body}"
    );
    Ok(())
}
