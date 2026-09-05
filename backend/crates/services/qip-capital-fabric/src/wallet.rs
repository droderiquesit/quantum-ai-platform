//! The wallet — the platform's single view of every unit of capital, and
//! nothing that can move one (blueprint §38, ADR 0021).
//!
//! Blueprint §38.1 splits the wallet into two systems that must never share a
//! code path: a *read* path that aggregates balances continuously over
//! read-only credentials, and a *write* path — the transfer engine, the gate
//! and the custody policy engine — that moves capital between venues. The
//! blueprint's guarantee is that a fully compromised read path leaks balances
//! and cannot move a dollar.
//!
//! This platform implements the read path and, by ADR 0021, **the write path
//! does not exist**. There is no transfer engine, no custody policy *engine*
//! (the policy itself is data, in [`crate::custody`]) and no signing of any
//! kind, and ADR 0023 keeps it that way until a separately approved decision
//! says otherwise. The read path's "holds no key material" property is
//! therefore not enforced here by a dependency audit as §38.1 proposes; it is
//! structural. No type in this module has a field that could hold a
//! credential, an address the platform controls, or a key. A
//! [`HoldingObservation`] records *which class* of read-only channel reported
//! a balance ([`Provenance`]) and never the channel itself.
//!
//! # What a wallet does
//!
//! [`Wallet::assemble`] pairs what venues report ([`HoldingObservation`])
//! with what the ledger believes ([`LedgerView`]) and refuses evidence it
//! cannot trust: an observation older than the caller's freshness bound, one
//! dated in the future, or two observations of the same venue-asset. Two
//! independent claims about the same fact will disagree, and the wallet does
//! not choose between them.
//!
//! [`Wallet::reconcile`] then runs §38.3's arithmetic per venue-asset:
//!
//! ```text
//! expected = ledger_balance - reserved + in_flight
//! delta    = observed - expected
//! |delta| < tolerance  -> reconciled
//! otherwise            -> HALT that venue-asset, alert, never correct
//! ```
//!
//! A halt is a [`ReconciliationOutcome::Halt`] carrying a
//! [`ReconciliationAlert`]. It names one venue-asset and touches no other,
//! and it is a value: it holds no reference to the ledger, and there is no
//! method anywhere on [`Wallet`] that mutates a ledger balance. The blueprint's
//! "the Wallet never writes a correction to the ledger" is here a property of
//! the API rather than of the operator's restraint. A surplus halts exactly as
//! a shortfall does — an external balance above expectation is money the
//! ledger cannot explain, which is as serious as money it cannot find.
//!
//! # Tolerance
//!
//! §38.3 makes tolerance a formula per asset class — a funding interval's
//! accrual for perpetuals, a day's interest for fiat, dust for spot — and the
//! evaluation of that formula belongs to whoever holds the rates. The wallet
//! takes the evaluated figures as a [`TolerancePolicy`] keyed by asset, refuses
//! one that is not strictly positive, and refuses to reconcile an asset it has
//! no tolerance for rather than guessing a generous one.

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// An asset the wallet can hold a balance of — a currency, a coin, a token, a
/// collateral line.
///
/// A newtype rather than [`qip_core::Currency`] because a venue-custodied
/// holding is not always a three-letter code, and rather than a bare string
/// so an asset cannot be silently compared against a venue or a label.
/// Non-empty by construction: an empty asset would key every unnamed balance
/// onto one entry and reconcile them against each other.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Asset(String);

impl Asset {
    /// Name an asset, refusing an empty or whitespace-only name.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "an asset needs a name; an empty one would key every unnamed balance together",
            ));
        }
        Ok(Self(name))
    }

    /// The asset's name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Asset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which class of read-only channel reported a balance (§38.1, §38.2).
///
/// Deliberately an enum of *kinds* rather than a record of the channel. The
/// point of §38.1 is that the read path holds nothing capable of moving
/// capital; the surest way to hold no credential is to have no field for one.
/// A wallet that stored "the API key this came from" for auditability would
/// have crossed the line the audit exists to check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// A venue or broker balance read over a key scoped to reading.
    ReadOnlyApiKey,
    /// A chain balance at an address the platform watches but cannot spend from.
    WatchOnlyAddress,
    /// A balance decoded with a view key that reveals and cannot spend.
    ViewKey,
    /// A custodian, bank or administrator statement.
    Statement,
}

impl Provenance {
    /// A stable label for alerts and logs.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnlyApiKey => "read_only_api_key",
            Self::WatchOnlyAddress => "watch_only_address",
            Self::ViewKey => "view_key",
            Self::Statement => "statement",
        }
    }
}

/// One venue-asset — the unit reconciliation halts.
///
/// Ordered so a wallet iterates its holdings in a stable sequence and two
/// reconciliations of the same inputs produce byte-identical outcomes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct VenueAsset {
    /// The venue, broker, custodian or chain holding it.
    pub venue: VenueId,
    /// What is held.
    pub asset: Asset,
}

