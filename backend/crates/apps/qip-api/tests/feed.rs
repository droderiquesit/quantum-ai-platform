//! The API's SENSE stage: what `POST /cycle` observes before it runs the loop,
//! which selections the composition root refuses, and the licensing gate a
//! connector passes before any socket is opened.
//!
//! Every test asserts its premise first. The failure each guards is the one
//! this process lived with until the feed existed: a cycle that ran over a
//! platform nothing observed into, reported `traversed_every_stage: true`,
//! and left every research route honestly empty — a loop that looked busy
//! and was blind.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::feed::{
    ApiFeed, CONNECTOR_BASE_URL_VARIABLE, CONNECTOR_SOURCE_VARIABLE, ConnectorSettings,
    FeedSettings, TAPE_PATH_VARIABLE,
};
use qip_api::http::{Handler, Method, Request, Response};
use qip_api::routes::Api;
use qip_contracts::governance::Usage;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Context, Decimal, Duration, ManualClock, Timestamp};
use qip_data_finder::admission::CatalogueEntry;
use qip_data_finder::legal::{LicensingPosture, SourceLicense};
use qip_financial::quality::LicensingClass;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::Interval;
use qip_market_ingestion::tape::{SCHEMA_VERSION, TapeDocument, TapeObservation};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// --- fixtures ---------------------------------------------------------------

const ANALYST_TOKEN: &str = "analyst-token";

/// The wall clock the API, its credentials and a connector run on. After the
/// shipped Frankfurter fixture's reference date plus the ECB's sixteen-hour
/// publication delay, so a connector poll at this instant releases the table
/// rather than withholding it as not yet knowable.
fn wall() -> Timestamp {
    Timestamp::parse_rfc3339("2026-08-27T00:00:00Z").expect("a literal instant parses")
}

fn first_close() -> Timestamp {
    Timestamp::parse_rfc3339("2025-01-06T21:00:00Z").expect("a literal instant parses")
}

/// A daily tape of `days` bars on each instrument, written to a file under
/// a directory of its own. Publication trails the close by fifteen minutes.
fn tape_file(name: &str, instruments: &[&str], days: i64) -> (std::path::PathBuf, String) {
    let mut observations = Vec::new();
    for day in 0..days {
        let at = first_close().saturating_add(Duration::from_days(day));
        for (index, object_id) in instruments.iter().enumerate() {
            let price = 100 + index as i64 * 10 + day;
            let open = Decimal::from_int(price);
            observations.push(TapeObservation {
                object_id: (*object_id).to_string(),
                venue: "XNYS".to_string(),
                at: at.to_rfc3339(),
                known_at: at.saturating_add(Duration::from_mins(15)).to_rfc3339(),
                open,
                high: Decimal::from_int(price + 1),
                low: Decimal::from_int(price - 1),
                close: open,
                volume: Decimal::from_int(1_000),
            });
        }
    }
    let document = TapeDocument {
        schema_version: SCHEMA_VERSION,
        name: name.to_string(),
        description: "a feed test tape".to_string(),
        interval: Interval::Day,
        observations,
        macro_releases: Vec::new(),
        alternative_data: Vec::new(),
        dividend_declarations: Vec::new(),
    };
    let directory =
        std::env::temp_dir().join(format!("qip-api-feed-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("the directory is created");
    let path = directory.join("tape.json");
    std::fs::write(
        &path,
        serde_json::to_string(&document).expect("the tape serialises"),
    )
    .expect("the tape is written");
    (directory, path.display().to_string())
}

struct Rig {
    api: Api,
    platform: Arc<Mutex<Platform>>,
    /// The clock the platform was assembled on: the feed's where it owns
    /// one, a manual wall clock otherwise.
    platform_clock: Arc<ManualClock>,
}

/// An API whose platform is assembled the way the composition root
/// assembles it — on the feed's own clock when the feed owns one.
fn rig(feed: Option<ApiFeed>) -> Result<Rig> {
    let config = PlatformConfig::default();
    let platform_clock = match feed.as_ref().and_then(ApiFeed::owned_clock) {
        Some(tape_clock) => tape_clock,
        None => Arc::new(ManualClock::new(wall())),
    };
    let context = Context::new(platform_clock.clone(), config.seed);
    let platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;
    let platform = Arc::new(Mutex::new(platform));
    // Credentials, like the root's, live on the wall clock: a token issued
    // today must still open a tape from last year.
    let authenticator = Arc::new(Authenticator::new(vec![Credential::from_token(
        "analyst@example.com",
        Role::Analyst,
        ANALYST_TOKEN.to_string(),
        wall(),
        wall().saturating_add(Duration::from_days(30)),
    )]));
    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(60), 1000));
    let mut api = Api::new(
        platform.clone(),
        authenticator,
        rate_limiter,
        Arc::new(ManualClock::new(wall())),
    );
    if let Some(feed) = feed {
        api = api.with_feed(Arc::new(Mutex::new(feed)));
    }
    Ok(Rig {
        api,
        platform,
        platform_clock,
    })
}

