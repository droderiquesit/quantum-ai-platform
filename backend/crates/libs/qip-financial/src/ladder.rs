//! The liquidity ladder: what can become cash, how fast, and at what cost
//! (blueprint §25.4).
//!
//! The ladder orders everything the platform holds into seven rungs, from cash
//! at a venue down to private-equity commitments. Its defining property is
//! that cost to liquidate rises monotonically as you descend, and a request
//! for cash is served from the top downward — so a routine reduction is met
//! from the cheapest rungs and never by forcing a position closed at a bad
//! price.
//!
//! # This describes depth. It never reaches for it.
//!
//! Nothing in this module creates, enables or eases an order path. A
//! [`LiquidationPlan`] carries object identifiers, amounts and costs; it
//! carries no venue, no side, no order type, no time in force and no
//! identifier any execution surface could act on, and there is no function
//! here that hands one to anything. It is an answer to "where would the money
//! come from, and what would it cost", computed so that a risk read or a
//! divestment ranking can be argued about before anything is decided. Turning
//! a plan into orders would be a separate, reviewed act in the execution
//! domain, under the paper-trading boundary that domain already holds.
//!
//! The blueprint's own caller for this — serving a withdrawal from the top of
//! the ladder downward — does not exist and cannot. ADR 0021 and ADR 0023
//! refuse the path by which capital leaves the platform, and
//! `qip_capital::ledger::WithdrawalEntitlement` has exactly one variant,
//! `Refused`. The ladder is therefore a valuation and risk instrument here:
//! it says how much of the book is reachable within a horizon, which is a
//! question worth answering whether or not anything is ever withdrawn.
//!
//! Every value and every cost is a [`Decimal`], because both are money. The
//! horizons are categorical and the rungs are an enum, so no money here is
//! ever a statistic. Two numbers are not money and both are days:
//! [`LiquidationHorizon::least_days`], the boundary of a horizon's own bucket,
//! and [`LadderEntry::measured_days_to_exit`], the record's own measurement
//! where a caller supplied one. [`LadderEntry::days_to_exit`] is the larger of
//! the two and the only one anything outside this module should read. The
//! bucket boundary alone was read for one release, and it reported a holding
//! stated at forty-five days as taking two.

use crate::asset_class::AssetClass;
use crate::costs::LiquidityProfile;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// How long a rung takes to turn into cash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiquidationHorizon {
    Immediate,
    Seconds,
    SameDay,
    Days,
    Months,
    Years,
}

impl LiquidationHorizon {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Seconds => "seconds",
            Self::SameDay => "same_day",
            Self::Days => "days",
            Self::Months => "months",
            Self::Years => "years",
        }
    }

    /// The fewest days an exit on this horizon can honestly be claimed to
    /// take.
    ///
    /// A **floor**, not an estimate: the boundary of the horizon's own bucket
    /// and nothing more. It is one of the two lower bounds
    /// [`LadderEntry::days_to_exit`] takes the larger of, and on its own it is
    /// **not** a safe answer to "how long does this holding take to leave".
    /// This doc claimed it was, in these words: "it can call a position slower
    /// to exit than it is, never faster". That claim was false, and the
    /// failure was live. `Days` floors at two, and [`Rung::classify`] puts
    /// every non-negotiated holding stated to take more than one day on
    /// [`Rung::BondsAndLessLiquidListed`] — so a record stating **forty-five**
    /// days was read as two, `MinLiquidity` counted the position as exitable
    /// within a week, and `MaxDaysToLiquidate { limit: 10.0 }` compared 2.0
    /// against 10 and recorded nothing. The record held the right number and
    /// the bucket boundary discarded it.
    ///
    /// The bucket is therefore never used alone. [`LadderEntry::days_to_exit`]
    /// carries the record's own measurement where the caller supplied one and
    /// falls back to this floor only where nobody measured, which is the one
    /// case where the boundary is the best available lower bound rather than a
    /// replacement for a better one.
    ///
    /// The values are the boundaries the horizon names, not risk parameters:
    /// `Immediate` and `Seconds` are both inside one trading day and are
    /// therefore zero days, `SameDay` is one, `Days` is two because
    /// [`Rung::classify`] reserves it for anything stated as taking more than
    /// a single day, `Months` is thirty and `Years` is
    /// three-hundred-and-sixty-five. Anyone tempted to tune one of these is
    /// tuning a calendar, which is the signal that the limit wanted a
    /// different horizon rather than a different number.
    pub fn least_days(&self) -> f64 {
        match self {
            Self::Immediate | Self::Seconds => 0.0,
            Self::SameDay => 1.0,
            Self::Days => 2.0,
            Self::Months => 30.0,
            Self::Years => 365.0,
        }
    }
}

