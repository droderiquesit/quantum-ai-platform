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
//! | total capital (NAV) | tracked equity |
//! | free cash | equity less the reservation ledger's active holds — `Platform::deployable_capital`'s own figure |
//! | reserved cash | the reservation ledger's active holds |
//! | margin | [`CapitalObligations::margin_required`] — the cross-margin review's maintenance on every venue carrying gross |
//! | collateral | [`CapitalObligations::collateral_encumbered`] |
//! | settlement | [`CapitalObligations::settlement_due`] |
//! | funding maturity | the commitment book's unfunded total |
//! | emergency reserve | [`SurvivalPolicy::emergency_reserve_fraction`] of equity, carried on the NAV-floor tier |
//! | hedge capacity | [`SurvivalPolicy::hedge_capacity_fraction`] of equity |
//! | stressed-liquidity reserve | the excess of the worst stressed loss over the tolerance ([`TailResponse`]) |
//! | deployable / opportunity | what is left |
//!
//! Margin, collateral and settlement arrive through the [`CapitalObligations`]
//! trait rather than as numbers a caller types in, because "a reserve typed in
//! by a caller is not an account" (R12-CAPITAL-4087). The platform implements
//! it over the cross-margin review it already runs every cycle
//! (`crate::cross_margin`); a ledger of record that carries margin, collateral
//! and settlement accounts implements the same three methods and replaces it
//! without touching the waterfall. What the review cannot see it says, in the
//! implementation's doc, rather than reading as zero by omission.
//!
//! # The ceiling the gate reads, and the reservations inside it
//!
//! [`SurvivalGovernor::gross_ceiling`] is the gross the kernel stands behind:
//! the book's gross now, **plus the capital already held for proposals that
//! have not yet traded, plus the hedge capacity, plus what this construction
//! released**. The held term is the reservation accounting
//! `LimitKind::MaxSurvivalGross` needed before it could ship in the kernel's
//! limit set ([`install_survival_limit`]): without it a proposal approved on
//! one cycle and executed on a later one was sized inside a budget, held its
//! capital in the reservation ledger, and was then vetoed by a ceiling that
//! had been recomputed from a `free` balance that no longer counted that hold
//! — the survival limit refusing exactly the orders the survival kernel had
//! already paid for. The hedge term is there because the capacity is withheld
//! *so that a hedge can be held*; a ceiling that excluded it would veto the
//! hedge the waterfall reserved capital for.
//!
//! A construction the waterfall refuses does not leave the previous ceiling
//! standing. It drops the ceiling to the gross already held plus what is
//! already reserved: nothing new is stood behind, an order that sheds gross
//! still passes (the limit's own de-risking arm), and the book is not halted
//! for holding what it holds. Leaving the previous, larger ceiling in place
//! after a refusal would be the survival kernel saying "nothing" to DECIDE and
//! "carry on" to the gate in the same cycle.
//!
//! # Two derivations, reconciled
//!
//! `Platform::deployable_capital` already answers "equity less holds less
//! commitments". The waterfall answers the same question with two more tiers
//! withheld, so the two must reconcile exactly — opportunity plus margin,
//! collateral, settlement, emergency reserve, hedge and stressed reserve equals
//! deployable — and a construction where they do not is refused. Two independent claims about one balance disagree eventually; this
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
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use serde::{Deserialize, Serialize};

/// The name the survival limit carries in every limit set the kernel runs.
pub const SURVIVAL_LIMIT_NAME: &str = "survival-gross";

/// The bound the kernel's survival limit carries: gross may not exceed the
/// ceiling the survival kernel released. One constant, so the limit the
/// kernel installs and the one [`install_survival_limit`] will accept from a
/// handed-in set cannot drift apart.
pub const SURVIVAL_LIMIT_BOUND: f64 = 1.0;

/// The configuration variable a composition root reads the hedge-capacity
/// fraction from. Read in `apps/*/src/main.rs` and nowhere else.
pub const HEDGE_CAPACITY_VARIABLE: &str = "QIP_SURVIVAL_HEDGE_FRACTION";

/// The configuration variable a composition root reads the emergency-reserve
/// fraction from.
pub const EMERGENCY_RESERVE_VARIABLE: &str = "QIP_SURVIVAL_EMERGENCY_FRACTION";

/// The hedge capacity a shipped binary withholds when nothing is configured:
/// five per cent of equity.
///
/// Not calibrated to any mandate, and said so: it is the restrictive default
/// a deployment starts from, in the same register as
/// `LimitSet::conservative_default`. Before it existed every binary ran with
/// [`SurvivalPolicy::default`]'s zero, because no composition root called
/// `Platform::set_survival_policy`, so the hedge tier of the waterfall
/// withheld nothing in any process that has ever run (R12-CAPITAL-4089). It is
/// also a **floor**: a configured figure below it is refused at start-up,
/// because a setting that relaxes a safety default stops the process rather
/// than lowering it (principle 3).
pub const SHIPPED_HEDGE_CAPACITY_FRACTION: Decimal = Decimal::from_raw(50_000_000);

