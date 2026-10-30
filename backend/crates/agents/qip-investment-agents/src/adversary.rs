//! The adversary probes (R12-REASON-3023): the two questions the Adversarial
//! Brain asks that no other agent asks.
//!
//! The adversarial reviewer attacks a hypothesis and the poisoning detector
//! attacks the evidence it rests on. Neither asks whether somebody is trading
//! against the platform itself, or whether the market the models were fitted
//! on is still the market. Until these two existed the organisation's scope
//! had no probe for either, and a blueprint row that said the Adversarial
//! Brain covered "regime breaks" and "strategies that exploit Algorik itself"
//! was describing coverage nothing produced.
//!
//! Both are deterministic arithmetic over facilities the desk holds, like the
//! poisoning detector: a probe whose verdict depended on a model's reading
//! of the tape would be a finding nobody could reproduce. Both report
//! `Ambiguous` rather than `Negative` when they find something, for the
//! reason `poisoning.rs` gives — a warning about the platform's own fills or
//! about the tape says nothing about which way the instrument in the brief
//! will move, and a negative finding would tilt every thesis the same way.

use crate::desk::{Desk, ExecutionRecord};
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use std::collections::BTreeMap;
use std::sync::Arc;

// --- self-exploitation ------------------------------------------------------

/// What counts as the platform being exploited, as numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct SelfExploitationThresholds {
    /// Fewer fills than this and the adverse-share test is not run: eight
    /// adverse fills out of eight is a bad week, not a counterparty.
    pub minimum_fills: usize,
    /// The share of fills worse than arrival at or above which the flow is
    /// read as being traded against.
    pub adverse_share: f64,
    /// An (instrument, side, quantity) triple repeated this many times is a
    /// flow a counterparty can learn.
    pub repeated_pattern: usize,
}

