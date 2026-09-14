//! Blueprint §34.3: a decentralised venue is not an order book behind a
//! different protocol, and this module is the arithmetic that says so.
//!
//! Every assumption a central limit order book lets the platform make is
//! false on a chain, and each falsehood has a type here rather than a
//! comment:
//!
//! * **Slippage does not come from depth, it comes from the curve.** There is
//!   no book to walk. [`PoolState::quote`] evaluates the pool's own invariant
//!   and returns the price the trade would actually execute at, which for any
//!   non-zero size is worse than the marginal price the pool advertises.
//! * **Execution is block-time granular and probabilistic.**
//!   [`BlockExecution`] carries the interval and the confirmations a fill is
//!   only believed after, and [`BlockExecution::quotes_are_firm`] is `false`
//!   with nothing that can make it true.
//! * **The intent is public before it lands.** [`MevEstimate`] prices what a
//!   reordering adversary can take out of a trade that is visible and
//!   unconfirmed for [`BlockExecution::settlement_window`].
//! * **Counterparty risk is the contract.** [`ContractRisk`] names the
//!   deployed contract every position at this venue depends on, so a limit
//!   can be set against it the way one is set against a counterparty.
//!
//! # Money is `Decimal` and there is no `f64` in this file
//!
//! Pool reserves, the amounts traded, the fee, the execution price and the
//! slippage and MEV figures in basis points are all [`Decimal`]. This is the
//! module where an `f64` would be most tempting — a curve is arithmetic, and
//! a ratio reads like a statistic — and it is exactly where it would be most
//! expensive: a slippage figure is the difference between the price a trade
//! reasoned about and the price it got, which is money. There is no crossing
//! point in this file to state, because there is no crossing.
//!
//! # Refuse, never guess
//!
//! Three refusals are worth naming because each one is a place a model could
//! have quietly invented a number:
//!
//! * A concentrated-liquidity pool is modelled **inside its stated range
//!   only**. A trade that would push the price out of the range is refused,
//!   not extrapolated, because what happens beyond the boundary depends on
//!   liquidity in ticks this model was never given. A venue that cannot
//!   answer for a size is a venue that is observe-only for that size.
//! * A slippage tolerance below the quote's own slippage is refused rather
//!   than raised to fit. An order that cannot execute at the tolerance it
//!   states is a caller bug, and clamping it would submit a trade at a worse
//!   price than the caller agreed to.
//! * A [`DexModel`] missing any of the four pieces above answers
//!   [`DexModel::is_observe_only`] with `true` and will not quote at all,
//!   which is the blueprint's own fallback: "where these are absent, the
//!   venue is registered as observe-only until they exist".

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// Ten thousand, as the denominator every basis-point figure here divides by.
const BPS_DENOMINATOR: Decimal = Decimal::from_raw(10_000 * qip_core::decimal::SCALE);

/// The exposure axis a contract-risk flag is carried on, so that a position
/// held through a deployed contract is charged against that contract the way
/// a position faced with a dealer is charged against the dealer.
///
/// The same shape as `qip_risk::limits::COUNTERPARTY_AXIS`, and deliberately
/// a *different* axis: a counterparty limit is about an entity that can be
/// called; a contract limit is about code that cannot be, and netting the two
/// would let a book concentrated in one unaudited contract read as
/// diversified because its counterparties were many.
pub const CONTRACT_RISK_AXIS: &str = "contract";

fn mul(a: Decimal, b: Decimal, what: &str) -> Result<Decimal> {
    a.checked_mul(b)
        .ok_or_else(|| Error::numeric(format!("{what} overflows the fixed-point range")))
}

fn div(a: Decimal, b: Decimal, what: &str) -> Result<Decimal> {
    a.checked_div(b)
        .ok_or_else(|| Error::numeric(format!("{what} is not representable ({a} / {b})")))
}

fn sub(a: Decimal, b: Decimal, what: &str) -> Result<Decimal> {
    a.checked_sub(b)
        .ok_or_else(|| Error::numeric(format!("{what} overflows the fixed-point range")))
}

fn add(a: Decimal, b: Decimal, what: &str) -> Result<Decimal> {
    a.checked_add(b)
        .ok_or_else(|| Error::numeric(format!("{what} overflows the fixed-point range")))
}

