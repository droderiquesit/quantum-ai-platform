//! The edge cell node's composable parts.
//!
//! `main.rs` is the process: it reads the environment, refuses to start
//! without what a cell needs, and serves the health surface. This library is
//! what that process is assembled *from*, exposed so the pieces can be tested
//! against the same types the binary uses rather than against a copy of them.
//!
//! Every module is a seam: the venue seam, where the cell's orders meet a
//! matching engine; the *selection* of that venue, which is the one decision
//! in this binary whose wrong answer is not recoverable; the feed and the
//! pass, where the venue's depth reaches the cell and the cell's decision
//! reaches the venue; the durability seam, where the cell's decision record
//! leaves the process; and the mesh seam, where the cell's state reaches the
//! central plane and the central plane's signed capital reaches the cell.
//! Each is somewhere the node
//! could look healthy while doing nothing — or, in the venue's case, while
//! doing something nobody asked for — so each is exercised against the types
//! the binary actually uses.

/// The ceiling on the capital the whole cell may commit, as the deployment
/// states it; the centre's share funds the cell up to it.
pub mod allocation;
/// The arbitrage desk, built from the payload's whitelist once capital arrives.
pub mod arbitrage;
/// The simulated venue's quote feed, and the one value it may be configured as.
pub mod feed;
pub mod gateway;
/// The second halt wire: a flag on the node's own filesystem, polled.
pub mod halt;
pub mod mesh;
pub mod mirror;
/// One pass of the node: feed, decide, act, reconcile.
pub mod pass;
/// Cancel-and-replace of a stale resting order, beneath the cell's placer
/// seam — the caller `qip_routing::reprice` was written for.
pub mod reprice;
/// The cell's share of its region's grant, as the health body reports it.
pub mod share;
/// The strategies the payload's plan names, deployed under their grants.
pub mod strategies;
/// The node's own metric seam: the mesh link, rendered as a series.
pub mod telemetry;
pub mod venue;

use allocation::RegionCapital;
use qip_core::Clock;
use qip_core::error::Result;
use qip_edge::cell::{Cell, CellConfig};
use qip_feature_dag::engine::FeatureEngine;
use qip_observability::Telemetry;
use qip_observability::metrics::Metrics;
use std::sync::Arc;
use telemetry::MeshSeries;

/// Everything in this node that records a metric, and the one registry all
/// of it records into.
///
/// Built by [`assemble`] rather than piecewise in `main.rs`, so that the
/// property `main.rs` cannot test — that the cell, the mesh series and the
/// scrape surface share a registry — is held by a function a test can call.
/// The registry the scrape serves is [`Self::scrape_registry`], and it is the
/// telemetry's own; the cell and the series were handed that same `Arc`, and
/// a test proves it by pointer identity rather than by reading the source.
#[derive(Debug)]
pub struct NodeAssembly {
    /// The process's telemetry. Only the metric half has a consumer at the
    /// edge; the tracer and the logger reach nothing in this node, and that
    /// is named rather than hidden behind a narrower constructor.
    pub telemetry: Telemetry,
    pub cell: Cell,
    pub mesh_series: MeshSeries,
}

impl NodeAssembly {
    /// The registry a scrape reads. Returned from the telemetry, not from a
    /// handle taken elsewhere, so that a second registry introduced anywhere
    /// in [`assemble`] shows up as a pointer that no longer matches.
    pub fn scrape_registry(&self) -> &Arc<Metrics> {
        &self.telemetry.metrics
    }
}

/// Wire the cell and the mesh series into the one registry the scrape serves,
/// under the one bound on what the whole cell may commit.
///
/// The defect this exists to make testable is one nothing at runtime would
/// report: a second `Metrics` built for the health thread. The cell would
/// record diligently into one registry while every scrape answered empty from
/// another, forever — the exact gap the edge's observability seam closes,
/// rebuilt one level up. `qip-fastbrain` and `qip-deepbrain` take their
/// registry handle the same way, before the telemetry is used anywhere else.
///
/// The region ceiling is a parameter rather than a later builder call for
/// the same reason: `Cell::with_region_allocation` existed, was tested, and
/// no composition root called it, so every node this binary built ran with
/// no bound on the sum of its strategies. Taking a [`RegionCapital`] here —
/// a type only [`RegionCapital::read`] can produce — means a node cannot be
/// assembled without an amount the deployment stated and this crate checked.
///
/// The table opens **unfunded** under that amount (ADR 0039, option (a)):
/// the cell places nothing until a verified policy payload's grant manifest
/// names grants it holds, and the operator's amount is the most any share
/// will ever fund it to. Opened funded, as every node was before, two nodes
/// under one region's grant held two operator-typed amounts nothing summed,
/// and could together spend the grant twice before the centre's first
/// payload reached either — the double-spend window traceability F6 called
/// operator discipline. Opened unfunded, a node that has never been granted
/// to has nothing in advance, which is ADR 0008 read strictly; its health
/// body says so, and the cost — a fresh node sends nothing until the centre
/// ships it a share — is the ADR's stated price.
pub fn assemble(
    config: CellConfig,
    features: FeatureEngine,
    clock: Arc<dyn Clock>,
    allocation: RegionCapital,
) -> Result<NodeAssembly> {
    let cell_id = config.cell_id.clone();
    let region = config.region.clone();
    let telemetry = Telemetry::new("qip-edge-node", clock);
    let metrics: Arc<Metrics> = Arc::clone(&telemetry.metrics);
    let cell = Cell::new(config, features)?
        .with_metrics(Arc::clone(&metrics))
        .with_unfunded_region(allocation.amount())?;
    let mesh_series = MeshSeries::new(metrics, &cell_id, &region);
    Ok(NodeAssembly {
        telemetry,
        cell,
        mesh_series,
    })
}
