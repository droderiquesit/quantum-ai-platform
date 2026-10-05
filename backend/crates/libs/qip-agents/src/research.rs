//! The Research Registry (EXPAND-039): hypotheses, and what became of them.
//!
//! A null result that nobody can find is a null result that will be paid for
//! again. Every proposed hypothesis is screened here before an experiment is
//! scheduled, so a repeat is matched to its recorded failure first.

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

#[derive(Clone, Debug, Default)]
pub struct ResearchRegistry {
    entries: BTreeMap<String, (String, Verdict)>,
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

    pub fn record(&mut self, hypothesis: &str, verdict: Verdict) -> Result<()> {
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
        self.entries
            .insert(k, (hypothesis.trim().to_string(), verdict));
        Ok(())
    }

    /// Screen a proposal before any experiment runs.
    pub fn screen(&self, hypothesis: &str) -> Screening {
        match self.entries.get(&key(hypothesis)) {
            None => Screening::Novel,
            Some((h, Verdict::Null { evidence })) => Screening::PriorNull {
                hypothesis: h.clone(),
                evidence: evidence.clone(),
            },
            Some((h, _)) => Screening::AlreadyKnown {
                hypothesis: h.clone(),
            },
        }
    }
}
