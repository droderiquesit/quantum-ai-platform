//! What a venue says about itself, and what this process has seen it do.
//!
//! Blueprint §34.4's observed rung compares a venue's declarations against
//! measurement taken independently of them. Until this module existed the
//! comparison had no way to happen: [`crate::broker::Broker`] carried a name,
//! a simulation flag, availability, capabilities, submit and cancel, and not
//! one fact about how long the venue took to answer or which order types it
//! actually accepted when one was sent. Those facts exist in `qip-brokers` —
//! `Heartbeat::round_trip`, `OrderAck::latency`, `VenueOrderState::Rejected`,
//! the exchange's own fee tally — and reached nothing above the adapter,
//! because there was no port for them to cross.
//!
//! # The one rule this module is shaped by
//!
//! **A venue that does not report a fact must be distinguishable from one
//! reporting zero.** An unmeasured venue is not a fast venue, a venue that
//! itemises no fees is not a free venue, and a venue nobody sent a `twap` to
//! has not been shown to accept one. So every fact an adapter may be unable
//! to supply is an [`Option`] with no default, [`Broker::observation`] itself
//! defaults to [`None`] rather than to a zeroed record, and
//! [`ObservedVenueFacts::fee_bps`] refuses to divide by a notional of zero
//! instead of answering "free".
//!
//! [`Broker::observation`]: crate::broker::Broker::observation
//!
//! The cost of getting this wrong is not abstract. A promotion ladder fed a
//! defaulted zero would read "declared 50 ms, measured 0 ms" and promote the
//! venue for answering faster than it claimed, on the strength of a number
//! nobody took. That is worse than an empty ladder, because an empty ladder
//! is visibly empty.
//!
//! # What this module is not
//!
//! It is not a judgement. Nothing here decides whether a venue may be used;
//! it carries facts across a port. `qip-lifecycle`'s venue ladder judges, and
//! its ceiling is the simulator.

use qip_contracts::venue::{VenueClass, VenueId};
use qip_core::{Decimal, Duration};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Basis points in a whole fraction. A rate of one is ten thousand of them.
const BASIS_POINTS_PER_UNIT: i64 = 10_000;

/// What a venue states about itself, in the terms the observed rung compares
/// against measurement.
///
/// Every field is a *claim*. The venue's own documentation, its capability
/// message, or the settings a simulator was built with — never a measurement,
/// and kept in a separate type from one so that the two cannot be confused at
/// a call site.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredVenueProfile {
    /// What kind of venue this is — §34.4's registered rung is "venue type
    /// identified", and the adapter is the only layer that knows.
    ///
    /// Not a convenience. The promotion ladder's simulated rung applies the
    /// §34.3 decentralised-venue checks on the strength of this one field, so
    /// a venue whose class arrived as somebody's guess would be judged by the
    /// wrong gate. It is a declaration rather than a measurement and it is
    /// filed with the other declarations for that reason.
    pub class: VenueClass,
    /// The taker fee the venue states, in basis points.
    pub fee_bps: Decimal,
    /// The acknowledgement latency the venue states.
    pub acknowledgement_latency: Duration,
    /// The order types the venue states it accepts.
    pub order_types: BTreeSet<String>,
}

/// What this process has actually seen a venue do.
///
/// Read the module header before adding a field. Every fact a venue might not
/// be able to report is an [`Option`], and the reason is in the one rule
/// there: a missing fact and a zero are different findings and only one of
/// them is evidence.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedVenueFacts {
    /// Instructions the venue answered, fills and refusals alike.
    ///
    /// Not an [`Option`]: a venue that has answered nothing has answered
    /// zero, which is a count somebody took rather than a fact nobody has.
    pub acknowledgements: usize,
    /// The mean round trip across those acknowledgements.
    ///
    /// [`None`] from a venue that does not time them — which includes every
    /// venue that applies a configured latency rather than measuring one,
    /// because a setting read back is not an observation of anything. That
    /// distinction is the reason this field is an option rather than a
    /// duration defaulting to zero.
    pub acknowledgement_latency: Option<Duration>,
    /// Fees the venue actually charged. [`None`] from a venue that does not
    /// itemise them.
    pub fees_charged: Option<Decimal>,
    /// The notional those fees were charged on. [`None`] likewise, and kept
    /// beside the fees rather than folded into a rate because a rate is a
    /// quotient and the denominator is the half that can be zero.
    pub notional_filled: Option<Decimal>,
    /// Order types the venue accepted when one was sent.
    pub accepted_order_types: BTreeSet<String>,
    /// Order types the venue refused *for being that type*.
    ///
    /// Deliberately not every refusal. A venue that refuses a small seeded
    /// fraction of orders for no stated reason has said nothing about the
    /// type it happened to refuse, and recording it here would make the
    /// ladder conclude the venue does not support `market` because one market
    /// order lost a coin flip. Only a refusal attributable to the order's
    /// kind belongs in this set.
    pub rejected_order_types: BTreeSet<String>,
}

