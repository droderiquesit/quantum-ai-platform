//! Performance attribution.
//!
//! Decomposes a period's profit and loss into where it came from. The
//! decomposition is *exact*: [`Attribution::residual`] is the difference
//! between the total P&L and the sum of the components, and it is asserted to
//! be zero rather than quietly absorbed into an "other" bucket.
//!
//! That constraint is the whole value of the module. An attribution that
//! nearly adds up tells you nothing, because the missing piece is exactly
//! where the thing you did not understand is hiding. Every component here is
//! measured against an explicit counterfactual, so the arithmetic closes.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a piece of P&L came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The market moved and the position was there for it.
    MarketMove,
    /// The position's own move relative to its factor exposures.
    Idiosyncratic,
    /// Factor exposures the position carried.
    Factor,
    /// Dividends, coupons and other income.
    Carry,
    /// Financing on leverage and shorts.
    Financing,
    /// Commissions and fees.
    Commission,
    /// Half-spread paid crossing.
    Spread,
    /// Price moved by the order itself.
    MarketImpact,
    /// The cost of not trading at the decision price: the gap between when the
    /// decision was made and when the order arrived.
    ImplementationShortfall,
    /// Currency translation.
    Currency,
}

impl Source {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::MarketMove => "market_move",
            Self::Idiosyncratic => "idiosyncratic",
            Self::Factor => "factor",
            Self::Carry => "carry",
            Self::Financing => "financing",
            Self::Commission => "commission",
            Self::Spread => "spread",
            Self::MarketImpact => "market_impact",
            Self::ImplementationShortfall => "implementation_shortfall",
            Self::Currency => "currency",
        }
    }

    /// Whether this source is a cost the platform could in principle reduce.
    ///
    /// Separated because the interesting question after a bad quarter is
    /// whether the thesis was wrong or the implementation was.
    pub const fn is_implementation_cost(&self) -> bool {
        matches!(
            self,
            Self::Commission | Self::Spread | Self::MarketImpact | Self::ImplementationShortfall
        )
    }
}

/// One position's contribution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionAttribution {
    pub object_id: String,
    /// The hypotheses this position expressed.
    pub hypotheses: Vec<String>,
    /// Contribution by source, summing to `total`.
    pub components: BTreeMap<String, Decimal>,
    pub total: Decimal,
    /// Average notional over the period, for computing a return.
    pub average_notional: Decimal,
}

impl PositionAttribution {
    /// Return on the average notional held.
    pub fn return_fraction(&self) -> Option<f64> {
        let notional = self.average_notional.to_f64();
        if notional.abs() < 1e-9 {
            return None;
        }
        Some(self.total.to_f64() / notional.abs())
    }

    pub fn component(&self, source: Source) -> Decimal {
        self.components
            .get(source.as_str())
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    /// Everything the implementation cost.
    pub fn implementation_cost(&self) -> Decimal {
        self.components
            .iter()
            .filter(|(name, _)| {
                [
                    Source::Commission,
                    Source::Spread,
                    Source::MarketImpact,
                    Source::ImplementationShortfall,
                ]
                .iter()
                .any(|source| source.as_str() == name.as_str())
            })
            .map(|(_, value)| *value)
            .fold(Decimal::ZERO, |a, b| a + b)
    }
}

/// A period's attribution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attribution {
    pub from: Timestamp,
    pub to: Timestamp,
    /// Total P&L over the period, from the books.
    pub total_pnl: Decimal,
    /// Equity at the start, for computing a return.
    pub opening_equity: Decimal,
    pub positions: Vec<PositionAttribution>,
    /// Totals by source across every position.
    pub by_source: BTreeMap<String, Decimal>,
}

impl Attribution {
    /// The difference between the total and the sum of the components.
    ///
    /// Must be zero. An attribution that nearly adds up hides exactly the
    /// thing nobody understood.
    pub fn residual(&self) -> Decimal {
        let attributed = self
            .positions
            .iter()
            .map(|position| position.total)
            .fold(Decimal::ZERO, |a, b| a + b);
        self.total_pnl - attributed
    }

    /// Whether the decomposition closes.
    pub fn reconciles(&self) -> bool {
        // A single unit at the ninth decimal place, which is the smallest
        // difference the fixed-point representation can express.
        self.residual().abs() <= Decimal::from_raw(1)
    }

    pub fn period_return(&self) -> Option<f64> {
        let opening = self.opening_equity.to_f64();
        if opening.abs() < 1e-9 {
            return None;
        }
        Some(self.total_pnl.to_f64() / opening)
    }