/// The emergency reserve a shipped binary withholds when nothing is
/// configured: five per cent of equity, never released to opportunity. A
/// floor for the same reason as [`SHIPPED_HEDGE_CAPACITY_FRACTION`].
pub const SHIPPED_EMERGENCY_RESERVE_FRACTION: Decimal = Decimal::from_raw(50_000_000);

/// Put `LimitKind::MaxSurvivalGross` into `limits` unless a survival limit is
/// already there.
///
/// The limit existed, evaluated deterministically, and was in no limit set
/// any process ran: `LimitSet::conservative_default` does not carry it, so
/// the survival ceiling the kernel computed every construction was a figure
/// in a `RiskState` that nothing compared an order against (R12-RISK-7205).
/// It is installed here — by the kernel, on whatever set the root hands in —
/// rather than in the shipped default, because the ceiling it reads is a
/// figure only this kernel produces: an edge gate or a library test holding
/// the shipped set has no survival kernel behind it, and would refuse every
/// order on the absent ceiling.
///
/// A bound of one: gross may not exceed the ceiling. Refuses a set that
/// already carries a *different* limit under [`SURVIVAL_LIMIT_NAME`], because
/// adding a second limit of that name would leave a rule review and a breach
/// report unable to tell the two apart.
pub fn install_survival_limit(limits: LimitSet) -> Result<LimitSet> {
    // A survival limit already in the set is kept only if it is the one this
    // function would install. This used to return early on *any*
    // `MaxSurvivalGross`, whatever its bound: a handed-in set carrying one
    // with a bound of 1000 left gross free to reach a thousand times the
    // ceiling the survival kernel computed, and the kernel installed nothing
    // because a limit of the right kind was "already there" — a ceiling that
    // cannot fire, wearing the name of one that can. Refused rather than
    // replaced, so the operator who wrote that bound learns it was not used.
    let mut carried = false;
    for limit in &limits.limits {
        if let LimitKind::MaxSurvivalGross { limit: bound } = limit.kind {
            if bound != SURVIVAL_LIMIT_BOUND {
                return Err(Error::invalid(format!(
                    "the limit set {} carries a survival-gross limit `{}` with a bound of \
                     {bound}; the kernel's survival ceiling is enforced at \
                     {SURVIVAL_LIMIT_BOUND} of what the survival kernel released, and a set \
                     that states another bound is refused rather than trusted or silently \
                     replaced — remove it, and the kernel installs its own",
                    limits.name, limit.name
                )));
            }
            carried = true;
        }
    }
    if carried {
        return Ok(limits);
    }
    if limits
        .limits
        .iter()
        .any(|limit| limit.name == SURVIVAL_LIMIT_NAME)
    {
        return Err(Error::invalid(format!(
            "the limit set {} already names a limit `{SURVIVAL_LIMIT_NAME}` that is not the \
             survival ceiling; rename it, because the kernel installs its survival limit under \
             that name",
            limits.name
        )));
    }
    Ok(limits.with(
        Limit::new(
            SURVIVAL_LIMIT_NAME,
            LimitKind::MaxSurvivalGross {
                limit: SURVIVAL_LIMIT_BOUND,
            },
        )
        .with_rationale(
            "the capital survival kernel stands behind no more gross than the book holds, \
                 what is already held for approved proposals, the hedge capacity it withheld and \
                 what its last construction released; an order past that is one the waterfall \
                 never funded",
        ),
    ))
}

/// The operator's survival settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurvivalPolicy {
    hedge_capacity_fraction: Decimal,
    #[serde(default)]
    emergency_reserve_fraction: Decimal,
}

impl Default for SurvivalPolicy {
    /// No hedge capacity and no emergency reserve withheld. Zero is not a
    /// claim that hedging needs no capital; it is the library's absence of a
    /// configured figure. A shipped binary never runs on it: every
    /// composition root takes [`SurvivalPolicy::from_settings`], whose unset
    /// value is [`SurvivalPolicy::shipped`]. The tail tier needs no setting —
    /// it is measured every cycle.
    fn default() -> Self {
        Self {
            hedge_capacity_fraction: Decimal::ZERO,
            emergency_reserve_fraction: Decimal::ZERO,
        }
    }
}

impl SurvivalPolicy {
    /// Withhold `fraction` of equity as hedge capacity before any opportunity
    /// capital is released. Refuses a fraction outside `[0, 1)`: a negative
    /// one would release capital the waterfall withholds, and one of a whole
    /// or more leaves nothing to trade and should be a halt, not a sizing.
    pub fn with_hedge_capacity_fraction(fraction: Decimal) -> Result<Self> {
        Self::new(fraction, Decimal::ZERO)
    }

