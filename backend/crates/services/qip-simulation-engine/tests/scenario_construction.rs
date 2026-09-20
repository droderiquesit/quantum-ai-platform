//! Blueprint §23.7's third and fourth methods, as constructions.
//!
//! The library is applied through a beta, so it can only reach a position
//! the factor model measured, and only along the correlations the tape
//! shows. `causal_scenario` reaches a position because a path in the causal
//! graph reaches it, and `StressTester::adversarial_sequence` is built from
//! the positions rather than from history. Each test asserts the premise the
//! property rests on before the property itself, because a construction that
//! returned nothing would pass an assertion about what it did not contain.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::Decimal;
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_simulation_engine::scenario::{
    ADVERSARIAL_SCENARIO_NAME, CAUSAL_SCENARIO_PREFIX, DriverShockSizing, FactorExposure,
    PropagatedShock, STANDARD_DRIVER_SHOCK, StressTester, causal_exposures, causal_scenario,
    standard_library,
};

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn exposure(object_id: &str, notional: i64, betas: &[(&str, f64)]) -> FactorExposure {
    FactorExposure {
        object_id: object_id.to_string(),
        notional: Decimal::from_int(notional),
        betas: betas
            .iter()
            .map(|(factor, beta)| ((*factor).to_string(), *beta))
            .collect(),
    }
}

// --- causal propagation ------------------------------------------------------

#[test]
fn a_driver_shock_reaches_a_held_position_through_the_path_and_not_through_a_beta() -> Result<()> {
    // The failure this prevents: a position the factor model cannot measure
    // — no beta at all — is stressed by nothing in the library, and reads as
    // immune. Here `shipping-co` carries no beta and sits two mechanisms
    // downstream of an oil shock; the path is what reaches it.
    let book = vec![
        exposure("airline", 1_000_000, &[("equity", 1.2)]),
        exposure("shipping-co", 500_000, &[]),
    ];
    let propagated = vec![
        PropagatedShock {
            target: "jet-fuel".to_string(),
            magnitude: 0.08,
            order: 1,
            over_days: 1.0,
        },
        PropagatedShock {
            target: "shipping-co".to_string(),
            magnitude: -0.05,
            order: 2,
            over_days: 3.0,
        },
    ];
    let scenario = causal_scenario(
        "oil",
        STANDARD_DRIVER_SHOCK,
        DriverShockSizing::StandardDriverShock,
        &propagated,
        &book,
    )?
    .expect("a walk that reaches a held position is a scenario");

    // Premise: the scenario is named for its driver and shocks exactly the
    // held target, not the intermediate node and not the origin nobody holds.
    assert_eq!(scenario.name, format!("{CAUSAL_SCENARIO_PREFIX}oil"));
    assert_eq!(
        scenario.shocks.len(),
        1,
        "only the held target is a shock: {:?}",
        scenario.shocks
    );
    assert_eq!(scenario.shocks[0].factor, "shipping-co");
    assert!(
        scenario.description.contains("standard_driver_shock"),
        "the sizing is recorded on the scenario: {}",
        scenario.description
    );

    // Property: applied to the book on its own nodes, the unmeasured position
    // loses exactly notional times the propagated move, and the position no
    // path reached is listed rather than credited with zero.
    let result =
        StressTester::new(0.0).apply(&scenario, &causal_exposures(&book), 10_000_000.0, at())?;
    let hit = result
        .positions
        .iter()
        .find(|impact| impact.object_id == "shipping-co")
        .expect("the reached position carries an impact");
    assert!(
        (hit.profit_and_loss - (-25_000.0)).abs() < 1e-9,
        "500,000 at a propagated -5% is a 25,000 loss, not {}",
        hit.profit_and_loss
    );
    assert_eq!(
        result.unmodelled,
        vec!["airline".to_string()],
        "a position no path reached is named, not silently zero"
    );
    Ok(())
}

