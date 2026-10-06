#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

//! The expansion loop's curriculum, one test per register row it closes.
//! Generated cases come from a fixed-seed generator, so a failure replays.

use qip_agents::research::{ResearchRegistry, Verdict};
use qip_agents::tools::{ToolKind, ToolPermission, ToolRegistry};
use qip_contracts::expansion::{CurriculumItem, GapClass};
use qip_core::Decimal;
use qip_expansion::curriculum::{
    Bounds, Budget, Candidate, Criteria, Need, ResearchQueue, State, TaskKind,
};
use qip_expansion::gap::{GapTrigger, Observation, Wanting, classify, raise};
use std::collections::BTreeSet;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    /// A fraction in `[0, 1]`.
    fn unit(&mut self) -> f64 {
        f64::from(u32::try_from(self.next() % 10_001).unwrap()) / 10_000.0
    }
    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[usize::try_from(self.next()).unwrap() % from.len()]
    }
    fn whole(&mut self, below: u64) -> i64 {
        i64::try_from(self.next() % below).unwrap()
    }
}

fn dec(v: i64) -> Decimal {
    Decimal::from_int(v)
}

/// What the candidates below are admitted against.
struct World {
    licensed: BTreeSet<String>,
    tools: ToolRegistry,
    eligible: BTreeSet<String>,
    research: ResearchRegistry,
}

impl World {
    fn new() -> Self {
        let mut tools = ToolRegistry::new();
        tools.register("twin", ToolKind::Simulator).unwrap();
        Self {
            licensed: BTreeSet::from(["cpi-series".to_string()]),
            tools,
            eligible: BTreeSet::from(["US".to_string()]),
            research: ResearchRegistry::new(),
        }
    }
    fn bounds(&self) -> Bounds<'_> {
        Bounds {
            licensed_sources: &self.licensed,
            tools: &self.tools,
            eligible_jurisdictions: &self.eligible,
            research: &self.research,
        }
    }
}

fn criteria(value: i64, cost: i64) -> Criteria {
    Criteria {
        expected_economic_value: Some(dec(value)),
        uncertainty_reduction: Some(0.11),
        strategic_coverage: Some(0.12),
        risk_reduction: Some(0.13),
        information_gain: Some(1.5),
        reuse_across_domains: Some(0.14),
        compute_data_cost: Some(dec(cost)),
        feasibility: Some(0.9),
        novelty: Some(0.15),
        failure_frequency: Some(0.16),
        urgency: Some(0.17),
    }
}

/// A complete candidate that needs one licensed source and nothing else.
fn candidate(kind: TaskKind, question: &str, value: i64, cost: i64) -> Candidate {
    Candidate {
        kind,
        item: CurriculumItem {
            research_question: question.to_string(),
            expected_information_value: 1.5,
            expected_economic_value: dec(value),
            tools_and_data: vec!["cpi-series".to_string()],
            budget: dec(50),
            owner: "deepbrain".to_string(),
            evaluation_suite: "macro-suite".to_string(),
            stop_conditions: vec!["budget spent".to_string()],
        },
        criteria: criteria(value, cost),
        origin: None,
        needs: vec![Need::Source("cpi-series".to_string())],
        jurisdiction: "US".to_string(),
    }
}

fn observation(trigger: GapTrigger, wanting: Vec<Wanting>) -> Observation {
    Observation {
        trigger,
        wanting,
        observation: format!("{trigger:?} on the rates book"),
        affected_domains: vec!["macro".to_string()],
        evidence: vec!["ev-1".to_string()],
        severity: 0.7,
        economic_value: dec(500),
    }
}

const WANTING: [Wanting; 10] = [
    Wanting::Source,
    Wanting::Type,
    Wanting::CausalLink,
    Wanting::Episodes,
    Wanting::Model,
    Wanting::Tool,
    Wanting::Specialist,
    Wanting::Execution,
    Wanting::Compute,
    Wanting::Unknown,
];

const NINE: [GapClass; 9] = [
    GapClass::Data,
    GapClass::Ontology,
    GapClass::Causal,
    GapClass::Memory,
    GapClass::Model,
    GapClass::Tool,
    GapClass::Specialist,
    GapClass::Execution,
    GapClass::ComputeQuantumResearch,
];

