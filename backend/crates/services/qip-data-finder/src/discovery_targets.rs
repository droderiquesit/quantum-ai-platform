//! Where source discovery starts: what the platform is wrong about, or does
//! not know, and has no source for (DATA-031), and the queries that follow
//! from it (DATA-032).
//!
//! Discovery used to begin from an operator's candidate list, so the platform
//! looked only where a person already thought to look, and its own forecast
//! errors — the cheapest evidence that a source is missing — went unread. A
//! forecast error on an entity something already covers is not a discovery
//! problem (the source exists and is wrong, which `health` handles), so only an
//! uncovered entity becomes a target.
//!
//! Pure functions over supplied observations: no clock, no I/O, deterministic
//! order. Nothing here fetches anything; the targets are an input to
//! `DataFinder::assess`, whose legal and robots gates still apply to whatever
//! a query eventually finds.

use crate::coverage::SourceRegion;
use crate::source::SourceCandidate;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// What is known about an entity for discovery purposes. Every field is
/// required: a query with no language cannot be sent, and guessing one would
/// search the wrong web.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityProfile {
    pub entity: String,
    pub geographies: Vec<String>,
    pub domains: Vec<String>,
    pub languages: Vec<String>,
}

/// A gap the world model reports: an entity it holds no recent knowledge of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeGap {
    pub entity: String,
}

/// A realised forecast error against the error the model normally makes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForecastError {
    pub entity: String,
    pub error: f64,
    /// The entity's usual absolute error; a spike is relative to it.
    pub baseline: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TargetReason {
    KnowledgeGap,
    ForecastErrorSpike { ratio: f64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryTarget {
    pub entity: String,
    pub reason: TargetReason,
}

/// One discovery query, always carrying all four dimensions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryQuery {
    pub entity: String,
    pub geography: String,
    pub domain: String,
    pub language: String,
    pub text: String,
}

fn nonempty(label: &str, v: &str) -> Result<()> {
    if v.trim().is_empty() {
        Err(Error::invalid(format!(
            "{label} is empty; state it rather than letting discovery guess"
        )))
    } else {
        Ok(())
    }
}

/// Targets from gaps and forecast-error spikes, uncovered entities only.
///
/// A spike is `error / baseline >= spike_multiple`. A baseline that is not
/// positive and finite is refused: dividing by it would manufacture a spike
/// out of an entity nobody has measured.
pub fn targets_from(
    gaps: &[KnowledgeGap],
    errors: &[ForecastError],
    covered: &BTreeSet<String>,
    spike_multiple: f64,
) -> Result<Vec<DiscoveryTarget>> {
    if !(spike_multiple.is_finite() && spike_multiple > 1.0) {
        return Err(Error::invalid(
            "spike_multiple must be finite and above 1; a multiple of 1 or less flags every error",
        ));
    }
    let mut by_entity: BTreeMap<String, TargetReason> = BTreeMap::new();
    for gap in gaps {
        nonempty("gap entity", &gap.entity)?;
        if !covered.contains(&gap.entity) {
            by_entity
                .entry(gap.entity.clone())
                .or_insert(TargetReason::KnowledgeGap);
        }
    }
    for e in errors {
        nonempty("forecast-error entity", &e.entity)?;
        if !(e.error.is_finite() && e.baseline.is_finite() && e.baseline > 0.0 && e.error >= 0.0) {
            return Err(Error::invalid(format!(
                "forecast error for {} needs a finite non-negative error and a positive finite \
                 baseline",
                e.entity
            )));
        }
        let ratio = e.error / e.baseline;
        if ratio >= spike_multiple && !covered.contains(&e.entity) {
            // A spike outranks a bare gap: it is evidence, not an absence.
            let keep = match by_entity.get(&e.entity) {
                Some(TargetReason::ForecastErrorSpike { ratio: r }) => ratio > *r,
                _ => true,
            };
            if keep {
                by_entity.insert(e.entity.clone(), TargetReason::ForecastErrorSpike { ratio });
            }
        }
    }
    let mut out: Vec<DiscoveryTarget> = by_entity
        .into_iter()
        .map(|(entity, reason)| DiscoveryTarget { entity, reason })
        .collect();
    // Largest spike first, then gaps; ties by entity name (already sorted).
    out.sort_by(|a, b| rank(&b.reason).total_cmp(&rank(&a.reason)));
    Ok(out)
}

fn rank(r: &TargetReason) -> f64 {
    match r {
        TargetReason::ForecastErrorSpike { ratio } => *ratio,
        TargetReason::KnowledgeGap => 0.0,
    }
}

/// One query per geography x domain x language of the target's profile.
/// Refuses a profile missing any dimension, so no query is untagged.
pub fn queries_for(
    target: &DiscoveryTarget,
    profile: &EntityProfile,
) -> Result<Vec<DiscoveryQuery>> {
    if profile.entity != target.entity {
        return Err(Error::invalid(format!(
            "profile is for {} but the target is {}",
            profile.entity, target.entity
        )));
    }
    nonempty("entity", &profile.entity)?;
    for (label, values) in [
        ("geographies", &profile.geographies),
        ("domains", &profile.domains),
        ("languages", &profile.languages),
    ] {
        if values.is_empty() {
            return Err(Error::invalid(format!(
                "{} has no {label}; discovery needs every dimension",
                profile.entity
            )));
        }
        for v in values {
            nonempty(label, v)?;
        }
    }
    let mut out = Vec::new();
    for geography in &profile.geographies {
        for domain in &profile.domains {
            for language in &profile.languages {
                out.push(DiscoveryQuery {
                    entity: profile.entity.clone(),
                    geography: geography.clone(),
                    domain: domain.clone(),
                    language: language.clone(),
                    text: format!("{} {domain} {geography} lang:{language}", profile.entity),
                });
            }
        }
    }
    Ok(out)
}

/// Somewhere a query could be answered from: a catalogued candidate that
/// declares the query's entity and serves its geography.
///
/// The four tags are the query's — what the location is proposed *for*, not
/// what its publisher has been shown to hold. `DataFinder::assess` finds
/// that out, under the same legal and robots gates as any other candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateLocation {
    pub entity: String,
    pub geography: String,
    pub domain: String,
    pub language: String,
    pub source_id: String,
    pub locator: String,
}

