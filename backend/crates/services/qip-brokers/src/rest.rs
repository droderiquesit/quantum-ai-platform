//! An order-entry adapter that opens a socket.
//!
//! Every other venue in this crate matches in process. [`SimulatedExchange`]
//! owns the book, the counterparty and the clock, so an order submitted to it
//! cannot be lost, cannot be duplicated, and cannot be in a state nobody knows.
//! This adapter is the first one where all three can happen, because it puts
//! bytes on a wire and waits for an answer that may not come.
//!
//! Transport is [`qip_transport::HttpClient`] — blocking, one connection per
//! request, every limit explicit. The shape is REST rather than a session
//! protocol because [`VenueAdapter`] is already request/response and because a
//! streaming session would need a runtime this build does not have.
//!
//! [`SimulatedExchange`]: crate::exchange::SimulatedExchange
//!
//! # The cardinal rule: a fill is never inferred
//!
//! An order's state here comes from one place only — a response this adapter
//! read in full, parsed completely, and could map onto
//! [`VenueOrderState`]. **Everything else makes the order's state
//! [`OrderOutcome::Unknown`].** A timeout is unknown. A connection refused is
//! unknown. A 500 is unknown. A body that is not JSON, a state code this
//! decoder does not recognise, a fill without an identifier, an acknowledgement
//! naming a different order — all unknown.
//!
//! Unknown is not "probably rejected" and it is not "assume it worked". It is a
//! third thing, and it stays that way until [`RestOrderEntryAdapter::reconcile`]
//! asks the venue and the venue answers. The reasoning is asymmetric on purpose:
//!
//! * An adapter that guessed *filled* would create a position the platform
//!   believes in and the venue does not. Every downstream number — exposure,
//!   margin, P&L, the risk engine's view of the book — would then be wrong, and
//!   nothing in the platform could detect it, because the fabricated fill looks
//!   exactly like a real one.
//! * An adapter that guessed *rejected* would let the caller submit again, and
//!   the second order would join the first one that did in fact arrive. Two
//!   orders is a doubled position, which is the same failure with a different
//!   sign.
//!
//! So the adapter refuses to have an opinion. An unknown order is a held order:
//! it cannot be resubmitted (see below), it contributes no fills, and it is
//! listed by [`RestOrderEntryAdapter::unknown_orders`] so an operator or a
//! reconciliation loop can see exactly how many there are.
//!
//! # Idempotency, and what happens when the venue does not have it
//!
//! Every submit carries a client-generated key in a header: the SHA-256 of the
//! order's material terms — venue, account, order id, instrument, side,
//! quantity, order type and limit. It is a pure function of the order, so a
//! retry computes the same key, and a *different* order cannot collide with it.
//! Reusing one order id for two different sets of terms produces two different
//! keys, and the adapter refuses that outright rather than letting an amendment
//! masquerade as a resubmission.
//!
//! What the adapter does with the key depends on what the deployment says the
//! venue does with it, declared in [`IdempotencySupport`]:
//!
//! * [`IdempotencySupport::Honoured`] — the venue documents that a repeated key
//!   returns the original order instead of creating a second one. A submit
//!   whose outcome is unknown may then be sent again with the same key. The
//!   adapter still checks the answer: if the venue returns a *different* venue
//!   order id than it returned before for that key, that is a duplicate order
//!   and the adapter refuses loudly with [`Error::guard`] rather than accepting
//!   the second one.
//! * [`IdempotencySupport::Absent`] — the default. The key still goes on the
//!   wire, because it costs nothing and a venue that starts honouring it later
//!   will find it there, but the adapter **will not retry a submit**. An order
//!   left unknown stays unknown until a query resolves it. This is the losing
//!   direction, and it is chosen deliberately: a missed order costs an
//!   opportunity, a duplicated order costs a position nobody sized.
//!
//! A resubmission of an order this adapter already has a *known* answer for is
//! recognised locally and sends nothing at all. The venue's idempotency is a
//! second line, not the first one.
//!
//! # It refuses, and it never stands in for anything
//!
//! With no endpoint it is unavailable; with no credential it is unavailable;
//! unauthenticated it cannot mint a [`ReadyTicket`] and so cannot submit. In
//! every one of those cases it returns [`Error::unavailable`] naming what is
//! missing and **opens no connection**.
//!
//! There is no fallback. This type holds no matching engine, no book and no
//! ledger; there is no code path from here into [`crate::exchange`]. That is
//! deliberate in the way `qip_training::vertex` and
//! `qip_market_ingestion::rest` are deliberate: a live-trading path that
//! degraded to a simulator would report fills for orders that never left the
//! process, and every control downstream that distinguishes paper from real
//! keys off a flag that would, in that moment, be lying.
//!
//! # What this adapter does not do
//!
//! * **It does not amend.** [`VenueAdapter::replace_order`] refuses. A replace
//!   has a submit's ambiguity plus one of its own — an amendment that may have
//!   been partially applied — and the safe composition is cancel then submit,
//!   whose intermediate state is flat rather than uncertain. That costs queue
//!   priority, which is the cheaper of the two things to lose.
//! * **It does not keep books.** [`VenueAdapter::query_positions`],
//!   [`VenueAdapter::query_cash`] and [`VenueAdapter::query_margin`] refuse
//!   rather than deriving a position from the acknowledgements this adapter
//!   happens to have seen. A book built from one order-entry session's partial
//!   view would disagree with the venue and look authoritative doing it.
//!   [`VenueAdapter::query_fills`] returns the fills the venue reported *to
//!   this process*, which is a different and lesser thing than the venue's own
//!   fill history, and is documented as such on the method.
//! * **It does not serve market data.** [`VenueAdapter::market_data`] refuses.
//!   Market data comes from `qip_market_ingestion`, and an order-entry
//!   credential is not a market-data entitlement.
//! * **It does not resolve "the venue has no record of it" into "it never
//!   arrived".** A 404 on a query leaves the order unknown. A venue that
//!   indexes orders by its own identifier rather than the client's would 404 an
//!   order it is holding, and treating that as absence is exactly how a live
//!   order becomes invisible. Only a state the venue states positively resolves
//!   an unknown; the one other way out is
//!   [`RestOrderEntryAdapter::resolve_manually`], which is attributed, refuses
//!   to assert a fill, and is named to be conspicuous in review.
//!
//! # The peer is untrusted, and so is the endpoint
//!
//! Every response is bounded before it is buffered ([`ClientLimits::max_body`]),
//! every wait is bounded ([`ClientLimits::connect_timeout`],
//! [`ClientLimits::read_timeout`], [`ClientLimits::write_timeout`]), the number
//! of fills one response may carry is capped
//! ([`RestVenueConfig::max_fills`]), and an acknowledgement that contradicts
//! itself — filled in full while reporting a smaller quantity, rejected while
//! reporting a fill — is refused rather than reconciled by this adapter's
//! guesswork.
//!
//! The credential travels in a header and never in a URL, because a URL is
//! written to every access log on the path. It is held in a
//! [`crate::credential::Secret`], which redacts in `Debug` and cannot be
//! serialised, and this type's own `Debug` reports only whether one is present.
//!
//! **The class is a claim about the endpoint, not a control over it.** This
//! adapter reports [`AdapterClass::Sandbox`], which is the strongest thing the
//! crate's type system permits — there is no `Live` variant to report. But
//! nothing in this code can tell a venue's sandbox host from its production
//! host: point [`RestVenueConfig::base_url`] at the latter and the orders are
//! real while [`Broker::is_simulated`] still answers `true`. That is a
//! deployment control (egress policy, and the operator enablement the
//! requirement list names), and it is the first item in
//! [`RestOrderEntryAdapter::REQUIREMENTS`] because it is the one this crate
//! cannot enforce for itself.
//!
//! # What it keeps, and for how long
//!
//! It remembers every order it has been told about and every fill identifier it
//! has booked, for the life of the process, and prunes neither. That is what
//! makes [`RestOrderEntryAdapter::unknown_orders`] a complete list and what
//! stops a repeated fill being booked twice, and it means memory grows with the
//! number of orders a session sends. A deployment running for months through
//! one adapter has to recycle it — at which point the unknown orders it was
//! holding are no longer listed by anything, so they must be reconciled first.
//! Nothing here is persisted: a process that restarts has forgotten which
//! orders were unknown, which is why the venue's own records, and not this
//! adapter's, are the reconciliation of record.
//!
//! # Time
//!
//! Every timestamp written into an order, a fill or an acknowledgement comes
//! from the caller's `at` or from the venue's own message. Nothing here reads a
//! wall clock. The one exception is the round trip on an acknowledgement, which
//! is measured with [`std::time::Instant`]: it is a monotonic *interval*, not a
//! statement about when it is, and there is no way to learn how long a socket
//! took from a timestamp the caller passed in before the socket was opened.

use crate::adapter::{
    AdapterClass, CashBalance, Heartbeat, MarginState, MarketData, OrderAck, PositionSnapshot,
    VenueAdapter, VenueOrder, VenueOrderState, stamp_simulated,
};
use crate::connection::{ConnectionState, ReadyTicket};
use crate::credential::{
    RequirementKind, Secret, VenueCredential, VenueRequirement, requirements_of_kind,
    standard_requirements,
};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::ids::{FillId, ObjectId, OrderId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Currency, Decimal};
use qip_execution_engine::broker::{Broker, VenueCapabilities};
use qip_execution_engine::order::{Fill, Order, OrderType, Side};
use qip_transport::{ClientLimits, HttpClient, HttpRequest, HttpResponse, Method, Url};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

/// Default path of the order collection under the base address.
const DEFAULT_ORDERS_PATH: &str = "/v1/orders";
/// Default path of the cheap authenticated read used as a liveness probe.
const DEFAULT_HEALTH_PATH: &str = "/v1/health";
/// Default header the credential travels in. Never a query parameter: a URL is
/// written to every access log on the path, and a credential in one is a
/// credential in all of them.
const DEFAULT_KEY_HEADER: &str = "x-api-key";
/// Default header the idempotency key travels in.
const DEFAULT_IDEMPOTENCY_HEADER: &str = "idempotency-key";
/// Headers `qip_transport::HttpRequest` writes itself and silently drops from a
/// caller. Naming them here turns "the credential quietly vanished" into a
/// configuration error raised where the configuration was written.
const CLIENT_OWNED_HEADERS: [&str; 4] =
    ["host", "content-length", "connection", "transfer-encoding"];
/// Statuses whose body this adapter will read as the venue's own account of the
/// order.
///
/// 2xx is the venue accepting. These three are the venue *refusing and saying
/// why*: 400 malformed, 409 a conflict (which is what a replayed idempotency
/// key usually is), 422 semantically unacceptable. Every other status —
/// including 401, 404 and the whole 5xx range — leaves the order unknown,
/// because those are the venue declining to process rather than the venue
/// describing an order, and the difference between "we refused it" and "we
/// could not tell you" is the whole of this module.
const STATEFUL_STATUSES: [u16; 3] = [400, 409, 422];

