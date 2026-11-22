//! The adversary probes (R12-REASON-3023): the four questions the Adversarial
//! Brain asks that no other agent asks.
//!
//! The adversarial reviewer attacks a hypothesis and the poisoning detector
//! attacks the evidence it rests on. Neither asks whether somebody is trading
//! against the platform itself, whether the market the models were fitted on
//! is still the market, whether the book or the tape is being worked, or
//! whether the models behind a claim are failing. Until these four existed
//! the organisation's scope had no probe for any of them, and a blueprint row
//! that said the Adversarial Brain covered "manipulation", "regime breaks",
//! "model failure" and "strategies that exploit Algorik itself" was
//! describing coverage nothing produced.
//!
//! All four are deterministic arithmetic over facilities the desk holds, like
//! the poisoning detector: a probe whose verdict depended on a model's reading
//! of the tape would be a finding nobody could reproduce. All four report
//! `Ambiguous` rather than `Negative` when they find something, for the
//! reason `poisoning.rs` gives — a warning about the platform's own fills,
//! the tape, the book or the models says nothing about which way the
//! instrument in the brief will move, and a negative finding would tilt
//! every thesis the same way.

use crate::desk::{Desk, ExecutionRecord, ForecasterHealth, ModelsView};
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

// --- market manipulation ----------------------------------------------------

/// What counts as a worked book or tape, as numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct ManipulationThresholds {
    /// Depth stacked behind the touch on one side, as a multiple of the
    /// touch's own size, at or above which the book reads as layered.
    pub layering_multiple: f64,
    /// Book levels the layering check reads past the touch.
    pub layering_levels: usize,
    /// The last bar's volume as a multiple of the median of the bars before
    /// it, at or above which, with a flat close, the volume reads as wash.
    pub wash_volume_multiple: f64,
    /// A close-to-close move inside this, in fraction, is "no price".
    pub flat_return: f64,
    /// Bars needed before a bar's volume has a median to be judged against.
    pub minimum_bars: usize,
}

