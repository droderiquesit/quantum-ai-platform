//! Canonicalisation of provider records.

use qip_core::{Decimal, ObjectId, Timestamp};
use qip_market_ingestion::adapter::SensedRecord;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;

/// Maps a provider's symbol onto a canonical object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SymbolMapping {
    pub provider: String,
    /// Symbol as the provider writes it.
    pub provider_symbol: String,
    pub object_id: ObjectId,
    pub canonical_symbol: String,
    pub canonical_venue: String,
}

/// A multiplicative correction applied to a provider's prices.
///
/// The classic case is London equities quoted in pence against a currency of
/// GBP: without a 0.01 factor every notional is a hundred times too large.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnitConversion {
    pub provider: String,
    /// Object the conversion applies to, or `None` for every object from the
    /// provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<ObjectId>,
    pub price_factor: Decimal,
    pub quantity_factor: Decimal,
    pub reason: String,
}

/// Exactly 0.01, built from `Decimal`'s raw fixed-point representation.
///
/// Not parsed. A parsed constant carries a failure arm, and on a unit conversion
/// every value that arm could supply is wrong in the dangerous direction: a
/// fallback of `ONE` passes pence through untouched and every notional
/// downstream is a hundred times too large, which no ratio-based contract can
/// see (see [`ScaleGuard`]). `from_raw` is `const` and total, so the failure
/// cannot occur and there is nothing to fall back to.
const PENCE_PER_POUND: Decimal = Decimal::from_raw(qip_core::decimal::SCALE / 100);

impl UnitConversion {
    /// Pence-quoted instruments on a pounds-denominated venue.
    pub fn pence_to_pounds(provider: impl Into<String>, object_id: ObjectId) -> Self {
        Self {
            provider: provider.into(),
            object_id: Some(object_id),
            price_factor: PENCE_PER_POUND,
            quantity_factor: Decimal::ONE,
            reason: "provider quotes in pence against a GBP-denominated instrument".into(),
        }
    }
}

/// What normalisation did to a batch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NormalizationReport {
    pub processed: u64,
    pub symbols_mapped: u64,
    pub venues_canonicalised: u64,
    pub units_converted: u64,
    pub timestamps_corrected: u64,
    /// Records dropped because no mapping existed for the symbol.
    pub unmapped: u64,
    /// Price discontinuities the scale guard flagged.
    pub scale_warnings: Vec<String>,
    /// Provider symbols with no mapping, so the gap can be filled.
    pub unmapped_symbols: BTreeMap<String, u64>,
}

impl NormalizationReport {
    pub fn had_changes(&self) -> bool {
        self.symbols_mapped
            + self.venues_canonicalised
            + self.units_converted
            + self.timestamps_corrected
            > 0
    }
}

/// Detects step changes in an instrument's price level that no market
/// produces.
///
/// Ratio-based contracts cannot catch a unit error: a spread expressed in basis
/// points is scale-invariant, so a whole feed switching from pounds to pence
/// leaves every ratio rule satisfied while every notional is wrong by a factor
/// of a hundred. The only thing that catches it is continuity against the last
/// accepted price, which is what this does.
#[derive(Debug)]
pub struct ScaleGuard {
    last_price: RefCell<BTreeMap<String, f64>>,
    /// Largest single-step ratio treated as a real market move.
    max_ratio: f64,
}

impl Default for ScaleGuard {
    fn default() -> Self {
        // A 4x move in one observation is a unit error or an unapplied
        // corporate action. Real markets do move violently, but not by 300% in
        // one tick, and the cases that come close arrive as corporate actions.
        Self {
            last_price: RefCell::new(BTreeMap::new()),
            max_ratio: 4.0,
        }
    }
}

impl ScaleGuard {
    pub fn new(max_ratio: f64) -> Self {
        Self {
            last_price: RefCell::new(BTreeMap::new()),
            max_ratio: max_ratio.max(1.0001),
        }
    }

    /// Check a price against the last one accepted for the same object.
    ///
    /// Returns a description of the discontinuity, or `None` when the move is
    /// within bounds. The new price is recorded either way, so a genuine
    /// re-denomination is flagged once rather than on every subsequent record.
    pub fn check(&self, object_id: &ObjectId, price: f64) -> Option<String> {
        if !price.is_finite() || price <= 0.0 {
            return Some(format!("non-positive price {price}"));
        }
        let key = object_id.as_str().to_string();
        let previous = self.last_price.borrow().get(&key).copied();
        self.last_price.borrow_mut().insert(key, price);

        let previous = previous?;
        let ratio = if price > previous {
            price / previous
        } else {
            previous / price
        };
        if ratio > self.max_ratio {
            return Some(format!(
                "price moved from {previous} to {price}, a factor of {ratio:.1}: probable unit \
                 error or unapplied corporate action"
            ));
        }
        None
    }

    /// Forget the recorded level, after a corporate action legitimately
    /// re-bases the price.
    pub fn reset(&self, object_id: &ObjectId) {
        self.last_price.borrow_mut().remove(object_id.as_str());
    }

    pub fn tracked(&self) -> usize {
        self.last_price.borrow().len()
    }
}

