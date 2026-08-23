//! The check that makes this node's guarantee real.
//!
//! The rule is that **nothing the deep brain hosts can reach a venue**. Order
//! submission belongs to the execution path; this node reasons about what
//! should happen and never makes it happen. The check is run rather than
//! assumed: the node refuses to start if any agent it would host holds a
//! market-touching capability.
//!
//! It is the mirror image of `qip-fastbrain`'s check and not a copy of it. The
//! fast brain refuses agents that could *think slowly* — anything holding
//! `call_language_model` or budgeting for one. This node hosts precisely those
//! agents on purpose, and refuses the one thing the fast brain is for. Between
//! them the two refusals partition the roster, which is why each node names its
//! own and neither defers to a shared list.
//!
//! It runs before the platform is assembled and before the listener is bound,
//! so a node that fails it has done nothing an operator has to undo.

use qip_agents::capability::Capability;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_investment_agents::manifests;

/// The agent this node deliberately does not host.
///
/// Named as the exclusion rather than listing the seventeen that are hosted,
/// because the hosted set is "the organisation", and a literal list here would
/// silently stop covering an agent added to the roster tomorrow — which is the
/// agent most worth checking.
pub const NOT_HOSTED: &[&str] = &[manifests::ids::EXECUTION];

/// One agent the check cleared, and what it cleared it for.
///
/// Carried out of the check rather than printed inside it, so the start-up
/// banner, the health surface and a test read the same values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClearedAgent {
    pub id: String,
    /// How long this agent may run. Reported rather than bounded: on this node
    /// a long budget is the point, and a ceiling here would be the fast brain's
    /// rule applied where it does not belong.
    pub wall_time: Duration,
    /// Whether this agent may consult a language model. Permitted here, and
    /// worth surfacing because it is the property that makes a cycle's
    /// duration unpredictable.
    pub may_call_a_model: bool,
}

/// Everything the check established, for the banner and the health surface.
///
/// A health response saying `"roster_validated": true` is only worth reading
/// because this value cannot be constructed except by passing the check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClearedRoster {
    pub agents: Vec<ClearedAgent>,
    /// The agents excluded from this node, so the banner can say what is
    /// missing as well as what is present.
    pub excluded: Vec<String>,
}

impl ClearedRoster {
    /// The agent ids, in the order they were checked.
    pub fn ids(&self) -> Vec<&str> {
        self.agents.iter().map(|agent| agent.id.as_str()).collect()
    }

    /// How many hosted agents may consult a language model.
    ///
    /// In the banner because it is the honest answer to "why might a cycle here
    /// take minutes": this many of them can block on somebody else's service.
    pub fn model_callers(&self) -> usize {
        self.agents
            .iter()
            .filter(|agent| agent.may_call_a_model)
            .count()
    }
}

/// Clear the roster, or refuse.
pub fn clear(now: Timestamp) -> Result<ClearedRoster> {
    clear_excluding(NOT_HOSTED, now)
}

