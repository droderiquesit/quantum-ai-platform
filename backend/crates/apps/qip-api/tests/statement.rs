//! The wallet statement feed: the composition root's one way of handing a
//! venue balance to the kernel, and what `/wallet` says with and without it.
//!
//! Every test asserts its premise before the property. `/wallet` answers
//! `assembled: false` for a platform nothing observed into, so the tests
//! that prove a statement reaches the wallet first prove the wallet was
//! unassembled, and the test that proves an unset variable leaves it
//! unassembled first runs a cycle — so the answer is "nothing was observed"
//! and not "no cycle has run".

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::http::{Handler, Method, Request, Response};
use qip_api::ledger_views::NO_WALLET;
use qip_api::routes::Api;
use qip_api::statement::{
    MAX_STATEMENT_HOLDINGS, STATEMENT_PATH_VARIABLE, Statement, StatementFeed, StatementRefresh,
    absent_banner,
};
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, Context, Decimal, ManualClock};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const ANALYST_TOKEN: &str = "analyst-token";
const VIEWER_TOKEN: &str = "viewer-token";
/// The simulated broker's venue, which is where the kernel's ledger books
/// the desk's cash — so a statement at it reconciles against a figure.
const DESK_VENUE: &str = "simulated-venue";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// An hour before the clock: dated, and fresh against the kernel's one-day
/// statement freshness.
fn dated() -> Timestamp {
    start().saturating_sub(Duration::from_secs(3_600))
}

/// A directory of this test's own, so two tests running at once cannot see
/// each other's file.
fn fixture_dir(name: &str) -> std::path::PathBuf {
    let directory =
        std::env::temp_dir().join(format!("qip-api-statement-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("the fixture directory is created");
    directory
}

fn write_fixture(directory: &std::path::Path, text: &str) -> String {
    let path = directory.join("statement.json");
    std::fs::write(&path, text).expect("the fixture is written");
    path.display().to_string()
}

/// A statement of the desk's cash at its venue, to the unit.
fn desk_statement(quantity: Decimal) -> String {
    format!(
        r#"{{"as_of": "{}", "venue": "{DESK_VENUE}", "tolerance": "1",
            "holdings": [{{"asset": "USD", "quantity": "{quantity}"}}]}}"#,
        dated().to_rfc3339()
    )
}

struct Rig {
    api: Api,
    platform: Arc<Mutex<Platform>>,
    authenticator: Arc<Authenticator>,
    clock: Arc<ManualClock>,
}

fn rig() -> Result<Rig> {
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
    let authenticator = Arc::new(Authenticator::new(vec![
        Credential::from_token(
            "analyst@example.com",
            Role::Analyst,
            ANALYST_TOKEN.to_string(),
            start(),
            start().saturating_add(Duration::from_days(30)),
        ),
        Credential::from_token(
            "viewer@example.com",
            Role::Viewer,
            VIEWER_TOKEN.to_string(),
            start(),
            start().saturating_add(Duration::from_days(30)),
        ),
    ]));
    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(60), 1000));
    Ok(Rig {
        api: Api::new(
            platform.clone(),
            authenticator.clone(),
            rate_limiter,
            clock.clone(),
        ),
        platform,
        authenticator,
        clock,
    })
}

impl Rig {
    /// The root's assembly: the feed observed into the platform at start,
    /// and the API wrapped so an admitted cycle re-reads the file.
    fn with_feed(self, path: &str) -> Result<(StatementRefresh<Api>, Arc<Mutex<Platform>>)> {
        let feed = StatementFeed::open(path, self.clock.now())?;
        {
            let mut platform = self
                .platform
                .lock()
                .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
            feed.statement().observe_into(&mut platform)?;
        }
        let handler = StatementRefresh::new(
            self.api,
            Arc::new(Mutex::new(feed)),
            self.platform.clone(),
            self.authenticator,
            self.clock,
        );
        Ok((handler, self.platform))
    }

    fn initial_equity(&self) -> Result<Decimal> {
        let platform = self
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        Ok(platform.config().initial_equity)
    }
}

