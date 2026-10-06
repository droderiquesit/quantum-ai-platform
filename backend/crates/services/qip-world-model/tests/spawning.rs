use qip_core::{Decimal, Duration, Timestamp};
use qip_world_model::WorldModelSpawner;
use qip_world_model::federation::{
    Abstraction, CalibrationClaim, Declaration, Federation, ModelKind, Scope, WorldModelSpec,
};

#[test]
fn a_world_model_branch_is_proposed_and_registered_when_residual_exceeds_insufficiency() {
    let threshold = Decimal::from_scaled(5, 1).unwrap(); // 0.5
    let spawner = WorldModelSpawner::new(threshold).expect("valid spawner");

    let now = Timestamp::from_millis(1_000_000_000);

    // Create parent spec and register it with the federation
    let parent_spec = WorldModelSpec::declare(Declaration {
        id: "parent-model".to_string(),
        scope: Some(Scope {
            domain: "equities".to_string(),
            region: "global".to_string(),
            horizon: Duration::from_days(1),
            abstraction: Abstraction::Meso,
            hypothesis: "baseline".to_string(),
            kind: ModelKind::Observational,
        }),
        evidence_lineage: vec!["source-1".to_string()],
        calibration: Some(CalibrationClaim { max_brier: 0.25 }),
        update_cadence: Some(Duration::from_days(1)),
        expires_at: Some(now.saturating_add(Duration::from_days(30))),
    })
    .expect("valid spec");

    let mut federation = Federation::new();
    let now = Timestamp::from_millis(1_000_000_000);
    federation
        .register(parent_spec, now)
        .expect("parent registered");

    // Residual that exceeds threshold
    let residual = Decimal::from_scaled(6, 1).unwrap(); // 0.6 > 0.5

    // Propose the branch
    let branch = spawner
        .propose_branch(
            "parent-model",
            residual,
            "branch-001",
            "alternative_explanation_1",
            now,
        )
        .expect("branch proposed");

    assert_eq!(branch.parent_id, "parent-model");
    assert_eq!(branch.hypothesis, "alternative_explanation_1");
    assert_eq!(branch.trigger_residual, residual);

    // Register the branch into the federation
    spawner
        .register_branch(&mut federation, &branch)
        .expect("branch registered");

    // Verify the branch is in the federation with correct lineage
    let lineage = federation
        .lineage("branch-001")
        .expect("branch lineage exists");
    assert_eq!(lineage.parents.len(), 1);
    assert_eq!(lineage.parents[0], "parent-model");
    assert!(lineage.trigger.is_some());

    let trigger = lineage.trigger.as_ref().unwrap();
    assert!(trigger.contains("residual_insufficiency"));
    assert!(trigger.contains("0.6"));
}

#[test]
fn proposing_a_branch_requires_residual_to_exceed_the_insufficiency_threshold() {
    let threshold = Decimal::from_scaled(5, 1).unwrap(); // 0.5
    let spawner = WorldModelSpawner::new(threshold).expect("valid spawner");

    let now = Timestamp::from_millis(1_000_000_000);

    // Residual below threshold: rejected
    let below = spawner.propose_branch(
        "parent",
        Decimal::from_scaled(4, 1).unwrap(), // 0.4 < 0.5
        "branch-1",
        "hypothesis",
        now,
    );
    assert!(below.is_err());

    // Residual at threshold: rejected (exclusive comparison)
    let at_threshold = spawner.propose_branch(
        "parent",
        Decimal::from_scaled(5, 1).unwrap(), // 0.5 = 0.5
        "branch-2",
        "hypothesis",
        now,
    );
    assert!(at_threshold.is_err());

    // Residual above threshold: accepted
    let above = spawner.propose_branch(
        "parent",
        Decimal::from_scaled(6, 1).unwrap(), // 0.6 > 0.5
        "branch-3",
        "hypothesis",
        now,
    );
    assert!(above.is_ok());
}

#[test]
fn negative_residuals_also_trigger_branching_on_absolute_value() {
    let threshold = Decimal::from_scaled(5, 1).unwrap(); // 0.5
    let spawner = WorldModelSpawner::new(threshold).expect("valid spawner");

    let now = Timestamp::from_millis(1_000_000_000);

    // Negative residual exceeding threshold (in absolute value)
    let negative_large = spawner.propose_branch(
        "parent",
        Decimal::from_scaled(-6, 1).unwrap(), // |-0.6| > 0.5
        "branch-neg",
        "hypothesis",
        now,
    );
    assert!(negative_large.is_ok());

    let branch = negative_large.unwrap();
    assert_eq!(
        branch.trigger_residual,
        Decimal::from_scaled(-6, 1).unwrap()
    );
}
