//! The implied volatility surface: construction, refusals, skew, term
//! structure, forward volatility and dispersion.
//!
//! The refusal tests carry most of the weight. A surface that quietly answers
//! a query it has no data for is worse than one that has no data, because the
//! caller cannot tell the difference — and the number goes straight into a
//! payoff valuation.

use qip_core::testing::approx_eq;
use qip_core::{Decimal, Duration, Timestamp, dec};
use qip_market::volatility::{VolPoint, VolatilitySurface, implied_correlation};

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 22)
}

fn point(expiry: &str, strike: &str, vol: f64) -> VolPoint {
    VolPoint::new(
        Decimal::parse(expiry).unwrap(),
        Decimal::parse(strike).unwrap(),
        vol,
        now(),
    )
}

/// A two-expiry equity-style surface with a downward skew at each expiry and a
/// mildly upward-sloping term structure. Forward is 100.
fn equity_surface() -> VolatilitySurface {
    VolatilitySurface::new(
        "obj-spx",
        now(),
        dec!("100"),
        vec![
            point("0.25", "80", 0.32),
            point("0.25", "90", 0.26),
            point("0.25", "100", 0.22),
            point("0.25", "110", 0.20),
            point("0.25", "120", 0.19),
            point("1.0", "80", 0.30),
            point("1.0", "90", 0.27),
            point("1.0", "100", 0.24),
            point("1.0", "110", 0.22),
            point("1.0", "120", 0.21),
        ],
    )
    .unwrap()
}

// --- construction and refusals ----------------------------------------------

#[test]
fn a_surface_groups_its_points_into_one_smile_per_expiry_in_expiry_order() {
    let surface = equity_surface();

    // Premise: the surface was built from ten points spanning two expiries,
    // so there is something to group.
    let expiries = surface.expiries();
    assert_eq!(
        expiries,
        vec![dec!("0.25"), dec!("1.0")],
        "expiries must come back ascending"
    );

    let front = surface.smile_at(dec!("0.25")).expect("the 3m smile exists");
    assert_eq!(front.strikes().len(), 5, "the 3m smile has five strikes");
    assert_eq!(front.strike_range(), (dec!("80"), dec!("120")));
    assert_eq!(surface.forward(), dec!("100"));
}

#[test]
fn a_strike_expiry_pair_observed_twice_is_refused_and_both_coordinates_are_named() {
    // A feed that publishes two implied volatilities for one grid node has a
    // defect. Picking one silently is how a surface comes to disagree with the
    // quotes it was built from.
    let result = VolatilitySurface::new(
        "obj-spx",
        now(),
        dec!("100"),
        vec![
            point("0.25", "100", 0.22),
            point("0.25", "110", 0.20),
            point("0.25", "100", 0.23),
        ],
    );

    let err = result.expect_err("a repeated strike-expiry pair must be refused");
    let message = err.message();
    assert!(
        message.contains("observed twice"),
        "the refusal must say what happened, got: {message}"
    );
    assert!(
        message.contains("100") && message.contains("0.25"),
        "the refusal must name the duplicated coordinates, got: {message}"
    );
}

#[test]
fn a_negative_implied_volatility_is_refused_rather_than_floored() {
    // Premise: the same points with a positive volatility build fine, so it is
    // the sign and nothing else that is refused.
    assert!(
        VolatilitySurface::new(
            "obj-spx",
            now(),
            dec!("100"),
            vec![point("0.25", "90", 0.26), point("0.25", "100", 0.22)],
        )
        .is_ok(),
        "the control surface must build"
    );

    let result = VolatilitySurface::new(
        "obj-spx",
        now(),
        dec!("100"),
        vec![point("0.25", "90", -0.26), point("0.25", "100", 0.22)],
    );
    let err = result.expect_err("a negative implied volatility must be refused");
    assert_eq!(err.code(), "numeric", "a bad number is a numeric refusal");
    assert!(
        err.message().contains("not a positive finite number"),
        "got: {}",
        err.message()
    );
}

#[test]
fn a_zero_implied_volatility_is_refused_because_it_prices_every_option_at_intrinsic() {
    let result = VolatilitySurface::new(
        "obj-spx",
        now(),
        dec!("100"),
        vec![point("0.25", "90", 0.0), point("0.25", "100", 0.22)],
    );
    assert!(
        result.is_err(),
        "a zero implied volatility must be refused, not admitted as a valid quote"
    );
}