/// The invariant a pool prices against.
///
/// Two arms, because these are the two the blueprint names and the two that
/// account for essentially every automated market maker a desk would meet. A
/// third arm would need its own arithmetic; it would not need a different
/// [`PoolState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolCurve {
    /// `x · y = k`, the whole real line of prices, liquidity spread thin.
    ConstantProduct,
    /// Liquidity concentrated between two prices, quoted in units of the
    /// input asset per unit of the output asset.
    ///
    /// The reserves a caller states for this arm are the pool's *virtual*
    /// reserves — the ones the concentrated position behaves as a
    /// constant-product pool over while the price stays inside the range.
    /// Outside the range the position is entirely one asset and this model
    /// refuses rather than pretending to know the next tick's liquidity.
    ConcentratedLiquidity {
        /// The low end of the range, in input per output.
        lower: Decimal,
        /// The high end. Strictly above `lower`.
        upper: Decimal,
    },
}

impl PoolCurve {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ConstantProduct => "constant_product",
            Self::ConcentratedLiquidity { .. } => "concentrated_liquidity",
        }
    }
}

/// A pool, as much of it as pricing one trade needs.
///
/// Private fields and one refusing constructor. A pool with a zero reserve
/// prices everything at infinity and a pool with a fee of ten thousand basis
/// points takes the whole trade, and either would be a number this model
/// produced rather than a venue it described.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PoolStateWire")]
pub struct PoolState {
    curve: PoolCurve,
    reserve_in: Decimal,
    reserve_out: Decimal,
    fee_bps: Decimal,
}

/// The deserialised shape, routed through [`PoolState::new`] so a pool read
/// from a configuration file meets the same refusals as one built in code.
#[derive(Deserialize)]
struct PoolStateWire {
    curve: PoolCurve,
    reserve_in: Decimal,
    reserve_out: Decimal,
    fee_bps: Decimal,
}

impl TryFrom<PoolStateWire> for PoolState {
    type Error = Error;

    fn try_from(wire: PoolStateWire) -> Result<Self> {
        Self::new(wire.curve, wire.reserve_in, wire.reserve_out, wire.fee_bps)
    }
}

impl PoolState {
    /// Refuses a non-positive reserve, a fee outside `[0, 10000)`, a range
    /// that is not a range, and a concentrated pool whose current price is
    /// already outside the range it declares.
    pub fn new(
        curve: PoolCurve,
        reserve_in: Decimal,
        reserve_out: Decimal,
        fee_bps: Decimal,
    ) -> Result<Self> {
        if !reserve_in.is_positive() || !reserve_out.is_positive() {
            return Err(Error::invalid(format!(
                "a pool with reserves {reserve_in}/{reserve_out} cannot price a trade; state \
                 both sides as positive quantities"
            )));
        }
        if fee_bps.is_negative() || fee_bps >= BPS_DENOMINATOR {
            return Err(Error::invalid(format!(
                "a fee of {fee_bps} basis points is not a pool fee; state it in [0, 10000)"
            )));
        }
        let mid = div(reserve_in, reserve_out, "the pool's marginal price")?;
        if let PoolCurve::ConcentratedLiquidity { lower, upper } = curve {
            if !lower.is_positive() || upper <= lower {
                return Err(Error::invalid(format!(
                    "a concentrated range of [{lower}, {upper}] is not a range; state a positive \
                     lower bound strictly below the upper"
                )));
            }
            if mid < lower || mid > upper {
                return Err(Error::invalid(format!(
                    "the pool's marginal price {mid} is outside the declared range [{lower}, \
                     {upper}]; a position whose price has left its range holds one asset only \
                     and this model will not guess at the liquidity beyond it"
                )));
            }
        }
        Ok(Self {
            curve,
            reserve_in,
            reserve_out,
            fee_bps,
        })
    }

    pub const fn curve(&self) -> PoolCurve {
        self.curve
    }

    pub const fn reserve_in(&self) -> Decimal {
        self.reserve_in
    }

    pub const fn reserve_out(&self) -> Decimal {
        self.reserve_out
    }

    pub const fn fee_bps(&self) -> Decimal {
        self.fee_bps
    }

    /// The price of an infinitesimal trade: input per unit of output.
    ///
    /// The number a chain explorer shows and the number nobody trades at.
    /// Every quote below is worse than this, and the gap is the finding.
    pub fn marginal_price(&self) -> Result<Decimal> {
        div(self.reserve_in, self.reserve_out, "the marginal price")
    }

