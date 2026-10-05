//! Alignment of asynchronous streams without artificial simultaneity
//! (blueprint TICK-042).
//!
//! Two venues, two instruments or two regions never share a clock. A lead/lag
//! estimate, a cross-venue feature or a causal reading built by sorting their
//! events on a timestamp column asserts an order the timestamps cannot carry:
//! a quote stamped 40 microseconds before a trade, each on a clock known to
//! 100, did not demonstrably come first. The sort says it did, and the model
//! trained on the sort learns a lead that is clock error.
//!
//! So an order is stated here only when the stamps are further apart than
//! both uncertainties together, and everything else is [`Order::Unordered`].
//! There is deliberately no "simultaneous" answer. Simultaneity is the one
//! claim two independent clocks can never support, and a variant for it would
//! be the variant every tie fell into.
//!
//! Two different orderings are kept apart, because they answer different
//! questions and only one of them is uncertain:
//!
//! * **Event time** is when each thing happened, on a reference clock, and is
//!   known only to within the uncertainty the clock discipline published.
//! * **Arrival** is when this cell learned of each, on its own clock. Both
//!   arrivals are read from one clock, so their order is exact. It is what a
//!   decision could have known, and it is not evidence of which happened
//!   first: a far venue's earlier event routinely arrives after a near
//!   venue's later one.
//!
//! Nothing here reads a clock.

use crate::clock::{CorrectionKind, DisciplinedTime};
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// One event placed on the reference timeline, with how well.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placed {
    event_time: Timestamp,
    uncertainty: Duration,
    arrival: Timestamp,
}

impl Placed {
    /// An event at `event_time` give or take `uncertainty`, learned of at
    /// `arrival`.
    ///
    /// The true instant is taken to lie within `event_time ± uncertainty`.
    /// A negative uncertainty is refused rather than read as zero: it is a
    /// sign error upstream, and treating it as "exact" would let the event be
    /// ordered against everything.
    pub fn new(event_time: Timestamp, uncertainty: Duration, arrival: Timestamp) -> Result<Self> {
        if uncertainty < Duration::ZERO {
            return Err(Error::invalid(format!(
                "a clock uncertainty of {uncertainty:?} is negative; state the half-width of \
                 the interval the true instant lies in, which is zero or more"
            )));
        }
        Ok(Self {
            event_time,
            uncertainty,
            arrival,
        })
    }

    /// Place a timestamp the clock discipline normalized, carrying the
    /// uncertainty it published through.
    ///
    /// Refused when no offset estimate was applied. A venue time that was
    /// never corrected is on the venue's clock, not the reference one, by an
    /// amount nobody measured; its uncertainty is unknown rather than zero,
    /// and an aligner that took it would order it exactly. State the
    /// uncertainty with [`Placed::new`] if it is known some other way.
    pub fn from_disciplined(time: &DisciplinedTime) -> Result<Self> {
        let estimate = time
            .corrections
            .iter()
            .find(|correction| correction.kind == CorrectionKind::OffsetEstimate)
            .ok_or_else(|| {
                Error::invalid(
                    "this timestamp carries no offset-estimate correction, so how far the \
                     venue's clock is from the reference clock was never measured; it cannot \
                     be aligned against another stream. Discipline the feed until the \
                     estimate is trustworthy, or state the uncertainty with Placed::new",
                )
            })?;
        // The published figure is a statistic in nanoseconds. Rounded up at
        // the crossing into integer time, because rounding an uncertainty
        // down is rounding towards an order that may not be supported.
        let nanos = estimate.uncertainty_nanos_f64.ceil();
        if !nanos.is_finite() || nanos < 0.0 {
            return Err(Error::numeric(format!(
                "the clock estimate published an uncertainty of {} ns; it must be finite and \
                 not negative before a timestamp corrected by it can be aligned",
                estimate.uncertainty_nanos_f64
            )));
        }
        // The monotonic floor moved the stamp forward to keep time from
        // running backwards. That is a statement about ordering within one
        // feed, not a measurement, so the whole of the move is error as far
        // as another stream is concerned and is added rather than dropped.
        let floored = time
            .corrections
            .iter()
            .filter(|correction| correction.kind == CorrectionKind::MonotonicFloor)
            .fold(Duration::ZERO, |sum, correction| {
                sum + correction.delta.abs()
            });
        Self::new(
            time.normalized,
            Duration::from_nanos(nanos as i64) + floored,
            time.capture_time,
        )
    }

    pub fn event_time(&self) -> Timestamp {
        self.event_time
    }

    pub fn uncertainty(&self) -> Duration {
        self.uncertainty
    }

    pub fn arrival(&self) -> Timestamp {
        self.arrival
    }
}

/// What the timestamps support about which of two events happened first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    /// The first happened before the second, by more than both clocks' error.
    Before,
    /// The first happened after the second, by more than both clocks' error.
    After,
    /// The stamps are within the combined uncertainty. Not "at the same
    /// time": unknown.
    Unordered,
}

/// How two events relate, with the uncertainty carried through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alignment {
    /// The event-time order the stamps support.
    pub order: Order,
    /// Second minus first, as stamped. Signed.
    pub lag: Duration,
    /// The true lag lies within `lag ± lag_uncertainty`: both uncertainties,
    /// summed.
    pub lag_uncertainty: Duration,
    /// Whether this cell had learned of the first by the time it learned of
    /// the second. Exact, because both arrivals are on the cell's own clock,
    /// and independent of [`Self::order`].
    pub first_known_when_second_arrived: bool,
}

/// Relate two events from streams that do not share a clock.
pub fn align(first: &Placed, second: &Placed) -> Alignment {
    let lag = second.event_time.since(first.event_time);
    let lag_uncertainty = first.uncertainty + second.uncertainty;
    // Strictly more than the combined uncertainty. At exactly the bound the
    // two intervals touch, and an order that holds only if both clocks erred
    // by their full width in opposite directions is not one to assert.
    let order = if lag.abs() <= lag_uncertainty {
        Order::Unordered
    } else if lag > Duration::ZERO {
        Order::Before
    } else {
        Order::After
    };
    Alignment {
        order,
        lag,
        lag_uncertainty,
        first_known_when_second_arrived: first.arrival <= second.arrival,
    }
}

/// What of another stream a target event can be related to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsOf {
    /// Index of the latest event in the stream that demonstrably happened
    /// before the target *and* had arrived by the time the target did.
    pub latest_prior: Option<usize>,
    /// Indices of the events whose order against the target is unknown.
    /// A feature that needs "the state of the other venue at this instant"
    /// has this many candidates for it, and choosing one is a guess.
    pub unordered: Vec<usize>,
}

/// The as-of join of one event against another stream, by event time and
/// information arrival, never by a tie on a timestamp.
///
/// `stream` is taken in any order; nothing is assumed sorted, because a stream
/// sorted by its own stamps is not sorted by true time either.
pub fn as_of(target: &Placed, stream: &[Placed]) -> AsOf {
    let mut latest_prior: Option<usize> = None;
    let mut unordered = Vec::new();
    for (index, candidate) in stream.iter().enumerate() {
        let alignment = align(candidate, target);
        match alignment.order {
            Order::Unordered => unordered.push(index),
            Order::Before if alignment.first_known_when_second_arrived => {
                let later = latest_prior
                    .and_then(|held| stream.get(held))
                    .is_none_or(|held| candidate.event_time > held.event_time);
                if later {
                    latest_prior = Some(index);
                }
            }
            Order::Before | Order::After => {}
        }
    }
    AsOf {
        latest_prior,
        unordered,
    }
}
