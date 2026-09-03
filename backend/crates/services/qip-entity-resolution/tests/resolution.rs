//! Entity resolution: matching, linking, refusing to guess.

// Exact float comparison is deliberate: an unresolved record must carry exactly
// zero confidence, and an identical name must score exactly one.
#![allow(clippy::float_cmp)]

use qip_core::testing::approx_eq;
use qip_core::{Context, Timestamp};
use qip_entity_resolution::entity::{Entity, EntityKind, EntityRecord};
use qip_entity_resolution::matching::{
    jaro_winkler, name_similarity, normalise_name, token_containment,
};
use qip_entity_resolution::resolver::{
    Decision, Resolver, ResolverConfig, seed_reference_entities,
};
use qip_financial::identifiers::{IdentifierKind, Identifiers};

fn context() -> (Context, Timestamp) {
    let now = Timestamp::from_civil(2026, 8, 24);
    let (context, _clock) = Context::deterministic(now, 42);
    (context, now)
}

fn record(name: &str, source: &str) -> EntityRecord {
    EntityRecord::new(
        format!("{source}-{name}"),
        EntityKind::Company,
        name,
        source,
        Timestamp::from_civil(2026, 8, 24),
    )
}

// --- name normalisation and similarity ---------------------------------------

#[test]
fn legal_forms_are_stripped_but_distinguishing_words_are_not() {
    assert_eq!(
        normalise_name("Northwind Semiconductor Corporation"),
        normalise_name("Northwind Semiconductor Corp.")
    );
    assert_eq!(normalise_name("Kestrel Materials PLC"), "kestrel materials");
    assert_eq!(normalise_name("THE VANTAGE GROUP, INC."), "vantage");

    // Words that actually distinguish entities survive.
    assert_ne!(
        normalise_name("Acme Series A"),
        normalise_name("Acme Series B")
    );
    assert_ne!(
        normalise_name("Meridian Energy Operating"),
        normalise_name("Meridian Energy Trading")
    );
}

#[test]
fn a_name_made_only_of_legal_forms_is_not_erased() {
    // Stripping to nothing would make every such name identical to every other.
    assert!(!normalise_name("The Company Group").is_empty());
    assert_ne!(
        normalise_name("The Company Group"),
        normalise_name("Holdings Ltd")
    );
}

#[test]
fn jaro_winkler_rewards_a_shared_prefix() {
    assert!(approx_eq(
        jaro_winkler("northwind", "northwind"),
        1.0,
        1e-12
    ));
    let shared_prefix = jaro_winkler("northwind semi", "northwind semic");
    let no_prefix = jaro_winkler("northwind semi", "xorthwind semi");
    assert!(shared_prefix > no_prefix, "{shared_prefix} vs {no_prefix}");
    assert!(jaro_winkler("", "") > 0.99);
    assert_eq!(jaro_winkler("abc", ""), 0.0);
}

#[test]
fn token_containment_catches_abbreviations() {
    assert!(
        token_containment("Northwind Semi", "Northwind Semiconductor Corporation") > 0.99,
        "every token of the short form appears in the long one"
    );
    assert!(token_containment("Kestrel Materials", "Meridian Energy") < 0.1);
}

#[test]
fn name_similarity_links_the_forms_a_company_actually_appears_in() {
    let canonical = "Northwind Semiconductor Corporation";
    for variant in [
        "Northwind Semiconductor Corp",
        "NORTHWIND SEMICONDUCTOR CORPORATION",
        "Northwind Semiconductor",
        "Northwind Semi",
    ] {
        let score = name_similarity(canonical, variant);
        assert!(score > 0.85, "`{variant}` scored only {score}");
    }
}

#[test]
fn name_similarity_separates_genuinely_different_companies() {
    // These are the pairs a resolver must not merge.
    for (a, b) in [
        ("Northwind Semiconductor", "Meridian Energy Holdings"),
        ("Atlas Federal Bancorp", "Atlas Copco"),
        ("Vantage Devices", "Advantage Solutions"),
        ("Kestrel Materials", "Kestrel Mining"),
        ("Aurora Mining Group", "Kestrel Materials"),
    ] {
        let score = name_similarity(a, b);
        assert!(score < 0.85, "`{a}` and `{b}` scored {score}, too close");
    }

    // Two names with nothing in common must score at zero, not at the noise
    // floor that character overlap between arbitrary English words produces.
    assert!(name_similarity("Aurora Mining Group", "Kestrel Materials") < 0.1);
}