    /// Price one trade against the curve.
    ///
    /// The fee is taken from the input before the invariant sees it, which is
    /// how both curve families charge it, and is included in the execution
    /// price the caller is told, because the fee is money the caller paid for
    /// the output they received.
    pub fn quote(&self, amount_in: Decimal) -> Result<PoolQuote> {
        if !amount_in.is_positive() {
            return Err(Error::invalid(format!(
                "{amount_in} is not a trade; state a positive input amount"
            )));
        }
        let fee_paid = div(
            mul(amount_in, self.fee_bps, "the pool fee")?,
            BPS_DENOMINATOR,
            "the pool fee",
        )?;
        let net_in = sub(amount_in, fee_paid, "the input net of fee")?;
        if !net_in.is_positive() {
            return Err(Error::invalid(format!(
                "a trade of {amount_in} is entirely consumed by the pool's {} basis point fee",
                self.fee_bps
            )));
        }
        let denominator = add(self.reserve_in, net_in, "the post-trade input reserve")?;
        let amount_out = div(
            mul(self.reserve_out, net_in, "the output amount")?,
            denominator,
            "the output amount",
        )?;
        if !amount_out.is_positive() {
            return Err(Error::invalid(format!(
                "a trade of {amount_in} receives nothing from a pool holding {} of the output \
                 asset; the trade is below the pool's representable scale",
                self.reserve_out
            )));
        }
        if amount_out >= self.reserve_out {
            return Err(Error::invalid(format!(
                "a trade of {amount_in} would take {amount_out} from a pool holding {}; a \
                 constant-product pool cannot be emptied and a size this large has no price",
                self.reserve_out
            )));
        }
        let marginal_price = self.marginal_price()?;
        let execution_price = div(amount_in, amount_out, "the execution price")?;
        // Both prices are input per unit of output, so a worse fill is a
        // larger number and the difference is never negative for a positive
        // trade against either curve.
        let slippage_bps = div(
            mul(
                sub(execution_price, marginal_price, "the price impact")?,
                BPS_DENOMINATOR,
                "the price impact in basis points",
            )?,
            marginal_price,
            "the price impact in basis points",
        )?;
        if let PoolCurve::ConcentratedLiquidity { lower, upper } = self.curve {
            let post_in = add(self.reserve_in, net_in, "the post-trade input reserve")?;
            let post_out = sub(
                self.reserve_out,
                amount_out,
                "the post-trade output reserve",
            )?;
            let post_price = div(post_in, post_out, "the post-trade marginal price")?;
            if post_price < lower || post_price > upper {
                return Err(Error::invalid(format!(
                    "a trade of {amount_in} moves the pool's price to {post_price}, outside the \
                     declared range [{lower}, {upper}]; split the order or register the venue \
                     observe-only for this size, because the liquidity beyond the range is not \
                     something this model was given"
                )));
            }
        }
        Ok(PoolQuote {
            amount_in,
            amount_out,
            fee_paid,
            marginal_price,
            execution_price,
            slippage_bps,
        })
    }
}

/// What one trade against a pool would actually do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolQuote {
    pub amount_in: Decimal,
    pub amount_out: Decimal,
    /// The pool's fee, in input units, already inside `execution_price`.
    pub fee_paid: Decimal,
    /// Input per unit of output for an infinitesimal trade.
    pub marginal_price: Decimal,
    /// Input per unit of output for *this* trade, fee included.
    pub execution_price: Decimal,
    /// How much worse than marginal, in basis points. Never negative.
    pub slippage_bps: Decimal,
}

/// How a chain executes: in blocks, and not certainly.
///
/// The type exists so that the two facts an order-book adapter is allowed to
/// assume — that a fill is immediate and that a fill is final — cannot be
/// assumed here by omission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BlockExecutionWire")]
pub struct BlockExecution {
    block_interval: Duration,
    confirmations: u32,
}

#[derive(Deserialize)]
struct BlockExecutionWire {
    block_interval: Duration,
    confirmations: u32,
}

impl TryFrom<BlockExecutionWire> for BlockExecution {
    type Error = Error;

    fn try_from(wire: BlockExecutionWire) -> Result<Self> {
        Self::new(wire.block_interval, wire.confirmations)
    }
}

