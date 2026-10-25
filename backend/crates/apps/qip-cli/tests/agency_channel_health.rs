//! R12-OBS-7132 on `qip agency shadow`: each channel-health dimension the
//! guard judges is recorded per channel.
//!
//! The guard learned to note its verdicts in `preflight_observed`, and the
//! deep brain's door recorded them for its one outbox channel; but the
//! engine's `execute` — the path this binary's every pass goes through —
//! still called the plain `preflight`, which builds the same verdicts and
//! drops them. A CLI pass judging a `press` draft therefore recorded
//! nothing, and an independent review found it. These tests drive
//! `Session::pass`, the function `qip agency shadow` runs per document, and
//! read the registry the session records into.
//!
//! The helpers are copied from `agency.rs`, not shared, as `agency_record.rs`
//! does, so no lane edits another's suite.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use qip_agency::guard::{HealthDimension, HealthOutcome};
use qip_cli::agency::{Session, UNCONFIGURED_CHANNEL};
use qip_core::Timestamp;
use qip_events::EventLog;
use qip_observability::metrics::{Snapshot, labels, names};
use serde_json::{Value, json};

/// One paper-venue tool registered on two channels, `paper-venue` and
/// `press`, each with an audience the conduct policy admits.
fn policy_document() -> Value {
    json!({
        "guard": {
            "conduct": {
                "authorized_sources": ["venue-feed"],
                "authorized_identities": ["desk-agent"],
                "required_disclosures": ["paper_venue"],
                "channel_policies": {
                    "paper-venue": ["venue-a participants"],
                    "press": ["venue-a participants"]
                },
                "permitted_jurisdictions": ["US"],
                "conduct_rules": [
                    "no_false_claims", "no_manipulation", "no_fake_consensus",
                    "no_rumour_engineering"
                ]
            },
            "tools": [{
                "tool": "quoter",
                "acting_identity": "desk-agent",
                "permission_scope": ["quote"],
                "disclosure_policy": "none required on a paper venue",
                "channels": ["paper-venue", "press"],
                "max_actions_per_window": 1,
                "budget_units": 1000,
                "revocation_path": "operator removes it from this file",
                "provenance": "in-tree test fixture, no third-party dependency",
                "sandbox_tests": ["quoter-sandbox-smoke"],
                "cost_latency": {"cost_units": 1, "latency_ms": 100},
                "allowed_callers": ["desk-agent"]
            }]
        },
        "evidence": [{
            "id": "spread-series-1", "source_id": "venue-feed",
            "statement": "quoted spreads on venue-a are wider than the desk target"
        }],
        "eligibility": [
            {
                "subject": {"kind": "venue", "id": "venue-a"},
                "jurisdictions": ["US"],
                "granted_by": "compliance-officer",
                "granted_at": "2020-01-01T00:00:00Z",
                "expires_at": "2100-01-01T00:00:00Z"
            },
            {
                "subject": {"kind": "jurisdiction", "id": "US"},
                "jurisdictions": ["US"],
                "granted_by": "compliance-officer",
                "granted_at": "2020-01-01T00:00:00Z",
                "expires_at": "2100-01-01T00:00:00Z"
            }
        ],
        "action_policies": [{
            "class": "market",
            "uplift_low": "1",
            "uplift_high": "5",
            "predicted_side_effects": []
        }]
    })
}

