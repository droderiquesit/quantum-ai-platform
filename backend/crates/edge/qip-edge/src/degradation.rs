//! The degradation envelope a reflex cell declares for every input it depends
//! on, refused at assembly if any input has none (R12-RES-8026).
//!
//! # The failure this prevents
//!
//! A cell's behaviour as an input ages was spread across four places: the
//! payload slots' time to live, [`CellConfig::max_staleness`],
//! [`CellConfig::max_clock_skew`], the venue-health quarantine, and the
//! multipliers in `qip_contracts::degradation`. Nothing stated, per input,
//! *both* when it stops being trusted and what that does to size, and nothing
//! refused a cell that depended on an input whose loss had never been
//! decided. A dependency nobody wrote a row for behaves however the code
//! around it happens to fall, which is a decision made by omission.
//!
//! [`DegradationEnvelope::for_cell`] derives the table from the cell's own
//! configuration and from the contract's multipliers — it restates neither —
//! and [`DegradationEnvelope::new`] refuses it unless every input the cell
//! reads has exactly one row with a usable bound and multipliers that only
//! ever narrow. [`crate::cell::Cell::new`] calls it, so a configuration that
//! leaves an input without an envelope never becomes a cell.
//!
//! # Ingestion is a row, and a different kind of row
//!
//! Ingestion has no capability-level narrowing in a cell: a book older than
//! [`CellConfig::max_staleness`] is refused outright, per book, under the
//! `stale_book` gate, which is stricter than a table multiplier would be —
//! a multiplier would still trade the stale book at a fraction of its size.
//! Leaving it out of the table would make that choice invisible, and the
//! next reader would take the gap for an oversight. So it has a row whose
//! bound is [`FreshnessBound::PerBook`] and whose multipliers are one, and a
//! table that omits it, or states it as an ordinary age, is refused with a
//! message saying where its envelope actually lives.
//!
//! # What is not a dependency of a cell
//!
//! Counterfactual scoring never ships and §6.2 gives its loss no trading
//! impact; the self-model is the centre's alone; custody has no reading in a
//! cell. A row for any of them would declare behaviour nothing exercises, so
//! one is refused the way an unknown venue's quote limits are.

use crate::cell::CellConfig;
use qip_contracts::degradation::{Capability, DegradationState, Freshness};
use qip_contracts::policy::PolicyItem;
use qip_core::{Decimal, Duration, Error, Result};
use std::collections::{BTreeMap, BTreeSet};

/// When a row's input stops reading fresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreshnessBound {
    /// The input reads stale once it is older than this. The payload slot's
    /// time to live for the three policy-fed rows; the silence a venue feed
    /// is allowed for [`Capability::Feed`].
    Age(Duration),
    /// How far ahead of the cell's clock a venue may stamp a message before
    /// [`Capability::Clock`] reads that venue as skewed.
    Lead(Duration),
    /// How long a venue quarantined for rejecting orders stays withdrawn
    /// before it is tried again — the interval over which
    /// [`Capability::VenueSession`] reads narrowed.
    Quarantine(Duration),
    /// Judged per book and refused outright past this age, under the
    /// `stale_book` gate. Only [`Capability::Ingestion`] has this bound.
    PerBook(Duration),
    /// The node judges the journal spool's heartbeat and occupancy and hands
    /// the cell the reading (`crate::pressure`); the cell reads no clock for
    /// [`Capability::Ledger`] and has no duration of its own to state.
    SpoolReading,
    /// Derived from the policy-fed rows, so it ages as they do; the duration
    /// is the shortest of their times to live, the instant the first of them
    /// can go stale and the fabric row with it.
    DerivedFromPayload(Duration),
}

impl FreshnessBound {
    /// The duration this bound states, where it states one.
    pub const fn duration(&self) -> Option<Duration> {
        match self {
            Self::Age(bound)
            | Self::Lead(bound)
            | Self::Quarantine(bound)
            | Self::PerBook(bound)
            | Self::DerivedFromPayload(bound) => Some(*bound),
            Self::SpoolReading => None,
        }
    }
}

/// One input's envelope: when it stops being trusted, and what its partial
/// and total loss do to the size of a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvelopeRow {
    pub bound: FreshnessBound,
    /// The multiplier while the row reads [`Freshness::Stale`].
    pub stale: Decimal,
    /// The multiplier while the row reads [`Freshness::Unavailable`].
    pub unavailable: Decimal,
}

