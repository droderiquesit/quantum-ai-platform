//! The finding an ambient brain covers its scope by (R12-AMBIENT-3027,
//! ADR 0725).
//!
//! The attention router is registered in the Brain Registry, and a registry
//! entry is a record that a brain exists: remove the router and leave the
//! entry and the registry still reads as covering attention routing. Coverage
//! counts what reported, never what was constructed, so the router's scope is
//! covered only by a finding composed here from the attention events one
//! sweep's router actually returned. No router, no events, no finding, no
//! coverage.
//!
//! The finding is advisory and carries no direction and no conviction: the
//! router decides what wakes, it holds no view on a market. It is composed
//! by the deep brain rather than emitted by an organisation agent because the
//! router has no seat on the Desk's roster and reads no brief.
//!
//! One finding per round, as the owner's ruling of 2026-10-31 says, quiet
//! rounds included. A round whose router returned nothing composes a finding
//! that says so, with its counts observed from the router at that round; it
//! is what a loop without a router cannot compose, so the gauge reads 1 on a
//! quiet round and 0 only when the router is gone. An earlier version
//! composed nothing on a quiet round, which made a quiet router and a removed
//! one read the same and departed from the ruling; two readers refuted it on
//! that ground on 2026-11-23.

use qip_agents::finding::{AgentFinding, NumericFact};
use qip_contracts::ambient::{AttentionEvent, Disposition, Pathway};
use qip_core::error::Result;
use qip_core::{AgentRunId, Timestamp};
use std::collections::BTreeMap;

use crate::brains::AMBIENT_ATTENTION;

/// The fact label `AmbientScope::AttentionRouting` is evidenced by: how many
/// routing decisions the router returned in the sweep.
pub const ATTENTION_ROUTING_FACT: &str = "attention_routing";

/// How many of those decisions activated a pathway.
pub const ATTENTION_ACTIVATED_FACT: &str = "attention_activated";

/// The fact label counting the activations one pathway received.
pub const fn woke_fact(pathway: Pathway) -> &'static str {
    match pathway {
        Pathway::ResearchTask => "attention_woke_research_task",
        Pathway::SpecialistActivation => "attention_woke_specialist_activation",
        Pathway::RiskReview => "attention_woke_risk_review",
        Pathway::CapitalPreparation => "attention_woke_capital_preparation",
        Pathway::ReflexModelUpdate => "attention_woke_reflex_model_update",
    }
}

/// The one finding a round's router composes from the attention events it
/// returned.
///
/// The caller composes it only for a loop that holds a router (see
/// `ResearchLoop::sweep_over`), so a zero under the scope's label never stands
/// for an absent router: an absent router composes no finding at all.
pub fn attention_finding(events: &[AttentionEvent], now: Timestamp) -> Result<AgentFinding> {
    let Some(first) = events.first() else {
        return quiet_finding(now);
    };
    let ids: Vec<String> = events.iter().map(|event| event.id.clone()).collect();
    let mut woke: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut activated = 0usize;
    for event in events {
        if event.disposition == Disposition::Activated {
            activated += 1;
            *woke.entry(woke_fact(event.pathway)).or_default() += 1;
        }
    }
    let mut finding = AgentFinding::new(
        AgentRunId::from_string(format!("{AMBIENT_ATTENTION}/{}", first.id)),
        AMBIENT_ATTENTION,
        now,
        now,
        format!(
            "the attention router returned {} routing decision(s) this sweep and activated {activated}",
            events.len()
        ),
    );
    // Counts crossing into `f64`: a statistic about the sweep, not money.
    finding.facts.push(NumericFact::computed(
        ATTENTION_ROUTING_FACT,
        events.len() as f64,
        "events",
        AMBIENT_ATTENTION,
        ids.clone(),
    ));
    finding.facts.push(NumericFact::computed(
        ATTENTION_ACTIVATED_FACT,
        activated as f64,
        "events",
        AMBIENT_ATTENTION,
        ids.clone(),
    ));
    for (label, count) in woke {
        finding.facts.push(NumericFact::computed(
            label,
            count as f64,
            "events",
            AMBIENT_ATTENTION,
            ids.clone(),
        ));
    }
    finding.evidence = ids;
    finding.falsifiers.push(
        "an attention event named in this finding's evidence has no Attention record in the \
         ambient journal for the sweep"
            .to_string(),
    );
    finding.validate()?;
    Ok(finding)
}

/// The finding of a round in which the router was consulted and returned no
/// decision. Its two counts are observed from the router at the round, since
/// there is no event to compute them from and a computed fact with no inputs
/// is refused by `NumericFact::validate`.
fn quiet_finding(now: Timestamp) -> Result<AgentFinding> {
    let round = format!("{AMBIENT_ATTENTION}/quiet/{}", now.as_nanos());
    let mut finding = AgentFinding::new(
        AgentRunId::from_string(round.clone()),
        AMBIENT_ATTENTION,
        now,
        now,
        "the attention router returned no routing decision this sweep".to_string(),
    );
    for label in [ATTENTION_ROUTING_FACT, ATTENTION_ACTIVATED_FACT] {
        finding.facts.push(NumericFact::observed(
            label,
            0.0,
            "events",
            AMBIENT_ATTENTION,
            now,
            round.clone(),
        ));
    }
    finding.falsifiers.push(
        "the ambient journal holds an Attention record for this sweep, so the router did return \
         a decision"
            .to_string(),
    );
    finding.validate()?;
    Ok(finding)
}
