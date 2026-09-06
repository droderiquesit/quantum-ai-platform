//! Transaction costs and liquidity.
//!
//! Every instrument carries its own cost model. Execution and simulation share
//! it, which is the point: a backtest that assumes a tighter spread than the
//! execution engine will actually pay produces a strategy that only works on
//! paper.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// How readily a position can be turned into cash.
///
/// This is also the wire shape of a catalogue record's `liquidity` block, read
/// by [`crate::catalogue`] with no second definition beside it — two
/// declarations of one fact drift, and the one nobody is looking at is the one
/// that drifts. Every field is therefore required of a catalogue that states
/// the block — serde refuses a missing one — and `deny_unknown_fields` names
/// the key that should not be there: a catalogue writing `days_to_liquidation`
/// would otherwise be refused for a *missing* `days_to_liquidate`, pointing
/// the operator at a key they are looking straight at.
///
/// # There is deliberately no `Default`
///
/// There was one, and it asserted `typical_spread_bps: 10.0` and
/// `days_to_liquidate: 1.0` — close to the tightest quote and the fastest exit
/// a listed name plausibly has, asserted by a library about instruments it had
/// never seen. `MinLiquidity` and `MaxDaysToLiquidate`, controls whose whole
/// job is to veto trading, read exactly those two fields, so every instrument
/// that never stated its liquidity was vetoed — or declined to be vetoed — on
/// a figure that existed in no file, in no diff a reviewer read, and under no
/// manifest hash. It was the shape [`Self::illiquid`] was cured of, one level
/// out and one level milder: that constant sat on a low rung and became a
/// ceiling on every listed quote above it, whereas this one was uniform and so
/// inverted nothing. Uniform is not harmless. It answered a question nobody
/// asked it, in the direction that permits trading.
///
/// **A more conservative default would have been the same defect with a
/// different number, and a refusable sentinel would have been the same defect
/// with a longer fuse.** Making the two quoted figures `f64::NAN` was measured
/// — 260 failing tests — and it would have worked only for the two seams that
/// look: [`crate::ladder::Rung::classify`] and `ladder_reference_of` in
/// `qip-kernel`. Three other readers do not look. This type reaches
/// `qip_capital::TradeCapacity`, which divides by `average_daily_volume`;
/// `qip_risk`'s aggregate, which carries `days_to_liquidate` into the horizon
/// a limit is compared against; and `qip_twin`'s counterfactual, which sizes
/// participation off the volume. A `NaN` reaching those produces a `NaN`
/// capacity, and every `<= 0.0` guard on the way answers `false` to a `NaN`.
/// The sentinel would have travelled further than the number it replaced.
///
/// So the absence is not representable. A value of this type can be obtained
/// four ways and each is somebody stating figures: [`Self::listed`],
/// [`Self::illiquid`], a struct literal naming all six fields, and `serde`,
/// which refuses a missing one. [`crate::object::FinancialObject::builder`]
/// takes one **by position** for the same reason — an object cannot be started
/// without saying what it costs to leave it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiquidityProfile {
    /// Typical traded volume per day, in instrument units.
    pub average_daily_volume: Decimal,
    /// Quoted bid-ask spread in basis points of mid.
    pub typical_spread_bps: f64,
    /// Depth available at the touch, in units.
    pub top_of_book_depth: Decimal,
    /// Days to exit a position of average size without undue impact.
    pub days_to_liquidate: f64,
    /// Fraction of daily volume the platform is willing to be.
    pub max_participation_rate: f64,
    /// True where the instrument only trades by appointment or negotiation.
    pub is_negotiated: bool,
}

impl LiquidityProfile {
    /// The fraction of a day's volume the platform is willing to be.
    ///
    /// A policy rather than a measurement — it is what the desk permits
    /// itself, not what the instrument does — which is why it is a named
    /// constant here and a stated field on every record that departs from it.
    /// [`crate::catalogue`] records state their own; the two constructors
    /// below apply this one and say so.
    pub const HOUSE_PARTICIPATION_RATE: f64 = 0.1;

