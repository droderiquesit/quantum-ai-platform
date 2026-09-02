//! Tests for the operator console.
//!
//! Three properties are worth defending, and they are the three rules the
//! console is built around:
//!
//! * A view never invents a number. Where nothing has reported, the page says
//!   so; where something reported nothing, the page says *that* instead, and
//!   the two are different markup.
//! * Nothing in the console can act, except tripping the kill switch. There is
//!   no path that clears one.
//! * Every value that reaches a page is escaped, because [`Element::text`] is
//!   the only way in.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_web::console::model::{
    AgentCallRow, AlphaRow, ArbitrageRow, CapitalRow, CellRow, ConsoleModel, ExposureRow, FillRow,
    KillSwitchState, Metric, ModelRow, QuantumRow, RefusalRow, ServiceRow, SourceRow, StrategyRow,
};
use qip_web::console::{TRIP_PATH, View, render};
use qip_web::escape;
use qip_web::panel::Panel;
use qip_web::view::{OpportunityRow, OrderRow, Posture};

/// Every `action="..."` value in a page, decoded back from its escaped form.
///
/// The escaper turns `/` into `&#47;`, which browsers decode and a naive
/// substring assertion would miss, so the test decodes rather than guessing.
fn form_actions(page: &str) -> Vec<String> {
    let mut actions = Vec::new();
    let mut rest = page;
    while let Some(index) = rest.find("action=\"") {
        let after = &rest[index + 8..];
        let Some(end) = after.find('"') else { break };
        actions.push(after[..end].replace("&#47;", "/"));
        rest = &after[end..];
    }
    actions
}

