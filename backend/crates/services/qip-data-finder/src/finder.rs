//! The orchestrator: discover → classify → probe → assess → score → route →
//! register → monitor → detect drift → find replacement.
//!
//! Given the same candidates, the same probe, the same seed and the same
//! timestamps, [`DataFinder`] produces byte-identical decisions. That is not a
//! nicety: a registration decision is a legal position, and a legal position
//! that cannot be reproduced cannot be defended. Candidates are sorted by
//! identifier before anything happens, every collection is ordered, and the
//! only randomness is a tie-break derived from the seed and the source's own
//! identifier — never from iteration order, which would make the answer depend
//! on how the caller happened to build the list.

use crate::coverage::UpdateFrequency;
use crate::decision::{
    DecisionOutcome, LifecycleStage, Reasoning, RegisteredSource, RegistrationDecision,
};
use crate::endpoint::AccessMechanism;
use crate::health::{HealthObservation, SourceHealth};
use crate::legal::{HostRules, LegalAssessment, Legality, RateLimit, SourcePolicy};
use crate::probe::{ProbeEvidence, SourceProbe};
use crate::replacement::{self, ReplacementOutcome};
use crate::robots::PathVerdict;
use crate::schema::{SchemaDrift, SourceSchema};
use crate::scoring::{Routing, SourceScores};
use crate::source::{Source, SourceCandidate, SourceLineage};
use crate::tier::{
    AccessMode, BulkCadence, CredentialReference, DeepWebAdapter, DiscoveryEnclave, Interface,
    RenderingBudget, SourceTier, TierEvidence,
};
use qip_contracts::governance::Usage;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Hasher256, Rng, Timestamp, Xoshiro256};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How the finder is configured for a deployment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinderConfig {
    user_agent: String,
    host_rules: HostRules,
    required_usage: Usage,
    default_rate_limit: Option<RateLimit>,
    seed: u64,
    owner: String,
    /// The credential the deployment projects as a file for each host that
    /// needs one, by name. Absent for a host means the `registered` and
    /// `licensed` modes are refused for it, not that one is invented.
    #[serde(default)]
    credential_references: BTreeMap<String, CredentialReference>,
    /// Where rendered and bulk fetches run. Absent means those modes are
    /// refused, because there is nowhere for them to run safely.
    #[serde(default)]
    enclave: Option<DiscoveryEnclave>,
}

impl FinderConfig {
    /// Configure the finder.
    ///
    /// The user agent is mandatory and cannot be blank. A publisher's only
    /// means of refusing a crawler is to name it in robots.txt, and a crawler
    /// that will not say who it is has taken that away from them.
    pub fn new(
        user_agent: impl Into<String>,
        required_usage: Usage,
        owner: impl Into<String>,
        seed: u64,
    ) -> Result<Self> {
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() {
            return Err(Error::invalid(
                "the finder must present a user agent; an unnamed crawler is one a publisher \
                 cannot refuse",
            ));
        }
        let owner = owner.into();
        if owner.trim().is_empty() {
            return Err(Error::invalid(
                "registered sources need an accountable owner, as the mesh catalogue requires",
            ));
        }
        Ok(Self {
            user_agent,
            host_rules: HostRules::open(),
            required_usage,
            default_rate_limit: None,
            seed,
            owner,
            credential_references: BTreeMap::new(),
            enclave: None,
        })
    }

    pub fn with_host_rules(mut self, rules: HostRules) -> Self {
        self.host_rules = rules;
        self
    }

    /// Name the credential the deployment projects as a file for `host`.
    ///
    /// The name, never the value: a [`CredentialReference`] cannot be built
    /// from anything that looks like one.
    pub fn with_credential_reference(
        mut self,
        host: impl Into<String>,
        reference: CredentialReference,
    ) -> Self {
        self.credential_references
            .insert(host.into().to_ascii_lowercase(), reference);
        self
    }

    /// Name the enclave rendered and bulk fetches run in.
    pub fn with_discovery_enclave(mut self, enclave: DiscoveryEnclave) -> Self {
        self.enclave = Some(enclave);
        self
    }

