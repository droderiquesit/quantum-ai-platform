//! The deep brain's research loop as the binary runs it, after lane R2-DEEP:
//! poisoning findings over the world fabric become research, the compute
//! market decides what runs and retires what stops yielding, a fill-bias
//! campaign is carried out and concluded, and the Cognitive Compiler
//! distils the promoted result into a reflex package a cell can load
//! (R12-EVID-2039, R12-AMBIENT-7202/3110..3114, R12-EXPAND-6025/6049..6051,
//! R12-REASON-7203/3115).
//!
//! Every test drives `ResearchLoop::sweep` or `sweep_over` — the same entry
//! `node::run_with` calls between cycles — and asserts its premise before
//! its property.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::reflex_package::{
    PackageSigner, PackageVerifier, ReflexInput, SignedReflexPackage,
};
use qip_core::error::{Error, Result};
use qip_core::{Context, Duration, Timestamp};
use qip_deepbrain::research::{FillObservation, ResearchConfig, ResearchLoop};
use qip_expansion::desk::CloseReason;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::connector::evidence::CheckedEvidence;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::{BlobStore, MemoryBlobStore};
use qip_world_model::evidence::{META_ATTRIBUTE, META_SUBJECT, META_VALUE};
use qip_world_model::{EvidenceFabric, EvidenceRecord};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const VENUE: &str = "XSIM";