/// A model in which every panel has reported something, and several of the
/// reported values are hostile.
fn populated() -> ConsoleModel {
    let as_of = "2025-10-09T00:00:00Z";
    ConsoleModel {
        posture: Posture::default(),
        rendered_at: as_of.to_string(),
        cycle: 7,
        events_logged: 42,
        chain_intact: true,

        regions: Panel::current(
            vec![CellRow {
                cell: "eu-west".to_string(),
                status: "reporting".to_string(),
                reported_at: as_of.to_string(),
                age: "2s".to_string(),
                positions: 3,
                gross: "1,000".to_string(),
                net: "250".to_string(),
                strategies: 1,
                reconciliation_breaks: 0,
                halted: false,
            }],
            as_of,
        ),
        market_state: Panel::current(vec![Metric::new("Venues open", "3")], as_of),
        opportunities: Panel::current(
            vec![OpportunityRow {
                id: "opp-1".to_string(),
                headline: "a headline".to_string(),
                score: 0.8,
                confidence: 0.7,
                detectors: vec!["return-anomaly".to_string()],
            }],
            as_of,
        ),
        strategies: Panel::current(
            vec![StrategyRow {
                // A strategy id carrying a script element.
                id: "<script>alert(1)</script>".to_string(),
                cell: "eu-west".to_string(),
                // A venue name carrying a quote.
                venue: "XNYS\" onmouseover=\"alert(1)".to_string(),
                stage: "pilot".to_string(),
                holds_capital: true,
                registered_at: as_of.to_string(),
            }],
            as_of,
        ),
        capital_distribution: Panel::current(
            vec![CapitalRow {
                subject: "eu-west/strat-1".to_string(),
                cell: "eu-west".to_string(),
                strategy: "strat-1".to_string(),
                granted: "100,000".to_string(),
                used: "10,000".to_string(),
                utilisation: "10.0%".to_string(),
                expires_at: as_of.to_string(),
            }],
            as_of,
        ),
        system_health: Panel::current(
            vec![ServiceRow {
                name: "event-log".to_string(),
                state: "ok".to_string(),
                detail: "hash chain intact".to_string(),
            }],
            as_of,
        ),

        cell_brains: Panel::current(vec![Metric::new("Fast brain", "idle")], as_of),
        local_opportunities: Panel::current(Vec::new(), as_of),
        cell_latency: Panel::current(vec![Metric::new("p99", "120us")], as_of),
        brokers: Panel::current(
            vec![ServiceRow {
                name: "simulated".to_string(),
                state: "ok".to_string(),
                detail: String::new(),
            }],
            as_of,
        ),
        venues: Panel::current(
            vec![ServiceRow {
                name: "XNYS".to_string(),
                state: "ok".to_string(),
                detail: String::new(),
            }],
            as_of,
        ),
        inventory: Panel::current(
            vec![ExposureRow {
                axis: "instrument".to_string(),
                bucket: "obj-AAA".to_string(),
                gross: "1,000".to_string(),
                net: "1,000".to_string(),
                share: 1.0,
                limit: 0.1,
                breached: true,
            }],
            as_of,
        ),
        cash: Panel::current(vec![Metric::new("Cash", "10,000,000")], as_of),
        cell_models: Panel::current(
            vec![ModelRow {
                id: "deterministic".to_string(),
                purpose: "narration".to_string(),
                calls: "3".to_string(),
                tokens: "120".to_string(),
                cost: "$0.00".to_string(),
                status: "ok".to_string(),
            }],
            as_of,
        ),

        positions: Panel::current(Vec::new(), as_of),
        pending_orders: Panel::current(
            vec![OrderRow {
                id: "ord-1".to_string(),
                instrument: "obj-AAA".to_string(),
                side: "buy".to_string(),
                quantity: "100".to_string(),
                state: "working".to_string(),
                simulated: true,
            }],
            as_of,
        ),
        fills: Panel::current(
            vec![FillRow {
                order: "ord-1".to_string(),
                instrument: "obj-AAA".to_string(),
                side: "buy".to_string(),
                quantity: "100".to_string(),
                price: "100".to_string(),
                venue: "simulated".to_string(),
                simulated: true,
            }],
            as_of,
        ),
        pnl: Panel::current(vec![Metric::new("Realised", "0")], as_of),
        alpha: Panel::current(
            vec![AlphaRow {
                subject: "strat-1".to_string(),
                expected: "12bp".to_string(),
                realised: "8bp".to_string(),
                difference: "-4bp".to_string(),
            }],
            as_of,
        ),
        refusals: Panel::current(
            vec![
                RefusalRow {
                    order: "ord-2".to_string(),
                    at: as_of.to_string(),
                    kind: "safety control".to_string(),
                    reason: "the autonomy level is paper_trading, and execution needs at \
                             least supervised_live"
                        .to_string(),
                },
                RefusalRow {
                    order: "ord-3".to_string(),
                    at: as_of.to_string(),
                    kind: "fault".to_string(),
                    reason: "XNYS is unavailable: no session".to_string(),
                },
            ],
            as_of,
        ),

        three_arm: Panel::current(
            vec![ArbitrageRow {
                id: "arb-1".to_string(),
                shape: "three-arm".to_string(),
                legs: 3,
                capital_required: "50,000".to_string(),
                fill_state: "partial".to_string(),
                hedge_state: "pending".to_string(),
            }],
            as_of,
        ),
        n_leg: Panel::current(Vec::new(), as_of),
        arbitrage_capital: Panel::current(vec![Metric::new("Required", "50,000")], as_of),

        models: Panel::current(
            vec![ModelRow {
                id: "deterministic".to_string(),
                purpose: "narration".to_string(),
                calls: "3".to_string(),
                tokens: "120".to_string(),
                cost: "$0.00".to_string(),
                status: "ok".to_string(),
            }],
            as_of,
        ),
        model_reputation: Panel::current(vec![Metric::new("Contexts scored", "0")], as_of),
        agent_calls: Panel::current(
            vec![AgentCallRow {
                agent: "macro-analyst".to_string(),
                run: "run-1".to_string(),
                status: "succeeded".to_string(),
                tool_calls: 4,
                model_calls: 1,
                tokens: 120,
                cost: "$0.00".to_string(),
                utilisation: 0.12,
                conviction: None,
            }],
            as_of,
        ),
        training: Panel::current(vec![Metric::new("Runs", "0")], as_of),

        quantum_jobs: Panel::current(
            vec![QuantumRow {
                job: "job-1".to_string(),
                solver: "qaoa".to_string(),
                runtime: "12ms".to_string(),
                result: "0.91".to_string(),
                classical_solver: "greedy".to_string(),
                classical_runtime: "1ms".to_string(),
                classical_result: "0.90".to_string(),
                verdict: "no advantage".to_string(),
            }],
            as_of,
        ),
        quantum_routing: Panel::current(vec![Metric::new("Provider", "none")], as_of),

        sources: Panel::current(
            vec![SourceRow {
                // A source name carrying markup.
                name: "<img src=x onerror=alert(1)>".to_string(),
                state: "rejected".to_string(),
                health: "down".to_string(),
                freshness: "never".to_string(),
                cost: "$0".to_string(),
                licence: "unlicensed".to_string(),
            }],
            as_of,
        ),
        source_health: Panel::current(vec![Metric::new("Registered", "1")], as_of),

        limits: Panel::current(Vec::new(), as_of),
        exposure: Panel::current(Vec::new(), as_of),
        tail_risk: Panel::current(vec![Metric::new("Drawdown", "0.0%")], as_of),
        concentration: Panel::current(Vec::new(), as_of),
        regional_limits: Panel::current(Vec::new(), as_of),
        kill_switch: KillSwitchState::default(),

        services: Panel::current(Vec::new(), as_of),
        transports: Panel::current(Vec::new(), as_of),
        clusters: Panel::current(Vec::new(), as_of),
        model_health: Panel::current(Vec::new(), as_of),
        source_outages: Panel::current(Vec::new(), as_of),
        operating_cost: Panel::current(vec![Metric::new("Spend", "$0.00")], as_of),
        governance: Panel::current(Vec::new(), as_of),
    }
}

