//! Properties the strategy compiler and runtime have to hold.

use qip_contracts::{
    Conviction, FeatureKey, FeatureValue, FeatureVector, Revision, SignalKind, StrategyId,
};
use qip_core::error::Result;
use qip_core::testing::approx_eq;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompilerLimits, StrategyCompiler, WarningKind};
use qip_strategy::ir::{ArithmeticOp, CompareOp, Expr, LogicalOp, Rule, StrategySpec, Type};
use qip_strategy::model::{DistilledModel, TreeNode};
use qip_strategy::program::{Node, NodeRef, Op, Program};
use qip_strategy::runtime::StrategyRuntime;

// --- fixtures ---------------------------------------------------------------

fn subject() -> ObjectId {
    ObjectId::from_string("OBJ00000000000000000000AAA")
}

fn mid() -> FeatureKey {
    FeatureKey::new("mid", subject())
}

fn spread() -> FeatureKey {
    FeatureKey::new("spread", subject())
}

fn pressure() -> FeatureKey {
    FeatureKey::new("book_pressure", subject()).with("levels", 5)
}

fn volatility() -> FeatureKey {
    FeatureKey::new("realised_volatility", subject()).with("window", 20)
}

fn trade_gap() -> FeatureKey {
    FeatureKey::new("time_since_last_trade", subject())
}

fn catalogue() -> FeatureCatalogue {
    let mut catalogue = FeatureCatalogue::new();
    for (key, value_type) in [
        (mid(), Type::Exact),
        (spread(), Type::Exact),
        (pressure(), Type::Statistic),
        (volatility(), Type::Statistic),
        (trade_gap(), Type::Count),
    ] {
        catalogue.declare(key, value_type).unwrap();
    }
    catalogue
}

fn compiler() -> StrategyCompiler {
    StrategyCompiler::new(catalogue())
}

fn spec_with(name: &str, condition: Expr) -> StrategySpec {
    StrategySpec::new(StrategyId::new(name), subject(), Duration::from_millis(250)).with_rule(
        Rule::new(
            "enter",
            SignalKind::Enter,
            condition,
            Expr::Exact(Decimal::from_int(100)),
            Expr::Statistic(0.6),
            400,
        ),
    )
}

fn vector_of(entries: &[(FeatureKey, FeatureValue)], as_of: Timestamp) -> FeatureVector {
    let mut vector = FeatureVector::new(as_of);
    for (index, (key, value)) in entries.iter().enumerate() {
        vector.insert(key.clone(), *value, Revision::new(index as u64 + 7));
    }
    vector
}

/// A leaning condition two strategies can plausibly both want.
fn leaning() -> Expr {
    Expr::feature(pressure()).greater_than(Expr::Statistic(0.4))
}

// --- static type checking ---------------------------------------------------

#[test]
fn comparing_an_exact_quantity_to_a_statistic_is_refused_and_both_types_are_named() {
    let refused = compiler()
        .compile(&spec_with(
            "mixed",
            Expr::feature(mid()).greater_than(Expr::feature(volatility())),
        ))
        .unwrap_err();
    let message = refused.to_string();
    assert!(message.contains("exact"), "{message}");
    assert!(message.contains("statistic"), "{message}");
    assert!(message.contains("compare"), "{message}");
}

#[test]
fn mixing_an_exact_quantity_and_a_statistic_in_arithmetic_is_refused() {
    let refused = compiler()
        .compile(&spec_with(
            "mixed-arithmetic",
            Expr::feature(mid())
                .plus(Expr::feature(volatility()))
                .greater_than(Expr::feature(mid())),
        ))
        .unwrap_err();
    assert!(refused.to_string().contains("exact"), "{refused}");
    assert!(refused.to_string().contains("statistic"), "{refused}");
}

#[test]
fn crossing_between_exact_and_statistical_values_is_possible_but_must_be_written_out() {
    // The same comparison, with the conversion made explicit, compiles.
    let compiled = compiler()
        .compile(&spec_with(
            "widened",
            Expr::feature(mid())
                .widened()
                .greater_than(Expr::feature(volatility())),
        ))
        .unwrap();
    assert!(compiled.warnings().is_empty());

    // And the ratio of two exact quantities is a statistic, so it may be
    // compared against one.
    let compiled = compiler()
        .compile(&spec_with(
            "ratio",
            Expr::feature(spread())
                .over(Expr::feature(mid()))
                .greater_than(Expr::Statistic(0.0005)),
        ))
        .unwrap();
    assert!(compiled.cost() > 0);
}

#[test]
fn a_rule_whose_parts_have_the_wrong_types_is_refused() {
    let base = StrategySpec::new(StrategyId::new("wrong-parts"), subject(), Duration::ZERO);

    let bad_condition = base.clone().with_rule(Rule::new(
        "enter",
        SignalKind::Enter,
        Expr::feature(mid()),
        Expr::Exact(Decimal::ONE),
        Expr::Statistic(0.5),
        10,
    ));
    let refused = compiler().compile(&bad_condition).unwrap_err();
    assert!(refused.to_string().contains("flag"), "{refused}");

    let bad_size = base.clone().with_rule(Rule::new(
        "enter",
        SignalKind::Enter,
        leaning(),
        // A size is a quantity. A statistic is not a quantity, and letting it
        // stand in for one is how a rounding error reaches an order.
        Expr::Statistic(100.0),
        Expr::Statistic(0.5),
        10,
    ));
    let refused = compiler().compile(&bad_size).unwrap_err();
    assert!(refused.to_string().contains("exact"), "{refused}");

    let bad_conviction = base.with_rule(Rule::new(
        "enter",
        SignalKind::Enter,
        leaning(),
        Expr::Exact(Decimal::ONE),
        Expr::Exact(Decimal::ONE),
        10,
    ));
    let refused = compiler().compile(&bad_conviction).unwrap_err();
    assert!(refused.to_string().contains("statistic"), "{refused}");
}

#[test]
fn a_strategy_naming_a_feature_the_graph_does_not_have_is_refused() {
    let absent = FeatureKey::new("invented_by_the_author", subject()).with("window", 3);
    let refused = compiler()
        .compile(&spec_with(
            "undeclared",
            Expr::feature(absent.clone()).greater_than(Expr::Statistic(0.0)),
        ))
        .unwrap_err();
    assert_eq!(refused.code(), "not_found", "{refused}");
    assert!(
        refused.to_string().contains(&absent.canonical()),
        "{refused}"
    );
}

