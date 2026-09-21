//! The JSON shapes of the treasury surface: the four read routes
//! `/ledger/users`, `/wallet`, `/corridors` and `/transfer-gate`, and the two
//! operator routes beside them, `POST /ledger/users/{user}/eligibility` and
//! `POST /ledger/users/{user}/investment-requests`.
//!
//! The operator routes are not an exception to what the rest of this file says
//! about the layer's authority. Neither decides anything itself: each screens
//! a body, resolves the user against the mandate registry, and hands the
//! kernel — `Platform::decide_eligibility`, `Platform::decide_investment` — an
//! identity built from the authenticated session, the same intent-raising
//! shape the kill switch and the venue approval take. Each answers the
//! ledger's own answer rather than its own claim about it: the user's
//! `/ledger/users` row, and for a request the verdict the mandate returned.
//! Nothing here can move capital: an eligibility record is a precondition the
//! ledger checks before a funding, and an admitted investment request is a
//! statement that the mandate would admit one. Neither is a transfer, and the
//! request body carries a constant `funded: false` so no interface can imply
//! otherwise.
//!
//! The contract these serialise to is written out in `ROUTES-LEDGER.md`
//! beside the crate manifest, and a page is built against that file rather
//! than against this one. Keep the two exact: a key renamed here and not
//! there is a panel that renders blank with no error anywhere.
//!
//! Three properties are structural rather than asserted:
//!
//! * Every figure of money is a `String`. The API is forbidden a field typed
//!   as money (`api_boundary.rs`) so that nothing here can add, and a page
//!   receives the platform's exact decimal text rather than a float.
//! * Nothing here names a capital or fabric type. The application layer may
//!   not depend on `qip-capital` or `qip-capital-fabric` at all, so every
//!   view is built by calling methods on what the kernel hands over — an
//!   entitlement it evaluated, a mandate it holds, the fabric state its
//!   journal built — and the API cannot construct a mandate, an entitlement,
//!   a corridor or an intent of its own. Where a value is an enum the API
//!   cannot match on (a destination's status, a gate verdict), it is read
//!   from the type's own serialisation, the way the withdrawal flag is.
//! * The withdrawal capability's `granted` flag is read from the type's own
//!   serialisation rather than written as a literal: the type has one arm,
//!   `Refused`, and the flag is whether a `Granted` arm was serialised. A
//!   literal `false` would survive the day someone added the arm the ADR
//!   refuses; this reads `true` that day, and the test that pins it fires.
//!
//! What the process holds is read from the kernel's fabric journal: the
//! wallet as last assembled, its reconciliation outcomes, every corridor and
//! destination, and the last gate assessment. What it does not yet hold — a
//! wallet, until a statement has been handed in and a cycle has run — is
//! stated with a flag and a reason, the way `crate::missing` states the rest.

use crate::json;
use qip_core::time::Timestamp;
use qip_kernel::Platform;
use serde::Serialize;

/// The posture literal every treasury body carries.
///
/// One constant, so the four routes and the test that pins them cannot
/// disagree about the text a page renders.
pub const POSTURE: &str = "PAPER TRADING";

/// The ledger role every entitlement on this surface is evaluated under.
pub const EVALUATED_AS_ROLE: &str = "viewer";

/// Why `/wallet` has no wallet behind it yet.
pub const NO_WALLET: &str = "no wallet is assembled yet. A wallet is a read model over holdings \
    observed through read-only channels paired with what the ledger booked, and the kernel \
    observes no custodian, venue balance or chain address of its own; it assembles one in the \
    LEARN stage of each cycle from the statements handed to it, and none has been handed in, \
    or no cycle has run since. A wallet showing zero holdings would read as an empty account \
    rather than an unobserved one.";

/// Why a user has no entitlement rows.
pub const NO_PRODUCTS: &str = "no product to evaluate against: an entitlement is decided per \
    strategy family the central factory has registered, and none is registered in this process.";

/// What `/transfer-gate` says about itself.
///
/// The last clause was added on 2026-09-07 because the one before it was true
/// and still misleading. "null when none has been made" reads as *not yet* —
/// an operator refreshing a page waits for a first assessment that cannot
/// arrive. `last_assessment` is null **structurally**: no production code
/// issues a `FabricCommand::Gate`, so `assessments()` is permanently empty,
/// not transiently. A page that renders a control's history as pending when
/// the control has no producer is the same defect this platform keeps finding
/// in its own registers, moved to a surface an operator actually reads.
pub const GATE_NOTE: &str = "the gate is veto-only and has no transfer engine behind it: an \
    approval is a record that the seven checks passed, and nothing in this platform consumes \
    one. An intent reaches the gate only through the kernel's fabric journal, and every \
    assessment is a record in the event log; last_assessment is the newest, or null when none \
    has been made. Null now means none has been made, not that none can be: an intent is \
    stated in the capital-fabric declaration the deployment mounts at \
    QIP_CAPITAL_FABRIC_PATH, and this said the null was structural until that mount existed. \
    With nothing declared the checks below are the roster the gate would apply rather than a \
    history of it applying them, and the banner of this process says which of the two you are \
    reading.";

// --- /ledger/users ----------------------------------------------------------

/// A capability as a page renders it: whether, and the basis or the refusal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CapabilityView {
    pub granted: bool,
    /// The basis of a grant, or the input that refused.
    pub reason: String,
}

impl CapabilityView {
    /// Read a capability from its own serialisation.
    ///
    /// The capital crate's `Capability` and `WithdrawalEntitlement` both
    /// serialise as an externally tagged enum — `{"Granted":{"basis":..}}`
    /// or `{"Refused":{"reason":..}}` — and this reads that tag rather than
    /// matching the type, which the API cannot name. A tag other than those
    /// two is refused as a shape this reader does not understand, so a
    /// variant added to the type surfaces as an error here and not as a
    /// silently ungranted capability.
    fn from_serialised(value: &impl Serialize) -> Result<Self, String> {
        let value = serde_json::to_value(value).map_err(|error| error.to_string())?;
        if let Some(basis) = value.get("Granted").and_then(|arm| arm.get("basis")) {
            return Ok(Self {
                granted: true,
                reason: basis.as_str().unwrap_or_default().to_string(),
            });
        }
        if let Some(reason) = value.get("Refused").and_then(|arm| arm.get("reason")) {
            return Ok(Self {
                granted: false,
                reason: reason.as_str().unwrap_or_default().to_string(),
            });
        }
        Err(format!(
            "a capability serialised as neither Granted nor Refused: {value}"
        ))
    }
}

/// One product's entitlement evaluation for one user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EntitlementView {
    pub family: String,
    pub role: String,
    pub evaluated_at: String,
    pub can_view: CapabilityView,
    pub can_invest: CapabilityView,
    /// Never granted. See the module comment for why the flag is read
    /// rather than written.
    pub can_withdraw: CapabilityView,
}

/// Which strategy families a mandate admits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PermittedFamiliesView {
    pub any: bool,
    pub families: Vec<String>,
}

/// A mandate's terms, as text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MandateView {
    pub capital: String,
    pub currency: String,
    pub risk_tolerance: String,
    pub liquidity_floor: String,
    pub investable: String,
    pub exploration_share: String,
    pub jurisdiction: String,
    pub permitted_families: PermittedFamiliesView,
}

/// One declared, unposted inflow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExpectedInflowView {
    pub reference: String,
    pub amount: String,
    pub declared_at: String,
}

/// One `(strategy, currency)` book of one user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BalanceView {
    pub strategy: String,
    pub currency: String,
    pub settled: String,
    pub reserved: String,
    /// `settled - reserved`. Expected inflows are not in it, and nor is
    /// `uninvestable`.
    pub available: String,
    /// Cash that arrived past the mandate's ceilings when it was posted and
    /// is held: received, never in `available`, never sized against (ADR
    /// 0085). Zero on every deployed platform today, because nothing in this
    /// build posts an arrival — see `inflow_posting` on the body.
    pub uninvestable: String,
    /// Visible and never added to anything.
    pub expected_inflows_total: String,
    pub expected_inflows: Vec<ExpectedInflowView>,
    pub entries: u64,
    pub last_entry_at: Option<String>,
}

/// Whether a user may have capital put to work, as the ledger decides it
/// at request time.
///
/// `eligible` is the ledger's own verdict and the terms are read off the
/// record an operator wrote; `refused` carries the ledger's stable token
/// (`unknown_user`, `expired`, …) and its sentence when the verdict is no.
/// The failure this guards: a page listing a user with balances and no way
/// to tell whether the next funding would be refused, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EligibilityView {
    pub eligible: bool,
    pub verified_at: Option<String>,
    pub can_invest: Option<bool>,
    pub jurisdiction: Option<String>,
    pub expires_at: Option<String>,
    pub refused: Option<String>,
    pub reason: Option<String>,
}

/// One enrolled user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UserView {
    pub user_id: String,
    pub mandate: MandateView,
    pub eligibility: EligibilityView,
    pub balances: Vec<BalanceView>,
    pub entitlements: Vec<EntitlementView>,
    /// Set when `entitlements` is empty, saying why.
    pub entitlements_note: Option<String>,
}

