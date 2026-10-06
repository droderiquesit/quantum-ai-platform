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

/// Placeholder test structure for full EVENT-001 integration.
///
/// To be completed once EVENT-005 (market creation) provides event fixtures.
/// The test will verify:
/// - Identical replay of cycles with event contracts
/// - Scoring integration in the LEARN stage
/// - Event log attribution to scoring results
#[test]
#[ignore = "EVENT-001 integration blocked: EVENT-005 (market creation) not yet implemented"]
fn a_cycle_containing_event_contracts_replays_identically_and_scores_them_in_learn() {
    // Once EVENT-005 exists:
    // let mut platform = platform(PlatformConfig::default()).expect("platform");
    //
    // // Seed with event contract fixtures
    // let event_venue = create_simulated_event_venue();
    // platform.seed_event_markets(vec![/* contracts */]);
    //
    // // Run first cycle
    // let cycle_time = Timestamp::from_secs(1_000);
    // platform.run_cycle(cycle_time);
    // let first_log = platform.event_log().records().clone();
    //
    // // Create new platform with same config
    // let mut platform_replay = platform(PlatformConfig::default()).expect("platform");
    // platform_replay.seed_event_markets(vec![/* same contracts */]);
    // platform_replay.run_cycle(cycle_time);
    // let second_log = platform_replay.event_log().records().clone();
    //
    // // Assert identical replay
    // assert_eq!(
    //     first_log, second_log,
    //     "event contract scoring must replay identically"
    // );
    //
    // // Assert scoring was invoked
    // let scoring_records = first_log
    //     .iter()
    //     .filter(|r| r.topic == Topic::ScoringCompleted)
    //     .count();
    // assert!(
    //     scoring_records > 0,
    //     "qip_prediction::scoring::compare must be invoked in LEARN stage"
    // );
}