/// What the deployment says this venue does with an idempotency key.
///
/// Declared rather than probed, because probing it means submitting a duplicate
/// order on purpose to see what happens, and there is no venue where that is an
/// acceptable experiment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdempotencySupport {
    /// The venue's documentation says a repeated key returns the original order
    /// rather than creating a second one. A submit whose outcome is unknown may
    /// be sent again with the same key.
    Honoured,
    /// The venue makes no such promise. This is the default, and under it the
    /// adapter refuses to retry a submit: an order that may or may not exist
    /// stays unknown until a query says which.
    #[default]
    Absent,
}

impl IdempotencySupport {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Honoured => "honoured",
            Self::Absent => "absent",
        }
    }

    /// Whether a submit whose outcome is unknown may be sent a second time.
    pub const fn permits_resubmission(&self) -> bool {
        matches!(self, Self::Honoured)
    }
}

/// Everything a deployment has to decide before this adapter can send an order.
///
/// Carries no secret. The credential arrives at
/// [`VenueAdapter::authenticate`] inside a [`VenueCredential`], which is where
/// every other adapter in this crate takes it and where the redaction already
/// lives.
#[derive(Clone, Debug)]
pub struct RestVenueConfig {
    /// `http://host[:port]` of the venue. `None` means unconfigured, which is
    /// what makes the adapter report itself unavailable rather than guess.
    ///
    /// This is the field that decides whether the orders are real. See the
    /// module documentation: nothing in this code can tell a sandbox host from
    /// a production one.
    pub base_url: Option<String>,
    /// Path of the order collection. May not carry a query string: this adapter
    /// builds the query itself, and a second `?` would put the order id
    /// somewhere the venue does not read it.
    pub orders_path: String,
    /// Path of a cheap authenticated read, used to prove the endpoint is
    /// reachable and the credential is accepted. A venue with no such endpoint
    /// can be pointed at any read-only one.
    pub health_path: String,
    /// Header the credential travels in, since venues disagree.
    pub api_key_header: String,
    /// Header the idempotency key travels in, since venues disagree.
    pub idempotency_header: String,
    /// What the venue does with a repeated key. Defaults to
    /// [`IdempotencySupport::Absent`], which is the answer that loses orders
    /// rather than duplicating them.
    pub idempotency: IdempotencySupport,
    /// How long a heartbeat gap the session tolerates before it is untrustworthy.
    pub heartbeat_interval: Duration,
    /// Most fills one response may carry. A response small enough to read is
    /// not automatically one worth expanding.
    pub max_fills: usize,
    /// The venue's smallest tradable increment, for [`Broker::capabilities`].
    /// Reported, not enforced: the venue enforces its own and this adapter does
    /// not second-guess it.
    pub lot_size: Decimal,
    /// The venue's commission rate, for [`Broker::capabilities`]. A statistic,
    /// so `f64`; every charge against cash uses the exact `Decimal` the venue
    /// reported on the fill.
    pub commission_rate: f64,
    /// The account currency, for [`Broker::capabilities`] and nothing else.
    pub currency: Currency,
    /// Transport limits. The peer chooses how much to send; these decide how
    /// much this process will hold and how long it will wait.
    pub http: ClientLimits,
}

impl Default for RestVenueConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            orders_path: DEFAULT_ORDERS_PATH.into(),
            health_path: DEFAULT_HEALTH_PATH.into(),
            api_key_header: DEFAULT_KEY_HEADER.into(),
            idempotency_header: DEFAULT_IDEMPOTENCY_HEADER.into(),
            idempotency: IdempotencySupport::Absent,
            heartbeat_interval: Duration::from_secs(30),
            max_fills: 512,
            lot_size: Decimal::ONE,
            commission_rate: 0.0,
            currency: Currency::USD,
            http: ClientLimits {
                // An order acknowledgement is a few hundred bytes; one carrying
                // a long fill list is a few kilobytes. 256 kB is generous and
                // still small enough that refusing more costs nothing.
                max_body: 256 * 1024,
                max_headers: 32,
                // Order entry is the latency-sensitive path and the one where
                // waiting is most expensive: every second spent waiting on an
                // ambiguous submit is a second the platform does not know its
                // own position. Short, explicit, and the same on every call.
                connect_timeout: StdDuration::from_secs(2),
                read_timeout: StdDuration::from_secs(5),
                write_timeout: StdDuration::from_secs(5),
                ..ClientLimits::default()
            },
        }
    }
}

/// What the venue said about an order, or the fact that nobody knows.
///
/// The second variant is the reason this type exists rather than a bare
/// [`VenueOrderState`]. `VenueOrderState` has five variants and every one of
/// them is an assertion; there is no way to say "the request left and no
/// answer this adapter could read came back", and that is precisely the state
/// an order-entry path spends its worst minutes in.
#[derive(Clone, Debug, PartialEq)]
pub enum OrderOutcome {
    /// The venue said this, in a response read and parsed in full.
    Known(VenueOrderState),
    /// A request for this order left the process and its outcome is not known.
    /// The order may be working at the venue, may have filled, may never have
    /// arrived. Resolved only by [`RestOrderEntryAdapter::reconcile`].
    Unknown { reason: String },
}

impl OrderOutcome {
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown { .. })
    }

    /// The venue's state, when there is one. `None` is not a state — it is the
    /// absence of one, and a caller that treats it as flat is the bug this
    /// whole module is arranged against.
    pub const fn state(&self) -> Option<&VenueOrderState> {
        match self {
            Self::Known(state) => Some(state),
            Self::Unknown { .. } => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Known(state) => state.as_str().to_string(),
            Self::Unknown { reason } => format!("unknown ({reason})"),
        }
    }
}

/// What this adapter knows about one order it has sent for.
///
/// Kept because the venue's answer and the client's identity have to be joined
/// somewhere, and because an order whose state is unknown has to be listed by
/// something. It is not a book: no position, no cash, no P&L.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackedOrder {
    pub order_id: OrderId,
    pub object_id: ObjectId,
    pub side: Side,
    pub quantity: Decimal,
    pub original_quantity: Decimal,
    pub limit: Option<Decimal>,
    /// The key every request for this order carried. A pure function of the
    /// order's terms; see the module documentation.
    pub idempotency_key: String,
    /// The venue's own identifier, once it has given one. A second, different
    /// one for the same key is a duplicated order and is refused.
    pub venue_order_id: Option<String>,
    pub outcome: OrderOutcome,
    /// Cumulative filled quantity as the venue last reported it. Zero while the
    /// state is unknown, because an unknown order has no reported fills — not
    /// because it has none.
    pub filled: Decimal,
    pub revision: u32,
    pub priority: u64,
    pub submitted_at: Timestamp,
    pub updated_at: Timestamp,
    /// How many submit requests for this order have left the process. Greater
    /// than one only where [`IdempotencySupport::Honoured`] permitted a retry,
    /// so it is also the count of the times this adapter relied on the venue's
    /// deduplication rather than its own.
    pub sends: u32,
    /// The last thing said about this order, for an operator reading a list.
    pub detail: String,
}

impl TrackedOrder {
    pub fn remaining(&self) -> Decimal {
        self.quantity - self.filled
    }
}

/// What this adapter has done, for metrics and for tests that assert a request
/// happened rather than assuming it did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestOrderStats {
    /// Submit requests this adapter committed to sending. Incremented before
    /// the socket is written, because an order recorded as sent that was not is
    /// recoverable and an order sent that was not recorded is not.
    pub submits_sent: u64,
    /// Cancel requests that left the process.
    pub cancels_sent: u64,
    /// Order-status requests that left the process.
    pub queries_sent: u64,
    /// Responses that became a known order state.
    pub acknowledged: u64,
    /// Orders the venue refused, parsed as refusals rather than as errors.
    pub rejected: u64,
    /// Times an order moved into the unknown state. Cumulative, so it counts
    /// an order that went unknown twice twice; the current set is
    /// [`RestOrderEntryAdapter::unknown_orders`].
    pub entered_unknown: u64,
    /// Unknown orders resolved by asking the venue.
    pub reconciled: u64,
    /// Submits recognised locally as repeats and never sent.
    pub duplicates_suppressed: u64,
    /// Orders an operator resolved by hand, which the venue never confirmed.
    pub resolved_manually: u64,
    /// Fills the venue reported again that this process had already booked.
    /// Ordinary rather than alarming — venues repeat the fill list on every
    /// answer about an order — and counted so that a test can assert the
    /// repetition was recognised instead of assuming it was.
    pub fills_deduplicated: u64,
}

/// A venue reached over HTTP.
pub struct RestOrderEntryAdapter {
    venue: VenueId,
    config: RestVenueConfig,
    /// Prebuilt endpoints, parsed once at construction so a malformed venue
    /// address fails where it was configured rather than on the first order.
    orders_endpoint: Option<Url>,
    health_endpoint: Option<Url>,
    connection: ConnectionState,
    account: Option<String>,
    /// The resolved session secret. `None` until [`VenueAdapter::authenticate`]
    /// has been handed one, and there is no default.
    credential: Option<Secret>,
    client: HttpClient,
    /// Keyed by client order id, which is what goes on the wire and comes back.
    orders: BTreeMap<String, TrackedOrder>,
    /// Fills the venue reported to this process, in the order they arrived.
    fills: Vec<Fill>,
    /// Every fill identifier already booked, and the order it was booked
    /// against. Venues repeat the whole fill list on every response about an
    /// order, so without this a cancel or a `reconcile` would re-report fills
    /// the submit already reported and the platform would book the same trade
    /// twice — inventing quantity by arithmetic rather than by assumption,
    /// which is the same failure this module exists to prevent.
    seen_fills: BTreeMap<String, String>,
    heartbeats: u64,
    stats: RestOrderStats,
}

/// Written by hand rather than derived, for the same reason
/// [`VenueCredential`]'s is: this type holds a secret, and a derived `Debug`
/// would print whatever field the struct grows next.
impl std::fmt::Debug for RestOrderEntryAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestOrderEntryAdapter")
            .field("venue", &self.venue.as_str())
            .field("config", &self.config)
            .field("account", &self.account)
            // Present or absent is worth knowing; the value never is. `Secret`
            // redacts too, so this is the belt to its braces.
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "<redacted>"),
            )
            .field("phase", &self.connection.phase())
            .field("tracked_orders", &self.orders.len())
            .field("unknown_orders", &self.unknown_orders().len())
            .field("stats", &self.stats)
            .finish()
    }
}