#[test]
fn a_strategy_with_no_rules_is_refused() {
    let empty = StrategySpec::new(StrategyId::new("silent"), subject(), Duration::ZERO);
    assert!(compiler().compile(&empty).is_err());
}

// --- bounded execution ------------------------------------------------------

/// A balanced tree of additions, `2^depth` leaves wide.
fn wide_tree(depth: usize) -> Expr {
    if depth == 0 {
        return Expr::feature(volatility());
    }
    wide_tree(depth - 1).plus(wide_tree(depth - 1))
}

/// A chain of additions, `length` deep.
fn deep_chain(length: usize) -> Expr {
    let mut expr = Expr::feature(volatility());
    for _ in 0..length {
        expr = expr.plus(Expr::Statistic(1.0));
    }
    expr
}

/// A balanced tree of additions, `2^depth` leaves wide, whose leaves are all
/// distinct feature reads so nothing about it folds or dedupes at compile
/// time.
///
/// Unlike [`wide_tree`] — every leaf of which is the same feature read, so
/// the compiler interns the whole tree down to two nodes — and unlike a tree
/// of literal leaves, which the constant folder collapses to a single node
/// however wide it is written, this one really does cost what its node count
/// says: `2 * 2^depth - 1` reachable nodes, at a call-stack depth of only
/// `depth`. `next` must start at `0` and the same sequence of keys must be
/// declared into the catalogue the tree is compiled against.
fn distinct_wide_tree(depth: usize, subject: &ObjectId, next: &mut u64) -> Expr {
    if depth == 0 {
        let key = FeatureKey::new("leaf", subject.clone()).with("index", *next);
        *next += 1;
        return Expr::feature(key);
    }
    let left = distinct_wide_tree(depth - 1, subject, next);
    let right = distinct_wide_tree(depth - 1, subject, next);
    left.plus(right)
}

/// Every leaf key [`distinct_wide_tree`] reads for a given `depth`, in the
/// same order it reads them — so a caller can declare them all before
/// compiling.
fn distinct_wide_tree_leaves(depth: usize, subject: &ObjectId) -> Vec<FeatureKey> {
    (0..(1u64 << depth))
        .map(|index| FeatureKey::new("leaf", subject.clone()).with("index", index))
        .collect()
}

#[test]
fn a_strategy_whose_worst_case_cost_exceeds_the_budget_is_refused() {
    let refused = compiler()
        .compile(&spec_with(
            "sprawling",
            wide_tree(12).greater_than(Expr::Statistic(0.0)),
        ))
        .unwrap_err();
    assert_eq!(refused.code(), "guard", "{refused}");
    assert!(refused.to_string().contains("budget"), "{refused}");
}

#[test]
fn a_strategy_that_nests_deeper_than_the_compiler_will_walk_is_refused() {
    let limits = CompilerLimits {
        max_nodes: 4096,
        max_depth: 16,
    };
    let mut compiler = StrategyCompiler::with_limits(catalogue(), limits);
    let refused = compiler
        .compile(&spec_with(
            "deep",
            deep_chain(64).greater_than(Expr::Statistic(0.0)),
        ))
        .unwrap_err();
    assert_eq!(refused.code(), "guard", "{refused}");
    assert!(refused.to_string().contains("deeper"), "{refused}");
}

#[test]
fn a_refused_strategy_leaves_the_shared_program_exactly_as_it_was() {
    let mut compiler = compiler();
    compiler.compile(&spec_with("good", leaning())).unwrap();
    let before = compiler.report();

    compiler
        .compile(&spec_with(
            "bad",
            leaning().and(Expr::feature(mid()).greater_than(Expr::feature(volatility()))),
        ))
        .unwrap_err();

    let after = compiler.report();
    assert_eq!(
        before.unique_nodes, after.unique_nodes,
        "a refusal must not leave half a strategy in the shared program"
    );
    assert_eq!(before.submitted_nodes, after.submitted_nodes);
    assert_eq!(before.strategies, after.strategies);
}

#[test]
fn a_compiled_program_whose_nodes_do_not_point_strictly_backwards_is_refused() {
    // The one shape that could evaluate forever: a node that reads itself.
    let self_reading = Program::from_nodes(vec![Node {
        op: Op::Negate(NodeRef::new(0)),
        value_type: Type::Statistic,
    }]);
    let refused = self_reading.unwrap_err();
    assert_eq!(refused.code(), "guard", "{refused}");
    assert!(refused.to_string().contains("bounded"), "{refused}");

    // And a forward reference, which is the same problem one step removed.
    let forward = Program::from_nodes(vec![
        Node {
            op: Op::Negate(NodeRef::new(1)),
            value_type: Type::Statistic,
        },
        Node {
            op: Op::Literal(FeatureValue::Statistic(1.0)),
            value_type: Type::Statistic,
        },
    ]);
    assert!(forward.is_err());

    // A runtime will not accept one either.
    let refused = StrategyRuntime::new(Program::from_nodes(vec![]).unwrap());
    assert!(refused.is_ok(), "an empty program is bounded, if useless");
}

// --- common subexpression elimination ---------------------------------------

#[test]
fn two_strategies_sharing_a_subexpression_compile_to_one_shared_node() {
    let mut compiler = compiler();
    let first = compiler
        .compile(&spec_with(
            "lean-and-calm",
            leaning().and(Expr::feature(volatility()).less_than(Expr::Statistic(0.5))),
        ))
        .unwrap();
    let after_first = compiler.report().unique_nodes;

    let second = compiler
        .compile(&spec_with(
            "lean-and-active",
            leaning().and(Expr::feature(trade_gap()).less_than(Expr::Count(1_000_000))),
        ))
        .unwrap();
    let after_second = compiler.report().unique_nodes;

    let shared: Vec<NodeRef> = first
        .plan()
        .iter()
        .copied()
        .filter(|node| second.plan().contains(node))
        .collect();
    assert!(
        !shared.is_empty(),
        "two strategies asking the same question must share the node that answers it"
    );

    let program = compiler.program();
    assert!(
        shared
            .iter()
            .any(|node| matches!(program.node(*node).map(|n| &n.op), Some(Op::Compare { .. }))),
        "the shared node must be the comparison itself, not just its literals"
    );
    assert!(
        after_second - after_first < second.cost(),
        "the second strategy must cost fewer new nodes than it evaluates: \
         {} new for {} evaluated",
        after_second - after_first,
        second.cost()
    );
}

