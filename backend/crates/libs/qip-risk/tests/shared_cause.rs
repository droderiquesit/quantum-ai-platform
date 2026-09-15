//! Blueprint §25.3's three levels no instrument record carries.
//!
//! Every test here is about one distinction: a control that ran and found
//! nothing, a control that could not run, and a control that was never wired.
//! The platform has already shipped the confusion between the first and the
//! third twice — the liquidity floor that abstained on a refused ladder, and
//! the tail limits that looked up a key nothing wrote — and both times the
//! venue saw the same silence either way.

use qip_core::Decimal;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk::shared_cause::{
    CAUSAL_DRIVER_AXIS, FACTOR_AXIS, FAMILY_AXIS, SHARED_CAUSE_AXES, SharedCauseExposure,
    divides_an_overlapping_axis, measures_an_overlapping_axis,
};
use std::collections::BTreeMap;

/// A book of named positions against a stated equity.
fn book(equity: i64, positions: &[(&str, i64)]) -> RiskState {
    RiskState {
        equity: Decimal::from_int(equity),
        cash: Decimal::from_int(equity),
        gross_exposure: Decimal::from_int(positions.iter().map(|(_, n)| n.abs()).sum()),
        position_notionals: positions
            .iter()
            .map(|(name, notional)| ((*name).to_string(), Decimal::from_int(*notional)))
            .collect(),
        ..RiskState::default()
    }
}

#[test]
fn a_level_that_ran_and_found_nothing_is_present_and_empty_rather_than_absent() {
    // The distinction this whole type exists for. `LimitKind::MaxAxisWeight`
    // takes its early-return arm on an absent axis and records nothing, and a
    // level that ran over a book sharing no named cause records nothing
    // either — so at the venue "the control is not wired" and "the control
    // looked and the book is clean" arrive as the same approval. Only the
    // state can tell them apart, and only if the producer says which.
    let mut exposure = SharedCauseExposure::new();
    exposure
        .declare(CAUSAL_DRIVER_AXIS)
        .expect("a producer may declare a level it owns");

    let state = exposure.apply(book(1_000_000, &[("AAA", 100_000)]));

    // The premise: the book is not empty, so an empty bucket map below is a
    // statement about the drivers and not about the positions.
    assert_eq!(
        state.position_notionals.len(),
        1,
        "the book under test holds no positions, so nothing below measures anything"
    );

    let declared = state
        .axis_exposures
        .get(CAUSAL_DRIVER_AXIS)
        .expect("a declared level is present in the state even when it charged nothing");
    assert!(
        declared.is_empty(),
        "a level that attributed nothing carries {} bucket(s)",
        declared.len()
    );

    // And the level nobody declared is absent, which is the other half of the
    // same claim. Without this the assertion above would pass on an
    // implementation that inserted every axis unconditionally, and the four
    // states would collapse back into three.
    assert!(
        !state.axis_exposures.contains_key(FACTOR_AXIS),
        "a level no producer declared is present in the state, so an unwired level now \
         reads exactly like an idle one"
    );
    assert!(
        state.unevaluated.is_empty(),
        "a level that ran and found nothing filed a refusal: {:?}",
        state.unevaluated
    );
}

#[test]
fn a_level_whose_source_could_not_be_read_refuses_rather_than_reading_as_clean() {
    // The fail-closed direction. A producer that cannot read its source must
    // not leave an empty axis behind, because an empty axis is a *measured*
    // statement that the book shares no named cause. `RiskState::unevaluated`
    // is what `PreTradeChecker::check` turns into a refusal of every order.
    let mut exposure = SharedCauseExposure::new();
    exposure
        .refuse(CAUSAL_DRIVER_AXIS, "the causal graph could not be read")
        .expect("a producer may refuse a level it owns");

    let state = exposure.apply(book(1_000_000, &[("AAA", 100_000)]));

    assert!(
        !state.axis_exposures.contains_key(CAUSAL_DRIVER_AXIS),
        "a refused level left buckets behind, so a limit would fire on a number nobody \
         computed"
    );
    let refusal = state
        .unevaluated
        .get(CAUSAL_DRIVER_AXIS)
        .expect("a refused level files the figure it could not compute");
    assert!(
        refusal.contains("causal graph"),
        "the refusal does not carry the producer's own reason: {refusal}"
    );
}

