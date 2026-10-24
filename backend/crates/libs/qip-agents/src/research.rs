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
use qip_core::hash::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
    /// The experiments this verdict cites, as `<campaign id>/<stage>`
    /// references (ADR 0152 decision 11), sorted and unique. Empty only for a
    /// verdict recorded through [`ResearchRegistry::record`], which names no
    /// experiment and is therefore only ever `Open` on a running path.
    #[serde(default)]
    pub experiments: Vec<String>,
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
pub fn normalise(hypothesis: &str) -> String {
    hypothesis
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn key(hypothesis: &str) -> String {
    normalise(hypothesis)
}

/// A research campaign's identity: the SHA-256 digest of its normalised
/// question (ADR 0152 decision 11). Content-derived and not a timestamp or a
/// counter, so the same question asked twice is one campaign and a verdict
/// can be matched to the campaign that produced it after a restart.
pub fn campaign_id(question: &str) -> Result<String> {
    let normalised = normalise(question);
    if normalised.is_empty() {
        return Err(Error::invalid(
            "a campaign researches a question; state the question it answers",
        ));
    }
    Ok(sha256_hex(normalised.as_bytes()))
}

/// The reference an experiment is cited by: `<campaign id>/<stage>`.
pub fn experiment_ref(campaign_id: &str, stage: &str) -> String {
    format!("{campaign_id}/{stage}")
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

    /// Record a concluded verdict together with the experiments it rests on.
    ///
    /// Each reference must be `<campaign id>/<stage>` for *this* hypothesis's
    /// campaign: a verdict citing another question's experiment, or none, is
    /// refused, because a null that names no experiment cannot be audited and
    /// would stop a later repeat on free text alone. `Open` takes none.
    pub fn record_concluded(
        &mut self,
        hypothesis: &str,
        verdict: Verdict,
        experiments: &[String],
    ) -> Result<u32> {
        let id = campaign_id(hypothesis)?;
        let prefix = format!("{id}/");
        let concluded = !matches!(verdict, Verdict::Open);
        if concluded && experiments.is_empty() {
            return Err(Error::invalid(
                "a concluded verdict must cite the experiments it rests on; none were given",
            ));
        }
        let mut cited = BTreeSet::new();
        for reference in experiments {
            let stage = reference.strip_prefix(&prefix).unwrap_or_default();
            if stage.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "experiment reference `{reference}` is not `{prefix}<stage>` for this \
                     hypothesis's campaign"
                )));
            }
            cited.insert(reference.clone());
        }
        let next = self
            .latest_version(hypothesis)
            .map_or(1, |latest| latest.saturating_add(1));
        self.record_version(hypothesis, next, verdict)?;
        if let Some(recorded) = self
            .entries
            .get_mut(&key(hypothesis))
            .and_then(|versions| versions.last_mut())
        {
            recorded.experiments = cited.into_iter().collect();
        }
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
            experiments: Vec::new(),
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
