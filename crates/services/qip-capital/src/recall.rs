//! Withdrawing capital from a cell that is already trading on it.
//!
//! A recall is the central plane changing its mind mid-flight. It is a
//! request, not a guarantee: the cell may be busy, partitioned, or gone. That
//! is the difference between this and a limit change in a single process, and
//! pretending otherwise is how a distributed trading system ends up believing
//! it has stopped something it has not.
//!
//! So the model has two layers, and only one of them is reliable:
//!
//! * **The recall** reaches the cell, the cell acknowledges, and its exposure
//!   winds down promptly. This is the fast path and it is the one that usually
//!   happens.
//! * **The expiry** is what happens when the recall does not arrive. The
//!   envelope stops admitting orders at
//!   [`CapitalEnvelope::expires_at`] whether or not anyone told the cell
//!   anything, because the check is local to the cell and depends on nothing
//!   but its clock. An unreachable cell's worst case is therefore bounded by
//!   the grant it already held and the time left on it — which is precisely
//!   why [`crate::envelope::MAXIMUM_ENVELOPE_VALIDITY`] is hours rather than
//!   weeks.
//!
//! [`RecallRegister::outstanding_exposure`] reports the first quantity anyone
//! asks for during an incident: how much unacknowledged risk is still out
//! there, and when it stops on its own.

use qip_contracts::signal::StrategyId;
use qip_contracts::CapitalEnvelope;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Why capital is being pulled back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecallReason {
    /// The strategy was demoted out of a capital-holding stage.
    StrategyDemoted,
    /// The book breached a limit and the budget is being reduced everywhere.
    RiskReduction,
    /// The cell itself is being drained, for an upgrade or an incident.
    CellDrain,
    /// A better use was found for the capital.
    Reallocation,
    /// Something is wrong and nobody is sure what yet.
    Precautionary,
}

impl RecallReason {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::StrategyDemoted => "strategy_demoted",
            Self::RiskReduction => "risk_reduction",
            Self::CellDrain => "cell_drain",
            Self::Reallocation => "reallocation",
            Self::Precautionary => "precautionary",
        }
    }

    /// Whether the cell should flatten rather than merely stop opening.
    ///
    /// A reallocation can wait for positions to roll off naturally; a risk
    /// reduction cannot, and a precautionary recall is treated as urgent
    /// because "we do not know" is a reason to be smaller, not a reason to
    /// wait.
    pub const fn requires_immediate_flatten(&self) -> bool {
        matches!(self, Self::RiskReduction | Self::Precautionary)
    }
}

/// The central plane asking a cell to give capital back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecallOrder {
    pub strategy: StrategyId,
    pub cell: String,
    pub reason: RecallReason,
    pub detail: String,
    pub issued_at: Timestamp,
    /// When an acknowledgement is expected by.
    pub acknowledge_by: Timestamp,
    /// The grant being withdrawn, identified by its signature.
    pub envelope_signature: String,
    /// Gross the cell was permitted before the recall.
    pub gross_recalled: Decimal,
    /// When the grant stops admitting orders regardless of this recall.
    ///
    /// The backstop. If the cell never hears this order, its exposure is still
    /// bounded by the envelope it holds and stops at this instant.
    pub backstop_expiry: Timestamp,
}

impl RecallOrder {
    /// How long the cell can still trade if it never receives the recall.
    pub fn unbounded_window(&self, now: Timestamp) -> qip_core::Duration {
        if now >= self.backstop_expiry {
            return qip_core::Duration::ZERO;
        }
        self.backstop_expiry.since(now)
    }

    /// A line for the incident channel.
    pub fn describe(&self) -> String {
        format!(
            "recalling {} from {} at {} ({}): {}; the grant expires at {} regardless",
            self.gross_recalled,
            self.strategy,
            self.cell,
            self.reason.as_str(),
            self.detail,
            self.backstop_expiry.to_rfc3339()
        )
    }
}

/// Where a recall has got to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RecallState {
    /// Sent, no answer yet, and the acknowledgement deadline has not passed.
    Issued,
    /// The cell confirmed, and reported what it still holds.
    Acknowledged {
        at: Timestamp,
        /// Gross the cell reports still open after acknowledging.
        residual_gross: Decimal,
    },
    /// The deadline passed with no answer. The envelope's expiry is now the
    /// only thing bounding this cell's risk.
    Unreachable { since: Timestamp },
}

impl RecallState {
    pub const fn is_settled(&self) -> bool {
        matches!(self, Self::Acknowledged { .. })
    }
}

