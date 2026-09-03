//! The hedge engine: proposed orders that reduce a named exposure, and the
//! refusals that keep a hedge from becoming a position.
//!
//! # Where this sits, and why here
//!
//! Hedging is risk *reduction*, and its guardrails are limit-engine questions:
//! is the exposure large enough to act on, and would the hedge's own notional
//! breach the limits it is meant to defend. Those answers live in
//! [`crate::limits`], and the exposures being reduced are the
//! [`qip_portfolio::exposure`] types this crate already consumes — so the
//! engine lives here, beside its inputs, rather than in a crate of its own.
//!
//! # The seam: what calls this, with what, at what cadence
//!
//! Nothing in this module submits an order, and nothing in it is wired yet —
//! deliberately. The intended composition:
//!
//! * **Caller**: the kernel's decide stage (or the central plane), once per
//!   cycle, after exposures have been valued — the same cadence at which the
//!   `RiskMonitor` already runs.
//! * **Inputs**: a [`HedgeExposures`] built from
//!   `Portfolio::exposures` (plus per-instrument signed notionals), the
//!   current [`crate::limits::RiskState`], the governing
//!   [`crate::limits::LimitSet`], a price per declared hedge instrument, and
//!   the caller's clock. Policies come from governance configuration, exactly
//!   as limits do.
//! * **Output**: [`HedgeProposal`]s. A proposal enters the platform's
//!   proposal → approval → order path — the kernel's governed submit, behind
//!   pre-trade risk — like any other proposal. It is never an order and this module has
//!   no way to make it one: there is no broker type here to hand it to.
//!
//! # The arithmetic, stated so it can be checked
//!
//! The hedge ratio comes from a **declared** beta — a policy input written
//! down by a person, exactly as a limit is. It is never estimated here: a
//! silently estimated beta becomes a doubled position the day the correlation
//! it was fitted on flips sign, and nobody can audit a number that was never
//! declared. `beta` means: units of hedge-instrument notional that offset one
//! unit of the named exposure (the exposed book's beta *to* the hedge
//! instrument), so
//!
//! ```text
//! hedge notional = beta × |net exposure − target|
//! ```
//!
//! Quantity is that notional at the supplied price, rounded **down** to the
//! instrument's lot — toward *under*-hedging, always. An over-hedge is not a
//! smaller residual; past the target it is a brand-new naked position the
//! other way, wearing a hedge's name. The rounding is followed by an explicit
//! invariant check, because the division that sizes the quantity rounds
//! half-away at the ninth decimal and a one-billionth drift upward is still
//! the wrong direction.
//!
//! # What this module does not promise
//!
//! * It does not estimate beta, correlation, or anything else. A policy with
//!   no declared beta does not exist; there is no default.
//! * It does not accept a negative or zero beta. Declare the co-moving
//!   instrument; for an inverse product, declare its underlying.
//! * It does not net across policies. Two policies hedging overlapping
//!   exposures will each propose in full; approval is where netting judgement
//!   lives.
//! * Its pre-trade projection covers notional, position, leverage and net
//!   limits. It does not project the per-axis bucket caps: `project_hedge`
//!   moves gross exposure and not `axis_exposures`, so a hedge's own bucket
//!   weight is invisible to a proposal-time check. It does not project
//!   volatility, VaR or expected shortfall either — those need a return
//!   series a proposal-time check does not have. Pre-trade risk re-checks the
//!   order against the full set anyway, which is where a bucket cap actually
//!   holds.
//! * It does not track whether past hedges worked. Effectiveness is an
//!   attribution question, answered elsewhere from realised outcomes.

use crate::limits::{LimitBreach, LimitCheck, LimitSet, RiskState};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_portfolio::exposure::{Exposure, ExposureBreakdown};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The axis a hedge policy names its exposure on.
///
/// The same axes the limit engine checks, plus `Instrument` for a single-name
/// hedge, because a limit is breached along one axis and a hedge defends one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HedgeAxis {
    Instrument,
    AssetClass,
    Sector,
    Country,
    Currency,
    Issuer,
    Factor,
}

impl HedgeAxis {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Instrument => "instrument",
            Self::AssetClass => "asset_class",
            Self::Sector => "sector",
            Self::Country => "country",
            Self::Currency => "currency",
            Self::Issuer => "issuer",
            Self::Factor => "factor",
        }
    }
}