/// A rung of the liquidity ladder, blueprint §25.4.
///
/// The declaration order **is** the ladder order, top first, and the derived
/// [`Ord`] is what a [`BTreeMap`] keyed on a rung iterates in. That is
/// deliberate rather than incidental: the ordering reaches output, so it must
/// be a property of the type rather than of a sort call somebody could forget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rung {
    /// Cash and stablecoin at a venue. Immediate, zero cost.
    CashAtVenue,
    /// Liquid spot and perpetual. Seconds; spread and fee.
    LiquidSpotAndPerpetual,
    /// Listed equities and futures. Same day; spread, fee and impact.
    ListedEquityAndFutures,
    /// Bonds and less liquid listed. Days; wider spread, dealer inventory.
    BondsAndLessLiquidListed,
    /// Resting and anchored positions. Days, by abandoning the cycle;
    /// opportunity cost plus unwind.
    RestingAndAnchored,
    /// Private credit and real assets. Months, if at all; substantial discount.
    PrivateCreditAndRealAssets,
    /// Private-equity commitments. Years, or a secondary at a discount.
    PrivateEquityCommitments,
}

impl Rung {
    /// Every rung, top of the ladder first.
    pub const ALL: [Self; 7] = [
        Self::CashAtVenue,
        Self::LiquidSpotAndPerpetual,
        Self::ListedEquityAndFutures,
        Self::BondsAndLessLiquidListed,
        Self::RestingAndAnchored,
        Self::PrivateCreditAndRealAssets,
        Self::PrivateEquityCommitments,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CashAtVenue => "cash_at_venue",
            Self::LiquidSpotAndPerpetual => "liquid_spot_and_perpetual",
            Self::ListedEquityAndFutures => "listed_equity_and_futures",
            Self::BondsAndLessLiquidListed => "bonds_and_less_liquid_listed",
            Self::RestingAndAnchored => "resting_and_anchored",
            Self::PrivateCreditAndRealAssets => "private_credit_and_real_assets",
            Self::PrivateEquityCommitments => "private_equity_commitments",
        }
    }

    /// Depth from the top of the ladder, `0` being cash.
    pub fn depth(&self) -> usize {
        match self {
            Self::CashAtVenue => 0,
            Self::LiquidSpotAndPerpetual => 1,
            Self::ListedEquityAndFutures => 2,
            Self::BondsAndLessLiquidListed => 3,
            Self::RestingAndAnchored => 4,
            Self::PrivateCreditAndRealAssets => 5,
            Self::PrivateEquityCommitments => 6,
        }
    }

    pub fn horizon(&self) -> LiquidationHorizon {
        match self {
            Self::CashAtVenue => LiquidationHorizon::Immediate,
            Self::LiquidSpotAndPerpetual => LiquidationHorizon::Seconds,
            Self::ListedEquityAndFutures => LiquidationHorizon::SameDay,
            Self::BondsAndLessLiquidListed | Self::RestingAndAnchored => LiquidationHorizon::Days,
            Self::PrivateCreditAndRealAssets => LiquidationHorizon::Months,
            Self::PrivateEquityCommitments => LiquidationHorizon::Years,
        }
    }

    /// The rung an instrument sits on, from its class and its liquidity, or a
    /// refusal naming the figure that could not be read.
    ///
    /// Two facts decide it, and the second can only push an object *down*: an
    /// instrument that trades by negotiation cannot settle in seconds however
    /// its asset class is labelled, and one that takes more than a day to exit
    /// is not a same-day rung. That is classification from evidence, not a
    /// correction of a bad input — a negotiated equity in a private placement
    /// is genuinely on a lower rung than a listed one.
    ///
    /// **A `days_to_liquidate` that is not a number of days is refused rather
    /// than compared.** The comparison that pushes an instrument below its
    /// class is `> 1.0`, and `f64` makes that `false` for `NaN` and for a
    /// negative: the liquidity arm then answered [`Self::CashAtVenue`], which
    /// can push nothing down, so an instrument nobody had measured kept its
    /// asset class's rung and the book reported it exitable on that class's
    /// horizon. An equity with `NaN` days classified `ListedEquityAndFutures`
    /// — same day. Infinity was worse in the other direction: `inf > 1.0` is
    /// `true`, so an instrument stated never to liquidate landed on
    /// [`Self::BondsAndLessLiquidListed`] and read as exitable in two days.
    /// `credit.rs` in this crate refuses a non-finite covenant observation for
    /// exactly this reason; a liquidity measurement is no different, and this
    /// one feeds a floor that vetoes trading.
    ///
    /// The refusal is deliberate and it fires early: `qip-kernel`'s
    /// `ladder_reference_of` classifies every universe record at assembly, so
    /// a catalogue carrying such a figure stops `Platform::new` rather than
    /// producing a liquidity floor computed over an instrument nobody
    /// measured.
    ///
    /// [`Rung::RestingAndAnchored`] is never returned. A position is on that
    /// rung because a strategy is holding it deliberately, and no property of
    /// the instrument reveals that; the caller who knows the strategy sets it.
    ///
    /// **The rung this returns is a cost ordering and no longer an exit
    /// time.** `days > 1.0` puts everything from a day and a half to a decade
    /// on [`Self::BondsAndLessLiquidListed`], whose horizon floors at two
    /// days, and for one release that boundary *was* the exit time every risk
    /// limit read. It is not, now: the caller carries the record's own
    /// `days_to_liquidate` onto the entry through
    /// [`LadderEntry::exiting_over_days`], and [`LadderEntry::days_to_exit`]
    /// reads the larger of the two. Widening the buckets here would not have
    /// fixed it — a bucket one order of magnitude away is still a bucket —
    /// which is why this mapping is deliberately unchanged.
    pub fn classify(class: AssetClass, liquidity: &LiquidityProfile) -> Result<Self> {
        let days = liquidity.days_to_liquidate;
        if !days.is_finite() || days < 0.0 {
            return Err(Error::invalid(format!(
                "a liquidity record states {days} days to liquidate, which is not a number of \
                 days an exit can take; correct the record before placing the holding — the \
                 ladder cannot read this figure, and a figure it cannot read leaves the holding \
                 on its asset class's rung and reports the book more exitable than it is"
            )));
        }
        let by_class = match class {
            AssetClass::Cash => Self::CashAtVenue,
            AssetClass::DigitalAsset | AssetClass::ForeignExchange => Self::LiquidSpotAndPerpetual,
            AssetClass::Equity
            | AssetClass::Derivative
            | AssetClass::Commodity
            | AssetClass::Rates => Self::ListedEquityAndFutures,
            AssetClass::FixedIncome
            | AssetClass::Credit
            | AssetClass::Fund
            | AssetClass::StructuredProduct => Self::BondsAndLessLiquidListed,
            AssetClass::RealAsset => Self::PrivateCreditAndRealAssets,
            AssetClass::PrivateMarket => Self::PrivateEquityCommitments,
        };
        let by_liquidity = if liquidity.is_negotiated {
            Self::PrivateCreditAndRealAssets
        } else if days > 1.0 {
            Self::BondsAndLessLiquidListed
        } else {
            Self::CashAtVenue
        };
        Ok(by_class.max(by_liquidity))
    }
}