    pub fn source_total(&self, source: Source) -> Decimal {
        self.by_source
            .get(source.as_str())
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    /// Everything the implementation cost, across the book.
    pub fn implementation_cost(&self) -> Decimal {
        [
            Source::Commission,
            Source::Spread,
            Source::MarketImpact,
            Source::ImplementationShortfall,
        ]
        .iter()
        .map(|source| self.source_total(*source))
        .fold(Decimal::ZERO, |a, b| a + b)
    }

    /// Positions ranked by contribution, worst first.
    pub fn worst(&self, limit: usize) -> Vec<&PositionAttribution> {
        let mut ranked: Vec<&PositionAttribution> = self.positions.iter().collect();
        ranked.sort_by(|a, b| a.total.cmp(&b.total));
        ranked.truncate(limit);
        ranked
    }

    /// Contribution grouped by hypothesis.
    ///
    /// The join that makes learning possible: it says what each thesis
    /// actually earned, which is the only honest input to whether the
    /// reasoning behind it was any good.
    pub fn by_hypothesis(&self) -> BTreeMap<String, Decimal> {
        let mut totals: BTreeMap<String, Decimal> = BTreeMap::new();
        for position in &self.positions {
            if position.hypotheses.is_empty() {
                continue;
            }
            // A position expressing several theses splits its P&L evenly
            // between them. Any other split would need a weighting nobody
            // recorded at the time, and inventing one here would make the
            // attribution a guess.
            let share = position.total / Decimal::from_int(position.hypotheses.len() as i64);
            for hypothesis in &position.hypotheses {
                *totals.entry(hypothesis.clone()).or_insert(Decimal::ZERO) += share;
            }
        }
        totals
    }

    pub fn validate(&self) -> Result<()> {
        if self.to < self.from {
            return Err(Error::invalid(
                "an attribution period ends before it starts",
            ));
        }
        if !self.reconciles() {
            return Err(Error::numeric(format!(
                "attribution does not reconcile: {} of P&L is unexplained, which is exactly where whatever nobody understood is hiding",
                self.residual()
            )));
        }
        for position in &self.positions {
            let sum = position
                .components
                .values()
                .fold(Decimal::ZERO, |a, b| a + *b);
            if (sum - position.total).abs() > Decimal::from_raw(1) {
                return Err(Error::numeric(format!(
                    "position {} has components summing to {sum} against a total of {}",
                    position.object_id, position.total
                )));
            }
        }
        Ok(())
    }
}

/// Split a total across weights pro rata, exactly.
///
/// The join between a netted fill and the strategies behind it (blueprint
/// §27, §43.4): one order carries several strategies' shares, and each must
/// receive its fraction of the fill so that the fractions sum to the fill.
/// A split by floating-point multiplication does not sum — three thirds of a
/// hundred come to 99.999999999 — and a split that rounds each share loses or
/// invents the difference, which is exactly the residual [`Attribution`]
/// refuses to carry.
///
/// Largest-remainder in the fixed-point representation's own units: every
/// share takes the floor of its exact fraction, and the units the floors
/// left over go one each to the shares with the largest fractional parts,
/// ties broken by position so two runs over the same vector split it the
/// same way. The shares sum to `total` to the last unit, whatever the
/// weights, and the function asserts it before returning.
///
/// Refuses an empty weight vector, a negative weight, and weights that sum
/// to zero: each is a caller that has nothing to split by, and inventing an
/// even split for it would attribute a fill to strategies that contributed
/// nothing.
pub fn split_pro_rata(total: Decimal, weights: &[Decimal]) -> Result<Vec<Decimal>> {
    if weights.is_empty() {
        return Err(Error::invalid(
            "a pro-rata split needs at least one weight; there is nobody to attribute to",
        ));
    }
    if weights.iter().any(|weight| weight.is_negative()) {
        return Err(Error::invalid(
            "a pro-rata weight cannot be negative; pass the magnitude of a signed share",
        ));
    }
    let denominator: u128 = weights
        .iter()
        .try_fold(0u128, |sum, weight| {
            sum.checked_add(weight.raw().unsigned_abs())
        })
        .ok_or_else(|| Error::numeric("the pro-rata weights overflow when summed"))?;
    if denominator == 0 {
        return Err(Error::invalid(
            "every pro-rata weight is zero, so no share can be computed",
        ));
    }

    let magnitude = total.raw().unsigned_abs();
    let mut floors = Vec::with_capacity(weights.len());
    let mut remainders = Vec::with_capacity(weights.len());
    for weight in weights {
        let scaled = magnitude
            .checked_mul(weight.raw().unsigned_abs())
            .ok_or_else(|| {
                Error::numeric("the pro-rata split overflows; the total is too large")
            })?;
        floors.push(scaled / denominator);
        remainders.push(scaled % denominator);
    }
    let allocated: u128 = floors.iter().sum();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|a, b| remainders[*b].cmp(&remainders[*a]).then(a.cmp(b)));
    // At most one unit per share is ever left over, and fewer units than
    // shares, so this loop hands each leftover to the share that was
    // closest to rounding up.
    for index in order.into_iter().take((magnitude - allocated) as usize) {
        floors[index] += 1;
    }

