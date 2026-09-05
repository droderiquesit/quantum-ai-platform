//! The wallet statement feed: the one way a venue balance reaches this
//! process.
//!
//! `Platform::observe_statement` is the kernel's only observation channel —
//! it holds no read-only key, no watch-only address and no view key, so the
//! only provenance it can attest is a statement a person handed it. Until
//! this module existed nothing in any binary called it, so every deployed
//! `/wallet` answered `assembled: false` for ever: the LEARN stage's
//! `reconcile_wallet` was complete, tested, and reached by no statement.
//!
//! The statement is a JSON file `QIP_WALLET_STATEMENT_PATH` names — a file
//! mounted by a deployment, a fixture in a test — holding what the desk's
//! broker or custodian reported:
//!
//! ```json
//! {
//!   "as_of": "2026-09-05T06:00:00Z",
//!   "venue": "simulated-venue",
//!   "tolerance": "1",
//!   "holdings": [
//!     { "asset": "USD", "quantity": "10000000" },
//!     { "asset": "BTC", "quantity": "0.5", "tolerance": "0.0001" }
//!   ]
//! }
//! ```
//!
//! Quantities and tolerances are decimal **strings**. A JSON number is
//! refused by name rather than parsed: `serde_json` reads `0.1` as an `f64`
//! and `Decimal::from_f64` would then record a balance the custodian never
//! stated, and a wallet reconciled against a figure the platform invented
//! is a break the platform manufactured itself. `tolerance` at the top
//! level applies to every holding that does not carry its own; a holding
//! with neither is refused, because a defaulted tolerance is a number nobody
//! decided.
//!
//! The composition root reads and validates the file at start, refusing
//! anything malformed, an `as_of` in the future, an empty statement, or more
//! holdings than the kernel will hold — never clamping any of them — and
//! observes it into the platform before serving. Each admitted `POST /cycle`
//! then re-reads the file when its modification time or length has moved,
//! so an operator who replaces the mounted file sees the next cycle's LEARN
//! stage reconcile the new figures. A named file that has stopped reading or
//! parsing refuses the cycle, in the same way a feed that fails to answer
//! does: a cycle over yesterday's statement with a note attached would
//! reconcile a balance the desk has already corrected.
//!
//! Absent variable: no feed, the banner says so, and `/wallet` keeps
//! answering `assembled: false` — honestly, because nothing was observed.

use crate::auth::Authenticator;
use crate::http::{Handler, Method, Request, Response, StreamDecision};
use crate::json;
use crate::routes::Api;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Decimal, Timestamp};
use qip_kernel::Platform;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Where the statement is read from. Unset means no feed.
pub const STATEMENT_PATH_VARIABLE: &str = "QIP_WALLET_STATEMENT_PATH";

/// The most holdings one statement may carry.
///
/// The kernel holds at most 256 venue-assets and refuses the 257th by name;
/// this is the same bound applied where the file is parsed, so a statement
/// past it is refused at start naming the count rather than half-observed
/// and then refused mid-way through the list. The kernel's own check stays
/// the backstop — two statements at different venues still share its bound.
pub const MAX_STATEMENT_HOLDINGS: usize = 256;

/// The keys a statement document may carry, and the keys a holding may.
///
/// Anything else is refused by name: a misspelt `tolerence` that was
/// silently ignored would leave the holding on the default nobody decided,
/// which is exactly the case the required-tolerance rule exists to refuse.
const STATEMENT_KEYS: [&str; 4] = ["as_of", "venue", "tolerance", "holdings"];
const HOLDING_KEYS: [&str; 3] = ["asset", "quantity", "tolerance"];

/// One balance the statement reports, with the tolerance its reconciliation
/// is judged against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatementHolding {
    /// What is held, as the statement names it.
    pub asset: String,
    /// The balance the custodian reported, as the exact decimal text the
    /// file carried. May be negative: a margin account in debit is a real
    /// balance, and the kernel accepts sign. Kept as text rather than a
    /// number because the application layer owns no field typed as money
    /// (`api_boundary.rs`): a struct that could add is a struct that could
    /// drift from the figure it was handed. Validated as a decimal at parse
    /// and parsed again at the one place it is handed to the kernel.
    pub quantity: String,
    /// Strictly positive, validated at parse naming the holding; the kernel
    /// refuses anything else as well. Text for the same reason as the
    /// quantity.
    pub tolerance: String,
}

