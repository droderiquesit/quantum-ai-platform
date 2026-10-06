//! AGENCY-012 / AGENCY-004: the action-affordance graph and the levers drawn
//! from it.
//!
//! The market `CausalGraph` is observational: it can say two things move
//! together, not that Algorik can move either. A planner reading it would
//! pick a "lever" nobody can pull. This graph records, per variable, whether
//! it can be observed and whether it can be controlled, and a tool edge (who
//! can move it, at what latency and cost, with what side effects and under
//! what authority) exists only on a controllable variable.
//!
//! AGENCY-023: an operational or product tool edge exists only on a variable
//! in the owned-system registry, so "change a system nobody here owns" is not
//! a lever a plan can name.

use crate::{required, text};
use qip_core::{Decimal, Error};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// How a tool moves its variable. The first six are the action domains §1
/// names; every other variant is a method AGENCY-010 and AGENCY-011 prohibit.
///
/// The graph may *record* a prohibited lever, because pretending a lever does
/// not exist is how it gets used unreviewed. What it may never do is reach a
/// plan: [`crate::plan::InterventionPlan::plan`] refuses it (AGENCY-047).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Communication,
    Capital,
    Product,
    Operational,
    Research,
    Market,
    FabricatedIdentity,
    Sockpuppet,
    FakeConsensus,
    MisleadingClaim,
    UndisclosedPromotion,
    RumourManufacture,
    WashTrading,
    Spoofing,
    PumpAndDump,
}

impl Method {
    /// Deceptive or manipulative. Written as "not one of the lawful six" so a
    /// variant added later is prohibited until somebody argues it is not.
    pub fn is_deceptive(self) -> bool {
        !matches!(
            self,
            Self::Communication
                | Self::Capital
                | Self::Product
                | Self::Operational
                | Self::Research
                | Self::Market
        )
    }

    /// Changes a system or product, so its target must be one Algorik owns.
    fn needs_ownership(self) -> bool {
        matches!(self, Self::Operational | Self::Product)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Variable {
    observable: bool,
    controllable: bool,
}

/// A tool-to-variable edge as declared. Every field optional so a missing
/// one is a refusal that names it; an empty side-effect or dependency list
/// is a legitimate declaration of "none".
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolEdgeDraft {
    pub tool: Option<String>,
    pub method: Option<Method>,
    /// Whether the tool's effect can be undone. Not defaulted: a tool nobody
    /// declared reversible is not narrow-reversible authority's to use.
    pub reversible: Option<bool>,
    pub latency_ms: Option<u64>,
    pub cost: Option<Decimal>,
    pub side_effects: Option<Vec<String>>,
    pub dependencies: Option<Vec<String>>,
    /// The authority an acting identity must hold to use this edge.
    pub authority: Option<String>,
}

/// A validated tool edge.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ToolEdge {
    pub tool: String,
    pub method: Method,
    pub reversible: bool,
    pub latency_ms: u64,
    pub cost: Decimal,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub authority: String,
}

/// Variables, causal paths between them, and the tools that can move them.
#[derive(Debug, Clone, Default)]
pub struct AffordanceGraph {
    variables: BTreeMap<String, Variable>,
    causes: BTreeMap<String, BTreeSet<String>>,
    tools: BTreeMap<String, Vec<ToolEdge>>,
    /// The owned-system registry (AGENCY-023).
    owned: BTreeSet<String>,
}

impl AffordanceGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare a variable: whether Algorik can observe it and whether it can
    /// control it. Neither implies the other.
    pub fn add_variable(
        &mut self,
        name: &str,
        observable: bool,
        controllable: bool,
    ) -> Result<(), Error> {
        let name = text("variable", Some(name.to_string()))?;
        self.variables.insert(
            name,
            Variable {
                observable,
                controllable,
            },
        );
        Ok(())
    }

    /// Enter a declared variable in the owned-system registry: a system or
    /// product Algorik owns, and so may change.
    pub fn declare_owned(&mut self, name: &str) -> Result<(), Error> {
        if !self.variables.contains_key(name) {
            return Err(Error::not_found(format!(
                "variable `{name}` is not in the graph; declare it first"
            )));
        }
        self.owned.insert(name.to_string());
        Ok(())
    }

    /// Whether the variable is declared observable; `None` if undeclared.
    pub fn is_observable(&self, name: &str) -> Option<bool> {
        self.variables.get(name).map(|v| v.observable)
    }

    /// `from` causally influences `to`; both must already be declared.
    pub fn add_cause(&mut self, from: &str, to: &str) -> Result<(), Error> {
        for v in [from, to] {
            if !self.variables.contains_key(v) {
                return Err(Error::not_found(format!(
                    "variable `{v}` is not in the graph; declare it first"
                )));
            }
        }
        self.causes
            .entry(from.to_string())
            .or_default()
            .insert(to.to_string());
        Ok(())
    }

    /// Attach a tool to a controllable variable. Refuses an undeclared
    /// variable, an observable-only one, and any missing declaration.
    pub fn add_tool_edge(&mut self, variable: &str, draft: ToolEdgeDraft) -> Result<(), Error> {
        let Some(var) = self.variables.get(variable) else {
            return Err(Error::not_found(format!(
                "variable `{variable}` is not in the graph; declare it first"
            )));
        };
        if !var.controllable {
            return Err(Error::denied(format!(
                "variable `{variable}` is observable-only; it can hold no tool edge"
            )));
        }
        let edge = ToolEdge {
            tool: text("tool", draft.tool)?,
            method: required("method", draft.method)?,
            reversible: required("reversible", draft.reversible)?,
            latency_ms: required("latency_ms", draft.latency_ms)?,
            cost: required("cost", draft.cost)?,
            side_effects: required("side_effects", draft.side_effects)?,
            dependencies: required("dependencies", draft.dependencies)?,
            authority: text("authority", draft.authority)?,
        };
        if edge.cost.is_negative() {
            return Err(Error::invalid("`cost` must not be negative"));
        }
        if edge.method.needs_ownership() && !self.owned.contains(variable) {
            return Err(Error::denied(format!(
                "`{variable}` is not in the owned-system registry; an operational or product \
                 action is confined to systems Algorik owns, so declare it owned or drop the edge"
            )));
        }
        self.tools
            .entry(variable.to_string())
            .or_default()
            .push(edge);
        Ok(())
    }

    /// The tool edges on a variable.
    pub fn edges(&self, variable: &str) -> &[ToolEdge] {
        self.tools.get(variable).map_or(&[], Vec::as_slice)
    }

    /// Controllable variables with at least one tool edge that can reach a
    /// target through the causal paths (a target itself, if controllable,
    /// counts). Observable-only variables never appear.
    pub fn levers(&self, targets: &[&str]) -> Vec<String> {
        let mut reaches: BTreeSet<&str> = targets.iter().copied().collect();
        loop {
            let before = reaches.len();
            for (from, tos) in &self.causes {
                if tos.iter().any(|t| reaches.contains(t.as_str())) {
                    reaches.insert(from.as_str());
                }
            }
            if reaches.len() == before {
                break;
            }
        }
        reaches
            .into_iter()
            .filter(|v| {
                self.variables.get(*v).is_some_and(|x| x.controllable) && !self.edges(v).is_empty()
            })
            .map(str::to_string)
            .collect()
    }
}
