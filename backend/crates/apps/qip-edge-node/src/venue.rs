//! Which venue this node's orders actually reach, decided once and said aloud.
//!
//! Until this module existed the answer was fixed in code: [`SimulatedGateway`]
//! and nothing else, because that was the only adapter the binary named.
//! `qip_brokers::rest::RestOrderEntryAdapter` opens a socket and is fully
//! tested, and nothing instantiated it. This module is the seam where a
//! deployment chooses between the two — and, much more importantly, the seam
//! where the dangerous choice is refused unless somebody typed the destination
//! out by hand.
//!
//! # The default is the simulator, and silence selects it
//!
//! A deployment that sets nothing gets exactly what it got before: the
//! in-process matching engine, no egress, no counterparty, no money. Every
//! variable this module reads is absent in that case and every one of them has
//! to be *present* to move the node off it. There is no configuration that
//! makes the live path the fallback, because the failure mode of a fallback is
//! that forgetting something is the dangerous case.
//!
//! # The opt-in is the endpoint, typed twice
//!
//! `RestOrderEntryAdapter::REQUIREMENTS[0]` records the fact this module is
//! arranged around: **nothing in the platform's code can tell a venue's
//! sandbox host from its production host.** Point the adapter at the latter
//! and the orders are real while `Broker::is_simulated` still answers `true`,
//! because [`AdapterClass`] has no variant that could say otherwise. No
//! parser, no allow-list and no host-name heuristic fixes that; a venue's
//! production host is whatever the venue says it is, and this process has no
//! way to ask.
//!
//! So the control is not a check, it is a witness. Selecting the REST adapter
//! requires [`ACKNOWLEDGEMENT_VARIABLE`] to be set to the *exact* value of the
//! endpoint, character for character. That has two properties a boolean flag
//! does not:
//!
//! * It cannot be set once and forgotten. Changing the endpoint — which is
//!   precisely the sandbox-to-production edit — invalidates the
//!   acknowledgement, and the node refuses to start until somebody types the
//!   new destination out.
//! * It appears in the deployment's own configuration, next to the endpoint,
//!   where a reviewer reading the diff sees both halves.
//!
//! # And when the ceiling permits live trading, that is not enough
//!
//! Two independent things have to be true before this process can put real
//! risk on: an adapter that can reach a venue, and an autonomy ceiling that
//! permits live execution. Either alone is survivable. Together they are the
//! combination this node refuses without a second, per-venue enablement —
//! `QIP_<VENUE>_ENABLED`, the variable `qip_brokers::credential`'s own
//! requirement list already names, with the reasoning this module borrows
//! verbatim: *holding a credential is not the same as having decided to
//! trade*. It must carry the venue's own id, so an environment block copied
//! from another venue's deployment does not silently enable this one.
//!
//! # What this module does not promise
//!
//! * **It does not make the REST adapter safe.** It decides *whether* the
//!   adapter is constructed and makes the choice loud. Everything after that —
//!   the unknown-order accounting, the idempotency rule, the refusal to infer
//!   a fill — belongs to `qip_brokers::rest`, which is where it is tested.
//! * **It does not verify the endpoint.** See above. It cannot, and the banner
//!   says so in as many words rather than implying a check happened.
//! * **It does not authenticate anything itself.** The secret is read from the
//!   environment and handed to the adapter inside a
//!   [`Secret`], which redacts in `Debug` and cannot be serialised. Whether
//!   the venue accepts it is settled by the venue, on a socket, at bring-up.
//! * **It reads no clock and opens no connection.** [`VenueChoice::read`] is a
//!   pure function of an environment lookup, which is what lets the tests
//!   exercise every refusal without mutating a process-global environment.
//!
//! [`SimulatedGateway`]: crate::gateway::SimulatedGateway
//! [`AdapterClass`]: qip_brokers::adapter::AdapterClass

use qip_brokers::credential::{
    RequirementKind, Secret, VenueRequirement, requirements_of_kind, standard_requirements,
};
use qip_brokers::rest::{IdempotencySupport, RestVenueConfig};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};

