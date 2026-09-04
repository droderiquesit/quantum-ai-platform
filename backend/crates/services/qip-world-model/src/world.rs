//! The world model: the aggregate that absorbs observations and answers
//! questions about what the platform believes.

use qip_ai::embedding::{Embedder, HashingEmbedder};
use qip_ai::retrieval::{Document, RetrievalResult, SearchIndex};
use qip_core::error::Result;
use qip_core::{Context, Duration, Timestamp};
use qip_entity_resolution::entity::{Entity, EntityKind, EntityRecord};
use qip_entity_resolution::resolver::Resolver;
use qip_financial::intelligence::{
    AlternativeDataPoint, FundamentalUpdate, MacroObservation, NewsItem,
};
use qip_market::bar::Bar;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::causal::{CausalEdge, CausalGraph, Mechanism, PropagationResult};
use crate::features::{Feature, FeatureStore, FeatureValue};
use crate::graph::{Fact, KnowledgeGraph, Node, NodeKind};
use crate::relationship::{Relationship, RelationshipKind};
use crate::state::{Change, ChangeKind, WorldDiff, WorldState};
use crate::vocabulary::{AltMetric, MacroSeries, SubjectKind, names};

/// Published when the world model absorbs something material.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldModelUpdated {
    pub subject: String,
    pub change_kind: String,
    pub description: String,
    pub materiality: f64,
    pub at: Timestamp,
}

impl qip_events::EventBody for WorldModelUpdated {
    const TOPIC: qip_events::Topic = qip_events::Topic::WorldModelUpdated;
    const SCHEMA_VERSION: u32 = 1;
}

/// The platform's model of the world.
#[derive(Debug)]
pub struct WorldModel {
    graph: KnowledgeGraph,
    causal: CausalGraph,
    features: FeatureStore,
    resolver: Resolver,
    index: SearchIndex,
    /// Changes recorded in absorption order, for diffing.
    journal: Vec<Change>,
}

impl Default for WorldModel {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldModel {
    pub fn new() -> Self {
        let embedder: Arc<dyn Embedder> = Arc::new(HashingEmbedder::default());
        let mut model = Self {
            graph: KnowledgeGraph::new(),
            causal: CausalGraph::new(),
            features: FeatureStore::new(),
            resolver: Resolver::default(),
            index: SearchIndex::new().with_embedder(embedder),
            journal: Vec::new(),
        };
        model.define_standard_features();
        model
    }

    /// The features the platform always computes, spelled by the vocabulary.
    fn define_standard_features(&mut self) {
        for feature in [
            Feature::new(names::CLOSE, "last traded price", "market-ingestion")
                .with_staleness(Duration::from_days(5)),
            Feature::new(
                names::REALISED_VOLATILITY_20D,
                "20-day realised volatility",
                "quant",
            )
            .with_staleness(Duration::from_days(5)),
            Feature::new(names::VOLUME, "session volume", "market-ingestion")
                .with_staleness(Duration::from_days(5)),
            Feature::new(names::REVENUE, "reported quarterly revenue", "fundamentals")
                // Fundamentals arrive weeks after the period they describe, and
                // the lag is the whole reason point-in-time access matters.
                .with_lag(Duration::from_days(30))
                .with_staleness(Duration::from_days(200)),
            Feature::new(
                names::REVENUE_SURPRISE,
                "revenue against consensus",
                "fundamentals",
            )
            .with_lag(Duration::from_days(30))
            .with_staleness(Duration::from_days(200)),
            Feature::new(names::SENTIMENT, "aggregate news sentiment", "world-model")
                .with_subject_kind(SubjectKind::Entity)
                .with_staleness(Duration::from_days(10)),
            Feature::new(names::MACRO_LEVEL, "macroeconomic series level", "macro")
                .with_lag(Duration::from_days(15))
                .with_staleness(Duration::from_days(120)),
            Feature::new(
                names::MACRO_SURPRISE,
                "macro release against consensus",
                "macro",
            )
            .with_lag(Duration::from_days(15))
            .with_staleness(Duration::from_days(120)),
        ] {
            self.features.define(feature);
        }
        // The series the macro analyst reads, keyed by economy. Monthly at
        // best, published weeks after the period, so the staleness bound is
        // the same one the raw macro series carries.
        for series in MacroSeries::ALL {
            self.features.define(
                Feature::new(
                    series.feature(),
                    format!("{} by economy", series.series_code()),
                    "macro",
                )
                .with_subject_kind(SubjectKind::Economy)
                .with_lag(Duration::from_days(15))
                .with_staleness(Duration::from_days(120)),
            );
        }
        // The series the alternative-data analyst reads, keyed by the
        // reading's subject. The producer is the dataset — the name the
        // licence is held under — so the analyst can see whose series it is
        // reading before it decides whether it may.
        for metric in AltMetric::ALL {
            self.features.define(
                Feature::new(
                    metric.feature(),
                    format!("{} from the {} dataset", metric.feature(), metric.dataset()),
                    metric.dataset(),
                )
                .with_subject_kind(SubjectKind::Instrument)
                .with_staleness(Duration::from_days(30)),
            );
        }
    }

