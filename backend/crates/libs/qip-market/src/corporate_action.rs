//! Corporate actions and price adjustment.
//!
//! An unadjusted price history containing a split looks like a crash. Every
//! historical series the platform reasons over is adjusted through these
//! factors, and the adjustment is recorded rather than baked in, so a raw price
//! can always be recovered for reconciliation against a broker statement.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CorporateActionKind {
    /// `ratio` new shares for each old share; 2.0 is a two-for-one split.
    Split { ratio: Decimal },
    /// Cash paid per share.
    CashDividend { amount: Decimal },
    /// Additional shares paid per share held.
    StockDividend { ratio: Decimal },
    /// Right to buy `ratio` new shares at `price` per share held.
    RightsIssue { ratio: Decimal, price: Decimal },
    /// Acquired; holders receive `cash_per_share` and/or shares in `acquirer`.
    Merger {
        acquirer: String,
        cash_per_share: Decimal,
        share_ratio: Decimal,
    },
    /// A business is separated out; `value_fraction` of value leaves.
    Spinoff {
        spun_entity: String,
        value_fraction: f64,
    },
    /// The instrument ceases to trade.
    Delisting { reason: String },
    /// Ticker or name change; economics are unaffected.
    Renamed { new_symbol: String },
}

/// A corporate action affecting one instrument.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorporateAction {
    pub object_id: ObjectId,
    /// First date the instrument trades without the entitlement.
    pub ex_date: Timestamp,
    /// Date holders of record are determined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_date: Option<Timestamp>,
    /// Date the entitlement is paid or delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_date: Option<Timestamp>,
    pub kind: CorporateActionKind,
    pub announced_at: Timestamp,
}

