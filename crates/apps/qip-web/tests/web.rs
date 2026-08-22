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