    let shares: Vec<Decimal> = floors
        .into_iter()
        .map(|units| {
            let signed = i128::try_from(units)
                .map_err(|_| Error::numeric("a pro-rata share overflows the decimal range"))?;
            Ok(Decimal::from_raw(if total.is_negative() {
                -signed
            } else {
                signed
            }))
        })
        .collect::<Result<_>>()?;
    let sum: Decimal = shares.iter().copied().sum();
    if sum != total {
        return Err(Error::numeric(format!(
            "the pro-rata shares sum to {sum} against a total of {total}; the split is not exact"
        )));
    }
    Ok(shares)
}

/// The inputs needed to attribute one position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionPeriod {
    pub object_id: String,
    pub hypotheses: Vec<String>,
    /// Quantity held at the start.
    pub opening_quantity: Decimal,
    pub opening_price: Decimal,
    pub closing_quantity: Decimal,
    pub closing_price: Decimal,
    /// The price the decision was made at, for implementation shortfall.
    pub decision_price: Decimal,
    /// Quantity traded during the period, signed.
    pub traded_quantity: Decimal,
    /// Average price achieved on those trades.
    pub traded_price: Decimal,
    pub commission: Decimal,
    pub spread_cost: Decimal,
    pub impact_cost: Decimal,
    pub income: Decimal,
    pub financing: Decimal,
    /// Realised P&L booked on closures during the period.
    pub realised_pnl: Decimal,
    /// The position's beta to each factor, and each factor's return.
    pub factor_returns: BTreeMap<String, f64>,
    pub factor_betas: BTreeMap<String, f64>,
    pub contract_multiplier: Decimal,
}

impl PositionPeriod {
    pub fn average_notional(&self) -> Decimal {
        let opening = self.opening_quantity * self.opening_price * self.contract_multiplier;
        let closing = self.closing_quantity * self.closing_price * self.contract_multiplier;
        (opening + closing) / Decimal::from_int(2)
    }
}

/// Computes attributions.
#[derive(Debug, Default)]
pub struct Attributor;

impl Attributor {
    pub fn new() -> Self {
        Self
    }

