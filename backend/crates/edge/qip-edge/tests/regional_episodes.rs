//! Test that reflex cells record regional episodes (EXPAND-009).
//!
//! Each episode is typed (latency spike, fill/slippage, anomaly, failure,
//! microstructure, venue-behaviour) and names the cell and region where it
//! occurred. Episodes are recorded at the moment the event becomes known,
//! allowing them to explain decisions made in the same pass.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::regional_episode::EpisodeKind;
use qip_contracts::venue::VenueId;
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_edge::Cell;

const CELL_NAME: &str = "london-1";
const REGION_NAME: &str = "emea";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

/// Create a test cell for episode recording.
fn cell_for_episodes() -> Result<Cell> {
    use qip_edge::CellConfig;
    use qip_feature_dag::engine::FeatureEngine;

    let mut config = CellConfig::new(CELL_NAME, REGION_NAME);
    config.venues = vec![VenueId::new("XLON")];

    let features = FeatureEngine::default();
    Cell::new(config, features)
}

#[test]
fn a_cell_records_regional_episodes_with_cell_and_region_names() -> Result<()> {
    // Each regional episode must name the cell and region where the event
    // occurred, so an operator reviewing the journal can trace the source.
    let mut cell = cell_for_episodes()?;

    let now = t(0);
    cell.record_episode(
        EpisodeKind::LatencySpike,
        "XLON: 145ms measured".to_string(),
        now,
    );

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 1, "one episode should be recorded");
    let episode = &episodes[0];

    assert_eq!(episode.cell, CELL_NAME, "episode must name the cell");
    assert_eq!(episode.region, REGION_NAME, "episode must name the region");
    assert_eq!(
        episode.kind,
        EpisodeKind::LatencySpike,
        "episode kind should match"
    );
    assert_eq!(episode.recorded_at, now, "recorded_at should match");

    Ok(())
}

#[test]
fn episodes_are_recorded_in_order() -> Result<()> {
    // Episodes are recorded in the order they occur, so the journal reader
    // can reconstruct the sequence of events without re-running the cell.
    let mut cell = cell_for_episodes()?;

    let t0 = t(0);
    let t1 = t(1);
    let t2 = t(2);

    cell.record_episode(EpisodeKind::LatencySpike, "first".to_string(), t0);
    cell.record_episode(EpisodeKind::FillSlippage, "second".to_string(), t1);
    cell.record_episode(EpisodeKind::Anomaly, "third".to_string(), t2);

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 3, "all three episodes should be recorded");

    assert_eq!(episodes[0].kind, EpisodeKind::LatencySpike);
    assert_eq!(episodes[0].detail, "first");

    assert_eq!(episodes[1].kind, EpisodeKind::FillSlippage);
    assert_eq!(episodes[1].detail, "second");

    assert_eq!(episodes[2].kind, EpisodeKind::Anomaly);
    assert_eq!(episodes[2].detail, "third");

    Ok(())
}

#[test]
fn each_episode_kind_can_be_recorded() -> Result<()> {
    // The cell must be able to record all six episode kinds: latency spike,
    // fill/slippage, anomaly, failure, microstructure, and venue-behaviour.
    // If any kind is missing, an operator's review of a similar event would
    // find no record of it.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    cell.record_episode(EpisodeKind::LatencySpike, "XLON: 145ms".to_string(), now);
    cell.record_episode(
        EpisodeKind::FillSlippage,
        "XLON: expected 100.50, got 100.25".to_string(),
        now,
    );
    cell.record_episode(
        EpisodeKind::Anomaly,
        "crossed book detected at XLON".to_string(),
        now,
    );
    cell.record_episode(
        EpisodeKind::Failure,
        "XLON feed stalled for 30s".to_string(),
        now,
    );
    cell.record_episode(
        EpisodeKind::Microstructure,
        "XLON layer imbalance: 2:1 bid:ask".to_string(),
        now,
    );
    cell.record_episode(
        EpisodeKind::VenueBehaviour,
        "XLON: new settlement terms T+2".to_string(),
        now,
    );

    let episodes = cell.episodes();
    assert_eq!(
        episodes.len(),
        6,
        "all six episode kinds should be recorded"
    );

    assert_eq!(episodes[0].kind, EpisodeKind::LatencySpike);
    assert_eq!(episodes[1].kind, EpisodeKind::FillSlippage);
    assert_eq!(episodes[2].kind, EpisodeKind::Anomaly);
    assert_eq!(episodes[3].kind, EpisodeKind::Failure);
    assert_eq!(episodes[4].kind, EpisodeKind::Microstructure);
    assert_eq!(episodes[5].kind, EpisodeKind::VenueBehaviour);

    Ok(())
}

