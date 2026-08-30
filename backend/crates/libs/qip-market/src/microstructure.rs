//! Microstructure metrics.
//!
//! These are what the Fast Brain reads to judge whether the market is behaving
//! normally: how wide it is, which way flow is leaning, how much price moves
//! per unit of volume. All are computed from observed quotes and trades — no
//! model calibration, no lookahead.

use qip_core::Decimal;
use serde::{Deserialize, Serialize};

use crate::book::Side;
use crate::quote::{Quote, Trade};

/// Microstructure state derived from a window of quotes and trades.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MicrostructureMetrics {
    /// Time-weighted quoted spread, in basis points.
    pub quoted_spread_bps: f64,
    /// Twice the signed distance from trade price to the prevailing mid, in
    /// basis points. What a taker actually paid, as opposed to what was quoted.
    pub effective_spread_bps: f64,
    /// Effective spread net of the mid's subsequent move — the part the market
    /// maker keeps rather than loses to informed flow.
    pub realised_spread_bps: f64,
    /// Permanent price move attributable to the trade, in basis points.
    pub price_impact_bps: f64,
    /// Signed traded volume as a fraction of total volume, in `[-1, 1]`.
    pub order_flow_imbalance: f64,
    /// Depth imbalance at the touch, in `[-1, 1]`.
    pub quote_imbalance: f64,
    /// Kyle's lambda: price move per unit of signed volume, in basis points.
    pub kyle_lambda: f64,
    /// Fraction of trades initiated by buyers.
    pub buy_initiated_fraction: f64,
    pub trade_count: u64,
    pub total_volume: Decimal,
}

impl MicrostructureMetrics {
    /// Compute metrics from aligned quotes and trades.
    ///
    /// `future_mids[i]` is the mid observed a fixed horizon after trade `i`,
    /// used for the realised-spread and impact decomposition. Passing it in
    /// keeps the lookahead explicit and confined to post-trade analysis, where
    /// it is legitimate.
    pub fn compute(quotes: &[Quote], trades: &[Trade], future_mids: &[f64]) -> Self {
        let mut metrics = Self::default();
        if quotes.is_empty() {
            return metrics;
        }

        let spreads: Vec<f64> = quotes.iter().filter_map(|q| q.spread_bps()).collect();
        metrics.quoted_spread_bps = qip_numerics::stats::mean(&spreads);
        metrics.quote_imbalance =
            qip_numerics::stats::mean(&quotes.iter().map(Quote::imbalance).collect::<Vec<_>>());

        let price_forming: Vec<&Trade> = trades
            .iter()
            .filter(|t| t.condition.is_price_forming())
            .collect();
        metrics.trade_count = price_forming.len() as u64;
        metrics.total_volume = price_forming.iter().map(|t| t.size).sum();
        if price_forming.is_empty() {
            return metrics;
        }

        let mut effective = Vec::new();
        let mut realised = Vec::new();
        let mut impact = Vec::new();
        let mut signed_volume = 0.0;
        let mut absolute_volume = 0.0;
        let mut buy_count = 0u64;
        let mut signed_flows = Vec::new();
        let mut price_moves = Vec::new();

        for (index, trade) in price_forming.iter().enumerate() {
            let Some(quote) = prevailing_quote(quotes, trade) else {
                continue;
            };
            let Some(mid) = quote.mid() else {
                continue;
            };
            if !mid.is_positive() {
                continue;
            }
            let mid_f = mid.to_f64();
            let side = trade.infer_aggressor(quote);
            let direction = match side {
                Some(Side::Buy) => 1.0,
                Some(Side::Sell) => -1.0,
                None => 0.0,
            };
            if direction > 0.0 {
                buy_count += 1;
            }

            let price = trade.price.to_f64();
            let size = trade.size.to_f64();
            signed_volume += direction * size;
            absolute_volume += size;

            // Effective spread: twice the signed distance from the mid.
            effective.push(2.0 * direction * (price - mid_f) / mid_f * 10_000.0);

            if let Some(future_mid) = future_mids.get(index).copied()
                && future_mid > 0.0
            {
                realised.push(2.0 * direction * (price - future_mid) / mid_f * 10_000.0);
                impact.push(2.0 * direction * (future_mid - mid_f) / mid_f * 10_000.0);
                price_moves.push((future_mid - mid_f) / mid_f * 10_000.0);
                signed_flows.push(direction * size);
            }
        }

        metrics.effective_spread_bps = qip_numerics::stats::mean(&effective);
        metrics.realised_spread_bps = qip_numerics::stats::mean(&realised);
        metrics.price_impact_bps = qip_numerics::stats::mean(&impact);
        metrics.order_flow_imbalance = if absolute_volume > 0.0 {
            (signed_volume / absolute_volume).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        metrics.buy_initiated_fraction = buy_count as f64 / price_forming.len() as f64;
        metrics.kyle_lambda = estimate_kyle_lambda(&signed_flows, &price_moves);
        metrics
    }

    /// Whether the market looks disorderly enough to widen or pause execution.
    ///
    /// Any one of these alone is ordinary; the combination is what execution
    /// algorithms should back away from.
    pub fn is_stressed(&self, normal_spread_bps: f64) -> bool {
        self.quoted_spread_bps > normal_spread_bps * 3.0
            || self.order_flow_imbalance.abs() > 0.8
            || self.price_impact_bps.abs() > normal_spread_bps * 2.0
    }
}

/// The most recent quote at or before the trade.
fn prevailing_quote<'a>(quotes: &'a [Quote], trade: &Trade) -> Option<&'a Quote> {
    quotes
        .iter()
        .rfind(|q| q.at <= trade.at)
        .or_else(|| quotes.first())
}

/// Regress price moves on signed volume; the slope is Kyle's lambda.
fn estimate_kyle_lambda(signed_flows: &[f64], price_moves: &[f64]) -> f64 {
    if signed_flows.len() < 3 {
        return 0.0;
    }
    let Ok(design) =
        qip_numerics::matrix::Matrix::from_vec(signed_flows.len(), 1, signed_flows.to_vec())
    else {
        return 0.0;
    };
    match qip_numerics::stats::ols(&design, price_moves) {
        Ok(fit) => fit.coefficients.get(1).copied().unwrap_or(0.0),
        Err(_) => 0.0,
    }
}