/// Every recall in flight, and what it means for exposure.
#[derive(Clone, Debug, Default)]
pub struct RecallRegister {
    entries: BTreeMap<(String, String), (RecallOrder, RecallState)>,
}

impl RecallRegister {
    pub fn new() -> Self {
        Self::default()
    }

    /// Issue a recall against a live envelope.
    ///
    /// The backstop expiry is taken from the envelope rather than supplied,
    /// because the whole value of the backstop is that it is the number the
    /// cell will actually enforce, not one the central plane would like it to.
    pub fn issue(
        &mut self,
        envelope: &CapitalEnvelope,
        reason: RecallReason,
        detail: impl Into<String>,
        acknowledge_within: qip_core::Duration,
        now: Timestamp,
    ) -> Result<RecallOrder> {
        if acknowledge_within <= qip_core::Duration::ZERO {
            return Err(Error::invalid(
                "a recall needs a positive window to be acknowledged in",
            ));
        }
        let order = RecallOrder {
            strategy: envelope.strategy().clone(),
            cell: envelope.cell().to_string(),
            reason,
            detail: detail.into(),
            issued_at: now,
            acknowledge_by: now.saturating_add(acknowledge_within),
            envelope_signature: envelope.signature().to_string(),
            gross_recalled: envelope.gross_limit(),
            backstop_expiry: envelope.expires_at(),
        };
        self.entries.insert(
            (order.cell.clone(), order.strategy.as_str().to_string()),
            (order.clone(), RecallState::Issued),
        );
        Ok(order)
    }

    /// Record a cell's acknowledgement.
    pub fn acknowledge(
        &mut self,
        cell: &str,
        strategy: &StrategyId,
        residual_gross: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        let key = (cell.to_string(), strategy.as_str().to_string());
        let entry = self.entries.get_mut(&key).ok_or_else(|| {
            Error::not_found(format!("no recall outstanding for {strategy} at {cell}"))
        })?;
        entry.1 = RecallState::Acknowledged { at, residual_gross };
        Ok(())
    }

    /// Mark everything past its deadline unreachable.
    ///
    /// Called on a timer. A recall that is merely late and one whose cell is
    /// gone look identical from here, and both are treated as the worse case,
    /// because assuming a slow cell will answer is how an incident gets
    /// managed on a picture of the book that is not true.
    pub fn expire_unacknowledged(&mut self, now: Timestamp) -> Vec<&RecallOrder> {
        for (order, state) in self.entries.values_mut() {
            if matches!(state, RecallState::Issued) && now > order.acknowledge_by {
                *state = RecallState::Unreachable {
                    since: order.acknowledge_by,
                };
            }
        }
        self.entries
            .values()
            .filter(|(_, state)| matches!(state, RecallState::Unreachable { .. }))
            .map(|(order, _)| order)
            .collect()
    }

    pub fn state(&self, cell: &str, strategy: &StrategyId) -> Option<&RecallState> {
        self.entries
            .get(&(cell.to_string(), strategy.as_str().to_string()))
            .map(|(_, state)| state)
    }

    pub fn orders(&self) -> impl Iterator<Item = &RecallOrder> {
        self.entries.values().map(|(order, _)| order)
    }

    /// Gross still at risk under recalls nobody has confirmed.
    ///
    /// Counts the full grant for an unacknowledged cell — not an estimate of
    /// what it is probably holding — because an unreachable cell is exactly
    /// the case where the central plane's estimate is worthless. Once an
    /// envelope's expiry has passed the grant admits nothing, so it
    /// contributes zero however silent the cell is: the exposure is bounded by
    /// the clock without anyone having to be reachable.
    pub fn outstanding_exposure(&self, now: Timestamp) -> Decimal {
        self.entries
            .values()
            .filter(|(order, state)| !state.is_settled() && now < order.backstop_expiry)
            .map(|(order, _)| order.gross_recalled)
            .sum()
    }

    /// The last instant at which any unsettled recall's grant is still live.
    ///
    /// The answer to "when is this incident definitely over, even if we never
    /// hear from anyone".
    pub fn bounded_until(&self) -> Option<Timestamp> {
        self.entries
            .values()
            .filter(|(_, state)| !state.is_settled())
            .map(|(order, _)| order.backstop_expiry)
            .max()
    }

    /// Recalls that are neither acknowledged nor yet expired.
    pub fn outstanding(&self, now: Timestamp) -> Vec<&RecallOrder> {
        self.entries
            .values()
            .filter(|(order, state)| !state.is_settled() && now < order.backstop_expiry)
            .map(|(order, _)| order)
            .collect()
    }
}