    pub fn credential_reference_for(&self, host: &str) -> Option<&CredentialReference> {
        self.credential_references.get(&host.to_ascii_lowercase())
    }

    pub fn discovery_enclave(&self) -> Option<&DiscoveryEnclave> {
        self.enclave.as_ref()
    }

    /// The request budget applied where a source states none of its own.
    pub fn with_default_rate_limit(mut self, limit: RateLimit) -> Self {
        self.default_rate_limit = Some(limit);
        self
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    pub fn host_rules(&self) -> &HostRules {
        &self.host_rules
    }

    /// What the platform intends to do with these sources. A source is
    /// assessed against this usage and no other, so a finder configured for
    /// research cannot register anything as tradeable.
    pub fn required_usage(&self) -> Usage {
        self.required_usage
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }
}

/// What monitoring one registered source concluded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "monitor", rename_all = "snake_case")]
pub enum MonitorOutcome {
    Healthy {
        health: Box<SourceHealth>,
    },
    Degraded {
        health: Box<SourceHealth>,
        reason: String,
    },
    /// Stopped, because its shape moved. Not degraded: a source whose fields
    /// changed meaning produces numbers that are wrong rather than late.
    Quarantined {
        drift: Box<SchemaDrift>,
        reason: String,
    },
    /// Answered nothing across a window long enough to mean it.
    Dead {
        health: Box<SourceHealth>,
        reason: String,
    },
}

impl MonitorOutcome {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy { .. } => "healthy",
            Self::Degraded { .. } => "degraded",
            Self::Quarantined { .. } => "quarantined",
            Self::Dead { .. } => "dead",
        }
    }

    /// Whether this outcome means the source must be replaced.
    pub const fn needs_replacement(&self) -> bool {
        matches!(self, Self::Quarantined { .. } | Self::Dead { .. })
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::Healthy { .. } => "the source is answering within its promises",
            Self::Degraded { reason, .. }
            | Self::Quarantined { reason, .. }
            | Self::Dead { reason, .. } => reason,
        }
    }
}

/// Runs the source lifecycle.
#[derive(Debug)]
pub struct DataFinder {
    config: FinderConfig,
    registry: BTreeMap<String, RegisteredSource>,
}

impl DataFinder {
    /// History depth at which the historical-value score saturates.
    ///
    /// Ten years covers more than one full credit cycle, which is the longest
    /// structure most of the platform's models can identify. Deeper history
    /// is not worthless, but it stops changing a decision, and a score that
    /// keeps rising past the point of usefulness distorts every comparison.
    pub const HISTORY_SATURATION: Duration = Duration::from_days(3_650);

    /// Monthly spend at which cost efficiency reaches zero.
    pub const COST_CEILING_PER_MONTH: i64 = 10_000;

    /// Pages an hour a rendered source may cost, and the time one page may
    /// take. Sixty an hour is one a minute — a human reader's pace, which is
    /// the pace a publisher's rate limiter was written for.
    pub const RENDERING_PAGES_PER_HOUR: u32 = 60;
    pub const RENDERING_MAX_TIME: Duration = Duration::from_secs(30);

    /// How long a bulk extract may be kept after its facts are taken.
    ///
    /// A week covers a re-extraction after a schema fix without the extract
    /// becoming a second copy of the source.
    pub const BULK_EXTRACT_RETENTION: Duration = Duration::from_days(7);

    /// The request budget an open query interface is enumerated under when
    /// neither the source nor the configuration states one.
    pub const OPEN_QUERY_REQUESTS_PER_MINUTE: u32 = 30;

    pub fn new(config: FinderConfig) -> Self {
        Self {
            config,
            registry: BTreeMap::new(),
        }
    }

    pub fn config(&self) -> &FinderConfig {
        &self.config
    }

    /// Sources currently registered, by identifier.
    pub fn registry(&self) -> &BTreeMap<String, RegisteredSource> {
        &self.registry
    }

    pub fn registered(&self, id: &str) -> Option<&RegisteredSource> {
        self.registry.get(id)
    }