impl RestOrderEntryAdapter {
    /// What a deployment must supply on top of a working configuration.
    ///
    /// These stand even when every field is set, which is why
    /// [`Self::requirement`] never comes back empty. A configured adapter is
    /// not by itself a venue anyone should be trading through.
    pub const REQUIREMENTS: [&'static str; 6] = [
        "an endpoint that is the venue's *sandbox*, enforced by egress policy rather than by this \
         code: nothing here can tell a sandbox host from a production one, so a `base_url` \
         pointing at production sends real orders while `is_simulated()` still answers true. This \
         is the requirement the crate cannot enforce for itself and therefore the one a \
         deployment has to",
        "a TLS-terminating egress proxy in front of this adapter, or a venue reachable over the \
         cluster network: `qip_transport::http` has no TLS stack and refuses `https` by name \
         rather than downgrading it, so a credential sent straight to a public venue would cross \
         the internet in clear text — and on this path so would the orders",
        "the venue's documented position on idempotency keys, set in `idempotency`. Left at the \
         default the adapter refuses to retry a submit and orders are lost rather than \
         duplicated; set to `honoured` on a venue that does not honour them, it will duplicate",
        "an alert on the count of unknown orders, which is what a venue that has started timing \
         out looks like from the outside. An unknown order is a position the platform cannot \
         account for, and the only thing worse than having one is not knowing how many there are",
        "a reconciliation that runs against the venue's own order and fill records, not against \
         this adapter's: `query_fills` reports what this process was told, which is a strictly \
         smaller thing than what the venue did",
        "an explicit operator enablement, separate from the credential, because holding a secret \
         is not the same as having decided to trade",
    ];

    /// Build an adapter. Succeeds even when nothing is configured: an adapter
    /// that cannot send still has to exist in order to say so.
    ///
    /// Fails only on configuration that is present and wrong — an unparseable
    /// address, a path carrying a query, a header name that cannot be written.
    pub fn new(venue: VenueId, config: RestVenueConfig, at: Timestamp) -> Result<Self> {
        if venue.as_str().trim().is_empty() {
            return Err(Error::invalid(
                "a venue adapter needs a venue id: it names the account every order is sent for \
                 and stamps every fill",
            ));
        }
        if config.max_fills == 0 {
            return Err(Error::invalid(
                "max_fills is zero, which would refuse every acknowledgement that carried a fill",
            ));
        }
        validate_header_name(&config.api_key_header, "the credential")?;
        validate_header_name(&config.idempotency_header, "the idempotency key")?;
        if config
            .api_key_header
            .trim()
            .eq_ignore_ascii_case(config.idempotency_header.trim())
        {
            return Err(Error::invalid(
                "the credential and the idempotency key are configured to travel in the same \
                 header; one would overwrite the other and which one is not something this \
                 adapter should decide",
            ));
        }
        validate_path(&config.orders_path, "orders_path")?;
        validate_path(&config.health_path, "health_path")?;

        let (orders_endpoint, health_endpoint) = match &config.base_url {
            Some(base) => {
                let url = Url::parse(base).map_err(Error::from)?;
                (
                    Some(url.with_path(&config.orders_path).map_err(Error::from)?),
                    Some(url.with_path(&config.health_path).map_err(Error::from)?),
                )
            }
            None => (None, None),
        };

        let client = HttpClient::new(config.http);
        Ok(Self {
            connection: ConnectionState::new(venue.clone(), config.heartbeat_interval, at),
            venue,
            config,
            orders_endpoint,
            health_endpoint,
            account: None,
            credential: None,
            client,
            orders: BTreeMap::new(),
            fills: Vec::new(),
            seen_fills: BTreeMap::new(),
            heartbeats: 0,
            stats: RestOrderStats::default(),
        })
    }

    pub fn config(&self) -> &RestVenueConfig {
        &self.config
    }

    pub fn stats(&self) -> RestOrderStats {
        self.stats
    }

    /// Everything this adapter is tracking, in client order id order.
    pub fn tracked_orders(&self) -> Vec<&TrackedOrder> {
        self.orders.values().collect()
    }

    /// What this adapter knows about one order, including that it knows nothing.
    pub fn tracked(&self, order_id: &OrderId) -> Option<&TrackedOrder> {
        self.orders.get(order_id.as_str())
    }

    /// Every order whose state nobody knows.
    ///
    /// The number to alert on. Each entry is an order that may be working at
    /// the venue, may have filled, or may never have arrived, and the platform
    /// cannot tell which until [`Self::reconcile`] asks.
    pub fn unknown_orders(&self) -> Vec<OrderId> {
        self.orders
            .values()
            .filter(|tracked| tracked.outcome.is_unknown())
            .map(|tracked| tracked.order_id.clone())
            .collect()
    }

    /// Whether this adapter can send a request at all.
    ///
    /// Availability is narrower than [`VenueAdapter::missing_requirements`]:
    /// this is "can bytes leave", that is "what does production still owe". An
    /// adapter can be available and still be missing an entitlement nobody
    /// granted.
    pub fn is_configured(&self) -> bool {
        self.missing_configuration().is_empty()
    }

    /// Configuration a deployment has not supplied, each named on its own.
    ///
    /// Separately rather than as one "not configured", so an operator with two
    /// of the three learns which one is left instead of re-checking all of them.
    pub fn missing_configuration(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.orders_endpoint.is_none() {
            missing.push(format!(
                "no endpoint: set `base_url` to the venue's REST base address, which this adapter \
                 requests `{}` under",
                self.config.orders_path
            ));
        }
        if self.credential.is_none() {
            missing.push(format!(
                "no credential: authenticate with a `VenueCredential` carrying the resolved \
                 session secret, which is sent in the `{}` header. A reference-only credential \
                 names where the secret lives and does not carry it, which is enough for a \
                 simulator and not enough for a transport that has to write it",
                self.config.api_key_header
            ));
        }
        if self.account.is_none() {
            missing.push(
                "no account: authenticate with a credential naming the account orders are sent \
                 for. There is no default account, and an order sent for one nobody chose cannot \
                 be attributed to a book"
                    .into(),
            );
        }
        missing
    }

    /// The full text of what production must supply: what is missing now,
    /// followed by what is required even when nothing is.
    pub fn requirement(&self) -> String {
        let mut parts = self.missing_configuration();
        parts.extend(Self::REQUIREMENTS.iter().map(|r| (*r).to_string()));
        parts.join("; ")
    }

    /// The refusal every entry point returns when the adapter cannot send.
    fn unavailable(&self) -> Error {
        Error::unavailable(format!(
            "{} cannot reach a venue and will not stand in for one: {}",
            self.venue.as_str(),
            self.requirement()
        ))
    }

    /// The requirements this adapter enforces before it will log anyone on.
    ///
    /// It cannot check an entitlement nobody granted or a comp id a REST venue
    /// does not use, and claiming otherwise would be theatre. It can refuse to
    /// run for an account nobody named, using a secret nobody resolved.
    fn enforced_requirements(&self) -> Vec<VenueRequirement> {
        requirements_of_kind(
            &standard_requirements(&self.venue),
            &[RequirementKind::Account, RequirementKind::SessionCredential],
        )
    }

    /// The key every request for `order` carries.
    ///
    /// A pure function of the order's material terms, so a retry computes the
    /// same value and two different orders cannot collide. Exposed because a
    /// deployment reconciling against the venue's records needs to be able to
    /// compute it without submitting anything.
    pub fn idempotency_key_for(&self, order: &Order) -> Result<String> {
        let account = self.account.as_deref().ok_or_else(|| self.unavailable())?;
        let limit = limit_of(&self.venue, order)?;
        Ok(idempotency_key(&self.venue, account, order, limit))
    }

    // --- the wire ----------------------------------------------------------

    /// Build a request carrying the credential, and nothing that is not needed.
    ///
    /// Takes `&self` and returns an owned request so the borrow of the secret
    /// ends before anything is recorded, and so every authenticated request in
    /// this module is written in exactly one place.
    fn authenticated(
        &self,
        method: Method,
        target: &str,
        idempotency_key: Option<&str>,
        body: Option<Vec<u8>>,
    ) -> Result<HttpRequest> {
        let secret = self.credential.as_ref().ok_or_else(|| self.unavailable())?;
        let value = secret.expose_str()?;
        let mut request = match body {
            Some(bytes) => HttpRequest::json(method, target, bytes).map_err(Error::from)?,
            None => HttpRequest::new(method, target)
                .map_err(Error::from)?
                .with_header("accept", "application/json"),
        };
        request = request.with_header(&self.config.api_key_header, value);
        if let Some(key) = idempotency_key {
            request = request.with_header(&self.config.idempotency_header, key);
        }
        Ok(request)
    }

