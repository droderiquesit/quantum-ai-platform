//! Embeddings, retrieval, the language-model boundary, governance, calibration.

// Exact float comparison is deliberate: these assert that a mismatch yields
// exactly zero and that a vacuous adjustment is exactly one, not merely close.
#![allow(clippy::float_cmp)]

use qip_ai::embedding::{Embedder, Embedding, HashingEmbedder};
use qip_ai::evaluation::{Calibration, DriftReport, PredictionOutcome};
use qip_ai::language::{
    Completion, DeterministicModel, FallbackChain, FieldSpec, LanguageModel, ModelRequest,
    NumericGuard, OutputSchema, RemoteModel, RemoteModelConfig,
};
use qip_ai::registry::{EvaluationRecord, ModelCard, ModelRegistry, ModelStage};
use qip_ai::retrieval::{Document, SearchIndex, SearchWeights};
use qip_core::testing::approx_eq;
use qip_core::{Duration, ModelId, Timestamp};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

// --- embeddings -------------------------------------------------------------

#[test]
fn embeddings_are_deterministic_and_normalised() {
    let embedder = HashingEmbedder::default();
    let a = embedder.embed("Northwind reports quarterly revenue above consensus");
    let b = embedder.embed("Northwind reports quarterly revenue above consensus");
    assert_eq!(a, b, "the same text must embed identically");
    assert_eq!(a.dimensions(), embedder.dimensions());

    let norm: f32 = a.values.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-5,
        "expected unit length, got {norm}"
    );
}

#[test]
fn related_texts_are_closer_than_unrelated_ones() {
    let embedder = HashingEmbedder::default();
    let subject = embedder.embed("Northwind cut its full year revenue guidance");
    let related = embedder.embed("Northwind reduced guidance for full year revenue");
    let unrelated = embedder.embed("Central bank holds policy rate steady at four percent");

    let close = subject.cosine_similarity(&related);
    let far = subject.cosine_similarity(&unrelated);
    assert!(close > far, "related {close} should beat unrelated {far}");
    assert!(
        close > 0.3,
        "paraphrase similarity {close} is too low to be useful"
    );
}

#[test]
fn embeddings_from_different_models_never_compare() {
    // Comparing across models is meaningless, not merely inaccurate, so the
    // similarity is zero rather than a plausible-looking number.
    let a = Embedding::new(vec![1.0, 0.0], "model-a");
    let b = Embedding::new(vec![1.0, 0.0], "model-b");
    assert_eq!(a.cosine_similarity(&b), 0.0);
    assert!(approx_eq(f64::from(a.cosine_similarity(&a)), 1.0, 1e-6));
}

#[test]
fn dimension_mismatch_is_handled_rather_than_panicking() {
    let a = Embedding::new(vec![1.0, 0.0, 0.0], "m");
    let b = Embedding::new(vec![1.0, 0.0], "m");
    assert_eq!(a.cosine_similarity(&b), 0.0);
    assert!(a.distance(&b).is_infinite());
}

#[test]
fn a_centroid_averages_and_rejects_ragged_input() {
    let a = Embedding::new(vec![1.0, 0.0], "m");
    let b = Embedding::new(vec![0.0, 1.0], "m");
    let centroid = Embedding::centroid(&[a.clone(), b]).unwrap();
    assert!(approx_eq(f64::from(centroid.values[0]), 0.5, 1e-6));
    assert!(Embedding::centroid(&[a, Embedding::new(vec![1.0], "m")]).is_none());
    assert!(Embedding::centroid(&[]).is_none());
}

// --- retrieval --------------------------------------------------------------

