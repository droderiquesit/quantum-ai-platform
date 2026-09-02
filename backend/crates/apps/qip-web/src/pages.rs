//! The nine surfaces.
//!
//! Each is a pure function from a view model to HTML. Nothing here reaches
//! into the platform: the caller assembles a [`crate::view::ViewModel`] and
//! passes it in, which keeps the rendering testable and keeps the pages from
//! quietly acquiring a dependency on a lock.
//!
//! The banner at the top of every page states whether the platform is paper
//! trading, live, or halted. It is the first thing rendered and the last thing
//! that should ever be ambiguous.

use crate::html::{
    Element, a, code, div, h1, h2, h3, header, li, main_element, nav, p, section, span, strong,
    table, tbody, td, th, thead, tr, ul,
};
use crate::style::STYLESHEET;
use crate::view::{AgentRow, OpportunityRow, OrderRow, Posture, ProposalRow, StageRow, ViewModel};

/// The nine surfaces the platform exposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Overview,
    Opportunities,
    Theses,
    Portfolio,
    Risk,
    Execution,
    Agents,
    Governance,
    Audit,
}

impl Surface {
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Overview => "/",
            Self::Opportunities => "/opportunities",
            Self::Theses => "/theses",
            Self::Portfolio => "/portfolio",
            Self::Risk => "/risk",
            Self::Execution => "/execution",
            Self::Agents => "/agents",
            Self::Governance => "/governance",
            Self::Audit => "/audit",
        }
    }

    pub const fn title(&self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Opportunities => "Opportunities",
            Self::Theses => "Theses",
            Self::Portfolio => "Portfolio",
            Self::Risk => "Risk",
            Self::Execution => "Execution",
            Self::Agents => "Agents",
            Self::Governance => "Governance",
            Self::Audit => "Audit",
        }
    }

    pub fn all() -> Vec<Self> {
        vec![
            Self::Overview,
            Self::Opportunities,
            Self::Theses,
            Self::Portfolio,
            Self::Risk,
            Self::Execution,
            Self::Agents,
            Self::Governance,
            Self::Audit,
        ]
    }

    pub fn from_path(path: &str) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|surface| surface.path() == path)
    }
}

/// Render a full page.
pub fn render(surface: Surface, model: &ViewModel) -> String {
    let body = match surface {
        Surface::Overview => overview(model),
        Surface::Opportunities => opportunities(model),
        Surface::Theses => theses(model),
        Surface::Portfolio => portfolio(model),
        Surface::Risk => risk(model),
        Surface::Execution => execution(model),
        Surface::Agents => agents(model),
        Surface::Governance => governance(model),
        Surface::Audit => audit(model),
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
                .child(
                    Element::new("title")
                        .text(format!("{} — Quantum Investment Platform", surface.title())),
                )
                .child(Element::new("style").text(STYLESHEET)),
        )
        .child(
            Element::new("body")
                .child(chrome(surface))
                .child(main_element().child(banner(model)).child(body))
                .child(footer(model)),
        );

    format!("<!DOCTYPE html>{}", document.render())
}

fn chrome(current: Surface) -> Element {
    let links: Vec<Element> = Surface::all()
        .into_iter()
        .map(|surface| {
            let link = a(surface.path()).text(surface.title());
            if surface == current {
                link.class("current")
            } else {
                link
            }
        })
        .collect();
    header()
        .child(h1().text("Quantum Investment Platform"))
        .child(nav().children(links))
}

/// The banner. Whether the platform is trading real money is the one thing
/// that must never be ambiguous, so it is stated at the top of every page in
/// its own colour.
fn banner(model: &ViewModel) -> Element {
    banner_of(&model.posture())
}

/// The banner, from a [`Posture`].
///
/// One implementation, used by these surfaces and by the operator console. Two
/// banners rendered from two structs would be two chances to disagree about
/// the one fact that must never be ambiguous.
pub(crate) fn banner_of(posture: &Posture) -> Element {
    if posture.halted {
        let banner = div()
            .class("banner halted")
            .child(strong().text("HALTED"))
            .text(format!(" — {}", posture.halt_reason))
            .child(
                p().class("muted")
                    .text("No orders will be sent until an operator clears the halt."),
            );
        // A halt does not change the posture underneath it. The halted banner
        // used to drop the label, which made it the one place the interface
        // showed posture without saying PAPER TRADING — a reader of a halted
        // page could not tell a halted simulator from a halted live book. A
        // halted live platform is still not labelled live: the halt is the
        // fact that matters there, and "LIVE TRADING" on a page where nothing
        // trades misstates it.
        if posture.live {
            return banner;
        }
        return banner.child(p().class("muted").text(format!(
            "PAPER TRADING — orders reach no market in any state. This deployment's ceiling is {}.",
            posture.autonomy_ceiling
        )));
    }
    if posture.live {
        return div()
            .class("banner live")
            .child(strong().text("LIVE TRADING"))
            .text(format!(" — autonomy level {}", posture.autonomy_level))
            .child(
                p().class("muted")
                    .text("Orders reach real venues. Every fill is a real fill."),
            );
    }
    div()
        .class("banner paper")
        .child(strong().text("PAPER TRADING"))
        .text(format!(" — autonomy level {}", posture.autonomy_level))
        .child(p().class("muted").text(format!(
            "Orders are worked against a simulated venue and reach no market. This deployment's ceiling is {}.",
            posture.autonomy_ceiling
        )))
}

