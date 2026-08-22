//! Who resolves the market, how sure they are, and what happens when they are
//! wrong.
//!
//! A resolved market is not a settled one. Between the two sits a dispute
//! window, and the position that looks risk-free — bought at 0.97, resolution
//! announced, waiting for the payout — is precisely the position that loses
//! money when the resolution is challenged and overturned. The state machine
//! here exists so that "resolved" and "settleable" are different questions
//! with different answers, and so that settling on a disputed resolution is
//! impossible rather than merely unwise.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::market::{EventMarket, FeeSchedule, OutcomeId};
use crate::resolution::SettlementRule;

/// What kind of resolver this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OracleKind {
    /// Reads the official publication and reports it.
    Attestor,
    /// A panel that votes.
    Committee,
    /// Anyone may propose; anyone may challenge with a bond.
    Optimistic,
    /// A model. Fast, and wrong in ways a human would not be.
    Model,
}

impl OracleKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Attestor => "attestor",
            Self::Committee => "committee",
            Self::Optimistic => "optimistic",
            Self::Model => "model",
        }
    }

    /// Whether a report from this kind of oracle can be challenged at all.
    pub const fn is_challengeable(&self) -> bool {
        !matches!(self, Self::Attestor)
    }
}

/// The oracle's identity and terms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OracleIdentity {
    pub name: String,
    pub kind: OracleKind,
    /// How long a proposed resolution can be challenged.
    pub dispute_window: Duration,
    /// Confidence below which a report is not accepted. A statistic, and named
    /// so.
    pub minimum_confidence: f64,
}

impl OracleIdentity {
    pub fn new(
        name: impl Into<String>,
        kind: OracleKind,
        dispute_window: Duration,
        minimum_confidence: f64,
    ) -> Result<Self> {
        if !(0.0..=1.0).contains(&minimum_confidence) {
            return Err(Error::invalid(
                "a minimum confidence must lie in [0, 1]",
            ));
        }
        Ok(Self {
            name: name.into(),
            kind,
            dispute_window,
            minimum_confidence,
        })
    }
}

/// What an oracle says happened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OracleReport {
    pub outcome: OutcomeId,
    /// How sure the oracle is. A statistic; it never enters a price directly.
    pub confidence: f64,
    pub reported_at: Timestamp,
    /// What the oracle read to reach this. Carried so a dispute has something
    /// to be about other than opinion.
    pub evidence: String,
}

/// A challenge to a proposed resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dispute {
    pub raised_by: String,
    pub reason: String,
    pub raised_at: Timestamp,
    /// The outcome the challenger says should have won.
    pub competing: Option<OutcomeId>,
}

/// Where a market's resolution has got to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ResolutionState {
    /// Before the resolution date, with nothing reported.
    Pending,
    /// The resolution date has passed and the oracle has not reported. Not an
    /// error state — a delayed oracle is the normal case — but a position held
    /// against it is a position with an unknown horizon.
    Overdue { since: Timestamp },
    /// Reported, and challengeable until the deadline.
    Proposed {
        report: OracleReport,
        dispute_deadline: Timestamp,
    },
    /// Challenged. Nothing settles from here until the challenge is decided.
    Disputed {
        report: OracleReport,
        dispute: Dispute,
    },
    /// Decided and settleable.
    Final { outcome: OutcomeId, at: Timestamp },
    /// Cancelled. Stakes are returned rather than paid out.
    Void { reason: String, at: Timestamp },
}

impl ResolutionState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Overdue { .. } => "overdue",
            Self::Proposed { .. } => "proposed",
            Self::Disputed { .. } => "disputed",
            Self::Final { .. } => "final",
            Self::Void { .. } => "void",
        }
    }

    /// Whether a payout may be computed from this state.
    pub const fn is_settleable(&self) -> bool {
        matches!(self, Self::Final { .. })
    }
}

/// What a settled position is worth.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settlement {
    pub outcome: OutcomeId,
    pub quantity: Decimal,
    /// Before the settlement fee.
    pub gross: Decimal,
    pub fee: Decimal,
    pub net: Decimal,
}