impl fmt::Display for VenueAsset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.venue, self.asset)
    }
}

/// A balance an external system reported, and when, and through what kind of
/// channel.
///
/// The observed figure may be negative — a margin account in debit is a real
/// balance — so nothing here refuses sign. What is refused is age, at
/// assembly, because a balance read yesterday reconciled against a ledger
/// updated today is a break the wallet manufactured itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldingObservation {
    /// Where the balance was read.
    pub venue: VenueId,
    /// What was read.
    pub asset: Asset,
    /// The balance the external system reported.
    pub observed: Decimal,
    /// When the external system said it was true.
    pub observed_at: Timestamp,
    /// Which class of read-only channel reported it.
    pub provenance: Provenance,
}

impl HoldingObservation {
    /// Record an observation.
    pub fn new(
        venue: VenueId,
        asset: Asset,
        observed: Decimal,
        observed_at: Timestamp,
        provenance: Provenance,
    ) -> Self {
        Self {
            venue,
            asset,
            observed,
            observed_at,
            provenance,
        }
    }

    /// The venue-asset this is evidence about.
    pub fn key(&self) -> VenueAsset {
        VenueAsset {
            venue: self.venue.clone(),
            asset: self.asset.clone(),
        }
    }
}

/// What the ledger believes about one venue-asset: the booked balance, the
/// part of it reserved against intents, and the part still in flight towards
/// it.
///
/// Reserved and in-flight are quantities and are refused if negative — a
/// negative reservation is a sign error that would inflate the expected
/// balance and hide a shortfall of exactly its size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerView {
    /// The venue the ledger books this balance at.
    pub venue: VenueId,
    /// The asset.
    pub asset: Asset,
    /// The balance the ledger has booked.
    pub ledger_balance: Decimal,
    /// The portion reserved against open intents, and so not expected at the
    /// venue as free balance.
    pub reserved: Decimal,
    /// Capital instructed towards this venue-asset and not yet booked.
    pub in_flight: Decimal,
}

impl LedgerView {
    /// Record the ledger's view, refusing a negative reservation or a negative
    /// in-flight quantity.
    pub fn new(
        venue: VenueId,
        asset: Asset,
        ledger_balance: Decimal,
        reserved: Decimal,
        in_flight: Decimal,
    ) -> Result<Self> {
        if reserved.is_negative() {
            return Err(Error::invalid(format!(
                "reserved for {venue}/{asset} is {reserved}; a reservation is a quantity and \
                 cannot be negative — book a release instead"
            )));
        }
        if in_flight.is_negative() {
            return Err(Error::invalid(format!(
                "in-flight for {venue}/{asset} is {in_flight}; capital leaving is the other \
                 venue-asset's in-flight, not a negative arrival here"
            )));
        }
        Ok(Self {
            venue,
            asset,
            ledger_balance,
            reserved,
            in_flight,
        })
    }

    /// The venue-asset this view describes.
    pub fn key(&self) -> VenueAsset {
        VenueAsset {
            venue: self.venue.clone(),
            asset: self.asset.clone(),
        }
    }

    /// §38.3: `expected = ledger_balance - reserved + in_flight`.
    ///
    /// Checked arithmetic, because an overflow that saturated would reconcile
    /// a balance against a number nobody computed.
    pub fn expected(&self) -> Result<Decimal> {
        self.ledger_balance
            .checked_sub(self.reserved)
            .and_then(|free| free.checked_add(self.in_flight))
            .ok_or_else(|| {
                Error::numeric(format!(
                    "expected balance for {}/{} overflowed from ledger {} - reserved {} + \
                     in-flight {}",
                    self.venue, self.asset, self.ledger_balance, self.reserved, self.in_flight
                ))
            })
    }
}

/// The evaluated tolerance per asset for one reconciliation pass.
///
/// Strictly positive per asset. A zero tolerance halts on a delta of zero,
/// which is every reconciled balance; a negative one halts on nothing at all.
/// Both are refused at construction rather than clamped to something that
/// reads like a control.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TolerancePolicy {
    per_asset: BTreeMap<Asset, Decimal>,
}

impl TolerancePolicy {
    /// An empty policy — reconciling anything against it is refused.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set an asset's tolerance, refusing anything that is not strictly positive.
    pub fn with_tolerance(mut self, asset: Asset, tolerance: Decimal) -> Result<Self> {
        if !tolerance.is_positive() {
            return Err(Error::invalid(format!(
                "tolerance for {asset} is {tolerance}; it must be strictly positive — zero \
                 halts every reconciled balance and a negative figure halts none"
            )));
        }
        self.per_asset.insert(asset, tolerance);
        Ok(self)
    }

