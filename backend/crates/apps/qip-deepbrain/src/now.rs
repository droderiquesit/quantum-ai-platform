//! A first NOW estimate (WORLD-3075): one latent scalar per domain, updated
//! from several sources at once, with an uncertainty that widens when the
//! sources disagree.
//!
//! The failure this names. The platform collected facts per subsystem —
//! anomaly detectors over prices, fill residuals per venue, hit rates per
//! belief class — and nothing combined them into a single estimate of "how
//! things are now", so an operator asking whether the market is stressed got
//! nine detector outputs and no answer. Worse, a combination that averaged
//! them would report two sources saying 0.1 and 0.9 as a confident 0.5. Here
//! the dispersion of the sources is added to the posterior variance, so a
//! contradiction is reported as uncertainty rather than hidden inside a mean.
//!
//! This is deliberately small: a scalar per domain over three domains the
//! research loop already reads. The §9.6 NOW Brain proper — eight scopes,
//! competing estimators, fenced promotion — runs in the kernel's UNDERSTAND
//! stage (`qip_kernel::now_stage`) on every cycle this binary runs; this
//! file's second half is how its surprises reach the research loop's
//! attention router ([`surprise_signals`]).

use qip_contracts::BrainSpec;
use qip_contracts::ambient::{AmbientSignal, Detection, Pathway, SignalClass, Trigger};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_kernel::now_stage::{NowSurprise, Specialist};
use std::collections::BTreeMap;

/// What every NOW surprise signal's id starts with.
pub const NOW_SIGNAL_PREFIX: &str = "now-surprise/";

/// Specialists a NOW surprise woke, by specialist.
pub const NOW_WAKES: &str = "qip_now_specialist_wakes_total";

/// The pathway each specialist is woken through, and the signal class that
/// routes there. Forecast, adversarial and sensing specialists are
/// specialists, so a surprise wakes them through specialist activation; the
/// risk specialist is a risk review.
pub const fn route_of(specialist: Specialist) -> (SignalClass, Pathway) {
    match specialist {
        Specialist::Forecast | Specialist::Adversarial | Specialist::Sensing => {
            (SignalClass::Surprise, Pathway::SpecialistActivation)
        }
        Specialist::Risk => (SignalClass::Risk, Pathway::RiskReview),
    }
}

/// One ambient signal per specialist a NOW surprise names (AMBIENT-3079).
///
/// Severity is the surprise's size, a thousand basis points per standard
/// deviation, so the kernel's three-sigma bound lands at 3,000 — above the
/// research loop's 1,000 bp materiality, so no NOW surprise is silently below
/// the bar. A surprise of ten sigma or more reads as the scale's top: that is
/// the severity scale's meaning ("as severe as it gets"), not a corrected
/// input, and the exact size travels as the observed deviation. Evidence is the manifest the
/// surprised state rests on, so the activation can be replayed to its
/// readings. Nothing here acts: the signal wakes, the router decides.
pub fn surprise_signals(
    surprise: &NowSurprise,
    now: Timestamp,
    ttl: Duration,
) -> Result<Vec<(Specialist, AmbientSignal)>> {
    if !surprise.sigmas.is_finite() {
        return Err(Error::numeric(format!(
            "surprise {} states a non-finite size",
            surprise.id
        )));
    }
    // Statistic to integer: sigmas to basis points, a thousand per sigma, at
    // most the severity scale's top.
    let severity = (surprise.sigmas.abs() * 1_000.0).round().min(10_000.0) as u32;
    let deviation = Decimal::from_f64(surprise.sigmas)
        .ok_or_else(|| Error::numeric("the surprise has no decimal form"))?;
    let evidence: Vec<String> = surprise
        .manifests
        .iter()
        .map(|m| format!("{}#{}", m.source, m.reference))
        .collect();
    surprise
        .wakes
        .iter()
        .map(|specialist| {
            let (class, pathway) = route_of(*specialist);
            let signal = AmbientSignal::new(
                format!("{NOW_SIGNAL_PREFIX}{}/{}", surprise.id, specialist.as_str()),
                class,
                format!("{}:{}", specialist.as_str(), surprise.key),
                severity,
                Trigger::Event("now-surprise".to_string()),
                now,
                Detection {
                    observed_deviation: deviation,
                    expected_baseline: Decimal::ZERO,
                    horizon: Duration::ZERO,
                    novelty_bp: severity,
                    affected_entities: vec![surprise.key.scope.id.clone()],
                    urgency_bp: severity,
                    wake_targets: vec![pathway],
                    evidence_ids: evidence.clone(),
                    expiry: now.saturating_add(ttl),
                },
            )?;
            Ok((*specialist, signal))
        })
        .collect()
}