impl BlockExecution {
    /// Refuses a non-positive interval and a confirmation count of zero.
    ///
    /// Zero confirmations is the interesting refusal: a trade nobody waited
    /// to confirm is a trade that a reorganisation can remove from history
    /// after the platform has booked it, and a venue model that admitted it
    /// would be describing a settlement guarantee no chain offers.
    pub fn new(block_interval: Duration, confirmations: u32) -> Result<Self> {
        if block_interval.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "a block interval of {} nanoseconds is not block time; state the chain's \
                 observed interval",
                block_interval.as_nanos()
            )));
        }
        if confirmations == 0 {
            return Err(Error::invalid(
                "zero confirmations is not a settlement rule: a trade nobody waited to confirm \
                 can be reorganised out of history after the platform has booked it. State the \
                 number of blocks this venue's fills are believed after",
            ));
        }
        Ok(Self {
            block_interval,
            confirmations,
        })
    }

    pub const fn block_interval(&self) -> Duration {
        self.block_interval
    }

    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }

    /// How long an intent is public and unconfirmed: the window an adversary
    /// has, and the window a fill cannot be relied upon within.
    pub fn settlement_window(&self) -> Result<Duration> {
        let confirmations = i64::from(self.confirmations);
        self.block_interval
            .as_nanos()
            .checked_mul(confirmations)
            .map(Duration::from_nanos)
            .ok_or_else(|| {
                Error::numeric(format!(
                    "{confirmations} blocks of {} nanoseconds is not a representable window",
                    self.block_interval.as_nanos()
                ))
            })
    }

    /// Always false, and nothing sets it.
    ///
    /// A quote read from a pool is a fact about a state that anyone may
    /// change before the trade lands. This mirrors
    /// `qip_contracts::venue::VenueClass::quotes_are_firm`, which is already
    /// false for `DecentralisedExchange`; it is restated on the execution
    /// mode so that a caller holding a [`BlockExecution`] and no `VenueClass`
    /// still cannot conclude otherwise.
    pub const fn quotes_are_firm(&self) -> bool {
        false
    }

    /// Always true, and nothing clears it.
    ///
    /// The mempool is public. Every intent is visible between submission and
    /// inclusion, which is the whole reason [`MevEstimate`] exists.
    pub const fn mempool_is_public(&self) -> bool {
        true
    }
}

/// What a reordering adversary can take out of one trade.
///
/// # The model, and its one honest claim
///
/// A sandwich works by moving the price against the victim before their trade
/// and back after it. What bounds the attacker is the victim's own slippage
/// tolerance: the attacker pushes the price to the worst level the victim
/// said they would accept, and no further, because beyond it the victim's
/// trade reverts and the attack pays for nothing. So the extractable amount
/// is the *headroom* — the distance between the price the pool would give
/// this trade alone and the worst price the trade is willing to sign for.
///
/// That is a bound, not a forecast. It does not claim an adversary is
/// present, and it does not model gas auctions, priority fees, or private
/// order flow. It claims one thing that is true of every public mempool: a
/// trade that signs for 200 basis points of slippage and needs 30 has offered
/// 170 to whoever sees it first, and a trade that signs for exactly what it
/// needs has offered nothing.
///
/// The consequence is deliberately counter-intuitive and worth stating: a
/// *wide* tolerance is the risk, not a thin pool. A desk that sets a generous
/// tolerance to avoid reverts is paying for that convenience in a currency
/// this number denominates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MevEstimate {
    /// The headroom an adversary can take, in basis points of the trade's
    /// input. Zero when the tolerance is exactly the quote's own slippage.
    pub extractable_bps: Decimal,
    /// The same, in input units, so a caller comparing it against a fee or an
    /// expected edge does not have to re-derive it.
    pub extractable_amount: Decimal,
    /// The tolerance the estimate was taken against.
    pub tolerance_bps: Decimal,
    /// The quote's own slippage, restated so the arithmetic is checkable from
    /// the estimate alone.
    pub slippage_bps: Decimal,
    /// How long the intent is visible and unconfirmed.
    pub exposure: Duration,
}

impl MevEstimate {
    /// Price the headroom one trade leaves on the table.
    ///
    /// Refuses a negative tolerance, and refuses a tolerance below the
    /// quote's own slippage rather than raising it: an order that cannot
    /// execute at the tolerance it states is a caller bug, and correcting it
    /// here would submit a trade at a price the caller never agreed to.
    pub fn of(
        quote: &PoolQuote,
        tolerance_bps: Decimal,
        execution: &BlockExecution,
    ) -> Result<Self> {
        if tolerance_bps.is_negative() {
            return Err(Error::invalid(format!(
                "a slippage tolerance of {tolerance_bps} basis points is not a tolerance; state \
                 a non-negative figure"
            )));
        }
        if tolerance_bps < quote.slippage_bps {
            return Err(Error::invalid(format!(
                "a tolerance of {tolerance_bps} basis points cannot execute a trade the pool \
                 prices {} basis points from marginal; reduce the size or raise the tolerance \
                 deliberately, because raising it here would sign for a price the caller did not",
                quote.slippage_bps
            )));
        }
        let extractable_bps = sub(tolerance_bps, quote.slippage_bps, "the MEV headroom")?;
        let extractable_amount = div(
            mul(quote.amount_in, extractable_bps, "the MEV headroom")?,
            BPS_DENOMINATOR,
            "the MEV headroom",
        )?;
        Ok(Self {
            extractable_bps,
            extractable_amount,
            tolerance_bps,
            slippage_bps: quote.slippage_bps,
            exposure: execution.settlement_window()?,
        })
    }