/// Refuse a set of reference records whose quoted exit rates cannot sit on
/// this ladder together, whatever a book comes to hold.
///
/// The same law [`LiquidityLadder::new`]'s monotonicity proof enforces, asked
/// one stage earlier and of the reference data rather than of a book. Each `(record,
/// rung, spread_bps)` is what one catalogue record vouches for; a holding's
/// exit cost is its mark times its own quoted spread, so a rung occupied by a
/// single holding has exactly that holding's rate, and a rung occupied by
/// several has a value-weighted mean of theirs — which is never above the
/// widest of them nor below the tightest. Monotonicity therefore holds for
/// **every** book drawable from these records exactly when, walking the
/// occupied rungs downward, no rung's widest quote exceeds the next occupied
/// rung's tightest. Adjacent comparisons suffice because the relation chains:
/// widest(a) <= tightest(b) <= widest(b) <= tightest(c).
///
/// # Why this is asked at assembly and not left to the ladder
///
/// It was left to the ladder, and the operator met it in the wrong place. A
/// listed name quoted at 300bps beside a negotiated holding whose profile
/// claimed 250 assembled without complaint, and then every cycle refused the
/// whole liquidity read with a sentence about rung arithmetic — "rung
/// listed_equity_and_futures costs more to liquidate than the lower rung
/// private_credit_and_real_assets (4500 on 150000 against 125 on 5000)" —
/// given to someone whose actual problem was one wrong reference record. Since
/// the refusal fails closed, that sentence was also the whole desk stopping.
///
/// This is deliberately **stricter than the ladder**: it refuses a catalogue
/// where *some* drawable book would invert, not only the book presently held.
/// That is the intended direction. A catalogue that trades until the day the
/// desk first buys the offending name, and then stops the desk, is the failure
/// this exists to prevent, and the two records are nameable now and not then.
///
/// The refusal names both records, both rungs and both rates, because the
/// property is relational: neither record is wrong on its own, and an operator
/// given one id could not tell which of the two to correct.
///
/// Ties are broken toward the lexicographically smaller identifier so that the
/// same catalogue names the same pair on every run. A refusal a replay could
/// attribute to a different record is not a replay.
///
/// Every comparison is [`f64::total_cmp`] rather than `>` and `==`, which
/// gives a `NaN` quote an order instead of an answer of `false` in both
/// directions. A `NaN` therefore sorts widest and is refused here, where `>`
/// would have admitted it — and [`LiquidityLadder`] would have admitted it
/// too, because `Decimal::apply_bps` turns a `NaN` rate into a zero cost, so
/// the rung would have read as free to exit. `qip-kernel`'s
/// `ladder_reference_of` refuses a non-finite spread before this is reached;
/// this is the arm that keeps that refusal from being the only thing standing
/// between a `NaN` and a liquidity floor.
pub fn prove_quotes_can_coexist<'a>(
    quotes: impl IntoIterator<Item = (&'a str, Rung, f64)>,
) -> Result<()> {
    // Widest and tightest quote on each occupied rung, with the record each
    // came from. `BTreeMap` because the walk below has to be in ladder order
    // and the pair named in a refusal has to be the same one on every replay.
    let mut extremes: BTreeMap<Rung, (&'a str, f64, &'a str, f64)> = BTreeMap::new();
    for (id, rung, spread_bps) in quotes {
        let slot = extremes
            .entry(rung)
            .or_insert((id, spread_bps, id, spread_bps));
        let (widest_id, widest_bps, tightest_id, tightest_bps) = *slot;
        match spread_bps.total_cmp(&widest_bps) {
            Ordering::Greater => *slot = (id, spread_bps, tightest_id, tightest_bps),
            Ordering::Equal if id < widest_id => {
                *slot = (id, spread_bps, tightest_id, tightest_bps);
            }
            _ => {}
        }
        let (widest_id, widest_bps, tightest_id, tightest_bps) = *slot;
        match spread_bps.total_cmp(&tightest_bps) {
            Ordering::Less => *slot = (widest_id, widest_bps, id, spread_bps),
            Ordering::Equal if id < tightest_id => {
                *slot = (widest_id, widest_bps, id, spread_bps);
            }
            _ => {}
        }
    }

    let mut above: Option<(Rung, &'a str, f64)> = None;
    for (rung, (widest_id, widest_bps, tightest_id, tightest_bps)) in extremes {
        if let Some((upper, upper_id, upper_bps)) = above
            && upper_bps.total_cmp(&tightest_bps) == Ordering::Greater
        {
            return Err(Error::invalid(format!(
                "reference records {upper_id} and {tightest_id} cannot both sit on this liquidity \
                 ladder: {upper_id} is on rung {upper} quoted at {upper_bps}bps, wider than \
                 {tightest_id} on the lower rung {lower} at {tightest_bps}bps; correct whichever \
                 record is wrong, or place them on the rungs their exit cost actually puts them \
                 on — the ladder proves cost rises as it descends, and a book holding both would \
                 refuse the whole liquidity read with a sentence about rung arithmetic instead of \
                 about these two records",
                upper = upper.as_str(),
                lower = rung.as_str()
            )));
        }
        above = Some((rung, widest_id, widest_bps));
    }
    Ok(())
}

/// One holding placed on the ladder.
///
/// [`Eq`] is deliberately absent: `measured_days_to_exit` is an `f64` and
/// there is no total equality on one. It is the only number here that is not
/// money, and it is a statistic — a measurement of how long an exit takes —
/// which is why it may be an `f64` at all.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LadderEntry {
    /// The object this value sits in.
    pub object_id: String,
    pub rung: Rung,
    /// What the holding is marked at. Money.
    pub value: Decimal,
    /// What it would cost to turn all of `value` into cash — spread, fee,
    /// impact and discount together. Money, in the same currency as `value`.
    pub cost_to_liquidate: Decimal,
    /// The record's own stated days to exit this holding, where the caller
    /// had one.
    ///
    /// `None` means nobody measured, not "zero" and not "immediately".
    /// Read through [`Self::days_to_exit`], never directly, so that the fall
    /// back to the rung's own floor happens in one place.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub measured_days_to_exit: Option<f64>,
}

