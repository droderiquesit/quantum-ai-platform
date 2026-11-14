//! The shared settlement convention: one term set for both halves.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::settlement::SettlementConvention;

const ALL: [SettlementConvention; 3] = [
    SettlementConvention::T0,
    SettlementConvention::T1,
    SettlementConvention::T2,
];

#[test]
fn each_convention_counts_its_own_settlement_days_and_spells_itself_distinctly() {
    let days: Vec<u32> = ALL.iter().map(|c| c.days()).collect();
    assert_eq!(days, vec![0, 1, 2]);
    let labels: Vec<&str> = ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(labels, vec!["T+0", "T+1", "T+2"]);
}

#[test]
fn every_spelling_the_convention_writes_parses_back_to_itself() {
    for c in ALL {
        assert_eq!(SettlementConvention::parse(c.as_str()).unwrap(), c);
    }
}

#[test]
fn a_convention_nobody_defined_is_refused_rather_than_defaulted() {
    for bad in ["T+3", "", "instant", "T+"] {
        assert!(SettlementConvention::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn the_wire_form_is_snake_case_so_the_centre_and_a_cell_read_the_same_value() {
    let json = serde_json::to_string(&SettlementConvention::T2).unwrap();
    assert_eq!(json, "\"t2\"");
    let back: SettlementConvention = serde_json::from_str(&json).unwrap();
    assert_eq!(back, SettlementConvention::T2);
}
