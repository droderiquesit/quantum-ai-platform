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
        })
    }

    /// Attach a call and distribution schedule.
    ///
    /// Refuses a forecast about a different subject, a forecast with a
    /// different origin, and a schedule whose unweighted calls and fees exceed
    /// the unfunded balance. The last is the consistency check that matters:
    /// a pacing model that schedules more than was ever promised is wrong, and
    /// a reserve computed from it would be a number with no counterpart in the
    /// contract.
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
        for flow in forecast.flows() {
            if flow.kind().is_outflow() {
                scheduled = scheduled.checked_add(flow.amount()).ok_or_else(|| {
                    Error::numeric("the scheduled call total overflows".to_string())
                })?;
            }
        }
        let unfunded = self.unfunded();
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

    /// Probability-weighted capital demanded within `horizon`, or the whole
    /// unfunded balance where no schedule exists.
    ///
    /// The fallback is deliberately the conservative one. A commitment with no
    /// pacing model could be called in full tomorrow, and the honest reserve
    /// against a timing nobody has modelled is the whole obligation.
    pub fn demand_within(&self, as_of: Timestamp, horizon: Duration) -> Result<Decimal> {
        self.guard_knowable(as_of)?;
        match &self.forecast {
            Some(forecast) => forecast.expected_demand_within(as_of, horizon),
            None => Ok(self.unfunded()),
        }
    }

    /// Derive a commitment from the private-asset record the object model
    /// already carries.
    ///
    /// Nothing is invented: `committed_capital` and `called_capital` are on the
    /// record, and `known_at` is when the platform last updated that record.
    /// Returns `None` where the record shows nothing unfunded — a fully drawn
    /// fund obliges the book to no further capital, and recording a zero
    /// commitment would put a row in the book that can never reserve anything.
    pub fn from_private_asset(
        subject: impl Into<String>,
        details: &PrivateAssetDetails,
        origin: Timestamp,
        known_at: Timestamp,
    ) -> Result<Option<Self>> {
        if !details.unfunded_commitment().is_positive() {
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

    /// The whole unfunded balance across every commitment knowable at `as_of`.
    ///
    /// This is what the capital engine must treat as reserved. A commitment
    /// not yet knowable is refused rather than skipped, because a reserve that
    /// silently omits an obligation is the failure this book exists to
    /// prevent.
    pub fn unfunded_total(&self, as_of: Timestamp) -> Result<Decimal> {
        let mut total = Decimal::ZERO;
        for commitment in self.commitments.values() {
            commitment.guard_knowable(as_of)?;
            total = total.checked_add(commitment.unfunded()).ok_or_else(|| {
                Error::numeric(
                    "the unfunded commitment total overflows; nothing is reserved against a number \
                     that cannot be represented"
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
