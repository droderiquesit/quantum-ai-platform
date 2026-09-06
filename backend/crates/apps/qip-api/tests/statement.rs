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

/// A statement whose one holding carries its tolerance under `key`.
///
/// Two spellings of that key are the same length, which is what lets a test
/// replace the file's contents without moving its fingerprint.
fn keyed_statement(key: &str, quantity: Decimal) -> String {
    format!(
        r#"{{"as_of": "{}", "venue": "{DESK_VENUE}",
            "holdings": [{{"asset": "USD", "quantity": "{quantity}", "{key}": "1"}}]}}"#,
        dated().to_rfc3339()
    )
}

/// Move the file's modification time to `when`, so a change is visible
/// however coarse the filesystem's timestamps are.
fn touch(path: &str, when: std::time::SystemTime) {
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("the fixture opens for writing");
    file.set_modified(when)
        .expect("the modification time moves");
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
            // The row, not the key: a key is text whoever wrote the document
            // chose, so quoting it back publishes a piece of the document.
            // `no_refusal_of_a_statement_quotes_a_value_the_file_carried`
            // holds the other half — that the key itself does not appear.
            "an unknown key",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "1", "tolerence": "2"}}]}}"#
            ),
            "holdings[0]",
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

// --- a refusal never publishes the document ---------------------------------

/// A value no custodian would state, put in a fixture so its appearance in a
/// refusal is unambiguous. Anything a refusal echoes is echoed to the caller
/// of the route, to stderr, and to whichever ticket the line is pasted into.
const SENTINEL_QUANTITY: &str = "8675309.1234567";
const SENTINEL_ASSET: &str = "SENTINELASSET";
const SENTINEL_TEXT: &str = "SENTINELTEXT";
const SENTINEL_KEY: &str = "tolerence";

#[test]
fn no_refusal_of_a_statement_quotes_a_value_the_file_carried() {
    // A statement is a custodian's document about the desk's money. Every
    // refusal names the field, the row or the position and stops there —
    // the rule `ledger_views` already follows for an unknown body key. Each
    // case below carries a value nothing else could produce, and the test
    // asserts both halves: the refusal locates the fault, *and* the value is
    // absent. Naming the field alone would pass with the value beside it.
    let now = start();
    let past = dated().to_rfc3339();
    let future = now.saturating_add(Duration::from_secs(60)).to_rfc3339();
    // label, document, the field the refusal must name, what it must not say
    let cases: Vec<(&str, String, &str, Vec<String>)> = vec![
        (
            "an as_of that will not parse",
            format!(
                r#"{{"as_of": "{SENTINEL_TEXT}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "1"}}]}}"#
            ),
            "as_of",
            vec![SENTINEL_TEXT.to_string()],
        ),
        (
            "an as_of in the future",
            format!(
                r#"{{"as_of": "{future}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "1"}}]}}"#
            ),
            "as_of",
            vec![future.clone()],
        ),
        (
            "an as_of that is not a string",
            r#"{"as_of": 8675309, "venue": "v", "tolerance": "1", "holdings": [{"asset": "USD", "quantity": "1"}]}"#
                .to_string(),
            "as_of",
            vec!["8675309".to_string()],
        ),
        (
            "a venue that is not a string",
            format!(
                r#"{{"as_of": "{past}", "venue": 8675309, "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "1"}}]}}"#
            ),
            "venue",
            vec!["8675309".to_string()],
        ),
        (
            "a quantity written as a JSON number",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": {SENTINEL_QUANTITY}}}]}}"#
            ),
            "holdings[0].quantity",
            vec![SENTINEL_QUANTITY.to_string()],
        ),
        (
            "a quantity that is not a decimal",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "{SENTINEL_TEXT}"}}]}}"#
            ),
            "holdings[0].quantity",
            vec![SENTINEL_TEXT.to_string()],
        ),
        (
            "a tolerance that is not positive",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "-{SENTINEL_QUANTITY}", "holdings": [{{"asset": "USD", "quantity": "1"}}]}}"#
            ),
            "tolerance",
            vec![SENTINEL_QUANTITY.to_string()],
        ),
        (
            "a holding tolerance that is not positive",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "holdings": [{{"asset": "USD", "quantity": "1", "tolerance": "-{SENTINEL_QUANTITY}"}}]}}"#
            ),
            "holdings[0].tolerance",
            vec![SENTINEL_QUANTITY.to_string()],
        ),
        (
            "a misspelt key",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "USD", "quantity": "1", "{SENTINEL_KEY}": "2"}}]}}"#
            ),
            "holdings[0]",
            vec![SENTINEL_KEY.to_string()],
        ),
        (
            "an asset stated twice",
            format!(
                r#"{{"as_of": "{past}", "venue": "v", "tolerance": "1", "holdings": [{{"asset": "{SENTINEL_ASSET}", "quantity": "1"}}, {{"asset": "{SENTINEL_ASSET}", "quantity": "2"}}]}}"#
            ),
            "holdings[1].asset",
            vec![SENTINEL_ASSET.to_string()],
        ),
        (
            "a document that is not JSON at all",
            format!(r#"{{"as_of": "{past}", "venue": "{SENTINEL_TEXT}" "#),
            "line",
            vec![SENTINEL_TEXT.to_string()],
        ),
    ];

    for (label, text, field, forbidden) in cases {
        // Premise, and the one that matters: the value really is in the
        // document. Asserting its absence from a message when it was never
        // in the file would pass for ever and guard nothing.
        for value in &forbidden {
            assert!(
                text.contains(value.as_str()),
                "{label}: the fixture does not carry {value}, so its absence proves nothing"
            );
        }
        let message = match Statement::parse(&text, now) {
            Ok(_) => panic!("{label} was accepted"),
            Err(error) => error.message().to_string(),
        };
        assert!(
            names(&message, field),
            "{label}: the refusal does not name {field}: {message}"
        );
        for value in &forbidden {
            assert!(
                !message.contains(value.as_str()),
                "{label}: the refusal quotes a value the file carried: {message}"
            );
        }
    }
}