impl Rig {
    fn with_platform<T>(&self, f: impl FnOnce(&Platform) -> T) -> Result<T> {
        let platform = self
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        Ok(f(&platform))
    }

    fn cycle(&self) -> Response {
        let mut headers = BTreeMap::new();
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {ANALYST_TOKEN}"),
        );
        self.api.handle(&Request {
            method: Method::Post,
            path: "/api/v1/cycle".to_string(),
            query: BTreeMap::new(),
            headers,
            body: Vec::new(),
            peer: "127.0.0.1:1".to_string(),
        })
    }

    fn closes_held(&self) -> Result<BTreeMap<String, usize>> {
        self.with_platform(|platform| {
            platform
                .price_history()
                .iter()
                .map(|(instrument, closes)| (instrument.clone(), closes.len()))
                .collect()
        })
    }
}

fn body(response: &Response) -> String {
    String::from_utf8(response.body.clone()).expect("a UTF-8 body")
}

/// Whether `text` carries `name` as a whole token. `contains` is a trap:
/// `QIP_CONNECTOR_SOURCE` is a substring of nothing here today, but a refusal
/// that named `QIP_CONNECTOR_SOURCE_FILE` tomorrow would satisfy it.
fn names_token(text: &str, name: &str) -> bool {
    text.split(|c: char| c.is_whitespace() || c == ',' || c == '.' || c == ';')
        .any(|token| token == name)
}

fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// The body the shipped Frankfurter fixture records from the live endpoint.
const RATE_TABLE: &str = r#"{"amount":1.0,"base":"EUR","date":"2026-08-24","rates":{"GBP":0.84215,"JPY":171.94,"USD":1.0827}}"#;

/// A loopback HTTP/1.1 server that answers every request with one JSON body
/// and counts the connections it accepted. What the licensing gate has to
/// prove is that a refused source opens *no* socket, and the count is the
/// only witness of that.
struct RateServer {
    url: String,
    served: Arc<AtomicUsize>,
}

impl RateServer {
    fn serving(body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let url = format!(
            "http://{}",
            listener
                .local_addr()
                .expect("the listener has a local address")
        );
        let served = Arc::new(AtomicUsize::new(0));
        let counter = served.clone();
        // Detached: the listener lives as long as the test process, and a
        // request that arrives after the test is over is answered and
        // ignored rather than left to hang the client.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while stream.read(&mut byte).is_ok_and(|n| n == 1) {
                    request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { url, served }
    }

    fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }
}

// --- the tape arm -------------------------------------------------------------