/// EXPAND-026. A classifier with a default would pass the first half of this
/// and file every unexplained symptom under whichever class came first.
#[test]
fn every_generated_gap_is_classified_into_one_of_the_nine_classes_and_an_unclassifiable_one_is_refused()
 {
    // Each lack has its own class: a swap of two arms is caught here.
    for (wanting, class) in WANTING.iter().zip(NINE) {
        assert_eq!(classify(&[*wanting]).unwrap(), class);
    }

    let mut rng = Lcg(7);
    let mut reached = BTreeSet::new();
    let mut refused = 0;
    for _ in 0..2_000 {
        let pieces = rng.next() % 4;
        let wanting: Vec<Wanting> = (0..pieces).map(|_| rng.pick(&WANTING)).collect();
        let agreed = wanting
            .first()
            .is_some_and(|first| *first != Wanting::Unknown && wanting.iter().all(|w| w == first));
        let raised = raise(observation(rng.pick(&GapTrigger::ALL), wanting.clone()));
        if agreed {
            let gap = raised.expect("evidence agreeing on one lack is classified");
            assert!(NINE.contains(&gap.signal.gap_class));
            reached.insert(format!("{:?}", gap.signal.gap_class));
        } else {
            let refusal = raised.expect_err(&format!("{wanting:?} was given a class"));
            assert_eq!(refusal.code(), "invalid");
            refused += 1;
        }
    }
    // The premise: the generator produced every class and plenty of signals
    // that name nothing, name `Unknown` or name two lacks.
    assert_eq!(reached.len(), 9, "only {reached:?} were generated");
    assert!(refused > 500, "only {refused} unclassifiable signals");
}

/// EXPAND-025, the engine's half: one detection is one signal, and the
/// signal says which of the seven triggers raised it.
#[test]
fn each_of_the_seven_triggers_raises_exactly_one_gap_signal_naming_its_trigger() {
    assert_eq!(GapTrigger::ALL.len(), 7);
    let raised: Vec<_> = GapTrigger::ALL
        .into_iter()
        .map(|trigger| raise(observation(trigger, vec![Wanting::Model])).unwrap())
        .collect();
    let named: BTreeSet<GapTrigger> = raised.iter().map(|gap| gap.trigger).collect();
    assert_eq!(raised.len(), 7);
    assert_eq!(named, BTreeSet::from(GapTrigger::ALL));
    for (gap, trigger) in raised.iter().zip(GapTrigger::ALL) {
        assert_eq!(gap.trigger, trigger);
        assert!(gap.signal.observation.contains(&format!("{trigger:?} on")));
    }
}

/// EXPAND-015. A criterion nobody measured, scored as zero, ranks a task as
/// though it had been measured and found worthless.
#[test]
fn a_scored_candidate_holds_all_eleven_inputs_and_one_missing_any_of_them_is_refused_by_name() {
    let score = criteria(900, 30)
        .score()
        .expect("premise: a candidate stating all eleven is scored");
    assert_eq!(score.expected_economic_value, dec(900));
    assert_eq!(score.compute_data_cost, dec(30));
    let held = [
        score.uncertainty_reduction,
        score.strategic_coverage,
        score.risk_reduction,
        score.information_gain,
        score.reuse_across_domains,
        score.feasibility,
        score.novelty,
        score.failure_frequency,
        score.urgency,
    ];
    let stated = [0.11, 0.12, 0.13, 1.5, 0.14, 0.9, 0.15, 0.16, 0.17];
    for (held, stated) in held.iter().zip(stated) {
        assert_eq!(held.to_bits(), f64::to_bits(stated));
    }

    type Clear = fn(&mut Criteria);
    let each: [(&str, Clear); 11] = [
        ("expected_economic_value", |c| {
            c.expected_economic_value = None;
        }),
        ("uncertainty_reduction", |c| c.uncertainty_reduction = None),
        ("strategic_coverage", |c| c.strategic_coverage = None),
        ("risk_reduction", |c| c.risk_reduction = None),
        ("information_gain", |c| c.information_gain = None),
        ("reuse_across_domains", |c| c.reuse_across_domains = None),
        ("compute_data_cost", |c| c.compute_data_cost = None),
        ("feasibility", |c| c.feasibility = None),
        ("novelty", |c| c.novelty = None),
        ("failure_frequency", |c| c.failure_frequency = None),
        ("urgency", |c| c.urgency = None),
    ];
    for (name, clear) in each {
        let mut missing = criteria(900, 30);
        clear(&mut missing);
        let refusal = missing
            .score()
            .expect_err(&format!("a candidate with no {name} was scored"));
        assert!(
            refusal.message().contains(&format!("states no {name};")),
            "the refusal for {name} names something else: {}",
            refusal.message()
        );
    }
}