#[test]
fn a_cycle_refused_by_a_broken_statement_does_not_publish_the_file_into_the_response() -> Result<()>
{
    // The 503 reaches whoever can call the route, and the same line goes to
    // stderr. Premise first: the cycle ran while the file was good, so the
    // refusal below is of the replacement and not of the route.
    let directory = fixture_dir("no-echo");
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

    let broken = format!(
        r#"{{"as_of": "{}", "venue": "{DESK_VENUE}", "tolerance": "1",
            "holdings": [{{"asset": "{SENTINEL_ASSET}", "quantity": {SENTINEL_QUANTITY}}}]}}"#,
        dated().to_rfc3339()
    );
    // Premise: the file carries both values, so their absence below is the
    // refusal's doing.
    assert!(broken.contains(SENTINEL_ASSET) && broken.contains(SENTINEL_QUANTITY));
    write_fixture(&directory, &broken);
    touch(
        &path,
        std::time::SystemTime::now() + std::time::Duration::from_secs(2),
    );

    let response = handler.handle(&request(Method::Post, "/cycle", ANALYST_TOKEN));
    assert_eq!(response.status, 503);
    let (text, body) = body_of(response);
    let message = body["error"].as_str().expect("an error message");
    assert!(
        names(message, STATEMENT_PATH_VARIABLE) && names(message, "holdings[0].quantity"),
        "the refusal does not locate the fault: {text}"
    );
    assert!(
        !text.contains(SENTINEL_QUANTITY),
        "the refusal published a figure the statement carried: {text}"
    );
    assert!(
        !text.contains(SENTINEL_ASSET),
        "the refusal published an asset the statement carried: {text}"
    );
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

// --- a broken file is refused, and cheaply --------------------------------

