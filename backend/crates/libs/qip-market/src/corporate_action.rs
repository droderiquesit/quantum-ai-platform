//! Corporate actions and price adjustment.
//!
//! An unadjusted price history containing a split looks like a crash. Every
//! historical series the platform reasons over is adjusted through these
//! factors, and the adjustment is recorded rather than baked in, so a raw price
//! can always be recovered for reconciliation against a broker statement.

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
    pub fn price_adjustment_factor(&self, reference_price: Decimal) -> Decimal {
        match &self.kind {
            CorporateActionKind::Split { ratio } => {
                if ratio.is_positive() {
                    Decimal::ONE.checked_div(*ratio).unwrap_or(Decimal::ONE)
                } else {
                    Decimal::ONE
                }
            }
            CorporateActionKind::StockDividend { ratio } => {
                let denominator = Decimal::ONE + *ratio;
                if denominator.is_positive() {
                    Decimal::ONE
                        .checked_div(denominator)
                        .unwrap_or(Decimal::ONE)
                } else {
                    Decimal::ONE
                }
            }
            CorporateActionKind::CashDividend { amount } => {
                if !reference_price.is_positive() {
                    return Decimal::ONE;
                }
                (reference_price - *amount)
                    .checked_div(reference_price)
                    .unwrap_or(Decimal::ONE)
                    .max(Decimal::ZERO)
            }
            CorporateActionKind::Spinoff { value_fraction, .. } => {
                Decimal::from_f64(1.0 - value_fraction.clamp(0.0, 1.0)).unwrap_or(Decimal::ONE)
            }
            CorporateActionKind::RightsIssue { ratio, price } => {
                // Theoretical ex-rights price over cum price.
                if !reference_price.is_positive() {
                    return Decimal::ONE;
                }
                let total_shares = Decimal::ONE + *ratio;
                let total_value = reference_price + *price * *ratio;
                total_value
                    .checked_div(total_shares)
                    .and_then(|terp| terp.checked_div(reference_price))
                    .unwrap_or(Decimal::ONE)
            }
            // Mergers, delistings and renames do not adjust the prior series:
            // the instrument's history stands as traded.
            CorporateActionKind::Merger { .. }
            | CorporateActionKind::Delisting { .. }
            | CorporateActionKind::Renamed { .. } => Decimal::ONE,
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
pub fn adjust_prices(
    prices: &[(Timestamp, Decimal)],
    actions: &[CorporateAction],
) -> Vec<(Timestamp, Decimal)> {
    let mut adjusted: Vec<(Timestamp, Decimal)> = prices.to_vec();
    let mut sorted: Vec<&CorporateAction> = actions.iter().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.ex_date.as_nanos()));

    for action in sorted {
        // The reference price is the last close before the ex-date.
        let reference = adjusted
            .iter()
            .rfind(|(t, _)| *t < action.ex_date)
            .map(|(_, p)| *p)
            .unwrap_or(Decimal::ONE);
        let factor = action.price_adjustment_factor(reference);
        if factor == Decimal::ONE {
            continue;
        }
        for (timestamp, price) in adjusted.iter_mut() {
            if *timestamp < action.ex_date {
                *price = *price * factor;
            }
        }
    }
    adjusted
}