    /// Run the lifecycle over a candidate set.
    ///
    /// Candidates are processed in identifier order regardless of the order
    /// they arrive in, so two runs over the same set — however it was
    /// assembled — produce the same decisions in the same sequence.
    pub fn assess(
        &mut self,
        candidates: Vec<SourceCandidate>,
        probe: &mut dyn SourceProbe,
        now: Timestamp,
    ) -> Result<Vec<RegistrationDecision>> {
        let mut ordered = candidates;
        ordered.sort_by(|left, right| left.id().cmp(right.id()));

        let mut decisions = Vec::with_capacity(ordered.len());
        for candidate in ordered {
            decisions.push(self.assess_one(candidate, probe, now)?);
        }
        Ok(decisions)
    }

    fn assess_one(
        &mut self,
        candidate: SourceCandidate,
        probe: &mut dyn SourceProbe,
        now: Timestamp,
    ) -> Result<RegistrationDecision> {
        let mut reasoning = Reasoning::new();
        let id = candidate.id().to_string();
        reasoning.record(
            LifecycleStage::Discover,
            format!(
                "found via {} at {}",
                candidate.discovered_from(),
                candidate.discovered_at()
            ),
        );

        let plan = candidate.endpoint().mechanism().poll_plan();
        reasoning.record(
            LifecycleStage::Classify,
            format!(
                "{} over {}, {:?} delivery, natural interval {:?}{}",
                candidate.endpoint().mechanism().kind(),
                candidate.endpoint().scheme().as_str(),
                plan.delivery,
                plan.natural_interval,
                match &plan.credential_required {
                    Some(credential) => format!(", requires {credential}"),
                    None => String::new(),
                }
            ),
        );

        // The tier is asked about before any request is made, on what the
        // candidate's own description supports. It usually cannot be settled
        // yet — surface web is a finding about reachability the probe has to
        // make — and that is recorded as not yet classifiable rather than
        // guessed. The one tier decidable here is the one that must be: a
        // hidden-service host is refused without ever being contacted,
        // because contacting it is the thing the dark-web rule forbids.
        match SourceTier::classify(&TierEvidence::from_candidate(&candidate)) {
            Ok(SourceTier::DarkWeb) => {
                let adapter = DeepWebAdapter::new(
                    &id,
                    candidate.endpoint().host(),
                    SourceTier::DarkWeb,
                    AccessMode::Api,
                )?;
                let refusal = match adapter.admissible(candidate.declared_licensing(), None) {
                    Ok(()) => {
                        return Err(Error::denied(format!(
                            "`{id}` is on the dark web and an access mode was found admissible \
                             for it; that cannot be, and the finder stops rather than route it"
                        )));
                    }
                    Err(error) => error.message().to_string(),
                };
                reasoning.record(
                    LifecycleStage::Classify,
                    format!("tier {}: {refusal}", SourceTier::DarkWeb.as_str()),
                );
                reasoning.record(
                    LifecycleStage::Probe,
                    "not probed: a hidden service is never contacted for signal",
                );
                reasoning.record(
                    LifecycleStage::Route,
                    format!(
                        "rejected on tier {}: monitoring-only, no access mode admissible",
                        SourceTier::DarkWeb.as_str()
                    ),
                );
                return RegistrationDecision::new(
                    id,
                    DecisionOutcome::Rejected { reason: refusal },
                    reasoning,
                    now,
                );
            }
            Ok(tier) => reasoning.record(
                LifecycleStage::Classify,
                format!(
                    "tier {} from the candidate's own description",
                    tier.as_str()
                ),
            ),
            Err(error) => reasoning.record(
                LifecycleStage::Classify,
                format!("tier not yet classifiable before the probe: {error}"),
            ),
        }

        // The host check runs before the probe, and a refusal stops here.
        // Deferring it until after the fetch would mean a denylisted host had
        // already been contacted, which is the one outcome a denylist exists
        // to prevent.
        let host_verdict = self.config.host_rules.verdict(candidate.endpoint().host());
        if host_verdict.is_forbidden() {
            reasoning.record(LifecycleStage::AssessLegality, host_verdict.describe());
            reasoning.record(
                LifecycleStage::Probe,
                "not probed: the host was refused before any request was made",
            );
            let legality = LegalAssessment::combine(
                host_verdict.clone(),
                Legality::unknown("robots.txt was not fetched from a refused host"),
                Legality::unknown("licensing was not assessed for a refused host"),
                self.config.required_usage,
            );
            reasoning.record(
                LifecycleStage::Route,
                Routing::decide(
                    legality.overall(),
                    &SourceScores::new(0.0, 0.0, 0.0, 0.0, 0.0)?,
                )
                .basis()
                .to_string(),
            );
            return RegistrationDecision::new(
                id,
                DecisionOutcome::Rejected {
                    reason: host_verdict.reason().to_string(),
                },
                reasoning,
                now,
            )
            .map(|decision| decision.with_legality(legality));
        }
        reasoning.record(LifecycleStage::AssessLegality, host_verdict.describe());

        let evidence = match ProbeEvidence::gather(probe, candidate.endpoint(), now) {
            Ok(evidence) => evidence,
            Err(error) => {
                reasoning.record(
                    LifecycleStage::Probe,
                    format!("the probe could not reach this source: {error}"),
                );
                return RegistrationDecision::new(
                    id,
                    DecisionOutcome::Deferred {
                        reason: error.to_string(),
                    },
                    reasoning,
                    now,
                );
            }
        };
        reasoning.record(
            LifecycleStage::Probe,
            format!(
                "{}; HEAD {} in {:?}; sampled {} byte(s) with fingerprint {}",
                evidence.robots().describe(),
                evidence.head().status,
                evidence.head().latency,
                evidence.sample().body.len(),
                &evidence.schema().fingerprint()[..16.min(evidence.schema().fingerprint().len())]
            ),
        );

        let source = Source::from_evidence(candidate, evidence);

        // With the probe's answers in hand the tier can be settled, and if
        // it still cannot be, nothing is decided: a source nobody can place
        // is deferred, as a probe failure is, rather than rejected as if
        // something about it had been found wanting.
        let tier = match SourceTier::classify(&TierEvidence::from_source(&source)) {
            Ok(tier) => tier,
            Err(error) => {
                reasoning.record(
                    LifecycleStage::Classify,
                    format!("tier not classifiable after the probe: {error}"),
                );
                return RegistrationDecision::new(
                    id,
                    DecisionOutcome::Deferred {
                        reason: error.to_string(),
                    },
                    reasoning,
                    now,
                );
            }
        };
        reasoning.record(
            LifecycleStage::Classify,
            format!("tier {} on the probe's evidence", tier.as_str()),
        );
        let source = source.with_tier(tier);

        let robots_verdict = self.robots_legality(&source);
        let licensing_verdict = source
            .licensing()
            .legality_for(self.config.required_usage, now);
        reasoning.record(LifecycleStage::AssessLegality, robots_verdict.describe());
        reasoning.record(LifecycleStage::AssessLegality, licensing_verdict.describe());

        let legality = LegalAssessment::combine(
            host_verdict,
            robots_verdict,
            licensing_verdict,
            self.config.required_usage,
        );

        let scores = self.score(&source, now)?;
        for (name, value) in scores.named() {
            reasoning.record(LifecycleStage::Score, format!("{name} {value:.3}"));
        }

        let routing = Routing::decide(legality.overall(), &scores);
        reasoning.record(LifecycleStage::Route, routing.basis().to_string());

        if !routing.class().is_collected() {
            return RegistrationDecision::new(
                id,
                DecisionOutcome::Rejected {
                    reason: routing.basis().to_string(),
                },
                reasoning,
                now,
            )
            .map(|decision| decision.with_legality(legality).with_scores(scores));
        }

        // Routing by tier, after routing by legality and score: a source the
        // score would collect is still refused when the way it has to be
        // reached is not one this deployment can reach it by. The refusal
        // carries the tier and the mode so a reviewer can see which rule
        // fired without re-deriving it.
        match self.route_by_tier(&source, tier) {
            Ok(finding) => reasoning.record(LifecycleStage::Route, finding),
            Err(error) => {
                let reason = error.message().to_string();
                reasoning.record(
                    LifecycleStage::Route,
                    format!("rejected on tier {}: {reason}", tier.as_str()),
                );
                return RegistrationDecision::new(
                    id,
                    DecisionOutcome::Rejected { reason },
                    reasoning,
                    now,
                )
                .map(|decision| decision.with_legality(legality).with_scores(scores));
            }
        }

        let policy = self.policy_for(&source);
        reasoning.record(
            LifecycleStage::Register,
            format!(
                "polling `{}` no more often than every {:?} as `{}`{}",
                policy.host(),
                policy.min_request_interval(),
                policy.user_agent(),
                match policy.crawl_delay() {
                    Some(delay) => format!(", honouring a crawl-delay of {delay:?}"),
                    None => String::new(),
                }
            ),
        );

        let dataset = format!("source.{id}");
        let entitlements = source
            .licensing()
            .license()
            .map(|license| license.entitlements(&dataset, now))
            .unwrap_or_default();
        let lineage = SourceLineage::new(&source, now);

        // The legality assessment is handed to the constructor rather than
        // attached afterwards: `registered` is the only way to a `Registered`
        // outcome, and it refuses an assessment that did not permit
        // collection. This finder's own registry is populated *after* that
        // constructor succeeds, and from the pieces it actually accepted
        // (`registration.routing()`, `.policy()`, `.entitlements()`) rather
        // than from the locals computed before the call. A source that
        // reaches `self.registry` is a source `RegistrationDecision::registered`
        // has already agreed to register — never one this function merely
        // hoped would pass, which is what inserting first and deciding after
        // would leave standing on an error path. `self.registry` backs
        // `DataFinder::registered`, `DataFinder::registry`, and the
        // uniqueness term in `score`, all of which the rest of the platform
        // (via `Platform::data_finder`) reads as "currently collected" — so a
        // premature insert here is not a bookkeeping nicety, it is a second,
        // ungated path onto the same effective catalogue this crate exists
        // to gate.
        let decision = RegistrationDecision::registered(
            id.clone(),
            legality,
            routing,
            policy,
            entitlements,
            reasoning,
            now,
        )?
        .with_scores(scores)
        .with_lineage(lineage.clone());

        if let Some(registration) = decision.registration() {
            self.registry.insert(
                id,
                RegisteredSource::new(
                    source,
                    registration.routing().clone(),
                    registration.policy().clone(),
                    lineage,
                    registration.entitlements().to_vec(),
                    now,
                ),
            );
        }

        Ok(decision)
    }

