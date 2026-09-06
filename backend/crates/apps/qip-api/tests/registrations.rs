//! The venue-registration surface: `GET /registrations` and
//! `POST /registrations/{source}/approve`, against the contract in
//! `ROUTES-REGISTRATIONS.md`, and the feed's registration gate.
//!
//! Every test asserts its premise before the property: that the source
//! *was* pending, that the viewer *can* read, that the operator *is*
//! accepted with a good body — so a refusal below is about the thing under
//! test and not about a route that never answered. The failure the whole
//! file guards is a credential with nobody's name on it, and its two
//! shadows: a name a caller chose rather than authenticated, and a key
//! pasted where a variable name belongs and echoed back in a refusal.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::feed::{ApiFeed, ConnectorSettings};
use qip_api::http::{Handler, Method, Request, Response};
use qip_api::registration_views::{POSTURE, secret_command};
use qip_api::routes::{Api, ROUTES};
use qip_contracts::governance::Usage;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, ManualClock, ObjectId, dec};
use qip_data_finder::admission::CatalogueEntry;
use qip_data_finder::legal::{LicensingPosture, SourceLicense};
use qip_data_finder::registration::{
    NOT_OFFERED, RegistrationRecord, RegistrationRegistry, RegistrationStanding,
};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{LicensingClass, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_market_ingestion::connector::manifest::SecretRef;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

// --- fixtures ---------------------------------------------------------------

const OPERATOR_TOKEN: &str = "operator-token";
const VIEWER_TOKEN: &str = "viewer-token";
const OPERATOR_SUBJECT: &str = "operator@example.com";
const INSTRUMENT: &str = "obj-AAA";

/// A source the shipped table says needs an account.
const ACCOUNT_SOURCE: &str = "alpaca-daily-bars";
const ACCOUNT_SLOT: &str = "QIP_ALPACA_API_SECRET_KEY";
const ACCOUNT_COMPANION_SLOT: &str = "QIP_ALPACA_API_KEY_ID";
const TERMS: &str = "https://alpaca.markets/terms-and-conditions";
/// A source the shipped table says is keyless.
const KEYLESS_SOURCE: &str = "coinbase-spot-ticker";

const APPROVE_PATH: &str = "/registrations/alpaca-daily-bars/approve";

/// The operator-role list that carries the credential slots.
const SLOTS_PATH: &str = "/registrations/slots";

/// The prefix of every deployment variable this platform reads a credential
/// under. What a viewer's list must not contain anywhere in its bytes.
const SLOT_PREFIX: &str = "QIP_";

/// The head of the one command that writes a credential. The whole point of
/// keeping it off the viewer's list: it names the secret and the way to fill
/// it in one line a person can paste.
const WRITE_COMMAND: &str = "gcloud secrets versions add";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn universe() -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string(INSTRUMENT),
            "AAA",
            InstrumentType::CommonStock,
            fixture_liquidity(),
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())?,
    )?;
    Ok(universe)
}

