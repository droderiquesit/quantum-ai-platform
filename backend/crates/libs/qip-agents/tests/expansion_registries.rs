#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_agents::memory::PromotionPolicy;
use qip_agents::registries::{CapabilityRegistry, CitedEvaluation, OntologyRegistry};
use qip_agents::research::{ResearchRegistry, Screening, Verdict};
use qip_agents::tools::{ToolKind, ToolName, ToolPermission, ToolRegistry};
use qip_contracts::expansion::{CapabilityEntry, CapabilityVerb};
use qip_contracts::expansion::{OntologyTypeFamily, OntologyTypeSpec};

/// ADR 0198: the Ontology Registry has one door above version one, `extend`,
/// and it demands a passing evaluation. Until 2026-10-31 `register` admitted
/// any higher version directly, so the evaluation guarded a door nobody had
/// to use. Mutation: delete the `spec.version != 1` refusal in `register` —
/// the direct version-two registration is admitted and the first `is_err`
/// fails.
#[test]
fn a_type_above_version_one_enters_the_ontology_only_through_an_evaluated_extension() {
    let spec = |version: u32| OntologyTypeSpec {
        family: OntologyTypeFamily::Event,
        name: "earnings-surprise".to_string(),
        version,
        schema: r#"{"type":"object"}"#.to_string(),
        description: "an earnings print away from consensus".to_string(),
    };
    let mut registry = OntologyRegistry::new();
    assert_eq!(registry.type_count(), 0, "premise: empty");
    registry.register(spec(1)).unwrap();
    assert!(
        registry.register(spec(2)).is_err(),
        "version two cannot be registered directly"
    );
    assert!(
        registry.register(spec(7)).is_err(),
        "nor any higher version"
    );
    assert!(
        registry
            .extend(
                OntologyTypeFamily::Event,
                "earnings-surprise",
                r#"{"type":"object","required":["consensus"]}"#,
                "names the consensus it was measured against",
                &CitedEvaluation {
                    id: "eval-9".to_string(),
                    passed: false,
                },
            )
            .is_err(),
        "a failed evaluation extends nothing"
    );
    let next = registry
        .extend(
            OntologyTypeFamily::Event,
            "earnings-surprise",
            r#"{"type":"object","required":["consensus"]}"#,
            "names the consensus it was measured against",
            &CitedEvaluation {
                id: "eval-9".to_string(),
                passed: true,
            },
        )
        .unwrap();
    assert_eq!(next, 2);
    assert_eq!(
        registry
            .get_latest(OntologyTypeFamily::Event, "earnings-surprise")
            .map(|t| t.version),
        Some(2)
    );
    assert_eq!(registry.type_count(), 2, "both versions are kept");
}

