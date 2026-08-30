//! What the console renders.
//!
//! Plain data, assembled by the caller and handed in. Nothing here reaches
//! into the platform, so a view can be tested without one and — more usefully
//! — a rendering path cannot acquire a lock by accident and stall a trading
//! loop behind an HTML page.
//!
//! Every collection is a [`Panel`], never a bare `Vec`. See
//! [`crate::panel`] for why: a panel carries whether its contents can be
//! believed, so a view cannot render "no exposure" when it means "no cell is
//! reporting".

use crate::panel::Panel;
use crate::view::{GovernanceRow, LimitRow, OpportunityRow, OrderRow, Posture};
use serde::{Deserialize, Serialize};

/// One labelled figure.
///
/// The value is a pre-formatted string rather than a number, because a console
/// that formats numbers itself has to decide what to show for a number it does
/// not have — and every answer to that is a lie. A figure that exists has a
/// string; one that does not is not in the panel at all.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metric {
    pub label: String,
    pub value: String,
    /// Where the figure came from, or what bounds it. Rendered small.
    pub note: String,
}

impl Metric {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            note: String::new(),
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }
}

/// One edge cell, as the centre last heard from it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellRow {
    pub cell: String,
    /// `reporting`, `stale`, or `halted`.
    pub status: String,
    /// When the cell last reported, formatted.
    pub reported_at: String,
    /// How long ago that was, formatted. Empty when never.
    pub age: String,
    pub positions: usize,
    pub gross: String,
    pub net: String,
    pub strategies: usize,
    pub reconciliation_breaks: usize,
    pub halted: bool,
}

/// One strategy on the promotion ladder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRow {
    pub id: String,
    pub cell: String,
    pub venue: String,
    pub stage: String,
    pub holds_capital: bool,
    pub registered_at: String,
}

/// Gross exposure in one bucket of one axis.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExposureRow {
    pub axis: String,
    pub bucket: String,
    pub gross: String,
    pub net: String,
    /// Share of total gross, in `[0, 1]`.
    pub share: f64,
    /// The concentration limit for this axis, in `[0, 1]`.
    pub limit: f64,
    pub breached: bool,
}

/// One capital grant, or one bound on capital.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapitalRow {
    pub subject: String,
    pub cell: String,
    pub strategy: String,
    pub granted: String,
    pub used: String,
    pub utilisation: String,
    pub expires_at: String,
}

/// One fill.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FillRow {
    pub order: String,
    pub instrument: String,
    pub side: String,
    pub quantity: String,
    pub price: String,
    pub venue: String,
    /// Whether the fill came from a simulated venue. Shown on every row: a
    /// reader glancing at a blotter should never have to work it out.
    pub simulated: bool,
}

/// One order the platform refused, and why.
///
/// A row rather than a figure, and the distinction is not cosmetic. A card
/// reading "Refusals 3" says three orders did not happen without saying which
/// three, or why, or whether the refusal was a control doing its job — and a
/// risk limit refusing an order and a venue being unreachable are opposite
/// findings: the first is the platform working, the second is the platform
/// broken. A count cannot tell them apart, so the panel does not render one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusalRow {
    pub order: String,
    /// When the submission was refused, formatted.
    pub at: String,
    /// `safety control`, `fault`, or `not recorded`.
    ///
    /// A safety refusal must never be retried automatically and a transient
    /// fault may be, so the two never render as the same thing. A refusal
    /// whose reason was not recorded is neither: claiming it was a fault would
    /// invite a retry nobody is entitled to.
    pub kind: String,
    /// The reason, in the words the order manager recorded.
    pub reason: String,
}

/// Expected against realised, for one subject.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlphaRow {
    pub subject: String,
    pub expected: String,
    pub realised: String,
    pub difference: String,
}

/// One arbitrage path.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArbitrageRow {
    pub id: String,
    /// `three-arm` or `n-leg`.
    pub shape: String,
    pub legs: usize,
    pub capital_required: String,
    pub fill_state: String,
    pub hedge_state: String,
}

/// One model the platform can call.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRow {
    pub id: String,
    pub purpose: String,
    pub calls: String,
    pub tokens: String,
    pub cost: String,
    pub status: String,
}