/// EXPAND-042. Curiosity with no value attached is the failure: a queue led
/// by whatever is newest learns a great deal about nothing it can use.
#[test]
fn novelty_alone_never_outranks_a_candidate_with_economic_value_and_information_gain() {
    // Maximum novelty, zero wherever zero can be stated. Cost cannot be zero
    // and a feasibility of zero is refused outright, which is asserted last.
    let novel_only = Criteria {
        expected_economic_value: Some(Decimal::ZERO),
        uncertainty_reduction: Some(0.0),
        strategic_coverage: Some(0.0),
        risk_reduction: Some(0.0),
        information_gain: Some(0.0),
        reuse_across_domains: Some(0.0),
        compute_data_cost: Some(dec(1)),
        feasibility: Some(0.01),
        novelty: Some(1.0),
        failure_frequency: Some(0.0),
        urgency: Some(0.0),
    };
    let floor = novel_only.score().unwrap().priority();

    let mut rng = Lcg(42);
    let mut least = f64::MAX;
    for round in 0..2_000 {
        // Every fourth candidate has value and information gain and nothing
        // else: the weakest thing the requirement says must still rank above.
        let bare = round % 4 == 0;
        let term = |rng: &mut Lcg| if bare { 0.0 } else { rng.unit() };
        let valued = Criteria {
            expected_economic_value: Some(dec(1 + rng.whole(1_000))),
            uncertainty_reduction: Some(term(&mut rng)),
            strategic_coverage: Some(term(&mut rng)),
            risk_reduction: Some(term(&mut rng)),
            information_gain: Some(0.01 + rng.unit()),
            reuse_across_domains: Some(term(&mut rng)),
            compute_data_cost: Some(dec(1 + rng.whole(500))),
            feasibility: Some(if bare { 0.01 } else { 0.01 + 0.99 * rng.unit() }),
            novelty: Some(term(&mut rng)),
            failure_frequency: Some(term(&mut rng)),
            urgency: Some(term(&mut rng)),
        };
        let priority = valued.score().unwrap().priority();
        assert!(
            priority > floor,
            "{valued:?} ranks at {priority}, not above novelty alone at {floor}"
        );
        least = least.min(priority);
    }
    // The premise: the generator reached candidates weak enough that adding
    // novelty instead of multiplying by it would have put novelty first.
    assert!(least < 0.01, "the weakest valued candidate scored {least}");

    // Through the queue, with the tie-break set against the valued one.
    let world = World::new();
    let mut queue = ResearchQueue::new();
    let mut curiosity = candidate(TaskKind::Experiment, "a novel curiosity", 0, 1);
    curiosity.criteria = novel_only;
    queue.admit(curiosity, &world.bounds()).unwrap();
    queue
        .admit(
            candidate(TaskKind::Experiment, "z does cpi lead rates", 1, 500),
            &world.bounds(),
        )
        .unwrap();
    assert_eq!(
        queue.ranked()[0].item().research_question,
        "z does cpi lead rates"
    );

    // Equal novelty, different value: the rank is decided by the rest.
    let cheap = criteria(100, 30).score().unwrap().priority();
    let rich = criteria(900, 30).score().unwrap().priority();
    assert!(rich > cheap);

    let infeasible = Criteria {
        feasibility: Some(0.0),
        ..novel_only
    };
    assert_eq!(infeasible.score().unwrap_err().code(), "denied");
}