/// The catalogued candidates that could answer `query`, in catalogue order.
///
/// A location is built only from a query and carries its tags, and a query
/// missing any of the four is refused here — its fields are public, so one
/// can be written by hand without [`queries_for`] — which is what leaves no
/// way to an untagged location. A candidate qualifies when it declares the
/// entity among its instruments and serves the geography: a declared region
/// whose [`SourceRegion::as_str`] is the query's geography, or
/// [`SourceRegion::Global`]. Nothing is invented: with no such candidate the
/// answer is empty, which says the catalogue has nowhere to look and is the
/// finding CRAWL (DATA-020) would act on.
pub fn locations_for(
    query: &DiscoveryQuery,
    catalogue: &[SourceCandidate],
) -> Result<Vec<CandidateLocation>> {
    for (label, value) in [
        ("entity", &query.entity),
        ("geography", &query.geography),
        ("domain", &query.domain),
        ("language", &query.language),
    ] {
        nonempty(label, value)?;
    }
    Ok(catalogue
        .iter()
        .filter(|candidate| {
            let coverage = candidate.declared_coverage();
            coverage.instruments().contains(&query.entity)
                && coverage.regions().iter().any(|region| {
                    *region == SourceRegion::Global || region.as_str() == query.geography
                })
        })
        .map(|candidate| CandidateLocation {
            entity: query.entity.clone(),
            geography: query.geography.clone(),
            domain: query.domain.clone(),
            language: query.language.clone(),
            source_id: candidate.id().to_string(),
            locator: candidate.endpoint().url(),
        })
        .collect())
}