    /// What robots.txt says about this endpoint.
    ///
    /// Three outcomes, deliberately: a served file whose rules permit or
    /// forbid, and anything else — absent, unreachable, unparsed — as
    /// undetermined. The last of those is where the property lives. A 404 on
    /// robots.txt is the most common way a source has no stated position, and
    /// reading "no prohibition" as "permission" is exactly the inference this
    /// crate refuses to make.
    fn robots_legality(&self, source: &Source) -> Legality {
        if !source.endpoint().mechanism().is_governed_by_robots() {
            return Legality::permitted(format!(
                "robots.txt does not govern a {} source; its terms are settled by its licence",
                source.endpoint().mechanism().kind()
            ));
        }
        let Some(policy) = source.evidence().robots_policy() else {
            return Legality::unknown(format!(
                "{} for `{}`; the absence of a robots.txt is not a permission to crawl it",
                source.evidence().robots().describe(),
                source.endpoint().host()
            ));
        };
        let verdict = policy.verdict(&self.config.user_agent, source.endpoint().path());
        match verdict {
            PathVerdict::DisallowedByRule { .. } => Legality::forbidden(format!(
                "robots.txt for `{}` {} for `{}`",
                source.endpoint().host(),
                verdict.describe(),
                source.endpoint().path()
            )),
            PathVerdict::AllowedByRule { .. } | PathVerdict::AllowedByAbsenceOfRule => {
                Legality::permitted(format!(
                    "robots.txt for `{}` {} for `{}`",
                    source.endpoint().host(),
                    verdict.describe(),
                    source.endpoint().path()
                ))
            }
            PathVerdict::NoApplicableGroup => Legality::permitted(format!(
                "robots.txt for `{}` was served and {}",
                source.endpoint().host(),
                verdict.describe()
            )),
        }
    }

