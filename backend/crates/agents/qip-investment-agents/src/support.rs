//! Shared machinery for the specialist agents.
//!
//! Eighteen agents written independently drift. These helpers keep the parts
//! that must not drift in one place: how a finding is assembled, how missing
//! data is reported, and how a conviction is derived from a z-score.
//!
//! The conviction mapping is the one worth reading. Every agent needs to turn
//! "this is 2.3 standard deviations from normal" into a number in `[0, 1]`, and
//! if each invented its own the same evidence would carry different weight
//! depending on which desk happened to look at it.

use qip_agents::finding::{AgentFinding, Direction, NumericFact};
use qip_agents::runtime::AgentContext;
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_numerics::stats;

/// Turn a standardised deviation into a conviction in `[0, 1]`.
///
/// `tanh(|z| / 2)` reaches 0.46 at one sigma, 0.76 at two and 0.90 at three,
/// and never saturates. The alternative — a normal CDF — hits 0.999 at three
/// sigma, which would let a single three-sigma reading produce near-certainty
/// out of what is, in fat-tailed financial data, a monthly occurrence.
pub fn conviction_from_z(z: f64) -> f64 {
    if !z.is_finite() {
        return 0.0;
    }
    (z.abs() / 2.0).tanh().clamp(0.0, 1.0)
}

/// Direction implied by a signed reading, with a dead zone around zero.
///
/// Readings inside the dead zone are `Neutral` rather than a weak view: an
/// agent reporting a direction it does not hold pollutes the aggregate.
pub fn direction_from(value: f64, dead_zone: f64) -> Direction {
    if !value.is_finite() || value.abs() <= dead_zone {
        return Direction::Neutral;
    }
    if value > 0.0 {
        Direction::Positive
    } else {
        Direction::Negative
    }
}

/// Standardise the last observation against its own history.
///
/// Returns `None` rather than zero when there is not enough history or the
/// series does not vary: a z-score of zero would read as "perfectly normal",
/// which is a different claim from "cannot tell".
pub fn z_score_of_last(series: &[f64], minimum: usize) -> Option<f64> {
    if series.len() < minimum {
        return None;
    }
    let (head, last) = series.split_at(series.len() - 1);
    let sigma = stats::stddev(head);
    if !sigma.is_finite() || sigma < 1e-12 {
        return None;
    }
    let z = (last[0] - stats::mean(head)) / sigma;
    z.is_finite().then_some(z)
}

/// Robust variant, standardising on the median absolute deviation.
///
/// Preferred where a single outlier would otherwise inflate the denominator
/// and hide the very move the agent is looking for.
pub fn robust_z_score_of_last(series: &[f64], minimum: usize) -> Option<f64> {
    if series.len() < minimum {
        return None;
    }
    let (head, last) = series.split_at(series.len() - 1);
    let mad = stats::median_absolute_deviation(head);
    if !mad.is_finite() || mad < 1e-12 {
        return None;
    }
    // 1.4826 scales the MAD to a standard deviation for normal data.
    let z = (last[0] - stats::median(head)) / (1.4826 * mad);
    z.is_finite().then_some(z)
}

/// Assemble a finding, filling in the fields every agent must supply.
#[derive(Debug)]
pub struct FindingBuilder {
    finding: AgentFinding,
}

impl FindingBuilder {
    pub fn new(ctx: &AgentContext, as_of: Timestamp, claim: impl Into<String>) -> Self {
        Self {
            finding: AgentFinding::new(
                ctx.run_id().clone(),
                ctx.manifest().id.clone(),
                ctx.now(),
                as_of,
                claim,
            ),
        }
    }

    pub fn direction(mut self, direction: Direction, conviction: f64) -> Self {
        self.finding = self.finding.with_direction(direction, conviction);
        self
    }

    pub fn fact(mut self, fact: NumericFact) -> Self {
        self.finding = self.finding.with_fact(fact);
        self
    }

    pub fn evidence(mut self, evidence: Vec<String>) -> Self {
        self.finding = self.finding.with_evidence(evidence);
        self
    }

    pub fn falsifiers(mut self, falsifiers: Vec<String>) -> Self {
        self.finding = self.finding.with_falsifiers(falsifiers);
        self
    }

    pub fn caveats(mut self, caveats: Vec<String>) -> Self {
        self.finding = self.finding.with_caveats(caveats);
        self
    }

    pub fn missing(mut self, missing: Vec<String>) -> Self {
        self.finding = self.finding.with_missing_inputs(missing);
        self
    }

    pub fn follow_ups(mut self, follow_ups: Vec<String>) -> Self {
        self.finding.follow_ups = follow_ups;
        self
    }

    pub fn build(self) -> Result<AgentFinding> {
        self.finding.validate()?;
        Ok(self.finding)
    }
}

/// A finding recording that the agent had nothing to work with.
///
/// Used wherever the data an agent needs is absent. Reporting this rather than
/// erroring keeps a missing feed from taking down the whole cycle, and reporting
/// it rather than returning a neutral view keeps it out of the aggregate.
pub fn no_data(ctx: &AgentContext, as_of: Timestamp, reason: impl Into<String>) -> AgentFinding {
    AgentFinding::no_view(
        ctx.run_id().clone(),
        ctx.manifest().id.clone(),
        ctx.now(),
        as_of,
        reason,
    )
}

/// A finding recording that the question was outside the agent's competence.
pub fn out_of_scope(
    ctx: &AgentContext,
    as_of: Timestamp,
    reason: impl Into<String>,
) -> AgentFinding {
    AgentFinding::deferred(
        ctx.run_id().clone(),
        ctx.manifest().id.clone(),
        ctx.now(),
        as_of,
        reason,
    )
}

/// The computed-fact constructor, with the agent's own name as the producer.
pub fn computed(
    ctx: &AgentContext,
    label: &str,
    value: f64,
    unit: &str,
    inputs: &[&str],
) -> NumericFact {
    NumericFact::computed(
        label,
        value,
        unit,
        ctx.manifest().id.clone(),
        inputs.iter().map(|s| (*s).to_string()).collect(),
    )
}