/// What a page must say beside every expected inflow it renders: that this
/// build never posts one.
///
/// A declared inflow reads as "arriving" to anyone who sees it, and blueprint
/// §40.12's flow continues `detected → settled → available`. It does not
/// continue here. The ledger's `post_inflow` is reachable from nothing a
/// deployed process runs, because nothing in this tree can honestly say a
/// user's wire landed — the only custodian statement the platform observes
/// is the desk's own wallet — and ADR 0085 refuses to invent a statement.
/// Rendered as a constant so the sentence and the code cannot drift apart:
/// the day a reconciled statement posts an inflow, this constant is the
/// thing to delete, and the test that pins it fires.
pub const INFLOW_POSTING: &str = "no declared inflow is ever posted by this build: nothing here \
    can say a user's wire landed, so an expected inflow stays expected until an operator \
    cancels it, and `uninvestable` is zero on every balance until a reconciled statement \
    exists (ADR 0085)";

/// The body of `GET /ledger/users`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LedgerUsersView {
    pub posture: &'static str,
    pub served_at: String,
    pub evaluated_as_role: &'static str,
    pub products: Vec<String>,
    pub fills_journalled: u64,
    /// [`INFLOW_POSTING`]: why every `expected_inflows` entry below is a
    /// claim and never becomes a balance in this build.
    pub inflow_posting: &'static str,
    /// The refusal *this caller* would receive at the two inflow routes, or
    /// `None` if they would get past the presence gate.
    ///
    /// The `reinstatement_refusal` pattern on `/venues/withdrawals`, and here
    /// for the same reason: a console about to offer a declaration form is
    /// rendering for the person who would submit it, and ADR 0065 means the
    /// signature cannot be taken. Without this the portal had only the route
    /// contract's own sentence to show — a statement about the build rather
    /// than about the caller — or it would have had to attempt the write to
    /// find out, which is a mutating request sent to learn a fact.
    ///
    /// Obtained by making the same call the route makes rather than by
    /// asserting what it would answer, so it cannot drift from the gate.
    pub inflow_refusal: Option<String>,
    pub users: Vec<UserView>,
}

/// Build `/ledger/users` from the platform at `now`, with the caller's own
/// inflow refusal.
///
/// `inflow_refusal` is passed in by the handler that holds the principal,
/// because this module has no principal and must not invent one — the rule
/// `venue_views::withdrawals` already keeps.
///
/// Every figure is read at request time; nothing is cached between calls,
/// because a mandate that changed or a fill that landed is reflected on the
/// next read rather than the next restart. Users are every holder in the
/// kernel's registry — the desk and each mandate the configuration enrolled
/// — in user-id order, and a user's balances are the books the kernel's
/// pro-rata split and funding actually moved.
pub fn ledger_users(
    platform: &Platform,
    now: Timestamp,
    inflow_refusal: Option<String>,
) -> Result<LedgerUsersView, String> {
    let ledger = platform.user_ledger();
    let products: Vec<String> = platform
        .central()
        .factory()
        .candidates()
        .map(|candidate| candidate.family().as_str().to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(LedgerUsersView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        evaluated_as_role: EVALUATED_AS_ROLE,
        products,
        fills_journalled: ledger.fills_journalled(),
        inflow_posting: INFLOW_POSTING,
        inflow_refusal,
        users: user_rows(platform, now, None)?,
    })
}

