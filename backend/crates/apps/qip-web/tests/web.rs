//! Tests for the operator interface.
//!
//! Two things are worth defending: that nothing user-supplied can carry markup
//! into a page, and that the banner never misstates whether real money is
//! moving.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_web::html::{div, escape, p};
use qip_web::pages::{Surface, render};
use qip_web::view::{AgentRow, OpportunityRow, OrderRow, ViewModel};

// --- escaping ---------------------------------------------------------------

#[test]
fn text_is_escaped_on_the_way_in() {
    let dangerous = "<script>alert('x')</script>";
    let rendered = p().text(dangerous).render();
    assert!(!rendered.contains("<script>"), "{rendered}");
    assert!(rendered.contains("&lt;script&gt;"), "{rendered}");
}

#[test]
fn attribute_values_are_escaped_too() {
    // An unescaped quote in an attribute closes it and everything after is
    // markup.
    let rendered = div().attr("class", r#"x" onload="alert(1)"#).render();
    assert!(!rendered.contains("onload=\""), "{rendered}");
    assert!(rendered.contains("&quot;"), "{rendered}");
}

#[test]
fn every_dangerous_character_is_escaped() {
    // The property is that no *unescaped* delimiter survives. `&` is expected
    // in the output, since every entity reference starts with one; what must
    // not survive is an `&` that is not the start of an entity.
    let escaped = escape("<>&\"'/");
    assert_eq!(escaped, "&lt;&gt;&amp;&quot;&#39;&#47;");
    for c in ['<', '>', '"', '\'', '/'] {
        assert!(!escaped.contains(c), "{c} survived escaping: {escaped}");
    }
    // Every `&` in the output opens an entity reference.
    for fragment in escaped.split('&').skip(1) {
        assert!(
            fragment.contains(';'),
            "a bare ampersand survived: {escaped}"
        );
    }
}

#[test]
fn there_is_no_way_to_insert_raw_markup() {
    // The property, stated as a test: every path into a page goes through
    // `text`, which escapes. Anything a caller supplies comes out as content.
    //
    // The check is on the *delimiters*, not on the words. Escaped text may
    // legitimately contain "onerror" or "script" as content — that is exactly
    // what escaping is for — and asserting on the words would pass for the
    // wrong reason on a page that happened not to mention them.
    let attempts = [
        "</p><script>alert(1)</script><p>",
        "\"><img src=x onerror=alert(1)>",
        "javascript:alert(1)",
        "<!--<script>-->",
    ];
    for attempt in attempts {
        let rendered = div().text(attempt).render();
        let inner = rendered
            .strip_prefix("<div>")
            .and_then(|s| s.strip_suffix("</div>"))
            .expect("the wrapper is the only markup");
        for delimiter in ['<', '>', '"', '\''] {
            assert!(
                !inner.contains(delimiter),
                "{attempt} left a bare {delimiter}: {inner}"
            );
        }
    }
}

// --- the banner -------------------------------------------------------------

#[test]
fn a_default_model_renders_a_paper_trading_banner() {
    // The safe reading, and the one a page rendered before the platform
    // reported anything must show.
    let page = render(Surface::Overview, &ViewModel::default());
    assert!(page.contains("PAPER TRADING"), "{page}");
    assert!(!page.contains("LIVE TRADING"));
    assert!(!page.contains("HALTED"));
}

#[test]
fn a_live_platform_says_so_unambiguously() {
    let model = ViewModel {
        live: true,
        autonomy_level: "supervised_live".to_string(),
        ..ViewModel::default()
    };
    let page = render(Surface::Overview, &model);
    assert!(page.contains("LIVE TRADING"), "{page}");
    assert!(page.contains("Every fill is a real fill"));
    assert!(!page.contains("PAPER TRADING"));
}

#[test]
fn a_halt_overrides_everything_else_in_the_banner() {
    // A halted live platform is halted; showing it as live would be worse than
    // showing nothing.
    let model = ViewModel {
        live: true,
        halted: true,
        halt_reason: "drawdown limit breached".to_string(),
        ..ViewModel::default()
    };
    let page = render(Surface::Overview, &model);
    assert!(page.contains("HALTED"), "{page}");
    assert!(page.contains("drawdown limit breached"));
    assert!(!page.contains("LIVE TRADING"));
}

#[test]
fn a_halted_paper_platform_still_says_paper_trading_on_every_surface() {
    // The halted banner once replaced the posture label instead of adding to
    // it, so a halted simulator and a halted live book rendered the same
    // page. The halt is shown, and so is the fact that no order could have
    // reached a market before it.
    let model = ViewModel {
        halted: true,
        halt_reason: "drawdown limit breached".to_string(),
        ..ViewModel::default()
    };
    assert!(!model.live, "the premise is a halted paper platform");
    for surface in Surface::all() {
        let page = render(surface, &model);
        assert!(page.contains("HALTED"), "{} lost the halt", surface.title());
        assert!(
            page.contains("PAPER TRADING"),
            "{} shows a halted platform without its posture",
            surface.title()
        );
        assert!(!page.contains("LIVE TRADING"), "{page}");
    }

    // A halted live platform is halted, not paper: the label must not leak
    // onto a book whose fills were real.
    let live = ViewModel {
        live: true,
        halted: true,
        halt_reason: "drawdown limit breached".to_string(),
        ..ViewModel::default()
    };
    let page = render(Surface::Overview, &live);
    assert!(page.contains("HALTED"), "{page}");
    assert!(!page.contains("PAPER TRADING"), "{page}");
    assert!(!page.contains("LIVE TRADING"), "{page}");
}

#[test]
fn the_banner_appears_on_every_surface() {
    let model = ViewModel::default();
    for surface in Surface::all() {
        let page = render(surface, &model);
        assert!(
            page.contains("PAPER TRADING"),
            "{} has no banner",
            surface.title()
        );
    }
}

// --- the stylesheet -----------------------------------------------------------

#[test]
fn the_inlined_stylesheet_reaches_the_page_as_css_rather_than_as_escaped_text() {
    // `<style>` is a "raw text" element in the HTML5 parsing model: a browser
    // never decodes an entity reference inside one. Routing the stylesheet
    // through the same escaping path as page content shipped broken CSS on
    // every surface — a quoted font-family became the literal characters
    // `&quot;SF Mono&quot;`, and every `/* ... */` comment became
    // `&#47;* ... *&#47;`, which is not a comment to a CSS parser.
    let page = render(Surface::Overview, &ViewModel::default());
    assert!(
        page.contains("\"SF Mono\""),
        "the quoted font name was HTML-escaped: {page}"
    );
    assert!(
        page.contains("/* A panel with nothing behind it."),
        "a CSS comment delimiter was HTML-escaped: {page}"
    );
    assert!(
        !page.contains("&quot;SF Mono&quot;"),
        "the stylesheet is still escaped: {page}"
    );
    assert!(
        !page.contains("&#47;*"),
        "a CSS comment delimiter is still escaped: {page}"
    );
}

// --- the surfaces -----------------------------------------------------------

#[test]
fn there_are_nine_surfaces_and_each_renders() {
    assert_eq!(Surface::all().len(), 9);
    let model = ViewModel::default();
    for surface in Surface::all() {
        let page = render(surface, &model);
        assert!(page.starts_with("<!DOCTYPE html>"));
        assert!(page.contains(surface.title()));
        assert!(page.contains("</html>"));
    }
}

#[test]
fn no_page_contains_a_script_element() {
    // The decision, enforced. The content-security policy forbids script
    // entirely, so a page containing one would silently not work.
    let model = ViewModel {
        opportunities: vec![OpportunityRow {
            id: "opp-1".to_string(),
            headline: "a headline".to_string(),
            score: 0.8,
            confidence: 0.7,
            detectors: vec!["return-anomaly".to_string()],
        }],
        agents: vec![AgentRow {
            id: "macro-analyst".to_string(),
            name: "Macro Analyst".to_string(),
            role: "research".to_string(),
            owner: "investment-research".to_string(),
            purpose: "reads the world".to_string(),
            capabilities: vec!["read_market_data".to_string()],
        }],
        ..ViewModel::default()
    };
    for surface in Surface::all() {
        let page = render(surface, &model);
        assert!(
            !page.contains("<script"),
            "{} contains a script element",
            surface.title()
        );
        assert!(
            !page.contains("javascript:"),
            "{} contains a javascript URL",
            surface.title()
        );
        assert!(
            !page.contains(" on") || !page.contains("=\"alert"),
            "{} may contain an inline handler",
            surface.title()
        );
    }
}

#[test]
fn an_empty_surface_explains_itself_rather_than_showing_a_blank_table() {
    // "Nothing happened" and "nothing was attempted" are different, and a
    // blank table says neither.
    let model = ViewModel::default();
    let page = render(Surface::Opportunities, &model);
    assert!(page.contains("Nothing is queued"), "{page}");

    let page = render(Surface::Theses, &model);
    assert!(page.contains("No thesis has been formed"), "{page}");
}

#[test]
fn an_order_row_states_whether_it_was_real() {
    // A reader glancing at a blotter should never have to work it out.
    let model = ViewModel {
        orders: vec![
            OrderRow {
                id: "ord-1".to_string(),
                instrument: "obj-AAA".to_string(),
                side: "buy".to_string(),
                quantity: "1000".to_string(),
                state: "filled".to_string(),
                simulated: true,
            },
            OrderRow {
                id: "ord-2".to_string(),
                instrument: "obj-BBB".to_string(),
                side: "sell".to_string(),
                quantity: "500".to_string(),
                state: "filled".to_string(),
                simulated: false,
            },
        ],
        ..ViewModel::default()
    };
    let page = render(Surface::Execution, &model);
    assert!(page.contains(">paper<"), "{page}");
    assert!(page.contains(">live<"), "{page}");
}

#[test]
fn a_headline_from_outside_cannot_carry_markup_into_a_page() {
    // The end-to-end version of the escaping tests: data that came from a
    // detector, rendered through a real surface.
    let model = ViewModel {
        opportunities: vec![OpportunityRow {
            id: "opp-1".to_string(),
            headline: "<img src=x onerror=alert(1)>".to_string(),
            score: 0.8,
            confidence: 0.7,
            detectors: vec!["<script>".to_string()],
        }],
        ..ViewModel::default()
    };
    let page = render(Surface::Opportunities, &model);
    assert!(!page.contains("<img"), "{page}");
    assert!(!page.contains("<script>"), "{page}");
    assert!(
        page.contains("&lt;img"),
        "the content is still shown, escaped"
    );
}

#[test]
fn surfaces_round_trip_through_their_paths() {
    for surface in Surface::all() {
        assert_eq!(Surface::from_path(surface.path()), Some(surface));
    }
    assert_eq!(Surface::from_path("/nonexistent"), None);
}

// --- facts the platform did not record --------------------------------------

/// A model in which every new panel carries a value, so the banner tests
/// below render pages that show a platform fact rather than an empty page.
fn with_platform_facts() -> ViewModel {
    use qip_web::panel::Panel;
    use qip_web::view::{
        EdgeCellRow, Fact, FactRow, ShippedPolicyRow, UniverseExclusionRow, UniverseView,
    };
    let as_of = "2025-10-09T00:00:00Z";
    ViewModel {
        cells: Panel::current(
            vec![EdgeCellRow {
                cell: "eu-west".to_string(),
                reported_at: as_of.to_string(),
                age: "0s".to_string(),
                stale: false,
                positions: 0,
                strategies: 1,
                breaks_shipped: 0,
                orders_sent: Fact::not_recorded("kept nowhere"),
                fills_confirmed: Fact::not_recorded("kept nowhere"),
                halted_by_centre: false,
                policy_halt_flag: false,
                cell_reports_halted: Fact::recorded("no (delta 4)"),
                polled_halt_flag: Fact::not_recorded("stays on the node"),
            }],
            as_of,
        ),
        settlement: vec![FactRow::new(
            "central_orders_sent",
            "Orders sent",
            Fact::recorded("3"),
        )],
        shipped_policy: Panel::current(
            vec![ShippedPolicyRow {
                cell: "eu-west".to_string(),
                issued_at: as_of.to_string(),
                sequence: Fact::not_recorded("not journaled"),
                whitelist: "cycle whitelist for eu-west: empty, CentralConfig::arbitrage is unset"
                    .to_string(),
                slots: vec![FactRow::new(
                    "cycle_whitelist",
                    "cycle_whitelist",
                    Fact::recorded("produced"),
                )],
            }],
            as_of,
        ),
        universe: UniverseView {
            version: Fact::not_recorded("not on the platform"),
            sha256: Fact::not_recorded("not on the platform"),
            instruments: Fact::not_recorded("not on the platform"),
            not_decision_grade: Panel::current(
                vec![UniverseExclusionRow {
                    object: "obj-AAA".to_string(),
                    reason: "licensing class Synthetic is not production-eligible".to_string(),
                }],
                as_of,
            ),
        },
        ..ViewModel::default()
    }
}

#[test]
fn a_figure_the_platform_did_not_record_renders_as_not_recorded_and_never_as_zero() {
    // The scrape surface's rule, on HTML. A default model has no platform
    // behind it, so every new figure must say so; a `0` anywhere in one of
    // these cells would be a claim about a platform that made none.
    let model = ViewModel::default();
    assert!(
        !model.universe.version.is_recorded(),
        "the premise is a model nothing reported"
    );
    assert!(model.cells.is_absent() && model.shipped_policy.is_absent());

    let overview = render(Surface::Overview, &model);
    for key in [
        "universe.version",
        "universe.sha256",
        "universe.instruments",
    ] {
        assert!(
            overview.contains(&format!(
                r#"data-fact="{key}" data-state="not-recorded">not recorded<"#
            )),
            "{key} is not rendered as not recorded: {overview}"
        );
        assert!(
            !overview.contains(&format!(r#"data-fact="{key}" data-state="recorded">0<"#)),
            "{key} rendered a zero the platform never recorded"
        );
    }
    // The count of not-decision-grade instruments is a panel, and an absent
    // panel is not a zero either.
    assert!(
        overview.contains(r#"data-fact="universe.not_decision_grade" data-state="not-recorded">"#),
        "{overview}"
    );
    let execution = render(Surface::Execution, &model);
    assert!(
        execution.contains(r#"data-panel="Cells" data-state="absent""#),
        "{execution}"
    );
    assert!(execution.contains("Not recorded."), "{execution}");
    assert!(!execution.contains("<table"), "{execution}");

    // And the same figures, once recorded, render the recorded value and not
    // the placeholder — the renderer distinguishes the two arms.
    let page = render(Surface::Execution, &with_platform_facts());
    assert!(
        page.contains(r#"data-fact="central_orders_sent" data-state="recorded">3<"#),
        "{page}"
    );
    assert!(
        page.contains(
            r#"data-fact="cell.eu-west.orders_sent" data-state="not-recorded">not recorded<"#
        ),
        "{page}"
    );
}

#[test]
fn every_surface_that_renders_a_platform_fact_still_says_paper_trading() {
    // The three surfaces that gained platform facts, each rendered with a
    // fact on it, each still carrying the posture as its own delimited token.
    // `>PAPER TRADING<` rather than the bare words, so a mention inside a
    // reason or a rationale cannot satisfy the test on the banner's behalf.
    let model = with_platform_facts();
    assert!(!model.live, "the premise is a paper platform");
    assert!(!model.cells.is_absent(), "the premise is a rendered cell");
    for surface in [Surface::Overview, Surface::Execution, Surface::Governance] {
        let page = render(surface, &model);
        let fact_rendered = match surface {
            Surface::Overview => page.contains("obj-AAA"),
            Surface::Execution => page.contains("eu-west"),
            _ => page.contains("cycle whitelist for eu-west"),
        };
        assert!(fact_rendered, "{} shows no platform fact", surface.title());
        assert!(
            page.contains(">PAPER TRADING<"),
            "{} renders a platform fact without its posture",
            surface.title()
        );
    }
}
