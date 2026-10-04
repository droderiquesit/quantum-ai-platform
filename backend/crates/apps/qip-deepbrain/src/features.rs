//! The bar-derived features a model is fitted on, and the lineage that names
//! them (MODEL-029).
//!
//! Kept in a file of their own so that [`code_digest`] can be the SHA-256 of
//! *this file's source*: the digest of the transformation code is then a fact
//! about the code a fit ran, and it changes when a feature definition does and
//! not when an unrelated part of the learning desk is edited. A version label
//! a person remembers to bump would be a claim about the code; this is the
//! code.

use qip_core::Timestamp;
use qip_core::hash::sha256_hex;
use qip_market::bar::Bar;
use std::collections::BTreeMap;

/// The features a bar-derived model reads, in the order the dataset carries
/// them.
///
/// Deliberately the vocabulary the strategy harness already computes rather
/// than a second one: two definitions of "momentum over five bars" that drift
/// apart is a defect nobody finds, because both look right in isolation.
pub const FEATURES: [&str; 5] = [
    "return_1",
    "momentum_5",
    "volatility_10",
    "range_frac",
    "volume_share",
];

/// Bars of history a feature row needs behind it.
///
/// The longest window any feature above reads. A row assembled with less is not
/// a row with a smaller window; it is a row whose features are computed from
/// data that is not there.
pub const LOOKBACK: usize = 10;

/// Feature columns over `bars`, one row per bar that has both a full lookback
/// behind it and a next bar ahead of it.
///
/// Row *i* reads bars up to and including `bars[LOOKBACK + i]`, and the target
/// for that row spans that bar to the next. The label is therefore always on
/// the far side of every value used to predict it, which is the property a
/// backtest cannot recover if the dataset does not have it.
pub fn feature_columns(bars: &[Bar]) -> BTreeMap<String, Vec<f64>> {
    let mut columns: BTreeMap<String, Vec<f64>> = FEATURES
        .iter()
        .map(|name| ((*name).to_string(), Vec::new()))
        .collect();
    if bars.len() <= LOOKBACK + 1 {
        return columns;
    }
    let closes: Vec<f64> = bars.iter().map(|bar| bar.close.to_f64()).collect();
    let volumes: Vec<f64> = bars.iter().map(|bar| bar.volume.to_f64()).collect();

    let mut push = |name: &str, value: f64| {
        if let Some(column) = columns.get_mut(name) {
            column.push(if value.is_finite() { value } else { 0.0 });
        }
    };

    for at in LOOKBACK..bars.len() - 1 {
        let close = closes[at];
        push("return_1", ratio(close, closes[at - 1]));
        push("momentum_5", ratio(close, closes[at - 5]));

        let window: Vec<f64> = (at - 9..=at)
            .map(|i| ratio(closes[i], closes[i - 1]))
            .collect();
        push("volatility_10", qip_numerics::stats::stddev(&window));

        let bar = &bars[at];
        let high = bar.high.to_f64();
        let low = bar.low.to_f64();
        push(
            "range_frac",
            if close.abs() > f64::EPSILON {
                (high - low) / close
            } else {
                0.0
            },
        );

        // Volume relative to its own trailing mean, not raw volume. A raw
        // level is an instrument-specific magnitude, and a model fitted on one
        // instrument's volume learns that instrument's size rather than
        // anything about markets.
        let trailing: f64 = volumes[at - LOOKBACK..at].iter().sum::<f64>() / LOOKBACK as f64;
        push(
            "volume_share",
            if trailing > f64::EPSILON {
                volumes[at] / trailing
            } else {
                0.0
            },
        );
    }
    columns
}

/// The return from each feature row's bar to the next.
pub fn next_bar_returns(bars: &[Bar]) -> Vec<f64> {
    if bars.len() <= LOOKBACK + 1 {
        return Vec::new();
    }
    (LOOKBACK..bars.len() - 1)
        .map(|at| ratio(bars[at + 1].close.to_f64(), bars[at].close.to_f64()))
        .collect()
}

/// A simple return, guarded against a zero denominator.
///
/// The crossing point from money to statistics: the closes are `Decimal`
/// because they are prices, and everything from here is `f64` because a return
/// is a ratio and a ratio is not money.
pub(crate) fn ratio(current: f64, previous: f64) -> f64 {
    if previous.abs() < 1e-12 {
        return 0.0;
    }
    current / previous - 1.0
}

/// SHA-256 of this file's source: the version of the transformation code.
pub fn code_digest() -> String {
    sha256_hex(include_str!("features.rs").as_bytes())
}

/// SHA-256 over the canonical text of every bar a window holds: the identity
/// of the source data a feature column was computed from.
pub fn source_digest(bars: &[Bar]) -> String {
    let mut text = String::new();
    for bar in bars {
        text.push_str(&format!(
            "{}|{}|{}|{}|{}|{}\n",
            bar.close_time().as_nanos(),
            bar.open,
            bar.high,
            bar.low,
            bar.close,
            bar.volume
        ));
    }
    sha256_hex(text.as_bytes())
}

/// Where a model's features came from: the source window, the code that
/// computed them, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureLineage {
    pub features: Vec<String>,
    pub code_digest: String,
    pub source_digest: String,
    pub computed_at: Timestamp,
}

impl FeatureLineage {
    /// The lineage of a fit over `bars` at `now`, computed by this code.
    pub fn of(bars: &[Bar], now: Timestamp) -> Self {
        Self {
            features: FEATURES.iter().map(|name| (*name).to_string()).collect(),
            code_digest: code_digest(),
            source_digest: source_digest(bars),
            computed_at: now,
        }
    }

    /// Write the lineage onto a card's parameters, where it outlives this
    /// process's own bookkeeping.
    pub fn record_on(&self, card: &mut qip_ai::registry::ModelCard) {
        card.parameters
            .insert("feature_code_digest".to_string(), self.code_digest.clone());
        card.parameters.insert(
            "feature_source_digest".to_string(),
            self.source_digest.clone(),
        );
        card.parameters.insert(
            "feature_computed_at".to_string(),
            self.computed_at.to_rfc3339(),
        );
    }
}
