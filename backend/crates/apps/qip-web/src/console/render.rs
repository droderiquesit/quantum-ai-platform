//! Rendering the nine console views.
//!
//! Every value reaches the page through [`crate::html::Element::text`], which
//! escapes. There is no other path — the module that would provide one does
//! not exist — so a venue name carrying a quote, a source name carrying a tag
//! and a strategy id carrying a script element all render as content.
//!
//! The panel renderers are the interesting part. A [`Panel`] has three
//! readings and each gets its own markup and its own `data-state`, so the
//! difference between "the platform observed nothing" and "nothing is
//! reaching the platform" survives into the HTML rather than being flattened
//! into an empty table.

use crate::html::{
    Element, a, button, code, div, form, h2, h3, li, p, section, small, span, strong, table, tbody,
    td, th, thead, tr, ul,
};
use crate::panel::{Freshness, Panel};
use crate::view::{GovernanceRow, LimitRow, OpportunityRow, OrderRow};

use super::model::{
    AgentCallRow, AlphaRow, ArbitrageRow, CapitalRow, CellRow, ConsoleModel, ExposureRow, FillRow,
    KillSwitchState, Metric, ModelRow, QuantumRow, RefusalRow, ServiceRow, SourceRow, StrategyRow,
};

/// Where a kill-switch trip is submitted.
///
/// Public so the server routing and the page cannot disagree about the path.
/// There is deliberately no matching constant for clearing a halt: clearing
/// needs a fresh operator credential and belongs on the authenticated API.
pub const TRIP_PATH: &str = "/console/risk/kill-switch";

// --- panels -----------------------------------------------------------------

/// A panel with nothing behind it, and why.
fn nodata(reason: &str) -> Element {
    div()
        .class("nodata")
        .child(strong().text("No data."))
        .child(p().text(reason))
}

/// A panel the platform reported and that is genuinely empty.
///
/// Deliberately not the same markup as [`nodata`]. This is the only state in
/// the console that means zero, and a reader must be able to tell it from the
/// state that means the feed is down.
fn reported_empty(as_of: &str) -> Element {
    div()
        .class("nodata reported")
        .child(strong().text("Reported, and empty."))
        .child(p().text(format!(
            "The platform reported this panel as of {as_of} and it holds nothing. \
             This is an observed zero, not a missing feed."
        )))
}

/// The line above a panel saying how far its contents can be trusted.
fn freshness_note(freshness: &Freshness) -> Option<Element> {
    match freshness {
        Freshness::Current { as_of } => Some(
            p().class("muted mono")
                .text(format!("reported as of {as_of}")),
        ),
        Freshness::Stale { as_of, age, bound } => Some(
            p().class("stale")
                .child(span().class("pill bad").text("STALE"))
                .text(format!(
                    " last reported {as_of}, {age} ago; the freshness bound for this panel is \
                     {bound}. What follows is the last thing seen, not what is true now."
                )),
        ),
        Freshness::Absent { .. } => None,
    }
}

/// Render a panel as a table, or as the reason there is no table.
fn tabular<T>(
    title: &str,
    columns: &[&str],
    panel: &Panel<T>,
    row: impl Fn(&T) -> Element,
) -> Element {
    let rows = panel.rows();
    let mut out = section()
        .attr("data-panel", title)
        .attr(
            "data-state",
            panel.freshness().state_attribute(!rows.is_empty()),
        )
        .child(h2().text(title));
    if let Some(note) = freshness_note(panel.freshness()) {
        out = out.child(note);
    }
    match panel.freshness() {
        Freshness::Absent { reason } => out.child(nodata(reason)),
        Freshness::Current { as_of } if rows.is_empty() => out.child(reported_empty(as_of)),
        Freshness::Stale { as_of, .. } if rows.is_empty() => out.child(reported_empty(as_of)),
        _ => out.child(
            table()
                .child(
                    thead().child(tr().children(columns.iter().map(|column| th().text(*column)))),
                )
                .child(tbody().children(rows.iter().map(row))),
        ),
    }
}