    /// Decide whether the source can be reached the way its tier requires,
    /// returning the finding for the record or the refusal.
    ///
    /// A surface-web API is the case every source before this tier existed
    /// fell into, and it is left exactly as it was: no adapter, no enclave,
    /// the finding recorded and nothing else changed. Every other combination
    /// builds a [`DeepWebAdapter`] and asks it whether the deployment can run
    /// it.
    fn route_by_tier(&self, source: &Source, tier: SourceTier) -> Result<String> {
        let mode = self.access_mode_for(source, tier)?;
        if tier == SourceTier::SurfaceWeb && mode == AccessMode::Api {
            return Ok(format!(
                "tier {} via access mode {}: reached directly, no deep-web adapter needed",
                tier.as_str(),
                mode.as_str()
            ));
        }
        let adapter = DeepWebAdapter::new(source.id(), source.endpoint().host(), tier, mode)?;
        adapter.admissible(source.licensing(), self.config.enclave.as_ref())?;
        Ok(format!(
            "tier {} via access mode {}{}{}",
            tier.as_str(),
            adapter.mode().as_str(),
            match adapter.mode().credential() {
                Some(credential) => format!(
                    ", credential `{}` projected as a file by the deployment",
                    credential.name()
                ),
                None => String::new(),
            },
            match (adapter.mode().needs_enclave(), &self.config.enclave) {
                (true, Some(enclave)) => format!(", inside {}", enclave.describe()),
                _ => String::new(),
            }
        ))
    }