#[test]
fn a_surface_with_no_points_and_a_non_positive_forward_are_both_refused() {
    let empty = VolatilitySurface::new("obj-spx", now(), dec!("100"), Vec::new());
    assert!(
        empty.is_err(),
        "an empty surface answers every query with an invention"
    );

    let no_forward = VolatilitySurface::new(
        "obj-spx",
        now(),
        Decimal::ZERO,
        vec![point("0.25", "100", 0.22)],
    );
    let err = no_forward.expect_err("a zero forward must be refused");
    assert!(
        err.message().contains("forward must be positive"),
        "got: {}",
        err.message()
    );
}

/// ADR 0050's requirement 3, and the entry it lists under "what would make
/// this wrong": *a `VolatilitySurface` constructed from points with more than
/// one true instant*.
///
/// The failure this prevents is invisible everywhere else. A chain assembled
/// from quotes taken a minute apart, at different underlying prices, is read
/// against the surface's single `forward`, interpolates perfectly and returns
/// a skew nobody quoted — so no downstream check can tell it from a
/// synchronous surface. Until the instant lived on the point, `as_of` was a
/// figure the type asserted about itself and nothing could verify.
///
/// The premise is asserted first: the same three points sharing one instant do
/// build a surface, so the refusal below is about the instant and not about
/// the points.
#[test]
fn a_surface_assembled_from_quotes_taken_at_two_instants_is_refused_naming_both() {
    let synchronous = vec![
        point("0.25", "90", 0.26),
        point("0.25", "100", 0.22),
        point("0.25", "110", 0.20),
    ];
    assert!(
        VolatilitySurface::new("obj-spx", now(), dec!("100"), synchronous.clone()).is_ok(),
        "premise: three points sharing one instant must build a surface, or the refusal \
         below would prove nothing about the instant"
    );

    let a_minute_later = now().saturating_add(Duration::from_secs(60));
    assert_ne!(
        a_minute_later,
        now(),
        "premise: the smeared point must actually be at a different instant"
    );
    let mut smeared = synchronous;
    smeared[2].observed_at = a_minute_later;

    let err = VolatilitySurface::new("obj-spx", now(), dec!("100"), smeared)
        .expect_err("a surface spanning two instants must be refused, not interpolated");
    let message = err.message();
    // Matched as the whole clause rather than as "110": a bare strike is a
    // substring of the timestamps the same message prints.
    assert!(
        message.contains("strike 110, expiry 0.25"),
        "the refusal must name the point that broke the synchrony; got: {message}"
    );
    assert!(
        message.contains(&format!("observed at {a_minute_later}")),
        "the refusal must name the instant the point was observed at; got: {message}"
    );
    assert!(
        message.contains(&format!("as of {}", now())),
        "the refusal must name the instant the surface claims; got: {message}"
    );
}

#[test]
fn a_non_positive_expiry_or_strike_is_refused_by_name() {
    let bad_expiry = VolatilitySurface::new(
        "obj-spx",
        now(),
        dec!("100"),
        vec![point("0", "100", 0.22), point("0.25", "100", 0.22)],
    );
    assert!(
        bad_expiry
            .expect_err("a zero expiry must be refused")
            .message()
            .contains("is not positive"),
        "the refusal must name the expiry"
    );

    let bad_strike = VolatilitySurface::new(
        "obj-spx",
        now(),
        dec!("100"),
        vec![point("0.25", "-100", 0.22)],
    );
    assert!(
        bad_strike.is_err(),
        "a negative strike must be refused, not taken as a moneyness"
    );
}

// --- reading the surface: the no-extrapolation rule --------------------------

#[test]
fn a_quoted_grid_node_reads_back_exactly_the_volatility_that_was_observed() {
    let surface = equity_surface();

    // Premise: 90 at 3m was quoted at 0.26. Interpolation must not move a
    // point that was actually observed.
    let quoted = surface.vol_at(dec!("0.25"), dec!("90")).unwrap();
    assert!(
        approx_eq(quoted, 0.26, 1e-12),
        "an observed node must read back exactly, got {quoted}"
    );
}

