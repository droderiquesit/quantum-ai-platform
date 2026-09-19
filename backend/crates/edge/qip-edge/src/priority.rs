//! Which quote update gets the message when there are not enough left
//! (§29.2).
//!
//! §29.2's second control: *"Under constraint, instruments are repriced in
//! order of expected value of the update."* Everything else in that section
//! decides **whether** a message may be sent — the per-venue token bucket,
//! the requote threshold, the widening the bucket's depletion imposes. None
//! of them decides **which** message goes first, and until this module the
//! answer was the order the cell happened to hold its open orders in, which
//! is the order their ids sort in. A session with budget for one requote
//! spent it on whichever instrument was alphabetically first.
//!
//! # What an update is worth
//!
//! [`worth`] is the quantity still working times how far behind the touch the
//! order rests: the money the update recovers if the replacement fills where
//! the original no longer can. **It is a stated proxy and not a calibrated
//! expectation**, in the same sense as `RateLimits::depletion`'s band
//! multiples, and it is written down as such so that a later measurement can
//! replace it without anybody having to work out whether it was evidence.
//! What it is not is a probability: nothing here models whether the
//! replacement fills, because the cell holds no fill model and a number
//! invented for one would rank every order by a fiction.
//!
//! Money, so [`Decimal`] throughout. The drift the repricer reasons in —
//! `Drift::ticks_f64` and `Drift::bps_f64` — is explicitly a statistic and
//! deliberately not used here: a ranking in ticks says a one-tick move on a
//! thousand lots matters less than a two-tick move on one, and the whole
//! point of ranking by value is that it does not.
//!
//! # The ranking cannot refuse anything
//!
//! [`allocate`] returns every candidate it was given, reordered. It never
//! drops one, and an update whose worth cannot be stated is placed last
//! rather than removed, so every gate downstream is still asked about it and
//! a bug here can cost an order its turn but never its consideration. That
//! separation is the reason the ranking is safe to compute from a cheaper
//! approximation than the repricer's own: the repricer still decides, and
//! this only decides who it is asked about first.
//!
//! The order is total. Ties break on the order id, ascending, so two updates
//! worth the same are allocated in the same sequence on a replay — and a
//! replay that reordered them would spend the same budget on different
//! instruments.

use qip_core::Decimal;

/// What repricing one resting order recovers, or `None` when that cannot be
/// stated.
///
/// `remaining` is the quantity still working at the venue and `behind_by` the
/// price distance the touch has moved past the price it rests at, signed so
/// that positive means behind. Both are the caller's, because the side
/// convention — a buy rests on the bid and falls behind when the bid rises —
/// belongs to the repricer that owns it, and a second copy of it here would
/// be a second place for it to be wrong.
///
/// Zero for an order with nothing left to recover: no quantity working, or a
/// price at or ahead of the touch. `None` only when the product cannot be
/// represented, which [`allocate`] places last rather than treating as
/// worthless — an unrepresentable product is an enormous one, and ranking it
/// as zero would be the one arithmetic failure that also hides itself.
pub fn worth(remaining: Decimal, behind_by: Decimal) -> Option<Decimal> {
    if !remaining.is_positive() || !behind_by.is_positive() {
        return Some(Decimal::ZERO);
    }
    remaining.checked_mul(behind_by)
}

/// One pending update, and what sending it is worth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    order_id: String,
    worth: Option<Decimal>,
}

impl Candidate {
    /// A candidate for `order_id` worth `worth`.
    pub fn new(order_id: impl Into<String>, worth: Option<Decimal>) -> Self {
        Self {
            order_id: order_id.into(),
            worth,
        }
    }

    pub fn order_id(&self) -> &str {
        &self.order_id
    }

    /// What the update recovers, or `None` when that could not be stated.
    pub const fn worth(&self) -> Option<Decimal> {
        self.worth
    }
}

