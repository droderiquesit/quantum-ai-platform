//! What agents are asked, and what they may answer.
//!
//! The organisation exchanges two types: an [`AgentBrief`] going in and an
//! [`AgentFinding`] coming out. Making them universal is what lets a
//! coordinator dispatch to eighteen specialists without knowing what any of
//! them does, and what lets the audit trail have one shape.
//!
//! The important type here is [`NumericFact`]. Every number an agent reports
//! carries its own provenance, and the only provenances that exist are
//! *observed* and *computed*. There is no variant for "the model said so",
//! which is how the platform's rule that language models never produce
//! decision numbers survives contact with eighteen agents written at
//! different times.

use qip_core::error::{Error, Result};
use qip_core::ids::{AgentRunId, ObjectId};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Where a number came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NumericProvenance {
    /// Read from a data source, as of a stated time.
    Observed {
        source: String,
        as_of: Timestamp,
        /// The record it was read from, so it can be re-fetched.
        record_id: String,
    },
    /// Produced by a named calculation from named inputs.
    Computed {
        /// The function or component that computed it.
        by: String,
        /// The facts it was computed from, by label.
        inputs: Vec<String>,
    },
}

impl NumericProvenance {
    pub fn observed(
        source: impl Into<String>,
        as_of: Timestamp,
        record_id: impl Into<String>,
    ) -> Self {
        Self::Observed {
            source: source.into(),
            as_of,
            record_id: record_id.into(),
        }
    }

    pub fn computed(by: impl Into<String>, inputs: Vec<String>) -> Self {
        Self::Computed {
            by: by.into(),
            inputs,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Observed {
                source,
                as_of,
                record_id,
            } => format!("observed from {source} record {record_id} as of {as_of}"),
            Self::Computed { by, inputs } => {
                format!("computed by {by} from [{}]", inputs.join(", "))
            }
        }
    }
}

/// A number an agent reports, with where it came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NumericFact {
    /// What the number means, e.g. `expected_move_bps`.
    pub label: String,
    pub value: f64,
    /// Unit, e.g. `bps`, `usd`, `ratio`, `days`. Never blank: an unlabelled
    /// number is the input to a units error nobody catches.
    pub unit: String,
    pub provenance: NumericProvenance,
}

impl NumericFact {
    pub fn observed(
        label: impl Into<String>,
        value: f64,
        unit: impl Into<String>,
        source: impl Into<String>,
        as_of: Timestamp,
        record_id: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            value,
            unit: unit.into(),
            provenance: NumericProvenance::observed(source, as_of, record_id),
        }
    }

    pub fn computed(
        label: impl Into<String>,
        value: f64,
        unit: impl Into<String>,
        by: impl Into<String>,
        inputs: Vec<String>,
    ) -> Self {
        Self {
            label: label.into(),
            value,
            unit: unit.into(),
            provenance: NumericProvenance::computed(by, inputs),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.label.trim().is_empty() {
            return Err(Error::invalid("numeric fact has no label"));
        }
        if self.unit.trim().is_empty() {
            return Err(Error::invalid(format!(
                "numeric fact {} has no unit",
                self.label
            )));
        }
        if !self.value.is_finite() {
            return Err(Error::numeric(format!(
                "numeric fact {} is not finite: {}",
                self.label, self.value
            )));
        }
        if let NumericProvenance::Computed { by, inputs } = &self.provenance
            && inputs.is_empty()
        {
            return Err(Error::invalid(format!(
                "numeric fact {} claims to be computed by {by} from nothing",
                self.label
            )));
        }
        Ok(())
    }
}

/// Which way an agent thinks something cuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Positive,
    Negative,
    /// Genuinely two-sided: volatility rises either way.
    Ambiguous,
    /// No view. Distinct from `Ambiguous`: one is a finding, the other is not.
    Neutral,
}

