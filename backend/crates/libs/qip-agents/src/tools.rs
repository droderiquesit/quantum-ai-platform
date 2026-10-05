//! The Tool Registry (EXPAND-007, EXPAND-052).
//!
//! A tool the platform can use is registry data, not a code path an agent
//! happens to reach. Every tool enters with the narrowest scope there is --
//! read-only, inside its sandbox -- and keeps it until a promotion carrying
//! evaluation evidence widens it. A scope granted at registration would make
//! "evaluated" a comment on a tool that had never been looked at.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The ten kinds of tool the blueprint names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Connector,
    Browser,
    Parser,
    Solver,
    CodeRuntime,
    Simulator,
    Geospatial,
    VenueAdapter,
    ModelEndpoint,
    QuantumBackend,
}

impl ToolKind {
    pub const ALL: [Self; 10] = [
        Self::Connector,
        Self::Browser,
        Self::Parser,
        Self::Solver,
        Self::CodeRuntime,
        Self::Simulator,
        Self::Geospatial,
        Self::VenueAdapter,
        Self::ModelEndpoint,
        Self::QuantumBackend,
    ];
}

/// One thing a tool may do. `Read` is the only one a tool starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermission {
    Read,
    Write,
    /// A call that leaves the tool's sandbox.
    LeaveSandbox,
}

/// A registered tool and the scope it currently holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    name: String,
    kind: ToolKind,
    scope: BTreeSet<ToolPermission>,
}

impl ToolSpec {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn kind(&self) -> ToolKind {
        self.kind
    }
    pub const fn scope(&self) -> &BTreeSet<ToolPermission> {
        &self.scope
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, ToolSpec>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. It always starts read-only and sandboxed; there is
    /// deliberately no parameter to start wider.
    pub fn register(&mut self, name: &str, kind: ToolKind) -> Result<&ToolSpec> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid(
                "a tool needs a name; register it under the name callers will resolve it by",
            ));
        }
        if self.tools.contains_key(name) {
            return Err(Error::invalid(format!(
                "tool {name} is already registered; widen it through `promote`, not by registering it again"
            )));
        }
        let spec = ToolSpec {
            name: name.to_string(),
            kind,
            scope: BTreeSet::from([ToolPermission::Read]),
        };
        Ok(self.tools.entry(name.to_string()).or_insert(spec))
    }

    /// Every tool of one kind, in name order.
    pub fn by_kind(&self, kind: ToolKind) -> Vec<&ToolSpec> {
        self.tools.values().filter(|t| t.kind == kind).collect()
    }

    /// Widen a tool's scope through a promotion. `evaluation` must name the
    /// evaluation of the tool's behaviour and data handling; a blank one is
    /// refused, so a grant cannot be made without having looked.
    pub fn promote(&mut self, name: &str, grant: ToolPermission, evaluation: &str) -> Result<()> {
        if evaluation.trim().is_empty() {
            return Err(Error::denied(
                "a scope widens only through a promotion that cites its evaluation; attach the evaluation record",
            ));
        }
        let tool = self
            .tools
            .get_mut(name)
            .ok_or_else(|| Error::not_found(format!("tool {name} is not registered")))?;
        tool.scope.insert(grant);
        Ok(())
    }

    /// Refuse any use of a tool outside the scope it holds.
    pub fn authorise(&self, name: &str, permission: ToolPermission) -> Result<()> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| Error::not_found(format!("tool {name} is not registered")))?;
        if tool.scope.contains(&permission) {
            Ok(())
        } else {
            Err(Error::denied(format!(
                "tool {name} holds no {permission:?} permission; request a promotion with evaluation evidence"
            )))
        }
    }
}