impl LadderEntry {
    pub fn new(
        object_id: impl Into<String>,
        rung: Rung,
        value: Decimal,
        cost_to_liquidate: Decimal,
    ) -> Self {
        Self {
            object_id: object_id.into(),
            rung,
            value,
            cost_to_liquidate,
            measured_days_to_exit: None,
        }
    }

    /// Carry the record's own measurement of how long this holding takes to
    /// leave.
    ///
    /// The caller that holds the reference record is the only one that knows
    /// this, and the ladder cannot derive it: [`Rung`] is a cost ordering, and
    /// a rung's [`LiquidationHorizon::least_days`] is the boundary of a
    /// bucket. A holding stated at forty-five days sits on
    /// [`Rung::BondsAndLessLiquidListed`] beside one stated at one and a
    /// half, and reading the rung alone reported both as two days.
    ///
    /// `days` is validated by [`LiquidityLadder::new`], with every other
    /// refusal a lying entry can take, rather than here: an entry is a value
    /// object, and the ladder is where an entry that would make the book lie
    /// is refused.
    pub fn exiting_over_days(mut self, days: f64) -> Self {
        self.measured_days_to_exit = Some(days);
        self
    }

    /// The fewest days this holding can honestly be claimed to take to leave.
    ///
    /// The larger of the record's own measurement and the rung's floor, which
    /// is the best available lower bound rather than a preference between two
    /// numbers. Both are lower bounds on the truth: the rung's floor because
    /// the rung is a bucket, the record's because it is what somebody
    /// measured. Taking the maximum is what makes the statement "this ladder
    /// never calls a position faster to exit than it is" true; taking the
    /// rung's alone made it false, and
    /// `qip_risk::limits::LimitKind::MaxDaysToLiquidate` compared 2.0 against
    /// its bound for a holding the record said took forty-five days.
    pub fn days_to_exit(&self) -> f64 {
        let floor = self.rung.horizon().least_days();
        match self.measured_days_to_exit {
            // `f64::max` returns the other operand for a `NaN`, which would
            // silently answer the floor. `LiquidityLadder::new` refuses a
            // `NaN` measurement, so no entry reachable through the
            // constructor can take that path — this is the arm that keeps the
            // refusal from being the only thing standing between a `NaN` and
            // a liquidity floor.
            Some(days) if days.is_finite() && days > floor => days,
            _ => floor,
        }
    }
}