    /// Send, and report how long the socket took.
    ///
    /// The interval is monotonic and is the only thing in this module that does
    /// not come from a caller's timestamp or a venue's message; see the module
    /// documentation on time.
    fn send(&self, request: &HttpRequest) -> (std::result::Result<HttpResponse, Error>, Duration) {
        let began = std::time::Instant::now();
        let outcome = self.client.send(request).map_err(Error::from);
        let elapsed = began.elapsed();
        let nanos = i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX);
        (outcome, Duration::from_nanos(nanos))
    }

    /// The order collection's address, or the refusal.
    fn orders_url(&self) -> Result<String> {
        self.orders_endpoint
            .as_ref()
            .map(Url::to_string)
            .ok_or_else(|| self.unavailable())
    }

    /// The address of one order, addressed by the client's own identifier.
    ///
    /// A query parameter rather than a path segment because this adapter builds
    /// the query by hand and a client order id is validated against a charset
    /// that is safe in one; a path segment would additionally have to survive
    /// whatever normalisation sits between here and the venue.
    fn order_url(&self, order_id: &str) -> Result<String> {
        validate_wire_id(order_id, "a client order id")?;
        Ok(format!("{}?client_order_id={order_id}", self.orders_url()?))
    }

    fn health_url(&self) -> Result<String> {
        self.health_endpoint
            .as_ref()
            .map(Url::to_string)
            .ok_or_else(|| self.unavailable())
    }

    // --- state ------------------------------------------------------------

    /// Record that a request for this order is about to leave the process.
    ///
    /// Called *before* the send, not after. An order whose answer never arrives
    /// must already be unknown by the time the failure is handled, and the only
    /// way to guarantee that is to write the unknown down first. The cost is
    /// that a submit refused by the transport before a byte moved also shows as
    /// unknown, which is the cheap direction to be wrong in.
    fn begin_submit(
        &mut self,
        order: &Order,
        key: &str,
        limit: Option<Decimal>,
        at: Timestamp,
    ) -> Result<()> {
        let id = order.order_id.as_str().to_string();
        let reason =
            "a submit left this process and no answer it could read has come back yet".to_string();
        match self.orders.get_mut(&id) {
            Some(tracked) => {
                tracked.sends = tracked.sends.saturating_add(1);
                tracked.outcome = OrderOutcome::Unknown {
                    reason: reason.clone(),
                };
                tracked.updated_at = at;
                tracked.detail = reason;
            }
            None => {
                self.orders.insert(
                    id,
                    TrackedOrder {
                        order_id: order.order_id.clone(),
                        object_id: order.object_id.clone(),
                        side: order.side,
                        quantity: order.quantity,
                        original_quantity: order.quantity,
                        limit,
                        idempotency_key: key.to_string(),
                        venue_order_id: None,
                        outcome: OrderOutcome::Unknown {
                            reason: reason.clone(),
                        },
                        filled: Decimal::ZERO,
                        revision: 0,
                        priority: 0,
                        submitted_at: at,
                        updated_at: at,
                        sends: 1,
                        detail: reason,
                    },
                );
            }
        }
        self.stats.entered_unknown = self.stats.entered_unknown.saturating_add(1);
        self.stats.submits_sent = self.stats.submits_sent.saturating_add(1);
        Ok(())
    }

    /// Move a tracked order into the unknown state, with the reason.
    ///
    /// Does nothing for an order this adapter is not tracking: a cancel for an
    /// order submitted by a previous process is legal, and there is nothing
    /// here to make unknown. The caller still gets the error.
    fn mark_unknown(&mut self, order_id: &str, at: Timestamp, reason: String) {
        let entered = match self.orders.get_mut(order_id) {
            Some(tracked) => {
                let was_unknown = tracked.outcome.is_unknown();
                tracked.outcome = OrderOutcome::Unknown {
                    reason: reason.clone(),
                };
                tracked.updated_at = at;
                tracked.detail = reason;
                !was_unknown
            }
            None => false,
        };
        if entered {
            self.stats.entered_unknown = self.stats.entered_unknown.saturating_add(1);
        }
    }

    /// Write a parsed venue answer into the tracked order, and hand back the
    /// fills on it this process had not already booked.
    ///
    /// This is the only function in the module that produces a known state, and
    /// it is reached only from a response that parsed in full.
    ///
    /// It returns *new* fills rather than every fill the response carried.
    /// Venues repeat the whole fill list on every answer about an order, so a
    /// cancel or a [`Self::reconcile`] routinely re-reports what the submit
    /// already reported. Booking those again would double a real position — a
    /// fabricated fill arrived at by arithmetic instead of by assumption — so a
    /// fill identifier this adapter has already seen for this order is counted
    /// in [`RestOrderStats::fills_deduplicated`] and handed on to nobody.
    /// `filled` still comes from the venue's own cumulative number, which is
    /// the authority on how much traded.
    fn commit(&mut self, wire: &WireOrder, at: Timestamp) -> Result<Vec<Fill>> {
        let decoded = decode_order(&self.venue, wire, self.config.max_fills)?;
        let id = wire.client_order_id.clone();
        let simulated = self.class().is_paper();

        if let Some(tracked) = self.orders.get(&id) {
            check_consistent(&self.venue, tracked, &decoded)?;
        }

        // Partitioned before anything is written, so a response carrying a
        // contradiction leaves no half-applied state behind it.
        let mut fills = Vec::with_capacity(decoded.fills.len());
        let mut repeated = 0u64;
        for fill in &decoded.fills {
            match self.seen_fills.get(fill.fill_id.as_str()) {
                // Already booked against this same order: the venue is
                // repeating itself, which is normal and must not be counted.
                Some(owner) if owner == &id => repeated = repeated.saturating_add(1),
                // The same fill identifier against a *different* order. One of
                // the two attributions is wrong and this adapter has no basis
                // for choosing, so the answer is refused and the order is left
                // unknown rather than a trade being attributed to the wrong
                // book.
                Some(owner) => {
                    return Err(Error::numeric(format!(
                        "{} reported fill {} on order {id} and has already reported it on order \
                         {owner}: one fill cannot belong to two orders, and this adapter \
                         will not decide which book it lands in",
                        self.venue.as_str(),
                        fill.fill_id.as_str()
                    )));
                }
                None => fills.push(fill.clone()),
            }
        }
        stamp_simulated(&mut fills, simulated);

        let detail = format!(
            "the venue reported {} for {} filled of {}",
            decoded.state.as_str(),
            decoded.filled,
            decoded.quantity
        );
        match self.orders.get_mut(&id) {
            Some(tracked) => {
                tracked.outcome = OrderOutcome::Known(decoded.state.clone());
                tracked.filled = decoded.filled;
                tracked.quantity = decoded.quantity;
                tracked.limit = decoded.limit;
                tracked.venue_order_id = decoded
                    .venue_order_id
                    .clone()
                    .or_else(|| tracked.venue_order_id.clone());
                tracked.revision = decoded.revision;
                tracked.priority = decoded.priority;
                tracked.updated_at = decoded.updated_at.unwrap_or(at);
                tracked.detail = detail;
            }
            None => {
                // A cancel or a query for an order this process did not submit.
                // Tracking it from the venue's own answer is the honest record:
                // every field below came off the wire.
                self.orders.insert(
                    id.clone(),
                    TrackedOrder {
                        order_id: OrderId::from_string(wire.client_order_id.clone()),
                        object_id: decoded.object_id.clone(),
                        side: decoded.side,
                        quantity: decoded.quantity,
                        original_quantity: decoded.quantity,
                        limit: decoded.limit,
                        idempotency_key: String::new(),
                        venue_order_id: decoded.venue_order_id.clone(),
                        outcome: OrderOutcome::Known(decoded.state.clone()),
                        filled: decoded.filled,
                        revision: decoded.revision,
                        priority: decoded.priority,
                        submitted_at: decoded.submitted_at.unwrap_or(at),
                        updated_at: decoded.updated_at.unwrap_or(at),
                        sends: 0,
                        detail,
                    },
                );
            }
        }

        self.stats.acknowledged = self.stats.acknowledged.saturating_add(1);
        if matches!(decoded.state, VenueOrderState::Rejected { .. }) {
            self.stats.rejected = self.stats.rejected.saturating_add(1);
        }
        self.stats.fills_deduplicated = self.stats.fills_deduplicated.saturating_add(repeated);
        // Recorded here rather than by the caller, so there is exactly one
        // place a fill can enter this process and it is downstream of every
        // check.
        for fill in &fills {
            self.seen_fills
                .insert(fill.fill_id.as_str().to_string(), id.clone());
        }
        self.fills.extend(fills.iter().cloned());
        Ok(fills)
    }

    /// Read a response as the venue's account of an order, or say why it is not.
    ///
    /// Every path out of this function that is not `Ok` leaves the caller
    /// obliged to make the order unknown.
    fn decode_response(&self, response: &HttpResponse, expected: &str) -> Result<WireOrder> {
        if !response.is_success() && !STATEFUL_STATUSES.contains(&response.status) {
            return Err(self.status_refusal(response.status, &response.body_excerpt()));
        }
        let body = response.body_as_str().map_err(Error::from)?;
        let wire: WireOrder = serde_json::from_str(body).map_err(|error| {
            Error::schema(format!(
                "{} answered HTTP {} with a body this adapter cannot read as an order: {error}. \
                 The first bytes of it were: {}",
                self.venue.as_str(),
                response.status,
                response.body_excerpt()
            ))
        })?;
        if wire.client_order_id != expected {
            return Err(Error::invalid(format!(
                "{} answered a request about order {expected} with a record for {}; an \
                 acknowledgement that names a different order is not evidence about this one",
                self.venue.as_str(),
                wire.client_order_id
            )));
        }
        Ok(wire)
    }

    /// What a status this adapter will not read an order state from means.
    ///
    /// Separated by class because the operator action differs: a rejected
    /// credential is a deployment to fix, a 404 is a path to fix, a 429 or a
    /// 5xx is a venue to wait for. One error type for all three would put every
    /// one of them on the same runbook page.
    fn status_refusal(&self, status: u16, excerpt: &str) -> Error {
        let venue = self.venue.as_str();
        match status {
            401 | 403 => Error::denied(format!(
                "{venue} rejected this deployment's credential with HTTP {status}. The credential \
                 itself is not quoted here, and is not written to any log by this adapter"
            )),
            404 => Error::not_found(format!(
                "{venue} has no endpoint at the configured path, or no record of this order (HTTP \
                 404): {excerpt}"
            )),
            408 | 429 => Error::unavailable(format!(
                "{venue} is rate-limiting or timing out this deployment (HTTP {status}): {excerpt}"
            )),
            500..=599 => Error::unavailable(format!(
                "{venue} failed to serve the request (HTTP {status}): {excerpt}"
            )),
            other => Error::invalid(format!(
                "{venue} answered HTTP {other}, which this adapter will not read an order state \
                 from: {excerpt}"
            )),
        }
    }

    /// Build the acknowledgement for a committed answer.
    fn ack(
        &self,
        order_id: &OrderId,
        state: VenueOrderState,
        fills: Vec<Fill>,
        remaining: Decimal,
        at: Timestamp,
        latency: Duration,
        detail: impl Into<String>,
    ) -> OrderAck {
        OrderAck {
            order_id: order_id.clone(),
            venue: self.venue.as_str().to_string(),
            at: at.saturating_add(latency),
            latency,
            state,
            fills,
            remaining,
            simulated: self.class().is_paper(),
            detail: detail.into(),
        }
    }

    // --- reconciliation ----------------------------------------------------

    /// Ask the venue what an order is doing, and write the answer down.
    ///
    /// The only way an unknown order becomes known without an operator saying
    /// so. It is a plain query — it creates nothing, cancels nothing, and is
    /// safe to call on an order in any state, including one that was never
    /// submitted.
    ///
    /// A venue that answers "no such order" does **not** resolve the unknown:
    /// see the module documentation. The error comes back, the order stays
    /// unknown, and the deployment decides.
    pub fn reconcile(&mut self, order_id: &OrderId, at: Timestamp) -> Result<VenueOrder> {
        let was_unknown = self
            .orders
            .get(order_id.as_str())
            .is_some_and(|tracked| tracked.outcome.is_unknown());

        let target = self.order_url(order_id.as_str())?;
        let request = self.authenticated(Method::Get, &target, None, None)?;
        self.stats.queries_sent = self.stats.queries_sent.saturating_add(1);
        let (outcome, _) = self.send(&request);
        let response = outcome?;
        let wire = self.decode_response(&response, order_id.as_str())?;
        self.commit(&wire, at)?;
        if was_unknown {
            self.stats.reconciled = self.stats.reconciled.saturating_add(1);
        }
        self.venue_order_of(order_id)
    }

    /// Record an operator's own finding about an order the venue will not
    /// resolve.
    ///
    /// Named to be conspicuous. This is the one path in the module from unknown
    /// to known that the venue did not authorise, and it exists because an
    /// order the venue has no record of and will not confirm would otherwise
    /// stay unknown for ever, which is an operational dead end rather than a
    /// control.
    ///
    /// Two restrictions make it survivable:
    ///
    /// * It refuses to assert a fill or a partial fill. A fill is exactly the
    ///   thing that must come from the venue, and a hand-written one is the
    ///   fabricated position this whole module is arranged against. Only
    ///   [`VenueOrderState::Cancelled`] and [`VenueOrderState::Rejected`] — the
    ///   two flat states — can be asserted.
    /// * It requires an attributed note, which is recorded on the order and
    ///   counted in [`RestOrderStats::resolved_manually`].
    ///
    /// It is still dangerous: asserting `cancelled` for an order the venue
    /// actually filled leaves a real position the platform thinks is flat. It
    /// is a last resort after a human has looked at the venue's own screens,
    /// not a way to clear a queue.
    pub fn resolve_manually(
        &mut self,
        order_id: &OrderId,
        state: VenueOrderState,
        operator: &str,
        at: Timestamp,
    ) -> Result<()> {
        if operator.trim().is_empty() {
            return Err(Error::invalid(
                "a manual resolution has to name who made it; an unattributed one is \
                 indistinguishable from a bug",
            ));
        }
        match state {
            VenueOrderState::Cancelled { .. } | VenueOrderState::Rejected { .. } => {}
            other => {
                return Err(Error::denied(format!(
                    "an operator may assert that an order is flat — cancelled or rejected — and \
                     may not assert {}. A fill comes from the venue or it does not exist",
                    other.as_str()
                )));
            }
        }
        let venue = self.venue.as_str().to_string();
        let tracked = self.orders.get_mut(order_id.as_str()).ok_or_else(|| {
            Error::not_found(format!(
                "{venue} is not tracking order {}; there is nothing to resolve",
                order_id.as_str()
            ))
        })?;
        if !tracked.outcome.is_unknown() {
            return Err(Error::denied(format!(
                "order {} is {} at {venue} and does not need resolving; an operator does not \
                 overwrite what the venue said",
                order_id.as_str(),
                tracked.outcome.describe()
            )));
        }
        tracked.outcome = OrderOutcome::Known(state.clone());
        tracked.updated_at = at;
        tracked.detail = format!(
            "resolved by hand as {} by {operator}; the venue never confirmed this",
            state.as_str()
        );
        self.stats.resolved_manually = self.stats.resolved_manually.saturating_add(1);
        Ok(())
    }

    /// The tracked order as a [`VenueOrder`], or the refusal to invent one.
    fn venue_order_of(&self, order_id: &OrderId) -> Result<VenueOrder> {
        let venue = self.venue.as_str();
        let tracked = self.orders.get(order_id.as_str()).ok_or_else(|| {
            Error::not_found(format!(
                "{venue} is not tracking order {}; ask the venue with `reconcile` rather than \
                 reading this adapter's memory",
                order_id.as_str()
            ))
        })?;
        let state = tracked.outcome.state().ok_or_else(|| {
            Error::unavailable(format!(
                "the state of order {} at {venue} is not known: {}. It has not been assumed \
                 filled and has not been assumed rejected; call `reconcile` to ask the venue",
                order_id.as_str(),
                tracked.outcome.describe()
            ))
        })?;
        Ok(VenueOrder {
            order_id: tracked.order_id.clone(),
            object_id: tracked.object_id.clone(),
            venue: venue.to_string(),
            side: tracked.side,
            quantity: tracked.quantity,
            filled: tracked.filled,
            limit: tracked.limit,
            state: state.clone(),
            revision: tracked.revision,
            original_quantity: tracked.original_quantity,
            submitted_at: tracked.submitted_at,
            updated_at: tracked.updated_at,
            priority: tracked.priority,
            simulated: self.class().is_paper(),
        })
    }

    /// One authenticated liveness request, used by logon and by the heartbeat.
    fn probe(&self, at: Timestamp) -> Result<Duration> {
        let target = self.health_url()?;
        let request = self.authenticated(Method::Get, &target, None, None)?;
        let (outcome, latency) = self.send(&request);
        let response = outcome.map_err(|error| {
            named(
                &error,
                &format!(
                    "{} did not answer a liveness request at {}",
                    self.venue.as_str(),
                    at.to_rfc3339()
                ),
            )
        })?;
        if !response.is_success() {
            return Err(self.status_refusal(response.status, &response.body_excerpt()));
        }
        Ok(latency)
    }

    /// Decide what a submit for an already-tracked order should do.
    fn resubmission_decision(&self, order: &Order, key: &str) -> Result<Resubmission> {
        let Some(tracked) = self.orders.get(order.order_id.as_str()) else {
            return Ok(Resubmission::Send);
        };
        // An empty key means this adapter learned of the order from a venue
        // answer rather than by sending it — a query or a cancel for an order a
        // previous process submitted. There is nothing to compare, and saying
        // the terms differ would accuse the caller of something it did not do.
        let learned_from_venue = tracked.idempotency_key.is_empty();
        if !learned_from_venue && tracked.idempotency_key != key {
            return Err(Error::invalid(format!(
                "order {} was already submitted to {} under a different set of terms. An \
                 idempotency key is a function of the order's terms, so sending this one would \
                 create a second order rather than repeat the first; an amendment is a replace, \
                 not a resubmission",
                order.order_id.as_str(),
                self.venue.as_str()
            )));
        }
        match &tracked.outcome {
            OrderOutcome::Known(_) => Ok(Resubmission::AlreadyKnown),
            OrderOutcome::Unknown { reason } => {
                if learned_from_venue {
                    Err(Error::guard(format!(
                        "{} holds an order under the id {} that this process did not submit and \
                         whose state is unknown ({reason}). No idempotency key was computed for \
                         it, so a submit now could not be recognised as a repeat of it and would \
                         risk a second order; call `reconcile` first",
                        self.venue.as_str(),
                        order.order_id.as_str()
                    )))
                } else if self.config.idempotency.permits_resubmission() {
                    Ok(Resubmission::Send)
                } else {
                    Err(Error::guard(format!(
                        "the outcome of the last submit of order {} to {} is unknown ({reason}), \
                         and this venue is configured as not honouring idempotency keys \
                         (`idempotency = absent`), so sending it again could create a second \
                         order. This adapter will not do that: a missed order costs an \
                         opportunity and a duplicated one costs a position nobody sized. Call \
                         `reconcile` to ask the venue what happened",
                        order.order_id.as_str(),
                        self.venue.as_str()
                    )))
                }
            }
        }
    }
}