#[test]
fn deduplication_is_measured_and_grows_with_the_number_of_related_strategies() {
    let mut compiler = compiler();
    // Twenty strategies that all lean on the book, each with its own second
    // condition — the realistic shape of a strategy family.
    for index in 0..20 {
        let spec = spec_with(
            &format!("family-{index}"),
            leaning()
                .and(
                    Expr::feature(volatility())
                        .less_than(Expr::Statistic(0.2 + f64::from(index) * 0.01)),
                )
                .and(Expr::feature(spread()).at_most(Expr::feature(mid()))),
        );
        compiler.compile(&spec).unwrap();
    }
    let report = compiler.report();
    println!(
        "deduplication: {} submitted, {} unique, ratio {:.3}",
        report.submitted_nodes,
        report.unique_nodes,
        report.deduplication_ratio()
    );
    assert_eq!(report.strategies, 20);
    assert!(
        report.deduplication_ratio() > 0.5,
        "a family of related strategies must share most of its nodes: {:.3}",
        report.deduplication_ratio()
    );
}

#[test]
fn an_expression_with_no_features_in_it_is_folded_at_compile_time() {
    let mut compiler = compiler();
    let compiled = compiler
        .compile(&spec_with(
            "folded",
            Expr::Statistic(2.0)
                .times(Expr::Statistic(3.0))
                .greater_than(Expr::Statistic(5.0))
                .and(leaning()),
        ))
        .unwrap();
    // The constant half collapses to one literal, so the strategy evaluates
    // far fewer nodes than were written.
    assert!(compiled.cost() < 8, "cost {}", compiled.cost());
    assert!(
        compiled
            .warnings()
            .iter()
            .any(|warning| warning.kind == WarningKind::AlwaysTrue)
            || compiled.inputs().len() == 1
    );
}

// --- the shared plan: one node per distinct computation ---------------------
//
// The compiler interns children before parents, keyed on structure, so two
// syntactically identical subtrees — in one strategy or across many — are one
// node in the program. The tests below pin what that has to mean: the node
// count drops by exactly the duplicated nodes and no more, evaluation through
// the shared plan is indistinguishable from evaluating the written tree, the
// numbering a compile assigns is the same every time, and sharing never
// rescues a strategy from the cost ceiling.

/// The spread as a fraction of the mid — the kind of ratio several clauses of
/// one strategy plausibly all want.
fn relative_spread() -> Expr {
    Expr::feature(spread()).over(Expr::feature(mid()))
}

/// How many subtrees of `haystack` are structurally equal to `needle`.
fn occurrences(haystack: &Expr, needle: &Expr) -> usize {
    let here = usize::from(haystack == needle);
    here + haystack
        .children()
        .iter()
        .map(|child| occurrences(child, needle))
        .sum::<usize>()
}

/// Every operator in `expr`, counted as written — the node count a program
/// would hold if nothing were shared.
fn syntactic_size(expr: &Expr) -> usize {
    1 + expr
        .children()
        .iter()
        .map(|child| syntactic_size(child))
        .sum::<usize>()
}

/// The nodes a whole strategy would hold if nothing were shared.
fn syntactic_size_of(spec: &StrategySpec) -> usize {
    spec.rules
        .iter()
        .map(|rule| {
            syntactic_size(&rule.condition)
                + syntactic_size(&rule.size)
                + syntactic_size(&rule.conviction)
        })
        .sum()
}

/// Two rules, each comparing the same relative spread against its own
/// threshold.
fn two_clauses_over_the_relative_spread() -> StrategySpec {
    StrategySpec::new(
        StrategyId::new("two-clauses"),
        subject(),
        Duration::from_millis(250),
    )
    .with_rule(Rule::new(
        "tight",
        SignalKind::Enter,
        relative_spread().less_than(Expr::Statistic(0.0005)),
        Expr::Exact(Decimal::from_int(100)),
        Expr::Statistic(0.6),
        400,
    ))
    .with_rule(Rule::new(
        "wide",
        SignalKind::Exit,
        relative_spread().greater_than(Expr::Statistic(0.002)),
        Expr::Exact(Decimal::from_int(50)),
        Expr::Statistic(0.6),
        200,
    ))
}

#[test]
fn two_clauses_sharing_a_ratio_compile_to_one_ratio_node_rather_than_two() {
    let spec = two_clauses_over_the_relative_spread();

    // Premise: the two clauses carry the same subtree as two separate
    // allocations. A deduplication keyed on identity rather than structure
    // would see two different ratios here — that is the failure this test
    // exists to catch, so the premise has to hold before the claim means
    // anything.
    let (
        Expr::Compare {
            left: first_ratio, ..
        },
        Expr::Compare {
            left: second_ratio, ..
        },
    ) = (&spec.rules[0].condition, &spec.rules[1].condition)
    else {
        panic!("fixture: both conditions compare a ratio against a threshold");
    };
    assert_eq!(
        first_ratio, second_ratio,
        "the fixture must write the same ratio in both clauses"
    );
    assert!(
        !std::ptr::eq(first_ratio.as_ref(), second_ratio.as_ref()),
        "the two ratios must be distinct allocations, or identity would pass for structure"
    );
    let written = syntactic_size_of(&spec);
    assert_eq!(written, 14, "two clauses of seven nodes each, as written");

    let mut compiler = compiler();
    let compiled = compiler.compile(&spec).unwrap();
    let report = compiler.report();
    assert_eq!(report.submitted_nodes, written);

    // The ratio subtree — two feature reads and the ratio — and the conviction
    // literal are each written twice and exist once: four nodes saved, and
    // not one more, because the thresholds and the sizes differ.
    assert_eq!(
        report.unique_nodes,
        written - 4,
        "N shared nodes, not 2N: {} unique for {written} written",
        report.unique_nodes
    );
    assert_eq!(compiled.cost(), report.unique_nodes);

    let program = compiler.program();
    let ratios: Vec<NodeRef> = compiled
        .plan()
        .iter()
        .copied()
        .filter(|node| matches!(program.node(*node).map(|n| &n.op), Some(Op::Ratio { .. })))
        .collect();
    assert_eq!(ratios.len(), 1, "exactly one ratio node: {ratios:?}");
    for rule in compiled.rules() {
        let Some(Op::Compare { left, .. }) = program.node(rule.condition()).map(|n| &n.op) else {
            panic!("rule '{}' must compile to a comparison", rule.name());
        };
        assert_eq!(
            *left,
            ratios[0],
            "rule '{}' must read the one shared ratio",
            rule.name()
        );
    }
}