/// The exposures a hedge survey reads: every axis the portfolio reports, plus
/// signed per-instrument notionals, which [`ExposureBreakdown`] does not carry.
///
/// Built by the caller from `Portfolio::exposures` and the position notionals
/// it already values, so this module never prices anything itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HedgeExposures {
    pub breakdown: ExposureBreakdown,
    /// Signed notional per instrument, on the same convention as the
    /// breakdown: positive long, negative short.
    pub by_instrument: Exposure,
}

impl HedgeExposures {
    pub fn new(breakdown: ExposureBreakdown) -> Self {
        Self {
            breakdown,
            by_instrument: Exposure::new(),
        }
    }

    pub fn with_instruments(mut self, by_instrument: Exposure) -> Self {
        self.by_instrument = by_instrument;
        self
    }

    /// The exposure along one axis.
    pub fn along(&self, axis: HedgeAxis) -> &Exposure {
        match axis {
            HedgeAxis::Instrument => &self.by_instrument,
            HedgeAxis::AssetClass => &self.breakdown.by_asset_class,
            HedgeAxis::Sector => &self.breakdown.by_sector,
            HedgeAxis::Country => &self.breakdown.by_country,
            HedgeAxis::Currency => &self.breakdown.by_currency,
            HedgeAxis::Issuer => &self.breakdown.by_issuer,
            HedgeAxis::Factor => &self.breakdown.by_factor,
        }
    }
}

/// The instrument a policy hedges with. Declared, like everything else here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HedgeInstrument {
    pub object_id: ObjectId,
    pub symbol: String,
    /// Notional per unit per unit of price, exactly as `qip-portfolio` uses it.
    pub contract_multiplier: Decimal,
    /// The increment quantities are rounded down to. One share, one contract,
    /// one venue lot — whatever the instrument trades in.
    pub lot_size: Decimal,
}

impl HedgeInstrument {
    pub fn new(object_id: ObjectId, symbol: impl Into<String>) -> Self {
        Self {
            object_id,
            symbol: symbol.into(),
            contract_multiplier: Decimal::ONE,
            lot_size: Decimal::ONE,
        }
    }

    pub fn with_multiplier(mut self, multiplier: Decimal) -> Self {
        self.contract_multiplier = multiplier;
        self
    }

    fn validate(&self) -> Result<()> {
        if self.symbol.trim().is_empty() {
            return Err(Error::invalid(
                "a hedge instrument needs a symbol; a proposal in an unnamed instrument cannot \
                 be approved by anyone",
            ));
        }
        if self.contract_multiplier <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "hedge instrument {} has a non-positive contract multiplier",
                self.symbol
            )));
        }
        if self.lot_size <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "hedge instrument {} has a non-positive lot size, so no quantity could be \
                 rounded to it",
                self.symbol
            )));
        }
        Ok(())
    }
}

/// One declared hedge: which exposure, toward what, with what, at what ratio.
///
/// Every number here is a governance decision recorded in configuration, for
/// the same reason limits are: the place for judgement is in *setting* the
/// policy, and a hedge engine that invents its own inputs cannot be audited.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HedgePolicy {
    pub name: String,
    pub axis: HedgeAxis,
    /// The bucket on that axis: a sector name, a currency code, an instrument.
    pub bucket: String,
    /// The net exposure the bucket is steered toward. Zero for a full hedge.
    pub target_net: Decimal,
    /// Below this distance from target, no proposal is made. A hedge smaller
    /// than its own friction is churn wearing a hedge's name.
    pub de_minimis: Decimal,
    /// Declared beta of the named exposure to the hedge instrument: units of
    /// hedge notional that offset one unit of exposure. **Never estimated.**
    pub beta: Decimal,
    /// The instrument to hedge with. `None` is not a default waiting to be
    /// filled in — it is the refusal [`HedgeRefusal::NoInstrumentDeclared`].
    pub instrument: Option<HedgeInstrument>,
    /// Why the policy exists, carried onto every proposal it produces.
    pub rationale: String,
}

impl HedgePolicy {
    pub fn new(
        name: impl Into<String>,
        axis: HedgeAxis,
        bucket: impl Into<String>,
        beta: Decimal,
    ) -> Self {
        Self {
            name: name.into(),
            axis,
            bucket: bucket.into(),
            target_net: Decimal::ZERO,
            de_minimis: Decimal::ZERO,
            beta,
            instrument: None,
            rationale: String::new(),
        }
    }

