//! Cashflow forecasting, commitments and capital calls — blueprint §16.4.
//!
//! Every other position in this platform can be exited at some price. A
//! private commitment cannot be exited at all for years, and it can demand
//! more capital on somebody else's schedule. That asymmetry is why this module
//! exists: an unfunded commitment is a liability, and capital that might be
//! called next quarter is not capital that may be deployed this quarter.
//!
//! Three refusals carry the weight, and each one names a way the platform has
//! been able to lie to itself about capital:
//!
//! * **A flow dated before its origin is refused.** A fund cannot call capital
//!   before it existed, and a schedule that contains such a flow is a data
//!   error that would otherwise be discounted, weighted and summed into a
//!   reserve figure nobody could reproduce.
//! * **A forecast is refused at an instant it was not knowable.** Bitemporality
//!   is not decoration here. A call schedule published in March, applied to a
//!   January valuation, produces a backtest in which the platform reserved
//!   against a demand it had not yet been told about — and every number
//!   downstream of it is then better than reality.
//! * **A scheduled call larger than the unfunded balance is refused.** You
//!   cannot be called for more than you committed. Silently capping it would
//!   turn a corrupt record into a plausible one.
//!
//! Money is [`Decimal`] throughout. Probabilities and discount rates are
//! `f64`, because they are statistics rather than amounts; every point where
//! the two meet is marked in a comment at the multiplication.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::extensions::PrivateAssetDetails;

/// What a forecast flow is, and therefore which way the capital moves.
///
/// The direction belongs to the kind rather than to the sign of the amount, so
/// a caller cannot express a negative capital call — an amount that reads as
/// money coming *in* from an event that takes money *out*.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CashflowKind {
    /// Capital demanded by a fund on its own schedule.
    CapitalCall,
    /// Management or performance fee drawn against the commitment.
    Fee,
    /// Capital and gains returned.
    Distribution,
    /// A contractual interest payment.
    Coupon,
    /// Return of principal at maturity.
    Principal,
}

impl CashflowKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::CapitalCall => "capital_call",
            Self::Fee => "fee",
            Self::Distribution => "distribution",
            Self::Coupon => "coupon",
            Self::Principal => "principal",
        }
    }

    /// Whether this kind takes capital out of the book.
    pub const fn is_outflow(self) -> bool {
        matches!(self, Self::CapitalCall | Self::Fee)
    }
}

/// One dated, probability-weighted flow.
///
/// Fields are private so that the invariants the constructor establishes —
/// positive magnitude, a probability that is a probability — cannot be undone
/// by a caller assigning to a field after the fact.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForecastCashflow {
    kind: CashflowKind,
    due_at: Timestamp,
    /// Magnitude only, always strictly positive. Direction is [`Self::kind`]'s.
    amount: Decimal,
    /// Probability the flow occurs at all. A statistic, never money.
    probability: f64,
}

impl ForecastCashflow {
    /// Refuses a non-positive amount and a probability that is not one.
    ///
    /// A zero-amount flow is refused rather than dropped: a schedule that
    /// quietly discards entries reports a smaller obligation than the record
    /// it was built from, and the two disagree with nothing to say which is
    /// right.
    pub fn new(
        kind: CashflowKind,
        due_at: Timestamp,
        amount: Decimal,
        probability: f64,
    ) -> Result<Self> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "a {} flow of {amount} is not a flow; supply a strictly positive magnitude, and \
                 omit the entry rather than scheduling a zero",
                kind.label()
            )));
        }
        if !probability.is_finite() || probability <= 0.0 || probability > 1.0 {
            return Err(Error::invalid(format!(
                "a {} flow needs a probability in (0, 1]; {probability} is not one — omit the \
                 entry rather than scheduling it as impossible",
                kind.label()
            )));
        }
        Ok(Self {
            kind,
            due_at,
            amount,
            probability,
        })
    }

    pub const fn kind(&self) -> CashflowKind {
        self.kind
    }

    pub const fn due_at(&self) -> Timestamp {
        self.due_at
    }

    pub const fn amount(&self) -> Decimal {
        self.amount
    }

    pub const fn probability(&self) -> f64 {
        self.probability
    }

    /// The amount signed by direction: negative where capital leaves.
    pub fn signed(&self) -> Decimal {
        if self.kind.is_outflow() {
            Decimal::ZERO - self.amount
        } else {
            self.amount
        }
    }

    /// The signed amount weighted by its probability.
    ///
    /// **Statistic meets money here.** `probability` is `f64`; `amount` is
    /// [`Decimal`]. The probability is converted to `Decimal` and the product
    /// taken in `Decimal`, so the result is exact at the module's scale rather
    /// than carrying binary-float residue into a reserve figure. A probability
    /// that cannot be represented is refused rather than rounded.
    pub fn expected_signed(&self) -> Result<Decimal> {
        let weight = Decimal::from_f64(self.probability).ok_or_else(|| {
            Error::numeric(format!(
                "probability {} cannot be represented as a decimal weight; supply a probability \
                 with at most nine decimal places",
                self.probability
            ))
        })?;
        self.signed().checked_mul(weight).ok_or_else(|| {
            Error::numeric(format!(
                "weighting {} by {} overflows; nothing is reserved against a number that cannot \
                 be represented",
                self.amount, self.probability
            ))
        })
    }
}

/// A dated stream of forecast flows for one subject.
///
/// Keyed by `(due_at, kind)` in a [`BTreeMap`] so iteration order is the
/// schedule's own order on every run: a present value that reorders its
/// summands is not a replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CashflowForecast {
    subject: String,
    /// When the instrument this forecasts came into existence.
    origin: Timestamp,
    /// When this forecast became knowable to the platform.
    known_at: Timestamp,
    flows: BTreeMap<(Timestamp, CashflowKind), ForecastCashflow>,
}

impl CashflowForecast {
    /// An empty forecast for `subject`.
    ///
    /// Refuses an empty subject: a forecast nobody can attribute to an
    /// instrument cannot be reconciled against the instrument's own record.
    pub fn new(subject: impl Into<String>, origin: Timestamp, known_at: Timestamp) -> Result<Self> {
        let subject = subject.into();
        if subject.trim().is_empty() {
            return Err(Error::invalid(
                "a cashflow forecast needs the object id it forecasts; supply one rather than an \
                 empty subject",
            ));
        }
        Ok(Self {
            subject,
            origin,
            known_at,
            flows: BTreeMap::new(),
        })
    }

