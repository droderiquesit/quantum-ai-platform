//! The cell's deterministic Risk Gate, as this node installs it
//! (R12-RISK-4093, R12-RISK-7205).
//!
//! `qip_edge::CellRiskGate` was written, tested and installable through
//! `Cell::with_risk_gate`, and no composition root called it: every node this
//! binary built sent orders with the gate's `Option` empty, so the reflex path
//! — the one process here that places orders without asking the centre
//! (ADR 0008) — measured no aggregate leverage and no survival ceiling at all.
//! The route table in `qip-acceptance` said "gated" because the call was in
//! the source; the running cell skipped it every time.
//!
//! This module is the one way the node obtains the limits, and
//! [`crate::assemble_with_risk`] takes them by value, so a node cannot be
//! assembled without a gate. There is deliberately no "no gate" value:
//!
//! * **Unset** installs the [`CellRiskLimits::shipped`] set — the two limits a
//!   cell can always evaluate from what it holds: gross against the region
//!   allocation, and gross against the gross its live verified grants sum to
//!   (the survival ceiling the centre released, R12-RISK-7205). Unset is the
//!   restrictive default, not an absence.
//! * **Set** names a file holding a limit-set document — the same JSON a
//!   signed recalibration renders as — which may move a bound and add a
//!   limit, and may not remove one of the shipped two
//!   (`LimitSet::from_document_against`). A file naming a limit the cell
//!   cannot produce a figure for (value at risk, drawdown, …) is refused at
//!   start by `CellRiskGate::new`, naming it, because at the venue it would
//!   take its silent `None` arm and pass every order.
//! * **Unreadable or malformed** stops the process. A node told to load
//!   limits and unable to is never quietly downgraded to the shipped set.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_edge::CellRiskGate;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

/// The path of the cell's limit-set document. Unset installs the shipped set.
pub const RISK_LIMITS_VARIABLE: &str = "QIP_CELL_RISK_LIMITS";

/// Where the limits came from, for the start-up banner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LimitSource {
    Shipped,
    File(String),
}

/// The limits the node's Risk Gate evaluates. Only [`Self::shipped`] and
/// [`Self::read`] produce one, and both have already refused a set the gate
/// would refuse — so assembling cannot fail on the limits for a reason start-up
/// did not already name.
#[derive(Clone, Debug)]
pub struct CellRiskLimits {
    limits: LimitSet,
    source: LimitSource,
}

impl CellRiskLimits {
    /// The set a cell ships with. Both limits read figures the cell computes
    /// from its own book, so neither can take a silent pass.
    pub fn shipped_set() -> LimitSet {
        LimitSet::new("cell-shipped")
            .with(
                Limit::new("cell-leverage", LimitKind::MaxLeverage { limit: 1.0 }).with_rationale(
                    "the cell's gross — confirmed positions and every working order at its \
                     remaining size — may not exceed the region allocation the deployment \
                     stated: a dozen grants each inside its own envelope must not together \
                     lever the cell",
                ),
            )
            .with(
                Limit::new(
                    "cell-survival-gross",
                    LimitKind::MaxSurvivalGross { limit: 1.0 },
                )
                .with_rationale(
                    "the cell's gross may not exceed the gross its live verified grants sum to, \
                     which is what the centre's survival kernel released to it; a cell whose \
                     grants have lapsed stands behind nothing new and may still shed gross",
                ),
            )
    }

    pub fn shipped() -> Self {
        Self {
            limits: Self::shipped_set(),
            source: LimitSource::Shipped,
        }
    }

    /// The limits named by [`RISK_LIMITS_VARIABLE`], or the shipped set when
    /// it is unset or blank. `load` reads the named file; the binary passes
    /// `std::fs::read_to_string`, a test passes a closure.
    pub fn read(
        value: Option<&str>,
        load: impl FnOnce(&str) -> std::io::Result<String>,
    ) -> Result<Self> {
        let Some(path) = value.map(str::trim).filter(|path| !path.is_empty()) else {
            return Ok(Self::shipped());
        };
        let text = load(path).map_err(|error| {
            Error::invalid(format!(
                "configuration: {RISK_LIMITS_VARIABLE}={path} cannot be read ({error}); the node \
                 was told to gate on that file and will not fall back to the shipped set. Mount \
                 the file, or unset the variable to run on the shipped limits"
            ))
        })?;
        Self::from_document(&text, path)
    }

