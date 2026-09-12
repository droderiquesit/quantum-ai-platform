//! A production caller for source discovery (blueprint §7.4-§7.6.2).
//!
//! `Platform::assess_sources` — ASSESS, REGISTER and SCORE over a candidate
//! list, wrapping `qip_data_finder::DataFinder::assess` — was built and
//! tested and reachable from nothing outside `qip-kernel`'s own tests and
//! `qip-acceptance`'s end-to-end suite. `grep -rn assess_sources
//! --include=*.rs backend/crates/apps` returned nothing before this module:
//! the whole discovery lifecycle was proven correct and never once run by a
//! deployed process.
//!
//! # What this gives it, and what it does not
//!
//! A cadence and a candidate list, which is exactly the shape this crate
//! already uses for the evolution loop ([`crate::evolution::EvolutionConfig`])
//! and the learning desk ([`crate::learning::LearningConfig`]): a knob a
//! deployment can turn to zero, checked here rather than left to whichever
//! caller remembers to gate it.
//!
//! It does **not** build the CRAWL stage §7.4 asks for — seeding, following
//! links, expanding into related domains. That stays absent
//! (`grep -rn 'follow_links\|expand_from\|fn crawl(' backend/crates/services/qip-data-finder/src`
//! finds nothing, and this module does not change that). The candidate list
//! is stated by an operator, in a committed file, for the same reason
//! `qip-deepbrain`'s universe and `qip-api`'s capital-fabric declaration are:
//! nothing in this workspace discovers a source on its own, and a stage that
//! invented candidates in order to have something to assess would be a
//! control assessing inputs it manufactured.
//!
//! # The probe
//!
//! [`qip_data_finder::probe::NetworkProbe`] is the one production
//! implementation of [`qip_data_finder::probe::SourceProbe`], and it refuses
//! every call by name: `NetworkProbe::TRANSPORT_REQUIREMENT` states that no
//! HTTP/1.1-with-TLS client is linked into this build (ADR 0009). Attaching
//! it here is therefore honest about what runs today — every candidate is
//! refused as unavailable, precisely and legibly, rather than the crate
//! having no production probe to attach at all. The day a TLS-capable
//! transport is authorised, this caller starts assessing real candidates
//! without a line here changing.

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_data_finder::probe::{NetworkProbe, SourceProbe};
use qip_data_finder::source::SourceCandidate;
use qip_kernel::{Platform, SourceAssessment};

/// How the discovery pass is tuned.
///
/// `every_cycles` defaults to zero — no discovery — which `Default` states
/// structurally rather than a hand-written impl restating it.
#[derive(Clone, Debug, Default)]
pub struct DiscoveryConfig {
    /// Run a pass every this many research cycles. Zero disables the pass
    /// entirely — a deployment's honest way of saying "no discovery here"
    /// rather than an unset variable nobody can distinguish from a bug.
    pub every_cycles: u64,
}

impl DiscoveryConfig {
    /// Read the one operator-facing knob from the environment.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        let mut config = Self::default();
        if let Some(raw) = lookup("QIP_DEEPBRAIN_DISCOVER_EVERY") {
            config.every_cycles = raw.trim().parse().map_err(|_| {
                qip_core::error::Error::invalid(format!(
                    "QIP_DEEPBRAIN_DISCOVER_EVERY is {raw:?}, not a cycle count; 0 disables the \
                     discovery pass"
                ))
            })?;
        }
        Ok(config)
    }
}

/// Runs [`Platform::assess_sources`] on its own cadence, against a fixed
/// candidate list and a real (if today universally-refusing) probe.
pub struct DiscoveryDesk {
    config: DiscoveryConfig,
    candidates: Vec<SourceCandidate>,
    probe: Box<dyn SourceProbe>,
}

impl std::fmt::Debug for DiscoveryDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryDesk")
            .field("every_cycles", &self.config.every_cycles)
            .field("candidates", &self.candidates.len())
            .finish_non_exhaustive()
    }
}

impl DiscoveryDesk {
    pub fn new(config: DiscoveryConfig, candidates: Vec<SourceCandidate>) -> Self {
        Self::with_probe(
            config,
            candidates,
            Box::new(NetworkProbe::unconfigured().identified_as("qip-deepbrain-discovery")),
        )
    }

    /// As [`Self::new`], with an explicit probe — the seam a test replaces
    /// with a scripted one, since [`NetworkProbe`] refuses every call by
    /// construction and a test against it would prove only that refusal.
    pub fn with_probe(
        config: DiscoveryConfig,
        candidates: Vec<SourceCandidate>,
        probe: Box<dyn SourceProbe>,
    ) -> Self {
        Self {
            config,
            candidates,
            probe,
        }
    }

    pub const fn every_cycles(&self) -> u64 {
        self.config.every_cycles
    }

    pub fn candidates(&self) -> &[SourceCandidate] {
        &self.candidates
    }

