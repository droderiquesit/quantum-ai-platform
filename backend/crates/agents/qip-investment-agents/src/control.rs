//! The control functions.
//!
//! Two agents hold the veto: [`RiskControl`] and [`ComplianceControl`]. Both
//! decide by arithmetic against declared rules, and neither can form an
//! investment view or move the limit it is checking against.
//!
//! Compliance has no language model at all. A compliance decision that depends
//! on how a sentence was phrased is not a compliance decision, and the manifest
//! withholds the capability so the question cannot arise.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use std::sync::Arc;

/// Standard deviations in the generated stress day.
const STRESS_SIGMAS: f64 = 3.0;

/// Trading days a year, to take annualised volatility to one day.
const TRADING_DAYS: f64 = 252.0;

/// Limit kinds whose measured value scales linearly with a proportional cut
/// in gross exposure, by `LimitKind::label`.
const SCALES_WITH_GROSS: [&str; 5] = [
    "max_leverage",
    "max_volatility",
    "max_value_at_risk",
    "max_expected_shortfall",
    "max_survival_gross",
];

/// Evaluates the book against its limits.
#[derive(Debug)]
pub struct RiskControl {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl RiskControl {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for RiskControl {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let risk = self.desk.risk.get(ctx)?;
        let check = risk.limits.check(&risk.state);

        let blocking = check.blocking();
        let warnings = check.warnings();

        // A veto is a statement of fact about a limit, so it is reported with
        // full conviction. A warning is a statement that a limit is close,
        // which is information rather than a decision.
        //
        // **The reduction question is asked first, and the order is the whole
        // control.** `LimitCheck::requires_reduction` is
        // `any(|b| b.blocks() && b.forces_reduction)`, which implies
        // `is_blocked` — `any(LimitBreach::blocks)` — so while `is_blocked`
        // was tested first this arm could never be taken. That made every
        // `Limit::forcing_reduction()` mark in the platform inert: the
        // shipped `leverage`, `drawdown` and `daily-loss` limits all carry it,
        // this is the only production caller of `requires_reduction`, and a
        // desk reading a finding could not tell "stop taking risk" from
        // "unwind what you are holding".
        //
        // Both arms carry full conviction. The reduction arm used to carry
        // 0.8, which was defensible only while it sat *below* the block arm
        // and could be read as a weaker finding; a breach that forces a
        // reduction is a stricter fact than one that merely blocks, and
        // reporting it less confidently would invert the severity the desk
        // asked for.
        let (direction, conviction, claim) = if check.requires_reduction() {
            (
                Direction::Negative,
                1.0,
                format!(
                    "{} limit(s) block and {} of them require the book to be reduced: {}",
                    blocking.len(),
                    check.forcing_reduction().len(),
                    check.reason()
                ),
            )
        } else if check.is_blocked() {
            (
                Direction::Negative,
                1.0,
                format!("{} limit(s) block: {}", blocking.len(), check.reason()),
            )
        } else if !warnings.is_empty() {
            (
                Direction::Neutral,
                0.0,
                format!(
                    "no limit is breached; {} within warning distance",
                    warnings.len()
                ),
            )
        } else {
            (
                Direction::Neutral,
                0.0,
                "no limit is breached or approached".to_string(),
            )
        };

        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(direction, conviction)
            .fact(computed(
                ctx,
                "blocking_breach_count",
                blocking.len() as f64,
                "count",
                &["risk_state", "limit_set"],
            ))
            .fact(computed(
                ctx,
                "warning_count",
                warnings.len() as f64,
                "count",
                &["risk_state", "limit_set"],
            ))
            // Gross exposure and drawdown are read from the risk state, but
            // `RiskState` is the risk engine's aggregate of the book, not a
            // record from a feed: it carries no as-of instant and no record
            // id, so an observed stamp would have to invent both. They stay
            // computed, from the state, until the state can say when it was
            // valued.
            .fact(computed(
                ctx,
                "gross_exposure",
                risk.state.gross_exposure.to_f64(),
                "currency",
                &["risk_state"],
            ))
            .fact(computed(
                ctx,
                "drawdown",
                risk.state.drawdown,
                "ratio",
                &["risk_state"],
            ))
            .evidence(vec![format!(
                "limit-set:{} evaluated against risk state",
                risk.limits.len()
            )])
            .caveats(vec![
                "a limit check is arithmetic against declared limits; it is not a view on whether the position is a good idea"
                    .to_string(),
            ]);

