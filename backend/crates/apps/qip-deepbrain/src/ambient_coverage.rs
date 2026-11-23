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

/// The one finding a sweep's attention events compose, or `None` where the
/// router returned none.
///
/// `None` rather than a finding stating zero: a sweep in which the router
/// decided nothing evidences no routing, and a zero under the scope's label
/// would cover the scope on a sweep where the router was absent just as well
/// as on one where it was quiet.
pub fn attention_finding(
    events: &[AttentionEvent],
    now: Timestamp,
) -> Result<Option<AgentFinding>> {
    let Some(first) = events.first() else {
        return Ok(None);
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
    Ok(Some(finding))
}