/// EXPAND-014. A queue that started items in arrival order would be a list,
/// and the ranking would decide nothing.
#[test]
fn candidates_of_all_five_kinds_are_ranked_and_the_scheduler_starts_the_highest_ranked_eligible_item_first()
 {
    let world = World::new();
    let mut queue = ResearchQueue::new();
    let values = [300, 900, 100, 700, 500];
    for (kind, value) in TaskKind::RESEARCH.into_iter().zip(values) {
        let state = queue
            .admit(
                candidate(kind, &format!("{kind:?} question"), value, 10),
                &world.bounds(),
            )
            .unwrap();
        assert_eq!(state, State::Queued);
    }

    let ranked: Vec<TaskKind> = queue.ranked().iter().map(|e| e.kind()).collect();
    assert_eq!(
        ranked,
        [
            TaskKind::Simulation,
            TaskKind::Experiment,
            TaskKind::Campaign,
            TaskKind::DataAcquisition,
            TaskKind::Label,
        ]
    );
    // The premise: rank order is not the order they arrived in.
    assert_ne!(ranked, TaskKind::RESEARCH);
    let priorities: Vec<f64> = queue
        .ranked()
        .iter()
        .map(|e| e.score().priority())
        .collect();
    assert!(priorities.windows(2).all(|pair| pair[0] > pair[1]));

    // A budget for exactly one task per round.
    let one = Budget {
        compute: dec(10),
        capital: dec(50),
    };
    let first = queue.schedule(one).unwrap();
    assert_eq!(first.started, ["Simulation question"]);
    assert_eq!(first.deferred.len(), 4);
    // The started item is no longer eligible, so the next round starts the
    // next in rank rather than the top of the list again.
    let second = queue.schedule(one).unwrap();
    assert_eq!(second.started, ["Experiment question"]);
    assert_eq!(
        second.deferred,
        [
            "Campaign question",
            "DataAcquisition question",
            "Label question"
        ]
    );
}

/// EXPAND-059. Research that starts and then finds the budget gone has
/// already spent what the budget existed to withhold.
#[test]
fn a_queue_costing_more_than_the_expansion_budget_starts_only_what_fits_and_defers_the_rest_by_name()
 {
    let world = World::new();
    let mut queue = ResearchQueue::new();
    for (question, value) in [("a", 400), ("b", 300), ("c", 200), ("d", 100)] {
        queue
            .admit(
                candidate(TaskKind::Experiment, question, value, 40),
                &world.bounds(),
            )
            .unwrap();
    }
    let budget = Budget {
        compute: dec(100),
        capital: dec(1_000),
    };
    // The premise: the queue costs more compute than the round has.
    assert!(dec(4 * 40) > budget.compute);

    let round = queue.schedule(budget).unwrap();
    assert_eq!(round.started, ["a", "b"]);
    assert_eq!(round.deferred, ["c", "d"]);
    for question in ["c", "d"] {
        let State::Deferred { reason } = queue.entry(question).unwrap().state() else {
            panic!("{question} was not recorded as deferred");
        };
        assert!(reason.contains("needs 40 of compute"), "{reason}");
    }

    // Capital is its own bound: compute to spare, and still not started.
    let thin = Budget {
        compute: dec(1_000),
        capital: dec(49),
    };
    let round = queue.schedule(thin).unwrap();
    assert!(round.started.is_empty());
    assert_eq!(round.deferred, ["c", "d"]);

    // A deferred task is asked again, and starts once the round covers it.
    let round = queue.schedule(budget).unwrap();
    assert_eq!(round.started, ["c", "d"]);

    let negative = Budget {
        compute: dec(-1),
        capital: dec(0),
    };
    assert!(queue.schedule(negative).is_err());
}

