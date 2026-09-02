//! The resolver.
//!
//! Three stages: block to find plausible candidates, score each with explicit
//! evidence, then decide. The decision has three outcomes, not two — a match
//! the resolver is unsure of is held rather than forced, because a wrong merge
//! attributes one company's news to another's securities and is far more
//! damaging than a duplicate entity that a later record will resolve.

use qip_core::error::Result;
use qip_core::{Context, EntityId, Timestamp};
use qip_financial::identifiers::IdentifierKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::entity::{Entity, EntityKind, EntityRecord, EntityResolved};
use crate::matching::{MatchEvidence, name_similarity, normalise_name};

/// Thresholds governing the resolver.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolverConfig {
    /// Score above which a link is made automatically.
    pub link_threshold: f64,
    /// Score below which a record is treated as a new entity.
    ///
    /// Between the two, the record is held for review. A wide band is
    /// deliberate: the cost of a wrong merge is much higher than the cost of
    /// waiting.
    pub new_entity_threshold: f64,
    /// Minimum name similarity for a candidate to be scored at all.
    pub blocking_similarity: f64,
    /// Maximum candidates to score per record.
    pub max_candidates: usize,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            link_threshold: 0.90,
            new_entity_threshold: 0.72,
            blocking_similarity: 0.55,
            max_candidates: 25,
        }
    }
}

/// What the resolver concluded about a record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Linked to an existing entity.
    Linked {
        entity_id: EntityId,
        score: f64,
        evidence: Vec<MatchEvidence>,
    },
    /// No plausible match; a new entity was created.
    Created { entity_id: EntityId },
    /// Plausible matches exist but none is convincing. Held for review.
    Ambiguous {
        candidates: Vec<(EntityId, f64)>,
        reason: String,
    },
}

impl Decision {
    pub fn entity_id(&self) -> Option<&EntityId> {
        match self {
            Self::Linked { entity_id, .. } | Self::Created { entity_id } => Some(entity_id),
            Self::Ambiguous { .. } => None,
        }
    }

    pub fn is_ambiguous(&self) -> bool {
        matches!(self, Self::Ambiguous { .. })
    }

    /// A one-line explanation, recorded on the resolution event.
    pub fn rationale(&self) -> String {
        match self {
            Self::Linked {
                score, evidence, ..
            } => {
                let strongest = evidence
                    .iter()
                    .filter(|e| !e.is_disconfirming())
                    .max_by(|a, b| {
                        a.strength
                            .partial_cmp(&b.strength)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                match strongest {
                    Some(e) => format!("linked at {score:.3} on {}: {}", e.signal, e.detail),
                    None => format!("linked at {score:.3}"),
                }
            }
            Self::Created { .. } => "no plausible existing entity".into(),
            Self::Ambiguous { reason, .. } => reason.clone(),
        }
    }
}

/// Summary of a resolution run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResolutionReport {
    pub processed: u64,
    pub linked: u64,
    pub created: u64,
    pub ambiguous: u64,
    /// Records held for review, with their reasons.
    pub held: Vec<(String, String)>,
}

impl ResolutionReport {
    /// Fraction of records the resolver could decide automatically.
    pub fn automation_rate(&self) -> f64 {
        if self.processed == 0 {
            return 1.0;
        }
        (self.linked + self.created) as f64 / self.processed as f64
    }
}

/// Resolves records onto canonical entities.
#[derive(Debug)]
pub struct Resolver {
    entities: BTreeMap<String, Entity>,
    /// Normalised name to entity ids, for blocking.
    name_index: BTreeMap<String, Vec<String>>,
    /// Identifier to entity id, for exact lookup.
    identifier_index: BTreeMap<(IdentifierKind, String), String>,
    config: ResolverConfig,
    report: ResolutionReport,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new(ResolverConfig::default())
    }
}