fn request(method: Method, path: &str, token: &str) -> Request {
    let mut headers = BTreeMap::new();
    headers.insert("authorization".to_string(), format!("Bearer {token}"));
    Request {
        method,
        path: format!("/api/v1{path}"),
        query: BTreeMap::new(),
        headers,
        body: Vec::new(),
        peer: "127.0.0.1:1".to_string(),
    }
}

fn body_of(response: Response) -> (String, serde_json::Value) {
    let text = String::from_utf8(response.body).expect("a UTF-8 body");
    let value = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"));
    (text, value)
}

fn wallet(handler: &dyn Handler) -> (String, serde_json::Value) {
    body_of(handler.handle(&request(Method::Get, "/wallet", ANALYST_TOKEN)))
}

/// Whether `message` carries `token` as a whitespace-delimited word.
///
/// Delimited on purpose: `contains("as_of")` is true of a message that
/// mentions the field while refusing something else.
fn names(message: &str, token: &str) -> bool {
    message
        .split_whitespace()
        .any(|word| word.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | '(' | ')')) == token)
}

// --- the feed reaches the wallet -------------------------------------------

#[test]
fn a_valid_statement_file_makes_the_wallet_answer_assembled_after_one_cycle() -> Result<()> {
    // The failure this guards: `Platform::observe_statement` existed and
    // nothing in any binary called it, so every deployed `/wallet` answered
    // `assembled: false` for ever while LEARN's reconciliation sat complete
    // and unreached. Premise first: the wallet is unassembled before the
    // cycle even with the statement observed, because assembly is LEARN's.
    let directory = fixture_dir("valid");
    let rig = rig()?;
    let equity = rig.initial_equity()?;
    let path = write_fixture(&directory, &desk_statement(equity));
    let (handler, _platform) = rig.with_feed(&path)?;

    let (text, before) = wallet(&handler);
    assert_eq!(before["assembled"], serde_json::json!(false), "{text}");
    assert_eq!(before["reason"], serde_json::json!(NO_WALLET), "{text}");

    let response = handler.handle(&request(Method::Post, "/cycle", ANALYST_TOKEN));
    assert_eq!(
        response.status,
        202,
        "{}",
        String::from_utf8_lossy(&response.body)
    );

    let (text, body) = wallet(&handler);
    assert_eq!(body["assembled"], serde_json::json!(true), "{text}");
    assert_eq!(body["reason"], serde_json::Value::Null, "{text}");
    let holdings = body["holdings"].as_array().expect("a list");
    assert_eq!(holdings.len(), 1, "{text}");
    assert_eq!(
        holdings[0],
        serde_json::json!({
            "venue": DESK_VENUE,
            "asset": "USD",
            "observed_quantity": equity.to_string(),
            "observed_at": dated().to_rfc3339(),
            "provenance": "statement",
            "ledger_expected": equity.to_string()
        }),
        "{text}"
    );
    assert_eq!(
        body["reconciliation"]["outcomes"],
        serde_json::json!([{
            "outcome": "reconciled",
            "venue": DESK_VENUE,
            "asset": "USD",
            "delta": "0"
        }]),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

#[test]
fn a_changed_statement_file_is_re_read_before_the_next_cycle() -> Result<()> {
    // A file the operator replaced reaches the next cycle's LEARN stage —
    // here as a break, because the custodian now says five units less than
    // the ledger books and the tolerance is one. Premise first: the first
    // statement reconciled clean, so the halt below is the new figure's.
    let directory = fixture_dir("changed");
    let rig = rig()?;
    let equity = rig.initial_equity()?;
    let path = write_fixture(&directory, &desk_statement(equity));
    let (handler, _platform) = rig.with_feed(&path)?;
    let response = handler.handle(&request(Method::Post, "/cycle", ANALYST_TOKEN));
    assert_eq!(response.status, 202);
    let (text, body) = wallet(&handler);
    assert_eq!(
        body["reconciliation"]["halted_venue_assets"],
        serde_json::json!(0),
        "the premise: the first statement reconciles to the unit: {text}"
    );

    // Replaced, with a modification time moved a whole second forward so
    // the change is visible however coarse the filesystem's timestamps are.
    let short = equity - Decimal::from_int(5);
    std::fs::write(&path, desk_statement(short)).expect("the fixture is rewritten");
    let file = std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("the fixture opens for writing");
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
        .expect("the modification time moves");
    drop(file);

    let response = handler.handle(&request(Method::Post, "/cycle", ANALYST_TOKEN));
    assert_eq!(
        response.status,
        202,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let (text, body) = wallet(&handler);
    assert_eq!(
        body["holdings"][0]["observed_quantity"],
        serde_json::json!(short.to_string()),
        "the replaced file did not reach the wallet: {text}"
    );
    assert_eq!(
        body["reconciliation"]["halted_venue_assets"],
        serde_json::json!(1),
        "{text}"
    );
    assert_eq!(
        body["reconciliation"]["outcomes"][0]["alert"]["cause"],
        serde_json::json!("delta_beyond_tolerance"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

#[test]
fn a_statement_file_that_stops_reading_refuses_the_cycle_rather_than_cycling_on_the_last_one()
-> Result<()> {
    // The premise is a cycle that ran with the file in place; then the file
    // goes, and the next cycle is refused naming the variable — the same
    // rule the feed follows, because a cycle over a statement the desk has
    // withdrawn reconciles a figure nobody stands behind.
    let directory = fixture_dir("vanished");
    let rig = rig()?;
    let equity = rig.initial_equity()?;
    let path = write_fixture(&directory, &desk_statement(equity));
    let (handler, _platform) = rig.with_feed(&path)?;
    assert_eq!(
        handler
            .handle(&request(Method::Post, "/cycle", ANALYST_TOKEN))
            .status,
        202
    );

    std::fs::remove_file(&path).expect("the fixture is removed");
    let response = handler.handle(&request(Method::Post, "/cycle", ANALYST_TOKEN));
    let (text, body) = body_of(response);
    assert_eq!(
        body["source"],
        serde_json::json!(STATEMENT_PATH_VARIABLE),
        "{text}"
    );
    let message = body["error"].as_str().expect("an error message");
    assert!(
        names(message, STATEMENT_PATH_VARIABLE),
        "the refusal does not name the variable: {text}"
    );
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

#[test]
fn a_caller_the_cycle_route_would_refuse_does_not_make_the_file_be_re_read() -> Result<()> {
    // The wrapper sits in front of the API's own authorisation, so it has to
    // apply the same ladder before it touches the file: a viewer, whom the
    // route table holds below the cycle, is refused by the API and the
    // platform holds exactly what it held. Premise: the file has changed,
    // so an analyst's cycle would have re-read it.
    let directory = fixture_dir("viewer");
    let rig = rig()?;
    let equity = rig.initial_equity()?;
    let path = write_fixture(&directory, &desk_statement(equity));
    let (handler, platform) = rig.with_feed(&path)?;
    let short = equity - Decimal::from_int(5);
    std::fs::write(&path, desk_statement(short)).expect("the fixture is rewritten");
    let file = std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("the fixture opens for writing");
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
        .expect("the modification time moves");
    drop(file);

    let response = handler.handle(&request(Method::Post, "/cycle", VIEWER_TOKEN));
    assert_eq!(response.status, 403);
    {
        let mut platform = platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        platform.run_cycle(start().saturating_add(Duration::from_secs(1)));
    }
    let (text, body) = wallet(&handler);
    assert_eq!(
        body["holdings"][0]["observed_quantity"],
        serde_json::json!(equity.to_string()),
        "a refused caller made the process re-read the statement: {text}"
    );
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

// --- refusals at start ------------------------------------------------------

#[test]
fn a_malformed_or_future_dated_statement_is_refused_naming_the_field() {
    // Each case is a file an operator could plausibly write, and each refusal
    // has to name the field so the fix is one edit rather than a search.
    // Nothing is clamped: the 257-holding statement is refused, not cut.
    let now = start();
    let future = now.saturating_add(Duration::from_secs(60)).to_rfc3339();
    let past = dated().to_rfc3339();
    let mut too_many = String::new();
    for index in 0..=MAX_STATEMENT_HOLDINGS {
        if index > 0 {
            too_many.push(',');
        }
        too_many.push_str(&format!(r#"{{"asset": "A{index}", "quantity": "1"}}"#));
    }
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "future as_of",
            format!(
                r#"{{"as_of": "{future}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "1"}}]}}"#
            ),
            "as_of",
        ),
        (
            "unparseable as_of",
            r#"{"as_of": "yesterday", "venue": "v", "tolerance": "1", "holdings": [{"asset": "USD", "quantity": "1"}]}"#
                .to_string(),
            "as_of",
        ),
        (
            "a float quantity",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": 0.1}}]}}"#
            ),
            "holdings[0].quantity",
        ),
        (
            "an integer quantity",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": 100}}]}}"#
            ),
            "holdings[0].quantity",
        ),
        (
            "a zero tolerance",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "0", "holdings": [{{"asset": "USD", "quantity": "1"}}]}}"#
            ),
            "tolerance",
        ),
        (
            "no tolerance anywhere",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "holdings": [{{"asset": "USD", "quantity": "1"}}]}}"#
            ),
            "holdings[0].tolerance",
        ),
        (
            "an unknown key",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "1", "tolerence": "2"}}]}}"#
            ),
            "\"tolerence\"",
        ),
        (
            "a duplicated asset",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "1"}}, {{"asset": "USD", "quantity": "2"}}]}}"#
            ),
            "holdings[1].asset",
        ),
        (
            "no holdings",
            format!(r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": []}}"#),
            "holdings",
        ),
        (
            "too many holdings",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": [{too_many}]}}"#
            ),
            "holdings",
        ),
        (
            "an empty venue",
            format!(
                r#"{{"as_of": "{past}", "venue": " ", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "1"}}]}}"#
            ),
            "venue",
        ),
    ];
    // Premise: the shape every case is a corruption of is accepted, so a
    // refusal below is of the corruption and not of the shape.
    let valid = format!(
        r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "1"}}]}}"#
    );
    let statement = Statement::parse(&valid, now).expect("the valid shape is accepted");
    assert_eq!(statement.holdings.len(), 1);

    for (label, text, field) in cases {
        let message = match Statement::parse(&text, now) {
            Ok(_) => panic!("{label} was accepted"),
            Err(error) => error.message().to_string(),
        };
        assert!(
            names(&message, field),
            "{label}: the refusal does not name {field}: {message}"
        );
    }

    // And through the feed, so the refusal the root prints names the
    // variable and the path as well as the field.
    let directory = fixture_dir("refused");
    let path = write_fixture(
        &directory,
        &format!(
            r#"{{"as_of": "{future}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "1"}}]}}"#
        ),
    );
    let message = match StatementFeed::open(&path, now) {
        Ok(_) => panic!("a future-dated statement file was opened"),
        Err(error) => error.message().to_string(),
    };
    assert!(
        names(&message, STATEMENT_PATH_VARIABLE) && names(&message, "as_of"),
        "the root's refusal names neither the variable nor the field: {message}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

// --- no feed ------------------------------------------------------------------

#[test]
fn an_unset_variable_leaves_the_wallet_unassembled_and_the_banner_says_there_is_no_feed()
-> Result<()> {
    // Premise: a cycle has run, so `assembled: false` below is "nothing was
    // observed" and not "no cycle yet" — the two answers the same body gives
    // for different reasons, and only the first is what an unset variable
    // should mean.
    let feed = StatementFeed::from_env(&|_| None, start())?;
    assert!(feed.is_none(), "an unset variable opened a feed");
    let empty = StatementFeed::from_env(
        &|name| (name == STATEMENT_PATH_VARIABLE).then(|| "  ".to_string()),
        start(),
    )?;
    assert!(empty.is_none(), "a blank variable opened a feed");

    let rig = rig()?;
    {
        let mut platform = rig
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        platform.run_cycle(start().saturating_add(Duration::from_secs(1)));
    }
    let (text, body) = wallet(&rig.api);
    assert_eq!(body["assembled"], serde_json::json!(false), "{text}");
    assert_eq!(body["reason"], serde_json::json!(NO_WALLET), "{text}");
    assert_eq!(body["holdings"], serde_json::json!([]), "{text}");

    let banner = absent_banner();
    assert!(
        names(&banner, STATEMENT_PATH_VARIABLE),
        "the banner does not name the variable: {banner}"
    );
    assert!(
        banner.starts_with("none (") && banner.contains("assembled: false"),
        "the banner does not say there is no feed and what /wallet answers: {banner}"
    );
    Ok(())
}
