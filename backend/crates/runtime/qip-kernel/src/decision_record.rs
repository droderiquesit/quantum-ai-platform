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
use qip_core::deployment::Deployment;
use qip_core::error::Error;
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

/// The source commit this build was stamped with, read at compile time.
///
/// `infrastructure/docker/Dockerfile` requires `QIP_SOURCE_COMMIT` as a build
/// argument — the commit `deploy.yml` and the private pool build from, never
/// a repository variable — and exports it into `cargo build`, so every image
/// carries the commit it was built from. Until 2026-10-11 nothing set it,
/// and every deployed `code_version` was the crate version alone: the same
/// string for every commit since the version was last bumped.
pub const SOURCE_COMMIT: Option<&str> = option_env!("QIP_SOURCE_COMMIT");

/// This build.
pub fn code_version() -> String {
    match SOURCE_COMMIT {
        Some(commit) => format!("qip-kernel {} ({commit})", env!("CARGO_PKG_VERSION")),
        None => format!("qip-kernel {}", env!("CARGO_PKG_VERSION")),
    }
}

/// Refuse a deployed process whose build names no source commit.
///
/// R12-GOV-6121's `code_version` is the one version a person cannot
/// recompute from the log: the limits, the graph, the log position and the
/// controls are all on the record, the code is not. A deployment that cannot
/// say which commit decided is one whose every decision is unreproducible,
/// so it does not start. Only [`Deployment::Local`] — a developer's `cargo
/// run` — may run unstamped. A stamp that is not a full 40-hex commit is
/// refused too: an abbreviated or decorated id is one a later reader may not
/// be able to resolve.
pub fn check_source_commit(stamped: Option<&str>, deployment: Deployment) -> Result<()> {
    if deployment == Deployment::Local {
        return Ok(());
    }
    match stamped {
        Some(commit)
            if commit.len() == 40
                && commit
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
        {
            Ok(())
        }
        Some(commit) => Err(Error::denied(format!(
            "this build is stamped with QIP_SOURCE_COMMIT={commit:?}, which is not a full \
             lowercase commit sha, so its decisions would name code nobody can check out. Build \
             it with --build-arg QIP_SOURCE_COMMIT=$(git rev-parse HEAD)"
        ))),
        None => Err(Error::denied(format!(
            "QIP_DEPLOYMENT={deployment} and this build names no source commit, so every \
             decision it journals would say `{}` and no more (R12-GOV-6121). Build the image \
             through infrastructure/docker/Dockerfile, which requires QIP_SOURCE_COMMIT; only \
             QIP_DEPLOYMENT=local may run unstamped",
            code_version()
        ))),
    }
}

/// [`check_source_commit`] for this build. Called by every composition root
/// that runs the cycle, once the deployment is known and before anything
/// is served.
pub fn require_source_commit(deployment: Deployment) -> Result<()> {
    check_source_commit(SOURCE_COMMIT, deployment)
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