    /// The whole cost of crossing, in basis points: what the curve takes and
    /// what an adversary can take, together.
    ///
    /// This is the figure a feasibility gate reads. The two are additive
    /// because they are charged against the same notional on the same trade,
    /// and a gate reading only slippage would admit a trade whose headroom
    /// costs more than its edge.
    pub fn total_cost_bps(&self) -> Result<Decimal> {
        add(
            self.slippage_bps,
            self.extractable_bps,
            "the total cost of crossing",
        )
    }
}

/// The deployed contract every position at this venue depends on.
///
/// A venue's counterparty risk is normally an entity: it can be called, sued,
/// or asked to explain a break. A contract can do none of those, and the risk
/// it carries is not the risk of a firm failing but of code behaving as
/// written. So it is a distinct exposure with its own axis
/// ([`CONTRACT_RISK_AXIS`]) rather than a counterparty with an unusual name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ContractRiskWire")]
pub struct ContractRisk {
    contract: String,
    audited_at: Option<Timestamp>,
    upgradeable: bool,
}

#[derive(Deserialize)]
struct ContractRiskWire {
    contract: String,
    #[serde(default)]
    audited_at: Option<Timestamp>,
    upgradeable: bool,
}

impl TryFrom<ContractRiskWire> for ContractRisk {
    type Error = Error;

    fn try_from(wire: ContractRiskWire) -> Result<Self> {
        Self::new(wire.contract, wire.audited_at, wire.upgradeable)
    }
}

impl ContractRisk {
    /// Refuses a blank contract identifier.
    ///
    /// An unnamed contract cannot be the key of an exposure bucket, and a
    /// contract-risk flag that all venues share is a flag that measures
    /// nothing.
    pub fn new(
        contract: impl Into<String>,
        audited_at: Option<Timestamp>,
        upgradeable: bool,
    ) -> Result<Self> {
        let contract = contract.into();
        if contract.trim().is_empty() {
            return Err(Error::invalid(
                "a contract-risk flag with no contract names nothing; state the deployed \
                 contract every position at this venue depends on",
            ));
        }
        Ok(Self {
            contract,
            audited_at,
            upgradeable,
        })
    }

    pub fn contract(&self) -> &str {
        &self.contract
    }

    pub const fn audited_at(&self) -> Option<Timestamp> {
        self.audited_at
    }

    pub const fn upgradeable(&self) -> bool {
        self.upgradeable
    }

    /// The axis and bucket a risk envelope charges this exposure to.
    ///
    /// Returned as a pair so a caller inserts it into the axis map it already
    /// builds rather than learning a second mechanism.
    pub fn axis(&self) -> (String, String) {
        (CONTRACT_RISK_AXIS.to_string(), self.contract.clone())
    }

    /// Whether the code behind every position here can change under it.
    ///
    /// An upgradeable, unaudited contract is the case where "counterparty
    /// risk is the smart contract" is at its most literal: the thing the
    /// platform assessed is not necessarily the thing it will be trading
    /// against tomorrow.
    pub const fn is_mutable_and_unreviewed(&self) -> bool {
        self.upgradeable && self.audited_at.is_none()
    }
}

/// The four pieces blueprint §34.3 requires of a decentralised venue, and the
/// honest answer when one is missing.
///
/// Each is optional in the type because the blueprint's fallback is a real
/// state, not an error: "where these are absent, the venue is registered as
/// observe-only until they exist". A model that refused to exist without all
/// four would leave a desk with nothing to register and a venue with no
/// record of what it still owes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DexModel {
    pool: Option<PoolState>,
    execution: Option<BlockExecution>,
    /// The slippage tolerance orders at this venue are submitted under. This
    /// is the MEV piece: without it no headroom can be computed, and an
    /// adversary's take would be a number nobody stated.
    tolerance_bps: Option<Decimal>,
    contract: Option<ContractRisk>,
}

/// The four pieces, as the names [`DexModel::missing`] reports.
pub const DEX_MODEL_PIECES: [&str; 4] = [
    "pool_math",
    "block_execution",
    "mev_estimate",
    "contract_risk",
];

