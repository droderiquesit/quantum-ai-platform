//! The JSON shapes of the treasury read surface: `/ledger/users`, `/wallet`,
//! `/corridors` and `/transfer-gate`.
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
pub const GATE_NOTE: &str = "the gate is veto-only and has no transfer engine behind it: an \
    approval is a record that the seven checks passed, and nothing in this platform consumes \
    one. An intent reaches the gate only through the kernel's fabric journal, and every \
    assessment is a record in the event log; last_assessment is the newest, or null when none \
    has been made.";

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
    /// `settled - reserved`. Expected inflows are not in it.
    pub available: String,
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

/// The body of `GET /ledger/users`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LedgerUsersView {
    pub posture: &'static str,
    pub served_at: String,
    pub evaluated_as_role: &'static str,
    pub products: Vec<String>,
    pub fills_journalled: u64,
    pub users: Vec<UserView>,
}

/// Build `/ledger/users` from the platform at `now`.
///
/// Every figure is read at request time; nothing is cached between calls,
/// because a mandate that changed or a fill that landed is reflected on the
/// next read rather than the next restart. Users are every holder in the
/// kernel's registry — the desk and each mandate the configuration enrolled
/// — in user-id order, and a user's balances are the books the kernel's
/// pro-rata split and funding actually moved.
pub fn ledger_users(platform: &Platform, now: Timestamp) -> Result<LedgerUsersView, String> {
    let ledger = platform.user_ledger();
    let products: Vec<String> = platform
        .central()
        .factory()
        .candidates()
        .map(|candidate| candidate.family().as_str().to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let entitlements = platform.viewer_entitlements(now);

    let mut users = Vec::with_capacity(ledger.mandates().len());
    for (user, mandate) in ledger.mandates() {
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
    Ok(LedgerUsersView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        evaluated_as_role: EVALUATED_AS_ROLE,
        products,
        fills_journalled: ledger.fills_journalled(),
        users,
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
