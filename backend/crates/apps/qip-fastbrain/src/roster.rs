//! The check that makes the fast path's guarantee real.
//!
//! The rule is that **nothing on the fast path waits for a language model**,
//! and it is checked rather than assumed: the node refuses to run if any agent
//! it would host holds `call_language_model`, has a budget permitting one, or
//! may run for longer than the fast path allows. A fast path that blocks on a
//! model call is not a fast path, and discovering that under load is expensive.
//!
//! It runs before the platform is assembled and before the listener is bound,
//! so a node that fails it has done nothing an operator has to undo.

use qip_agents::capability::Capability;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_investment_agents::manifests;

/// The agents this node hosts.
pub const FAST_PATH_AGENTS: &[&str] = &[manifests::ids::MICROSTRUCTURE];

/// The longest a fast-path agent may take.
///
/// Fifty milliseconds is generous for what these agents do and tight enough
/// that anything reaching for a network call fails.
pub const MAXIMUM_BUDGET: Duration = Duration::from_millis(50);

/// One agent the roster check cleared, and what it cleared it for.
///
/// Carried out of the check rather than printed inside it, so the start-up
/// banner and a test read the same values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClearedAgent {
    pub id: String,
    pub wall_time: Duration,
    pub tool_calls: u32,
}

/// Everything the roster check established, for the banner and the health
/// surface to report.
///
/// A health response that says `"roster_validated": true` is only worth reading
/// because this value cannot be constructed except by passing the check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClearedRoster {
    pub agents: Vec<ClearedAgent>,
    pub ceiling: Duration,
}

impl ClearedRoster {
    /// The agent ids, in the order they were checked.
    pub fn ids(&self) -> Vec<&str> {
        self.agents.iter().map(|agent| agent.id.as_str()).collect()
    }
}

/// Clear the roster, or refuse.
///
/// Every hosted agent is checked; the first failure stops the node. The three
/// refusals are separate on purpose — "holds the capability", "has a budget for
/// it" and "may run long enough to make one" are three different ways to end up
/// waiting on a model, and an operator reading the message should be told which
/// one happened rather than that "the roster is invalid".
pub fn clear(now: Timestamp) -> Result<ClearedRoster> {
    clear_agents(FAST_PATH_AGENTS, MAXIMUM_BUDGET, now)
}

/// The check itself, over an explicit agent list and ceiling.
///
/// Parameterised so a test can hold the check against an agent that should fail
/// it. A check that can only be exercised by the roster it is deployed with is
/// a check nobody has seen refuse anything.
pub fn clear_agents(agents: &[&str], ceiling: Duration, now: Timestamp) -> Result<ClearedRoster> {
    let roster = manifests::roster(now);
    let mut cleared = Vec::with_capacity(agents.len());

    for id in agents {
        let manifest = roster
            .get(id)
            .ok_or_else(|| Error::not_found(format!("{id} is not on the roster")))?;

        // The check that makes the guarantee real.
        if manifest
            .capabilities
            .contains(Capability::CallLanguageModel)
        {
            return Err(Error::denied(format!(
                "{id} holds call_language_model; the fast path must never wait on a model"
            )));
        }
        if manifest.budget.language_model_calls > 0 {
            return Err(Error::denied(format!(
                "{id} has a budget for {} language-model call(s); the fast path must never wait on a model",
                manifest.budget.language_model_calls
            )));
        }
        if manifest.budget.wall_time > ceiling {
            return Err(Error::denied(format!(
                "{id} may run for {:?}, beyond the {ceiling:?} the fast path allows",
                manifest.budget.wall_time
            )));
        }
        manifest.authorisation(now)?;

        cleared.push(ClearedAgent {
            id: (*id).to_string(),
            wall_time: manifest.budget.wall_time,
            tool_calls: manifest.budget.tool_calls,
        });
    }

    Ok(ClearedRoster {
        agents: cleared,
        ceiling,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    #[test]
    fn the_agents_this_node_hosts_hold_no_language_model_capability_and_no_budget_for_one() {
        let cleared = clear(now()).expect("the deployed roster clears its own check");
        assert!(
            !cleared.agents.is_empty(),
            "the check cleared nothing, so it proves nothing about this node"
        );
        for agent in &cleared.agents {
            assert!(
                agent.wall_time <= MAXIMUM_BUDGET,
                "{} cleared with a {:?} budget, beyond the {:?} ceiling",
                agent.id,
                agent.wall_time,
                MAXIMUM_BUDGET
            );
        }
    }

    #[test]
    fn an_agent_that_may_call_a_language_model_is_refused_rather_than_hosted() {
        // The premise: this agent really does hold the capability, so the
        // refusal below is the check working rather than a lookup failure.
        let roster = manifests::roster(now());
        let chief = roster
            .get(manifests::ids::CHIEF)
            .expect("the chief is on the roster");
        assert!(
            chief.capabilities.contains(Capability::CallLanguageModel)
                || chief.budget.language_model_calls > 0,
            "the chief neither holds call_language_model nor budgets for one, so \
             hosting it would not exercise the refusal"
        );

        let refusal = clear_agents(&[manifests::ids::CHIEF], MAXIMUM_BUDGET, now())
            .expect_err("an agent that can reach a model must not be hosted here");
        assert!(
            refusal.message().contains("must never wait on a model"),
            "the refusal does not say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn an_agent_whose_budget_exceeds_the_ceiling_is_refused_even_with_no_model_capability() {
        // A ceiling of one nanosecond no real agent can meet, so what fails is
        // the wall-time comparison and not the capability check.
        let refusal = clear_agents(FAST_PATH_AGENTS, Duration::from_nanos(1), now())
            .expect_err("no agent runs inside a nanosecond");
        assert!(
            refusal.message().contains("beyond the"),
            "the refusal is not the budget refusal: {}",
            refusal.message()
        );
    }

    #[test]
    fn an_agent_that_is_not_on_the_roster_is_a_missing_agent_rather_than_a_cleared_one() {
        let refusal = clear_agents(&["no-such-agent"], MAXIMUM_BUDGET, now())
            .expect_err("an unknown agent cannot be cleared");
        assert!(
            refusal.message().contains("is not on the roster"),
            "the refusal does not name the problem: {}",
            refusal.message()
        );
    }
}
