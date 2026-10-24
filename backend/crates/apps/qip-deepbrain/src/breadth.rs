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

/// The plan's permission registers, re-read on every iteration of the run
/// loop so a revocation reaches the Market Factory's validity monitor
/// (R12-EXEC-5081) and the event plane (R12-COVERAGE-5107).
///
/// Until 2026-10-11 the register was fixed when the plan was installed at
/// start-up: `FactoryStage::set_permissions` had no caller outside a test,
/// so a venue rulebook withdrawn, a product approval lapsed or a legal
/// opinion retracted could be recorded nowhere a running node would see,
/// and the monitor's legal check — which withdraws a market whose
/// permission lapsed — could not fire in any binary. The operator's act is
/// now the ordinary one: edit the plan file the node was started with and
/// delete the entry. The factory's `permissions` and, since 2026-10-24, the
/// event-markets section's `permissions` are re-read — until then this
/// watch re-read only the factory's, and a permission deleted from the
/// event section stopped nothing, because the plane consulted its register
/// once, at install. Markets, event sources and commerce stay as
/// installed, because adding a market is a decision made at start-up and
/// removing a permission is not.
///
/// Fails closed. A plan file that cannot be read or parsed, that no longer
/// holds a section the platform installed, or whose register the platform
/// refuses, revokes **every** permission of both registers — each live
/// market is withdrawn at the next review, and each event market at the
/// next pass — and the reason is returned for the cycle line. A register
/// nobody can read is not one a market may keep trading under.
#[derive(Debug)]
pub struct PermissionWatch {
    path: String,
    applied: Option<Registers>,
}

/// The two registers as last applied: `None` for a section the platform
/// did not install.
type Registers = (
    Option<Vec<qip_kernel::market_factory_stage::PermissionEntry>>,
    Option<Vec<qip_kernel::market_factory_stage::PermissionEntry>>,
);

impl PermissionWatch {
    /// A watch over the plan [`PLAN_VARIABLE`] names, or `None` when it is
    /// unset or blank — no plan, no factory, nothing to revoke.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let path = lookup(PLAN_VARIABLE).filter(|value| !value.trim().is_empty())?;
        Some(Self {
            path: path.trim().to_string(),
            applied: None,
        })
    }

    /// Re-read both registers and apply them if either changed.
    /// `Ok(Some(n))` is `n` entries newly applied across the installed
    /// sections, `Ok(None)` an unchanged pair or a platform with neither a
    /// factory nor an event plane, `Err` a register refused — after which
    /// every permission of both has been revoked.
    pub fn refresh(&mut self, platform: &mut Platform) -> Result<Option<usize>> {
        let factory = platform.market_factory().is_some();
        let events = platform.event_markets().is_some();
        if !factory && !events {
            return Ok(None);
        }
        let read = std::fs::read_to_string(&self.path)
            .map_err(|error| {
                Error::invalid(format!(
                    "{PLAN_VARIABLE}={} cannot be re-read: {error}",
                    self.path
                ))
            })
            .and_then(|text| BreadthPlan::from_json(&text))
            .and_then(|plan| {
                let factory_entries = if factory {
                    Some(
                        plan.market_factory
                            .map(|section| section.permissions)
                            .ok_or_else(|| {
                                Error::invalid(format!(
                                    "{PLAN_VARIABLE}={} no longer holds a market_factory section",
                                    self.path
                                ))
                            })?,
                    )
                } else {
                    None
                };
                let event_entries = if events {
                    Some(
                        plan.event_markets
                            .map(|section| section.permissions)
                            .ok_or_else(|| {
                                Error::invalid(format!(
                                    "{PLAN_VARIABLE}={} no longer holds an event_markets section",
                                    self.path
                                ))
                            })?,
                    )
                } else {
                    None
                };
                Ok((factory_entries, event_entries))
            });
        let registers = match read {
            Ok(registers) => registers,
            Err(error) => return Err(self.revoke_all(platform, error)),
        };
        if self.applied.as_ref() == Some(&registers) {
            return Ok(None);
        }
        let mut applied = 0;
        if let Some(entries) = &registers.0 {
            match platform.revise_market_permissions(entries) {
                Ok(count) => applied += count,
                Err(error) => return Err(self.revoke_all(platform, error)),
            }
        }
        if let Some(entries) = &registers.1 {
            match platform.revise_event_market_permissions(entries) {
                Ok(count) => applied += count,
                Err(error) => return Err(self.revoke_all(platform, error)),
            }
        }
        self.applied = Some(registers);
        Ok(Some(applied))
    }

    fn revoke_all(&mut self, platform: &mut Platform, error: Error) -> Error {
        platform.revoke_market_permissions();
        platform.revoke_event_market_permissions();
        self.applied = Some((Some(Vec::new()), Some(Vec::new())));
        Error::denied(format!(
            "market permissions revoked, every created market will be withdrawn: {}",
            error.message()
        ))
    }
}