/// Names the venue adapter the node sends orders through.
///
/// Absent or `simulated` selects the in-process matching engine, which is what
/// every deployment of this binary has had until now.
pub const ADAPTER_VARIABLE: &str = "QIP_VENUE_ADAPTER";

/// The operator's acknowledgement of where the orders are going.
///
/// Must equal the venue's endpoint exactly. See the module documentation: this
/// is a witness, not a check, and it is the only control that exists for a
/// distinction the code cannot draw.
pub const ACKNOWLEDGEMENT_VARIABLE: &str = "QIP_VENUE_ORDER_ENTRY_ACKNOWLEDGED";

/// Declares what the venue does with a repeated idempotency key.
///
/// `absent` — the default — makes the adapter refuse to retry an ambiguous
/// submit, which loses orders rather than duplicating them.
pub const IDEMPOTENCY_VARIABLE: &str = "QIP_VENUE_IDEMPOTENCY";

/// The value of [`ADAPTER_VARIABLE`] that selects the in-process exchange.
pub const SIMULATED_ADAPTER: &str = "simulated";

/// The value of [`ADAPTER_VARIABLE`] that selects the REST order-entry adapter.
pub const REST_ADAPTER: &str = "rest";

/// The seed the simulated venue draws its rejections from.
pub const SEED_VARIABLE: &str = "QIP_GATEWAY_SEED";

/// Marks every banner line that names the order destination.
///
/// One fixed prefix, so an operator scanning a start-up log and a `grep` in a
/// deployment check are looking for the same string. It is loud on purpose:
/// the one question this banner answers is the one whose wrong answer is not
/// recoverable.
pub const DESTINATION_PREFIX: &str = "qip-edge-node: ORDER DESTINATION";

/// Where the node will send the cell's orders.
///
/// Deliberately not `Default`: there is no such thing as a venue choice nobody
/// made, and the one that would be defaulted to is the one a test must be able
/// to see was *selected*.
#[derive(Debug)]
pub enum VenueChoice {
    /// The in-process matching engine. Nothing leaves this process.
    Simulated {
        /// The rejection and matching stream, so a session replays.
        seed: u64,
    },
    /// A venue reached over HTTP by `qip_brokers::rest`.
    Live(LiveVenueChoice),
}

/// Everything the REST order-entry adapter needs, resolved from the
/// environment and acknowledged by an operator.
///
/// `Debug` is derived because every field that could leak is already a type
/// that redacts: [`Secret`] prints `<redacted>` and cannot be serialised at
/// all.
#[derive(Debug)]
pub struct LiveVenueChoice {
    /// The venue whose account the orders are sent for.
    pub venue: VenueId,
    /// `http://host[:port]`. The field that decides whether the orders are
    /// real, and the one nothing here can classify.
    pub endpoint: String,
    /// The account the venue books the fills against.
    pub account: String,
    /// The session secret, held in a type that refuses to print itself.
    pub credential: Secret,
    /// What the deployment says the venue does with a repeated key.
    pub idempotency: IdempotencySupport,
    /// Whether the autonomy ceiling permitted live execution when this was
    /// read. Carried so the banner states both halves of the combination
    /// rather than only the one this module chose.
    pub ceiling_permits_live: bool,
}

impl LiveVenueChoice {
    /// The adapter configuration this choice implies.
    ///
    /// Everything not named here stays at `RestVenueConfig`'s default, which
    /// is the losing-safely default: `IdempotencySupport::Absent` unless the
    /// deployment says otherwise, and transport limits sized for order entry.
    pub fn adapter_config(&self) -> RestVenueConfig {
        RestVenueConfig {
            base_url: Some(self.endpoint.clone()),
            idempotency: self.idempotency,
            ..RestVenueConfig::default()
        }
    }
}

impl VenueChoice {
    /// Read the choice from the process environment.
    ///
    /// `ceiling_permits_live` comes from the cell's own autonomy controller
    /// rather than from configuration, so the dangerous combination is
    /// detected from what the cell will actually allow and not from what a
    /// variable claims about it.
    pub fn from_env(venue: &VenueId, ceiling_permits_live: bool) -> Result<Self> {
        Self::read(
            &|name| std::env::var(name).ok(),
            venue,
            ceiling_permits_live,
        )
    }