    /// Add a flow, refusing one dated before the subject's origin and one that
    /// collides with a flow already scheduled.
    ///
    /// The collision is refused rather than summed. Two records claiming a
    /// call of a different size on the same day for the same fund disagree,
    /// and adding them together produces a third number neither source
    /// asserted.
    pub fn with_flow(mut self, flow: ForecastCashflow) -> Result<Self> {
        if flow.due_at() < self.origin {
            return Err(Error::invalid(format!(
                "a {} flow for {} is dated {} , before the subject's origin {}; correct the date \
                 or the origin — a flow cannot precede the thing that produces it",
                flow.kind().label(),
                self.subject,
                flow.due_at().to_rfc3339(),
                self.origin.to_rfc3339()
            )));
        }
        let key = (flow.due_at(), flow.kind());
        if self.flows.contains_key(&key) {
            return Err(Error::invalid(format!(
                "{} already has a {} flow due {}; combine the two into one entry rather than \
                 scheduling a second, because summing them asserts a total neither source claimed",
                self.subject,
                flow.kind().label(),
                flow.due_at().to_rfc3339()
            )));
        }
        self.flows.insert(key, flow);
        Ok(self)
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub const fn origin(&self) -> Timestamp {
        self.origin
    }

    pub const fn known_at(&self) -> Timestamp {
        self.known_at
    }

    pub fn is_empty(&self) -> bool {
        self.flows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.flows.len()
    }

    /// The flows in schedule order.
    pub fn flows(&self) -> impl Iterator<Item = &ForecastCashflow> {
        self.flows.values()
    }

    /// Whether this forecast was knowable at `as_of`.
    pub fn is_knowable_at(&self, as_of: Timestamp) -> bool {
        self.known_at <= as_of
    }

    /// Refuse to be read at an instant before this forecast was knowable.
    ///
    /// The point-in-time defect this prevents has a shape: a call schedule
    /// published after a valuation date, used at that valuation date, makes
    /// the platform look as though it reserved against a demand it had not
    /// been told about. Every backtest downstream is then better than reality
    /// by exactly the information it should not have had.
    pub fn guard_knowable(&self, as_of: Timestamp) -> Result<()> {
        if self.is_knowable_at(as_of) {
            return Ok(());
        }
        Err(Error::invalid(format!(
            "the cashflow forecast for {} became knowable at {} and cannot be read as of {}; \
             rebuild it from records known at or before {}",
            self.subject,
            self.known_at.to_rfc3339(),
            as_of.to_rfc3339(),
            as_of.to_rfc3339()
        )))
    }

    /// Probability-weighted net flow in `[from, to]`, signed by direction.
    pub fn expected_between(
        &self,
        as_of: Timestamp,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<Decimal> {
        self.guard_knowable(as_of)?;
        if from > to {
            return Err(Error::invalid(format!(
                "the window {} to {} runs backwards; supply the earlier instant first",
                from.to_rfc3339(),
                to.to_rfc3339()
            )));
        }
        let mut total = Decimal::ZERO;
        for flow in self.flows.values() {
            if flow.due_at() < from || flow.due_at() > to {
                continue;
            }
            total = total
                .checked_add(flow.expected_signed()?)
                .ok_or_else(|| Error::numeric("the expected net flow overflows".to_string()))?;
        }
        Ok(total)
    }

    /// Probability-weighted capital demanded within `horizon` of `as_of`, as a
    /// positive figure. Distributions do not offset it: capital returning next
    /// year cannot pay a call due next month.
    pub fn expected_demand_within(&self, as_of: Timestamp, horizon: Duration) -> Result<Decimal> {
        self.guard_knowable(as_of)?;
        if horizon.as_nanos() < 0 {
            return Err(Error::invalid(format!(
                "a demand horizon of {} nanoseconds runs backwards; supply a non-negative horizon",
                horizon.as_nanos()
            )));
        }
        let until = as_of.saturating_add(horizon);
        let mut total = Decimal::ZERO;
        for flow in self.flows.values() {
            if !flow.kind().is_outflow() || flow.due_at() < as_of || flow.due_at() > until {
                continue;
            }
            let weighted = flow.expected_signed()?;
            total = total
                .checked_sub(weighted)
                .ok_or_else(|| Error::numeric("the expected demand overflows".to_string()))?;
        }
        Ok(total)
    }

    /// The first instant at or after `as_of` at which this forecast returns
    /// capital, or `None` where it schedules none.
    ///
    /// The question a liquidity read has to ask of a private holding, and the
    /// one an unfunded balance cannot answer. A commitment is an amount; it
    /// says nothing about *when* the position turns into cash. The only other
    /// figure anything here holds for that is the catalogue's stated days to
    /// liquidate, which is a vendor's estimate of a market, whereas the
    /// lockup this schedule is dated from is the contract that binds. Where
    /// the two disagree the contract is the one a book has to live with.
    ///
    /// **An outflow is never an answer.** A capital call is the position
    /// taking cash, and a read that let one satisfy "when does this become
    /// cash" would report a fund at its deepest draw as its most liquid — the
    /// J-curve read upside down.
    ///
    /// `None` rather than a refusal where nothing is scheduled ahead: a
    /// schedule that returns nothing inside its own horizon is a fact about
    /// the schedule, and a caller holding another bound should keep using it
    /// rather than be told the read failed. Refusing here would make the
    /// commoner case — a holding out of its lockup — indistinguishable from a
    /// broken record.
    pub fn first_return_at(&self, as_of: Timestamp) -> Result<Option<Timestamp>> {
        self.guard_knowable(as_of)?;
        Ok(self
            .flows
            .values()
            .filter(|flow| !flow.kind().is_outflow() && flow.due_at() >= as_of)
            .map(ForecastCashflow::due_at)
            .min())
    }

    /// A forecast holding only the flows still ahead at `as_of`.
    ///
    /// The companion to [`Self::present_value`], which refuses a settled flow
    /// rather than skipping it.
    pub fn remaining_at(&self, as_of: Timestamp) -> Result<Self> {
        self.guard_knowable(as_of)?;
        let flows = self
            .flows
            .iter()
            .filter(|((due, _), _)| *due >= as_of)
            .map(|(k, v)| (*k, *v))
            .collect();
        Ok(Self {
            subject: self.subject.clone(),
            origin: self.origin,
            known_at: self.known_at,
            flows,
        })
    }

    /// Present value at `as_of`, discounting each flow at `annual_rate`.
    ///
    /// Refuses rather than guesses in three ways. A flow already due is
    /// refused, not skipped — a present value that silently drops settled
    /// flows reports a different number from the schedule it names, and
    /// [`Self::remaining_at`] exists so the caller says out loud that it meant
    /// to drop them. A rate at or below −100% is refused, because the discount
    /// factor it implies is zero or negative and a negative present value of a
    /// positive inflow is not a valuation, it is a sign error wearing one. An
    /// empty forecast is refused, because a present value of zero derived from
    /// no flows reads identically to a genuine zero.
    pub fn present_value(&self, as_of: Timestamp, annual_rate: f64) -> Result<Decimal> {
        self.guard_knowable(as_of)?;
        if self.flows.is_empty() {
            return Err(Error::invalid(format!(
                "the cashflow forecast for {} holds no flows, so its present value would be a \
                 zero nobody computed; schedule the flows before discounting them",
                self.subject
            )));
        }
        if !annual_rate.is_finite() || annual_rate <= -1.0 {
            return Err(Error::invalid(format!(
                "a discount rate of {annual_rate} implies a discount factor that is not positive; \
                 supply a rate above -1.0"
            )));
        }
        let mut total = Decimal::ZERO;
        for flow in self.flows.values() {
            if flow.due_at() < as_of {
                return Err(Error::invalid(format!(
                    "{} has a {} flow due {}, before the valuation instant {}; call \
                     `remaining_at` first if the settled flows are meant to be excluded",
                    self.subject,
                    flow.kind().label(),
                    flow.due_at().to_rfc3339(),
                    as_of.to_rfc3339()
                )));
            }
            // Statistic meets money here. The discount factor is arithmetic on
            // a rate and a year fraction — both `f64` — and is computed in
            // `f64`. It crosses into `Decimal` once, at this conversion, and
            // every multiplication after it is exact decimal arithmetic. The
            // alternative, carrying the amount into `f64`, loses cents on a
            // nine-figure commitment.
            let years = flow.due_at().since(as_of).as_years_f64();
            let factor = (1.0 + annual_rate).powf(years);
            if !factor.is_finite() || factor <= 0.0 {
                return Err(Error::numeric(format!(
                    "discounting {} over {years:.4} years at {annual_rate} produced the factor \
                     {factor}, which is not a usable discount factor",
                    flow.amount()
                )));
            }
            let discount = Decimal::from_f64(1.0 / factor).ok_or_else(|| {
                Error::numeric(format!(
                    "the discount factor {} cannot be represented at decimal scale",
                    1.0 / factor
                ))
            })?;
            let discounted = flow
                .expected_signed()?
                .checked_mul(discount)
                .ok_or_else(|| Error::numeric("discounting overflows".to_string()))?;
            total = total
                .checked_add(discounted)
                .ok_or_else(|| Error::numeric("the present value overflows".to_string()))?;
        }
        Ok(total)
    }

    /// The deepest point of cumulative net cashflow, and when it is reached.
    ///
    /// This is the J-curve made explicit (blueprint §16.4): fees and calls
    /// come first and value accrues later, so an early mark read without the
    /// trough in front of it looks like a loss rather than a schedule. Returns
    /// `None` for a forecast that never goes negative.
    pub fn j_curve_trough(&self, as_of: Timestamp) -> Result<Option<(Timestamp, Decimal)>> {
        self.guard_knowable(as_of)?;
        let mut running = Decimal::ZERO;
        let mut trough: Option<(Timestamp, Decimal)> = None;
        for flow in self.flows.values() {
            running = running
                .checked_add(flow.expected_signed()?)
                .ok_or_else(|| Error::numeric("the cumulative net flow overflows".to_string()))?;
            if running.is_negative() && trough.is_none_or(|(_, low)| running < low) {
                trough = Some((flow.due_at(), running));
            }
        }
        Ok(trough)
    }
}

/// What the book suffers if a call is not met by the date it fell due.
///
/// Blueprint §43.2 names the consequence of failure as part of the object, not
/// as commentary on it, and the reason is that a book short of cash has to
/// rank a capital call against everything else it owes. A demand with no
/// priced consequence ranks last by default, which is the wrong answer: a
/// missed drawdown on a private commitment is the one payment failure that
/// can cost more than the payment.
///
/// The arms are three rather than one because they behave differently in
/// time. Interest grows with lateness; forfeiture is taken once and does not;
/// acceleration costs no extra principal at all and instead moves every
/// remaining call to today. A single "penalty" number could not express the
/// third, and the third is the one that breaks a liquidity plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallConsequence {
    /// Default interest accrues on the unmet amount at an annual rate, in
    /// basis points, for every day past the due date.
    Interest {
        /// Annual rate in basis points. An integer, so the whole penalty
        /// computation stays in [`Decimal`].
        annual_rate_bps: u32,
    },
    /// A fraction of the unmet amount, in basis points, is forfeited the
    /// moment the call is missed. Taken once; it does not grow with lateness.
    Forfeiture {
        /// Fraction forfeited, in basis points of the unmet amount.
        fraction_bps: u32,
    },
    /// The whole unfunded balance falls due at once.
    ///
    /// Costs no additional principal — the balance was already owed — so its
    /// penalty is zero and its effect is entirely on *when* the capital is
    /// demanded. [`Commitment::demand_within`] is where that is read.
    Acceleration,
}

/// One basis point is a ten-thousandth.
const BPS: i64 = 10_000;

/// Basis points times the day count of a year, so an annual rate quoted in
/// basis points becomes a daily one in a single division.
const BPS_YEAR: i64 = BPS * 365;

/// Nanoseconds in a day, for turning a [`Duration`] of lateness into whole
/// days in integer arithmetic.
///
/// [`Duration::as_days_f64`] exists and is not used: it would put a day count
/// feeding a money computation through `f64`, and a penalty that disagrees
/// with the fund's own notice in the last cent is a reconciliation nobody can
/// close.
const NANOS_PER_DAY: i64 = 86_400 * 1_000_000_000;

impl CallConsequence {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Interest { .. } => "interest",
            Self::Forfeiture { .. } => "forfeiture",
            Self::Acceleration => "acceleration",
        }
    }

    /// The capital this consequence costs on top of the amount already owed.
    ///
    /// **Money never leaves [`Decimal`] here, and that is deliberate.** The
    /// module's rule is that probabilities and discount rates are `f64` and
    /// the crossing point is marked where it happens; this computation has no
    /// crossing point, because a rate in basis points and a count of days are
    /// both integers. [`Decimal::apply_bps`] would have been the obvious call
    /// and it takes an `f64` rate — using it would have put a binary rounding
    /// error into a default-interest figure somebody has to reconcile against
    /// a fund's own notice, which is exactly the class of disagreement this
    /// platform refuses to create.
    ///
    /// A 365-day year is asserted rather than derived. Partnership agreements
    /// differ and some count 360; this returns what the platform believes it
    /// owes, and the fund's notice remains the record.
    pub fn penalty_on(self, unmet: Decimal, days_late: i64) -> Result<Decimal> {
        if unmet.is_negative() {
            return Err(Error::invalid(format!(
                "a default penalty was asked for on an unmet amount of {unmet}; pass the \
                 outstanding amount as a non-negative figure, because a negative unmet balance is \
                 a call that was overpaid and has no penalty to compute"
            )));
        }
        if days_late < 0 {
            return Err(Error::invalid(format!(
                "a default penalty was asked for {days_late} days late; a call that has not yet \
                 fallen due carries no penalty, so ask at or after its due date"
            )));
        }
        match self {
            Self::Interest { annual_rate_bps } => unmet
                .checked_mul(Decimal::from_int(i64::from(annual_rate_bps)))
                .and_then(|accrued| accrued.checked_mul(Decimal::from_int(days_late)))
                .and_then(|accrued| accrued.checked_div(Decimal::from_int(BPS_YEAR)))
                .ok_or_else(|| {
                    Error::numeric(format!(
                        "default interest on {unmet} at {annual_rate_bps}bp over {days_late} \
                         day(s) overflows; nothing is reserved against a penalty that cannot be \
                         represented"
                    ))
                }),
            Self::Forfeiture { fraction_bps } => unmet
                .checked_mul(Decimal::from_int(i64::from(fraction_bps)))
                .and_then(|taken| taken.checked_div(Decimal::from_int(BPS)))
                .ok_or_else(|| {
                    Error::numeric(format!(
                        "a forfeiture of {fraction_bps}bp on {unmet} overflows; nothing is \
                         reserved against a penalty that cannot be represented"
                    ))
                }),
            Self::Acceleration => Ok(Decimal::ZERO),
        }
    }
}