/// R12-EXPAND-6056's bar: an Ontology Registry entry missing a declared field
/// is refused. A type registered with a blank schema is a type every detector
/// reading it would consult and find nothing in — the registry would say the
/// type exists while recording no shape for it, and a blank name or
/// description is an entry nobody can find or explain. Each is refused at
/// `register` and leaves the registry empty, and a stored entry whose JSON
/// lacks a field is refused at `decode` naming serde's missing field. The
/// full spec is admitted first, so the refusals are of the blank field and
/// not of the fixture. Mutation: drop the `schema` check in
/// `OntologyTypeSpec::validate` — the blank-schema spec is registered and
/// `get_latest` finds it.
#[test]
fn an_ontology_entry_missing_a_declared_field_is_refused_and_registers_nothing() {
    let full = || OntologyTypeSpec {
        family: OntologyTypeFamily::Asset,
        name: "equity".to_string(),
        version: 1,
        schema: r#"{"type":"object","required":["isin"]}"#.to_string(),
        description: "a listed share".to_string(),
    };
    let mut admitted = OntologyRegistry::new();
    admitted
        .register(full())
        .expect("premise: the full spec is admitted");
    assert!(
        admitted
            .get_latest(OntologyTypeFamily::Asset, "equity")
            .is_some(),
        "premise: an admitted spec is found"
    );

    let blanks = [
        (
            "name",
            OntologyTypeSpec {
                name: " ".to_string(),
                ..full()
            },
        ),
        (
            "schema",
            OntologyTypeSpec {
                schema: String::new(),
                ..full()
            },
        ),
        (
            "description",
            OntologyTypeSpec {
                description: "\t".to_string(),
                ..full()
            },
        ),
    ];
    for (field, spec) in blanks {
        let mut registry = OntologyRegistry::new();
        let refused = registry
            .register(spec.clone())
            .expect_err(&format!("a spec with a blank {field} was registered"));
        assert!(
            refused
                .message()
                .contains(&format!("OntologyTypeSpec.{field} is blank")),
            "the blank {field} was refused for another reason: {refused}"
        );
        assert_eq!(registry.type_count(), 0, "blank {field} left an entry");
        assert!(
            registry
                .get_latest(OntologyTypeFamily::Asset, &spec.name)
                .is_none(),
            "blank {field} is found by get_latest"
        );
    }

    let object = serde_json::to_value(full()).unwrap();
    let object = object.as_object().unwrap();
    assert!(
        OntologyTypeSpec::decode(&serde_json::to_vec(object).unwrap()).is_ok(),
        "premise: the full spec decodes"
    );
    assert_eq!(object.len(), 5, "premise: five declared fields");
    for key in object.keys() {
        let mut missing = object.clone();
        missing.remove(key);
        let refused = OntologyTypeSpec::decode(&serde_json::to_vec(&missing).unwrap())
            .expect_err(&format!("a spec without `{key}` decoded"));
        assert!(
            refused
                .to_string()
                .contains(&format!("missing field `{key}`")),
            "a spec without `{key}` was refused for another reason: {refused}"
        );
    }
}