    pub fn graph(&self) -> &KnowledgeGraph {
        &self.graph
    }

    pub fn graph_mut(&mut self) -> &mut KnowledgeGraph {
        &mut self.graph
    }

    pub fn causal(&self) -> &CausalGraph {
        &self.causal
    }

    pub fn features(&self) -> &FeatureStore {
        &self.features
    }

    pub fn features_mut(&mut self) -> &mut FeatureStore {
        &mut self.features
    }

    pub fn resolver(&self) -> &Resolver {
        &self.resolver
    }

    pub fn index(&self) -> &SearchIndex {
        &self.index
    }

    pub fn changes(&self) -> &[Change] {
        &self.journal
    }

    /// Register an entity and add it to the graph.
    pub fn add_entity(&mut self, entity: Entity) {
        let node = Node::new(
            entity.entity_id.as_str(),
            NodeKind::Entity,
            &entity.canonical_name,
            entity.created_at,
        )
        .with_attribute("kind", entity.kind.as_str());
        self.graph.add_node(node);
        self.journal.push(Change::new(
            ChangeKind::EntityAdded,
            entity.entity_id.as_str(),
            format!("entity `{}` added", entity.canonical_name),
            0.3,
            entity.created_at,
        ));
        self.resolver.insert(entity);
    }

    /// Assert a relationship, adding it to the graph.
    pub fn relate(
        &mut self,
        relationship: Relationship,
        valid_from: Timestamp,
        recorded_at: Timestamp,
        confidence: f64,
    ) {
        let description = format!(
            "{} {} {}",
            relationship.from,
            relationship.kind.as_str(),
            relationship.to
        );
        let subject = relationship.key();
        self.graph.assert_fact(
            Fact::new(relationship, valid_from, recorded_at).with_confidence(confidence),
        );
        self.journal.push(Change::new(
            ChangeKind::RelationshipAdded,
            subject,
            description,
            0.4,
            recorded_at,
        ));
    }

    /// Record a causal claim.
    pub fn claim_causal(&mut self, edge: CausalEdge) {
        let description = format!(
            "{} affects {} via {}",
            edge.cause,
            edge.effect,
            edge.mechanism.as_str()
        );
        let at = edge.recorded_at;
        let subject = format!("{}->{}", edge.cause, edge.effect);
        let materiality = edge.transmission().clamp(0.0, 1.0);
        self.causal.add(edge);
        self.journal.push(Change::new(
            ChangeKind::CausalClaimAdded,
            subject,
            description,
            materiality,
            at,
        ));
    }