/// The rows `/ledger/users` lists, or the one row for `only`.
///
/// One builder for both callers so a row an operator route answers with and
/// the same row in the list cannot come to differ — which is the whole point
/// of answering the row rather than a bespoke acknowledgement. Every capital
/// type stays inside this body: the application layer holds no edge to
/// `qip-capital` (`api_boundary.rs` pins it), so nothing here may appear in a
/// signature, and `only` is therefore the user id as text rather than the
/// ledger's own key type.
fn user_rows(
    platform: &Platform,
    now: Timestamp,
    only: Option<&str>,
) -> Result<Vec<UserView>, String> {
    let ledger = platform.user_ledger();
    let entitlements = platform.viewer_entitlements(now);

    let mut users = Vec::with_capacity(ledger.mandates().len());
    for (user, mandate) in ledger
        .mandates()
        .iter()
        .filter(|(user, _)| only.is_none_or(|wanted| user.as_str() == wanted))
    {
        let permitted = serde_json::to_value(mandate.permitted_families())
            .map_err(|error| error.to_string())?;
        let permitted_families = PermittedFamiliesView {
            any: permitted.as_str() == Some("Any"),
            families: permitted
                .get("Only")
                .and_then(|only| only.as_array())
                .map(|only| {
                    only.iter()
                        .filter_map(|family| family.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        };
        let balances = ledger
            .books()
            .iter()
            .filter(|((owner, _), _)| owner == user)
            .flat_map(|((_, strategy), book)| {
                book.balances().iter().map(|(currency, cash)| BalanceView {
                    strategy: strategy.as_str().to_string(),
                    currency: currency.to_string(),
                    settled: cash.settled().to_string(),
                    reserved: cash.reserved().to_string(),
                    available: cash.available().to_string(),
                    uninvestable: cash.uninvestable().to_string(),
                    expected_inflows_total: cash.expected_total().to_string(),
                    expected_inflows: cash
                        .expected_inflows()
                        .iter()
                        .map(|(reference, inflow)| ExpectedInflowView {
                            reference: reference.clone(),
                            amount: inflow.amount.to_string(),
                            declared_at: inflow.declared_at.to_rfc3339(),
                        })
                        .collect(),
                    entries: book.entries(),
                    last_entry_at: book.last_entry_at().map(Timestamp::to_rfc3339),
                })
            })
            .collect();
        let mut rows = Vec::new();
        for entitlement in entitlements.iter().filter(|e| e.user() == user) {
            rows.push(EntitlementView {
                family: entitlement.family().to_string(),
                role: EVALUATED_AS_ROLE.to_string(),
                evaluated_at: entitlement.evaluated_at().to_rfc3339(),
                can_view: CapabilityView::from_serialised(entitlement.can_view())?,
                can_invest: CapabilityView::from_serialised(entitlement.can_invest())?,
                can_withdraw: CapabilityView::from_serialised(entitlement.can_withdraw())?,
            });
        }
        let entitlements_note = rows.is_empty().then(|| NO_PRODUCTS.to_string());
        let eligibility = match ledger.eligibility_of(user, now) {
            Ok(record) => EligibilityView {
                eligible: true,
                verified_at: Some(record.verified_at().to_rfc3339()),
                can_invest: Some(record.can_invest()),
                jurisdiction: Some(record.jurisdiction().to_string()),
                expires_at: Some(record.expires_at().to_rfc3339()),
                refused: None,
                reason: None,
            },
            Err(ineligible) => EligibilityView {
                eligible: false,
                verified_at: None,
                can_invest: None,
                jurisdiction: None,
                expires_at: None,
                refused: Some(ineligible.name().to_string()),
                reason: Some(ineligible.describe(user)),
            },
        };
        users.push(UserView {
            user_id: user.as_str().to_string(),
            mandate: MandateView {
                capital: mandate.capital().to_string(),
                currency: mandate.currency().to_string(),
                risk_tolerance: mandate.risk_tolerance().to_string(),
                liquidity_floor: mandate.liquidity_floor().to_string(),
                investable: mandate.investable().to_string(),
                exploration_share: mandate.exploration_share().to_string(),
                jurisdiction: mandate.jurisdiction().to_string(),
                permitted_families,
            },
            balances,
            eligibility,
            entitlements: rows,
            entitlements_note,
        });
    }
    Ok(users)
}

// --- POST /ledger/users/{user}/eligibility ----------------------------------

/// The sentence every unknown key on the eligibility body is refused with.
///
/// It names the fields and then answers the mistake the shape invites: the
/// blueprint lists `can_withdraw` beside `can_invest`, and a caller who sends
/// it is a caller who believes this platform has a withdrawal path. A key
/// silently ignored would let them keep believing it, which is the failure
/// that matters here — not the malformed request.
pub const NO_WITHDRAWAL_FIELD: &str = "an eligibility decision reads `decision` and `reason`, \
    with `verified_at`, `can_invest`, `jurisdiction` and `expires_at` on a grant, and nothing \
    else. There is no withdrawal field on an eligibility record and there is no route that \
    could read one: ADR 0021 refuses the path by which capital leaves the platform";

/// What `POST /ledger/users/{user}/eligibility` accepts.
///
/// Parsed by hand from a JSON object rather than derived, for the reason the
/// registration approval's body is: a derived refusal quotes the offending
/// value and names a Rust type, and a refusal here has to name the *field* a
/// person has to fix. Unknown keys are refused rather than ignored — see
/// [`NO_WITHDRAWAL_FIELD`].
///
/// The decision is carried as the JSON the kernel's own decision type
/// deserialises from, rebuilt here from validated pieces rather than passed
/// through from the caller's object, so no key this route does not read can
/// reach the ledger even by accident.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EligibilityRequest {
    decision: serde_json::Value,
    reason: String,
}

impl EligibilityRequest {
    /// The keys a grant may carry, and the only ones.
    const GRANTED_FIELDS: [&'static str; 6] = [
        "decision",
        "verified_at",
        "can_invest",
        "jurisdiction",
        "expires_at",
        "reason",
    ];
    /// The keys a revocation may carry. A revocation states no terms: it
    /// withdraws the ones on record, and a body restating them would be a
    /// second claim about what was verified.
    const REVOKED_FIELDS: [&'static str; 2] = ["decision", "reason"];

    pub fn parse(body: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(body).map_err(|_| {
            "the body is not JSON; send {\"decision\": \"granted\", \"verified_at\": <RFC 3339>, \
             \"can_invest\": <bool>, \"jurisdiction\": \"GB\", \"expires_at\": <RFC 3339>, \
             \"reason\": \"<why>\"} or {\"decision\": \"revoked\", \"reason\": \"<why>\"}"
                .to_string()
        })?;
        let Some(object) = value.as_object() else {
            return Err(
                "the body must be a JSON object carrying `decision` and `reason`".to_string(),
            );
        };
        let decision = Self::text(object, "decision")?;
        let permitted: &[&str] = match decision.as_str() {
            "granted" => &Self::GRANTED_FIELDS,
            "revoked" => &Self::REVOKED_FIELDS,
            _ => {
                return Err(
                    "`decision` must be `granted` or `revoked`; an operator either verified \
                     this user or withdrew a verification, and there is no third thing to record"
                        .to_string(),
                );
            }
        };
        if let Some(position) = object
            .keys()
            .position(|key| !permitted.contains(&key.as_str()))
        {
            // Named by position, never quoted, for the reason the venue
            // approval's refusal is: a refusal that echoed what a caller sent
            // publishes it, to the response, to stderr and to whichever
            // ticket the line is copied into.
            return Err(format!(
                "the body's key at position {} is not one this route reads; {NO_WITHDRAWAL_FIELD}",
                position + 1
            ));
        }
        let reason = Self::text(object, "reason")?;
        // Built from pieces this function validated, never from `object`.
        let decision = if decision == "revoked" {
            serde_json::json!({ "decision": "revoked", "reason": reason })
        } else {
            serde_json::json!({
                "decision": "granted",
                "eligibility": {
                    "verified_at": Self::instant(object, "verified_at")?,
                    "can_invest": Self::flag(object, "can_invest")?,
                    "jurisdiction": Self::text(object, "jurisdiction")?,
                    "expires_at": Self::instant(object, "expires_at")?,
                }
            })
        };
        Ok(Self { decision, reason })
    }

    /// The decision as the kernel's own type.
    ///
    /// Generic so that the type is inferred from
    /// `Platform::decide_eligibility`'s own signature and this crate never
    /// names it: the application layer ships no edge to `qip-capital`
    /// (`api_boundary.rs`), and every rule the type keeps — a two-letter ISO
    /// 3166 jurisdiction, an expiry after the verification — is enforced by
    /// the type itself rather than restated here, where a second copy would
    /// be free to drift.
    pub fn decision<D: serde::de::DeserializeOwned>(&self) -> Result<D, String> {
        serde_json::from_value(self.decision.clone())
            .map_err(|error| format!("the eligibility terms were refused: {error}"))
    }

    /// What the audit trail records about why this decision was taken. The
    /// kernel holds it to a length of its own; this only refuses a blank.
    pub fn reason(&self) -> &str {
        &self.reason
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
            return Err(format!(
                "`{field}` is blank; an eligibility decision nobody can read back is a decision \
                 nobody can be asked about"
            ));
        }
        Ok(text.to_string())
    }

    /// A boolean field, refused rather than coerced: `"true"` and `1` are a
    /// caller who has not decided, and clamping either to a verdict about
    /// whether a person may have capital put to work is not this route's to
    /// do.
    fn flag(
        object: &serde_json::Map<String, serde_json::Value>,
        field: &str,
    ) -> Result<bool, String> {
        match object.get(field) {
            None => Err(format!(
                "the body has no `{field}`; it is required on a grant"
            )),
            Some(serde_json::Value::Bool(flag)) => Ok(*flag),
            Some(_) => Err(format!(
                "`{field}` must be a JSON boolean, `true` or `false`, and not a string or a number"
            )),
        }
    }

    /// An RFC 3339 instant, re-rendered from the parsed value so that what
    /// reaches the ledger is the instant this route understood and not the
    /// text it was sent.
    fn instant(
        object: &serde_json::Map<String, serde_json::Value>,
        field: &str,
    ) -> Result<String, String> {
        let text = Self::text(object, field)?;
        match Timestamp::parse_rfc3339(&text) {
            Some(at) => Ok(at.to_rfc3339()),
            None => Err(format!(
                "`{field}` is not an RFC 3339 instant in UTC, such as \
                 \"2025-10-09T08:53:20.000Z\""
            )),
        }
    }
}

/// What the eligibility route answers: the user's row, exactly as
/// `/ledger/users` renders it, after the registry adopted the decision.
///
/// The row rather than an acknowledgement, because an acknowledgement is the
/// route's claim about what it did and the row is the ledger's. The two would
/// disagree the day a decision was journalled and the registry refused it,
/// and it is the ledger's answer an operator needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EligibilityDecisionView {
    pub posture: &'static str,
    pub served_at: String,
    pub user: UserView,
}

/// Build the answer to an eligibility decision: the one `/ledger/users` row
/// for `user`, read back from the platform after the decision was applied.
pub fn decided_eligibility(
    platform: &Platform,
    user: &str,
    now: Timestamp,
) -> Result<EligibilityDecisionView, String> {
    let mut rows = user_rows(platform, now, Some(user))?;
    if rows.len() != 1 {
        // Unreachable through the route, which resolves the user against the
        // mandate registry before it decides anything. Answered rather than
        // indexed: a panic here would poison the lock every other route waits
        // on.
        return Err(format!(
            "the ledger holds {} rows for `{user}` after the decision was applied; it must hold \
             exactly one",
            rows.len()
        ));
    }
    Ok(EligibilityDecisionView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        user: rows.remove(0),
    })
}

// --- POST and DELETE /ledger/users/{user}/expected-inflows -------------------

/// The sentence every unknown key on an inflow declaration is refused with.
///
/// It answers the mistake §40.12's flow invites: the blueprint's "Add
/// capital" runs `funding source → destination → amount → … → settled →
/// available`, so a caller writing against it will send a `source`, a
/// `destination` or a `settled`. None of those is read. This route records
/// that a deposit is *expected*; it does not receive one, post one or make
/// one available, and a key silently ignored would let a caller believe it
/// had.
pub const NO_ARRIVAL_FIELD: &str = "an inflow declaration reads `strategy`, `reference` and \
    `amount`, and nothing else. It records that the user says a deposit is on its way: the \
    amount is held beside the balance and outside `available` until the ledger posts it, \
    which this build never does, and there is no field on this route that could receive, \
    post or invest anything";

/// What `POST /ledger/users/{user}/expected-inflows` accepts.
///
/// Parsed by hand for the reason the other two operator bodies are: a
/// refusal names the *field* a person must fix. The user is not in the body
/// — it is the path's, resolved against the mandate registry — and the
/// instant is the server's, because a declaration dated by its caller is a
/// caller-chosen position in the eligibility's history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InflowDeclarationBody {
    declaration: serde_json::Value,
}

impl InflowDeclarationBody {
    /// The keys this route reads, and the only ones.
    const FIELDS: [&'static str; 3] = ["strategy", "reference", "amount"];

    pub fn parse(body: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(body).map_err(|_| {
            "the body is not JSON; send {\"strategy\": \"<id>\", \"reference\": \"<the wire's \
             reference>\", \"amount\": \"1000.00\"}"
                .to_string()
        })?;
        let Some(object) = value.as_object() else {
            return Err(
                "the body must be a JSON object carrying an inflow declaration".to_string(),
            );
        };
        if let Some(position) = object
            .keys()
            .position(|key| !Self::FIELDS.contains(&key.as_str()))
        {
            // Named by position and never quoted, for the reason every
            // other operator body's refusal is: echoing what a caller sent
            // publishes it to the response and to every log that copies it.
            return Err(format!(
                "the body's key at position {} is not one this route reads; {NO_ARRIVAL_FIELD}",
                position + 1
            ));
        }
        // Built from pieces this function validated, never from `object`.
        let declaration = serde_json::json!({
            "strategy": Self::text(object, "strategy")?,
            "reference": Self::text(object, "reference")?,
            "amount": Self::amount(object)?,
        });
        Ok(Self { declaration })
    }

    /// The declaration as the kernel's own type.
    ///
    /// Generic so the type is inferred from `Platform::expect_inflow`'s
    /// signature and this crate never names it: the application layer ships
    /// no edge to `qip-capital` and no field typed as money
    /// (`api_boundary.rs`), and the exact-decimal rule is the type's own.
    pub fn declaration<D: serde::de::DeserializeOwned>(&self) -> Result<D, String> {
        serde_json::from_value(self.declaration.clone())
            .map_err(|error| format!("the inflow declaration was refused: {error}"))
    }

    /// The amount, as text and only as text — a JSON number is refused
    /// rather than converted, for the reason the investment request gives.
    fn amount(object: &serde_json::Map<String, serde_json::Value>) -> Result<String, String> {
        match object.get("amount") {
            None => Err("the body has no `amount`; it is required".to_string()),
            Some(serde_json::Value::String(amount)) if !amount.trim().is_empty() => {
                Ok(amount.trim().to_string())
            }
            Some(serde_json::Value::String(_)) => Err("`amount` is blank".to_string()),
            Some(_) => Err(
                "`amount` must be a JSON string such as \"1000.00\", never a number: a number \
                 is parsed as a float and a deposit a parser rounded is not the deposit anyone \
                 declared"
                    .to_string(),
            ),
        }
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
            return Err(format!("`{field}` is blank"));
        }
        Ok(text.to_string())
    }
}