/// ADR 0198: a lesson's promotion policy cannot have its floors lowered to a
/// gate that cannot refuse. Mutation: drop the `minimum_support < 2` refusal
/// in `PromotionPolicy::new` — the zero-support policy is admitted and the
/// first assertion fails.
#[test]
fn a_promotion_policy_with_a_floor_that_cannot_refuse_is_itself_refused() {
    assert!(PromotionPolicy::new(0, 0.65).is_err(), "no support floor");
    assert!(PromotionPolicy::new(1, 0.65).is_err(), "one observation");
    assert!(PromotionPolicy::new(4, 0.5).is_err(), "a coin flip");
    assert!(PromotionPolicy::new(4, 1.5).is_err(), "above certainty");
    assert!(PromotionPolicy::new(4, f64::NAN).is_err(), "not a number");
    let policy = PromotionPolicy::new(4, 0.65).unwrap();
    assert_eq!(policy.minimum_support(), 4);
    assert_eq!(
        policy,
        PromotionPolicy::default(),
        "the default states the same floors"
    );
    let decoded: Result<PromotionPolicy, _> =
        serde_json::from_str(r#"{"minimum_support":0,"minimum_smoothed_support":0.65}"#);
    assert!(
        decoded.is_err(),
        "a decoded policy goes through the same gate"
    );
}

/// ADR 0158 decision 15: the one way the Capability Registry is populated is
/// a promoted capability declaring its verb. Two promotions under one verb
/// are one entry with both implementations at the lower confidence; the same
/// implementation promoted twice is refused, so a consumer counting a verb's
/// implementations counts capabilities and not promotions; and a second
/// implementation cannot widen the verb's eligibility by riding in beside it.
/// Mutation: make `promote` call `add_capability(entry)` without merging —
/// the second promotion replaces the first and the `implementations` and
/// `confidence_level` assertions fail; delete the duplicate check — the
/// `is_err` assertion fails; delete the live-eligibility refusal — the
/// `limited_autonomous_live` entry is recorded and `capability_count` is 1.
#[test]
fn a_second_promotion_under_one_verb_merges_its_implementation_and_a_repeat_is_refused() {
    let entry = |implementation: &str, confidence: f64, eligibility: &str| CapabilityEntry {
        verb: CapabilityVerb::Simulate,
        implementations: vec![implementation.to_string()],
        confidence_level: confidence,
        eligibility: eligibility.to_string(),
    };
    let mut registry = CapabilityRegistry::new();
    assert_eq!(registry.capability_count(), 0, "premise: empty");

    registry
        .promote(entry("fill-bias XSIM", 1.0, "paper and shadow only"))
        .unwrap();
    registry
        .promote(entry("fill-bias XNYS", 0.8, "paper and shadow only"))
        .unwrap();
    let simulate = registry
        .get_capability(CapabilityVerb::Simulate)
        .expect("the verb is recorded");
    assert_eq!(
        simulate.implementations,
        vec!["fill-bias XNYS".to_string(), "fill-bias XSIM".to_string()]
    );
    assert!(
        (simulate.confidence_level - 0.8).abs() < 1e-12,
        "{simulate:?}"
    );
    assert_eq!(
        registry.capability_count(),
        1,
        "one verb, two implementations"
    );
    assert_eq!(registry.current_version(), 3, "each promotion is a version");

    let repeat = registry.promote(entry("fill-bias XSIM", 1.0, "paper and shadow only"));
    assert!(repeat.is_err(), "the same implementation promoted twice");
    let widened = registry.promote(entry("fill-bias XLON", 1.0, "paper only"));
    assert!(widened.is_err(), "another eligibility under the same verb");
    let mut fresh = CapabilityRegistry::new();
    let live = CapabilityEntry {
        verb: CapabilityVerb::Settle,
        implementations: vec!["settle-at-venue".to_string()],
        confidence_level: 1.0,
        eligibility: "limited_autonomous_live".to_string(),
    };
    assert!(
        fresh.promote(live).is_err(),
        "an eligibility naming live is refused by the registry itself, not only by its callers"
    );
    assert_eq!(fresh.capability_count(), 0);
    assert_eq!(registry.current_version(), 3, "a refusal moves no version");
    assert_eq!(
        registry
            .get_capability(CapabilityVerb::Simulate)
            .map(|c| c.implementations.len()),
        Some(2)
    );
    assert!(
        registry
            .promote(entry("", 1.0, "paper and shadow only"))
            .is_err(),
        "an entry the contract refuses is not recorded"
    );
}

#[test]
fn a_tool_of_each_of_the_ten_kinds_resolves_by_kind_with_its_scope() {
    assert_eq!(ToolKind::ALL.len(), 10);
    let mut registry = ToolRegistry::new();
    for (i, kind) in ToolKind::ALL.into_iter().enumerate() {
        registry
            .register(ToolName::parse(&format!("tool-{i}")).unwrap(), kind)
            .unwrap();
    }
    for (i, kind) in ToolKind::ALL.into_iter().enumerate() {
        let found = registry.by_kind(kind);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name(), format!("tool-{i}"));
        assert!(found[0].scope().contains(&ToolPermission::Read));
    }
}

/// R12-EXPAND-6059: a tool's declared fields are its name, its kind and its
/// scope. Kind is an enum and scope is fixed at read-only by construction, so
/// the name is the one field a caller can leave out -- and until 2026-10-24
/// the refusal of a blank one was tested by nothing: deleting it left this
/// suite and the deep brain's green, and a nameless tool would then resolve
/// under `""` for any caller that trimmed a typo to nothing.
#[test]
fn a_tool_proposed_or_registered_without_a_name_is_refused_and_nothing_is_registered() {
    let mut registry = ToolRegistry::new();
    // Premise: the same call with a name is admitted, so what follows is a
    // refusal of the missing field and not of the kind or the registry.
    registry
        .register(ToolName::parse("solver-a").unwrap(), ToolKind::Solver)
        .unwrap();
    assert_eq!(registry.by_kind(ToolKind::Solver).len(), 1);

    // Since R12-SEC-9065 `propose` and `register` take a `ToolName`, so a
    // blank name is refused where a name is made, before any registry is
    // reached: no blank can be passed to either at all.
    let empty = ToolName::parse("").unwrap_err();
    assert!(empty.to_string().contains("a tool needs a name"), "{empty}");
    for blank in ["   ", "\t\n"] {
        let refused = ToolName::parse(blank)
            .err()
            .unwrap_or_else(|| panic!("a whitespace-only name {blank:?} became a ToolName"));
        assert!(
            refused.to_string().contains("a tool needs a name"),
            "{refused}"
        );
    }
    // This test also asserted `registry.get(blank).is_none()` for each blank
    // and for `""`. Since `get` takes a `ToolName` those calls no longer
    // compile — the refusal moved from a lookup that found nothing to a key
    // that cannot be built, which the parse refusals above assert, and the
    // `compile_fail` block on
    // `tools::a_completion_over_fetched_content_cannot_name_a_tool_until_validated`
    // holds that a string is no key at all. What a lookup can still be
    // asked is whether the one registered name resolves and nothing else
    // was registered beside it.
    assert!(
        registry
            .get(&ToolName::parse("solver-a").unwrap())
            .is_some()
    );
    assert_eq!(registry.by_kind(ToolKind::Solver).len(), 1);
}

