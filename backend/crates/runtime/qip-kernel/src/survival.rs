//! The capital survival kernel in the cycle: what DECIDE may size against, and
//! the ceiling the Risk Gate holds orders to (R12-CAPITAL-4086, 4087, 4089;
//! R12-RISK-4116..4124, 7205; R12-CAPITAL-7204).
//!
//! `qip_capital::CapitalSurvivalState` held the priority waterfall — NAV,
//! reserved cash, margin, collateral, settlement, funding maturity, hedge
//! capacity, stressed liquidity, and what is left as opportunity — and nothing
//! in the runtime constructed one. The construction budget was the free
//! balance less unfunded commitments, so hedge capacity and the stressed-
//! liquidity reserve were withheld from nothing, and the stress report
//! SIMULATE produced every cycle was read by a gauge and an accessor and
//! changed no number anything was sized against. A book that would not
//! survive its own worst scenario sized its next position exactly as one that
//! would.
//!
//! [`SurvivalGovernor::size`] now runs the waterfall every construction, from
//! the platform's own ledgers:
//!
//! | Tier | Derived from |
//! |---|---|
//! | total capital | tracked equity |
//! | reserved cash | the reservation ledger's active holds |
//! | funding maturity | the commitment book's unfunded total |
//! | hedge capacity | [`SurvivalPolicy::hedge_capacity_fraction`] of equity |
//! | stressed-liquidity reserve | the excess of the worst stressed loss over the tolerance ([`TailResponse`]) |
//!
//! Margin, collateral and settlement are not derived here yet and stand at
//! zero; that is R12-CAPITAL-4087's remaining gap and is stated rather than
//! hidden.
//!
//! # Two derivations, reconciled
//!
//! `Platform::deployable_capital` already answers "equity less holds less
//! commitments". The waterfall answers the same question with two more tiers
//! withheld, so the two must reconcile exactly — opportunity plus hedge plus
//! stressed reserve equals deployable — and a construction where they do not is
//! refused. Two independent claims about one balance disagree eventually; this
//! is where the disagreement becomes a refusal rather than a size.
//!
//! # Contraction only
//!
//! Every tier withholds, the confidence governor multiplies by a factor in
//! `(0, 1]`, and the result is checked against the deployable capital it was
//! derived from: [`SizingBudget::released`] can never exceed what the ledger
//! already allowed. A tail world the book does not survive can shrink the next
//! construction to a refusal; it can never make one larger.
//!
//! # Enforcement stays with the gate
//!
//! This module sizes; it does not stop orders. What it releases becomes
//! [`SurvivalGovernor::gross_ceiling`], which the kernel writes into the
//! `RiskState` every pre-trade check reads, where `LimitKind::MaxSurvivalGross`
//! evaluates it deterministically. No alpha or model component reaches this
//! module's inputs except through measurements it refuses when malformed, and
//! none can raise its output (R12-CAPITAL-7204).

use crate::confidence::{ConfidenceDecision, ConfidenceSignals, ThesisView};
use crate::platform::StressReport;
use qip_capital::{CapitalSurvivalState, SurvivalReport};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

/// The operator's survival settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurvivalPolicy {
    hedge_capacity_fraction: Decimal,
}

impl Default for SurvivalPolicy {
    /// No hedge capacity withheld. Zero is not a claim that hedging needs no
    /// capital; it is the absence of a configured figure, and a figure written
    /// here would be a number nobody calibrated (principle 6). The tail tier
    /// needs no setting — it is measured every cycle.
    fn default() -> Self {
        Self {
            hedge_capacity_fraction: Decimal::ZERO,
        }
    }
}

impl SurvivalPolicy {
    /// Withhold `fraction` of equity as hedge capacity before any opportunity
    /// capital is released. Refuses a fraction outside `[0, 1)`: a negative
    /// one would release capital the waterfall withholds, and one of a whole
    /// or more leaves nothing to trade and should be a halt, not a sizing.
    pub fn with_hedge_capacity_fraction(fraction: Decimal) -> Result<Self> {
        if fraction.is_negative() || fraction >= Decimal::ONE {
            return Err(Error::invalid(format!(
                "a hedge-capacity fraction of {fraction} is outside [0, 1); halt the book rather \
                 than reserve all of it, and never reserve less than nothing"
            )));
        }
        Ok(Self {
            hedge_capacity_fraction: fraction,
        })
    }