    /// Absorb a news item: resolve its entities, index it, update sentiment.
    pub fn absorb_news(&mut self, item: &NewsItem, context: &Context) -> Vec<String> {
        let now = context.now();
        let mut resolved = Vec::new();

        for mention in &item.entities {
            let record = EntityRecord::new(
                format!("{}:{}", item.item_id, mention.text),
                EntityKind::Company,
                &mention.text,
                item.provenance.source.clone(),
                item.published_at,
            );
            let (decision, _) = self.resolver.resolve(&record, context);
            if let Some(entity_id) = decision.entity_id() {
                let id = entity_id.as_str().to_string();
                if self.graph.node(&id).is_none()
                    && let Some(entity) = self.resolver.get(entity_id)
                {
                    let node = Node::new(&id, NodeKind::Entity, &entity.canonical_name, now)
                        .with_attribute("kind", entity.kind.as_str());
                    self.graph.add_node(node);
                }
                resolved.push(id.clone());

                // The item becomes an evidence node linked to the entity.
                let event_id = format!("news:{}", item.item_id);
                if self.graph.node(&event_id).is_none() {
                    self.graph.add_node(
                        Node::new(&event_id, NodeKind::Event, &item.headline, now)
                            .with_attribute("source", item.source.as_str()),
                    );
                }
                self.graph.assert_fact(Fact::new(
                    Relationship::new(
                        &event_id,
                        &id,
                        RelationshipKind::ConcernsEntity,
                        mention.confidence,
                        item.provenance.source.clone(),
                    ),
                    item.published_at,
                    now,
                ));

                // Sentiment is a feature of the entity, available when the item
                // was published rather than when it was written.
                if mention.is_primary {
                    self.features.record(
                        "sentiment",
                        &id,
                        FeatureValue {
                            value: item.sentiment.effective(),
                            valid_at: item.published_at,
                            available_at: item.published_at,
                            confidence: item.evidential_weight(),
                            imputed: item.quality.is_imputed,
                        },
                    );
                }
            }
        }

        self.index.add(
            Document::new(
                format!("news:{}", item.item_id),
                format!("{}\n{}", item.headline, item.body),
                item.published_at,
            )
            .with_attribute("source", item.source.as_str())
            .with_attribute("kind", "news")
            .with_reliability(item.evidential_weight()),
        );

        if item.sentiment.is_material() {
            self.journal.push(Change::new(
                ChangeKind::EntityUpdated,
                resolved
                    .first()
                    .cloned()
                    .unwrap_or_else(|| item.item_id.clone()),
                format!("material news: {}", item.headline),
                item.sentiment.effective().abs().clamp(0.0, 1.0),
                item.published_at,
            ));
        }

        resolved
    }

    /// Absorb a reported fundamental as point-in-time features.
    pub fn absorb_fundamental(&mut self, update: &FundamentalUpdate) {
        // A fundamental is true for the period it covers but only usable when
        // it is published, which is what the two timestamps record. Whether
        // the vendor observed it or filled it in travels with it: a value
        // stamped observed here regardless of `quality.is_imputed` reads
        // downstream as a reported number, and no point-in-time query can
        // tell the two apart afterwards.
        let value = FeatureValue {
            value: update.value.to_f64(),
            valid_at: update.period_end,
            available_at: update.provenance.ingestion_time,
            confidence: update.quality.score(),
            imputed: update.quality.is_imputed,
        };
        self.features
            .record(&update.metric, &update.entity_id, value);

        if let Some(surprise) = update.surprise() {
            self.features.record(
                &format!("{}_surprise", update.metric),
                &update.entity_id,
                FeatureValue {
                    value: surprise,
                    valid_at: update.period_end,
                    available_at: update.provenance.ingestion_time,
                    confidence: update.quality.score(),
                    imputed: update.quality.is_imputed,
                },
            );
            if surprise.abs() > 0.05 {
                self.journal.push(Change::new(
                    ChangeKind::FeatureMoved,
                    update.entity_id.clone(),
                    format!(
                        "{} surprised consensus by {:.1}%",
                        update.metric,
                        surprise * 100.0
                    ),
                    surprise.abs().clamp(0.0, 1.0),
                    update.provenance.ingestion_time,
                ));
            }
        }
    }