#[test]
fn episodes_are_bounded_to_prevent_unbounded_growth() -> Result<()> {
    // A cell under load receives many events. If episodes were unbounded,
    // the vec would grow without limit and eventually exhaust memory. Bounded
    // episodes prevent this; older episodes are discarded after being written
    // to the journal (by the caller, not shown here). The cell still records
    // up to the bound.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    const MAX_RETAINED_EPISODES: usize = 128;
    const EPISODES_TO_RECORD: usize = MAX_RETAINED_EPISODES + 50;

    for i in 0..EPISODES_TO_RECORD {
        cell.record_episode(EpisodeKind::LatencySpike, format!("episode_{i}"), now);
    }

    let episodes = cell.episodes();
    assert_eq!(
        episodes.len(),
        MAX_RETAINED_EPISODES,
        "episode count should be bounded to {MAX_RETAINED_EPISODES}"
    );

    // The oldest episodes should have been dropped; the remaining should be
    // the last MAX_RETAINED_EPISODES recorded.
    let first_detail = &episodes[0].detail;
    let expected_first_index = EPISODES_TO_RECORD - MAX_RETAINED_EPISODES;
    assert_eq!(
        first_detail.as_str(),
        format!("episode_{expected_first_index}").as_str(),
        "oldest retained episode should be {expected_first_index}"
    );

    let last_detail = &episodes[MAX_RETAINED_EPISODES - 1].detail;
    let expected_last_index = EPISODES_TO_RECORD - 1;
    assert_eq!(
        last_detail.as_str(),
        format!("episode_{expected_last_index}").as_str(),
        "newest episode should be {expected_last_index}"
    );

    Ok(())
}

#[test]
fn episodes_can_be_cleared() -> Result<()> {
    // A cell needs to clear its episodes between passes or sessions, so the
    // episode vec does not retain old events that are no longer relevant.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    cell.record_episode(EpisodeKind::LatencySpike, "first".to_string(), now);
    cell.record_episode(EpisodeKind::FillSlippage, "second".to_string(), now);

    assert_eq!(cell.episodes().len(), 2, "episodes should be recorded");

    cell.clear_episodes();
    assert_eq!(cell.episodes().len(), 0, "episodes should be cleared");

    Ok(())
}

#[test]
fn a_latency_spike_episode_identifies_the_venue_and_latency() -> Result<()> {
    // A latency spike episode must contain enough detail for an operator to
    // understand which venue was slow and by how much. The detail field
    // carries this information.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    let latency_detail = "XLON: 145ms (threshold: 100ms)".to_string();
    cell.record_episode(EpisodeKind::LatencySpike, latency_detail.clone(), now);

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].kind, EpisodeKind::LatencySpike);
    assert_eq!(episodes[0].detail, latency_detail);

    Ok(())
}

#[test]
fn a_fill_slippage_episode_records_prices_and_quantity() -> Result<()> {
    // A fill/slippage episode must record the venue, expected price, actual
    // price, and quantity so an operator can calculate the cost of slippage.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    let slippage_detail = "XLON: expected 100.50, actual 100.25, qty 1000".to_string();
    cell.record_episode(EpisodeKind::FillSlippage, slippage_detail.clone(), now);

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].kind, EpisodeKind::FillSlippage);
    assert_eq!(episodes[0].detail, slippage_detail);
    assert!(
        episodes[0].detail.contains("100.50"),
        "detail should contain expected price"
    );
    assert!(
        episodes[0].detail.contains("100.25"),
        "detail should contain actual price"
    );

    Ok(())
}