        // Forecast: the tail the risk engine forecast for the book, at the
        // highest confidence it computed. Read, not recomputed — the state
        // is the engine's aggregate and a second derivation here would be a
        // second answer — and reported only where a figure exists, so a
        // state that forecast nothing is not reported as a zero tail.
        for (label, tail) in [
            ("forecast_value_at_risk", &risk.state.value_at_risk),
            (
                "forecast_expected_shortfall",
                &risk.state.expected_shortfall,
            ),
        ] {
            if let Some((_, value)) = tail
                .iter()
                .filter(|(_, value)| value.is_finite())
                .max_by(|a, b| a.0.cmp(b.0))
            {
                builder = builder.fact(computed(ctx, label, *value, "ratio", &["risk_state"]));
            }
        }

        // Stress: a generated scenario rather than a forecast — a three-sigma
        // day at the book's own annualised volatility, scaled by its
        // leverage, as a fraction of equity. Not a tail estimate: it says what
        // one bad day of the size the book's volatility already implies would
        // cost, which is the figure a desk can hold against its drawdown room.
        let equity = risk.state.equity.to_f64();
        let leverage = risk.state.gross_exposure.to_f64() / equity;
        if equity > 0.0 && risk.state.volatility > 0.0 && leverage.is_finite() {
            builder = builder.fact(computed(
                ctx,
                "stress_loss_3sd_day",
                STRESS_SIGMAS * risk.state.volatility / TRADING_DAYS.sqrt() * leverage,
                "ratio",
                &["risk_state"],
            ));
        }

        // Recommendation: the gross exposure to shed so every blocking limit
        // whose measure scales with the book's size is back inside its
        // bound. Leverage, volatility, the tail figures and the survival
        // ceiling all scale linearly with a proportional cut, so the cut is
        // set by the worst utilisation among them. A breach of any other
        // kind — a drawdown, a cash floor — is not cleared by selling, and
        // is left to the block above rather than given a number that would
        // not fix it. Zero when nothing that scales blocks.
        let scale_to_clear = blocking
            .iter()
            .filter(|breach| SCALES_WITH_GROSS.contains(&breach.limit_kind.as_str()))
            .filter(|breach| breach.observed > breach.bound && breach.bound > 0.0)
            .map(|breach| breach.bound / breach.observed)
            .fold(1.0_f64, f64::min);
        builder = builder.fact(computed(
            ctx,
            "recommended_gross_reduction",
            risk.state.gross_exposure.to_f64() * (1.0 - scale_to_clear),
            "currency",
            &["risk_state", "limit_set"],
        ));

        // Capital position (R12-CAPITAL-3020, ADR 0147 decision 8): the
        // capital facts the risk state already holds, read and never
        // recomputed, and each reported only where the state holds a value,
        // so a funding buffer nobody measured is not reported as zero. No
        // veto, limit or figure here reads them: they are what the desk sees,
        // not something it acts on through this finding.
        builder = builder.fact(computed(
            ctx,
            "cash",
            risk.state.cash.to_f64(),
            "currency",
            &["risk_state"],
        ));
        if let Some(buffer) = risk.state.funding_buffer.filter(|value| value.is_finite()) {
            builder = builder.fact(computed(
                ctx,
                "funding_buffer",
                buffer,
                "ratio",
                &["risk_state"],
            ));
        }
        if let Some(utilisation) = risk
            .state
            .margin_utilisation
            .filter(|value| value.is_finite())
        {
            builder = builder.fact(computed(
                ctx,
                "margin_utilisation",
                utilisation,
                "ratio",
                &["risk_state"],
            ));
        }
        if let Some(ceiling) = risk.state.survival_gross_ceiling {
            builder = builder.fact(computed(
                ctx,
                "survival_gross_ceiling",
                ceiling.to_f64(),
                "currency",
                &["risk_state"],
            ));
        }