fn corpus() -> SearchIndex {
    let embedder: Arc<dyn Embedder> = Arc::new(HashingEmbedder::default());
    let mut index = SearchIndex::new().with_embedder(embedder);
    index.add(
        Document::new(
            "doc-guidance",
            "Northwind Semiconductor cut full year revenue guidance citing weak demand",
            now(),
        )
        .with_attribute("entity", "ent-northwind")
        .with_reliability(0.95),
    );
    index.add(
        Document::new(
            "doc-supply",
            "Northwind Semiconductor disclosed a production disruption affecting output",
            now().saturating_sub(Duration::from_days(3)),
        )
        .with_attribute("entity", "ent-northwind")
        .with_reliability(0.9),
    );
    index.add(
        Document::new(
            "doc-macro",
            "Consumer price inflation surprised to the upside, repricing rate expectations",
            now().saturating_sub(Duration::from_days(1)),
        )
        .with_attribute("entity", "US")
        .with_reliability(0.97),
    );
    index.add(
        Document::new(
            "doc-old",
            "Northwind Semiconductor announces routine leadership appointment",
            now().saturating_sub(Duration::from_days(200)),
        )
        .with_attribute("entity", "ent-northwind")
        .with_reliability(0.6),
    );
    index
}

#[test]
fn retrieval_ranks_the_relevant_document_first() {
    let index = corpus();
    let results = index.search("revenue guidance cut", 3);
    assert!(!results.is_empty());
    assert_eq!(results[0].document.id, "doc-guidance");
    assert!(results[0].lexical_score > 0.0);
    assert!(results[0].matched_terms.contains(&"guidance".to_string()));
}

#[test]
fn retrieval_is_deterministic() {
    let index = corpus();
    let first: Vec<String> = index
        .search("Northwind", 4)
        .into_iter()
        .map(|r| r.document.id)
        .collect();
    let second: Vec<String> = index
        .search("Northwind", 4)
        .into_iter()
        .map(|r| r.document.id)
        .collect();
    assert_eq!(first, second);
}

#[test]
fn point_in_time_retrieval_cannot_return_a_future_document() {
    // The look-ahead guard for research: a thesis formed on a past date must
    // not cite something published after it.
    let index = corpus();
    let cutoff = now().saturating_sub(Duration::from_days(2));
    let results = index.search_as_of("Northwind guidance disruption", 10, cutoff);
    assert!(!results.is_empty());
    for result in &results {
        assert!(
            result.document.occurred_at <= cutoff,
            "{} was published after the cutoff",
            result.document.id
        );
    }
    assert!(
        !results.iter().any(|r| r.document.id == "doc-guidance"),
        "today's document must not be visible two days ago"
    );
}

#[test]
fn attribute_filters_restrict_the_candidate_set() {
    let index = corpus();
    let results = index.search_with(
        "inflation guidance",
        10,
        SearchWeights::default(),
        None,
        &[("entity", "US")],
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].document.id, "doc-macro");
}

#[test]
fn recency_weighting_demotes_a_stale_document() {
    let index = corpus();
    let results = index.search("Northwind Semiconductor", 4);
    let positions: Vec<&str> = results.iter().map(|r| r.document.id.as_str()).collect();
    let old = positions.iter().position(|id| *id == "doc-old");
    let recent = positions.iter().position(|id| *id == "doc-supply");
    assert!(
        old > recent,
        "a 200-day-old routine story should rank below a recent one: {positions:?}"
    );
}

#[test]
fn exact_weights_disable_semantic_matching() {
    let index = corpus();
    let results = index.search_with(
        "production disruption",
        3,
        SearchWeights::exact(),
        None,
        &[],
    );
    assert_eq!(results[0].document.id, "doc-supply");
    assert_eq!(results[0].vector_score, 0.0, "vector scoring must be off");
}

#[test]
fn a_query_matching_nothing_returns_nothing() {
    let index = corpus();
    assert!(index.search("zygomatic paleobotany", 5).is_empty());
    assert!(index.search("Northwind", 0).is_empty());
    assert!(SearchIndex::new().search("anything", 5).is_empty());
}

