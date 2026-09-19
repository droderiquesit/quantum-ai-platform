//! §49.1's targets, checked against the values that claim to carry them.
//!
//! This suite exists because the objectives were prose. `slo.rs` shipped a
//! `Slo` type, eight default objectives and an `evaluate`, and
//! `grep -rln 'Slo::availability\|Slo::latency\|default_slos' backend/crates
//! --include=*.rs` printed exactly two paths — the module and its own unit
//! test. None of the eight was a §49.1 target. So the section stating what the
//! platform must achieve had nothing asserting it and nothing able to
//! contradict it, which is indistinguishable from not having targets.
//!
//! What this file can and cannot do is worth stating plainly, because the
//! difference is the whole value of it.
//!
//! * It **can** assert that the declared set is §49.1's set, that each target
//!   is the blueprint's own figure, and that an objective nothing measured is
//!   never reported as achieved. Those are properties of the declaration and
//!   hold on any machine.
//! * It **cannot** measure most of the targets. Four are ratios over
//!   wall-clock time on a node that does not exist. The rest need a driven
//!   system this suite does not stand up, and the two latency percentiles are
//!   deliberately not asserted here at all — see the note on that below.
//!
//! An honest partial is the point. A suite that evaluated fifteen objectives
//! against no observations would report all fifteen met, which is worse than
//! reporting none.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_observability::slo::{
    BLUEPRINT_SLOS_NEEDING_A_DEPLOYMENT, Slo, SloWindow, blueprint_slos, default_slos,
};

/// §49.1's rows, as the blueprint states them, paired with the objective name
/// that must carry each and the target that must be on it.
///
/// Transcribed from
/// `awk '/^49\.1 /{f=1} f&&/^49\.2 /{exit} f'
/// docs/architecture/algorik-blueprint-v10.1-source.md`.
/// Fifteen entries for fourteen rows: "Cycle completion — Path 1 / Path 2"
/// states two targets and is two objectives here.
const BLUEPRINT_ROWS: &[(&str, f64)] = &[
    ("node-availability-market-hours", 0.999),
    ("strategy-evaluation-p99", 0.99),
    ("wire-to-first-order-p99", 0.99),
    ("belief-calibration-within-tolerance", 1.0),
    ("netting-ratio", 1.0),
    ("cycle-completion-path-1", 0.97),
    ("cycle-completion-path-2", 0.93),
    ("arrival-dispersion-p99", 0.99),
    ("quote-message-to-trade-within-venue-requirement", 1.0),
    ("mirror-drift-inside-soft-band", 0.95),
    ("live-versus-holdout-consistency", 0.80),
    ("mark-staleness-zero-past-limit", 1.0),
    ("effective-breadth-above-floor", 1.0),
    ("reconciliation-breaks-zero-unexplained", 1.0),
    ("unauthorised-transfers-reaching-execution-zero", 1.0),
];

fn named<'a>(slos: &'a [Slo], name: &str) -> &'a Slo {
    slos.iter()
        .find(|slo| slo.name == name)
        .unwrap_or_else(|| panic!("§49.1 declares no objective named {name}"))
}

#[test]
fn every_row_of_section_49_1_is_declared_as_an_objective_with_its_own_target() {
    let declared = blueprint_slos();
    // Premise: the set is non-empty and is not the eight that shipped before.
    // Without this the loop below would pass vacuously on an empty vector,
    // which is the exact failure mode this suite was written to end.
    assert!(!declared.is_empty(), "§49.1 declares no objectives at all");
    assert_eq!(
        declared.len(),
        BLUEPRINT_ROWS.len(),
        "§49.1 has {} objectives across its fourteen rows; the declaration has {}. \
         A row dropped from the declaration is a target nothing can miss",
        BLUEPRINT_ROWS.len(),
        declared.len()
    );

    for (name, target) in BLUEPRINT_ROWS {
        let slo = named(&declared, name);
        assert!(
            (slo.target - target).abs() < 1e-12,
            "{name} must carry §49.1's own figure of {target}, not {}. A target \
             loosened in the declaration is a target loosened everywhere, and \
             nothing outside this assertion would notice",
            slo.target
        );
    }
}

