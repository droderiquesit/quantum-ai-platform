//! A position in one instrument.

use qip_core::{Decimal, ObjectId, Timestamp};
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

use crate::lifecycle::PositionLifecycle;
use crate::lot::{HoldingTerm, Lot, LotSelection, RealisedTrade, close_lots_under};
use qip_financial::constraints::Jurisdiction;

/// Which way a position points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionSide {
    Long,
    Short,
    Flat,
}

impl PositionSide {
    pub fn of(quantity: Decimal) -> Self {
        match quantity.signum() {
            1 => Self::Long,
            -1 => Self::Short,
            _ => Self::Flat,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
            Self::Flat => "flat",
        }
    }
}

/// A holding, tracked by lot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub object_id: ObjectId,
    pub symbol: String,
    /// Open lots. Empty when flat.
    pub lots: Vec<Lot>,
    /// Cumulative realised profit, net of costs.
    pub realised_pnl: Decimal,
    /// Cumulative transaction costs paid.
    pub total_costs: Decimal,
    /// Multiplier from one unit to the underlying notional.
    pub contract_multiplier: Decimal,
    /// Closed round trips, for attribution.
    pub closed_trades: Vec<RealisedTrade>,
    /// Which lot a closing fill consumes, and whether that choice consults a
    /// holding-period rule.
    ///
    /// Private, like [`Self::lifecycle`] and for the same reason. The only
    /// writer is [`Position::declare_selection`], which refuses a
    /// holding-period arm whose jurisdiction is not this position's — so a
    /// term-aware selection cannot be attached to a position it could never
    /// classify, and then quietly behave as first-in-first-out for the rest
    /// of the position's life.
    selection: LotSelection,
    /// The single regulatory jurisdiction of the instrument this position is
    /// in, where it has exactly one.
    ///
    /// Copied from `FinancialObject::regulatory` by
    /// [`crate::portfolio::Portfolio::apply_fill`] when the instrument names
    /// exactly one jurisdiction, and left `None` when it names none or
    /// several. Several is not narrowed to the first: which of them governs
    /// a lot's holding period is a question the instrument record does not
    /// answer, and picking one would be this platform inventing a tax
    /// position. `None` is carried forward honestly as
    /// [`HoldingTerm::Undetermined`] on every trade the position realises.
    ///
    /// It sits on the position rather than on each [`Lot`] because it is a
    /// fact about the instrument, one per position: a copy on every lot would
    /// be the same fact stored many times, free to drift apart.
    jurisdiction: Option<Jurisdiction>,
    pub opened_at: Option<Timestamp>,
    pub updated_at: Timestamp,
    /// What the desk is doing about the position, independent of the lot
    /// ledger above.
    ///
    /// Private, and that is the guarantee rather than a comment about one.
    /// The doc here used to say a caller outside this module "cannot walk the
    /// field into one of those states without going through a move the table
    /// actually permits" while the field was `pub`, which made the sentence
    /// false for every crate in the workspace: `position.lifecycle =
    /// PositionLifecycle::Held` on a closed record compiled. The only writer
    /// is now [`Position::move_lifecycle`], which goes through
    /// [`PositionLifecycle::transition`], so the terminal `Closed` arm is held
    /// by the type system and not by everybody remembering.
    lifecycle: PositionLifecycle,
}

impl Position {
    pub fn new(object_id: ObjectId, symbol: impl Into<String>, at: Timestamp) -> Self {
        Self {
            object_id,
            symbol: symbol.into(),
            lots: Vec::new(),
            realised_pnl: Decimal::ZERO,
            total_costs: Decimal::ZERO,
            contract_multiplier: Decimal::ONE,
            closed_trades: Vec::new(),
            selection: LotSelection::default(),
            jurisdiction: None,
            opened_at: None,
            updated_at: at,
            lifecycle: PositionLifecycle::Opened,
        }
    }

    /// Where the position sits in its life.
    pub fn lifecycle(&self) -> PositionLifecycle {
        self.lifecycle
    }