#[test]
fn node_numbering_is_stable_across_independent_compiles() {
    let specs = [
        two_clauses_over_the_relative_spread(),
        spec_with(
            "lean-and-calm",
            leaning().and(Expr::feature(volatility()).less_than(Expr::Statistic(0.5))),
        ),
        spec_with(
            "lean-and-tight",
            leaning().and(relative_spread().less_than(Expr::Statistic(0.0005))),
        ),
    ];
    let compile_all = || {
        let mut compiler = compiler();
        let compiled: Vec<_> = specs
            .iter()
            .map(|spec| compiler.compile(spec).unwrap())
            .collect();
        (compiled, compiler.into_program())
    };

    let (first, first_program) = compile_all();
    let (second, second_program) = compile_all();

    // Premise: sharing happened, so which index a node received is a real
    // question rather than a count.
    let written: usize = specs.iter().map(syntactic_size_of).sum();
    assert!(
        first_program.len() < written,
        "{} nodes for {written} written — nothing was shared",
        first_program.len()
    );

    // A replay that renumbers is not a replay: a stored plan names indices,
    // and a program recompiled from the same source must put the same
    // computation at each of them.
    assert_eq!(first_program, second_program);
    assert_eq!(first, second);
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.plan(), b.plan(), "strategy {} was renumbered", a.id());
    }
}

/// A strategy whose three expressions all read the relative spread — two
/// conditions and one conviction — so the shared plan computes it once where
/// the written tree computes it three times.
fn relative_spread_family() -> StrategySpec {
    StrategySpec::new(
        StrategyId::new("relative-spread"),
        subject(),
        Duration::from_millis(250),
    )
    .with_rule(Rule::new(
        "tight",
        SignalKind::Enter,
        relative_spread()
            .less_than(Expr::Statistic(0.0005))
            .and(leaning()),
        Expr::Exact(Decimal::from_int(100)),
        Expr::Statistic(0.6),
        400,
    ))
    .with_rule(Rule::new(
        "wide",
        SignalKind::Exit,
        relative_spread()
            .greater_than(Expr::Statistic(0.002))
            .or(Expr::feature(volatility()).greater_than(Expr::Statistic(0.5))),
        Expr::Exact(Decimal::from_int(50)),
        Expr::Statistic(0.5).plus(relative_spread()),
        200,
    ))
}

/// The written tree, evaluated directly and without sharing.
///
/// Deliberately a second implementation over the `Expr` tree rather than a
/// call into the crate: it is the reference the compiled plan is checked
/// against, so it must not be built from the thing under test. It covers only
/// the operators the fixture uses and panics on anything else, which keeps it
/// small enough to be read as a specification.
fn reference(expr: &Expr, vector: &FeatureVector) -> FeatureValue {
    let statistics =
        |left: &Expr, right: &Expr| match (reference(left, vector), reference(right, vector)) {
            (FeatureValue::Statistic(a), FeatureValue::Statistic(b)) => (a, b),
            other => panic!("reference: expected two statistics, found {other:?}"),
        };
    match expr {
        Expr::Exact(value) => FeatureValue::Exact(*value),
        Expr::Statistic(value) => FeatureValue::Statistic(*value),
        Expr::Feature(key) => vector
            .get(key)
            .expect("the fixture vector carries every feature"),
        Expr::Ratio {
            numerator,
            denominator,
        } => match (reference(numerator, vector), reference(denominator, vector)) {
            (FeatureValue::Exact(top), FeatureValue::Exact(bottom)) => {
                FeatureValue::Statistic(top.to_f64() / bottom.to_f64())
            }
            other => panic!("reference: a ratio of {other:?}"),
        },
        Expr::Compare { op, left, right } => {
            let (a, b) = statistics(left, right);
            FeatureValue::Flag(match op {
                CompareOp::Less => a < b,
                CompareOp::Greater => a > b,
                other => panic!("reference: comparison {} is not covered", other.as_str()),
            })
        }
        Expr::Logical { op, left, right } => {
            match (reference(left, vector), reference(right, vector)) {
                (FeatureValue::Flag(a), FeatureValue::Flag(b)) => FeatureValue::Flag(match op {
                    LogicalOp::And => a && b,
                    LogicalOp::Or => a || b,
                }),
                other => panic!("reference: a logical over {other:?}"),
            }
        }
        Expr::Arithmetic {
            op: ArithmeticOp::Add,
            left,
            right,
        } => {
            let (a, b) = statistics(left, right);
            FeatureValue::Statistic(a + b)
        }
        other => panic!("reference: {} is not covered", other.label()),
    }
}

/// The first rule whose condition holds under the reference, as what the
/// runtime would emit for it.
fn reference_signal(
    spec: &StrategySpec,
    vector: &FeatureVector,
) -> Option<(SignalKind, Decimal, Conviction)> {
    spec.rules
        .iter()
        .find_map(|rule| match reference(&rule.condition, vector) {
            FeatureValue::Flag(false) => None,
            FeatureValue::Flag(true) => {
                let FeatureValue::Exact(quantity) = reference(&rule.size, vector) else {
                    panic!("reference: a size that is not exact");
                };
                let FeatureValue::Statistic(probability) = reference(&rule.conviction, vector)
                else {
                    panic!("reference: a conviction that is not a statistic");
                };
                Some((
                    rule.kind,
                    quantity,
                    Conviction::new(probability, rule.observations),
                ))
            }
            other => panic!("reference: a condition that evaluated to {other:?}"),
        })
}