/// The answer to a declaration or a cancellation: the user's `/ledger/users`
/// row, read back after the ledger adopted the record.
///
/// The row rather than an acknowledgement, for the reason the eligibility
/// answer is the row: the acknowledgement would be the route's claim and the
/// row is the ledger's, and an operator needs to see the declared amount
/// beside `available` — which it did not move — and beside `inflow_posting`,
/// which says it never will in this build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InflowRowView {
    pub posture: &'static str,
    pub served_at: String,
    pub inflow_posting: &'static str,
    pub user: UserView,
}

/// Build the answer to a declaration or a cancellation for `user`.
pub fn inflow_row(
    platform: &Platform,
    user: &str,
    now: Timestamp,
) -> Result<InflowRowView, String> {
    let mut rows = user_rows(platform, now, Some(user))?;
    if rows.len() != 1 {
        // Unreachable through the routes, which resolve the user against the
        // mandate registry first. Answered rather than indexed, for the
        // reason `decided_eligibility` gives.
        return Err(format!(
            "the ledger holds {} rows for `{user}` after the record was applied; it must hold \
             exactly one",
            rows.len()
        ));
    }
    Ok(InflowRowView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        inflow_posting: INFLOW_POSTING,
        user: rows.remove(0),
    })
}

// --- GET /ledger/commitments and the capital-call routes (ADR 0085 §5) --------

/// What a page must say beside every capital call it renders: that this
/// build never settles one.
///
/// A filed notice reads as "being paid" to anyone who sees it, and blueprint
/// §43.2's object is a demand the desk must meet. It is not met here. Meeting
/// a call is capital leaving the desk, and the only party that can attest
/// that is the custodian the reserve was paid from; the one statement the
/// platform observes is the desk's own wallet, which names venues and
/// assets and never a fund administrator, and ADR 0085 §2 refuses to invent
/// a statement in the direction that would *lower* the reserve. So a notice
/// stands — and once overdue charges its consequence against what the
/// platform will deploy — until an operator withdraws it. Rendered as a
/// constant so the sentence and the code cannot drift apart: the day a
/// statement line at the administrator settles a call, this constant is the
/// thing to delete, and the test that pins it fires.
pub const CALL_SETTLEMENT: &str = "no filed capital call is ever settled by this build: meeting \
    a call is capital leaving the desk, which only the custodian it was paid from can attest \
    and no statement observed here names a fund administrator, so a notice stands — and once \
    overdue charges its consequence against what the platform will deploy — until an operator \
    withdraws it (ADR 0085 §5)";

/// The sentence every unknown key on a capital-call body is refused with.
///
/// It answers the mistake the object invites: a drawdown notice is something
/// the desk *pays*, so a caller will send `paid`, `settled` or `source`.
/// None is read. This route files that a fund has demanded capital; it does
/// not pay it, settle it or move it, and a key silently ignored would let a
/// caller believe it had.
pub const NO_PAYMENT_FIELD: &str = "a capital call reads `reference`, `amount`, `due` and \
    `consequence`, and nothing else. It files a fund's drawdown notice against one of the \
    desk's commitments: the notice is held against the reserve the platform sizes from, and \
    there is no field on this route that could pay, settle or transfer anything";

/// The consequence of failing a call, as `ROUTES-LEDGER.md` writes it out:
/// a `kind` and the basis points the kind carries, or none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConsequenceView {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annual_rate_bps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fraction_bps: Option<u32>,
}

impl ConsequenceView {
    fn of(consequence: qip_financial::cashflow::CallConsequence) -> Self {
        use qip_financial::cashflow::CallConsequence;
        match consequence {
            CallConsequence::Interest { annual_rate_bps } => Self {
                kind: consequence.label(),
                annual_rate_bps: Some(annual_rate_bps),
                fraction_bps: None,
            },
            CallConsequence::Forfeiture { fraction_bps } => Self {
                kind: consequence.label(),
                annual_rate_bps: None,
                fraction_bps: Some(fraction_bps),
            },
            CallConsequence::Acceleration => Self {
                kind: consequence.label(),
                annual_rate_bps: None,
                fraction_bps: None,
            },
        }
    }
}

/// One filed notice, as the book holds it at the instant served.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CapitalCallView {
    pub reference: String,
    pub amount: String,
    pub issued_at: String,
    pub due_at: String,
    pub consequence: ConsequenceView,
    /// Whether the money was due and the notice knowable, both by the
    /// instant served.
    pub overdue: bool,
    /// Whole days past due, zero where not overdue. A count, not money.
    pub days_late: i64,
    /// What failing this notice has cost by the instant served, on top of
    /// the amount owed. Zero until it is overdue.
    pub penalty: String,
}

/// One of the desk's private commitments with the notices standing against
/// it. `obligation` is what the reserve actually subtracts for this
/// commitment: the unfunded balance plus the accrued penalty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommitmentView {
    pub subject: String,
    pub committed: String,
    pub called: String,
    pub unfunded: String,
    pub accrued_default_penalty: String,
    pub obligation: String,
    pub known_at: String,
    pub capital_calls: Vec<CapitalCallView>,
}

/// The body of `GET /ledger/commitments`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommitmentsView {
    pub posture: &'static str,
    pub served_at: String,
    /// [`CALL_SETTLEMENT`]: why every notice below stands until withdrawn
    /// and is never settled by this build.
    pub call_settlement: &'static str,
    /// The figure `Platform::deployable_capital` subtracts before anything
    /// is sized: every commitment's obligation, summed by the book itself.
    pub obligation_total: String,
    pub accrued_default_penalty: String,
    pub commitments: Vec<CommitmentView>,
}

/// The answer to a filing or a withdrawal: the commitment's row, read back
/// after the book adopted the record, beside the sentence saying the notice
/// will never be settled here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommitmentRowView {
    pub posture: &'static str,
    pub served_at: String,
    pub call_settlement: &'static str,
    pub commitment: CommitmentView,
}

/// Render one commitment as the book holds it at `now`.
///
/// Every figure is the type's own — `obligation`, `accrued_default_penalty`,
/// each notice's `penalty_at` — called through the kernel's read-only view
/// of the book; nothing here adds. A commitment the book refuses to read at
/// `now` (one not yet knowable) is a refusal in the body rather than a row
/// with a zero in it.
fn commitment_view(
    commitment: &qip_financial::cashflow::Commitment,
    now: Timestamp,
) -> Result<CommitmentView, String> {
    let mut capital_calls = Vec::new();
    for call in commitment.calls() {
        capital_calls.push(CapitalCallView {
            reference: call.reference().to_string(),
            amount: call.amount().to_string(),
            issued_at: call.issued_at().to_rfc3339(),
            due_at: call.due_at().to_rfc3339(),
            consequence: ConsequenceView::of(call.consequence()),
            overdue: call.is_overdue_at(now),
            days_late: call.days_late_at(now),
            penalty: call
                .penalty_at(now)
                .map_err(|error| error.message().to_string())?
                .to_string(),
        });
    }
    Ok(CommitmentView {
        subject: commitment.subject().to_string(),
        committed: commitment.committed().to_string(),
        called: commitment.called().to_string(),
        unfunded: commitment.unfunded().to_string(),
        accrued_default_penalty: commitment
            .accrued_default_penalty(now)
            .map_err(|error| error.message().to_string())?
            .to_string(),
        obligation: commitment
            .obligation(now)
            .map_err(|error| error.message().to_string())?
            .to_string(),
        known_at: commitment.known_at().to_rfc3339(),
        capital_calls,
    })
}

/// Build `/ledger/commitments` from the platform at `now`.
pub fn commitments(platform: &Platform, now: Timestamp) -> Result<CommitmentsView, String> {
    let book = platform.commitments();
    let mut commitments = Vec::with_capacity(book.len());
    for commitment in book.iter() {
        commitments.push(commitment_view(commitment, now)?);
    }
    Ok(CommitmentsView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        call_settlement: CALL_SETTLEMENT,
        obligation_total: book
            .unfunded_total(now)
            .map_err(|error| error.message().to_string())?
            .to_string(),
        accrued_default_penalty: book
            .accrued_default_penalty(now)
            .map_err(|error| error.message().to_string())?
            .to_string(),
        commitments,
    })
}