#[test]
fn a_new_tool_is_read_only_and_sandboxed_until_a_promotion_widens_exactly_the_granted_scope() {
    let mut registry = ToolRegistry::new();
    let parser = ToolName::parse("parser-a").unwrap();
    registry.register(parser.clone(), ToolKind::Parser).unwrap();
    registry.authorise(&parser, ToolPermission::Read).unwrap();
    assert!(registry.authorise(&parser, ToolPermission::Write).is_err());
    assert!(
        registry
            .authorise(&parser, ToolPermission::LeaveSandbox)
            .is_err()
    );

    assert!(
        registry
            .promote(&parser, ToolPermission::Write, "  ")
            .is_err()
    );
    assert!(registry.authorise(&parser, ToolPermission::Write).is_err());

    registry
        .promote(&parser, ToolPermission::Write, "eval-2026-10-04")
        .unwrap();
    registry.authorise(&parser, ToolPermission::Write).unwrap();
    assert!(
        registry
            .authorise(&parser, ToolPermission::LeaveSandbox)
            .is_err()
    );
}

#[test]
fn a_repeated_hypothesis_is_matched_to_its_recorded_null_result_before_any_experiment() {
    let mut registry = ResearchRegistry::new();
    assert_eq!(registry.screen("Moon phase predicts oil"), Screening::Novel);
    registry
        .record(
            "moon phase predicts oil",
            Verdict::Null {
                evidence: "exp-17".into(),
            },
        )
        .unwrap();
    match registry.screen("  Moon   Phase predicts OIL ") {
        Screening::PriorNull { evidence, .. } => assert_eq!(evidence, "exp-17"),
        other => panic!("expected a prior null, got {other:?}"),
    }
    assert!(
        registry
            .record(
                "x",
                Verdict::Null {
                    evidence: String::new()
                }
            )
            .is_err()
    );
}

/// R12-COVERAGE-7170: the research registry is versioned like the ontology
/// registry. Recording once overwrote the entry, so a null result re-opened
/// later was lost from the record it was screened against. Mutation:
/// accepting a version equal to the latest lets the stale `version 2` in.
#[test]
fn a_research_verdict_that_does_not_raise_the_version_is_refused_and_every_version_is_kept() {
    let mut registry = ResearchRegistry::new();
    let first = registry
        .record(
            "credit leads equity",
            Verdict::Null {
                evidence: "exp-1".into(),
            },
        )
        .unwrap();
    let second = registry
        .record("Credit leads  equity", Verdict::Open)
        .unwrap();
    // Premise: two verdicts, numbered in order, both held.
    assert_eq!((first, second), (1, 2));
    assert_eq!(registry.history("credit leads equity").len(), 2);

    for stale in [0, 1, 2] {
        assert!(
            registry
                .record_version(
                    "credit leads equity",
                    stale,
                    Verdict::Supported {
                        evidence: "exp-2".into(),
                    },
                )
                .is_err(),
            "version {stale} was accepted over latest 2"
        );
    }
    assert_eq!(registry.latest_version("credit leads equity"), Some(2));
    assert_eq!(
        registry.screen("credit leads equity"),
        Screening::AlreadyKnown {
            hypothesis: "Credit leads  equity".into(),
        }
    );
    assert_eq!(
        registry.history("credit leads equity")[0].verdict,
        Verdict::Null {
            evidence: "exp-1".into()
        }
    );
    registry
        .record_version(
            "credit leads equity",
            3,
            Verdict::Supported {
                evidence: "exp-2".into(),
            },
        )
        .unwrap();
    assert_eq!(registry.latest_version("credit leads equity"), Some(3));
}