    /// Move the lifecycle field to `next`, refusing an illegal move.
    ///
    /// This is the only path by which `Flagged`, `Unwinding` or `Orphaned`
    /// reach the field: `apply_fill` advances `Opened` -> `Held` -> `Closed`
    /// on its own, through this same refusing transition, but never assigns
    /// any of the other three. The seams that raise the other three are
    /// [`crate::portfolio::Portfolio::flag_position`] and
    /// [`crate::portfolio::Portfolio::begin_unwind`], which call this and
    /// carry its refusal to their own caller.
    pub fn move_lifecycle(&mut self, next: PositionLifecycle) -> qip_core::Result<()> {
        self.lifecycle = self.lifecycle.transition(next)?;
        Ok(())
    }

    pub fn with_multiplier(mut self, multiplier: Decimal) -> Self {
        self.contract_multiplier = multiplier;
        self
    }

    /// State the one jurisdiction the instrument sits in.
    ///
    /// `None` is a real answer and the one a caller should pass when the
    /// instrument names no jurisdiction or more than one. See the field's
    /// documentation for why several is not narrowed to one.
    pub fn with_jurisdiction(mut self, jurisdiction: Option<Jurisdiction>) -> Self {
        self.jurisdiction = jurisdiction;
        self
    }

    /// The jurisdiction a holding-period rule for this position must name.
    pub fn jurisdiction(&self) -> Option<Jurisdiction> {
        self.jurisdiction
    }

    /// Which lot the next closing fill will consume.
    pub fn selection(&self) -> LotSelection {
        self.selection
    }

    /// Choose how closing fills pick lots, refusing a holding-period rule
    /// this position could not apply.
    ///
    /// The refusal is the point. A [`crate::lot::HoldingPeriodTest`] declared
    /// for a jurisdiction other than this position's classifies every one of
    /// its lots as [`HoldingTerm::Undetermined`], which leaves the ordering
    /// indistinguishable from first-in-first-out — a tax policy that appears
    /// to be in force and is not. Attaching it is refused here, where the
    /// mismatch is knowable, rather than discovered later from a realised
    /// gain that came out short-term when the desk expected long.
    pub fn declare_selection(&mut self, selection: LotSelection) -> qip_core::Result<()> {
        if let Some(test) = selection.holding_period_test() {
            match self.jurisdiction {
                Some(held) if held == test.jurisdiction() => {}
                Some(held) => {
                    return Err(qip_core::error::Error::invalid(format!(
                        "{} sits in {} but the holding-period rule offered for it was declared \
                         for {}; declare the rule for {} or close this position under a \
                         mechanical lot method, because a rule for another jurisdiction would \
                         order the lots exactly as first-in-first-out while reading as a tax \
                         policy",
                        self.symbol,
                        held.as_str(),
                        test.jurisdiction().as_str(),
                        held.as_str(),
                    )));
                }
                None => {
                    return Err(qip_core::error::Error::invalid(format!(
                        "{} names no single regulatory jurisdiction, so the holding-period rule \
                         declared for {} cannot be applied to it; register the instrument with \
                         exactly one jurisdiction, or close this position under a mechanical lot \
                         method",
                        self.symbol,
                        test.jurisdiction().as_str(),
                    )));
                }
            }
        }
        self.selection = selection;
        Ok(())
    }

    /// Realised profit split by the term it was realised at.
    ///
    /// Every state appears, including the ones with nothing in them, so a
    /// report cannot read an absent key as a zero it measured. The
    /// [`HoldingTerm::Undetermined`] share is first, and a desk that finds
    /// its whole book there is being told no holding-period rule reached it.
    pub fn realised_by_term(&self) -> std::collections::BTreeMap<HoldingTerm, Decimal> {
        let mut split: std::collections::BTreeMap<HoldingTerm, Decimal> = HoldingTerm::ALL
            .iter()
            .map(|term| (*term, Decimal::ZERO))
            .collect();
        for trade in &self.closed_trades {
            let entry = split.entry(trade.term).or_insert(Decimal::ZERO);
            *entry += trade.realised_pnl() * self.contract_multiplier;
        }
        split
    }