/// Every input a cell reads, with its envelope; validated at construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DegradationEnvelope {
    rows: BTreeMap<Capability, EnvelopeRow>,
}

impl DegradationEnvelope {
    /// The inputs a cell assembled from `config` depends on.
    ///
    /// The ledger only with the journal wire and the venue session only with
    /// venue health: a cell without either has no reading of that input,
    /// records no row for it in [`crate::cell::Cell::narrowing`], and a row
    /// here would be an envelope for a number nobody computes.
    pub fn dependencies_of(config: &CellConfig) -> BTreeSet<Capability> {
        let mut required: BTreeSet<Capability> = [
            Capability::Ingestion,
            Capability::CausalGraph,
            Capability::EpisodicMemory,
            Capability::BeliefState,
            Capability::Fabric,
            Capability::Feed,
            Capability::Clock,
        ]
        .into_iter()
        .collect();
        if config.journal_wire {
            required.insert(Capability::Ledger);
        }
        if config.venue_health.is_some() {
            required.insert(Capability::VenueSession);
        }
        required
    }

    /// The rows `config` and the contract's table imply, before validation.
    ///
    /// Exposed so a test can remove one and watch [`Self::new`] refuse; the
    /// only production path is [`Self::for_cell`].
    pub fn rows_for(config: &CellConfig) -> BTreeMap<Capability, EnvelopeRow> {
        let mut rows = BTreeMap::new();
        let mut shortest_payload_ttl: Option<Duration> = None;
        for capability in [
            Capability::CausalGraph,
            Capability::EpisodicMemory,
            Capability::BeliefState,
        ] {
            let Some(ttl) = PolicyItem::all()
                .into_iter()
                .find(|item| item.capability() == Some(capability))
                .map(|item| item.time_to_live())
            else {
                // No payload slot feeds this row: no bound can be stated, so
                // no row is written and `new` refuses the table by name.
                continue;
            };
            shortest_payload_ttl = Some(shortest_payload_ttl.map_or(ttl, |held| held.min(ttl)));
            rows.insert(
                capability,
                EnvelopeRow {
                    bound: FreshnessBound::Age(ttl),
                    stale: cognitive(capability, Freshness::Stale),
                    unavailable: cognitive(capability, Freshness::Unavailable),
                },
            );
        }
        if let Some(ttl) = shortest_payload_ttl {
            rows.insert(
                Capability::Fabric,
                dependency(Capability::Fabric, FreshnessBound::DerivedFromPayload(ttl)),
            );
        }
        rows.insert(
            Capability::Ingestion,
            EnvelopeRow {
                bound: FreshnessBound::PerBook(config.max_staleness),
                // One, because the table does not size on it: the
                // `stale_book` gate refuses the stale book whole.
                stale: Decimal::ONE,
                unavailable: Decimal::ONE,
            },
        );
        rows.insert(
            Capability::Feed,
            dependency(Capability::Feed, FreshnessBound::Age(config.max_staleness)),
        );
        rows.insert(
            Capability::Clock,
            dependency(
                Capability::Clock,
                FreshnessBound::Lead(config.max_clock_skew),
            ),
        );
        if config.journal_wire {
            // The stale multiplier states ADR 0100 §6's half, which is the
            // contract's row too; a node may hand a narrower reading, and the
            // pass sizes by the reading itself (`Narrowing::new` refuses one
            // above one), so this is the declared default and never wider.
            rows.insert(
                Capability::Ledger,
                dependency(Capability::Ledger, FreshnessBound::SpoolReading),
            );
        }
        if let Some(policy) = config.venue_health {
            rows.insert(
                Capability::VenueSession,
                dependency(
                    Capability::VenueSession,
                    FreshnessBound::Quarantine(policy.quarantine_for),
                ),
            );
        }
        rows
    }

    /// The envelope of a cell assembled from `config`, or the reason it has
    /// none.
    pub fn for_cell(config: &CellConfig) -> Result<Self> {
        Self::new(&Self::dependencies_of(config), Self::rows_for(config))
    }