/// Why a private position carries no mark.
///
/// The refusal the valuation plane returned, word for word, rather than a
/// null. A record with no residual, no net cost and no schedule cannot be
/// marked, and the plane says which of the three is missing; a surface that
/// rendered an absence instead would tell an operator the platform had not
/// looked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MarkRefusalView {
    pub available: bool,
    pub reason: String,
}

/// The mark on one private position, as the plane struck it.
///
/// `confidence_now` is the struck confidence decayed to the instant served
/// and is the number the platform actually sizes against; `struck_confidence`
/// is what the method carried on the day. Both are here because a position
/// marked at 0.9 eighteen months ago and one marked at 0.55 this morning are
/// different facts that a single figure would flatten. `stale` is the mark's
/// own `is_stale`, not a comparison this layer makes — a mark past its review
/// date is one nothing may be sized into, and the surface says so rather than
/// leaving a reader to compare two instants.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MarkView {
    pub available: bool,
    pub value: String,
    pub method: &'static str,
    pub struck_confidence: f64,
    pub confidence_now: f64,
    pub as_of: String,
    pub next_review: String,
    pub stale: bool,
}

/// One forecast flow, as the administrator's record dates it.
///
/// `amount` is a magnitude and `kind` carries the direction, exactly as
/// `ForecastCashflow` holds them. Nothing is signed or summed here.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ForecastFlowView {
    pub kind: &'static str,
    pub due_at: String,
    pub amount: String,
    pub probability: f64,
}

/// The forecast cash-flow schedule a private record states, or its statement
/// that it states none.
///
/// **This was `DistributionsView`, and the name was a lie about the
/// contents.** It carries whatever `Platform::private_forecast` holds, and
/// since the record's published call schedule began reaching that forecast
/// (`IlliquidValuator::forecast_private_asset` emits a
/// [`qip_financial::cashflow::CashflowKind::CapitalCall`] per scheduled draw
/// beside the residual `Distribution`) a fund that paces its drawdowns
/// rendered its **draws** under a field called `distributions`. The rendering
/// was never wrong — [`ForecastFlowView::kind`] carries the direction per
/// flow, which is exactly why the defect survived: every value in the payload
/// was correct and only the container's name said otherwise. A name that
/// contradicts its contents is the more dangerous half of that, because a
/// reader who trusts the field name never reads the values, and reads a
/// capital call as money coming back.
///
/// `stated: false` is the record saying nothing — no residual reported, no
/// call schedule published, or a lockup already run out — and is deliberately
/// not an empty `flows` array with `stated: true` beside it. "Nothing is
/// scheduled" and "this record schedules nothing" are different claims, and
/// the surface that conflates them tells an operator a fund will never
/// distribute when the truth is that nobody said.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ForecastScheduleView {
    pub stated: bool,
    pub flows: Vec<ForecastFlowView>,
}

/// One of the desk's private positions: what it is marked at and by what
/// method, what it is still on the hook for, and every flow its record dates
/// — the draws it has paced as well as the capital it says will come back.
/// This sentence said only "what its record says will come back" while the
/// field beside it already carried scheduled capital calls.
///
/// `commitment` is absent where the holding has been fully called — a
/// position with nothing unfunded has no commitment in the book and is not
/// thereby less of a position, which is why this surface is walked from the
/// marks rather than from the commitment book.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PrivatePositionView {
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark: Option<MarkView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unmarkable: Option<MarkRefusalView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commitment: Option<CommitmentView>,
    /// Every flow the record schedules, in both directions — see
    /// [`ForecastScheduleView`] on why this is not called `distributions`.
    pub forecast_schedule: ForecastScheduleView,
}

/// The body of `GET /ledger/private-positions`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PrivatePositionsView {
    pub posture: &'static str,
    pub served_at: String,
    /// [`CALL_SETTLEMENT`]: a notice on a position below stands until it is
    /// withdrawn, and is never met here.
    pub call_settlement: &'static str,
    pub positions: Vec<PrivatePositionView>,
}

/// Build `/ledger/private-positions` from the platform at `now`.
///
/// Blueprint §40.1's private-positions surface: commitments, the call
/// schedule, expected distributions, and the mark with its method. Every
/// figure is one the valuation plane struck at assembly and the DECIDE stage
/// already sizes against — `Platform::sizing_confidence` reads the same mark,
/// and `deployable_capital` the same obligation. Until this route existed the
/// marks and the schedules were computed on every assembly and reachable by
/// nothing outside the cycle, which is the shape
/// `.claude/rules/domains/risk-and-execution.md` calls a control nobody can
/// see rather than a spare part.
///
/// Walked over the union of the marked and the unmarkable, in the kernel's
/// own `BTreeMap` order, so a replay renders the same list in the same order.
/// Every private asset in the universe is in exactly one of the two: the
/// sweep marks it or records why it could not.
pub fn private_positions(
    platform: &Platform,
    now: Timestamp,
) -> Result<PrivatePositionsView, String> {
    let marks = platform.illiquid_marks();
    let unmarkable = platform.illiquid_unmarkable();
    let book = platform.commitments();
    let subjects: std::collections::BTreeSet<&str> = marks
        .keys()
        .chain(unmarkable.keys())
        .map(String::as_str)
        .collect();
    let mut positions = Vec::with_capacity(subjects.len());
    for subject in subjects {
        let mark = match marks.get(subject) {
            Some(mark) => Some(MarkView {
                available: true,
                value: mark.value().to_string(),
                method: mark.method().label(),
                struck_confidence: mark.struck_confidence(),
                confidence_now: mark
                    .confidence_at(now)
                    .map_err(|error| error.message().to_string())?,
                as_of: mark.as_of().to_rfc3339(),
                next_review: mark.next_review().to_rfc3339(),
                stale: mark.is_stale(now),
            }),
            None => None,
        };
        let unmarked = unmarkable.get(subject).map(|reason| MarkRefusalView {
            available: false,
            reason: reason.clone(),
        });
        let commitment = match book.get(subject) {
            Some(commitment) => Some(commitment_view(commitment, now)?),
            None => None,
        };
        let forecast_schedule = match platform.private_forecast(subject) {
            Some(forecast) => ForecastScheduleView {
                stated: true,
                flows: forecast
                    .flows()
                    .map(|flow| ForecastFlowView {
                        kind: flow.kind().label(),
                        due_at: flow.due_at().to_rfc3339(),
                        amount: flow.amount().to_string(),
                        probability: flow.probability(),
                    })
                    .collect(),
            },
            None => ForecastScheduleView {
                stated: false,
                flows: Vec::new(),
            },
        };
        positions.push(PrivatePositionView {
            subject: subject.to_string(),
            mark,
            unmarkable: unmarked,
            commitment,
            forecast_schedule,
        });
    }
    Ok(PrivatePositionsView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        call_settlement: CALL_SETTLEMENT,
        positions,
    })
}

/// Build the answer to a filing or a withdrawal against `subject`.
pub fn commitment_row(
    platform: &Platform,
    subject: &str,
    now: Timestamp,
) -> Result<CommitmentRowView, String> {
    let Some(commitment) = platform.commitments().get(subject) else {
        // Unreachable through the routes, which reach this only after the
        // kernel admitted a record against the commitment. Answered rather
        // than indexed, for the reason `inflow_row` gives.
        return Err(format!(
            "the commitment book holds no row for `{subject}` after the record was applied"
        ));
    };
    Ok(CommitmentRowView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        call_settlement: CALL_SETTLEMENT,
        commitment: commitment_view(commitment, now)?,
    })
}

/// What `POST /ledger/commitments/{commitment}/capital-calls` accepts.
///
/// Parsed by hand for the reason the inflow body is: a refusal names the
/// *field* a person must fix. The commitment is not in the body — it is the
/// path's — and the issued instant is the server's, because a notice dated
/// by its caller is a caller-chosen position in the reserve's history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapitalCallBody {
    notice: serde_json::Value,
}

