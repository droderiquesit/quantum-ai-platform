//! Control 6 — kill switches and incident response.
//!
//! The asymmetry under test: anything may stop the platform, only a named
//! operator with a fresh credential may start it again, and the record says
//! who and why.

#![allow(clippy::panic_in_result_fn)]

use qip_compliance::approval::OperatorCredential;
use qip_compliance::incident::{HaltScope, Incident, IncidentLog, ResponsePolicy};
use qip_contracts::governance::Severity;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn operator(age: Duration) -> Result<OperatorCredential> {
    OperatorCredential::verified("a.duarte", "hardware-token", now().saturating_sub(age))
}

fn incident(
    id: &str,
    severity: Severity,
    scope: Option<&str>,
    cell: Option<&str>,
) -> Result<Incident> {
    Incident::new(
        id,
        now(),
        severity,
        "pretrade-checker",
        "order rate exceeded the venue's published limit",
        scope.map(str::to_string),
        cell.map(str::to_string),
    )
}

#[test]
fn severity_maps_to_the_right_halt_scope() -> Result<()> {
    let mut log = IncidentLog::new(ResponsePolicy::standard());

    // An observation is recorded and nothing stops.
    let response = log.record(incident("i-1", Severity::Observation, None, None)?);
    assert_eq!(response, HaltScope::Nothing);
    assert!(!response.halts_something());
    assert!(!log.is_halted("stat-arb-eu", "frankfurt-1"));

    // A scoped incident stops that strategy wherever it runs.
    let response = log.record(incident("i-2", Severity::Scoped, Some("stat-arb-eu"), None)?);
    assert_eq!(response, HaltScope::Scope("stat-arb-eu".to_string()));
    assert!(log.is_halted("stat-arb-eu", "frankfurt-1"));
    assert!(log.is_halted("stat-arb-eu", "london-1"));
    assert!(!log.is_halted("momentum-us", "london-1"));

    // A cell incident stops everything on that cell.
    let response = log.record(incident("i-3", Severity::Cell, None, Some("london-1"))?);
    assert_eq!(response, HaltScope::Cell("london-1".to_string()));
    assert!(log.is_halted("momentum-us", "london-1"));
    assert!(!log.is_halted("momentum-us", "tokyo-1"));

    // A global incident stops everything.
    let response = log.record(incident("i-4", Severity::Global, None, None)?);
    assert_eq!(response, HaltScope::Everything);
    assert!(log.is_globally_halted());
    assert!(log.is_halted("anything", "anywhere"));
    Ok(())
}

#[test]
fn tripping_a_halt_requires_no_authority_at_all() -> Result<()> {
    // The cost of a false stop is a day of missed opportunity; the cost of a
    // missed one is the book. Nothing here takes a credential.
    let mut log = IncidentLog::new(ResponsePolicy::standard());
    log.record(incident("i-1", Severity::Global, None, None)?);
    assert!(log.is_globally_halted());
    assert_eq!(log.incidents().len(), 1);
    Ok(())
}

#[test]
fn an_incident_whose_severity_names_a_target_it_does_not_carry_is_refused() {
    // Otherwise the response mapping has a case with nothing to halt, and the
    // safe reading of that case and the convenient one are very far apart.
    assert!(incident("i-1", Severity::Scoped, None, None).is_err());
    assert!(incident("i-2", Severity::Cell, None, None).is_err());
    assert!(incident("i-3", Severity::Cell, Some("a-scope"), None).is_err());
    // Global and observation need neither.
    assert!(incident("i-4", Severity::Global, None, None).is_ok());
    assert!(incident("i-5", Severity::Observation, None, None).is_ok());
}

#[test]
fn clearing_without_a_fresh_credential_is_refused() -> Result<()> {
    let mut log = IncidentLog::new(ResponsePolicy::standard());
    log.record(incident("i-1", Severity::Global, None, None)?);

    // Sixteen minutes: one past the window the risk engine uses.
    let error = log
        .clear_global(
            &operator(Duration::from_mins(16))?,
            now(),
            "the venue confirmed the limit was raised",
        )
        .expect_err("a stale credential must not lift a halt");
    assert!(error.message().contains("a.duarte"));
    assert!(error.message().contains("stale"));
    assert!(log.is_globally_halted());

    // Fresh, and it lifts.
    log.clear_global(
        &operator(Duration::from_mins(2))?,
        now(),
        "the venue confirmed the limit was raised",
    )?;
    assert!(!log.is_globally_halted());
    Ok(())
}