#[test]
fn the_percentile_rows_carry_their_thresholds_in_the_unit_the_blueprint_states() {
    // §49.1 states three percentile bounds, in two different units: 90 µs for
    // strategy evaluation, 1.3 ms and 1.5 ms for the other two. `Slo` holds
    // milliseconds, so 90 µs is 0.09. The conversion is the thing most likely
    // to be got wrong by a factor of a thousand, and a factor of a thousand on
    // a latency ceiling is a ceiling that can never fire.
    let declared = blueprint_slos();
    for (name, threshold_ms) in [
        ("strategy-evaluation-p99", 0.09),
        ("wire-to-first-order-p99", 1.3),
        ("arrival-dispersion-p99", 1.5),
    ] {
        let slo = named(&declared, name);
        let got = slo
            .latency_threshold_ms
            .unwrap_or_else(|| panic!("{name} is a percentile bound and carries no threshold"));
        assert!(
            (got - threshold_ms).abs() < 1e-12,
            "{name} must bound at {threshold_ms}ms, not {got}ms"
        );
        assert!(
            (slo.target - 0.99).abs() < 1e-12,
            "{name} is a p99, so its target is 0.99 and not {}",
            slo.target
        );
    }
}

#[test]
fn the_netting_ratio_objective_carries_the_floor_as_a_number() {
    // "Netting ratio — above 1.5". Held as a number rather than in the
    // description, because a figure that lives only in prose is a figure
    // nothing can check and an edit can move it without a test noticing.
    let declared = blueprint_slos();
    let slo = named(&declared, "netting-ratio");
    let floor = slo
        .ratio_floor
        .expect("the netting-ratio objective must carry §49.1's floor as a value");
    assert!(
        (floor - 1.5).abs() < 1e-12,
        "§49.1 puts the netting ratio above 1.5, not {floor}"
    );
    // The premise that makes the assertion above mean something: no other
    // objective carries a ratio floor, so this one is not passing because the
    // field happens to be populated everywhere.
    let carrying: Vec<&str> = declared
        .iter()
        .filter(|s| s.ratio_floor.is_some())
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(carrying, vec!["netting-ratio"]);
}

#[test]
fn an_objective_nothing_measured_is_never_reported_as_achieved() {
    // The defect this guards is the one `MaxExpectedShortfall` already shipped
    // here once: a control that reads as protection while the state it
    // consults is always empty. `evaluate(0, 0)` reports `is_met`, on purpose,
    // so a quiet window does not page — which means `is_met` alone cannot
    // distinguish an objective achieved from one never measured. Four of these
    // fifteen have no way to be measured at all until something is deployed.
    let declared = blueprint_slos();
    let mut checked = 0;
    for slo in &declared {
        let status = slo.evaluate(0, 0);
        assert!(
            status.is_met,
            "{}: the no-data arm still reports met, and a change to that would \
             page on every quiet window",
            slo.name
        );
        assert!(
            !status.is_observed(),
            "{}: nothing was measured, so nothing may say it was",
            slo.name
        );
        assert!(
            !status.is_page_worthy(),
            "{}: no data must not page",
            slo.name
        );
        checked += 1;
    }
    assert_eq!(checked, BLUEPRINT_ROWS.len(), "every objective was checked");

    // And the discriminator is real rather than always false: an objective
    // that *was* measured reports observed.
    let measured = named(&declared, "netting-ratio").evaluate(100, 100);
    assert!(
        measured.is_observed(),
        "a measured objective must read observed"
    );
    assert!(measured.is_met);
}

#[test]
fn the_objectives_needing_a_deployment_are_named_and_are_a_strict_subset() {
    // Naming the gap as a value rather than a sentence. These four are ratios
    // over wall-clock time on a node that does not exist — `execution_nodes =
    // {}` in every environment — so no fixture produces one, and a summary
    // that folded them in with the rest would report four objectives met
    // having measured none of them.
    let declared = blueprint_slos();
    assert!(
        !BLUEPRINT_SLOS_NEEDING_A_DEPLOYMENT.is_empty(),
        "the premise: there is a gap, and it is named"
    );
    for name in BLUEPRINT_SLOS_NEEDING_A_DEPLOYMENT {
        assert!(
            declared.iter().any(|slo| slo.name == *name),
            "{name} is listed as needing a deployment but is not a declared objective; \
             a gap naming something that does not exist is not a record of anything"
        );
    }
    // Strict subset: if every objective needed a deployment, the list would be
    // a restatement of the set rather than a finding about part of it.
    assert!(
        BLUEPRINT_SLOS_NEEDING_A_DEPLOYMENT.len() < declared.len(),
        "every objective cannot need a deployment, or the distinction says nothing"
    );
}