#[test]
fn a_driver_whose_walk_reaches_no_held_position_is_not_a_scenario() -> Result<()> {
    let book = vec![exposure("airline", 1_000_000, &[("equity", 1.2)])];
    let elsewhere = vec![PropagatedShock {
        target: "jet-fuel".to_string(),
        magnitude: 0.08,
        order: 1,
        over_days: 1.0,
    }];
    // Premise: the walk did reach something — just nothing the book holds.
    assert!(!elsewhere.is_empty());
    let none = causal_scenario(
        "oil",
        STANDARD_DRIVER_SHOCK,
        DriverShockSizing::StandardDriverShock,
        &elsewhere,
        &book,
    )?;
    assert!(
        none.is_none(),
        "a driver the book does not sit downstream of must not become a scenario shocking \
         nothing: {none:?}"
    );

    // And the origin itself is a target when it is held: the driver moves by
    // the initial shock before anything downstream does.
    let holding_the_driver = vec![exposure("oil", 200_000, &[])];
    let own = causal_scenario(
        "oil",
        -0.10,
        DriverShockSizing::ObservedWorstPeriod,
        &elsewhere,
        &holding_the_driver,
    )?
    .expect("a held driver is reached at order zero");
    assert_eq!(own.shocks.len(), 1);
    assert_eq!(own.shocks[0].factor, "oil");
    assert!((own.shocks[0].magnitude - (-0.10)).abs() < 1e-12);
    Ok(())
}

#[test]
fn a_propagation_onto_a_yield_quoted_name_or_with_a_non_finite_move_is_refused() {
    // `StressTester::apply` signs a shock named `rates` or `credit` the other
    // way, because those are yields. A propagated move is a price move; a
    // held object that happened to carry one of those names would be
    // silently inverted, so it is refused by name.
    let book = vec![exposure("rates", 1_000_000, &[])];
    let onto_rates = vec![PropagatedShock {
        target: "rates".to_string(),
        magnitude: -0.02,
        order: 1,
        over_days: 1.0,
    }];
    let refused = causal_scenario(
        "policy",
        STANDARD_DRIVER_SHOCK,
        DriverShockSizing::StandardDriverShock,
        &onto_rates,
        &book,
    );
    let message = refused
        .expect_err("a target named rates is refused")
        .message()
        .to_string();
    assert!(
        message.contains("yield move"),
        "the refusal names the inversion it prevents: {message}"
    );

    let book = vec![exposure("airline", 1_000_000, &[])];
    let broken = vec![PropagatedShock {
        target: "airline".to_string(),
        magnitude: f64::NAN,
        order: 1,
        over_days: 1.0,
    }];
    let refused = causal_scenario(
        "oil",
        STANDARD_DRIVER_SHOCK,
        DriverShockSizing::StandardDriverShock,
        &broken,
        &book,
    );
    assert!(
        refused.is_err(),
        "a move that is not a number is refused rather than stressed on"
    );

    let refused = causal_scenario(
        "oil",
        0.0,
        DriverShockSizing::StandardDriverShock,
        &[],
        &book,
    );
    assert!(refused.is_err(), "a zero driver shock is not a move");
}

// --- the adversarial sequence -------------------------------------------------