fn limits() -> LimitSet {
    LimitSet::new("registration-routes-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

struct Rig {
    api: Api,
    platform: Arc<Mutex<Platform>>,
    /// Held so a test can move the clock past the credential window. The
    /// freshness gate is a comparison against the session's issue instant, so
    /// nothing can exercise it without a clock the test can advance.
    clock: Arc<ManualClock>,
}

fn rig() -> Result<Rig> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock.clone(), config.seed);
    let platform = Platform::new(config, context, Telemetry::silent(), universe()?, limits())?;
    let platform = Arc::new(Mutex::new(platform));
    let authenticator = Arc::new(Authenticator::new(vec![
        Credential::from_token(
            OPERATOR_SUBJECT,
            Role::Operator,
            OPERATOR_TOKEN.to_string(),
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
        api: Api::new(platform.clone(), authenticator, rate_limiter, clock.clone()),
        platform,
        clock,
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

    fn call(&self, method: Method, path: &str, token: &str, body: &str) -> Response {
        let mut headers = BTreeMap::new();
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
        self.api.handle(&Request {
            method,
            path: format!("/api/v1{path}"),
            query: BTreeMap::new(),
            headers,
            body: body.as_bytes().to_vec(),
            peer: "127.0.0.1:1".to_string(),
        })
    }

    fn list(&self, token: &str) -> Response {
        self.call(Method::Get, "/registrations", token, "")
    }

    /// The operator list: the same rows with the credential slots beside
    /// them.
    fn slots(&self, token: &str) -> Response {
        self.call(Method::Get, SLOTS_PATH, token, "")
    }

    fn approve(&self, token: &str, body: &str) -> Response {
        self.call(Method::Post, APPROVE_PATH, token, body)
    }

    /// The registry's own answer for the account source, through the kernel.
    fn standing(&self) -> Result<std::result::Result<RegistrationStanding, Error>> {
        self.with_platform(|platform| platform.registration_standing(ACCOUNT_SOURCE))
    }
}

fn body_of(response: Response) -> (String, serde_json::Value) {
    let text = String::from_utf8(response.body).expect("a UTF-8 body");
    let value = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"));
    (text, value)
}

/// The row of the list for one source.
fn row<'a>(list: &'a serde_json::Value, source_id: &str) -> &'a serde_json::Value {
    list["sources"]
        .as_array()
        .expect("sources is a list")
        .iter()
        .find(|row| row["source_id"] == source_id)
        .unwrap_or_else(|| panic!("{source_id} is not in the list: {list}"))
}

fn good_body() -> String {
    serde_json::json!({ "terms": TERMS, "secret": ACCOUNT_SLOT }).to_string()
}

// --- the approval -------------------------------------------------------------

#[test]
fn an_approval_by_an_operator_moves_a_pending_source_to_registered_and_the_journal_replays_it()
-> Result<()> {
    let rig = rig()?;

    // Premise: the list says the account source is pending and names who
    // must register, and the kernel agrees.
    let (_, before) = body_of(rig.list(VIEWER_TOKEN));
    let pending = row(&before, ACCOUNT_SOURCE);
    assert_eq!(pending["requirement"], "account");
    assert_eq!(pending["standing"]["standing"], "pending");
    assert_eq!(
        pending["standing"]["who_must_register"],
        rig.with_platform(|platform| platform.config().owner.clone())?
    );
    assert!(
        pending["standing"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains(NOT_OFFERED)),
        "{pending}"
    );
    assert!(rig.standing()?.is_err());

    let response = rig.approve(OPERATOR_TOKEN, &good_body());
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let (text, approved) = body_of(response);
    assert_eq!(approved["posture"], POSTURE);
    assert_eq!(approved["source_id"], ACCOUNT_SOURCE);
    assert_eq!(approved["standing"]["standing"], "registered");
    // The operator is the authenticated subject, not anything the body said
    // — the body named nobody.
    assert_eq!(approved["standing"]["operator"], OPERATOR_SUBJECT);
    assert_eq!(approved["standing"]["secret"], ACCOUNT_SLOT);
    assert_eq!(approved["standing"]["terms_read_at"], start().to_rfc3339());
    // The body names the variable and nothing that could be a value: the
    // only `secret` key in it is the slot name.
    assert_eq!(text.matches("\"secret\"").count(), 1, "{text}");

    // The kernel holds the record, the list now says registered, and the
    // event log alone rebuilds the registry the platform is acting on.
    match rig
        .standing()?
        .map_err(|error| Error::invalid(error.message()))?
    {
        RegistrationStanding::Registered { record } => {
            assert_eq!(record.operator(), OPERATOR_SUBJECT);
            assert_eq!(record.terms(), TERMS);
        }
        RegistrationStanding::Keyless => panic!("an account source stood as keyless"),
    }
    let (_, after) = body_of(rig.list(VIEWER_TOKEN));
    assert_eq!(
        row(&after, ACCOUNT_SOURCE)["standing"]["operator"],
        OPERATOR_SUBJECT
    );
    rig.with_platform(|platform| -> Result<()> {
        let replayed = platform.replay_registrations()?;
        assert_eq!(&replayed, platform.registrations());
        assert_eq!(
            replayed
                .record(ACCOUNT_SOURCE)
                .map(RegistrationRecord::operator),
            Some(OPERATOR_SUBJECT)
        );
        Ok(())
    })??;
    Ok(())
}

#[test]
fn a_viewer_cannot_approve_a_registration() -> Result<()> {
    let rig = rig()?;

    // Premise: the viewer's credential is good — it reads the list — and the
    // route exists at operator: the table says so, and an operator with the
    // same body is admitted (proven in the test above; here the table).
    assert_eq!(rig.list(VIEWER_TOKEN).status, 200);
    let route = ROUTES
        .iter()
        .find(|route| {
            route.method == Method::Post && route.pattern == "/registrations/:source/approve"
        })
        .expect("the approval route is in the table");
    assert_eq!(route.required_role, Role::Operator);

    let response = rig.approve(VIEWER_TOKEN, &good_body());
    assert_eq!(
        response.status,
        403,
        "{}",
        String::from_utf8_lossy(&response.body)
    );

    // Nothing moved: the source is still pending and the log holds no
    // registration.
    assert!(rig.standing()?.is_err());
    rig.with_platform(|platform| -> Result<()> {
        assert!(platform.registrations().record(ACCOUNT_SOURCE).is_none());
        assert_eq!(
            &platform.replay_registrations()?,
            &RegistrationRegistry::shipped()
        );
        Ok(())
    })??;
    Ok(())
}

#[test]
fn a_blank_or_key_shaped_secret_is_refused_and_echoed_nowhere() -> Result<()> {
    let rig = rig()?;
    // Three shapes a pasted credential takes, and a blank.
    let pasted = [
        "sk-live-9f2a7c1e4b8d",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "QIP_ALPACA_API_SECRET_KEY=abc123",
    ];

    // Premise: the same body with the slot name is what the route accepts
    // (the approval test proves it end to end); here, that each refused
    // body differs from the accepted one only in `secret`.
    let accepted: serde_json::Value = serde_json::from_str(&good_body())?;
    for value in pasted {
        let mut body = accepted.clone();
        body["secret"] = serde_json::Value::String(value.to_string());
        assert_ne!(body, accepted);

        let response = rig.approve(OPERATOR_TOKEN, &body.to_string());
        let (text, refusal) = body_of(response);
        assert!(
            refusal["error"]
                .as_str()
                .is_some_and(|reason| reason.contains("not a deployment variable name")),
            "the refusal is not the shape screen's: {text}"
        );
        // Echoed nowhere: not in the refusal, and not in the event log.
        assert!(
            !text.contains(value),
            "the refusal echoed the value: {text}"
        );
        let log_text = rig.with_platform(|platform| {
            serde_json::to_string(platform.event_log().records()).unwrap_or_default()
        })?;
        assert!(
            !log_text.contains(value),
            "the event log carries the refused value"
        );
    }
    // The refusal status is the caller's to fix.
    assert_eq!(
        rig.approve(
            OPERATOR_TOKEN,
            &serde_json::json!({ "terms": TERMS, "secret": pasted[0] }).to_string()
        )
        .status,
        400
    );

    // Blank secret, blank terms, and each missing: refused by name.
    for (body, names) in [
        (
            serde_json::json!({ "terms": TERMS, "secret": "" }),
            "`secret` is blank",
        ),
        (
            serde_json::json!({ "terms": "   ", "secret": ACCOUNT_SLOT }),
            "`terms` is blank",
        ),
        (serde_json::json!({ "terms": TERMS }), "no `secret`"),
        (serde_json::json!({ "secret": ACCOUNT_SLOT }), "no `terms`"),
    ] {
        let response = rig.approve(OPERATOR_TOKEN, &body.to_string());
        assert_eq!(response.status, 400, "{body}");
        let (text, refusal) = body_of(response);
        assert!(
            refusal["error"]
                .as_str()
                .is_some_and(|reason| reason.contains(names)),
            "{text}"
        );
    }
    // And a body that is not JSON at all.
    assert_eq!(rig.approve(OPERATOR_TOKEN, "not json").status, 400);

    // Nothing moved.
    assert!(rig.standing()?.is_err());
    rig.with_platform(|platform| -> Result<()> {
        assert_eq!(
            &platform.replay_registrations()?,
            &RegistrationRegistry::shipped()
        );
        Ok(())
    })??;
    Ok(())
}

// --- the operator's list ------------------------------------------------------

#[test]
fn the_operator_list_names_the_command_for_each_slot_and_the_posture() -> Result<()> {
    let rig = rig()?;
    let response = rig.slots(OPERATOR_TOKEN);
    assert_eq!(response.status, 200);
    let (text, list) = body_of(response);

    // The posture, first and verbatim.
    assert_eq!(list["posture"], POSTURE);
    assert!(
        text.starts_with(&format!(r#"{{"posture":"{POSTURE}""#)),
        "{text}"
    );
    assert_eq!(list["served_at"], start().to_rfc3339());

    // Premise: every source the finder catalogues is a row, so a source
    // missing below is missing from the list and not from the catalogue.
    let catalogue = qip_data_finder::admission::catalogue()?;
    assert!(!catalogue.is_empty());
    for entry in &catalogue {
        row(&list, entry.source_id);
    }

    // The account source: the slot the manifest reads the credential under,
    // the one line that fills it, the companion the manifest also reads,
    // and the terms the catalogue sends an operator to.
    let account = row(&list, ACCOUNT_SOURCE);
    assert_eq!(account["secret_slot"], ACCOUNT_SLOT);
    assert_eq!(
        account["secret_command"],
        "gcloud secrets versions add qip-alpaca-api-secret-key --data-file=-"
    );
    assert_eq!(account["secret_command"], secret_command(ACCOUNT_SLOT));
    assert_eq!(
        account["companion_secret_slots"][0]["variable"],
        ACCOUNT_COMPANION_SLOT
    );
    assert_eq!(
        account["companion_secret_slots"][0]["secret_command"],
        "gcloud secrets versions add qip-alpaca-api-key-id --data-file=-"
    );
    assert_eq!(account["terms"], TERMS);

    // The keyless source: no slot, no command, standing keyless, and the
    // licence identifier as its terms reference.
    let keyless = row(&list, KEYLESS_SOURCE);
    assert_eq!(keyless["requirement"], "keyless");
    assert_eq!(
        keyless["standing"],
        serde_json::json!({ "standing": "keyless" })
    );
    assert!(keyless["secret_slot"].is_null(), "{keyless}");
    assert!(keyless["secret_command"].is_null(), "{keyless}");
    assert_eq!(keyless["companion_secret_slots"], serde_json::json!([]));
    assert_eq!(keyless["terms"], "coinbase-exchange-market-data-terms");

    // No value anywhere: every `secret` in the body is a variable name.
    for value in text.split('"').filter(|token| token.starts_with("QIP_")) {
        SecretRef::new(value)?;
    }
    Ok(())
}

// --- the viewer's list, and what it must not carry ----------------------------

#[test]
fn a_viewer_is_refused_the_operator_slot_list_and_the_refusal_names_no_slot() -> Result<()> {
    let rig = rig()?;

    // Premise one: the viewer's credential is good on this surface — it
    // reads the standings list — so the refusal below is about the route and
    // not about a credential that works nowhere.
    assert_eq!(rig.list(VIEWER_TOKEN).status, 200);

    // Premise two, from the table rather than from memory: the slot list is
    // declared at the operator role. A test that only drove the handler
    // would pass just as well if the row said `Role::Viewer` and the 403
    // came from somewhere else.
    let route = ROUTES
        .iter()
        .find(|route| route.method == Method::Get && route.pattern == SLOTS_PATH)
        .expect("the slot list is in the route table");
    assert_eq!(route.required_role, Role::Operator);

    // Premise three: an operator is admitted, so the route is reachable and
    // the viewer's refusal is the authorisation.
    let allowed = rig.slots(OPERATOR_TOKEN);
    assert_eq!(allowed.status, 200);
    let (allowed_text, _) = body_of(allowed);
    assert!(allowed_text.contains(ACCOUNT_SLOT), "{allowed_text}");

    let refused = rig.slots(VIEWER_TOKEN);
    assert_eq!(
        refused.status,
        403,
        "{}",
        String::from_utf8_lossy(&refused.body)
    );
    // A refusal that named what it was withholding would withhold nothing.
    let (refused_text, _) = body_of(refused);
    assert!(!refused_text.contains(SLOT_PREFIX), "{refused_text}");
    assert!(!refused_text.contains(WRITE_COMMAND), "{refused_text}");
    Ok(())
}

#[test]
fn the_viewer_list_carries_the_standing_and_never_a_slot_or_the_command_that_fills_it() -> Result<()>
{
    let rig = rig()?;

    // The premise this test exists for, asserted rather than assumed: the
    // material really is in this process and really is on a list — the
    // operator's. Without this the assertions below would pass just as well
    // against a build whose manifests declared no credential at all, which
    // is the cheapest kind of green.
    let (operator_text, operator_list) = body_of(rig.slots(OPERATOR_TOKEN));
    assert!(operator_text.contains(SLOT_PREFIX), "{operator_text}");
    assert!(operator_text.contains(WRITE_COMMAND), "{operator_text}");
    assert_eq!(
        row(&operator_list, ACCOUNT_SOURCE)["secret_slot"],
        ACCOUNT_SLOT
    );

    // What a viewer is answered with. This was reproduced on the wire
    // against the built binary before the split existed: a viewer token on
    // `GET /api/v1/registrations` came back naming
    // `QIP_ALPACA_API_SECRET_KEY` and the `gcloud secrets versions add` line
    // that writes it — the slot a credential lives in and the exact command
    // to put one there, served to a role whose whole authority is reading
    // what the platform decided.
    let response = rig.list(VIEWER_TOKEN);
    assert_eq!(response.status, 200);
    let (text, list) = body_of(response);
    assert!(!text.contains(SLOT_PREFIX), "{text}");
    assert!(!text.contains(WRITE_COMMAND), "{text}");
    // The keys themselves are gone, not emptied. A `"secret_slot": null` on
    // every row would read as a platform that reads no credentials.
    // Matched with the quotes and the colon, because `secret_command` has
    // `secret_slot`'s neighbours in it and a bare substring check would pass
    // on the wrong field.
    for key in ["secret_slot", "secret_command", "companion_secret_slots"] {
        assert!(!text.contains(&format!(r#""{key}":"#)), "{key}: {text}");
    }

    // And it is still the list: every catalogued source, its requirement,
    // its standing and its terms. A route that answered nothing would also
    // contain no slot.
    let catalogue = qip_data_finder::admission::catalogue()?;
    assert!(!catalogue.is_empty());
    for entry in &catalogue {
        row(&list, entry.source_id);
    }
    assert_eq!(list["posture"], POSTURE);
    let account = row(&list, ACCOUNT_SOURCE);
    assert_eq!(account["requirement"], "account");
    assert_eq!(account["standing"]["standing"], "pending");
    assert_eq!(account["terms"], TERMS);
    assert_eq!(
        row(&list, KEYLESS_SOURCE)["standing"],
        serde_json::json!({ "standing": "keyless" })
    );

    // The half that a fix to the row alone would have left open. The
    // registered standing carries the variable the *record* names, so before
    // this test a viewer saw no slot for a pending source and saw one the
    // moment an operator approved it. Approve, then read the viewer's list
    // again.
    assert_eq!(rig.approve(OPERATOR_TOKEN, &good_body()).status, 200);
    let (after_text, after) = body_of(rig.list(VIEWER_TOKEN));
    let registered = row(&after, ACCOUNT_SOURCE);
    assert_eq!(registered["standing"]["standing"], "registered");
    assert_eq!(registered["standing"]["operator"], OPERATOR_SUBJECT);
    assert!(registered["standing"]["secret"].is_null(), "{registered}");
    assert!(!after_text.contains(SLOT_PREFIX), "{after_text}");
    assert!(!after_text.contains(WRITE_COMMAND), "{after_text}");

    // The operator's list still has it, from the record the kernel adopted.
    let (_, operator_after) = body_of(rig.slots(OPERATOR_TOKEN));
    assert_eq!(
        row(&operator_after, ACCOUNT_SOURCE)["standing"]["secret"],
        ACCOUNT_SLOT
    );
    Ok(())
}

// --- the feed's gate ----------------------------------------------------------

#[test]
fn the_feed_refuses_an_account_source_nobody_registered_and_admits_it_on_the_owners_record()
-> Result<()> {
    // A catalogue entry the real catalogue does not yet contain: the account
    // source with its terms read and every usage granted, so the licensing
    // questions pass and the only gate left is the registration one.
    let terms_read = vec![CatalogueEntry {
        source_id: ACCOUNT_SOURCE,
        expected_class: LicensingClass::Restricted,
        posture: LicensingPosture::declared(SourceLicense::new(
            "alpaca-terms-as-read",
            [Usage::Research, Usage::Derive, Usage::Trade],
        )?),
    }];
    // Nothing listens here; a gate that let the source through would fail
    // on the socket, with a different refusal.
    let settings = ConnectorSettings {
        source_id: ACCOUNT_SOURCE.to_string(),
        base_url: "http://127.0.0.1:9".to_string(),
    };
    // Premise: the shipped registry needs a record for this source and
    // holds none.
    let shipped = RegistrationRegistry::shipped();
    assert!(
        shipped
            .requirement(ACCOUNT_SOURCE)
            .is_some_and(|requirement| requirement.needs_registration())
    );
    assert!(shipped.record(ACCOUNT_SOURCE).is_none());

    let refused =
        ApiFeed::connector_admitted_by_registered(&terms_read, &shipped, &settings, 7, start())
            .expect_err("a source needing an account was opened with nobody registered");
    assert!(
        refused.message().contains(NOT_OFFERED),
        "the refusal is not the registration gate's: {}",
        refused.message()
    );
    // The door the composition root does not take says the same.
    let refused = ApiFeed::connector_admitted_by(&terms_read, &settings, 7, start())
        .expect_err("the shipped-registry door opened an account source");
    assert!(refused.message().contains(NOT_OFFERED));

    // With the owner's record — the registry a configuration stands for —
    // the registration gate passes. Whatever happens after is the socket's
    // business and is not the registration refusal.
    let config = PlatformConfig::default().with_venue_registrations(vec![RegistrationRecord::new(
        ACCOUNT_SOURCE,
        "desk-owner",
        start(),
        TERMS,
        SecretRef::new(ACCOUNT_SLOT)?,
    )?]);
    let registered = config.registration_registry()?;
    match ApiFeed::connector_admitted_by_registered(&terms_read, &registered, &settings, 7, start())
    {
        Ok(feed) => {
            let decision = feed
                .licensing_decision()
                .expect("a connector carries its decision");
            assert!(
                decision.describe().contains("registered by desk-owner"),
                "{}",
                decision.describe()
            );
        }
        Err(error) => assert!(
            !error.message().contains(NOT_OFFERED),
            "the owner's record did not satisfy the gate: {}",
            error.message()
        ),
    }
    Ok(())
}

/// A session older than the kernel's credential window cannot approve.
///
/// This is the gate that could not fire. The route built its
/// `OperatorIdentity` with `authenticated_at = now`, so
/// `is_fresh(now, REGISTRATION_CREDENTIAL_AGE)` measured the age of the
/// identity it had just stamped and got zero every time. Fifteen minutes was
/// enforced by a comparison whose two sides were the same value. Nothing
/// caught it because every test approved at the instant the rig issued the
/// token, where a correct implementation and a broken one agree.
///
/// So this test asserts both halves, which is what distinguishes a working
/// gate from one that refuses everything: the same operator, the same body
/// and the same source are refused once the session has aged past the window
/// and accepted while it is inside it.
#[test]
fn a_session_older_than_the_credential_window_cannot_approve_a_registration() -> Result<()> {
    let aged = rig()?;
    // Premise: the source really is pending, so a refusal below is the
    // freshness gate rather than a source that was never approvable.
    assert!(
        aged.standing()?.is_err(),
        "the source is not pending before the approval, so this test would pass whatever the \
         gate did"
    );

    // One second past the window. The credential itself is good for thirty
    // days, so what expires here is the operator's *authentication*, not the
    // token — the two are different clocks and only one of them is the gate.
    aged.clock.advance(Duration::from_secs(15 * 60 + 1));
    let stale = aged.approve(OPERATOR_TOKEN, &good_body());
    let stale_text = String::from_utf8_lossy(&stale.body).to_string();
    assert_ne!(
        stale.status, 200,
        "a session {} past the fifteen-minute window approved a registration: {stale_text}",
        "one second"
    );
    // The refusal says what to do instead, and the registry did not move.
    // A refusal names what to do instead. Matched on the delimited window
    // the kernel prints rather than on "15", which is a substring of far too
    // much, and on the remedy rather than on the complaint.
    assert!(
        stale_text.contains("900.000s ago"),
        "the refusal does not name the window the operator has fallen outside: {stale_text}"
    );
    assert!(
        stale_text.contains("re-authenticate"),
        "the refusal does not say what to do instead: {stale_text}"
    );
    assert!(
        aged.standing()?.is_err(),
        "the registry adopted a record from a session the gate refused: {stale_text}"
    );

    // And the other half: a fresh session is still admitted. A gate that
    // refused every approval would pass every assertion above.
    let inside_the_window = rig()?;
    let accepted = inside_the_window.approve(OPERATOR_TOKEN, &good_body());
    assert_eq!(
        accepted.status,
        200,
        "a session inside the window was refused, so the gate refuses everything rather than \
         refusing stale credentials: {}",
        String::from_utf8_lossy(&accepted.body)
    );
    Ok(())
}
