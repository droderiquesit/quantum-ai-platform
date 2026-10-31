//! The compiled form's opportunity digest (ADR 0149 decision 7).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::{SignalKind, StrategyId};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};

fn mid() -> qip_contracts::FeatureKey {
    qip_contracts::FeatureKey::new("mid", ObjectId::from_string("obj"))
}

/// One declared feature, so a predicate reads something the compiler cannot
/// fold to a literal.
fn catalogue() -> qip_strategy::catalogue::FeatureCatalogue {
    let mut catalogue = qip_strategy::catalogue::FeatureCatalogue::new();
    catalogue
        .declare(mid(), Type::Exact)
        .expect("one declaration");
    catalogue
}

/// ADR 0149 decision 7: an opportunity's definition digest is the compiled
/// predicate's structure, not its place in the arena. The publisher and the
/// node compile into different arenas, so a digest by position would make
/// every definition read as unnamed. Mutation: digest the node's index (or
/// stop blanking child positions) and the two compilations below differ.
#[test]
fn an_entry_rule_digests_alike_wherever_the_arena_put_its_predicate() -> Result<()> {
    use qip_strategy::compile::StrategyCompiler;
    let rule = |name: &str, threshold: &str| {
        Rule::new(
            name,
            SignalKind::Enter,
            Expr::Compare {
                op: qip_strategy::ir::CompareOp::Greater,
                left: Box::new(Expr::Feature(mid())),
                right: Box::new(Expr::Exact(Decimal::parse(threshold).expect("a literal"))),
            },
            Expr::Exact(Decimal::ONE),
            Expr::Statistic(0.5),
            10,
        )
    };
    let target = StrategySpec::new(
        StrategyId::new("target"),
        ObjectId::from_string("obj"),
        Duration::from_secs(30),
    )
    .with_rule(rule("go", "3"));
    let filler = StrategySpec::new(
        StrategyId::new("filler"),
        ObjectId::from_string("obj"),
        Duration::from_secs(30),
    )
    .with_rule(rule("first", "7"));

    let mut alone = StrategyCompiler::new(catalogue());
    let compiled_alone = alone.compile(&target)?;
    let digest_alone = compiled_alone.opportunities(&alone.into_program())?;

    let mut shared = StrategyCompiler::new(catalogue());
    shared.compile(&filler)?;
    let compiled_after = shared.compile(&target)?;
    let program = shared.into_program();
    // Premise: the predicate sits at a different index in the two arenas.
    assert_ne!(
        compiled_alone.rules()[0].condition(),
        compiled_after.rules()[0].condition(),
        "premise: the arenas placed the predicate alike"
    );
    assert_eq!(digest_alone, compiled_after.opportunities(&program)?);
    // And a different predicate digests differently.
    let other = StrategySpec::new(
        StrategyId::new("target"),
        ObjectId::from_string("obj"),
        Duration::from_secs(30),
    )
    .with_rule(rule("go", "4"));
    let mut third = StrategyCompiler::new(catalogue());
    let compiled_other = third.compile(&other)?;
    assert_ne!(
        digest_alone,
        compiled_other.opportunities(&third.into_program())?
    );
    Ok(())
}