impl CapitalCallBody {
    /// The keys this route reads, and the only ones.
    const FIELDS: [&'static str; 4] = ["reference", "amount", "due", "consequence"];

    pub fn parse(body: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(body).map_err(|_| {
            "the body is not JSON; send {\"reference\": \"<the fund's notice id>\", \"amount\": \
             \"100000.00\", \"due\": \"<RFC 3339>\", \"consequence\": {\"kind\": \"interest\", \
             \"annual_rate_bps\": 800}}"
                .to_string()
        })?;
        let Some(object) = value.as_object() else {
            return Err("the body must be a JSON object carrying a capital call".to_string());
        };
        if let Some(position) = object
            .keys()
            .position(|key| !Self::FIELDS.contains(&key.as_str()))
        {
            // Named by position and never quoted, for the reason every
            // other operator body's refusal is: echoing what a caller sent
            // publishes it to the response and to every log that copies it.
            return Err(format!(
                "the body's key at position {} is not one this route reads; {NO_PAYMENT_FIELD}",
                position + 1
            ));
        }
        // Built from pieces this function validated, never from `object`.
        let notice = serde_json::json!({
            "reference": Self::text(object, "reference")?,
            "amount": Self::amount(object)?,
            "due": Self::due(object)?,
            "consequence": Self::consequence(object)?,
        });
        Ok(Self { notice })
    }

    /// The notice as the kernel's own type.
    ///
    /// Generic so the type is inferred from `Platform::record_capital_call`'s
    /// signature and this crate never names it: the application layer ships
    /// no field typed as money (`api_boundary.rs`), and the exact-decimal
    /// rule is the type's own.
    pub fn notice<D: serde::de::DeserializeOwned>(&self) -> Result<D, String> {
        serde_json::from_value(self.notice.clone())
            .map_err(|error| format!("the capital call was refused: {error}"))
    }

    /// The amount, as text and only as text — a JSON number is refused
    /// rather than converted, for the reason the inflow declaration gives.
    fn amount(object: &serde_json::Map<String, serde_json::Value>) -> Result<String, String> {
        match object.get("amount") {
            None => Err("the body has no `amount`; it is required".to_string()),
            Some(serde_json::Value::String(amount)) if !amount.trim().is_empty() => {
                Ok(amount.trim().to_string())
            }
            Some(serde_json::Value::String(_)) => Err("`amount` is blank".to_string()),
            Some(_) => Err(
                "`amount` must be a JSON string such as \"100000.00\", never a number: a \
                 number is parsed as a float and a demand a parser rounded is not the demand \
                 the fund made"
                    .to_string(),
            ),
        }
    }

    /// The due instant, as an RFC 3339 string the kernel's own clock type
    /// reads. Refused as a number: an epoch figure is a unit nobody stated.
    fn due(object: &serde_json::Map<String, serde_json::Value>) -> Result<String, String> {
        let due = Self::text(object, "due")?;
        if Timestamp::parse_rfc3339(&due).is_none() {
            return Err("`due` must be an RFC 3339 instant with its zone, such as \
                 \"2026-10-01T00:00:00Z\""
                .to_string());
        }
        Ok(due)
    }

    /// The consequence of failing the call, in the shape `ROUTES-LEDGER.md`
    /// writes out: `{"kind": "interest", "annual_rate_bps": N}`,
    /// `{"kind": "forfeiture", "fraction_bps": N}` or
    /// `{"kind": "acceleration"}`, translated to the kernel type's own
    /// serialisation. Required rather than defaulted: the three arms behave
    /// differently in time, and a notice filed without one would reserve
    /// against a penalty nobody stated.
    fn consequence(
        object: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let Some(value) = object.get("consequence") else {
            return Err(
                "the body has no `consequence`; it is required, because what failing the call \
                 costs is part of the notice and is not defaulted here"
                    .to_string(),
            );
        };
        let Some(consequence) = value.as_object() else {
            return Err("`consequence` must be a JSON object with a `kind`".to_string());
        };
        let kind = Self::text(consequence, "kind")?;
        let bps_field = match kind.as_str() {
            "interest" => "annual_rate_bps",
            "forfeiture" => "fraction_bps",
            "acceleration" => {
                if consequence.len() != 1 {
                    return Err(
                        "`consequence.kind` is `acceleration`, which carries no rate; send \
                         `{\"kind\": \"acceleration\"}` and nothing else"
                            .to_string(),
                    );
                }
                return Ok(serde_json::json!("acceleration"));
            }
            _ => {
                return Err(
                    "`consequence.kind` must be `interest`, `forfeiture` or `acceleration`"
                        .to_string(),
                );
            }
        };
        if consequence.len() != 2 {
            return Err(format!(
                "`consequence` of kind `{kind}` reads `kind` and `{bps_field}` and nothing else"
            ));
        }
        let bps = consequence
            .get(bps_field)
            .and_then(serde_json::Value::as_u64)
            .filter(|bps| u32::try_from(*bps).is_ok())
            .ok_or_else(|| {
                format!(
                    "`consequence.{bps_field}` must be a whole number of basis points that fits \
                     in 32 bits; it is a rate, not money, and is the one figure this route reads \
                     as a JSON number"
                )
            })?;
        Ok(serde_json::json!({ kind: { bps_field: bps } }))
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
            return Err(format!("`{field}` is blank"));
        }
        Ok(text.to_string())
    }
}

// --- POST /ledger/users/{user}/investment-requests ---------------------------

/// The sentence every unknown key on an investment-request body is refused
/// with.
///
/// It answers the mistake this shape invites, which is not a typo: §40.9's
/// investment step ends in a *funded* position, and a caller who sends
/// `fund`, `execute` or `order` believes this route places one. A key
/// silently ignored would let them keep believing it until they wondered why
/// nothing traded.
pub const NO_FUNDING_FIELD: &str = "an investment request reads `strategy`, `family`, `currency`, \
    `amount` and `reason`, and nothing else. It raises a request and funds nothing: the mandate \
    decides whether this much could be put to work at this strategy now, and there is no field \
    on this route that could place an order or move capital";

/// What `POST /ledger/users/{user}/investment-requests` accepts.
///
/// Parsed by hand for the reason the eligibility body is: a refusal here has
/// to name the *field* a person must fix rather than quote a Rust type.
/// `user` and `requested_at` are not read from the body — the user is the
/// path's, resolved against the mandate registry, and the instant is the
/// server's clock. A caller-stated instant is a caller-chosen position in the
/// mandate's history, and the ledger decides against the books as they are
/// now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvestmentRequestBody {
    request: serde_json::Value,
    reason: String,
}

impl InvestmentRequestBody {
    /// The keys the body may carry, and the only ones.
    const FIELDS: [&'static str; 5] = ["strategy", "family", "currency", "amount", "reason"];

    pub fn parse(body: &str, user: &str, now: Timestamp) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(body).map_err(|_| {
            "the body is not JSON; send {\"strategy\": \"<id>\", \"family\": \"<family>\", \
             \"currency\": \"USD\", \"amount\": \"1000.00\", \"reason\": \"<why>\"}"
                .to_string()
        })?;
        let Some(object) = value.as_object() else {
            return Err(
                "the body must be a JSON object carrying an investment request".to_string(),
            );
        };
        if let Some(position) = object
            .keys()
            .position(|key| !Self::FIELDS.contains(&key.as_str()))
        {
            // Named by position and never quoted, for the reason the
            // eligibility body's refusal is: echoing what a caller sent
            // publishes it to the response and to every log that copies it.
            return Err(format!(
                "the body's key at position {} is not one this route reads; {NO_FUNDING_FIELD}",
                position + 1
            ));
        }
        let reason = Self::text(object, "reason")?;
        // Built from pieces this function validated, never from `object`, so
        // no key this route does not read can reach the ledger by accident —
        // and `user` and `requested_at` are supplied here rather than
        // accepted.
        let request = serde_json::json!({
            "user": user,
            "strategy": Self::text(object, "strategy")?,
            "family": Self::text(object, "family")?,
            "currency": Self::text(object, "currency")?,
            "amount": Self::amount(object)?,
            "requested_at": now.to_rfc3339(),
        });
        Ok(Self { request, reason })
    }

    /// The request as the kernel's own type.
    ///
    /// Generic so the type is inferred from `Platform::decide_investment`'s
    /// signature and this crate never names it: the application layer ships
    /// no edge to `qip-capital` (`api_boundary.rs`), and every rule the type
    /// keeps — a valid user id, a currency code, an exact decimal — is
    /// enforced by the type rather than restated here where a second copy
    /// would be free to drift.
    pub fn request<R: serde::de::DeserializeOwned>(&self) -> Result<R, String> {
        serde_json::from_value(self.request.clone())
            .map_err(|error| format!("the investment request was refused: {error}"))
    }

    /// Why the request was raised, for the audit trail. The kernel holds it
    /// to a length of its own; this only refuses a blank.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// The amount, as text and only as text.
    ///
    /// A JSON number is refused rather than converted. `1000.10` does not
    /// exist as an IEEE double, and a request for someone's capital rounded
    /// by the parser is the exact failure `Decimal` exists to prevent — the
    /// rest of this surface renders money as strings for the same reason.
    fn amount(object: &serde_json::Map<String, serde_json::Value>) -> Result<String, String> {
        match object.get("amount") {
            None => Err("the body has no `amount`; it is required".to_string()),
            Some(serde_json::Value::String(amount)) if !amount.trim().is_empty() => {
                Ok(amount.trim().to_string())
            }
            Some(serde_json::Value::String(_)) => Err("`amount` is blank".to_string()),
            Some(_) => Err(
                "`amount` must be a JSON string such as \"1000.00\", never a number: a \
                            number is parsed as a float and an amount of capital that a parser \
                            rounded is not the amount anyone asked for"
                    .to_string(),
            ),
        }
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
            return Err(format!("`{field}` is blank"));
        }
        Ok(text.to_string())
    }
}

