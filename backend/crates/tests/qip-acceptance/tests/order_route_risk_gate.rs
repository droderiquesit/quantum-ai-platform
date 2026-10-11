//! Every place an order reaches a broker or a placer, enumerated, and each one
//! shown to pass the deterministic Risk Gate (W1.9, R12-RISK-4093).
//!
//! The desk path ran `PreTradeChecker::check` before `Broker::submit`; the
//! edge cell's `Cell::send` called a `Placer` after envelope and feasibility
//! gates only. Nothing listed the call sites, so a new route to a venue could
//! be written beside the gated ones and every suite would stay green. This
//! file walks the production source of every crate, finds each call that hands
//! an order to a venue, and requires it to be one of the rows below — so a new
//! call site fails here until somebody decides, in this table, how it is gated.
//!
//! A site is one of:
//!
//! * **Gated** — the gate's own call appears in the same function, before the
//!   venue call. Asserted textually here; asserted behaviourally by
//!   `qip-risk-engine/tests/risk_engine.rs` and `qip-edge/tests/risk_gate.rs`,
//!   which drive a breaching order and watch the venue not be called.
//! * **Delegation** — the call is inside a `place`/`try_place`/`submit`
//!   implementation that forwards to an inner venue. It is reached only
//!   through a gated call, which is what makes it not a second route.
//! * **Release** — a gateway releasing an order it accepted through a gated
//!   `place` and held for its release instant (ADR 0084).
//! * **Replacement** — a requote re-sending a cancelled order's remainder.
//!   **This one does not pass the cell's Risk Gate**, and the row says so
//!   rather than hiding it: `qip-edge-node`'s repricer cancels a resting order
//!   and places its acknowledged remainder at a new price directly on the
//!   gateway. It cannot add quantity — the replacement is minted from the
//!   remainder the venue acknowledged withdrawing, which this test checks is
//!   cancelled first — but a new price is a new notional, and the gate never
//!   sees it. Closing it belongs to the lane that owns `qip-edge-node`; the row
//!   is here so the gap is a fact in a test, not a sentence in a report.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// `gate` appears in the enclosing function before the venue call.
    Gated {
        gate: &'static str,
    },
    Delegation,
    Release,
    /// `withdrawn_first` appears in the enclosing function before the call.
    Replacement {
        withdrawn_first: &'static str,
    },
}

/// `(path under backend/crates, enclosing function, how it is gated)`.
const SITES: &[(&str, &str, Route)] = &[
    (
        "edge/qip-edge/src/cell.rs",
        "send",
        Route::Gated {
            gate: "gate.check(",
        },
    ),
    ("edge/qip-edge/src/cell.rs", "try_place", Route::Delegation),
    (
        "services/qip-execution-engine/src/oms.rs",
        "submit",
        Route::Gated {
            gate: "self.checker.check(",
        },
    ),
    (
        "apps/qip-edge-node/src/gateway.rs",
        "submit_now",
        Route::Release,
    ),
    (
        "apps/qip-edge-node/src/gateway.rs",
        "place",
        Route::Delegation,
    ),
    (
        "apps/qip-edge-node/src/gateway.rs",
        "try_place",
        Route::Delegation,
    ),
    (
        "apps/qip-edge-node/src/reprice.rs",
        "place",
        Route::Delegation,
    ),
    (
        "apps/qip-edge-node/src/reprice.rs",
        "try_place",
        Route::Delegation,
    ),
    (
        "apps/qip-edge-node/src/reprice.rs",
        "replace",
        Route::Replacement {
            withdrawn_first: "venue.cancel(",
        },
    ),
];

/// One call that hands an order to a venue.
#[derive(Debug)]
struct Site {
    path: String,
    line: usize,
    function: String,
    /// Lines of the enclosing function from its signature up to the call.
    preamble: String,
}

fn crates_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend/crates")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The name of a `fn` declared on `line`, if it declares one.
fn declared_fn(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = ["pub(crate) fn ", "pub fn ", "fn "]
        .iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix))?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Whether `lines[index]` hands an order to a venue: a `try_place(` or
/// `place(` whose arguments carry a quantity and a price (the `Placer`
/// signature), or a `submit(&order` (the `Broker` signature). A declaration
/// of one of those methods is not a call.
fn is_order_call(lines: &[&str], index: usize) -> bool {
    let line = lines[index].trim();
    if line.starts_with("//") {
        return false;
    }
    if ["fn place(", "fn try_place(", "fn submit("]
        .iter()
        .any(|declaration| line.contains(declaration))
    {
        return false;
    }
    if line.contains(".submit(&order") {
        return true;
    }
    if line.contains("try_place(") || line.contains(".place(") {
        let call: String = lines[index..lines.len().min(index + 10)].join(" ");
        let arguments = call
            .split_once("place(")
            .map(|(_, after)| after.split(')').next().unwrap_or(""))
            .unwrap_or("");
        return arguments.contains("quantity") && arguments.contains("price");
    }
    false
}

