//! Per-venue execution-mode enablement (EXEC-020).
//!
//! A mode is enabled for a venue only where it is legally and operationally
//! supported there and for the trading entity's jurisdiction, and a request in
//! a mode that is not enabled is refused here, before any order object is
//! built, because [`ModeGate::admit`] returns the only token a caller may build
//! an order from. With nothing enabled, every mode is refused: the default is
//! the restrictive one, and enabling is the act that needs evidence.
//!
//! This gate sits beneath the paper-trading boundary and does not replace it:
//! an enabled mode still reaches only simulated or sandbox venues.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::{Error, Result};

/// The eight execution modes of blueprint section 18.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExecutionMode {
    ContinuousQuoting,
    OrderTaking,
    LiquidityProvision,
    Routing,
    Derivatives,
    EventContracts,
    DecentralisedVenues,
    PhysicalMarketplaces,
}

impl ExecutionMode {
    /// Every mode, so a test can prove it covered all of them.
    pub const ALL: [Self; 8] = [
        Self::ContinuousQuoting,
        Self::OrderTaking,
        Self::LiquidityProvision,
        Self::Routing,
        Self::Derivatives,
        Self::EventContracts,
        Self::DecentralisedVenues,
        Self::PhysicalMarketplaces,
    ];
}

/// Evidence that a mode is supported for a venue and jurisdiction. Both halves
/// are required: permission does not make a venue operationally ready, and a
/// working connection is not permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModeSupport {
    pub jurisdiction: String,
    pub legally_permitted: bool,
    pub operationally_supported: bool,
}

/// Proof that a mode was admitted at a venue. Its fields are private, so an
/// order builder that demands one cannot be reached by a refused request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModeAdmission {
    venue: String,
    mode: ExecutionMode,
}

impl ModeAdmission {
    pub fn venue(&self) -> &str {
        &self.venue
    }

    pub fn mode(&self) -> ExecutionMode {
        self.mode
    }
}

/// Which modes are enabled at which venue. Empty by construction.
#[derive(Clone, Debug, Default)]
pub struct ModeGate {
    enabled: BTreeMap<String, BTreeSet<ExecutionMode>>,
}

impl ModeGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable one mode at one venue, refusing unless both legal and
    /// operational support are recorded for a named jurisdiction.
    pub fn enable(
        &mut self,
        venue: &str,
        mode: ExecutionMode,
        support: &ModeSupport,
    ) -> Result<()> {
        if venue.trim().is_empty() || support.jurisdiction.trim().is_empty() {
            return Err(Error::invalid(
                "enabling a mode needs a named venue and jurisdiction; record both",
            ));
        }
        if !support.legally_permitted || !support.operationally_supported {
            return Err(Error::denied(format!(
                "{mode:?} stays disabled at {venue} for {}: it needs both legal permission \
                 (have {}) and operational support (have {}); record the missing one first",
                support.jurisdiction, support.legally_permitted, support.operationally_supported
            )));
        }
        self.enabled
            .entry(venue.to_string())
            .or_default()
            .insert(mode);
        Ok(())
    }

    /// Admit a request in `mode` at `venue`, or refuse it before an order exists.
    pub fn admit(&self, venue: &str, mode: ExecutionMode) -> Result<ModeAdmission> {
        if self.enabled.get(venue).is_some_and(|m| m.contains(&mode)) {
            return Ok(ModeAdmission {
                venue: venue.to_string(),
                mode,
            });
        }
        Err(Error::denied(format!(
            "{mode:?} is not enabled at {venue}; enable it with recorded legal and operational \
             support for the trading entity's jurisdiction, or send the order in an enabled mode"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn support() -> ModeSupport {
        ModeSupport {
            jurisdiction: "US".to_string(),
            legally_permitted: true,
            operationally_supported: true,
        }
    }

    #[test]
    fn with_nothing_enabled_every_mode_is_refused_and_enabling_one_admits_only_that_one() {
        let mut gate = ModeGate::new();
        // Premise: the loops below cover eight modes, not an empty list.
        assert_eq!(ExecutionMode::ALL.len(), 8);
        for mode in ExecutionMode::ALL {
            assert!(
                gate.admit("SIM", mode).is_err(),
                "{mode:?} must start refused"
            );
        }
        gate.enable("SIM", ExecutionMode::Routing, &support())
            .expect("a fully supported mode enables");
        for mode in ExecutionMode::ALL {
            assert_eq!(
                gate.admit("SIM", mode).is_ok(),
                mode == ExecutionMode::Routing,
                "{mode:?}"
            );
        }
        assert!(gate.admit("OTHER", ExecutionMode::Routing).is_err());
    }

    #[test]
    fn a_mode_without_both_legal_and_operational_support_cannot_be_enabled() {
        let mut gate = ModeGate::new();
        for (legal, ops) in [(true, false), (false, true), (false, false)] {
            let s = ModeSupport {
                legally_permitted: legal,
                operationally_supported: ops,
                ..support()
            };
            assert!(gate.enable("SIM", ExecutionMode::Derivatives, &s).is_err());
        }
        assert!(gate.admit("SIM", ExecutionMode::Derivatives).is_err());
    }
}