#[test]
fn the_adversarial_sequence_moves_every_carried_factor_against_the_book_inside_the_library_envelope()
-> Result<()> {
    // A long equity book with a long bond: equity down hurts, yields up
    // hurts. The sequence must pick each direction from the book, not from
    // history — the library's own rates shocks are mostly *down*.
    let book = vec![
        exposure("stock", 4_000_000, &[("equity", 1.0)]),
        exposure("bond", 2_000_000, &[("rates", 6.0)]),
    ];
    let library = standard_library();
    let equity = 10_000_000.0;
    let tester = StressTester::new(10.0);
    let sequence = tester
        .adversarial_sequence(&library, &book, equity, 0.20, at())?
        .expect("a book carrying betas has a sequence");

    // Premise: both carried factors became steps, and the factors the book
    // carries nothing for are named as unsized rather than dropped silently.
    let factors: Vec<&str> = sequence
        .steps
        .iter()
        .map(|step| step.factor.as_str())
        .collect();
    assert_eq!(
        factors,
        vec!["equity", "rates"],
        "largest loss first: {factors:?}"
    );
    assert!(
        sequence
            .unsized_factors
            .iter()
            .any(|factor| factor == "volatility"),
        "a library factor the book carries no beta for is named: {:?}",
        sequence.unsized_factors
    );
    assert_eq!(sequence.scenario.name, ADVERSARIAL_SCENARIO_NAME);

    // Property one: each move is the library's largest for that factor, and
    // signed against the book. Equity's largest is -0.50 in 2008 and a long
    // book loses on a fall; rates' largest is 0.035 in 2022 and a long bond
    // loses on a rise, whatever sign the library mostly used.
    let largest = |factor: &str| {
        library
            .iter()
            .flat_map(|scenario| &scenario.shocks)
            .filter(|shock| shock.factor == factor)
            .map(|shock| shock.magnitude.abs())
            .fold(0.0_f64, f64::max)
    };
    let equity_step = &sequence.steps[0];
    assert!((equity_step.magnitude - (-largest("equity"))).abs() < 1e-12);
    assert!(equity_step.book_sensitivity > 0.0);
    let rates_step = &sequence.steps[1];
    assert!(
        (rates_step.magnitude - largest("rates")).abs() < 1e-12,
        "a long bond is hurt by yields up, so the move is positive: {}",
        rates_step.magnitude
    );
    assert!(rates_step.book_sensitivity < 0.0);
    for step in &sequence.steps {
        assert!(step.loss > 0.0, "every step loses: {step:?}");
        assert!(
            step.magnitude.signum() == -step.book_sensitivity.signum(),
            "the move is signed against the book: {step:?}"
        );
    }

    // Property two: the cumulative path is non-decreasing, lands on the
    // combined scenario's loss, and the breach is named at the step it
    // happens. 4,000,000 at -50% is 2,000,000 — a fifth of equity on its
    // own, so with the exit cost the first step already breaches.
    let mut previous = 0.0;
    for step in &sequence.steps {
        assert!(step.cumulative_loss_fraction >= previous);
        previous = step.cumulative_loss_fraction;
    }
    assert!(
        (previous - sequence.result.loss_fraction).abs() < 1e-9,
        "the last cumulative figure is the scenario's loss: {previous} vs {}",
        sequence.result.loss_fraction
    );
    assert_eq!(sequence.first_breach, Some(0), "{}", sequence.summarise());

    // Property three: the sequence is at least as bad as any library
    // scenario for this book, because every move is at the envelope and
    // against the book.
    let worst_library = tester
        .apply_all(&library, &book, equity, at())?
        .into_iter()
        .map(|result| result.loss_fraction)
        .fold(0.0_f64, f64::max);
    assert!(worst_library > 0.0, "premise: the library hurts this book");
    assert!(
        sequence.result.loss_fraction >= worst_library,
        "the adversarial loss {} is below a library scenario's {worst_library}",
        sequence.result.loss_fraction
    );
    Ok(())
}

#[test]
fn a_book_carrying_no_library_factor_has_no_adversarial_sequence_and_a_bad_tolerance_is_refused()
-> Result<()> {
    let tester = StressTester::new(10.0);
    let library = standard_library();
    let unmeasured = vec![exposure("private-fund", 1_000_000, &[])];
    let none = tester.adversarial_sequence(&library, &unmeasured, 10_000_000.0, 0.20, at())?;
    assert!(
        none.is_none(),
        "with no beta to sign against there is no sequence, not an empty one: {none:?}"
    );

    // A long and a short that cancel exactly are measured and flat: no step,
    // and not unsized either.
    let flat = vec![
        exposure("long", 1_000_000, &[("equity", 1.0)]),
        exposure("short", -1_000_000, &[("equity", 1.0)]),
    ];
    let none = tester.adversarial_sequence(&library, &flat, 10_000_000.0, 0.20, at())?;
    assert!(
        none.is_none(),
        "a flat book has no direction to move against"
    );

    let book = vec![exposure("stock", 1_000_000, &[("equity", 1.0)])];
    assert!(
        tester
            .adversarial_sequence(&library, &book, 10_000_000.0, 1.0, at())
            .is_err(),
        "a tolerance of one is not a fraction of equity below one"
    );
    assert!(
        tester
            .adversarial_sequence(&[], &book, 10_000_000.0, 0.2, at())
            .is_err(),
        "an empty library bounds nothing"
    );
    Ok(())
}