fn order_call_sites() -> Vec<Site> {
    let root = crates_root();
    let mut files = Vec::new();
    for group in [
        "libs", "services", "runtime", "apps", "edge", "agents", "quant",
    ] {
        let Ok(members) = std::fs::read_dir(root.join(group)) else {
            continue;
        };
        for member in members.flatten() {
            rust_files(&member.path().join("src"), &mut files);
        }
    }
    files.sort();
    let mut sites = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("a source file is readable");
        let all: Vec<&str> = text.lines().collect();
        // Production source only: everything from the first unindented
        // `#[cfg(test)]` on is a test module.
        let end = all
            .iter()
            .position(|line| *line == "#[cfg(test)]")
            .unwrap_or(all.len());
        let lines = &all[..end];
        for index in 0..lines.len() {
            if !is_order_call(lines, index) {
                continue;
            }
            let start = (0..=index)
                .rev()
                .find(|i| declared_fn(lines[*i]).is_some())
                .unwrap_or(0);
            let function = declared_fn(lines[start]).unwrap_or_else(|| "?".to_string());
            let path = file
                .strip_prefix(&root)
                .expect("under backend/crates")
                .to_string_lossy()
                .replace('\\', "/");
            sites.push(Site {
                path,
                line: index + 1,
                function,
                preamble: lines[start..index].join("\n"),
            });
        }
    }
    sites
}

#[test]
fn every_call_that_hands_an_order_to_a_venue_is_a_row_in_the_gate_table() {
    let sites = order_call_sites();
    // Premise: the walk found the two seams everyone knows exist, so an
    // empty or partial walk cannot pass the comparison below vacuously.
    for (path, function) in [
        ("edge/qip-edge/src/cell.rs", "send"),
        ("services/qip-execution-engine/src/oms.rs", "submit"),
    ] {
        assert!(
            sites
                .iter()
                .any(|site| site.path == path && site.function == function),
            "premise: the walk did not find {path}::{function}; found {sites:?}"
        );
    }

    let found: BTreeSet<(String, String)> = sites
        .iter()
        .map(|site| (site.path.clone(), site.function.clone()))
        .collect();
    let pinned: BTreeSet<(String, String)> = SITES
        .iter()
        .map(|(path, function, _)| ((*path).to_string(), (*function).to_string()))
        .collect();
    let unclassified: Vec<String> = sites
        .iter()
        .filter(|site| !pinned.contains(&(site.path.clone(), site.function.clone())))
        .map(|site| format!("{}:{} in fn {}", site.path, site.line, site.function))
        .collect();
    assert!(
        unclassified.is_empty(),
        "an order reaches a venue at a call site nobody has classified; route it through \
         `Cell::send` or `OrderManager::submit`, or add a row saying how it is gated: \
         {unclassified:?}"
    );
    let stale: Vec<&(String, String)> = pinned.difference(&found).collect();
    assert!(
        stale.is_empty(),
        "rows naming a call site that no longer exists — a table that lists routes nobody \
         has is a table nobody can trust: {stale:?}"
    );
}

#[test]
fn each_gated_route_runs_its_gate_before_the_venue_call() {
    let sites = order_call_sites();
    let mut gated = 0;
    for site in &sites {
        let Some((_, _, route)) = SITES
            .iter()
            .find(|(path, function, _)| *path == site.path && *function == site.function)
        else {
            continue;
        };
        match route {
            Route::Gated { gate } => {
                gated += 1;
                assert!(
                    site.preamble.contains(gate),
                    "{}:{} hands an order to a venue in fn {} without `{gate}` before it",
                    site.path,
                    site.line,
                    site.function
                );
            }
            Route::Delegation => assert!(
                ["place", "try_place", "submit"].contains(&site.function.as_str()),
                "{}:{} is listed as a delegation but sits in fn {}, not in a venue method",
                site.path,
                site.line,
                site.function
            ),
            Route::Release => {}
            Route::Replacement { withdrawn_first } => assert!(
                site.preamble.contains(withdrawn_first),
                "{}:{} re-sends an order in fn {} without `{withdrawn_first}` first: a \
                 replacement that does not withdraw the original first is new exposure",
                site.path,
                site.line,
                site.function
            ),
        }
    }
    assert!(
        gated >= 2,
        "premise: both gated seams were examined, {gated} were"
    );
}

#[test]
fn the_desk_and_the_cell_gate_through_the_same_deterministic_checker() {
    // One gate, not two: the cell's gate is `qip_risk_engine`'s
    // `PreTradeChecker`, the same type the order manager holds. A cell gate
    // with an arithmetic of its own would be a second risk authority whose
    // answers could disagree with the desk's.
    let gate = std::fs::read_to_string(crates_root().join("edge/qip-edge/src/risk_gate.rs"))
        .expect("the cell risk gate exists");
    assert!(
        gate.contains("use qip_risk_engine::pretrade::{PreTradeChecker"),
        "the cell's gate must be the desk's PreTradeChecker"
    );
    let oms =
        std::fs::read_to_string(crates_root().join("services/qip-execution-engine/src/oms.rs"))
            .expect("the order manager exists");
    assert!(
        oms.contains("PreTradeChecker"),
        "premise: the desk holds one"
    );
    // And neither reaches a model: no cost-router rung, no agent, no
    // reasoning engine is named by the gate's source.
    for forbidden in [
        "qip_cost_router",
        "qip_reasoning_engine",
        "qip_investment_agents",
        "qip_ai",
    ] {
        assert!(
            !gate.contains(forbidden),
            "the cell's risk gate names {forbidden}; a model in the gate is not a gate"
        );
    }
}