impl Direction {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
            Self::Ambiguous => "ambiguous",
            Self::Neutral => "neutral",
        }
    }

    pub const fn sign(&self) -> f64 {
        match self {
            Self::Positive => 1.0,
            Self::Negative => -1.0,
            Self::Ambiguous | Self::Neutral => 0.0,
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The question put to an agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentBrief {
    /// What is being asked, in one sentence.
    pub question: String,
    /// Why it is being asked, so the agent can judge relevance.
    pub context: String,
    /// Instruments in scope.
    pub objects: Vec<ObjectId>,
    /// Entities in scope, by resolved entity id.
    pub entities: Vec<String>,
    /// The point in time the agent must reason as of. Every read the agent
    /// makes is filtered to this, which is what makes a replayed run produce
    /// the replayed answer.
    pub as_of: Timestamp,
    /// Horizon the answer should address.
    pub horizon: Duration,
    /// Specific sub-questions, when the caller has them.
    pub questions: Vec<String>,
}

impl AgentBrief {
    pub fn new(question: impl Into<String>, as_of: Timestamp, horizon: Duration) -> Self {
        Self {
            question: question.into(),
            context: String::new(),
            objects: Vec::new(),
            entities: Vec::new(),
            as_of,
            horizon,
            questions: Vec::new(),
        }
    }

    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = context.into();
        self
    }

    pub fn about_objects(mut self, objects: Vec<ObjectId>) -> Self {
        self.objects = objects;
        self
    }

    pub fn about_entities(mut self, entities: Vec<String>) -> Self {
        self.entities = entities;
        self
    }

    pub fn asking(mut self, questions: Vec<String>) -> Self {
        self.questions = questions;
        self
    }
}

/// How an agent concluded its work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    /// The agent answered the question it was asked.
    Complete,
    /// The agent ran out of budget or data and says so.
    Partial,
    /// The question was outside the agent's declared competence.
    Deferred,
    /// The agent looked and found nothing. A real answer, not a failure.
    NoView,
}

impl FindingStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Deferred => "deferred",
            Self::NoView => "no_view",
        }
    }

    /// Whether downstream reasoning may weight this finding.
    ///
    /// A deferred finding carries no information about the question and must
    /// not be averaged in as a neutral vote — that would let ignorance dilute
    /// a strong view.
    pub const fn is_informative(&self) -> bool {
        matches!(self, Self::Complete | Self::Partial | Self::NoView)
    }
}

/// One agent's answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentFinding {
    pub run_id: AgentRunId,
    pub agent_id: String,
    pub produced_at: Timestamp,
    /// The as-of time the agent reasoned at, copied from the brief so a
    /// finding is self-describing once it leaves the run.
    pub as_of: Timestamp,
    pub status: FindingStatus,
    /// The claim, in the agent's own words.
    pub claim: String,
    pub direction: Direction,
    /// How strongly the agent holds the view, in `[0, 1]`.
    ///
    /// Set by the agent's own arithmetic over its evidence, never by narrative.
    pub conviction: f64,
    /// The numbers behind the claim, each with provenance.
    pub facts: Vec<NumericFact>,
    /// Record ids supporting the claim.
    pub evidence: Vec<String>,
    /// What would make this wrong. An empty list is a validation failure: a
    /// claim with no falsifier is not a finding, it is an opinion.
    pub falsifiers: Vec<String>,
    /// Known weaknesses in the analysis.
    pub caveats: Vec<String>,
    /// Questions this agent could not answer and thinks someone should.
    pub follow_ups: Vec<String>,
    /// Data the agent wanted and did not have. Surfaced rather than silently
    /// worked around, because a missing input is usually the whole story.
    pub missing_inputs: Vec<String>,
}

impl AgentFinding {
    pub fn new(
        run_id: AgentRunId,
        agent_id: impl Into<String>,
        produced_at: Timestamp,
        as_of: Timestamp,
        claim: impl Into<String>,
    ) -> Self {
        Self {
            run_id,
            agent_id: agent_id.into(),
            produced_at,
            as_of,
            status: FindingStatus::Complete,
            claim: claim.into(),
            direction: Direction::Neutral,
            conviction: 0.0,
            facts: Vec::new(),
            evidence: Vec::new(),
            falsifiers: Vec::new(),
            caveats: Vec::new(),
            follow_ups: Vec::new(),
            missing_inputs: Vec::new(),
        }
    }