mod memory_registry {
    use qip_agents::memory::{
        Disposition, Episode, EpisodeOutcome, MemoryRegistry, RETAINED_MEMORY_ENTRIES,
        ResearchMemory,
    };
    use qip_contracts::expansion::{MemoryEntry, MemoryKind, RetentionPolicy, ValuePolicy};
    use qip_core::ids::AgentRunId;
    use qip_core::time::Timestamp;

    fn entry(kind: MemoryKind, max_age_ms: u64) -> MemoryEntry {
        MemoryEntry {
            id: format!("{kind:?}-1"),
            kind,
            content: "which way it would move: concluded positive; moved 10 bps".to_string(),
            retention_policy: RetentionPolicy {
                max_age_ms,
                description: "one year".to_string(),
            },
            value_policy: ValuePolicy {
                importance_threshold: 0.3,
                rationale: "calibration surprise".to_string(),
            },
            created_at_ms: 1_760_000_000_000,
            importance: 0.9,
            tags: vec!["analyst-a".to_string()],
        }
    }

    /// R12-EXPAND-6057: the registry LEARN writes through holds typed entries
    /// with their policies declared, and refuses one that declares none. A
    /// retention of zero milliseconds keeps nothing and is the "missing"
    /// retention policy in a struct whose field cannot be absent. Mutation:
    /// make `admit` skip `validate` — the zero-retention entry is admitted,
    /// `admitted(Failure)` reads 1 and `refused()` 0.
    #[test]
    fn a_memory_entry_without_a_retention_policy_is_refused_by_the_registry_learn_writes_through() {
        let mut registry = MemoryRegistry::standard();
        assert_eq!(registry.entries().count(), 0, "premise: empty");

        registry.admit(entry(MemoryKind::Episodic, 1_000)).unwrap();
        registry
            .admit(entry(MemoryKind::Simulation, 1_000))
            .unwrap();
        assert_eq!(registry.admitted(MemoryKind::Episodic), 1);
        assert_eq!(registry.admitted(MemoryKind::Simulation), 1);

        let refused = registry.admit(entry(MemoryKind::Failure, 0));
        assert!(
            refused.is_err(),
            "a retention policy keeping nothing is no policy"
        );
        assert_eq!(registry.admitted(MemoryKind::Failure), 0);
        assert_eq!(registry.refused(), 1);
        assert_eq!(
            registry.entries().count(),
            2,
            "the refused entry is not held"
        );

        let mut blank = entry(MemoryKind::Procedural, 1_000);
        blank.tags.clear();
        assert!(
            registry.admit(blank).is_err(),
            "an entry with no tags names nothing"
        );
        assert_eq!(registry.refused(), 2);
    }

    fn at(text: &str) -> Timestamp {
        Timestamp::parse_rfc3339(text).unwrap()
    }

    fn call(run: &str, conviction: f64, correct: bool) -> Episode {
        Episode {
            run_id: AgentRunId::from_string(run.to_string()),
            agent_id: "analyst-a".to_string(),
            occurred_at: at("2026-10-01T00:00:00Z"),
            question: "which way would it move".to_string(),
            conclusion: "positive".to_string(),
            evidence: vec![run.to_string()],
            conviction,
            outcome: Some(EpisodeOutcome {
                resolved_at: at("2026-10-01T00:00:00Z"),
                correct,
                realised_bps: Some(10.0),
                explanation: "moved".to_string(),
            }),
        }
    }