/// The resolution of one market, tracked through its states.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketResolution {
    pub market_id: ObjectId,
    pub oracle: OracleIdentity,
    pub resolves_at: Timestamp,
    pub settlement: SettlementRule,
    state: ResolutionState,
}

impl MarketResolution {
    pub fn new(market: &EventMarket, oracle: OracleIdentity) -> Self {
        Self {
            market_id: market.market_id.clone(),
            oracle,
            resolves_at: market.proposition.resolves_at,
            settlement: market.proposition.settlement,
            state: ResolutionState::Pending,
        }
    }

    pub const fn state(&self) -> &ResolutionState {
        &self.state
    }

    /// Whether the oracle is late as of `now`.
    pub fn is_delayed(&self, now: Timestamp) -> bool {
        now > self.resolves_at
            && matches!(
                self.state,
                ResolutionState::Pending | ResolutionState::Overdue { .. }
            )
    }

    /// Advance the state with whatever the oracle has said by `now`.
    ///
    /// A report the oracle is not confident enough about is refused rather
    /// than recorded: an under-confident resolution that becomes the default
    /// after a quiet dispute window is a resolution nobody agreed to.
    pub fn observe(&mut self, report: Option<OracleReport>, now: Timestamp) -> Result<&ResolutionState> {
        match report {
            None => {
                if self.is_delayed(now) {
                    self.state = ResolutionState::Overdue {
                        since: self.resolves_at,
                    };
                }
                Ok(&self.state)
            }
            Some(report) => {
                if !matches!(
                    self.state,
                    ResolutionState::Pending | ResolutionState::Overdue { .. }
                ) {
                    return Err(Error::denied(format!(
                        "market {} is already {}",
                        self.market_id,
                        self.state.as_str()
                    )));
                }
                if report.confidence < self.oracle.minimum_confidence {
                    return Err(Error::guard(format!(
                        "oracle {} reported at confidence {:.3}, below its own minimum of {:.3}",
                        self.oracle.name, report.confidence, self.oracle.minimum_confidence
                    )));
                }
                let dispute_deadline = report.reported_at.saturating_add(self.oracle.dispute_window);
                self.state = ResolutionState::Proposed {
                    report,
                    dispute_deadline,
                };
                Ok(&self.state)
            }
        }
    }

    /// Challenge a proposed resolution, inside its window.
    pub fn dispute(&mut self, dispute: Dispute) -> Result<()> {
        let ResolutionState::Proposed {
            report,
            dispute_deadline,
        } = &self.state
        else {
            return Err(Error::denied(format!(
                "a {} resolution cannot be disputed",
                self.state.as_str()
            )));
        };
        if !self.oracle.kind.is_challengeable() {
            return Err(Error::denied(format!(
                "oracle {} is an {} and publishes no challenge process",
                self.oracle.name,
                self.oracle.kind.as_str()
            )));
        }
        if dispute.raised_at > *dispute_deadline {
            return Err(Error::denied(format!(
                "the dispute window for market {} closed at {}",
                self.market_id,
                dispute_deadline.to_rfc3339()
            )));
        }
        self.state = ResolutionState::Disputed {
            report: report.clone(),
            dispute,
        };
        Ok(())
    }

    /// Decide a dispute in the oracle's favour.
    pub fn uphold(&mut self, at: Timestamp) -> Result<()> {
        let ResolutionState::Disputed { report, .. } = &self.state else {
            return Err(Error::denied(format!(
                "there is no dispute to uphold; the market is {}",
                self.state.as_str()
            )));
        };
        self.state = ResolutionState::Final {
            outcome: report.outcome.clone(),
            at,
        };
        Ok(())
    }

    /// Decide a dispute against the oracle: another outcome won, or none did.
    pub fn overturn(&mut self, outcome: Option<OutcomeId>, at: Timestamp) -> Result<()> {
        if !matches!(self.state, ResolutionState::Disputed { .. }) {
            return Err(Error::denied(format!(
                "there is no dispute to overturn; the market is {}",
                self.state.as_str()
            )));
        }
        self.state = match outcome {
            Some(outcome) => ResolutionState::Final { outcome, at },
            None => ResolutionState::Void {
                reason: "the resolution was overturned with no replacement".to_string(),
                at,
            },
        };
        Ok(())
    }