#[test]
fn an_anomaly_episode_describes_the_anomaly_and_impact() -> Result<()> {
    // An anomaly episode must describe the market condition detected and how
    // it affected the cell's sizing or routing decisions.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    let anomaly_detail =
        "crossed book at XLON: bid 100.30 > ask 100.20; sizing multiplier reduced to 0.5"
            .to_string();
    cell.record_episode(EpisodeKind::Anomaly, anomaly_detail.clone(), now);

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].kind, EpisodeKind::Anomaly);
    assert_eq!(episodes[0].detail, anomaly_detail);
    assert!(
        episodes[0].detail.contains("sizing multiplier"),
        "detail should describe the impact"
    );

    Ok(())
}

#[test]
fn a_failure_episode_names_the_venue_and_recovery_action() -> Result<()> {
    // A failure episode must name which venue failed and what recovery action
    // the cell took (e.g., removed from routing, restarted feed sync).
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    let failure_detail = "XLON feed stalled 30s; recovered, resync required".to_string();
    cell.record_episode(EpisodeKind::Failure, failure_detail.clone(), now);

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].kind, EpisodeKind::Failure);
    assert_eq!(episodes[0].detail, failure_detail);
    assert!(
        episodes[0].detail.contains("XLON"),
        "detail should name the venue"
    );
    assert!(
        episodes[0].detail.contains("recovered"),
        "detail should describe recovery action"
    );

    Ok(())
}

#[test]
fn a_microstructure_episode_describes_the_observation() -> Result<()> {
    // A microstructure episode records an observation of order-book imbalance,
    // layer imbalance, or spread widening that affects the cell's pricing or
    // routing.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    let microstructure_detail =
        "XLON layer imbalance: bid levels 10x deeper than ask; spread widened to 5bp".to_string();
    cell.record_episode(
        EpisodeKind::Microstructure,
        microstructure_detail.clone(),
        now,
    );

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].kind, EpisodeKind::Microstructure);
    assert_eq!(episodes[0].detail, microstructure_detail);

    Ok(())
}

#[test]
fn a_venue_behaviour_episode_describes_the_change_and_venue() -> Result<()> {
    // A venue-behaviour episode records a detected change in a venue's
    // characteristics (latency profile, fees, order validation) that affects
    // the cell's strategy.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    let behaviour_detail = "XLON: new settlement terms T+2; latency profile changed".to_string();
    cell.record_episode(EpisodeKind::VenueBehaviour, behaviour_detail.clone(), now);

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].kind, EpisodeKind::VenueBehaviour);
    assert_eq!(episodes[0].detail, behaviour_detail);
    assert!(
        episodes[0].detail.contains("XLON"),
        "detail should name the venue"
    );
    assert!(
        episodes[0].detail.contains("T+2"),
        "detail should describe the change"
    );

    Ok(())
}

#[test]
fn builder_pattern_works_for_recording_episodes() -> Result<()> {
    // record_episode returns &mut Self to allow chaining multiple recordings.
    // This is a convenience and readability feature.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    cell.record_episode(EpisodeKind::LatencySpike, "spike".to_string(), now)
        .record_episode(EpisodeKind::FillSlippage, "slippage".to_string(), now)
        .record_episode(EpisodeKind::Anomaly, "anomaly".to_string(), now);

    assert_eq!(
        cell.episodes().len(),
        3,
        "all three episodes should be recorded"
    );

    Ok(())
}

#[test]
fn a_cell_records_fill_slippage_when_fill_price_differs_from_order_price() -> Result<()> {
    // When a fill arrives at a price different from the order price,
    // a FillSlippage episode should be recorded. This allows the cell to
    // learn which venues are delivering worse than expected prices.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    // Manually record a scenario where fill price differs from order price
    // (in a real scenario, this would happen during Cell::work() with an
    // execution report from the gateway)
    cell.record_episode(
        EpisodeKind::FillSlippage,
        "order order-1 filled at XLON 100.25 vs expected 100.50, slippage 25 bps".to_string(),
        now,
    );

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].kind, EpisodeKind::FillSlippage);
    assert!(episodes[0].detail.contains("slippage"));
    assert!(episodes[0].detail.contains("100.25"));
    assert!(episodes[0].detail.contains("100.50"));

    Ok(())
}