    /// Absorb a macro observation.
    ///
    /// Every release lands under [`names::MACRO_LEVEL`] keyed by the vendor's
    /// series id — the raw record. A release the vocabulary recognises
    /// ([`MacroSeries::recognise`]) also lands under the analyst's name keyed
    /// by the observation's region, which is the key the macro analyst reads
    /// by: the instrument's geography. Until this second write existed the
    /// macro analyst had never once found a series, on any deployment.
    pub fn absorb_macro(&mut self, observation: &MacroObservation) {
        let value = FeatureValue {
            value: observation.value,
            valid_at: observation.reference_date,
            available_at: observation.provenance.ingestion_time,
            confidence: observation.quality.score(),
            imputed: observation.quality.is_imputed,
        };
        if let Some(series) = MacroSeries::recognise(&observation.series_id, &observation.region) {
            self.features
                .record(series.feature(), &observation.region, value.clone());
        }
        self.features
            .record(names::MACRO_LEVEL, &observation.series_id, value);
        if let Some(surprise) = observation.surprise() {
            self.features.record(
                names::MACRO_SURPRISE,
                &observation.series_id,
                FeatureValue {
                    value: surprise,
                    valid_at: observation.reference_date,
                    available_at: observation.provenance.ingestion_time,
                    confidence: observation.quality.score(),
                    imputed: observation.quality.is_imputed,
                },
            );
            if surprise.abs() > 0.1 {
                self.journal.push(Change::new(
                    ChangeKind::FeatureMoved,
                    observation.series_id.clone(),
                    format!("{} surprised by {surprise:+.2}", observation.series_id),
                    surprise.abs().clamp(0.0, 1.0),
                    observation.provenance.ingestion_time,
                ));
            }
        }
    }

    /// Absorb an alternative-data reading as a point-in-time feature.
    ///
    /// A reading whose metric the vocabulary names lands under that name,
    /// keyed by the reading's subject, in the series whose definition names
    /// the dataset as its producer — the dataset travels as provenance, not
    /// as a path prefix nothing reads. A metric the vocabulary does not name
    /// is still recorded, under `alt/{dataset}/{metric}`, because a dataset
    /// nobody has vocabularised must not vanish; nothing reads it, and the
    /// name says so.
    ///
    /// Refused, not stored: a vocabulary metric from a dataset other than the
    /// one the vocabulary holds it under. The analyst's licence check is by
    /// dataset, so a reading laundered under the licensed name would pass
    /// it. The refusal names both datasets and what to do instead.
    pub fn absorb_alternative_data(&mut self, point: &AlternativeDataPoint) -> Result<()> {
        let value = FeatureValue {
            value: point.value,
            valid_at: point.observed_at,
            available_at: point.provenance.ingestion_time,
            confidence: point.quality.score(),
            imputed: point.quality.is_imputed,
        };
        match AltMetric::recognise(&point.dataset, &point.metric)? {
            Some(metric) => {
                self.features
                    .record(metric.feature(), &point.subject_id, value);
            }
            None => {
                let feature = format!("alt/{}/{}", point.dataset, point.metric);
                if self.features.definition(&feature).is_none() {
                    self.features.define(
                        Feature::new(
                            &feature,
                            "alternative data series outside the vocabulary; read by nothing",
                            point.dataset.clone(),
                        )
                        .with_staleness(Duration::from_days(30)),
                    );
                }
                self.features.record(&feature, &point.subject_id, value);
            }
        }
        Ok(())
    }