#[test]
fn matched_terms_do_not_repeat_a_query_term_that_recurs_non_adjacently() {
    // "revenue" appears twice in the query with "guidance" between the two
    // occurrences, so a dedup that only collapses adjacent duplicates would
    // leave "revenue" twice in `matched_terms`. The field is documented as
    // the terms the document contains, not one entry per query occurrence.
    let index = corpus();
    let results = index.search("revenue guidance revenue", 3);
    assert!(!results.is_empty());
    let top = &results[0];
    let occurrences = top
        .matched_terms
        .iter()
        .filter(|t| t.as_str() == "revenue")
        .count();
    assert_eq!(
        occurrences, 1,
        "revenue must be listed once, not once per occurrence in the query: {:?}",
        top.matched_terms
    );
}

#[test]
fn similar_documents_exclude_the_query_document() {
    let index = corpus();
    let similar = index.similar_to("doc-guidance", 3);
    assert!(!similar.iter().any(|r| r.document.id == "doc-guidance"));
    assert!(similar.iter().any(|r| r.document.id.starts_with("doc-")));
    assert!(index.similar_to("nonexistent", 3).is_empty());
}

// --- the numeric boundary ---------------------------------------------------

#[test]
fn the_numeric_guard_finds_numbers_anywhere_in_the_output() {
    let clean = json!({
        "thesis": "Guidance cut signals demand weakness",
        "evidence": ["filing", "transcript"],
        "direction": "bearish"
    });
    assert!(NumericGuard::find_numbers(&clean).is_empty());
    assert!(NumericGuard::enforce(&clean).is_ok());

    let contaminated = json!({
        "thesis": "Guidance cut signals demand weakness",
        "expected_return": 0.08,
        "nested": {"confidence": 0.72},
        "list": ["fine", 3]
    });
    let found = NumericGuard::find_numbers(&contaminated);
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found.contains(&"expected_return".to_string()));
    assert!(found.contains(&"nested.confidence".to_string()));
    assert!(found.iter().any(|p| p.starts_with("list[")));

    let error = NumericGuard::enforce(&contaminated).unwrap_err();
    assert_eq!(error.code(), "guard");
    assert!(error.to_string().contains("must be computed"));
}

#[test]
fn a_schema_rejects_missing_and_malformed_fields() {
    let schema = OutputSchema::new(
        "hypothesis",
        vec![
            FieldSpec::text("title", "one line summary"),
            FieldSpec::list("supporting_evidence", "evidence ids"),
            FieldSpec::one_of(
                "direction",
                &["bullish", "bearish", "neutral"],
                "expected direction",
            ),
            FieldSpec::text("caveat", "optional note").optional(),
        ],
    );

    schema
        .validate(&json!({
            "title": "Demand weakness",
            "supporting_evidence": ["doc-guidance"],
            "direction": "bearish"
        }))
        .expect("a well-formed output");

    let missing = schema.validate(&json!({"title": "x", "direction": "bearish"}));
    assert!(
        missing
            .unwrap_err()
            .to_string()
            .contains("supporting_evidence")
    );

    let bad_enum = schema.validate(&json!({
        "title": "x",
        "supporting_evidence": [],
        "direction": "sideways"
    }));
    assert!(bad_enum.unwrap_err().to_string().contains("not one of"));

    let bad_list = schema.validate(&json!({
        "title": "x",
        "supporting_evidence": "not a list",
        "direction": "bearish"
    }));
    assert!(bad_list.unwrap_err().to_string().contains("must be a list"));

    assert!(schema.validate(&json!("not an object")).is_err());
}

#[test]
fn the_schema_description_tells_a_model_not_to_produce_numbers() {
    let schema = OutputSchema::new("hypothesis", vec![FieldSpec::text("title", "summary")]);
    let description = schema.describe();
    assert!(description.contains("title"));
    assert!(description.contains("Do not include numeric values"));
}