#[test]
fn the_shipped_service_objectives_and_the_blueprint_objectives_stay_distinct() {
    // `default_slos()` is the platform's own per-service set and predates
    // §49.1 being written down. They are different things with different
    // windows, and merging them would let a §49.1 target be satisfied by an
    // unrelated service objective that happens to share a name.
    let shipped = default_slos();
    let blueprint = blueprint_slos();
    assert!(
        !shipped.is_empty() && !blueprint.is_empty(),
        "both sets exist"
    );
    for slo in &blueprint {
        assert!(
            !shipped.iter().any(|other| other.name == slo.name),
            "{} appears in both sets; a §49.1 target must not be met by a \
             service objective wearing the same name",
            slo.name
        );
    }
}

#[test]
fn a_missed_blueprint_target_is_actually_reported_as_missed() {
    // The assertions above are about the declaration. This one is about the
    // arithmetic: a target that cannot be missed is not a target. §49.1's
    // zero-tolerance rows are the sharpest case — one reconciliation break
    // must miss an objective whose target is 1.0.
    let declared = blueprint_slos();
    let zero_tolerance = named(&declared, "reconciliation-breaks-zero-unexplained");
    assert!(
        (zero_tolerance.target - 1.0).abs() < 1e-12,
        "the premise: this objective admits no failures at all"
    );

    let clean = zero_tolerance.evaluate(1_000, 1_000);
    assert!(clean.is_met, "a window with no breaks meets the objective");

    let one_break = zero_tolerance.evaluate(999, 1_000);
    assert!(
        !one_break.is_met,
        "one unexplained reconciliation break is severity one in §49.1, and an \
         objective that survived it would be measuring nothing"
    );
    // Exactly 1.0, not more: a target of 1.0 leaves no budget to divide by, so
    // `evaluate` saturates rather than reporting a ratio against zero. The
    // assertion is written `>=` deliberately — asserting `> 1.0` here failed,
    // and the failure was this test being wrong about the arithmetic, not the
    // arithmetic being wrong.
    assert!(
        one_break.budget_consumed >= 1.0,
        "a zero-tolerance objective has no error budget to spend, consumed {}",
        one_break.budget_consumed
    );
    assert!(
        clean.budget_consumed < 1.0,
        "the premise for the line above: a clean window has not exhausted it"
    );

    // And a target below 1.0 tolerates what this one does not, so the
    // strictness above belongs to the target and not to `evaluate`.
    let tolerant = named(&declared, "live-versus-holdout-consistency");
    assert!(
        tolerant.evaluate(850, 1_000).is_met,
        "§49.1 asks 80 percent of funded strategies to sit within band, so 85 \
         percent meets it"
    );
}

#[test]
fn every_blueprint_objective_names_a_window_and_a_service() {
    // An objective with no window is a rate over an unstated period, and an
    // objective with no service is a target nobody owns. Both read as
    // measurement and neither can be acted on.
    let declared = blueprint_slos();
    assert!(!declared.is_empty(), "the premise: there are objectives");
    for slo in &declared {
        assert!(!slo.service.is_empty(), "{} names no service", slo.name);
        assert!(
            !slo.description.is_empty(),
            "{} has no description",
            slo.name
        );
        assert!(
            slo.window.hours() > 0.0,
            "{} has a window of no length",
            slo.name
        );
        assert!(
            (0.0..=1.0).contains(&slo.target),
            "{} has a target of {}, which is not a fraction",
            slo.name,
            slo.target
        );
    }
    // The windows actually differ: percentile objectives are hourly and the
    // slower ones are not, so `SloWindow` is carrying information rather than
    // being the same value fifteen times.
    assert!(
        declared.iter().any(|s| s.window == SloWindow::Hour),
        "no objective is measured hourly"
    );
    assert!(
        declared.iter().any(|s| s.window != SloWindow::Hour),
        "every objective is hourly, so the window distinguishes nothing"
    );
}