/// Rewrites provider records into canonical form.
#[derive(Debug, Default)]
pub struct Normalizer {
    symbols: BTreeMap<(String, String), SymbolMapping>,
    venue_aliases: BTreeMap<String, String>,
    conversions: Vec<UnitConversion>,
    /// Drop records whose symbol has no mapping. On by default: an unmapped
    /// symbol is an unknown instrument, and guessing produces a phantom.
    drop_unmapped: bool,
    scale_guard: ScaleGuard,
}

impl Normalizer {
    pub fn new() -> Self {
        Self {
            drop_unmapped: true,
            ..Self::default()
        }
    }

    /// The scale guard, for inspecting or resetting price continuity.
    pub fn scale_guard(&self) -> &ScaleGuard {
        &self.scale_guard
    }

    /// A normalizer preloaded with the common venue aliases.
    pub fn with_standard_venues() -> Self {
        let mut normalizer = Self::new();
        for (alias, canonical) in [
            ("NYSE", "XNYS"),
            ("N", "XNYS"),
            ("NASDAQ", "XNAS"),
            ("Q", "XNAS"),
            ("O", "XNAS"),
            ("LSE", "XLON"),
            ("L", "XLON"),
            ("LON", "XLON"),
            ("XETRA", "XETR"),
            ("DE", "XETR"),
            ("TSE", "XTKS"),
            ("T", "XTKS"),
            ("OTC", "OTCM"),
        ] {
            normalizer.add_venue_alias(alias, canonical);
        }
        normalizer
    }

    pub fn add_symbol_mapping(&mut self, mapping: SymbolMapping) {
        self.symbols.insert(
            (
                mapping.provider.to_ascii_lowercase(),
                normalise_key(&mapping.provider_symbol),
            ),
            mapping,
        );
    }

    pub fn add_venue_alias(&mut self, alias: impl Into<String>, canonical: impl Into<String>) {
        self.venue_aliases
            .insert(normalise_key(&alias.into()), canonical.into());
    }

    pub fn add_conversion(&mut self, conversion: UnitConversion) {
        self.conversions.push(conversion);
    }

    /// Whether records with unknown symbols are dropped or passed through.
    pub fn dropping_unmapped(mut self, drop: bool) -> Self {
        self.drop_unmapped = drop;
        self
    }

    pub fn canonical_venue(&self, venue: &str) -> Option<&str> {
        self.venue_aliases
            .get(&normalise_key(venue))
            .map(String::as_str)
    }

    pub fn lookup(&self, provider: &str, symbol: &str) -> Option<&SymbolMapping> {
        self.symbols
            .get(&(provider.to_ascii_lowercase(), normalise_key(symbol)))
    }

    /// Normalise a batch, returning the rewritten records and a report.
    pub fn normalise(
        &self,
        provider: &str,
        records: Vec<SensedRecord>,
        received_at: Timestamp,
    ) -> (Vec<SensedRecord>, NormalizationReport) {
        let mut report = NormalizationReport::default();
        let mut out = Vec::with_capacity(records.len());

        for mut record in records {
            report.processed += 1;

            if let Some(venue) = record_venue(&record) {
                if let Some(canonical) = self.canonical_venue(&venue)
                    && canonical != venue
                {
                    set_record_venue(&mut record, canonical.to_string());
                    report.venues_canonicalised += 1;
                }
            }

            if let Some(object_id) = record_object(&record) {
                for conversion in &self.conversions {
                    let applies = conversion.provider.eq_ignore_ascii_case(provider)
                        && conversion
                            .object_id
                            .as_ref()
                            .is_none_or(|id| id == &object_id);
                    if applies && apply_conversion(&mut record, conversion) {
                        report.units_converted += 1;
                    }
                }
            }

            // Continuity check against the last accepted price for the object.
            if let (Some(object_id), Some(price)) = (record_object(&record), record_price(&record))
                && let Some(warning) = self.scale_guard.check(&object_id, price)
            {
                report
                    .scale_warnings
                    .push(format!("{object_id}: {warning}"));
            }

            // A record stamped in the future is a clock or unit error upstream;
            // clamping it is safer than letting it poison a point-in-time view.
            // `clamp_timestamp` reports whether it actually changed anything —
            // the report must never claim a correction that did not happen.
            if record.occurred_at() > received_at && clamp_timestamp(&mut record, received_at) {
                report.timestamps_corrected += 1;
            }

            out.push(record);
        }

        (out, report)
    }

    /// Resolve a provider symbol, recording a miss when there is no mapping.
    pub fn resolve_symbol(
        &self,
        provider: &str,
        symbol: &str,
        report: &mut NormalizationReport,
    ) -> Option<&SymbolMapping> {
        match self.lookup(provider, symbol) {
            Some(mapping) => {
                report.symbols_mapped += 1;
                Some(mapping)
            }
            None => {
                report.unmapped += 1;
                *report
                    .unmapped_symbols
                    .entry(format!("{provider}:{symbol}"))
                    .or_insert(0) += 1;
                None
            }
        }
    }
}