    /// Absorb a bar as price and volume features.
    /// Absorb a bar, stating when this platform could first have acted on it.
    ///
    /// `known_at` is not the bar's close time. A bar closes at the venue and
    /// arrives here later — over a wire, through a vendor, after a batch — and
    /// the gap is routinely hundreds of milliseconds. Stamping availability at
    /// the close would make every feature derived from the bar readable before
    /// it existed, which is a look-ahead a point-in-time read cannot catch
    /// because the record itself claims to have been available.
    ///
    /// A caller computing a bar from ticks it has already absorbed passes the
    /// close time and is right to; a caller reading a feed passes the arrival
    /// time. Requiring the argument is what makes the difference a decision
    /// rather than a default nobody revisits.
    ///
    /// A `known_at` before the close is clamped forward: a bar cannot have
    /// been knowable before the period it summarises had finished, and the
    /// combination always means a clock or a parser rather than a fast feed.
    pub fn absorb_bar(&mut self, bar: &Bar, known_at: Timestamp) {
        let object = bar.object_id.as_str();
        let valid_at = bar.close_time();
        let available_at = if known_at < valid_at {
            valid_at
        } else {
            known_at
        };
        self.features.record(
            "close",
            object,
            FeatureValue::new(bar.close.to_f64(), valid_at, available_at),
        );
        self.features.record(
            "volume",
            object,
            FeatureValue::new(bar.volume.to_f64(), valid_at, available_at),
        );
    }