/// What a submit for an order this adapter has already seen should do.
enum Resubmission {
    /// Put a request on the wire.
    Send,
    /// The venue has already answered about this exact order. Send nothing.
    AlreadyKnown,
}

impl VenueAdapter for RestOrderEntryAdapter {
    fn venue_id(&self) -> &VenueId {
        &self.venue
    }

    /// Sandbox: a real protocol against a real endpoint.
    ///
    /// The strongest claim the enum permits, and — as the module documentation
    /// says at length — a claim about the endpoint a deployment supplied rather
    /// than one this code can check.
    fn class(&self) -> AdapterClass {
        AdapterClass::Sandbox
    }

    fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    fn connection(&self) -> &ConnectionState {
        &self.connection
    }

    fn requirements(&self) -> Vec<VenueRequirement> {
        standard_requirements(&self.venue)
    }

    /// What is still outstanding.
    ///
    /// Never empty. The endpoint, the credential and the account clear as they
    /// are supplied; the protocol parameters and both entitlements do not,
    /// because this adapter cannot verify an entitlement nobody granted and
    /// cannot verify that the endpoint it was given is the sandbox one. An
    /// adapter that reported itself complete would be claiming to have checked
    /// things it has no way to check.
    fn missing_requirements(&self) -> Vec<VenueRequirement> {
        standard_requirements(&self.venue)
            .into_iter()
            .filter(|requirement| match requirement.kind {
                RequirementKind::Endpoint => self.orders_endpoint.is_none(),
                RequirementKind::SessionCredential => self.credential.is_none(),
                RequirementKind::Account => self.account.is_none(),
                RequirementKind::ProtocolParameter | RequirementKind::Entitlement => true,
            })
            .collect()
    }

    /// Prove the endpoint is reachable, without offering the credential.
    ///
    /// A real request, deliberately unauthenticated. Any HTTP answer at all —
    /// including a 401 — means the transport works, and that is exactly the
    /// fact this phase is supposed to establish. Separating it from the logon
    /// is what turns "the venue is not working" into either "nothing is
    /// listening on that address" or "the address is right and the credential
    /// is wrong", which are different tickets for different people.
    fn connect(&mut self, at: Timestamp) -> Result<()> {
        let target = self.health_url()?;
        let request = HttpRequest::new(Method::Get, &target)
            .map_err(Error::from)?
            .with_header("accept", "application/json");
        let (outcome, _) = self.send(&request);
        match outcome {
            Ok(_) => self.connection.connect(at),
            Err(error) => Err(Error::io(format!(
                "{} is not reachable at {target}: {}. Nothing was sent and no session exists",
                self.venue.as_str(),
                error.message()
            ))),
        }
    }

    /// Log on by making one authenticated request and seeing whether it is
    /// accepted.
    ///
    /// A REST venue has no session to hold open, so there is nothing here that
    /// corresponds to a FIX logon. A logon that consisted of setting a boolean
    /// would report a session that does not exist and would defer the discovery
    /// of a bad credential to the first order, which is the worst moment to
    /// find out. So this asks the venue.
    ///
    /// The credential must carry a *resolved* secret. A reference-only
    /// credential names the environment variable the secret lives in, which is
    /// enough for an adapter that never dials and not enough for one that has
    /// to write the value into a header.
    fn authenticate(&mut self, credential: &VenueCredential, at: Timestamp) -> Result<()> {
        if self.orders_endpoint.is_none() {
            return Err(self.unavailable());
        }
        credential.require(&self.venue, &self.enforced_requirements())?;
        let name = standard_requirements(&self.venue)
            .into_iter()
            .find(|requirement| requirement.kind == RequirementKind::SessionCredential)
            .map(|requirement| requirement.name)
            .ok_or_else(|| {
                Error::invalid(
                    "the standard requirement list no longer names a session credential, so this \
                     adapter cannot tell which secret to send",
                )
            })?;
        let secret = credential.secret(&name).ok_or_else(|| {
            Error::unavailable(format!(
                "the credential for {} references the session secret but does not carry its \
                 value. This adapter has to write it into the `{}` header, so a reference is not \
                 enough; resolve it with `VenueCredential::with_secret`",
                self.venue.as_str(),
                self.config.api_key_header
            ))
        })?;
        if secret.is_empty() {
            return Err(Error::unavailable(format!(
                "the session secret for {} is empty. An environment variable set to the empty \
                 string is the failure that looks exactly like success, and this adapter will not \
                 send an empty credential to find out",
                self.venue.as_str()
            )));
        }
        validate_credential(secret.expose_str()?)?;

        // Held before the probe, because the probe is the thing that needs it.
        // Cleared again if the venue refuses, so a rejected credential does not
        // leave the adapter looking configured.
        self.credential = Some(secret.clone());
        self.account = Some(credential.account().to_string());
        match self.probe(at) {
            Ok(_) => self.connection.authenticated(at, credential.account()),
            Err(error) => {
                self.credential = None;
                self.account = None;
                Err(error)
            }
        }
    }