    pub fn with_instrument(mut self, instrument: HedgeInstrument) -> Self {
        self.instrument = Some(instrument);
        self
    }

    pub fn with_target(mut self, target_net: Decimal) -> Self {
        self.target_net = target_net;
        self
    }

    pub fn with_de_minimis(mut self, de_minimis: Decimal) -> Self {
        self.de_minimis = de_minimis;
        self
    }

    pub fn with_rationale(mut self, rationale: impl Into<String>) -> Self {
        self.rationale = rationale.into();
        self
    }

    /// Whether the policy's declared numbers are usable at all.
    ///
    /// The instrument's absence is deliberately *not* checked here: a policy
    /// with no instrument is a refusal the survey reports per-policy, not a
    /// configuration error that stops the others.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::invalid("a hedge policy needs a name"));
        }
        if self.bucket.trim().is_empty() {
            return Err(Error::invalid(format!(
                "hedge policy {} names no bucket on the {} axis",
                self.name,
                self.axis.as_str()
            )));
        }
        if self.beta <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "hedge policy {} declares a beta of {}, which is not positive. Declare the \
                 co-moving instrument; for an inverse product, declare its underlying. A \
                 non-positive beta would put the hedge on the same side as the exposure",
                self.name, self.beta
            )));
        }
        if self.de_minimis < Decimal::ZERO {
            return Err(Error::invalid(format!(
                "hedge policy {} has a negative de-minimis threshold",
                self.name
            )));
        }
        if let Some(instrument) = &self.instrument {
            instrument.validate()?;
        }
        Ok(())
    }
}

/// Which way the proposed hedge order goes.
///
/// A local type rather than the execution engine's `Side`, because this crate
/// sits below the execution engine and a proposal is not yet an order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HedgeSide {
    Buy,
    Sell,
}

impl HedgeSide {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }

    pub const fn sign(&self) -> i32 {
        match self {
            Self::Buy => 1,
            Self::Sell => -1,
        }
    }
}

/// A proposed hedge order, with the reasoning that produced it.
///
/// A proposal, not an order: it has no venue, no order id and no way to reach
/// a broker from this crate. The governance path — proposal, approval, then
/// the kernel's governed submit behind pre-trade risk — remains the only road
/// from here to a market. The submitting type is deliberately not named here:
/// the acceptance suite refuses its name outside a composition root, in prose
/// as much as in code, because a mention is how the type creeps in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HedgeProposal {
    /// Deterministic: the policy name and the timestamp, so the same survey
    /// proposes under the same identity on a replay.
    pub proposal_id: String,
    pub policy: String,
    pub axis: HedgeAxis,
    pub bucket: String,
    /// The net exposure observed at proposal time.
    pub observed_net: Decimal,
    pub target_net: Decimal,
    /// Observed minus target: the signed exposure being reduced.
    pub excess: Decimal,
    /// The declared beta the ratio came from, carried for the approver.
    pub beta: Decimal,
    pub instrument: HedgeInstrument,
    /// The price the proposal was sized against.
    pub price: Decimal,
    pub side: HedgeSide,
    /// Always positive; the side carries the direction. Rounded down to the
    /// instrument's lot — under-hedged, never over.
    pub quantity: Decimal,
    /// `quantity × price × multiplier`, unsigned.
    pub hedge_notional: Decimal,
    /// The exposure that remains after the hedge, at the declared beta. Same
    /// sign as `excess` or zero — never flipped, which is the under-hedge
    /// invariant in one number.
    pub expected_residual: Decimal,
    /// The arithmetic in words, one step per line, for the approver.
    pub reasoning: Vec<String>,
    pub at: Timestamp,
}