/// A validated statement: one venue, dated, with at least one holding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    /// When the custodian said the balances were true.
    pub as_of: Timestamp,
    /// The venue, broker or custodian that reported them.
    pub venue: VenueId,
    /// The balances, in file order, each asset at most once.
    pub holdings: Vec<StatementHolding>,
}

impl Statement {
    /// Parse and validate a statement document against `now`.
    ///
    /// `now` is the wall clock: a statement is a document a person dated,
    /// and one dated after the moment it is read is a clock or a typo the
    /// desk should see rather than a balance the LEARN stage would reconcile
    /// as fresh for a day longer than it is. Every refusal names the field.
    pub fn parse(text: &str, now: Timestamp) -> Result<Self> {
        let document: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| Error::invalid(format!("the file is not JSON: {error}")))?;
        let object = object_of(&document, "the statement")?;
        refuse_unknown_keys(object, &STATEMENT_KEYS, "the statement")?;

        let as_of_text = string_field(object, "as_of", "as_of")?;
        let as_of = Timestamp::parse_rfc3339(as_of_text).ok_or_else(|| {
            Error::invalid(format!(
                "as_of is {as_of_text:?}, which is not an RFC 3339 instant such as \
                 2026-09-05T06:00:00Z"
            ))
        })?;
        if as_of > now {
            return Err(Error::invalid(format!(
                "as_of is {} and now is {}; a statement dated in the future is a clock or \
                 a typo, and the wallet would carry it as fresh for longer than it is",
                as_of.to_rfc3339(),
                now.to_rfc3339()
            )));
        }

        let venue_text = string_field(object, "venue", "venue")?;
        if venue_text.trim().is_empty() {
            return Err(Error::invalid(
                "venue is empty; name the venue, broker or custodian the statement is from",
            ));
        }
        let venue = VenueId::new(venue_text);

        let default_tolerance = match object.get("tolerance") {
            None => None,
            Some(value) => {
                let tolerance = decimal_of(value, "tolerance")?;
                if !tolerance.is_positive() {
                    return Err(Error::invalid(format!(
                        "tolerance is {tolerance}; a tolerance is the largest gap \
                         reconciliation accepts and must be strictly positive"
                    )));
                }
                Some(decimal_text_of(value, "tolerance")?)
            }
        };

        let holdings_value = object.get("holdings").ok_or_else(|| {
            Error::invalid("holdings is missing; a statement lists what the venue holds")
        })?;
        let holdings_list = holdings_value.as_array().ok_or_else(|| {
            Error::invalid("holdings is not a list; a statement lists what the venue holds")
        })?;
        if holdings_list.is_empty() {
            return Err(Error::invalid(format!(
                "holdings is empty; a statement of nothing observes nothing, and the wallet \
                 would stay unassembled while the banner said a feed was on. Unset \
                 {STATEMENT_PATH_VARIABLE} to run with no feed"
            )));
        }
        if holdings_list.len() > MAX_STATEMENT_HOLDINGS {
            return Err(Error::denied(format!(
                "holdings lists {} entries against a bound of {MAX_STATEMENT_HOLDINGS}; the \
                 kernel holds no more venue-assets than that, and a statement past it would \
                 be observed only in part",
                holdings_list.len()
            )));
        }

