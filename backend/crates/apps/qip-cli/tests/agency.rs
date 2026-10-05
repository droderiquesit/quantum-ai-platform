//! `qip agency shadow`, run as the shipped binary.
//!
//! The agency contracts were a library nothing called. This is the one path
//! on which a goal document, an affordance graph and a proposed plan meet the
//! engine in a deployed binary, so it is driven as a process: a library call
//! would pass with the `agency` arm missing from `main`.
//!
//! The premise comes first. "Exits three" proves nothing unless the same
//! document, inside its bounds, exits zero.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};
use std::process::Command;

/// A goal with budget 100 and risk envelope 50, tolerating uncertainty 0.2;
/// one lever, `quote_size`, identified at 0.9; one proposal on it.
fn request() -> Value {
    json!({
        "goal": {
            "class": "liquidity",
            "target_state": "spread under 5 bps",
            "entities_affected": ["venue-a"],
            "time_horizon_secs": 3600,
            "success_metric": "median spread",
            "acceptable_uncertainty": "0.2",
            "budget": "100",
            "risk_envelope": "50",
            "jurisdictions": ["US"],
            "acting_identity": "desk-agent",
            "prohibited_side_effects": [],
            "prohibited_methods": [],
            "stop_conditions": ["spread widens"]
        },
        "targets": ["spread"],
        "variables": [
            {"name": "quote_size", "observable": true, "controllable": true},
            {"name": "spread", "observable": true, "controllable": false}
        ],
        "causes": [["quote_size", "spread"]],
        "tool_edges": [{
            "variable": "quote_size",
            "edge": {
                "tool": "quoter", "method": "market", "reversible": true,
                "latency_ms": 10, "cost": "30", "side_effects": [],
                "dependencies": [], "authority": "quote"
            }
        }],
        "identity": {"name": "desk-agent", "authorities": ["quote"]},
        "evidence": {"quote_size": {"identifiability": "0.9", "experimentable": true}},
        "observed_facts": [],
        "proposals": [{
            "root": {"step": {"tool": "quoter", "variable": "quote_size", "exposure": "20"}},
            "comparison": {
                "expected_causal_effect": "5", "confidence": "0.5", "cost": "1",
                "capital_usage": "1", "time_to_effect_secs": 60, "reversibility": "1",
                "legally_eligible": true, "conduct_risk": "0", "downside": "2"
            }
        }]
    })
}

/// What the binary printed, what it complained of, and what it exited with.
fn qip(name: &str, document: &Value) -> (String, String, i32) {
    let path = std::env::temp_dir().join(format!("qip-agency-{}-{name}.json", std::process::id()));
    std::fs::write(&path, document.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_qip"))
        .args(["agency", "shadow", "--request"])
        .arg(&path)
        .output()
        .expect("the qip binary runs");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn the_shipped_binary_shadows_a_bounded_plan_and_declines_one_outside_its_goal() {
    // Premise: inside every bound the plan clears every gate, is reported as
    // what would have run, and the run exits zero.
    let (out, err, code) = qip("bounded", &request());
    assert_eq!(code, 0, "premise: stdout {out} stderr {err}");
    assert!(
        out.contains(
            "stages passed: [Observe, Predict, Specify, IdentifyLevers, Generate, Simulate, Gate]"
        ),
        "{out}"
    );
    assert!(out.contains("outcome: shadowed"), "{out}");
    assert!(out.contains("quoter on quote_size, exposure 20"), "{out}");

    // Exposure 51 against an envelope of 50: refused by the plan's own
    // constructor, named, and the baseline is what remains.
    let mut over = request();
    over["proposals"][0]["root"]["step"]["exposure"] = json!("51");
    let (out, _, code) = qip("over", &over);
    assert_eq!(code, 3, "{out}");
    assert!(
        out.contains("refused proposal 1: plan exposure 51 exceeds risk envelope 50"),
        "{out}"
    );
    assert!(out.contains("outcome: no action"), "{out}");

    // A lever nobody has measured stops at the gate, however good the plan.
    let mut unmeasured = request();
    unmeasured["evidence"] = json!({});
    let (out, _, code) = qip("unmeasured", &unmeasured);
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("stage failed: Gate"), "{out}");
    assert!(
        out.contains("outcome: gather evidence; nothing is measured about: quote_size"),
        "{out}"
    );

    // A deceptive method is infeasible whatever effect is claimed for it.
    let mut deceptive = request();
    deceptive["tool_edges"][0]["edge"]["method"] = json!("spoofing");
    deceptive["proposals"][0]["comparison"]["expected_causal_effect"] = json!("1000000000");
    let (out, _, code) = qip("deceptive", &deceptive);
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("deceptive or manipulative method"), "{out}");

    // An operational tool on a system outside the owned registry is not a
    // lever this binary will plan with; the same edge on an owned one is.
    let mut foreign = request();
    foreign["tool_edges"][0]["edge"]["method"] = json!("operational");
    let (out, err, code) = qip("foreign", &foreign);
    assert_eq!(code, 1, "{out}");
    assert!(err.contains("owned-system registry"), "{err}");
    foreign["variables"][0]["owned"] = json!(true);
    let (out, err, code) = qip("owned", &foreign);
    assert_eq!(code, 0, "stdout {out} stderr {err}");

    // A goal missing a declaration is not a request: exit one, nothing run.
    let mut vague = request();
    vague["goal"].as_object_mut().unwrap().remove("budget");
    let (out, err, code) = qip("vague", &vague);
    assert_eq!(code, 1, "{out}");
    assert!(err.contains("`budget` is not declared"), "{err}");
    assert!(!out.contains("outcome"), "{out}");
}
