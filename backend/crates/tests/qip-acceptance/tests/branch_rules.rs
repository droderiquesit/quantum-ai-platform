//! main's rulesets, committed and checked (R12-CICD-9095).
//!
//! Branch protection was a repository setting no file held, so nothing could
//! say whether `main` was protected or notice it being switched off.
//! `.github/rulesets/` now holds the rules as GitHub's API takes them, and
//! `.github/workflows/rules.yml` reads the rules GitHub applies to `main` and
//! fails when one is missing. These tests pin the committed half: what the
//! rulesets refuse, that no one bypasses the integrity ruleset, that every
//! required check is a job this repository runs (a required check nobody
//! runs blocks every merge), and, driven under bash with `gh` stubbed, that
//! the checker fails on each kind of missing rule and passes when none is.

// The workspace denies `panic_in_result_fn` for production code. In a test
// the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_acceptance::read;
use serde_json::{Value, json};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Distinguishes each fixture directory a run of the checker writes.
static FIXTURES: AtomicUsize = AtomicUsize::new(0);

const INTEGRITY: &str = ".github/rulesets/main-integrity.json";
const REVIEW: &str = ".github/rulesets/main-review.json";
const CHECKER: &str = ".github/workflows/rules.yml";

fn ruleset(path: &str) -> Value {
    let value: Value = serde_json::from_str(&read(path)).expect("a ruleset is JSON");
    assert_eq!(value["target"], "branch", "{path} does not target a branch");
    assert_eq!(value["enforcement"], "active", "{path} is not enforced");
    assert!(
        value["conditions"]["ref_name"]["include"]
            .as_array()
            .is_some_and(|refs| refs.iter().any(|r| r == "~DEFAULT_BRANCH")),
        "{path} does not apply to the default branch"
    );
    value
}

fn rule<'a>(set: &'a Value, kind: &str) -> Option<&'a Value> {
    set["rules"]
        .as_array()
        .and_then(|rules| rules.iter().find(|rule| rule["type"] == kind))
}

