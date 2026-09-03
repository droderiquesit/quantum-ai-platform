//! The operator console: nine server-rendered views over the whole platform.
//!
//! Three rules shape everything here, and they matter more than the layout.
//!
//! **A view never invents a number.** Every collection is a
//! [`crate::panel::Panel`], which carries whether its contents can be believed.
//! An empty table that looks like "zero exposure" when it means "no cell is
//! reporting" is the most dangerous thing a trading console can render, so the
//! two are different markup with different `data-state` values and are tested
//! apart.
//!
//! **Nothing here can act.** The console is for visibility and governance. The
//! single exception is tripping the kill switch, which is offered because
//! tripping needs no authority — a component that notices something wrong may
//! stop the platform. Clearing a halt is not offered at all, because
//! `qip_risk_engine::autonomy::KillSwitch::clear_global` requires a freshly
//! verified operator identity that a page cannot establish. Stopping is easy,
//! restarting is not, and the console embodies that asymmetry by having no
//! clear path to render.
//!
//! **Escaping is not optional.** Every value reaches the page through
//! [`crate::html::Element::text`]. There is no raw-markup method to reach for.
//!
//! The views render under `default-src 'none'; style-src 'self';
//! form-action 'self'` — no script, no external resource, one inline
//! stylesheet and one form.

pub mod model;
pub mod render;

pub use model::{
    AgentCallRow, AlphaRow, ArbitrageRow, CapitalRow, CellRow, ConsoleModel, ExposureRow, FillRow,
    KillSwitchState, Metric, ModelRow, QuantumRow, RefusalRow, ServiceRow, SourceRow, StrategyRow,
};
pub use render::TRIP_PATH;

use crate::html::{Element, a, h1, header, main_element, nav, p};
use crate::pages::banner_of;
use crate::style::STYLESHEET;

/// The nine console views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// Regional status, market state, opportunities, strategies, capital,
    /// health.
    Global,
    /// One region: brains, local opportunities, latency, brokers, venues,
    /// inventory, cash, models.
    Regional,
    /// Opportunities, positions, orders, fills, P&L, expected against realised.
    Trading,
    /// Three-arm and N-leg paths, capital, fills, hedges.
    Arbitrage,
    /// Models, reputation, agent calls, confidence, cost, training.
    Ai,
    /// Jobs, solver, runtime, result, and the classical run beside it.
    Quantum,
    /// Discovered, approved and rejected sources; health, freshness, cost,
    /// licensing.
    DataFinder,
    /// Exposure, limits, drawdown, VaR/CVaR, concentration, the kill switch.
    Risk,
    /// Services, transports, clusters, model health, outages, cost.
    Operations,
}

impl View {
    /// The path this view is served at.
    ///
    /// Under `/console` so nothing collides with the eight investment surfaces
    /// in [`crate::pages`], which have their own `/risk` and `/opportunities`.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Global => "/console",
            Self::Regional => "/console/regional",
            Self::Trading => "/console/trading",
            Self::Arbitrage => "/console/arbitrage",
            Self::Ai => "/console/ai",
            Self::Quantum => "/console/quantum",
            Self::DataFinder => "/console/data-finder",
            Self::Risk => "/console/risk",
            Self::Operations => "/console/operations",
        }
    }

    pub const fn title(&self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::Regional => "Regional",
            Self::Trading => "Trading",
            Self::Arbitrage => "Arbitrage",
            Self::Ai => "AI",
            Self::Quantum => "Quantum",
            Self::DataFinder => "Data Finder",
            Self::Risk => "Risk",
            Self::Operations => "Operations",
        }
    }

    pub fn all() -> Vec<Self> {
        vec![
            Self::Global,
            Self::Regional,
            Self::Trading,
            Self::Arbitrage,
            Self::Ai,
            Self::Quantum,
            Self::DataFinder,
            Self::Risk,
            Self::Operations,
        ]
    }

    pub fn from_path(path: &str) -> Option<Self> {
        Self::all().into_iter().find(|view| view.path() == path)
    }
}

/// Render one console view.
pub fn render(view: View, model: &ConsoleModel) -> String {
    let body = match view {
        View::Global => render::global(model),
        View::Regional => render::regional(model),
        View::Trading => render::trading(model),
        View::Arbitrage => render::arbitrage(model),
        View::Ai => render::ai(model),
        View::Quantum => render::quantum(model),
        View::DataFinder => render::data_finder(model),
        View::Risk => render::risk(model),
        View::Operations => render::operations(model),
    };

    let document = Element::new("html")
        .attr("lang", "en")
        .child(
            Element::new("head")
                .child(Element::new("meta").attr("charset", "utf-8"))
                .child(
                    Element::new("meta")
                        .attr("name", "viewport")
                        .attr("content", "width=device-width, initial-scale=1"),
                )
                .child(Element::new("title").text(format!(
                    "{} — Quantum Investment Platform operator console",
                    view.title()
                )))
                // Inlined rather than linked: the policy permits
                // `style-src 'self'`, and one small stylesheet is not worth a
                // second round trip.
                //
                // `.raw`, not `.text`: `<style>` is a raw-text element in the
                // HTML5 parsing model, so nothing inside it is entity-decoded
                // and escaping shipped broken CSS. See `Element::raw`.
                .child(Element::new("style").raw(STYLESHEET)),
        )
        .child(
            Element::new("body")
                .child(
                    header()
                        .child(h1().text("Operator console"))
                        .child(render::navigation(view))
                        .child(
                            nav()
                                .class("secondary")
                                .child(a("/").text("Investment surfaces")),
                        ),
                )
                .child(main_element().child(banner_of(&model.posture)).child(body))
                .child(footer(model)),
        );

    format!("<!DOCTYPE html>{}", document.render())
}

fn footer(model: &ConsoleModel) -> Element {
    Element::new("footer")
        .text(format!(
            "cycle {} · {} event(s) logged · hash chain {} · rendered at {}",
            model.cycle,
            model.events_logged,
            if model.chain_intact {
                "intact"
            } else {
                "BROKEN"
            },
            model.rendered_at
        ))
        .child(p().class("muted").text(
            "Read-only. The only action this console offers is tripping the kill switch; \
             clearing one requires an operator credential and happens on the API.",
        ))
}