    /// Attribute one period.
    ///
    /// The decomposition is built so it closes by construction: every explicit
    /// component is computed from its own inputs, and the market-move term is
    /// whatever is left once they are subtracted from the total. That makes
    /// the market move the residual rather than the reconciliation, which is
    /// the right place for it — a price change is the one thing that is always
    /// exactly measurable from the books.
    pub fn attribute(
        &self,
        positions: &[PositionPeriod],
        total_pnl: Decimal,
        opening_equity: Decimal,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<Attribution> {
        if to < from {
            return Err(Error::invalid(
                "an attribution period ends before it starts",
            ));
        }

        let mut attributed = Vec::with_capacity(positions.len());
        let mut by_source: BTreeMap<String, Decimal> = BTreeMap::new();

        for period in positions {
            let mut components: BTreeMap<String, Decimal> = BTreeMap::new();

            // The position's total P&L: the change in mark-to-market value plus
            // whatever was realised, less the costs of getting there.
            let opening_value =
                period.opening_quantity * period.opening_price * period.contract_multiplier;
            let closing_value =
                period.closing_quantity * period.closing_price * period.contract_multiplier;
            let traded_value =
                period.traded_quantity * period.traded_price * period.contract_multiplier;
            // Value change net of what was paid or received for the trades.
            let gross = closing_value - opening_value - traded_value;

            let costs = period.commission + period.spread_cost + period.impact_cost;
            // Implementation shortfall: the difference between the decision
            // price and what was actually achieved, on the quantity traded.
            let shortfall = if period.decision_price > Decimal::ZERO {
                (period.traded_price - period.decision_price)
                    * period.traded_quantity
                    * period.contract_multiplier
            } else {
                Decimal::ZERO
            };

            let total = gross + period.income - period.financing - costs;

            // Factor contribution, on the average notional held. Betas and
            // returns are statistics, so the multiplication happens in f64;
            // the result crosses back to `Decimal` immediately below, because
            // everything downstream of this line is money.
            let average = period.average_notional();
            let factor_total: f64 = period
                .factor_betas
                .iter()
                .map(|(factor, beta)| {
                    beta * period.factor_returns.get(factor).copied().unwrap_or(0.0)
                })
                .sum();
            // A NaN or out-of-range factor return would otherwise be swallowed
            // to zero here and reappear, unexplained, inside the idiosyncratic
            // residual below — the exact failure mode `Attribution::residual`
            // exists to surface, reintroduced through the one term that still
            // touched a float. Refuse instead of guessing.
            let factor_pnl = Decimal::from_f64(average.to_f64() * factor_total).ok_or_else(|| {
                Error::numeric(format!(
                    "position {} has a factor contribution that does not convert to a decimal (average notional {average}, factor total {factor_total}); the input factor betas or returns are not finite",
                    period.object_id
                ))
            })?;

            components.insert(Source::Commission.as_str().to_string(), -period.commission);
            components.insert(Source::Spread.as_str().to_string(), -period.spread_cost);
            components.insert(
                Source::MarketImpact.as_str().to_string(),
                -period.impact_cost,
            );
            components.insert(
                Source::ImplementationShortfall.as_str().to_string(),
                -shortfall,
            );
            components.insert(Source::Carry.as_str().to_string(), period.income);
            components.insert(Source::Financing.as_str().to_string(), -period.financing);
            components.insert(Source::Factor.as_str().to_string(), factor_pnl);

            // Everything not otherwise explained is the position's own move.
            // Making this the balancing term is what guarantees the
            // decomposition closes: a price change is always exactly
            // measurable, so there is nothing lost by defining it residually.
            let explained = components.values().fold(Decimal::ZERO, |a, b| a + *b);
            let idiosyncratic = total - explained;
            components.insert(Source::Idiosyncratic.as_str().to_string(), idiosyncratic);

            for (source, value) in &components {
                *by_source.entry(source.clone()).or_insert(Decimal::ZERO) += *value;
            }

            attributed.push(PositionAttribution {
                object_id: period.object_id.clone(),
                hypotheses: period.hypotheses.clone(),
                components,
                total,
                average_notional: average,
            });
        }

        let attribution = Attribution {
            from,
            to,
            total_pnl,
            opening_equity,
            positions: attributed,
            by_source,
        };
        attribution.validate()?;
        Ok(attribution)
    }
}

#[cfg(test)]
mod tests {
    // In a test the assertion is the deliverable; the workspace denies this
    // lint for production code, where a panic on this path would be a bug.
    #![allow(clippy::panic_in_result_fn)]

    use super::*;

    fn dec(text: &str) -> Decimal {
        Decimal::parse(text).expect("a decimal literal")
    }

    #[test]
    fn a_split_the_weights_do_not_divide_still_sums_to_the_total_to_the_last_unit() -> Result<()> {
        // Three equal contributors to a fill of one hundred. A share of
        // 33.333333333 three times is 99.999999999, and the missing unit is
        // exactly the residual an attribution must not carry. Premise first:
        // the fraction genuinely does not divide at nine places.
        let total = dec("100");
        let weights = [dec("1"), dec("1"), dec("1")];
        assert_ne!(
            (total / dec("3")) * dec("3"),
            total,
            "the fixture divides exactly, so it proves nothing about the remainder"
        );

        let shares = split_pro_rata(total, &weights)?;
        assert_eq!(shares.len(), 3);
        let sum: Decimal = shares.iter().copied().sum();
        assert_eq!(sum, total, "shares {shares:?} do not sum to the total");
        // One share carries the leftover unit, and it is the first: ties in
        // the fractional part break by position so a replay splits the same
        // way.
        assert_eq!(shares[0], dec("33.333333334"));
        assert_eq!(shares[1], dec("33.333333333"));
        assert_eq!(shares[2], dec("33.333333333"));
        Ok(())
    }

    #[test]
    fn shares_follow_the_weights_and_a_negative_total_splits_to_negative_shares() -> Result<()> {
        let shares = split_pro_rata(dec("-100"), &[dec("60"), dec("40")])?;
        assert_eq!(shares, vec![dec("-60"), dec("-40")]);
        let sum: Decimal = shares.iter().copied().sum();
        assert_eq!(sum, dec("-100"));
        Ok(())
    }

    #[test]
    fn a_split_with_nothing_to_split_by_is_refused_rather_than_made_even() {
        assert!(split_pro_rata(dec("100"), &[]).is_err());
        assert!(split_pro_rata(dec("100"), &[Decimal::ZERO, Decimal::ZERO]).is_err());
        assert!(split_pro_rata(dec("100"), &[dec("1"), dec("-1")]).is_err());
    }
}