/// Why a hedge was refused. Refusals carry their numbers: a refusal that
/// cannot be re-derived is an argument, not a control.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum HedgeRefusal {
    /// The policy declares no hedge instrument. Nothing is inferred: an
    /// engine that picks its own instrument is sizing a position nobody chose.
    NoInstrumentDeclared { policy: String, detail: String },
    /// The policy's declared numbers are unusable (see
    /// [`HedgePolicy::validate`]); the detail says which and why.
    MisdeclaredPolicy { policy: String, detail: String },
    /// No usable price was supplied for the declared instrument. A hedge
    /// sized against a guessed price is a guessed hedge.
    UnusablePrice {
        policy: String,
        instrument: String,
        detail: String,
    },
    /// The hedge's own notional would breach the limits it is meant to
    /// defend. Both numbers are carried: the notional that was refused and
    /// every limit that bound, each with its observed value and its bound.
    WouldBreachLimits {
        policy: String,
        hedge_notional: Decimal,
        breaches: Vec<LimitBreach>,
        detail: String,
    },
}

impl HedgeRefusal {
    pub fn describe(&self) -> &str {
        match self {
            Self::NoInstrumentDeclared { detail, .. }
            | Self::MisdeclaredPolicy { detail, .. }
            | Self::UnusablePrice { detail, .. }
            | Self::WouldBreachLimits { detail, .. } => detail,
        }
    }
}

/// What one policy produced against one exposure reading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum HedgeOutcome {
    /// A proposal, for the approval path.
    Proposed(Box<HedgeProposal>),
    /// Nothing to do, and why — a de-minimis exposure, or a quantity that
    /// rounded down to nothing. Not a refusal: the policy is healthy and the
    /// book does not need it today.
    NoAction {
        policy: String,
        residual: Decimal,
        detail: String,
    },
    /// Refused, with the reason and its numbers.
    Refused(HedgeRefusal),
}

impl HedgeOutcome {
    pub const fn is_proposal(&self) -> bool {
        matches!(self, Self::Proposed(_))
    }
}

/// The hedge engine: declared policies, surveyed against observed exposures.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HedgeEngine {
    policies: Vec<HedgePolicy>,
}

impl HedgeEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, policy: HedgePolicy) -> Self {
        self.policies.push(policy);
        self
    }

    pub fn policies(&self) -> &[HedgePolicy] {
        &self.policies
    }

    /// Run every policy against the current exposures.
    ///
    /// `prices` is keyed by the hedge instrument's object id, the same
    /// convention `Portfolio::value` prices on. One outcome per policy, in
    /// policy order, every one either a proposal, a stated no-action, or a
    /// refusal with its numbers — never silence.
    pub fn survey(
        &self,
        exposures: &HedgeExposures,
        prices: &BTreeMap<String, Decimal>,
        limits: &LimitSet,
        state: &RiskState,
        at: Timestamp,
    ) -> Vec<HedgeOutcome> {
        self.policies
            .iter()
            .map(|policy| propose_hedge(policy, exposures, prices, limits, state, at))
            .collect()
    }
}