    #[test]
    fn a_below_threshold_episode_is_kept_compressed_and_still_counts_in_the_hit_rate() {
        let mut registry = MemoryRegistry::new(0.3).unwrap();
        let mut memory = ResearchMemory::new();
        // Nine confident calls that came off (surprise 0.1) and one confident
        // call that missed (surprise 0.9): the registry keeps only the miss
        // whole, and a hit rate over what it kept would read 0 rather than 0.9.
        let mut calls: Vec<Episode> = (0..9)
            .map(|n| call(&format!("hit-{n}"), 0.9, true))
            .collect();
        calls.push(call("miss", 0.9, false));
        let mut dispositions = Vec::new();
        for episode in calls {
            dispositions.push(registry.consult(&episode).unwrap());
            memory.record_episode(episode);
        }
        // The premise: both dispositions occurred, so the assertions below
        // are not about a registry that only ever did one thing.
        assert!(dispositions.contains(&Disposition::Retained));
        assert!(dispositions.contains(&Disposition::Compressed));

        assert_eq!(registry.retained_for("analyst-a"), 1);
        let summary = registry.summary_for("analyst-a").unwrap();
        assert_eq!(summary.episodes, 9);
        assert_eq!(summary.correct, 9);
        assert_eq!(memory.episode_count(), 10, "compression drops no episode");
        let rate = memory
            .hit_rate("analyst-a", at("2026-10-02T00:00:00Z"))
            .unwrap();
        assert!((rate - 0.9).abs() < 1e-9, "hit rate was {rate}");
    }

    #[test]
    fn a_registry_refuses_a_threshold_that_could_never_compress_and_an_unresolved_episode() {
        for bad in [0.0, -0.1, 1.5, f64::NAN, f64::INFINITY] {
            assert!(MemoryRegistry::new(bad).is_err(), "{bad} was accepted");
        }
        let mut registry = MemoryRegistry::new(1.0).unwrap();
        let mut open = call("open", 0.5, true);
        open.outcome = None;
        assert_eq!(registry.consult(&open), None);
        assert_eq!(registry.retained_for("analyst-a"), 0);
    }

    fn valued(id: &str, created_at_ms: u64, importance: f64, max_age_ms: u64) -> MemoryEntry {
        let mut e = entry(MemoryKind::Episodic, max_age_ms);
        e.id = id.to_string();
        e.created_at_ms = created_at_ms;
        e.importance = importance;
        e
    }

    /// R12-EXPAND-6057: at capacity the registry gives up the entry its own
    /// value policy rates least, not the oldest. The oldest entry is a
    /// costly failure (importance 0.9); a later one sits below its 0.3
    /// threshold, and a third has lower importance but clears its own lower
    /// threshold. First-in-first-out would drop the failure; ranking by raw
    /// importance would drop the third. Mutations: evict index 0 in `admit`
    /// — the failure is gone; drop the threshold key from the ordering —
    /// the modest entry is gone.
    #[test]
    fn a_full_registry_evicts_the_entry_below_its_value_threshold_rather_than_the_oldest() {
        let mut registry = MemoryRegistry::standard();
        let year = 365 * 24 * 60 * 60 * 1_000;
        let base = 1_760_000_000_000;
        registry
            .admit(valued("costly-miss", base, 0.9, year))
            .unwrap();
        registry
            .admit(valued("unsurprising", base + 1, 0.25, year))
            .unwrap();
        // Lower importance still, but above its own, lower threshold: the
        // value policy, not the raw importance, decides.
        let mut modest = valued("modest-but-valued", base + 2, 0.2, year);
        modest.value_policy.importance_threshold = 0.1;
        registry.admit(modest).unwrap();
        for n in 3..RETAINED_MEMORY_ENTRIES as u64 {
            registry
                .admit(valued(&format!("call-{n}"), base + n, 0.6, year))
                .unwrap();
        }
        assert_eq!(
            registry.entries().count(),
            RETAINED_MEMORY_ENTRIES,
            "premise: the working set is full"
        );
        registry
            .admit(valued("newest", base + 10_000, 0.6, year))
            .unwrap();
        let ids: Vec<&str> = registry.entries().map(|e| e.id.as_str()).collect();
        assert_eq!(ids.len(), RETAINED_MEMORY_ENTRIES);
        assert!(
            ids.contains(&"costly-miss"),
            "the oldest, valuable entry stays"
        );
        assert!(
            !ids.contains(&"unsurprising"),
            "the entry below its threshold left"
        );
        assert!(ids.contains(&"modest-but-valued"), "{ids:?}");
        assert_eq!(registry.evicted_for_value(), 1);
        assert_eq!(registry.expired(), 0);
    }