/// A pass in rate window `window` whose one draft is said on `channel`,
/// widening the market policy on a hold-out it reproduces and acting under
/// it, so the step reaches the binary's adapter, which refuses every call.
fn acting_on(channel: &str, window: u64) -> Value {
    let units: Vec<String> = (0..12).map(|i| format!("cell-{i:02}")).collect();
    let design = qip_agency::estimate::Experiment::design("seed-2026-10-18", &units, 2500).unwrap();
    let outcomes: serde_json::Map<String, Value> = units
        .iter()
        .map(|u| {
            let value = if design.treated().contains(u) {
                4.0
            } else {
                1.0
            };
            (u.clone(), json!(value))
        })
        .collect();
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
            "stop_conditions": ["spread widens"],
            "rollback_conditions": ["restore the prior quote width"]
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
        }],
        "touches": [{"kind": "venue", "id": "venue-a"}],
        "pass": {
            "communications": {"quoter": {
                "id": format!("quote-{window}"),
                "content": "Quote 20 lots on venue-a at the published spread",
                "channel": channel,
                "sender_identity": "desk-agent",
                "evidence_citations": [{
                    "source_id": "venue-feed", "evidence_id": "spread-series-1",
                    "claim": "quoted spreads on venue-a are wider than the desk target",
                    "confidence_bps": 9000
                }],
                "disclosures": [{"kind": "paper_venue", "text": "Paper venue: no live order is placed"}],
                "target_audience": "venue-a participants",
                "jurisdiction": "US"
            }},
            "grounds": [{"evidence": "spread series spread-series-1 shows quotes under-sized"}],
            "approvals": {
                "goal_approver": "pm-a", "goal_approved_at": "2026-01-01T00:00:10Z",
                "plan_approver": "risk-b", "plan_approved_at": "2026-01-01T00:00:20Z",
                "execute_at": "2026-01-01T00:00:30Z"
            },
            "rate_window": window
        },
        "act_as": "market",
        "structural": {
            "models": [{
                "name": "desk",
                "links": [{"cause": "quote_size", "effect": "spread", "coefficient": -0.6}],
                "intercepts": {"spread": 6.0}
            }],
            "scenarios": [
                {"name": "calm", "exogenous": {}},
                {"name": "stressed", "exogenous": {"spread": 3.0}}
            ],
            "objective": {"variable": "spread", "direction": "decrease"},
            "actuation": {"quoter": 10.0}
        },
        "widening": {
            "class": "market",
            "approver": {"name": "risk-officer", "authorities": ["agency:widen-autonomy"]},
            "attribution": {
                "goal_id": "g1", "intervention_id": "i1", "action_id": "a1",
                "action_class": "market",
                "identification": "experiment",
                "observations": [{"horizon_ms": 60000, "value": 3.0}],
                "counterfactual_method": "randomised control",
                "effect": {"mean": 3.0, "std_dev": 0.5},
                "causal_chain": ["quote_size", "spread"],
                "confounders": ["none considered"],
                "competing_explanations": ["seasonality"],
                "side_effects": ["none observed"],
                "identifiability": 0.8,
                "resulting_updates": ["raise quote-size prior"]
            },
            "evidence": {"hold_out_experiment": {
                "seed": "seed-2026-10-18", "units": units, "hold_out_bps": 2500,
                "outcomes": outcomes
            }}
        }
    })
}

fn at() -> Timestamp {
    Timestamp::parse_rfc3339("2026-01-01T00:01:00Z").unwrap()
}

/// One channel-health series.
fn health(
    snapshot: &Snapshot,
    channel: &str,
    dimension: HealthDimension,
    outcome: HealthOutcome,
) -> u64 {
    snapshot.counter(
        names::AGENCY_CHANNEL_HEALTH,
        &labels([
            ("channel", channel),
            ("dimension", dimension.as_str()),
            ("outcome", outcome.as_str()),
        ]),
    )
}