impl ObservedVenueFacts {
    /// The fee actually charged, in basis points.
    ///
    /// [`None`] in three cases, all of which mean the same thing to a caller:
    /// the venue itemises no fees, the venue reports no notional, or nothing
    /// has filled. The last is the one worth naming — a rate is a quotient,
    /// and a quotient over a notional of zero is not zero basis points, it is
    /// no answer. Returning zero there would present a venue that has never
    /// traded as the cheapest one available.
    pub fn fee_bps(&self) -> Option<Decimal> {
        let fees = self.fees_charged?;
        let notional = self.notional_filled?;
        if notional.is_zero() {
            return None;
        }
        fees.checked_div(notional)?
            .checked_mul(Decimal::from_int(BASIS_POINTS_PER_UNIT))
    }

    /// Order types that were declared and never tried.
    ///
    /// Separate from the rejected set because a capability nobody exercised
    /// and a capability the venue refused are different findings, and only
    /// the second is the venue misdescribing itself. Both stop a promotion;
    /// they stop it for different reasons and an operator acts on them
    /// differently.
    pub fn untried_order_types(&self, declared: &BTreeSet<String>) -> BTreeSet<String> {
        declared
            .iter()
            .filter(|kind| {
                !self.accepted_order_types.contains(*kind)
                    && !self.rejected_order_types.contains(*kind)
            })
            .cloned()
            .collect()
    }
}

/// One venue's declaration and its measurement, as the adapter holds them.
///
/// The two halves travel together because they are only useful together, and
/// they stay separate types because §34.4's whole argument is that the first
/// is not evidence for the second.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueObservation {
    /// The venue this is about.
    ///
    /// A [`VenueId`] rather than the bare string [`crate::broker::Broker::name`]
    /// returns, because the ladder that reads this keys on one and a string
    /// converted at the last moment is a string somebody converts twice.
    pub venue: VenueId,
    pub declared: DeclaredVenueProfile,
    pub observed: ObservedVenueFacts,
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn facts() -> ObservedVenueFacts {
        ObservedVenueFacts::default()
    }

    #[test]
    fn a_venue_that_has_filled_no_notional_reports_no_fee_rate_rather_than_a_free_one() {
        // The failure this prevents: a venue that has never traded reading as
        // the cheapest venue available, and clearing a fee gate on the
        // strength of a division nobody could perform.
        let observed = ObservedVenueFacts {
            fees_charged: Some(Decimal::ZERO),
            notional_filled: Some(Decimal::ZERO),
            ..facts()
        };
        assert_eq!(observed.fee_bps(), None);
        // The premise: the same type does answer once there is a denominator,
        // so the `None` above is about the notional and not about the type
        // being inert.
        let traded = ObservedVenueFacts {
            fees_charged: Some(dec!("10")),
            notional_filled: Some(dec!("100000")),
            ..facts()
        };
        assert_eq!(traded.fee_bps(), Some(dec!("1")));
    }

    #[test]
    fn a_venue_that_itemises_no_fees_is_not_reported_as_charging_nothing() {
        // `None` and `Some(ZERO)` are different claims: the first is a venue
        // that keeps no tally, the second is a venue that charged nothing on
        // something it did fill.
        let silent = ObservedVenueFacts {
            fees_charged: None,
            notional_filled: Some(dec!("100000")),
            ..facts()
        };
        assert_eq!(silent.fee_bps(), None);
        let free = ObservedVenueFacts {
            fees_charged: Some(Decimal::ZERO),
            notional_filled: Some(dec!("100000")),
            ..facts()
        };
        assert_eq!(free.fee_bps(), Some(Decimal::ZERO));
    }

    #[test]
    fn a_declared_order_type_nobody_sent_is_untried_rather_than_accepted() {
        let declared: BTreeSet<String> = ["limit", "market", "twap"]
            .iter()
            .map(|k| (*k).to_string())
            .collect();
        let observed = ObservedVenueFacts {
            accepted_order_types: ["limit".to_string()].into_iter().collect(),
            rejected_order_types: ["market".to_string()].into_iter().collect(),
            ..facts()
        };
        let untried = observed.untried_order_types(&declared);
        assert_eq!(
            untried,
            ["twap".to_string()].into_iter().collect::<BTreeSet<_>>(),
            "a type that was accepted or refused was counted as never tried, or one that was \
             never sent was counted as settled"
        );
    }
}