    /// The tolerance for an asset, or `None` when the caller never supplied one.
    pub fn for_asset(&self, asset: &Asset) -> Option<Decimal> {
        self.per_asset.get(asset).copied()
    }
}

/// Why a venue-asset halted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakCause {
    /// Observed and expected disagree by at least the tolerance.
    DeltaBeyondTolerance,
    /// The venue reports a holding the ledger has never booked. The ledger's
    /// balance, reservation and in-flight for it are all nothing, so the
    /// expectation is zero by fact rather than by guess.
    UnrecordedByLedger,
}

impl BreakCause {
    /// A stable label for alerts and metrics.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::DeltaBeyondTolerance => "delta_beyond_tolerance",
            Self::UnrecordedByLedger => "unrecorded_by_ledger",
        }
    }
}

/// The alert a halt raises, with the figures behind it.
///
/// A record, complete in itself: everything an operator needs to investigate
/// without touching the wallet again. It carries no handle to the ledger, so
/// there is nothing an alert consumer could correct through it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationAlert {
    /// The venue-asset that halted.
    pub venue_asset: VenueAsset,
    /// Why.
    pub cause: BreakCause,
    /// What the ledger expected.
    pub expected: Decimal,
    /// What the external system reported.
    pub observed: Decimal,
    /// `observed - expected`, signed: a surplus is as much a break as a shortfall.
    pub delta: Decimal,
    /// The tolerance the delta was judged against.
    pub tolerance: Decimal,
    /// When the observation was true.
    pub observed_at: Timestamp,
    /// Which class of read-only channel reported it.
    pub provenance: Provenance,
    /// What to do, in a sentence.
    pub message: String,
}

/// The result of reconciling one venue-asset.
///
/// A value, not a handle: it holds no reference to the ledger or to the
/// wallet, and the `Halt` arm instructs rather than acts. That is the whole
/// of §38.3's "never auto-correct" — there is no correction for anything to
/// apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ReconciliationOutcome {
    /// The delta is inside tolerance.
    Reconciled {
        /// The venue-asset.
        venue: VenueId,
        /// The asset.
        asset: Asset,
        /// `observed - expected`, kept so a persistent non-zero delta inside
        /// tolerance — §38.3's modelling defect — can be seen rather than lost.
        delta: Decimal,
    },
    /// Halt this venue-asset and alert. Nothing else is touched.
    Halt {
        /// The venue-asset.
        venue: VenueId,
        /// The asset.
        asset: Asset,
        /// `observed - expected`.
        delta: Decimal,
        /// The record for the operator.
        alert: ReconciliationAlert,
    },
}

impl ReconciliationOutcome {
    /// The venue-asset the outcome is about.
    pub fn venue_asset(&self) -> VenueAsset {
        match self {
            Self::Reconciled { venue, asset, .. } | Self::Halt { venue, asset, .. } => VenueAsset {
                venue: venue.clone(),
                asset: asset.clone(),
            },
        }
    }

    /// Whether this outcome halts its venue-asset.
    pub fn is_halt(&self) -> bool {
        matches!(self, Self::Halt { .. })
    }
}

/// The read model: every observed holding paired with the ledger's view of it.
///
/// Built only by [`Wallet::assemble`], read only through `&self`. There is no
/// method that changes a balance, a reservation or an in-flight figure after
/// assembly, because a wallet that could would be the write path §38.1 keeps
/// in a different trust zone and ADR 0021 keeps out of this platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wallet {
    observed: BTreeMap<VenueAsset, HoldingObservation>,
    ledger: BTreeMap<VenueAsset, LedgerView>,
    as_of: Timestamp,
}