    /// The access mode a source has to be reached by, from its mechanism,
    /// its cost and what the deployment names for its host.
    ///
    /// A mode that needs something the deployment has not provided is not
    /// built with a placeholder; the refusal names what to provide.
    fn access_mode_for(&self, source: &Source, tier: SourceTier) -> Result<AccessMode> {
        let endpoint = source.endpoint();
        let mechanism = endpoint.mechanism();
        if let AccessMechanism::BulkFile {
            published_every, ..
        } = mechanism
        {
            return Ok(AccessMode::Bulk {
                cadence: BulkCadence::new(*published_every, Self::BULK_EXTRACT_RETENTION)?,
            });
        }
        if Interface::of(mechanism) == Interface::ExtractedPage {
            // A query string in the endpoint's own path is a parameterised
            // interface; a bare path is a page whose content the extractor
            // has to render.
            if endpoint.path().contains('?') {
                let rate_limit = match self.config.default_rate_limit {
                    Some(limit) => limit,
                    None => RateLimit::new(
                        Self::OPEN_QUERY_REQUESTS_PER_MINUTE,
                        Duration::from_mins(1),
                    )?,
                };
                return Ok(AccessMode::OpenQuery { rate_limit });
            }
            return Ok(AccessMode::Rendered {
                budget: RenderingBudget::new(
                    Self::RENDERING_PAGES_PER_HOUR,
                    Self::RENDERING_MAX_TIME,
                )?,
            });
        }
        if let Some(need) = mechanism.poll_plan().credential_required {
            let credential = self
                .config
                .credential_reference_for(endpoint.host())
                .cloned()
                .ok_or_else(|| {
                    Error::denied(format!(
                        "`{}` needs {need}, and no credential reference is named for `{}`; \
                         name the secret the deployment projects as a file with \
                         `FinderConfig::with_credential_reference`. A credential is never \
                         invented and never carried by value",
                        source.id(),
                        endpoint.host()
                    ))
                })?;
            let paid =
                !source.cost().monthly_fee().is_zero() || !source.cost().per_request().is_zero();
            if !paid {
                return Ok(AccessMode::Registered { credential });
            }
            let licence = source
                .licensing()
                .license()
                .map(|license| license.identifier().to_string())
                .ok_or_else(|| {
                    Error::denied(format!(
                        "`{}` is a paid subscription and its licensing posture declares no \
                         licence; a licensed source is reached only under terms that have \
                         been read",
                        source.id()
                    ))
                })?;
            return Ok(AccessMode::Licensed {
                credential,
                licence,
            });
        }
        // Machine-readable and unauthenticated by its own description. On
        // the surface web that is an API; on the deep web it means the probe
        // was turned away, so the source wants a credential the candidate
        // did not declare, and the finder will not guess which.
        match tier {
            SourceTier::SurfaceWeb => Ok(AccessMode::Api),
            SourceTier::DeepWeb => Err(Error::denied(format!(
                "`{}` declares no credential and the probe was refused without one (HTTP {}); \
                 the source is gated in fact. Declare its authentication requirement and name \
                 a credential reference rather than letting the finder guess",
                source.id(),
                source.evidence().head().status
            ))),
            SourceTier::DarkWeb => Err(Error::denied(format!(
                "`{}` is on the dark web, which admits no access mode",
                source.id()
            ))),
        }
    }

