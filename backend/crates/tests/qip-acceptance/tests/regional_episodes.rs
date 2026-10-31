//! A regional episode, from the cell that classified it to the centre's store
//! (R12-REFLEX-6012, ADR 0207).
//!
//! ```text
//! Cell::confirm_execution_reports (an anomaly the cell cannot book)
//!   -> Cell::record_episode (bounded ring)
//!   -> Cell::state_delta -> CellStateDelta::to_frame (the mesh wire)
//!   -> qip_mesh::delta::decode_cell_delta (the centre's codec)
//!   -> CellReport::with_episodes -> CentralPlane::ingest (bounded per-cell store)
//! ```
//!
//! Every hop is the production type. What is not reached is the socket and
//! the API's sink, which `qip-api/tests/mesh.rs` covers over a real listener;
//! the overlap is the decoded interval. Until ADR 0207 the cell's ring was
//! read by nothing off the cell: the centre could ask a region what it held
//! and never what it had seen go wrong.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::message::BookSide;
use qip_contracts::regional_episode::EpisodeKind;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, Placer, WorkReport};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_kernel::central::CentralConfig;
use qip_kernel::{CellReport, CentralPlane};
use qip_risk_engine::autonomy::KillSwitch;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

/// A venue channel that reports exactly what the test tells it to.
#[derive(Debug, Default)]
struct ReportingGateway {
    reports: Vec<ExecutionReport>,
}

impl Placer for ReportingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.reports)
    }
}

/// Mutations (each run, each failed here, each restored byte for byte):
/// * `Cell::state_delta` shipping `episodes: Vec::new()` in place of the
///   ring — the decoded interval carries none and the premise on the wire
///   fails;
/// * `CentralPlane::ingest` not calling `store_episodes` — the store stays
///   empty and the final assertion fails.
#[test]
fn an_episode_recorded_at_a_cell_reaches_the_central_store() -> Result<()> {
    let config = CellConfig::new(CELL, REGION).with_venue(VenueId::new(VENUE));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    assert!(
        cell.episodes().is_empty(),
        "premise: a fresh cell has seen nothing"
    );

    // The venue's channel reports a fill on an order the cell never sent:
    // an anomaly the cell classifies and records, then halts on.
    let mut gateway = ReportingGateway::default();
    gateway.reports.push(ExecutionReport {
        order_id: "london-1-9".to_string(),
        venue: VenueId::new(VENUE),
        quantity: Decimal::from_int(10),
        price: Decimal::from_int(100),
        at: t(9),
    });
    let confirmed = cell.confirm_execution_reports(&mut gateway, t(10));
    assert!(confirmed.is_empty(), "a fill on an unsent order was booked");
    let recorded = cell.episodes().to_vec();
    assert!(
        recorded
            .iter()
            .any(|episode| episode.kind == EpisodeKind::Anomaly),
        "premise: the cell did not classify the report as an anomaly: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .all(|e| e.cell == CELL && e.region == REGION)
    );

    // The delta, as the node publishes it, through the wire codec.
    let delta = cell.state_delta(&WorkReport::default(), t(10));
    assert_eq!(delta.episodes, recorded, "the delta is not the cell's view");
    let frame = delta.to_frame()?;
    let decoded = qip_mesh::delta::decode_cell_delta(&frame)?;
    assert_eq!(
        decoded.interval.episodes, recorded,
        "the episodes did not survive the wire"
    );
    assert_eq!(decoded.interval.episodes_omitted, 0);

    // The centre, as the API's sink builds the report.
    let mut plane = CentralPlane::new(&[7u8; 32], CentralConfig::default())?;
    let mut kill_switch = KillSwitch::new();
    assert!(
        plane.episodes(CELL).is_empty(),
        "premise: the store is empty"
    );
    let report = CellReport::new(decoded.standing.cell.clone(), decoded.standing.at)
        .with_region(decoded.standing.region.clone())
        .with_episodes(decoded.interval.episodes.clone());
    plane.ingest(report, &mut kill_switch, t(11))?;
    assert_eq!(
        plane.episodes(CELL),
        &recorded[..],
        "the episode the cell recorded is not what the centre holds"
    );
    Ok(())
}
