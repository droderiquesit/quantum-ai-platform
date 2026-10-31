// The inputs that let a one-instrument cycle reach the shipped eligibility
// rule (ADR 0238, owner decision 18 of 2026-10-31).
//
// Since ADR 0238 a thesis whose acting problem is unsatisfiable is
// unresolved and is never sized. `reason/advisory-eligibility`'s
// `r10-eligible` needs an uncontested panel at 5000 bp or more, and a bare
// drop on a quiet tape reaches neither: the simulation analyst bootstraps
// the tape's own returns, and on a flat tape with a drop at the end their
// mean is negative, so it dissents from the reversion buy; and with the
// equity analyst as the only well-grounded supporter the panel settles near
// 0.36. The owner ruled that the pack stays as shipped and the fixtures
// change, so these are the inputs a scenario adds. Each one is data, and
// none of them is policy:
//
// - [`DRIFT`], a small daily rise under the tape's noise, so the
//   bootstrapped expectation is positive and the simulation analyst agrees;
// - [`macro_calendar`], forty prints of the four series the macro analyst
//   reads for the instrument's economy, ending on a risk-supportive step, so
//   a second independent origin supports the claim (the recipe
//   `qip-acceptance`'s `vertical_slice_decide.rs` already uses);
// - [`declaration`], a revenue figure filed three days before the drop, so
//   the catalyst detector links the move to a knowable event. With a macro
//   calendar present and no catalyst the move reads `UnexplainedMove`, which
//   forms no hypothesis at all;
// - [`alternative_data`], forty readings of each alternative-data series
//   ending on a rise, with [`licensed_datasets`] naming the vendor datasets
//   the readings come from. A private fund needs this third origin: the
//   equity analyst declines anything that is not equity, and with the macro
//   and simulation analysts alone a fund settled at 0.48. The licence list
//   is the record that the scenario's own vendor data may be used for
//   decisions; it admits nothing the scenario does not supply.
//
// Measured on 2026-10-31. A listed equity on a 120- or 400-bar tape dropping
// nine or twelve percent, with the macro calendar and the declaration and a
// drift of 0.001 to 0.003, was uncontested, approved at 0.54 to 0.61 and
// proposed a leg; without them the best of 192 tapes was 4823 bp. A private
// fund with all four inputs and a drift of 0.004 was approved at 0.55 and
// proposed a leg; without the alternative data it reached 0.48 at best.

use qip_core::dec;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_financial::intelligence::{
    AlternativeDataPoint, FiscalPeriod, FundamentalUpdate, MacroObservation,
};
use qip_financial::quality::{DataQuality, LicensingClass, Provenance};
use qip_market_ingestion::adapter::SensedRecord;
use qip_world_model::vocabulary::{AltMetric, MacroSeries};

/// The daily rise added under a tape's noise. Forty basis points a day:
/// small beside the final drop, large enough that the bootstrapped
/// expectation is positive and the simulation analyst agrees with the buy.
pub(crate) const DRIFT: f64 = 0.004;

/// Forty prints of each series the macro analyst reads for `region`, ending
/// on a step that is supportive for risk assets by six noise-widths. Each
/// print is knowable on the day it refers to, before `start`.
pub(crate) fn macro_calendar(region: &str, start: Timestamp) -> Vec<SensedRecord> {
    const PRINTS: usize = 40;
    const STEPS: f64 = 6.0;
    let mut records = Vec::new();
    // (series, level, noise amplitude, sign of a risk-supportive move)
    for (series, level, amplitude, supportive) in [
        (MacroSeries::PolicyRate, 4.0, 0.05, -1.0),
        (MacroSeries::InflationYoy, 3.0, 0.1, -1.0),
        (MacroSeries::GrowthYoy, 2.0, 0.2, 1.0),
        (MacroSeries::CreditSpreadBps, 120.0, 4.0, -1.0),
    ] {
        for i in 0..PRINTS {
            let noise = ((i as f64 * 0.7548776662 + 0.3) % 1.0 - 0.5) * 2.0 * amplitude;
            let step = if i == PRINTS - 1 {
                supportive * STEPS * amplitude
            } else {
                0.0
            };
            let at = start.saturating_sub(Duration::from_days((PRINTS - i) as i64 + 1));
            records.push(SensedRecord::Macro(Box::new(MacroObservation {
                series_id: series.series_id(region),
                region: region.to_string(),
                value: level + noise + step,
                unit: "pct".to_string(),
                reference_date: at,
                consensus: None,
                previous: None,
                is_revision: false,
                provenance: Provenance::synthetic("macro-fixture", at)
                    .with_licensing(LicensingClass::Synthetic),
                quality: DataQuality::default(),
                evidence_unretrievable: false,
            })));
        }
    }
    records
}