    /// Ask the venue whether it is still there, over a real socket.
    fn heartbeat(&mut self, at: Timestamp) -> Result<Heartbeat> {
        self.connection.observe(at);
        self.connection.require_session(at)?;
        let round_trip = self.probe(at)?;
        let phase = self.connection.observe_heartbeat(at)?;
        self.heartbeats = self.heartbeats.saturating_add(1);
        Ok(Heartbeat {
            venue: self.venue.as_str().to_string(),
            at: at.saturating_add(round_trip),
            round_trip,
            phase,
            session: self.connection.session(),
            sequence: self.heartbeats,
        })
    }

    /// Refused. This is an order-entry adapter.
    ///
    /// An order-entry credential is not a market-data entitlement, and a venue's
    /// order API is not a quote source. Market data comes from
    /// `qip_market_ingestion`, whose adapters carry the licensing class and the
    /// dissemination delay that decide what a price may be used for — neither
    /// of which exists on this path.
    fn market_data(&self, object_id: &ObjectId, _at: Timestamp) -> Result<MarketData> {
        Err(Error::unavailable(format!(
            "{} is an order-entry adapter and serves no market data for {}: an order API is not a \
             quote source, and a price with no licensing class and no dissemination delay behind \
             it cannot be used for anything. Use a `qip_market_ingestion` adapter",
            self.venue.as_str(),
            object_id.as_str()
        )))
    }

    /// Send an order, once.
    ///
    /// Every exit from this function leaves the order in exactly one of three
    /// places: a state the venue stated, the unknown state, or — where the
    /// adapter refused before anything left the process — untouched. There is
    /// no fourth.
    fn submit_order(
        &mut self,
        ticket: &ReadyTicket,
        order: &Order,
        at: Timestamp,
    ) -> Result<OrderAck> {
        self.connection.observe(at);
        self.connection.authorise(ticket, at)?;
        if !self.is_configured() {
            return Err(self.unavailable());
        }
        let account = self.account.clone().ok_or_else(|| self.unavailable())?;
        let limit = limit_of(&self.venue, order)?;
        if !order.quantity.is_positive() {
            return Err(Error::invalid(format!(
                "order {} has quantity {} and {} will not be sent an order for nothing",
                order.order_id.as_str(),
                order.quantity,
                self.venue.as_str()
            )));
        }
        validate_wire_id(order.order_id.as_str(), "a client order id")?;
        validate_wire_id(order.object_id.as_str(), "an instrument id")?;

        let key = idempotency_key(&self.venue, &account, order, limit);
        match self.resubmission_decision(order, &key)? {
            Resubmission::AlreadyKnown => {
                self.stats.duplicates_suppressed =
                    self.stats.duplicates_suppressed.saturating_add(1);
                let known = self.venue_order_of(&order.order_id)?;
                return Ok(self.ack(
                    &order.order_id,
                    known.state.clone(),
                    // No fills: the fills belong to the acknowledgement that
                    // first reported them, and reporting them again here would
                    // book them twice.
                    Vec::new(),
                    known.remaining(),
                    at,
                    Duration::ZERO,
                    format!(
                        "recognised as a resubmission of an order {} has already answered about; \
                         nothing was sent",
                        self.venue.as_str()
                    ),
                ));
            }
            Resubmission::Send => {}
        }

        let body = serde_json::to_vec(&WireSubmit {
            client_order_id: order.order_id.as_str(),
            idempotency_key: &key,
            account: &account,
            instrument: order.object_id.as_str(),
            side: side_code(order.side),
            quantity: order.quantity,
            order_type: if limit.is_some() { "limit" } else { "market" },
            limit_price: limit,
            submitted_at: at,
        })
        .map_err(|error| Error::schema(format!("this order cannot be written as JSON: {error}")))?;

        let target = self.orders_url()?;
        let request = self.authenticated(Method::Post, &target, Some(&key), Some(body))?;
        let previous_venue_order_id = self
            .orders
            .get(order.order_id.as_str())
            .and_then(|tracked| tracked.venue_order_id.clone());

        // Written down before the send. See `begin_submit`.
        self.begin_submit(order, &key, limit, at)?;
        let (outcome, latency) = self.send(&request);

        let response = match outcome {
            Ok(response) => response,
            Err(error) => {
                let reason = format!(
                    "the submit failed with no answer this adapter could read ({}). The order may \
                     be working at the venue, may have filled, or may never have arrived",
                    error.message()
                );
                self.mark_unknown(order.order_id.as_str(), at, reason);
                return Err(error);
            }
        };
        let wire = match self.decode_response(&response, order.order_id.as_str()) {
            Ok(wire) => wire,
            Err(error) => {
                let reason = format!(
                    "the venue answered and the answer could not be read as an order state ({}). \
                     The order may be working at the venue, may have filled, or may never have \
                     arrived",
                    error.message()
                );
                self.mark_unknown(order.order_id.as_str(), at, reason);
                return Err(error);
            }
        };

        // A venue that returns a second, different identifier for one
        // idempotency key has created a second order. That is the failure the
        // key exists to prevent, and it is refused loudly rather than absorbed:
        // there is now an order at the venue this adapter cannot account for.
        if let (Some(previous), Some(current)) = (&previous_venue_order_id, &wire.venue_order_id)
            && previous != current
        {
            let reason = format!(
                "a resubmission under idempotency key {key} came back with venue order id \
                 {current} where the first submit returned {previous}: the venue did not \
                 deduplicate and there are now two orders"
            );
            self.mark_unknown(order.order_id.as_str(), at, reason.clone());
            return Err(Error::guard(format!(
                "{}: {reason}. `idempotency` is configured as `{}` for this venue and that is \
                 wrong; stop submitting and reconcile both orders by hand",
                self.venue.as_str(),
                self.config.idempotency.as_str()
            )));
        }

        let fills = match self.commit(&wire, at) {
            Ok(fills) => fills,
            Err(error) => {
                let reason = format!(
                    "the venue's answer contradicted itself and was refused ({}). The order's \
                     real state is whatever the venue holds, which this adapter has not been able \
                     to read",
                    error.message()
                );
                self.mark_unknown(order.order_id.as_str(), at, reason);
                return Err(error);
            }
        };
        let known = self.venue_order_of(&order.order_id)?;
        let detail = format!(
            "submitted under idempotency key {key}; the venue answered HTTP {} with {}",
            response.status,
            known.state.as_str()
        );
        Ok(self.ack(
            &order.order_id,
            known.state.clone(),
            fills,
            known.remaining(),
            at,
            latency,
            detail,
        ))
    }

    /// Pull a working order.
    ///
    /// No ticket, per the trait: cancelling reduces risk and a degraded session
    /// is exactly when it is most needed. An order this adapter is not tracking
    /// can still be cancelled — one submitted by a previous process is the
    /// ordinary case — and the venue's answer is what gets recorded.
    ///
    /// An ambiguous cancel makes the order unknown too. "The cancel may or may
    /// not have been applied" is the same shape of ignorance as "the submit may
    /// or may not have arrived", and it is resolved the same way.
    fn cancel_order(&mut self, order_id: &OrderId, at: Timestamp) -> Result<OrderAck> {
        self.connection.observe(at);
        self.connection.require_session(at)?;
        if !self.is_configured() {
            return Err(self.unavailable());
        }
        let target = self.order_url(order_id.as_str())?;
        let request = self.authenticated(Method::Delete, &target, None, None)?;
        self.stats.cancels_sent = self.stats.cancels_sent.saturating_add(1);
        let (outcome, latency) = self.send(&request);

        let response = match outcome {
            Ok(response) => response,
            Err(error) => {
                let reason = format!(
                    "a cancel was sent and no answer this adapter could read came back ({}). The \
                     order may have been cancelled, may still be working, and may have filled in \
                     between",
                    error.message()
                );
                self.mark_unknown(order_id.as_str(), at, reason);
                return Err(error);
            }
        };
        let wire = match self.decode_response(&response, order_id.as_str()) {
            Ok(wire) => wire,
            Err(error) => {
                let reason = format!(
                    "a cancel was answered with something this adapter could not read as an order \
                     state ({}); whether it was applied is not known",
                    error.message()
                );
                self.mark_unknown(order_id.as_str(), at, reason);
                return Err(error);
            }
        };
        let fills = match self.commit(&wire, at) {
            Ok(fills) => fills,
            Err(error) => {
                let reason = format!(
                    "a cancel was answered with a record that contradicted itself ({}); whether \
                     it was applied is not known",
                    error.message()
                );
                self.mark_unknown(order_id.as_str(), at, reason);
                return Err(error);
            }
        };
        let known = self.venue_order_of(order_id)?;
        let detail = format!(
            "cancel answered HTTP {} with {}",
            response.status,
            known.state.as_str()
        );
        Ok(self.ack(
            order_id,
            known.state.clone(),
            fills,
            known.remaining(),
            at,
            latency,
            detail,
        ))
    }

    /// Refused, on purpose.
    ///
    /// An amendment carries a submit's ambiguity — the request may or may not
    /// have arrived — plus one that is worse: an amendment that arrived may
    /// have been applied to a quantity that had already partly filled, so the
    /// resulting order is neither the old one nor the new one and the client
    /// cannot compute which. Cancel then submit has the same effect with a flat
    /// intermediate state, at the cost of queue priority, and losing priority
    /// is cheaper than not knowing the size of a live order.
    fn replace_order(
        &mut self,
        _ticket: &ReadyTicket,
        order_id: &OrderId,
        _quantity: Decimal,
        _limit: Option<Decimal>,
        _at: Timestamp,
    ) -> Result<OrderAck> {
        Err(Error::unavailable(format!(
            "{} does not amend order {} over this adapter. An amendment that may have been \
             partially applied leaves an order whose size the client cannot compute, which is a \
             worse failure than the queue priority lost by cancelling and submitting again — so \
             cancel it and submit a new order",
            self.venue.as_str(),
            order_id.as_str()
        )))
    }

    /// What this adapter last heard about an order, without asking the venue.
    ///
    /// `&self`, so it cannot record anything; that is why the live query is
    /// [`Self::reconcile`] and this one reads memory. An order whose state is
    /// unknown is refused here rather than summarised, because every caller of
    /// this method wants a state and there is not one.
    fn query_order(&self, order_id: &OrderId) -> Result<VenueOrder> {
        self.venue_order_of(order_id)
    }

    /// Refused. This adapter keeps no books.
    fn query_positions(&self) -> Result<Vec<PositionSnapshot>> {
        Err(Error::unavailable(format!(
            "{} keeps no positions. A book built from the acknowledgements one order-entry \
             session happened to see would disagree with the venue and would look authoritative \
             doing it; ask the venue's own position endpoint, or the platform's portfolio",
            self.venue.as_str()
        )))
    }

