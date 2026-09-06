//! The JSON shapes of the venue-registration surface: `GET /registrations`
//! and `POST /registrations/{source}/approve`.
//!
//! The contract these serialise to is written out in
//! `ROUTES-REGISTRATIONS.md` beside the crate manifest, and a page is built
//! against that file rather than against this one. Keep the two exact.
//!
//! What this surface is for: the platform does everything about a venue
//! registration except the part that must be a person's — reading the
//! terms, opening the account, creating the key, writing it to Secret
//! Manager. So the list says, per catalogued source, what the venue demands,
//! where the source stands, which terms the catalogue cites, which
//! deployment variable the manifest reads the credential under, and the one
//! command an operator runs to put a version behind it. The approval route
//! then records that the operator did those things, under the operator's
//! own authenticated subject.
//!
//! Three properties are structural rather than asserted:
//!
//! * No body carries a credential value and no request body is accepted
//!   that could be one. The approval body's `secret` goes through
//!   [`SecretRef::new`] — the manifest's own shape screen — before anything
//!   else reads it, and a refusal describes the shape of what was refused
//!   and never its text. The list's `secret` fields are variable names read
//!   off the manifests; a value is nowhere in this process to be shown.
//! * The operator on a record is the authenticated principal's subject. The
//!   route builds an `OperatorIdentity` from the session exactly as the
//!   kill-switch route does and the kernel takes the name from it, so
//!   nothing a caller sends can attribute a registration to someone else.
//! * Standing is the registry's own answer, read through the kernel. The
//!   feed's admission gate asks the same registry the same question, so the
//!   page and the gate cannot disagree about who registered.
//!
//! An approval also re-opens this process's connector, where the approved
//! source is the one it senses and the deployment can actually read the
//! credential the record names. What that did is in the answer's `connector`
//! key — `null` when the approved source is not the configured connector,
//! `admitted: false` with the gate's own sentence when it was refused. It
//! used to change nothing at all until the process was restarted, and
//! nothing said so.

use crate::json;
use qip_core::error::Error;
use qip_core::time::Timestamp;
use qip_data_finder::admission::{self, CatalogueEntry};
use qip_data_finder::legal::LicensingPosture;
use qip_data_finder::registration::{RegistrationRecord, RegistrationStanding};
use qip_kernel::Platform;
use qip_market_ingestion::connector::manifest::{SecretRef, SourceManifest};
use qip_market_ingestion::connectors::{
    AlpacaBarsConnector, CoinbaseTickerConnector, FrankfurterRatesConnector, KalshiMarketsConnector,
};
use serde::Serialize;

/// The posture literal every body on this surface carries — the same one
/// the treasury surface renders, so a page has one string to look for.
pub const POSTURE: &str = crate::ledger_views::POSTURE;

/// The Secret Manager secret a deployment variable's value is written to,
/// by the naming the environment already uses for every secret it declares:
/// `QIP_TOKEN_VIEWER` is `qip-token-viewer`, `QIP_VENUE_CREDENTIAL` is
/// `qip-venue-credential`. The variable in lower case with `_` as `-`. The
/// environment module suffixes each secret with the environment name at
/// creation; the command names the secret as the runbooks do, without it.
pub fn secret_manager_name(variable: &str) -> String {
    variable.to_ascii_lowercase().replace('_', "-")
}

/// The one line an operator runs to add a version to the secret behind a
/// slot. The value comes from stdin (`--data-file=-`), so it is in no shell
/// history, no argument list and no process listing — the same form
/// `docs/operations/enabling-live-trading.md` uses for the venue credential.
pub fn secret_command(variable: &str) -> String {
    format!(
        "gcloud secrets versions add {} --data-file=-",
        secret_manager_name(variable)
    )
}

// --- the list -----------------------------------------------------------------

/// Where one source stands, as a page renders it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "standing", rename_all = "snake_case")]
pub enum StandingView {
    /// The source needs no registration.
    Keyless,
    /// A named person registered: who, when they read the terms, and the
    /// deployment variable the credential is read under. Never the value.
    Registered {
        operator: String,
        terms_read_at: String,
        secret: String,
    },
    /// Nobody has registered and the source is refused until somebody does.
    Pending {
        who_must_register: String,
        reason: String,
    },
}