/// Render a panel of figures as cards, or as the reason there are none.
///
/// Cards are the dangerous shape — a card reading `0` is read as a fact — so
/// an absent panel never renders one.
fn figures(title: &str, panel: &Panel<Metric>) -> Element {
    let rows = panel.rows();
    let mut out = section()
        .attr("data-panel", title)
        .attr(
            "data-state",
            panel.freshness().state_attribute(!rows.is_empty()),
        )
        .child(h2().text(title));
    if let Some(note) = freshness_note(panel.freshness()) {
        out = out.child(note);
    }
    match panel.freshness() {
        Freshness::Absent { reason } => out.child(nodata(reason)),
        Freshness::Current { as_of } if rows.is_empty() => out.child(reported_empty(as_of)),
        Freshness::Stale { as_of, .. } if rows.is_empty() => out.child(reported_empty(as_of)),
        _ => out.child(div().class("cards").children(rows.iter().map(|metric| {
            let card = div()
                .class(if panel.is_stale() {
                    "card stale"
                } else {
                    "card"
                })
                .child(div().class("label").text(&metric.label))
                .child(div().class("value").text(&metric.value));
            if metric.note.is_empty() {
                card
            } else {
                card.child(small().class("muted").text(&metric.note))
            }
        }))),
    }
}

fn percent(value: f64) -> String {
    format!("{:.1}%", value * 100.0)
}

fn pill(state: &str) -> Element {
    let class = match state {
        "ok" | "reporting" | "approved" | "filled" | "intact" => "pill good",
        "stale" | "degraded" | "pending" | "discovered" | "partial" => "pill warn",
        "halted" | "down" | "rejected" | "broken" | "breached" => "pill bad",
        _ => "pill",
    };
    span().class(class).text(state)
}

// --- row renderers ----------------------------------------------------------

fn cell_row(row: &CellRow) -> Element {
    tr().child(td().class("mono").text(&row.cell))
        .child(td().child(pill(&row.status)))
        .child(td().class("mono").text(&row.reported_at))
        .child(td().class("mono").text(&row.age))
        .child(td().class("mono").text(row.positions.to_string()))
        .child(td().class("mono").text(&row.gross))
        .child(td().class("mono").text(&row.net))
        .child(td().class("mono").text(row.strategies.to_string()))
        .child(
            td().class(if row.reconciliation_breaks > 0 {
                "mono bad"
            } else {
                "mono"
            })
            .text(row.reconciliation_breaks.to_string()),
        )
}

fn strategy_row(row: &StrategyRow) -> Element {
    tr().child(td().class("mono").text(&row.id))
        .child(td().class("mono").text(&row.cell))
        .child(td().class("mono").text(&row.venue))
        .child(td().child(pill(&row.stage)))
        .child(td().text(if row.holds_capital { "yes" } else { "no" }))
        .child(td().class("mono muted").text(&row.registered_at))
}

fn exposure_row(row: &ExposureRow) -> Element {
    tr().child(td().class("muted").text(&row.axis))
        .child(td().class("mono").text(&row.bucket))
        .child(td().class("mono").text(&row.gross))
        .child(td().class("mono").text(&row.net))
        .child(td().class("mono").text(percent(row.share)))
        .child(td().class("mono muted").text(percent(row.limit)))
        .child(td().child(pill(if row.breached { "breached" } else { "ok" })))
}

fn capital_row(row: &CapitalRow) -> Element {
    tr().child(td().text(&row.subject))
        .child(td().class("mono").text(&row.cell))
        .child(td().class("mono").text(&row.strategy))
        .child(td().class("mono").text(&row.granted))
        .child(td().class("mono").text(&row.used))
        .child(td().class("mono").text(&row.utilisation))
        .child(td().class("mono muted").text(&row.expires_at))
}