/// One rung's contribution to a plan.
///
/// Deliberately not an order: there is no venue, no side and no time in force
/// here, and nothing in this crate turns one into any of those.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanLeg {
    pub object_id: String,
    pub rung: Rung,
    /// Value drawn from this holding. Money.
    pub amount: Decimal,
    /// Cost of drawing exactly that much. Money.
    pub cost: Decimal,
}

/// Where a given amount of cash would come from, and what it would cost.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidationPlan {
    pub legs: Vec<PlanLeg>,
    /// Total value drawn. Equals the requested amount.
    pub raised: Decimal,
    /// Total cost of drawing it. Money.
    pub cost: Decimal,
    /// The lowest rung the plan had to reach. The number an operator reads
    /// first: a routine request that reaches private credit is a warning
    /// about the shape of the book, not about the request.
    pub deepest_rung: Rung,
}

/// The book, ordered by how readily it becomes cash.
///
/// [`Eq`] is absent for the reason it is absent on [`LadderEntry`]: an entry
/// carries a measured exit time in days, and there is no total equality on an
/// `f64`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiquidityLadder {
    /// Keyed on `(rung, object_id)` so iteration order is ladder order and a
    /// replay reproduces the same plan leg for leg.
    entries: BTreeMap<(Rung, String), LadderEntry>,
}