impl DexModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_pool(mut self, pool: PoolState) -> Self {
        self.pool = Some(pool);
        self
    }

    pub fn with_execution(mut self, execution: BlockExecution) -> Self {
        self.execution = Some(execution);
        self
    }

    /// Refuses a negative tolerance at the point it is declared, so a model
    /// that exists is a model that can quote.
    pub fn with_tolerance_bps(mut self, tolerance_bps: Decimal) -> Result<Self> {
        if tolerance_bps.is_negative() {
            return Err(Error::invalid(format!(
                "a slippage tolerance of {tolerance_bps} basis points is not a tolerance; state \
                 a non-negative figure"
            )));
        }
        self.tolerance_bps = Some(tolerance_bps);
        Ok(self)
    }

    pub fn with_contract(mut self, contract: ContractRisk) -> Self {
        self.contract = Some(contract);
        self
    }

    pub fn pool(&self) -> Option<&PoolState> {
        self.pool.as_ref()
    }

    pub fn execution(&self) -> Option<&BlockExecution> {
        self.execution.as_ref()
    }

    pub const fn tolerance_bps(&self) -> Option<Decimal> {
        self.tolerance_bps
    }

    pub fn contract(&self) -> Option<&ContractRisk> {
        self.contract.as_ref()
    }

    /// Which of the four pieces this venue still owes, in the fixed order of
    /// [`DEX_MODEL_PIECES`].
    ///
    /// A `Vec` of source literals: bounded by this file, so a finding or a
    /// gate keyed on it is bounded too.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.pool.is_none() {
            missing.push(DEX_MODEL_PIECES[0]);
        }
        if self.execution.is_none() {
            missing.push(DEX_MODEL_PIECES[1]);
        }
        if self.tolerance_bps.is_none() {
            missing.push(DEX_MODEL_PIECES[2]);
        }
        if self.contract.is_none() {
            missing.push(DEX_MODEL_PIECES[3]);
        }
        missing
    }

    /// The blueprint's fallback state: true until all four pieces exist.
    pub fn is_observe_only(&self) -> bool {
        !self.missing().is_empty()
    }

    /// Price one trade, end to end, or say which piece is missing.
    ///
    /// The one entry point a caller needs, and the reason the four pieces are
    /// held together rather than passed around separately: a quote without a
    /// block time is a quote that pretends to be immediate, and an MEV
    /// estimate without a quote has no headroom to measure.
    pub fn quote(&self, amount_in: Decimal) -> Result<DexQuote> {
        // All four, not the three the arithmetic below happens to need. The
        // contract-risk flag contributes no term to a price, and a first cut
        // of this function destructured only the three that do — so a venue
        // whose contract nobody had named still quoted, and the blueprint's
        // observe-only fallback was a state the model could describe and
        // never enter. The exposure a contract carries is not optional
        // because it does not appear in a quote.
        if self.is_observe_only() {
            return Err(Error::denied(format!(
                "this venue is observe-only: it still owes {}. A decentralised venue quotes \
                 nothing until all four of §34.3's pieces exist — its pool math, its block-time \
                 execution mode, the slippage tolerance an MEV estimate is taken against, and \
                 the contract every position at it depends on",
                self.missing().join(", ")
            )));
        }
        let (Some(pool), Some(execution), Some(tolerance_bps)) = (
            self.pool.as_ref(),
            self.execution.as_ref(),
            self.tolerance_bps,
        ) else {
            return Err(Error::denied(
                "this venue is observe-only; the pieces it owes were reported by `missing`",
            ));
        };
        let pool_quote = pool.quote(amount_in)?;
        let mev = MevEstimate::of(&pool_quote, tolerance_bps, execution)?;
        Ok(DexQuote {
            pool: pool_quote,
            mev,
            settlement: execution.settlement_window()?,
        })
    }
}

/// One trade at a decentralised venue, priced against all four pieces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DexQuote {
    pub pool: PoolQuote,
    pub mev: MevEstimate,
    /// How long before the fill is believed.
    pub settlement: Duration,
}

impl DexQuote {
    /// Slippage plus headroom, in basis points: the figure a feasibility gate
    /// compares against an expected edge.
    pub fn total_cost_bps(&self) -> Result<Decimal> {
        self.mev.total_cost_bps()
    }