impl Resolver {
    pub fn new(config: ResolverConfig) -> Self {
        Self {
            entities: BTreeMap::new(),
            name_index: BTreeMap::new(),
            identifier_index: BTreeMap::new(),
            config,
            report: ResolutionReport::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn report(&self) -> &ResolutionReport {
        &self.report
    }

    pub fn get(&self, id: &EntityId) -> Option<&Entity> {
        self.entities.get(id.as_str())
    }

    pub fn entities(&self) -> impl Iterator<Item = &Entity> {
        self.entities.values()
    }

    /// Register an entity directly, for seeding a known universe.
    pub fn insert(&mut self, entity: Entity) {
        self.index(&entity);
        self.entities
            .insert(entity.entity_id.as_str().to_string(), entity);
    }

    /// Resolve a record, updating the entity set.
    pub fn resolve(
        &mut self,
        record: &EntityRecord,
        context: &Context,
    ) -> (Decision, EntityResolved) {
        self.report.processed += 1;
        let now = context.now();

        let candidates = self.score_candidates(record);
        let decision = self.decide(record, candidates, context);

        match &decision {
            Decision::Linked {
                entity_id, score, ..
            } => {
                self.report.linked += 1;
                if let Some(entity) = self.entities.get_mut(entity_id.as_str()) {
                    entity.absorb(record, *score, now);
                    let snapshot = entity.clone();
                    self.index(&snapshot);
                }
            }
            Decision::Created { entity_id } => {
                self.report.created += 1;
                let mut entity =
                    Entity::new(entity_id.clone(), record.kind, record.name.clone(), now);
                entity.absorb(record, 1.0, now);
                self.index(&entity);
                self.entities.insert(entity_id.as_str().to_string(), entity);
            }
            Decision::Ambiguous { reason, .. } => {
                self.report.ambiguous += 1;
                self.report
                    .held
                    .push((record.record_id.clone(), reason.clone()));
            }
        }

        let event = EntityResolved {
            record_id: record.record_id.clone(),
            source: record.source.clone(),
            entity_id: decision
                .entity_id()
                .cloned()
                .unwrap_or_else(|| EntityId::from_string("unresolved")),
            confidence: match &decision {
                Decision::Linked { score, .. } => *score,
                Decision::Created { .. } => 1.0,
                Decision::Ambiguous { .. } => 0.0,
            },
            rationale: decision.rationale(),
            created_new: matches!(decision, Decision::Created { .. }),
            resolved_at: now,
        };
        (decision, event)
    }

    /// Score every plausible candidate for a record, best first.
    pub fn score_candidates(
        &self,
        record: &EntityRecord,
    ) -> Vec<(String, f64, Vec<MatchEvidence>)> {
        let mut candidate_ids: Vec<String> = Vec::new();

        // Blocking on identifiers is exact and cheap; do it first.
        for (kind, value) in record.identifiers.iter() {
            if let Some(id) = self
                .identifier_index
                .get(&(kind, value.to_ascii_uppercase()))
            {
                candidate_ids.push(id.clone());
            }
        }

        // Then on name, comparing against every alias.
        let normalised = normalise_name(&record.name);
        if let Some(exact) = self.name_index.get(&normalised) {
            candidate_ids.extend(exact.iter().cloned());
        }
        for (indexed_name, ids) in &self.name_index {
            if candidate_ids.len() >= self.config.max_candidates * 4 {
                break;
            }
            if name_similarity(indexed_name, &normalised) >= self.config.blocking_similarity {
                candidate_ids.extend(ids.iter().cloned());
            }
        }

        candidate_ids.sort();
        candidate_ids.dedup();

        let mut scored: Vec<(String, f64, Vec<MatchEvidence>)> = candidate_ids
            .into_iter()
            .filter_map(|id| {
                let entity = self.entities.get(&id)?;
                if entity.kind != record.kind {
                    return None;
                }
                let (score, evidence) = score_pair(entity, record);
                Some((id, score, evidence))
            })
            .collect();

        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(self.config.max_candidates);
        scored
    }

    fn decide(
        &self,
        record: &EntityRecord,
        candidates: Vec<(String, f64, Vec<MatchEvidence>)>,
        context: &Context,
    ) -> Decision {
        let Some((best_id, best_score, evidence)) = candidates.first().cloned() else {
            return Decision::Created {
                entity_id: context.ids().generate(context.now()),
            };
        };

        if best_score >= self.config.link_threshold {
            // A second candidate scoring nearly as well means the evidence does
            // not actually distinguish them, however high the top score is.
            if let Some((_, runner_up, _)) = candidates.get(1)
                && best_score - runner_up < 0.05
            {
                return Decision::Ambiguous {
                    candidates: candidates
                        .iter()
                        .take(3)
                        .map(|(id, score, _)| (EntityId::from_string(id.clone()), *score))
                        .collect(),
                    reason: format!(
                        "`{}` matches two entities almost equally well ({best_score:.3} and \
                         {runner_up:.3}); linking either would be a guess",
                        record.name
                    ),
                };
            }
            return Decision::Linked {
                entity_id: EntityId::from_string(best_id),
                score: best_score,
                evidence,
            };
        }

        if best_score < self.config.new_entity_threshold {
            return Decision::Created {
                entity_id: context.ids().generate(context.now()),
            };
        }

        Decision::Ambiguous {
            candidates: candidates
                .iter()
                .take(3)
                .map(|(id, score, _)| (EntityId::from_string(id.clone()), *score))
                .collect(),
            reason: format!(
                "`{}` scored {best_score:.3}, between the new-entity threshold {:.2} and the link \
                 threshold {:.2}",
                record.name, self.config.new_entity_threshold, self.config.link_threshold
            ),
        }
    }

    fn index(&mut self, entity: &Entity) {
        let id = entity.entity_id.as_str().to_string();
        for alias in &entity.aliases {
            let key = normalise_name(alias);
            let bucket = self.name_index.entry(key).or_default();
            if !bucket.contains(&id) {
                bucket.push(id.clone());
            }
        }
        for (kind, value) in entity.identifiers.iter() {
            self.identifier_index
                .insert((kind, value.to_ascii_uppercase()), id.clone());
        }
    }

    /// Merge two entities, keeping the first. Used to resolve a review by hand.
    pub fn merge(&mut self, keep: &EntityId, absorb: &EntityId, at: Timestamp) -> Result<()> {
        let Some(absorbed) = self.entities.remove(absorb.as_str()) else {
            return Err(qip_core::Error::not_found(format!("no entity {absorb}")));
        };
        let Some(target) = self.entities.get_mut(keep.as_str()) else {
            // Put it back rather than losing it.
            self.entities.insert(absorb.as_str().to_string(), absorbed);
            return Err(qip_core::Error::not_found(format!("no entity {keep}")));
        };
        for alias in &absorbed.aliases {
            target.add_alias(alias);
        }
        target.identifiers.merge(&absorbed.identifiers);
        for (key, value) in &absorbed.attributes {
            target
                .attributes
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        for source in &absorbed.contributing_sources {
            if !target.contributing_sources.contains(source) {
                target.contributing_sources.push(source.clone());
            }
        }
        target.updated_at = at;
        target.coherence = target.coherence.min(absorbed.coherence);
        let snapshot = target.clone();

        // Re-point the absorbed entity's index entries at the survivor.
        for ids in self.name_index.values_mut() {
            for id in ids.iter_mut() {
                if id == absorb.as_str() {
                    *id = keep.as_str().to_string();
                }
            }
            ids.dedup();
        }
        for id in self.identifier_index.values_mut() {
            if id == absorb.as_str() {
                *id = keep.as_str().to_string();
            }
        }
        self.index(&snapshot);
        Ok(())
    }

    /// Entities whose coherence has fallen below the reliability bar.
    pub fn unreliable(&self) -> Vec<&Entity> {
        self.entities
            .values()
            .filter(|e| !e.is_reliable())
            .collect()
    }
}

/// Score one candidate entity against a record, collecting the evidence.
fn score_pair(entity: &Entity, record: &EntityRecord) -> (f64, Vec<MatchEvidence>) {
    let mut evidence = Vec::new();

    // Identifier agreement is the strongest signal available, and identifier
    // *disagreement* is decisive the other way.
    let identifier_strength = entity.identifiers.match_strength(&record.identifiers);
    let has_shared_identifier_kind = record
        .identifiers
        .iter()
        .any(|(kind, _)| entity.identifiers.contains(kind));

    if identifier_strength > 0.0 {
        evidence.push(MatchEvidence::new(
            "identifier",
            identifier_strength,
            format!("agreeing identifier at strength {identifier_strength:.2}"),
        ));
        // A strong identifier match settles it; nothing else should override it.
        if identifier_strength >= 0.9 {
            return (identifier_strength.min(1.0), evidence);
        }
    } else if has_shared_identifier_kind {
        evidence.push(MatchEvidence::new(
            "identifier",
            -1.0,
            "a strong identifier is present on both and they disagree".to_string(),
        ));
        return (0.0, evidence);
    }

    // Name similarity against the best-matching alias.
    let name_score = entity
        .aliases
        .iter()
        .map(|alias| name_similarity(alias, &record.name))
        .fold(0.0f64, f64::max);
    evidence.push(MatchEvidence::new(
        "name",
        name_score,
        format!("best alias similarity {name_score:.2}"),
    ));

    // Country agreement corroborates; disagreement is strong evidence against,
    // since two companies with similar names in different countries are
    // usually genuinely different companies.
    let mut score = name_score;
    match (&entity.country, &record.country) {
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => {
            evidence.push(MatchEvidence::new("country", 0.2, format!("both in {a}")));
            score = (score + 0.08).min(1.0);
        }
        (Some(a), Some(b)) => {
            evidence.push(MatchEvidence::new(
                "country",
                -0.5,
                format!("{a} against {b}"),
            ));
            score *= 0.5;
        }
        _ => {}
    }

    // Agreeing attributes nudge upward; each is weak on its own.
    let agreeing = record
        .attributes
        .iter()
        .filter(|(key, value)| entity.attributes.get(*key) == Some(*value))
        .count();
    if agreeing > 0 {
        let boost = (agreeing as f64 * 0.03).min(0.09);
        evidence.push(MatchEvidence::new(
            "attributes",
            boost,
            format!("{agreeing} agreeing attributes"),
        ));
        score = (score + boost).min(1.0);
    }

    (score.clamp(0.0, 1.0), evidence)
}

/// The entity kinds that always exist, seeded at start-up.
pub fn seed_reference_entities(resolver: &mut Resolver, context: &Context) {
    let now = context.now();
    for (name, kind) in [
        ("United States", EntityKind::Country),
        ("Euro Area", EntityKind::Country),
        ("United Kingdom", EntityKind::Country),
        ("Federal Reserve", EntityKind::CentralBank),
        ("European Central Bank", EntityKind::CentralBank),
        ("Bank of England", EntityKind::CentralBank),
    ] {
        let entity = Entity::new(context.ids().generate(now), kind, name, now);
        resolver.insert(entity);
    }
}