/// A drawdown demand: a date, an amount, and what failing it costs.
///
/// Blueprint §43.2's tenth object, and the one the platform was missing. A
/// [`CashflowKind::CapitalCall`] variant already existed and is not this: that
/// is a *probability-weighted projection* of a call somebody might make, which
/// is the right shape for pacing and the wrong shape for a notice that has
/// actually arrived. A forecast flow can be discounted. An issued notice
/// cannot — the money is due on the stated date or the consequence follows.
///
/// Both instants are kept because they are different facts. `issued` is when
/// the notice became knowable to this platform, and reading a call before it
/// is point-in-time leakage of the kind this module's header names second;
/// `due` is when the capital has to be there. A backtest that saw the notice
/// on the due date rather than the day it arrived would report a liquidity
/// squeeze the desk would in fact have had two weeks to prepare for, and a
/// backtest that saw it early would report one it never had.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapitalCall {
    subject: String,
    reference: String,
    amount: Decimal,
    issued: Timestamp,
    due: Timestamp,
    consequence: CallConsequence,
}

impl CapitalCall {
    /// Record a notice, refusing one that cannot be true.
    ///
    /// Refuses an empty subject or reference, a non-positive amount, and a
    /// call falling due before the notice that demanded it was knowable. The
    /// last is the one worth stating: a call due before it was issued is not
    /// a tight deadline, it is a record whose two dates came from different
    /// places, and treating it as merely overdue would accrue default
    /// interest from an instant nobody was told about.
    ///
    /// The reference is the fund's own notice identifier. It is required
    /// rather than generated because two calls for the same amount on the
    /// same date are ordinary, and a book that cannot tell them apart will
    /// silently hold only one.
    pub fn notice(
        subject: impl Into<String>,
        reference: impl Into<String>,
        amount: Decimal,
        issued: Timestamp,
        due: Timestamp,
        consequence: CallConsequence,
    ) -> Result<Self> {
        let subject = subject.into();
        let reference = reference.into();
        if subject.trim().is_empty() {
            return Err(Error::invalid(
                "a capital call needs the object id it draws against; supply one rather than an \
                 empty subject",
            ));
        }
        if reference.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the call against {subject} carries no notice reference; supply the fund's own \
                 identifier, because two calls for the same amount on the same date are ordinary \
                 and a book that cannot tell them apart will hold only one"
            )));
        }
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "notice {reference} against {subject} demands {amount}, which is not a drawdown; \
                 record a strictly positive amount, or record a distribution in the forecast if \
                 capital is being returned"
            )));
        }
        if due < issued {
            return Err(Error::invalid(format!(
                "notice {reference} against {subject} falls due at {} but became knowable at {}; \
                 correct the dates rather than recording a call that was already late when it \
                 arrived, because default interest would accrue from an instant nobody was told \
                 about",
                due.to_rfc3339(),
                issued.to_rfc3339()
            )));
        }
        Ok(Self {
            subject,
            reference,
            amount,
            issued,
            due,
            consequence,
        })
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn reference(&self) -> &str {
        &self.reference
    }

    pub const fn amount(&self) -> Decimal {
        self.amount
    }

    /// The instant the notice became knowable to this platform.
    pub const fn issued_at(&self) -> Timestamp {
        self.issued
    }

    /// The instant the capital has to be there.
    pub const fn due_at(&self) -> Timestamp {
        self.due
    }

    pub const fn consequence(&self) -> CallConsequence {
        self.consequence
    }

    /// Whether this notice had reached the platform by `as_of`.
    pub fn is_knowable_at(&self, as_of: Timestamp) -> bool {
        self.issued <= as_of
    }

    /// Refuse to be read at an instant before the notice arrived.
    pub fn guard_knowable(&self, as_of: Timestamp) -> Result<()> {
        if self.is_knowable_at(as_of) {
            return Ok(());
        }
        Err(Error::invalid(format!(
            "notice {} against {} became knowable at {} and cannot be read as of {}; a book \
             cannot reserve against a demand it had not yet been told about",
            self.reference,
            self.subject,
            self.issued.to_rfc3339(),
            as_of.to_rfc3339()
        )))
    }

    /// Whether the money was due and this notice was knowable, both by
    /// `as_of`.
    ///
    /// Both halves are load-bearing. A call issued tomorrow and due tomorrow
    /// is not overdue today however the dates sort, and asking only whether
    /// the due date has passed would make a notice recorded today with a past
    /// due date accrue interest for a period the platform was never told
    /// about.
    pub fn is_overdue_at(&self, as_of: Timestamp) -> bool {
        self.is_knowable_at(as_of) && self.due < as_of
    }

    /// Whole days between the due date and `as_of`, or zero where the call is
    /// not overdue.
    ///
    /// Whole days, floored, because default interest is quoted per day and a
    /// part-day is not a day. Rounding up would bill the book for lateness it
    /// does not have.
    pub fn days_late_at(&self, as_of: Timestamp) -> i64 {
        if !self.is_overdue_at(as_of) {
            return 0;
        }
        as_of.since(self.due).as_nanos() / NANOS_PER_DAY
    }

    /// What failing this call has cost by `as_of`, on top of the amount owed.
    ///
    /// Zero until the call is both knowable and past due, so a notice sitting
    /// in the book ahead of its date costs nothing and a notice nobody has
    /// sent yet costs nothing.
    pub fn penalty_at(&self, as_of: Timestamp) -> Result<Decimal> {
        if !self.is_overdue_at(as_of) {
            return Ok(Decimal::ZERO);
        }
        self.consequence
            .penalty_on(self.amount, self.days_late_at(as_of))
    }

    pub fn describe(&self) -> String {
        format!(
            "{} calls {} on {} under notice {} ({} on default)",
            self.subject,
            self.amount,
            self.due.to_date_string(),
            self.reference,
            self.consequence.label()
        )
    }
}
/// A promise of capital, mostly unfunded, that somebody else may call.
///
/// The unfunded balance is a hard obligation whether or not a schedule for it
/// exists, which is why [`Self::unscheduled`] is a first-class constructor
/// rather than a degraded one. Failing a capital call typically forfeits the
/// position; a reserve that waits for a pacing model is a reserve that is not
/// there when the notice arrives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Commitment {
    subject: String,
    committed: Decimal,
    called: Decimal,
    origin: Timestamp,
    known_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    forecast: Option<CashflowForecast>,
    /// Issued notices, keyed by the fund's own reference.
    ///
    /// A `BTreeMap` rather than a `HashMap` because these reach output — a
    /// describe, a penalty total, a demand figure summed in iteration order —
    /// and a replay that reorders is not a replay.
    ///
    /// `serde(default)` so a commitment written before notices existed still
    /// reads back, and the field is skipped when empty so those payloads do
    /// not change shape. Deserialisation does not route through
    /// [`Commitment::record_call`], so a payload could carry notices whose
    /// sum exceeds the unfunded balance. That direction is the safe one and
    /// is the reason it is tolerated rather than an oversight: notices only
    /// ever *raise* [`Commitment::obligation`], and a raised obligation makes
    /// the platform hold more capital back, never less.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    calls: BTreeMap<String, CapitalCall>,
}