        let mut holdings = Vec::with_capacity(holdings_list.len());
        for (index, value) in holdings_list.iter().enumerate() {
            let at = format!("holdings[{index}]");
            let holding = object_of(value, &at)?;
            refuse_unknown_keys(holding, &HOLDING_KEYS, &at)?;
            let asset = string_field(holding, "asset", &format!("{at}.asset"))?;
            if asset.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "{at}.asset is empty; an unnamed balance would key every unnamed balance \
                     together"
                )));
            }
            if holdings
                .iter()
                .any(|seen: &StatementHolding| seen.asset == asset)
            {
                return Err(Error::invalid(format!(
                    "{at}.asset is {asset:?}, which an earlier holding already states; two \
                     claims about one balance will disagree and the later one would win \
                     silently"
                )));
            }
            let quantity_value = holding.get("quantity").ok_or_else(|| {
                Error::invalid(format!("{at}.quantity is missing; a holding is a balance"))
            })?;
            let quantity = decimal_text_of(quantity_value, &format!("{at}.quantity"))?;
            let tolerance = match holding.get("tolerance") {
                Some(value) => {
                    let text = decimal_text_of(value, &format!("{at}.tolerance"))?;
                    let tolerance = decimal_of(value, &format!("{at}.tolerance"))?;
                    if !tolerance.is_positive() {
                        return Err(Error::invalid(format!(
                            "{at}.tolerance is {tolerance}; a tolerance is the largest gap \
                             reconciliation accepts and must be strictly positive"
                        )));
                    }
                    text
                }
                None => default_tolerance.clone().ok_or_else(|| {
                    Error::invalid(format!(
                        "{at}.tolerance is missing and the statement sets no tolerance; a \
                         defaulted tolerance is a number nobody decided, so set one on the \
                         holding or on the statement"
                    ))
                })?,
            };
            holdings.push(StatementHolding {
                asset: asset.to_string(),
                quantity,
                tolerance,
            });
        }

        Ok(Self {
            as_of,
            venue,
            holdings,
        })
    }

    /// Hand every holding to the platform, so the next LEARN stage assembles
    /// and reconciles against them.
    ///
    /// The kernel is the judge of the asset name and of its venue-asset
    /// bound; a refusal from it stops the caller with the kernel's own
    /// message. Holdings before the refused one have been observed and the
    /// rest have not — the kernel offers no transaction — which is why the
    /// root refuses to start on it and the middleware refuses the cycle and
    /// re-applies the whole file at the next change.
    pub fn observe_into(&self, platform: &mut Platform) -> Result<()> {
        for holding in &self.holdings {
            // Parsed here, at the hand-over, from the text the parser already
            // validated; a failure is a defect in that validation, named.
            let quantity = Decimal::parse(&holding.quantity).ok_or_else(|| {
                Error::invalid(format!(
                    "{}.quantity {:?} passed validation and does not parse",
                    holding.asset, holding.quantity
                ))
            })?;
            let tolerance = Decimal::parse(&holding.tolerance).ok_or_else(|| {
                Error::invalid(format!(
                    "{}.tolerance {:?} passed validation and does not parse",
                    holding.asset, holding.tolerance
                ))
            })?;
            platform.observe_statement(
                self.venue.clone(),
                &holding.asset,
                quantity,
                tolerance,
                self.as_of,
            )?;
        }
        Ok(())
    }
}

/// What the file looked like when it was last read: modification time and
/// length.
///
/// Both, because a file rewritten within the filesystem's timestamp
/// granularity keeps its modification time, and a length that also stayed
/// the same is the one edit this cannot see — a figure retyped with the same
/// number of digits inside the same tick. Said here so nobody reads the
/// check as content-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    modified: Option<SystemTime>,
    len: u64,
}

impl Fingerprint {
    fn of(path: &str) -> Result<Self> {
        let metadata = std::fs::metadata(path).map_err(|error| {
            Error::io(format!(
                "{STATEMENT_PATH_VARIABLE} names {path}, which cannot be read: {error}. Unset \
                 it to run with no feed; a named statement that does not read is not a feed \
                 that is off"
            ))
        })?;
        Ok(Self {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        })
    }
}

/// The statement file this process re-reads, and the statement it last
/// applied.
#[derive(Debug)]
pub struct StatementFeed {
    path: String,
    fingerprint: Fingerprint,
    statement: Statement,
}

