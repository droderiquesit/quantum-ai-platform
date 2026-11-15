//! R12-CAPITAL-4090 (ADR 0686). Written without being compiled or run, and
//! no mutation was run against any of these tests.

use qip_capital::placement::{PlacementRequirement, PlacementSource, optimise, reconcile};
use qip_core::{Decimal, dec};
use std::collections::BTreeMap;

fn source(
    id: &str,
    currency: &str,
    entity: &str,
    venue: Option<&str>,
    available: Decimal,
    carry: Decimal,
) -> PlacementSource {
    PlacementSource {
        id: id.to_string(),
        currency: currency.to_string(),
        entity: entity.to_string(),
        counterparty: format!("cp-{id}"),
        venue: venue.map(str::to_string),
        available,
        carry_bps: carry,
    }
}

fn need(
    id: &str,
    currency: &str,
    entity: &str,
    venue: &str,
    amount: Decimal,
) -> PlacementRequirement {
    PlacementRequirement {
        id: id.to_string(),
        currency: currency.to_string(),
        entity: entity.to_string(),
        venue: venue.to_string(),
        amount,
    }
}

fn ledger(rows: &[(&str, Decimal)]) -> BTreeMap<String, Decimal> {
    rows.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
}

#[test]
fn the_cheapest_eligible_source_is_drawn_first_and_the_totals_reconcile_to_the_ledger() {
    let sources = vec![
        source("dear", "USD", "ent-a", None, dec!("1000"), dec!("90")),
        source("cheap", "USD", "ent-a", None, dec!("600"), dec!("20")),
    ];
    let ledger = ledger(&[("dear", dec!("1000")), ("cheap", dec!("600"))]);
    let requirements = vec![need("r1", "USD", "ent-a", "XNYS", dec!("800"))];
    let plan = optimise(&requirements, &sources, &ledger).expect("plan");
    // Premise: something was placed.
    assert!(!plan.draws.is_empty());
    assert_eq!(plan.drawn_from("cheap"), dec!("600"));
    assert_eq!(plan.drawn_from("dear"), dec!("200"));
    assert_eq!(plan.placed_for("r1"), dec!("800"));
    assert!(plan.unplaced.is_empty());
    reconcile(&plan, &requirements, &ledger).expect("reconciles");
}

#[test]
fn a_source_offered_above_the_ledger_is_refused_rather_than_cut_to_it() {
    let sources = vec![source("s", "USD", "ent-a", None, dec!("500"), dec!("10"))];
    let ledger = ledger(&[("s", dec!("499"))]);
    let refusal = optimise(
        &[need("r", "USD", "ent-a", "XNYS", dec!("1"))],
        &sources,
        &ledger,
    )
    .expect_err("offer above ledger");
    assert!(refusal.message().contains("ledger is authoritative"));
}

#[test]
fn a_source_the_ledger_does_not_hold_is_refused() {
    let sources = vec![source("ghost", "USD", "ent-a", None, dec!("1"), dec!("10"))];
    let refusal = optimise(&[], &sources, &BTreeMap::new()).expect_err("unknown source");
    assert!(refusal.message().contains("ghost"));
}

#[test]
fn a_requirement_the_eligible_sources_cannot_meet_is_unplaced_whole_and_draws_nothing() {
    let sources = vec![
        source("a", "USD", "ent-a", None, dec!("300"), dec!("10")),
        // Right amount, wrong entity: must not be counted.
        source("b", "USD", "ent-b", None, dec!("5000"), dec!("1")),
        // Right amount, wrong currency: must not be counted.
        source("c", "EUR", "ent-a", None, dec!("5000"), dec!("1")),
    ];
    let ledger = ledger(&[("a", dec!("300")), ("b", dec!("5000")), ("c", dec!("5000"))]);
    let plan = optimise(
        &[need("r", "USD", "ent-a", "XNYS", dec!("301"))],
        &sources,
        &ledger,
    )
    .expect("plan");
    assert!(plan.draws.is_empty());
    assert_eq!(plan.unplaced.len(), 1);
    assert_eq!(plan.unplaced[0].requirement, "r");
}

#[test]
fn a_source_bound_to_one_venue_funds_no_other_venue() {
    let sources = vec![source(
        "v",
        "USD",
        "ent-a",
        Some("XLON"),
        dec!("900"),
        dec!("5"),
    )];
    let ledger = ledger(&[("v", dec!("900"))]);
    let plan = optimise(
        &[
            need("at-xlon", "USD", "ent-a", "XLON", dec!("100")),
            need("at-xnys", "USD", "ent-a", "XNYS", dec!("100")),
        ],
        &sources,
        &ledger,
    )
    .expect("plan");
    assert_eq!(plan.placed_for("at-xlon"), dec!("100"));
    assert_eq!(plan.placed_for("at-xnys"), Decimal::ZERO);
    assert_eq!(plan.unplaced.len(), 1);
}

#[test]
fn two_requirements_cannot_draw_one_balance_twice() {
    let sources = vec![source("s", "USD", "ent-a", None, dec!("100"), dec!("5"))];
    let ledger = ledger(&[("s", dec!("100"))]);
    let plan = optimise(
        &[
            need("first", "USD", "ent-a", "XNYS", dec!("70")),
            need("second", "USD", "ent-a", "XNYS", dec!("70")),
        ],
        &sources,
        &ledger,
    )
    .expect("plan");
    assert_eq!(plan.placed_for("first"), dec!("70"));
    assert_eq!(plan.placed_for("second"), Decimal::ZERO);
    assert!(plan.drawn_from("s") <= dec!("100"));
}

#[test]
fn reconcile_refuses_a_plan_that_outruns_the_ledger() {
    use qip_capital::placement::{PlacementDraw, PlacementPlan};
    let plan = PlacementPlan {
        draws: vec![PlacementDraw {
            requirement: "r".to_string(),
            source: "s".to_string(),
            amount: dec!("101"),
        }],
        unplaced: Vec::new(),
    };
    let refusal = reconcile(
        &plan,
        &[need("r", "USD", "ent-a", "XNYS", dec!("101"))],
        &ledger(&[("s", dec!("100"))]),
    )
    .expect_err("overdraw");
    assert!(refusal.message().contains("invented capital"));
}

#[test]
fn a_duplicate_source_or_a_negative_requirement_is_refused() {
    let one = source("s", "USD", "ent-a", None, dec!("1"), dec!("1"));
    let ledger = ledger(&[("s", dec!("1"))]);
    assert!(optimise(&[], &[one.clone(), one], &ledger).is_err());
    let sources = vec![source("s", "USD", "ent-a", None, dec!("1"), dec!("1"))];
    assert!(
        optimise(
            &[need("r", "USD", "ent-a", "XNYS", dec!("-1"))],
            &sources,
            &ledger
        )
        .is_err()
    );
}