impl Commitment {
    /// A commitment whose call timing is not yet modelled.
    ///
    /// Refuses a non-positive commitment, a negative called balance, and a
    /// called balance exceeding the commitment — the last because you cannot
    /// be called for more than you promised, and capping it silently would
    /// turn a corrupt record into a plausible one.
    pub fn unscheduled(
        subject: impl Into<String>,
        committed: Decimal,
        called: Decimal,
        origin: Timestamp,
        known_at: Timestamp,
    ) -> Result<Self> {
        let subject = subject.into();
        if subject.trim().is_empty() {
            return Err(Error::invalid(
                "a commitment needs the object id it obliges the book to; supply one rather than \
                 an empty subject",
            ));
        }
        if !committed.is_positive() {
            return Err(Error::invalid(format!(
                "{subject} commits {committed}, which is not a commitment; record a strictly \
                 positive amount or do not record the commitment"
            )));
        }
        if called.is_negative() {
            return Err(Error::invalid(format!(
                "{subject} reports called capital of {called}; a negative call is a distribution \
                 and belongs in the forecast, not in the called balance"
            )));
        }
        if called > committed {
            return Err(Error::invalid(format!(
                "{subject} reports {called} called against a commitment of {committed}; correct \
                 the record — a fund cannot call more than was promised, and the excess will not \
                 be capped here"
            )));
        }
        if known_at < origin {
            return Err(Error::invalid(format!(
                "{subject} became knowable at {} , before its origin {}; a commitment cannot be \
                 known before it was made",
                known_at.to_rfc3339(),
                origin.to_rfc3339()
            )));
        }
        Ok(Self {
            subject,
            committed,
            called,
            origin,
            known_at,
            forecast: None,
            calls: BTreeMap::new(),
        })
    }