impl StandingView {
    /// The registry's answer for one source, through the kernel.
    ///
    /// A refusal is the pending arm: the registry's own refusal text is the
    /// reason, verbatim, so a page shows the same sentence the feed's gate
    /// would print. `who_must_register` is the deployment's configured
    /// owner — the name the platform holds accountable for what it
    /// registers — because the refusal says "the platform's owner" and a
    /// page should be able to say who that is.
    pub fn of(platform: &Platform, source_id: &str) -> Self {
        match platform.registration_standing(source_id) {
            Ok(RegistrationStanding::Keyless) => Self::Keyless,
            Ok(RegistrationStanding::Registered { record }) => Self::registered(&record),
            Err(refused) => Self::Pending {
                who_must_register: platform.config().owner.clone(),
                reason: refused.message().to_string(),
            },
        }
    }

    fn registered(record: &RegistrationRecord) -> Self {
        Self::Registered {
            operator: record.operator().to_string(),
            terms_read_at: record.terms_read_at().to_rfc3339(),
            secret: record.secret().variable().to_string(),
        }
    }
}

/// A second deployment variable a manifest reads beside the primary one,
/// with the command that fills it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SecretSlotView {
    pub variable: String,
    pub secret_command: String,
}

/// One catalogued source: what it demands, where it stands, and what an
/// operator has to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceRegistrationView {
    pub source_id: String,
    /// The declared requirement (`keyless`, `self_service_api_key`,
    /// `account`, `account_with_identity_verification`), or `null` when the
    /// registry declares none — which the standing then reports as pending,
    /// because an unasked question is not a keyless source.
    pub requirement: Option<String>,
    pub standing: StandingView,
    /// The terms reference the catalogue carries: the licence identifier of
    /// a declared posture, the URL an ambiguous posture's evidence names, or
    /// `null`. What an operator reads before approving; the record then
    /// cites what they actually read.
    pub terms: Option<String>,
    /// The deployment variable the manifest reads the credential under — the
    /// name an approval's `secret` must carry — or `null` for a source whose
    /// manifest names no credential.
    pub secret_slot: Option<String>,
    /// The one line that puts a version behind `secret_slot`, or `null`.
    pub secret_command: Option<String>,
    /// Every further variable the manifest reads (Alpaca's key id beside its
    /// secret key), each with its command. Empty for a single-secret or
    /// keyless source.
    pub companion_secret_slots: Vec<SecretSlotView>,
}

/// The whole list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RegistrationsView {
    pub posture: &'static str,
    pub served_at: String,
    /// Every source in the finder's catalogue, in catalogue order.
    pub sources: Vec<SourceRegistrationView>,
}

/// `GET /registrations`.
///
/// Refuses — a 500 with the reason — when the catalogue or a shipped
/// manifest does not build, because a list that silently omitted a source
/// would read as a source that needs nothing.
pub fn registrations(platform: &Platform, now: Timestamp) -> Result<RegistrationsView, String> {
    let catalogue = admission::catalogue().map_err(|error| error.message().to_string())?;
    let mut sources = Vec::with_capacity(catalogue.len());
    for entry in &catalogue {
        let manifest = shipped_manifest(entry.source_id)
            .transpose()
            .map_err(|error| error.message().to_string())?;
        sources.push(source_view(platform, entry, manifest.as_ref()));
    }
    Ok(RegistrationsView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        sources,
    })
}

/// One row of the list.
pub fn source_view(
    platform: &Platform,
    entry: &CatalogueEntry,
    manifest: Option<&SourceManifest>,
) -> SourceRegistrationView {
    let primary = manifest.and_then(|manifest| manifest.auth.secret.as_ref());
    let companions: Vec<SecretSlotView> = manifest
        .and_then(|manifest| manifest.auth.companion.as_ref())
        .map(|companion| SecretSlotView {
            variable: companion.secret.variable().to_string(),
            secret_command: secret_command(companion.secret.variable()),
        })
        .into_iter()
        .collect();
    SourceRegistrationView {
        source_id: entry.source_id.to_string(),
        requirement: platform
            .registrations()
            .requirement(entry.source_id)
            .map(|requirement| requirement.as_str().to_string()),
        standing: StandingView::of(platform, entry.source_id),
        terms: terms_reference(&entry.posture),
        secret_slot: primary.map(|secret| secret.variable().to_string()),
        secret_command: primary.map(|secret| secret_command(secret.variable())),
        companion_secret_slots: companions,
    }
}