impl LiquidityLadder {
    /// Build a ladder, refusing anything that would make it lie.
    ///
    /// Refusals, each naming what to do instead:
    ///
    /// * a non-positive `value` — a holding worth nothing is not a rung of the
    ///   ladder, and dividing a cost across it has no meaning;
    /// * a negative `cost_to_liquidate`;
    /// * a cost exceeding the value it liquidates — a position that costs more
    ///   to exit than it is marked at is a marking error, and serving a
    ///   request from it would report raising cash while destroying more;
    /// * the same `object_id` twice, which would double-count the book;
    /// * a **non-monotonic ladder**: a lower rung cheaper to liquidate than a
    ///   higher one. The ladder's entire purpose is that serving from the top
    ///   downward is serving from the cheapest downward. If that does not
    ///   hold, the rungs are assigned wrongly and every plan built on them
    ///   picks the expensive source first;
    /// * a book whose value **does not add up inside the decimal range**.
    ///   Proved here so that every reader afterwards is reading a total
    ///   somebody computed. It was not: the sums saturated at
    ///   [`Decimal::MAX`] or dropped the entry that overflowed, so a ladder
    ///   holding one unit of cash beside 1.7e29 of spot reported a total
    ///   value of one, and a rung holding more than the range can express
    ///   reported exactly [`Decimal::MAX`] into the monotonicity proof above.
    ///   Under-reporting the book is not a safe direction here:
    ///   `reachable_within` feeds `RiskState::liquidatable_within`, which
    ///   `LimitKind::MinLiquidity` divides, and a clamp inside a control that
    ///   vetoes trading is a control reading a number nobody computed;
    /// * a **measured exit time that is not a number of days**. A `NaN` would
    ///   make [`LadderEntry::days_to_exit`] fall back to the rung's floor and
    ///   the holding would read as exitable on its bucket's horizon, which is
    ///   the exact shape of the defect carrying the measurement exists to
    ///   close. Supplying no measurement is honest; supplying one that is not
    ///   a number is not.
    pub fn new(entries: Vec<LadderEntry>) -> Result<Self> {
        let mut map: BTreeMap<(Rung, String), LadderEntry> = BTreeMap::new();
        let mut seen: BTreeMap<String, Rung> = BTreeMap::new();

        for entry in entries {
            if let Some(days) = entry.measured_days_to_exit
                && (!days.is_finite() || days < 0.0)
            {
                return Err(Error::invalid(format!(
                    "holding {id} states {days} days to exit, which is not a number of days; \
                     correct the record or place the holding with no measurement — a figure the \
                     ladder cannot read would leave the holding on its rung's own floor and \
                     report the book more exitable than it is",
                    id = entry.object_id
                )));
            }
            if !entry.value.is_positive() {
                return Err(Error::invalid(format!(
                    "holding {id} has value {value}, which is not positive; remove a written-\
                     down holding from the ladder rather than placing it on a rung",
                    id = entry.object_id,
                    value = entry.value
                )));
            }
            if entry.cost_to_liquidate.is_negative() {
                return Err(Error::invalid(format!(
                    "holding {id} has a negative cost to liquidate ({cost}); a cost is what \
                     exiting consumes — supply zero if exiting is free",
                    id = entry.object_id,
                    cost = entry.cost_to_liquidate
                )));
            }
            if entry.cost_to_liquidate > entry.value {
                return Err(Error::invalid(format!(
                    "holding {id} costs {cost} to liquidate but is marked at {value}; correct \
                     the mark or the cost model — a holding that consumes more than it \
                     realises raises no cash and this ladder will not pretend otherwise",
                    id = entry.object_id,
                    cost = entry.cost_to_liquidate,
                    value = entry.value
                )));
            }
            if let Some(existing) = seen.insert(entry.object_id.clone(), entry.rung) {
                return Err(Error::invalid(format!(
                    "holding {id} appears twice, on rungs {first} and {second}; place each \
                     holding on exactly one rung — a repeat double-counts the book",
                    id = entry.object_id,
                    first = existing.as_str(),
                    second = entry.rung.as_str()
                )));
            }
            map.insert((entry.rung, entry.object_id.clone()), entry);
        }

        let ladder = Self { entries: map };
        ladder.prove_monotonic()?;
        // The whole book, proved to add up before anything reads it. The
        // per-rung totals inside `prove_monotonic` can each fit while their
        // sum does not, so this is a second question rather than the same one.
        ladder.total_value()?;
        Ok(ladder)
    }

    /// Refuse a ladder whose cost does not rise as it descends.
    ///
    /// Rates are compared by cross-multiplication rather than by dividing:
    /// `cost_a / value_a > cost_b / value_b` becomes
    /// `cost_a * value_b > cost_b * value_a`, which keeps money in [`Decimal`]
    /// and never rounds a comparison into or out of a refusal.
    fn prove_monotonic(&self) -> Result<()> {
        let totals = self.value_and_cost_by_rung()?;
        let mut previous: Option<(Rung, Decimal, Decimal)> = None;
        for (rung, (value, cost)) in totals {
            if let Some((above, above_value, above_cost)) = previous {
                let left = above_cost.checked_mul(value).ok_or_else(|| {
                    Error::numeric(format!(
                        "comparing the cost of rung {} against rung {} overflowed; the values \
                         on this ladder are implausibly large",
                        above.as_str(),
                        rung.as_str()
                    ))
                })?;
                let right = cost.checked_mul(above_value).ok_or_else(|| {
                    Error::numeric(format!(
                        "comparing the cost of rung {} against rung {} overflowed; the values \
                         on this ladder are implausibly large",
                        above.as_str(),
                        rung.as_str()
                    ))
                })?;
                if left > right {
                    return Err(Error::invalid(format!(
                        "rung {above} costs more to liquidate than the lower rung {rung} \
                         ({above_cost} on {above_value} against {cost} on {value}); the \
                         ladder must get more expensive as it descends, so reassign these \
                         holdings to the rungs their exit cost actually places them on",
                        above = above.as_str(),
                        rung = rung.as_str()
                    )));
                }
            }
            previous = Some((rung, value, cost));
        }
        Ok(())
    }

