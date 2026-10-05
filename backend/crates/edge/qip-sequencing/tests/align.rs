//! Alignment without artificial simultaneity (blueprint TICK-042): an order
//! between two streams' events is reported only where the timestamps and
//! their published uncertainty support it.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::Property;
use qip_core::{Duration, Timestamp};
use qip_sequencing::{
    ClockDiscipline, ClockObservation, CorrectionKind, Order, Placed, align, as_of,
};

const BASE: i64 = 1_704_207_845_000_000_000;

fn at(nanos: i64) -> Timestamp {
    Timestamp::from_nanos(BASE + nanos)
}

fn placed(event: i64, uncertainty: i64, arrival: i64) -> Placed {
    Placed::new(at(event), Duration::from_nanos(uncertainty), at(arrival))
        .expect("a non-negative uncertainty")
}

/// Uniform in `[-bound, bound]`.
fn within(rng: &mut Xoshiro256, bound: i64) -> i64 {
    rng.below(2 * bound as u64 + 1) as i64 - bound
}

/// Two events on two clocks: when each truly happened, how far each clock may
/// be off, and how far each actually was.
#[derive(Debug)]
struct Pair {
    true_first: i64,
    true_second: i64,
    uncertainty_first: i64,
    uncertainty_second: i64,
    error_first: i64,
    error_second: i64,
}

impl Pair {
    fn generate(rng: &mut Xoshiro256) -> Self {
        let uncertainty_first = rng.below(301) as i64;
        let uncertainty_second = rng.below(301) as i64;
        let true_first = within(rng, 5_000);
        Self {
            true_first,
            // Separations from well inside the combined uncertainty to well
            // outside it, so both halves of the property are exercised.
            true_second: true_first + within(rng, 2_000),
            uncertainty_first,
            uncertainty_second,
            error_first: within(rng, uncertainty_first),
            error_second: within(rng, uncertainty_second),
        }
    }

    fn stamped(&self) -> (Placed, Placed) {
        (
            placed(
                self.true_first + self.error_first,
                self.uncertainty_first,
                0,
            ),
            placed(
                self.true_second + self.error_second,
                self.uncertainty_second,
                0,
            ),
        )
    }

    fn combined(&self) -> i64 {
        self.uncertainty_first + self.uncertainty_second
    }
}

/// The requirement's own check, over generated pairs of events whose clocks
/// each err by up to their stated uncertainty.
///
/// The failure it prevents is the sort. Ordering two venues' events by their
/// timestamp column asserts an order for every pair, and for the pairs inside
/// the clocks' error it is wrong about half the time; a lead/lag model fitted
/// to that learns clock error and calls it a lead. So the property has three
/// parts: inside the combined uncertainty the answer is `Unordered`; outside
/// it the answer is the *true* order, never merely the stamped one; and a pair
/// truly separated by more than twice the uncertainty is always ordered, so
/// "unordered" is not an answer the aligner can retreat to for everything.
#[test]
fn an_order_is_reported_only_beyond_the_combined_uncertainty_and_is_then_the_true_order() {
    let mut unordered = 0usize;
    let mut ordered = 0usize;
    let mut stamps_that_lie = 0usize;

    Property::new("alignment never asserts an unsupported order")
        .cases(4_000)
        .for_all(Pair::generate, |pair| {
            let (first, second) = pair.stamped();
            let alignment = align(&first, &second);
            let stamped_lag =
                (pair.true_second + pair.error_second) - (pair.true_first + pair.error_first);
            let true_lag = pair.true_second - pair.true_first;

            if alignment.lag != Duration::from_nanos(stamped_lag) {
                return Err(format!("lag {:?} is not the stamped lag", alignment.lag));
            }
            if alignment.lag_uncertainty != Duration::from_nanos(pair.combined()) {
                return Err(format!(
                    "the uncertainty carried through is {:?}, not both clocks' {}ns",
                    alignment.lag_uncertainty,
                    pair.combined()
                ));
            }

            // A pair the timestamp sort would get backwards.
            let stamp_lies = true_lag != 0 && (stamped_lag > 0) != (true_lag > 0);
            if stamp_lies {
                stamps_that_lie += 1;
            }

            if stamped_lag.abs() <= pair.combined() {
                unordered += 1;
                if alignment.order != Order::Unordered {
                    return Err(format!(
                        "stamps {stamped_lag}ns apart on clocks good to {}ns were ordered {:?}",
                        pair.combined(),
                        alignment.order
                    ));
                }
            } else {
                ordered += 1;
                let truth = if true_lag > 0 {
                    Order::Before
                } else {
                    Order::After
                };
                if true_lag == 0 || alignment.order != truth {
                    return Err(format!(
                        "reported {:?} for a pair whose true lag is {true_lag}ns",
                        alignment.order
                    ));
                }
            }
            if stamp_lies && alignment.order != Order::Unordered {
                return Err(format!(
                    "the stamps order this pair backwards and the aligner reported {:?}",
                    alignment.order
                ));
            }
            if true_lag.abs() > 2 * pair.combined() && alignment.order == Order::Unordered {
                return Err(format!(
                    "a pair truly {true_lag}ns apart on clocks good to {}ns was left unordered",
                    pair.combined()
                ));
            }
            Ok(())
        });

    // Premise: the generator reached both halves, and reached the case the
    // property exists for. Without a single pair whose stamps lie, "never
    // reports a wrong order" was never put to the question.
    assert!(
        unordered > 200,
        "only {unordered} pairs fell inside the uncertainty"
    );
    assert!(
        ordered > 200,
        "only {ordered} pairs fell outside the uncertainty"
    );
    assert!(
        stamps_that_lie > 20,
        "only {stamps_that_lie} pairs had stamps in the wrong order; the sort was not tested"
    );
}