    /// Both fractions. Refuses either outside `[0, 1)` and a sum of a whole
    /// or more, for the reason [`Self::with_hedge_capacity_fraction`] gives.
    pub fn new(hedge_capacity: Decimal, emergency_reserve: Decimal) -> Result<Self> {
        for (name, fraction) in [
            ("hedge-capacity", hedge_capacity),
            ("emergency-reserve", emergency_reserve),
        ] {
            if fraction.is_negative() || fraction >= Decimal::ONE {
                return Err(Error::invalid(format!(
                    "a {name} fraction of {fraction} is outside [0, 1); halt the book rather \
                     than reserve all of it, and never reserve less than nothing"
                )));
            }
        }
        let sum = hedge_capacity
            .checked_add(emergency_reserve)
            .ok_or_else(|| Error::numeric("the survival fractions overflow".to_string()))?;
        if sum >= Decimal::ONE {
            return Err(Error::invalid(format!(
                "a hedge capacity of {hedge_capacity} and an emergency reserve of \
                 {emergency_reserve} withhold the whole book between them; halt it instead"
            )));
        }
        Ok(Self {
            hedge_capacity_fraction: hedge_capacity,
            emergency_reserve_fraction: emergency_reserve,
        })
    }

    /// What a shipped binary runs when nothing is configured.
    pub fn shipped() -> Self {
        Self {
            hedge_capacity_fraction: SHIPPED_HEDGE_CAPACITY_FRACTION,
            emergency_reserve_fraction: SHIPPED_EMERGENCY_RESERVE_FRACTION,
        }
    }

    /// The policy a composition root runs, from the two configuration values
    /// it read — `None` where the variable is unset.
    ///
    /// Unset is [`Self::shipped`], never zero. A value that does not parse as
    /// a decimal, lies outside `[0, 1)`, or sits **below** the shipped figure
    /// is refused with the variable named: a typo that read as zero, or a
    /// deployment that quietly halved its reserves, is exactly the
    /// configuration principle 3 says must stop the process. Raising either
    /// fraction is always admitted — it only contracts what is released.
    pub fn from_settings(hedge: Option<&str>, emergency: Option<&str>) -> Result<Self> {
        let read = |variable: &str, value: Option<&str>, floor: Decimal| -> Result<Decimal> {
            let Some(text) = value else {
                return Ok(floor);
            };
            let fraction = Decimal::parse(text.trim()).ok_or_else(|| {
                Error::invalid(format!(
                    "configuration: {variable}={text:?} is not a decimal fraction; set a value \
                     in [{floor}, 1) or unset it for the shipped {floor}"
                ))
            })?;
            if fraction < floor {
                return Err(Error::invalid(format!(
                    "configuration: {variable}={fraction} is below the shipped {floor}; a \
                     survival reserve is raised by configuration and never lowered — unset it to \
                     run the shipped figure"
                )));
            }
            Ok(fraction)
        };
        let hedge = read(
            HEDGE_CAPACITY_VARIABLE,
            hedge,
            SHIPPED_HEDGE_CAPACITY_FRACTION,
        )?;
        let emergency = read(
            EMERGENCY_RESERVE_VARIABLE,
            emergency,
            SHIPPED_EMERGENCY_RESERVE_FRACTION,
        )?;
        Self::new(hedge, emergency)
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))
    }

    pub fn hedge_capacity_fraction(&self) -> Decimal {
        self.hedge_capacity_fraction
    }

    pub fn emergency_reserve_fraction(&self) -> Decimal {
        self.emergency_reserve_fraction
    }

    /// One line for a start-up banner.
    pub fn describe(&self) -> String {
        format!(
            "{} of equity withheld as hedge capacity and {} as emergency reserve before any \
             opportunity capital is released",
            self.hedge_capacity_fraction, self.emergency_reserve_fraction
        )
    }
}

/// The three obligations the platform's own ledgers carry that the waterfall
/// withholds beside the reservation holds and the commitment book
/// (R12-CAPITAL-4086, 4087).
///
/// A trait rather than three fields a caller fills, so that the figure is
/// always *derived from* something that holds the account — today the
/// cross-margin review, tomorrow a ledger of record with margin, collateral
/// and settlement accounts — and never typed in beside it. Each method refuses
/// rather than answering zero when its source cannot be read.
pub trait CapitalObligations {
    /// What venues require to stay posted against the exposure carried there.
    fn margin_required(&self) -> Result<Decimal>;
    /// Collateral pledged against obligations **other than** the margin above,
    /// so the same money is never withheld twice.
    fn collateral_encumbered(&self) -> Result<Decimal>;
    /// Cash owed for trades that have executed and not yet settled.
    fn settlement_due(&self) -> Result<Decimal>;
}

/// The cross-margin review the platform runs every cycle, as an obligation
/// source.
///
/// * **Margin** is `maintenance_required`: the model's maintenance rate on the
///   gross at *every* venue carrying it, observed or not, plus every observed
///   debit. An unobserved venue is charged rather than skipped, because an
///   account nobody has read must not read as one that needs nothing posted.
/// * **Collateral** is zero, derived rather than assumed: every pledge this
///   platform can attest is a pledge *into a margin domain* (see
///   `crate::cross_margin::review`), so it is the cover for the margin above
///   and not a second encumbrance. No pledge outside a margin domain is
///   recorded anywhere in this platform; one would be the ledger's to report.
/// * **Settlement** is zero for the same reason: the simulated venue books
///   cash at the fill, so no executed trade owes anything later. A venue that
///   settles T+n reports its unsettled cash through a ledger implementing this
///   trait, not through this one.
impl CapitalObligations for crate::cross_margin::CrossMarginReview {
    fn margin_required(&self) -> Result<Decimal> {
        if self.maintenance_required.is_negative() {
            return Err(Error::invalid(format!(
                "the cross-margin review requires {} of maintenance, which is not an amount \
                 anybody can post",
                self.maintenance_required
            )));
        }
        Ok(self.maintenance_required)
    }