    /// Value and cost on each occupied rung, or a refusal where a rung's
    /// total leaves the decimal range.
    ///
    /// Fallible because the alternative was a fabrication: this used to
    /// saturate at [`Decimal::MAX`] on overflow, and [`Self::prove_monotonic`]
    /// then compared a number nobody had computed against a real one and
    /// pronounced the ladder sound.
    fn value_and_cost_by_rung(&self) -> Result<BTreeMap<Rung, (Decimal, Decimal)>> {
        let mut totals: BTreeMap<Rung, (Decimal, Decimal)> = BTreeMap::new();
        for entry in self.entries.values() {
            let slot = totals
                .entry(entry.rung)
                .or_insert((Decimal::ZERO, Decimal::ZERO));
            slot.0 = slot.0.checked_add(entry.value).ok_or_else(|| {
                Error::numeric(format!(
                    "the value on rung {rung} leaves the decimal range at holding {id}; split \
                     the book or correct the marks — a saturated rung total is read by the \
                     monotonicity proof as though somebody had computed it",
                    rung = entry.rung.as_str(),
                    id = entry.object_id
                ))
            })?;
            slot.1 = slot.1.checked_add(entry.cost_to_liquidate).ok_or_else(|| {
                Error::numeric(format!(
                    "the exit cost on rung {rung} leaves the decimal range at holding {id}; \
                     split the book or correct the cost model — a saturated rung cost is read \
                     by the monotonicity proof as though somebody had computed it",
                    rung = entry.rung.as_str(),
                    id = entry.object_id
                ))
            })?;
        }
        Ok(totals)
    }

    /// Every entry, ladder order, top rung first.
    pub fn entries(&self) -> impl Iterator<Item = &LadderEntry> {
        self.entries.values()
    }

    /// Value on each occupied rung, ladder order, or a refusal where a rung's
    /// total leaves the decimal range.
    pub fn value_by_rung(&self) -> Result<BTreeMap<Rung, Decimal>> {
        Ok(self
            .value_and_cost_by_rung()?
            .into_iter()
            .map(|(rung, (value, _))| (rung, value))
            .collect())
    }

    /// Everything the ladder holds, or a refusal where the book does not add
    /// up inside the decimal range.
    ///
    /// The fold used to keep the accumulator on overflow, which **dropped the
    /// entry**: a ladder holding one unit of cash beside 1.7e29 of spot
    /// answered one. Fallible rather than saturating because both directions
    /// lie, and this total is the denominator of the fraction
    /// `LimitKind::MinLiquidity` vetoes trading on.
    ///
    /// [`Self::new`] proves this before returning, so no ladder reachable
    /// through the constructor can take the refusal. It is still a `Result`
    /// rather than a `Decimal`, because the only two ways to write a fallible
    /// sum with an infallible signature are a clamp and a panic, and this
    /// module's whole argument is that it refuses rather than lies. The proof
    /// belongs in the constructor so that a refusal reaches the cycle report
    /// through `Platform::liquidity_ladder`; the `Result` here is what makes
    /// the proof's absence impossible to write by accident.
    pub fn total_value(&self) -> Result<Decimal> {
        self.entries.values().try_fold(Decimal::ZERO, |acc, e| {
            acc.checked_add(e.value).ok_or_else(|| {
                Error::numeric(format!(
                    "the ladder's total value leaves the decimal range at holding {id}; split \
                     the book or correct the marks — a total that dropped a holding would \
                     under-report the book, and the liquidity floor divides by it",
                    id = e.object_id
                ))
            })
        })
    }

