//! `qip registrations propose --candidates <file>` — the adapter a person
//! would build for each source the finder approves (R12-DATA-2002).
//!
//! The finder already turns a registration into the ingestion crate's own
//! plan and descriptor (`qip_data_finder::ingestion::plan_for` and
//! `descriptor_for`), and nothing outside tests ever called either: an
//! approved source ended in a decision nobody could act on. ADR 0157 §4 and
//! §5 keep it that way for the running platform — no descriptor polls and no
//! adapter is generated from a registration — and allow tooling to emit a
//! proposal for a person to read. This is that tooling, and it is only that:
//!
//! * **A proposal, never an adapter.** The output is printed. Nothing here
//!   writes a manifest, a connector or a configuration file, and the
//!   candidate catalogue the deep brain loads (`QIP_DEEPBRAIN_SOURCE_
//!   CANDIDATES_PATH`) never reads what this prints.
//! * **Only for what the finder approved.** A candidate refused, deferred or
//!   quarantined yields its reason and no plan: a plan for a source whose
//!   terms forbid use would read as permission.
//! * **The finder's own decision.** Each candidate is assessed through
//!   `Platform::assess_sources`, the seam the deep brain's discovery desk
//!   uses, so the licence, robots and manipulation checks are the ones that
//!   would apply in a deployment rather than a second copy of them.

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_data_finder::catalogue::CandidateEntry;
use qip_data_finder::decision::DecisionOutcome;
use qip_data_finder::ingestion::{IngestionPlan, descriptor_for, plan_for};
use qip_data_finder::probe::{NetworkProbe, SourceProbe};
use qip_kernel::Platform;
use qip_market_ingestion::adapter::SourceDescriptor;

/// The user agent a proposal's probes identify as: an operator's review,
/// not the deep brain's discovery pass.
pub const PROPOSAL_USER_AGENT: &str = "qip-cli-adapter-proposal/1.0";

/// What the finder made of one candidate.
#[derive(Clone, Debug, PartialEq)]
pub enum Proposal {
    /// Approved: the adapter a person would build to ingest it.
    Adapter {
        source: String,
        plan: Box<IngestionPlan>,
        descriptor: Box<SourceDescriptor>,
    },
    /// Not approved, so no adapter is proposed, and the finder's reason.
    None { source: String, reason: String },
}

impl Proposal {
    pub fn source(&self) -> &str {
        match self {
            Self::Adapter { source, .. } | Self::None { source, .. } => source,
        }
    }

    /// The lines a reviewer reads.
    pub fn lines(&self) -> Vec<String> {
        match self {
            Self::Adapter {
                source,
                plan,
                descriptor,
            } => {
                let mut lines = vec![
                    format!("{source}: PROPOSED adapter `{}`", plan.adapter_name),
                    format!("  mechanism:     {} ({:?})", plan.mechanism, plan.delivery),
                    format!("  url:           {}", plan.url),
                    format!("  poll no faster than {:?}", plan.min_poll_interval),
                    format!("  incremental:   {}", plan.incremental),
                    format!("  schema:        {}", plan.schema_fingerprint),
                    format!(
                        "  provider:      {} ({:?})",
                        descriptor.provider, descriptor.licensing
                    ),
                ];
                if plan.requires_buffering() {
                    lines.push(
                        "  push source: the adapter must buffer arrivals and drain them on poll"
                            .to_string(),
                    );
                }
                if let Some(credential) = &plan.credential_required {
                    lines.push(format!("  needs before the first poll: {credential}"));
                }
                lines
            }
            Self::None { source, reason } => {
                vec![format!("{source}: no adapter proposed: {reason}")]
            }
        }
    }
}

/// Assess every entry through the platform and propose an adapter for each
/// one the finder registers. `probe` replaces the network for a test; with
/// `None`, each entry is probed through its own reviewed egress route, as
/// the deep brain's discovery desk does.
pub fn propose(
    platform: &mut Platform,
    entries: &[CandidateEntry],
    mut probe: Option<&mut dyn SourceProbe>,
    now: Timestamp,
) -> Result<Vec<Proposal>> {
    let mut proposals = Vec::with_capacity(entries.len());
    for entry in entries {
        let candidates = vec![entry.candidate.clone()];
        let assessment = match probe.as_deref_mut() {
            Some(probe) => platform.assess_sources(candidates, probe, now)?,
            None => {
                let mut network = NetworkProbe::through(&entry.egress_route, PROPOSAL_USER_AGENT)?;
                platform.assess_sources(candidates, &mut network, now)?
            }
        };
        for decision in &assessment.decisions {
            let source = decision.source_id().to_string();
            let proposal = match decision.outcome() {
                DecisionOutcome::Registered(_) => {
                    match platform.registered_sources().get(&source) {
                        Some(registered) => Proposal::Adapter {
                            source,
                            plan: Box::new(plan_for(registered)),
                            descriptor: Box::new(descriptor_for(registered)),
                        },
                        // Registered by the decision and absent from the
                        // registry is a disagreement inside the finder; said,
                        // not papered over with a plan built from the decision.
                        None => Proposal::None {
                            reason: "registered by the decision but absent from the registry"
                                .to_string(),
                            source,
                        },
                    }
                }
                DecisionOutcome::Rejected { reason } => Proposal::None {
                    source,
                    reason: format!("refused: {reason}"),
                },
                DecisionOutcome::Deferred { reason } => Proposal::None {
                    source,
                    reason: format!("deferred: {reason}"),
                },
                DecisionOutcome::Quarantined { reason, .. } => Proposal::None {
                    source,
                    reason: format!("quarantined: {reason}"),
                },
            };
            proposals.push(proposal);
        }
    }
    Ok(proposals)
}