/// The shipped manifest of a catalogued source, or `None` for a source no
/// connector in this build carries.
///
/// Matched on each connector's own `SOURCE_ID` rather than on a table kept
/// here, so a connector renamed upstream is a compile error here and not a
/// row that silently shows no slot.
fn shipped_manifest(source_id: &str) -> Option<qip_core::error::Result<SourceManifest>> {
    match source_id {
        CoinbaseTickerConnector::SOURCE_ID => Some(CoinbaseTickerConnector::shipped_manifest()),
        FrankfurterRatesConnector::SOURCE_ID => Some(FrankfurterRatesConnector::shipped_manifest()),
        KalshiMarketsConnector::SOURCE_ID => Some(KalshiMarketsConnector::shipped_manifest()),
        AlpacaBarsConnector::SOURCE_ID => Some(AlpacaBarsConnector::shipped_manifest()),
        _ => None,
    }
}

/// The terms reference a posture carries.
///
/// A declared posture names the licence it was written against; an
/// ambiguous one names, in its evidence, the document nobody has yet read
/// against the platform's usages — the URL is lifted out so an operator is
/// sent to the document rather than to the sentence about it. Undetermined
/// carries nothing.
fn terms_reference(posture: &LicensingPosture) -> Option<String> {
    match posture {
        LicensingPosture::Declared { license } => Some(license.identifier().to_string()),
        LicensingPosture::Ambiguous { evidence } => evidence
            .split_whitespace()
            .find(|token| token.starts_with("https://") || token.starts_with("http://"))
            .map(|url| url.trim_end_matches([',', ';', '.', ')']).to_string()),
        LicensingPosture::Undetermined => None,
    }
}

// --- the approval -------------------------------------------------------------

/// What `POST /registrations/{source}/approve` accepts: the terms the
/// operator read and the deployment variable the credential is read under.
///
/// Parsed by hand from a JSON object rather than derived, so that no refusal
/// can repeat a value: serde's type errors quote the offending value, and the
/// offending value in the one case this route exists to refuse is a pasted
/// key. Every message here names a field or a shape and never text the
/// caller sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub terms: String,
    pub secret: SecretRef,
}