    /// A finding recording that the question was outside the agent's remit.
    pub fn deferred(
        run_id: AgentRunId,
        agent_id: impl Into<String>,
        produced_at: Timestamp,
        as_of: Timestamp,
        reason: impl Into<String>,
    ) -> Self {
        let mut finding = Self::new(run_id, agent_id, produced_at, as_of, reason);
        finding.status = FindingStatus::Deferred;
        finding
    }

    /// A finding recording that the agent looked and found nothing.
    pub fn no_view(
        run_id: AgentRunId,
        agent_id: impl Into<String>,
        produced_at: Timestamp,
        as_of: Timestamp,
        reason: impl Into<String>,
    ) -> Self {
        let mut finding = Self::new(run_id, agent_id, produced_at, as_of, reason);
        finding.status = FindingStatus::NoView;
        // Looking and finding nothing is a definite statement about the
        // world, so it is falsifiable in the same way any other claim is.
        finding.falsifiers =
            vec!["evidence appears that the agent's search would have found".into()];
        finding
    }

    pub fn with_direction(mut self, direction: Direction, conviction: f64) -> Self {
        self.direction = direction;
        self.conviction = conviction.clamp(0.0, 1.0);
        self
    }

    pub fn with_fact(mut self, fact: NumericFact) -> Self {
        self.facts.push(fact);
        self
    }

    pub fn with_evidence(mut self, evidence: Vec<String>) -> Self {
        self.evidence = evidence;
        self
    }

    pub fn with_falsifiers(mut self, falsifiers: Vec<String>) -> Self {
        self.falsifiers = falsifiers;
        self
    }

    pub fn with_caveats(mut self, caveats: Vec<String>) -> Self {
        self.caveats = caveats;
        self
    }

    pub fn with_missing_inputs(mut self, missing: Vec<String>) -> Self {
        if !missing.is_empty() && self.status == FindingStatus::Complete {
            self.status = FindingStatus::Partial;
        }
        self.missing_inputs = missing;
        self
    }

    pub fn partial(mut self) -> Self {
        self.status = FindingStatus::Partial;
        self
    }

    pub fn fact(&self, label: &str) -> Option<&NumericFact> {
        self.facts.iter().find(|f| f.label == label)
    }

    /// Conviction weighted down for a partial answer.
    ///
    /// An agent that saw half the data should not carry the same weight as one
    /// that saw all of it, and asking each agent to remember to discount
    /// itself would not survive eighteen implementations.
    pub fn effective_conviction(&self) -> f64 {
        match self.status {
            FindingStatus::Complete => self.conviction,
            FindingStatus::Partial => self.conviction * 0.6,
            FindingStatus::Deferred | FindingStatus::NoView => 0.0,
        }
    }

    /// Check the finding is well formed before it enters the record.
    pub fn validate(&self) -> Result<()> {
        if self.claim.trim().is_empty() {
            return Err(Error::invalid(format!(
                "agent {} produced a finding with no claim",
                self.agent_id
            )));
        }
        if !(0.0..=1.0).contains(&self.conviction) {
            return Err(Error::invalid(format!(
                "agent {} reported conviction {} outside [0, 1]",
                self.agent_id, self.conviction
            )));
        }
        if self.produced_at < self.as_of {
            return Err(Error::invalid(format!(
                "agent {} produced a finding at {} for an as-of time of {}, which is in its future",
                self.agent_id, self.produced_at, self.as_of
            )));
        }
        for fact in &self.facts {
            fact.validate()?;
        }
        let directional = matches!(self.direction, Direction::Positive | Direction::Negative);
        if directional && self.falsifiers.is_empty() {
            return Err(Error::invalid(format!(
                "agent {} took a {} view with no falsifier; an unfalsifiable claim cannot be reviewed",
                self.agent_id, self.direction
            )));
        }
        if directional && self.conviction > 0.0 && self.evidence.is_empty() && self.facts.is_empty()
        {
            return Err(Error::invalid(format!(
                "agent {} took a {} view with conviction {:.2} on no evidence",
                self.agent_id, self.direction, self.conviction
            )));
        }
        if self.status == FindingStatus::Partial && self.missing_inputs.is_empty() {
            return Err(Error::invalid(format!(
                "agent {} reported a partial finding without saying what was missing",
                self.agent_id
            )));
        }
        Ok(())
    }
}
