//! The Cognitive Compiler (§9.13; R12-REASON-7203, 3115): a verified
//! discovery becomes a machine-checkable derivation and a bounded reflex
//! package with a stated cost, which a cell can load only signed, only
//! unaltered and only inside its bounds.
//!
//! Each test asserts its premise first — that the discovery would compile —
//! so a refusal test cannot pass on a compiler that refuses everything.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::reflex_package::worst_case_ops;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_reasoning_engine::compiler::{
    CRITERIA, CognitiveCompiler, CompilerBounds, Condition, Discovery, Finding, MAX_ADJUSTMENT_BPS,
    PackageSigner, PackageVerifier, ReflexInput, ReflexPackage, ReflexPackageFields, ReflexRule,
    SignedReflexPackage, Verification,
};
use std::collections::BTreeMap;

fn now() -> Timestamp {
    Timestamp::from_secs(1_800_000_000)
}

/// A stand-in for ADR 0110's signer: the "signature" is the digest under a
/// key name. It proves the plumbing — that what is loaded is what was
/// signed — and nothing about cryptography, which is the KMS's.
struct TestKey;

impl PackageSigner for TestKey {
    fn key_id(&self) -> &str {
        "test-key"
    }
    fn sign(&self, digest: &str) -> Result<String> {
        Ok(format!("signed:{digest}"))
    }
}

impl PackageVerifier for TestKey {
    fn verify(&self, key_id: &str, digest: &str, signature: &str) -> Result<()> {
        if key_id == "test-key" && signature == format!("signed:{digest}") {
            Ok(())
        } else {
            Err(Error::denied("the signature does not verify"))
        }
    }
}

fn rule(add_bps: i32) -> ReflexRule {
    ReflexRule {
        id: "fill-bias@XSIM".to_string(),
        when: vec![Condition::Equals {
            input: "venue".to_string(),
            value: "XSIM".to_string(),
        }],
        output: "twin_entry_bps".to_string(),
        add_bps,
    }
}

fn discovery(add_bps: i32) -> Discovery {
    Discovery {
        discovery_id: "fill-bias-XSIM".to_string(),
        question: "Model gap: fills at `XSIM` miss the twin".to_string(),
        conclusion: "the twin's entry at `XSIM` is corrected by -40 bp".to_string(),
        findings: vec![
            Finding {
                statement: "the historical stage fitted a 40 bp bias".to_string(),
                evidence: vec!["digest-historical".to_string()],
            },
            Finding {
                statement: "the shadow stage improved by 38 bp".to_string(),
                evidence: vec!["digest-shadow".to_string()],
            },
        ],
        verification: Verification {
            evaluation: "evaluation:fill-bias".to_string(),
            criteria_passed: CRITERIA,
        },
        uncertainty_bp: 400,
        rules: vec![rule(add_bps)],
    }
}

fn compiler() -> CognitiveCompiler {
    CognitiveCompiler::new(CompilerBounds::default()).unwrap()
}

fn venue(name: &str) -> BTreeMap<String, ReflexInput> {
    BTreeMap::from([("venue".to_string(), ReflexInput::Label(name.to_string()))])
}

/// The whole road: compiled with a stated cost equal to its rules' cost,
/// signed, loaded by a cell-side verifier, and applying its adjustment to
/// the venue it was discovered at and nowhere else. Mutation: drop the
/// `Condition::Equals` arm's comparison (`matches!(.., Some(_))`) — the
/// other venue is adjusted too and the last assertion fails.
#[test]
fn a_verified_discovery_compiles_into_a_bounded_package_a_cell_loads_and_applies() {
    let mut compiler = compiler();
    let compiled = compiler.compile(&discovery(-40), now()).unwrap();
    assert_eq!(
        compiled.stated_cost,
        worst_case_ops(&compiled.package.rules)
    );
    assert_eq!(
        compiled.package.bounds.worst_case_cost,
        compiled.stated_cost
    );
    assert!(compiled.package.bounds.reflex_compatible);
    assert_eq!(
        compiled.package.derivation_id,
        compiled.derivation.derivation_id
    );

    let signed = compiler.sign(compiled.package, &TestKey).unwrap();
    let loaded = SignedReflexPackage::load(&signed.encode().unwrap(), &TestKey, now()).unwrap();

    assert_eq!(
        loaded.evaluate(&venue("XSIM")).get("twin_entry_bps"),
        Some(&-40)
    );
    assert!(loaded.evaluate(&venue("ELSEWHERE")).is_empty());
}

