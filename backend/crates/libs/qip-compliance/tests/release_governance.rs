//! CICD-001 promotion needs five gates, CICD-025 remediation runs only signed
//! approved runbooks, CICD-068 windowed changes need a declared window.
//!
//! Each test first proves the admitting path works, so a refusal cannot be an
//! artefact of a fixture that was never admissible.

#![allow(clippy::panic_in_result_fn)]

use qip_compliance::release::{
    ChangeKind, ChangeWindows, Gate, GateVerdict, PromotionPolicy, RemediationExecutor, Runbook,
};
use qip_compliance::signing::SigningKey;
use qip_core::Timestamp;
use qip_core::error::Result;

const DIGEST: &str = "sha256:aaaa";

fn verdicts(digest: &str, passing: bool) -> Vec<GateVerdict> {
    Gate::ALL
        .iter()
        .map(|g| GateVerdict {
            gate: *g,
            digest: digest.to_string(),
            passed: passing,
        })
        .collect()
}

#[test]
fn the_promotion_policy_names_all_five_gates() {
    assert_eq!(
        PromotionPolicy.gates(),
        [
            "simulation",
            "integration",
            "security",
            "performance",
            "paper-trading"
        ]
    );
}

#[test]
fn an_artifact_with_a_passing_verdict_from_every_gate_is_promotable() -> Result<()> {
    let admitted = PromotionPolicy.admit(DIGEST, &verdicts(DIGEST, true))?;
    assert_eq!(admitted.digest(), DIGEST);
    Ok(())
}

#[test]
fn an_artifact_lacking_any_one_gate_verdict_is_refused_naming_that_gate() {
    for skipped in Gate::ALL {
        let mut v = verdicts(DIGEST, true);
        v.retain(|x| x.gate != skipped);
        assert_eq!(v.len(), 4, "premise: exactly one verdict was removed");
        let error = PromotionPolicy.admit(DIGEST, &v).expect_err("must refuse");
        assert!(
            error
                .message()
                .contains(&format!("missing verdict for [{}]", skipped.name())),
            "{}",
            error.message()
        );
    }
}

#[test]
fn an_artifact_carrying_one_failing_verdict_is_refused_even_if_a_later_run_passed() {
    for failing in Gate::ALL {
        // The red run comes first and a green rerun after it; the red stands.
        let mut v = vec![GateVerdict {
            gate: failing,
            digest: DIGEST.into(),
            passed: false,
        }];
        v.extend(verdicts(DIGEST, true));
        assert_eq!(
            v.len(),
            6,
            "premise: the gate has a red and a green verdict"
        );
        let error = PromotionPolicy.admit(DIGEST, &v).expect_err("must refuse");
        assert!(
            error
                .message()
                .contains(&format!("failing [{}]", failing.name())),
            "{}",
            error.message()
        );
    }
}

#[test]
fn verdicts_earned_by_a_different_digest_do_not_promote_this_one() {
    let error = PromotionPolicy
        .admit(DIGEST, &verdicts("sha256:bbbb", true))
        .expect_err("another build's passes are not this build's");
    assert!(error.message().contains("missing verdict for [simulation"));
}

fn key() -> Result<SigningKey> {
    SigningKey::from_secret("release-key", &[9u8; 32])
}

fn runbook(action: &str) -> Runbook {
    Runbook {
        id: "rb-1".into(),
        action: action.into(),
        body: "restart the stalled consumer".into(),
    }
}

#[test]
fn the_executor_runs_a_signed_approved_runbook_and_refuses_an_unsigned_or_unapproved_one()
-> Result<()> {
    let key = key()?;
    let exec = RemediationExecutor::new(&key, ["restart-consumer".to_string()]);
    let rb = runbook("restart-consumer");
    let sig = rb.sign(&key);

    // Premise: the admitting path runs the action.
    let ran = exec.run(&rb, &sig, |r| Ok(r.action.clone()))?;
    assert_eq!(ran, "restart-consumer");

    // Unsigned.
    let mut ran_unsigned = false;
    let error = exec
        .run(&rb, "", |_| {
            ran_unsigned = true;
            Ok(())
        })
        .expect_err("unsigned must be refused");
    assert!(error.message().contains("no signature"));
    assert!(!ran_unsigned);

    // Body edited after signing.
    let mut edited = rb.clone();
    edited.body = "drop the table".into();
    assert!(exec.run(&edited, &sig, |_| Ok(())).is_err());

    // Signed but the action was never approved.
    let other = runbook("scale-to-zero");
    let mut ran_unapproved = false;
    let error = exec
        .run(&other, &other.sign(&key), |_| {
            ran_unapproved = true;
            Ok(())
        })
        .expect_err("unapproved action must be refused");
    assert!(
        error
            .message()
            .contains("not on the approved remediation list")
    );
    assert!(!ran_unapproved);
    Ok(())
}

#[test]
fn a_network_change_outside_every_declared_window_is_refused_and_inside_one_is_admitted()
-> Result<()> {
    let mut windows = ChangeWindows::default();
    let open = Timestamp::from_secs(1_760_000_000);
    let close = Timestamp::from_secs(1_760_003_600);
    windows.declare(open, close)?;

    // Premise: inside the window every kind is admitted.
    for kind in [
        ChangeKind::Folder,
        ChangeKind::Project,
        ChangeKind::Network,
        ChangeKind::Database,
    ] {
        windows.admit(kind, open)?;
    }
    // Half-open: the closing instant is outside.
    for kind in [
        ChangeKind::Folder,
        ChangeKind::Project,
        ChangeKind::Network,
        ChangeKind::Database,
    ] {
        assert!(windows.admit(kind, close).is_err(), "{kind:?} at close");
    }
    // A kind the blueprint does not window is not held back.
    windows.admit(ChangeKind::Other, close)?;
    // No window declared means none open.
    assert!(
        ChangeWindows::default()
            .admit(ChangeKind::Database, open)
            .is_err()
    );
    // An inverted window is refused rather than never opening.
    assert!(windows.declare(close, open).is_err());
    assert!(
        windows.declare(open, open).is_err(),
        "an empty window never opens"
    );
    Ok(())
}
