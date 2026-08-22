//! The property the whole crate exists for: no port returns a value the
//! platform could not have known yet.
//!
//! Rust cannot assert the *absence* of a `latest(&self, key)` method from a
//! trait, so the surface is held by review and by the module documentation.
//! What is asserted here is the behaviour that absence exists to guarantee:
//! every read method on every port, given an as-of before the write became
//! known, returns nothing.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::time::Stamped;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp, dec};
use qip_mesh::ports::{
    Aggregation, AnalyticalStore, ColumnFilter, Edge, EvidenceStore, GraphStore, HotSeries,
    Lakehouse, MasterData, Row,
};
use qip_mesh::{
    MemoryAnalytics, MemoryEvidence, MemoryGraph, MemoryHotSeries, MemoryLakehouse,
    MemoryMasterData,
};

/// The moment every read below claims to be reasoning as of.
fn as_of() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// Ten seconds after the as-of: when the platform actually learned each fact.
fn later() -> Timestamp {
    as_of().saturating_add(Duration::from_secs(10))
}

fn row(symbol: &str, price: &str) -> Row {
    serde_json::json!({ "symbol": symbol, "price": price })
}

#[test]
fn the_lakehouse_shows_nothing_committed_after_the_as_of() -> Result<()> {
    let lakehouse = MemoryLakehouse::new();
    lakehouse.append(
        "quotes",
        vec![Stamped::new(row("VOD.L", "72.5"), as_of(), later())],
        later(),
    )?;

    // A table whose first version was committed later did not exist then, and
    // says so identically to a table that never existed — otherwise the error
    // message itself would tell a caller the future.
    assert!(lakehouse.snapshot("quotes", as_of()).is_err());
    assert!(lakehouse.versions("quotes", as_of()).is_err());
    assert!(lakehouse.tables(as_of())?.value().is_empty());

    // The same reads a moment later see everything.
    assert_eq!(lakehouse.snapshot("quotes", later())?.value().len(), 1);
    assert_eq!(lakehouse.versions("quotes", later())?.value().len(), 1);
    assert_eq!(lakehouse.tables(later())?.value().len(), 1);
    Ok(())
}

#[test]
fn the_analytical_store_scans_nothing_loaded_after_the_as_of() -> Result<()> {
    let analytics = MemoryAnalytics::new();
    analytics.load(
        "fills",
        vec![Stamped::new(row("VOD.L", "72.5"), as_of(), later())],
    )?;

    assert!(
        analytics
            .scan("fills", &[], &ColumnFilter::Any, as_of())
            .is_err()
    );
    assert!(
        analytics
            .count("fills", &ColumnFilter::Any, as_of())
            .is_err()
    );
    assert!(
        analytics
            .aggregate(
                "fills",
                "price",
                Aggregation::Sum,
                &ColumnFilter::Any,
                as_of()
            )
            .is_err()
    );
    assert!(analytics.datasets(as_of())?.value().is_empty());

    assert_eq!(
        analytics
            .scan("fills", &[], &ColumnFilter::Any, later())?
            .value()
            .len(),
        1
    );
    assert_eq!(
        *analytics
            .count("fills", &ColumnFilter::Any, later())?
            .value(),
        1
    );
    Ok(())
}

#[test]
fn the_hot_series_returns_no_point_recorded_after_the_as_of() -> Result<()> {
    let hot = MemoryHotSeries::new(Duration::from_hours(1));
    hot.record("VOD.L.mid", Stamped::new(dec!("72.5"), as_of(), later()))?;

    assert!(hot.latest_as_of("VOD.L.mid", as_of()).is_err());
    assert!(hot.window("VOD.L.mid", Timestamp::EPOCH, as_of()).is_err());
    assert!(hot.series(as_of())?.value().is_empty());

    assert_eq!(
        *hot.latest_as_of("VOD.L.mid", later())?.value(),
        Some(dec!("72.5"))
    );
    Ok(())
}

#[test]
fn master_data_returns_no_version_recorded_after_the_as_of() -> Result<()> {
    let master = MemoryMasterData::new();
    master.upsert(
        "instrument",
        "VOD.L",
        Stamped::new(row("VOD.L", "72.5"), as_of(), later()),
    )?;

    assert!(master.lookup("instrument", "VOD.L", as_of()).is_err());
    assert!(master.list("instrument", as_of()).is_err());
    assert!(master.history("instrument", "VOD.L", as_of()).is_err());
    assert!(master.entities(as_of())?.value().is_empty());

    assert!(
        master
            .lookup("instrument", "VOD.L", later())?
            .value()
            .is_some()
    );
    Ok(())
}