/// Evaluate one policy against the current exposures.
///
/// Deterministic: the same exposures, prices, limits and state produce the
/// same outcome, to the digit. Nothing here reads a clock, a market or a
/// model; `at` is a parameter and every ratio is declared.
pub fn propose_hedge(
    policy: &HedgePolicy,
    exposures: &HedgeExposures,
    prices: &BTreeMap<String, Decimal>,
    limits: &LimitSet,
    state: &RiskState,
    at: Timestamp,
) -> HedgeOutcome {
    if let Err(error) = policy.validate() {
        return HedgeOutcome::Refused(HedgeRefusal::MisdeclaredPolicy {
            policy: policy.name.clone(),
            detail: error.message().to_string(),
        });
    }
    let Some(instrument) = &policy.instrument else {
        return HedgeOutcome::Refused(HedgeRefusal::NoInstrumentDeclared {
            policy: policy.name.clone(),
            detail: format!(
                "hedge policy {} declares no hedge instrument, so there is nothing this engine \
                 may size. It will not choose one: an engine that picks its own instrument is \
                 sizing a position nobody chose",
                policy.name
            ),
        });
    };

    // The exposure being reduced, measured from the declared target.
    let observed_net = exposures.along(policy.axis).net_of(&policy.bucket);
    let excess = observed_net - policy.target_net;
    let magnitude = excess.abs();

    if magnitude <= policy.de_minimis {
        return HedgeOutcome::NoAction {
            policy: policy.name.clone(),
            residual: excess,
            detail: format!(
                "net {}:{} exposure is {observed_net}, {magnitude} from the target of {}, at or \
                 inside the de-minimis threshold of {}; a hedge smaller than its own friction \
                 is churn, so nothing is proposed",
                policy.axis.as_str(),
                policy.bucket,
                policy.target_net,
                policy.de_minimis
            ),
        };
    }

    let Some(price) = prices.get(instrument.object_id.as_str()).copied() else {
        return HedgeOutcome::Refused(HedgeRefusal::UnusablePrice {
            policy: policy.name.clone(),
            instrument: instrument.symbol.clone(),
            detail: format!(
                "no price was supplied for {} ({}), so the hedge cannot be sized. A hedge sized \
                 against a guessed price is a guessed hedge",
                instrument.symbol,
                instrument.object_id.as_str()
            ),
        });
    };
    if price <= Decimal::ZERO {
        return HedgeOutcome::Refused(HedgeRefusal::UnusablePrice {
            policy: policy.name.clone(),
            instrument: instrument.symbol.clone(),
            detail: format!(
                "the supplied price for {} is {price}, which cannot size a quantity",
                instrument.symbol
            ),
        });
    }

    // hedge notional = beta × |excess|; quantity = notional / (price × mult),
    // rounded DOWN to the lot. Checked arithmetic throughout: an overflow is
    // an answer ("this cannot be sized"), not a panic.
    let sizing = size_hedge(policy, instrument, magnitude, price);
    let (quantity, hedge_notional, desired_notional) = match sizing {
        Ok(sized) => sized,
        Err(error) => {
            return HedgeOutcome::Refused(HedgeRefusal::MisdeclaredPolicy {
                policy: policy.name.clone(),
                detail: error.message().to_string(),
            });
        }
    };

    if quantity <= Decimal::ZERO {
        return HedgeOutcome::NoAction {
            policy: policy.name.clone(),
            residual: excess,
            detail: format!(
                "offsetting {excess} takes {desired_notional} of {} notional, which is less \
                 than one {} lot at {price}; under-hedging rounds down, and down from less \
                 than one lot is nothing",
                instrument.symbol, instrument.lot_size
            ),
        };
    }

    // A long excess is sold off; a short excess is bought back.
    let side = if excess.is_positive() {
        HedgeSide::Sell
    } else {
        HedgeSide::Buy
    };

    // The exposure this hedge offsets, back through the same declared beta,
    // and the residual left on the book. Same sign as the excess, or zero:
    // that is the under-hedge invariant, checked again in tests.
    let offset = hedge_notional
        .checked_div(policy.beta)
        .unwrap_or(Decimal::ZERO);
    let signed_offset = if excess.is_positive() {
        offset
    } else {
        -offset
    };
    let expected_residual = excess - signed_offset;

    // Would the hedge itself breach the limits it defends? Projected before
    // proposing, so a refusal happens here with both numbers rather than
    // downstream with one.
    let projection = project_hedge(state, instrument, side, quantity, price, hedge_notional);
    let check: LimitCheck = limits.check(&projection);
    if check.is_blocked() {
        let blocking: Vec<LimitBreach> = check.blocking().into_iter().cloned().collect();
        let worst = check.reason();
        return HedgeOutcome::Refused(HedgeRefusal::WouldBreachLimits {
            policy: policy.name.clone(),
            hedge_notional,
            detail: format!(
                "the proposed hedge of {hedge_notional} notional in {} would itself breach the \
                 limits it is meant to defend: {worst}. A hedge that breaks a limit is a \
                 position, whatever it is called",
                instrument.symbol
            ),
            breaches: blocking,
        });
    }

    let reasoning = vec![
        format!(
            "net {}:{} exposure is {observed_net}; the declared target is {}, leaving {excess} \
             to reduce",
            policy.axis.as_str(),
            policy.bucket,
            policy.target_net
        ),
        format!(
            "the declared beta of this exposure to {} is {} (a policy input, never estimated), \
             so offsetting {magnitude} takes {desired_notional} of hedge notional",
            instrument.symbol, policy.beta
        ),
        format!(
            "at a price of {price} and a contract multiplier of {}, that rounds down to \
             {quantity} units ({hedge_notional} notional) on the {} lot — under-hedged by \
             design, because an over-hedge is a new naked position the other way",
            instrument.contract_multiplier, instrument.lot_size
        ),
        format!(
            "the hedge is a {} of {quantity} {}, offsetting {offset} of exposure and leaving \
             an expected residual of {expected_residual} on the same side as the excess",
            side.as_str(),
            instrument.symbol
        ),
        format!(
            "projected against {} limit(s), none block; this is a proposal, and only the \
             approval path can make it an order",
            check.evaluated
        ),
    ];

    HedgeOutcome::Proposed(Box::new(HedgeProposal {
        proposal_id: format!("hedge-{}-{}", policy.name, at.as_nanos()),
        policy: policy.name.clone(),
        axis: policy.axis,
        bucket: policy.bucket.clone(),
        observed_net,
        target_net: policy.target_net,
        excess,
        beta: policy.beta,
        instrument: instrument.clone(),
        price,
        side,
        quantity,
        hedge_notional,
        expected_residual,
        reasoning,
        at,
    }))
}