/// The enum has no variant for "at the same time", and this match is what
/// keeps it so: a fourth variant stops this file compiling.
fn describe(order: Order) -> &'static str {
    match order {
        Order::Before => "before",
        Order::After => "after",
        Order::Unordered => "unordered",
    }
}

#[test]
fn identical_stamps_are_unordered_rather_than_simultaneous_and_the_bound_itself_is_not_an_order() {
    // Two exact clocks, one instant. Nothing supports an order, and there is
    // no simultaneity to assert instead.
    let same = align(&placed(1_000, 0, 0), &placed(1_000, 0, 0));
    assert_eq!(describe(same.order), "unordered");

    // Exactly at the combined uncertainty the intervals touch: unordered.
    // One nanosecond past it, in each direction: ordered.
    let first = placed(0, 60, 0);
    assert_eq!(align(&first, &placed(100, 40, 0)).order, Order::Unordered);
    assert_eq!(align(&first, &placed(101, 40, 0)).order, Order::Before);
    assert_eq!(align(&first, &placed(-100, 40, 0)).order, Order::Unordered);
    assert_eq!(align(&first, &placed(-101, 40, 0)).order, Order::After);
    // Premise for the line above: without the second clock's 40ns the first
    // clock's own 60ns would have ordered a 100ns gap.
    assert_eq!(align(&first, &placed(100, 0, 0)).order, Order::Before);
}

/// A far venue's earlier event routinely arrives after a near venue's later
/// one. Reading arrival as event order is the other way to manufacture a
/// lead that is only a cable length.
#[test]
fn arrival_order_is_reported_exactly_and_never_stands_in_for_event_order() {
    let far_and_earlier = placed(100, 5, 900);
    let near_and_later = placed(400, 5, 450);
    let alignment = align(&far_and_earlier, &near_and_later);
    assert_eq!(
        alignment.order,
        Order::Before,
        "the far event happened first"
    );
    assert!(
        !alignment.first_known_when_second_arrived,
        "and this cell had not heard of it when the near one arrived"
    );
    // The same two events, had the far one arrived in time.
    let in_time = align(&placed(100, 5, 300), &near_and_later);
    assert_eq!(in_time.order, Order::Before);
    assert!(in_time.first_known_when_second_arrived);
}