/// The formalisation is a derivation the contract can re-check: it cites
/// the evidence each finding rests on and reaches the conclusion. Mutation:
/// omit the findings' evidence from `evidence` in `formalise` — the
/// contract refuses the trace's undeclared premise and `compile` errs.
#[test]
fn the_formalisation_is_a_machine_checkable_derivation_citing_its_evidence() {
    let compiled = compiler().compile(&discovery(-40), now()).unwrap();
    let derivation = &compiled.derivation;
    assert_eq!(derivation.conclusion, discovery(-40).conclusion);
    for id in ["digest-historical", "digest-shadow", "evaluation:fill-bias"] {
        assert!(
            derivation.evidence.iter().any(|e| e == id),
            "{id} not cited"
        );
    }
    assert_eq!(
        derivation.trace.last().map(|s| s.statement.as_str()),
        Some(derivation.conclusion.as_str())
    );
}

/// Research that did not pass every criterion is not compiled. Mutation:
/// delete the `criteria_passed != CRITERIA` check — it compiles and the
/// assertion fails.
#[test]
fn a_discovery_that_failed_a_criterion_is_not_compiled() {
    assert!(
        compiler().compile(&discovery(-40), now()).is_ok(),
        "premise"
    );
    let mut partial = discovery(-40);
    partial.verification.criteria_passed = CRITERIA - 1;
    let error = compiler().compile(&partial, now()).unwrap_err();
    assert!(error.message().contains("criteria"), "{}", error.message());
}

/// A discovery the slow lane was not sure enough of stays in the slow lane.
#[test]
fn a_discovery_above_the_uncertainty_bound_stays_in_the_slow_lane() {
    let mut unsure = discovery(-40);
    unsure.uncertainty_bp = CompilerBounds::default().max_uncertainty_bp + 1;
    assert!(compiler().compile(&unsure, now()).is_err());
}

/// An adjustment beyond the reflex bound is refused, not clamped to it.
/// Mutation: clamp `add_bps` to `MAX_ADJUSTMENT_BPS` in the contract's
/// check instead of refusing — it compiles and the assertion fails.
#[test]
fn an_adjustment_beyond_the_reflex_bound_is_refused_not_clamped() {
    assert!(
        compiler()
            .compile(&discovery(-MAX_ADJUSTMENT_BPS), now())
            .is_ok(),
        "premise: the bound itself is admitted"
    );
    let error = compiler()
        .compile(&discovery(-MAX_ADJUSTMENT_BPS - 1), now())
        .unwrap_err();
    assert!(error.message().contains("bounded"), "{}", error.message());
}

/// A package that misstates its own cost is refused by the contract, so a
/// cell can budget what it loads. Mutation: delete the stated-cost check in
/// `ReflexPackageFields::check` — the misstated package is admitted.
#[test]
fn a_package_that_misstates_its_cost_is_refused() {
    let compiled = compiler().compile(&discovery(-40), now()).unwrap();
    let mut fields: ReflexPackageFields = compiled.package.into_fields();
    assert!(
        ReflexPackage::new(fields.clone()).is_ok(),
        "premise: the package is valid as compiled"
    );
    fields.bounds.worst_case_cost += 1;
    fields.bounds.latency_micros += 1;
    assert!(ReflexPackage::new(fields).is_err());
}

/// What the cell loads is what was signed: a package altered after signing
/// does not load, and neither does one past its expiry. Mutation: have
/// `load` verify the *stored* `signed.digest` and delete the comparison
/// with the recomputed one — the altered package then loads under the
/// original signature and the second assertion fails.
#[test]
fn a_package_altered_after_signing_or_past_expiry_does_not_load() {
    let mut compiler = compiler();
    let compiled = compiler.compile(&discovery(-40), now()).unwrap();
    let signed = compiler.sign(compiled.package, &TestKey).unwrap();
    let bytes = signed.encode().unwrap();
    assert!(
        SignedReflexPackage::load(&bytes, &TestKey, now()).is_ok(),
        "premise: the signed package loads"
    );

    let altered = String::from_utf8(bytes.clone())
        .unwrap()
        .replace("\"add_bps\":-40", "\"add_bps\":-400");
    assert_ne!(altered.as_bytes(), bytes.as_slice(), "premise: altered");
    assert!(SignedReflexPackage::load(altered.as_bytes(), &TestKey, now()).is_err());

    let later = now().saturating_add(Duration::from_days(30));
    assert!(SignedReflexPackage::load(&bytes, &TestKey, later).is_err());
}
