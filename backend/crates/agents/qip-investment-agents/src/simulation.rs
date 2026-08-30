//! The simulation analyst.
//!
//! Turns a thesis into a distribution of outcomes rather than a point estimate,
//! and reports the loss tail alongside the centre. A thesis with a good
//! expected value and an unacceptable tail is a bad thesis, and reporting only
//! the mean is how that gets missed.
//!
//! The resampling is a stationary block bootstrap. Independent resampling of
//! daily returns would destroy the volatility clustering the underlying data
//! has, and understate the tail by exactly the amount that matters.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data, out_of_scope};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use qip_core::rng::Rng;
use qip_numerics::stats;
use std::sync::Arc;

/// Paths simulated. Enough that the 5th percentile is stable to about a tenth
/// of a percent, which is the precision the number is used at.
const PATHS: usize = 2_000;

/// Mean block length in observations.
///
/// Roughly a fortnight of trading. Long enough to carry a volatility cluster
/// through, short enough that a path is not a replay of one historical episode.
const MEAN_BLOCK: f64 = 10.0;

const MINIMUM_HISTORY: usize = 60;

/// Runs the distribution of outcomes a thesis implies.
#[derive(Debug)]
pub struct SimulationAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl SimulationAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for SimulationAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn accepts(&self, brief: &AgentBrief) -> bool {
        !brief.objects.is_empty() && brief.horizon.as_nanos() > 0
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let Some(subject) = brief.objects.first() else {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                "a simulation needs an instrument to simulate",
            ));
        };

        let returns: Vec<f64> = {
            let market = self.desk.market.get(ctx)?;
            let Some(state) = market.snapshot.get(subject) else {
                return Ok(no_data(
                    ctx,
                    brief.as_of,
                    format!("no market state for {}", subject.as_str()),
                ));
            };
            let closes: Vec<f64> = state
                .bars
                .as_of(brief.as_of)
                .iter()
                .map(|b| b.close.to_f64())
                .filter(|c| c.is_finite() && *c > 0.0)
                .collect();
            stats::log_returns(&closes)
        };

        if returns.len() < MINIMUM_HISTORY {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "{} return observations for {}, {MINIMUM_HISTORY} needed to resample",
                    returns.len(),
                    subject.as_str()
                ),
            ));
        }

        // The horizon in trading days, which is what the return series is in.
        let steps = (brief.horizon.as_days_f64() * (252.0 / 365.0))
            .round()
            .max(1.0) as usize;

        let mut outcomes = Vec::with_capacity(PATHS);
        for _ in 0..PATHS {
            outcomes.push(bootstrap_path(ctx, &returns, steps));
        }
        outcomes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let expected = stats::mean(&outcomes);
        let median = stats::quantile_sorted(&outcomes, 0.5);
        let p05 = stats::quantile_sorted(&outcomes, 0.05);
        let p95 = stats::quantile_sorted(&outcomes, 0.95);
        // Expected shortfall at 95%: the average of the worst 5% of paths, not
        // the 5th percentile itself. The percentile says where the tail starts;
        // the shortfall says what is in it.
        let tail_size = (PATHS / 20).max(1);
        let expected_shortfall = stats::mean(&outcomes[..tail_size]);
        let probability_of_loss =
            outcomes.iter().filter(|o| **o < 0.0).count() as f64 / PATHS as f64;

        // Only the sign of the expected outcome is a view, and even that is
        // held weakly: a bootstrap of the past is not a forecast.
        let conviction = if expected.abs() < 1e-9 {
            0.0
        } else {
            // How far the expected outcome sits from zero relative to the
            // spread of outcomes: the simulation's own signal-to-noise.
            let spread = stats::stddev(&outcomes).max(1e-9);
            ((expected.abs() / spread) / 2.0).tanh().min(0.6)
        };

        FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "over {steps} trading days {} returns {:+.2}% expected, {:.0}% chance of loss, {:.2}% expected shortfall in the worst 5%",
                subject.as_str(),
                expected * 100.0,
                probability_of_loss * 100.0,
                expected_shortfall * 100.0
            ),
        )
        .direction(
            if expected > 0.0 {
                Direction::Positive
            } else if expected < 0.0 {
                Direction::Negative
            } else {
                Direction::Neutral
            },
            conviction,
        )
        .fact(computed(ctx, "expected_return", expected, "ratio", &["close"]))
        .fact(computed(ctx, "median_return", median, "ratio", &["close"]))
        .fact(computed(ctx, "return_p05", p05, "ratio", &["close"]))
        .fact(computed(ctx, "return_p95", p95, "ratio", &["close"]))
        .fact(computed(
            ctx,
            "expected_shortfall_95",
            expected_shortfall,
            "ratio",
            &["close"],
        ))
        .fact(computed(
            ctx,
            "probability_of_loss",
            probability_of_loss,
            "probability",
            &["close"],
        ))
        .fact(computed(ctx, "paths", PATHS as f64, "count", &["close"]))
        .evidence(vec![format!(
            "bars:{}@{} ({} returns)",
            subject.as_str(),
            brief.as_of,
            returns.len()
        )])
        .falsifiers(vec![
            format!(
                "the realised {steps}-day return falls outside [{:.2}%, {:.2}%], the simulated 5-95 range",
                p05 * 100.0,
                p95 * 100.0
            ),
        ])
        .caveats(vec![
            "a block bootstrap resamples the past; it cannot produce a regime the history does not contain"
                .to_string(),
            format!(
                "the tail rests on {tail_size} paths, so the shortfall is the least precisely estimated number here"
            ),
        ])
        .build()
    }
}

/// One bootstrapped cumulative log return over `steps` observations.
///
/// Blocks are geometric in length with mean [`MEAN_BLOCK`], which makes the
/// resampled series stationary — a fixed block length introduces a periodicity
/// the original data does not have.
fn bootstrap_path(ctx: &mut AgentContext, returns: &[f64], steps: usize) -> f64 {
    let n = returns.len();
    let continue_probability = 1.0 - 1.0 / MEAN_BLOCK;
    let mut total = 0.0;
    let mut index = ctx.rng().below(n as u64) as usize;
    for _ in 0..steps {
        total += returns[index];
        if ctx.rng().next_f64() < continue_probability {
            // Wrap rather than stop, so every observation is equally likely to
            // appear at every position in the path.
            index = (index + 1) % n;
        } else {
            index = ctx.rng().below(n as u64) as usize;
        }
    }
    total.exp() - 1.0
}