    /// The policy the source will be polled under.
    fn policy_for(&self, source: &Source) -> SourcePolicy {
        let crawl_delay = source
            .evidence()
            .robots_policy()
            .and_then(|policy| policy.crawl_delay_for(&self.config.user_agent));
        let disallowed = source
            .evidence()
            .robots_policy()
            .map(|policy| policy.disallowed_paths_for(&self.config.user_agent))
            .unwrap_or_default();
        let attribution = source
            .licensing()
            .license()
            .is_some_and(|license| license.attribution_required());
        SourcePolicy::assemble(
            source.endpoint().host(),
            &self.config.user_agent,
            crawl_delay,
            self.config.default_rate_limit,
            disallowed,
            true,
        )
        .requiring_attribution(attribution)
    }

    /// Score a probed source.
    ///
    /// Every input is either observed evidence or already-registered state.
    /// The tie-break draw is derived from the seed and the source identifier
    /// rather than drawn in sequence, so a candidate's score does not depend
    /// on how many candidates preceded it.
    fn score(&self, source: &Source, now: Timestamp) -> Result<SourceScores> {
        let head = source.evidence().head();
        let mut reliability: f64 = if head.is_success() { 0.9 } else { 0.1 };
        if !source.endpoint().scheme().is_encrypted() {
            // A feed the network can rewrite is a feed that can be made to
            // say anything, and the platform would not know.
            reliability -= 0.3;
        }
        if source.schema().is_empty() {
            reliability -= 0.2;
        }
        let reliability = reliability.clamp(0.0, 1.0);

        let freshness = match source.evidence().sample().payload_at {
            Some(payload_at) => {
                let age = now.since(payload_at).as_secs_f64().max(0.0);
                match source
                    .coverage()
                    .update_frequency()
                    .typical_interval()
                    .map(|interval| interval.as_secs_f64())
                {
                    Some(expected) if expected > 0.0 => {
                        (1.0 - (age / (expected * 2.0))).clamp(0.0, 1.0)
                    }
                    // An irregular publisher cannot be late, so freshness
                    // carries no information about it either way.
                    _ => 0.5,
                }
            }
            None => 0.0,
        };

        let uniqueness = 1.0
            - self
                .registry
                .values()
                .map(|registered| {
                    registered
                        .source()
                        .coverage()
                        .overlap_with(source.coverage())
                })
                .fold(0.0f64, f64::max);
        let uniqueness = uniqueness.clamp(0.0, 1.0);

        let depth = source.coverage().history_depth(now).as_nanos() as f64;
        let historical_value = (depth / Self::HISTORY_SATURATION.as_nanos() as f64).clamp(0.0, 1.0);

        let monthly = source
            .cost()
            .monthly_cost_at(u64::from(estimated_monthly_requests(
                source.coverage().update_frequency(),
            )))?;
        let ceiling = Decimal::from_int(Self::COST_CEILING_PER_MONTH);
        let cost_efficiency = match monthly.checked_div(ceiling) {
            Some(ratio) => (1.0 - ratio.to_f64()).clamp(0.0, 1.0),
            None => 0.0,
        };

        // A deterministic hair's-worth of separation, so two sources that are
        // otherwise identical still order stably and reproducibly rather than
        // by whichever the map yielded first.
        let mut hasher = Hasher256::new();
        hasher.update(&self.config.seed.to_le_bytes());
        hasher.update(source.id().as_bytes());
        let digest = hasher.finish();
        let mut seed_bytes = [0u8; 8];
        seed_bytes.copy_from_slice(&digest[..8]);
        let jitter = Xoshiro256::seeded(u64::from_le_bytes(seed_bytes)).next_f64() * 1e-6;

        SourceScores::new(
            (reliability + jitter).clamp(0.0, 1.0),
            freshness,
            uniqueness,
            historical_value,
            cost_efficiency,
        )
    }