// --- resolution -------------------------------------------------------------

#[test]
fn the_first_record_creates_an_entity() {
    let (context, _) = context();
    let mut resolver = Resolver::default();
    let (decision, event) = resolver.resolve(
        &record("Northwind Semiconductor Corporation", "filings"),
        &context,
    );

    assert!(matches!(decision, Decision::Created { .. }));
    assert!(event.created_new);
    assert_eq!(resolver.len(), 1);
    assert_eq!(event.confidence, 1.0);
}

#[test]
fn variant_spellings_collapse_onto_one_entity() {
    let (context, _) = context();
    let mut resolver = Resolver::default();
    for (name, source) in [
        ("Northwind Semiconductor Corporation", "filings"),
        ("Northwind Semiconductor Corp", "newswire"),
        ("NORTHWIND SEMICONDUCTOR CORPORATION", "vendor"),
        ("Northwind Semi", "research"),
    ] {
        resolver.resolve(&record(name, source), &context);
    }

    assert_eq!(resolver.len(), 1, "four spellings, one company");
    let entity = resolver.entities().next().unwrap();
    // Spellings that normalise identically are one alias; a genuinely distinct
    // surface form is kept so it can be matched later.
    assert!(
        entity
            .aliases
            .iter()
            .any(|a| a.contains("Semi") && !a.contains("Semiconductor")),
        "aliases: {:?}",
        entity.aliases
    );
    assert_eq!(entity.contributing_sources.len(), 4);
    assert_eq!(resolver.report().linked, 3);
    assert_eq!(resolver.report().created, 1);
}

#[test]
fn an_identifier_match_links_across_unrelated_names() {
    // A record whose name is a bare ticker still resolves, because the ISIN
    // agrees. Name similarity alone would miss this entirely.
    let (context, _) = context();
    let mut resolver = Resolver::default();
    let full = record("Northwind Semiconductor Corporation", "filings")
        .with_identifiers(Identifiers::new().with(IdentifierKind::Isin, "US0378331005"));
    resolver.resolve(&full, &context);

    let ticker_only = record("NWSC", "market-feed")
        .with_identifiers(Identifiers::new().with(IdentifierKind::Isin, "US0378331005"));
    let (decision, event) = resolver.resolve(&ticker_only, &context);

    assert!(matches!(decision, Decision::Linked { .. }), "{decision:?}");
    assert_eq!(resolver.len(), 1);
    assert!(
        event.rationale.contains("identifier"),
        "{}",
        event.rationale
    );
}

#[test]
fn conflicting_identifiers_prevent_a_merge_despite_an_identical_name() {
    // Two companies genuinely can share a name. Different ISINs settle it, and
    // must outweigh a perfect name match.
    let (context, _) = context();
    let mut resolver = Resolver::default();
    resolver.resolve(
        &record("Atlas Holdings", "vendor-a")
            .with_identifiers(Identifiers::new().with(IdentifierKind::Isin, "US0378331005")),
        &context,
    );
    let (decision, _) = resolver.resolve(
        &record("Atlas Holdings", "vendor-b")
            .with_identifiers(Identifiers::new().with(IdentifierKind::Isin, "US5949181045")),
        &context,
    );

    assert!(
        matches!(decision, Decision::Created { .. }),
        "different ISINs mean different companies: {decision:?}"
    );
    assert_eq!(resolver.len(), 2);
}

#[test]
fn a_country_mismatch_blocks_a_merge_on_a_similar_name() {
    let (context, _) = context();
    let mut resolver = Resolver::default();
    resolver.resolve(
        &record("Kestrel Materials", "vendor").with_country("GB"),
        &context,
    );
    let (decision, _) = resolver.resolve(
        &record("Kestrel Materials", "vendor").with_country("AU"),
        &context,
    );

    assert!(
        !matches!(decision, Decision::Linked { .. }),
        "same name in different countries is usually a different company: {decision:?}"
    );
}

#[test]
fn matching_countries_corroborate_a_name_match() {
    let (context, _) = context();
    let mut resolver = Resolver::default();
    resolver.resolve(
        &record("Meridian Energy Holdings", "filings").with_country("US"),
        &context,
    );
    let (decision, _) = resolver.resolve(
        &record("Meridian Energy", "newswire").with_country("US"),
        &context,
    );

    match decision {
        Decision::Linked { evidence, .. } => {
            assert!(
                evidence
                    .iter()
                    .any(|e| e.signal == "country" && e.strength > 0.0)
            );
        }
        other => panic!("expected a link, got {other:?}"),
    }
}

