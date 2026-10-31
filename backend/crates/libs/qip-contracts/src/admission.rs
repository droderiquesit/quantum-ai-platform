//! One admission header for every sensed record (ADR 0132 decision 1,
//! R12-TICK-2053).
//!
//! A venue's order-book message and a central bank's macro release are
//! different facts, and a single payload type spanning both would be one
//! nobody can match exhaustively. What is one is how a record is *admitted*:
//! the three times (when it happened, when this platform received it, the
//! disciplined instant it is placed at), the clock's doubt about them, the
//! hash of the bytes it was built from and the entitlement it is used under.
//! [`Admission`] is that header, and its one constructor holds the refusals
//! that used to live in `MarketEvent::new` alone, so a point-in-time rule
//! lands once rather than in every place a record is built — which is how
//! `capture_time` had to land in several places on 2026-10-11.
//!
//! # The refusals
//!
//! - A negative uncertainty is not a tighter bound; it is a subtraction that
//!   ran the wrong way.
//! - An entitlement not granted at the event's time: a record may not be
//!   built from data this platform is not licensed to use.
//! - **An event received before it happened**, by more than the clock's
//!   doubt. A record whose event time follows its receive time beyond the
//!   uncertainty was stamped by a clock nobody trusts or fabricated after
//!   the fact, and a feature computed from it is knowable before it was
//!   true: the leak every bitemporal rule here exists to prevent. Refused,
//!   never reconciled. Inside the uncertainty the two instants are the same
//!   instant as far as the clocks can tell, and the record is admitted.
//! - A record whose own times disagree with its header ([`Admitted::admit`]):
//!   a tick whose `at` is not the header's event time, or whose capture time
//!   is not the header's receive time, carries two accounts of one fact, and
//!   the header is the authority. Refused rather than reconciled.
//!
//! # What this slice does not yet do
//!
//! `MarketEvent` keeps its own fields and wire form — the lake and the spool
//! hold millions of lines of it — and admits through [`Admission::new`]
//! inside its constructor rather than becoming `Admitted<MarketMessage>` on
//! the wire. Every `SensedRecord` producer does not yet build a header; the
//! kernel admits each record by the times it carries itself, with no
//! entitlement and no source hash (see `Platform::absorb_records`). Both are
//! named in R12-TICK-2053's gap.

use crate::governance::Entitlement;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// The header every admitted record carries. See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Admission {
    pub(crate) event_time: Timestamp,
    pub(crate) receive_time: Timestamp,
    pub(crate) normalized_time: Timestamp,
    pub(crate) uncertainty: Duration,
    pub(crate) source_hash: String,
    pub(crate) entitlement: Entitlement,
}

impl Admission {
    /// Admit a record by its three times, the clock's doubt, the hash of
    /// its source bytes and its entitlement. The refusals are the module
    /// documentation's.
    pub fn new(
        event_time: Timestamp,
        receive_time: Timestamp,
        normalized_time: Timestamp,
        uncertainty: Duration,
        source_hash: impl Into<String>,
        entitlement: Entitlement,
    ) -> Result<Self> {
        if uncertainty.as_nanos() < 0 {
            return Err(Error::invalid(
                "market event uncertainty must not be negative; a negative value is not a \
                 tighter bound, it is a subtraction that ran the wrong way",
            ));
        }
        if !entitlement.is_granted(event_time) {
            return Err(Error::denied(format!(
                "{} carries no active entitlement at this event's time; a market event may not \
                 be built from data this platform is not licensed to use",
                entitlement.dataset()
            )));
        }
        Self::check_order(event_time, receive_time, uncertainty)?;
        let source_hash = source_hash.into();
        if source_hash.trim().is_empty() {
            return Err(Error::invalid(
                "an admission names the hash of the bytes the record was built from; an empty \
                 hash is a record nobody can trace to its source",
            ));
        }
        Ok(Self {
            event_time,
            receive_time,
            normalized_time,
            uncertainty,
            source_hash,
            entitlement,
        })
    }

    /// The one point-in-time rule, on its own so a record that carries no
    /// header yet can still be held to it: `event_time` may not follow
    /// `receive_time` by more than `uncertainty`.
    pub fn check_order(
        event_time: Timestamp,
        receive_time: Timestamp,
        uncertainty: Duration,
    ) -> Result<()> {
        if uncertainty.as_nanos() < 0 {
            return Err(Error::invalid(
                "market event uncertainty must not be negative; a negative value is not a \
                 tighter bound, it is a subtraction that ran the wrong way",
            ));
        }
        let lead = event_time
            .as_nanos()
            .saturating_sub(receive_time.as_nanos());
        if lead > uncertainty.as_nanos() {
            return Err(Error::invalid(format!(
                "the event at {event_time} was received at {receive_time}, {lead} ns before it \
                 happened and beyond the {} ns the clocks are doubted by; a record received \
                 before it happened is refused, not reconciled",
                uncertainty.as_nanos()
            )));
        }
        Ok(())
    }

    pub fn event_time(&self) -> Timestamp {
        self.event_time
    }

    pub fn receive_time(&self) -> Timestamp {
        self.receive_time
    }

    pub fn normalized_time(&self) -> Timestamp {
        self.normalized_time
    }

    pub fn uncertainty(&self) -> Duration {
        self.uncertainty
    }

    pub fn source_hash(&self) -> &str {
        &self.source_hash
    }

    pub fn entitlement(&self) -> &Entitlement {
        &self.entitlement
    }
}

/// The times a record states about itself, for [`Admitted::admit`] to hold
/// against its header. `None` is a record that does not state that instant,
/// not one that states it unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnTimes {
    pub event_time: Option<Timestamp>,
    pub receive_time: Option<Timestamp>,
}

/// A record that can be admitted under an [`Admission`].
pub trait Admissible {
    /// The instants the record carries itself.
    fn own_times(&self) -> OwnTimes;
}

impl Admissible for crate::message::MarketMessage {
    fn own_times(&self) -> OwnTimes {
        OwnTimes {
            event_time: Some(self.venue_time),
            receive_time: Some(self.capture_time),
        }
    }
}

/// A record under its admission header.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Admitted<T> {
    admission: Admission,
    record: T,
}

impl<T: Admissible> Admitted<T> {
    /// Admit `record` under `admission`, refusing a record whose own times
    /// disagree with the header's.
    pub fn admit(record: T, admission: Admission) -> Result<Self> {
        let own = record.own_times();
        if let Some(event_time) = own.event_time
            && event_time != admission.event_time
        {
            return Err(Error::invalid(format!(
                "the record says it happened at {event_time} and its admission says \
                 {}; two accounts of one instant are refused, not reconciled",
                admission.event_time
            )));
        }
        if let Some(receive_time) = own.receive_time
            && receive_time != admission.receive_time
        {
            return Err(Error::invalid(format!(
                "the record says it was received at {receive_time} and its admission says \
                 {}; two accounts of one instant are refused, not reconciled",
                admission.receive_time
            )));
        }
        Ok(Self { admission, record })
    }
}

impl<T> Admitted<T> {
    pub fn admission(&self) -> &Admission {
        &self.admission
    }

    pub fn record(&self) -> &T {
        &self.record
    }

    pub fn into_record(self) -> T {
        self.record
    }
}