#[test]
fn structured_completion_enforces_both_schema_and_numeric_guard() {
    let mut model = DeterministicModel::new();
    model.register(
        "clean",
        |_| json!({"title": "A conclusion", "direction": "bearish"}),
    );
    model.register(
        "dirty",
        |_| json!({"title": "A conclusion", "expected_return": 0.08}),
    );

    let schema = |name: &str| OutputSchema::new(name, vec![FieldSpec::text("title", "summary")]);

    let ok = model.complete_structured(
        &ModelRequest::new("system", "prompt").with_schema(schema("clean")),
        now(),
    );
    assert!(ok.is_ok(), "{:?}", ok.err());

    let blocked = model.complete_structured(
        &ModelRequest::new("system", "prompt").with_schema(schema("dirty")),
        now(),
    );
    let error = blocked.unwrap_err();
    assert_eq!(
        error.code(),
        "guard",
        "a generated number must be refused: {error}"
    );
}

#[test]
fn the_deterministic_model_refuses_an_unregistered_schema() {
    let model = DeterministicModel::new();
    let request = ModelRequest::new("s", "p").with_schema(OutputSchema::new("unknown", Vec::new()));
    let error = model.complete(&request, now()).unwrap_err();
    assert_eq!(error.code(), "unavailable");
    assert!(error.to_string().contains("no deterministic template"));
}

#[test]
fn the_deterministic_model_summarises_context_without_a_schema() {
    let model = DeterministicModel::new();
    let request = ModelRequest::new("system", "summarise")
        .with_context("evidence", "Northwind cut guidance")
        .with_context("statistics", "z-score computed elsewhere");
    let completion = model.complete(&request, now()).unwrap();
    assert!(completion.text.contains("Northwind cut guidance"));
    assert!(completion.is_complete());
    assert!(completion.total_tokens() > 0);
    assert!(completion.structured.is_none());
}

#[test]
fn a_hosted_model_reports_itself_unavailable_and_says_what_is_missing() {
    let model = RemoteModel::with_credential_present(
        RemoteModelConfig {
            provider: "anthropic".into(),
            model: "claude-opus-5".into(),
            credential_env: "ANTHROPIC_API_KEY".into(),
            endpoint: None,
        },
        true,
    );
    assert!(
        !model.is_available(),
        "no TLS transport exists in this build"
    );
    let error = model
        .complete(&ModelRequest::new("s", "p"), now())
        .unwrap_err();
    assert_eq!(error.code(), "unavailable");
    assert!(
        error
            .to_string()
            .contains("docs/operations/external-dependencies.md")
    );
    assert!(model.requirement().contains("ANTHROPIC_API_KEY"));
}

#[test]
fn a_remote_model_resolves_its_credential_through_the_shared_secret_reader() {
    // `RemoteModel::new` once read `std::env::var` directly, which would
    // report a credential mounted only as `<VAR>_FILE` by the Secret Manager
    // CSI driver as absent — an operator would be sent chasing a missing
    // variable that was in fact supplied correctly. Routing through
    // `qip_core::secret` is what makes the `_FILE` indirection work here the
    // same way it does everywhere else credential material is read.
    let config = RemoteModelConfig {
        provider: "anthropic".into(),
        model: "claude-test".into(),
        credential_env: "QIP_AI_TEST_UNSET_CREDENTIAL_7f3a1".into(),
        endpoint: None,
    };
    let model = RemoteModel::new(config).expect("an absent credential must not be an error");
    assert!(
        !model.is_available(),
        "no TLS transport exists in this build regardless of credential state"
    );
    let error = model
        .complete(&ModelRequest::new("s", "p"), now())
        .unwrap_err();
    assert!(
        error.to_string().contains("no credential is configured"),
        "the resolver must have found the variable genuinely absent: {error}"
    );
}