impl ApprovalRequest {
    /// The keys the body may carry, and the only ones.
    const FIELDS: [&'static str; 2] = ["terms", "secret"];

    /// The longest citation the record will hold. A bound rather than a
    /// guess: `terms` reaches the hash-chained event log, and an unbounded
    /// field is an unbounded record with no erase path.
    const MAX_TERMS: usize = 512;

    pub fn parse(body: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(body).map_err(|_| {
            "the body is not JSON; send {\"terms\": \"<the URL of the terms you read>\", \
             \"secret\": \"<the deployment variable name>\"}"
                .to_string()
        })?;
        let Some(object) = value.as_object() else {
            return Err("the body must be a JSON object with `terms` and `secret`".to_string());
        };
        if let Some(position) = object
            .keys()
            .position(|key| !Self::FIELDS.contains(&key.as_str()))
        {
            // The offending key is named by position, not by text. This
            // refusal used to interpolate the key, twelve lines below a
            // module comment promising that nothing here repeats what a
            // caller sent — and a caller who pasted a credential where a key
            // name belongs would have had it written to the response, to
            // stderr and to whichever ticket the line was copied into.
            return Err(format!(
                "the body's key at position {} is not one this route reads; it takes `terms` \
                 and `secret` only. The key is named by position rather than quoted, because \
                 this route exists to refuse a pasted credential and a refusal that echoed one \
                 would publish it",
                position + 1
            ));
        }
        let terms = Self::terms(object)?;
        let secret = Self::text(object, "secret")?;
        // The manifest's own shape screen: refuses anything that is not a
        // deployment variable name and describes the shape of what it
        // refused, never the text. It is a shape screen and not an identity
        // check — an uppercase-alphanumeric access-key id passes it — so the
        // route holds the value against the manifest's declared slots as
        // well; see [`screen_source`].
        let secret = SecretRef::new(secret).map_err(|error| error.message().to_string())?;
        Ok(Self { terms, secret })
    }

    /// The citation, which must be a URL.
    ///
    /// Two failures, one screen. The first: `terms` was free text on the same
    /// dialog as the credential slot, one field above it, and it reaches the
    /// sealed event log and the process banner — a mis-paste into the wrong
    /// box was recorded verbatim with no erase path, and the shape screen
    /// beside it did not apply. The second: a document *name* is a citation
    /// nobody else can re-read, which is the whole reason the field exists.
    /// A URL is both a shape a credential does not have and the thing a
    /// reviewer actually needs.
    ///
    /// The refusal never repeats what it refused, for the reason above.
    fn terms(object: &serde_json::Map<String, serde_json::Value>) -> Result<String, String> {
        let text = Self::text(object, "terms")?;
        let refusal = format!(
            "`terms` must be the URL of the terms you read, beginning `https://` or `http://`, \
             with no whitespace and at most {} characters. A document name is a citation nobody \
             else can re-read, and what you sent is deliberately not repeated here: this field \
             sits beside the credential slot and a value pasted into the wrong box would \
             otherwise reach the event log, which has no erase path",
            Self::MAX_TERMS
        );
        let scheme_ok = text.starts_with("https://") || text.starts_with("http://");
        let host_ok = text
            .split_once("//")
            .is_some_and(|(_, rest)| rest.contains('.') && !rest.starts_with('.'));
        if !scheme_ok
            || !host_ok
            || text.len() > Self::MAX_TERMS
            || text.split_whitespace().count() != 1
        {
            return Err(refusal);
        }
        Ok(text)
    }

    /// A non-blank string field, or a refusal naming the field.
    fn text(
        object: &serde_json::Map<String, serde_json::Value>,
        field: &str,
    ) -> Result<String, String> {
        let Some(value) = object.get(field) else {
            return Err(format!("the body has no `{field}`; it is required"));
        };
        let Some(text) = value.as_str() else {
            return Err(format!("`{field}` must be a JSON string"));
        };
        if text.trim().is_empty() {
            return Err(match field {
                "terms" => "`terms` is blank; cite the URL or document name of the terms you \
                            read, or nobody can re-read them when they change"
                    .to_string(),
                _ => "`secret` is blank; name the deployment variable the manifest reads the \
                      credential under (the `secret_slot` the list shows), never the value"
                    .to_string(),
            });
        }
        Ok(text.to_string())
    }
}

/// What became of this process's own connector when a registration was
/// approved.
///
/// Present only when the approved source *is* the connector this process
/// senses; `null` otherwise, because "the source you approved is not the one
/// this process reads" and "the re-admission failed" are different answers
/// and a page that showed them the same way would send an operator looking
/// for a fault that is not there.
///
/// `reason` carries the gate's own sentence on a refusal and the feed's own
/// banner line on a success. Neither can carry a credential: the refusal
/// paths name a deployment variable, a licence or a source id, and the
/// banner is built from the manifest and the licensing decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConnectorAdmissionView {
    pub admitted: bool,
    pub reason: String,
}

impl ConnectorAdmissionView {
    /// The gate ran again and this process is now sensing through the
    /// re-opened connector.
    pub fn admitted(reason: impl Into<String>) -> Self {
        Self {
            admitted: true,
            reason: reason.into(),
        }
    }

    /// The gate refused, or was not reached. The registration still stands —
    /// it was journalled before the registry adopted it and before anything
    /// here ran — and the process senses exactly what it sensed before.
    pub fn refused(reason: impl Into<String>) -> Self {
        Self {
            admitted: false,
            reason: reason.into(),
        }
    }
}

/// What the approval route answers: the source's standing after the record
/// was journalled and adopted, and what that did to this process's feed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ApprovalView {
    pub posture: &'static str,
    pub served_at: String,
    pub source_id: String,
    pub standing: StandingView,
    pub connector: Option<ConnectorAdmissionView>,
}

pub fn approved(
    platform: &Platform,
    source_id: &str,
    now: Timestamp,
    connector: Option<ConnectorAdmissionView>,
) -> ApprovalView {
    ApprovalView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        source_id: source_id.to_string(),
        standing: StandingView::of(platform, source_id),
        connector,
    }
}

/// The deployment variables a source's shipped connector manifest actually
/// reads: the primary credential and any companion beside it, in that order.
///
/// Empty for a source no connector in this build carries, and for one whose
/// manifest declares no credential at all — which is not the same as an
/// error, and the caller distinguishes them.
pub fn declared_slots(source_id: &str) -> Result<Vec<String>, String> {
    let Some(manifest) = shipped_manifest(source_id) else {
        return Ok(Vec::new());
    };
    let manifest = manifest.map_err(|error| error.message().to_string())?;
    let mut slots = Vec::new();
    if let Some(secret) = manifest.auth.secret.as_ref() {
        slots.push(secret.variable().to_string());
    }
    if let Some(companion) = manifest.auth.companion.as_ref() {
        slots.push(companion.secret.variable().to_string());
    }
    Ok(slots)
}

