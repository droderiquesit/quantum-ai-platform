//! R12-GOV-6121: every DECIDE leaves, on the hash-chained log beside its
//! decisions, the versions of the code, policy, model, data and controls it
//! ran under.
//!
//! The failure prevented is a decision that cannot be reproduced because
//! nobody recorded what produced it. The log already held each decision; it
//! did not hold which build made it, which limit set bounded it, which world
//! model's causal graph informed it, how much of the log it had read, or
//! where the kill switch and autonomy stood. A replay months later would
//! have to assume all five were what they are today.
//!
//! [`DecisionVersions`] carries all five as identifiers a person can check:
//!
//! * `code_version` — the kernel's crate version and, when the build stamped
//!   one through `QIP_SOURCE_COMMIT`, the source commit;
//! * `policy_version` — a SHA-256 of the limit set's canonical JSON, the
//!   policy every pre-trade check in the stage reads;
//! * `model_version` — [`model_version`], a SHA-256 over the world model's
//!   causal edges in force at the instant (cause, effect, mechanism, signed
//!   transmission), so two DECIDEs on the same graph carry the same version
//!   and any re-estimation, addition or retirement changes it;
//! * `data_version` — the hash of the newest record in the event log when
//!   the stage ran: everything the decision could have read is chained
//!   behind it;
//! * `control_version` — the autonomy level, its ceiling and whether the
//!   global kill switch is tripped.
//!
//! The record decodes only with every field present (`deny_unknown_fields`
//! and no defaults), so a record with one dropped fails replay rather than
//! reading as "unknown". The same [`model_version`] is what an episode
//! formed in REASON now carries in `model_version`, where it used to carry
//! `None`.

use crate::platform::Platform;
use qip_core::{Result, Timestamp, sha256_hex};
use qip_events::{EventBody, Topic};
use qip_streaming::envelope::StreamEnvelope;
use qip_world_model::causal::CausalGraph;
use serde::{Deserialize, Serialize};

/// The producer every versions record carries.
pub const DECISION_VERSIONS_ORIGIN: &str = "kernel/decision-versions";

/// The five versions one DECIDE ran under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionVersions {
    pub cycle: u64,
    pub code_version: String,
    pub policy_version: String,
    pub model_version: String,
    pub data_version: String,
    pub control_version: String,
}

impl EventBody for DecisionVersions {
    /// A record about how a decision was governed, read with the other
    /// governance records on this topic and told apart by producer; the
    /// topic's class is irreplaceable, so the log never evicts one.
    const TOPIC: Topic = Topic::ComplianceEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}

/// This build.
pub fn code_version() -> String {
    match option_env!("QIP_SOURCE_COMMIT") {
        Some(commit) => format!("qip-kernel {} ({commit})", env!("CARGO_PKG_VERSION")),
        None => format!("qip-kernel {}", env!("CARGO_PKG_VERSION")),
    }
}

/// A digest of the world model's causal edges in force at `known_at`.
pub fn model_version(graph: &CausalGraph, known_at: Timestamp) -> String {
    let mut lines: Vec<String> = graph
        .edges()
        .iter()
        .filter(|edge| !edge.retired_by(known_at))
        .map(|edge| {
            format!(
                "{}|{}|{}|{}",
                edge.cause,
                edge.effect,
                edge.mechanism.as_str(),
                edge.signed_transmission()
            )
        })
        .collect();
    // Sorted so the version is a fact about the graph, not about the order
    // edges happened to be added in.
    lines.sort();
    format!(
        "world-model:sha256:{}",
        sha256_hex(lines.join("\n").as_bytes())
    )
}

/// The five versions `platform` is deciding under at `now`.
pub fn versions(platform: &Platform, now: Timestamp) -> Result<DecisionVersions> {
    let limits = serde_json::to_string(platform.limits())?;
    let autonomy = platform.autonomy();
    let data_version = platform
        .event_log()
        .records()
        .last()
        .map(|record| record.record_hash.clone())
        .unwrap_or_else(|| qip_events::log::GENESIS_HASH.to_string());
    Ok(DecisionVersions {
        cycle: platform.cycle_count(),
        code_version: code_version(),
        policy_version: format!("limits:sha256:{}", sha256_hex(limits.as_bytes())),
        model_version: model_version(platform.world().causal(), now),
        data_version: format!("log:{data_version}"),
        control_version: format!(
            "autonomy:{:?}/ceiling:{:?}/kill_switch:{}",
            autonomy.level(),
            autonomy.ceiling(),
            autonomy.kill_switch().is_globally_tripped()
        ),
    })
}

/// Compute and journal the versions for this DECIDE.
pub fn journal(platform: &mut Platform, now: Timestamp) -> Result<DecisionVersions> {
    let record = versions(platform, now)?;
    platform.journal_record(record.clone(), DECISION_VERSIONS_ORIGIN, now)?;
    Ok(record)
}

/// Every versions record in the platform's log, in order.
pub fn recorded(platform: &Platform) -> Result<Vec<DecisionVersions>> {
    let mut out = Vec::new();
    for event in platform.event_log().events() {
        if event.topic == Topic::ComplianceEvaluated
            && event.lineage.producer == DECISION_VERSIONS_ORIGIN
        {
            out.push(
                StreamEnvelope::from_frame(event)?
                    .decode::<DecisionVersions>()?
                    .body,
            );
        }
    }
    Ok(out)
}