    /// Liquid, tight-spread listed instrument.
    ///
    /// **Choosing this constructor is itself the claim**, and it is a claim
    /// about two things the caller must actually believe: that the instrument
    /// exits within one session, and that the desk may be
    /// [`Self::HOUSE_PARTICIPATION_RATE`] of its volume. `Rung::classify`
    /// reads the first — `days > 1.0` drops a holding to
    /// [`crate::ladder::Rung::BondsAndLessLiquidListed`] — so a caller whose
    /// instrument takes longer than a session to leave is not describing a
    /// listed name and must state the six fields itself. This is not the
    /// defect the removed `Default` was: a default is inherited by a caller
    /// who said nothing, and this is chosen by a caller who named it.
    pub fn listed(average_daily_volume: Decimal, spread_bps: f64) -> Self {
        Self {
            average_daily_volume,
            typical_spread_bps: spread_bps,
            top_of_book_depth: average_daily_volume
                .checked_div(Decimal::from_int(2000))
                .unwrap_or(Decimal::ZERO),
            days_to_liquidate: 1.0,
            max_participation_rate: Self::HOUSE_PARTICIPATION_RATE,
            is_negotiated: false,
        }
    }

    /// Instrument that trades only by negotiation: private credit, real assets.
    ///
    /// **The spread is the caller's, exactly as [`Self::listed`] takes it.**
    /// This constructor used to hardcode `typical_spread_bps: 250.0` and take
    /// only the exit time, so every negotiated holding in the platform
    /// asserted a two-and-a-half-percent quote that nobody had measured —
    /// a number with a cost model attached, which is the
    /// `MaxExpectedShortfall` shape: a figure that reads as evidence and is
    /// not.
    ///
    /// It was not inert. The liquidity ladder proves that cost rises as it
    /// descends, and `Rung::classify` puts a negotiated holding on
    /// [`crate::ladder::Rung::PrivateCreditAndRealAssets`], below every listed
    /// name. So the invented 250 became the ceiling on what any listed
    /// instrument above it could be quoted at: one ordinary small-cap at
    /// 300bps inverted the per-rung rate, `LiquidityLadder::new` refused the
    /// whole ladder, `RiskState::liquidatable_within` came back empty, and —
    /// once that refusal was made to fail closed — the desk stopped trading
    /// entirely. Measured on one book, one field differing:
    ///
    /// ```text
    /// FAST listed at 5bps   beside illiquid(30.0) => accepted 10/10
    /// FAST listed at 300bps beside illiquid(30.0) => accepted  0/10
    /// ```
    ///
    /// Widening the constant would have moved that ceiling without removing
    /// it, which is clamping an invalid input one order of magnitude further
    /// out. There is no honest default here: what a private-credit position
    /// costs to leave is a fact about that position, and the caller holding
    /// the record is the only one who has it.
    ///
    /// `spread_bps` is not validated here, for the reason [`Self::listed`]
    /// does not validate its own: a profile is a value object, and the
    /// refusals belong where the figure is read — `Rung::classify` for the
    /// exit time, and `qip-kernel`'s `ladder_reference_of` for a spread that
    /// is not a spread or is at or beyond the whole value of the holding.
    pub fn illiquid(days_to_liquidate: f64, spread_bps: f64) -> Self {
        Self {
            average_daily_volume: Decimal::ZERO,
            typical_spread_bps: spread_bps,
            top_of_book_depth: Decimal::ZERO,
            days_to_liquidate,
            max_participation_rate: 0.0,
            is_negotiated: true,
        }
    }

    /// Days to exit `quantity` at the permitted participation rate.
    ///
    /// Returns `None` for a negotiated instrument, where no volume-based
    /// estimate is meaningful and `days_to_liquidate` is the only guide.
    pub fn days_to_exit(&self, quantity: Decimal) -> Option<f64> {
        if self.is_negotiated || self.max_participation_rate <= 0.0 {
            return None;
        }
        let daily_capacity = self.average_daily_volume.to_f64() * self.max_participation_rate;
        if daily_capacity <= 0.0 {
            return None;
        }
        Some((quantity.abs().to_f64() / daily_capacity).max(0.0))
    }
}