    /// Validate a table against the inputs a cell depends on.
    ///
    /// Refused, never repaired: a missing row is not given a default, a row
    /// for an input nobody reads is not dropped, and a multiplier out of
    /// range is not clamped. Each would let the table say something the cell
    /// does not do.
    pub fn new(
        required: &BTreeSet<Capability>,
        rows: BTreeMap<Capability, EnvelopeRow>,
    ) -> Result<Self> {
        for capability in required {
            if rows.contains_key(capability) {
                continue;
            }
            return Err(if *capability == Capability::Ingestion {
                Error::invalid(
                    "the degradation table has no row for ingestion; a cell judges ingestion per \
                     book and refuses a book older than max_staleness under the stale_book gate, \
                     which is stricter than any table multiplier — state that as a PerBook row \
                     rather than omitting it, so the choice is visible",
                )
            } else {
                Error::invalid(format!(
                    "the cell depends on {} and its degradation table has no row for it; state \
                     how the cell behaves as {} ages or disappears — a freshness bound and a \
                     stale and unavailable multiplier — rather than leaving its loss undecided",
                    capability.as_str(),
                    capability.as_str()
                ))
            });
        }
        for (capability, row) in &rows {
            if !required.contains(capability) {
                return Err(Error::invalid(format!(
                    "the degradation table has a row for {}, which this cell does not read; a \
                     row nothing exercises declares behaviour the cell does not have, so remove \
                     it or configure the reading it describes",
                    capability.as_str()
                )));
            }
            let per_book = matches!(row.bound, FreshnessBound::PerBook(_));
            if (*capability == Capability::Ingestion) != per_book {
                return Err(Error::invalid(format!(
                    "the degradation row for {} has a per-book bound only ingestion may have, or \
                     ingestion has another; ingestion is refused per book under the stale_book \
                     gate and every other row narrows the whole cell, so state each as what it is",
                    capability.as_str()
                )));
            }
            if let Some(duration) = row.bound.duration()
                && duration <= Duration::ZERO
            {
                return Err(Error::invalid(format!(
                    "the degradation row for {} has a freshness bound of {} ms, under which the \
                     input is never fresh; state a positive bound",
                    capability.as_str(),
                    duration.as_millis()
                )));
            }
            if row.stale > Decimal::ONE
                || row.unavailable.is_negative()
                || row.unavailable > row.stale
            {
                return Err(Error::invalid(format!(
                    "the degradation row for {} sizes a stale input at {} and a lost one at {}; \
                     a multiplier above one widens and a total loss narrowing less than a partial \
                     one rewards the worse case, so state 0 <= unavailable <= stale <= 1",
                    capability.as_str(),
                    row.stale,
                    row.unavailable
                )));
            }
        }
        Ok(Self { rows })
    }

    /// The row for one input, if the cell depends on it.
    pub fn row(&self, capability: Capability) -> Option<&EnvelopeRow> {
        self.rows.get(&capability)
    }

    /// Every row, in [`Capability`] order.
    pub fn rows(&self) -> &BTreeMap<Capability, EnvelopeRow> {
        &self.rows
    }

    /// What one row does to size, given what the cell observed of it.
    ///
    /// Unobserved and fresh are one: a row the cell has not measured scales
    /// nothing, for the reason [`Capability::dependencies`] gives. A row this
    /// table does not hold narrows to nothing — unreachable for a validated
    /// table over a cell's own dependencies, and zero rather than one so that
    /// a caller asking about an input the cell never declared cannot size at
    /// full on it.
    pub fn multiplier(&self, capability: Capability, observed: Option<Freshness>) -> Decimal {
        let Some(row) = self.rows.get(&capability) else {
            return Decimal::ZERO;
        };
        match observed {
            None | Some(Freshness::Fresh) => Decimal::ONE,
            Some(Freshness::Stale) => row.stale,
            Some(Freshness::Unavailable) => row.unavailable,
        }
    }
}

/// A cognitive row's multiplier, read from the contract's own sizing with
/// that row alone narrowed, so this table cannot drift from what sizes.
fn cognitive(capability: Capability, freshness: Freshness) -> Decimal {
    let mut state = DegradationState::fully_available();
    state.observe(capability, freshness);
    state.sizing_multiplier()
}

/// A dependency row, with both multipliers read from the contract's table.
fn dependency(capability: Capability, bound: FreshnessBound) -> EnvelopeRow {
    let at = |freshness: Freshness| {
        let mut state = DegradationState::fully_available();
        state.observe(capability, freshness);
        state.dependency_row_multiplier(capability)
    };
    EnvelopeRow {
        bound,
        stale: at(Freshness::Stale),
        unavailable: at(Freshness::Unavailable),
    }
}