/// EXPAND-008. A task that fails because a parser does not exist, and says
/// only that it failed, is rediscovered by the next task that needs one.
#[test]
fn a_task_needing_an_unregistered_parser_is_blocked_on_it_and_a_tool_proposal_and_build_item_are_queued()
 {
    let world = World::new();
    // The premise: there is a registry, and the parser is not in it.
    assert!(world.tools.get("twin").is_some());
    assert!(world.tools.get("filing-parser").is_none());

    let needs_parser = |question: &str| {
        let mut task = candidate(TaskKind::DataAcquisition, question, 600, 20);
        task.item.tools_and_data.push("filing-parser".to_string());
        task.needs.push(Need::Tool {
            name: "filing-parser".to_string(),
            kind: ToolKind::Parser,
            permission: ToolPermission::Read,
        });
        task
    };
    let mut queue = ResearchQueue::new();
    let state = queue
        .admit(needs_parser("read the filings"), &world.bounds())
        .unwrap();
    let blocked = State::Blocked {
        tools: vec!["filing-parser".to_string()],
    };
    assert_eq!(state, blocked);
    assert_eq!(queue.entry("read the filings").unwrap().state(), &blocked);

    // The proposal is a spec and nothing more: read-only, and not registered.
    let [proposal] = queue.proposals() else {
        panic!("expected one proposal, found {:?}", queue.proposals());
    };
    assert_eq!(proposal.name(), "filing-parser");
    assert_eq!(proposal.kind(), ToolKind::Parser);
    assert_eq!(proposal.scope(), &BTreeSet::from([ToolPermission::Read]));
    assert!(world.tools.get("filing-parser").is_none());

    let builds: Vec<_> = queue
        .ranked()
        .into_iter()
        .filter(|e| e.kind() == TaskKind::ToolBuild)
        .collect();
    let [build] = builds.as_slice() else {
        panic!("expected one build item, found {}", builds.len());
    };
    assert_eq!(build.item().tools_and_data, ["filing-parser"]);
    assert_eq!(build.state(), &State::Queued);
    let build_question = build.item().research_question.clone();

    // The build can start; the task waiting on it cannot.
    let round = queue
        .schedule(Budget {
            compute: dec(1_000),
            capital: dec(1_000),
        })
        .unwrap();
    assert_eq!(round.started, [build_question]);
    assert_eq!(queue.entry("read the filings").unwrap().state(), &blocked);

    // A second task blocked on the same parser is recorded, not re-proposed.
    queue
        .admit(needs_parser("read the annexes"), &world.bounds())
        .unwrap();
    assert_eq!(queue.proposals().len(), 1);
    assert_eq!(queue.entry("read the annexes").unwrap().state(), &blocked);
}

/// EXPAND-060, the engine's half. Ranking an unlawful task last still leaves
/// it in the queue for the round with budget to spare.
#[test]
fn a_task_outside_licence_security_policy_or_jurisdiction_is_refused_before_anything_is_queued() {
    let mut world = World::new();
    world
        .research
        .record(
            "does the moon lead rates",
            Verdict::Null {
                evidence: "exp-7".to_string(),
            },
        )
        .unwrap();
    // The premise: the same bounds admit a task that stays inside them.
    let mut control = ResearchQueue::new();
    control
        .admit(
            candidate(TaskKind::Experiment, "control", 100, 10),
            &world.bounds(),
        )
        .unwrap();

    let mut unlicensed = candidate(TaskKind::DataAcquisition, "unlicensed", 100, 10);
    unlicensed.item.tools_and_data = vec!["scraped-feed".to_string()];
    unlicensed.needs = vec![Need::Source("scraped-feed".to_string())];

    let mut outside_policy = candidate(TaskKind::Simulation, "outside policy", 100, 10);
    outside_policy.item.tools_and_data = vec!["twin".to_string()];
    outside_policy.needs = vec![Need::Tool {
        name: "twin".to_string(),
        kind: ToolKind::Simulator,
        permission: ToolPermission::LeaveSandbox,
    }];

    let mut abroad = candidate(TaskKind::Campaign, "abroad", 100, 10);
    abroad.jurisdiction = "XX".to_string();

    let repeat = candidate(TaskKind::Experiment, "Does the Moon lead rates", 100, 10);

    let mut undeclared = candidate(TaskKind::Experiment, "undeclared", 100, 10);
    undeclared
        .item
        .tools_and_data
        .push("scraped-feed".to_string());

    let mut queue = ResearchQueue::new();
    for (task, says) in [
        (unlicensed, "no evaluated licence"),
        (outside_policy, "holds no LeaveSandbox permission"),
        (abroad, "jurisdiction `XX`"),
        (repeat, "found null (exp-7)"),
        (undeclared, "whether it is a source or a tool"),
    ] {
        let question = task.item.research_question.clone();
        let refusal = queue
            .admit(task, &world.bounds())
            .expect_err(&format!("`{question}` was admitted"));
        assert!(
            refusal.message().contains(says),
            "`{question}` was refused for another reason: {}",
            refusal.message()
        );
    }
    assert!(queue.ranked().is_empty());
    let round = queue
        .schedule(Budget {
            compute: dec(1_000),
            capital: dec(1_000),
        })
        .unwrap();
    assert!(round.started.is_empty() && round.deferred.is_empty());
}
