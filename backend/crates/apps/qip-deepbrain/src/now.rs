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
//! research loop already reads. It is not the §9.6 NOW Brain over economies,
//! supply chains and policy, and the register row stays PARTIAL on that.

use qip_core::error::{Error, Result};
use std::collections::BTreeMap;

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