impl CorporateAction {
    /// Multiplicative factor applied to prices before the ex-date.
    ///
    /// A two-for-one split gives 0.5: prior prices are halved so the series is
    /// continuous. A dividend's factor depends on the price it was paid from,
    /// which is why the reference price is a parameter.
    ///
    /// # Why this returns a `Result`
    ///
    /// It returned `Decimal`, so it had exactly one way to describe an action
    /// it could not price: `Decimal::ONE`, which is indistinguishable from the
    /// honest answer given by a merger or a rename — *no adjustment*. Every
    /// arm fell through to it. A split whose ratio was zero, a stock dividend
    /// at a ratio of -1, a dividend larger than the price it was paid from, a
    /// rights issue against a zero reference: each of those is a corrupt
    /// record, and each was answered with "this action changes nothing". The
    /// result is a price series that still contains the split, which reads as
    /// a crash, and every figure derived from it — volatility, drawdown,
    /// return — inherits it silently. There is no fallback here now; the two
    /// values are a factor somebody can defend and a refusal naming the field.
    ///
    /// The spinoff arm additionally **clamped** `value_fraction` into `[0, 1]`
    /// before converting it, which is the clamp the core-Rust rules prohibit by
    /// name: a fraction of 4.0 became 1.0 and zeroed the whole prior series,
    /// and `NaN` survived the clamp, failed `from_f64` and landed on the same
    /// `Decimal::ONE`. It is a validation now, not a correction.
    pub fn price_adjustment_factor(&self, reference_price: Decimal) -> Result<Decimal> {
        match &self.kind {
            CorporateActionKind::Split { ratio } => {
                if !ratio.is_positive() {
                    return Err(Error::invalid(format!(
                        "{} carries a split ratio of {ratio}; supply a strictly positive number of \
                         new shares per old share — a split that cannot be priced leaves the step \
                         in the series, and an unadjusted split reads as a crash",
                        self.object_id
                    )));
                }
                Decimal::ONE.checked_div(*ratio).ok_or_else(|| {
                    Error::numeric(format!(
                        "{} carries a split ratio of {ratio}, whose reciprocal is not \
                         representable; supply a ratio this platform can invert",
                        self.object_id
                    ))
                })
            }
            CorporateActionKind::StockDividend { ratio } => {
                let denominator = Decimal::ONE + *ratio;
                if !denominator.is_positive() {
                    return Err(Error::invalid(format!(
                        "{} carries a stock dividend ratio of {ratio}, which pays away at least \
                         the whole holding; supply a ratio greater than -1 — additional shares \
                         per share held",
                        self.object_id
                    )));
                }
                Decimal::ONE.checked_div(denominator).ok_or_else(|| {
                    Error::numeric(format!(
                        "{} carries a stock dividend ratio of {ratio} whose adjustment factor is \
                         not representable; supply a ratio this platform can invert",
                        self.object_id
                    ))
                })
            }
            CorporateActionKind::CashDividend { amount } => {
                if !reference_price.is_positive() {
                    return Err(Error::invalid(format!(
                        "{} is a cash dividend priced against a reference of {reference_price}; \
                         supply the last close before the ex-date — a dividend yield cannot be \
                         taken from a price of zero, and reporting no adjustment leaves the drop \
                         in the series",
                        self.object_id
                    )));
                }
                if amount.is_negative() {
                    return Err(Error::invalid(format!(
                        "{} carries a cash dividend of {amount}; supply a non-negative amount per \
                         share — a negative dividend is a capital call, which is a different \
                         action",
                        self.object_id
                    )));
                }
                // Replaces a `.max(Decimal::ZERO)` that turned a dividend at or
                // beyond the share price into a factor of zero, which does not
                // adjust the prior series so much as erase it.
                if *amount >= reference_price {
                    return Err(Error::invalid(format!(
                        "{} pays a cash dividend of {amount} against a reference price of \
                         {reference_price}; supply a dividend below the price it was paid from — \
                         at or beyond it the adjusted history is all zeroes, not a continuous \
                         series",
                        self.object_id
                    )));
                }
                (reference_price - *amount)
                    .checked_div(reference_price)
                    .ok_or_else(|| {
                        Error::numeric(format!(
                            "{} pays {amount} against {reference_price} and the ratio between \
                             them is not representable; correct the dividend record",
                            self.object_id
                        ))
                    })
            }
            CorporateActionKind::Spinoff {
                spun_entity,
                value_fraction,
            } => {
                if !value_fraction.is_finite() || !(0.0..1.0).contains(value_fraction) {
                    return Err(Error::invalid(format!(
                        "{} spins off {spun_entity} carrying {value_fraction} of the value; \
                         supply a finite fraction in [0, 1) — a fraction outside it was clamped, \
                         and a clamp to 1.0 zeroes the whole prior series while `NaN` reported no \
                         adjustment at all",
                        self.object_id
                    )));
                }
                // Statistic to money: the fraction is an f64 on the record and
                // the factor multiplies prices. The guard above holds the
                // argument in (0, 1], which `Decimal::from_f64` represents, so
                // the refusal below is unreachable rather than absent — stated
                // as a refusal because an `unwrap_or` here is what put
                // `Decimal::ONE` in front of a `NaN`.
                Decimal::from_f64(1.0 - value_fraction).ok_or_else(|| {
                    Error::numeric(format!(
                        "{} spins off {spun_entity} carrying {value_fraction} of the value, which \
                         is not a representable adjustment factor",
                        self.object_id
                    ))
                })
            }
            CorporateActionKind::RightsIssue { ratio, price } => {
                // Theoretical ex-rights price over cum price.
                if !reference_price.is_positive() {
                    return Err(Error::invalid(format!(
                        "{} is a rights issue priced against a reference of {reference_price}; \
                         supply the last close before the ex-date — the theoretical ex-rights \
                         price is a ratio to it",
                        self.object_id
                    )));
                }
                let total_shares = Decimal::ONE + *ratio;
                if !total_shares.is_positive() {
                    return Err(Error::invalid(format!(
                        "{} carries a rights ratio of {ratio}, which leaves no shares outstanding \
                         after the issue; supply a ratio greater than -1 — new shares offered per \
                         share held",
                        self.object_id
                    )));
                }
                let subscription = price.checked_mul(*ratio).ok_or_else(|| {
                    Error::numeric(format!(
                        "{} offers {ratio} shares at {price}, whose subscription value is not \
                         representable; correct the rights record",
                        self.object_id
                    ))
                })?;
                let total_value = reference_price.checked_add(subscription).ok_or_else(|| {
                    Error::numeric(format!(
                        "{} offers {ratio} shares at {price} against {reference_price}, and the \
                         cum value of the two is not representable; correct the rights record",
                        self.object_id
                    ))
                })?;
                total_value
                    .checked_div(total_shares)
                    .and_then(|terp| terp.checked_div(reference_price))
                    .ok_or_else(|| {
                        Error::numeric(format!(
                            "{} has no representable theoretical ex-rights price at {ratio} \
                             shares of {price} against {reference_price}; correct the rights \
                             record",
                            self.object_id
                        ))
                    })
            }
            // Mergers, delistings and renames do not adjust the prior series:
            // the instrument's history stands as traded. This is the only
            // `Decimal::ONE` left in the function, and it is now the only thing
            // it can mean.
            CorporateActionKind::Merger { .. }
            | CorporateActionKind::Delisting { .. }
            | CorporateActionKind::Renamed { .. } => Ok(Decimal::ONE),
        }
    }