    /// Close an unchallenged window and make the resolution final.
    pub fn finalise(&mut self, now: Timestamp) -> Result<OutcomeId> {
        let ResolutionState::Proposed {
            report,
            dispute_deadline,
        } = &self.state
        else {
            return Err(Error::denied(format!(
                "a {} resolution cannot be finalised",
                self.state.as_str()
            )));
        };
        if now < *dispute_deadline {
            return Err(Error::denied(format!(
                "the dispute window for market {} is open until {}",
                self.market_id,
                dispute_deadline.to_rfc3339()
            )));
        }
        let outcome = report.outcome.clone();
        self.state = ResolutionState::Final {
            outcome: outcome.clone(),
            at: now,
        };
        Ok(outcome)
    }

    /// Void the market, applying the settlement rule for an unresolvable one.
    pub fn void(&mut self, reason: impl Into<String>, at: Timestamp) -> Result<()> {
        if self.state.is_settleable() {
            return Err(Error::denied(format!(
                "market {} is already final",
                self.market_id
            )));
        }
        self.state = ResolutionState::Void {
            reason: reason.into(),
            at,
        };
        Ok(())
    }

    /// What a position in `outcome` pays.
    ///
    /// Refuses everything that is not final. A proposed resolution inside its
    /// dispute window is not money, and a disputed one is a coin toss with the
    /// downside already banked into the price the position was bought at.
    pub fn settle(
        &self,
        outcome: &OutcomeId,
        quantity: Decimal,
        fees: &FeeSchedule,
    ) -> Result<Settlement> {
        let ResolutionState::Final { outcome: won, .. } = &self.state else {
            return Err(Error::denied(format!(
                "market {} is {} and cannot be settled",
                self.market_id,
                self.state.as_str()
            )));
        };
        if !quantity.is_positive() {
            return Err(Error::invalid("a settlement needs a positive quantity"));
        }
        let per_contract = if won == outcome {
            self.settlement.payoff
        } else {
            Decimal::ZERO
        };
        let gross = per_contract * quantity;
        let fee = fees.settlement_cost(gross);
        Ok(Settlement {
            outcome: outcome.clone(),
            quantity,
            gross,
            fee,
            net: gross - fee,
        })
    }
}

/// Who resolves a market.
pub trait Oracle: std::fmt::Debug {
    fn identity(&self) -> OracleIdentity;

    /// What the oracle says as of `at`, if anything.
    ///
    /// Pull, like every other source in the platform: the caller owns the
    /// clock, so a replay reaches the same resolution at the same moment.
    fn report(&self, market: &EventMarket, at: Timestamp) -> Result<Option<OracleReport>>;
}

/// An oracle that reports exactly what it was told to, when it was told to.
///
/// The deterministic stand-in for a real resolver: it models delay and low
/// confidence, which are the two behaviours worth testing against.
#[derive(Clone, Debug)]
pub struct ScriptedOracle {
    identity: OracleIdentity,
    reports: BTreeMap<String, (Timestamp, OracleReport)>,
}

impl ScriptedOracle {
    pub fn new(identity: OracleIdentity) -> Self {
        Self {
            identity,
            reports: BTreeMap::new(),
        }
    }

    /// Schedule a report to become visible at `available_at`.
    pub fn schedule(
        mut self,
        market_id: &ObjectId,
        available_at: Timestamp,
        report: OracleReport,
    ) -> Self {
        self.reports
            .insert(market_id.to_string(), (available_at, report));
        self
    }
}

impl Oracle for ScriptedOracle {
    fn identity(&self) -> OracleIdentity {
        self.identity.clone()
    }

    fn report(&self, market: &EventMarket, at: Timestamp) -> Result<Option<OracleReport>> {
        Ok(self
            .reports
            .get(&market.market_id.to_string())
            .filter(|(available_at, _)| *available_at <= at)
            .map(|(_, report)| report.clone()))
    }
}
