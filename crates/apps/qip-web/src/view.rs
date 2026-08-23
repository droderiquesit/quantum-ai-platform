//! The view model.
//!
//! A plain data structure that the pages render and that the API layer
//! assembles. Keeping it separate means the pages can be tested without a
//! platform, and — more usefully — that a page cannot acquire a dependency on
//! a lock by accident, which is how a rendering path ends up able to deadlock
//! a trading loop.

use serde::{Deserialize, Serialize};

/// Whether real money is moving, and how much authority the platform holds.
///
/// Extracted from the wider view models so the banner has exactly one
/// implementation. Two banners rendered from two structs is two chances to
/// disagree about the single fact that must never be ambiguous.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posture {
    /// The autonomy level, as a string for display.
    pub autonomy_level: String,
    /// The highest level this deployment may ever reach.
    pub autonomy_ceiling: String,
    /// Whether orders reach a real venue.
    pub live: bool,
    pub halted: bool,
    pub halt_reason: String,
}

impl Default for Posture {
    /// The safe reading, and the one a page rendered before the platform
    /// reported anything must show: paper trading, not halted.
    fn default() -> Self {
        Self {
            autonomy_level: "paper_trading".to_string(),
            autonomy_ceiling: "paper_trading".to_string(),
            live: false,
            halted: false,
            halt_reason: String::new(),
        }
    }
}

/// One stage of the last cycle.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StageRow {
    pub stage: String,
    pub ran: bool,
    pub produced: usize,
    pub detail: String,
}

/// One queued opportunity.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpportunityRow {
    pub id: String,
    pub headline: String,
    pub score: f64,
    pub confidence: f64,
    pub detectors: Vec<String>,
}

/// One thesis.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ThesisRow {
    pub id: String,
    pub statement: String,
    pub status: String,
    pub confidence: f64,
    pub rationale: String,
}

/// One proposal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProposalRow {
    pub id: String,
    pub status: String,
    pub legs: usize,
    pub rationale: String,
}

/// One order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrderRow {
    pub id: String,
    pub instrument: String,
    pub side: String,
    pub quantity: String,
    pub state: String,
    /// Whether the fills came from a simulated venue. Rendered as a badge on
    /// every row, because a reader glancing at an order blotter should never
    /// have to work out whether it was real.
    pub simulated: bool,
}

/// One limit and how much of it is used.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LimitRow {
    pub name: String,
    pub observed: f64,
    pub bound: f64,
    pub utilisation: f64,
    pub breached: bool,
    pub rationale: String,
}

/// One agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentRow {
    pub id: String,
    pub name: String,
    pub role: String,
    pub owner: String,
    pub purpose: String,
    pub capabilities: Vec<String>,
}

/// One governance finding.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GovernanceRow {
    pub severity: String,
    pub rule: String,
    pub detail: String,
}

/// Everything the pages render.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewModel {
    /// The autonomy level, as a string for display.
    pub autonomy_level: String,
    pub autonomy_ceiling: String,
    /// Whether orders reach a real venue.
    pub live: bool,
    pub halted: bool,
    pub halt_reason: String,
    pub cycle: u64,
    pub correlation_id: String,
    pub events_logged: usize,
    pub chain_intact: bool,
    pub rendered_at: String,

    pub equity: String,
    pub position_count: usize,
    pub gross_exposure: f64,
    pub net_exposure: f64,
    /// Whether every fill in the book came from a simulated venue.
    pub paper_only: bool,

    pub stages: Vec<StageRow>,
    pub opportunities: Vec<OpportunityRow>,
    pub theses: Vec<ThesisRow>,
    pub proposals: Vec<ProposalRow>,
    pub orders: Vec<OrderRow>,
    pub refusals: Vec<String>,
    pub limits: Vec<LimitRow>,
    pub agents: Vec<AgentRow>,
    pub governance: Vec<GovernanceRow>,
}

impl ViewModel {
    /// The banner's view of this model.
    ///
    /// Copied out rather than borrowed so the banner has no lifetime tying it
    /// to the model it came from, which is what lets one banner serve both
    /// this model and the console's.
    pub fn posture(&self) -> Posture {
        Posture {
            autonomy_level: self.autonomy_level.clone(),
            autonomy_ceiling: self.autonomy_ceiling.clone(),
            live: self.live,
            halted: self.halted,
            halt_reason: self.halt_reason.clone(),
        }
    }
}

impl Default for ViewModel {
    /// A model describing a platform that has not run.
    ///
    /// The defaults are the safe readings: paper trading, not live, chain
    /// intact, paper-only. A field that defaulted the other way would show a
    /// misleading banner on a page rendered before the platform reported
    /// anything.
    fn default() -> Self {
        Self {
            autonomy_level: "paper_trading".to_string(),
            autonomy_ceiling: "paper_trading".to_string(),
            live: false,
            halted: false,
            halt_reason: String::new(),
            cycle: 0,
            correlation_id: "none".to_string(),
            events_logged: 0,
            chain_intact: true,
            rendered_at: "never".to_string(),
            equity: "0".to_string(),
            position_count: 0,
            gross_exposure: 0.0,
            net_exposure: 0.0,
            paper_only: true,
            stages: Vec::new(),
            opportunities: Vec::new(),
            theses: Vec::new(),
            proposals: Vec::new(),
            orders: Vec::new(),
            refusals: Vec::new(),
            limits: Vec::new(),
            agents: Vec::new(),
            governance: Vec::new(),
        }
    }
}