/// The candidates in the order the messages that are left should go to them.
///
/// Richest first. An update whose worth could not be stated goes last, and
/// ties — including the very common tie at zero, every order that is not
/// behind the touch at all — break on the order id so the sequence replays.
pub fn allocate(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates.sort_by(|left, right| {
        match (left.worth, right.worth) {
            // Descending: the larger worth sorts first.
            (Some(left_worth), Some(right_worth)) => right_worth.cmp(&left_worth),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| left.order_id.cmp(&right.order_id))
    });
    candidates
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn d(value: &str) -> Decimal {
        Decimal::parse(value).expect("a literal decimal in a test")
    }

    fn ids(ranked: &[Candidate]) -> Vec<&str> {
        ranked.iter().map(Candidate::order_id).collect()
    }

    #[test]
    fn a_small_drift_on_a_large_order_outranks_a_large_drift_on_a_small_one() {
        // The whole reason the ranking is in money rather than in the ticks
        // the repricer already computes. In ticks the second order wins by
        // ten to one, and repricing it recovers a hundredth of what
        // repricing the first does.
        let large = Candidate::new("a-large-order", worth(d("1000"), d("0.01")));
        let small = Candidate::new("b-small-order", worth(d("1"), d("0.10")));
        assert_eq!(
            large.worth(),
            Some(d("10")),
            "the premise failed: the large order's update is not worth ten"
        );
        assert_eq!(
            small.worth(),
            Some(d("0.10")),
            "the premise failed: the small order's update is not worth a tenth"
        );
        assert_eq!(
            ids(&allocate(vec![small, large])),
            vec!["a-large-order", "b-small-order"],
            "the cheaper update was allocated the message first"
        );
    }

    #[test]
    fn the_ranking_returns_every_candidate_it_was_given_and_refuses_none() {
        // Load-bearing: the budget decides what is sent, and this decides
        // only what is asked about first. A ranking that dropped a candidate
        // would be a second gate nobody wrote a refusal for.
        let candidates = vec![
            Candidate::new("c", worth(d("1"), d("1"))),
            Candidate::new("a", worth(Decimal::MAX, Decimal::MAX)),
            Candidate::new("b", worth(d("-5"), d("1"))),
        ];
        assert_eq!(
            candidates[1].worth(),
            None,
            "the premise failed: the overflowing candidate stated a worth"
        );
        let ranked = allocate(candidates);
        assert_eq!(ranked.len(), 3, "the ranking dropped a candidate");
        assert_eq!(
            ids(&ranked),
            vec!["c", "b", "a"],
            "an update whose worth could not be stated was not placed last"
        );
    }

    #[test]
    fn two_updates_worth_the_same_are_allocated_in_the_same_sequence_every_time() {
        // A replay that reordered equal candidates would spend the same
        // budget on different instruments and produce a different set of
        // orders from the same inputs.
        let build = || {
            vec![
                Candidate::new("zulu", worth(d("2"), d("3"))),
                Candidate::new("alpha", worth(d("3"), d("2"))),
                Candidate::new("mike", worth(d("6"), d("1"))),
            ]
        };
        let once = allocate(build());
        assert_eq!(
            once.iter().map(Candidate::worth).collect::<Vec<_>>(),
            vec![Some(d("6")), Some(d("6")), Some(d("6"))],
            "the premise failed: the three updates are not worth the same"
        );
        assert_eq!(
            ids(&once),
            vec!["alpha", "mike", "zulu"],
            "equal updates were not ordered by the one tie-break that replays"
        );
        assert_eq!(
            ids(&allocate(build())),
            ids(&once),
            "the same candidates ranked twice produced two different sequences"
        );
    }

    #[test]
    fn an_order_at_or_ahead_of_the_touch_is_worth_nothing_rather_than_a_negative() {
        // A negative worth would sort an order that is *ahead* of the touch
        // below one that is merely level with it, which is a ranking by how
        // well an order is doing rather than by what an update recovers.
        assert_eq!(
            worth(d("100"), d("-0.05")),
            Some(Decimal::ZERO),
            "an order ahead of the touch was given a negative worth"
        );
        assert_eq!(
            worth(d("100"), Decimal::ZERO),
            Some(Decimal::ZERO),
            "an order level with the touch was not worth nothing"
        );
        assert_eq!(
            worth(Decimal::ZERO, d("5")),
            Some(Decimal::ZERO),
            "an order with nothing working was worth something"
        );
    }
}