impl StatementFeed {
    /// Open the feed the environment names, or `None` when it names none.
    ///
    /// The environment is passed in rather than read: the composition root
    /// is the one place that may read it, and a test hands in a map.
    pub fn from_env(
        lookup: &dyn Fn(&str) -> Option<String>,
        now: Timestamp,
    ) -> Result<Option<Self>> {
        match lookup(STATEMENT_PATH_VARIABLE).filter(|value| !value.trim().is_empty()) {
            None => Ok(None),
            Some(path) => Self::open(&path, now).map(Some),
        }
    }

    /// Read and validate the file at `path`, refusing it by name.
    pub fn open(path: &str, now: Timestamp) -> Result<Self> {
        let (fingerprint, statement) = read(path, now)?;
        Ok(Self {
            path: path.to_string(),
            fingerprint,
            statement,
        })
    }

    /// The statement as last read.
    pub fn statement(&self) -> &Statement {
        &self.statement
    }

    /// Where the statement is read from.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Re-read the file when it has changed since the last read.
    ///
    /// `Some` carries the new statement, already replacing the held one;
    /// `None` means the file is as it was. A file that has gone, or has
    /// changed into something the parser refuses, is an error and the held
    /// statement is left as it was — the caller refuses the cycle rather
    /// than reconciling against a statement the desk has since withdrawn.
    pub fn refresh(&mut self, now: Timestamp) -> Result<Option<&Statement>> {
        let fingerprint = Fingerprint::of(&self.path)?;
        if fingerprint == self.fingerprint {
            return Ok(None);
        }
        let (fingerprint, statement) = read(&self.path, now)?;
        self.fingerprint = fingerprint;
        self.statement = statement;
        Ok(Some(&self.statement))
    }

    /// The banner line: where the statement is from and what it says.
    pub fn describe(&self) -> String {
        format!(
            "statement from {}: {} holding(s) at {} as of {}; re-read before each POST /cycle \
             when the file changes, and reconciled in LEARN",
            self.path,
            self.statement.holdings.len(),
            self.statement.venue,
            self.statement.as_of.to_rfc3339()
        )
    }
}

/// The banner line when no feed is configured.
///
/// Says the consequence rather than only the absence: an operator reading
/// `assembled: false` on `/wallet` should find the reason here, not in a
/// kernel comment.
pub fn absent_banner() -> String {
    format!(
        "none ({STATEMENT_PATH_VARIABLE} is not set); no venue balance is observed, nothing \
         is reconciled in LEARN, and /wallet answers assembled: false"
    )
}

/// Read and validate the file, refusing it by name.
fn read(path: &str, now: Timestamp) -> Result<(Fingerprint, Statement)> {
    let fingerprint = Fingerprint::of(path)?;
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::io(format!(
            "{STATEMENT_PATH_VARIABLE} names {path}, which cannot be read: {error}. Unset it \
             to run with no feed; a named statement that does not read is not a feed that is \
             off"
        ))
    })?;
    let statement = Statement::parse(&text, now).map_err(|error| {
        Error::invalid(format!(
            "{STATEMENT_PATH_VARIABLE} names {path}, which is not a wallet statement: {}",
            error.message()
        ))
    })?;
    Ok((fingerprint, statement))
}

fn object_of<'a>(
    value: &'a serde_json::Value,
    at: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>> {
    value
        .as_object()
        .ok_or_else(|| Error::invalid(format!("{at} is not a JSON object")))
}

fn refuse_unknown_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    known: &[&str],
    at: &str,
) -> Result<()> {
    for key in object.keys() {
        if !known.contains(&key.as_str()) {
            return Err(Error::invalid(format!(
                "{at} carries the key {key:?}, which is not one of {known:?}; a key nothing \
                 reads is a value the desk believes is in force and is not"
            )));
        }
    }
    Ok(())
}

fn string_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    at: &str,
) -> Result<&'a str> {
    let value = object
        .get(key)
        .ok_or_else(|| Error::invalid(format!("{at} is missing")))?;
    value
        .as_str()
        .ok_or_else(|| Error::invalid(format!("{at} is {value}, which is not a string")))
}