impl Default for ManipulationThresholds {
    fn default() -> Self {
        Self {
            layering_multiple: 20.0,
            layering_levels: 5,
            wash_volume_multiple: 5.0,
            flat_return: 0.0005,
            minimum_bars: 11,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManipulationKind {
    /// Bid at or above the ask: a book somebody is holding in a state the
    /// venue should not let stand.
    CrossedBook,
    /// Depth stacked behind a thin touch on one side: the shape layering
    /// and spoofing leave while the orders rest.
    LayeredBook,
    /// Volume far above the tape's own median on a close that did not
    /// move: the shape wash trading leaves.
    WashVolume,
}

impl ManipulationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CrossedBook => "crossed_book",
            Self::LayeredBook => "layered_book",
            Self::WashVolume => "wash_volume",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ManipulationFinding {
    pub instrument: String,
    pub kind: ManipulationKind,
    pub evidence: String,
}

/// The shapes one instrument's book shows, if any.
pub fn probe_book(
    instrument: &str,
    book: &qip_market::book::OrderBook,
    thresholds: &ManipulationThresholds,
) -> Vec<ManipulationFinding> {
    use qip_market::book::Side;
    let mut findings = Vec::new();
    if book.is_crossed() {
        findings.push(ManipulationFinding {
            instrument: instrument.to_string(),
            kind: ManipulationKind::CrossedBook,
            evidence: format!(
                "bid {:?} at or above ask {:?}",
                book.best_bid().map(|l| l.price),
                book.best_ask().map(|l| l.price)
            ),
        });
    }
    for (side, touch) in [(Side::Buy, book.best_bid()), (Side::Sell, book.best_ask())] {
        let Some(touch) = touch else { continue };
        // Sizes cross from `Decimal` to `f64` here: a layering multiple is a
        // ratio, a statistic, not money.
        let touch_size = touch.size.to_f64();
        if !(touch_size.is_finite() && touch_size > 0.0) {
            continue;
        }
        let behind = book.depth(side, thresholds.layering_levels).to_f64() - touch_size;
        let multiple = behind / touch_size;
        if multiple.is_finite() && multiple >= thresholds.layering_multiple {
            findings.push(ManipulationFinding {
                instrument: instrument.to_string(),
                kind: ManipulationKind::LayeredBook,
                evidence: format!(
                    "{} side holds {multiple:.0}x the touch's size behind it within {} level(s)",
                    match side {
                        Side::Buy => "bid",
                        Side::Sell => "ask",
                    },
                    thresholds.layering_levels
                ),
            });
        }
    }
    findings
}

/// The wash shape in one instrument's bars, if the tape is long enough.
/// `None` when it is not, so the caller can tell unexamined from clean.
pub fn probe_tape(
    instrument: &str,
    bars: &[qip_market::bar::Bar],
    thresholds: &ManipulationThresholds,
) -> Option<Option<ManipulationFinding>> {
    if bars.len() < thresholds.minimum_bars.max(2) {
        return None;
    }
    let (last, prior) = bars.split_last()?;
    // Volumes and closes cross from `Decimal` to `f64` here, once: a volume
    // multiple and a return are ratios, statistics, not money.
    let mut volumes: Vec<f64> = prior.iter().map(|bar| bar.volume.to_f64()).collect();
    volumes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = volumes[volumes.len() / 2];
    if !(median.is_finite() && median > 0.0) {
        return None;
    }
    let multiple = last.volume.to_f64() / median;
    let previous_close = prior.last()?.close.to_f64();
    if !(previous_close.is_finite() && previous_close > 0.0) {
        return None;
    }
    let moved = (last.close.to_f64() / previous_close - 1.0).abs();
    let wash = multiple >= thresholds.wash_volume_multiple && moved <= thresholds.flat_return;
    Some(wash.then(|| ManipulationFinding {
        instrument: instrument.to_string(),
        kind: ManipulationKind::WashVolume,
        evidence: format!(
            "last bar's volume is {multiple:.1}x the tape's median on a {:.2} bp move",
            moved * 10_000.0
        ),
    }))
}

/// Asks whether the tape or the book is being worked.
#[derive(Debug)]
pub struct ManipulationProbe {
    manifest: AgentManifest,
    desk: Arc<Desk>,
    thresholds: ManipulationThresholds,
}

impl ManipulationProbe {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self {
            manifest,
            desk,
            thresholds: ManipulationThresholds::default(),
        }
    }

    pub fn with_thresholds(mut self, thresholds: ManipulationThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }
}

impl Agent for ManipulationProbe {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let market = self.desk.market.get(ctx)?;
        let mut examined = 0usize;
        let mut findings: Vec<ManipulationFinding> = Vec::new();
        for (instrument, state) in market.snapshot.instruments() {
            let mut looked = false;
            if let Some(book) = &state.book {
                looked = true;
                findings.extend(probe_book(instrument, book, &self.thresholds));
            }
            if let Some(tape) = probe_tape(instrument, state.bars.bars(), &self.thresholds) {
                looked = true;
                findings.extend(tape);
            }
            if looked {
                examined += 1;
            }
        }
        if examined == 0 {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "no instrument on the desk holds a book or enough bars to examine for manipulation",
            ));
        }
        let (direction, conviction, claim) = if findings.is_empty() {
            (
                Direction::Neutral,
                0.0,
                format!(
                    "{examined} instrument(s) examined; no manipulation shape in the book or the tape"
                ),
            )
        } else {
            (
                Direction::Ambiguous,
                1.0,
                format!(
                    "{} manipulation finding(s) over {examined} instrument(s): {}",
                    findings.len(),
                    findings
                        .iter()
                        .map(|f| format!("{}:{}", f.instrument, f.kind.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        };
        let evidence: Vec<String> = findings
            .iter()
            .map(|f| {
                format!(
                    "manipulation:{}:{}: {}",
                    f.kind.as_str(),
                    f.instrument,
                    f.evidence
                )
            })
            .collect();
        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(direction, conviction)
            .fact(computed(
                ctx,
                "instruments_examined",
                examined as f64,
                "count",
                &["market_snapshot"],
            ))
            .fact(computed(
                ctx,
                "manipulation_findings",
                findings.len() as f64,
                "count",
                &["market_snapshot"],
            ))
            .caveats(vec![
                "names the shape, not the actor; a book snapshot cannot show an order being pulled"
                    .to_string(),
            ]);
        if !evidence.is_empty() {
            builder = builder.evidence(evidence).falsifiers(vec![
                "the next snapshot's book is unlayered and uncrossed and the volume spike carried a move"
                    .to_string(),
            ]);
        }
        builder.build()
    }
}

// --- model failure ----------------------------------------------------------

/// What counts as a failing model, as numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelFailureThresholds {
    /// Mean Brier score at or above which a binary forecaster is worse than
    /// a coin: 0.25 is the coin's own score.
    pub brier_worse_than_chance: f64,
    /// The meta-model's predicted failure probability at or above which the
    /// forecaster is read as failing now.
    pub predicted_failure: f64,
}

impl Default for ModelFailureThresholds {
    fn default() -> Self {
        Self {
            brier_worse_than_chance: 0.25,
            predicted_failure: 0.6,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFailureKind {
    /// Scored worse than chance.
    WorseThanChance,
    /// The meta-model expects it to fail in this regime.
    PredictedToFail,
    /// A production model the deep brain reported past its drift threshold.
    Drifted,
}

impl ModelFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorseThanChance => "worse_than_chance",
            Self::PredictedToFail => "predicted_to_fail",
            Self::Drifted => "drifted",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelFailureFinding {
    pub model: String,
    pub kind: ModelFailureKind,
    pub evidence: String,
}

/// The failing models in `view`. Pure, so it can be judged on a fixture.
pub fn probe_models(
    view: &ModelsView,
    thresholds: &ModelFailureThresholds,
) -> Vec<ModelFailureFinding> {
    let mut findings = Vec::new();
    for ForecasterHealth {
        name,
        mean_brier,
        predicted_failure,
    } in &view.forecasters
    {
        if let Some(brier) = mean_brier
            && *brier >= thresholds.brier_worse_than_chance
        {
            findings.push(ModelFailureFinding {
                model: name.clone(),
                kind: ModelFailureKind::WorseThanChance,
                evidence: format!(
                    "mean Brier {brier:.3} against a coin's {:.2}",
                    thresholds.brier_worse_than_chance
                ),
            });
        }
        if *predicted_failure >= thresholds.predicted_failure {
            findings.push(ModelFailureFinding {
                model: name.clone(),
                kind: ModelFailureKind::PredictedToFail,
                evidence: format!(
                    "the meta-model puts its failure at {:.0}% in this regime",
                    predicted_failure * 100.0
                ),
            });
        }
    }
    for reference in &view.drifted {
        findings.push(ModelFailureFinding {
            model: reference.clone(),
            kind: ModelFailureKind::Drifted,
            evidence: "past its own drift threshold on the deep brain's last report".to_string(),
        });
    }
    findings
}

/// Asks whether the models behind a claim are failing.
#[derive(Debug)]
pub struct ModelFailureProbe {
    manifest: AgentManifest,
    desk: Arc<Desk>,
    thresholds: ModelFailureThresholds,
}

impl ModelFailureProbe {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self {
            manifest,
            desk,
            thresholds: ModelFailureThresholds::default(),
        }
    }

    pub fn with_thresholds(mut self, thresholds: ModelFailureThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }
}

impl Agent for ModelFailureProbe {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let view = self.desk.models.get(ctx)?;
        let examined = view.forecasters.len() + view.drifted.len();
        if examined == 0 {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "the desk holds no scored forecaster and no drift report, so no model was examined",
            ));
        }
        let findings = probe_models(&view, &self.thresholds);
        let (direction, conviction, claim) = if findings.is_empty() {
            (
                Direction::Neutral,
                0.0,
                format!("{examined} model(s) examined; none failing or drifted"),
            )
        } else {
            (
                Direction::Ambiguous,
                1.0,
                format!(
                    "{} model failure finding(s) over {examined} model(s): {}",
                    findings.len(),
                    findings
                        .iter()
                        .map(|f| format!("{}:{}", f.model, f.kind.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        };
        let evidence: Vec<String> = findings
            .iter()
            .map(|f| format!("model:{}:{}: {}", f.kind.as_str(), f.model, f.evidence))
            .collect();
        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(direction, conviction)
            .fact(computed(
                ctx,
                "models_examined",
                examined as f64,
                "count",
                &["model_registry"],
            ))
            .fact(computed(
                ctx,
                "model_failure_findings",
                findings.len() as f64,
                "count",
                &["model_registry"],
            ))
            .caveats(vec![
                "sees only forecasters the tournament holds a question from and the deep brain's last drift report"
                    .to_string(),
            ]);
        if !evidence.is_empty() {
            builder = builder.evidence(evidence).falsifiers(vec![
                "the forecaster's next scored questions bring its Brier under chance and the drift report clears"
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

    fn level(price: &str, size: &str) -> qip_market::book::BookLevel {
        qip_market::book::BookLevel::new(
            Decimal::parse(price).unwrap(),
            Decimal::parse(size).unwrap(),
        )
    }

    /// Mutation: change `multiple >= thresholds.layering_multiple` to `>` and
    /// set the fixture to exactly twenty times — not run; the one run:
    /// replace `behind / touch_size` with `touch_size / touch_size` (always
    /// one) — the layered book is missed and the first assertion fails.
    #[test]
    fn a_layered_or_crossed_book_is_a_manipulation_shape_and_a_plain_one_is_not() {
        use qip_market::book::OrderBook;
        let at = Timestamp::from_secs(1_760_000_000);
        let thresholds = ManipulationThresholds::default();
        let layered = OrderBook::from_levels(
            qip_core::ObjectId::from_string("ACME"),
            "XNYS",
            at,
            vec![
                level("99.95", "100"),
                level("99.90", "1500"),
                level("99.85", "1500"),
            ],
            vec![level("100.05", "100"), level("100.10", "120")],
        );
        let found = probe_book("ACME", &layered, &thresholds);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].kind, ManipulationKind::LayeredBook);
        let plain = OrderBook::from_levels(
            qip_core::ObjectId::from_string("ACME"),
            "XNYS",
            at,
            vec![
                level("99.95", "5000"),
                level("99.90", "8000"),
                level("99.85", "12000"),
            ],
            vec![
                level("100.05", "5000"),
                level("100.10", "8000"),
                level("100.15", "12000"),
            ],
        );
        assert!(probe_book("ACME", &plain, &thresholds).is_empty());
        let crossed = OrderBook::from_levels(
            qip_core::ObjectId::from_string("ACME"),
            "XNYS",
            at,
            vec![level("100.10", "500")],
            vec![level("100.05", "500")],
        );
        assert!(
            probe_book("ACME", &crossed, &thresholds)
                .iter()
                .any(|f| f.kind == ManipulationKind::CrossedBook)
        );
    }

    fn bar(n: i64, close: f64, volume: &str) -> qip_market::bar::Bar {
        let price = Decimal::from_f64(close).unwrap();
        qip_market::bar::Bar {
            object_id: qip_core::ObjectId::from_string("ACME"),
            venue: "XNYS".to_string(),
            interval: qip_market::bar::Interval::Day,
            open_time: Timestamp::from_secs(1_760_000_000 + n * 86_400),
            open: price,
            high: price,
            low: price,
            close: price,
            volume: Decimal::parse(volume).unwrap(),
            trade_count: 100,
            vwap: Some(price),
            quality: qip_financial::quality::DataQuality::default(),
        }
    }

    /// Mutation: replace `moved <= thresholds.flat_return` with `true` — the
    /// spike that carried a move is read as wash and the second assertion
    /// fails.
    #[test]
    fn volume_on_a_flat_close_is_wash_and_volume_that_moved_the_price_is_not() {
        let thresholds = ManipulationThresholds::default();
        let mut flat: Vec<_> = (0..12).map(|n| bar(n, 100.0, "1000")).collect();
        flat.push(bar(12, 100.01, "9000"));
        let finding = probe_tape("ACME", &flat, &thresholds)
            .expect("examined")
            .expect("a nine-fold spike on a one-basis-point move is wash");
        assert_eq!(finding.kind, ManipulationKind::WashVolume);
        let mut moved: Vec<_> = (0..12).map(|n| bar(n, 100.0, "1000")).collect();
        moved.push(bar(12, 101.5, "9000"));
        assert_eq!(probe_tape("ACME", &moved, &thresholds), Some(None));
        assert_eq!(
            probe_tape("ACME", &flat[..5], &thresholds),
            None,
            "too short to judge"
        );
    }

    /// Mutation: change `*brier >= thresholds.brier_worse_than_chance` to
    /// `>` with the fixture at exactly 0.25 — not run; the one run: drop the
    /// `drifted` loop in `probe_models` — the drifted reference is missed
    /// and the count assertion fails.
    #[test]
    fn a_forecaster_worse_than_chance_or_expected_to_fail_and_a_drifted_model_are_findings() {
        let view = ModelsView {
            forecasters: vec![
                ForecasterHealth {
                    name: "detector:momentum".into(),
                    mean_brier: Some(0.31),
                    predicted_failure: 0.2,
                },
                ForecasterHealth {
                    name: "detector:reversion".into(),
                    mean_brier: Some(0.12),
                    predicted_failure: 0.75,
                },
                ForecasterHealth {
                    name: "detector:carry".into(),
                    mean_brier: Some(0.18),
                    predicted_failure: 0.1,
                },
                ForecasterHealth {
                    name: "detector:unscored".into(),
                    mean_brier: None,
                    predicted_failure: 0.0,
                },
            ],
            drifted: vec!["ridge-ACME@3".into()],
        };
        let findings = probe_models(&view, &ModelFailureThresholds::default());
        let kinds: Vec<(&str, ModelFailureKind)> = findings
            .iter()
            .map(|f| (f.model.as_str(), f.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("detector:momentum", ModelFailureKind::WorseThanChance),
                ("detector:reversion", ModelFailureKind::PredictedToFail),
                ("ridge-ACME@3", ModelFailureKind::Drifted),
            ],
            "{findings:?}"
        );
    }
}