    /// Check a registered source against fresh observations and a fresh
    /// sample of its shape.
    ///
    /// Drift is checked before health. A source that is answering perfectly
    /// with a changed schema is the dangerous case, and checking availability
    /// first would report it healthy.
    pub fn monitor(
        &mut self,
        id: &str,
        observations: &[HealthObservation],
        observed_schema: Option<&SourceSchema>,
        now: Timestamp,
        window: Duration,
    ) -> Result<MonitorOutcome> {
        let registered = self.registry.get(id).ok_or_else(|| {
            Error::not_found(format!("no source `{id}` is registered with the finder"))
        })?;

        if let Some(observed) = observed_schema {
            let drift = registered.source().schema().drift_to(observed);
            if drift.severity().requires_quarantine() {
                let reason = format!(
                    "the schema of `{id}` drifted ({}): {}. A source whose fields changed \
                     meaning is worse than one that stopped, because it keeps producing numbers",
                    drift.severity().as_str(),
                    drift.describe()
                );
                if let Some(entry) = self.registry.get_mut(id) {
                    entry.quarantine(reason.clone());
                }
                return Ok(MonitorOutcome::Quarantined {
                    drift: Box::new(drift),
                    reason,
                });
            }
        }

        let health = SourceHealth::over(observations, now, window)?;
        let expected_freshness = self
            .registry
            .get(id)
            .and_then(|entry| {
                entry
                    .source()
                    .coverage()
                    .update_frequency()
                    .typical_interval()
            })
            .unwrap_or(Duration::from_days(1));

        if health.is_dead() {
            let reason = format!(
                "`{id}` served nothing across {} attempt(s) in the window ending {now}",
                health.samples()
            );
            if let Some(entry) = self.registry.get_mut(id) {
                entry.quarantine(reason.clone());
            }
            return Ok(MonitorOutcome::Dead {
                health: Box::new(health),
                reason,
            });
        }
        if health.is_degraded(expected_freshness) {
            let reason = format!("`{id}` is below its promises: {}", health.describe());
            return Ok(MonitorOutcome::Degraded {
                health: Box::new(health),
                reason,
            });
        }
        Ok(MonitorOutcome::Healthy {
            health: Box::new(health),
        })
    }

    /// Find something to take over from a registered source.
    ///
    /// Candidates are scored the same way registration scores them, so a
    /// replacement is ranked on the same basis as the source it replaces.
    /// Reports honestly when nothing covers it.
    pub fn find_replacement(
        &self,
        lost_id: &str,
        pool: &[Source],
        now: Timestamp,
    ) -> Result<ReplacementOutcome> {
        let lost = self.registry.get(lost_id).ok_or_else(|| {
            Error::not_found(format!(
                "no source `{lost_id}` is registered, so there is nothing to replace"
            ))
        })?;

        let mut candidates = Vec::with_capacity(pool.len());
        for candidate in pool {
            if candidate.id() == lost_id {
                continue;
            }
            candidates.push((
                candidate.id().to_string(),
                candidate.coverage().clone(),
                self.score(candidate, now)?,
            ));
        }
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(replacement::search(lost.source().coverage(), &candidates))
    }
}

/// Requests a month at a publication cadence, for costing a metered source.
fn estimated_monthly_requests(frequency: UpdateFrequency) -> u32 {
    match frequency {
        UpdateFrequency::Streaming | UpdateFrequency::Secondly => 2_592_000,
        UpdateFrequency::Minutely => 43_200,
        UpdateFrequency::Hourly => 720,
        UpdateFrequency::Daily => 30,
        UpdateFrequency::Weekly => 4,
        UpdateFrequency::Monthly => 1,
        // An irregular source has to be asked on a schedule of our choosing;
        // hourly is what the polling default would do.
        UpdateFrequency::Irregular => 720,
    }
}