// --- the console renders at all ---------------------------------------------

#[test]
fn there_are_nine_console_views_and_each_renders_on_a_platform_that_has_reported_nothing() {
    assert_eq!(View::all().len(), 9);
    let model = ConsoleModel::default();
    for view in View::all() {
        let page = render(view, &model);
        assert!(page.starts_with("<!DOCTYPE html>"), "{}", view.title());
        assert!(page.contains("</html>"), "{}", view.title());
        assert!(page.contains(view.title()), "{}", view.title());
    }
}

#[test]
fn console_views_round_trip_through_their_paths_and_do_not_collide_with_the_surfaces() {
    for view in View::all() {
        assert_eq!(View::from_path(view.path()), Some(view));
        assert!(
            view.path().starts_with("/console"),
            "{} would shadow an investment surface",
            view.path()
        );
        assert!(
            qip_web::pages::Surface::from_path(view.path()).is_none(),
            "{} collides with an investment surface",
            view.path()
        );
    }
    assert_eq!(View::from_path("/console/nonexistent"), None);
}

// --- a view never invents a number ------------------------------------------

#[test]
fn a_panel_nobody_reported_defaults_to_absent_rather_than_to_zero() {
    // The default matters more than it looks: a field someone forgets to
    // assemble must not render as an observed zero.
    let panel: Panel<Metric> = Panel::default();
    assert!(panel.is_absent());
    assert!(panel.rows().is_empty());
    assert!(!panel.is_empty_but_reported());
}

#[test]
fn an_absent_panel_carries_no_rows_at_all() {
    // Enforced by the constructor, which takes none: an absent panel has
    // nowhere to put a number, so it cannot render one.
    let panel: Panel<Metric> = Panel::absent("the feed is down");
    assert!(panel.rows().is_empty());
    assert!(panel.is_absent());
}