#[test]
fn an_uncertain_match_is_held_for_review_rather_than_guessed() {
    // The band between the thresholds exists precisely so a borderline case
    // does not become a silent wrong merge.
    let (context, _) = context();
    let config = ResolverConfig {
        link_threshold: 0.95,
        new_entity_threshold: 0.60,
        ..ResolverConfig::default()
    };
    let mut resolver = Resolver::new(config);
    resolver.resolve(&record("Vantage Devices Incorporated", "filings"), &context);
    let (decision, event) = resolver.resolve(&record("Vantage Device Systems", "vendor"), &context);

    assert!(decision.is_ambiguous(), "{decision:?}");
    assert_eq!(event.confidence, 0.0);
    assert_eq!(
        resolver.len(),
        1,
        "an unresolved record must not create an entity either"
    );
    assert_eq!(resolver.report().ambiguous, 1);
    assert_eq!(resolver.report().held.len(), 1);
    assert!(resolver.report().held[0].1.contains("threshold"));
    assert!(resolver.report().automation_rate() < 1.0);
}

#[test]
fn two_equally_good_candidates_are_ambiguous_however_high_they_score() {
    // Even a very high score is not a decision when a second candidate scores
    // the same: the evidence does not distinguish them.
    let (context, _) = context();
    let mut resolver = Resolver::default();
    let now = Timestamp::from_civil(2026, 8, 24);
    for suffix in ["Holdings", "Group"] {
        let entity = Entity::new(
            context.ids().generate(now),
            EntityKind::Company,
            format!("Sterling Capital {suffix}"),
            now,
        );
        resolver.insert(entity);
    }

    let (decision, _) = resolver.resolve(&record("Sterling Capital", "vendor"), &context);
    match decision {
        Decision::Ambiguous { candidates, reason } => {
            assert!(candidates.len() >= 2);
            assert!(reason.contains("almost equally well"), "{reason}");
        }
        other => panic!("expected ambiguity, got {other:?}"),
    }
}

#[test]
fn entities_of_different_kinds_never_match() {
    let (context, now) = context();
    let mut resolver = Resolver::default();
    resolver.insert(Entity::new(
        context.ids().generate(now),
        EntityKind::Country,
        "Meridian",
        now,
    ));
    let (decision, _) = resolver.resolve(&record("Meridian", "vendor"), &context);
    assert!(
        matches!(decision, Decision::Created { .. }),
        "a country and a company are not the same thing: {decision:?}"
    );
    assert_eq!(resolver.len(), 2);
}

#[test]
fn a_weak_merge_lowers_the_entitys_coherence() {
    let (context, _) = context();
    let config = ResolverConfig {
        link_threshold: 0.80,
        ..ResolverConfig::default()
    };
    let mut resolver = Resolver::new(config);
    resolver.resolve(
        &record("Northwind Semiconductor Corporation", "filings"),
        &context,
    );
    resolver.resolve(&record("Northwind Semi", "social"), &context);

    let entity = resolver.entities().next().unwrap();
    assert!(
        entity.coherence < 1.0,
        "a weaker link should reduce coherence"
    );
    // And the platform can list entities that are no longer trustworthy.
    let unreliable = resolver.unreliable();
    assert_eq!(unreliable.len(), usize::from(entity.coherence < 0.85));
}

#[test]
fn a_manual_merge_repoints_the_indexes() {
    let (context, now) = context();
    let mut resolver = Resolver::default();
    let (first, _) = resolver.resolve(
        &record("Kestrel Materials PLC", "filings")
            .with_identifiers(Identifiers::new().with(IdentifierKind::Isin, "GB0002634946")),
        &context,
    );
    // A name with nothing in common, so both are created rather than compared.
    let (second, _) = resolver.resolve(&record("Aurora Mining Group", "vendor"), &context);
    assert_eq!(resolver.len(), 2);

    let keep = first.entity_id().unwrap().clone();
    let absorb = second.entity_id().unwrap().clone();
    resolver.merge(&keep, &absorb, now).unwrap();

    assert_eq!(resolver.len(), 1);
    let survivor = resolver.get(&keep).unwrap();
    assert!(
        survivor.aliases.iter().any(|a| a.contains("Aurora")),
        "aliases: {:?}",
        survivor.aliases
    );

    // A later record under the absorbed name now resolves to the survivor.
    let (decision, _) = resolver.resolve(&record("Aurora Mining Group", "vendor2"), &context);
    assert_eq!(decision.entity_id(), Some(&keep), "{decision:?}");
}