    fn collateral_encumbered(&self) -> Result<Decimal> {
        Ok(Decimal::ZERO)
    }

    fn settlement_due(&self) -> Result<Decimal> {
        Ok(Decimal::ZERO)
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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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
    /// [`CapitalObligations::margin_required`].
    pub margin: Decimal,
    /// [`CapitalObligations::collateral_encumbered`].
    pub collateral: Decimal,
    /// [`CapitalObligations::settlement_due`].
    pub settlement: Decimal,
}

impl LedgerFigures {
    /// The figures with the three obligations a ledger carries, read from
    /// `source`. Refuses a negative obligation by name: a negative margin
    /// requirement would release capital the waterfall withholds.
    pub fn with_obligations(mut self, source: &impl CapitalObligations) -> Result<Self> {
        let margin = source.margin_required()?;
        let collateral = source.collateral_encumbered()?;
        let settlement = source.settlement_due()?;
        for (name, amount) in [
            ("margin", margin),
            ("collateral", collateral),
            ("settlement", settlement),
        ] {
            if amount.is_negative() {
                return Err(Error::invalid(format!(
                    "the {name} obligation reads {amount}; an obligation below nothing would \
                     release capital the survival waterfall withholds"
                )));
            }
        }
        self.margin = margin;
        self.collateral = collateral;
        self.settlement = settlement;
        Ok(self)
    }

    /// Equity less the active holds: the free cash before any obligation.
    pub fn free_cash(&self) -> Result<Decimal> {
        self.equity
            .checked_sub(self.held)
            .ok_or_else(|| Error::numeric("free cash overflows".to_string()))
    }

    /// The gross the book already stands in plus the capital already held for
    /// approved proposals: what the survival kernel stands behind when it
    /// releases nothing new.
    fn standing(&self) -> Option<Decimal> {
        self.gross.abs().checked_add(self.held)
    }
}

/// What one construction was allowed to size against, and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SizingBudget {
    pub at: Timestamp,
    /// The waterfall as it ran. `nav_floor` carries the emergency reserve.
    pub survival: SurvivalReport,
    /// Equity less active holds.
    pub free_cash: Decimal,
    /// What the ledger alone would have deployed.
    pub deployable: Decimal,
    /// The tail world's demand, when the book was stressed.
    pub tail: Option<TailResponse>,
    /// The confidence governor's contraction.
    pub confidence: ConfidenceDecision,
    /// Opportunity capital narrowed by confidence: at most `deployable`.
    pub released: Decimal,
    /// The gross the survival kernel stands behind: the book's gross now, the
    /// capital held for approved proposals, the hedge capacity, and what was
    /// released.
    pub gross_ceiling: Decimal,
}

