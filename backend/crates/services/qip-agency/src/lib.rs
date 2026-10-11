//! `qip-agency`: the contracts a causal action engine must satisfy before it
//! may plan anything (blueprint §24, AGENCY domain).
//!
//! Nothing here acts. Every type is a refusal-at-construction record: a goal
//! missing one of its twelve declarations, a tool missing one of its six, a
//! candidate set without a no-action baseline, an attribution without a
//! causal chain. A malformed value that cannot be built cannot be planned,
//! simulated or executed, which is the structural form of "wanting an outcome
//! grants no permission to act".
//!
//! Status: one composition root reaches this crate. `qip agency shadow`
//! (`qip-cli`) reads a request document and runs [`engine::run`] with no
//! policy and an adapter that refuses, so the goal, graph, plan, ranking,
//! gate and stage order are on a shipped path and nothing can act through
//! it.
//!
//! Inside the crate, [`engine::execute`] is the only adapter caller and it
//! stands [`guard::ActionGuard`] in front of every call: the conduct gate,
//! the tool registry, the approval chain and the justification check. A
//! request document may carry the guard (`guards`), in which case even a
//! shadow pass reports a step the guard would refuse as withheld.
//! [`engine::learn`] writes [`memory`] and drives [`autonomy`]'s drift rule;
//! [`engine::widen`] is the one widening path. Neither `learn` nor `widen` is
//! called by a composition root yet, because no pass reaches `Act` in a
//! shipped binary: the CLI has no adapter and no policy, deliberately.
//!
//! [`goal`], [`tools`] and [`attribution`] re-export `qip-contracts` types:
//! each used to exist twice, once here and once there.

pub mod affordance;
pub mod attribution;
pub mod autonomy;
pub mod comparison;
pub mod conduct_brain;
pub mod engine;
pub mod goal;
pub mod guard;
pub mod knowledge;
pub mod memory;
pub mod plan;
pub mod tools;

use qip_core::Error;

/// A required text declaration: present and not blank, else a refusal naming it.
pub(crate) fn text(field: &str, value: Option<String>) -> Result<String, Error> {
    match value {
        Some(v) if !v.trim().is_empty() => Ok(v),
        Some(_) => Err(Error::invalid(format!(
            "`{field}` is blank; state it explicitly"
        ))),
        None => Err(Error::invalid(format!(
            "`{field}` is not declared; it is required"
        ))),
    }
}

/// A required value of any type: present, else a refusal naming the field.
pub(crate) fn required<T>(field: &str, value: Option<T>) -> Result<T, Error> {
    value.ok_or_else(|| Error::invalid(format!("`{field}` is not declared; it is required")))
}

/// A required list that must hold at least one non-blank entry.
pub(crate) fn non_empty(field: &str, value: Option<Vec<String>>) -> Result<Vec<String>, Error> {
    let list = required(field, value)?;
    if list.is_empty() || list.iter().any(|s| s.trim().is_empty()) {
        return Err(Error::invalid(format!(
            "`{field}` must list at least one non-blank entry"
        )));
    }
    Ok(list)
}