#[test]
fn a_strike_beyond_the_quoted_wings_is_refused_rather_than_flat_extrapolated() {
    let surface = equity_surface();

    // Premise: 120 is the highest quoted strike and reads fine.
    let edge = surface.vol_at(dec!("0.25"), dec!("120")).unwrap();
    assert!(approx_eq(edge, 0.19, 1e-12), "the wing itself must read");

    // 150 was never quoted. A flat extrapolation would return 0.19 — a number
    // indistinguishable downstream from one somebody observed.
    let err = surface
        .vol_at(dec!("0.25"), dec!("150"))
        .expect_err("a strike past the wing must be refused");
    let message = err.message();
    assert!(
        message.contains("does not extrapolate"),
        "the refusal must say the surface will not invent a wing, got: {message}"
    );
    assert!(
        message.contains("150"),
        "the refusal must name the strike asked for, got: {message}"
    );
}

#[test]
fn an_expiry_beyond_the_quoted_range_is_refused_rather_than_held_flat() {
    let surface = equity_surface();

    // Premise: 1.0 is the longest quoted expiry and reads fine.
    assert!(
        surface.vol_at(dec!("1.0"), dec!("100")).is_ok(),
        "the longest quoted expiry must read"
    );

    let err = surface
        .vol_at(dec!("2.0"), dec!("100"))
        .expect_err("a two-year read on a one-year surface must be refused");
    assert!(
        err.message().contains("does not extrapolate in time"),
        "got: {}",
        err.message()
    );
}

#[test]
fn a_strike_inside_one_bracketing_smile_but_outside_the_other_is_refused_as_a_hole() {
    // The front expiry is quoted with wide wings, the back expiry with narrow
    // ones. Reading 70 at six months would interpolate across a hole: the
    // one-year smile never quoted a 70 strike, so the answer would be half
    // observation and half invention.
    let surface = VolatilitySurface::new(
        "obj-spx",
        now(),
        dec!("100"),
        vec![
            point("0.25", "70", 0.36),
            point("0.25", "100", 0.22),
            point("0.25", "130", 0.20),
            point("1.0", "95", 0.25),
            point("1.0", "100", 0.24),
            point("1.0", "105", 0.235),
        ],
    )
    .unwrap();

    // Premise: 70 is quoted at the front expiry, so the hole is at the back.
    assert!(
        surface.vol_at(dec!("0.25"), dec!("70")).is_ok(),
        "70 must read at the front expiry, where it was quoted"
    );

    let err = surface
        .vol_at(dec!("0.5"), dec!("70"))
        .expect_err("a strike the back smile never quoted must be refused between expiries");
    assert!(
        err.message().contains("outside the quoted range"),
        "got: {}",
        err.message()
    );
}

#[test]
fn interpolating_between_expiries_is_linear_in_total_variance_not_in_volatility() {
    // Two flat smiles: 20% at one year, 30% at two years. Halfway in time the
    // total variance is (0.04*1 + 0.09*2)/2 = 0.11, so sigma = sqrt(0.11/1.5)
    // = 0.27080..., not the 0.25 a naive average of volatilities would give.
    let surface = VolatilitySurface::new(
        "obj-flat",
        now(),
        dec!("100"),
        vec![
            point("1.0", "90", 0.20),
            point("1.0", "100", 0.20),
            point("1.0", "110", 0.20),
            point("2.0", "90", 0.30),
            point("2.0", "100", 0.30),
            point("2.0", "110", 0.30),
        ],
    )
    .unwrap();

    // Premise: the two ends are what they were quoted as.
    assert!(approx_eq(
        surface.atm_vol(dec!("1.0")).unwrap(),
        0.20,
        1e-12
    ));
    assert!(approx_eq(
        surface.atm_vol(dec!("2.0")).unwrap(),
        0.30,
        1e-12
    ));

    let mid = surface.atm_vol(dec!("1.5")).unwrap();
    let expected = (0.11f64 / 1.5).sqrt();
    assert!(
        approx_eq(mid, expected, 1e-9),
        "variance interpolation must give {expected}, got {mid}"
    );
    assert!(
        !approx_eq(mid, 0.25, 1e-4),
        "a naive average of volatilities would give 0.25; that is the bug this guards"
    );
}

// --- skew, term structure, forward volatility --------------------------------