    /// The same, against any lookup.
    ///
    /// Pure, and that is the point: `std::env::set_var` is `unsafe` in this
    /// edition and this workspace forbids `unsafe_code`, so a test that had to
    /// mutate the environment to reach a refusal could not be written at all.
    /// Every branch below is reachable from a `BTreeMap`.
    pub fn read(
        lookup: &dyn Fn(&str) -> Option<String>,
        venue: &VenueId,
        ceiling_permits_live: bool,
    ) -> Result<Self> {
        let selected = value(lookup, ADAPTER_VARIABLE);
        match selected.as_deref() {
            None | Some(SIMULATED_ADAPTER) => Ok(Self::Simulated {
                seed: seed(lookup)?,
            }),
            Some(REST_ADAPTER) => {
                Self::read_live(lookup, venue, ceiling_permits_live).map(Self::Live)
            }
            Some(other) => Err(Error::invalid(format!(
                "configuration: {ADAPTER_VARIABLE}={other:?} names no venue adapter this node \
                 can build. It accepts {SIMULATED_ADAPTER:?}, the in-process matching engine \
                 that is also the default, and {REST_ADAPTER:?}, which opens a socket to a real \
                 venue. An unrecognised value is refused rather than falling back to the \
                 simulator, because a deployment that meant to trade and quietly did not is a \
                 deployment nobody finds out about until the fills are missing"
            ))),
        }
    }

    fn read_live(
        lookup: &dyn Fn(&str) -> Option<String>,
        venue: &VenueId,
        ceiling_permits_live: bool,
    ) -> Result<LiveVenueChoice> {
        let requirements = standard_requirements(venue);
        let endpoint_var = variable(&requirements, RequirementKind::Endpoint, venue)?;
        let credential_var = variable(&requirements, RequirementKind::SessionCredential, venue)?;
        let account_var = variable(&requirements, RequirementKind::Account, venue)?;

        // Every missing value at once, for the same reason `NodeConfig` reports
        // them together: deploying a venue should be one restart, not four.
        let mut missing = Vec::new();
        let endpoint = present(lookup, &endpoint_var, &mut missing);
        let credential = present(lookup, &credential_var, &mut missing);
        let account = present(lookup, &account_var, &mut missing);
        if !missing.is_empty() {
            return Err(Error::invalid(format!(
                "configuration: {ADAPTER_VARIABLE}={REST_ADAPTER} selects the REST order-entry \
                 adapter for {}, which needs {} to be set. These are the variables \
                 `qip_brokers::credential::standard_requirements` names for this venue; the node \
                 reads exactly those rather than inventing its own",
                venue.as_str(),
                missing.join(", ")
            )));
        }

        // The acknowledgement. Compared to the endpoint exactly, after
        // trimming the surrounding whitespace a shell here-doc adds and
        // nothing else: a comparison that normalised a trailing slash or a
        // default port would be a comparison that let two different hosts
        // match, which is the whole failure being guarded against.
        let acknowledged = value(lookup, ACKNOWLEDGEMENT_VARIABLE);
        match acknowledged {
            Some(acknowledged) if acknowledged == endpoint => {}
            Some(acknowledged) => {
                return Err(Error::denied(format!(
                    "configuration: {ACKNOWLEDGEMENT_VARIABLE} acknowledges {acknowledged:?} and \
                     {endpoint_var} points at {endpoint:?}. Nothing in this process can tell a \
                     venue's sandbox host from its production host, so an acknowledgement that \
                     does not name the endpoint being used acknowledges nothing. If the endpoint \
                     changed, that is exactly the edit this refusal exists for: set \
                     {ACKNOWLEDGEMENT_VARIABLE} to the address orders should go to and read it \
                     back before you do"
                )));
            }
            None => {
                return Err(Error::denied(format!(
                    "configuration: {ADAPTER_VARIABLE}={REST_ADAPTER} would send this cell's \
                     orders over a socket to {endpoint:?}, and {ACKNOWLEDGEMENT_VARIABLE} is not \
                     set. Nothing in this code can tell that host from a production one — the \
                     adapter reports its class as `sandbox` either way — so the only control \
                     that exists is an operator naming the destination: set \
                     {ACKNOWLEDGEMENT_VARIABLE} to exactly {endpoint:?}"
                )));
            }
        }

        // The dangerous combination. An adapter that can reach a venue is one
        // thing; a ceiling that permits live execution is another; the node
        // refuses to hold both on nothing more than the endpoint having been
        // typed twice.
        if ceiling_permits_live {
            let enablement_var = enablement_variable(&requirements, venue)?;
            match value(lookup, &enablement_var) {
                Some(enabled) if enabled == venue.as_str() => {}
                Some(enabled) => {
                    return Err(Error::denied(format!(
                        "configuration: this cell's autonomy ceiling permits live execution and \
                         {ADAPTER_VARIABLE}={REST_ADAPTER} points order entry at {endpoint:?}, so \
                         orders from this process can be real. {enablement_var}={enabled:?} does \
                         not name this venue; it must be set to {:?} exactly, so that an \
                         environment block copied from another venue's deployment cannot enable \
                         this one",
                        venue.as_str()
                    )));
                }
                None => {
                    return Err(Error::denied(format!(
                        "configuration: this cell's autonomy ceiling permits live execution and \
                         {ADAPTER_VARIABLE}={REST_ADAPTER} points order entry at {endpoint:?}. \
                         That combination puts real risk on, and holding a credential is not the \
                         same as having decided to trade: set {enablement_var}={:?} to enable \
                         this venue explicitly. {ACKNOWLEDGEMENT_VARIABLE} says where the orders \
                         go; this says that they may",
                        venue.as_str()
                    )));
                }
            }
        }

        Ok(LiveVenueChoice {
            venue: venue.clone(),
            endpoint,
            account,
            credential: Secret::new(credential),
            idempotency: idempotency(lookup)?,
            ceiling_permits_live,
        })
    }

