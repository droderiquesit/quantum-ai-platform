//! The peer saga and the desk price exposure with one model (R12-MESH-5069).
//!
//! `qip-peer-link`'s origination charges a crossing the expected adverse move
//! over the round trip the wire measured to the peer; the desk's net-edge
//! calculator charges a cycle the same move over its venue round trip plus any
//! coordination latency. Both call `qip_core::exposure`, so the formula cannot
//! drift — but the volatility each feeds it is a constant of its own, because
//! the saga may not name `qip-arbitrage` (the mesh and the durable fabric
//! share no crate, R12-COVERAGE-7194) and `qip-arbitrage` may not name the
//! peer crates (MESH-013). This crate links both, so this is where the two
//! constants are held to one value: a peer crossing priced at a kinder
//! volatility than the desk's would be a cycle the desk refuses and the mesh
//! opens.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_arbitrage::EdgeAssumptions;
use qip_core::Duration;
use qip_core::exposure::expected_absolute_move_fraction;
use qip_peer_link::saga::COORDINATION_VOLATILITY_ANNUAL_F64;

/// Mutation (run, failed here, restored): raise `EdgeAssumptions::default`'s
/// volatility to 0.45 — the first assertion fails naming both figures.
#[test]
fn the_peer_sagas_coordination_volatility_is_the_desks_default_volatility() {
    let desk = EdgeAssumptions::default();
    assert!(
        (COORDINATION_VOLATILITY_ANNUAL_F64 - desk.volatility_annual_f64).abs() < f64::EPSILON,
        "the saga prices a peer crossing at {COORDINATION_VOLATILITY_ANNUAL_F64} annualised \
         volatility and the desk prices its cycles at {}; one of them is a kinder model",
        desk.volatility_annual_f64
    );
    // Premise: the shared formula charges something for a measured trip, so
    // equality of the constants is equality of a deduction and not of two
    // numbers nothing reads.
    assert!(
        expected_absolute_move_fraction(
            COORDINATION_VOLATILITY_ANNUAL_F64,
            Duration::from_millis(250)
        ) > 0.0
    );
}