    /// Net signed quantity across all lots.
    pub fn quantity(&self) -> Decimal {
        self.lots.iter().map(|l| l.quantity).sum()
    }

    pub fn side(&self) -> PositionSide {
        PositionSide::of(self.quantity())
    }

    pub fn is_flat(&self) -> bool {
        self.quantity().is_zero()
    }

    /// Total outlay across open lots, including costs.
    pub fn cost_basis(&self) -> Decimal {
        self.lots.iter().map(Lot::cost_basis).sum()
    }

    /// Weighted average entry price of the open lots, excluding costs.
    pub fn average_price(&self) -> Decimal {
        let quantity = self.quantity();
        if quantity.is_zero() {
            return Decimal::ZERO;
        }
        let notional: Decimal = self.lots.iter().map(|l| l.quantity * l.price).sum();
        notional.checked_div(quantity).unwrap_or(Decimal::ZERO)
    }

    /// Market value at `price`, signed by direction.
    pub fn market_value(&self, price: Decimal) -> Decimal {
        self.quantity() * price * self.contract_multiplier
    }

    /// Gross notional exposure, unsigned.
    pub fn notional_exposure(&self, price: Decimal) -> Decimal {
        self.market_value(price).abs()
    }

    /// Profit not yet realised, at `price`, net of the costs still attributed
    /// to the open lots.
    ///
    /// Netting the costs is what makes realised plus unrealised equal the
    /// change in equity. Measuring unrealised gross while realised is net
    /// leaves the difference — the entry costs of positions still open —
    /// unaccounted for anywhere, and the books stop reconciling.
    pub fn unrealised_pnl(&self, price: Decimal) -> Decimal {
        let gross: Decimal = self
            .lots
            .iter()
            .map(|lot| (price - lot.price) * lot.quantity * self.contract_multiplier)
            .sum();
        let open_costs: Decimal = self.lots.iter().map(|lot| lot.costs).sum();
        gross - open_costs
    }

    /// Realised plus unrealised.
    pub fn total_pnl(&self, price: Decimal) -> Decimal {
        self.realised_pnl + self.unrealised_pnl(price)
    }