#[test]
fn an_equity_smile_with_a_bid_for_downside_reports_a_negative_skew() {
    let surface = equity_surface();
    let smile = surface.smile_at(dec!("0.25")).expect("the 3m smile exists");

    // Premise: the 80 strike is quoted above the 120 strike — this really is a
    // downward-sloping smile and not a flat one.
    assert!(
        smile.vols()[0] > smile.vols()[4],
        "premise: the downside wing must be bid over the upside wing"
    );

    let skew = smile.skew().unwrap();
    assert!(
        skew < 0.0,
        "a smile bid for downside must report a negative skew, got {skew}"
    );
}

#[test]
fn a_flat_smile_reports_a_skew_of_zero() {
    let surface = VolatilitySurface::new(
        "obj-flat",
        now(),
        dec!("100"),
        vec![
            point("1.0", "90", 0.20),
            point("1.0", "100", 0.20),
            point("1.0", "110", 0.20),
        ],
    )
    .unwrap();
    let smile = surface.smile_at(dec!("1.0")).expect("the smile exists");

    // Premise: every quote is the same, so any non-zero skew is manufactured.
    assert!(smile.vols().iter().all(|v| approx_eq(*v, 0.20, 1e-12)));
    assert!(approx_eq(smile.skew().unwrap(), 0.0, 1e-6));
}

#[test]
fn a_smile_whose_strikes_do_not_bracket_the_forward_refuses_to_report_a_skew() {
    // A surface quoted only in the upside wing has no at-the-money point. A
    // skew taken about a forward outside the data is a slope of an
    // extrapolation.
    let surface = VolatilitySurface::new(
        "obj-spx",
        now(),
        dec!("100"),
        vec![point("1.0", "120", 0.21), point("1.0", "130", 0.20)],
    )
    .unwrap();
    let smile = surface.smile_at(dec!("1.0")).expect("the smile exists");

    let err = smile
        .skew()
        .expect_err("a smile that does not bracket the forward has no skew to report");
    assert!(
        err.message().contains("no at-the-money point"),
        "got: {}",
        err.message()
    );
}

#[test]
fn an_upward_sloping_term_structure_is_not_reported_as_inverted() {
    let surface = equity_surface();

    // Premise: 3m ATM is 0.22 and 1y ATM is 0.24 — the structure really does
    // slope up.
    let front = surface.atm_vol(dec!("0.25")).unwrap();
    let back = surface.atm_vol(dec!("1.0")).unwrap();
    assert!(back > front, "premise: the term structure slopes upward");

    assert!(!surface.is_term_inverted().unwrap());
    let slope = surface.term_slope(dec!("0.25"), dec!("1.0")).unwrap();
    assert!(
        approx_eq(slope, 0.02, 1e-9),
        "the slope must be the difference of the ATM volatilities, got {slope}"
    );
}

#[test]
fn a_stressed_front_end_reports_an_inverted_term_structure() {
    // Front-end panic: 3m implied above 1y implied. This is the regime signal
    // the macro agent reads, and it must not be smoothed away.
    let surface = VolatilitySurface::new(
        "obj-spx",
        now(),
        dec!("100"),
        vec![
            point("0.25", "90", 0.60),
            point("0.25", "100", 0.55),
            point("0.25", "110", 0.52),
            point("1.0", "90", 0.32),
            point("1.0", "100", 0.30),
            point("1.0", "110", 0.29),
        ],
    )
    .unwrap();

    // Premise: the front really is above the back.
    assert!(surface.atm_vol(dec!("0.25")).unwrap() > surface.atm_vol(dec!("1.0")).unwrap());
    assert!(
        surface.is_term_inverted().unwrap(),
        "a front-end above the back-end is an inverted term structure"
    );
    assert!(surface.term_slope(dec!("0.25"), dec!("1.0")).unwrap() < 0.0);
}

#[test]
fn the_forward_volatility_between_two_expiries_comes_from_the_difference_of_total_variances() {
    // 20% to one year, 30% to two years: forward variance is
    // 0.09*2 - 0.04*1 = 0.14 over one year, so the forward vol is sqrt(0.14).
    let surface = VolatilitySurface::new(
        "obj-flat",
        now(),
        dec!("100"),
        vec![
            point("1.0", "90", 0.20),
            point("1.0", "100", 0.20),
            point("2.0", "90", 0.30),
            point("2.0", "100", 0.30),
        ],
    )
    .unwrap();

    // Premise: the two spot volatilities are the ones the arithmetic assumes.
    assert!(approx_eq(
        surface.atm_vol(dec!("1.0")).unwrap(),
        0.20,
        1e-12
    ));
    assert!(approx_eq(
        surface.atm_vol(dec!("2.0")).unwrap(),
        0.30,
        1e-12
    ));

    let forward = surface.forward_vol(dec!("1.0"), dec!("2.0")).unwrap();
    assert!(
        approx_eq(forward, 0.14f64.sqrt(), 1e-9),
        "expected sqrt(0.14) = {}, got {forward}",
        0.14f64.sqrt()
    );
    assert!(
        forward > 0.30,
        "an upward term structure implies a forward vol above the far spot vol"
    );
}