fn t0() -> Timestamp {
    Timestamp::from_secs(1_800_000_000)
}

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(t0(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

fn checked(i: usize) -> CheckedEvidence {
    let hash: String = std::iter::repeat_n(char::from(b'a' + i as u8), 64).collect();
    let record = EvidenceRecord::new(
        format!("k{i}"),
        format!("https://site{i}.example"),
        t0().saturating_sub(Duration::from_mins(5)),
        t0().saturating_sub(Duration::from_mins(4)),
        "connector:test/1.0".to_string(),
        "commercial".to_string(),
        "Acme CEO arrested".to_string(),
        0.9,
    )
    .unwrap()
    .with_claimed_at(t0().saturating_sub(Duration::from_mins(10)))
    .with_metadata(META_SUBJECT.into(), "ACME".into())
    .with_metadata(META_ATTRIBUTE.into(), "ceo_status".into())
    .with_metadata(META_VALUE.into(), "arrested".into());
    let (verdict, _) = EvidenceFabric::default()
        .admit(record.clone(), Some(&hash), t0())
        .unwrap();
    CheckedEvidence {
        record,
        document_hash: hash,
        verdict,
    }
}

/// A coordinated campaign forwarded into the world fabric is found by the
/// sweep and raised as a data gap naming its sources, so a poisoned feed
/// becomes research into replacing it. The premise is the audit's finding:
/// before the forward the same sweep finds nothing. Mutation: delete the
/// poisoning loop in `sweep_over` — `round.poisoning` is empty and the
/// queue holds no such question.
#[test]
fn a_poisoned_feed_in_the_world_fabric_becomes_a_data_gap_naming_its_sources() {
    let mut platform = platform();
    let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new()).unwrap();
    let quiet = research
        .sweep(&platform, t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    assert!(quiet.poisoning.is_empty(), "premise: {quiet:?}");

    platform.absorb_evidence((0..3).map(checked).collect(), t0());
    let round = research
        .sweep(&platform, t0(), &set(&[]), &set(&["US"]))
        .unwrap();

    assert_eq!(round.evidence_held, 3, "{round:?}");
    assert_eq!(
        round.poisoning.get("coordinated_manipulation"),
        Some(&1),
        "{round:?}"
    );
    let queued: Vec<String> = research
        .desk()
        .queue()
        .ranked()
        .iter()
        .map(|e| e.item().research_question.clone())
        .filter(|q| q.contains("coordinated_manipulation from"))
        .collect();
    assert_eq!(queued.len(), 1, "{round:?}");
    assert!(queued[0].contains("https://site0.example"), "{queued:?}");
}

/// The ADR 0110 signer's stand-in: the plumbing, not the cryptography.
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

/// Ten scored fills before `t0`, then `post` after it, all ~40 bp off.
fn fills(post: usize) -> Vec<FillObservation> {
    let at = |offset_mins: i64| t0().saturating_add(Duration::from_mins(offset_mins));
    let mut out: Vec<FillObservation> = (0..10)
        .map(|i| FillObservation {
            venue: VENUE.to_string(),
            object_id: "BTC-USD".to_string(),
            filled_at: at(i - 60),
            trade_error_bps: Some(if i % 2 == 0 { 35.0 } else { 45.0 }),
        })
        .collect();
    out.extend((0..post as i64).map(|i| FillObservation {
        venue: VENUE.to_string(),
        object_id: "BTC-USD".to_string(),
        filled_at: at(i + 1),
        trade_error_bps: Some(if i % 2 == 0 { 38.0 } else { 42.0 }),
    }));
    out
}

/// The whole road in the binary's loop: the twin's fill residuals raise a
/// gap, the market grants it, the desk starts it, the next sweep's fills
/// complete a four-environment campaign that passes nine criteria, the desk
/// promotes it, and the compiler publishes a signed reflex package a cell
/// loads and applies at that venue only. Mutation: delete the
/// `self.distil(..)` call in `carry_out` — the capability is promoted but no
/// package is published, and the store assertion fails.
#[test]
fn a_fill_bias_found_by_research_is_promoted_and_published_as_a_loadable_reflex_package() {
    let platform = platform();
    let store = Arc::new(MemoryBlobStore::new());
    let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new())
        .unwrap()
        .with_package_signer(Box::new(TestKey))
        .with_package_store(store.clone());

    let first = research
        .sweep_over(&platform, &fills(0), &[], t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    assert!(
        first.desk.started.iter().any(|q| q.contains(VENUE)),
        "premise: the residual gap was granted and started: {first:?}"
    );
    assert_eq!(first.campaigns.get("waiting"), Some(&1), "{first:?}");

    let later = t0().saturating_add(Duration::from_hours(1));
    let second = research
        .sweep_over(&platform, &fills(8), &[], later, &set(&[]), &set(&["US"]))
        .unwrap();

    assert_eq!(second.campaigns.get("promoted"), Some(&1), "{second:?}");
    assert!(
        research
            .desk()
            .closed()
            .any(|c| c.reason == CloseReason::Promoted && c.question.contains(VENUE))
    );
    assert_eq!(second.packages.get("published"), Some(&1), "{second:?}");
    let keys = store.list("").unwrap();
    assert_eq!(keys.len(), 1, "{keys:?}");
    let bytes = store.get(&keys[0]).unwrap().unwrap();
    let loaded = SignedReflexPackage::load(&bytes, &TestKey, later).unwrap();
    let at = |venue: &str| {
        BTreeMap::from([("venue".to_string(), ReflexInput::Label(venue.to_string()))])
    };
    assert_eq!(
        loaded.evaluate(&at(VENUE)).get("twin_entry_bps"),
        Some(&-40)
    );
    assert!(loaded.evaluate(&at("ELSEWHERE")).is_empty());
}

/// Without a signer the compiled package is held unsigned and nothing is
/// written: there is no unsigned path to a cell. Mutation: write the
/// unsigned package's encoding in the `(None, _)` arm — the store holds a
/// key and the assertion fails.
#[test]
fn without_a_signer_a_compiled_package_is_held_and_nothing_is_published() {
    let platform = platform();
    let store = Arc::new(MemoryBlobStore::new());
    let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new())
        .unwrap()
        .with_package_store(store.clone());
    research
        .sweep_over(&platform, &fills(0), &[], t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    let round = research
        .sweep_over(
            &platform,
            &fills(8),
            &[],
            t0().saturating_add(Duration::from_hours(1)),
            &set(&[]),
            &set(&["US"]),
        )
        .unwrap();
    assert_eq!(
        round.packages.get("held_unsigned"),
        Some(&1),
        "premise: a package was compiled: {round:?}"
    );
    assert!(store.list("").unwrap().is_empty());
    assert!(research.packages().all(|p| p.signed.is_none()));
}

/// Research whose acquisition has no executor realises nothing, and the
/// market retires it after its window rather than letting it hold a
/// started slot for twenty-four sweeps; the same question raised again is
/// declined as retired. Mutation: drop the `desk.retire` call in
/// `measure_last_round` — no sweep reports a retirement and the first
/// assertion fails.
#[test]
fn research_that_realises_nothing_is_retired_by_the_market_and_stays_retired() {
    let platform = platform();
    let coverage = BTreeMap::from([("Equity".to_string(), set(&["obj-A"]))]);
    let mut research = ResearchLoop::new(ResearchConfig::default(), coverage).unwrap();
    let first = research
        .sweep(&platform, t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    assert_eq!(first.desk.started.len(), 1, "premise: {first:?}");

    let mut retired_at = None;
    let mut declined_after = 0;
    for sweep in 1..8 {
        let now = t0().saturating_add(Duration::from_mins(sweep));
        let round = research
            .sweep(&platform, now, &set(&[]), &set(&["US"]))
            .unwrap();
        if round.retired > 0 && retired_at.is_none() {
            retired_at = Some(sweep);
        } else if retired_at.is_some() {
            declined_after += round.declined.get("retired").copied().unwrap_or(0);
        }
    }

    let retired_at = retired_at.expect("the unproductive item was never retired");
    assert!(
        retired_at <= i64::from(research.meta().policy().retire_after) + 1,
        "retired at sweep {retired_at}"
    );
    assert!(
        research
            .desk()
            .closed()
            .any(|c| c.reason == CloseReason::Unproductive)
    );
    assert!(
        declined_after >= 1,
        "the retired question was granted again"
    );
    assert!(
        research
            .acquisitions()
            .all(|a| a.outcome.as_str() == "not_executable"),
        "premise: nothing could be carried out for a coverage gap"
    );
}