fn order_row(row: &OrderRow) -> Element {
    tr().child(td().class("mono").text(&row.id))
        .child(td().class("mono").text(&row.instrument))
        .child(td().text(&row.side))
        .child(td().class("mono").text(&row.quantity))
        .child(td().text(&row.state))
        .child(
            td().child(
                span()
                    .class(if row.simulated { "pill" } else { "pill warn" })
                    .text(if row.simulated { "paper" } else { "live" }),
            ),
        )
}

fn fill_row(row: &FillRow) -> Element {
    tr().child(td().class("mono").text(&row.order))
        .child(td().class("mono").text(&row.instrument))
        .child(td().text(&row.side))
        .child(td().class("mono").text(&row.quantity))
        .child(td().class("mono").text(&row.price))
        .child(td().class("mono").text(&row.venue))
        .child(
            td().child(
                span()
                    .class(if row.simulated { "pill" } else { "pill warn" })
                    .text(if row.simulated { "paper" } else { "live" }),
            ),
        )
}

fn refusal_row(row: &RefusalRow) -> Element {
    tr().child(td().class("mono").text(&row.order))
        .child(td().class("mono muted").text(&row.at))
        .child(
            td().child(
                span()
                    .class(match row.kind.as_str() {
                        // A control refused it. Red, because an order was
                        // stopped, and green would read as "nothing happened".
                        "safety control" => "pill bad",
                        "fault" => "pill warn",
                        // Neither claim is available for a refusal whose
                        // reason nobody recorded.
                        _ => "pill",
                    })
                    .text(&row.kind),
            ),
        )
        .child(td().text(&row.reason))
}

fn alpha_row(row: &AlphaRow) -> Element {
    tr().child(td().text(&row.subject))
        .child(td().class("mono").text(&row.expected))
        .child(td().class("mono").text(&row.realised))
        .child(td().class("mono").text(&row.difference))
}

fn arbitrage_row(row: &ArbitrageRow) -> Element {
    tr().child(td().class("mono").text(&row.id))
        .child(td().text(&row.shape))
        .child(td().class("mono").text(row.legs.to_string()))
        .child(td().class("mono").text(&row.capital_required))
        .child(td().child(pill(&row.fill_state)))
        .child(td().child(pill(&row.hedge_state)))
}

fn model_row(row: &ModelRow) -> Element {
    tr().child(td().class("mono").text(&row.id))
        .child(td().text(&row.purpose))
        .child(td().class("mono").text(&row.calls))
        .child(td().class("mono").text(&row.tokens))
        .child(td().class("mono").text(&row.cost))
        .child(td().child(pill(&row.status)))
}

fn agent_call_row(row: &AgentCallRow) -> Element {
    tr().child(td().class("mono").text(&row.agent))
        .child(td().class("mono muted").text(&row.run))
        .child(td().child(pill(&row.status)))
        .child(td().class("mono").text(row.tool_calls.to_string()))
        .child(td().class("mono").text(row.model_calls.to_string()))
        .child(td().class("mono").text(row.tokens.to_string()))
        .child(td().class("mono").text(&row.cost))
        .child(td().class("mono").text(percent(row.utilisation)))
        .child(match row.conviction {
            Some(conviction) => td().class("mono").text(format!("{conviction:.2}")),
            // Not zero. An agent that produced no finding has not expressed
            // low conviction; it has expressed none.
            None => td().class("muted").text("no finding"),
        })
}

fn quantum_row(row: &QuantumRow) -> Element {
    tr().child(td().class("mono").text(&row.job))
        .child(td().class("mono").text(&row.solver))
        .child(td().class("mono").text(&row.runtime))
        .child(td().class("mono").text(&row.result))
        .child(td().class("mono muted").text(&row.classical_solver))
        .child(td().class("mono muted").text(&row.classical_runtime))
        .child(td().class("mono muted").text(&row.classical_result))
        .child(td().text(&row.verdict))
}

fn source_row(row: &SourceRow) -> Element {
    tr().child(td().text(&row.name))
        .child(td().child(pill(&row.state)))
        .child(td().child(pill(&row.health)))
        .child(td().class("mono").text(&row.freshness))
        .child(td().class("mono").text(&row.cost))
        .child(td().text(&row.licence))
}