/// Cost model for trading one instrument.
///
/// Total cost is the sum of an explicit fee, half the spread, and a market
/// impact term following the square-root law — the empirical regularity that
/// impact scales with the square root of participation rather than linearly.
///
/// # Why deserialisation is routed through [`Self::checked`]
///
/// Every component of [`Self::estimate`] crosses through
/// `Decimal::apply_bps`, which is `Decimal::from_f64(bps / 10_000.0)` followed
/// by a `checked_mul`. Neither half has an answer for `half_spread_bps: NaN`
/// or `1e35`, and the answer it gave was `Decimal::ZERO` — not an error, not a
/// refusal, a number: the instrument priced at **zero cost to trade**, which is
/// the one direction that makes a trade look profitable that it is not. The
/// plain `#[derive(Deserialize)]` was the way in, exactly as it was for
/// [`crate::valuation::ValuationInput`], and
/// [`crate::object::FinancialObject::validate`] — the gate `Universe::insert`
/// runs — did not look at this field at all. Both holes are closed: the wire
/// goes through `checked`, and `validate` now reports [`Self::problems`].
///
/// # What this does not close, and no check inside this type could
///
/// The fields stay public, so a struct literal remains a way in that runs no
/// check. That is not hypothetical: `grep -rn 'TransactionCostModel *{'
/// --include=*.rs` finds `qip-simulation-engine`'s `CostModel::pricing` and
/// `pricing_at` assembling one field by field (four sites on 2026-09-06;
/// **recount before quoting that number**, because a figure carried forward
/// from a tree that no longer exists reads as evidence and is not). Those are
/// *computed* models rather than reference data — they never enter a
/// [`crate::universe::Universe`] — and neither the wire nor `validate` sees
/// them. What `try_from` buys is precisely that a **file** cannot smuggle a
/// figure past the arithmetic, and what `validate` buys is that a **record**
/// cannot. A caller computing a coefficient is neither, and the remedy for it
/// is [`Self::checked`] at the seam where it is computed — which is in that
/// crate and not this one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "TransactionCostModelWire")]
pub struct TransactionCostModel {
    /// Commission and exchange fees, in basis points of notional.
    pub commission_bps: f64,
    /// Fixed cost per order, in the instrument currency.
    pub fixed_fee: Decimal,
    /// Half-spread paid on a marketable order, in basis points.
    pub half_spread_bps: f64,
    /// Coefficient of the square-root impact law, in basis points.
    ///
    /// Roughly the impact of trading 100% of a day's volume; empirical
    /// estimates for liquid equities cluster around 30-60bp.
    pub impact_coefficient_bps: f64,
    /// Taxes and levies, in basis points (stamp duty, financial transaction tax).
    pub tax_bps: f64,
    /// Borrow cost for a short position, in basis points per year.
    pub short_borrow_bps_annual: f64,
}

/// The on-disk shape. Deserialising goes through [`TransactionCostModel::checked`],
/// so a record whose spread is not a spread is refused at load rather than
/// discovered as a zero cost inside a profitability comparison.
///
/// The field set is identical to the struct's and no field carries
/// `serde(default)`, so what a document must state is unchanged; only what it
/// may state is narrower.
#[derive(Deserialize)]
struct TransactionCostModelWire {
    commission_bps: f64,
    fixed_fee: Decimal,
    half_spread_bps: f64,
    impact_coefficient_bps: f64,
    tax_bps: f64,
    short_borrow_bps_annual: f64,
}

impl TryFrom<TransactionCostModelWire> for TransactionCostModel {
    type Error = Error;