    /// Parse and validate a limit-set document against the shipped set, then
    /// prove the gate would install it.
    pub fn from_document(text: &str, path: &str) -> Result<Self> {
        let limits = LimitSet::from_document_against(text, path, &Self::shipped_set())?;
        // Any positive capital proves installability: the refusal this can
        // raise is about the kinds, not the amount.
        CellRiskGate::new(limits.clone(), Decimal::ONE).map_err(|error| {
            Error::invalid(format!("configuration: {path}: {}", error.message()))
        })?;
        Ok(Self {
            limits,
            source: LimitSource::File(path.to_string()),
        })
    }

    pub fn limits(&self) -> &LimitSet {
        &self.limits
    }

    pub fn source(&self) -> &LimitSource {
        &self.source
    }

    /// The gate over these limits, measuring ratios against `capital` — the
    /// region allocation, the most the deployment says the cell may commit.
    pub fn gate(&self, capital: Decimal) -> Result<CellRiskGate> {
        CellRiskGate::new(self.limits.clone(), capital)
    }

    /// One line for the start-up banner.
    pub fn describe(&self) -> String {
        let names: Vec<String> = self
            .limits
            .limits
            .iter()
            .map(|limit| format!("{} ({})", limit.name, limit.kind.label()))
            .collect();
        match &self.source {
            LimitSource::Shipped => format!(
                "the cell's risk gate evaluates the shipped set: {}",
                names.join(", ")
            ),
            LimitSource::File(path) => format!(
                "the cell's risk gate evaluates {path}: {}",
                names.join(", ")
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_set_installs_and_every_limit_in_it_is_one_the_cell_can_evaluate() {
        let shipped = CellRiskLimits::shipped_set();
        assert!(!shipped.is_empty(), "premise: the shipped set has limits");
        for limit in &shipped.limits {
            assert!(
                CellRiskGate::evaluable(&limit.kind),
                "{} is shipped and the cell has no figure for it",
                limit.name
            );
        }
        assert!(CellRiskLimits::shipped().gate(Decimal::ONE).is_ok());
    }

    #[test]
    fn an_unreadable_limits_file_stops_the_node_rather_than_falling_back() {
        let error = CellRiskLimits::read(Some("/no/such/limits.json"), |_| {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .expect_err("a file the node cannot read must stop it");
        assert!(error.message().starts_with("configuration:"));
        assert!(error.message().contains(RISK_LIMITS_VARIABLE));
    }

    #[test]
    fn unset_is_the_shipped_set_and_not_an_absent_gate() {
        let limits = CellRiskLimits::read(None, |_| unreachable_load()).expect("unset is shipped");
        assert_eq!(limits.source(), &LimitSource::Shipped);
        assert_eq!(limits.limits(), &CellRiskLimits::shipped_set());
    }

    fn unreachable_load() -> std::io::Result<String> {
        Err(std::io::Error::other("unset must not read a file"))
    }

    #[test]
    fn a_file_that_drops_a_shipped_limit_or_names_one_the_cell_cannot_evaluate_is_refused() {
        // Drops `cell-survival-gross`.
        let dropped = r#"{"name":"cell","limits":[{"name":"cell-leverage","kind":{"kind":"max_leverage","limit":1.0},"warning_threshold":0.85,"critical_multiple":1.25,"forces_reduction":false,"rationale":"r"}]}"#;
        assert!(CellRiskLimits::from_document(dropped, "dropped.json").is_err());
        // Carries both, plus a value-at-risk limit the cell has no figure for.
        let var = r#"{"name":"cell","limits":[
            {"name":"cell-leverage","kind":{"kind":"max_leverage","limit":1.0},"warning_threshold":0.85,"critical_multiple":1.25,"forces_reduction":false,"rationale":"r"},
            {"name":"cell-survival-gross","kind":{"kind":"max_survival_gross","limit":1.0},"warning_threshold":0.85,"critical_multiple":1.25,"forces_reduction":false,"rationale":"r"},
            {"name":"var","kind":{"kind":"max_value_at_risk","confidence":0.99,"limit":0.05},"warning_threshold":0.85,"critical_multiple":1.25,"forces_reduction":false,"rationale":"r"}]}"#;
        let error = CellRiskLimits::from_document(var, "var.json")
            .expect_err("a limit the cell cannot evaluate must stop the node");
        assert!(
            error.message().contains("max_value_at_risk"),
            "{}",
            error.message()
        );
    }
}
