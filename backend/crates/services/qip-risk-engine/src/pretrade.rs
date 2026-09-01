//! Pre-trade risk checking.
//!
//! Every order passes through [`PreTradeChecker::check`] before it can reach a
//! venue, and the check is deterministic arithmetic against declared limits.
//! No model, no judgement, no natural-language reasoning: a control whose
//! output depends on how a prompt was phrased is not a control.
//!
//! The check runs against the risk state the order *would* produce, not the
//! current one. Checking the state before the trade is the classic mistake — it
//! passes every order right up to the one that breaches, and then passes that
//! one too.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_risk::limits::{LimitCheck, LimitSet, RiskState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An order as the risk engine sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposedOrder {
    pub object_id: ObjectId,
    /// Signed: positive buys, negative sells.
    pub quantity: Decimal,
    pub reference_price: Decimal,
    /// Exposure axes this instrument belongs to, e.g. sector and country.
    pub axes: BTreeMap<String, String>,
    /// Counterparty the order would face.
    pub counterparty: Option<String>,
    /// The scope for kill-switch purposes, usually the strategy.
    pub scope: String,
}

impl ProposedOrder {
    pub fn notional(&self) -> Decimal {
        (self.quantity * self.reference_price).abs()
    }

    pub fn signed_notional(&self) -> Decimal {
        self.quantity * self.reference_price
    }

    pub fn validate(&self) -> Result<()> {
        if self.quantity.is_zero() {
            return Err(Error::invalid("an order for zero quantity is not an order"));
        }
        if self.reference_price <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "order for {} has no usable reference price",
                self.object_id.as_str()
            )));
        }
        if self.scope.trim().is_empty() {
            return Err(Error::invalid(format!(
                "order for {} names no scope, so a scoped kill switch could not stop it",
                self.object_id.as_str()
            )));
        }
        Ok(())
    }
}

/// What the pre-trade check decided.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PreTradeDecision {
    /// The order may proceed.
    Approved,
    /// The order may proceed at a reduced size.
    Reduced {
        /// The largest quantity that would pass.
        permitted_quantity: Decimal,
        limiting_constraint: String,
    },
    /// The order is refused.
    Rejected { reasons: Vec<String> },
}

impl PreTradeDecision {
    pub fn is_approved(&self) -> bool {
        matches!(self, Self::Approved)
    }

    pub fn permits_anything(&self) -> bool {
        !matches!(self, Self::Rejected { .. })
    }

    /// The quantity that may actually be sent.
    pub fn permitted_quantity(&self, requested: Decimal) -> Decimal {
        match self {
            Self::Approved => requested,
            Self::Reduced {
                permitted_quantity, ..
            } => *permitted_quantity,
            Self::Rejected { .. } => Decimal::ZERO,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Approved => "approved".to_string(),
            Self::Reduced {
                permitted_quantity,
                limiting_constraint,
            } => format!("reduced to {permitted_quantity} by {limiting_constraint}"),
            Self::Rejected { reasons } => format!("rejected: {}", reasons.join("; ")),
        }
    }
}

/// The full record of one pre-trade check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PreTradeResult {
    pub at: Timestamp,
    pub order: ProposedOrder,
    pub decision: PreTradeDecision,
    /// The limit evaluation against the post-trade state.
    pub post_trade_check: LimitCheck,
    /// Checks that ran and passed, so a clean result is evidence rather than
    /// an absence of evidence.
    pub checks_run: Vec<String>,
}

impl PreTradeResult {
    pub fn is_approved(&self) -> bool {
        self.decision.is_approved()
    }
}

/// Runs pre-trade checks.
#[derive(Debug)]
pub struct PreTradeChecker {
    limits: LimitSet,
    /// Whether a breaching order may be reduced to a permissible size rather
    /// than refused outright.
    ///
    /// Off by default. Silently resizing an order means the executed trade is
    /// not the one that was reviewed, and the difference is invisible unless
    /// someone reads the fills.
    allow_reduction: bool,
}

impl PreTradeChecker {
    pub fn new(limits: LimitSet) -> Self {
        Self {
            limits,
            allow_reduction: false,
        }
    }

    /// Permit reduction instead of outright refusal.
    pub fn allowing_reduction(mut self) -> Self {
        self.allow_reduction = true;
        self
    }

    pub fn limits(&self) -> &LimitSet {
        &self.limits
    }

    /// Check an order against the state it would produce.
    pub fn check(
        &self,
        order: &ProposedOrder,
        current: &RiskState,
        at: Timestamp,
    ) -> Result<PreTradeResult> {
        order.validate()?;

        let post_trade = self.project(order, current);
        let check = self.limits.check(&post_trade);
        let mut checks_run: Vec<String> = self
            .limits
            .limits
            .iter()
            .map(|limit| limit.name.clone())
            .collect();
        checks_run.sort();

        let decision = if check.is_blocked() {
            let reasons: Vec<String> = check
                .blocking()
                .iter()
                .map(|breach| {
                    format!(
                        "{}: {} (observed {:.6} against a limit of {:.6})",
                        breach.limit_name, breach.detail, breach.observed, breach.bound
                    )
                })
                .collect();
            if self.allow_reduction {
                match self.largest_permissible(order, current) {
                    Some((quantity, constraint)) if !quantity.is_zero() => {
                        PreTradeDecision::Reduced {
                            permitted_quantity: quantity,
                            limiting_constraint: constraint,
                        }
                    }
                    _ => PreTradeDecision::Rejected { reasons },
                }
            } else {
                PreTradeDecision::Rejected { reasons }
            }
        } else {
            PreTradeDecision::Approved
        };

        Ok(PreTradeResult {
            at,
            order: order.clone(),
            decision,
            post_trade_check: check,
            checks_run,
        })
    }