/// The representative price of a record, for the continuity check.
fn record_price(record: &SensedRecord) -> Option<f64> {
    match record {
        SensedRecord::Tick(t) => Some(t.price.to_f64()),
        SensedRecord::Quote(q) => q.mid().map(|m| m.to_f64()),
        SensedRecord::Trade(t) => Some(t.price.to_f64()),
        SensedRecord::Book(b) => b.mid().map(|m| m.to_f64()),
        SensedRecord::Bar(b) => Some(b.close.to_f64()),
        _ => None,
    }
}

fn normalise_key(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

fn record_venue(record: &SensedRecord) -> Option<String> {
    match record {
        SensedRecord::Tick(t) => Some(t.venue.clone()),
        SensedRecord::Quote(q) => Some(q.venue.clone()),
        SensedRecord::Trade(t) => Some(t.venue.clone()),
        SensedRecord::Book(b) => Some(b.venue.clone()),
        SensedRecord::Bar(b) => Some(b.venue.clone()),
        _ => None,
    }
}

fn set_record_venue(record: &mut SensedRecord, venue: String) {
    match record {
        SensedRecord::Tick(t) => t.venue = venue,
        SensedRecord::Quote(q) => q.venue = venue,
        SensedRecord::Trade(t) => t.venue = venue,
        SensedRecord::Book(b) => b.venue = venue,
        SensedRecord::Bar(b) => b.venue = venue,
        _ => {}
    }
}

fn record_object(record: &SensedRecord) -> Option<ObjectId> {
    match record {
        SensedRecord::Tick(t) => Some(t.object_id.clone()),
        SensedRecord::Quote(q) => Some(q.object_id.clone()),
        SensedRecord::Trade(t) => Some(t.object_id.clone()),
        SensedRecord::Book(b) => Some(b.object_id.clone()),
        SensedRecord::Bar(b) => Some(b.object_id.clone()),
        SensedRecord::CorporateAction(a) => Some(a.object_id.clone()),
        _ => None,
    }
}

/// Apply a unit conversion in place. Returns whether anything changed.
fn apply_conversion(record: &mut SensedRecord, conversion: &UnitConversion) -> bool {
    let price = conversion.price_factor;
    let quantity = conversion.quantity_factor;
    if price == Decimal::ONE && quantity == Decimal::ONE {
        return false;
    }
    match record {
        SensedRecord::Tick(t) => {
            t.price = t.price * price;
            t.volume = t.volume * quantity;
        }
        SensedRecord::Quote(q) => {
            q.bid = q.bid * price;
            q.ask = q.ask * price;
            q.bid_size = q.bid_size * quantity;
            q.ask_size = q.ask_size * quantity;
        }
        SensedRecord::Trade(t) => {
            t.price = t.price * price;
            t.size = t.size * quantity;
        }
        SensedRecord::Book(b) => {
            for level in b.bids.iter_mut().chain(b.asks.iter_mut()) {
                level.price = level.price * price;
                level.size = level.size * quantity;
            }
        }
        SensedRecord::Bar(b) => {
            b.open = b.open * price;
            b.high = b.high * price;
            b.low = b.low * price;
            b.close = b.close * price;
            b.volume = b.volume * quantity;
            b.vwap = b.vwap.map(|v| v * price);
        }
        _ => return false,
    }
    true
}

/// Force a future-stamped record's occurred-at back to the moment it was
/// received. Returns whether the record was actually changed.
///
/// Only record kinds whose occurred-at is an observation of something that
/// has already happened are handled: a tick, quote, trade, book snapshot, bar
/// close or a news item's publication can never legitimately be in the
/// future, so a future stamp on one of those is a clock or unit error
/// upstream. A corporate action's ex-date, a reference-data effective-from
/// date, a fundamental's period end and a macro series's reference date are
/// routinely announced or scheduled ahead of when they take effect — forcing
/// those to "now" would destroy exactly the information they exist to carry
/// — so this leaves them untouched and returns `false`.
///
/// The bug this prevents: the caller used to increment
/// `NormalizationReport::timestamps_corrected` unconditionally whenever
/// `occurred_at() > received_at`, including for the record kinds this match
/// falls through on (`_ => {}`, a no-op). The report then claimed a
/// correction — the exact defect a downstream consumer would rely on to
/// believe the record's timestamp was safe — that never happened, and a
/// future-dated `Bar` in particular passed through completely unclamped
/// despite being counted as fixed.
fn clamp_timestamp(record: &mut SensedRecord, at: Timestamp) -> bool {
    match record {
        SensedRecord::Tick(t) => {
            t.at = at;
            true
        }
        SensedRecord::Quote(q) => {
            q.at = at;
            true
        }
        SensedRecord::Trade(t) => {
            t.at = at;
            true
        }
        SensedRecord::Book(b) => {
            b.at = at;
            true
        }
        SensedRecord::News(n) => {
            n.published_at = at;
            true
        }
        SensedRecord::Bar(b) => {
            // `Bar::close_time` is derived from `open_time + interval`, so
            // clamping the close requires pushing `open_time` back by the
            // bar's own duration rather than assigning a field directly.
            b.open_time = at.saturating_sub(b.interval.duration());
            true
        }
        _ => false,
    }
}