#[test]
fn a_statement_file_that_stays_broken_is_refused_without_being_read_again_and_a_fix_needs_no_restart()
-> Result<()> {
    // A broken file refuses every cycle — that is not softened. What must not
    // happen is re-reading and re-parsing the same bytes to reach the same
    // answer. The proof is indirect and exact: after the refusal, the file is
    // put back to something that parses *without moving its modification time
    // or its length*, and the cycle is refused all the same. Only a feed that
    // did not read the file can answer that way. Then the timestamp moves and
    // the fix is picked up with no restart, which is the defect a cache that
    // never re-checked would introduce.
    let directory = fixture_dir("broken-cache");
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

    // The two documents differ by one letter inside a key, so they are the
    // same length by construction — asserted, because the whole test rests
    // on the fingerprint being identical.
    let later = equity - Decimal::from_int(5);
    let good = keyed_statement("tolerance", later);
    let bad = keyed_statement(SENTINEL_KEY, later);
    assert_eq!(
        good.len(),
        bad.len(),
        "the two documents are not the same length"
    );

    let frozen = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    write_fixture(&directory, &bad);
    touch(&path, frozen);
    let response = handler.handle(&request(Method::Post, "/cycle", ANALYST_TOKEN));
    assert_eq!(response.status, 503);
    let (_, body) = body_of(response);
    let first = body["error"]
        .as_str()
        .expect("an error message")
        .to_string();

    // The file now parses. Nothing else about it moved.
    write_fixture(&directory, &good);
    touch(&path, frozen);
    assert!(
        Statement::parse(&good, start()).is_ok(),
        "the premise fails: the replacement does not parse"
    );
    let metadata = std::fs::metadata(&path).expect("the fixture is there");
    assert_eq!(metadata.len() as usize, bad.len(), "the length moved");
    assert_eq!(
        metadata.modified().expect("a modification time"),
        frozen,
        "the modification time moved"
    );

    let response = handler.handle(&request(Method::Post, "/cycle", ANALYST_TOKEN));
    assert_eq!(
        response.status, 503,
        "the feed read a file whose fingerprint had not moved"
    );
    let (text, body) = body_of(response);
    assert_eq!(
        body["error"].as_str().expect("an error message"),
        first,
        "the second refusal is not the one the feed already gave: {text}"
    );

    // The operator's fix, seen: the same bytes, a moved timestamp.
    touch(&path, frozen + std::time::Duration::from_secs(2));
    let response = handler.handle(&request(Method::Post, "/cycle", ANALYST_TOKEN));
    assert_eq!(
        response.status,
        202,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let (text, wallet_body) = wallet(&handler);
    assert_eq!(
        wallet_body["holdings"][0]["observed_quantity"],
        serde_json::json!(later.to_string()),
        "the fixed file did not reach the wallet: {text}"
    );
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

// --- the platform is not held across the disk -------------------------------

#[test]
fn a_broken_statement_refuses_the_cycle_without_waiting_for_the_platform_lock() -> Result<()> {
    // The re-read is a filesystem call. Holding the kernel's lock across it
    // made every other request — every `/wallet`, every stream poll — wait on
    // a disk that had nothing to do with them, and made a refusal that never
    // needs the platform at all queue behind whatever held it. Here the test
    // holds the platform and the refusal must still be answered.
    let directory = fixture_dir("lock");
    let rig = rig()?;
    let equity = rig.initial_equity()?;
    let path = write_fixture(&directory, &desk_statement(equity));
    let (handler, platform) = rig.with_feed(&path)?;
    // Premise: with the file good and the platform free, the cycle is served.
    assert_eq!(
        handler
            .handle(&request(Method::Post, "/cycle", ANALYST_TOKEN))
            .status,
        202
    );
    write_fixture(&directory, "{ this is not a statement");
    touch(
        &path,
        std::time::SystemTime::now() + std::time::Duration::from_secs(2),
    );

    let held = platform
        .lock()
        .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let answered = std::thread::scope(|scope| {
        scope.spawn(|| {
            let response = handler.handle(&request(Method::Post, "/cycle", ANALYST_TOKEN));
            let _ = sender.send(response.status);
        });
        let answered = receiver.recv_timeout(std::time::Duration::from_secs(10));
        // Released whatever happened, so a failure is a failed assertion and
        // never a suite that hangs.
        drop(held);
        answered
    });
    let status = answered.map_err(|_| {
        Error::invalid(
            "the cycle was not refused while the platform lock was held; the statement re-read \
             is waiting on the platform",
        )
    })?;
    assert_eq!(status, 503);
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
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