    /// Attach a call and distribution schedule.
    ///
    /// Refuses a forecast about a different subject, a forecast with a
    /// different origin, a schedule whose unweighted calls and fees exceed the
    /// unfunded balance, and a schedule holding no call at all while capital
    /// remains unfunded.
    ///
    /// The third is the consistency check that matters in the obvious
    /// direction: a pacing model that schedules more than was ever promised is
    /// wrong, and a reserve computed from it would be a number with no
    /// counterpart in the contract.
    ///
    /// # The fourth refusal, and the forecast this platform actually builds
    ///
    /// Attaching a forecast changes what [`Self::demand_within`] projects from
    /// the whole unfunded balance to the schedule's own weighted calls. That
    /// is the point of a pacing model — a fund that calls nothing for two
    /// years demands nothing inside a month, and saying so is the reason to
    /// hold a schedule at all. But a forecast holding **no call at all** is
    /// not a pacing model for calls; it is the absence of one, and reading its
    /// silence as "no call, ever" collapses the projection to zero for a
    /// commitment that is still wholly unfunded.
    ///
    /// This is not hypothetical, and it is one call site away. The only
    /// [`CashflowForecast`] this platform constructs outside tests is
    /// [`crate::valuation::IlliquidValuator::forecast_private_asset`], which
    /// `qip-kernel`'s `private_holdings_of` builds for every private asset in
    /// the universe — and it holds exactly one flow, a
    /// [`CashflowKind::Distribution`] of the residual value at the end of the
    /// lockup. Attaching that forecast to that asset's own commitment would
    /// pass the subject, origin and over-schedule checks and take the reserve
    /// to nothing. The sweep keeps the two apart today; this makes the type,
    /// rather than that sweep, the thing that keeps them apart.
    ///
    /// Refusing rather than repairing, because both repairs are worse: adding
    /// the unfunded balance as a call the record does not state invents a
    /// demand, and reading the forecast for distributions while ignoring it
    /// for calls makes one attached object mean two things. The module's own
    /// first-class answer is already the right one — leave the commitment
    /// [`Self::unscheduled`], which reserves the balance whole — so the
    /// refusal names it.
    pub fn with_forecast(mut self, forecast: CashflowForecast) -> Result<Self> {
        if forecast.subject() != self.subject {
            return Err(Error::invalid(format!(
                "a forecast for {} cannot be attached to the commitment for {}; attach the \
                 forecast to its own subject",
                forecast.subject(),
                self.subject
            )));
        }
        if forecast.origin() != self.origin {
            return Err(Error::invalid(format!(
                "the forecast for {} originates {} but the commitment originates {}; reconcile \
                 the two rather than discounting against a date neither record holds",
                self.subject,
                forecast.origin().to_rfc3339(),
                self.origin.to_rfc3339()
            )));
        }
        let mut scheduled = Decimal::ZERO;
        let mut calls = 0usize;
        for flow in forecast.flows() {
            if flow.kind().is_outflow() {
                calls += 1;
                scheduled = scheduled.checked_add(flow.amount()).ok_or_else(|| {
                    Error::numeric("the scheduled call total overflows".to_string())
                })?;
            }
        }
        let unfunded = self.unfunded();
        if calls == 0 && unfunded.is_positive() {
            return Err(Error::invalid(format!(
                "{} still has {unfunded} unfunded and the forecast offered for it schedules no \
                 capital call; leave the commitment unscheduled, which reserves that balance \
                 whole, or attach the call schedule alongside the distributions — a forecast \
                 silent about calls is read as expecting none, and the capital demanded of this \
                 commitment inside any horizon would be zero",
                self.subject
            )));
        }
        if scheduled > unfunded {
            return Err(Error::invalid(format!(
                "{} schedules {scheduled} of calls and fees against an unfunded balance of \
                 {unfunded}; correct the schedule or the called balance — a fund cannot call more \
                 than remains promised",
                self.subject
            )));
        }
        self.forecast = Some(forecast);
        Ok(self)
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub const fn committed(&self) -> Decimal {
        self.committed
    }

    pub const fn called(&self) -> Decimal {
        self.called
    }

    pub const fn origin(&self) -> Timestamp {
        self.origin
    }

    pub const fn known_at(&self) -> Timestamp {
        self.known_at
    }

    pub const fn forecast(&self) -> Option<&CashflowForecast> {
        self.forecast.as_ref()
    }

    /// Capital promised and not yet drawn. The hard obligation.
    pub fn unfunded(&self) -> Decimal {
        (self.committed - self.called).max(Decimal::ZERO)
    }

    /// Record an issued drawdown notice against this commitment.
    ///
    /// Refuses a notice naming a different subject, a duplicate reference, a
    /// notice knowable before the commitment itself was, and — the one that
    /// matters — a notice which, with those already outstanding, would demand
    /// more than remains unfunded. That last refusal is the same argument
    /// [`Self::unscheduled`] makes about the called balance: a fund cannot
    /// call more than was promised, and capping the excess would turn a
    /// corrupt record into a plausible one.
    ///
    /// A duplicate reference is refused rather than overwritten because a
    /// second notice under one reference is either a restatement or a second
    /// draw, and the two demand different capital. Guessing which would move
    /// the reserve with no record of which claim won.
    pub fn record_call(&mut self, call: CapitalCall) -> Result<()> {
        if call.subject() != self.subject {
            return Err(Error::invalid(format!(
                "notice {} draws against {} and cannot be recorded on the commitment for {}; file it against its own subject",
                call.reference(),
                call.subject(),
                self.subject
            )));
        }
        if self.calls.contains_key(call.reference()) {
            return Err(Error::invalid(format!(
                "{} already holds notice {}; amend the existing one rather than recording a second under the same reference, because the reserve would move with no record of which claim won",
                self.subject,
                call.reference()
            )));
        }
        if call.issued_at() < self.known_at {
            return Err(Error::invalid(format!(
                "notice {} against {} became knowable at {}, before the commitment it draws on did at {}; correct the dates — a fund cannot call capital before the platform knew it had promised any",
                call.reference(),
                self.subject,
                call.issued_at().to_rfc3339(),
                self.known_at.to_rfc3339()
            )));
        }
        let unfunded = self.unfunded();
        let outstanding = self.outstanding_total()?;
        let demanded = outstanding.checked_add(call.amount()).ok_or_else(|| {
            Error::numeric(format!(
                "the outstanding notices against {} overflow when {} is added; nothing is reserved against a demand that cannot be represented",
                self.subject,
                call.reference()
            ))
        })?;
        if demanded > unfunded {
            return Err(Error::invalid(format!(
                "notice {} would take the notices outstanding against {} to {demanded} against an unfunded balance of {unfunded}; correct the notice or the called balance — a fund cannot call more than remains promised, and the excess will not be capped here",
                call.reference(),
                self.subject
            )));
        }
        self.calls.insert(call.reference().to_string(), call);
        Ok(())
    }

    /// Every notice outstanding against this commitment, in reference order.
    pub fn calls(&self) -> impl Iterator<Item = &CapitalCall> {
        self.calls.values()
    }

    /// One outstanding notice by its reference.
    pub fn call(&self, reference: &str) -> Option<&CapitalCall> {
        self.calls.get(reference)
    }

    /// Principal demanded by the notices outstanding, whether or not due.
    fn outstanding_total(&self) -> Result<Decimal> {
        let mut total = Decimal::ZERO;
        for call in self.calls.values() {
            total = total.checked_add(call.amount()).ok_or_else(|| {
                Error::numeric(format!(
                    "the notices outstanding against {} overflow; nothing is reserved against a demand that cannot be represented",
                    self.subject
                ))
            })?;
        }
        Ok(total)
    }

    /// Meet a notice: move its amount into the called balance and retire it.
    ///
    /// This is the one operation that reduces [`Self::unfunded`] with a
    /// record of *why*. Before notices existed the called balance was a bare
    /// figure that arrived from a vendor record with no provenance, and
    /// nothing could say which draws made it up.
    ///
    /// Refuses an unknown reference, and payment at an instant before the
    /// notice was knowable — money cannot have been sent against a demand
    /// nobody had received.
    pub fn settle_call(&mut self, reference: &str, paid_at: Timestamp) -> Result<Decimal> {
        let call = self.calls.get(reference).ok_or_else(|| {
            Error::invalid(format!(
                "{} holds no notice {reference}; record the notice before settling it, because a payment with no demand behind it would move the called balance with nothing to reconcile it against",
                self.subject
            ))
        })?;
        call.guard_knowable(paid_at)?;
        let amount = call.amount();
        let called = self.called.checked_add(amount).ok_or_else(|| {
            Error::numeric(format!(
                "settling {reference} overflows the called balance of {}; the record cannot be represented and is not written",
                self.subject
            ))
        })?;
        if called > self.committed {
            return Err(Error::invalid(format!(
                "settling {reference} would take {}'s called capital to {called} against a commitment of {}; correct the record rather than paying more than was promised",
                self.subject, self.committed
            )));
        }
        self.called = called;
        self.calls.remove(reference);
        Ok(self.unfunded())
    }

    /// What failing the overdue notices has cost by `as_of`.
    ///
    /// Zero where nothing is overdue, which is the ordinary case — so this
    /// adds nothing to a book that is paying its calls, and is not a figure
    /// that quietly inflates every reserve.
    pub fn accrued_default_penalty(&self, as_of: Timestamp) -> Result<Decimal> {
        let mut total = Decimal::ZERO;
        for call in self.calls.values() {
            total = total.checked_add(call.penalty_at(as_of)?).ok_or_else(|| {
                Error::numeric(format!(
                    "the default penalties accrued against {} overflow; nothing is reserved against a penalty that cannot be represented",
                    self.subject
                ))
            })?;
        }
        Ok(total)
    }

    /// Everything this commitment obliges the book to as of `as_of`: the
    /// unfunded balance plus whatever failing an overdue notice has cost.
    ///
    /// The two coincide exactly while no notice is overdue, which is why
    /// [`CommitmentBook::unfunded_total`] can read this without changing any
    /// figure the platform reports today. They diverge the moment a call is
    /// missed, and the divergence is the point: default interest is capital
    /// owed to the same counterparty on the same paper, and a book that
    /// deployed against an unfunded balance alone would be deploying money it
    /// had already lost.
    pub fn obligation(&self, as_of: Timestamp) -> Result<Decimal> {
        self.guard_knowable(as_of)?;
        self.unfunded()
            .checked_add(self.accrued_default_penalty(as_of)?)
            .ok_or_else(|| {
                Error::numeric(format!(
                    "the obligation for {} overflows; nothing is reserved against a liability that cannot be represented",
                    self.subject
                ))
            })
    }

    /// Principal the outstanding notices demand at or before `until`,
    /// counting anything already overdue as demanded now.
    ///
    /// Only notices the platform had received by `as_of` are counted; one
    /// issued later is a demand the desk had not been told about, and letting
    /// it into a backtest is the point-in-time leakage this module's header
    /// names second.
    fn noticed_demand_by(&self, as_of: Timestamp, until: Timestamp) -> Result<Decimal> {
        let mut total = Decimal::ZERO;
        for call in self.calls.values() {
            if !call.is_knowable_at(as_of) || call.due_at() > until {
                continue;
            }
            total = total.checked_add(call.amount()).ok_or_else(|| {
                Error::numeric(format!(
                    "the notices falling due against {} overflow; nothing is reserved against a demand that cannot be represented",
                    self.subject
                ))
            })?;
        }
        Ok(total)
    }

    /// Whether an overdue notice has accelerated the whole unfunded balance.
    fn is_accelerated_at(&self, as_of: Timestamp) -> bool {
        self.calls.values().any(|call| {
            call.is_overdue_at(as_of) && matches!(call.consequence(), CallConsequence::Acceleration)
        })
    }

    /// Whether this commitment was knowable at `as_of`.
    pub fn is_knowable_at(&self, as_of: Timestamp) -> bool {
        self.known_at <= as_of
    }

    /// Refuse to be read at an instant before this commitment was knowable.
    pub fn guard_knowable(&self, as_of: Timestamp) -> Result<()> {
        if self.is_knowable_at(as_of) {
            return Ok(());
        }
        Err(Error::invalid(format!(
            "the commitment for {} became knowable at {} and cannot be read as of {}; a book \
             cannot reserve against an obligation it had not been told about",
            self.subject,
            self.known_at.to_rfc3339(),
            as_of.to_rfc3339()
        )))
    }

    /// Capital demanded within `horizon`, from the notices actually issued
    /// and from the pacing model, plus anything an overdue notice has cost.
    ///
    /// The fallback where no schedule exists is deliberately the conservative
    /// one. A commitment with no pacing model could be called in full
    /// tomorrow, and the honest reserve against a timing nobody has modelled
    /// is the whole obligation.
    ///
    /// **An issued notice is taken as the greater of the two, never as a sum
    /// of them.** The forecast is a projection of calls somebody might make,
    /// and a notice that has arrived is very often one of the calls it
    /// projected; adding them would reserve for the same draw twice and
    /// report a squeeze the desk does not have. Taking the maximum cannot
    /// under-reserve against a demand already in writing, which is the
    /// failure that matters — before notices existed, a commitment carrying a
    /// pacing model returned the model's probability-weighted figure even
    /// where a fund had sent a notice for the full balance, and the reserve
    /// was short by the difference between a guess and a fact.
    ///
    /// An overdue [`CallConsequence::Acceleration`] demands the whole
    /// unfunded balance now, which is what that arm means and the only place
    /// it is read.
    pub fn demand_within(&self, as_of: Timestamp, horizon: Duration) -> Result<Decimal> {
        self.guard_knowable(as_of)?;
        if horizon.as_nanos() < 0 {
            return Err(Error::invalid(format!(
                "a demand horizon of {} nanoseconds runs backwards; supply a non-negative horizon",
                horizon.as_nanos()
            )));
        }
        let projected = match &self.forecast {
            Some(forecast) => forecast.expected_demand_within(as_of, horizon)?,
            None => self.unfunded(),
        };
        let noticed = if self.is_accelerated_at(as_of) {
            self.unfunded()
        } else {
            self.noticed_demand_by(as_of, as_of.saturating_add(horizon))?
        };
        projected
            .max(noticed)
            .checked_add(self.accrued_default_penalty(as_of)?)
            .ok_or_else(|| {
                Error::numeric(format!(
                    "the capital demanded of {} within the horizon overflows; nothing is reserved against a demand that cannot be represented",
                    self.subject
                ))
            })
    }

    /// Derive a commitment from the private-asset record the object model
    /// already carries.
    ///
    /// Nothing is invented: `committed_capital` and `called_capital` are on the
    /// record, and `known_at` is when the platform last updated that record.
    /// Returns `None` where the record shows nothing unfunded — a fully drawn
    /// fund obliges the book to no further capital, and recording a zero
    /// commitment would put a row in the book that can never reserve anything.
    ///
    /// # A record stating more called than committed is refused, and was not
    ///
    /// [`Self::unscheduled`] refuses that record by name and says why the
    /// excess "will not be capped here". It could never see one from this
    /// constructor. [`PrivateAssetDetails::unfunded_commitment`] is
    /// `(committed − called).max(0)`, so a record stating 12 called against 10
    /// committed answered an unfunded balance of zero, the guard above read
    /// that as "a fully drawn fund" and returned `None`, and the corrupt
    /// record left the sweep with no commitment recorded and no refusal
    /// raised. The clamp sat in front of the refusal and answered first —
    /// the shape `.claude/rules/domains/risk-and-execution.md` names by
    /// example, a control that reads as protection and cannot fire.
    ///
    /// The direction is the dangerous one. A commitment nobody recorded is
    /// capital `Platform::deployable_capital` treats as free, and a units
    /// error or a duplicated drawdown in an administrator's record is exactly
    /// how 12-against-10 arises. So the coherence of the record is asked
    /// before the balance derived from it, and an incoherent record falls
    /// through to [`Self::unscheduled`]'s own refusal rather than to a second
    /// copy of its sentence written here — one writer of that message, so the
    /// two cannot drift apart.
    ///
    /// This is the constructor `qip-kernel`'s `private_holdings_of` calls for
    /// every private asset in the universe at assembly, so the refusal stops
    /// `Platform::new` and names the record, exactly as the neighbouring
    /// `known_at < origin` refusal already does for a vintage typed a year
    /// early.
    pub fn from_private_asset(
        subject: impl Into<String>,
        details: &PrivateAssetDetails,
        origin: Timestamp,
        known_at: Timestamp,
    ) -> Result<Option<Self>> {
        if details.called_capital <= details.committed_capital
            && !details.unfunded_commitment().is_positive()
        {
            return Ok(None);
        }
        Self::unscheduled(
            subject,
            details.committed_capital,
            details.called_capital,
            origin,
            known_at,
        )
        .map(Some)
    }
}

/// Every commitment the book is on the hook for.
///
/// Keyed by subject in a [`BTreeMap`]: the reserve total is a sum, and a sum
/// whose order changes between runs is a figure that cannot be replayed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommitmentBook {
    commitments: BTreeMap<String, Commitment>,
}

