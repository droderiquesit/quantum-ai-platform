//! The research node's breadth plan: the Market Factory, event markets and
//! commerce research (R12-EXEC-5080/-5081, R12-COVERAGE-5107,
//! R12-EVENT-5082 to -5089, R12-COMMERCE-5090 to -5097).
//!
//! [`PLAN_VARIABLE`] names a JSON file holding a
//! [`qip_kernel::breadth::BreadthPlan`]. Unset is no breadth at all, and the
//! node runs exactly as it did before the plane existed — the fail-closed
//! default, because each section commits the platform to something: a
//! market on the simulated venue, paper positions in event contracts, paper
//! purchases of physical goods. Set, the file is read and parsed here, at
//! start-up, and installed into the platform before the first cycle; a file
//! that cannot be read, does not parse, or asks for something the platform
//! refuses stops the process with exit 78 naming the reason, rather than a
//! node that starts and silently runs less than its plan.
//!
//! The deep brain and not the fast brain, because every section is research:
//! the sources are synthetic and research-only, and the factory's validity
//! monitor is a SIMULATE-stage review, not an order path.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_kernel::Platform;
use qip_kernel::breadth::BreadthPlan;

/// The variable naming the breadth plan's file.
pub const PLAN_VARIABLE: &str = "QIP_DEEPBRAIN_BREADTH_PLAN";

/// The plan a deployment asked for, or `None` when the variable is unset or
/// blank. `lookup` is the environment in the binary and a map in a test.
pub fn load(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Option<BreadthPlan>> {
    let Some(path) = lookup(PLAN_VARIABLE).filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(path.trim()).map_err(|error| {
        Error::invalid(format!(
            "configuration: {PLAN_VARIABLE}={path} cannot be read: {error}"
        ))
    })?;
    let plan = BreadthPlan::from_json(&text)?;
    if plan.is_empty() {
        return Err(Error::invalid(format!(
            "configuration: {PLAN_VARIABLE}={path} names a plan with no section; unset the \
             variable to run without breadth, or state the sections to run"
        )));
    }
    Ok(Some(plan))
}

/// Load and install, returning the banner lines. `None` when no plan is set.
pub fn install(
    platform: &mut Platform,
    lookup: &dyn Fn(&str) -> Option<String>,
    now: Timestamp,
) -> Result<Option<Vec<String>>> {
    let Some(plan) = load(lookup)? else {
        return Ok(None);
    };
    let lines = platform.install_breadth(&plan, now).map_err(|error| {
        if error.message().starts_with("configuration:") {
            error
        } else {
            Error::invalid(format!("configuration: {}", error.message()))
        }
    })?;
    Ok(Some(lines))
}
