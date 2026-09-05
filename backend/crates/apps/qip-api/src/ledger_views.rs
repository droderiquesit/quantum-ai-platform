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
//!   entitlement it evaluated, a mandate it holds — and the API cannot
//!   construct a mandate, an entitlement, a corridor or an intent of its own.
//! * The withdrawal capability's `granted` flag is read from the type's own
//!   serialisation rather than written as a literal: the type has one arm,
//!   `Refused`, and the flag is whether a `Granted` arm was serialised. A
//!   literal `false` would survive the day someone added the arm the ADR
//!   refuses; this reads `true` that day, and the test that pins it fires.
//!
//! What the process does not hold — a wallet, a corridor registry, a
//! destination allowlist, a gate assessment — is stated with a flag and a
//! reason, in the same way `crate::missing` states the rest. Those reasons
//! live here rather than there only because this surface was added under a
//! path constraint; they belong in `crate::missing` and read as its entries.

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

/// Why `/wallet` has no wallet behind it.
pub const NO_WALLET: &str = "no wallet is assembled in this process. A wallet is a read model \
    over holdings observed through read-only channels, and the kernel observes no custodian, \
    venue balance or chain address; until an observation source is wired in there is nothing \
    to pair with the ledger, and a wallet showing zero would read as an empty account rather \
    than an unobserved one.";

/// Why `/corridors` lists no corridor.
pub const NO_CORRIDOR_REGISTRY: &str = "no corridor registry is held in this process. A \
    corridor is the signed record of where capital may go and under what caps; the kernel \
    composes no treasury and has proposed, reviewed or signed none, so there is no corridor to \
    list — not an empty registry that admits nothing, but no registry at all.";

/// Why `/corridors` lists no destination.
pub const NO_DESTINATION_ALLOWLIST: &str = "no destination allowlist is held in this process. \
    A destination is proposed, verified by a person with the institution and signed before it \
    is usable, and the kernel holds no allowlist for any of that to have happened in; there is \
    no destination to list.";

/// Why a user has no entitlement rows.
pub const NO_PRODUCTS: &str = "no product to evaluate against: an entitlement is decided per \
    strategy family the central factory has registered, and none is registered in this process.";

/// What `/transfer-gate` says about itself.
pub const GATE_NOTE: &str = "the gate is veto-only and has no transfer engine behind it: an \
    approval is a record that the seven checks passed, and nothing in this platform consumes \
    one. No caller has yet assessed an intent.";

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

/// One enrolled user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UserView {
    pub user_id: String,
    pub mandate: MandateView,
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
/// next read rather than the next restart.
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

/// One observed holding paired with the ledger's expectation. Never built in
/// this deployment; the shape is the contract a page renders once a wallet
/// is assembled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HoldingView {
    pub venue: String,
    pub asset: String,
    pub observed_quantity: String,
    pub observed_at: String,
    pub provenance: String,
    pub ledger_expected: String,
}

/// The reconciliation half of the wallet body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReconciliationView {
    /// The fabric's own outcome records, tagged `outcome`.
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

/// Build `/wallet` at `now`.
///
/// The kernel holds no wallet and this process observes no holding, so the
/// body says so. It takes the platform anyway, rather than nothing, so the
/// day the kernel grows a wallet accessor the route changes here and not in
/// its signature — and so a test that passes a platform is exercising the
/// same path a deployment does.
pub fn wallet(_platform: &Platform, now: Timestamp) -> WalletView {
    WalletView {
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
    }
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

/// One corridor record. Never built in this deployment.
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

/// One destination record. Never built in this deployment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DestinationView {
    pub asset: String,
    pub address: String,
    pub status: String,
    pub proposed_by: String,
    pub proposed_at: String,
    pub usable_from: Option<String>,
}

/// A registry the process may or may not hold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RegistryView<T> {
    pub held: bool,
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

/// Build `/corridors` at `now`. Neither registry is held; see [`wallet`] for
/// why the platform is still taken.
pub fn corridors(_platform: &Platform, now: Timestamp) -> CorridorsView {
    CorridorsView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        corridors: RegistryView {
            held: false,
            reason: Some(NO_CORRIDOR_REGISTRY.to_string()),
            records: Vec::new(),
        },
        destinations: RegistryView {
            held: false,
            reason: Some(NO_DESTINATION_ALLOWLIST.to_string()),
            records: Vec::new(),
        },
    }
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

/// An assessment, were one ever recorded. None ever is in this process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AssessmentView {
    pub assessed_at: String,
    pub outcome: String,
    pub check: Option<String>,
    pub reason: Option<String>,
    pub alert: bool,
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
/// roster, the kill switch from the platform's controller. `last_assessment`
/// is `None` because nothing in this process calls the gate; it is not a
/// cache that happens to be empty.
pub fn transfer_gate(platform: &Platform, now: Timestamp) -> TransferGateView {
    let checks = Platform::transfer_gate_checks()
        .iter()
        .enumerate()
        .map(|(index, check)| GateCheckView {
            order: index + 1,
            name: check.as_str().to_string(),
            alerts: check.alerts(),
        })
        .collect();
    let switch = platform.autonomy().kill_switch();
    let trip = switch.global_trip();
    TransferGateView {
        posture: POSTURE,
        served_at: now.to_rfc3339(),
        checks,
        last_assessment: None,
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
    }
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
