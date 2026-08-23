//! The handover to ingestion.
//!
//! A source the finder approves has to become something
//! [`qip_market_ingestion::adapter::DataAdapter`] can poll, or the whole
//! lifecycle ends in a report nobody can act on. This module produces the
//! adapter's own [`SourceDescriptor`] from a registration, so "approved"
//! and "ingestible" are the same fact rather than two that can drift.
//!
//! The plan carries the delivery mode explicitly because `poll(until)` leaves
//! the clock with the caller. A pull mechanism maps onto it directly; a push
//! one has to be buffered by its adapter and drained on `poll`, and an adapter
//! author who is not told that writes one that blocks.

use crate::decision::RegisteredSource;
use crate::endpoint::Delivery;
use qip_core::Duration;
use qip_financial::quality::LicensingClass;
use qip_market_ingestion::adapter::SourceDescriptor;
use serde::{Deserialize, Serialize};

/// Everything an adapter author needs to poll an approved source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestionPlan {
    /// The adapter's stable name, recorded as the provenance source.
    pub adapter_name: String,
    pub url: String,
    pub mechanism: String,
    pub delivery: Delivery,
    /// The shortest interval a poll may repeat at without breaching the
    /// source's policy. Already the maximum of crawl-delay and rate limit.
    pub min_poll_interval: Duration,
    /// Delay a caller should expect between an event and this source
    /// reporting it, as measured when probed.
    pub expected_latency: Duration,
    /// Whether a poll can ask only for what changed.
    pub incremental: bool,
    /// The schema fingerprint at registration. An adapter comparing against
    /// this is what turns drift into an alarm rather than a surprise.
    pub schema_fingerprint: String,
    /// What production must supply before the first poll succeeds.
    pub credential_required: Option<String>,
}

impl IngestionPlan {
    /// Whether an adapter over this source must buffer arrivals.
    pub fn requires_buffering(&self) -> bool {
        matches!(self.delivery, Delivery::PushBuffered)
    }
}

/// The ingestion handover for a registered source.
pub fn plan_for(registered: &RegisteredSource) -> IngestionPlan {
    let endpoint = registered.source().endpoint();
    let poll = endpoint.mechanism().poll_plan();
    IngestionPlan {
        adapter_name: registered.id().to_string(),
        url: endpoint.url(),
        mechanism: endpoint.mechanism().kind().to_string(),
        delivery: poll.delivery,
        // The policy is a floor and the mechanism's cadence is a ceiling on
        // usefulness; polling faster than the policy breaches it, so the
        // policy wins wherever the two disagree.
        min_poll_interval: registered
            .policy()
            .min_request_interval()
            .max(poll.natural_interval),
        expected_latency: registered.source().evidence().observed_latency(),
        incremental: poll.incremental,
        schema_fingerprint: registered.source().schema().fingerprint().to_string(),
        credential_required: poll.credential_required,
    }
}

/// The registration rendered as the ingestion crate's own descriptor.
///
/// `production_requirement` is populated whenever the source needs a
/// credential this build cannot hold, so an adapter built from this
/// descriptor reports what it is missing instead of appearing to work.
pub fn descriptor_for(registered: &RegisteredSource) -> SourceDescriptor {
    let plan = plan_for(registered);
    let licensing = licensing_class(registered);
    SourceDescriptor {
        name: registered.id().to_string(),
        provider: registered.source().identity().publisher().to_string(),
        licensing,
        topics: registered
            .source()
            .candidate()
            .produces()
            .iter()
            .copied()
            .collect(),
        expected_latency: plan.expected_latency,
        production_requirement: plan.credential_required,
    }
}

/// Map the finder's licensing posture onto the ingestion crate's class.
///
/// A source with no readable licence never reaches here — it is refused
/// before registration — so the only distinction left is whether the licence
/// permits redistribution.
fn licensing_class(registered: &RegisteredSource) -> LicensingClass {
    use qip_contracts::governance::Usage;
    match registered.source().licensing().license() {
        Some(license) if license.permits().contains(&Usage::Redistribute) => LicensingClass::Public,
        Some(_) => LicensingClass::Licensed,
        // Unreachable through the finder, and mapped to the most restrictive
        // class rather than asserted away: a panic here would be a crash in
        // the ingestion path over a governance question.
        None => LicensingClass::Restricted,
    }
}