#[test]
fn the_graph_traverses_no_edge_recorded_after_the_as_of() -> Result<()> {
    let graph = MemoryGraph::new();
    graph.add_edge(Stamped::new(
        Edge::new("VOD", "VOD.L", "issues"),
        as_of(),
        later(),
    ))?;

    assert!(graph.neighbours("VOD", None, as_of())?.value().is_empty());
    assert!(graph.reachable("VOD", None, 3, as_of())?.value().is_empty());
    assert!(graph.nodes(as_of())?.value().is_empty());

    assert_eq!(graph.neighbours("VOD", None, later())?.value().len(), 1);
    assert_eq!(graph.reachable("VOD", None, 3, later())?.value().len(), 1);
    Ok(())
}

#[test]
fn the_evidence_store_returns_nothing_written_after_the_as_of() -> Result<()> {
    let evidence = MemoryEvidence::new();
    evidence.put(
        "decisions/2026-08-22/d-1",
        b"the decision".to_vec(),
        later(),
    )?;

    assert!(
        evidence
            .get("decisions/2026-08-22/d-1", as_of())?
            .value()
            .is_none()
    );
    assert!(
        evidence
            .receipt("decisions/2026-08-22/d-1", as_of())?
            .value()
            .is_none()
    );
    assert!(evidence.keys("decisions/", as_of())?.value().is_empty());

    assert!(
        evidence
            .get("decisions/2026-08-22/d-1", later())?
            .value()
            .is_some()
    );
    assert_eq!(evidence.keys("decisions/", later())?.value().len(), 1);
    Ok(())
}

#[test]
fn an_answer_carries_the_known_time_it_is_current_as_of() -> Result<()> {
    // A read does not only filter; it says how current its answer is, so a
    // caller can tell "nothing happened" from "nothing has arrived yet".
    let lakehouse = MemoryLakehouse::new();
    let first = as_of();
    let second = as_of().saturating_add(Duration::from_secs(5));
    lakehouse.append(
        "quotes",
        vec![Stamped::new(row("VOD.L", "72.5"), first, first)],
        first,
    )?;
    lakehouse.append(
        "quotes",
        vec![Stamped::new(row("VOD.L", "72.6"), second, second)],
        second,
    )?;

    let early = lakehouse.snapshot("quotes", first)?;
    assert_eq!(early.value().len(), 1);
    assert_eq!(early.known_at(), first);

    let late = lakehouse.snapshot("quotes", later())?;
    assert_eq!(late.value().len(), 2);
    assert_eq!(late.known_at(), second);
    Ok(())
}

#[test]
fn an_empty_answer_is_stamped_at_the_as_of_rather_than_at_nothing() -> Result<()> {
    // "Nothing was knowable then" is a fact about the read. Stamping it
    // earlier would understate how much the caller is missing.
    let graph = MemoryGraph::new();
    let answer = graph.nodes(as_of())?;
    assert!(answer.value().is_empty());
    assert_eq!(answer.known_at(), as_of());
    Ok(())
}

#[test]
fn a_lakehouse_batch_cannot_commit_a_fact_the_platform_did_not_yet_know() -> Result<()> {
    // Otherwise a time-travel read by version and the same read by row would
    // disagree, and only one of them would be right.
    let lakehouse = MemoryLakehouse::new();
    let error = lakehouse
        .append(
            "quotes",
            vec![Stamped::new(row("VOD.L", "72.5"), as_of(), later())],
            as_of(),
        )
        .expect_err("a batch cannot carry a row from the future");
    assert!(error.message().contains("cannot be committed"));
    Ok(())
}

#[test]
fn a_tables_history_cannot_go_backwards() -> Result<()> {
    // A version committed before its predecessor is a clock bug, and accepting
    // it makes every subsequent time-travel read ambiguous.
    let lakehouse = MemoryLakehouse::new();
    lakehouse.append(
        "quotes",
        vec![Stamped::new(row("VOD.L", "72.5"), later(), later())],
        later(),
    )?;
    let error = lakehouse
        .append(
            "quotes",
            vec![Stamped::new(row("VOD.L", "72.6"), as_of(), as_of())],
            as_of(),
        )
        .expect_err("a table's history cannot go backwards");
    assert!(error.message().contains("cannot go backwards"));
    Ok(())
}
