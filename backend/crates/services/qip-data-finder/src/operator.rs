//! The named operator a catalogue entry is accountable to (R12-DATA-2020).
//!
//! A registered source reaches the mesh catalogue with an owner, and the
//! kernel filled that owner from a configuration default of `qip-platform` —
//! the process's own name (`qip-kernel/src/config.rs`, `default_owner`, from
//! 2026-10-11 until 2026-10-18, when the default became `research-desk`
//! because the kernel's refusal of it broke every default composition).
//! "Whoever ran the process" is not a name anybody can be asked
//! about six months later, which is the reason the configuration field exists
//! at all; the default defeats it. Moving the kernel onto this type is the
//! kernel lane's change, because it makes a deployment with no configured
//! operator fail to catalogue anything — the correct outcome, and one that
//! lane has to own.
//!
//! [`NamedOperator`] is the owner as a value that cannot be a placeholder:
//! blank, a service or process identity (`qip-…`, `platform`, `system`,
//! `root`…), or a non-answer (`unknown`, `n/a`, `tbd`) is refused by name.
//! [`crate::decision::RegistrationDecision::catalogue_entry_attributed`] takes
//! one, so a catalogue entry built through it names a person or an
//! accountable desk.

use qip_core::error::{Error, Result};
use serde::Serialize;

/// Owner names that identify a process, a host account or nobody.
const PLACEHOLDERS: &[&str] = &[
    "platform",
    "system",
    "service",
    "daemon",
    "root",
    "admin",
    "administrator",
    "default",
    "unknown",
    "none",
    "nobody",
    "n/a",
    "na",
    "tbd",
    "todo",
    "owner",
    "operator",
    "anonymous",
];

/// A validated, accountable owner for a catalogue entry.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct NamedOperator(String);

impl NamedOperator {
    /// Refuse a blank name, a placeholder, or a process identity.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(Error::invalid(
                "a catalogue entry must name the operator accountable for it; a blank owner is \
                 nobody",
            ));
        }
        let lowered = trimmed.to_ascii_lowercase();
        if PLACEHOLDERS.contains(&lowered.as_str()) || lowered.starts_with("qip-") {
            return Err(Error::invalid(format!(
                "`{trimmed}` names a process or a placeholder, not an operator anybody can be \
                 asked about the source; configure the accountable person or desk as the owner"
            )));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NamedOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