impl CommitmentBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a commitment, refusing a second one for the same subject.
    ///
    /// Overwriting would silently replace an obligation with another, and the
    /// reserve would move with no record of which claim won.
    pub fn record(&mut self, commitment: Commitment) -> Result<()> {
        if self.commitments.contains_key(commitment.subject()) {
            return Err(Error::invalid(format!(
                "{} already has a commitment recorded; amend the existing one rather than \
                 recording a second, because the reserve would move with no record of which claim \
                 won",
                commitment.subject()
            )));
        }
        self.commitments
            .insert(commitment.subject().to_string(), commitment);
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.commitments.is_empty()
    }

    pub fn len(&self) -> usize {
        self.commitments.len()
    }

    pub fn get(&self, subject: &str) -> Option<&Commitment> {
        self.commitments.get(subject)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Commitment> {
        self.commitments.values()
    }

    /// Everything the private book obliges the desk to at `as_of`: the whole
    /// unfunded balance, plus what failing any overdue notice has already
    /// cost.
    ///
    /// This is what the capital engine must treat as reserved. A commitment
    /// not yet knowable is refused rather than skipped, because a reserve that
    /// silently omits an obligation is the failure this book exists to
    /// prevent.
    ///
    /// **The penalty half is new and the name is unchanged, so read
    /// [`Commitment::obligation`] rather than the name.** The two figures are
    /// equal for every commitment with no overdue [`CapitalCall`], which is
    /// every commitment the platform holds today, so this returns exactly
    /// what it always did until a notice is missed. Once one is, default
    /// interest is capital owed to the same counterparty on the same paper,
    /// and a desk deploying against the unfunded balance alone would be
    /// deploying money it had already lost.
    pub fn unfunded_total(&self, as_of: Timestamp) -> Result<Decimal> {
        let mut total = Decimal::ZERO;
        for commitment in self.commitments.values() {
            commitment.guard_knowable(as_of)?;
            total = total
                .checked_add(commitment.obligation(as_of)?)
                .ok_or_else(|| {
                    Error::numeric(
                        "the unfunded commitment total overflows; nothing is reserved against a \
                         number that cannot be represented"
                            .to_string(),
                    )
                })?;
        }
        Ok(total)
    }

    /// File an issued drawdown notice against the commitment it draws on.
    ///
    /// Refuses a notice for a subject the book does not hold, rather than
    /// opening a commitment to receive it: a call against a commitment nobody
    /// recorded is either a notice for somebody else's book or a commitment
    /// this platform was never told about, and inventing the second to
    /// accommodate the first would put an obligation in the book with no
    /// promise behind it.
    pub fn record_call(&mut self, call: CapitalCall) -> Result<()> {
        let subject = call.subject().to_string();
        let commitment = self.commitments.get_mut(&subject).ok_or_else(|| {
            Error::invalid(format!(
                "no commitment is recorded for {subject}, so notice {} has nothing to draw \
                 against; record the commitment first rather than letting a call open one, \
                 because an obligation with no promise behind it cannot be reconciled",
                call.reference()
            ))
        })?;
        commitment.record_call(call)
    }

    /// What failing the book's overdue notices has cost by `as_of`.
    ///
    /// Reported separately as well as inside [`Self::unfunded_total`] so an
    /// operator can see the penalty on its own. A reserve that grew and a
    /// penalty that accrued look identical in a single total, and they call
    /// for different action: one is capital doing its job, the other is a
    /// payment the desk missed.
    pub fn accrued_default_penalty(&self, as_of: Timestamp) -> Result<Decimal> {
        let mut total = Decimal::ZERO;
        for commitment in self.commitments.values() {
            commitment.guard_knowable(as_of)?;
            total = total
                .checked_add(commitment.accrued_default_penalty(as_of)?)
                .ok_or_else(|| {
                    Error::numeric(
                        "the default penalties across the commitment book overflow; nothing is \
                         reserved against a penalty that cannot be represented"
                            .to_string(),
                    )
                })?;
        }
        Ok(total)
    }

    /// Probability-weighted capital demanded across the book within `horizon`.
    pub fn demand_within(&self, as_of: Timestamp, horizon: Duration) -> Result<Decimal> {
        let mut total = Decimal::ZERO;
        for commitment in self.commitments.values() {
            total = total
                .checked_add(commitment.demand_within(as_of, horizon)?)
                .ok_or_else(|| {
                    Error::numeric("the near-term capital demand overflows".to_string())
                })?;
        }
        Ok(total)
    }
}
