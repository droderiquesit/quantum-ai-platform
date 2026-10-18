//! Quantum-assisted research reaches capability only through
//! `qip_expansion::promotion::promote`, and `promote` asks every environment
//! and every criterion — `IncrementalValue`, the question of whether the
//! result beat its classical baseline (ADR 0006), included.
//!
//! `qip-expansion/tests/loop.rs` drives this through the desk's `conclude`;
//! this suite drives `promote` itself, once per criterion, so a criterion
//! skipped by the gate fails here by name rather than being masked by the
//! one failing criterion the desk test happens to choose. A "we used a
//! quantum computer" result that lost to its baseline must not arrive as
//! capability because nobody wired the gate to the exotic class.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::expansion::{CapabilityVerb, GapClass};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_expansion::desk::question_for;
use qip_expansion::gap::{GapTrigger, Observation, Wanting, raise};
use qip_expansion::plan::Environment;
use qip_expansion::promotion::{
    Criterion, EvaluationRecord, Finding, Provenance, ResearchCampaign, promote,
};

fn quantum_question() -> Result<String> {
    let raised = raise(Observation {
        trigger: GapTrigger::UnsupportedRequest,
        wanting: vec![Wanting::Compute],
        observation: "a 40-asset allocation the classical solver bounds but does not close".into(),
        affected_domains: vec!["optimization".into()],
        evidence: vec!["solver-gap:portfolio-40".into()],
        severity: 0.6,
        economic_value: Decimal::from_int(5),
    })?;
    assert_eq!(
        raised.signal.gap_class,
        GapClass::ComputeQuantumResearch,
        "premise: this gap is quantum research"
    );
    Ok(question_for(&raised))
}

fn provenance() -> Provenance {
    Provenance {
        dataset_digest: "sha256:quantum-promotion".into(),
        lineage: vec!["tape:1".into()],
        seed: 7,
    }
}

fn campaign_through(question: &str, environments: usize) -> Result<ResearchCampaign> {
    let mut campaign = ResearchCampaign::open(question)?;
    for environment in &Environment::ORDER[..environments] {
        campaign.record(*environment, provenance())?;
    }
    Ok(campaign)
}

fn evaluation(question: &str, failing: Option<Criterion>) -> Result<EvaluationRecord> {
    EvaluationRecord::new(
        question,
        Criterion::ALL
            .iter()
            .map(|criterion| {
                (
                    *criterion,
                    Finding {
                        passed: Some(*criterion) != failing,
                        evidence: format!("eval:{criterion:?}"),
                    },
                )
            })
            .collect(),
    )
}

#[test]
fn quantum_research_is_promoted_when_every_environment_and_criterion_holds() -> Result<()> {
    let question = quantum_question()?;
    let campaign = campaign_through(&question, Environment::ORDER.len())?;
    assert_eq!(Criterion::ALL.len(), 9, "premise: nine criteria");
    assert!(campaign.is_complete(), "premise: four environments ran");
    let capability = promote(
        GapClass::ComputeQuantumResearch,
        &campaign,
        &evaluation(&question, None)?,
    )?;
    assert_eq!(capability.verb, CapabilityVerb::Simulate);
    assert_eq!(capability.eligibility, "paper and shadow only");
    Ok(())
}

#[test]
fn quantum_research_failing_any_single_criterion_is_refused_naming_that_criterion() -> Result<()> {
    let question = quantum_question()?;
    let campaign = campaign_through(&question, Environment::ORDER.len())?;
    assert!(
        Criterion::ALL.contains(&Criterion::IncrementalValue),
        "premise: the baseline criterion is among the nine"
    );
    for criterion in Criterion::ALL {
        let refusal = promote(
            GapClass::ComputeQuantumResearch,
            &campaign,
            &evaluation(&question, Some(criterion))?,
        )
        .err()
        .ok_or_else(|| {
            Error::invalid(format!(
                "research failing {criterion:?} was promoted to a capability"
            ))
        })?;
        assert!(
            refusal.message().contains(&format!("{criterion:?}")),
            "{}",
            refusal.message()
        );
    }
    Ok(())
}

#[test]
fn quantum_research_that_stopped_before_shadow_is_refused_however_good_the_evaluation() -> Result<()>
{
    let question = quantum_question()?;
    let passing = evaluation(&question, None)?;
    for ran in 0..Environment::ORDER.len() {
        let campaign = campaign_through(&question, ran)?;
        assert!(!campaign.is_complete(), "premise: {ran} of 4 is incomplete");
        assert!(
            promote(GapClass::ComputeQuantumResearch, &campaign, &passing).is_err(),
            "a campaign that ran {ran} environments was promoted"
        );
    }
    Ok(())
}