    pub fn hedge_capacity_fraction(&self) -> Decimal {
        self.hedge_capacity_fraction
    }
}

/// What a tail world the book does not survive withheld.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TailResponse {
    /// The world that cost the most.
    pub scenario: String,
    pub loss_fraction: f64,
    /// The tolerance the loss was measured against.
    pub tolerance: f64,
    /// The excess of the loss over the tolerance, in money: what the book
    /// would have to stand behind beyond what it accepted, withheld as the
    /// stressed-liquidity reserve. Zero when the book survives.
    pub reserve: Decimal,
}

impl TailResponse {
    pub fn survives(&self) -> bool {
        self.reserve.is_zero()
    }
}

/// The worst loss any world in `stress` produced, and its name: the library,
/// the adversarial sequence, every causal driver, and the generated tail
/// worlds — every number SIMULATE computed, and not a curated subset of them.
pub fn worst_world(stress: &StressReport) -> Option<(String, f64)> {
    let library = stress
        .scenarios
        .iter()
        .map(|r| (r.scenario.clone(), r.loss_fraction));
    let adversarial = stress.adversarial.iter().map(|sequence| {
        (
            qip_simulation_engine::scenario::ADVERSARIAL_SCENARIO_NAME.to_string(),
            sequence.result.loss_fraction,
        )
    });
    let causal = stress
        .causal
        .iter()
        .map(|r| (r.scenario.clone(), r.loss_fraction));
    let tail = stress
        .tail_worlds
        .iter()
        .map(|w| (w.scenario.clone(), w.loss_fraction));
    library
        .chain(adversarial)
        .chain(causal)
        .chain(tail)
        // A loss that is not a number is the worst loss there is: it is
        // carried as infinity so it wins the comparison and refuses below.
        .map(|(name, loss)| (name, if loss.is_nan() { f64::INFINITY } else { loss }))
        .fold(
            None,
            |worst: Option<(String, f64)>, (name, loss)| match worst {
                Some((_, held)) if held >= loss => worst,
                _ => Some((name, loss)),
            },
        )
}

/// The stressed-liquidity reserve `stress` demands of a book with `equity`.
///
/// `None` when nothing was stressed. A stressed loss that cannot be turned into
/// money — infinite, or past what a `Decimal` holds — refuses: a reserve that
/// could not be computed must not be read as a reserve of zero.
pub fn tail_response(
    stress: Option<&StressReport>,
    equity: Decimal,
    tolerance: f64,
) -> Result<Option<TailResponse>> {
    let Some((scenario, loss_fraction)) = stress.and_then(worst_world) else {
        return Ok(None);
    };
    let excess = (loss_fraction - tolerance).max(0.0);
    // The statistics-to-money crossing, made once and here: the stressed loss
    // is an `f64` fraction from the scenario tester, the reserve is money.
    let reserve = if excess == 0.0 {
        Decimal::ZERO
    } else {
        Decimal::from_f64(excess * equity.to_f64())
            .filter(|reserve| !reserve.is_negative())
            .ok_or_else(|| {
                Error::numeric(format!(
                    "the {scenario} world's loss of {loss_fraction} of equity cannot be carried \
                     as a reserve; nothing is sized against a tail that cannot be measured"
                ))
            })?
    };
    Ok(Some(TailResponse {
        scenario,
        loss_fraction,
        tolerance,
        reserve,
    }))
}

/// The ledger's own figures for one construction, read by the caller from the
/// platform's state at the instant `deployable_capital` resynced it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LedgerFigures {
    pub equity: Decimal,
    /// Active reservation holds.
    pub held: Decimal,
    /// Unfunded capital commitments.
    pub obligations: Decimal,
    /// `Platform::deployable_capital` for the same instant.
    pub deployable: Decimal,
    /// The book's gross exposure now.
    pub gross: Decimal,
}

/// What one construction was allowed to size against, and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SizingBudget {
    pub at: Timestamp,
    /// The waterfall as it ran.
    pub survival: SurvivalReport,
    /// What the ledger alone would have deployed.
    pub deployable: Decimal,
    /// The tail world's demand, when the book was stressed.
    pub tail: Option<TailResponse>,
    /// The confidence governor's contraction.
    pub confidence: ConfidenceDecision,
    /// Opportunity capital narrowed by confidence: at most `deployable`.
    pub released: Decimal,
    /// The gross the survival kernel stands behind: the book's gross now plus
    /// what was released.
    pub gross_ceiling: Decimal,
}