    fn try_from(wire: TransactionCostModelWire) -> Result<Self> {
        Self {
            commission_bps: wire.commission_bps,
            fixed_fee: wire.fixed_fee,
            half_spread_bps: wire.half_spread_bps,
            impact_coefficient_bps: wire.impact_coefficient_bps,
            tax_bps: wire.tax_bps,
            short_borrow_bps_annual: wire.short_borrow_bps_annual,
        }
        .checked()
    }
}

/// The largest per-trade cost component this platform will price.
///
/// A single component at 10,000bps says one trade costs the entire notional.
/// The same bound, for the same reason and in the same words, is what
/// `qip-kernel`'s `ladder_reference_of` applies to
/// [`LiquidityProfile::typical_spread_bps`] — "at or beyond the whole value of
/// the holding". It is not a clamp and not a view on what venues charge: a
/// record stating more has a data error in it, and the alternative to refusing
/// it is `apply_bps` answering zero.
pub const MAX_TRADE_COST_BPS: f64 = 10_000.0;

/// The largest borrow rate this platform will hold on a record.
///
/// Wider than [`MAX_TRADE_COST_BPS`] and deliberately so: a borrow rate is
/// charged per year rather than per trade, and a hard-to-borrow name really
/// does quote past 100% per annum, so the per-trade ceiling would refuse a
/// figure a securities-lending desk writes down in an ordinary week. A hundred
/// times the position per year is not one.
pub const MAX_BORROW_BPS_ANNUAL: f64 = 1_000_000.0;

impl Default for TransactionCostModel {
    fn default() -> Self {
        Self {
            commission_bps: 1.0,
            fixed_fee: Decimal::ZERO,
            half_spread_bps: 2.5,
            impact_coefficient_bps: 40.0,
            tax_bps: 0.0,
            short_borrow_bps_annual: 50.0,
        }
    }
}

impl TransactionCostModel {
    /// Structural problems with the model. Empty means it can price a trade.
    ///
    /// Reported by [`crate::object::FinancialObject::validate`], so the gate
    /// `Universe::insert` already runs is the gate this field passes through
    /// too. It listed nine other checks and never looked here, which is how a
    /// zero-cost instrument reached the world model.
    ///
    /// Each message names the figure and what to supply instead, because the
    /// remedy is always the same and is never in this crate: correct the
    /// reference record.
    pub fn problems(&self) -> Vec<String> {
        let mut issues = Vec::new();
        for (name, value) in [
            ("commission_bps", self.commission_bps),
            ("half_spread_bps", self.half_spread_bps),
            ("impact_coefficient_bps", self.impact_coefficient_bps),
            ("tax_bps", self.tax_bps),
        ] {
            if !value.is_finite() || value < 0.0 {
                issues.push(format!(
                    "{name} is {value}, which is not a cost in basis points; supply a finite \
                     non-negative rate — a cost that is not a number is priced at zero by \
                     `Decimal::apply_bps`, and an instrument that is free to trade is profitable \
                     to trade"
                ));
            } else if value >= MAX_TRADE_COST_BPS {
                issues.push(format!(
                    "{name} is {value}bps, at or beyond the whole value of the notional; supply a \
                     rate under {MAX_TRADE_COST_BPS} — a figure this large is a data error, and \
                     one large enough to overflow is priced at zero rather than refused"
                ));
            }
        }
        if !self.short_borrow_bps_annual.is_finite() || self.short_borrow_bps_annual < 0.0 {
            issues.push(format!(
                "short_borrow_bps_annual is {}, which is not a borrow rate; supply a finite \
                 non-negative rate in basis points per year",
                self.short_borrow_bps_annual
            ));
        } else if self.short_borrow_bps_annual >= MAX_BORROW_BPS_ANNUAL {
            issues.push(format!(
                "short_borrow_bps_annual is {}bps per year, a hundred times the position; supply \
                 a rate under {MAX_BORROW_BPS_ANNUAL}",
                self.short_borrow_bps_annual
            ));
        }
        if self.fixed_fee.is_negative() {
            issues.push(format!(
                "fixed_fee is {}; supply a non-negative amount — a negative fee is a claim that \
                 the venue pays the desk to send an order, and it subtracts from every estimate \
                 the sizing engine compares against expected alpha",
                self.fixed_fee
            ));
        }
        issues
    }