/// The check itself, over an explicit exclusion list.
///
/// Parameterised so a test can hold the check against a roster that *does*
/// include a market-touching agent. A check that can only be exercised by the
/// roster it is deployed with is a check nobody has seen refuse anything.
pub fn clear_excluding(excluded: &[&str], now: Timestamp) -> Result<ClearedRoster> {
    let roster = manifests::roster(now);
    let mut cleared = Vec::new();

    for manifest in roster.iter() {
        let id = manifest.id.as_str();
        if excluded.contains(&id) {
            continue;
        }

        for capability in manifest.capabilities.iter() {
            if capability.touches_market() {
                return Err(Error::denied(format!(
                    "{id} holds {capability}; the deep brain hosts no agent that can reach a venue"
                )));
            }
        }
        // The same expiry check every node makes. An agent whose authorisation
        // lapsed is not cleared by the absence of a market capability.
        manifest.authorisation(now)?;

        cleared.push(ClearedAgent {
            id: id.to_string(),
            wall_time: manifest.budget.wall_time,
            may_call_a_model: manifest
                .capabilities
                .contains(Capability::CallLanguageModel)
                || manifest.budget.language_model_calls > 0,
        });
    }

    if cleared.is_empty() {
        return Err(Error::invalid(
            "the deep brain cleared no agents at all; a node hosting nothing has nothing to \
             research and would report itself healthy while doing it",
        ));
    }

    Ok(ClearedRoster {
        agents: cleared,
        excluded: excluded.iter().map(|id| (*id).to_string()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    #[test]
    fn every_agent_this_node_hosts_is_one_that_cannot_reach_a_venue() {
        let cleared = clear(now()).expect("the deployed roster clears its own check");
        assert!(
            !cleared.agents.is_empty(),
            "the check cleared nothing, so it proves nothing about this node"
        );

        // The premise, asserted rather than assumed: the roster really does
        // contain a market-touching agent, so the clear above is an exclusion
        // that worked and not a roster that never had one.
        let roster = manifests::roster(now());
        let execution = roster
            .get(manifests::ids::EXECUTION)
            .expect("the execution trader is on the roster");
        assert!(
            execution
                .capabilities
                .iter()
                .any(|capability| capability.touches_market()),
            "the execution trader holds no market capability, so excluding it proves nothing"
        );
        assert!(!cleared.ids().contains(&manifests::ids::EXECUTION));
    }

    #[test]
    fn an_agent_that_can_reach_a_venue_is_refused_rather_than_hosted() {
        let refusal = clear_excluding(&[], now())
            .expect_err("hosting the whole roster hosts an agent that can submit an order");
        assert!(
            refusal.message().contains("can reach a venue"),
            "the refusal does not say why: {}",
            refusal.message()
        );
        assert!(
            refusal.message().contains(manifests::ids::EXECUTION),
            "the refusal does not name the agent that failed it: {}",
            refusal.message()
        );
    }

    #[test]
    fn this_node_deliberately_hosts_agents_that_may_call_a_language_model() {
        // The property that separates this node from the fast brain, asserted
        // so that a change making the deep brain model-free would fail here
        // rather than quietly turning it into a second fast brain.
        let cleared = clear(now()).expect("the roster clears");
        assert!(
            cleared.model_callers() > 0,
            "no hosted agent may consult a model, which is the fast brain's rule, not this one's"
        );
    }

    #[test]
    fn a_node_that_would_host_nothing_is_refused_rather_than_started_empty() {
        let every_id: Vec<String> = manifests::roster(now())
            .iter()
            .map(|manifest| manifest.id.to_string())
            .collect();
        let excluded: Vec<&str> = every_id.iter().map(String::as_str).collect();

        let refusal = clear_excluding(&excluded, now())
            .expect_err("a node hosting no agent at all is not a deep brain");
        assert!(
            refusal.message().contains("nothing to research"),
            "the refusal does not say what is wrong: {}",
            refusal.message()
        );
    }

    #[test]
    fn nothing_is_cleared_whose_authorisation_has_lapsed_at_the_moment_it_is_checked() {
        // The capability check is not the only gate. Asserted against the
        // manifests directly rather than by moving the clock, because
        // `manifests::roster` stamps every manifest as reviewed at the instant
        // it is asked for — so there is no timestamp at which this roster is
        // both real and expired, and a test that pretended otherwise would be
        // asserting against a fixture rather than against the check.
        let cleared = clear(now()).expect("the roster clears");
        let roster = manifests::roster(now());
        for agent in &cleared.agents {
            let manifest = roster
                .get(&agent.id)
                .expect("a cleared agent is on the roster");
            assert!(
                manifest.authorisation(now()).is_ok(),
                "{} was cleared with an authorisation that does not hold at the checking time",
                agent.id
            );
        }
    }
}
