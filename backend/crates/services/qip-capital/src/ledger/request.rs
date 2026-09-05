//! A user asking for capital to be put to work, and the answer, as records.
//!
//! The failure this file prevents is the one
//! `.claude/rules/domains/risk-and-execution.md` opens with: a limit checked
//! after an order object exists is a limit that has already been passed
//! once. An [`InvestmentRequest`] is admitted or refused against the user's
//! mandate here, before anything downstream is asked to size, reserve or
//! place, and the refusal names the limit that refused it as a
//! [`RefusedLimit`] variant rather than a sentence — so "why can't I" is
//! answered by the record, and a test can assert *which* limit fired rather
//! than that something did.
//!
//! An [`InvestmentDecision`] is evidence of what was decided. It serialises
//! so it can be journalled and deliberately does not deserialise: a stored
//! decision is never an input that decides. Nothing here moves capital —
//! an admitted request has not funded anything, and funding is checked
//! against the mandate again at [`UserLedger::fund`] because the books may
//! have moved between the decision and the act.

use super::book::UserLedger;
use super::entitlement::{Capability, Entitlement, ProductEligibility, Role};
use super::identity::UserId;
use qip_contracts::signal::StrategyId;
use qip_core::{Currency, Decimal, Timestamp};
use serde::{Deserialize, Serialize};

/// What a user asked for: this much, at this strategy, under this family.
///
/// The family is stated by the caller because nothing in the tree yet maps
/// a `StrategyId` to its family at this seam; the request carries the claim
/// so the mandate's family gate has something to refuse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InvestmentRequest {
    pub user: UserId,
    pub strategy: StrategyId,
    pub family: String,
    pub currency: Currency,
    pub amount: Decimal,
    pub requested_at: Timestamp,
}

/// The limit that refused a request. One variant per gate, so a refusal is
/// a value a reviewer can group on and a test can name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum RefusedLimit {
    /// The user holds no mandate at all.
    NoMandate,
    /// The entitlement refused: role, jurisdiction, family or no investable
    /// capital. The reason carries which.
    Entitlement,
    /// The request is not in the mandate's currency.
    Currency,
    /// The amount is not positive.
    Amount,
    /// The mandate's capital less its liquidity floor, less what the user
    /// already has at work across every strategy.
    InvestableCapital,
    /// The share of capital the mandate tolerates losing, applied to what
    /// would be at one strategy: a strategy can lose all of what it holds.
    RiskTolerance,
}

/// How a request was answered.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum InvestmentOutcome {
    Admitted { basis: String },
    Refused { limit: RefusedLimit, reason: String },
}

/// A request and the ledger's answer to it, at the instant it was decided.
///
/// Fields are private and there is no `Deserialize`: the only way to hold
/// one is to have asked the ledger.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InvestmentDecision {
    request: InvestmentRequest,
    decided_at: Timestamp,
    outcome: InvestmentOutcome,
}

impl InvestmentDecision {
    pub fn request(&self) -> &InvestmentRequest {
        &self.request
    }

    pub fn decided_at(&self) -> Timestamp {
        self.decided_at
    }

    pub fn outcome(&self) -> &InvestmentOutcome {
        &self.outcome
    }

    pub fn is_admitted(&self) -> bool {
        matches!(self.outcome, InvestmentOutcome::Admitted { .. })
    }

    /// The limit that refused the request, or `None` for an admitted one.
    pub fn refused_by(&self) -> Option<RefusedLimit> {
        match &self.outcome {
            InvestmentOutcome::Admitted { .. } => None,
            InvestmentOutcome::Refused { limit, .. } => Some(*limit),
        }
    }
}

impl UserLedger {
    /// Decide whether a request is inside the user's mandate, without
    /// changing anything.
    ///
    /// The gates run in a fixed order and the first to refuse is the
    /// answer: mandate held, entitlement (role, product eligibility in the
    /// user's jurisdiction, family permitted, investable capital positive),
    /// currency, amount, investable capital net of what is already at work,
    /// and the risk tolerance applied to what would be at the one strategy.
    /// Two requests that differ only in when they were asked get the same
    /// answer from the same books, so a decision can be replayed from the
    /// record it came from.
    pub fn admit(
        &self,
        request: &InvestmentRequest,
        role: Role,
        product: &ProductEligibility,
        now: Timestamp,
    ) -> InvestmentDecision {
        let outcome = self.decide(request, role, product, now);
        InvestmentDecision {
            request: request.clone(),
            decided_at: now,
            outcome,
        }
    }

    fn decide(
        &self,
        request: &InvestmentRequest,
        role: Role,
        product: &ProductEligibility,
        now: Timestamp,
    ) -> InvestmentOutcome {
        let user = &request.user;
        let Some(mandate) = self.mandate(user) else {
            return InvestmentOutcome::Refused {
                limit: RefusedLimit::NoMandate,
                reason: format!("{user} holds no mandate; register one before requesting"),
            };
        };
        if product.family != request.family {
            return InvestmentOutcome::Refused {
                limit: RefusedLimit::Entitlement,
                reason: format!(
                    "the request names the family {} and the product offered is {}; a request \
                     is evaluated against the product it asks for",
                    request.family, product.family
                ),
            };
        }
        let entitlement = Entitlement::evaluate(user, mandate, role, product, now);
        let entitled_on = match entitlement.can_invest() {
            Capability::Granted { basis } => basis.clone(),
            Capability::Refused { reason } => {
                return InvestmentOutcome::Refused {
                    limit: RefusedLimit::Entitlement,
                    reason: reason.clone(),
                };
            }
        };
        if request.currency != mandate.currency() {
            return InvestmentOutcome::Refused {
                limit: RefusedLimit::Currency,
                reason: format!(
                    "the request is in {} and the mandate of {user} is in {}",
                    request.currency,
                    mandate.currency()
                ),
            };
        }
        if !request.amount.is_positive() {
            return InvestmentOutcome::Refused {
                limit: RefusedLimit::Amount,
                reason: format!(
                    "a request for {} is refused; request a positive amount",
                    request.amount
                ),
            };
        }
        let currency = mandate.currency();
        let investable = mandate.investable();
        let at_work = self.funded_total(user, currency);
        let headroom = investable - at_work;
        if request.amount > headroom {
            return InvestmentOutcome::Refused {
                limit: RefusedLimit::InvestableCapital,
                reason: format!(
                    "{} {currency} exceeds the {headroom} investable for {user}: {investable} \
                     under the mandate ({} capital less a {} liquidity floor) with {at_work} \
                     already at work",
                    request.amount,
                    mandate.capital(),
                    mandate.liquidity_floor()
                ),
            };
        }
        let tolerated = mandate.capital() * mandate.risk_tolerance();
        let at_strategy = self.funded_at(user, &request.strategy, currency);
        let would_hold = at_strategy + request.amount;
        if would_hold > tolerated {
            return InvestmentOutcome::Refused {
                limit: RefusedLimit::RiskTolerance,
                reason: format!(
                    "{} {currency} at {} would put {would_hold} at one strategy against the \
                     {tolerated} the mandate of {user} tolerates losing ({} of {} capital); a \
                     strategy can lose all of what it holds",
                    request.amount,
                    request.strategy,
                    mandate.risk_tolerance(),
                    mandate.capital()
                ),
            };
        }
        InvestmentOutcome::Admitted {
            basis: format!(
                "{} {currency} at {} for {user}: {headroom} investable, {tolerated} tolerated \
                 at one strategy with {at_strategy} already there; {entitled_on}",
                request.amount, request.strategy
            ),
        }
    }
}