/// The domains this estimator holds, as the fixed `domain` label values.
pub const DOMAINS: [&str; 3] = ["market", "execution", "beliefs"];

/// One domain's estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub mean: f64,
    pub variance: f64,
    pub updates: u64,
}

impl Estimate {
    pub fn std_dev(&self) -> f64 {
        self.variance.sqrt()
    }
}

/// Per-domain estimates with a random-walk prior.
#[derive(Debug, Clone)]
pub struct NowEstimator {
    /// Variance added per update before the sources are read: the state is
    /// assumed to move between sweeps, so an old estimate loses confidence.
    drift: f64,
    /// Variance of a single source's reading about the latent state.
    source_noise: f64,
    estimates: BTreeMap<&'static str, Estimate>,
}

impl NowEstimator {
    pub fn new(drift: f64, source_noise: f64) -> Result<Self> {
        if !(drift.is_finite() && drift >= 0.0 && source_noise.is_finite() && source_noise > 0.0) {
            return Err(Error::invalid(format!(
                "drift must be finite and not negative and source noise finite and positive; got {drift} and {source_noise}"
            )));
        }
        Ok(Self {
            drift,
            source_noise,
            estimates: BTreeMap::new(),
        })
    }

    pub fn estimate(&self, domain: &str) -> Option<Estimate> {
        self.estimates.get(domain).copied()
    }

    /// Update `domain` from this sweep's readings, one per source.
    ///
    /// The readings' precision-weighted mean updates the prior as a Kalman
    /// step would; then the readings' own sample variance is added to the
    /// posterior variance, which is the contradiction term: sources that agree
    /// add nothing, sources that split add their spread. An empty sweep only
    /// drifts the prior. Refuses a domain outside [`DOMAINS`], so the label
    /// set stays fixed, and a non-finite reading.
    pub fn observe(&mut self, domain: &str, readings: &[f64]) -> Result<Estimate> {
        let Some(&domain) = DOMAINS.iter().find(|d| **d == domain) else {
            return Err(Error::invalid(format!(
                "`{domain}` is not a NOW domain; the domains are {DOMAINS:?}"
            )));
        };
        if let Some(bad) = readings.iter().find(|r| !r.is_finite()) {
            return Err(Error::numeric(format!(
                "a non-finite reading ({bad}) reached the NOW estimate for {domain}; drop it at the source"
            )));
        }
        let prior = self.estimates.get(domain).copied();
        let mut estimate = match (prior, readings.is_empty()) {
            (None, true) => {
                return Err(Error::invalid(format!(
                    "no readings and no prior for {domain}; there is nothing to estimate yet"
                )));
            }
            (Some(p), true) => Estimate {
                variance: p.variance + self.drift,
                ..p
            },
            (prior, false) => {
                let n = readings.len() as f64;
                let mean = readings.iter().sum::<f64>() / n;
                let measured_variance = self.source_noise / n;
                let (mean, variance) = match prior {
                    None => (mean, measured_variance),
                    Some(p) => {
                        let prior_variance = p.variance + self.drift;
                        let gain = prior_variance / (prior_variance + measured_variance);
                        (
                            p.mean + gain * (mean - p.mean),
                            (1.0 - gain) * prior_variance,
                        )
                    }
                };
                let spread = if readings.len() > 1 {
                    readings.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0)
                } else {
                    0.0
                };
                Estimate {
                    mean,
                    variance: variance + spread,
                    updates: prior.map_or(0, |p| p.updates),
                }
            }
        };
        estimate.updates = estimate.updates.saturating_add(1);
        self.estimates.insert(domain, estimate);
        Ok(estimate)
    }
}