/// A quarterly revenue figure for `object`, published three days before
/// `start`: the knowable event the catalyst detector links the drop to.
///
/// A filing rather than a dividend declaration, which is what
/// `qip-acceptance`'s vertical slice uses, because a declaration is also a
/// corporate action the book then holds pending until its ex-date, and
/// `position_record.rs` asserts exactly which actions a held position has
/// pending. No consensus is stated, so there is no surprise for the
/// observation detector to raise a second opportunity on.
pub(crate) fn declaration(object: &ObjectId, start: Timestamp) -> SensedRecord {
    let published = start.saturating_sub(Duration::from_days(3));
    SensedRecord::Fundamental(Box::new(FundamentalUpdate {
        entity_id: object.as_str().to_string(),
        metric: "revenue".to_string(),
        value: dec!("1000000"),
        unit: "USD".to_string(),
        period_end: published,
        period: FiscalPeriod::Quarter,
        consensus: None,
        prior_value: None,
        is_restatement: false,
        provenance: Provenance::synthetic("filings-fixture", published),
        quality: DataQuality::default(),
        evidence_unretrievable: false,
    }))
}

/// The vendor datasets [`alternative_data`] reads from, for
/// `PlatformConfig::licensed_datasets`.
pub(crate) fn licensed_datasets() -> Vec<String> {
    AltMetric::ALL
        .iter()
        .map(|metric| metric.dataset().to_string())
        .collect()
}

/// Forty daily readings of each alternative-data series on `object`, quiet
/// and then twelve noise-widths up on the last, two days before `start`.
pub(crate) fn alternative_data(object: &ObjectId, start: Timestamp) -> Vec<SensedRecord> {
    const READINGS: usize = 40;
    let mut records = Vec::new();
    for metric in AltMetric::ALL {
        for i in 0..READINGS {
            let at = start.saturating_sub(Duration::from_days((READINGS - i) as i64 + 1));
            let noise = ((i as f64 * 0.7548776662 + 0.1) % 1.0 - 0.5) * 2.0;
            let step = if i == READINGS - 1 { 12.0 } else { 0.0 };
            records.push(SensedRecord::AlternativeData(Box::new(
                AlternativeDataPoint {
                    dataset: metric.dataset().to_string(),
                    subject_id: object.as_str().to_string(),
                    metric: metric.feature().to_string(),
                    value: 100.0 + noise + step,
                    unit: "index".to_string(),
                    observed_at: at,
                    lead_days: 14.0,
                    proxy_correlation: 0.6,
                    proxies_for: None,
                    provenance: Provenance::new("alt-vendor", at, at),
                    quality: DataQuality::default(),
                    evidence_unretrievable: false,
                },
            )));
        }
    }
    records
}

/// [`macro_calendar`] for `region`, and a [`declaration`] and
/// [`alternative_data`] on each object: the context a drifting tape needs
/// beside it. A platform fed this licenses [`licensed_datasets`].
pub(crate) fn context(region: &str, objects: &[ObjectId], start: Timestamp) -> Vec<SensedRecord> {
    let mut records = macro_calendar(region, start);
    for object in objects {
        records.push(declaration(object, start));
        records.extend(alternative_data(object, start));
    }
    records
}