impl Wallet {
    /// Assemble a wallet from what venues reported and what the ledger believes.
    ///
    /// Refuses, rather than quietly using:
    ///
    /// * a non-positive freshness bound — it would make every observation stale;
    /// * an observation older than `freshness` before `now`, because a balance
    ///   that was true yesterday reconciled against today's ledger is a break
    ///   the wallet invented;
    /// * an observation dated after `now`, which is a clock fault somewhere
    ///   and is not evidence about anything;
    /// * two observations, or two ledger views, of the same venue-asset — two
    ///   claims about one fact, and the wallet does not pick the louder one;
    /// * a ledger view with no observation — the ledger's belief is not
    ///   evidence, and a venue-asset nobody has looked at cannot be reconciled
    ///   and must not be reported as though it were.
    ///
    /// An observation with no ledger view is accepted and halts at
    /// reconciliation as [`BreakCause::UnrecordedByLedger`]: that is a break
    /// the wallet exists to find, not a defect in its inputs.
    pub fn assemble(
        observations: Vec<HoldingObservation>,
        ledger_views: Vec<LedgerView>,
        freshness: Duration,
        now: Timestamp,
    ) -> Result<Self> {
        if freshness <= Duration::ZERO {
            return Err(Error::invalid(format!(
                "freshness bound is {freshness:?}; it must be positive or every observation \
                 is stale"
            )));
        }
        let mut observed = BTreeMap::new();
        for observation in observations {
            let key = observation.key();
            if observation.observed_at > now {
                return Err(Error::invalid(format!(
                    "observation of {key} is dated {} but now is {now}; a balance from the \
                     future is a clock fault, not evidence",
                    observation.observed_at
                )));
            }
            let age = now.since(observation.observed_at);
            if age > freshness {
                return Err(Error::invalid(format!(
                    "observation of {key} is {age:?} old against a freshness bound of \
                     {freshness:?}; re-read the balance rather than reconciling a stale one"
                )));
            }
            if observed.insert(key.clone(), observation).is_some() {
                return Err(Error::invalid(format!(
                    "two observations of {key} were supplied; the wallet does not choose \
                     between competing claims about one balance"
                )));
            }
        }
        let mut ledger = BTreeMap::new();
        for view in ledger_views {
            let key = view.key();
            if !observed.contains_key(&key) {
                return Err(Error::invalid(format!(
                    "the ledger books {key} but no observation of it was supplied; a \
                     venue-asset nobody has read cannot be reconciled"
                )));
            }
            if ledger.insert(key.clone(), view).is_some() {
                return Err(Error::invalid(format!(
                    "two ledger views of {key} were supplied; the ledger holds one balance \
                     per venue-asset"
                )));
            }
        }
        Ok(Self {
            observed,
            ledger,
            as_of: now,
        })
    }

    /// The instant the wallet was assembled against.
    pub fn as_of(&self) -> Timestamp {
        self.as_of
    }

    /// Every venue-asset the wallet knows about, in stable order.
    pub fn venue_assets(&self) -> impl Iterator<Item = &VenueAsset> {
        self.observed.keys()
    }

    /// The observation for a venue-asset, if any.
    pub fn observation(&self, key: &VenueAsset) -> Option<&HoldingObservation> {
        self.observed.get(key)
    }

    /// The ledger's view of a venue-asset, if it has one.
    pub fn ledger_view(&self, key: &VenueAsset) -> Option<&LedgerView> {
        self.ledger.get(key)
    }

    /// Reconcile every venue-asset against its tolerance (§38.3).
    ///
    /// Returns one outcome per venue-asset in stable order. Refuses — for the
    /// whole pass — when an observed asset has no tolerance, because a
    /// reconciliation that guessed a tolerance for one asset would report
    /// "reconciled" on a figure nobody chose. A halt on one venue-asset does
    /// not affect any other's outcome, and nothing in this method or reachable
    /// from it writes to the ledger.
    pub fn reconcile(&self, tolerances: &TolerancePolicy) -> Result<Vec<ReconciliationOutcome>> {
        let mut outcomes = Vec::with_capacity(self.observed.len());
        for (key, observation) in &self.observed {
            let tolerance = tolerances.for_asset(&key.asset).ok_or_else(|| {
                Error::invalid(format!(
                    "no tolerance was supplied for {}; reconciliation will not guess one",
                    key.asset
                ))
            })?;
            let (expected, cause) = match self.ledger.get(key) {
                Some(view) => (view.expected()?, BreakCause::DeltaBeyondTolerance),
                None => (Decimal::ZERO, BreakCause::UnrecordedByLedger),
            };
            let delta = observation.observed.checked_sub(expected).ok_or_else(|| {
                Error::numeric(format!(
                    "delta for {key} overflowed from observed {} - expected {expected}",
                    observation.observed
                ))
            })?;
            let breaks = cause == BreakCause::UnrecordedByLedger || delta.abs() >= tolerance;
            if breaks {
                let message = format!(
                    "halt {key}: {} — observed {} against expected {expected} (delta {delta}, \
                     tolerance {tolerance}) as of {} via {}; investigate at the venue and the \
                     ledger, the wallet writes no correction",
                    cause.as_str(),
                    observation.observed,
                    observation.observed_at,
                    observation.provenance.as_str(),
                );
                outcomes.push(ReconciliationOutcome::Halt {
                    venue: key.venue.clone(),
                    asset: key.asset.clone(),
                    delta,
                    alert: ReconciliationAlert {
                        venue_asset: key.clone(),
                        cause,
                        expected,
                        observed: observation.observed,
                        delta,
                        tolerance,
                        observed_at: observation.observed_at,
                        provenance: observation.provenance,
                        message,
                    },
                });
            } else {
                outcomes.push(ReconciliationOutcome::Reconciled {
                    venue: key.venue.clone(),
                    asset: key.asset.clone(),
                    delta,
                });
            }
        }
        Ok(outcomes)
    }
}