    /// How much of the book could become cash within `days`, or a refusal
    /// where that is not a number of days or the sum leaves the decimal range.
    ///
    /// The risk read the ladder exists to give: a book where nine tenths of
    /// the value is reachable only in months is a different book from one
    /// where nine tenths is reachable the same day, whatever their marks say
    /// they are worth.
    ///
    /// **Stated in days, and compared against each holding's own days.** It
    /// took a [`LiquidationHorizon`] and filtered on `entry.rung.horizon()`,
    /// so a caller with a limit in days had to map it onto a bucket first and
    /// every holding answered with its bucket's boundary. Both ends of that
    /// lost the measurement: a limit at five days became the `Days` bucket,
    /// and a holding stated at forty-five days sat in the same bucket as one
    /// stated at one and a half. The book read as fully exitable inside a
    /// week. Days against days is one comparison instead of two mappings that
    /// could disagree — which is why `LiquidationHorizon::deepest_within`,
    /// the mapping the caller used, no longer exists.
    ///
    /// A `days` that is not a number of days is **refused, not floored**: a
    /// limit configured with a nonsense horizon must read as unevaluated, and
    /// the kernel turns that refusal into an order refusal rather than into a
    /// fraction nobody computed.
    ///
    /// Fallible for the reason [`Self::total_value`] is, and it matters more
    /// here: this is the numerator `RiskState::liquidatable_within` files
    /// under the horizon `LimitKind::MinLiquidity` looks up, so an entry
    /// silently dropped is a liquidity floor evaluated against a book that
    /// was never counted.
    pub fn reachable_within(&self, days: f64) -> Result<Decimal> {
        if !days.is_finite() || days < 0.0 {
            return Err(Error::invalid(format!(
                "cannot say what is reachable within {days} days, which is not a horizon an exit \
                 can have; state the horizon in days — a liquidity floor asked this way reads as \
                 unevaluated rather than as one every book passed"
            )));
        }
        self.entries
            .values()
            .filter(|e| e.days_to_exit() <= days)
            .try_fold(Decimal::ZERO, |acc, e| {
                acc.checked_add(e.value).ok_or_else(|| {
                    Error::numeric(format!(
                        "the value reachable within {days} days leaves the decimal range at \
                         holding {id}; split the book or correct the marks — a sum that dropped \
                         a holding would report the book less exitable than it is",
                        id = e.object_id
                    ))
                })
            })
    }

    /// Where `amount` of cash would come from, served from the top downward.
    ///
    /// Refuses a non-positive amount, and refuses **a depth that exceeds the
    /// book**: a request larger than the ladder holds is not served as far as
    /// it goes, because a plan that raises less than it was asked for reads
    /// downstream as a plan that worked. The shortfall is named.
    ///
    /// A partial draw on a holding is charged pro rata: the cost of taking
    /// half a position is half its cost to liquidate. That is an assumption,
    /// and it is the conservative one for the rungs where it is wrong — impact
    /// on a listed name is sublinear in size, so a pro-rata charge on a
    /// partial exit overstates rather than understates the cost.
    pub fn plan(&self, amount: Decimal) -> Result<LiquidationPlan> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "cannot plan for {amount}; ask for a positive amount of cash"
            )));
        }
        let available = self.total_value()?;
        if amount > available {
            // Named exactly, or not at all. A shortfall floored at zero would
            // have read as "nothing missing" inside a refusal about something
            // missing.
            let short = amount.checked_sub(available).ok_or_else(|| {
                Error::numeric(format!(
                    "the shortfall between the {amount} asked for and the {available} this \
                     ladder holds leaves the decimal range; ask for an amount inside it"
                ))
            })?;
            return Err(Error::invalid(format!(
                "the ladder holds {available} but {amount} was asked for, {short} short; ask \
                 for no more than the book holds — a plan that raises less than it was asked \
                 for reads as one that succeeded"
            )));
        }

        let mut remaining = amount;
        let mut legs = Vec::new();
        let mut cost = Decimal::ZERO;
        let mut deepest = Rung::CashAtVenue;

        for entry in self.entries.values() {
            if !remaining.is_positive() {
                break;
            }
            let take = remaining.min(entry.value);
            // Pro rata: cost * take / value. `value` is positive by `new`'s
            // refusal, so the division has a defined result.
            let leg_cost = entry
                .cost_to_liquidate
                .checked_mul(take)
                .and_then(|n| n.checked_div(entry.value))
                .ok_or_else(|| {
                    Error::numeric(format!(
                        "the pro-rata exit cost of holding {id} could not be computed; check \
                         its mark and cost for implausible magnitudes",
                        id = entry.object_id
                    ))
                })?;
            cost = cost.checked_add(leg_cost).ok_or_else(|| {
                Error::numeric("the plan's total exit cost overflowed; the ladder's values are implausibly large")
            })?;
            remaining = remaining.checked_sub(take).ok_or_else(|| {
                Error::numeric("the plan's remaining amount underflowed while drawing down")
            })?;
            deepest = deepest.max(entry.rung);
            legs.push(PlanLeg {
                object_id: entry.object_id.clone(),
                rung: entry.rung,
                amount: take,
                cost: leg_cost,
            });
        }

        if remaining.is_positive() {
            return Err(Error::numeric(format!(
                "the ladder reported enough value but the plan came up {remaining} short; the \
                 ladder's totals and its entries disagree"
            )));
        }

        Ok(LiquidationPlan {
            legs,
            raised: amount,
            cost,
            deepest_rung: deepest,
        })
    }
}