#[test]
fn the_fallback_chain_uses_the_local_model_when_the_hosted_one_is_absent() {
    let mut local = DeterministicModel::new();
    local.register("summary", |_| json!({"title": "local"}));

    let chain = FallbackChain::new(vec![
        Arc::new(RemoteModel::with_credential_present(
            RemoteModelConfig {
                provider: "vertex".into(),
                model: "gemini".into(),
                credential_env: "GOOGLE_APPLICATION_CREDENTIALS".into(),
                endpoint: None,
            },
            false,
        )),
        Arc::new(local),
    ]);

    assert!(chain.is_available());
    assert_eq!(chain.name(), "deterministic-local-v1");
    let completion = chain
        .complete(
            &ModelRequest::new("s", "p").with_schema(OutputSchema::new("summary", Vec::new())),
            now(),
        )
        .unwrap();
    assert_eq!(completion.model, "deterministic-local-v1");
}

#[test]
fn an_empty_chain_fails_clearly() {
    let chain = FallbackChain::new(Vec::new());
    assert!(!chain.is_available());
    assert_eq!(chain.name(), "none");
    assert!(chain.complete(&ModelRequest::new("s", "p"), now()).is_err());
}

#[test]
fn token_estimates_scale_with_the_request() {
    let small = ModelRequest::new("s", "short prompt");
    let large = ModelRequest::new("s", "short prompt").with_context("evidence", "x".repeat(4000));
    assert!(large.estimated_input_tokens() > small.estimated_input_tokens() * 10);
}

// --- model governance -------------------------------------------------------

fn evaluated_card(at: Timestamp, passed: bool) -> ModelCard {
    let mut card = ModelCard::new(
        ModelId::from_string("MDL0000000000000000000001"),
        "regime-classifier",
        "2.1.0",
        "quant-research",
        at,
    )
    .with_purpose("classify the prevailing volatility regime")
    .with_features(vec![
        "realised_volatility".into(),
        "term_structure_slope".into(),
    ])
    .with_training_data(vec!["synthetic-market-2020-2025".into()])
    .with_limitation("not validated on instruments with fewer than 250 observations");
    card.evaluations.push(EvaluationRecord {
        evaluated_at: at,
        dataset: "held-out-2025".into(),
        metrics: BTreeMap::from([("accuracy".to_string(), 0.82)]),
        passed,
    });
    card
}

#[test]
fn a_model_needs_a_passing_evaluation_before_promotion() {
    let mut registry = ModelRegistry::new();
    registry.register(evaluated_card(now(), false));
    let error = registry
        .promote("regime-classifier@2.1.0", now())
        .unwrap_err();
    assert!(error.to_string().contains("passing evaluation"));

    let mut registry = ModelRegistry::new();
    registry.register(evaluated_card(now(), true));
    registry.promote("regime-classifier@2.1.0", now()).unwrap();
    assert_eq!(
        registry.get("regime-classifier@2.1.0").unwrap().stage,
        ModelStage::Production
    );
}

#[test]
fn a_production_model_can_drive_a_decision() {
    let mut registry = ModelRegistry::new();
    registry.register(evaluated_card(now(), true));
    registry.promote("regime-classifier@2.1.0", now()).unwrap();
    let card = registry
        .require_for_decision("regime-classifier@2.1.0", now())
        .unwrap();
    assert_eq!(card.reference(), "regime-classifier@2.1.0");
    assert!(registry.ineligible(now()).is_empty());
}

#[test]
fn a_retired_model_is_refused_for_decisions() {
    let mut registry = ModelRegistry::new();
    registry.register(evaluated_card(now(), true));
    registry.promote("regime-classifier@2.1.0", now()).unwrap();
    registry.retire("regime-classifier@2.1.0", now()).unwrap();

    let error = registry
        .require_for_decision("regime-classifier@2.1.0", now())
        .unwrap_err();
    assert_eq!(error.code(), "denied");
    assert!(error.to_string().contains("not production"));
}

#[test]
fn a_drifted_model_is_refused_until_it_is_re_evaluated() {
    let mut registry = ModelRegistry::new();
    registry.register(evaluated_card(now(), true));
    registry.promote("regime-classifier@2.1.0", now()).unwrap();
    registry
        .record_drift("regime-classifier@2.1.0", 0.45)
        .unwrap();

    let error = registry
        .require_for_decision("regime-classifier@2.1.0", now())
        .unwrap_err();
    assert!(error.to_string().contains("drift"), "{error}");
    assert_eq!(registry.ineligible(now()).len(), 1);
}

