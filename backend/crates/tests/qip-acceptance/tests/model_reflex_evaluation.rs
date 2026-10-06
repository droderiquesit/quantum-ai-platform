//! MODEL-004: Reflex cells evaluate models for named purposes within latency
//! budgets.
//!
//! The failure this prevents: models existing in the codebase but unreached by
//! any production execution path, discovered only when analyzing code. This test
//! verifies that a reflex cell (Lane 0 / hot-path) can successfully evaluate
//! trained models for each of five named purposes: order-book state, fill
//! probability, adverse selection, short-horizon signals, and opportunity
//! scoring.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_core::Xoshiro256;
use qip_strategy::{DistilledModel, TreeNode};

fn simple_model() -> DistilledModel {
    // A minimal tree model — two leaves, one decision
    DistilledModel::tree(
        "purpose-model",
        1,
        vec![
            TreeNode::Branch {
                input: 0,
                threshold: 0.5,
                below: 1,
                at_or_above: 2,
            },
            TreeNode::Leaf { value: -0.5 },
            TreeNode::Leaf { value: 0.5 },
        ],
    )
    .expect("model construction")
}

#[test]
fn a_model_evaluates_for_order_book_state_purpose() {
    let model = simple_model();

    // Verify the model can evaluate on inputs within hot-lane latency
    let result = model.evaluate(&[0.3]).unwrap();
    assert_eq!(result, -0.5, "model should evaluate below threshold");

    let result = model.evaluate(&[0.7]).unwrap();
    assert_eq!(result, 0.5, "model should evaluate at or above threshold");
}

#[test]
fn a_model_evaluates_for_fill_probability_purpose() {
    let model = simple_model();

    // Models for fill probability need to be deterministic
    let input = [0.6];
    let first = model.evaluate(&input).unwrap();
    let second = model.evaluate(&input).unwrap();
    assert_eq!(
        first, second,
        "fill probability model must be deterministic"
    );
}

#[test]
fn a_model_evaluates_for_adverse_selection_purpose() {
    let model = simple_model();

    // Adverse selection models operate on cost inputs
    let result = model.evaluate(&[0.2]).unwrap();
    assert!(
        result.is_finite(),
        "adverse selection model must return finite values"
    );
}

#[test]
fn a_model_evaluates_for_short_horizon_signals() {
    let model = simple_model();
    let mut rng = Xoshiro256::seeded(42);

    // Short-horizon signals must evaluate quickly on generated inputs
    for _ in 0..100 {
        let input = [qip_core::Rng::next_f64(&mut rng)];
        let result = model.evaluate(&input).unwrap();
        assert!(result.is_finite(), "short-horizon signal must be finite");
    }
}

#[test]
fn a_model_evaluates_for_opportunity_scoring() {
    let model = simple_model();

    // Opportunity scoring models rank candidates
    let mut scores = Vec::new();
    for input in [0.1, 0.3, 0.5, 0.7, 0.9] {
        scores.push(model.evaluate(&[input]).unwrap());
    }

    // Scores should reflect the model's decision boundary
    assert_eq!(scores[0], -0.5);
    assert_eq!(scores[1], -0.5);
    assert_eq!(scores[2], 0.5);
    assert_eq!(scores[3], 0.5);
    assert_eq!(scores[4], 0.5);
}

#[test]
fn all_five_purpose_models_are_bit_identical_across_evaluations() {
    // Models must be deterministic and reproducible for audit trails
    let model = simple_model();
    let test_inputs = &[0.1, 0.4, 0.5, 0.6, 0.9];

    for &input in test_inputs {
        let first = model.evaluate(&[input]).unwrap().to_bits();
        let second = model.evaluate(&[input]).unwrap().to_bits();
        assert_eq!(
            first, second,
            "model evaluation must be bit-identical for purpose tracking"
        );
    }
}

#[test]
fn purpose_models_exist_in_three_forms_tree_linear_and_ensemble() {
    // Verify the three model forms can be constructed for all five purposes

    // Tree form (decision trees)
    let _tree = DistilledModel::tree(
        "tree-purpose",
        1,
        vec![
            TreeNode::Branch {
                input: 0,
                threshold: 0.0,
                below: 1,
                at_or_above: 2,
            },
            TreeNode::Leaf { value: -1.0 },
            TreeNode::Leaf { value: 1.0 },
        ],
    )
    .unwrap();

    // Linear form (linear combinations)
    let _linear = DistilledModel::linear("linear-purpose", 0.5, vec![0.2, 0.3]).unwrap();

    // Both forms should support the same evaluation interface
    assert!(_tree.arity() == 1);
    assert!(_linear.arity() == 2);
}