/// What the approval route requires of the source and the slot before the
/// kernel is asked for anything.
///
/// Two failures this closes, both found in review:
///
/// * **A mis-paste survives forever.** [`SecretRef`] is a *shape* screen: an
///   uppercase-alphanumeric access-key id passes it, and so does a venue key
///   id of the `PK…` form. A credential written into `secret` therefore
///   reached the hash-chained event log — which has no erase path — was
///   shown to every viewer of `GET /registrations`, and was printed at each
///   restart. Holding the submitted variable against the manifest's own
///   declared slots turns an open field into a choice from a list of two,
///   and the refusal names the list rather than what was sent.
/// * **An approval that records nothing.** A source whose requirement does
///   not need registration answers `keyless` on every read surface whatever
///   is written about it, so the record sat in the log where nothing read
///   it, attributing a named person to a venue relationship no decision
///   consults.
///
/// Both refusals are the caller's to fix and neither repeats a value.
pub fn screen_source(
    platform: &Platform,
    source_id: &str,
    secret: &SecretRef,
) -> Result<(), String> {
    let Some(requirement) = platform.registrations().requirement(source_id) else {
        return Err(format!(
            "`{source_id}` has no registration requirement declared, so what an approval would \
             satisfy is unknown and it is refused. Declare the requirement in the shipped table \
             first — it is a source-file literal, reviewed like code"
        ));
    };
    if !requirement.needs_registration() {
        return Err(format!(
            "`{source_id}` is `{}` and needs no registration, so there is nothing for an \
             approval to record: every read surface answers `keyless` for it whatever is \
             written here, and the record would sit in the sealed event log where nothing \
             reads it. Approve a source whose requirement is an account or a key",
            requirement.as_str()
        ));
    }
    let declared = declared_slots(source_id)?;
    if declared.is_empty() {
        return Err(format!(
            "no connector in this build reads a credential for `{source_id}`, so there is no \
             deployment variable an approval could name and any value written here would be \
             unverifiable text in the event log. Ship a manifest that declares the slot before \
             recording a registration against it"
        ));
    }
    if !declared.iter().any(|slot| slot == secret.variable()) {
        return Err(format!(
            "the `secret` in this body is not a deployment variable `{source_id}`'s manifest \
             reads. It reads {}; name one of those. What you sent is not repeated: the shape \
             screen in front of this admits a real credential — an uppercase-alphanumeric \
             access-key id passes it — and a mis-paste recorded here would reach the \
             hash-chained log, every reader of GET /registrations and the process banner, with \
             no erase path",
            declared.join(" and ")
        ));
    }
    Ok(())
}

/// The status a kernel refusal of an approval answers with.
///
/// Shared with the eligibility route, so the two operator surfaces refuse in
/// one grammar and a console explaining a status explains it once.
///
/// * **400** — the body. An invalid request or a schema violation is the
///   caller's to fix.
/// * **404** — nothing of that name.
/// * **409** — the platform will not act under its current state or on this
///   identity: a credential older than the kernel accepts, a guard tripped.
///   This is the status the contract explains as "re-authenticate, or clear
///   what is holding the platform".
/// * **500** — a numeric failure inside the kernel. Nothing the caller sent
///   is wrong and there is nothing for them to change.
/// * **503** — the process could not carry the request out: storage, a
///   dependency this deployment does not have, a deadline. It used to answer
///   409, so a journal write that failed told an operator to re-authenticate
///   — and they did, repeatedly, against a disk that was full.
pub fn refusal_status(error: &Error) -> u16 {
    match error {
        Error::Invalid(_) | Error::Schema(_) => 400,
        Error::NotFound(_) => 404,
        Error::Denied(_) | Error::Guard(_) => 409,
        Error::Numeric(_) => 500,
        Error::Io(_) | Error::Unavailable(_) | Error::Timeout(_) => 503,
    }
}

/// A refusal body, in the shape every other refusal on the API takes.
pub fn refusal(reason: &str) -> String {
    format!(r#"{{"error":{}}}"#, json::string(reason))
}