    /// The same cost in input units, which is what a minimum ticket size is
    /// derived from.
    pub fn total_cost_amount(&self) -> Result<Decimal> {
        let cost_bps = self.total_cost_bps()?;
        div(
            mul(self.pool.amount_in, cost_bps, "the cost of crossing")?,
            BPS_DENOMINATOR,
            "the cost of crossing",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn constant_product() -> PoolState {
        // A thousand of the input asset against a thousand of the output, at
        // thirty basis points — the fee every constant-product pool a desk
        // would meet actually charges.
        PoolState::new(
            PoolCurve::ConstantProduct,
            dec!("1000"),
            dec!("1000"),
            dec!("30"),
        )
        .expect("a valid pool")
    }

    #[test]
    fn a_trade_against_the_curve_executes_worse_than_the_price_the_pool_advertises() {
        // The whole of §34.3's first row: slippage does not come from depth,
        // it comes from the curve, and it is non-zero for any size.
        let pool = constant_product();
        let marginal = pool.marginal_price().expect("a price");
        assert_eq!(marginal, dec!("1"), "the premise: a balanced pool quotes 1");
        let quote = pool.quote(dec!("100")).expect("a quote");
        assert!(
            quote.execution_price > marginal,
            "a trade executed at or better than the marginal price against a constant-product \
             curve, which cannot happen"
        );
        assert!(quote.slippage_bps.is_positive());
        // 100 in, 0.3 of it fee, 99.7 net: out = 1000*99.7/1099.7.
        assert_eq!(quote.fee_paid, dec!("0.3"));
        assert_eq!(quote.amount_out.round_dp(6), dec!("90.661089"));
    }

    #[test]
    fn a_larger_trade_against_the_same_pool_pays_strictly_more_slippage() {
        // The property that makes this pool math rather than a fee table: a
        // flat per-trade cost would price both of these identically.
        let pool = constant_product();
        let small = pool.quote(dec!("1")).expect("a quote");
        let large = pool.quote(dec!("100")).expect("a quote");
        assert!(small.slippage_bps.is_positive(), "the premise: both slip");
        assert!(
            large.slippage_bps > small.slippage_bps,
            "a hundred-unit trade slipped no more than a one-unit trade: {} vs {}",
            large.slippage_bps,
            small.slippage_bps
        );
    }

    #[test]
    fn a_pool_with_a_zero_reserve_is_refused_rather_than_priced_at_infinity() {
        let refused = PoolState::new(
            PoolCurve::ConstantProduct,
            dec!("0"),
            dec!("1000"),
            dec!("30"),
        );
        assert!(refused.is_err());
        assert!(
            refused
                .err()
                .map(|error| error.message().contains("cannot price a trade"))
                .unwrap_or(false)
        );
    }

    #[test]
    fn a_concentrated_pool_refuses_a_trade_that_would_leave_its_range_rather_than_extrapolate() {
        // The refusal that keeps this model honest. Inside the range it is a
        // constant-product pool over virtual reserves and prices exactly;
        // outside it, the liquidity is in ticks nobody gave this model, and
        // a number produced there would be invented.
        let pool = PoolState::new(
            PoolCurve::ConcentratedLiquidity {
                lower: dec!("0.95"),
                upper: dec!("1.05"),
            },
            dec!("1000"),
            dec!("1000"),
            dec!("30"),
        )
        .expect("a valid pool");
        // Premise: a small trade stays inside the range and is priced.
        let inside = pool.quote(dec!("10")).expect("a quote inside the range");
        assert!(inside.slippage_bps.is_positive());
        let outside = pool.quote(dec!("100"));
        assert!(
            outside.is_err(),
            "a trade that moves the price out of the declared range was priced anyway"
        );
        assert!(
            outside
                .err()
                .map(|error| error.message().contains("outside the declared range"))
                .unwrap_or(false)
        );
    }

    #[test]
    fn the_mev_estimate_is_the_headroom_a_trade_leaves_and_nothing_else() {
        let pool = constant_product();
        let execution =
            BlockExecution::new(Duration::from_secs(12), 2).expect("a valid execution mode");
        let quote = pool.quote(dec!("10")).expect("a quote");
        // Premise: this trade slips, so a tolerance equal to its slippage is
        // the tight case rather than a trivially-zero one.
        assert!(quote.slippage_bps.is_positive());
        let tight = MevEstimate::of(&quote, quote.slippage_bps, &execution).expect("an estimate");
        assert_eq!(
            tight.extractable_bps,
            Decimal::ZERO,
            "a trade signing for exactly what it needs offered an adversary something"
        );
        let generous = MevEstimate::of(
            &quote,
            quote.slippage_bps.checked_add(dec!("170")).expect("a sum"),
            &execution,
        )
        .expect("an estimate");
        assert_eq!(generous.extractable_bps, dec!("170"));
        // 170 basis points of a 10-unit trade.
        assert_eq!(generous.extractable_amount, dec!("0.17"));
        assert_eq!(generous.exposure, Duration::from_secs(24));
    }

    #[test]
    fn a_tolerance_below_the_trades_own_slippage_is_refused_and_never_raised_to_fit() {
        let pool = constant_product();
        let execution =
            BlockExecution::new(Duration::from_secs(12), 1).expect("a valid execution mode");
        let quote = pool.quote(dec!("100")).expect("a quote");
        assert!(quote.slippage_bps > dec!("1"), "the premise: it slips");
        let refused = MevEstimate::of(&quote, dec!("1"), &execution);
        assert!(
            refused.is_err(),
            "a tolerance too tight to execute was accepted"
        );
        assert!(
            refused
                .err()
                .map(|error| error.message().contains("cannot execute"))
                .unwrap_or(false)
        );
    }

    #[test]
    fn zero_confirmations_is_refused_because_a_reorganisation_would_unbook_the_fill() {
        assert!(BlockExecution::new(Duration::from_secs(12), 0).is_err());
        assert!(BlockExecution::new(Duration::ZERO, 1).is_err());
        // The premise: a well-formed mode is admitted, so the refusals above
        // are about their own inputs and not about a constructor that
        // refuses everything.
        let ok = BlockExecution::new(Duration::from_secs(12), 3).expect("a valid execution mode");
        assert_eq!(
            ok.settlement_window().expect("a window"),
            Duration::from_secs(36)
        );
        assert!(!ok.quotes_are_firm());
        assert!(ok.mempool_is_public());
    }

    #[test]
    fn a_model_missing_any_piece_is_observe_only_and_quotes_nothing() {
        // §34.3's own fallback, and the order of `missing` is the order of
        // `DEX_MODEL_PIECES` so a finding keyed on it reads the same twice.
        let empty = DexModel::new();
        assert!(empty.is_observe_only());
        assert_eq!(empty.missing(), DEX_MODEL_PIECES.to_vec());
        assert!(empty.quote(dec!("10")).is_err());

        let three = DexModel::new()
            .with_pool(constant_product())
            .with_execution(BlockExecution::new(Duration::from_secs(12), 2).expect("a mode"))
            .with_tolerance_bps(dec!("300"))
            .expect("a tolerance");
        assert!(
            three.is_observe_only(),
            "a venue with no contract-risk flag quoted as though it had one"
        );
        assert_eq!(three.missing(), vec!["contract_risk"]);
        assert!(three.quote(dec!("10")).is_err());

        let complete = three
            .with_contract(ContractRisk::new("0xpool", None, true).expect("a contract-risk flag"));
        assert!(!complete.is_observe_only());
        assert!(complete.missing().is_empty());
        let quoted = complete.quote(dec!("10")).expect("a quote");
        assert_eq!(quoted.settlement, Duration::from_secs(24));
        assert!(quoted.total_cost_bps().expect("a cost") > quoted.pool.slippage_bps);
    }

    #[test]
    fn the_total_cost_of_crossing_adds_the_curves_take_to_the_adversarys() {
        // The number a feasibility gate reads. A gate reading slippage alone
        // would admit a trade whose headroom costs more than its edge, which
        // is the §34.3 failure this figure exists to prevent.
        let pool = constant_product();
        let execution = BlockExecution::new(Duration::from_secs(12), 1).expect("a mode");
        let quote = pool.quote(dec!("10")).expect("a quote");
        let mev = MevEstimate::of(
            &quote,
            quote.slippage_bps.checked_add(dec!("50")).expect("a sum"),
            &execution,
        )
        .expect("an estimate");
        assert_eq!(
            mev.total_cost_bps().expect("a total"),
            quote.slippage_bps.checked_add(dec!("50")).expect("a sum")
        );
    }

    #[test]
    fn a_contract_risk_flag_names_its_own_exposure_bucket_and_refuses_a_blank_contract() {
        assert!(ContractRisk::new("   ", None, false).is_err());
        let risk = ContractRisk::new("0xdeadbeef", None, true).expect("a flag");
        assert_eq!(
            risk.axis(),
            (CONTRACT_RISK_AXIS.to_string(), "0xdeadbeef".to_string())
        );
        assert_ne!(
            CONTRACT_RISK_AXIS, "counterparty",
            "contract risk shared the counterparty axis, so a book concentrated in one contract \
             would read as diversified across many counterparties"
        );
        assert!(risk.is_mutable_and_unreviewed());
        let audited =
            ContractRisk::new("0xdeadbeef", Some(Timestamp::from_secs(1)), true).expect("a flag");
        assert!(!audited.is_mutable_and_unreviewed());
    }

    #[test]
    fn a_pool_deserialised_from_a_file_meets_the_same_refusals_as_one_built_in_code() {
        // The path a runtime check misses: a venue descriptor read from
        // configuration. `serde(try_from)` routes it through `new`.
        let bad =
            r#"{"curve":"constant_product","reserve_in":"0","reserve_out":"1000","fee_bps":"30"}"#;
        assert!(serde_json::from_str::<PoolState>(bad).is_err());
        let good = r#"{"curve":"constant_product","reserve_in":"1000","reserve_out":"1000","fee_bps":"30"}"#;
        let pool: PoolState = serde_json::from_str(good).expect("a valid pool");
        assert_eq!(pool.reserve_in(), dec!("1000"));
    }
}