    /// The word an operator and the health surface both use for this choice.
    pub const fn selector(&self) -> &'static str {
        match self {
            Self::Simulated { .. } => SIMULATED_ADAPTER,
            Self::Live(_) => REST_ADAPTER,
        }
    }

    /// Whether this choice can put bytes on a wire.
    ///
    /// Not "whether the fills are real" — no code here can answer that, which
    /// is the point of the whole module. This is the narrower and answerable
    /// question of whether an order can leave the process.
    pub const fn reaches_a_socket(&self) -> bool {
        matches!(self, Self::Live(_))
    }

    /// The start-up banner's account of where the orders go.
    ///
    /// Separate from the printing so it can be asserted. Every line carries
    /// [`DESTINATION_PREFIX`], and the live case says three things in order:
    /// the destination, the fact that this process cannot classify it, and who
    /// acknowledged it. The last two are not decoration. An operator reading
    /// "sandbox" in a log and taking it as a guarantee is the failure mode
    /// this text exists to prevent, so the guarantee is disclaimed in the same
    /// breath as the word.
    pub fn banner_lines(&self, ceiling: &str) -> Vec<String> {
        match self {
            Self::Simulated { seed } => vec![
                format!(
                    "{DESTINATION_PREFIX}: the in-process simulated exchange. No order leaves \
                     this process, no socket is opened, and no counterparty exists"
                ),
                format!(
                    "{DESTINATION_PREFIX}: selected by default; set {ADAPTER_VARIABLE}={REST_ADAPTER} \
                     to send orders to a venue instead. Matching seed {seed}, autonomy ceiling \
                     {ceiling}"
                ),
            ],
            Self::Live(live) => vec![
                format!(
                    "{DESTINATION_PREFIX}: venue {} over HTTP at {}, account {}. Orders placed \
                     by this cell are sent there",
                    live.venue.as_str(),
                    live.endpoint,
                    live.account
                ),
                format!(
                    "{DESTINATION_PREFIX}: this process CANNOT tell that endpoint's sandbox host \
                     from its production host. The adapter reports class `sandbox` and stamps \
                     every fill `simulated` whichever it is, so if {} is production then real \
                     orders are being labelled paper and every downstream number that keys off \
                     that flag is wrong",
                    live.endpoint
                ),
                format!(
                    "{DESTINATION_PREFIX}: acknowledged by {ACKNOWLEDGEMENT_VARIABLE}; autonomy \
                     ceiling {ceiling}, which {}. Idempotency on repeated keys: {}",
                    if live.ceiling_permits_live {
                        "permits live execution"
                    } else {
                        "does not permit live execution"
                    },
                    live.idempotency.as_str()
                ),
            ],
        }
    }
}