/// The request as the ledger understood it, echoed back from the ledger's own
/// record of the decision rather than from what was posted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InvestmentRequestView {
    pub user_id: String,
    pub strategy: String,
    pub family: String,
    pub currency: String,
    pub amount: String,
    pub requested_at: String,
}

/// What the investment-request route answers.
///
/// `admitted` is the mandate's verdict; `refused_limit` is the ledger's own
/// variant name for the gate that refused — a value a page can group on and a
/// test can assert — and `detail` is its sentence, or the basis of a grant.
///
/// `funded` is a literal `false` that is not decoration. §40.9's investment
/// step ends in capital at work, and an interface that showed an admitted
/// request without saying it moved nothing would be read as a funding by
/// every person who saw it. The user's row travels with the answer so the
/// figures the verdict was reached against — mandate, balances, eligibility —
/// are readable beside it rather than fetched again from a ledger that may
/// have moved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InvestmentDecisionView {
    pub posture: &'static str,
    pub served_at: String,
    pub decided_at: String,
    pub request: InvestmentRequestView,
    pub admitted: bool,
    pub funded: bool,
    pub refused_limit: Option<String>,
    pub detail: String,
    pub user: UserView,
}

/// Build the answer to an investment request from the decision the ledger
/// returned, read through its own serialisation.
///
/// The API may not name `InvestmentDecision`, so the decision arrives as the
/// JSON it serialises to and every field is read out of that. A shape this
/// reader does not understand is an error rather than a defaulted body: a
/// variant added to the ledger's outcome enum must surface here and not as a
/// request that silently reads refused.
pub fn decided_investment(
    platform: &Platform,
    decision: &serde_json::Value,
    user: &str,
    now: Timestamp,
) -> Result<InvestmentDecisionView, String> {
    let field = |path: &[&str]| -> Result<String, String> {
        let mut value = decision;
        for key in path {
            value = value
                .get(*key)
                .ok_or_else(|| format!("the decision carries no {}: {decision}", path.join(".")))?;
        }
        value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("{} is not a string: {decision}", path.join(".")))
    };
    let outcome = decision
        .get("outcome")
        .ok_or_else(|| format!("the decision carries no outcome: {decision}"))?;
    let (admitted, refused_limit, detail) = if let Some(arm) = outcome.get("Admitted") {
        let basis = arm
            .get("basis")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("an admitted decision carries no basis: {decision}"))?;
        (true, None, basis.to_string())
    } else if let Some(arm) = outcome.get("Refused") {
        let reason = arm
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("a refused decision carries no reason: {decision}"))?;
        // The limit is an externally tagged enum where it carries a payload
        // (`{"Eligibility":"expired"}`) and a bare string where it does not,
        // so the name is the tag in the first case and the string in the
        // second. Read rather than matched: the API cannot name the type.
        let limit = arm
            .get("limit")
            .ok_or_else(|| format!("a refused decision names no limit: {decision}"))?;
        let name = match limit {
            serde_json::Value::String(name) => name.clone(),
            serde_json::Value::Object(map) => map
                .keys()
                .next()
                .cloned()
                .ok_or_else(|| format!("the limit is an empty object: {decision}"))?,
            _ => return Err(format!("the limit is neither a name nor a tag: {decision}")),
        };
        (false, Some(name), reason.to_string())
    } else {
        return Err(format!(
            "the decision's outcome is neither Admitted nor Refused: {decision}"
        ));
    };
    let mut rows = user_rows(platform, now, Some(user))?;
    if rows.len() != 1 {
        // Unreachable through the route, which resolves the user against the
        // mandate registry first. Answered rather than indexed: a panic here
        // would poison the lock every other route waits on.
        return Err(format!(
            "the ledger holds {} rows for `{user}`; it must hold exactly one",
            rows.len()
        ));
    }
    Ok(InvestmentDecisionView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        decided_at: field(&["decided_at"])?,
        request: InvestmentRequestView {
            user_id: field(&["request", "user"])?,
            strategy: field(&["request", "strategy"])?,
            family: field(&["request", "family"])?,
            currency: field(&["request", "currency"])?,
            amount: field(&["request", "amount"])?,
            requested_at: field(&["request", "requested_at"])?,
        },
        admitted,
        funded: false,
        refused_limit,
        detail,
        user: rows.remove(0),
    })
}

// --- /wallet ----------------------------------------------------------------

/// One observed holding paired with the ledger's expectation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HoldingView {
    pub venue: String,
    pub asset: String,
    pub observed_quantity: String,
    pub observed_at: String,
    pub provenance: String,
    /// `ledger_balance - reserved + in_flight`, or `null` when the ledger
    /// books nothing at this venue-asset — which reconciliation reports as
    /// a halt, `unrecorded_by_ledger`, rather than as a zero expectation
    /// somebody chose.
    pub ledger_expected: Option<String>,
}

/// The reconciliation half of the wallet body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReconciliationView {
    /// The fabric's own outcome records, tagged `outcome`, in venue-asset
    /// order.
    pub outcomes: Vec<serde_json::Value>,
    pub halted_venue_assets: usize,
}

/// The body of `GET /wallet`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WalletView {
    pub posture: &'static str,
    pub served_at: String,
    pub assembled: bool,
    pub reason: Option<String>,
    pub as_of: Option<String>,
    pub holdings: Vec<HoldingView>,
    pub reconciliation: ReconciliationView,
}

/// Build `/wallet` at `now`, from the wallet the kernel's fabric journal
/// last assembled.
///
/// `assembled` is whether the journal's state holds a wallet — it does once
/// a statement has been handed to the kernel and a cycle's LEARN stage has
/// assembled against it — and the holdings and outcomes are the journal's
/// own, not a copy the API keeps. The one arithmetic here, the ledger's
/// expectation, is the fabric's own checked sum called through the view the
/// kernel holds; an overflow is a refusal in the body rather than a number.
pub fn wallet(platform: &Platform, now: Timestamp) -> Result<WalletView, String> {
    let state = platform.fabric_state();
    let Some(assembled) = state.wallet() else {
        return Ok(WalletView {
            posture: POSTURE,
            served_at: now.to_rfc3339(),
            assembled: false,
            reason: Some(NO_WALLET.to_string()),
            as_of: None,
            holdings: Vec::new(),
            reconciliation: ReconciliationView {
                outcomes: Vec::new(),
                halted_venue_assets: 0,
            },
        });
    };
    let mut holdings = Vec::new();
    for key in assembled.venue_assets() {
        let Some(observation) = assembled.observation(key) else {
            // `venue_assets` is the observed set, so this arm is unreachable
            // through the wallet's own API; stated rather than unwrapped.
            continue;
        };
        let ledger_expected = match assembled.ledger_view(key) {
            Some(view) => Some(
                view.expected()
                    .map_err(|error| error.message().to_string())?
                    .to_string(),
            ),
            None => None,
        };
        holdings.push(HoldingView {
            venue: key.venue.to_string(),
            asset: key.asset.to_string(),
            observed_quantity: observation.observed.to_string(),
            observed_at: observation.observed_at.to_rfc3339(),
            provenance: observation.provenance.as_str().to_string(),
            ledger_expected,
        });
    }
    let mut outcomes = Vec::with_capacity(state.reconciliations().len());
    let mut halted_venue_assets = 0;
    for outcome in state.reconciliations().values() {
        if outcome.is_halt() {
            halted_venue_assets += 1;
        }
        outcomes.push(serde_json::to_value(outcome).map_err(|error| error.to_string())?);
    }
    Ok(WalletView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        assembled: true,
        reason: None,
        as_of: Some(assembled.as_of().to_rfc3339()),
        holdings,
        reconciliation: ReconciliationView {
            outcomes,
            halted_venue_assets,
        },
    })
}

// --- /corridors -------------------------------------------------------------

/// A corridor's caps, as text and seconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CorridorCapsView {
    pub max_per_transfer: String,
    pub max_per_hour: String,
    pub max_per_day: String,
    pub max_cumulative: String,
    pub min_interval_seconds: i64,
    pub permitted_hours: PermittedHoursView,
}

/// A half-open window `[start, end)` in whole hours.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PermittedHoursView {
    pub start: u32,
    pub end: u32,
}

/// Where capital sits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LocationView {
    pub region: String,
    pub currency: String,
    pub venue: String,
}

/// An allowlisted destination's key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DestinationKeyView {
    pub asset: String,
    pub address: String,
}

/// One corridor record, as the journal built it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CorridorView {
    pub id: String,
    pub source: LocationView,
    pub source_class: String,
    pub kind: String,
    pub destination: DestinationKeyView,
    pub caps: CorridorCapsView,
    pub purpose: String,
    pub stage: String,
    pub proposed_by: String,
    pub proposed_at: String,
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<String>,
    pub signed: bool,
    pub activation_at: Option<String>,
}

/// One destination record, as the journal built it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DestinationView {
    pub asset: String,
    pub address: String,
    pub status: String,
    pub proposed_by: String,
    pub proposed_at: String,
    pub usable_from: Option<String>,
}