    /// Return the model if it can price a trade, and a refusal naming the
    /// field and its value otherwise.
    ///
    /// Consumed rather than borrowed so a caller cannot hold on to the
    /// unchecked value it handed in.
    pub fn checked(self) -> Result<Self> {
        let issues = self.problems();
        if !issues.is_empty() {
            return Err(Error::invalid(format!(
                "invalid transaction cost model: {}",
                issues.join("; ")
            )));
        }
        Ok(self)
    }

    /// A cost model for a liquid listed instrument.
    pub fn listed(spread_bps: f64) -> Self {
        Self {
            half_spread_bps: spread_bps / 2.0,
            ..Self::default()
        }
    }

    /// A cost model for an instrument that trades by negotiation.
    pub fn negotiated() -> Self {
        Self {
            commission_bps: 25.0,
            fixed_fee: Decimal::ZERO,
            half_spread_bps: 125.0,
            impact_coefficient_bps: 0.0,
            tax_bps: 0.0,
            short_borrow_bps_annual: 0.0,
        }
    }

    /// Estimated all-in cost of trading `notional` at `participation` of daily
    /// volume, in the instrument's currency.
    pub fn estimate(&self, notional: Decimal, participation: f64) -> Decimal {
        let magnitude = notional.abs();
        let explicit = magnitude.apply_bps(self.commission_bps + self.tax_bps);
        let spread = magnitude.apply_bps(self.half_spread_bps);
        let impact = magnitude.apply_bps(self.impact_bps(participation));
        explicit + spread + impact + self.fixed_fee
    }

    /// Market impact in basis points at a given participation rate.
    pub fn impact_bps(&self, participation: f64) -> f64 {
        if !participation.is_finite() || participation <= 0.0 {
            return 0.0;
        }
        // Square-root law, capped so a pathological participation figure cannot
        // produce a nonsensical cost that silently blocks all trading.
        self.impact_coefficient_bps * participation.min(4.0).sqrt()
    }

    /// Total cost in basis points, for comparing instruments.
    pub fn total_bps(&self, participation: f64) -> f64 {
        self.commission_bps + self.tax_bps + self.half_spread_bps + self.impact_bps(participation)
    }

    /// The participation rate at which expected impact eats `alpha_bps` of
    /// expected alpha — the point past which trading faster destroys the trade.
    ///
    /// Inverts the same square-root law `impact_bps` prices, so it has to
    /// respect the same cap: `impact_bps` never lets modelled impact climb
    /// past `impact_coefficient_bps * 2.0` (participation capped at `4.0`,
    /// four full days of volume in one day), because a pathological
    /// participation figure must not produce a nonsensical cost. A budget
    /// at or beyond that ceiling is never actually reached by trading
    /// faster — impact stops climbing before it gets there — so the
    /// uncapped inverse-square would name a participation rate several
    /// multiples of a day's total volume as "the" breakeven, and a caller
    /// sizing capacity off it (`qip-capital`'s `TradeCapacity::capacity`)
    /// would be handed a bound the cost model itself never priced. Once
    /// this was found reporting 582% of average daily volume as a
    /// breakeven whose actual modelled impact was capped at a fifth of the
    /// alpha it claimed to exhaust.
    pub fn breakeven_participation(&self, alpha_bps: f64) -> f64 {
        let budget = alpha_bps - self.commission_bps - self.tax_bps - self.half_spread_bps;
        if budget <= 0.0 || self.impact_coefficient_bps <= 0.0 {
            return 0.0;
        }
        if budget >= self.impact_coefficient_bps * 2.0 {
            return 4.0;
        }
        (budget / self.impact_coefficient_bps).powi(2)
    }
}