fn service_row(row: &ServiceRow) -> Element {
    tr().child(td().class("mono").text(&row.name))
        .child(td().child(pill(&row.state)))
        .child(td().class("muted").text(&row.detail))
}

fn opportunity_row(row: &OpportunityRow) -> Element {
    tr().child(td().class("mono").text(&row.id))
        .child(td().text(&row.headline))
        .child(td().class("mono").text(format!("{:.3}", row.score)))
        .child(td().class("mono").text(format!("{:.2}", row.confidence)))
        .child(td().class("muted").text(row.detectors.join(", ")))
}

fn limit_row(row: &LimitRow) -> Element {
    tr().child(td().text(&row.name))
        .child(td().class("mono").text(format!("{:.4}", row.observed)))
        .child(td().class("mono").text(format!("{:.4}", row.bound)))
        .child(
            td().child(
                span()
                    .class(if row.breached {
                        "pill bad"
                    } else if row.utilisation > 0.8 {
                        "pill warn"
                    } else {
                        "pill good"
                    })
                    .text(percent(row.utilisation)),
            ),
        )
        .child(td().class("muted").text(&row.rationale))
}

fn governance_row(row: &GovernanceRow) -> Element {
    tr().child(td().child(pill(if row.severity == "error" {
        "down"
    } else {
        "degraded"
    })))
    .child(td().class("mono").text(&row.rule))
    .child(td().class("muted").text(&row.detail))
}

// --- the nine views ---------------------------------------------------------

pub(super) fn global(model: &ConsoleModel) -> Element {
    div()
        .child(tabular(
            "Regional status",
            &[
                "Cell",
                "Status",
                "Reported",
                "Age",
                "Positions",
                "Gross",
                "Net",
                "Strategies",
                "Breaks",
            ],
            &model.regions,
            cell_row,
        ))
        .child(figures("Market state", &model.market_state))
        .child(tabular(
            "Opportunities",
            &["Id", "Headline", "Score", "Confidence", "Detectors"],
            &model.opportunities,
            opportunity_row,
        ))
        .child(tabular(
            "Active strategies",
            &[
                "Strategy",
                "Cell",
                "Venue",
                "Stage",
                "Capital",
                "Registered",
            ],
            &model.strategies,
            strategy_row,
        ))
        .child(tabular(
            "Capital distribution",
            &[
                "Subject", "Cell", "Strategy", "Granted", "Used", "Used %", "Expires",
            ],
            &model.capital_distribution,
            capital_row,
        ))
        .child(tabular(
            "System health",
            &["Component", "State", "Detail"],
            &model.system_health,
            service_row,
        ))
}

pub(super) fn regional(model: &ConsoleModel) -> Element {
    div()
        .child(tabular(
            "Cells",
            &[
                "Cell",
                "Status",
                "Reported",
                "Age",
                "Positions",
                "Gross",
                "Net",
                "Strategies",
                "Breaks",
            ],
            &model.regions,
            cell_row,
        ))
        .child(figures("Brain state", &model.cell_brains))
        .child(tabular(
            "Local opportunities",
            &["Id", "Headline", "Score", "Confidence", "Detectors"],
            &model.local_opportunities,
            opportunity_row,
        ))
        .child(figures("Latency", &model.cell_latency))
        .child(tabular(
            "Brokers",
            &["Broker", "State", "Detail"],
            &model.brokers,
            service_row,
        ))
        .child(tabular(
            "Venues",
            &["Venue", "State", "Detail"],
            &model.venues,
            service_row,
        ))
        .child(tabular(
            "Inventory",
            &["Axis", "Bucket", "Gross", "Net", "Share", "Limit", "State"],
            &model.inventory,
            exposure_row,
        ))
        .child(figures("Cash", &model.cash))
        .child(tabular(
            "Models in the cell",
            &["Model", "Purpose", "Calls", "Tokens", "Cost", "State"],
            &model.cell_models,
            model_row,
        ))
}