/// A registry the process holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RegistryView<T> {
    pub held: bool,
    /// Kept for the contract; `null` now that both registries are held.
    pub reason: Option<String>,
    pub records: Vec<T>,
}

/// The body of `GET /corridors`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CorridorsView {
    pub posture: &'static str,
    pub served_at: String,
    pub corridors: RegistryView<CorridorView>,
    pub destinations: RegistryView<DestinationView>,
}

/// Build `/corridors` at `now`, from the registries the kernel's fabric
/// journal holds.
///
/// Both are held from assembly — an allowlist that permits nothing and a
/// corridor map with nothing in it are real, safe states — and every record
/// in them is one a command through the journal proposed, in id order. A
/// destination's `usable_from` is read from its status's own serialisation
/// (`{"signed": {"usable_from": ...}}`), because the API cannot name the
/// status enum to match on it.
pub fn corridors(platform: &Platform, now: Timestamp) -> Result<CorridorsView, String> {
    let state = platform.fabric_state();
    let mut corridors = Vec::with_capacity(state.corridors().len());
    for (id, corridor) in state.corridors() {
        let (proposed_by, proposed_at) = corridor.proposed();
        let reviewed = corridor.reviewed();
        let caps = corridor.caps();
        let hours = caps.permitted_hours();
        corridors.push(CorridorView {
            id: id.as_str().to_string(),
            source: LocationView {
                region: corridor.source().region.as_str().to_string(),
                currency: corridor.source().currency.to_string(),
                venue: corridor.source().venue.to_string(),
            },
            source_class: corridor.source_class().as_str().to_string(),
            kind: corridor.kind().as_str().to_string(),
            destination: DestinationKeyView {
                asset: corridor.destination().asset.as_str().to_string(),
                address: corridor.destination().address.clone(),
            },
            caps: CorridorCapsView {
                max_per_transfer: caps.max_per_transfer().to_string(),
                max_per_hour: caps.max_per_hour().to_string(),
                max_per_day: caps.max_per_day().to_string(),
                max_cumulative: caps.max_cumulative().to_string(),
                min_interval_seconds: caps.min_interval().as_millis() / 1000,
                permitted_hours: PermittedHoursView {
                    start: hours.start(),
                    end: hours.end(),
                },
            },
            purpose: corridor.purpose().to_string(),
            stage: corridor.stage().as_str().to_string(),
            proposed_by: proposed_by.as_str().to_string(),
            proposed_at: proposed_at.to_rfc3339(),
            reviewed_by: reviewed.map(|(by, _)| by.as_str().to_string()),
            reviewed_at: reviewed.map(|(_, at)| at.to_rfc3339()),
            signed: corridor.is_signed(),
            activation_at: corridor.activation_at().map(Timestamp::to_rfc3339),
        });
    }
    let mut destinations = Vec::with_capacity(state.destinations().len());
    for (key, record) in state.destinations().iter() {
        let status = serde_json::to_value(&record.status).map_err(|error| error.to_string())?;
        let usable_from = status
            .get("signed")
            .and_then(|signed| signed.get("usable_from"))
            .and_then(|at| at.as_str())
            .map(str::to_string);
        destinations.push(DestinationView {
            asset: key.asset.as_str().to_string(),
            address: key.address.clone(),
            status: record.status.as_str().to_string(),
            proposed_by: record.proposed_by.as_str().to_string(),
            proposed_at: record.proposed_at.to_rfc3339(),
            usable_from,
        });
    }
    Ok(CorridorsView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        corridors: RegistryView {
            held: true,
            reason: None,
            records: corridors,
        },
        destinations: RegistryView {
            held: true,
            reason: None,
            records: destinations,
        },
    })
}

// --- /transfer-gate ---------------------------------------------------------

/// One of the gate's seven checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GateCheckView {
    /// 1-based position in assessment order.
    pub order: usize,
    pub name: String,
    /// Whether a veto by this check is paired with an alert to a person.
    pub alerts: bool,
}

/// The newest gate assessment the journal holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AssessmentView {
    pub corridor: String,
    pub assessed_at: String,
    /// `"approved"` or `"vetoed"`.
    pub outcome: String,
    /// The check that vetoed, or `null` for an approval.
    pub check: Option<String>,
    pub reason: Option<String>,
    pub alert: bool,
}

impl AssessmentView {
    /// Read a verdict from its own serialisation: `{"admitted": {...}}` or
    /// `{"vetoed": {"check", "reason", "alert", "assessed_at"}}`. Any other
    /// tag is refused as a shape this reader does not understand.
    fn from_serialised(corridor: String, verdict: &impl Serialize) -> Result<Self, String> {
        let value = serde_json::to_value(verdict).map_err(|error| error.to_string())?;
        let at_of = |arm: &serde_json::Value| {
            arm.get("assessed_at")
                .and_then(|at| at.as_str())
                .map(str::to_string)
                .ok_or_else(|| format!("an assessment without an assessed_at: {value}"))
        };
        if let Some(admitted) = value.get("admitted") {
            return Ok(Self {
                corridor,
                assessed_at: at_of(admitted)?,
                outcome: "approved".to_string(),
                check: None,
                reason: None,
                alert: false,
            });
        }
        if let Some(vetoed) = value.get("vetoed") {
            return Ok(Self {
                corridor,
                assessed_at: at_of(vetoed)?,
                outcome: "vetoed".to_string(),
                check: vetoed
                    .get("check")
                    .and_then(|check| check.as_str())
                    .map(str::to_string),
                reason: vetoed
                    .get("reason")
                    .and_then(|reason| reason.as_str())
                    .map(str::to_string),
                alert: vetoed
                    .get("alert")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            });
        }
        Err(format!(
            "a gate verdict serialised as neither admitted nor vetoed: {value}"
        ))
    }
}

/// The platform's kill switch, as the gate's seventh check would read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KillSwitchView {
    pub halted: bool,
    pub halted_scopes: Vec<String>,
    pub tripped_by: Option<String>,
    pub reason: Option<String>,
    pub tripped_at: Option<String>,
}

/// The body of `GET /transfer-gate`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TransferGateView {
    pub posture: &'static str,
    pub served_at: String,
    pub checks: Vec<GateCheckView>,
    pub last_assessment: Option<AssessmentView>,
    pub kill_switch: KillSwitchView,
    /// Constant `false`: the gate cannot move anything.
    pub executes: bool,
    pub note: &'static str,
}

/// Build `/transfer-gate` at `now`.
///
/// The checks come from the kernel's pass-through of the fabric's own
/// roster, the kill switch from the platform's controller, and
/// `last_assessment` from the newest assessment the fabric journal holds —
/// `None` while none has been made, which is a fact about the journal and
/// not a cache that happens to be empty.
pub fn transfer_gate(platform: &Platform, now: Timestamp) -> Result<TransferGateView, String> {
    let checks = Platform::transfer_gate_checks()
        .iter()
        .enumerate()
        .map(|(index, check)| GateCheckView {
            order: index + 1,
            name: check.as_str().to_string(),
            alerts: check.alerts(),
        })
        .collect();
    let last_assessment = match platform.fabric_state().assessments().last() {
        Some(assessment) => Some(AssessmentView::from_serialised(
            assessment.corridor.as_str().to_string(),
            &assessment.verdict,
        )?),
        None => None,
    };
    let switch = platform.autonomy().kill_switch();
    let trip = switch.global_trip();
    Ok(TransferGateView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        checks,
        last_assessment,
        kill_switch: KillSwitchView {
            halted: switch.is_globally_tripped(),
            halted_scopes: switch
                .halted_scopes()
                .iter()
                .map(|scope| (*scope).to_string())
                .collect(),
            tripped_by: trip.map(|trip| trip.tripped_by.clone()),
            reason: trip.map(|trip| trip.reason.clone()),
            tripped_at: trip.map(|trip| trip.at.to_rfc3339()),
        },
        executes: false,
        note: GATE_NOTE,
    })
}

/// Serialise a view, or say in the body that serialisation failed.
///
/// A serde failure on a derived `Serialize` of plain strings and integers
/// cannot happen, and the arm is written anyway so the handler stays free of
/// `unwrap` — a 500 with a reason beats a panic under the platform lock,
/// which would poison it for every other route.
pub fn render(view: &impl Serialize) -> (u16, String) {
    match serde_json::to_string(view) {
        Ok(body) => (200, body),
        Err(error) => (
            500,
            format!(
                r#"{{"error":{}}}"#,
                json::string(&format!("the view did not serialise: {error}"))
            ),
        ),
    }
}

/// A view that may refuse, rendered: the body on success, a 500 naming the
/// refusal otherwise. Shared by the three treasury routes whose builders
/// can refuse, so a refusal reads the same on each.
pub fn render_fallible(view: Result<impl Serialize, String>) -> (u16, String) {
    match view {
        Ok(view) => render(&view),
        Err(reason) => (500, format!(r#"{{"error":{}}}"#, json::string(&reason))),
    }
}