fn footer(model: &ViewModel) -> Element {
    Element::new("footer").text(format!(
        "cycle {} · {} event(s) logged · rendered at {}",
        model.cycle, model.events_logged, model.rendered_at
    ))
}

fn cards(entries: &[(&str, String)]) -> Element {
    div()
        .class("cards")
        .children(entries.iter().map(|(label, value)| {
            div()
                .class("card")
                .child(div().class("label").text(*label))
                .child(div().class("value").text(value))
        }))
}

fn empty(message: &str) -> Element {
    p().class("empty").text(message)
}

// --- the surfaces -----------------------------------------------------------

fn overview(model: &ViewModel) -> Element {
    let stages: Vec<Element> = model
        .stages
        .iter()
        .map(|stage: &StageRow| {
            tr().child(td().class("mono").text(&stage.stage))
                .child(
                    td().child(
                        span()
                            .class(if stage.ran { "pill good" } else { "pill" })
                            .text(if stage.ran { "ran" } else { "skipped" }),
                    ),
                )
                .child(td().class("mono").text(stage.produced.to_string()))
                .child(td().text(&stage.detail))
        })
        .collect();

    section()
        .child(h2().text("Loop"))
        .child(cards(&[
            ("Cycles", model.cycle.to_string()),
            ("Queue", model.opportunities.len().to_string()),
            ("Proposals", model.proposals.len().to_string()),
            ("Orders", model.orders.len().to_string()),
        ]))
        .child(h3().text("Last cycle"))
        .child(if model.stages.is_empty() {
            empty("No cycle has run yet.")
        } else {
            table()
                .child(
                    thead().child(
                        tr().child(th().text("Stage"))
                            .child(th().text("Status"))
                            .child(th().text("Produced"))
                            .child(th().text("Detail")),
                    ),
                )
                .child(tbody().children(stages))
        })
}

fn opportunities(model: &ViewModel) -> Element {
    let rows: Vec<Element> = model
        .opportunities
        .iter()
        .map(|row: &OpportunityRow| {
            tr().child(td().class("mono").text(&row.id))
                .child(td().text(&row.headline))
                .child(td().class("mono").text(format!("{:.3}", row.score)))
                .child(td().class("mono").text(format!("{:.2}", row.confidence)))
                .child(td().class("muted").text(row.detectors.join(", ")))
        })
        .collect();

    section()
        .child(h2().text("Opportunity queue"))
        .child(if rows.is_empty() {
            empty("Nothing is queued. The detectors found nothing above the score floor.")
        } else {
            table()
                .child(
                    thead().child(
                        tr().child(th().text("Id"))
                            .child(th().text("Headline"))
                            .child(th().text("Score"))
                            .child(th().text("Confidence"))
                            .child(th().text("Detectors")),
                    ),
                )
                .child(tbody().children(rows))
        })
}

fn theses(model: &ViewModel) -> Element {
    let rows: Vec<Element> = model
        .theses
        .iter()
        .map(|row| {
            tr().child(td().class("mono").text(&row.id))
                .child(td().text(&row.statement))
                .child(
                    td().child(
                        span()
                            .class(match row.status.as_str() {
                                "approved" | "active" => "pill good",
                                "rejected" => "pill bad",
                                _ => "pill",
                            })
                            .text(&row.status),
                    ),
                )
                .child(td().class("mono").text(format!("{:.2}", row.confidence)))
                .child(td().class("muted").text(&row.rationale))
        })
        .collect();

    section().child(h2().text("Theses")).child(if rows.is_empty() {
        empty("No thesis has been formed. A thesis needs an opportunity whose anomaly implies a mechanism.")
    } else {
        table()
            .child(
                thead().child(
                    tr().child(th().text("Id"))
                        .child(th().text("Statement"))
                        .child(th().text("Status"))
                        .child(th().text("Confidence"))
                        .child(th().text("Review")),
                ),
            )
            .child(tbody().children(rows))
    })
}

fn portfolio(model: &ViewModel) -> Element {
    section()
        .child(h2().text("Portfolio"))
        .child(cards(&[
            ("Equity", model.equity.clone()),
            ("Positions", model.position_count.to_string()),
            ("Gross", format!("{:.1}%", model.gross_exposure * 100.0)),
            ("Net", format!("{:.1}%", model.net_exposure * 100.0)),
        ]))
        .child(p().class("muted").text(if model.paper_only {
            "Every fill in this book came from a simulated venue."
        } else {
            "This book contains fills from a real venue."
        }))
}