pub(super) fn trading(model: &ConsoleModel) -> Element {
    div()
        .child(tabular(
            "Opportunities",
            &["Id", "Headline", "Score", "Confidence", "Detectors"],
            &model.opportunities,
            opportunity_row,
        ))
        .child(tabular(
            "Active positions",
            &["Axis", "Bucket", "Gross", "Net", "Share", "Limit", "State"],
            &model.positions,
            exposure_row,
        ))
        .child(tabular(
            "Pending orders",
            &["Id", "Instrument", "Side", "Quantity", "State", "Venue"],
            &model.pending_orders,
            order_row,
        ))
        .child(tabular(
            "Fills",
            &[
                "Order",
                "Instrument",
                "Side",
                "Quantity",
                "Price",
                "Venue",
                "Kind",
            ],
            &model.fills,
            fill_row,
        ))
        .child(figures("Profit and loss", &model.pnl))
        .child(tabular(
            "Expected against realised alpha",
            &["Subject", "Expected", "Realised", "Difference"],
            &model.alpha,
            alpha_row,
        ))
        .child(tabular(
            "Refusals",
            &["Order", "Refused at", "Kind", "Reason"],
            &model.refusals,
            refusal_row,
        ))
}

pub(super) fn arbitrage(model: &ConsoleModel) -> Element {
    div()
        .child(tabular(
            "Three-arm paths",
            &["Id", "Shape", "Legs", "Capital", "Fills", "Hedge"],
            &model.three_arm,
            arbitrage_row,
        ))
        .child(tabular(
            "N-leg paths",
            &["Id", "Shape", "Legs", "Capital", "Fills", "Hedge"],
            &model.n_leg,
            arbitrage_row,
        ))
        .child(figures("Capital required", &model.arbitrage_capital))
}

pub(super) fn ai(model: &ConsoleModel) -> Element {
    div()
        .child(tabular(
            "Active models",
            &["Model", "Purpose", "Calls", "Tokens", "Cost", "State"],
            &model.models,
            model_row,
        ))
        .child(figures(
            "Contextual model reputation",
            &model.model_reputation,
        ))
        .child(tabular(
            "Agent calls",
            &[
                "Agent",
                "Run",
                "Status",
                "Tools",
                "Model calls",
                "Tokens",
                "Cost",
                "Budget used",
                "Conviction",
            ],
            &model.agent_calls,
            agent_call_row,
        ))
        .child(figures("Training", &model.training))
}

pub(super) fn quantum(model: &ConsoleModel) -> Element {
    div()
        .child(p().class("muted").text(
            "A quantum result is never shown without the classical run beside it. \
                 A speed-up nobody compared is a claim, not a measurement.",
        ))
        .child(tabular(
            "Submitted jobs",
            &[
                "Job",
                "Solver",
                "Runtime",
                "Result",
                "Classical solver",
                "Classical runtime",
                "Classical result",
                "Verdict",
            ],
            &model.quantum_jobs,
            quantum_row,
        ))
        .child(figures("Routing", &model.quantum_routing))
}

pub(super) fn data_finder(model: &ConsoleModel) -> Element {
    div()
        .child(tabular(
            "Sources",
            &["Source", "State", "Health", "Freshness", "Cost", "Licence"],
            &model.sources,
            source_row,
        ))
        .child(figures("Source health", &model.source_health))
}

pub(super) fn risk(model: &ConsoleModel) -> Element {
    div()
        .child(tabular(
            "Exposure",
            &["Axis", "Bucket", "Gross", "Net", "Share", "Limit", "State"],
            &model.exposure,
            exposure_row,
        ))
        .child(tabular(
            "Limits",
            &["Limit", "Observed", "Bound", "Used", "Rationale"],
            &model.limits,
            limit_row,
        ))
        .child(figures("Drawdown, VaR and CVaR", &model.tail_risk))
        .child(tabular(
            "Concentration",
            &["Axis", "Bucket", "Gross", "Net", "Share", "Limit", "State"],
            &model.concentration,
            exposure_row,
        ))
        .child(tabular(
            "Regional limits",
            &[
                "Subject", "Cell", "Strategy", "Granted", "Used", "Used %", "Expires",
            ],
            &model.regional_limits,
            capital_row,
        ))
        .child(kill_switch(&model.kill_switch))
}