    /// R12-EXPAND-6057: an entry leaves once its own declared retention age
    /// has passed by the registry's clock, the newest creation instant
    /// admitted, even while the set is far from full. Mutation: make the
    /// retention `retain` keep everything — the expired entry is still held
    /// and `expired()` reads 0.
    #[test]
    fn an_entry_past_its_retention_age_leaves_when_a_newer_one_is_admitted() {
        let mut registry = MemoryRegistry::standard();
        let base = 1_760_000_000_000;
        registry
            .admit(valued("short-lived", base, 0.9, 1_000))
            .unwrap();
        registry
            .admit(valued("long-lived", base, 0.9, 1_000_000))
            .unwrap();
        registry
            .admit(valued("on-time", base + 1_000, 0.9, 1_000_000))
            .unwrap();
        let ids: Vec<&str> = registry.entries().map(|e| e.id.as_str()).collect();
        assert!(
            ids.contains(&"short-lived"),
            "premise: an entry exactly at its age is still inside it: {ids:?}"
        );
        registry
            .admit(valued("later", base + 1_001, 0.9, 1_000_000))
            .unwrap();
        let ids: Vec<&str> = registry.entries().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["long-lived", "on-time", "later"]);
        assert_eq!(registry.expired(), 1);
        assert_eq!(registry.evicted_for_value(), 0);
    }
}

mod campaign_identity {
    use qip_agents::research::{ResearchRegistry, Verdict, campaign_id, experiment_ref};

    #[test]
    fn a_campaign_has_a_stable_id_and_a_verdict_cites_its_experiment_references() {
        // Case and spacing do not make a question new, and a different
        // question is a different campaign.
        let id = campaign_id("Does  momentum persist?").unwrap();
        assert_eq!(id, campaign_id("does momentum persist?").unwrap());
        assert_ne!(id, campaign_id("does reversal persist?").unwrap());
        assert!(campaign_id("   ").is_err());

        let mut registry = ResearchRegistry::new();
        let null = Verdict::Null {
            evidence: "failed leakage".to_string(),
        };
        let bench = qip_agents::research::BenchmarkRef::new(
            "fill-bias capability",
            "the uncorrected twin",
            "IncrementalValue",
        )
        .unwrap();
        // No experiment, another question's experiment, and a bare campaign
        // id with no stage are each refused; the premise is that the id used
        // below is the one a correct reference would carry.
        let other = campaign_id("does reversal persist?").unwrap();
        for bad in [
            vec![],
            vec![experiment_ref(&other, "historical")],
            vec![format!("{id}/")],
            vec![id.clone()],
        ] {
            assert!(
                registry
                    .record_concluded("Does momentum persist?", null.clone(), &bad, Some(&bench))
                    .is_err(),
                "{bad:?} was accepted"
            );
        }
        assert!(registry.history("does momentum persist?").is_empty());

        let refs = [
            experiment_ref(&id, "simulated"),
            experiment_ref(&id, "historical"),
            experiment_ref(&id, "historical"),
        ];
        let version = registry
            .record_concluded("Does momentum persist?", null, &refs, Some(&bench))
            .unwrap();
        assert_eq!(version, 1);
        let held = registry.history("DOES MOMENTUM PERSIST?");
        assert_eq!(
            held[0].experiments,
            vec![
                experiment_ref(&id, "historical"),
                experiment_ref(&id, "simulated")
            ],
            "sorted and de-duplicated"
        );
    }
}

