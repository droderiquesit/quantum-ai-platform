//! The in-memory and file-backed adapters, exercised through every port.
//!
//! Two things are checked throughout: that each port does the job its access
//! pattern exists for, and that the file-backed adapter answers identically to
//! the in-memory one and survives a restart. A durable adapter that answered
//! differently would make a deployment's behaviour depend on its storage
//! configuration, which is exactly what a port is supposed to prevent.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::time::Stamped;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp, dec};
use qip_mesh::ports::{
    Aggregation, AnalyticalStore, ColumnFilter, Edge, GraphStore, HotSeries, Lakehouse, MasterData,
    Row,
};
use qip_mesh::{
    FileAnalytics, FileGraph, FileHotSeries, FileLakehouse, FileMasterData, MemoryAnalytics,
    MemoryGraph, MemoryHotSeries, MemoryLakehouse, MemoryMasterData,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch(label: &str) -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path =
        std::env::temp_dir().join(format!("qip-mesh-{label}-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn quote(symbol: &str, price: &str) -> Row {
    serde_json::json!({ "symbol": symbol, "price": price })
}

// --- lakehouse --------------------------------------------------------------

#[test]
fn the_lakehouse_time_travels_to_any_committed_version() -> Result<()> {
    let lakehouse = MemoryLakehouse::new();
    let first = lakehouse.append(
        "quotes",
        vec![Stamped::immediate(quote("VOD.L", "72.5"), t(10))],
        t(10),
    )?;
    let second = lakehouse.append(
        "quotes",
        vec![Stamped::immediate(quote("BP.L", "410.2"), t(20))],
        t(20),
    )?;

    assert_eq!(first.version, 1);
    assert_eq!(second.version, 2);
    assert_eq!(second.rows, 2);
    // Versions chain by digest, so a history edited afterwards is detectable
    // without keeping a second copy of it.
    assert_ne!(first.digest, second.digest);

    assert_eq!(lakehouse.snapshot("quotes", t(15))?.value().len(), 1);
    assert_eq!(lakehouse.snapshot("quotes", t(25))?.value().len(), 2);
    assert_eq!(lakehouse.versions("quotes", t(15))?.value().len(), 1);
    Ok(())
}

#[test]
fn an_empty_batch_does_not_create_a_version() -> Result<()> {
    // A version nobody can point at a change in is noise in the history that
    // somebody will later try to explain.
    let lakehouse = MemoryLakehouse::new();
    assert!(lakehouse.append("quotes", vec![], t(10)).is_err());
    assert!(lakehouse.tables(t(10))?.value().is_empty());
    Ok(())
}

#[test]
fn the_file_backed_lakehouse_survives_a_restart_and_answers_identically() -> Result<()> {
    let root = scratch("lakehouse");
    std::fs::create_dir_all(&root)?;
    let path = root.join("lakehouse.json");

    let memory = MemoryLakehouse::new();
    {
        let filed = FileLakehouse::open(&path)?;
        for store in [&memory as &dyn Lakehouse, &filed as &dyn Lakehouse] {
            store.append(
                "quotes",
                vec![Stamped::immediate(quote("VOD.L", "72.5"), t(10))],
                t(10),
            )?;
            store.append(
                "quotes",
                vec![Stamped::immediate(quote("BP.L", "410.2"), t(20))],
                t(20),
            )?;
        }
    }

    let reopened = FileLakehouse::open(&path)?;
    for as_of in [t(5), t(15), t(25)] {
        let from_memory = memory.snapshot("quotes", as_of).map(|s| s.value().clone());
        let from_file = reopened
            .snapshot("quotes", as_of)
            .map(|s| s.value().clone());
        assert_eq!(from_memory.is_ok(), from_file.is_ok());
        if let (Ok(a), Ok(b)) = (from_memory, from_file) {
            assert_eq!(a, b, "adapters disagree as of {as_of}");
        }
    }
    // Continuing the history after a restart picks up where it left off.
    let third = reopened.append(
        "quotes",
        vec![Stamped::immediate(quote("HSBA.L", "610.0"), t(30))],
        t(30),
    )?;
    assert_eq!(third.version, 3);

    std::fs::remove_dir_all(&root)?;
    Ok(())
}

// --- analytical -------------------------------------------------------------

#[test]
fn the_analytical_store_projects_filters_and_aggregates() -> Result<()> {
    let analytics = MemoryAnalytics::new();
    analytics.load(
        "fills",
        vec![
            Stamped::immediate(
                serde_json::json!({"venue": "XLON", "notional": "1000"}),
                t(10),
            ),
            Stamped::immediate(
                serde_json::json!({"venue": "XLON", "notional": "2000"}),
                t(11),
            ),
            Stamped::immediate(
                serde_json::json!({"venue": "XETR", "notional": "4000"}),
                t(12),
            ),
        ],
    )?;

    let london = ColumnFilter::Equals("venue".to_string(), serde_json::json!("XLON"));
    assert_eq!(*analytics.count("fills", &london, t(20))?.value(), 2);

    let projected = analytics.scan("fills", &["notional".to_string()], &london, t(20))?;
    assert_eq!(projected.value().len(), 2);
    assert!(
        projected
            .value()
            .iter()
            .all(|row| row.get("venue").is_none())
    );

    let total = analytics.aggregate("fills", "notional", Aggregation::Sum, &london, t(20))?;
    assert_eq!(*total.value(), Some(3000.0));

    // Nothing matched is `None`, not zero — a zero is an answer.
    let none = ColumnFilter::Equals("venue".to_string(), serde_json::json!("XNAS"));
    assert_eq!(
        *analytics
            .aggregate("fills", "notional", Aggregation::Sum, &none, t(20))?
            .value(),
        None
    );
    Ok(())
}

#[test]
fn a_filter_on_a_missing_column_never_matches() -> Result<()> {
    // Treating an absent column as zero would silently include rows nobody
    // recorded a value for, which is how a filtered scan quietly widens.
    let analytics = MemoryAnalytics::new();
    analytics.load(
        "fills",
        vec![
            Stamped::immediate(serde_json::json!({"venue": "XLON"}), t(10)),
            Stamped::immediate(
                serde_json::json!({"venue": "XETR", "notional": "10"}),
                t(10),
            ),
        ],
    )?;

    let at_least = ColumnFilter::AtLeast("notional".to_string(), dec!("0"));
    assert_eq!(*analytics.count("fills", &at_least, t(20))?.value(), 1);
    let present = ColumnFilter::Present("notional".to_string());
    assert_eq!(*analytics.count("fills", &present, t(20))?.value(), 1);
    Ok(())
}

#[test]
fn decimal_filters_compare_exactly_rather_than_through_a_float() -> Result<()> {
    // A price stored as a string round-trips exactly; comparing it through an
    // f64 would select a different set of rows than the same filter by hand.
    let analytics = MemoryAnalytics::new();
    analytics.load(
        "prices",
        vec![
            Stamped::immediate(serde_json::json!({"price": "0.100000001"}), t(10)),
            Stamped::immediate(serde_json::json!({"price": "0.099999999"}), t(10)),
        ],
    )?;
    let filter = ColumnFilter::AtLeast("price".to_string(), dec!("0.1"));
    assert_eq!(*analytics.count("prices", &filter, t(20))?.value(), 1);
    Ok(())
}

#[test]
fn the_file_backed_analytics_survives_a_restart() -> Result<()> {
    let root = scratch("analytics");
    std::fs::create_dir_all(&root)?;
    let path = root.join("analytics.json");
    {
        let filed = FileAnalytics::open(&path)?;
        filed.load(
            "fills",
            vec![Stamped::immediate(
                serde_json::json!({"venue": "XLON"}),
                t(10),
            )],
        )?;
    }
    let reopened = FileAnalytics::open(&path)?;
    assert_eq!(
        *reopened.count("fills", &ColumnFilter::Any, t(20))?.value(),
        1
    );
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

// --- hot series -------------------------------------------------------------

#[test]
fn the_hot_series_keeps_a_bounded_window_and_says_so() -> Result<()> {
    // The bound is what makes it hot. A read outside the window truthfully
    // returns nothing rather than a stale value; history lives in the
    // lakehouse, and a hot store that answered from beyond its window would
    // become a second, slowly diverging book of record.
    let hot = MemoryHotSeries::new(Duration::from_secs(60));
    assert_eq!(hot.retention(), Duration::from_secs(60));

    hot.record("VOD.L.mid", Stamped::immediate(dec!("72.5"), t(0)))?;
    hot.record("VOD.L.mid", Stamped::immediate(dec!("72.6"), t(30)))?;
    assert_eq!(
        hot.window("VOD.L.mid", Timestamp::EPOCH, t(30))?
            .value()
            .len(),
        2
    );

    // A point two minutes on evicts everything older than one minute.
    hot.record("VOD.L.mid", Stamped::immediate(dec!("72.9"), t(120)))?;
    let window = hot.window("VOD.L.mid", Timestamp::EPOCH, t(120))?;
    assert_eq!(window.value().len(), 1);
    assert_eq!(
        *hot.latest_as_of("VOD.L.mid", t(120))?.value(),
        Some(dec!("72.9"))
    );
    Ok(())
}

#[test]
fn the_hot_series_returns_the_latest_point_knowable_not_the_latest_stored() -> Result<()> {
    let hot = MemoryHotSeries::new(Duration::from_hours(1));
    hot.record("VOD.L.mid", Stamped::new(dec!("72.5"), t(10), t(10)))?;
    // Valid at second 20, but the platform only heard about it at second 40.
    hot.record("VOD.L.mid", Stamped::new(dec!("72.9"), t(20), t(40)))?;

    assert_eq!(
        *hot.latest_as_of("VOD.L.mid", t(30))?.value(),
        Some(dec!("72.5"))
    );
    assert_eq!(
        *hot.latest_as_of("VOD.L.mid", t(40))?.value(),
        Some(dec!("72.9"))
    );
    Ok(())
}

#[test]
fn the_file_backed_hot_series_survives_a_restart_with_its_retention() -> Result<()> {
    let root = scratch("hot");
    std::fs::create_dir_all(&root)?;
    let path = root.join("hot.json");
    {
        let filed = FileHotSeries::open(&path, Duration::from_secs(60))?;
        filed.record("VOD.L.mid", Stamped::immediate(dec!("72.5"), t(0)))?;
    }
    // The retention passed on reopening is the default for a *new* store; the
    // persisted one keeps what it was created with, so a restart cannot
    // silently widen or narrow the window.
    let reopened = FileHotSeries::open(&path, Duration::from_hours(24))?;
    assert_eq!(reopened.retention(), Duration::from_secs(60));
    assert_eq!(
        *reopened.latest_as_of("VOD.L.mid", t(10))?.value(),
        Some(dec!("72.5"))
    );
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

// --- master data ------------------------------------------------------------

#[test]
fn master_data_returns_the_definition_in_force_not_the_current_one() -> Result<()> {
    // The reason this store is bitemporal rather than a map: a decision taken
    // in March must be re-readable against March's instrument definition.
    let master = MemoryMasterData::new();
    master.upsert(
        "instrument",
        "VOD.L",
        Stamped::immediate(serde_json::json!({"lot_size": 1}), t(0)),
    )?;
    master.upsert(
        "instrument",
        "VOD.L",
        Stamped::immediate(serde_json::json!({"lot_size": 100}), t(100)),
    )?;

    let march = master.lookup("instrument", "VOD.L", t(50))?;
    assert_eq!(
        march.value().as_ref().and_then(|r| r.get("lot_size")),
        Some(&serde_json::json!(1))
    );

    let today = master.lookup("instrument", "VOD.L", t(200))?;
    assert_eq!(
        today.value().as_ref().and_then(|r| r.get("lot_size")),
        Some(&serde_json::json!(100))
    );

    assert_eq!(
        master.history("instrument", "VOD.L", t(200))?.value().len(),
        2
    );
    assert_eq!(
        master.history("instrument", "VOD.L", t(50))?.value().len(),
        1
    );
    Ok(())
}

#[test]
fn an_unknown_key_in_a_known_entity_is_absent_rather_than_an_error() -> Result<()> {
    let master = MemoryMasterData::new();
    master.upsert(
        "instrument",
        "VOD.L",
        Stamped::immediate(serde_json::json!({"lot_size": 1}), t(0)),
    )?;
    assert!(
        master
            .lookup("instrument", "NOT.A.THING", t(10))?
            .value()
            .is_none()
    );
    assert!(master.lookup("counterparty", "VOD.L", t(10)).is_err());
    Ok(())
}

#[test]
fn the_file_backed_master_survives_a_restart() -> Result<()> {
    let root = scratch("master");
    std::fs::create_dir_all(&root)?;
    let path = root.join("master.json");
    {
        let filed = FileMasterData::open(&path)?;
        filed.upsert(
            "instrument",
            "VOD.L",
            Stamped::immediate(serde_json::json!({"lot_size": 1}), t(0)),
        )?;
    }
    let reopened = FileMasterData::open(&path)?;
    assert_eq!(reopened.list("instrument", t(10))?.value().len(), 1);
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

// --- graph ------------------------------------------------------------------

#[test]
fn the_graph_walks_relationships_by_kind_and_terminates_on_a_cycle() -> Result<()> {
    let graph = MemoryGraph::new();
    graph.add_edge(Stamped::immediate(
        Edge::new("VOD", "VOD.L", "issues"),
        t(0),
    ))?;
    graph.add_edge(Stamped::immediate(
        Edge::new("VOD.L", "VOD.L.CALL", "pays_off_from"),
        t(0),
    ))?;
    graph.add_edge(Stamped::immediate(
        Edge::new("VOD", "VOD.SUB", "owns"),
        t(0),
    ))?;
    // An issuer that guarantees its own subsidiary: a genuine cycle.
    graph.add_edge(Stamped::immediate(
        Edge::new("VOD.SUB", "VOD", "guarantees"),
        t(0),
    ))?;

    assert_eq!(graph.neighbours("VOD", None, t(10))?.value().len(), 2);
    assert_eq!(
        graph
            .neighbours("VOD", Some("issues"), t(10))?
            .value()
            .len(),
        1
    );

    let reachable = graph.reachable("VOD", None, 10, t(10))?;
    assert_eq!(
        reachable.value(),
        &[
            "VOD.L".to_string(),
            "VOD.L.CALL".to_string(),
            "VOD.SUB".to_string()
        ]
    );

    // Depth bounds the walk.
    assert_eq!(graph.reachable("VOD", None, 1, t(10))?.value().len(), 2);
    Ok(())
}

#[test]
fn an_edge_with_no_kind_is_refused() -> Result<()> {
    // An untyped edge cannot be traversed selectively, so every traversal
    // becomes a whole-graph walk and the payoff graph stops being useful.
    let graph = MemoryGraph::new();
    assert!(
        graph
            .add_edge(Stamped::immediate(Edge::new("a", "b", "  "), t(0)))
            .is_err()
    );
    assert!(
        graph
            .add_edge(Stamped::immediate(Edge::new("", "b", "owns"), t(0)))
            .is_err()
    );
    Ok(())
}

#[test]
fn an_edges_weight_stays_exact() -> Result<()> {
    // Payoff weights are quantities, not statistics.
    let graph = MemoryGraph::new();
    graph.add_edge(Stamped::immediate(
        Edge::new("VOD.L", "VOD.L.CALL", "pays_off_from").weighing(dec!("0.333333333")),
        t(0),
    ))?;
    let edges = graph.neighbours("VOD.L", None, t(10))?;
    assert_eq!(edges.value()[0].weight, Some(dec!("0.333333333")));
    Ok(())
}

#[test]
fn the_file_backed_graph_survives_a_restart() -> Result<()> {
    let root = scratch("graph");
    std::fs::create_dir_all(&root)?;
    let path = root.join("graph.json");
    {
        let filed = FileGraph::open(&path)?;
        filed.add_edge(Stamped::immediate(
            Edge::new("VOD", "VOD.L", "issues"),
            t(0),
        ))?;
    }
    let reopened = FileGraph::open(&path)?;
    assert_eq!(reopened.nodes(t(10))?.value().len(), 2);
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn a_write_that_cannot_be_persisted_leaves_the_store_unchanged() -> Result<()> {
    // The file-backed adapter applies a write to a copy, flushes it, and only
    // then commits, so a store and its file never disagree about what
    // happened. Here the flush cannot succeed because the path's parent has
    // been removed underneath it.
    let root = scratch("unflushable");
    std::fs::create_dir_all(&root)?;
    let path = root.join("graph.json");
    let filed = FileGraph::open(&path)?;
    filed.add_edge(Stamped::immediate(Edge::new("a", "b", "owns"), t(0)))?;
    std::fs::remove_dir_all(&root)?;

    assert!(
        filed
            .add_edge(Stamped::immediate(Edge::new("b", "c", "owns"), t(0)))
            .is_err()
    );
    // The failed write is not visible, so a caller that retried would not
    // double-apply it.
    assert_eq!(filed.nodes(t(10))?.value().len(), 2);
    Ok(())
}
