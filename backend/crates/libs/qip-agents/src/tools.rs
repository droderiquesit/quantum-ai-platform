//! The Tool Registry (EXPAND-007, EXPAND-052).
//!
//! A tool the platform can use is registry data, not a code path an agent
//! happens to reach. Every tool enters with the narrowest scope there is --
//! read-only, inside its sandbox -- and keeps it until a promotion carrying
//! evaluation evidence widens it. A scope granted at registration would make
//! "evaluated" a comment on a tool that had never been looked at.
//!
//! CONTRACT-034 requires every ToolSpec to carry capability, interface,
//! permission scope, data handling, dependency/security provenance, sandbox
//! tests, cost/latency, and allowed callers—all mandatory. validate() refuses
//! any missing or blank field. A tool is thus declared before it can run,
//! carrying its permission scope, provenance and allowed callers.

use crate::capability::Capability;
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
/// Carries all eight fields required by CONTRACT-034: capability, interface,
/// permission scope, data handling, dependency/security provenance, sandbox
/// tests, cost/latency, and allowed callers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    name: String,
    kind: ToolKind,
    scope: BTreeSet<ToolPermission>,
    /// Capability required to invoke this tool.
    capability: Capability,
    /// Interface description: signature, parameters, return value.
    interface: String,
    /// Data handling policy and restrictions.
    data_handling: String,
    /// Dependency/security provenance: reference to dependency record.
    provenance: String,
    /// Sandbox tests that must pass: test names or identifiers.
    sandbox_tests: Vec<String>,
    /// Cost bounds: format is tool-specific (e.g., "tokens: 1000, latency_ms: 500").
    cost_latency: String,
    /// Allowed callers: agent IDs or roles permitted to invoke.
    allowed_callers: Vec<String>,
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
    pub fn capability(&self) -> Capability {
        self.capability
    }
    pub fn interface(&self) -> &str {
        &self.interface
    }
    pub fn data_handling(&self) -> &str {
        &self.data_handling
    }
    pub fn provenance(&self) -> &str {
        &self.provenance
    }
    pub fn sandbox_tests(&self) -> &[String] {
        &self.sandbox_tests
    }
    pub fn cost_latency(&self) -> &str {
        &self.cost_latency
    }
    pub fn allowed_callers(&self) -> &[String] {
        &self.allowed_callers
    }

    /// Construct a complete ToolSpec with all eight required fields.
    /// Use this to build a spec for testing or when all fields are known.
    pub fn new(
        name: impl Into<String>,
        kind: ToolKind,
        capability: Capability,
        interface: impl Into<String>,
        data_handling: impl Into<String>,
        provenance: impl Into<String>,
        sandbox_tests: Vec<String>,
        cost_latency: impl Into<String>,
        allowed_callers: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            scope: BTreeSet::from([ToolPermission::Read]),
            capability,
            interface: interface.into(),
            data_handling: data_handling.into(),
            provenance: provenance.into(),
            sandbox_tests,
            cost_latency: cost_latency.into(),
            allowed_callers,
        }
    }

    /// Check the tool spec is internally coherent and all fields are present.
    /// Every field is required: none may be missing, blank or empty.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::invalid("tool spec has no name"));
        }
        if self.interface.trim().is_empty() {
            return Err(Error::invalid(format!(
                "tool {} declares no interface; describe the tool's signature",
                self.name
            )));
        }
        if self.data_handling.trim().is_empty() {
            return Err(Error::invalid(format!(
                "tool {} declares no data handling policy",
                self.name
            )));
        }
        if self.provenance.trim().is_empty() {
            return Err(Error::invalid(format!(
                "tool {} declares no dependency/security provenance",
                self.name
            )));
        }
        if self.sandbox_tests.iter().all(|t| t.trim().is_empty()) {
            return Err(Error::invalid(format!(
                "tool {} lists no sandbox tests; name the tests it must pass",
                self.name
            )));
        }
        if self.cost_latency.trim().is_empty() {
            return Err(Error::invalid(format!(
                "tool {} declares no cost/latency bounds",
                self.name
            )));
        }
        if self.allowed_callers.iter().all(|c| c.trim().is_empty()) {
            return Err(Error::invalid(format!(
                "tool {} names no allowed callers; list agent IDs or roles",
                self.name
            )));
        }
        Ok(())
    }

    /// Builder method to set data handling policy.
    pub fn with_data_handling(mut self, policy: impl Into<String>) -> Self {
        self.data_handling = policy.into();
        self
    }

    /// Builder method to set provenance reference.
    pub fn with_provenance(mut self, provenance: impl Into<String>) -> Self {
        self.provenance = provenance.into();
        self
    }

    /// Builder method to set sandbox tests.
    pub fn with_sandbox_tests(mut self, tests: Vec<String>) -> Self {
        self.sandbox_tests = tests;
        self
    }

    /// Builder method to set cost/latency bounds.
    pub fn with_cost_latency(mut self, bounds: impl Into<String>) -> Self {
        self.cost_latency = bounds.into();
        self
    }

    /// Builder method to set allowed callers.
    pub fn with_allowed_callers(mut self, callers: Vec<String>) -> Self {
        self.allowed_callers = callers;
        self
    }

    /// Builder method to set interface description.
    pub fn with_interface(mut self, interface: impl Into<String>) -> Self {
        self.interface = interface.into();
        self
    }

    /// Builder method to set capability.
    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.capability = capability;
        self
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

    /// The spec `register` would admit for a tool the platform does not have
    /// yet (EXPAND-008). Nothing is registered: a proposal is a record of what
    /// to build, and a missing tool that was registered on being asked for
    /// would be authorised to read before it existed. The proposal carries the
    /// tool's name and kind but empty required fields (CONTRACT-034): use builder
    /// methods to fill them before calling validate().
    pub fn propose(&self, name: &str, kind: ToolKind) -> Result<ToolSpec> {
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
        Ok(ToolSpec {
            name: name.to_string(),
            kind,
            scope: BTreeSet::from([ToolPermission::Read]),
            capability: Capability::CallLanguageModel,
            interface: String::new(),
            data_handling: String::new(),
            provenance: String::new(),
            sandbox_tests: Vec::new(),
            cost_latency: String::new(),
            allowed_callers: Vec::new(),
        })
    }

    /// Register a tool. It always starts read-only and sandboxed; there is
    /// deliberately no parameter to start wider.
    pub fn register(&mut self, name: &str, kind: ToolKind) -> Result<&ToolSpec> {
        let spec = self.propose(name, kind)?;
        Ok(self.tools.entry(spec.name.clone()).or_insert(spec))
    }

    pub fn get(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.get(name.trim())
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
