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

use std::collections::BTreeSet;

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
/// The centre has withdrawn the venue this order is bound for (edge only).
///
/// Not a fact about the order's size, its grid or the book: a fact about the
/// venue, shipped on policy slot 11 and refused against here so that a desk
/// a cell installed *before* the withdrawal stops reaching the venue on its
/// next pass rather than on its next process restart (ADR 0062).
pub const GATE_WITHDRAWN_VENUE: &str = "feasibility_withdrawn_venue";

/// Every gate a cell's feasibility module can refuse under — the set the
/// centre admits a carried refusal by. A gate outside this array is not a
/// feasibility refusal whatever its text says.
pub const EDGE_GATES: [&str; 9] = [
    GATE_MINIMUM_QUANTITY,
    GATE_MINIMUM_NOTIONAL,
    GATE_LOT,
    GATE_TICK,
    GATE_DEPTH,
    GATE_FEE_FLOOR,
    GATE_GAS_FLOOR,
    GATE_CONSTRAINT,
    GATE_WITHDRAWN_VENUE,
];

/// Whether a carried refusal is the echo of a withdrawal the **centre
/// itself holds** — the platform's own decision arriving back at it — rather
/// than an observation about a venue.
///
/// **Why the centre's set and not the gate alone.** This took `gate` only
/// until a security review of ADR 0062's edge closure: whether a refusal
/// counted as "the platform citing itself" was then decided by a string on a
/// report arriving over a wire `qip-edge/src/mesh.rs` says authenticates
/// nobody. A cell holding a stale slot 11 — the centre stopped shipping
/// policy, or two operators reinstated the venue and the cell never heard —
/// refuses every intent at a venue *currently in use* under
/// [`GATE_WITHDRAWN_VENUE`], and the centre believed it. Those refusals then
/// reached no evidence window, so the venue could not be withdrawn a second
/// time on edge evidence for as long as the slot stayed stale: a control
/// that reads as protection and cannot fire. So the centre's own withdrawn
/// set decides, and a cell citing a withdrawal the centre does not hold is
/// an ordinary refusal at a venue in use.
///
/// **What an echo is for, given it is not evidence.** It is still a refusal,
/// and the venue it names is one the platform is still attempting. A share
/// test whose denominator dropped that venue the moment it was withdrawn
/// would make the runner-up a cluster of whatever remained — which is the
/// cascade `venue_review::assess` exists to refuse. So an echo is admitted
/// to the window as a denominator entry and can never be a numerator: the
/// candidate filter in `assess` is the withdrawn set, and a withdrawn venue
/// is never a candidate. What stops it holding the window hostage is
/// `venue_review`'s weighting — an echo may *sustain* a withdrawn venue's
/// weight up to the genuine evidence that venue still holds and never beyond
/// — and the centre admitting at most one echo per venue per report.
pub fn is_withdrawal_echo(gate: &str, venue: &str, withdrawn: &BTreeSet<String>) -> bool {
    gate == GATE_WITHDRAWN_VENUE && withdrawn.contains(venue)
}

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

    #[test]
    fn the_withdrawn_venue_gate_is_vocabulary_the_centre_admits_and_an_echo_only_the_centre_names()
    {
        // Three halves, and each guards a different failure. If the literal
        // ever left `EDGE_GATES`, `CentralPlane::attribute_refusals` would
        // file every withdrawal echo under the `other` constraint, which is
        // the label that means "a cell used a gate name this build does not
        // know" — a drift alarm firing on a gate this build declares. If
        // `is_withdrawal_echo` ever answered on the gate alone, a cell
        // holding a stale slot 11 would decide, by a string on an
        // unauthenticated wire, that its refusals may not be evidence — the
        // security finding this signature exists to close. And if it
        // answered true for a gate that asks a question about an *order*,
        // that order's refusal would be weighed as the platform citing
        // itself.
        let withdrawn: BTreeSet<String> = ["XLON".to_string()].into_iter().collect();
        assert!(
            EDGE_GATES.contains(&GATE_WITHDRAWN_VENUE),
            "the premise: the centre must recognise the gate it is about to weigh as an echo"
        );
        assert!(
            is_withdrawal_echo(GATE_WITHDRAWN_VENUE, "XLON", &withdrawn),
            "a refusal citing a withdrawal the centre itself holds was not read as its own echo"
        );
        assert!(
            !is_withdrawal_echo(GATE_WITHDRAWN_VENUE, "XNYS", &withdrawn),
            "a cell citing a withdrawal of a venue the centre has not withdrawn was believed, so \
             a stale slot decides what counts as evidence"
        );
        assert!(
            !is_withdrawal_echo(GATE_WITHDRAWN_VENUE, "XLON", &BTreeSet::new()),
            "with nothing withdrawn anywhere the centre still read a refusal as its own echo"
        );
        for gate in EDGE_GATES {
            if gate == GATE_WITHDRAWN_VENUE {
                continue;
            }
            assert!(
                !is_withdrawal_echo(gate, "XLON", &withdrawn),
                "{gate} asks a question about an order at a venue and was weighed as the \
                 platform citing itself anyway"
            );
        }
    }
}