#[test]
fn the_as_of_join_takes_the_latest_demonstrably_prior_event_that_had_arrived_and_lists_what_it_cannot_order()
 {
    let target = placed(1_000, 10, 2_000);
    // Deliberately not in time order: a stream sorted by its own stamps is not
    // sorted by true time, so the join may not rely on it.
    let stream = [
        placed(800, 10, 1_500),   // 0: prior and arrived, the latest such
        placed(1_015, 10, 1_600), // 1: within 20ns of the target: unordered
        placed(300, 10, 400),     // 2: prior and arrived, but older
        placed(900, 10, 2_500),   // 3: prior, later than 0, not yet arrived
        placed(1_500, 10, 1_700), // 4: after
        placed(985, 10, 1_800),   // 5: within 20ns on the other side: unordered
    ];
    let joined = as_of(&target, &stream);
    assert_eq!(joined.latest_prior, Some(0));
    assert_eq!(joined.unordered, vec![1, 5]);

    // Premise: event 3 is what a join on event time alone would have taken,
    // so its exclusion is the arrival rule and not an accident of the data.
    assert_eq!(align(&stream[3], &target).order, Order::Before);
    assert!(stream[3].event_time() > stream[0].event_time());

    assert_eq!(as_of(&target, &[]).latest_prior, None);
}

fn disciplined(samples: usize) -> ClockDiscipline {
    let mut discipline =
        ClockDiscipline::new(64, 8, Duration::from_micros(500)).expect("a valid window");
    let mut rng = Xoshiro256::seeded(11);
    for index in 0..samples {
        let venue = at(index as i64 * 1_000_000);
        let queuing = Duration::from_nanos(rng.below(20_000) as i64);
        discipline.observe(ClockObservation::new(
            venue,
            venue.saturating_add(Duration::from_micros(150) + queuing),
        ));
    }
    discipline
}

/// The uncertainty an alignment carries has to come from somewhere. A venue
/// time no estimate was applied to is off by an amount nobody measured, and
/// placing it with zero uncertainty would order it exactly against everything.
#[test]
fn a_timestamp_no_estimate_corrected_is_refused_and_a_corrected_one_carries_its_published_uncertainty()
 {
    // Too few samples: the estimate is not trusted and nothing is corrected.
    let uncorrected = disciplined(3).discipline_traced(at(70_000_000), at(70_200_000));
    assert!(uncorrected.corrections.is_empty(), "premise: no correction");
    let refused = Placed::from_disciplined(&uncorrected).expect_err("must be refused");
    assert!(
        refused.message().contains("never measured"),
        "the refusal says why: {refused}"
    );
    // The floor alone is a correction and not a measurement: a time that was
    // only floored is still on a clock nobody estimated.
    let mut untrusted = disciplined(3);
    untrusted.discipline_traced(at(70_000_000), at(70_200_000));
    let only_floored = untrusted.discipline_traced(at(60_000_000), at(70_300_000));
    assert_eq!(
        only_floored
            .corrections
            .iter()
            .map(|correction| correction.kind)
            .collect::<Vec<_>>(),
        vec![CorrectionKind::MonotonicFloor],
        "premise: floored and nothing else"
    );
    assert!(Placed::from_disciplined(&only_floored).is_err());

    let mut discipline = disciplined(64);
    let published = discipline
        .estimate()
        .expect("observations were made")
        .uncertainty_nanos_f64;
    assert!(published > 0.0, "premise: a real uncertainty was published");
    let corrected = discipline.discipline_traced(at(70_000_000), at(70_200_000));
    let placed = Placed::from_disciplined(&corrected).expect("a trusted estimate places");
    assert_eq!(placed.event_time(), corrected.normalized);
    assert_eq!(placed.arrival(), at(70_200_000));
    assert_eq!(
        placed.uncertainty(),
        Duration::from_nanos(published.ceil() as i64)
    );

    // A stamp the monotonic floor moved was moved by a rule, not a
    // measurement: the whole move is added to what is carried.
    let floored = discipline.discipline_traced(at(60_000_000), at(70_300_000));
    let floor = floored
        .corrections
        .iter()
        .find(|correction| correction.kind == CorrectionKind::MonotonicFloor)
        .expect("premise: an earlier venue time was floored");
    assert!(floor.delta > Duration::ZERO, "premise: the floor moved it");
    assert_eq!(
        Placed::from_disciplined(&floored)
            .expect("a floored time still places")
            .uncertainty(),
        Duration::from_nanos(published.ceil() as i64) + floor.delta
    );

    assert!(
        Placed::new(at(0), Duration::from_nanos(-1), at(0)).is_err(),
        "a negative uncertainty is a sign error, not an exact clock"
    );
}
