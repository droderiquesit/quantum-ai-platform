//! Ensembles held by regime, each with its own calibrated interval
//! (R12-MODEL-4006).
//!
//! A blend calibrated in one regime says nothing reliable about another: its
//! interval assumes the calibration rows and a new row are exchangeable, and a
//! regime change is the case where they are not. So the blend a caller reads
//! is the one registered for the regime the platform classified, and a regime
//! nothing was registered for is a refusal, not the nearest regime's blend.
//!
//! # What this does not decide
//!
//! Which members and shares belong to a regime is the caller's (the deep
//! brain's class board); this type holds the result and reads it. The cap on
//! members the "compact in hot" clause asks for is a figure nobody has given,
//! so it is a parameter of [`RegimeEnsembles::new`], never a constant here.

use std::collections::BTreeMap;

use qip_core::error::{Error, Result};

use crate::conformal::ConformalInterval;
use crate::ensemble::Ensemble;

/// A prediction with the interval it was calibrated to and the regime it
/// belongs to.
#[derive(Clone, Debug, PartialEq)]
pub struct EnsembleReading {
    pub regime: String,
    pub point: f64,
    pub low: f64,
    pub high: f64,
    /// Member references and shares, so a reader sees who produced the point.
    pub members: Vec<(String, f64)>,
    /// Rows the interval was calibrated on, the evidence behind it.
    pub calibration_rows: usize,
}

/// One calibrated ensemble per regime.
#[derive(Clone, Debug)]
pub struct RegimeEnsembles {
    max_members: usize,
    by_regime: BTreeMap<String, (Ensemble, ConformalInterval)>,
}

impl RegimeEnsembles {
    /// `max_members` bounds how many members any registered ensemble may
    /// carry. It must be at least one.
    pub fn new(max_members: usize) -> Result<Self> {
        if max_members == 0 {
            return Err(Error::invalid(
                "an ensemble set needs a member cap of at least one; zero admits nothing",
            ));
        }
        Ok(Self {
            max_members,
            by_regime: BTreeMap::new(),
        })
    }

    /// Register `ensemble` and its `interval` for `regime`, replacing an
    /// earlier registration for that regime. Refuses an empty regime key and
    /// an ensemble above the cap: a blend over the cap is refused, not
    /// trimmed, because trimming would change the shares the interval was
    /// calibrated against.
    pub fn register(
        &mut self,
        regime: &str,
        ensemble: Ensemble,
        interval: ConformalInterval,
    ) -> Result<()> {
        if regime.trim().is_empty() {
            return Err(Error::invalid(
                "an ensemble is registered under the regime it was calibrated in; got none",
            ));
        }
        if ensemble.len() > self.max_members {
            return Err(Error::invalid(format!(
                "{} members exceed the cap of {}; register a smaller blend and calibrate it",
                ensemble.len(),
                self.max_members
            )));
        }
        self.by_regime
            .insert(regime.to_string(), (ensemble, interval));
        Ok(())
    }

    /// The regimes an ensemble is registered for.
    pub fn regimes(&self) -> Vec<&str> {
        self.by_regime.keys().map(String::as_str).collect()
    }

    /// Predict `inputs` with the ensemble registered for `regime`. Refuses a
    /// regime with none: a neighbour's interval is not this regime's.
    pub fn predict(&self, regime: &str, inputs: &[f64]) -> Result<EnsembleReading> {
        let Some((ensemble, interval)) = self.by_regime.get(regime) else {
            return Err(Error::invalid(format!(
                "no ensemble is registered for regime {regime}; its interval was calibrated \
                 elsewhere and does not transfer"
            )));
        };
        let point = ensemble.predict(inputs)?;
        let (low, high) = interval.around(point);
        Ok(EnsembleReading {
            regime: regime.to_string(),
            point,
            low,
            high,
            members: ensemble.shares(),
            calibration_rows: interval.calibration_rows(),
        })
    }
}