/// One market state the fixture strategy reads.
fn market(
    mid_price: &str,
    spread_width: &str,
    lean: f64,
    vol: f64,
    at: Timestamp,
) -> FeatureVector {
    vector_of(
        &[
            (
                mid(),
                FeatureValue::Exact(Decimal::parse(mid_price).expect("fixture decimal")),
            ),
            (
                spread(),
                FeatureValue::Exact(Decimal::parse(spread_width).expect("fixture decimal")),
            ),
            (pressure(), FeatureValue::Statistic(lean)),
            (volatility(), FeatureValue::Statistic(vol)),
        ],
        at,
    )
}

#[test]
fn a_shared_plan_evaluates_exactly_as_the_unshared_expression_tree_does() {
    let spec = relative_spread_family();

    // Premise: the fixture has something to share. Three occurrences of the
    // ratio, across two conditions and a conviction, so the shared plan
    // computes once what the written tree computes three times.
    let duplicates: usize = spec
        .rules
        .iter()
        .flat_map(|rule| [&rule.condition, &rule.size, &rule.conviction])
        .map(|expr| occurrences(expr, &relative_spread()))
        .sum();
    assert!(
        duplicates >= 2,
        "the fixture must write the ratio at least twice, wrote it {duplicates} times"
    );

    let mut compiler = compiler();
    let compiled = compiler.compile(&spec).unwrap();
    let report = compiler.report();
    assert!(
        report.unique_nodes < report.submitted_nodes,
        "the compiler must actually have shared something: {} unique for {} submitted",
        report.unique_nodes,
        report.submitted_nodes
    );
    let mut runtime = StrategyRuntime::new(compiler.into_program()).unwrap();

    let at = Timestamp::from_secs(1_700_000_000);
    let fixtures = [
        // A tight spread and a leaning book: the first rule fires.
        market("100", "0.01", 0.7, 0.1, at),
        // A wide spread alone: the second rule fires on the ratio.
        market("100", "0.5", 0.2, 0.1, at),
        // Tight but not leaning, and calm: nothing fires.
        market("250.5", "0.1", 0.2, 0.1, at),
        // Tight but not leaning, and volatile: the second rule fires on
        // volatility, with a conviction that reads the ratio.
        market("250.5", "0.1", 0.2, 0.9, at),
        // Neither tight nor wide, volatile: the second rule, via volatility.
        market("100", "0.1", 0.9, 0.9, at),
        // Both rules would fire; order decides, and the first wins.
        market("100", "0.01", 0.7, 0.9, at),
    ];

    // Premise: the fixture set reaches every outcome — each rule, and none —
    // or agreement on it proves less than it appears to.
    let expected: Vec<_> = fixtures
        .iter()
        .map(|vector| reference_signal(&spec, vector))
        .collect();
    for wanted in [Some(SignalKind::Enter), Some(SignalKind::Exit), None] {
        assert!(
            expected
                .iter()
                .any(|outcome| outcome.as_ref().map(|(kind, _, _)| *kind) == wanted),
            "the fixture set never reaches {wanted:?}"
        );
    }

    for (vector, wanted) in fixtures.iter().zip(&expected) {
        let got = runtime
            .run(&compiled, vector, at)
            .unwrap()
            .map(|signal| (signal.kind, signal.desired_quantity, signal.conviction));
        assert_eq!(
            &got, wanted,
            "the shared plan and the written tree disagree on {vector:?}"
        );
    }
}

#[test]
fn the_size_refusal_still_fires_on_a_program_that_is_oversized_after_sharing() {
    // Depth 9 gives 512 leaves and 1023 nodes, all distinct feature reads.
    // Written twice, the strategy is 2,051 nodes as written and 1,028 once
    // shared — either side of nothing, and both above the default ceiling.
    let depth = 9;
    let mut catalogue = catalogue();
    for key in distinct_wide_tree_leaves(depth, &subject()) {
        catalogue.declare(key, Type::Statistic).unwrap();
    }
    let mut next = 0u64;
    let tree = distinct_wide_tree(depth, &subject(), &mut next);
    let doubled = tree
        .clone()
        .greater_than(Expr::Statistic(0.0))
        .and(tree.clone().less_than(Expr::Statistic(1.0)));

    // Premise: the tree is written twice, so sharing has something to do.
    assert_eq!(occurrences(&doubled, &tree), 2);

    // Premise: it does not fit even once shared. A generous compiler admits
    // it and states the shared cost, which must be below what was written and
    // above what the default ceiling allows — otherwise a refusal below would
    // be the syntactic measure firing, and say nothing about sharing.
    let generous = CompilerLimits {
        max_nodes: 4096,
        max_depth: 32,
    };
    let mut wide_open = StrategyCompiler::with_limits(catalogue.clone(), generous);
    let shared = wide_open
        .compile(&spec_with("doubled", doubled.clone()))
        .unwrap();
    let written = syntactic_size(&doubled);
    assert!(
        shared.cost() < written,
        "sharing must have happened: {} for {written} written",
        shared.cost()
    );
    assert!(
        shared.cost() > CompilerLimits::default().max_nodes,
        "the fixture must exceed the default ceiling once shared: {} of {}",
        shared.cost(),
        CompilerLimits::default().max_nodes
    );

    // Sharing is not a way past the budget.
    let refused = StrategyCompiler::new(catalogue)
        .compile(&spec_with("doubled", doubled))
        .unwrap_err();
    assert_eq!(refused.code(), "guard", "{refused}");
    assert!(refused.to_string().contains("budget"), "{refused}");
}

// --- warnings ---------------------------------------------------------------