#[test]
fn merging_an_unknown_entity_is_an_error_and_loses_nothing() {
    let (context, now) = context();
    let mut resolver = Resolver::default();
    let (created, _) = resolver.resolve(&record("Kestrel Materials", "filings"), &context);
    let real = created.entity_id().unwrap().clone();
    let ghost = qip_core::EntityId::from_string("ENT0000000000000000GHOST");

    assert!(resolver.merge(&real, &ghost, now).is_err());
    assert_eq!(resolver.len(), 1, "nothing lost");
    assert!(resolver.merge(&ghost, &real, now).is_err());
    assert_eq!(resolver.len(), 1, "the real entity is put back");
}

#[test]
fn reference_entities_can_be_seeded() {
    let (context, _) = context();
    let mut resolver = Resolver::default();
    seed_reference_entities(&mut resolver, &context);
    assert!(resolver.len() >= 6);

    // A macro record now resolves onto the seeded country.
    let mut fed = EntityRecord::new(
        "macro-1",
        qip_entity_resolution::entity::EntityKind::CentralBank,
        "Federal Reserve",
        "macro-feed",
        Timestamp::from_civil(2026, 8, 24),
    );
    fed.attributes.insert("kind".into(), "central_bank".into());
    let (decision, _) = resolver.resolve(&fed, &context);
    assert!(matches!(decision, Decision::Linked { .. }), "{decision:?}");
}

#[test]
fn resolution_is_deterministic() {
    let run = || {
        let (context, _) = context();
        let mut resolver = Resolver::default();
        for (name, source) in [
            ("Northwind Semiconductor Corporation", "filings"),
            ("Meridian Energy Holdings", "filings"),
            ("Northwind Semi", "newswire"),
            ("Kestrel Materials PLC", "vendor"),
            ("Meridian Energy", "newswire"),
        ] {
            resolver.resolve(&record(name, source), &context);
        }
        let mut names: Vec<String> = resolver
            .entities()
            .map(|e| e.canonical_name.clone())
            .collect();
        names.sort();
        (resolver.len(), names)
    };
    assert_eq!(run(), run());
}

#[test]
fn a_disagreeing_weak_identifier_does_not_override_an_identical_name() {
    // Exchange tickers are deliberately weak evidence (confidence 0.4): they
    // are reused across venues and reassigned over time, so an *agreeing*
    // ticker must never be authoritative on its own (see
    // `IdentifierKind::confidence`). The same logic means a *disagreeing*
    // weak ticker must not be treated as decisive either — only a strong
    // identifier (ISIN, FIGI, LEI, ...) disagreeing is grounds to force two
    // records apart despite an identical name.
    let (context, _) = context();
    let mut resolver = Resolver::default();
    resolver.resolve(
        &record("Northwind Semiconductor Corporation", "filings")
            .with_identifiers(Identifiers::new().with(IdentifierKind::ExchangeSymbol, "NWSC")),
        &context,
    );
    let (decision, _) = resolver.resolve(
        &record("Northwind Semiconductor Corporation", "newswire")
            .with_identifiers(Identifiers::new().with(IdentifierKind::ExchangeSymbol, "NWS")),
        &context,
    );

    assert!(
        matches!(decision, Decision::Linked { .. }),
        "a disagreeing exchange ticker must not override an identical name: {decision:?}"
    );
}

#[test]
fn a_link_carries_the_evidence_that_produced_it() {
    let (context, _) = context();
    let mut resolver = Resolver::default();
    resolver.resolve(
        &record("Northwind Semiconductor Corporation", "filings").with_country("US"),
        &context,
    );
    let (decision, event) = resolver.resolve(
        &record("Northwind Semiconductor Corp", "newswire").with_country("US"),
        &context,
    );

    match decision {
        Decision::Linked {
            score, evidence, ..
        } => {
            assert!(score > 0.9);
            assert!(evidence.iter().any(|e| e.signal == "name"));
            assert!(!evidence.iter().any(|e| e.is_disconfirming()));
            assert!(!event.rationale.is_empty());
        }
        other => panic!("expected a link, got {other:?}"),
    }
}