/// ADR 0348 (R12-EXPAND-6061): an entry missing a declared field is
/// refused. A concluded verdict with no benchmark, a benchmark naming no
/// incumbent, and a contradiction citing no evidence or keyed by something
/// that is not a campaign id are each refused, and nothing is recorded. The
/// premise is that the same entry with every field is accepted.
///
/// Mutation: accept a Supported verdict with no benchmark.
#[test]
fn an_entry_missing_a_declared_field_is_refused() {
    use qip_agents::research::{BenchmarkRef, campaign_id, experiment_ref};
    let question = "Does the twin miss fills at XSIM?";
    let id = campaign_id(question).unwrap();
    let refs = [experiment_ref(&id, "shadow")];
    let supported = Verdict::Supported {
        evidence: "promoted".to_string(),
    };
    let bench = BenchmarkRef::new(
        "fill-bias capability",
        "the uncorrected twin",
        "IncrementalValue",
    )
    .unwrap();

    let mut registry = ResearchRegistry::new();
    assert!(
        registry
            .record_concluded(question, supported.clone(), &refs, None)
            .is_err(),
        "a Supported verdict with no benchmark was accepted"
    );
    assert!(registry.history(question).is_empty());
    assert!(BenchmarkRef::new("fill-bias capability", " ", "IncrementalValue").is_err());
    assert!(registry.record_contradiction(&id, &[]).is_err());
    assert!(
        registry
            .record_contradiction(&id, &[" ".to_string()])
            .is_err()
    );
    assert!(
        registry
            .record_contradiction("price-vs-belief", &["detector:contradiction".to_string()])
            .is_err(),
        "a contradiction keyed by a subject, not a campaign, was accepted"
    );
    assert!(registry.unresolved_contradictions().is_empty());

    // Every field present: accepted, and the benchmark is kept on the version.
    let version = registry
        .record_concluded(question, supported, &refs, Some(&bench))
        .unwrap();
    assert_eq!(version, 1);
    assert_eq!(
        registry.history(question)[0].benchmark.as_ref(),
        Some(&bench)
    );
}

/// ADR 0348: a contradiction stays unresolved until a campaign on the same
/// id concludes. An `Open` verdict on the question does not resolve it, a
/// concluded verdict on another question does not, and the concluded one
/// on its own question does, naming the version. The open question is
/// listed while its latest verdict is `Open`, and not after.
///
/// Mutation: let record_concluded skip the resolution.
#[test]
fn a_contradiction_stays_unresolved_until_a_campaign_on_the_same_id_concludes() {
    use qip_agents::research::{BenchmarkRef, campaign_id, experiment_ref};
    let question = "Do the two drift detectors disagree on AAA?";
    let other = "Does the twin miss fills at XSIM?";
    let id = campaign_id(question).unwrap();
    let bench = BenchmarkRef::new(
        "fill-bias capability",
        "the uncorrected twin",
        "IncrementalValue",
    )
    .unwrap();
    let null = Verdict::Null {
        evidence: "failed IncrementalValue".to_string(),
    };
    let mut registry = ResearchRegistry::new();
    registry
        .record_contradiction(&id, &["detector:contradiction".to_string()])
        .unwrap();
    registry
        .record_contradiction(
            &id,
            &[
                "detector:contradiction".to_string(),
                "drift:AAA".to_string(),
            ],
        )
        .unwrap();
    let open = registry.unresolved_contradictions();
    assert_eq!(open.len(), 1, "a repeat opened a second record: {open:?}");
    assert_eq!(
        open[0].evidence,
        ["detector:contradiction", "drift:AAA"].map(String::from)
    );

    registry.record(question, Verdict::Open).unwrap();
    assert_eq!(registry.open_questions().len(), 1);
    assert_eq!(
        registry.unresolved_contradictions().len(),
        1,
        "Open resolved it"
    );
    let other_id = campaign_id(other).unwrap();
    registry
        .record_concluded(
            other,
            null.clone(),
            &[experiment_ref(&other_id, "shadow")],
            Some(&bench),
        )
        .unwrap();
    assert_eq!(
        registry.unresolved_contradictions().len(),
        1,
        "another campaign's verdict resolved it"
    );

    let version = registry
        .record_concluded(
            question,
            null,
            &[experiment_ref(&id, "shadow")],
            Some(&bench),
        )
        .unwrap();
    assert!(registry.unresolved_contradictions().is_empty());
    assert!(
        registry.open_questions().is_empty(),
        "a concluded question is still listed open"
    );
    assert_eq!(version, 2);
}
