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
    /// The benchmark a concluded verdict was judged against (ADR 0348): the
    /// class's row of ADR 0192's table. `None` only on an `Open` version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark: Option<BenchmarkRef>,
}

/// A capability class's benchmark as ADR 0192's table names it: the class,
/// the incumbent a candidate must beat, and the criterion that gates it
/// (ADR 0348 decision 2). Typed so a verdict cites a row of the table, never
/// a free-text suite ADR 0152 declined.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchmarkRef {
    pub class: String,
    pub incumbent: String,
    pub criterion: String,
}

impl BenchmarkRef {
    /// Refuses a blank field: a benchmark naming no incumbent or no gate is
    /// the free text this type replaces.
    pub fn new(class: &str, incumbent: &str, criterion: &str) -> Result<Self> {
        for (name, value) in [
            ("class", class),
            ("incumbent", incumbent),
            ("criterion", criterion),
        ] {
            if value.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "a benchmark names its {name}; cite the class's row of ADR 0192's table"
                )));
            }
        }
        Ok(Self {
            class: class.trim().to_string(),
            incumbent: incumbent.trim().to_string(),
            criterion: criterion.trim().to_string(),
        })
    }
}

/// A contradiction the detectors raised, kept under the campaign id of the
/// question it raised until a campaign on that question concludes
/// (ADR 0348 decision 3). Keyed by campaign, so no subject is named.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contradiction {
    pub campaign: String,
    /// The evidence references the raising signal cited, sorted and unique.
    pub evidence: Vec<String>,
    /// The registry version of the concluded verdict that resolved it.
    pub resolved_by: Option<u32>,
}

/// Contradiction records kept. Past it the oldest resolved one is given up,
/// and a registry full of unresolved ones refuses the next.
pub const MAX_CONTRADICTIONS: usize = 1024;

/// Versions kept per hypothesis. Past it the oldest is given up; the latest
/// version number still rises, so a refused stale version stays refused.
pub const VERSIONS_PER_HYPOTHESIS: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct ResearchRegistry {
    /// Versions per normalised hypothesis, oldest first, never empty.
    entries: BTreeMap<String, Vec<ResearchVersion>>,
    /// Contradictions by campaign id, in the order first recorded.
    contradictions: Vec<Contradiction>,
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
    ///
    /// A concluded verdict also cites the benchmark it was judged against
    /// (ADR 0348): one without is refused, because a verdict that names no
    /// incumbent cannot say what it beat or failed to beat. Recording it
    /// resolves any open contradiction on the same campaign.
    pub fn record_concluded(
        &mut self,
        hypothesis: &str,
        verdict: Verdict,
        experiments: &[String],
        benchmark: Option<&BenchmarkRef>,
    ) -> Result<u32> {
        let id = campaign_id(hypothesis)?;
        let prefix = format!("{id}/");
        let concluded = !matches!(verdict, Verdict::Open);
        if concluded && experiments.is_empty() {
            return Err(Error::invalid(
                "a concluded verdict must cite the experiments it rests on; none were given",
            ));
        }
        if concluded && benchmark.is_none() {
            return Err(Error::invalid(
                "a concluded verdict must cite its class's benchmark (ADR 0192's table, ADR 0348); \
                 none was given",
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
            recorded.benchmark = benchmark.cloned();
        }
        if concluded {
            for open in self
                .contradictions
                .iter_mut()
                .filter(|c| c.campaign == id && c.resolved_by.is_none())
            {
                open.resolved_by = Some(next);
            }
        }
        Ok(next)
    }

    /// Record a contradiction under `campaign`, the campaign id of the
    /// question it raised (ADR 0348 decision 3). Refuses empty evidence and
    /// an id that is not a campaign digest. A contradiction already open on
    /// the campaign takes the new evidence rather than opening a second.
    pub fn record_contradiction(&mut self, campaign: &str, evidence: &[String]) -> Result<()> {
        let cited: BTreeSet<String> = evidence
            .iter()
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty())
            .collect();
        if cited.is_empty() {
            return Err(Error::invalid(
                "a contradiction cites the evidence that raised it; none was given",
            ));
        }
        if campaign.len() != 64 || !campaign.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::invalid(format!(
                "`{campaign}` is not a campaign id; key a contradiction by `campaign_id(question)`"
            )));
        }
        if let Some(open) = self
            .contradictions
            .iter_mut()
            .find(|c| c.campaign == campaign && c.resolved_by.is_none())
        {
            let merged: BTreeSet<String> = open.evidence.drain(..).chain(cited).collect();
            open.evidence = merged.into_iter().collect();
            return Ok(());
        }
        if self.contradictions.len() >= MAX_CONTRADICTIONS {
            let Some(oldest) = self
                .contradictions
                .iter()
                .position(|c| c.resolved_by.is_some())
            else {
                return Err(Error::invalid(format!(
                    "the registry holds {MAX_CONTRADICTIONS} unresolved contradictions; conclude \
                     a campaign before recording another"
                )));
            };
            self.contradictions.remove(oldest);
        }
        self.contradictions.push(Contradiction {
            campaign: campaign.to_string(),
            evidence: cited.into_iter().collect(),
            resolved_by: None,
        });
        Ok(())
    }

    /// Contradictions no concluded campaign has resolved, oldest first.
    pub fn unresolved_contradictions(&self) -> Vec<&Contradiction> {
        self.contradictions
            .iter()
            .filter(|c| c.resolved_by.is_none())
            .collect()
    }

    /// Every hypothesis whose latest verdict is `Open` (ADR 0348 decision 1).
    pub fn open_questions(&self) -> Vec<&ResearchVersion> {
        self.entries
            .values()
            .filter_map(|versions| versions.last())
            .filter(|latest| matches!(latest.verdict, Verdict::Open))
            .collect()
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
            benchmark: None,
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