/// The Brain Registry entries for the specialists a NOW surprise can wake
/// (R12-CONTRACT-7086), keyed by [`Specialist::as_str`].
///
/// Until these existed the research loop woke "the risk specialist" by a
/// label, and nothing on record said what that brain was allowed to read,
/// which tools it held, what it would abstain on or what authority its
/// output carried — so a wake was attributable to a name and nothing more.
/// Each spec here is validated by [`BrainSpec::validate`] before it is
/// returned, and `ResearchLoop::new` refuses to start unless every
/// specialist has one and every tool a spec names is a tool this node
/// registered.
///
/// What is stated honestly: `calibration` is `0.0` because no specialist's
/// output has yet been scored against an outcome — zero is the floor of the
/// field, not a measurement — and `model_stack` names the in-tree NOW
/// estimators, because no specialist here consults a hosted model
/// (R12-MODEL-4002).
pub fn specialist_registry() -> Result<BTreeMap<&'static str, BrainSpec>> {
    let mut registry = BTreeMap::new();
    for specialist in Specialist::ALL {
        let name = specialist.as_str();
        let (domains, tool): (&[&str], &str) = match specialist {
            Specialist::Forecast => (&["now-state", "forecast"], "model-fit"),
            Specialist::Adversarial => (&["now-state", "premise-testing"], "stress-simulator"),
            Specialist::Sensing => (&["now-state", "source-acquisition"], "source-discovery"),
            Specialist::Risk => (&["now-state", "exposure"], "stress-simulator"),
        };
        let (_, pathway) = route_of(specialist);
        let spec = BrainSpec {
            id: format!("now-specialist/{name}"),
            name: format!("NOW {name} specialist"),
            competency_domains: domains.iter().map(|d| (*d).to_string()).collect(),
            tools: vec![tool.to_string()],
            source_permissions: vec!["platform-state:read".to_string()],
            memory_scope: "research-loop: bounded, one sweep".to_string(),
            model_stack: vec!["qip-world-model::now estimators (in-tree)".to_string()],
            evaluations: vec![
                "qip-deepbrain tests/research.rs: a_now_surprise_raised_in_the_cycle_wakes_forecast_adversarial_sensing_and_risk_specialists"
                    .to_string(),
            ],
            abstention_rules: vec![
                "a surprise below the attention router's materiality wakes nothing".to_string(),
                "a signal past its expiry wakes nothing".to_string(),
            ],
            authority_envelope: format!("advisory_only via {pathway:?}"),
            calibration: 0.0,
            version: 1,
            parent_version_id: None,
        };
        spec.validate()?;
        registry.insert(name, spec);
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimator() -> NowEstimator {
        NowEstimator::new(0.0, 0.01).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn contradictory_sources_widen_the_estimate_and_agreeing_ones_do_not() {
        let agree = estimator()
            .observe("market", &[0.5, 0.5])
            .unwrap_or_else(|e| panic!("{e}"));
        let split = estimator()
            .observe("market", &[0.1, 0.9])
            .unwrap_or_else(|e| panic!("{e}"));
        // Premise: both have the same mean, so only the spread differs.
        assert!((agree.mean - split.mean).abs() < 1e-12);
        assert!(
            split.variance > agree.variance * 10.0,
            "a 0.1/0.9 split must read far less certain than agreement: {agree:?} vs {split:?}"
        );
    }

    #[test]
    fn each_update_moves_the_estimate_towards_the_readings_and_counts() {
        let mut now = estimator();
        let first = now
            .observe("beliefs", &[0.2])
            .unwrap_or_else(|e| panic!("{e}"));
        let second = now
            .observe("beliefs", &[0.8])
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(second.mean > first.mean && second.mean < 0.8);
        assert_eq!(second.updates, 2);
        assert!(now.observe("weather", &[1.0]).is_err());
    }
}