        if direction == Direction::Negative {
            builder = builder.falsifiers(vec![
                "the risk state changes so the breached limit is no longer breached".to_string(),
                "an authorised operator changes the limit, which no agent can do".to_string(),
            ]);
        }
        builder.build()
    }
}

// --- compliance -------------------------------------------------------------

/// Checks proposals against restricted lists, jurisdiction and venue rules.
#[derive(Debug)]
pub struct ComplianceControl {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl ComplianceControl {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for ComplianceControl {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        if brief.objects.is_empty() {
            return Ok(no_data(ctx, brief.as_of, "no instrument in scope to check"));
        }

        let compliance = self.desk.compliance.get(ctx)?;

        let mut prohibited = Vec::new();
        let mut buy_blocked = Vec::new();
        let mut short_blocked = Vec::new();
        let mut uncovered = Vec::new();
        let mut evidence = Vec::new();

        for object in &brief.objects {
            let id = object.as_str();
            if compliance.is_prohibited(id) {
                prohibited.push(id.to_string());
                evidence.push(format!("prohibition:{id}"));
                continue;
            }
            let Some(constraints) = compliance.constraints_for(id) else {
                // No record is not permission. An instrument the compliance
                // register does not cover is escalated, not waved through.
                uncovered.push(id.to_string());
                continue;
            };
            evidence.push(format!("constraints:{id}"));
            let buy = constraints.buy_blockers();
            if !buy.is_empty() {
                buy_blocked.push(format!(
                    "{id}: {}",
                    buy.iter()
                        .map(|r| r.describe())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            let short = constraints.short_blockers();
            if !short.is_empty() {
                short_blocked.push(format!(
                    "{id}: {}",
                    short
                        .iter()
                        .map(|r| r.describe())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        let blocked = !prohibited.is_empty() || !buy_blocked.is_empty();
        let claim = if !prohibited.is_empty() {
            format!(
                "{} name(s) are under an absolute prohibition: {}",
                prohibited.len(),
                prohibited.join(", ")
            )
        } else if blocked {
            format!("buying is blocked for: {}", buy_blocked.join("; "))
        } else if !uncovered.is_empty() {
            format!(
                "{} name(s) are not covered by the compliance register and must be escalated: {}",
                uncovered.len(),
                uncovered.join(", ")
            )
        } else {
            format!("{} name(s) carry no buy restriction", brief.objects.len())
        };

        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(
                if blocked {
                    Direction::Negative
                } else {
                    Direction::Neutral
                },
                if blocked { 1.0 } else { 0.0 },
            )
            .fact(computed(
                ctx,
                "prohibited_count",
                prohibited.len() as f64,
                "count",
                &["compliance_register"],
            ))
            .fact(computed(
                ctx,
                "buy_blocked_count",
                buy_blocked.len() as f64,
                "count",
                &["compliance_register"],
            ))
            .fact(computed(
                ctx,
                "short_blocked_count",
                short_blocked.len() as f64,
                "count",
                &["compliance_register"],
            ))
            .fact(computed(
                ctx,
                "uncovered_count",
                uncovered.len() as f64,
                "count",
                &["compliance_register"],
            ))
            .evidence(evidence)
            .caveats(vec![
                "restriction checks are deterministic lookups; anything the register does not cover is escalated rather than decided"
                    .to_string(),
            ]);

        if blocked {
            builder = builder.falsifiers(vec![
                "the restriction is lifted from the register by an authorised change".to_string(),
            ]);
        }
        if !uncovered.is_empty() {
            builder = builder.missing(
                uncovered
                    .iter()
                    .map(|id| format!("compliance register entry for {id}"))
                    .collect(),
            );
        }
        builder.build()
    }
}
