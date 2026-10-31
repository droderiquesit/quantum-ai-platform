//! R12-GCP-9120: a saved plan that changes a core resource is refused outside
//! the committed change window, and admitted inside it.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_cli::change_window::{Policy, core_changes, judge};
use qip_core::Timestamp;
use serde_json::{Value, json};

fn policy() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../infrastructure/change-windows.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn plan(changes: &[(&str, &[&str])]) -> Value {
    json!({ "resource_changes": changes.iter().map(|(kind, actions)| json!({
        "type": kind, "address": format!("{kind}.x"), "change": { "actions": actions }
    })).collect::<Vec<_>>() })
}

/// Wednesday 2026-10-21 15:00 UTC, inside the committed window.
fn inside() -> Timestamp {
    Timestamp::parse_rfc3339("2026-10-21T15:00:00Z").unwrap()
}

/// Saturday 2026-10-24 03:00 UTC, outside it.
fn outside() -> Timestamp {
    Timestamp::parse_rfc3339("2026-10-24T03:00:00Z").unwrap()
}

/// Mutation: drop the window check in `judge` and the inside case fails;
/// drop the core-change check and the outside case is admitted.
#[test]
fn a_core_resource_change_is_refused_outside_the_window_and_admitted_inside_it() {
    let network = plan(&[("google_compute_network", &["update"])]);
    let parsed = Policy::parse(&policy()).unwrap();
    assert_eq!(
        core_changes(&network, &parsed).unwrap().len(),
        1,
        "premise: a network is core"
    );
    assert!(judge(&network, &policy(), outside()).unwrap().is_some());
    assert_eq!(judge(&network, &policy(), inside()).unwrap(), None);
}

#[test]
fn deleting_or_replacing_a_database_is_a_core_change() {
    for actions in [&["delete"][..], &["delete", "create"][..]] {
        let p = plan(&[("google_spanner_database", actions)]);
        assert!(
            judge(&p, &policy(), outside()).unwrap().is_some(),
            "{actions:?}"
        );
    }
}

#[test]
fn a_plan_touching_no_core_resource_is_admitted_at_any_time() {
    let p = plan(&[
        ("google_cloud_run_v2_service", &["update"]),
        ("google_compute_network", &["no-op"]),
    ]);
    assert_eq!(judge(&p, &policy(), outside()).unwrap(), None);
}

#[test]
fn it_fails_closed_on_a_change_with_no_actions_and_on_a_window_that_cannot_open() {
    let bare =
        json!({ "resource_changes": [{ "type": "google_project", "address": "p", "change": {} }] });
    assert!(judge(&bare, &policy(), inside()).is_err());
    let mut none = policy();
    none["windows"] = json!([]);
    assert!(Policy::parse(&none).is_err());
    let mut empty = policy();
    empty["windows"] = json!([{ "days": ["Wed"], "from": "20:00", "to": "14:00" }]);
    assert!(Policy::parse(&empty).is_err());
    let mut bad_day = policy();
    bad_day["windows"] = json!([{ "days": ["Wednesday"], "from": "14:00", "to": "20:00" }]);
    assert!(Policy::parse(&bad_day).is_err());
}

#[test]
fn the_committed_policy_names_the_hierarchy_the_networks_and_the_databases_as_core() {
    let parsed = Policy::parse(&policy()).unwrap();
    for kind in [
        "google_folder",
        "google_project",
        "google_compute_network",
        "google_spanner_database",
    ] {
        assert!(
            parsed.core_resource_types.iter().any(|k| k == kind),
            "{kind}"
        );
    }
    // The window opens on the weekday it names and shuts at its edge.
    assert!(parsed.open_at(inside()));
    assert!(!parsed.open_at(Timestamp::parse_rfc3339("2026-10-21T20:00:00Z").unwrap()));
    // Each weekday at 15:00 UTC, Monday 2026-10-19 to Sunday 2026-10-25:
    // the committed window is Tuesday to Thursday, so a weekday computed one
    // day off opens Monday or Friday and fails here.
    let open: Vec<bool> = (19..=25)
        .map(|day| {
            parsed.open_at(Timestamp::parse_rfc3339(&format!("2026-10-{day}T15:00:00Z")).unwrap())
        })
        .collect();
    assert_eq!(open, vec![false, true, true, true, false, false, false]);
}