pub(super) fn operations(model: &ConsoleModel) -> Element {
    div()
        .child(tabular(
            "Services",
            &["Service", "State", "Detail"],
            &model.services,
            service_row,
        ))
        .child(tabular(
            "Transports",
            &["Transport", "State", "Detail"],
            &model.transports,
            service_row,
        ))
        .child(tabular(
            "Clusters",
            &["Cluster", "State", "Detail"],
            &model.clusters,
            service_row,
        ))
        .child(tabular(
            "Model health",
            &["Model", "State", "Detail"],
            &model.model_health,
            service_row,
        ))
        .child(tabular(
            "Source outages",
            &["Source", "State", "Detail"],
            &model.source_outages,
            service_row,
        ))
        .child(figures("Cost", &model.operating_cost))
        .child(tabular(
            "Governance",
            &["Severity", "Rule", "Detail"],
            &model.governance,
            governance_row,
        ))
}

/// The one control the console has, and the one it deliberately does not.
///
/// Tripping is offered because tripping needs no authority: any component that
/// notices something wrong may stop the platform, and an operator watching a
/// screen at three in the morning is a component that noticed something wrong.
/// Clearing is not offered at all — not disabled, not hidden behind a role,
/// simply absent — because `qip_risk_engine::autonomy::KillSwitch::clear_global`
/// requires a freshly verified operator identity, and a page cannot establish
/// one. Lifting a halt happens on the authenticated API and is recorded there.
fn kill_switch(state: &KillSwitchState) -> Element {
    let mut out = section()
        .attr("data-panel", "Kill switch")
        .attr(
            "data-state",
            if state.halted { "halted" } else { "running" },
        )
        .child(h2().text("Kill switch"));

    if state.halted {
        out = out
            .child(
                div()
                    .class("banner halted")
                    .child(strong().text("HALTED"))
                    .text(format!(
                        " — {} (tripped by {} at {})",
                        state.reason, state.tripped_by, state.tripped_at
                    )),
            )
            .child(p().text(
                "Trading is stopped. This console cannot lift the halt. Lifting one requires an \
                 operator credential verified within the last fifteen minutes, which a page \
                 cannot establish, so it is done through the authenticated API and recorded \
                 there.",
            ));
    } else {
        out = out
            .child(p().text(
                "Tripping the kill switch stops every order the platform would otherwise send. \
                 It needs no authority beyond noticing something wrong, which is why it is here.",
            ))
            .child(
                form(TRIP_PATH)
                    .class("killswitch")
                    .child(
                        Element::new("input")
                            .attr("type", "hidden")
                            .attr("name", "confirm")
                            .attr("value", "halt"),
                    )
                    .child(button().class("danger").text("Trip the kill switch")),
            )
            .child(p().class("muted").text(
                "Stopping is easy and restarting is not, on purpose. Once tripped, this console \
                 offers no way to clear it.",
            ));
    }

    if !state.halted_scopes.is_empty() {
        out = out.child(h3().text("Halted scopes")).child(
            ul().children(
                state
                    .halted_scopes
                    .iter()
                    .map(|scope| li().child(code().text(scope))),
            ),
        );
    }

    out.child(p().class("muted").child(small().text(format!(
        "{} halt(s) have been lifted on this switch, each recorded.",
        state.clearances
    ))))
}

/// A link back to the eight non-console surfaces, and the console's own nav.
pub(super) fn navigation(current: super::View) -> Element {
    Element::new("nav").children(super::View::all().into_iter().map(|view| {
        let link = a(view.path()).text(view.title());
        if view == current {
            link.class("current")
        } else {
            link
        }
    }))
}