#[test]
fn a_condition_that_cannot_vary_is_reported_along_with_the_rules_it_shadows() {
    let spec = StrategySpec::new(StrategyId::new("shadowed"), subject(), Duration::ZERO)
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(Decimal::ONE),
            Expr::Statistic(0.5),
            10,
        ))
        .with_rule(Rule::new(
            "never-reached",
            SignalKind::Exit,
            leaning(),
            Expr::Exact(Decimal::ONE),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler().compile(&spec).unwrap();
    let kinds: Vec<WarningKind> = compiled.warnings().iter().map(|w| w.kind).collect();
    assert!(kinds.contains(&WarningKind::AlwaysTrue), "{kinds:?}");
    assert!(kinds.contains(&WarningKind::Unreachable), "{kinds:?}");
}

#[test]
fn a_condition_that_can_never_hold_is_reported() {
    let compiled = compiler()
        .compile(&spec_with(
            "dead",
            Expr::Statistic(1.0).greater_than(Expr::Statistic(2.0)),
        ))
        .unwrap();
    assert!(
        compiled
            .warnings()
            .iter()
            .any(|warning| warning.kind == WarningKind::AlwaysFalse),
        "{:?}",
        compiled.warnings()
    );
}

#[test]
fn two_rules_asking_the_same_question_leave_the_later_one_unreachable() {
    let spec = StrategySpec::new(StrategyId::new("duplicated"), subject(), Duration::ZERO)
        .with_rule(Rule::new(
            "first",
            SignalKind::Enter,
            leaning(),
            Expr::Exact(Decimal::ONE),
            Expr::Statistic(0.5),
            10,
        ))
        .with_rule(Rule::new(
            "second",
            SignalKind::Hedge,
            leaning(),
            Expr::Exact(Decimal::ONE),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler().compile(&spec).unwrap();
    let unreachable = compiled
        .warnings()
        .iter()
        .find(|warning| warning.kind == WarningKind::Unreachable)
        .expect("the shadowed rule must be reported");
    assert!(unreachable.detail.contains("first"), "{unreachable:?}");
}

#[test]
fn a_branch_chosen_at_compile_time_is_reported_as_dead() {
    let compiled = compiler()
        .compile(&spec_with(
            "constant-branch",
            Expr::select(
                Expr::Flag(true),
                Expr::feature(volatility()),
                Expr::Statistic(99.0),
            )
            .greater_than(Expr::Statistic(0.1)),
        ))
        .unwrap();
    assert!(
        compiled
            .warnings()
            .iter()
            .any(|warning| warning.kind == WarningKind::DeadBranch),
        "{:?}",
        compiled.warnings()
    );
}

#[test]
fn exact_equality_between_two_statistics_is_warned_about() {
    let compiled = compiler()
        .compile(&spec_with(
            "equality",
            Expr::feature(volatility()).equals(Expr::Statistic(0.2)),
        ))
        .unwrap();
    assert!(
        compiled
            .warnings()
            .iter()
            .any(|warning| warning.kind == WarningKind::ExactEqualityOnStatistic),
        "{:?}",
        compiled.warnings()
    );
}

// --- the runtime ------------------------------------------------------------

fn built(condition: Expr) -> Result<(StrategyRuntime, qip_strategy::CompiledStrategy)> {
    let mut compiler = compiler();
    let compiled = compiler.compile(&spec_with("runtime", condition))?;
    let runtime = StrategyRuntime::new(compiler.into_program())?;
    Ok((runtime, compiled))
}

#[test]
fn a_signal_is_never_emitted_from_an_undefined_input() {
    let (mut runtime, compiled) = built(leaning()).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);

    let missing = vector_of(&[(pressure(), FeatureValue::Undefined)], at);
    assert_eq!(
        runtime.run(&compiled, &missing, at).unwrap(),
        None,
        "an undefined input must stop the signal, not default it"
    );

    let present = vector_of(&[(pressure(), FeatureValue::Statistic(0.7))], at);
    assert!(
        runtime.run(&compiled, &present, at).unwrap().is_some(),
        "the same strategy must fire once the input exists, or the test above proves nothing"
    );
}

#[test]
fn a_feature_the_vector_does_not_carry_at_all_is_an_error_rather_than_a_silence() {
    let (mut runtime, compiled) = built(leaning()).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    let empty = FeatureVector::new(at);
    let refused = runtime.run(&compiled, &empty, at).unwrap_err();
    assert_eq!(refused.code(), "not_found", "{refused}");
}

#[test]
fn a_signal_records_the_revisions_of_the_features_it_came_from() {
    let (mut runtime, compiled) =
        built(leaning().and(Expr::feature(volatility()).less_than(Expr::Statistic(0.9)))).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    let vector = vector_of(
        &[
            (pressure(), FeatureValue::Statistic(0.7)),
            (volatility(), FeatureValue::Statistic(0.2)),
        ],
        at,
    );

    let signal = runtime.run(&compiled, &vector, at).unwrap().unwrap();
    assert_eq!(signal.inputs.len(), 2);
    for key in [pressure(), volatility()] {
        let recorded = signal
            .inputs
            .iter()
            .find(|(name, _)| *name == key.canonical())
            .expect("every input must be attributed");
        assert_eq!(
            Revision::new(recorded.1),
            vector.revision_of(&key).unwrap(),
            "the recorded revision must be the one the signal was computed from"
        );
    }
    assert_eq!(signal.at, at);
    assert_eq!(signal.valid_until, at.saturating_add(compiled.validity()));
    assert_eq!(signal.kind, SignalKind::Enter);
    assert_eq!(signal.desired_quantity, Decimal::from_int(100));
}

#[test]
fn evaluation_is_deterministic() {
    let (mut runtime, compiled) = built(leaning()).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    let vector = vector_of(&[(pressure(), FeatureValue::Statistic(0.55))], at);

    let first = runtime.run(&compiled, &vector, at).unwrap();
    let second = runtime.run(&compiled, &vector, at).unwrap();
    let third = runtime.run(&compiled, &vector, at).unwrap();
    assert_eq!(first, second);
    assert_eq!(second, third);
}

#[test]
fn the_runtime_budget_is_enforced() {
    let mut compiler = compiler();
    let compiled = compiler
        .compile(&spec_with(
            "budgeted",
            leaning().and(Expr::feature(volatility()).less_than(Expr::Statistic(0.5))),
        ))
        .unwrap();
    let program = compiler.into_program();

    let mut generous = StrategyRuntime::new(program.clone()).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    let vector = vector_of(
        &[
            (pressure(), FeatureValue::Statistic(0.7)),
            (volatility(), FeatureValue::Statistic(0.2)),
        ],
        at,
    );
    assert!(generous.run(&compiled, &vector, at).unwrap().is_some());

    let mut mean = StrategyRuntime::with_budget(program, 2).unwrap();
    let refused = mean.run(&compiled, &vector, at).unwrap_err();
    assert_eq!(refused.code(), "guard", "{refused}");
    assert!(refused.to_string().contains("budget"), "{refused}");
}

/// A budget drawn from the program's own size is not a bound: a strategy's
/// reachable cost is always a subset sum of the nodes the arena holds, so it
/// can never exceed a ceiling computed from that same arena. Compile with
/// limits wide enough to admit a strategy above the compiler's *default*
/// ceiling, then hand the resulting program — and nothing else — to
/// `StrategyRuntime::new`. If the runtime's default budget were derived from
/// this program (its total cost, say) the oversized strategy would sail
/// through, because its cost and the ceiling would be the same number.
#[test]
fn the_default_runtime_budget_does_not_grow_with_the_program_it_is_given() {
    // Depth 10 gives 1024 leaves and 2047 reachable nodes — comfortably above
    // the compiler's default ceiling of 512 — at a call-stack depth of 10.
    let depth = 10;
    let generous_limits = CompilerLimits {
        max_nodes: 4096,
        max_depth: 32,
    };
    let mut catalogue = catalogue();
    for key in distinct_wide_tree_leaves(depth, &subject()) {
        catalogue.declare(key, Type::Statistic).unwrap();
    }
    let mut compiler = StrategyCompiler::with_limits(catalogue, generous_limits);
    let mut next = 0u64;
    let tree = distinct_wide_tree(depth, &subject(), &mut next);
    let compiled = compiler
        .compile(&spec_with(
            "oversized",
            tree.greater_than(Expr::Statistic(0.0)),
        ))
        .unwrap();
    // Assert the premise: this fixture must actually exceed the default
    // ceiling, or a refusal below would prove nothing about the fix.
    assert!(
        compiled.cost() > CompilerLimits::default().max_nodes,
        "fixture cost {} does not exceed the default ceiling of {}",
        compiled.cost(),
        CompilerLimits::default().max_nodes,
    );
    let program = compiler.into_program();

    // `StrategyRuntime::new` takes no budget from the caller, so this is the
    // exact scenario the default has to protect against: a program built
    // with limits the compiler's own default would have refused.
    let mut runtime = StrategyRuntime::new(program).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    let empty = FeatureVector::new(at);
    let refused = runtime.run(&compiled, &empty, at).unwrap_err();
    assert_eq!(refused.code(), "guard", "{refused}");
    assert!(refused.to_string().contains("budget"), "{refused}");
}

#[test]
fn the_first_rule_whose_condition_holds_is_the_one_that_fires() {
    let spec = StrategySpec::new(StrategyId::new("ordered"), subject(), Duration::ZERO)
        .with_rule(Rule::new(
            "exit-first",
            SignalKind::Exit,
            Expr::feature(volatility()).greater_than(Expr::Statistic(0.5)),
            Expr::Exact(Decimal::from_int(5)),
            Expr::Statistic(0.9),
            100,
        ))
        .with_rule(Rule::new(
            "enter-second",
            SignalKind::Enter,
            leaning(),
            Expr::Exact(Decimal::from_int(50)),
            Expr::Statistic(0.6),
            100,
        ));
    let mut compiler = compiler();
    let compiled = compiler.compile(&spec).unwrap();
    let mut runtime = StrategyRuntime::new(compiler.into_program()).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);

    // Both conditions hold; rule order decides, and an exit in front of an
    // entry is the whole reason order is part of the strategy.
    let both = vector_of(
        &[
            (pressure(), FeatureValue::Statistic(0.9)),
            (volatility(), FeatureValue::Statistic(0.8)),
        ],
        at,
    );
    assert_eq!(
        runtime.run(&compiled, &both, at).unwrap().unwrap().kind,
        SignalKind::Exit
    );

    let calm = vector_of(
        &[
            (pressure(), FeatureValue::Statistic(0.9)),
            (volatility(), FeatureValue::Statistic(0.1)),
        ],
        at,
    );
    assert_eq!(
        runtime.run(&compiled, &calm, at).unwrap().unwrap().kind,
        SignalKind::Enter
    );

    let quiet = vector_of(
        &[
            (pressure(), FeatureValue::Statistic(0.0)),
            (volatility(), FeatureValue::Statistic(0.1)),
        ],
        at,
    );
    assert_eq!(runtime.run(&compiled, &quiet, at).unwrap(), None);
}

#[test]
fn a_conviction_is_shrunk_by_how_little_evidence_supports_it() {
    // Not a property of this crate, but the runtime has to hand the sample
    // size through for it to hold at all.
    let (mut runtime, compiled) = built(leaning()).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    let vector = vector_of(&[(pressure(), FeatureValue::Statistic(0.7))], at);
    let signal = runtime.run(&compiled, &vector, at).unwrap().unwrap();
    assert_eq!(signal.conviction, Conviction::new(0.6, 400));
    assert!(signal.conviction.shrunk() > 0.55);
}

// --- distilled models -------------------------------------------------------

#[test]
fn a_distilled_model_carries_its_own_coefficients_and_evaluates_in_bounded_steps() {
    let model = DistilledModel::linear("flow-tilt", 0.1, vec![0.5, -0.25]).unwrap();
    assert_eq!(model.arity(), 2);
    assert_eq!(model.cost(), 3);

    let mut compiler = compiler();
    let compiled = compiler
        .compile(&spec_with(
            "distilled",
            Expr::Model {
                model: model.clone(),
                inputs: vec![Expr::feature(pressure()), Expr::feature(volatility())],
            }
            .greater_than(Expr::Statistic(0.0)),
        ))
        .unwrap();
    // The model's size is charged against the budget, not treated as one node.
    assert!(compiled.cost() >= model.cost());

    let mut runtime = StrategyRuntime::new(compiler.into_program()).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    let vector = vector_of(
        &[
            (pressure(), FeatureValue::Statistic(1.0)),
            (volatility(), FeatureValue::Statistic(0.4)),
        ],
        at,
    );
    // 0.1 + 0.5 * 1.0 - 0.25 * 0.4 = 0.5
    assert!(runtime.run(&compiled, &vector, at).unwrap().is_some());
}

#[test]
fn a_decision_tree_that_could_descend_backwards_is_refused() {
    let looping = DistilledModel::tree(
        "loop",
        1,
        vec![TreeNode::Branch {
            input: 0,
            threshold: 0.0,
            below: 0,
            at_or_above: 0,
        }],
    );
    let refused = looping.unwrap_err();
    assert!(refused.to_string().contains("bounded"), "{refused}");

    let sound = DistilledModel::tree(
        "sound",
        1,
        vec![
            TreeNode::Branch {
                input: 0,
                threshold: 0.5,
                below: 1,
                at_or_above: 2,
            },
            TreeNode::Leaf { value: -1.0 },
            TreeNode::Leaf { value: 1.0 },
        ],
    )
    .unwrap();
    assert!(approx_eq(sound.evaluate(&[0.1]).unwrap(), -1.0, 1e-12));
    assert!(approx_eq(sound.evaluate(&[0.9]).unwrap(), 1.0, 1e-12));
    assert!(sound.evaluate(&[0.1, 0.2]).is_err(), "arity is checked");
    assert!(
        sound.evaluate(&[f64::NAN]).is_err(),
        "a NaN input is refused"
    );
}

#[test]
fn a_model_given_an_input_that_is_not_a_statistic_is_refused_at_compile_time() {
    let model = DistilledModel::linear("one-input", 0.0, vec![1.0]).unwrap();
    let refused = compiler()
        .compile(&spec_with(
            "unwidened",
            Expr::Model {
                model,
                inputs: vec![Expr::feature(mid())],
            }
            .greater_than(Expr::Statistic(0.0)),
        ))
        .unwrap_err();
    assert!(refused.to_string().contains("widen"), "{refused}");
}

// --- the structural guarantee ----------------------------------------------

/// Every operation the compiled form can perform.
///
/// Exhaustive on purpose: adding a variant to [`Op`] breaks this match, and
/// whoever adds it has to say here what it does. That is the whole enforcement
/// mechanism for "nothing on the hot path calls out", and it is a compile
/// error rather than a review convention.
fn what_it_does(op: &Op) -> &'static str {
    match op {
        Op::Literal(_) => "reads a value fixed at compile time",
        Op::Feature(_) => "reads a value from the feature vector",
        Op::Negate(_) | Op::Magnitude(_) => "arithmetic on one operand",
        Op::Invert(_) => "boolean negation",
        Op::Widen(_) | Op::Ratio { .. } => "converts between exact and statistical values",
        Op::Arithmetic { .. } => "arithmetic on two operands",
        Op::Compare { .. } => "comparison",
        Op::Logical { .. } => "boolean combination",
        Op::Select { .. } | Op::Bounded { .. } | Op::Extremum { .. } => "chooses between values",
        Op::Model { .. } => "evaluates coefficients it carries itself",
    }
}