    /// Absorb many bars in one pass, each with its own knowability instant.
    ///
    /// Exactly [`WorldModel::absorb_bar`]'s semantics — the same clamp, the
    /// same two features — batched so that a feed handing over history costs
    /// one merge per series rather than one sorted insert per bar. History
    /// typically arrives newest-first, which is the worst case for repeated
    /// insertion: each bar would move everything already stored, and at feed
    /// rates that quadratic cost is what separates absorbing a replay from
    /// timing out on it.
    pub fn absorb_bars<'a>(&mut self, bars: impl IntoIterator<Item = (&'a Bar, Timestamp)>) {
        let mut closes: BTreeMap<String, Vec<FeatureValue>> = BTreeMap::new();
        let mut volumes: BTreeMap<String, Vec<FeatureValue>> = BTreeMap::new();
        for (bar, known_at) in bars {
            let valid_at = bar.close_time();
            // The same clamp as `absorb_bar`, for the same reason: a bar
            // cannot have been knowable before the period it summarises had
            // finished.
            let available_at = if known_at < valid_at {
                valid_at
            } else {
                known_at
            };
            let object = bar.object_id.as_str().to_string();
            closes
                .entry(object.clone())
                .or_default()
                .push(FeatureValue::new(
                    bar.close.to_f64(),
                    valid_at,
                    available_at,
                ));
            volumes.entry(object).or_default().push(FeatureValue::new(
                bar.volume.to_f64(),
                valid_at,
                available_at,
            ));
        }
        for (subject, values) in closes {
            self.features.record_many("close", &subject, values);
        }
        for (subject, values) in volumes {
            self.features.record_many("volume", &subject, values);
        }
    }

    /// Recompute realised volatility for an object from its close history.
    pub fn recompute_volatility(&mut self, object_id: &str, window: usize, now: Timestamp) {
        let history = self.features.history("close", object_id, now);
        if history.len() < window + 1 {
            return;
        }
        let closes: Vec<f64> = history
            .iter()
            .rev()
            .take(window + 1)
            .rev()
            .map(|v| v.value)
            .collect();
        let returns = qip_numerics::stats::log_returns(&closes);
        if returns.len() < 2 {
            return;
        }
        // Annualised from daily observations.
        let volatility = qip_numerics::stats::stddev(&returns) * 252.0f64.sqrt();
        self.features.record(
            "realised_volatility_20d",
            object_id,
            FeatureValue::new(volatility, now, now),
        );
    }

    /// Retrieve evidence relevant to a query, as known at a point in time.
    pub fn retrieve(&self, query: &str, limit: usize, as_of: Timestamp) -> Vec<RetrievalResult> {
        self.index.search_as_of(query, limit, as_of)
    }

    /// Propagate a shock through the causal graph.
    pub fn propagate(
        &self,
        origin: &str,
        shock: f64,
        max_order: usize,
        at: Timestamp,
        known_at: Timestamp,
    ) -> PropagationResult {
        // A tenth of a percent of the original move is the floor: below that,
        // an "effect" is indistinguishable from the noise in any price.
        self.causal
            .propagate(origin, shock, max_order, 0.001, at, known_at)
    }

    /// The state of the world as believed at a point in both time dimensions.
    pub fn state_at(&self, valid_at: Timestamp, known_at: Timestamp) -> WorldState {
        let facts = self.graph.facts_at(valid_at, known_at);
        let features: BTreeMap<String, f64> = self
            .features
            .definitions()
            .flat_map(|definition| {
                self.features
                    .cross_section(&definition.name, valid_at, known_at)
                    .into_iter()
                    .map(move |(subject, value)| {
                        (format!("{}/{}", definition.name, subject), value)
                    })
            })
            .collect();

        WorldState {
            valid_at,
            known_at,
            entity_count: self.graph.nodes_of_kind(NodeKind::Entity).len(),
            object_count: self.graph.nodes_of_kind(NodeKind::FinancialObject).len(),
            relationship_count: facts.len(),
            causal_claim_count: self
                .causal
                .edges()
                .iter()
                .filter(|e| e.recorded_at <= known_at)
                .count(),
            features,
            hubs: self
                .graph
                .most_connected(5, valid_at, known_at)
                .into_iter()
                .map(|(node, degree)| (node.id.clone(), degree))
                .collect(),
        }
    }

    /// What changed between two instants.
    pub fn diff(&self, from: Timestamp, to: Timestamp) -> WorldDiff {
        let mut changes: Vec<Change> = self
            .journal
            .iter()
            .filter(|c| c.at > from && c.at <= to)
            .cloned()
            .collect();

        // Feature moves are computed rather than journalled, because a value
        // changing is only interesting relative to its own history.
        for definition in self.features.definitions() {
            let before = self.features.cross_section(&definition.name, from, from);
            let after = self.features.cross_section(&definition.name, to, to);
            let previous: BTreeMap<&String, f64> = before.iter().map(|(s, v)| (s, *v)).collect();
            for (subject, value) in &after {
                let Some(old) = previous.get(subject) else {
                    continue;
                };
                if old.abs() < 1e-12 {
                    continue;
                }
                let relative = (value - old) / old.abs();
                if relative.abs() < 0.02 {
                    continue;
                }
                changes.push(Change::new(
                    ChangeKind::FeatureMoved,
                    format!("{}/{subject}", definition.name),
                    format!(
                        "{} for {subject} moved {:+.1}%",
                        definition.name,
                        relative * 100.0
                    ),
                    relative.abs().clamp(0.0, 1.0),
                    to,
                ));
            }
        }

        changes.sort_by(|a, b| {
            a.at.cmp(&b.at)
                .then_with(|| a.subject.cmp(&b.subject))
                .then_with(|| a.description.cmp(&b.description))
        });
        WorldDiff { from, to, changes }
    }

    /// Absorbed-record counts, for the system surface.
    pub fn statistics(&self) -> BTreeMap<String, usize> {
        BTreeMap::from([
            ("nodes".to_string(), self.graph.node_count()),
            ("facts".to_string(), self.graph.fact_count()),
            ("causal_claims".to_string(), self.causal.len()),
            ("features".to_string(), self.features.feature_count()),
            ("feature_values".to_string(), self.features.value_count()),
            ("entities".to_string(), self.resolver.len()),
            ("documents".to_string(), self.index.len()),
            ("changes".to_string(), self.journal.len()),
        ])
    }
}