    /// Refused. This adapter keeps no books.
    fn query_cash(&self) -> Result<CashBalance> {
        Err(Error::unavailable(format!(
            "{} keeps no cash balance: it charges nothing, settles nothing, and has seen only the \
             fills the venue reported to this process",
            self.venue.as_str()
        )))
    }

    /// Refused. This adapter keeps no books.
    fn query_margin(&self, _at: Timestamp) -> Result<MarginState> {
        Err(Error::unavailable(format!(
            "{} computes no margin. Margin is the venue's risk desk's number, and a locally \
             invented one would be wrong in the direction that permits new risk",
            self.venue.as_str()
        )))
    }

    /// The fills the venue reported *to this process*.
    ///
    /// Not the venue's fill history, and not a substitute for it. A fill the
    /// venue reported on an acknowledgement this adapter never read — because
    /// the response timed out, or because the process restarted — is not here
    /// and cannot be. A reconciliation has to read the venue's own records;
    /// this is for attributing what was seen, not for proving what happened.
    fn query_fills(&self, since: Option<Timestamp>) -> Result<Vec<Fill>> {
        let mut fills: Vec<Fill> = self
            .fills
            .iter()
            .filter(|fill| since.is_none_or(|from| fill.at >= from))
            .cloned()
            .collect();
        stamp_simulated(&mut fills, self.is_simulated());
        Ok(fills)
    }

    /// Hang up.
    ///
    /// Drops the credential with the session. There is no socket to close —
    /// the transport holds none — so what this actually does is make every
    /// subsequent instruction refuse until somebody authenticates again, which
    /// is what disconnecting is for.
    ///
    /// Tracked orders survive: an order whose state is unknown does not become
    /// known by hanging up, and forgetting it here is how it would stop being
    /// counted.
    fn disconnect(&mut self, reason: &str, at: Timestamp) -> Result<()> {
        self.connection.disconnect(at, reason);
        self.account = None;
        self.credential = None;
        Ok(())
    }
}

impl Broker for RestOrderEntryAdapter {
    fn name(&self) -> &str {
        self.venue.as_str()
    }

    /// True, because [`AdapterClass::Sandbox`] settles nothing.
    ///
    /// Read the module documentation before relying on it: this is a claim
    /// about the endpoint the deployment supplied, and this code cannot check
    /// it. It is derived from [`AdapterClass::is_paper`] rather than written as
    /// a literal so that it cannot drift from the class the adapter reports.
    fn is_simulated(&self) -> bool {
        self.class().is_paper()
    }

    fn is_available(&self) -> bool {
        self.is_configured()
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            name: self.venue.as_str().to_string(),
            // What this adapter will *send*. The venue may accept more; an
            // execution algorithm is worked into child orders before it reaches
            // any venue, so it is not on this list.
            supported_types: vec!["market".to_string(), "limit".to_string()],
            partial_fills: true,
            lot_size: self.config.lot_size,
            commission_rate: self.config.commission_rate,
        }
    }

    /// Submit through the narrow port the order management system uses.
    ///
    /// Mints its own readiness ticket, so an OMS that knows nothing about this
    /// crate still cannot reach a venue that is not ready.
    fn submit(&mut self, order: &Order, at: Timestamp) -> Result<Vec<Fill>> {
        let ticket = self.ready(at)?;
        self.submit_order(&ticket, order, at).map(|ack| ack.fills)
    }

    fn cancel(&mut self, order: &Order, at: Timestamp) -> Result<()> {
        self.cancel_order(&order.order_id, at).map(|_| ())
    }

    /// The trait method and the inherent [`RestOrderEntryAdapter::requirement`]
    /// share a name, and the inherent one wins method resolution. It is written
    /// out in full here so that this reads as the composition it is rather than
    /// as the infinite recursion it would be if the inherent method were ever
    /// removed.
    fn requirement(&self) -> String {
        format!(
            "{}. {}",
            self.requirement_summary(),
            RestOrderEntryAdapter::requirement(self)
        )
    }
}

// --- pure helpers -----------------------------------------------------------

/// The key every request for an order carries.
///
/// SHA-256 over the terms, newline-separated, with the field order fixed here.
/// Hex, so it is safe in a header without escaping. It covers the venue and the
/// account as well as the order, so one client order id sent to two venues, or
/// for two accounts, does not collide — a venue that saw the same key twice
/// would be right to return the first order, and it would be the wrong order.
fn idempotency_key(
    venue: &VenueId,
    account: &str,
    order: &Order,
    limit: Option<Decimal>,
) -> String {
    let canonical = format!(
        "qip-order-v1\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        venue.as_str(),
        account,
        order.order_id.as_str(),
        order.object_id.as_str(),
        side_code(order.side),
        order.quantity,
        if limit.is_some() { "limit" } else { "market" },
        match limit {
            Some(price) => price.to_string(),
            None => "-".to_string(),
        }
    );
    sha256_hex(canonical.as_bytes())
}

