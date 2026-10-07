//! EVENT-001 integration: the platform's probability vs. the market's, replay-proven.
//!
//! The scoring library is complete and tested in isolation
//! (backend/crates/services/qip-prediction/src/scoring.rs), but nothing yet
//! calls it from the kernel. This test verifies that when the wiring is done:
//!
//! (a) A cycle containing an event contract replays identically on a second run
//! (b) qip_prediction::scoring::compare is invoked during the LEARN stage
//! (c) The scored result (probability comparison) reaches the event log
//!
//! The test is currently a placeholder that asserts the preconditions exist.
//! Once EVENT-005 (market creation) lands, the full cycle test can be written.
#![allow(clippy::unwrap_used, clippy::expect_used)]

/// Placeholder: asserts that qip_prediction scoring is available.
///
/// Once EVENT-005 (event market creation) exists, this test will:
/// 1. Create a simulated event venue with outcome contracts
/// 2. Run Platform::cycle twice with identical inputs
/// 3. Assert both cycles produce identical event logs
/// 4. Assert scoring::compare was called in LEARN stage
/// 5. Assert the ScoredForecast result was recorded in the event log
#[test]
fn event_prediction_scoring_is_ready_for_kernel_integration() {
    // Verify the scoring module is available and exports the compare function
    use qip_prediction::scoring;

    // This test passes as soon as qip_prediction::scoring::compare is exported
    // and the module is reachable from qip-acceptance. The full integration
    // test that drives the cycle will be added once EVENT-005 lands.

    // Precondition: qip_prediction crate exists and is linked
    let _ = scoring::ScoredForecast::new;
    let _ = scoring::compare;
}