/// Seed the demo world: the companies, their supply chain, and the causal
/// claims that let a shock at one propagate to the others.
pub fn seed_demo_world(model: &mut WorldModel, context: &Context) -> Result<()> {
    let now = context.now();
    let known_from = now.saturating_sub(Duration::from_days(365));

    for (entity_id, name, country, sector) in [
        (
            "ent-northwind",
            "Northwind Semiconductor Corporation",
            "US",
            "information_technology",
        ),
        (
            "ent-vantage",
            "Vantage Devices Incorporated",
            "US",
            "information_technology",
        ),
        ("ent-kestrel", "Kestrel Materials PLC", "GB", "materials"),
        ("ent-meridian", "Meridian Energy Holdings", "US", "energy"),
        ("ent-atlas", "Atlas Federal Bancorp", "US", "financials"),
    ] {
        let entity = Entity::new(
            qip_core::EntityId::from_string(entity_id),
            EntityKind::Company,
            name,
            known_from,
        )
        .with_country(country)
        .with_attribute("sector", sector);
        model.add_entity(entity);
    }

    for (entity_id, name) in [("ctry-us", "United States"), ("ctry-gb", "United Kingdom")] {
        model.add_entity(Entity::new(
            qip_core::EntityId::from_string(entity_id),
            EntityKind::Country,
            name,
            known_from,
        ));
    }

    // The supply chain: Kestrel supplies Northwind, which supplies Vantage.
    model.relate(
        Relationship::new(
            "ent-kestrel",
            "ent-northwind",
            RelationshipKind::Supplies,
            0.35,
            "filings",
        ),
        known_from,
        known_from,
        0.9,
    );
    model.relate(
        Relationship::new(
            "ent-northwind",
            "ent-vantage",
            RelationshipKind::Supplies,
            0.55,
            "filings",
        ),
        known_from,
        known_from,
        0.92,
    );
    model.relate(
        Relationship::new(
            "ent-northwind",
            "ent-meridian",
            RelationshipKind::Competitor,
            0.1,
            "research",
        ),
        known_from,
        known_from,
        0.4,
    );
    for (entity, country) in [
        ("ent-northwind", "ctry-us"),
        ("ent-vantage", "ctry-us"),
        ("ent-meridian", "ctry-us"),
        ("ent-atlas", "ctry-us"),
        ("ent-kestrel", "ctry-gb"),
    ] {
        model.relate(
            Relationship::new(
                entity,
                country,
                RelationshipKind::DomiciledIn,
                1.0,
                "reference",
            ),
            known_from,
            known_from,
            1.0,
        );
    }

    // Causal claims. Each carries a mechanism, a lag and evidence.
    model.claim_causal(
        CausalEdge::new(
            "ent-northwind",
            "ent-vantage",
            Mechanism::SupplyChain,
            0.45,
            Duration::from_days(3),
            known_from,
        )
        .with_confidence(0.75)
        .with_evidence(vec!["filing:vantage-10k-supplier-concentration".into()]),
    );
    model.claim_causal(
        CausalEdge::new(
            "ent-kestrel",
            "ent-northwind",
            Mechanism::InputCost,
            0.30,
            Duration::from_days(7),
            known_from,
        )
        .with_confidence(0.65)
        .with_evidence(vec!["filing:northwind-10k-input-costs".into()]),
    );
    model.claim_causal(
        CausalEdge::new(
            "ent-northwind",
            "ent-meridian",
            Mechanism::CompetitiveSubstitution,
            0.15,
            Duration::from_days(5),
            known_from,
        )
        .with_confidence(0.45)
        .with_evidence(vec!["research:sector-substitution-note".into()]),
    );
    model.claim_causal(
        CausalEdge::new(
            "US.CPI.YOY",
            "ent-atlas",
            Mechanism::DiscountRate,
            0.55,
            Duration::from_days(1),
            known_from,
        )
        .with_confidence(0.8)
        .with_evidence(vec!["research:bank-rate-sensitivity".into()]),
    );

    Ok(())
}