    /// Run a pass if this cycle is on the cadence, cloning the candidate
    /// list into it every time.
    ///
    /// Cloned rather than drained: a candidate this pass could not register
    /// (a robots refusal, an unreachable host) is not spent — the same host
    /// is worth asking again next pass, the same way a universe is not
    /// consumed by a cycle that sizes against it. `assess_one` re-derives
    /// every decision from the probe's fresh evidence each time, so a
    /// repeated candidate costs a repeated fetch and never a stale verdict.
    pub fn maybe_run(
        &mut self,
        platform: &mut Platform,
        cycle: u64,
        now: Timestamp,
    ) -> Result<Option<SourceAssessment>> {
        if self.config.every_cycles == 0 || cycle % self.config.every_cycles != 0 {
            return Ok(None);
        }
        let candidates = self.candidates.clone();
        let assessment = platform.assess_sources(candidates, self.probe.as_mut(), now)?;
        Ok(Some(assessment))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic_in_result_fn)]

    use super::*;
    use qip_contracts::governance::Usage;
    use qip_core::error::Error;
    use qip_core::{Currency, Duration};
    use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
    use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
    use qip_data_finder::legal::{LicensingPosture, SourceLicense};
    use qip_data_finder::probe::{HeadResponse, PayloadSample, RobotsFetch};
    use qip_data_finder::quality::SourceCost;
    use qip_data_finder::source::SourceIdentity;
    use qip_events::Topic;
    use qip_financial::asset_class::AssetClass;
    use qip_financial::universe::Universe;
    use qip_kernel::config::PlatformConfig;
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn platform() -> Platform {
        let config = PlatformConfig::default();
        let (context, _clock) = qip_core::Context::deterministic(start(), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
        .expect("the platform assembles")
    }

    fn candidate(id: &str) -> Result<SourceCandidate> {
        let coverage = SourceCoverage::new(
            [AssetClass::Equity],
            [SourceRegion::Europe],
            ["EU0001".to_string()],
            UpdateFrequency::Minutely,
        )?
        .with_history_from(start().saturating_sub(Duration::from_days(3_650)));
        let licensing = LicensingPosture::declared(SourceLicense::new(
            "qip-discovery-test-terms",
            [Usage::Derive],
        )?);
        SourceCandidate::new(
            SourceIdentity::new(id, format!("{id} feed"), "Example Data Ltd")?,
            SourceEndpoint::parse(
                &format!("https://{id}.example/quotes"),
                AccessMechanism::Rest {
                    auth: AuthRequirement::None,
                    incremental_parameter: None,
                    page_size: 100,
                },
            )?,
            coverage,
            licensing,
            SourceCost::free(Currency::USD),
            SourceRegion::Europe,
            [Topic::MarketQuote],
            "test",
            start(),
        )
    }

    /// A probe that answers every call the same refusing way
    /// [`NetworkProbe`] does, so a test can assert the desk reached the
    /// probe without needing a real network.
    #[derive(Debug, Default)]
    struct RefusingProbe {
        asked: usize,
    }

    impl SourceProbe for RefusingProbe {
        fn robots(&mut self, host: &str, _at: Timestamp) -> Result<RobotsFetch> {
            self.asked += 1;
            Err(Error::unavailable(format!("no transport to reach {host}")))
        }

        fn head(&mut self, endpoint: &SourceEndpoint, _at: Timestamp) -> Result<HeadResponse> {
            self.asked += 1;
            Err(Error::unavailable(format!(
                "no transport to reach {}",
                endpoint.url()
            )))
        }

        fn sample(&mut self, endpoint: &SourceEndpoint, _at: Timestamp) -> Result<PayloadSample> {
            self.asked += 1;
            Err(Error::unavailable(format!(
                "no transport to reach {}",
                endpoint.url()
            )))
        }
    }

    #[test]
    fn a_desk_configured_off_never_reaches_the_probe() -> Result<()> {
        let mut platform = platform();
        let mut desk = DiscoveryDesk::with_probe(
            DiscoveryConfig { every_cycles: 0 },
            vec![candidate("a")?],
            Box::new(RefusingProbe::default()),
        );
        assert!(desk.maybe_run(&mut platform, 1, start())?.is_none());
        assert!(desk.maybe_run(&mut platform, 4, start())?.is_none());
        Ok(())
    }

    #[test]
    fn a_desk_on_cadence_reaches_the_platforms_assess_sources() -> Result<()> {
        let mut platform = platform();
        let mut desk = DiscoveryDesk::with_probe(
            DiscoveryConfig { every_cycles: 2 },
            vec![candidate("a")?, candidate("b")?],
            Box::new(RefusingProbe::default()),
        );
        // Off cadence: no assessment, and nothing reached the probe.
        assert!(desk.maybe_run(&mut platform, 1, start())?.is_none());

        // On cadence: an assessment for every candidate, even though the
        // probe refused every one of them -- an assessment is a decision
        // *about* a candidate, and "could not be reached" is one.
        let assessment = desk
            .maybe_run(&mut platform, 2, start())?
            .ok_or_else(|| Error::not_found("a pass on its own cadence produced nothing"))?;
        assert_eq!(
            assessment.decisions.len(),
            2,
            "two candidates went in; {} decision(s) came out",
            assessment.decisions.len()
        );
        Ok(())
    }
}