#[test]
fn a_calendar_arbitrage_in_the_quotes_refuses_to_yield_a_forward_volatility() {
    // 50% to one year and 20% to two years means total variance falls with
    // time: 0.04*2 = 0.08 against 0.25*1 = 0.25. There is no real number whose
    // square is the forward variance, and flooring it at zero would hand a
    // caller a forward volatility the market never offered.
    let surface = VolatilitySurface::new(
        "obj-broken",
        now(),
        dec!("100"),
        vec![
            point("1.0", "90", 0.50),
            point("1.0", "100", 0.50),
            point("2.0", "90", 0.20),
            point("2.0", "100", 0.20),
        ],
    )
    .unwrap();

    // Premise: the surface built — the refusal is about the calendar, not the
    // construction.
    assert!(approx_eq(
        surface.atm_vol(dec!("1.0")).unwrap(),
        0.50,
        1e-12
    ));

    let err = surface
        .forward_vol(dec!("1.0"), dec!("2.0"))
        .expect_err("a negative forward variance must be refused");
    assert_eq!(err.code(), "numeric");
    assert!(
        err.message().contains("negative forward variance"),
        "got: {}",
        err.message()
    );
    assert!(
        err.message().contains("calendar arbitrage"),
        "the refusal must say what the caller is looking at, got: {}",
        err.message()
    );
}

#[test]
fn a_forward_volatility_asked_for_backwards_is_refused_rather_than_swapped() {
    let surface = equity_surface();
    let err = surface
        .forward_vol(dec!("1.0"), dec!("0.25"))
        .expect_err("a far expiry below the near one must be refused");
    assert!(
        err.message().contains("swap the arguments"),
        "the refusal must say what to do instead, got: {}",
        err.message()
    );
}

// --- dispersion --------------------------------------------------------------

#[test]
fn an_index_quoted_below_its_components_implies_a_correlation_under_one() {
    // Diversification is exactly this: an index volatility below the weighted
    // component volatilities, and the gap is the implied correlation.
    let flat = |name: &str, vol: f64| {
        VolatilitySurface::new(
            name,
            now(),
            dec!("100"),
            vec![
                VolPoint::new(dec!("1.0"), dec!("90"), vol, now()),
                VolPoint::new(dec!("1.0"), dec!("100"), vol, now()),
                VolPoint::new(dec!("1.0"), dec!("110"), vol, now()),
            ],
        )
        .unwrap()
    };

    let index = flat("obj-index", 0.18);
    let a = flat("obj-a", 0.30);
    let b = flat("obj-b", 0.30);

    // Premise: the index really is quoted below both components, so there is
    // diversification to measure.
    assert!(index.atm_vol(dec!("1.0")).unwrap() < a.atm_vol(dec!("1.0")).unwrap());

    let rho = implied_correlation(&index, &[(0.5, &a), (0.5, &b)], dec!("1.0")).unwrap();
    // own = 2 * 0.25 * 0.09 = 0.045; cross = 2 * 0.25 * 0.09 = 0.045;
    // rho = (0.0324 - 0.045) / 0.045 = -0.28
    assert!(
        approx_eq(rho, -0.28, 1e-9),
        "expected -0.28, got {rho}; the dispersion arithmetic has moved"
    );
    assert!((-1.0..=1.0).contains(&rho));
}