#[test]
fn a_refusal_discovered_halfway_through_a_walk_supersedes_what_was_already_charged() {
    // A producer that seats three drivers and then finds its source
    // unreadable has a bucket built from *some* of the evidence, and that
    // bucket reads downstream as a measurement of all of it. Keeping the
    // partial charge beside the refusal would be the worse of the two
    // outcomes: the refusal blocks every order anyway, so the only thing the
    // leftover bucket could do is be read.
    let mut exposure = SharedCauseExposure::new();
    exposure
        .attribute(CAUSAL_DRIVER_AXIS, "rates", "AAA", 1.0)
        .expect("a positive weight on a named driver is attributed");
    // The premise: something really was seated, so the assertion below is
    // about the refusal clearing it and not about it never existing.
    assert_eq!(
        exposure.subjects(CAUSAL_DRIVER_AXIS).len(),
        1,
        "nothing was seated, so this test proves nothing about superseding"
    );

    exposure
        .refuse(CAUSAL_DRIVER_AXIS, "the graph went unreadable mid-walk")
        .expect("a producer may refuse a level it has partly charged");

    // The seating is gone, not merely outranked. `apply` reads the refusal
    // first, so a leftover attribution would never reach a bucket — which is
    // exactly why this has to be asserted on the producer rather than on the
    // state: a mutation deleting the clearing step passed against the state
    // assertion alone, and left `read_axes` reporting a level as read that
    // the producer had just said it could not read.
    assert!(
        exposure.subjects(CAUSAL_DRIVER_AXIS).is_empty(),
        "the refused level still names the instruments it had charged"
    );
    assert!(
        !exposure.read_axes().contains(&CAUSAL_DRIVER_AXIS),
        "a refused level is still reported as read: {:?}",
        exposure.read_axes()
    );

    let state = exposure.apply(book(1_000_000, &[("AAA", 900_000)]));
    assert!(
        !state.axis_exposures.contains_key(CAUSAL_DRIVER_AXIS),
        "the half-charged bucket survived the refusal"
    );
    assert!(
        state.unevaluated.contains_key(CAUSAL_DRIVER_AXIS),
        "the refusal did not reach the state"
    );

    // The reverse order is a contradiction rather than a correction, and is
    // refused: a producer that has said it cannot read the level must not
    // then charge it.
    assert!(
        exposure
            .attribute(CAUSAL_DRIVER_AXIS, "rates", "AAA", 1.0)
            .is_err(),
        "a refused level accepted an attribution, so the state would carry a refusal and a \
         bucket claiming the opposite"
    );
}

#[test]
fn a_long_and_a_short_that_share_one_driver_do_not_net_each_other_away() {
    // The diversification illusion §25.3 calls the concentration that ends
    // firms, written into the control meant to catch it. Two positions that
    // move on the same mechanism are two exposures to that mechanism, whatever
    // their signs say about the instruments. Netting here would report a book
    // with no shared-cause exposure at all — which is precisely the reading
    // every other control in the platform already gives it.
    let mut exposure = SharedCauseExposure::new();
    for instrument in ["AAA", "BBB"] {
        exposure
            .attribute(CAUSAL_DRIVER_AXIS, "rates", instrument, 1.0)
            .expect("a positive weight on a named driver is attributed");
    }

    let state = exposure.apply(book(1_000_000, &[("AAA", 400_000), ("BBB", -400_000)]));

    // The premise: the book really is signed in opposite directions, so a
    // netting implementation would have something to cancel.
    assert!(
        state.position_notionals["BBB"].is_negative(),
        "both positions are the same sign, so netting would have nothing to cancel and this \
         test would pass against an implementation that nets"
    );

    let charged = state.axis_exposures[CAUSAL_DRIVER_AXIS]["rates"];
    assert_eq!(
        charged,
        Decimal::from_int(800_000),
        "the driver's bucket is not the gross of the two positions it drives"
    );
}

#[test]
fn a_level_another_producer_already_charged_is_refused_rather_than_overwritten() {
    // A window with two writers is the `MaxExpectedShortfall` failure in a
    // new place: a control that reads as protection while being unable to say
    // whose number it fired on. Merging would be worse than overwriting,
    // because the sum of two producers' views of one level is a number
    // neither of them computed.
    let mut exposure = SharedCauseExposure::new();
    exposure
        .attribute(CAUSAL_DRIVER_AXIS, "rates", "AAA", 1.0)
        .expect("a positive weight on a named driver is attributed");

    let mut state = book(1_000_000, &[("AAA", 900_000)]);
    state.axis_exposures.insert(
        CAUSAL_DRIVER_AXIS.to_string(),
        BTreeMap::from([("credit".to_string(), Decimal::from_int(100_000))]),
    );
    let state = exposure.apply(state);

    assert_eq!(
        state.axis_exposures[CAUSAL_DRIVER_AXIS],
        BTreeMap::from([("credit".to_string(), Decimal::from_int(100_000))]),
        "the first producer's buckets were replaced or merged into"
    );
    assert!(
        state.unevaluated.contains_key(CAUSAL_DRIVER_AXIS),
        "a level with two writers was charged silently rather than refused"
    );
}

#[test]
fn a_weight_that_rounds_to_nothing_once_carried_as_money_is_refused_rather_than_dropped() {
    // Security review LOW-2 in a new place. A weight that reads positive as an
    // `f64` and rounds to zero at the nine decimal places a notional carries
    // would silently drop the position out of the bucket while the producer
    // believed it had charged it — a bucket short of one position is a bucket
    // that reads as a measurement of the book.
    let mut exposure = SharedCauseExposure::new();
    let refusal = exposure
        .attribute(CAUSAL_DRIVER_AXIS, "rates", "AAA", 1e-12)
        .expect_err("a weight that rounds to nothing was admitted");
    assert!(
        refusal.message().contains("rounds to"),
        "the refusal does not name the crossing that caused it: {}",
        refusal.message()
    );

    // And the ordinary refusals beside it, each of which would otherwise
    // reach the bucket as a number nobody could compare.
    for weight in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            exposure
                .attribute(CAUSAL_DRIVER_AXIS, "rates", "AAA", weight)
                .is_err(),
            "a weight of {weight} was admitted onto a level a veto reads"
        );
    }
}

