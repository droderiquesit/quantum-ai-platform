//! The view model.
//!
//! A plain data structure that the pages render and that the API layer
//! assembles. Keeping it separate means the pages can be tested without a
//! platform, and — more usefully — that a page cannot acquire a dependency on
//! a lock by accident, which is how a rendering path ends up able to deadlock
//! a trading loop.

use crate::panel::Panel;
use serde::{Deserialize, Serialize};

/// A figure the platform recorded, or the reason it has none.
///
/// The scrape surface's rule, applied to HTML: a value the platform never
/// recorded is not zero. A counter that never incremented has no series, a
/// settlement nothing retained has no count, and a page that printed `0` for
/// either would be making a claim the platform did not. So a figure reaches a
/// page as one of two things — the string the platform produced, or the
/// reason there is none — and the renderer has no arm that turns the second
/// into a number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Fact {
    /// The platform recorded this, and this is what it recorded.
    Recorded { value: String },
    /// The platform has no such fact. `reason` names what would have
    /// recorded it and why it did not, so an operator reading the gap can
    /// tell a counter that never moved from a wire that is not attached.
    NotRecorded { reason: String },
}

impl Fact {
    pub fn recorded(value: impl Into<String>) -> Self {
        Self::Recorded {
            value: value.into(),
        }
    }

    pub fn not_recorded(reason: impl Into<String>) -> Self {
        Self::NotRecorded {
            reason: reason.into(),
        }
    }

    pub fn is_recorded(&self) -> bool {
        matches!(self, Self::Recorded { .. })
    }
}

/// One labelled figure, with the key its markup is addressed by.
///
/// `key` becomes the `data-fact` attribute, so a test can find the one cell
/// it asserts on rather than matching a digit somewhere in the page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactRow {
    pub key: String,
    pub label: String,
    pub fact: Fact,
}

impl FactRow {
    pub fn new(key: impl Into<String>, label: impl Into<String>, fact: Fact) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            fact,
        }
    }
}

/// One edge cell, as the centre last heard from it.
///
/// Every figure here is one the centre holds about the cell: what the last
/// report said, what the centre's own switch says, and — where the centre
/// keeps no per-cell figure — a [`Fact::NotRecorded`] naming why. The three
/// halt wires are kept apart because they are three different facts: the
/// scope the centre itself halted, the flag the centre ships on the policy
/// payload, and the flag a node polls off its own filesystem, which never
/// reaches the centre at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeCellRow {
    pub cell: String,
    /// When the cell's last report was made, formatted.
    pub reported_at: String,
    /// How old that report is, formatted.
    pub age: String,
    pub stale: bool,
    pub positions: usize,
    pub strategies: usize,
    /// Reconciliation breaks the cell itself shipped on its last report.
    pub breaks_shipped: usize,
    /// Orders the centre registered as sent from this cell.
    pub orders_sent: Fact,
    /// Fills the centre settled from this cell.
    pub fills_confirmed: Fact,
    /// Whether the centre's own kill switch holds a halt scoped to this cell.
    pub halted_by_centre: bool,
    /// The halted flag the centre carries on the policy payload it ships.
    pub policy_halt_flag: bool,
    /// Whether the cell's last delta said it had stopped itself.
    pub cell_reports_halted: Fact,
    /// The polled halt flag on the node's own filesystem.
    pub polled_halt_flag: Fact,
}

/// What the centre last shipped one cell, as the platform's journal has it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShippedPolicyRow {
    pub cell: String,
    /// When the cycle whitelist was issued, from the journal.
    pub issued_at: String,
    /// The payload's sequence.
    pub sequence: Fact,
    /// The line the platform journaled for the cycle whitelist — the same
    /// line the cycle response carries.
    pub whitelist: String,
    /// One row per slot of the twelve-item payload, in the blueprint's order.
    pub slots: Vec<FactRow>,
}

/// One instrument the platform will not size a decision on, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniverseExclusionRow {
    pub object: String,
    pub reason: String,
}

/// The universe the platform assembled, as the platform can attest it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UniverseView {
    pub version: Fact,
    pub sha256: Fact,
    pub instruments: Fact,
    /// The instruments the universe itself said may not drive a decision.
    /// Current-and-empty is a real observation: the platform asked and every
    /// instrument answered decision-grade.
    pub not_decision_grade: Panel<UniverseExclusionRow>,
}

impl Default for UniverseView {
    /// Nothing attested. Every figure is absent for the reason the type
    /// gives, and none of them is zero.
    fn default() -> Self {
        let reason = "this view was not assembled; nothing reported it";
        Self {
            version: Fact::not_recorded(reason),
            sha256: Fact::not_recorded(reason),
            instruments: Fact::not_recorded(reason),
            not_decision_grade: Panel::default(),
        }
    }
}

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

    /// The edge cells, as the centre last heard from each.
    pub cells: Panel<EdgeCellRow>,
    /// What the central plane recorded settling every cell's reports, from
    /// its own counters: one row per series, recorded or not.
    pub settlement: Vec<FactRow>,
    /// The last policy the centre shipped each cell, from the journal.
    pub shipped_policy: Panel<ShippedPolicyRow>,
    pub universe: UniverseView,
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
            cells: Panel::default(),
            settlement: Vec::new(),
            shipped_policy: Panel::default(),
            universe: UniverseView::default(),
        }
    }
}