impl Default for SelfExploitationThresholds {
    fn default() -> Self {
        Self {
            minimum_fills: 10,
            adverse_share: 0.8,
            repeated_pattern: 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfExploitationKind {
    /// Fills are worse than arrival far more often than chance.
    AdverseSlippage,
    /// The same order keeps going out: a pattern to front-run.
    LearnableFlow,
}

impl SelfExploitationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdverseSlippage => "adverse_slippage",
            Self::LearnableFlow => "learnable_flow",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelfExploitationFinding {
    pub kind: SelfExploitationKind,
    pub evidence: String,
}

/// Fraction of `executions` whose fill was worse than arrival, and how many
/// had a measurable slippage at all.
fn adverse_share(executions: &[ExecutionRecord]) -> (usize, f64) {
    let slippages: Vec<f64> = executions
        .iter()
        .filter_map(ExecutionRecord::adverse_slippage_bp)
        .collect();
    if slippages.is_empty() {
        return (0, 0.0);
    }
    let adverse = slippages.iter().filter(|bp| **bp > 0.0).count();
    (slippages.len(), adverse as f64 / slippages.len() as f64)
}

/// The probe over the platform's own executions. Pure, so it can be judged
/// on a fixture without an organisation around it.
pub fn probe_self_exploitation(
    executions: &[ExecutionRecord],
    thresholds: &SelfExploitationThresholds,
) -> Vec<SelfExploitationFinding> {
    let mut findings = Vec::new();
    let (measured, share) = adverse_share(executions);
    if measured >= thresholds.minimum_fills && share >= thresholds.adverse_share {
        findings.push(SelfExploitationFinding {
            kind: SelfExploitationKind::AdverseSlippage,
            evidence: format!(
                "{:.0}% of {measured} measurable fill(s) were worse than arrival, against a \
                 threshold of {:.0}%",
                share * 100.0,
                thresholds.adverse_share * 100.0
            ),
        });
    }
    let mut patterns: BTreeMap<(String, &'static str, String), usize> = BTreeMap::new();
    for execution in executions {
        *patterns
            .entry((
                execution.object_id.clone(),
                execution.side.as_str(),
                execution.quantity.to_string(),
            ))
            .or_insert(0) += 1;
    }
    for ((object, side, quantity), count) in patterns {
        if count >= thresholds.repeated_pattern {
            findings.push(SelfExploitationFinding {
                kind: SelfExploitationKind::LearnableFlow,
                evidence: format!(
                    "{side} {quantity} {object} went out {count} time(s), a flow regular enough \
                     to be learned"
                ),
            });
        }
    }
    findings
}

/// Asks whether somebody is trading against the platform.
#[derive(Debug)]
pub struct SelfExploitationProbe {
    manifest: AgentManifest,
    desk: Arc<Desk>,
    thresholds: SelfExploitationThresholds,
}

impl SelfExploitationProbe {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self {
            manifest,
            desk,
            thresholds: SelfExploitationThresholds::default(),
        }
    }

    pub fn with_thresholds(mut self, thresholds: SelfExploitationThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }
}

impl Agent for SelfExploitationProbe {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let view = self.desk.executions.get(ctx)?;
        if view.executions.is_empty() {
            // Nothing examined is not nothing found.
            return Ok(no_data(
                ctx,
                brief.as_of,
                "the desk holds no executions, so there is no flow to probe for exploitation",
            ));
        }
        let findings = probe_self_exploitation(&view.executions, &self.thresholds);
        let (measured, share) = adverse_share(&view.executions);
        let (direction, conviction, claim) = if findings.is_empty() {
            (
                Direction::Neutral,
                0.0,
                format!(
                    "{} execution(s) probed; no pattern of the platform being traded against",
                    view.executions.len()
                ),
            )
        } else {
            (
                Direction::Ambiguous,
                1.0,
                format!(
                    "{} self-exploitation finding(s) over {} execution(s): {}",
                    findings.len(),
                    view.executions.len(),
                    findings
                        .iter()
                        .map(|finding| finding.kind.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        };
        let evidence: Vec<String> = findings
            .iter()
            .map(|finding| {
                format!(
                    "exploitation:{}: {}",
                    finding.kind.as_str(),
                    finding.evidence
                )
            })
            .collect();
        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(direction, conviction)
            .fact(computed(
                ctx,
                "fills_scanned",
                view.executions.len() as f64,
                "count",
                &["execution_history"],
            ))
            .fact(computed(
                ctx,
                "adverse_fill_share",
                share,
                "fraction",
                &["execution_history"],
            ))
            .fact(computed(
                ctx,
                "self_exploitation_findings",
                findings.len() as f64,
                "count",
                &["execution_history"],
            ))
            .caveats(vec![format!(
                "{measured} of {} execution(s) carried a measurable arrival price; the rest are \
                 counted but not judged",
                view.executions.len()
            )]);
        if !evidence.is_empty() {
            builder = builder.evidence(evidence).falsifiers(vec![
                "the next window's fills land on both sides of arrival and the repeated flow \
                 stops repeating"
                    .to_string(),
            ]);
        }
        builder.build()
    }
}

// --- regime break -----------------------------------------------------------

/// What counts as a regime break, as numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct RegimeBreakThresholds {
    /// Returns in the recent window.
    pub recent: usize,
    /// Returns in the prior window the recent one is judged against.
    pub prior: usize,
    /// Recent-to-prior volatility ratio at or above which the regime broke.
    pub volatility_ratio: f64,
    /// Mean shift of the recent window, in prior-window standard errors, at
    /// or above which the regime broke.
    pub mean_shift_z: f64,
}

impl Default for RegimeBreakThresholds {
    fn default() -> Self {
        Self {
            recent: 10,
            prior: 30,
            volatility_ratio: 2.0,
            mean_shift_z: 3.0,
        }
    }
}

impl RegimeBreakThresholds {
    /// Closes needed to form both windows: one more than the returns.
    pub fn minimum_closes(&self) -> usize {
        self.recent + self.prior + 1
    }
}

/// The two statistics of one instrument's recent window against its prior.
#[derive(Clone, Debug, PartialEq)]
pub struct RegimeComparison {
    pub volatility_ratio: f64,
    pub mean_shift_z: f64,
}

impl RegimeComparison {
    pub fn is_break(&self, thresholds: &RegimeBreakThresholds) -> bool {
        self.volatility_ratio >= thresholds.volatility_ratio
            || self.mean_shift_z.abs() >= thresholds.mean_shift_z
    }
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn standard_deviation(values: &[f64]) -> f64 {
    let m = mean(values);
    (values.iter().map(|v| (v - m).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
}

/// Compare the last `recent` returns of `closes` against the `prior` before
/// them. `None` when the tape is too short, or the prior window is flat so
/// that no band exists to leave.
pub fn compare_regimes(
    closes: &[f64],
    thresholds: &RegimeBreakThresholds,
) -> Option<RegimeComparison> {
    if closes.len() < thresholds.minimum_closes() || thresholds.recent == 0 {
        return None;
    }
    let returns: Vec<f64> = closes
        .windows(2)
        .filter(|pair| pair[0].is_finite() && pair[0] > 0.0 && pair[1].is_finite())
        .map(|pair| pair[1] / pair[0] - 1.0)
        .collect();
    if returns.len() < thresholds.recent + thresholds.prior {
        return None;
    }
    let recent = &returns[returns.len() - thresholds.recent..];
    let prior = &returns
        [returns.len() - thresholds.recent - thresholds.prior..returns.len() - thresholds.recent];
    let prior_sd = standard_deviation(prior);
    if !(prior_sd.is_finite() && prior_sd > 0.0) {
        return None;
    }
    let volatility_ratio = standard_deviation(recent) / prior_sd;
    let mean_shift_z =
        (mean(recent) - mean(prior)) / (prior_sd / (thresholds.recent as f64).sqrt());
    (volatility_ratio.is_finite() && mean_shift_z.is_finite()).then_some(RegimeComparison {
        volatility_ratio,
        mean_shift_z,
    })
}

/// Asks whether the market the models were fitted on is still the market.
#[derive(Debug)]
pub struct RegimeBreakProbe {
    manifest: AgentManifest,
    desk: Arc<Desk>,
    thresholds: RegimeBreakThresholds,
}

impl RegimeBreakProbe {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self {
            manifest,
            desk,
            thresholds: RegimeBreakThresholds::default(),
        }
    }

    pub fn with_thresholds(mut self, thresholds: RegimeBreakThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }
}

impl Agent for RegimeBreakProbe {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let market = self.desk.market.get(ctx)?;
        let mut scanned = 0usize;
        let mut breaks: Vec<(String, RegimeComparison)> = Vec::new();
        let mut max_ratio = 0.0_f64;
        for (instrument, state) in market.snapshot.instruments() {
            let Some(comparison) = compare_regimes(&state.bars.closes(), &self.thresholds) else {
                continue;
            };
            scanned += 1;
            max_ratio = max_ratio.max(comparison.volatility_ratio);
            if comparison.is_break(&self.thresholds) {
                breaks.push((instrument.to_string(), comparison));
            }
        }
        if scanned == 0 {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "no instrument on the desk holds the {} closes two windows need, so no regime \
                     was compared",
                    self.thresholds.minimum_closes()
                ),
            ));
        }
        let (direction, conviction, claim) = if breaks.is_empty() {
            (
                Direction::Neutral,
                0.0,
                format!("{scanned} instrument(s) compared; no regime break in the recent window"),
            )
        } else {
            (
                Direction::Ambiguous,
                1.0,
                format!(
                    "{} regime break(s) in {scanned} instrument(s): {}",
                    breaks.len(),
                    breaks
                        .iter()
                        .map(|(instrument, _)| instrument.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        };
        let evidence: Vec<String> = breaks
            .iter()
            .map(|(instrument, comparison)| {
                format!(
                    "regime:{instrument}: volatility ratio {:.2}, mean shift {:.2} standard errors",
                    comparison.volatility_ratio, comparison.mean_shift_z
                )
            })
            .collect();
        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(direction, conviction)
            .fact(computed(
                ctx,
                "instruments_scanned",
                scanned as f64,
                "count",
                &["market_snapshot"],
            ))
            .fact(computed(
                ctx,
                "regime_break_findings",
                breaks.len() as f64,
                "count",
                &["market_snapshot"],
            ))
            .fact(computed(
                ctx,
                "max_volatility_ratio",
                max_ratio,
                "ratio",
                &["market_snapshot"],
            ))
            .caveats(vec![
                "names a break in the tape, not its cause; a scheduled event in the recent window \
                 reads the same as a structural one"
                    .to_string(),
            ]);
        if !evidence.is_empty() {
            builder = builder.evidence(evidence).falsifiers(vec![
                "the next recent window's volatility and mean return to the prior window's band"
                    .to_string(),
            ]);
        }
        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desk::TradeSide;
    use qip_core::{Decimal, Timestamp, dec};

    fn execution(
        n: usize,
        side: TradeSide,
        quantity: &str,
        arrival: &str,
        fill: &str,
    ) -> ExecutionRecord {
        ExecutionRecord {
            order_id: format!("ord-{n}"),
            object_id: "ACME".into(),
            side,
            quantity: Decimal::parse(quantity).unwrap(),
            arrival_price: Decimal::parse(arrival).unwrap(),
            fill_price: Decimal::parse(fill).unwrap(),
            placed_at: Timestamp::from_secs(1_700_000_000 + n as i64 * 60),
            filled_at: Timestamp::from_secs(1_700_000_030 + n as i64 * 60),
            venue: "XSIM".into(),
        }
    }

    /// Mutation: change `**bp > 0.0` in `adverse_share` to `>= 0.0` — the
    /// balanced flow's zero-slippage fills count as adverse and the second
    /// assertion fails.
    #[test]
    fn fills_systematically_worse_than_arrival_are_a_finding_and_a_balanced_flow_is_not() {
        let thresholds = SelfExploitationThresholds::default();
        // Premise: twelve buys, every one filled above arrival.
        let bled: Vec<ExecutionRecord> = (0..12)
            .map(|n| execution(n, TradeSide::Buy, &format!("{}", 100 + n), "100", "100.08"))
            .collect();
        let findings = probe_self_exploitation(&bled, &thresholds);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].kind, SelfExploitationKind::AdverseSlippage);
        // Balanced: half better, half at arrival, distinct sizes. Nothing.
        let balanced: Vec<ExecutionRecord> = (0..12)
            .map(|n| {
                let fill = if n % 2 == 0 { "99.95" } else { "100" };
                execution(n, TradeSide::Sell, &format!("{}", 200 + n), "100", fill)
            })
            .collect();
        assert!(probe_self_exploitation(&balanced, &thresholds).is_empty());
        // Under the minimum, even all-adverse is not judged.
        assert!(probe_self_exploitation(&bled[..9], &thresholds).is_empty());
    }

    /// Mutation: change `count >= thresholds.repeated_pattern` to `>` — the
    /// three identical sells are no longer a finding and the assertion fails.
    #[test]
    fn the_same_order_three_times_is_a_learnable_flow() {
        let flow = vec![
            execution(0, TradeSide::Sell, "500", "100", "99.99"),
            execution(1, TradeSide::Sell, "500", "100", "100.01"),
            execution(2, TradeSide::Buy, "500", "100", "100.00"),
            execution(3, TradeSide::Sell, "500", "100", "100.00"),
        ];
        let findings = probe_self_exploitation(&flow, &SelfExploitationThresholds::default());
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].kind, SelfExploitationKind::LearnableFlow);
        assert!(
            findings[0]
                .evidence
                .starts_with("sell 500 ACME went out 3 time(s)")
        );
    }

    fn tape(prior_amplitude: f64, recent_amplitude: f64, recent_drift: f64) -> Vec<f64> {
        let mut closes = vec![100.0];
        for i in 0..40 {
            let wobble = if i % 2 == 0 { 1.0 } else { -1.0 };
            let (amplitude, drift) = if i < 30 {
                (prior_amplitude, 0.0)
            } else {
                (recent_amplitude, recent_drift)
            };
            let last = *closes.last().unwrap();
            closes.push(last * (1.0 + wobble * amplitude + drift));
        }
        closes
    }

    /// Mutation: change `>=` to `>` in `RegimeComparison::is_break` for the
    /// ratio and set the fixture ratio to exactly the threshold — not done
    /// here; the one run: replace `standard_deviation(recent) / prior_sd`
    /// with `prior_sd / prior_sd` (ratio always one) — the volatility break
    /// is missed and the first assertion fails.
    #[test]
    fn a_recent_window_whose_volatility_or_mean_leaves_the_prior_band_is_a_break() {
        let thresholds = RegimeBreakThresholds::default();
        // Volatility doubled and more in the last ten bars.
        let louder = compare_regimes(&tape(0.005, 0.02, 0.0), &thresholds).expect("compared");
        assert!(louder.volatility_ratio >= 3.0, "{louder:?}");
        assert!(louder.is_break(&thresholds), "{louder:?}");
        // Same amplitude, but every recent bar drifts up: a mean shift.
        let drifted = compare_regimes(&tape(0.005, 0.005, 0.01), &thresholds).expect("compared");
        assert!(drifted.volatility_ratio < 1.5, "{drifted:?}");
        assert!(drifted.mean_shift_z >= 3.0, "{drifted:?}");
        assert!(drifted.is_break(&thresholds));
        // Unchanged: no break. And a tape one close short is not compared.
        let steady = compare_regimes(&tape(0.005, 0.005, 0.0), &thresholds).expect("compared");
        assert!(!steady.is_break(&thresholds), "{steady:?}");
        assert!(compare_regimes(&tape(0.005, 0.005, 0.0)[1..], &thresholds).is_none());
        let _ = dec!("1");
    }
}