const fn side_code(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

/// The limit an order type implies, and whether this adapter will send it.
///
/// An execution algorithm is refused rather than flattened to a market order:
/// the two have very different fills, and a venue that was sent the second when
/// the caller asked for the first would look like it had worked.
fn limit_of(venue: &VenueId, order: &Order) -> Result<Option<Decimal>> {
    match order.order_type {
        OrderType::Market => Ok(None),
        OrderType::Limit { price } => {
            if price.is_positive() {
                Ok(Some(price))
            } else {
                Err(Error::invalid(format!(
                    "order {} has a limit of {price}, which is not a price this adapter will put \
                     on the wire",
                    order.order_id.as_str()
                )))
            }
        }
        other => Err(Error::denied(format!(
            "{} accepts market and limit orders; {} is an execution algorithm and has to be \
             worked into child orders before it reaches a venue",
            venue.as_str(),
            other.as_str()
        ))),
    }
}

/// Re-raise a failure with the venue and the operation named, keeping its kind.
///
/// The kind is the part a caller acts on — a timeout goes back on a retry
/// ladder and a schema error never should — so flattening every transport
/// failure into one variant would put a refused connection and an unreadable
/// body on the same runbook page. Only the message gains context.
fn named(error: &Error, context: &str) -> Error {
    let message = format!("{context}: {}", error.message());
    match error {
        Error::Invalid(_) => Error::invalid(message),
        Error::NotFound(_) => Error::not_found(message),
        Error::Denied(_) => Error::denied(message),
        Error::Numeric(_) => Error::numeric(message),
        Error::Schema(_) => Error::schema(message),
        Error::Io(_) => Error::io(message),
        Error::Unavailable(_) => Error::unavailable(message),
        Error::Guard(_) => Error::guard(message),
        Error::Timeout(_) => Error::timeout(message),
    }
}

fn validate_credential(key: &str) -> Result<()> {
    if key.trim().is_empty() {
        return Err(Error::invalid(
            "the session secret is blank; an unconfigured credential is absent, not empty, so \
             that the adapter reports itself unavailable instead of sending an empty header",
        ));
    }
    if key.chars().any(|c| c.is_control()) {
        return Err(Error::invalid(
            "the session secret contains a control character; sent as a header value it would end \
             the header and let the rest be read as another one",
        ));
    }
    Ok(())
}

/// Reject a header name that would break the request or be silently dropped.
fn validate_header_name(name: &str, what: &str) -> Result<()> {
    let header = name.trim().to_ascii_lowercase();
    if header.is_empty() {
        return Err(Error::invalid(format!(
            "{what} needs a header to travel in; it is never put in the URL"
        )));
    }
    // The client writes these four itself and silently drops a caller's copy,
    // which is right for framing headers and fatal for these: the request would
    // go out without what it was supposed to carry, and the venue's answer
    // would look like a bad secret rather than a header name nobody could use.
    if CLIENT_OWNED_HEADERS.contains(&header.as_str()) {
        return Err(Error::invalid(format!(
            "{what} cannot travel in the `{header}` header: the transport writes that one itself \
             and drops a caller's copy, so the request would leave without it"
        )));
    }
    if !header.chars().all(|c| c.is_ascii_graphic() && c != ':') {
        return Err(Error::invalid(format!(
            "{header:?} is not a usable header name: a space, a colon or a control character in \
             one would end the header and let the rest be read as another"
        )));
    }
    Ok(())
}

fn validate_path(path: &str, field: &str) -> Result<()> {
    if !path.starts_with('/') {
        return Err(Error::invalid(format!(
            "{field} is {path:?} and has to start with `/`: it is a path under the venue's base \
             address, not an address of its own"
        )));
    }
    if path.contains('?') || path.contains('#') {
        return Err(Error::invalid(format!(
            "{field} is {path:?} and carries a query or a fragment; this adapter builds the query \
             itself, and a second one would put the order id where the venue does not read it"
        )));
    }
    Ok(())
}

/// Identifiers go into a request line, so what may be in one is decided here
/// rather than discovered when an id splits a request.
fn validate_wire_id(id: &str, what: &str) -> Result<()> {
    if id.trim().is_empty() {
        return Err(Error::invalid(format!("{what} is empty")));
    }
    if id.len() > 128 {
        return Err(Error::invalid(format!(
            "{what} is {} characters long, which is longer than this adapter will put in a \
             request line",
            id.len()
        )));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
    {
        return Err(Error::invalid(format!(
            "{what} contains a character this adapter will not put in a request line: only ASCII \
             letters, digits and . - _ : are accepted, since the query is built by hand"
        )));
    }
    Ok(())
}

/// A venue answer that has been read in full and found not to contradict itself.
#[derive(Clone, Debug)]
struct DecodedOrder {
    object_id: ObjectId,
    side: Side,
    quantity: Decimal,
    filled: Decimal,
    limit: Option<Decimal>,
    state: VenueOrderState,
    venue_order_id: Option<String>,
    revision: u32,
    priority: u64,
    submitted_at: Option<Timestamp>,
    updated_at: Option<Timestamp>,
    fills: Vec<Fill>,
}

/// Turn one wire record into the platform's types, refusing anything that does
/// not add up.
///
/// The checks here are not defensive padding. Each one is a claim a venue could
/// make that would silently corrupt the platform's view of a position: a filled
/// order that filled less than its quantity, a rejected order that reports a
/// trade, fills that add to more than the order says filled. An adapter that
/// smoothed any of those over would be choosing which of two contradictory
/// numbers the platform believes, and it has no basis for choosing.
fn decode_order(venue: &VenueId, wire: &WireOrder, max_fills: usize) -> Result<DecodedOrder> {
    let name = venue.as_str();
    if wire.fills.len() > max_fills {
        return Err(Error::guard(format!(
            "{name} reported {} fills on one order and the cap is {max_fills}: a response small \
             enough to read is not automatically one worth expanding",
            wire.fills.len()
        )));
    }
    if !wire.quantity.is_positive() {
        return Err(Error::schema(format!(
            "{name} reported order {} with quantity {}",
            wire.client_order_id, wire.quantity
        )));
    }
    let filled = wire.filled.unwrap_or(Decimal::ZERO);
    if filled.is_negative() {
        return Err(Error::schema(format!(
            "{name} reported order {} as {filled} filled",
            wire.client_order_id
        )));
    }
    if filled > wire.quantity {
        return Err(Error::numeric(format!(
            "{name} reported order {} as {filled} filled of {}: an order cannot fill more than it \
             was sent for, and this adapter will not decide which of the two numbers is right",
            wire.client_order_id, wire.quantity
        )));
    }

    let state = state_from_code(name, wire, filled)?;
    let object_id = ObjectId::from_string(wire.instrument.clone());
    let side = side_from_code(name, &wire.side)?;

    let mut fills = Vec::with_capacity(wire.fills.len());
    let mut total = Decimal::ZERO;
    for entry in &wire.fills {
        if entry.fill_id.trim().is_empty() {
            return Err(Error::schema(format!(
                "{name} reported a fill on order {} with no identifier. A fill nobody can name \
                 cannot be deduplicated against a redelivery or joined to the venue's own record, \
                 so it is refused rather than given an invented one",
                wire.client_order_id
            )));
        }
        if !entry.quantity.is_positive() {
            return Err(Error::schema(format!(
                "{name} reported fill {} on order {} with quantity {}",
                entry.fill_id, wire.client_order_id, entry.quantity
            )));
        }
        total = total
            .checked_add(entry.quantity)
            .ok_or_else(|| Error::numeric("the fills on this order do not sum".to_string()))?;
        fills.push(Fill {
            fill_id: FillId::from_string(entry.fill_id.clone()),
            order_id: OrderId::from_string(wire.client_order_id.clone()),
            at: entry.at,
            quantity: entry.quantity,
            price: entry.price,
            costs: entry.costs,
            venue: name.to_string(),
            // Overwritten by `stamp_simulated` on the way out. The flag is the
            // adapter's to set, never the message's.
            simulated: true,
        });
    }
    if total > filled {
        return Err(Error::numeric(format!(
            "{name} reported fills totalling {total} on order {} while reporting it {filled} \
             filled: the two cannot both be true and this adapter will not pick one",
            wire.client_order_id
        )));
    }

    Ok(DecodedOrder {
        object_id,
        side,
        quantity: wire.quantity,
        filled,
        limit: wire.limit_price,
        state,
        venue_order_id: wire.venue_order_id.clone(),
        revision: wire.revision.unwrap_or(0),
        priority: wire.priority.unwrap_or(0),
        submitted_at: wire.submitted_at,
        updated_at: wire.updated_at,
        fills,
    })
}

/// The venue's state code, and the arithmetic that has to hold with it.
///
/// An unknown code is refused rather than defaulted to `working`: a state this
/// decoder cannot name is a state whose risk it cannot describe, and defaulting
/// would put an order the venue considers dead back on the platform's list of
/// live ones.
fn state_from_code(name: &str, wire: &WireOrder, filled: Decimal) -> Result<VenueOrderState> {
    let reason = || -> Result<String> {
        wire.reason
            .as_ref()
            .filter(|text| !text.trim().is_empty())
            .cloned()
            .ok_or_else(|| {
                Error::schema(format!(
                    "{name} reported order {} as {} with no reason. An order withdrawn or refused \
                     without one cannot be told from an order the venue lost, and the reason is \
                     what a person reads first",
                    wire.client_order_id, wire.state
                ))
            })
    };
    match wire.state.as_str() {
        "working" => {
            if filled.is_positive() {
                return Err(Error::schema(format!(
                    "{name} reported order {} as working with {filled} filled; a working order \
                     with fills is partially_filled, and the difference decides what the platform \
                     thinks it owns",
                    wire.client_order_id
                )));
            }
            Ok(VenueOrderState::Working)
        }
        "partially_filled" => {
            if !filled.is_positive() {
                return Err(Error::schema(format!(
                    "{name} reported order {} as partially_filled with {filled} filled",
                    wire.client_order_id
                )));
            }
            if filled >= wire.quantity {
                return Err(Error::schema(format!(
                    "{name} reported order {} as partially_filled with all {filled} of it filled",
                    wire.client_order_id
                )));
            }
            Ok(VenueOrderState::PartiallyFilled { filled })
        }
        "filled" => {
            if filled != wire.quantity {
                return Err(Error::numeric(format!(
                    "{name} reported order {} as filled with {filled} of {} filled. A fully \
                     filled order that filled less than its quantity is a contradiction, and \
                     believing the state over the number would book a position that never traded",
                    wire.client_order_id, wire.quantity
                )));
            }
            Ok(VenueOrderState::Filled)
        }
        "cancelled" => Ok(VenueOrderState::Cancelled { reason: reason()? }),
        "rejected" => {
            if filled.is_positive() || !wire.fills.is_empty() {
                return Err(Error::numeric(format!(
                    "{name} reported order {} as rejected while also reporting {filled} filled \
                     and {} fill(s). A rejected order traded nothing; one of these is wrong and \
                     this adapter will not guess which",
                    wire.client_order_id,
                    wire.fills.len()
                )));
            }
            Ok(VenueOrderState::Rejected { reason: reason()? })
        }
        other => Err(Error::schema(format!(
            "{name} reported order {} in state {other:?}, which this decoder cannot name. It \
             accepts working, partially_filled, filled, cancelled and rejected; an unnamed state \
             is not defaulted to working, because that would put a dead order back on the live list",
            wire.client_order_id
        ))),
    }
}

fn side_from_code(name: &str, code: &str) -> Result<Side> {
    match code {
        "buy" => Ok(Side::Buy),
        "sell" => Ok(Side::Sell),
        other => Err(Error::schema(format!(
            "{name} reported the side {other:?}: this decoder accepts buy and sell, and a guessed \
             side is a position with the wrong sign"
        ))),
    }
}

/// Refuse a venue answer that describes a different order than the one this
/// adapter sent.
///
/// A venue that echoes the client order id and then changes the instrument, the
/// side or the size is either confused or answering about somebody else's
/// order. Either way the answer is not evidence about this one.
fn check_consistent(venue: &VenueId, tracked: &TrackedOrder, decoded: &DecodedOrder) -> Result<()> {
    let name = venue.as_str();
    if tracked.object_id != decoded.object_id {
        return Err(Error::invalid(format!(
            "{name} answered about order {} with instrument {} where it was sent for {}",
            tracked.order_id.as_str(),
            decoded.object_id.as_str(),
            tracked.object_id.as_str()
        )));
    }
    if tracked.side != decoded.side {
        return Err(Error::invalid(format!(
            "{name} answered about order {} on the {} side where it was sent to {}",
            tracked.order_id.as_str(),
            side_code(decoded.side),
            side_code(tracked.side)
        )));
    }
    // Cumulative filled quantity only ever rises. A smaller one is a stale
    // read from a replica, or an answer about an earlier life of the order, and
    // writing it down would walk the platform's view of the position backwards
    // — silently un-booking a trade that really happened.
    if decoded.filled < tracked.filled {
        return Err(Error::numeric(format!(
            "{name} answered about order {} reporting {} filled where it has already \
             reported {}. Filled quantity is cumulative and cannot fall; this adapter \
             will not walk a position backwards on a stale answer",
            tracked.order_id.as_str(),
            decoded.filled,
            tracked.filled
        )));
    }
    if tracked.sends > 0 && tracked.original_quantity != decoded.quantity {
        return Err(Error::invalid(format!(
            "{name} answered about order {} with quantity {} where it was sent for {}; this \
             adapter does not amend, so the two cannot legitimately differ",
            tracked.order_id.as_str(),
            decoded.quantity,
            tracked.original_quantity
        )));
    }
    Ok(())
}

// --- the wire schema --------------------------------------------------------
//
// The shape this adapter speaks, and the whole of what it promises to read. No
// venue is obliged to speak it: a deployment whose venue does not either points
// this adapter at a translating endpoint or writes a second adapter beside it.
// Naming one schema and refusing everything else is the reason a malformed
// answer is an unknown order here rather than a half-populated one downstream.
//
// Unknown fields are ignored, because a venue adding one is not a fault and must
// not stop order entry. Unknown *values* in a field this adapter reads are
// refused, because those change what the record means.

/// What goes out on a submit.
#[derive(Debug, Serialize)]
struct WireSubmit<'a> {
    /// The platform's own order id. It is what the venue is asked to echo, and
    /// what every later query is keyed by.
    client_order_id: &'a str,
    /// Also sent in a header. In the body as well so that a venue whose
    /// deduplication reads the payload finds it, and so a captured request
    /// carries its own evidence of which order it was.
    idempotency_key: &'a str,
    account: &'a str,
    instrument: &'a str,
    side: &'a str,
    quantity: Decimal,
    order_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit_price: Option<Decimal>,
    /// The caller's timestamp, never a local clock read.
    submitted_at: Timestamp,
}

/// What comes back from a submit, a cancel or a query.
#[derive(Debug, Deserialize)]
struct WireOrder {
    client_order_id: String,
    /// The venue's own identifier. Optional, because not every venue issues
    /// one; where it is issued it is what catches a venue that did not
    /// deduplicate a repeated idempotency key.
    #[serde(default)]
    venue_order_id: Option<String>,
    /// `working`, `partially_filled`, `filled`, `cancelled`, `rejected`.
    state: String,
    /// Required even though the client knows it: an answer that names the
    /// instrument can be checked against the order that was sent, and one that
    /// does not has to be taken on trust.
    instrument: String,
    side: String,
    quantity: Decimal,
    /// Cumulative. Absent means zero.
    #[serde(default)]
    filled: Option<Decimal>,
    #[serde(default)]
    limit_price: Option<Decimal>,
    /// Required for `cancelled` and `rejected`.
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    revision: Option<u32>,
    #[serde(default)]
    priority: Option<u64>,
    #[serde(default)]
    submitted_at: Option<Timestamp>,
    #[serde(default)]
    updated_at: Option<Timestamp>,
    /// The fills this response reports. May be empty on a response about an
    /// order that has traded — a venue that reports fills incrementally has
    /// already reported them — which is why `filled` is cumulative and the
    /// fills are checked against it rather than summed into it.
    #[serde(default)]
    fills: Vec<WireFill>,
}

#[derive(Debug, Deserialize)]
struct WireFill {
    /// The venue's identity for the trade. Required: a fill nobody can name
    /// cannot be deduplicated or reconciled.
    fill_id: String,
    quantity: Decimal,
    price: Decimal,
    /// Commission and fees. Required rather than defaulted to zero: an unstated
    /// fee treated as no fee understates cost on every fill, and the error
    /// compounds where nobody is looking.
    costs: Decimal,
    at: Timestamp,
}