/// The environment variable one requirement kind is read from.
fn variable(
    requirements: &[VenueRequirement],
    kind: RequirementKind,
    venue: &VenueId,
) -> Result<String> {
    requirements_of_kind(requirements, &[kind])
        .into_iter()
        .next()
        .map(|requirement| requirement.env_var)
        .ok_or_else(|| {
            Error::invalid(format!(
                "configuration: the venue requirement list no longer names {} for {}, so this \
                 node cannot tell which variable to read it from",
                kind.as_str(),
                venue.as_str()
            ))
        })
}

/// The per-venue operator enablement, by the description its requirement
/// carries.
///
/// Matched on the requirement's own words rather than on a variable name
/// composed here, so the node and `qip_brokers` cannot drift into naming two
/// different variables for one decision. When the list stops carrying it the
/// node refuses rather than inventing a name nobody sets.
fn enablement_variable(requirements: &[VenueRequirement], venue: &VenueId) -> Result<String> {
    requirements_of_kind(requirements, &[RequirementKind::Entitlement])
        .into_iter()
        .find(|requirement| requirement.name.contains("operator enablement"))
        .map(|requirement| requirement.env_var)
        .ok_or_else(|| {
            Error::invalid(format!(
                "configuration: `qip_brokers::credential::standard_requirements` no longer names \
                 an operator enablement for {}, and this node will not invent a variable name \
                 for the one control that stands between a live ceiling and a live venue",
                venue.as_str()
            ))
        })
}

/// A set, non-blank environment value.
///
/// A variable set to the empty string is treated as absent, because that is
/// the failure that looks exactly like success: a secret manager that resolved
/// nothing writes an empty string, not an unset variable.
fn value(lookup: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    lookup(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn present(
    lookup: &dyn Fn(&str) -> Option<String>,
    name: &str,
    missing: &mut Vec<String>,
) -> String {
    match value(lookup, name) {
        Some(value) => value,
        None => {
            missing.push(name.to_string());
            String::new()
        }
    }
}

fn seed(lookup: &dyn Fn(&str) -> Option<String>) -> Result<u64> {
    match value(lookup, SEED_VARIABLE) {
        // Refused rather than defaulted. A seed that silently became 1 because
        // it was mistyped makes a session that does not replay, and the whole
        // reason the seed is configuration is that the session should.
        Some(raw) => raw.parse::<u64>().map_err(|_| {
            Error::invalid(format!(
                "configuration: {SEED_VARIABLE}={raw:?} is not an unsigned integer; the simulated \
                 venue's matching and rejection draws come from it, so a session seeded by \
                 accident is a session that does not replay"
            ))
        }),
        None => Ok(1),
    }
}

fn idempotency(lookup: &dyn Fn(&str) -> Option<String>) -> Result<IdempotencySupport> {
    match value(lookup, IDEMPOTENCY_VARIABLE).as_deref() {
        None => Ok(IdempotencySupport::Absent),
        Some(raw) if raw == IdempotencySupport::Absent.as_str() => Ok(IdempotencySupport::Absent),
        Some(raw) if raw == IdempotencySupport::Honoured.as_str() => {
            Ok(IdempotencySupport::Honoured)
        }
        Some(other) => Err(Error::invalid(format!(
            "configuration: {IDEMPOTENCY_VARIABLE}={other:?} is not something the adapter knows \
             how to be. It accepts {:?}, the default, under which an ambiguous submit is never \
             retried, and {:?}, which permits a retry and must only be set for a venue whose \
             documentation promises a repeated key returns the original order",
            IdempotencySupport::Absent.as_str(),
            IdempotencySupport::Honoured.as_str()
        ))),
    }
}