#[test]
fn the_compiled_form_has_no_reachable_path_to_a_language_model() {
    // 1. The crate cannot reach one: it does not depend on `qip-ai`, which is
    //    where every language model in the platform lives.
    let manifest = include_str!("../Cargo.toml");
    assert!(
        !manifest.contains("qip-ai"),
        "the strategy crate must not be able to reach a language model at all"
    );

    // 2. Neither the IR nor the compiled form names anything that could.
    for source in [
        include_str!("../src/ir.rs"),
        include_str!("../src/program.rs"),
        include_str!("../src/runtime.rs"),
    ] {
        for forbidden in [
            "LanguageModel",
            "qip_ai",
            "ModelRequest",
            "Completion",
            "std::net",
            "std::fs",
            "std::process",
        ] {
            assert!(
                !source.contains(forbidden),
                "the compiled path must not name {forbidden}"
            );
        }
    }

    // 3. Every operation the runtime can perform is local arithmetic over
    //    values it was handed. This match is exhaustive, so a new variant
    //    cannot be added without appearing here.
    let model = DistilledModel::linear("bounded", 0.0, vec![1.0]).unwrap();
    for op in [
        Op::Literal(FeatureValue::Flag(true)),
        Op::Feature(mid()),
        Op::Negate(NodeRef::new(0)),
        Op::Magnitude(NodeRef::new(0)),
        Op::Invert(NodeRef::new(0)),
        Op::Widen(NodeRef::new(0)),
        Op::Ratio {
            numerator: NodeRef::new(0),
            denominator: NodeRef::new(0),
        },
        Op::Model {
            model,
            inputs: vec![NodeRef::new(0)],
        },
    ] {
        assert!(!what_it_does(&op).is_empty());
    }
}

