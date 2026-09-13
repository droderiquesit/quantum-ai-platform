//! The feasibility gate vocabulary, declared once.
//!
//! Two feasibility modules refuse orders — `qip_edge::feasibility` at the
//! cells and `qip_execution_engine::feasibility` on the desk — and each
//! refusal is counted, journaled and carried under the literal of the gate
//! that made it. The two modules used to declare the same strings
//! separately, "identical to the edge crate's names… so a refusal on either
//! plane correlates under one vocabulary", held identical by nothing but
//! care. The centre now reads the vocabulary too, to admit a cell's refusal
//! to the window a venue is withdrawn on (blueprint §12.3, fourth row), and
//! a third copy held identical by care is how a reworded gate at one plane
//! becomes a refusal the centre silently files under `other`. So the
//! literals live here, in the crate both planes and the kernel already
//! share, and each module aliases them by name.
//!
//! Literals, so every series keyed on a gate stays bounded by the source
//! and never by the market.

/// The order is not a whole number of the venue's minimum quantity, or is
/// below it.
pub const GATE_MINIMUM_QUANTITY: &str = "feasibility_minimum_quantity";
/// The order's notional is below the venue's minimum.
pub const GATE_MINIMUM_NOTIONAL: &str = "feasibility_minimum_notional";
/// The order is not a whole number of lots.
pub const GATE_LOT: &str = "feasibility_lot";
/// The order's price is not on the venue's tick grid.
pub const GATE_TICK: &str = "feasibility_tick";
/// The order is larger than the size resting at the touch (edge only).
pub const GATE_DEPTH: &str = "feasibility_depth";
/// A cycle's fixed cost is not covered by its edge (edge desk only).
pub const GATE_FEE_FLOOR: &str = "feasibility_fee_floor";
/// A cycle's gas is not covered by its edge (edge desk only).
pub const GATE_GAS_FLOOR: &str = "feasibility_gas_floor";
/// A policy-fed constraint refused (edge only).
pub const GATE_CONSTRAINT: &str = "feasibility_constraint";

/// Every gate a cell's feasibility module can refuse under — the set the
/// centre admits a carried refusal by. A gate outside this array is not a
/// feasibility refusal whatever its text says.
pub const EDGE_GATES: [&str; 8] = [
    GATE_MINIMUM_QUANTITY,
    GATE_MINIMUM_NOTIONAL,
    GATE_LOT,
    GATE_TICK,
    GATE_DEPTH,
    GATE_FEE_FLOOR,
    GATE_GAS_FLOOR,
    GATE_CONSTRAINT,
];

/// Every gate the desk's feasibility module can refuse under: the four the
/// central path mirrors from the edge.
pub const DESK_GATES: [&str; 4] = [
    GATE_MINIMUM_QUANTITY,
    GATE_MINIMUM_NOTIONAL,
    GATE_LOT,
    GATE_TICK,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_gate_literal_is_distinct_and_carries_the_feasibility_prefix() {
        // A duplicated literal would merge two gates on every series keyed
        // on them; a literal without the prefix would be one an operator
        // reading `qip_edge_refusals_total{gate}` could not tell from a
        // posture gate.
        let mut seen = std::collections::BTreeSet::new();
        for gate in EDGE_GATES {
            assert!(seen.insert(gate), "{gate} is declared twice");
            assert!(
                gate.starts_with("feasibility_"),
                "{gate} does not read as a feasibility gate"
            );
        }
        for gate in DESK_GATES {
            assert!(
                EDGE_GATES.contains(&gate),
                "the desk refuses under {gate}, which no cell can"
            );
        }
    }

    #[test]
    fn the_desk_gates_literal_has_no_duplicate_of_its_own() {
        // A code-review finding: the test above checks `EDGE_GATES` for
        // internal duplication and `DESK_GATES` only for membership in
        // `EDGE_GATES` — a mutation that duplicated an entry inside
        // `DESK_GATES` itself (as opposed to `EDGE_GATES`) would pass both
        // checks there, because every duplicate is still a member of
        // `EDGE_GATES`. A duplicate here would merge two of the desk's four
        // gates on every series keyed on them — `qip_orders_refused_total`
        // and the declined-path evidence `venue_review` reads — exactly as
        // a duplicate in `EDGE_GATES` would for the cells.
        let mut seen = std::collections::BTreeSet::new();
        for gate in DESK_GATES {
            assert!(seen.insert(gate), "{gate} is declared twice in DESK_GATES");
        }
        assert_eq!(
            seen.len(),
            DESK_GATES.len(),
            "the premise: this test must actually walk every declared gate"
        );
    }
}