#[test]
fn a_stale_evaluation_expires() {
    let mut registry = ModelRegistry::new();
    registry.register(evaluated_card(now(), true));
    registry.promote("regime-classifier@2.1.0", now()).unwrap();

    let much_later = now().saturating_add(Duration::from_days(200));
    let error = registry
        .require_for_decision("regime-classifier@2.1.0", much_later)
        .unwrap_err();
    assert!(error.to_string().contains("validity window"), "{error}");
}

#[test]
fn a_never_evaluated_model_cannot_drive_a_decision() {
    let mut registry = ModelRegistry::new();
    let mut card = evaluated_card(now(), true);
    card.evaluations.clear();
    card.stage = ModelStage::Production;
    registry.register(card);
    let error = registry
        .require_for_decision("regime-classifier@2.1.0", now())
        .unwrap_err();
    assert!(error.to_string().contains("never been evaluated"));
}

#[test]
fn an_unregistered_model_is_a_not_found_error() {
    let registry = ModelRegistry::new();
    let error = registry
        .require_for_decision("ghost@1.0.0", now())
        .unwrap_err();
    assert_eq!(error.code(), "not_found");
}

#[test]
fn the_registry_finds_the_current_production_version() {
    let mut registry = ModelRegistry::new();
    let mut old = evaluated_card(now().saturating_sub(Duration::from_days(400)), true);
    old.version = "1.0.0".into();
    registry.register(old);
    registry.register(evaluated_card(now(), true));
    registry
        .promote(
            "regime-classifier@1.0.0",
            now().saturating_sub(Duration::from_days(300)),
        )
        .unwrap();
    registry.promote("regime-classifier@2.1.0", now()).unwrap();

    let current = registry.production_version("regime-classifier").unwrap();
    assert_eq!(current.version, "2.1.0");
    assert_eq!(registry.len(), 2);
}

// --- calibration and drift --------------------------------------------------

#[test]
fn a_well_calibrated_forecaster_scores_well() {
    // Predictions at p that occur p of the time.
    let mut outcomes = Vec::new();
    for (probability, total) in [(0.1, 100), (0.3, 100), (0.5, 100), (0.7, 100), (0.9, 100)] {
        let occurring = (probability * f64::from(total)).round() as i32;
        for i in 0..total {
            outcomes.push(PredictionOutcome::new(probability, i < occurring));
        }
    }
    let calibration = Calibration::assess(&outcomes, 10);
    assert_eq!(calibration.count, 500);
    assert!(
        calibration.expected_calibration_error < 0.02,
        "ECE {}",
        calibration.expected_calibration_error
    );
    assert!(calibration.bias.abs() < 0.02, "bias {}", calibration.bias);
    assert!(!calibration.is_overconfident(0.05));
    assert!(approx_eq(calibration.confidence_adjustment(), 1.0, 0.05));
}

#[test]
fn systematic_overconfidence_is_detected_and_corrected() {
    // Claims 90% confidence, right only 60% of the time.
    let outcomes: Vec<PredictionOutcome> = (0..200)
        .map(|i| PredictionOutcome::new(0.9, i % 10 < 6))
        .collect();
    let calibration = Calibration::assess(&outcomes, 10);
    assert!(
        calibration.is_overconfident(0.1),
        "bias {}",
        calibration.bias
    );
    assert!(approx_eq(calibration.bias, 0.3, 0.02));

    // The adjustment discounts stated confidence toward reality.
    let adjustment = calibration.confidence_adjustment();
    assert!(adjustment < 1.0, "adjustment {adjustment} should discount");
    assert!(
        approx_eq(0.9 * adjustment, 0.6, 0.05),
        "adjusted to {}",
        0.9 * adjustment
    );
}