impl SizingBudget {
    /// One line per tier that withheld or narrowed, for the DECIDE detail.
    /// Empty when nothing beyond the ledger bit.
    pub fn narrowing(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (amount, what) in [
            (self.survival.margin_required, "margin"),
            (self.survival.collateral_posted, "encumbered collateral"),
            (self.survival.settlement_required, "unsettled trades"),
            (self.survival.nav_floor, "emergency reserve"),
            (self.survival.hedge_capacity, "hedge capacity"),
        ] {
            if amount.is_positive() {
                lines.push(format!("{amount} withheld as {what}"));
            }
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
    /// What the gate reads. Set by every construction, successful or not, and
    /// by [`Self::seed`]; see the module doc for why a refusal lowers it.
    ceiling: Option<Decimal>,
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

    /// The last successful construction's budget.
    pub fn last(&self) -> Option<&SizingBudget> {
        self.last.as_ref()
    }

    /// The gross the survival kernel stands behind, for the risk state the
    /// gate reads. `None` only before anything was sized or seeded, which the
    /// survival limit refuses on rather than reading as unlimited.
    pub fn gross_ceiling(&self) -> Option<Decimal> {
        self.ceiling
    }

    /// The contraction an issued capital grant takes (R12-RISK-4111, 4112,
    /// 4114, 4115): the confidence governor narrows the next grant a cell is
    /// signed, not only the desk's construction budget. See
    /// [`ConfidenceSignals::decide_for_grant`].
    pub fn grant_decision(&self) -> Result<ConfidenceDecision> {
        self.signals
            .decide_for_grant(self.last.as_ref().map(|budget| &budget.confidence))
    }

    /// Run the waterfall and the confidence governor for one construction.
    ///
    /// Refuses when the waterfall withholds more than the book holds, when the
    /// waterfall and the ledger disagree about the same balance, and when any
    /// measurement the governor reads is malformed. Each refusal names the
    /// tier, because "nothing was sized" and "nothing could be sized because
    /// the tail reserve exceeds free capital" are different instructions to
    /// the desk. A refusal also lowers the ceiling to what is already stood
    /// in (module doc).
    pub fn size(
        &mut self,
        figures: LedgerFigures,
        stress: Option<&StressReport>,
        tolerance: f64,
        theses: &[ThesisView],
        now: Timestamp,
    ) -> Result<SizingBudget> {
        match self.budget(figures, stress, tolerance, theses, now) {
            Ok(budget) => {
                self.ceiling = Some(budget.gross_ceiling);
                self.last = Some(budget.clone());
                Ok(budget)
            }
            Err(error) => {
                self.ceiling = figures.standing();
                Err(error)
            }
        }
    }

    /// Lower the ceiling to `gross` plus `held`: the book stands behind what
    /// it holds and what is already reserved, and nothing new.
    ///
    /// For a construction that failed at a step outside [`Self::size`] —
    /// before it (no free capital, an unreadable margin) or after it (the
    /// constructor or the reservation refused). Without it the previous
    /// construction's release would stand at the gate for a cycle whose
    /// DECIDE sized nothing. An overflow leaves no ceiling, which the
    /// survival limit refuses on.
    pub fn release_nothing(&mut self, gross: Decimal, held: Decimal) {
        self.ceiling = gross.abs().checked_add(held);
    }

    /// Set the ceiling from the ledger with no construction: what the
    /// platform runs on between its assembly and its first DECIDE.
    ///
    /// Without it the survival limit would refuse every gross-raising order
    /// until the first thesis was sized, which is a control nobody can
    /// distinguish from an outage. Records no budget — `last` stays the last
    /// *construction* — and a refusal lowers the ceiling exactly as
    /// [`Self::size`]'s does.
    pub fn seed(
        &mut self,
        figures: LedgerFigures,
        stress: Option<&StressReport>,
        tolerance: f64,
        now: Timestamp,
    ) -> Result<Decimal> {
        match self.budget(figures, stress, tolerance, &[], now) {
            Ok(budget) => {
                self.ceiling = Some(budget.gross_ceiling);
                Ok(budget.gross_ceiling)
            }
            Err(error) => {
                self.ceiling = figures.standing();
                Err(error)
            }
        }
    }

    fn budget(
        &self,
        figures: LedgerFigures,
        stress: Option<&StressReport>,
        tolerance: f64,
        theses: &[ThesisView],
        now: Timestamp,
    ) -> Result<SizingBudget> {
        let fraction_of_equity = |fraction: Decimal, what: &str| {
            figures
                .equity
                .checked_mul(fraction)
                .ok_or_else(|| Error::numeric(format!("{what} overflows")))
        };
        let hedge = fraction_of_equity(self.policy.hedge_capacity_fraction, "hedge capacity")?;
        let emergency =
            fraction_of_equity(self.policy.emergency_reserve_fraction, "emergency reserve")?;
        let tail = tail_response(stress, figures.equity, tolerance)?;
        let stressed = tail.as_ref().map_or(Decimal::ZERO, |tail| tail.reserve);
        let state = CapitalSurvivalState::new(figures.equity)?
            .with_nav_floor(emergency)?
            .with_reserved_cash(figures.held)?
            .with_margin_required(figures.margin)?
            .with_collateral_posted(figures.collateral)?
            .with_settlement_required(figures.settlement)?
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

        // Every tier the ledger's `deployable` does not already subtract.
        let reconciled = [
            figures.margin,
            figures.collateral,
            figures.settlement,
            emergency,
            hedge,
            stressed,
        ]
        .into_iter()
        .try_fold(survival.opportunity_capital, Decimal::checked_add)
        .ok_or_else(|| Error::numeric("the survival tiers overflow".to_string()))?;
        if reconciled != figures.deployable {
            return Err(Error::invalid(format!(
                "the survival waterfall and the ledger disagree about the same balance: \
                 opportunity {} plus margin {}, collateral {}, settlement {}, emergency reserve \
                 {emergency}, hedge {hedge} and stressed reserve {stressed} is {reconciled}, and \
                 the ledger deploys {}; nothing is sized until the two derivations agree",
                survival.opportunity_capital,
                figures.margin,
                figures.collateral,
                figures.settlement,
                figures.deployable
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
        let gross_ceiling = figures
            .standing()
            .and_then(|standing| standing.checked_add(hedge))
            .and_then(|ceiling| ceiling.checked_add(released))
            .ok_or_else(|| Error::numeric("the survival gross ceiling overflows".to_string()))?;
        Ok(SizingBudget {
            at: now,
            survival,
            free_cash: figures.free_cash()?,
            deployable: figures.deployable,
            tail,
            confidence,
            released,
            gross_ceiling,
        })
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
            ..LedgerFigures::default()
        }
    }

    fn views() -> Vec<ThesisView> {
        vec![ThesisView {
            hypothesis_id: "h".into(),
            object_id: "obj-AAA".into(),
            regime: MarketRegime::Trending,
        }]
    }

    /// An obligation source with stated figures, standing in for a ledger.
    struct Ledger {
        margin: Decimal,
        collateral: Decimal,
        settlement: Decimal,
    }

    impl CapitalObligations for Ledger {
        fn margin_required(&self) -> Result<Decimal> {
            Ok(self.margin)
        }
        fn collateral_encumbered(&self) -> Result<Decimal> {
            Ok(self.collateral)
        }
        fn settlement_due(&self) -> Result<Decimal> {
            Ok(self.settlement)
        }
    }

    fn stress_with(worlds: Vec<crate::tail::TailWorld>) -> StressReport {
        StressReport {
            at: at(),
            scenarios: Vec::new(),
            modelled_positions: 0,
            unmodelled_positions: 0,
            decomposition: None,
            adversarial: None,
            causal: Vec::new(),
            causal_drivers: 0,
            causal_drivers_unconnected: 0,
            tail_worlds: worlds,
        }
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
        // Gross 400,000, plus 100,000 already held for approved proposals,
        // plus the 100,000 of hedge capacity the waterfall withheld so a hedge
        // can be held, plus the 750,000 released.
        assert_eq!(hedged.gross_ceiling, dec!("1350000"));
        assert!(hedged.released < open.released);
    }

    /// The reservation accounting `MaxSurvivalGross` needed before it could
    /// ship in the kernel's limit set: capital already held for an approved
    /// proposal is inside the ceiling, so the proposal's orders are not
    /// vetoed by the limit that sized them. Mutation: drop `self.held` from
    /// `LedgerFigures::standing` and the ceiling falls by the hold.
    #[test]
    fn capital_held_for_an_approved_proposal_stays_inside_the_ceiling_the_gate_reads() {
        let mut governor = SurvivalGovernor::default();
        let budget = governor
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap();
        assert_eq!(
            budget.released,
            dec!("850000"),
            "premise: the hold is not in what this construction released"
        );
        assert_eq!(
            governor.gross_ceiling(),
            Some(dec!("400000") + dec!("100000") + dec!("850000")),
            "the ceiling must stand behind the held proposal's orders as well as the release"
        );
    }

    /// Mutation: on refusal, leave `self.ceiling` untouched and the old,
    /// larger ceiling survives the refusal.
    #[test]
    fn a_refused_construction_lowers_the_ceiling_to_what_the_book_already_stands_in() {
        let mut governor = SurvivalGovernor::default();
        governor
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap();
        let before = governor.gross_ceiling().unwrap();
        assert!(
            before > dec!("500000"),
            "premise: a working ceiling above the standing gross"
        );
        // The ledger and the waterfall now disagree, so the construction is
        // refused: nothing new may be stood behind.
        assert!(
            governor
                .size(figures(dec!("900000")), None, 0.20, &views(), at())
                .is_err()
        );
        assert_eq!(governor.gross_ceiling(), Some(dec!("500000")));
        assert!(
            governor.last().is_some(),
            "the last successful budget is kept for the report"
        );
    }

    /// A construction refused outside the waterfall — the constructor or the
    /// reservation said no — leaves no release standing at the gate.
    /// Mutation: make `release_nothing` a no-op and the old ceiling survives.
    #[test]
    fn a_construction_refused_after_the_waterfall_leaves_no_release_standing() {
        let mut governor = SurvivalGovernor::default();
        governor
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap();
        assert_eq!(
            governor.gross_ceiling(),
            Some(dec!("1350000")),
            "premise: the release is in the ceiling"
        );
        governor.release_nothing(dec!("-400000"), dec!("100000"));
        assert_eq!(governor.gross_ceiling(), Some(dec!("500000")));
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
        assert_eq!(
            governor.gross_ceiling(),
            Some(dec!("500000")),
            "a refusal stands behind the gross held and nothing new"
        );
        assert!(governor.last().is_none(), "a refusal records no budget");
    }

    #[test]
    fn a_policy_that_would_release_withheld_capital_is_refused() {
        assert!(SurvivalPolicy::with_hedge_capacity_fraction(dec!("-0.1")).is_err());
        assert!(SurvivalPolicy::with_hedge_capacity_fraction(dec!("1")).is_err());
        assert!(SurvivalPolicy::with_hedge_capacity_fraction(dec!("0")).is_ok());
        assert!(SurvivalPolicy::new(dec!("0.6"), dec!("0.4")).is_err());
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

    /// R12-CAPITAL-4086/4087: margin, collateral and settlement come off the
    /// waterfall from the obligation source, the two derivations still
    /// reconcile, and an obligation the ledger's `deployable` did not
    /// subtract is not silently ignored. Mutation: drop `figures.margin` from
    /// the `.with_margin_required` call (pass zero) and the release does not
    /// fall.
    #[test]
    fn margin_collateral_and_settlement_are_withheld_from_the_ledger_and_reconciled() {
        let mut governor = SurvivalGovernor::default();
        let open = governor
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap();
        let ledger = Ledger {
            margin: dec!("100000"),
            collateral: dec!("20000"),
            settlement: dec!("30000"),
        };
        let charged = figures(dec!("850000")).with_obligations(&ledger).unwrap();
        let budget = governor.size(charged, None, 0.20, &views(), at()).unwrap();
        assert_eq!(budget.survival.margin_required, dec!("100000"));
        assert_eq!(budget.survival.collateral_posted, dec!("20000"));
        assert_eq!(budget.survival.settlement_required, dec!("30000"));
        assert_eq!(budget.released, open.released - dec!("150000"));
        assert_eq!(budget.free_cash, dec!("900000"));
        let lines = budget.narrowing().join("; ");
        for tier in ["margin", "encumbered collateral", "unsettled trades"] {
            assert!(lines.contains(tier), "{tier} missing from {lines}");
        }
        // A negative obligation is refused by name rather than released.
        let negative = Ledger {
            margin: dec!("-1"),
            collateral: Decimal::ZERO,
            settlement: Decimal::ZERO,
        };
        assert!(figures(dec!("850000")).with_obligations(&negative).is_err());
    }

    /// The emergency reserve is withheld before opportunity and reported as
    /// such. Mutation: pass `Decimal::ZERO` to `.with_nav_floor` and the
    /// release does not fall.
    #[test]
    fn the_emergency_reserve_is_withheld_before_opportunity() {
        let mut governor =
            SurvivalGovernor::with_policy(SurvivalPolicy::new(dec!("0"), dec!("0.05")).unwrap());
        let budget = governor
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap();
        assert_eq!(budget.survival.nav_floor, dec!("50000"));
        assert_eq!(budget.released, dec!("800000"));
        assert!(
            budget
                .narrowing()
                .iter()
                .any(|line| line.contains("emergency reserve"))
        );
    }

    /// R12-CAPITAL-4089's configuration half. Mutation: return `Ok(floor)`
    /// for a value below the floor and the second assertion fails.
    #[test]
    fn an_unset_setting_runs_the_shipped_reserves_and_a_lower_one_is_refused() {
        let unset = SurvivalPolicy::from_settings(None, None).unwrap();
        assert_eq!(unset, SurvivalPolicy::shipped());
        assert!(unset.hedge_capacity_fraction().is_positive());
        assert!(unset.emergency_reserve_fraction().is_positive());
        assert!(SurvivalPolicy::from_settings(Some("0.01"), None).is_err());
        assert!(SurvivalPolicy::from_settings(None, Some("0")).is_err());
        assert!(SurvivalPolicy::from_settings(Some("abc"), None).is_err());
        assert!(SurvivalPolicy::from_settings(Some("0.6"), Some("0.5")).is_err());
        let raised = SurvivalPolicy::from_settings(Some(" 0.10 "), Some("0.05")).unwrap();
        assert_eq!(raised.hedge_capacity_fraction(), dec!("0.1"));
    }

    /// R12-RISK-7205: the survival limit is installed once, on any set, and a
    /// set that already names something else `survival-gross` is refused.
    /// Mutation: return `Ok(limits)` unchanged from `install_survival_limit`
    /// and the first assertion fails.
    #[test]
    fn the_survival_limit_is_installed_once_and_a_name_collision_is_refused() {
        let shipped = LimitSet::conservative_default();
        assert!(
            !shipped
                .limits
                .iter()
                .any(|limit| matches!(limit.kind, LimitKind::MaxSurvivalGross { .. })),
            "premise: the shipped set does not carry the survival limit"
        );
        let installed = install_survival_limit(shipped.clone()).unwrap();
        assert_eq!(installed.len(), shipped.len() + 1);
        let again = install_survival_limit(installed.clone()).unwrap();
        assert_eq!(again.len(), installed.len(), "installed twice");
        let clash = LimitSet::new("clash").with(Limit::new(
            SURVIVAL_LIMIT_NAME,
            LimitKind::MaxLeverage { limit: 1.0 },
        ));
        assert!(install_survival_limit(clash).is_err());
    }

    /// A handed-in survival limit with any bound but the kernel's own is
    /// refused, naming it. Until round 3 any `MaxSurvivalGross` short-
    /// circuited installation, so a set carrying one bound at 1000 made the
    /// ceiling unable to fire.
    /// Mutation: restore the early return on any `MaxSurvivalGross` and the
    /// loose set is admitted.
    #[test]
    fn a_handed_in_survival_limit_with_another_bound_is_refused_rather_than_trusted() {
        let loose = LimitSet::new("handed-in").with(Limit::new(
            "operator-survival",
            LimitKind::MaxSurvivalGross { limit: 1000.0 },
        ));
        let error = install_survival_limit(loose).expect_err("a bound of 1000 cannot fire");
        assert!(
            error
                .message()
                .contains("`operator-survival` with a bound of 1000"),
            "the refusal names the limit and its bound: {}",
            error.message()
        );
        // Premise and contrast: the kernel's own bound under another name is
        // kept, and installed only once.
        let own = LimitSet::new("handed-in").with(Limit::new(
            "operator-survival",
            LimitKind::MaxSurvivalGross {
                limit: SURVIVAL_LIMIT_BOUND,
            },
        ));
        let kept = install_survival_limit(own.clone()).unwrap();
        assert_eq!(kept.len(), own.len());
    }

    /// The limit the kernel installs vetoes an order that carries gross past
    /// the ceiling the governor computed, and admits one inside it — so a
    /// survival kernel that releases less is a gate that admits less.
    /// Mutation: write `None` for the ceiling and both orders are refused;
    /// remove the installed limit and the large one passes.
    #[test]
    fn an_order_past_the_survival_ceiling_is_vetoed_by_the_installed_limit() {
        let mut governor = SurvivalGovernor::default();
        governor
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap();
        let ceiling = governor.gross_ceiling().unwrap();
        let limits = install_survival_limit(LimitSet::new("survival-only")).unwrap();
        let projected = |gross: Decimal| qip_risk::limits::RiskState {
            equity: dec!("1000000"),
            gross_exposure: gross,
            gross_before_order: Some(dec!("400000")),
            survival_gross_ceiling: Some(ceiling),
            ..qip_risk::limits::RiskState::default()
        };
        assert!(
            !limits.check(&projected(ceiling)).is_blocked(),
            "an order to the ceiling is admitted"
        );
        assert!(
            limits.check(&projected(ceiling + dec!("1"))).is_blocked(),
            "an order past the ceiling is vetoed"
        );
    }

    /// R12-RISK-4116..4120, 4123, 4124: each §14.3 world, on a book that does
    /// not survive it, withholds a stressed reserve named for that world and
    /// shrinks what is released — the recorded capital response. Mutation:
    /// remove any one world from `tail::survival_worlds` and the `expect`
    /// for that world fails; make `tail_response` return `Ok(None)` and the
    /// reserve assertion fails.
    #[test]
    fn every_survival_world_the_book_fails_withholds_a_reserve_named_for_it() {
        // A levered, volatile book with no ladder marks: it fails every
        // world at a 2% tolerance, which is what makes each response
        // observable.
        let history = std::collections::BTreeMap::from([
            (
                "obj-AAA".to_string(),
                vec![100.0, 108.0, 95.0, 104.0, 90.0, 99.0, 112.0, 97.0],
            ),
            (
                "obj-BBB".to_string(),
                vec![50.0, 52.0, 47.0, 51.0, 46.0, 49.0, 55.0, 48.0],
            ),
        ]);
        let positions = vec![
            crate::tail::TailPosition {
                object: "obj-AAA".into(),
                notional: 900_000.0,
                days_to_liquidate: None,
            },
            crate::tail::TailPosition {
                object: "obj-BBB".into(),
                notional: -700_000.0,
                days_to_liquidate: Some(3.0),
            },
        ];
        let generated = crate::tail::survival_worlds(
            &crate::tail::TailBook {
                positions: &positions,
                price_history: &history,
                equity: 1_000_000.0,
                seed: 42,
            },
            at(),
        );
        let ledger = LedgerFigures {
            equity: dec!("1000000"),
            held: Decimal::ZERO,
            obligations: Decimal::ZERO,
            deployable: dec!("1000000"),
            gross: dec!("1600000"),
            ..LedgerFigures::default()
        };
        let mut calm = SurvivalGovernor::default();
        let unstressed = calm.size(ledger, None, 0.02, &views(), at()).unwrap();
        let mut names: Vec<&str> = vec![
            crate::tail::LIQUIDITY_FREEZE_DEPTH,
            crate::tail::VENUE_OUTAGE,
            crate::tail::GAP_MOVE,
            crate::tail::CORRELATION_INVERSION,
            crate::tail::DATA_POISONING,
            crate::tail::MULTI_MARKET_SHOCK,
        ];
        names.extend(crate::tail::SYNTHETIC_WORLDS);
        for name in names {
            let world = generated
                .iter()
                .find(|world| world.scenario == name)
                .unwrap_or_else(|| panic!("the {name} world was not generated"))
                .clone();
            assert!(
                world.loss_fraction > 0.02,
                "premise: the book fails {name} at the tolerance; it lost {}",
                world.loss_fraction
            );
            let stress = stress_with(vec![world]);
            let mut governor = SurvivalGovernor::default();
            let Ok(budget) = governor.size(ledger, Some(&stress), 0.02, &views(), at()) else {
                // A world so severe the reserve exceeds the book: the
                // response is the refusal, and the ceiling falls to the gross
                // already held.
                assert_eq!(governor.gross_ceiling(), Some(dec!("1600000")), "{name}");
                continue;
            };
            let tail = budget
                .tail
                .as_ref()
                .expect("a stressed book has a response");
            assert_eq!(tail.scenario, name);
            assert!(!tail.survives(), "{name}");
            assert!(budget.survival.stressed_liquidity_reserve.is_positive());
            assert!(budget.released < unstressed.released, "{name}");
            assert!(
                budget.narrowing().iter().any(|line| line.contains(name)),
                "{name} is not on the DECIDE line"
            );
        }
    }
}
