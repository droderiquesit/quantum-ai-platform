//! Adding up what the cells have independently decided to hold.
//!
//! This is the failure mode a distributed trading system has and a
//! single-process one does not. Each cell is correct on its own terms: it
//! trades inside a grant somebody approved, checks its own limits, and never
//! exceeds them. Three of them can still each buy a fifth of their book in the
//! same name, on the same thesis, from the same signal — and nobody who can
//! see the whole position has looked at it. The concentration is not any one
//! cell's fault and it is not visible from inside any one cell.
//!
//! So the central plane keeps its own view: every cell's positions, added up
//! along the axes a limit is actually written against — instrument, sector,
//! venue, currency, and cell. [`qip_portfolio::exposure::Exposure`] already
//! does the per-axis netting and concentration arithmetic, so this module
//! supplies the axes and the cross-cell question the single-book type has no
//! reason to ask: [`AggregateExposure::crowded`], which names the instruments
//! more than one cell has independently accumulated.

use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::{Currency, Decimal};
use qip_financial::asset_class::Sector;
use qip_portfolio::exposure::Exposure;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One position at one cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellPosition {
    pub cell: String,
    pub strategy: StrategyId,
    /// The instrument, as the symbol the central plane resolves everything to.
    pub instrument: String,
    pub sector: Sector,
    pub venue: VenueId,
    pub currency: Currency,
    /// Signed: negative is short.
    pub quantity: Decimal,
    pub price: Decimal,
}

impl CellPosition {
    /// Signed notional, in the position's own currency.
    pub fn signed_notional(&self) -> Decimal {
        self.quantity
            .checked_mul(self.price)
            .unwrap_or(Decimal::ZERO)
    }

    pub fn is_short(&self) -> bool {
        self.quantity.is_negative()
    }
}

/// An instrument several cells hold at once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrowdedPosition {
    pub instrument: String,
    /// Cells holding it, in a stable order.
    pub cells: Vec<String>,
    /// Net across cells: long and short in the same name partly offset, and a
    /// net near zero with a large gross is a different problem from a large
    /// net, not a smaller one.
    pub net: Decimal,
    pub gross: Decimal,
    /// The net position's share of the whole book's gross.
    pub share_of_gross: f64,
    /// True where the cells disagree — some long, some short.
    pub cells_disagree: bool,
}

/// A concentration that exceeds a stated limit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConcentrationFinding {
    /// Which axis: `instrument`, `sector`, `venue`, `currency` or `cell`.
    pub axis: &'static str,
    pub bucket: String,
    pub gross: Decimal,
    /// Share of the book's gross exposure.
    pub share: f64,
    pub limit: f64,
}

impl ConcentrationFinding {
    pub fn describe(&self) -> String {
        format!(
            "{} {} is {:.1}% of gross against a {:.1}% limit ({})",
            self.axis,
            self.bucket,
            self.share * 100.0,
            self.limit * 100.0,
            self.gross
        )
    }
}

/// The shares of gross exposure any one bucket may take, per axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConcentrationLimits {
    pub per_instrument: f64,
    pub per_sector: f64,
    pub per_venue: f64,
    pub per_currency: f64,
    pub per_cell: f64,
}

impl Default for ConcentrationLimits {
    fn default() -> Self {
        // Loose enough that a deliberately concentrated book is possible and
        // tight enough that an accidental one is visible. The venue limit is
        // the tightest of the three market axes because it is an outage
        // boundary as well as a concentration one: half the book at one venue
        // is half the book unable to trade when that venue halts.
        Self {
            per_instrument: 0.10,
            per_sector: 0.30,
            per_venue: 0.40,
            per_currency: 0.60,
            per_cell: 0.50,
        }
    }
}

/// The whole book, as the central plane sees it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AggregateExposure {
    pub by_instrument: Exposure,
    pub by_sector: Exposure,
    pub by_venue: Exposure,
    pub by_currency: Exposure,
    pub by_cell: Exposure,
    /// Which cells hold each instrument.
    contributors: BTreeMap<String, BTreeSet<String>>,
    /// Signed notional per instrument per cell, for detecting cells that
    /// disagree with one another about the same name.
    per_cell_instrument: BTreeMap<String, BTreeMap<String, Decimal>>,
}