#[test]
fn the_tape_arm_observes_each_periods_records_into_the_platform_before_the_cycle_runs() -> Result<()>
{
    let (directory, path) = tape_file("observe", &["OBJ-A", "OBJ-B"], 3);
    let feed = ApiFeed::tape(&path)?;
    let tape_clock = feed.owned_clock().expect("a tape owns its clock");
    let rig = rig(Some(feed))?;

    // Premise: the platform holds no close, the platform's clock is the
    // tape's, and the tape has something to release. Without these the
    // assertions below would pass against a platform fed some other way.
    assert!(
        rig.closes_held()?.is_empty(),
        "the platform held closes before anything was sensed"
    );
    assert_eq!(rig.with_platform(Platform::cycle_count)?, 0);
    assert_eq!(rig.platform_clock.now(), tape_clock.now());
    let first_known_at = first_close().saturating_add(Duration::from_mins(15));
    assert_eq!(
        tape_clock.now(),
        first_known_at,
        "the tape clock did not start at the first knowable instant"
    );

    let response = rig.cycle();
    assert_eq!(response.status, 202, "{}", body(&response));
    let text = body(&response);
    // The response says what SENSE did, in the cycle's own body, so a caller
    // that ran a cycle can tell an observed period from an empty one.
    assert!(
        text.contains(r#""sense":{"source":"tape:observe","at":"2025-01-06T21:15:00.000Z","released":2,"observed":2,"rejected":0"#),
        "the cycle body does not report what was sensed: {text}"
    );
    assert!(
        text.contains(r#""cycle":1,"#),
        "the cycle did not run after sensing: {text}"
    );

    // The platform now holds exactly the first period: one close for each
    // of the two instruments, and nothing from a later day. This assertion,
    // not the body above, is what fails when the records are counted and
    // never handed over.
    let held = rig.closes_held()?;
    assert_eq!(
        held,
        [("OBJ-A".to_string(), 1), ("OBJ-B".to_string(), 1)]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        "the platform did not absorb the period the tape released"
    );
    assert_eq!(
        rig.platform_clock.now(),
        first_known_at,
        "the cycle ran at some instant other than the tape's"
    );

    // Two more periods, and the tape is spent; the fourth request is a
    // refusal rather than a cycle at a frozen instant.
    for expected in [2usize, 3] {
        let response = rig.cycle();
        assert_eq!(response.status, 202, "{}", body(&response));
        let held = rig.closes_held()?;
        assert_eq!(held.get("OBJ-A"), Some(&expected));
        assert_eq!(held.get("OBJ-B"), Some(&expected));
    }
    assert_eq!(rig.with_platform(Platform::cycle_count)?, 3);
    assert_eq!(
        rig.platform_clock.now(),
        first_known_at.saturating_add(Duration::from_days(2)),
        "three periods did not move the platform clock two days"
    );
    let spent = rig.cycle();
    assert_eq!(spent.status, 409, "{}", body(&spent));
    assert!(
        body(&spent).contains("spent"),
        "the refusal does not say the tape is spent: {}",
        body(&spent)
    );
    assert_eq!(
        rig.with_platform(Platform::cycle_count)?,
        3,
        "a spent tape still ran a cycle"
    );

    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

#[test]
fn a_process_with_no_feed_still_cycles_and_reports_no_sense_at_all() -> Result<()> {
    // The shipped state. Absent rather than `"observed":0`, for the same
    // reason `mesh` is absent: a zero would read as a source that went quiet.
    let rig = rig(None)?;
    let response = rig.cycle();
    assert_eq!(response.status, 202, "{}", body(&response));
    let text = body(&response);
    assert!(text.contains(r#""cycle":1,"#), "{text}");
    // The SENSE *stage* is still named in the stage list — it ran, and
    // said the platform is blind — so the absence asserted is the sense
    // object's, not the word's.
    assert!(
        text.contains(r#""stage":"sense""#),
        "the stage list no longer names SENSE; the assertion below would be vacuous: {text}"
    );
    assert!(
        !text.contains(r#""sense":{"#),
        "a process with no feed reported a sense outcome: {text}"
    );
    assert!(rig.closes_held()?.is_empty());
    Ok(())
}

#[test]
fn a_tape_that_outlasts_the_organisations_authorisation_is_refused_at_start_up() -> Result<()> {
    // The run that showed why: a 320-day daily tape convened its first panel
    // on tape day 103 and every agent was refused on every panel after,
    // reported as `failed`. Governance working as designed, read as a
    // defect, because nothing said the tape was too long. The assembled
    // organisation is asked directly, so the check cannot drift from the
    // roster's own review interval.
    let (long_directory, long) = tape_file("long", &["OBJ-A"], 100);
    let feed = ApiFeed::tape(&long)?;
    let long_rig = rig(Some(feed))?;
    let refusal = long_rig.with_platform(|platform| {
        ApiFeed::tape(&long)
            .expect("the tape opens")
            .refuse_tape_beyond_authorisation(platform)
    })?;
    let refusal = refusal.expect_err("a 100-day tape was admitted against a 90-day review");
    assert!(
        refusal.message().contains("re-reviewed") && refusal.message().contains("authorisation"),
        "the refusal does not say what lapses or what to do: {}",
        refusal.message()
    );
    let _ = std::fs::remove_dir_all(&long_directory);

    // And the premise on the other side: a tape inside the window is
    // admitted, or the gate refuses everything and proves nothing.
    let (short_directory, short) = tape_file("short", &["OBJ-A"], 10);
    let feed = ApiFeed::tape(&short)?;
    let short_rig = rig(Some(feed))?;
    short_rig
        .with_platform(|platform| {
            ApiFeed::tape(&short)
                .expect("the tape opens")
                .refuse_tape_beyond_authorisation(platform)
        })?
        .expect("a 10-day tape is inside a 90-day review interval");
    let _ = std::fs::remove_dir_all(&short_directory);
    Ok(())
}

// --- the selection ------------------------------------------------------------

#[test]
fn a_tape_and_a_connector_together_are_a_contradiction_refused_by_both_names() -> Result<()> {
    // Premise: each selection alone parses to its own arm, and nothing
    // configured is not an error — so the refusal below is the combination's
    // and not a parser that refuses everything.
    assert_eq!(FeedSettings::parse(&vars(&[]))?, FeedSettings::None);
    assert_eq!(
        FeedSettings::parse(&vars(&[(TAPE_PATH_VARIABLE, "/tmp/tape.json")]))?,
        FeedSettings::Tape("/tmp/tape.json".to_string())
    );
    assert_eq!(
        FeedSettings::parse(&vars(&[
            (CONNECTOR_SOURCE_VARIABLE, "frankfurter-ecb-reference-rates"),
            (CONNECTOR_BASE_URL_VARIABLE, "http://egress.test:8080"),
        ]))?,
        FeedSettings::Connector(ConnectorSettings {
            source_id: "frankfurter-ecb-reference-rates".to_string(),
            base_url: "http://egress.test:8080".to_string(),
        })
    );

    let refusal = FeedSettings::parse(&vars(&[
        (TAPE_PATH_VARIABLE, "/tmp/tape.json"),
        (CONNECTOR_SOURCE_VARIABLE, "frankfurter-ecb-reference-rates"),
        (CONNECTOR_BASE_URL_VARIABLE, "http://egress.test:8080"),
    ]))
    .expect_err("a tape and a connector on two clocks were opened as one feed");
    assert!(
        names_token(refusal.message(), TAPE_PATH_VARIABLE)
            && names_token(refusal.message(), CONNECTOR_SOURCE_VARIABLE),
        "the refusal does not name both variables: {}",
        refusal.message()
    );

    // Half a connector is refused by the name of the missing half, and a
    // vendor address over TLS by the name of what should be there instead.
    let half = FeedSettings::parse(&vars(&[(
        CONNECTOR_SOURCE_VARIABLE,
        "frankfurter-ecb-reference-rates",
    )]))
    .expect_err("a source with no egress address was accepted");
    assert!(
        names_token(half.message(), CONNECTOR_BASE_URL_VARIABLE),
        "{}",
        half.message()
    );
    let other_half = FeedSettings::parse(&vars(&[(
        CONNECTOR_BASE_URL_VARIABLE,
        "http://egress.test:8080",
    )]))
    .expect_err("an egress address with no source was accepted");
    assert!(
        names_token(other_half.message(), CONNECTOR_SOURCE_VARIABLE),
        "{}",
        other_half.message()
    );
    let tls = FeedSettings::parse(&vars(&[
        (CONNECTOR_SOURCE_VARIABLE, "frankfurter-ecb-reference-rates"),
        (CONNECTOR_BASE_URL_VARIABLE, "https://api.example.test"),
    ]))
    .expect_err("https was accepted by a transport with no TLS stack");
    assert!(tls.message().contains("egress proxy"), "{}", tls.message());
    Ok(())
}

// --- the licensing gate -------------------------------------------------------

#[test]
fn a_connector_whose_licensing_posture_is_not_evaluated_is_refused_before_any_socket_opens()
-> Result<()> {
    let server = RateServer::serving(RATE_TABLE);
    let settings = ConnectorSettings {
        source_id: "frankfurter-ecb-reference-rates".to_string(),
        base_url: server.url.clone(),
    };

    // Premise: the catalogue admits this source, the server answers, and the
    // whole path — gate, health probe, poll, observe, cycle — works. A gate
    // that refused everything would pass the refusals below and prove
    // nothing.
    let feed = ApiFeed::connector(&settings, 7, wall())?;
    let decision = feed
        .licensing_decision()
        .expect("a connector carries the decision that admitted it");
    assert_eq!(decision.licence, "ecb-reference-rates-via-frankfurter");
    assert_eq!(decision.class, LicensingClass::Public);
    assert_eq!(decision.usages, vec![Usage::Derive, Usage::Trade]);
    assert_eq!(decision.decided_at, wall());
    assert!(
        feed.describe()
            .contains("ecb-reference-rates-via-frankfurter"),
        "the banner line does not state the licensing decision: {}",
        feed.describe()
    );
    let probed = server.served();
    assert!(probed >= 1, "the admitted source opened no socket");
    let rig = rig(Some(feed))?;
    let response = rig.cycle();
    assert_eq!(response.status, 202, "{}", body(&response));
    let text = body(&response);
    assert!(
        text.contains(r#""sense":{"source":"frankfurter-ecb-reference-rates","at":"#)
            && text.contains(r#""released":3,"observed":3,"rejected":0"#),
        "the cycle did not observe the three currencies the rate table carries: {text}"
    );
    assert!(server.served() > probed, "the cycle polled nothing");
    let after_admitted = server.served();

    // An evaluated-but-undetermined posture: terms nobody located. Unknown
    // is not permission, and the refusal arrives with the socket count
    // unchanged — the transport was never constructed.
    let undetermined = vec![CatalogueEntry {
        source_id: "frankfurter-ecb-reference-rates",
        expected_class: LicensingClass::Public,
        posture: LicensingPosture::Undetermined,
    }];
    let refused = ApiFeed::connector_admitted_by(&undetermined, &settings, 7, wall())
        .expect_err("a source whose terms were never located was opened");
    assert!(
        refused.message().contains("undetermined"),
        "the refusal is not about the unevaluated posture: {}",
        refused.message()
    );
    assert_eq!(
        server.served(),
        after_admitted,
        "a refused source still opened a socket, so the gate ran after the transport"
    );

    // A research-only licence, which the rule's own example forbids from the
    // trading path: refused on the Trade question by name, and no socket.
    let research_only = vec![CatalogueEntry {
        source_id: "frankfurter-ecb-reference-rates",
        expected_class: LicensingClass::Public,
        posture: LicensingPosture::declared(SourceLicense::new(
            "research-only-terms",
            [Usage::Research, Usage::Derive],
        )?),
    }];
    let refused = ApiFeed::connector_admitted_by(&research_only, &settings, 7, wall())
        .expect_err("a research-only licence was admitted onto the trading path");
    assert!(
        refused.message().contains("for trade"),
        "the refusal was not about the trading usage: {}",
        refused.message()
    );
    assert_eq!(server.served(), after_admitted);

    // And a source the real catalogue has never evaluated at all.
    let unevaluated = ConnectorSettings {
        source_id: "some-unevaluated-endpoint".to_string(),
        base_url: server.url.clone(),
    };
    let refused = ApiFeed::connector(&unevaluated, 7, wall())
        .expect_err("an unevaluated source was opened, so its terms were never read");
    assert!(
        refused.message().contains("some-unevaluated-endpoint"),
        "{}",
        refused.message()
    );
    assert_eq!(server.served(), after_admitted);
    Ok(())
}
