//! The deterministic Risk Gate at the cell's one order seam (R12-RISK-4093,
//! R12-RISK-7205, W1.9).
//!
//! The desk's order manager runs `PreTradeChecker::check` — the projected
//! post-trade state against a `LimitSet` — before it reaches a broker. The cell
//! did not. A cell decides alone (ADR 0008), and what it checked before
//! `Cell::send` called a `Placer` was the envelope (per-order and gross limits
//! per strategy grant), the region hold, feasibility, the mode gate and the
//! live-venue refusal. Nothing measured the cell's *aggregate* book: a dozen
//! grants each inside its own envelope could together carry the cell to any
//! leverage, any single-name weight and any concentration at one venue, and
//! the first number that would have noticed was the centre's, after the fact.
//!
//! [`CellRiskGate`] is the same `PreTradeChecker` the desk runs, fed a
//! `RiskState` built from what the cell itself holds — confirmed positions,
//! working orders, and the grants it has verified — so the check is local,
//! deterministic and model-free, and needs nothing the cell cannot see while
//! partitioned from the centre.
//!
//! # Only limits the cell can evaluate
//!
//! A limit whose figure the cell has no producer for would take its silent
//! `None` arm and read as a pass — the `MaxExpectedShortfall` defect. So the
//! gate refuses, at installation, a `LimitSet` naming any kind outside
//! [`CellRiskGate::evaluable`], and names the kind. Value at risk, volatility,
//! drawdown, daily loss, the liquidity floors and the cash buffer need a
//! return series, a day anchor or a cash ledger the cell does not keep; they
//! stay with the desk's gate until the cell keeps one.
//!
//! # Exposure is counted conservatively
//!
//! A working order counts at its full remaining size, at its own limit price,
//! as though it had filled: an order the venue may fill is exposure the cell
//! has already committed to. A held position is priced at the last price this
//! gate saw an order for that instrument at. A position the gate has never
//! seen a price for — the gate was installed after the position was built —
//! cannot be priced, and the state files it as unevaluated, which the checker
//! refuses on: a book the gate cannot measure is not a book it passes.
//!
//! # The survival ceiling
//!
//! `LimitKind::MaxSurvivalGross` reads the gross the cell's live verified
//! grants sum to: the capital the centre released to this cell after its own
//! survival kernel. A cell whose grants have expired stands behind nothing new
//! — and may still sell, because that limit never refuses an order that sheds
//! gross.

use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_risk::limits::{COUNTERPARTY_AXIS, LimitKind, LimitSet, RiskState};
use qip_risk_engine::pretrade::{PreTradeChecker, PreTradeResult, ProposedOrder};
use std::collections::BTreeMap;

/// The gate a cell refuses under when its Risk Gate rejects an order. A
/// constant, like its neighbours in `cell.rs`, so the `gate` label on
/// `qip_edge_refusals_total` stays bounded by source text.
pub const GATE_RISK: &str = "risk_gate";

/// The figure name an unpriceable position is filed under. Fixed, because it
/// reaches the refusal text and must never carry an instrument.
pub const FIGURE_CELL_EXPOSURE: &str = "cell_exposure";

/// The marks the gate keeps: one per instrument, bounded by the instruments the
/// cell sends orders for.
const MAX_MARKS: usize = 4_096;

/// The cell's pre-trade Risk Gate.
#[derive(Debug)]
pub struct CellRiskGate {
    checker: PreTradeChecker,
    /// The capital ratios are measured against.
    capital: Decimal,
    /// The last price an order was sent at, per instrument.
    marks: BTreeMap<String, Decimal>,
}

/// One position or working order as the gate prices it.
pub(crate) struct Holding<'a> {
    pub venue: &'a str,
    pub object: &'a str,
    /// Positive long, negative short.
    pub signed_quantity: Decimal,
    /// The order's own limit price for a working order; `None` for a held
    /// position, which is priced at the gate's mark.
    pub price: Option<Decimal>,
}

impl CellRiskGate {
    /// Whether the cell can produce the figure `kind` reads.
    pub fn evaluable(kind: &LimitKind) -> bool {
        match kind {
            LimitKind::MaxOrderNotional { .. }
            | LimitKind::MaxPositionNotional { .. }
            | LimitKind::MaxPositionWeight { .. }
            | LimitKind::MaxLeverage { .. }
            | LimitKind::MaxNetExposure { .. }
            | LimitKind::MaxCounterpartyExposure { .. }
            | LimitKind::MaxVenueExposure { .. }
            | LimitKind::MaxVenueNotionalRate { .. }
            | LimitKind::MaxSurvivalGross { .. } => true,
            // The only axis the cell charges is the venue it sends to.
            LimitKind::MaxConcentration { axis, .. }
            | LimitKind::MaxAxisWeight { axis, .. }
            | LimitKind::MaxBucketExposure { axis, .. } => axis == COUNTERPARTY_AXIS,
            LimitKind::MaxVolatility { .. }
            | LimitKind::MaxValueAtRisk { .. }
            | LimitKind::MaxExpectedShortfall { .. }
            | LimitKind::MaxDrawdown { .. }
            | LimitKind::MaxDailyLoss { .. }
            | LimitKind::MinLiquidity { .. }
            | LimitKind::MaxDaysToLiquidate { .. }
            | LimitKind::MinCashBuffer { .. } => false,
        }
    }