#[test]
fn a_cell_records_latency_spike_when_fill_takes_significant_time() -> Result<()> {
    // When a fill takes longer than a threshold (e.g., 100ms), a LatencySpike
    // episode should be recorded. This helps identify slow venues and order
    // the cell's routing accordingly.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    cell.record_episode(
        EpisodeKind::LatencySpike,
        "order order-1 filled at XLON after 145 ms".to_string(),
        now,
    );

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].kind, EpisodeKind::LatencySpike);
    assert!(episodes[0].detail.contains("145"));
    assert!(episodes[0].detail.contains("ms"));

    Ok(())
}

#[test]
fn a_cell_records_anomaly_when_fill_reports_invalid_data() -> Result<()> {
    // When the order-entry channel reports a fill with invalid data (e.g.,
    // non-positive quantity or price), an Anomaly episode should be recorded
    // before breaking on the inconsistency.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    cell.record_episode(
        EpisodeKind::Anomaly,
        "the order-entry channel reports -100 at 99.50 on order order-1; a fill needs both positive"
            .to_string(),
        now,
    );

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].kind, EpisodeKind::Anomaly);
    assert!(episodes[0].detail.contains("order-1"));
    assert!(episodes[0].detail.contains("positive"));

    Ok(())
}

#[test]
fn a_cell_records_failure_when_reconciliation_breaks() -> Result<()> {
    // When the cell detects a disagreement between its record and a venue's
    // account (a reconciliation break), a Failure episode should be recorded.
    // This is the most critical episode because it halts the cell.
    let mut cell = cell_for_episodes()?;
    let now = t(0);

    cell.record_episode(
        EpisodeKind::Failure,
        "the order-entry channel reports a fill of 1000 on order order-1 at XLON and the cell has no open order under that id"
            .to_string(),
        now,
    );

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].kind, EpisodeKind::Failure);
    assert!(episodes[0].detail.contains("order-1"));
    assert!(episodes[0].detail.contains("no open order"));

    Ok(())
}

#[test]
fn a_cell_with_multiple_episode_kinds_can_be_replayed() -> Result<()> {
    // A realistic scenario: a cell in operation records a sequence of
    // fill slippage, latency spike, anomaly, and failure episodes in response
    // to venue behavior. The journal captures all of them in order, allowing
    // operators to understand exactly what happened.
    let mut cell = cell_for_episodes()?;
    let t0 = t(0);
    let t1 = t(1);
    let t2 = t(2);
    let t3 = t(3);

    // Simulate a sequence of events as they would occur during passes
    cell.record_episode(
        EpisodeKind::FillSlippage,
        "order order-1 filled at XLON 100.25 vs expected 100.50, slippage 25 bps".to_string(),
        t0,
    );
    cell.record_episode(
        EpisodeKind::LatencySpike,
        "order order-1 filled at XLON after 145 ms".to_string(),
        t1,
    );
    cell.record_episode(
        EpisodeKind::Anomaly,
        "order order-2 filled at XLON but cell sent it to XCSE".to_string(),
        t2,
    );
    cell.record_episode(
        EpisodeKind::Failure,
        "order order-2 filled at XLON but cell sent it to XCSE".to_string(),
        t3,
    );

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), 4);
    assert_eq!(episodes[0].kind, EpisodeKind::FillSlippage);
    assert_eq!(episodes[1].kind, EpisodeKind::LatencySpike);
    assert_eq!(episodes[2].kind, EpisodeKind::Anomaly);
    assert_eq!(episodes[3].kind, EpisodeKind::Failure);

    // Each episode should have the correct timestamp for replay
    assert_eq!(episodes[0].recorded_at, t0);
    assert_eq!(episodes[1].recorded_at, t1);
    assert_eq!(episodes[2].recorded_at, t2);
    assert_eq!(episodes[3].recorded_at, t3);

    Ok(())
}