fn risk(model: &ViewModel) -> Element {
    let rows: Vec<Element> = model
        .limits
        .iter()
        .map(|row| {
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
                            .text(format!("{:.0}%", row.utilisation * 100.0)),
                    ),
                )
                .child(td().class("muted").text(&row.rationale))
        })
        .collect();

    section()
        .child(h2().text("Limits"))
        .child(if rows.is_empty() {
            empty("No limits are configured, which is itself a finding.")
        } else {
            table()
                .child(
                    thead().child(
                        tr().child(th().text("Limit"))
                            .child(th().text("Observed"))
                            .child(th().text("Bound"))
                            .child(th().text("Used"))
                            .child(th().text("Rationale")),
                    ),
                )
                .child(tbody().children(rows))
        })
}

fn execution(model: &ViewModel) -> Element {
    let orders: Vec<Element> = model
        .orders
        .iter()
        .map(|row: &OrderRow| {
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
        })
        .collect();

    let refusals: Vec<Element> = model
        .refusals
        .iter()
        .map(|reason| li().text(reason))
        .collect();

    section()
        .child(h2().text("Orders"))
        .child(if orders.is_empty() {
            empty("No orders have been sent.")
        } else {
            table()
                .child(
                    thead().child(
                        tr().child(th().text("Id"))
                            .child(th().text("Instrument"))
                            .child(th().text("Side"))
                            .child(th().text("Quantity"))
                            .child(th().text("State"))
                            .child(th().text("Venue")),
                    ),
                )
                .child(tbody().children(orders))
        })
        .child(h3().text("Refusals"))
        .child(if refusals.is_empty() {
            p().class("muted").text("Nothing has been refused.")
        } else {
            ul().children(refusals)
        })
}

fn agents(model: &ViewModel) -> Element {
    let rows: Vec<Element> = model
        .agents
        .iter()
        .map(|row: &AgentRow| {
            tr().child(td().class("mono").text(&row.id))
                .child(td().text(&row.name))
                .child(td().text(&row.role))
                .child(td().class("muted").text(&row.owner))
                .child(td().class("mono").text(row.capabilities.len().to_string()))
                .child(td().class("muted").text(&row.purpose))
        })
        .collect();

    section().child(h2().text("The organisation")).child(
        table()
            .child(
                thead().child(
                    tr().child(th().text("Id"))
                        .child(th().text("Name"))
                        .child(th().text("Role"))
                        .child(th().text("Owner"))
                        .child(th().text("Grants"))
                        .child(th().text("Purpose")),
                ),
            )
            .child(tbody().children(rows)),
    )
}

fn governance(model: &ViewModel) -> Element {
    let findings: Vec<Element> = model
        .governance
        .iter()
        .map(|finding| {
            li().child(
                span()
                    .class(if finding.severity == "error" {
                        "pill bad"
                    } else {
                        "pill warn"
                    })
                    .text(&finding.severity),
            )
            .text(format!(" {} — {}", finding.rule, finding.detail))
        })
        .collect();

    section()
        .child(h2().text("Governance"))
        .child(cards(&[
            ("Agents", model.agents.len().to_string()),
            ("Findings", model.governance.len().to_string()),
            ("Ceiling", model.autonomy_ceiling.clone()),
        ]))
        .child(h3().text("Roster review"))
        .child(if findings.is_empty() {
            p().class("muted")
                .text("The roster passes every governance rule.")
        } else {
            ul().children(findings)
        })
}

fn audit(model: &ViewModel) -> Element {
    let proposals: Vec<Element> = model
        .proposals
        .iter()
        .map(|row: &ProposalRow| {
            tr().child(td().class("mono").text(&row.id))
                .child(td().text(&row.status))
                .child(td().class("mono").text(row.legs.to_string()))
                .child(td().class("muted").text(&row.rationale))
        })
        .collect();

    section()
        .child(h2().text("Audit"))
        .child(
            p().text(format!(
                "Every event in a cycle carries one correlation id, so a decision can be reconstructed from the log by a single key. The last cycle's key was {}.",
                model.correlation_id
            )),
        )
        .child(h3().text("Proposals"))
        .child(if proposals.is_empty() {
            empty("No proposal has been made.")
        } else {
            table()
                .child(
                    thead().child(
                        tr().child(th().text("Id"))
                            .child(th().text("Status"))
                            .child(th().text("Legs"))
                            .child(th().text("Rationale")),
                    ),
                )
                .child(tbody().children(proposals))
        })
        .child(h3().text("Event log"))
        .child(
            p().class("muted").child(code().text(format!(
                "{} record(s), hash chain {}",
                model.events_logged,
                if model.chain_intact {
                    "intact"
                } else {
                    "BROKEN"
                }
            ))),
        )
}