#[test]
fn clearing_without_a_stated_reason_is_refused() -> Result<()> {
    // The record of why somebody decided it was safe to continue is the point
    // of the control; the halt reason alone only says why it stopped.
    let mut log = IncidentLog::new(ResponsePolicy::standard());
    log.record(incident("i-1", Severity::Global, None, None)?);

    let error = log
        .clear_global(&operator(Duration::from_mins(1))?, now(), "ok")
        .expect_err("a clearance must state a reason");
    assert!(error.message().contains("stated reason"));
    assert!(log.is_globally_halted());
    Ok(())
}

#[test]
fn every_clearance_is_recorded_with_who_why_and_what_it_lifted() -> Result<()> {
    let mut log = IncidentLog::new(ResponsePolicy::standard());
    log.record(incident("i-1", Severity::Scoped, Some("stat-arb-eu"), None)?);
    log.record(incident("i-2", Severity::Cell, None, Some("london-1"))?);

    log.clear_scope(
        "stat-arb-eu",
        &operator(Duration::from_mins(1))?,
        now(),
        "the venue limit was raised and the strategy re-tested in shadow",
    )?;
    log.clear_cell(
        "london-1",
        &operator(Duration::from_mins(1))?,
        now(),
        "the failed host was replaced and the cell restarted clean",
    )?;

    assert_eq!(log.clearances().len(), 2);
    for clearance in log.clearances() {
        assert_eq!(clearance.operator, "a.duarte");
        assert_eq!(clearance.method, "hardware-token");
        assert!(clearance.reason.len() >= 10);
        // Both halves: why it stopped and who decided it was safe to continue.
        assert!(!clearance.cleared.summary().is_empty());
    }
    assert!(log.halted_scopes().is_empty());
    assert!(log.halted_cells().is_empty());
    Ok(())
}

#[test]
fn clearing_a_halt_that_is_not_in_force_is_idempotent_and_records_nothing() -> Result<()> {
    // The case an operator retrying a request hits. It is not an error, and it
    // must not leave a clearance record for a halt that never existed.
    let mut log = IncidentLog::new(ResponsePolicy::standard());
    log.clear_global(
        &operator(Duration::from_mins(1))?,
        now(),
        "nothing was halted but the button was pressed",
    )?;
    assert!(log.clearances().is_empty());
    Ok(())
}

#[test]
fn a_later_incident_does_not_overwrite_the_reason_the_platform_stopped() -> Result<()> {
    // The first reason is the one an incident review needs; the cascade of
    // consequences that followed is noise by comparison.
    let mut log = IncidentLog::new(ResponsePolicy::standard());
    log.record(Incident::new(
        "i-1",
        now(),
        Severity::Global,
        "risk-monitor",
        "portfolio breached its drawdown limit",
        None,
        None,
    )?);
    log.record(Incident::new(
        "i-2",
        now().saturating_add(Duration::from_secs(30)),
        Severity::Global,
        "execution-engine",
        "every venue connection dropped after the halt",
        None,
        None,
    )?);

    let halt = log
        .global_halt()
        .ok_or_else(|| qip_core::error::Error::not_found("global halt"))?;
    assert_eq!(halt.incident.id(), "i-1");
    assert_eq!(log.incidents().len(), 2);
    Ok(())
}

#[test]
fn a_raised_floor_can_only_widen_a_response_never_narrow_one() -> Result<()> {
    // A policy that could soften the response to a global incident would be a
    // way to disable the control from a config file.
    let policy = ResponsePolicy::with_floor(Severity::Cell);
    let mut log = IncidentLog::new(policy);

    // An observation is treated as a cell halt because the floor says so.
    let response = log.record(Incident::new(
        "i-1",
        now(),
        Severity::Observation,
        "monitor",
        "a metric drifted slightly out of band",
        None,
        Some("frankfurt-1".to_string()),
    )?);
    assert_eq!(response, HaltScope::Cell("frankfurt-1".to_string()));

    // A global incident is still global; the floor cannot pull it down.
    let response = log.record(incident("i-2", Severity::Global, None, None)?);
    assert_eq!(response, HaltScope::Everything);
    Ok(())
}

#[test]
fn a_raised_floor_with_no_target_to_halt_widens_rather_than_does_nothing() -> Result<()> {
    // Halting more than necessary is the safe direction. Halting nothing
    // because the incident named no cell is not.
    let mut log = IncidentLog::new(ResponsePolicy::with_floor(Severity::Cell));
    let response = log.record(incident("i-1", Severity::Observation, None, None)?);
    assert_eq!(response, HaltScope::Everything);
    Ok(())
}
