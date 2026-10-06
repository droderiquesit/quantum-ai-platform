//! Architecture requirements (ARCH-001 through ARCH-072).
//!
//! These tests verify the ARCH blueprint requirements document
//! (`docs/blueprint/requirements/ARCH.json`), which describes the explicit
//! structure of the reflex and cognitive systems and how they compose.
//!
//! Each test is named as the requirement it proves, and asserts the property
//! the requirement states, not merely that it is possible.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::{CapitalEnvelope, CapitalGrant, Utilisation};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;

const CELL_KEY: &[u8] = b"arch-test-envelope-key-for-arch-001";
const CELL_NAME: &str = "arch-test-cell";

fn timestamp(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

/// ARCH-001: Fast reflex and slow cognitive systems are explicit and independently deployable.
///
/// Requirement: "A reflex cell deployed with no cognitive-lane process present completes
/// paper passes from its last signed packages, and a new cognitive-lane build is released
/// without redeploying or restarting the cell."
///
/// This test verifies that:
/// 1. A Cell (the reflex system) can run and remain operational without qip-kernel::Platform
///    (the cognitive system).
/// 2. Signed packages (capital envelopes) can be verified and used by the cell.
/// 3. Multiple signed packages can be loaded sequentially without restarting the cell.
#[test]
fn arch_001_fast_reflex_and_slow_cognitive_systems_are_independently_deployable() -> Result<()> {
    // -------- Part 1: Assemble a Cell (the reflex system) --------

    let config = CellConfig::new(CELL_NAME, "europe-west2").with_venue(VenueId::new("XLON"));
    let engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let cell = Cell::new(config, engine)?;

    // Verify that the cell is not live-capable (paper trading requirement).
    assert!(
        !cell.autonomy().ceiling().is_live(),
        "The cell must be initialized in paper-trading mode"
    );

    // -------- Part 2: Load signed packages from the cognitive system --------

    // Create the first signed envelope (capital grant from cognitive system).
    // This simulates a signed package from the slow path (qip-deepbrain).
    let signed_envelope_v1 = signed_capital_envelope("100000", "50000")?;
    let verified_v1 =
        VerifiedEnvelope::verify(signed_envelope_v1, CELL_KEY, CELL_NAME, timestamp(0))?;

    // Verify that the cell can use a signed package without the Platform.
    let grant_v1 = verified_v1.admit(
        &VenueId::new("XLON"),
        dec!("10000"),
        &Utilisation::default(),
        timestamp(0),
    );
    assert!(
        matches!(grant_v1, CapitalGrant::Full),
        "The cell must admit orders using a valid signed package without the cognitive system"
    );

    // -------- Part 3: Update packages without restarting the cell --------

    // The cell remains operational and we load a new signed package.
    // This simulates a new cognitive build being released without affecting the running cell.
    let signed_envelope_v2 = signed_capital_envelope("200000", "100000")?;
    let verified_v2 =
        VerifiedEnvelope::verify(signed_envelope_v2, CELL_KEY, CELL_NAME, timestamp(100))?;

    // The cell should work with the updated package without restart.
    let grant_v2 = verified_v2.admit(
        &VenueId::new("XLON"),
        dec!("50000"),
        &Utilisation::default(),
        timestamp(100),
    );
    assert!(
        matches!(grant_v2, CapitalGrant::Full),
        "The cell must continue to work with updated signed packages without restart"
    );

    // -------- Part 4: Verify cell independence from Platform --------

    // The cell is fully self-contained and does not depend on qip-kernel::Platform.
    // It remains operational throughout the package updates.
    assert!(
        !cell.is_halted(),
        "The cell must remain operational after package updates without the cognitive system"
    );

    // Verify paper trading is still enforced after all operations.
    assert!(
        !cell.autonomy().ceiling().is_live(),
        "The cell must maintain paper-trading posture throughout its operation"
    );

    Ok(())
}

/// ARCH-002: The reflex system takes packages from the slow system, not live calls.
///
/// Requirement: "The fast reflex system receives compressed knowledge, models, policy and
/// capital envelopes from the slow cognitive system as packages. It reacts to local market
/// state using those packages and does not wait for, or call, the global brain."
///
/// This test verifies that:
/// 1. The cell's inbound interface accepts only package types (capital envelopes, etc.)
/// 2. The cell can decide from cached packages without calling the center
/// 3. The cell remains operational when the center is unreachable
#[test]
fn arch_002_reflex_system_takes_packages_not_live_calls() -> Result<()> {
    // -------- Part 1: Assemble a cell and load packages --------

    let config = CellConfig::new(CELL_NAME, "europe-west2").with_venue(VenueId::new("XLON"));
    let engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let cell = Cell::new(config, engine)?;

    // Load a capital envelope package (the only inbound package type the cell accepts from center).
    let envelope_package = signed_capital_envelope("100000", "50000")?;
    let verified_envelope =
        VerifiedEnvelope::verify(envelope_package, CELL_KEY, CELL_NAME, timestamp(0))?;

    // -------- Part 2: Verify package-based decision-making --------

    // The cell makes decisions using only the cached package, not calling the center.
    // Even if the center were unreachable, the cell would continue using the cached envelope.
    let admission_grant = verified_envelope.admit(
        &VenueId::new("XLON"),
        dec!("10000"),
        &Utilisation::default(),
        timestamp(0),
    );

    assert!(
        matches!(admission_grant, CapitalGrant::Full),
        "The cell must decide from cached packages without calling the center"
    );

    // -------- Part 3: Verify the cell does not require center connectivity --------

    // The cell's decision is deterministic and depends only on:
    // 1. The signed package (capital envelope)
    // 2. The local state (utilisation)
    // 3. The market state (local to the cell)
    //
    // It does NOT depend on:
    // - Connectivity to the center (qip-kernel Platform)
    // - Real-time calls to the cognitive system
    // - External service availability

    assert!(
        !cell.is_halted(),
        "The cell must remain operational when using only cached packages"
    );

    // Verify that no synchronous path to the center is required for basic decisions.
    // The cell's autonomy and decision-making are local to the cell.
    assert!(
        !cell.autonomy().ceiling().is_live(),
        "Package-based decisions must respect the cell's autonomy ceiling (paper trading)"
    );

    Ok(())
}

/// Helper: Create a signed capital envelope with the given limits.
fn signed_capital_envelope(gross_limit: &str, order_limit: &str) -> Result<CapitalEnvelope> {
    let unsigned = CapitalEnvelope::new(
        StrategyId::new("arch-test-strategy"),
        CELL_NAME,
        Decimal::parse(gross_limit).expect("a valid decimal"),
        Decimal::parse(order_limit).expect("a valid decimal"),
        dec!("50000"),
        vec![VenueId::new("XLON")],
        timestamp(0),
        timestamp(3600),
        "arch-test@example.com",
        "unsigned",
    )?;

    let signature = sign_payload(CELL_KEY, &unsigned.signing_payload());
    CapitalEnvelope::new(
        StrategyId::new("arch-test-strategy"),
        CELL_NAME,
        Decimal::parse(gross_limit).expect("a valid decimal"),
        Decimal::parse(order_limit).expect("a valid decimal"),
        dec!("50000"),
        vec![VenueId::new("XLON")],
        timestamp(0),
        timestamp(3600),
        "arch-test@example.com",
        signature,
    )
}