#[test]
fn perfectly_correlated_components_imply_a_correlation_of_one() {
    let flat = |name: &str, vol: f64| {
        VolatilitySurface::new(
            name,
            now(),
            dec!("100"),
            vec![
                VolPoint::new(dec!("1.0"), dec!("100"), vol, now()),
                VolPoint::new(dec!("1.0"), dec!("110"), vol, now()),
            ],
        )
        .unwrap()
    };

    // Two identical 30% names at half weight each: an index that also trades
    // at 30% is one where nothing diversifies anything.
    let index = flat("obj-index", 0.30);
    let a = flat("obj-a", 0.30);
    let b = flat("obj-b", 0.30);

    let rho = implied_correlation(&index, &[(0.5, &a), (0.5, &b)], dec!("1.0")).unwrap();
    assert!(
        approx_eq(rho, 1.0, 1e-9),
        "identical names and an identical index imply rho = 1, got {rho}"
    );
}

#[test]
fn weights_that_do_not_sum_to_one_are_refused_rather_than_renormalised() {
    let flat = |name: &str, vol: f64| {
        VolatilitySurface::new(
            name,
            now(),
            dec!("100"),
            vec![
                VolPoint::new(dec!("1.0"), dec!("100"), vol, now()),
                VolPoint::new(dec!("1.0"), dec!("110"), vol, now()),
            ],
        )
        .unwrap()
    };
    let index = flat("obj-index", 0.20);
    let a = flat("obj-a", 0.30);
    let b = flat("obj-b", 0.30);

    // Premise: the same call with full weights is accepted, so it is the
    // weights and nothing else being refused.
    assert!(implied_correlation(&index, &[(0.5, &a), (0.5, &b)], dec!("1.0")).is_ok());

    let err = implied_correlation(&index, &[(0.3, &a), (0.3, &b)], dec!("1.0"))
        .expect_err("a partial index composition must be refused");
    assert!(
        err.message().contains("not 1.0"),
        "the refusal must name the shortfall, got: {}",
        err.message()
    );
}

#[test]
fn a_single_component_is_refused_because_a_one_name_index_has_no_correlation() {
    let flat = |name: &str, vol: f64| {
        VolatilitySurface::new(
            name,
            now(),
            dec!("100"),
            vec![
                VolPoint::new(dec!("1.0"), dec!("100"), vol, now()),
                VolPoint::new(dec!("1.0"), dec!("110"), vol, now()),
            ],
        )
        .unwrap()
    };
    let index = flat("obj-index", 0.20);
    let a = flat("obj-a", 0.20);

    let err = implied_correlation(&index, &[(1.0, &a)], dec!("1.0"))
        .expect_err("one component cannot imply a correlation");
    assert!(
        err.message().contains("at least two components"),
        "got: {}",
        err.message()
    );
}

#[test]
fn an_index_quoted_far_above_its_components_is_refused_rather_than_clamped_to_one() {
    // An index at 60% built from two 20% names implies a correlation far above
    // one. That is not a stressed market; it is proof the index and the
    // components were not read together. Clamping to 1.0 would hide it.
    let flat = |name: &str, vol: f64| {
        VolatilitySurface::new(
            name,
            now(),
            dec!("100"),
            vec![
                VolPoint::new(dec!("1.0"), dec!("100"), vol, now()),
                VolPoint::new(dec!("1.0"), dec!("110"), vol, now()),
            ],
        )
        .unwrap()
    };
    let index = flat("obj-index", 0.60);
    let a = flat("obj-a", 0.20);
    let b = flat("obj-b", 0.20);

    let err = implied_correlation(&index, &[(0.5, &a), (0.5, &b)], dec!("1.0"))
        .expect_err("an impossible correlation must be refused");
    assert_eq!(err.code(), "numeric");
    assert!(
        err.message().contains("outside [-1, 1]"),
        "got: {}",
        err.message()
    );
}

#[test]
fn a_surface_survives_a_round_trip_through_json_with_its_interpolation_intact() {
    // The surface is an event contract as well as a domain object. A surface
    // that deserialises without its interpolator would answer a between-strike
    // query differently on the far side of the wire.
    let surface = equity_surface();
    let before = surface.vol_at(dec!("0.25"), dec!("95")).unwrap();

    let encoded = serde_json::to_string(&surface).unwrap();
    let decoded: VolatilitySurface = serde_json::from_str(&encoded).unwrap();

    let after = decoded.vol_at(dec!("0.25"), dec!("95")).unwrap();
    assert!(
        approx_eq(before, after, 1e-12),
        "a round trip changed an interpolated read: {before} then {after}"
    );
    // And the refusals must survive too.
    assert!(decoded.vol_at(dec!("0.25"), dec!("150")).is_err());
}