    /// Factor applied to share counts before the ex-date.
    pub fn quantity_adjustment_factor(&self) -> Decimal {
        match &self.kind {
            CorporateActionKind::Split { ratio } => *ratio,
            CorporateActionKind::StockDividend { ratio } => Decimal::ONE + *ratio,
            _ => Decimal::ONE,
        }
    }

    /// Whether the action terminates the instrument.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.kind,
            CorporateActionKind::Delisting { .. } | CorporateActionKind::Merger { .. }
        )
    }

    /// Cash entitlement per share held.
    pub fn cash_per_share(&self) -> Decimal {
        match &self.kind {
            CorporateActionKind::CashDividend { amount } => *amount,
            CorporateActionKind::Merger { cash_per_share, .. } => *cash_per_share,
            _ => Decimal::ZERO,
        }
    }
}

impl EventBody for CorporateAction {
    const TOPIC: Topic = Topic::MarketCorporateAction;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}",
            self.object_id,
            self.ex_date.as_nanos(),
            serde_json::to_string(&self.kind).unwrap_or_default()
        ))
    }
}

/// Apply a set of actions to a price series, producing an adjusted series.
///
/// Walks backwards from the present so each adjustment compounds onto
/// everything earlier, which is the only ordering that keeps a series with
/// several actions continuous.
///
/// Fallible because [`CorporateAction::price_adjustment_factor`] is: an action
/// this platform cannot price stops the adjustment rather than being applied as
/// no adjustment, since the caller cannot tell those apart in a returned
/// series and would go on to compute a volatility from a step that is still
/// there.
pub fn adjust_prices(
    prices: &[(Timestamp, Decimal)],
    actions: &[CorporateAction],
) -> Result<Vec<(Timestamp, Decimal)>> {
    let mut adjusted: Vec<(Timestamp, Decimal)> = prices.to_vec();
    let mut sorted: Vec<&CorporateAction> = actions.iter().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.ex_date.as_nanos()));

    for action in sorted {
        // The reference price is the last close before the ex-date. Where the
        // series has none, the action predates everything held and adjusts
        // nothing, so it is skipped rather than priced against a fabricated
        // reference. That `unwrap_or(Decimal::ONE)` was a made-up price of 1.00
        // entering the dividend and rights arithmetic; it changed no output
        // because `rfind` returns `None` exactly when the loop below has
        // nothing to touch, but it is now a refusable input to arms that refuse.
        let Some(reference) = adjusted
            .iter()
            .rfind(|(t, _)| *t < action.ex_date)
            .map(|(_, p)| *p)
        else {
            continue;
        };
        let factor = action.price_adjustment_factor(reference)?;
        if factor == Decimal::ONE {
            continue;
        }
        for (timestamp, price) in adjusted.iter_mut() {
            if *timestamp < action.ex_date {
                let current = *price;
                *price = current.checked_mul(factor).ok_or_else(|| {
                    Error::numeric(format!(
                        "{} adjusts a price of {current} by {factor} and the product is not \
                         representable; correct the price series or the action",
                        action.object_id
                    ))
                })?;
            }
        }
    }
    Ok(adjusted)
}