/// The exact text of a decimal from a JSON string, validated as a decimal
/// and kept as the text it was, for a field the API may hold.
fn decimal_text_of(value: &serde_json::Value, at: &str) -> Result<String> {
    decimal_of(value, at)?;
    Ok(value.as_str().unwrap_or_default().trim().to_string())
}

/// A decimal from a JSON string, and only from a string.
fn decimal_of(value: &serde_json::Value, at: &str) -> Result<Decimal> {
    let Some(text) = value.as_str() else {
        return Err(Error::invalid(format!(
            "{at} is {value}, which is not a string; write decimals as strings such as \
             \"0.1\", because a JSON number is read as a float and a balance the custodian \
             stated as 0.1 would be recorded as something else"
        )));
    };
    Decimal::parse(text)
        .ok_or_else(|| Error::invalid(format!("{at} is {text:?}, which is not a decimal")))
}

/// The handler that re-reads the statement before an admitted `POST /cycle`.
///
/// Wraps the router rather than living in `Api`, so the API's cycle handler
/// is unchanged and a process with no feed has no code on the cycle path
/// that could re-read anything. The request is put through the same
/// authenticator and the route table's own required role before the file is
/// touched; an unauthenticated caller changes nothing about the platform and
/// is then refused by the API exactly as before. The rate limiter is not
/// consulted here, so the caller's allowance is spent once, by the API.
///
/// Lock order is the platform's rule — platform first, then the feed — so
/// this cannot deadlock against a cycle already holding the platform.
pub struct StatementRefresh<H> {
    inner: H,
    feed: Arc<Mutex<StatementFeed>>,
    platform: Arc<Mutex<Platform>>,
    authenticator: Arc<Authenticator>,
    clock: Arc<dyn Clock>,
}

impl<H> std::fmt::Debug for StatementRefresh<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatementRefresh").finish_non_exhaustive()
    }
}

impl<H: Handler> StatementRefresh<H> {
    pub fn new(
        inner: H,
        feed: Arc<Mutex<StatementFeed>>,
        platform: Arc<Mutex<Platform>>,
        authenticator: Arc<Authenticator>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            inner,
            feed,
            platform,
            authenticator,
            clock,
        }
    }

    /// Whether `request` is a `POST /cycle` the API would admit.
    fn is_admitted_cycle(&self, request: &Request) -> bool {
        let Some(route) = Api::route_for(request.method, &request.path) else {
            return false;
        };
        if route.method != Method::Post || route.pattern != "/cycle" {
            return false;
        }
        let now = self.clock.now();
        self.authenticator
            .authenticate(request.header("authorization"), now)
            .and_then(|principal| principal.require(route.required_role))
            .is_ok()
    }

    /// Re-read the file if it moved and observe it; `Err` is the refusal
    /// the cycle is answered with.
    fn refresh(&self) -> Result<()> {
        let now = self.clock.now();
        let mut platform = self
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform is in an inconsistent state"))?;
        let mut feed = self
            .feed
            .lock()
            .map_err(|_| Error::invalid("the statement feed is in an inconsistent state"))?;
        if let Some(statement) = feed.refresh(now)? {
            statement.observe_into(&mut platform)?;
        }
        Ok(())
    }
}

impl<H: Handler> Handler for StatementRefresh<H> {
    fn handle(&self, request: &Request) -> Response {
        if self.is_admitted_cycle(request) {
            if let Err(error) = self.refresh() {
                eprintln!(
                    "qip-api: the wallet statement did not read: {}",
                    error.message()
                );
                return Response::json(
                    503,
                    format!(
                        r#"{{"error":{},"source":{}}}"#,
                        json::string(error.message()),
                        json::string(STATEMENT_PATH_VARIABLE)
                    ),
                );
            }
        }
        self.inner.handle(request)
    }

    fn stream(&self, request: &Request) -> StreamDecision {
        self.inner.stream(request)
    }
}