#[test]
fn every_view_on_an_unreported_platform_says_it_has_no_data_and_renders_no_table() {
    // The property that matters most. A blank table reads as "flat"; these
    // pages must read as "blind".
    let model = ConsoleModel::default();
    for view in View::all() {
        let page = render(view, &model);
        assert!(
            page.contains(r#"data-state="absent""#),
            "{} does not mark any panel absent",
            view.title()
        );
        assert!(
            page.contains("No data."),
            "{} does not say it has no data",
            view.title()
        );
        assert!(
            !page.contains("<table"),
            "{} renders a table of numbers nobody reported",
            view.title()
        );
        assert!(
            page.contains("this panel was not assembled"),
            "{} does not say why it has no data",
            view.title()
        );
    }
}

#[test]
fn an_observed_zero_and_an_unreported_panel_are_different_markup() {
    // "Nothing happened" and "nothing was attempted" are opposites here, and
    // an empty table says neither.
    let reported_empty = ConsoleModel {
        limits: Panel::current(Vec::new(), "2025-10-09T00:00:00Z"),
        ..ConsoleModel::default()
    };
    let page = render(View::Risk, &reported_empty);
    assert!(page.contains(r#"data-state="empty-reported""#), "{page}");
    assert!(page.contains("Reported, and empty."), "{page}");
    assert!(page.contains("This is an observed zero, not a missing feed."));

    let unreported = ConsoleModel {
        limits: Panel::absent("no limit set is attached to this deployment"),
        ..ConsoleModel::default()
    };
    let page = render(View::Risk, &unreported);
    assert!(
        page.contains("no limit set is attached to this deployment"),
        "{page}"
    );
    assert!(!page.contains("Reported, and empty."), "{page}");
}

#[test]
fn a_stale_cell_report_is_shown_as_stale_rather_than_as_the_current_book() {
    // The failure this prevents: a cell that stopped reporting an hour ago,
    // whose last book is still on the screen with no indication it is old.
    let model = ConsoleModel {
        regions: Panel::stale(
            vec![CellRow {
                cell: "eu-west".to_string(),
                status: "stale".to_string(),
                reported_at: "2025-10-09T00:00:00Z".to_string(),
                age: "1h 4m".to_string(),
                positions: 12,
                gross: "4,200,000".to_string(),
                net: "1,100,000".to_string(),
                strategies: 2,
                reconciliation_breaks: 0,
                halted: false,
            }],
            "2025-10-09T00:00:00Z",
            "1h 4m",
            "60s",
        ),
        ..ConsoleModel::default()
    };
    for view in [View::Global, View::Regional] {
        let page = render(view, &model);
        assert!(
            page.contains(r#"data-state="stale""#),
            "{} does not mark the panel stale",
            view.title()
        );
        assert!(page.contains("STALE"), "{}", view.title());
        assert!(
            page.contains("not what is true now"),
            "{} presents the last known book as current",
            view.title()
        );
        assert!(page.contains("1h 4m"), "{}", view.title());
        // The numbers are still shown — hiding them would lose information —
        // but never without the marking.
        assert!(page.contains("4,200,000"), "{}", view.title());
    }
}

#[test]
fn an_agent_that_produced_no_finding_shows_no_conviction_rather_than_zero_conviction() {
    // Zero conviction is a claim. No finding is the absence of one.
    let page = render(View::Ai, &populated());
    assert!(page.contains("no finding"), "{page}");
}

#[test]
fn a_refusal_names_the_order_and_the_reason_rather_than_counting_refusals() {
    // A card reading "Refusals 2" says two orders did not happen and nothing
    // an operator can act on. The panel is a table for that reason, and the
    // two kinds of refusal are never flattened together: a control refusing an
    // order is the platform working, a venue fault is the platform broken, and
    // only one of them may be retried.
    let page = render(View::Trading, &populated());
    assert!(page.contains("ord-2"), "{page}");
    assert!(
        page.contains("execution needs at least supervised_live"),
        "{page}"
    );
    assert!(page.contains("safety control"), "{page}");
    assert!(page.contains("XNYS is unavailable"), "{page}");
    assert!(page.contains("fault"), "{page}");
}

#[test]
fn a_platform_that_refused_nothing_says_so_and_a_silent_one_does_not() {
    // The distinction the whole panel type exists for, on this panel.
    let reported = ConsoleModel {
        refusals: Panel::current(Vec::new(), "2025-10-09T00:00:00Z"),
        ..ConsoleModel::default()
    };
    let page = render(View::Trading, &reported);
    assert!(page.contains("Reported, and empty."), "{page}");

    let unreported = ConsoleModel::default();
    let page = render(View::Trading, &unreported);
    assert!(!page.contains("Reported, and empty."), "{page}");
    assert!(page.contains("No data."), "{page}");
}

// --- escaping ---------------------------------------------------------------

#[test]
fn markup_in_a_source_name_a_venue_name_and_a_strategy_id_all_render_inert() {
    let model = populated();
    // A strategy id containing a script element, and a venue name containing a
    // quote, both on the Global view.
    let page = render(View::Global, &model);
    assert!(!page.contains("<script>alert"), "{page}");
    assert!(
        page.contains("&lt;script&gt;"),
        "the id is still shown, escaped"
    );
    // The quote is what would close the attribute and start a handler.
    assert!(!page.contains("onmouseover=\"alert"), "{page}");
    assert!(page.contains("&quot;"), "{page}");

    // A source name containing an image tag with a handler, on Data Finder.
    let page = render(View::DataFinder, &model);
    assert!(!page.contains("<img"), "{page}");
    assert!(
        page.contains("&lt;img src=x onerror=alert(1)&gt;"),
        "the name is still shown, escaped: {page}"
    );
}

#[test]
fn no_console_view_emits_a_script_element_or_any_off_origin_reference() {
    // The content-security policy is `default-src 'none'` with no script
    // source at all, so a page containing script would silently not work, and
    // a page referencing an external asset would silently not load it.
    for model in [ConsoleModel::default(), populated()] {
        for view in View::all() {
            let page = render(view, &model);
            assert!(
                !page.contains("<script"),
                "{} contains a script element",
                view.title()
            );
            assert!(
                !page.contains("<link"),
                "{} links an external stylesheet",
                view.title()
            );
            assert!(!page.contains("<img"), "{} loads an image", view.title());
            assert!(
                !page.contains("src=\"http"),
                "{} loads something off-origin",
                view.title()
            );
            assert!(
                !page.contains("href=\"http"),
                "{} references something off-origin",
                view.title()
            );
            // An inline handler is script by another name.
            for handler in ["onload=\"", "onerror=\"", "onclick=\"", "onmouseover=\""] {
                assert!(
                    !page.contains(handler),
                    "{} carries an inline {handler} handler",
                    view.title()
                );
            }
        }
    }
}

// --- nothing can act, except one thing --------------------------------------

#[test]
fn the_risk_view_can_trip_the_kill_switch() {
    // Tripping needs no authority beyond noticing something wrong, which is
    // exactly what an operator watching this screen has.
    let page = render(View::Risk, &ConsoleModel::default());
    assert_eq!(form_actions(&page), vec![TRIP_PATH.to_string()], "{page}");
    assert!(page.contains("Trip the kill switch"), "{page}");
    assert!(page.contains(&escape(TRIP_PATH)), "{page}");
}

#[test]
fn the_console_offers_no_way_to_clear_a_kill_switch_in_either_state() {
    // Clearing requires an operator credential verified minutes ago, which a
    // page cannot establish. So there is no control — not a disabled one, not
    // a hidden one, none.
    let running = ConsoleModel::default();
    let halted = ConsoleModel {
        kill_switch: KillSwitchState {
            halted: true,
            halted_scopes: vec!["eu-west".to_string()],
            tripped_by: "central-plane:reconciliation".to_string(),
            tripped_at: "2025-10-09T00:00:00Z".to_string(),
            reason: "a book that does not reconcile".to_string(),
            clearances: 0,
        },
        ..ConsoleModel::default()
    };

    for model in [running, halted] {
        for view in View::all() {
            let page = render(view, &model);
            for action in form_actions(&page) {
                assert_eq!(
                    action,
                    TRIP_PATH,
                    "{} submits to something other than the trip path",
                    view.title()
                );
            }
            for forbidden in ["clear", "resume", "restart", "unhalt"] {
                assert!(
                    !form_actions(&page).iter().any(|a| a.contains(forbidden)),
                    "{} offers a {forbidden} control",
                    view.title()
                );
            }
        }
    }
}

#[test]
fn a_halted_console_offers_no_control_at_all_and_says_where_lifting_happens() {
    let model = ConsoleModel {
        kill_switch: KillSwitchState {
            halted: true,
            halted_scopes: Vec::new(),
            tripped_by: "risk-monitor".to_string(),
            tripped_at: "2025-10-09T00:00:00Z".to_string(),
            reason: "drawdown limit breached".to_string(),
            clearances: 2,
        },
        ..ConsoleModel::default()
    };
    let page = render(View::Risk, &model);
    assert!(
        form_actions(&page).is_empty(),
        "a halted console has no form"
    );
    assert!(page.contains("cannot lift the halt"), "{page}");
    assert!(page.contains("drawdown limit breached"), "{page}");
    assert!(page.contains("2 halt(s) have been lifted"), "{page}");
}

// --- the banner -------------------------------------------------------------

#[test]
fn every_console_view_states_whether_real_money_is_moving() {
    let paper = ConsoleModel::default();
    for view in View::all() {
        assert!(
            render(view, &paper).contains("PAPER TRADING"),
            "{} has no banner",
            view.title()
        );
    }

    let live = ConsoleModel {
        posture: Posture {
            autonomy_level: "supervised_live".to_string(),
            autonomy_ceiling: "supervised_live".to_string(),
            live: true,
            halted: false,
            halt_reason: String::new(),
        },
        ..ConsoleModel::default()
    };
    let page = render(View::Trading, &live);
    assert!(page.contains("LIVE TRADING"), "{page}");
    assert!(!page.contains("PAPER TRADING"), "{page}");
}

#[test]
fn a_halted_console_still_states_that_it_is_paper_trading() {
    // The console shares the banner with the surfaces, so the same regression
    // is checked on its own views: a halted paper console says both.
    let halted = ConsoleModel {
        posture: Posture {
            halted: true,
            halt_reason: "a book that does not reconcile".to_string(),
            ..Posture::default()
        },
        ..ConsoleModel::default()
    };
    assert!(
        !halted.posture.live,
        "the premise is a halted paper console"
    );
    for view in View::all() {
        let page = render(view, &halted);
        assert!(page.contains("HALTED"), "{} lost the halt", view.title());
        assert!(
            page.contains("PAPER TRADING"),
            "{} shows a halted console without its posture",
            view.title()
        );
    }
}

#[test]
fn a_quantum_result_is_never_shown_without_the_classical_run_beside_it() {
    let page = render(View::Quantum, &populated());
    assert!(page.contains("Classical solver"), "{page}");
    assert!(page.contains("Classical result"), "{page}");
    assert!(page.contains("no advantage"), "{page}");
}
