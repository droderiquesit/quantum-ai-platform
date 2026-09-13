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
//! [`qip_data_finder::probe::NetworkProbe`] reaches one source through one
//! reviewed egress route (ADR 0060). The egress proxy is a reverse proxy: a
//! process picks a destination by picking a loopback port and cannot name a
//! host, because the request carries no field in which a host could be
//! named. So the candidate catalogue carries a route beside every candidate
//! ([`qip_data_finder::catalogue::CandidateEntry`]), refuses an entry
//! without one at load, and this desk builds **one probe per entry** at pass
//! time rather than one probe for the list — a probe is bound to a single
//! route and a batch would have to pick one of them.
//!
//! Until the merge that joined this desk to that probe, the desk attached a
//! `NetworkProbe` that refused every call by name, on the belief that the
//! missing piece was a TLS-capable transport (ADR 0009). It was not: the
//! proxy originates TLS upstream, and what was missing was the route. The
//! earlier claim is recorded here so it is not read back into the code.

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_data_finder::catalogue::CandidateEntry;
use qip_data_finder::probe::{NetworkProbe, SourceProbe};
use qip_kernel::{Platform, SourceAssessment};

/// How this platform identifies itself to a publisher it probes.
///
/// A publisher's only means of asking this platform to stop is to block a user
/// agent, so the name is stated here, in the composition root, rather than
/// defaulted inside the probe — a default would be a name nobody chose
/// appearing in somebody else's access log.
pub const DISCOVERY_USER_AGENT: &str = "qip-deepbrain-source-probe/1.0";

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
/// candidate catalogue, each entry probed through its own reviewed route.
pub struct DiscoveryDesk {
    config: DiscoveryConfig,
    entries: Vec<CandidateEntry>,
    /// A scripted probe answering for every entry, or `None` for the
    /// production shape: one [`NetworkProbe`] per entry, through its route.
    probe: Option<Box<dyn SourceProbe>>,
}

impl std::fmt::Debug for DiscoveryDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryDesk")
            .field("every_cycles", &self.config.every_cycles)
            .field("candidates", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl DiscoveryDesk {
    /// The production desk: every entry is probed through the route it names.
    pub fn new(config: DiscoveryConfig, entries: Vec<CandidateEntry>) -> Self {
        Self {
            config,
            entries,
            probe: None,
        }
    }

    /// As [`Self::new`], with one explicit probe answering for every entry —
    /// the seam a test replaces with a scripted one, so a pass can be proven
    /// to reach the platform without a socket.
    pub fn with_probe(
        config: DiscoveryConfig,
        entries: Vec<CandidateEntry>,
        probe: Box<dyn SourceProbe>,
    ) -> Self {
        Self {
            config,
            entries,
            probe: Some(probe),
        }
    }

    pub const fn every_cycles(&self) -> u64 {
        self.config.every_cycles
    }

    pub fn entries(&self) -> &[CandidateEntry] {
        &self.entries
    }

    /// Run a pass if this cycle is on the cadence, cloning each candidate
    /// into it every time.
    ///
    /// Cloned rather than drained: a candidate this pass could not register
    /// (a robots refusal, an unreachable host) is not spent — the same host
    /// is worth asking again next pass, the same way a universe is not
    /// consumed by a cycle that sizes against it. `assess_one` re-derives
    /// every decision from the probe's fresh evidence each time, so a
    /// repeated candidate costs a repeated fetch and never a stale verdict.
    ///
    /// One `assess_sources` call per entry rather than one for the list,
    /// because a probe is bound to a single route (ADR 0060). The per-entry
    /// assessments are merged into one so the caller reads one pass, and the
    /// decisions are restored to the identifier order
    /// [`SourceAssessment::decisions`] documents.
    pub fn maybe_run(
        &mut self,
        platform: &mut Platform,
        cycle: u64,
        now: Timestamp,
    ) -> Result<Option<SourceAssessment>> {
        if self.config.every_cycles == 0 || !cycle.is_multiple_of(self.config.every_cycles) {
            return Ok(None);
        }
        let mut merged = SourceAssessment {
            decisions: Vec::new(),
            catalogued: Vec::new(),
            catalogue_problems: Vec::new(),
        };
        for entry in &self.entries {
            let candidates = vec![entry.candidate.clone()];
            let assessment = match self.probe.as_mut() {
                Some(probe) => platform.assess_sources(candidates, probe.as_mut(), now)?,
                None => {
                    // The catalogue refused a malformed route at load, so a
                    // refusal here is a route that changed shape between
                    // load and pass. That is a bug, and it stops the pass by
                    // name rather than filing the source as unreachable for
                    // a reason that would read as the publisher's fault.
                    let mut probe =
                        NetworkProbe::through(&entry.egress_route, DISCOVERY_USER_AGENT)?;
                    platform.assess_sources(candidates, &mut probe, now)?
                }
            };
            merged.decisions.extend(assessment.decisions);
            merged.catalogued.extend(assessment.catalogued);
            merged
                .catalogue_problems
                .extend(assessment.catalogue_problems);
        }
        merged
            .decisions
            .sort_by(|left, right| left.source_id().cmp(right.source_id()));
        Ok(Some(merged))
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
    use qip_data_finder::source::{SourceCandidate, SourceIdentity};
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

    fn candidate(id: &str) -> Result<CandidateEntry> {
        entry(id, "http://127.0.0.1:9")
    }

    fn entry(id: &str, route: &str) -> Result<CandidateEntry> {
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
        let candidate = SourceCandidate::new(
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
        )?;
        Ok(CandidateEntry {
            candidate,
            egress_route: route.to_string(),
        })
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

    #[test]
    fn a_production_desk_probes_each_entry_through_the_route_it_names() -> Result<()> {
        // The seam `with_probe` bypasses. A desk built by `new` has to build a
        // `NetworkProbe` from the entry's own route and reach the platform
        // through it, and a desk that quietly skipped the entries whose
        // route it could not use would report a pass that assessed nothing
        // as a pass that found nothing. The route is a loopback port this
        // test just released, so the probe's connection is refused at once —
        // a decision about the candidate ("unreachable"), not an error.
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0")
                .map_err(|error| Error::io(format!("no loopback port: {error}")))?;
            listener
                .local_addr()
                .map_err(|error| Error::io(format!("no local address: {error}")))?
                .port()
        };
        let mut platform = platform();
        let mut desk = DiscoveryDesk::new(
            DiscoveryConfig { every_cycles: 1 },
            vec![entry("a", &format!("http://127.0.0.1:{port}"))?],
        );
        let assessment = desk
            .maybe_run(&mut platform, 1, start())?
            .ok_or_else(|| Error::not_found("a pass on its own cadence produced nothing"))?;
        assert_eq!(
            assessment.decisions.len(),
            1,
            "one entry went in; {} decision(s) came out",
            assessment.decisions.len()
        );
        assert!(
            !assessment.decisions[0].is_registered(),
            "a source nothing answered for was registered"
        );
        Ok(())
    }
}