    /// Apply a fill.
    ///
    /// A fill in the same direction opens a lot; one in the opposite direction
    /// closes lots and realises profit, and if it exceeds the position it flips
    /// it, opening a new lot for the excess.
    ///
    /// Returns the cash flow: negative when buying, positive when selling.
    pub fn apply_fill(
        &mut self,
        quantity: Decimal,
        price: Decimal,
        costs: Decimal,
        at: Timestamp,
        order_id: Option<String>,
    ) -> Decimal {
        // `total_costs` and `updated_at` are recorded before the early return
        // below: a zero-quantity, cost-only fill (a standalone fee, or a
        // requested quantity so small it rounds to zero at the decimal's
        // fixed scale) still moves cash by `-costs`, and a position whose
        // `total_costs` did not move to match would silently understate what
        // was actually paid against it.
        self.updated_at = at;
        self.total_costs += costs;
        if quantity.is_zero() {
            return -costs;
        }
        if self.opened_at.is_none() {
            self.opened_at = Some(at);
            // A fully closed position also has `opened_at == None` (closing
            // resets it below), so a confirmed lot arriving on a flat, closed
            // record is a *new round trip on the same instrument* — the
            // evolution engine and the simulated exchange re-enter a name
            // through the record they already hold. That is not the
            // walk-back the table refuses: the closed trades stay in
            // `closed_trades`, and the record starts its lifecycle again from
            // `Opened`, so the only edge taken is the table's own
            // `Opened -> Held`. An earlier version left the field at `Closed`
            // here and let the refusal stand; the position then held lots
            // while reading as closed, and the close that followed tried
            // `Closed -> Closed` and tripped the terminal guard in every
            // suite that traded a name twice.
            if self.lifecycle == PositionLifecycle::Closed {
                self.lifecycle = PositionLifecycle::Opened;
            }
            // The first confirmed lot of a round trip moves Opened -> Held.
            if let Err(refusal) = self.move_lifecycle(PositionLifecycle::Held) {
                debug_assert!(false, "unexpected refusal moving to held: {refusal:?}");
            }
        }

        let current = self.quantity();
        let cash_flow = -(quantity * price * self.contract_multiplier) - costs;

        // Same direction, or opening from flat: a new lot.
        if current.is_zero() || current.signum() == quantity.signum() {
            let mut lot = Lot::new(quantity, price, at).with_costs(costs);
            lot.order_id = order_id;
            self.lots.push(lot);
            return cash_flow;
        }

        // Opposite direction: close against existing lots.
        let closing = quantity.abs().min(current.abs());
        let trades = close_lots_under(
            &mut self.lots,
            closing,
            price,
            costs
                .checked_mul(closing)
                .and_then(|scaled| scaled.checked_div(quantity.abs()))
                .unwrap_or(Decimal::ZERO),
            at,
            self.selection,
            self.jurisdiction,
        );
        for trade in &trades {
            // Realised profit is measured in instrument units and scaled to
            // currency by the contract multiplier.
            self.realised_pnl += trade.realised_pnl() * self.contract_multiplier;
        }
        self.closed_trades.extend(trades);

        // Any excess flips the position.
        let excess = quantity.abs() - closing;
        if excess.is_positive() {
            let sign = if quantity.is_positive() {
                Decimal::ONE
            } else {
                Decimal::NEG_ONE
            };
            let remaining_costs = costs
                .checked_mul(excess)
                .and_then(|scaled| scaled.checked_div(quantity.abs()))
                .unwrap_or(Decimal::ZERO);
            let mut lot = Lot::new(excess * sign, price, at).with_costs(remaining_costs);
            lot.order_id = order_id;
            self.lots.push(lot);
        }

        if self.is_flat() {
            self.opened_at = None;
            // The last lot closed. Closed is reachable from every other
            // state on the table, so this only fails if the position was
            // already Closed — which cannot happen here, because a re-entry
            // restarts the lifecycle above before any lot is pushed.
            if let Err(refusal) = self.move_lifecycle(PositionLifecycle::Closed) {
                debug_assert!(false, "unexpected refusal moving to closed: {refusal:?}");
            }
        }
        cash_flow
    }

    /// Apply a corporate action's quantity and price adjustments.
    pub fn apply_adjustment(&mut self, quantity_factor: Decimal, price_factor: Decimal) {
        if quantity_factor == Decimal::ONE && price_factor == Decimal::ONE {
            return;
        }
        for lot in &mut self.lots {
            lot.quantity = lot.quantity * quantity_factor;
            lot.price = lot.price * price_factor;
        }
    }

    /// Number of round trips closed.
    pub fn trade_count(&self) -> usize {
        self.closed_trades.len()
    }

    /// Fraction of closed trades that were profitable.
    pub fn hit_rate(&self) -> f64 {
        if self.closed_trades.is_empty() {
            return 0.0;
        }
        self.closed_trades.iter().filter(|t| t.is_win()).count() as f64
            / self.closed_trades.len() as f64
    }
}

/// Published whenever a position changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionUpdated {
    pub portfolio_id: String,
    pub object_id: ObjectId,
    pub symbol: String,
    pub quantity: Decimal,
    pub average_price: Decimal,
    pub market_value: Decimal,
    pub realised_pnl: Decimal,
    pub unrealised_pnl: Decimal,
    pub at: Timestamp,
}

impl EventBody for PositionUpdated {
    const TOPIC: Topic = Topic::PositionUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}",
            self.portfolio_id,
            self.object_id,
            self.at.as_nanos()
        ))
    }
}