fn required_contexts() -> Vec<String> {
    let set = ruleset(INTEGRITY);
    rule(&set, "required_status_checks").expect("the integrity ruleset requires checks")
        ["parameters"]["required_status_checks"]
        .as_array()
        .expect("a list of checks")
        .iter()
        .filter_map(|check| check["context"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn main_cannot_be_deleted_force_pushed_or_merged_red_and_nobody_bypasses_that() {
    let set = ruleset(INTEGRITY);
    for kind in ["deletion", "non_fast_forward", "required_status_checks"] {
        assert!(
            rule(&set, kind).is_some(),
            "{INTEGRITY} has no `{kind}` rule"
        );
    }
    assert_eq!(
        set["bypass_actors"],
        json!([]),
        "{INTEGRITY} names a bypass actor, so somebody can force-push main or merge it red"
    );
    assert!(
        required_contexts().len() >= 5,
        "premise: {INTEGRITY} requires fewer than five checks"
    );
}

#[test]
fn every_required_check_is_a_job_this_repository_runs() {
    // GitHub names a matrix job `<name> (<value>, <value>)`. A required
    // context that matches no job is a check that never reports, and a
    // check that never reports blocks every merge.
    let ci = read(".github/workflows/ci.yml");
    let boundary = read(".github/workflows/agent-boundary.yml");
    let job_names: Vec<String> = [&ci, &boundary]
        .iter()
        .flat_map(|text| text.lines())
        .filter_map(|line| line.strip_prefix("    name: "))
        .map(|name| name.trim().trim_matches('"').to_string())
        .collect();
    assert!(
        job_names.iter().any(|n| n == "agent boundary"),
        "premise: job names were not read"
    );
    for context in required_contexts() {
        let (base, values) = match context.split_once(" (") {
            Some((base, rest)) => (base.to_string(), rest.trim_end_matches(')').to_string()),
            None => (context.clone(), String::new()),
        };
        assert!(
            job_names.contains(&base),
            "{INTEGRITY} requires `{context}`, but no job in ci.yml or agent-boundary.yml is named \
             `{base}`"
        );
        for value in values.split(", ").filter(|v| !v.is_empty()) {
            assert!(
                ci.lines().any(|line| {
                    let line = line.trim().trim_start_matches("- ");
                    line.split_once(": ")
                        .is_some_and(|(_, v)| v.trim_matches('"') == value)
                }),
                "{INTEGRITY} requires `{context}`, but ci.yml's matrix has no value `{value}`"
            );
        }
    }
}

#[test]
fn a_code_owner_reviews_every_change_and_only_an_admin_bypasses_it_and_only_through_a_pr() {
    let set = ruleset(REVIEW);
    let pr = rule(&set, "pull_request").expect("the review ruleset has a pull_request rule");
    assert_eq!(
        pr["parameters"]["require_code_owner_review"], true,
        "{REVIEW} does not require a code owner's review"
    );
    assert!(
        pr["parameters"]["required_approving_review_count"]
            .as_u64()
            .is_some_and(|n| n >= 1),
        "{REVIEW} requires no approving review"
    );
    let bypass = set["bypass_actors"].as_array().expect("a list");
    for actor in bypass {
        assert_eq!(
            (
                &actor["actor_type"],
                &actor["actor_id"],
                &actor["bypass_mode"]
            ),
            (&json!("RepositoryRole"), &json!(5), &json!("pull_request")),
            "{REVIEW} lets {actor} bypass review other than as the admin role through a pull \
             request"
        );
    }
}

/// The checker step's body, dedented.
fn checker_body() -> String {
    let text = read(CHECKER);
    let mut lines = text
        .lines()
        .skip_while(|line| !line.trim().starts_with("- name: read main's live rules"));
    assert!(lines.next().is_some(), "{CHECKER} has no checker step");
    let mut lines = lines.skip_while(|line| line.trim() != "run: |");
    let opening = lines.next().expect("a run body");
    let margin = opening.len() - opening.trim_start().len() + 2;
    lines
        .take_while(|line| line.trim().is_empty() || line.len() - line.trim_start().len() >= margin)
        .map(|line| line.get(margin..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The rules GitHub would report for main if both rulesets were applied.
fn applied() -> Value {
    let mut rules: Vec<Value> = Vec::new();
    for path in [INTEGRITY, REVIEW] {
        rules.extend(
            ruleset(path)["rules"]
                .as_array()
                .expect("rules")
                .iter()
                .cloned(),
        );
    }
    Value::Array(rules)
}

/// The checker's exit status when GitHub reports `live` as main's rules.
fn checker_exit(live: &Value) -> i32 {
    let directory = std::env::temp_dir().join(format!(
        "qip-branch-rules-{}-{}",
        std::process::id(),
        FIXTURES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let fixture = directory.join("live.json");
    std::fs::write(&fixture, live.to_string()).expect("fixture written");
    let script = format!(
        "gh() {{ case \"$2\" in */rules/branches/*) cat '{}' ;; *) echo main ;; esac; }}\n\
         REPOSITORY=o/r\n{}\n",
        fixture.display(),
        checker_body()
    );
    let status = Command::new("bash")
        .arg("-c")
        .arg(&script)
        .current_dir(qip_acceptance::repository_root())
        .output()
        .expect("bash runs")
        .status
        .code()
        .expect("an exit code");
    std::fs::remove_dir_all(&directory).ok();
    status
}

#[test]
fn the_checker_passes_applied_rulesets_and_fails_each_missing_rule() {
    let full = applied();
    assert_eq!(
        checker_exit(&full),
        0,
        "the checker refused the rulesets as committed"
    );

    let without = |kind: &str| -> Value {
        Value::Array(
            full.as_array()
                .unwrap()
                .iter()
                .filter(|rule| rule["type"] != kind)
                .cloned()
                .collect(),
        )
    };
    for kind in [
        "deletion",
        "non_fast_forward",
        "pull_request",
        "required_status_checks",
    ] {
        assert_ne!(
            checker_exit(&without(kind)),
            0,
            "main with no `{kind}` rule passed the checker"
        );
    }

    let mut no_owner = full.clone();
    for rule in no_owner.as_array_mut().unwrap() {
        if rule["type"] == "pull_request" {
            rule["parameters"]["require_code_owner_review"] = json!(false);
        }
    }
    assert_ne!(
        checker_exit(&no_owner),
        0,
        "main without a code-owner review passed the checker"
    );

    let mut one_check_short = full.clone();
    for rule in one_check_short.as_array_mut().unwrap() {
        if rule["type"] == "required_status_checks" {
            rule["parameters"]["required_status_checks"]
                .as_array_mut()
                .unwrap()
                .pop();
        }
    }
    assert_ne!(
        checker_exit(&one_check_short),
        0,
        "main requiring one check fewer than the committed ruleset passed the checker"
    );
}