/// Size the hedge: quantity, its notional, and the notional that was wanted.
///
/// The returned quantity's notional never exceeds `beta × magnitude`. The
/// division that produces the raw quantity rounds half-away at the ninth
/// decimal, so flooring alone can land one lot high when the true quantity
/// sits within 10⁻⁹ below a lot boundary; the explicit step-down afterwards
/// is what makes "rounded toward under-hedging" an invariant rather than an
/// intention.
fn size_hedge(
    policy: &HedgePolicy,
    instrument: &HedgeInstrument,
    magnitude: Decimal,
    price: Decimal,
) -> Result<(Decimal, Decimal, Decimal)> {
    let unit_notional = price
        .checked_mul(instrument.contract_multiplier)
        .ok_or_else(|| {
            Error::numeric(format!(
                "the unit notional of {} overflowed at price {price}",
                instrument.symbol
            ))
        })?;
    if unit_notional <= Decimal::ZERO {
        return Err(Error::invalid(format!(
            "{} has a unit notional of {unit_notional}, which cannot size a quantity",
            instrument.symbol
        )));
    }
    let desired_notional = magnitude.checked_mul(policy.beta).ok_or_else(|| {
        Error::numeric(format!(
            "beta {} times an excess of {magnitude} overflowed",
            policy.beta
        ))
    })?;
    let raw_quantity = desired_notional
        .checked_div(unit_notional)
        .ok_or_else(|| Error::numeric("sizing the hedge quantity overflowed"))?;
    let mut quantity = raw_quantity.floor_to_step(instrument.lot_size);

    let notional_of = |q: Decimal| -> Result<Decimal> {
        q.checked_mul(unit_notional)
            .ok_or_else(|| Error::numeric("the hedge notional overflowed"))
    };
    if quantity > Decimal::ZERO && notional_of(quantity)? > desired_notional {
        quantity = (quantity - instrument.lot_size).max(Decimal::ZERO);
    }
    let hedge_notional = notional_of(quantity)?;
    if hedge_notional > desired_notional {
        // Mathematically unreachable — one lot step covers the half-away
        // rounding of the division — but if it ever were reached, refusing is
        // right and over-hedging is not.
        return Err(Error::numeric(format!(
            "a hedge of {hedge_notional} still exceeds the desired {desired_notional} after \
             stepping down a lot; refusing rather than over-hedging"
        )));
    }
    Ok((quantity, hedge_notional, desired_notional))
}

/// The risk state as it would look with the hedge on, for the limit check.
///
/// Deliberately conservative and deliberately partial: the hedge's notional is
/// *added* to gross, position and order figures (a proposal-time check cannot
/// know how a venue nets), its signed value moves net exposure, and nothing
/// else is touched — the bucket exposures it reduces, volatility, VaR and
/// shortfall are left as observed. A projection that under-states risk to get
/// a hedge through would be the engine approving itself.
fn project_hedge(
    state: &RiskState,
    instrument: &HedgeInstrument,
    side: HedgeSide,
    quantity: Decimal,
    price: Decimal,
    hedge_notional: Decimal,
) -> RiskState {
    let mut projected = state.clone();
    let signed = if side.sign() >= 0 {
        quantity * price * instrument.contract_multiplier
    } else {
        -(quantity * price * instrument.contract_multiplier)
    };
    projected.order_notional = Some(hedge_notional);
    projected.order_subject = Some(instrument.symbol.clone());
    projected.gross_exposure += hedge_notional;
    projected.net_exposure += signed;
    *projected
        .position_notionals
        .entry(instrument.symbol.clone())
        .or_insert(Decimal::ZERO) += signed;
    projected
}