/// Two passes, one judging a `paper-venue` draft and one a `press` draft,
/// each acting under a widened policy so the step reaches the binary's
/// adapter. Every one of the five dimensions is then recorded for each
/// channel: the guard's four passed, and reach failed, because the binary's
/// adapter refuses every call (ADR 0114) — which is also what proves the
/// outcome is the one that happened rather than a constant.
///
/// Mutations, each verified: in `engine::execute`, call the plain
/// `guard.preflight` instead of `preflight_observed` — nothing but reach is
/// recorded and the identity-integrity assertion fails at zero; note reach
/// as passed whatever the adapter returned — the reach `fail` assertion
/// fails at zero.
#[test]
fn a_governed_cli_pass_records_each_channel_health_dimension_for_each_channel_it_judges() {
    let mut journal = EventLog::in_memory();
    let mut session = Session::open(&policy_document().to_string(), &journal).unwrap();
    assert_eq!(
        session
            .metrics()
            .snapshot()
            .counter_total(names::AGENCY_CHANNEL_HEALTH),
        0,
        "premise: nothing is recorded before a pass runs"
    );

    let mut printed = Vec::new();
    for (window, channel) in [(1, "paper-venue"), (2, "press")] {
        let (lines, _) = session
            .pass(&acting_on(channel, window).to_string(), &mut journal, at())
            .unwrap();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("outcome: attempted and refused")),
            "premise: the {channel} pass reached the adapter: {lines:#?}"
        );
        printed.extend(lines);
    }

    let snapshot = session.metrics().snapshot();
    for channel in ["paper-venue", "press"] {
        for dimension in [
            HealthDimension::IdentityIntegrity,
            HealthDimension::Provenance,
            HealthDimension::Violations,
            HealthDimension::Coordination,
        ] {
            assert_eq!(
                health(&snapshot, channel, dimension, HealthOutcome::Pass),
                1,
                "{channel} {} pass",
                dimension.as_str()
            );
            assert_eq!(
                health(&snapshot, channel, dimension, HealthOutcome::Fail),
                0,
                "{channel} {} fail",
                dimension.as_str()
            );
        }
        assert_eq!(
            health(
                &snapshot,
                channel,
                HealthDimension::Reach,
                HealthOutcome::Fail
            ),
            1,
            "{channel} reach fail: the adapter refused the call"
        );
        assert_eq!(
            health(
                &snapshot,
                channel,
                HealthDimension::Reach,
                HealthOutcome::Pass
            ),
            0,
            "{channel} reach pass: nothing reached the channel"
        );
        for dimension in HealthDimension::ALL {
            let outcome = if dimension == HealthDimension::Reach {
                "fail"
            } else {
                "pass"
            };
            let line = format!("channel health: {channel} {} {outcome}", dimension.as_str());
            assert!(
                printed.contains(&line),
                "the operator is told `{line}`: {printed:#?}"
            );
        }
    }
    assert_eq!(
        snapshot.counter_total(names::AGENCY_CHANNEL_HEALTH),
        10,
        "five dimensions on each of two channels, and nothing else"
    );
}

/// A draft naming a channel no registered tool declares is still judged, and
/// its verdicts are filed under the one literal, never under the string the
/// request wrote: a label taken from the request would be a series per
/// string anybody can type.
///
/// Mutation, verified: `Session::record_channel_health` labels the series
/// with the draft's channel instead of `channel_label`'s — the series lands
/// under `elsewhere` and the `unconfigured` assertion fails at zero.
#[test]
fn a_channel_outside_the_policy_is_recorded_under_one_literal_and_never_by_name() {
    let mut journal = EventLog::in_memory();
    let mut session = Session::open(&policy_document().to_string(), &journal).unwrap();
    let (lines, _) = session
        .pass(&acting_on("elsewhere", 1).to_string(), &mut journal, at())
        .unwrap();
    let snapshot = session.metrics().snapshot();
    let filed: u64 = HealthDimension::ALL
        .into_iter()
        .flat_map(|d| [(d, HealthOutcome::Pass), (d, HealthOutcome::Fail)])
        .map(|(d, o)| health(&snapshot, UNCONFIGURED_CHANNEL, d, o))
        .sum();
    assert!(filed > 0, "the draft was judged and filed: {lines:#?}");
    assert_eq!(
        filed,
        snapshot.counter_total(names::AGENCY_CHANNEL_HEALTH),
        "every verdict is under the literal"
    );
    assert!(
        snapshot
            .series
            .iter()
            .filter(|s| s.name == names::AGENCY_CHANNEL_HEALTH)
            .all(|s| s.labels.get("channel").map(String::as_str) == Some(UNCONFIGURED_CHANNEL)),
        "no series carries the request's own channel string"
    );
}