/// One agent run, as the audit trail recorded it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentCallRow {
    pub agent: String,
    pub run: String,
    pub status: String,
    pub tool_calls: u32,
    pub model_calls: u32,
    pub tokens: u32,
    pub cost: String,
    /// Fraction of the tightest budget line used, in `[0, ∞)`.
    pub utilisation: f64,
    /// The agent's own conviction, if it produced a finding. `None` renders as
    /// "no finding" rather than as zero conviction, which would be a claim.
    pub conviction: Option<f64>,
}

/// One quantum job and the classical run beside it.
///
/// The classical comparison is a field of the same row rather than a separate
/// panel, so a quantum result cannot be read without it. See
/// `docs/adr/0006-classical-baseline-always.md`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuantumRow {
    pub job: String,
    pub solver: String,
    pub runtime: String,
    pub result: String,
    pub classical_solver: String,
    pub classical_runtime: String,
    pub classical_result: String,
    /// What the comparison shows, in words.
    pub verdict: String,
}

/// One data source the finder knows about.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRow {
    pub name: String,
    /// `discovered`, `approved` or `rejected`.
    pub state: String,
    pub health: String,
    pub freshness: String,
    pub cost: String,
    pub licence: String,
}

/// One service, transport, cluster or dependency and how it is doing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRow {
    pub name: String,
    /// `ok`, `degraded`, `down` or `unknown`.
    pub state: String,
    pub detail: String,
}

/// The kill switch, as the console may show it.
///
/// Tripping is offered; clearing is not. The asymmetry is
/// `qip_risk_engine::autonomy`'s and is repeated here because a console that
/// could clear a halt would be a console that can restart trading without an
/// operator credential.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillSwitchState {
    pub halted: bool,
    pub halted_scopes: Vec<String>,
    pub tripped_by: String,
    pub tripped_at: String,
    pub reason: String,
    /// How many halts have been lifted, from the switch's own record.
    pub clearances: usize,
}

/// Everything the nine console views render.
///
/// Wide on purpose: one model assembled once per request, from one lock taken
/// once and released before rendering.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConsoleModel {
    /// Whether real money is moving. Rendered at the top of every view.
    pub posture: Posture,
    pub rendered_at: String,
    pub cycle: u64,
    pub events_logged: usize,
    pub chain_intact: bool,

    // --- global ---
    pub regions: Panel<CellRow>,
    pub market_state: Panel<Metric>,
    pub opportunities: Panel<OpportunityRow>,
    pub strategies: Panel<StrategyRow>,
    pub capital_distribution: Panel<CapitalRow>,
    pub system_health: Panel<ServiceRow>,

    // --- regional ---
    pub cell_brains: Panel<Metric>,
    pub local_opportunities: Panel<OpportunityRow>,
    pub cell_latency: Panel<Metric>,
    pub brokers: Panel<ServiceRow>,
    pub venues: Panel<ServiceRow>,
    pub inventory: Panel<ExposureRow>,
    pub cash: Panel<Metric>,
    pub cell_models: Panel<ModelRow>,

    // --- trading ---
    pub positions: Panel<ExposureRow>,
    pub pending_orders: Panel<OrderRow>,
    pub fills: Panel<FillRow>,
    pub pnl: Panel<Metric>,
    pub alpha: Panel<AlphaRow>,
    pub refusals: Panel<RefusalRow>,

    // --- arbitrage ---
    pub three_arm: Panel<ArbitrageRow>,
    pub n_leg: Panel<ArbitrageRow>,
    pub arbitrage_capital: Panel<Metric>,

    // --- ai ---
    pub models: Panel<ModelRow>,
    pub model_reputation: Panel<Metric>,
    pub agent_calls: Panel<AgentCallRow>,
    pub training: Panel<Metric>,

    // --- quantum ---
    pub quantum_jobs: Panel<QuantumRow>,
    pub quantum_routing: Panel<Metric>,

    // --- data finder ---
    pub sources: Panel<SourceRow>,
    pub source_health: Panel<Metric>,

    // --- risk ---
    pub limits: Panel<LimitRow>,
    pub exposure: Panel<ExposureRow>,
    pub tail_risk: Panel<Metric>,
    pub concentration: Panel<ExposureRow>,
    pub regional_limits: Panel<CapitalRow>,
    pub kill_switch: KillSwitchState,

    // --- operations ---
    pub services: Panel<ServiceRow>,
    pub transports: Panel<ServiceRow>,
    pub clusters: Panel<ServiceRow>,
    pub model_health: Panel<ServiceRow>,
    pub source_outages: Panel<ServiceRow>,
    pub operating_cost: Panel<Metric>,
    pub governance: Panel<GovernanceRow>,
}