impl SizingBudget {
    /// One line per tier that withheld or narrowed, for the DECIDE detail.
    /// Empty when nothing beyond the ledger bit.
    pub fn narrowing(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.survival.hedge_capacity.is_positive() {
            lines.push(format!(
                "{} withheld as hedge capacity",
                self.survival.hedge_capacity
            ));
        }
        if let Some(tail) = &self.tail
            && !tail.survives()
        {
            lines.push(format!(
                "the book loses {:.1}% of equity in {} against a {:.0}% tolerance, so {} is \
                 withheld as stressed-liquidity reserve",
                tail.loss_fraction * 100.0,
                tail.scenario,
                tail.tolerance * 100.0,
                tail.reserve
            ));
        }
        lines.extend(
            self.confidence
                .contractions
                .iter()
                .map(|contraction| contraction.reason.clone()),
        );
        lines
    }
}

/// The survival kernel's state between constructions.
#[derive(Clone, Debug, Default)]
pub struct SurvivalGovernor {
    policy: SurvivalPolicy,
    signals: ConfidenceSignals,
    last: Option<SizingBudget>,
}

impl SurvivalGovernor {
    pub fn with_policy(policy: SurvivalPolicy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    pub fn policy(&self) -> &SurvivalPolicy {
        &self.policy
    }

    /// The measurements the confidence governor reads, for the stages that
    /// take them.
    pub fn signals_mut(&mut self) -> &mut ConfidenceSignals {
        &mut self.signals
    }

    pub fn signals(&self) -> &ConfidenceSignals {
        &self.signals
    }

    /// The last construction's budget.
    pub fn last(&self) -> Option<&SizingBudget> {
        self.last.as_ref()
    }

    /// The gross the survival kernel last stood behind, for the risk state
    /// the gate reads. `None` before any construction, which the survival
    /// limit refuses on rather than reading as unlimited.
    pub fn gross_ceiling(&self) -> Option<Decimal> {
        self.last.as_ref().map(|budget| budget.gross_ceiling)
    }

    /// Run the waterfall and the confidence governor for one construction.
    ///
    /// Refuses when the waterfall withholds more than the book holds, when the
    /// waterfall and the ledger disagree about the same balance, and when any
    /// measurement the governor reads is malformed. Each refusal names the
    /// tier, because "nothing was sized" and "nothing could be sized because
    /// the tail reserve exceeds free capital" are different instructions to
    /// the desk.
    pub fn size(
        &mut self,
        figures: LedgerFigures,
        stress: Option<&StressReport>,
        tolerance: f64,
        theses: &[ThesisView],
        now: Timestamp,
    ) -> Result<SizingBudget> {
        let hedge = figures
            .equity
            .checked_mul(self.policy.hedge_capacity_fraction)
            .ok_or_else(|| Error::numeric("hedge capacity overflows".to_string()))?;
        let tail = tail_response(stress, figures.equity, tolerance)?;
        let stressed = tail.as_ref().map_or(Decimal::ZERO, |tail| tail.reserve);
        let state = CapitalSurvivalState::new(figures.equity)?
            .with_reserved_cash(figures.held)?
            .with_funding_maturity(figures.obligations)?
            .with_hedge_capacity(hedge)?
            .with_stressed_liquidity_reserve(stressed)?;
        let survival = state.report().map_err(|error| {
            Error::denied(format!(
                "the capital survival kernel releases nothing to size: {}; reduce the book until \
                 it survives its worst stressed world, or fund the reserves",
                error.message()
            ))
        })?;

        let reconciled = survival
            .opportunity_capital
            .checked_add(hedge)
            .and_then(|sum| sum.checked_add(stressed))
            .ok_or_else(|| Error::numeric("the survival tiers overflow".to_string()))?;
        if reconciled != figures.deployable {
            return Err(Error::invalid(format!(
                "the survival waterfall and the ledger disagree about the same balance: \
                 opportunity {} plus hedge {hedge} plus stressed reserve {stressed} is \
                 {reconciled}, and the ledger deploys {}; nothing is sized until the two \
                 derivations agree",
                survival.opportunity_capital, figures.deployable
            )));
        }

        let confidence = self.signals.decide(theses)?;
        let released = survival
            .opportunity_capital
            .checked_mul(confidence.multiplier)
            .ok_or_else(|| Error::numeric("the released capital overflows".to_string()))?;
        if released > figures.deployable || released.is_negative() {
            return Err(Error::invalid(format!(
                "the survival kernel would release {released} against {} deployable; it may only \
                 ever contract the budget",
                figures.deployable
            )));
        }
        let gross_ceiling =
            figures.gross.abs().checked_add(released).ok_or_else(|| {
                Error::numeric("the survival gross ceiling overflows".to_string())
            })?;
        let budget = SizingBudget {
            at: now,
            survival,
            deployable: figures.deployable,
            tail,
            confidence,
            released,
            gross_ceiling,
        };
        self.last = Some(budget.clone());
        Ok(budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;
    use qip_cost_router::context::MarketRegime;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn figures(deployable: Decimal) -> LedgerFigures {
        LedgerFigures {
            equity: dec!("1000000"),
            held: dec!("100000"),
            obligations: dec!("50000"),
            deployable,
            gross: dec!("400000"),
        }
    }

    fn views() -> Vec<ThesisView> {
        vec![ThesisView {
            hypothesis_id: "h".into(),
            object_id: "obj-AAA".into(),
            regime: MarketRegime::Trending,
        }]
    }

    #[test]
    fn hedge_capacity_is_withheld_before_opportunity_and_removing_it_raises_the_budget() {
        let mut without = SurvivalGovernor::default();
        let open = without
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap();
        assert_eq!(open.released, dec!("850000"), "premise: nothing withheld");

        let mut with = SurvivalGovernor::with_policy(
            SurvivalPolicy::with_hedge_capacity_fraction(dec!("0.1")).unwrap(),
        );
        let hedged = with
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap();
        assert_eq!(hedged.survival.hedge_capacity, dec!("100000"));
        assert_eq!(hedged.released, dec!("750000"));
        assert_eq!(hedged.gross_ceiling, dec!("1150000"));
        assert!(hedged.released < open.released);
    }

    #[test]
    fn a_waterfall_that_disagrees_with_the_ledger_is_refused() {
        let mut governor = SurvivalGovernor::default();
        assert!(
            governor
                .size(figures(dec!("850000")), None, 0.20, &views(), at())
                .is_ok(),
            "premise: the two derivations agree"
        );
        let error = governor
            .size(figures(dec!("900000")), None, 0.20, &views(), at())
            .unwrap_err();
        assert!(error.message().contains("disagree"), "{}", error.message());
    }

    #[test]
    fn reserves_beyond_the_book_refuse_rather_than_size_zero() {
        let mut governor = SurvivalGovernor::with_policy(
            SurvivalPolicy::with_hedge_capacity_fraction(dec!("0.9")).unwrap(),
        );
        let error = governor
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap_err();
        assert!(
            error.message().contains("releases nothing"),
            "{}",
            error.message()
        );
        assert!(
            governor.gross_ceiling().is_none(),
            "a refusal records no ceiling"
        );
    }

    #[test]
    fn a_policy_that_would_release_withheld_capital_is_refused() {
        assert!(SurvivalPolicy::with_hedge_capacity_fraction(dec!("-0.1")).is_err());
        assert!(SurvivalPolicy::with_hedge_capacity_fraction(dec!("1")).is_err());
        assert!(SurvivalPolicy::with_hedge_capacity_fraction(dec!("0")).is_ok());
    }

    #[test]
    fn the_confidence_governor_narrows_what_survival_releases_and_never_widens_it() {
        let mut governor = SurvivalGovernor::default();
        let calm = governor
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap();
        governor
            .signals_mut()
            .observe_calibration(0.40, 50, [MarketRegime::Trending]);
        let decayed = governor
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap();
        assert!(decayed.released < calm.released);
        assert_eq!(
            decayed.released,
            dec!("850000") * crate::confidence::CALIBRATION_FLOOR
        );
        assert!(decayed.released <= decayed.deployable);
    }
}
