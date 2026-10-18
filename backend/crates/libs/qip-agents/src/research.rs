//! The Research Registry (EXPAND-039): hypotheses, and what became of them.
//!
//! A null result that nobody can find is a null result that will be paid for
//! again. Every proposed hypothesis is screened here before an experiment is
//! scheduled, so a repeat is matched to its recorded failure first.
//!
//! Versioned like the ontology registry: every verdict recorded for a
//! hypothesis is kept as a numbered version, and a version that does not
//! raise the latest is refused. Recording once overwrote the entry, so a
//! hypothesis found null and later re-opened lost the null result it was
//! screened against, and nothing could say which verdict had been current
//! when an experiment was scheduled.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Open,
    Null { evidence: String },
    Supported { evidence: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screening {
    Novel,
    /// The same hypothesis was already tested and found null.
    PriorNull {
        hypothesis: String,
        evidence: String,
    },
    /// Already open or supported: no new experiment is needed to ask it.
    AlreadyKnown {
        hypothesis: String,
    },
}

/// One recorded verdict on a hypothesis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchVersion {
    /// One for the first verdict on a hypothesis, then strictly rising.
    pub version: u32,
    /// The hypothesis as it was stated when this verdict was recorded.
    pub hypothesis: String,
    pub verdict: Verdict,
}

/// Versions kept per hypothesis. Past it the oldest is given up; the latest
/// version number still rises, so a refused stale version stays refused.
pub const VERSIONS_PER_HYPOTHESIS: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct ResearchRegistry {
    /// Versions per normalised hypothesis, oldest first, never empty.
    entries: BTreeMap<String, Vec<ResearchVersion>>,
}

/// Case and whitespace do not make a hypothesis new.
fn key(hypothesis: &str) -> String {
    hypothesis
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

impl ResearchRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `verdict` as the next version of `hypothesis`, returning the
    /// version it was recorded as.
    pub fn record(&mut self, hypothesis: &str, verdict: Verdict) -> Result<u32> {
        let next = self
            .latest_version(hypothesis)
            .map_or(1, |latest| latest.saturating_add(1));
        self.record_version(hypothesis, next, verdict)?;
        Ok(next)
    }

    /// Record `verdict` as `version` of `hypothesis`. Refuses a version of
    /// zero and a version not above the latest held for the hypothesis: a
    /// late-arriving older verdict would otherwise replace the current one,
    /// the failure the ontology registry's same rule names.
    pub fn record_version(
        &mut self,
        hypothesis: &str,
        version: u32,
        verdict: Verdict,
    ) -> Result<()> {
        let k = key(hypothesis);
        if k.is_empty() {
            return Err(Error::invalid("a hypothesis must be stated to be recorded"));
        }
        if let Verdict::Null { evidence } | Verdict::Supported { evidence } = &verdict
            && evidence.trim().is_empty()
        {
            return Err(Error::invalid(
                "a verdict without evidence cannot stop a later repeat; cite the experiment",
            ));
        }
        if version == 0 {
            return Err(Error::invalid(
                "a research verdict of version zero carries no version; versions start at one",
            ));
        }
        if let Some(latest) = self.latest_version(hypothesis)
            && version <= latest
        {
            return Err(Error::invalid(format!(
                "`{}` version {version} is not above the latest recorded version {latest}; \
                 record version {} or higher",
                hypothesis.trim(),
                latest.saturating_add(1)
            )));
        }
        let versions = self.entries.entry(k).or_default();
        if versions.len() >= VERSIONS_PER_HYPOTHESIS {
            versions.remove(0);
        }
        versions.push(ResearchVersion {
            version,
            hypothesis: hypothesis.trim().to_string(),
            verdict,
        });
        Ok(())
    }

    /// The latest version recorded for `hypothesis`, if any.
    pub fn latest_version(&self, hypothesis: &str) -> Option<u32> {
        self.entries
            .get(&key(hypothesis))
            .and_then(|versions| versions.last())
            .map(|v| v.version)
    }

    /// Every held version of `hypothesis`, oldest first.
    pub fn history(&self, hypothesis: &str) -> &[ResearchVersion] {
        self.entries
            .get(&key(hypothesis))
            .map_or(&[], Vec::as_slice)
    }

    /// Screen a proposal before any experiment runs.
    pub fn screen(&self, hypothesis: &str) -> Screening {
        match self.history(hypothesis).last() {
            None => Screening::Novel,
            Some(ResearchVersion {
                hypothesis: h,
                verdict: Verdict::Null { evidence },
                ..
            }) => Screening::PriorNull {
                hypothesis: h.clone(),
                evidence: evidence.clone(),
            },
            Some(ResearchVersion { hypothesis: h, .. }) => Screening::AlreadyKnown {
                hypothesis: h.clone(),
            },
        }
    }
}