#[test]
fn every_expression_in_the_language_is_arithmetic_over_values_already_in_hand() {
    // The same guarantee at the source level: `Expr` is matched exhaustively,
    // so a variant that took a service, a handle or an address could not be
    // added without this test being rewritten deliberately.
    fn classify(expr: &Expr) -> &'static str {
        match expr {
            Expr::Exact(_) | Expr::Statistic(_) | Expr::Count(_) | Expr::Flag(_) => "literal",
            Expr::Feature(_) => "feature read",
            Expr::Negate(_) | Expr::Magnitude(_) | Expr::Invert(_) | Expr::Widen(_) => "unary",
            Expr::Ratio { .. } | Expr::Arithmetic { .. } => "arithmetic",
            Expr::Compare { .. } | Expr::Logical { .. } => "predicate",
            Expr::Select { .. } | Expr::Bounded { .. } | Expr::Extremum { .. } => "choice",
            Expr::Model { .. } => "distilled model",
        }
    }
    assert_eq!(classify(&Expr::Flag(true)), "literal");
    assert_eq!(classify(&Expr::feature(mid())), "feature read");
    assert_eq!(classify(&leaning()), "predicate");
}

#[test]
fn a_compiled_strategy_survives_a_round_trip_through_serialisation() {
    let mut compiler = compiler();
    let compiled = compiler.compile(&spec_with("stored", leaning())).unwrap();
    let program = compiler.into_program();

    let encoded = serde_json::to_string(&(&compiled, &program)).unwrap();
    let (restored, restored_program): (qip_strategy::CompiledStrategy, Program) =
        serde_json::from_str(&encoded).unwrap();
    assert_eq!(compiled, restored);
    assert_eq!(program, restored_program);

    // And the restored program is checked again rather than trusted.
    let mut runtime = StrategyRuntime::new(restored_program).unwrap();
    let at = Timestamp::from_secs(1_700_000_000);
    let vector = vector_of(&[(pressure(), FeatureValue::Statistic(0.7))], at);
    assert!(runtime.run(&restored, &vector, at).unwrap().is_some());
}