#[test]
fn a_coin_flip_forecaster_has_no_skill() {
    let outcomes: Vec<PredictionOutcome> = (0..400)
        .map(|i| PredictionOutcome::new(0.5, i % 2 == 0))
        .collect();
    let calibration = Calibration::assess(&outcomes, 10);
    assert!(approx_eq(calibration.brier_score, 0.25, 1e-9));
    assert!(
        calibration.skill_score.abs() < 1e-6,
        "skill {}",
        calibration.skill_score
    );
}

#[test]
fn a_confident_and_correct_forecaster_shows_skill() {
    let outcomes: Vec<PredictionOutcome> = (0..400)
        .map(|i| {
            let occurs = i % 2 == 0;
            PredictionOutcome::new(if occurs { 0.95 } else { 0.05 }, occurs)
        })
        .collect();
    let calibration = Calibration::assess(&outcomes, 10);
    assert!(calibration.brier_score < 0.01);
    assert!(
        calibration.skill_score > 0.9,
        "skill {}",
        calibration.skill_score
    );
    assert!(calibration.log_loss < 0.1);
}

#[test]
fn a_confident_miss_is_penalised_finitely() {
    // Log loss would be infinite without clamping, which would make one bad
    // call destroy every aggregate score.
    let outcomes = vec![PredictionOutcome::new(1.0, false)];
    let calibration = Calibration::assess(&outcomes, 10);
    assert!(calibration.log_loss.is_finite());
    assert!(
        calibration.log_loss > 10.0,
        "it should still hurt: {}",
        calibration.log_loss
    );
}

#[test]
fn calibration_of_an_empty_set_is_defined() {
    let calibration = Calibration::assess(&[], 10);
    assert_eq!(calibration.count, 0);
    assert_eq!(calibration.brier_score, 0.0);
    assert_eq!(calibration.confidence_adjustment(), 1.0);
}

#[test]
fn too_few_observations_do_not_justify_an_adjustment() {
    let outcomes: Vec<PredictionOutcome> =
        (0..5).map(|_| PredictionOutcome::new(0.9, false)).collect();
    let calibration = Calibration::assess(&outcomes, 10);
    assert_eq!(
        calibration.confidence_adjustment(),
        1.0,
        "five observations cannot support a correction"
    );
}

#[test]
fn drift_is_detected_when_the_distribution_moves() {
    use qip_core::rng::{Rng, Xoshiro256};
    let mut rng = Xoshiro256::seeded(3);
    let reference: Vec<f64> = (0..2000).map(|_| rng.normal()).collect();
    let same: Vec<f64> = (0..2000).map(|_| rng.normal()).collect();
    let shifted: Vec<f64> = (0..2000).map(|_| rng.normal_with(2.0, 1.0)).collect();
    let wider: Vec<f64> = (0..2000).map(|_| rng.normal_with(0.0, 4.0)).collect();

    let stable = DriftReport::compare(&reference, &same, 10);
    assert!(
        stable.population_stability_index < 0.1,
        "{}",
        stable.summary()
    );
    assert!(!stable.requires_attention());

    let moved = DriftReport::compare(&reference, &shifted, 10);
    assert!(
        moved.population_stability_index > 0.25,
        "{}",
        moved.summary()
    );
    assert!(moved.standardised_mean_shift > 1.5);
    assert!(moved.requires_attention());

    let volatile = DriftReport::compare(&reference, &wider, 10);
    assert!(volatile.volatility_ratio > 2.0, "{}", volatile.summary());
    assert!(volatile.requires_attention());
}

#[test]
fn drift_on_a_degenerate_sample_is_neutral() {
    let report = DriftReport::compare(&[1.0], &[2.0], 10);
    assert_eq!(report.population_stability_index, 0.0);
    assert!(!report.requires_attention());
}

fn _completion_is_inspectable(c: &Completion) -> bool {
    c.is_complete() && c.total_tokens() > 0
}