impl AggregateExposure {
    /// Aggregate independently-held positions from any number of cells.
    pub fn of(positions: &[CellPosition]) -> Self {
        let mut aggregate = Self::default();
        for position in positions {
            let notional = position.signed_notional();
            aggregate.by_instrument.add(&position.instrument, notional);
            aggregate.by_sector.add(position.sector.as_str(), notional);
            aggregate.by_venue.add(position.venue.as_str(), notional);
            aggregate
                .by_currency
                .add(position.currency.as_str(), notional);
            aggregate.by_cell.add(&position.cell, notional);
            aggregate
                .contributors
                .entry(position.instrument.clone())
                .or_default()
                .insert(position.cell.clone());
            *aggregate
                .per_cell_instrument
                .entry(position.instrument.clone())
                .or_default()
                .entry(position.cell.clone())
                .or_insert(Decimal::ZERO) += notional;
        }
        aggregate
    }

    /// Unsigned exposure across the whole book.
    ///
    /// Taken from the instrument axis, since every position belongs to exactly
    /// one instrument and to exactly one of every other axis too — so all five
    /// axes agree, and a disagreement would be a bug in [`Self::of`].
    pub fn gross(&self) -> Decimal {
        self.by_instrument.total_gross()
    }

    /// Signed exposure across the whole book.
    pub fn net(&self) -> Decimal {
        self.by_instrument.total_net()
    }

    /// Leverage against posted equity.
    pub fn leverage(&self, equity: Decimal) -> f64 {
        if !equity.is_positive() {
            return f64::INFINITY;
        }
        self.gross().to_f64() / equity.to_f64()
    }

    /// Cells holding a given instrument.
    pub fn cells_holding(&self, instrument: &str) -> Vec<&str> {
        self.contributors
            .get(instrument)
            .map(|cells| cells.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Instruments held independently by at least `minimum_cells` cells.
    ///
    /// The question no single cell can answer. Ordered by gross, largest
    /// first, because the answer is read during an incident.
    pub fn crowded(&self, minimum_cells: usize) -> Vec<CrowdedPosition> {
        let total_gross = self.gross().to_f64();
        let mut out: Vec<CrowdedPosition> = self
            .contributors
            .iter()
            .filter(|(_, cells)| cells.len() >= minimum_cells.max(2))
            .map(|(instrument, cells)| {
                let per_cell = self.per_cell_instrument.get(instrument);
                let cells_disagree = per_cell.is_some_and(|holdings| {
                    holdings.values().any(|v| v.is_positive())
                        && holdings.values().any(|v| v.is_negative())
                });
                let gross = self.by_instrument.gross_of(instrument);
                CrowdedPosition {
                    instrument: instrument.clone(),
                    cells: cells.iter().cloned().collect(),
                    net: self.by_instrument.net_of(instrument),
                    gross,
                    share_of_gross: if total_gross > 0.0 {
                        gross.to_f64() / total_gross
                    } else {
                        0.0
                    },
                    cells_disagree,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            b.gross
                .cmp(&a.gross)
                .then_with(|| a.instrument.cmp(&b.instrument))
        });
        out
    }

    /// Every bucket that exceeds its share of gross, on any axis.
    pub fn concentrations(&self, limits: &ConcentrationLimits) -> Vec<ConcentrationFinding> {
        let mut findings = Vec::new();
        for (axis, exposure, limit) in [
            ("instrument", &self.by_instrument, limits.per_instrument),
            ("sector", &self.by_sector, limits.per_sector),
            ("venue", &self.by_venue, limits.per_venue),
            ("currency", &self.by_currency, limits.per_currency),
            ("cell", &self.by_cell, limits.per_cell),
        ] {
            for (bucket, share) in exposure.shares() {
                if share > limit {
                    findings.push(ConcentrationFinding {
                        axis,
                        bucket: bucket.clone(),
                        gross: exposure.gross_of(&bucket),
                        share,
                        limit,
                    });
                }
            }
        }
        findings.sort_by(|a, b| {
            b.share
                .total_cmp(&a.share)
                .then_with(|| a.axis.cmp(b.axis))
                .then_with(|| a.bucket.cmp(&b.bucket))
        });
        findings
    }
}