    /// The risk state the order would produce.
    ///
    /// Only the fields an order actually changes are projected. Volatility,
    /// VaR and drawdown are left as they stand: recomputing them properly
    /// needs the covariance and the return history, and a crude approximation
    /// here would be a limit check against a number nobody could reproduce.
    pub fn project(&self, order: &ProposedOrder, current: &RiskState) -> RiskState {
        let mut projected = current.clone();
        let key = order.object_id.as_str().to_string();
        let signed = order.signed_notional();

        let existing = projected
            .position_notionals
            .get(&key)
            .copied()
            .unwrap_or(Decimal::ZERO);
        let updated = existing + signed;

        // Gross moves by the change in this position's absolute size, which is
        // not the order's notional when the order reduces an existing position.
        projected.gross_exposure = projected.gross_exposure - existing.abs() + updated.abs();
        projected.net_exposure += signed;
        projected.cash -= signed;
        projected.position_notionals.insert(key.clone(), updated);

        for (axis, bucket) in &order.axes {
            let entry = projected
                .axis_exposures
                .entry(axis.clone())
                .or_default()
                .entry(bucket.clone())
                .or_insert(Decimal::ZERO);
            *entry = *entry - existing.abs() + updated.abs();
        }

        if let Some(counterparty) = &order.counterparty {
            let entry = projected
                .counterparty_exposures
                .entry(counterparty.clone())
                .or_insert(Decimal::ZERO);
            *entry += order.notional();
        }

        projected.order_notional = Some(order.notional());
        projected.order_subject = Some(key);
        projected
    }

    /// The largest quantity in the order's direction that would pass.
    ///
    /// Found by bisection on the order size rather than by inverting each
    /// limit: the limits are heterogeneous and some are not analytically
    /// invertible, and a search over a monotone predicate is both simpler and
    /// harder to get subtly wrong.
    ///
    /// The bisection runs entirely in [`Decimal`]. It used to bisect a `f64`
    /// fraction and rebuild the quantity as
    /// `Decimal::from_f64(full.to_f64() * mid)`, which put a `Decimal → f64 →
    /// Decimal` round trip inside the loop that decides how much of a breaching
    /// order survives. Two failures followed from that. The reported permitted
    /// quantity was a binary-floating-point approximation of the true boundary,
    /// so it could land a hair *above* a hard limit — a control that returns a
    /// size the limit would reject is not a control. And the answer was not
    /// reproducible from the event log, because the rounding depended on the
    /// magnitude of the order rather than on the limits. There is no crossing
    /// into `f64` anywhere below; the endpoints are halved on the underlying
    /// scaled integer, which is exact.
    fn largest_permissible(
        &self,
        order: &ProposedOrder,
        current: &RiskState,
    ) -> Option<(Decimal, String)> {
        let passes = |quantity: Decimal| -> (bool, String) {
            if quantity.is_zero() {
                return (true, String::new());
            }
            let mut trial = order.clone();
            trial.quantity = quantity;
            let projected = self.project(&trial, current);
            let check = self.limits.check(&projected);
            let blocking = check.blocking();
            let name = blocking
                .first()
                .map(|breach| breach.limit_name.clone())
                .unwrap_or_default();
            (!check.is_blocked(), name)
        };

        let full = order.quantity;
        let (ok, blocked_by) = passes(full);
        if ok {
            return Some((full, String::new()));
        }

        // Bisect on the quantity itself. `low` is always a quantity that
        // passes — zero passes by construction, which is what makes the
        // invariant hold on the first iteration — and `high` is always one
        // that does not, `full` having just been shown to breach.
        let mut low = Decimal::ZERO;
        let mut high = full;
        // Seeded from the full-size breach so a reduction always names what
        // reduced it. An unnamed constraint reads to an operator as "the
        // system shrank my order and will not say why".
        let mut limiting = blocked_by;
        // One scaled unit is the smallest quantity the type can express, so a
        // gap of one means `low` is the largest representable passing size and
        // there is nothing left to search.
        let smallest_step: i128 = 1;
        // Bounded because a loop whose termination depends on arithmetic the
        // caller supplies is a hang waiting for an unusual order. `i128` at
        // nine decimal places needs at most 127 halvings to close any gap.
        for _ in 0..127 {
            if (high.raw() - low.raw()).abs() <= smallest_step {
                break;
            }
            // Halving the scaled integer is exact for even raws and truncates
            // toward zero otherwise, so the midpoint never overshoots `high`.
            let mid = Decimal::from_raw(low.raw() + (high.raw() - low.raw()) / 2);
            if mid == low || mid == high {
                break;
            }
            let (ok, name) = passes(mid);
            if ok {
                low = mid;
            } else {
                high = mid;
                if !name.is_empty() {
                    limiting = name;
                }
            }
        }
        if low.is_zero() {
            return None;
        }
        Some((low, limiting))
    }
}