#[test]
fn a_producer_may_not_write_an_axis_the_reference_data_vouches_for() {
    // The structural half of the two-writers rule. `sector`, `country`,
    // `asset_class`, `venue` and `counterparty` are maintained by
    // `RiskAggregates::apply_fill` as running per-bucket counters; a second
    // writer reaching into one of them would replace a counter the sector cap
    // reads with a figure derived from something else entirely.
    let mut exposure = SharedCauseExposure::new();
    for axis in ["sector", "country", "asset_class", "venue", "counterparty"] {
        assert!(
            exposure.declare(axis).is_err(),
            "{axis} was accepted as a shared-cause level"
        );
        assert!(
            exposure.attribute(axis, "rates", "AAA", 1.0).is_err(),
            "{axis} accepted an attribution from a shared-cause producer"
        );
        assert!(
            exposure.refuse(axis, "because").is_err(),
            "{axis} accepted a refusal from a shared-cause producer"
        );
    }
}

#[test]
fn the_shipped_set_caps_the_two_levels_that_have_a_producer_and_not_the_third() {
    // A limit over a level nothing fills is the `MaxExpectedShortfall` defect
    // being re-added under a new name, so the shipped set carries a cap for
    // exactly the levels `qip_kernel::shared_cause` feeds. Per family is not
    // one of them: the desk charges every fill it books to one budget holder,
    // so a family bucket would hold the whole book or none of it.
    let shipped = LimitSet::conservative_default();
    let axes: Vec<&str> = shipped
        .limits
        .iter()
        .filter_map(|limit| match &limit.kind {
            LimitKind::MaxAxisWeight { axis, .. } => Some(axis.as_str()),
            _ => None,
        })
        .collect();

    // The premise: the set really does carry axis-weight limits, so the
    // filter below is not answering over an empty list.
    assert!(
        axes.len() >= 2,
        "the shipped set carries no axis-weight limits at all, so this test measures nothing"
    );
    for level in [CAUSAL_DRIVER_AXIS, FACTOR_AXIS] {
        assert!(
            axes.contains(&level),
            "the shipped set caps no {level} exposure, so the level is measured and never \
             enforced; the axes it names are {axes:?}"
        );
    }
    assert!(
        !axes.contains(&FAMILY_AXIS),
        "the shipped set caps family exposure, and nothing fills that axis; a limit over a \
         level with no producer is the defect this lane exists to close"
    );
}

#[test]
fn the_shipped_set_never_divides_an_overlapping_level_by_its_own_sum() {
    // `MaxConcentration` divides one bucket by the sum of the buckets on its
    // axis. Sector and country partition the book so that sum is the book's
    // gross; a causal driver does not, because one instrument sits downstream
    // of several. Over an overlapping axis the denominator is a number nobody
    // computed, and the resulting share would *fall* as the book became more
    // concentrated in a second shared cause.
    let shipped = LimitSet::conservative_default();

    // The premise: `divides_an_overlapping_axis` can return true at all, or
    // the loop below would pass against a predicate that is constantly false.
    let would_be_wrong = Limit::new(
        "driver-share",
        LimitKind::MaxConcentration {
            axis: CAUSAL_DRIVER_AXIS.into(),
            limit: 0.5,
        },
    );
    assert!(
        divides_an_overlapping_axis(&would_be_wrong.kind),
        "the predicate does not recognise the shape it exists to refuse"
    );
    assert!(
        !measures_an_overlapping_axis(&would_be_wrong.kind),
        "a share-of-sum limit was reported as measuring an overlapping level against equity"
    );

    for limit in &shipped.limits {
        assert!(
            !divides_an_overlapping_axis(&limit.kind),
            "the shipped set carries {} over an overlapping level, whose bucket sum is a \
             number nobody computed",
            limit.name
        );
    }
}

#[test]
fn every_shared_cause_level_is_named_once_and_spelled_one_way() {
    // The `COUNTERPARTY_AXIS` lesson: an axis spelled two ways is a limit that
    // cannot fire, and this cap has already been one. The array is what bounds
    // the `unevaluated` keys these levels file under, so a duplicate entry
    // would also make `apply` charge a level twice.
    let mut seen: Vec<&str> = SHARED_CAUSE_AXES.to_vec();
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(
        seen.len(),
        before,
        "a shared-cause level is listed twice: {SHARED_CAUSE_AXES:?}"
    );
    assert_eq!(
        before, 3,
        "§25.3 names three levels the instrument record cannot carry, and the array holds \
         {before}"
    );
}