    /// A gate evaluating `limits` against `capital`.
    ///
    /// Refuses non-positive capital (every ratio would be undefined), an empty
    /// set (a gate that checks nothing reads as one that checked), and any
    /// limit the cell cannot produce a figure for.
    pub fn new(limits: LimitSet, capital: Decimal) -> Result<Self> {
        if !capital.is_positive() {
            return Err(Error::invalid(format!(
                "a cell risk gate needs positive capital to measure ratios against, got {capital}"
            )));
        }
        if limits.is_empty() {
            return Err(Error::invalid(
                "a cell risk gate with no limits checks nothing and would read as one that checked",
            ));
        }
        let unevaluable: Vec<String> = limits
            .limits
            .iter()
            .filter(|limit| !Self::evaluable(&limit.kind))
            .map(|limit| format!("{} ({})", limit.name, limit.kind.label()))
            .collect();
        if !unevaluable.is_empty() {
            return Err(Error::invalid(format!(
                "the cell has no producer for the figure these limits read, so at the venue they \
                 would pass silently: {}; evaluate them at the desk's gate, or give the cell a \
                 producer first",
                unevaluable.join(", ")
            )));
        }
        Ok(Self {
            checker: PreTradeChecker::new(limits),
            capital,
            marks: BTreeMap::new(),
        })
    }

    pub fn limits(&self) -> &LimitSet {
        self.checker.limits()
    }

    pub fn capital(&self) -> Decimal {
        self.capital
    }

    fn mark(&mut self, object: &str, price: Decimal) {
        if !self.marks.contains_key(object) && self.marks.len() >= MAX_MARKS {
            // Bounded: the oldest key leaves. A position on it is then
            // unpriceable and refuses, which is the safe direction.
            if let Some(first) = self.marks.keys().next().cloned() {
                self.marks.remove(&first);
            }
        }
        self.marks.insert(object.to_string(), price);
    }

    /// Check one order against the cell's book.
    ///
    /// `holdings` is every confirmed position and every working order;
    /// `granted` is the gross the cell's live verified grants sum to. The
    /// order's own price becomes the instrument's mark first, so a position
    /// on the same instrument is priced where the cell is about to trade it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn check<'a>(
        &mut self,
        cell: &str,
        holdings: impl IntoIterator<Item = Holding<'a>>,
        granted: Decimal,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        now: Timestamp,
    ) -> Result<PreTradeResult> {
        self.mark(object_id.as_str(), price);

        // Signed notional per venue and instrument, so a venue's gross can be
        // projected exactly for the order below.
        let mut per_pair: BTreeMap<(String, String), Decimal> = BTreeMap::new();
        let mut unpriced: Vec<String> = Vec::new();
        for holding in holdings {
            if holding.signed_quantity.is_zero() {
                continue;
            }
            let Some(price) = holding
                .price
                .or_else(|| self.marks.get(holding.object).copied())
            else {
                unpriced.push(holding.object.to_string());
                continue;
            };
            let notional = holding
                .signed_quantity
                .checked_mul(price)
                .ok_or_else(|| Error::numeric("a held notional overflows".to_string()))?;
            *per_pair
                .entry((holding.venue.to_string(), holding.object.to_string()))
                .or_insert(Decimal::ZERO) += notional;
        }

        let mut per_object: BTreeMap<String, Decimal> = BTreeMap::new();
        let mut per_venue: BTreeMap<String, Decimal> = BTreeMap::new();
        for ((venue_key, object), notional) in &per_pair {
            *per_object.entry(object.clone()).or_insert(Decimal::ZERO) += *notional;
            *per_venue.entry(venue_key.clone()).or_insert(Decimal::ZERO) += notional.abs();
        }
        let gross: Decimal = per_object.values().map(|n| n.abs()).sum();
        let net: Decimal = per_object.values().copied().sum();

        // A buy takes the ask at this seam (`Cell::send`'s convention).
        let signed = match side {
            BookSide::Ask => quantity,
            BookSide::Bid => -quantity,
        };
        let order_notional = signed
            .checked_mul(price)
            .ok_or_else(|| Error::numeric("the order's notional overflows".to_string()))?;

        // The checker projects positions and axes but not the venue map, so
        // the venue the order goes to is projected here, exactly: its gross
        // less this pair's size before the order plus its size after. Every
        // other venue is unmoved by the order and keeps its figure. The order's
        // venue therefore always has one, so a venue limit naming it measures
        // the order rather than refusing for want of a key.
        let pair_before = per_pair
            .get(&(venue.as_str().to_string(), object_id.as_str().to_string()))
            .copied()
            .unwrap_or(Decimal::ZERO);
        let pair_after = pair_before
            .checked_add(order_notional)
            .ok_or_else(|| Error::numeric("the projected venue figure overflows".to_string()))?;
        let mut venue_exposures = per_venue.clone();
        let venue_before = venue_exposures
            .get(venue.as_str())
            .copied()
            .unwrap_or(Decimal::ZERO);
        venue_exposures.insert(
            venue.as_str().to_string(),
            venue_before - pair_before.abs() + pair_after.abs(),
        );

        let mut state = RiskState {
            equity: self.capital,
            gross_exposure: gross,
            net_exposure: net,
            position_notionals: per_object,
            axis_exposures: BTreeMap::from([(COUNTERPARTY_AXIS.to_string(), per_venue)]),
            venue_exposures,
            survival_gross_ceiling: Some(granted),
            ..RiskState::default()
        };
        if !unpriced.is_empty() {
            unpriced.sort();
            unpriced.dedup();
            state = state.with_unevaluated(
                FIGURE_CELL_EXPOSURE,
                format!(
                    "{} held instrument(s) have no price this gate has seen, so the cell's \
                     exposure cannot be measured",
                    unpriced.len()
                ),
            );
        }

        let order = ProposedOrder {
            object_id: object_id.clone(),
            quantity: signed,
            reference_price: price,
            axes: BTreeMap::from([(COUNTERPARTY_AXIS.to_string(), venue.as_str().to_string())]),
            scope: cell.to_string(),
        };
        let result = self.checker.check(&order, &state, now)?;
        Ok(result)
    }
}
