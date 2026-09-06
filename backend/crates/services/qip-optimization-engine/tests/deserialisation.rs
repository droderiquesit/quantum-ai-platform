//! Every checked constructor in this crate, driven from a document.
//!
//! `#[derive(Deserialize)]` on a struct with private fields is a public
//! constructor that writes straight past whatever the named one refuses. This
//! platform has been burned by it three times — `AssetValuation`, `Judgement`
//! and `ValuationInput` — and the two modules under test here had no
//! `serde(try_from)` at all, so every guarantee in them was reachable only by
//! callers who chose to use the front door.
//!
//! Each test below carries the document that got past the door, and each
//! asserts the other half too: that a legitimate record still loads. A
//! validator that refuses everything is indistinguishable from one that works
//! until something real arrives, and the difference only shows up in
//! production.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::decimal::Decimal;
use qip_core::error::Result;
use qip_core::ids::StrategyId;
use qip_optimization_engine::families::{
    Diagnostics, FamilyAssignment, FamilyClustering, FamilyId, MIN_WINDOW_OBSERVATIONS,
    StrategyReturns, StressCorrelation, StressWindow,
};
use qip_optimization_engine::horizons::{
    CapitalPools, FamilyBudget, Horizon, HorizonReconciliation, ReconciledPlan, reconcile,
};

fn strategy(name: &str) -> StrategyId {
    StrategyId::from_string(name)
}

/// Pools that sum exactly, so a test about something else is not really a test
/// about the sum.
fn pools() -> Result<CapitalPools> {
    CapitalPools::new(
        Decimal::from_int(1_000),
        Decimal::from_int(400),
        Decimal::from_int(300),
        Decimal::from_int(200),
        Decimal::from_int(100),
        Decimal::ZERO,
    )
}

// --- the money gate ---------------------------------------------------------

#[test]
fn a_reconciled_plan_cannot_be_deserialised_past_the_gate_that_is_the_only_way_to_one() -> Result<()>
{
    // The document that produced `total=4 allocated=999999999
    // first_pos_committed=999999999`: a plan for four units of capital holding
    // a billion, with three of the four horizons simply not mentioned, so the
    // "no horizon is over its pool" test passed over the one horizon present
    // and vacuously over the rest.
    let forged = r#"{
      "positions": {
        "years": {
          "horizon": "years",
          "pool": "1",
          "committed": "999999999",
          "liability": "0",
          "families": [0]
        }
      },
      "allocations": {"0": "999999999"},
      "total": "4",
      "allocated": "999999999"
    }"#;
    let error = serde_json::from_str::<ReconciledPlan>(forged)
        .expect_err("a plan is what the gate yields, not what a document says it is");
    assert!(
        error.to_string().contains("no position for the "),
        "the refusal must name what is missing: {error}"
    );

    // The other half. A plan produced by the gate survives a round trip, so
    // this is a door and not a wall.
    let pools = pools()?;
    let budgets = vec![
        FamilyBudget::from_money(
            FamilyId::new(0),
            Horizon::MicrosecondsToMinutes,
            Decimal::from_int(350),
        )?,
        FamilyBudget::from_money(FamilyId::new(1), Horizon::Years, Decimal::from_int(90))?,
    ];
    let plan = reconcile(&pools, &budgets)?.into_plan()?;
    let document = serde_json::to_string(&plan).expect("a plan serialises");
    let restored: ReconciledPlan =
        serde_json::from_str(&document).expect("a plan the gate produced loads again");
    assert_eq!(restored, plan);
    assert_eq!(
        restored.allocation_for(FamilyId::new(0)),
        Some(Decimal::from_int(350))
    );
    Ok(())
}

#[test]
fn a_plan_whose_horizon_is_over_its_pool_is_refused_by_the_same_gate_on_the_way_in() -> Result<()> {
    // A serialised *reconciliation* that is genuinely over-committed at the
    // years horizon — every field internally consistent, so only `into_plan`
    // can be what refuses it. The document is produced by the platform rather
    // than written by hand, so it cannot drift from the shape the type emits.
    let pools = pools()?;
    let budgets = vec![FamilyBudget::from_money(
        FamilyId::new(0),
        Horizon::Years,
        Decimal::from_int(500),
    )?];
    let breached = reconcile(&pools, &budgets)?;
    assert!(
        !breached.is_balanced(),
        "the premise: 500 against a reserved pool of 100 is a breach"
    );
    let document = serde_json::to_string(&breached).expect("a reconciliation serialises");

    // As a reconciliation it loads: an operator has to be able to read a
    // breach in order to fix it.
    let restored: HorizonReconciliation =
        serde_json::from_str(&document).expect("a breached reconciliation is still a report");
    assert!(!restored.is_balanced());

    // As a plan it does not.
    let error = serde_json::from_str::<ReconciledPlan>(&document)
        .expect_err("an over-committed horizon yields no plan, whatever route it arrives by");
    assert!(
        error.to_string().contains("years is over its pool of 100"),
        "the refusal must be the gate's own, naming the horizon: {error}"
    );
    Ok(())
}

#[test]
fn a_reconciliation_that_does_not_re_derive_from_its_own_parts_is_refused() -> Result<()> {
    // Internally inconsistent rather than structurally wrong: the pools sum,
    // every horizon is present, the one budget is positive — and `allocated`
    // says something the budgets do not. Two claims about the same fact.
    let pools = pools()?;
    let budgets = vec![FamilyBudget::from_money(
        FamilyId::new(0),
        Horizon::HoursToDays,
        Decimal::from_int(250),
    )?];
    let honest =
        serde_json::to_string(&reconcile(&pools, &budgets)?).expect("a reconciliation serialises");
    assert!(
        honest.contains(r#""allocated":"250""#),
        "the premise: the honest document says 250 was allocated: {honest}"
    );
    let tampered = honest.replace(r#""allocated":"250""#, r#""allocated":"0""#);

    let error = serde_json::from_str::<HorizonReconciliation>(&tampered)
        .expect_err("a record that disagrees with its own budgets is not a reconciliation");
    assert!(
        error
            .to_string()
            .contains("does not re-derive from its own parts"),
        "the refusal must say the record was edited after it was produced: {error}"
    );
    // And the untampered one still loads.
    let restored: HorizonReconciliation =
        serde_json::from_str(&honest).expect("the honest document loads");
    assert_eq!(restored.allocated(), Decimal::from_int(250));
    Ok(())
}

// --- the four pools ---------------------------------------------------------

#[test]
fn capital_pools_that_do_not_sum_to_the_total_are_refused_when_they_arrive_as_a_document()
-> Result<()> {
    // The document that produced `total=1000 inv=1000 dep=1000 unres=1000
    // res=1000`, after which one unit was spent four times and the
    // reconciliation still called itself balanced.
    let forged = r#"{
      "total": "1000",
      "available_inventory": "1000",
      "deployable_capital": "1000",
      "capital_not_reserved_for_calls": "1000",
      "reserved_capital": "1000",
      "unfunded_commitments": "0"
    }"#;
    let error = serde_json::from_str::<CapitalPools>(forged)
        .expect_err("capital that belongs to no horizon is capital two horizons will both spend");
    assert!(
        error
            .to_string()
            .contains("sum to 4000 against a total of 1000"),
        "the refusal must name the sum it found: {error}"
    );

    // The other half: pools that do sum still load, and load as themselves.
    let honest = serde_json::to_string(&pools()?).expect("pools serialise");
    let restored: CapitalPools = serde_json::from_str(&honest).expect("summing pools load");
    assert_eq!(restored.total(), Decimal::from_int(1_000));
    assert_eq!(
        restored.pool_for(Horizon::MicrosecondsToMinutes),
        Decimal::from_int(400)
    );
    Ok(())
}

#[test]
fn a_negative_capital_pool_is_refused_when_it_arrives_as_a_document() {
    // Sums to the total, so only the sign check can be what refuses it.
    let forged = r#"{
      "total": "1000",
      "available_inventory": "-500",
      "deployable_capital": "500",
      "capital_not_reserved_for_calls": "500",
      "reserved_capital": "500",
      "unfunded_commitments": "0"
    }"#;
    let error = serde_json::from_str::<CapitalPools>(forged)
        .expect_err("a negative pool is a shortfall wearing a pool's clothes");
    assert!(
        error.to_string().contains("available_inventory is -500"),
        "the refusal must name the pool and the value: {error}"
    );
}

// --- one family's claim -----------------------------------------------------

#[test]
fn a_negative_family_budget_cannot_arrive_as_a_document_and_net_a_real_breach_away() -> Result<()> {
    // Assert the premise first: the breach this record would have netted away
    // is real, and the duplicate-budget guard cannot see the record, because
    // the two budgets belong to different families.
    let pools = pools()?;
    let real = FamilyBudget::from_money(FamilyId::new(0), Horizon::Years, Decimal::from_int(250))?;
    let alone = reconcile(&pools, std::slice::from_ref(&real))?;
    assert_eq!(
        alone.breaches().len(),
        1,
        "the premise: 250 against a reserved pool of 100 breaches the years horizon"
    );

    let error = serde_json::from_str::<FamilyBudget>(
        r#"{"family": 1, "horizon": "years", "money": "-250.0"}"#,
    )
    .expect_err("a negative budget is a short position, not a capital claim");
    assert!(
        error
            .to_string()
            .contains("is budgeted -250 at the years horizon"),
        "the refusal must name the family, the amount and the horizon: {error}"
    );

    // The other half: a positive budget loads, and the same second family at
    // the same horizon then *adds* to the breach rather than cancelling it.
    let second: FamilyBudget =
        serde_json::from_str(r#"{"family": 1, "horizon": "years", "money": "250.0"}"#)
            .expect("a positive budget is an ordinary claim");
    let both = reconcile(&pools, &[real, second])?;
    assert_eq!(
        both.position(Horizon::Years).map(|p| p.committed),
        Some(Decimal::from_int(500)),
        "two claims on one pool add; the only way they subtract is a negative one"
    );
    assert!(!both.is_balanced());
    Ok(())
}

// --- the stress window ------------------------------------------------------

#[test]
fn a_stress_window_from_a_document_cannot_name_an_observation_it_does_not_have() -> Result<()> {
    // The document that made `StressCorrelation::from_returns` panic with
    // "index out of bounds: the len is 13 but the index is 900" — inside a
    // function whose signature promises a refusal. `from_returns` checks that
    // every series is as long as the window says and then indexes with the
    // window's own indices, which is correct as long as the window is the only
    // thing that can produce those indices.
    let forged = r#"{"observations": 13, "stress": [900, 901, 902], "provenance": "forged"}"#;
    let error = serde_json::from_str::<StressWindow>(forged)
        .expect_err("an index outside the sample is not an observation");
    assert!(
        error
            .to_string()
            .contains("stress observation 900 is outside the 13 observations supplied"),
        "the refusal must name the index and the sample: {error}"
    );

    // The other half: a window that names observations it has loads, and the
    // series it describes can then be correlated without a panic.
    let honest = StressWindow::explicit(
        40,
        (0..16).collect(),
        "fixture: the first sixteen observations are the drawdown",
    )?;
    let document = serde_json::to_string(&honest).expect("a window serialises");
    let restored: StressWindow = serde_json::from_str(&document).expect("an honest window loads");
    assert_eq!(restored, honest);
    assert_eq!(restored.stress_indices().len(), 16);
    Ok(())
}

#[test]
fn a_stress_window_from_a_document_cannot_repeat_an_observation() {
    // Every index is in range, so only the duplicate check can refuse it. A
    // repeat weights one day twice and silently makes it the family boundary.
    let forged =
        r#"{"observations": 40, "stress": [0,1,2,3,4,5,6,7,8,9,10,11,3], "provenance": "forged"}"#;
    let error = serde_json::from_str::<StressWindow>(forged)
        .expect_err("a repeat weights one observation twice");
    assert!(
        error.to_string().contains("repeats an observation"),
        "the refusal must name the defect: {error}"
    );
}

// --- the clustering request -------------------------------------------------

#[test]
fn a_clustering_into_zero_families_cannot_arrive_as_a_document() -> Result<()> {
    // With the plain derive this loaded, and the merge loop then ran itself
    // down to no clusters and reported the caller's bad input as "this is a
    // bug in the clustering, not a bad input" — a refusal that sends the
    // reader to the wrong file.
    let error =
        serde_json::from_str::<FamilyClustering>(r#"{"target_families": 0, "linkage": "average"}"#)
            .expect_err("a clustering into zero families produces nothing to allocate across");
    assert!(
        error.to_string().contains("ask for at least one"),
        "the refusal must name the fix: {error}"
    );

    // The other half: a real request loads and keeps its linkage, which is the
    // field a `try_from` that rebuilt through `new` alone would drop.
    let restored: FamilyClustering =
        serde_json::from_str(r#"{"target_families": 3, "linkage": "complete"}"#)
            .expect("a clustering into three families is an ordinary request");
    assert_eq!(restored.target_families(), 3);
    assert_eq!(
        restored.linkage(),
        qip_optimization_engine::families::Linkage::Complete,
        "the linkage must survive the trip; a clustering silently switched to average linkage \
         would draw different family boundaries and say nothing"
    );
    Ok(())
}

#[test]
fn a_return_series_from_a_document_cannot_be_empty() {
    let empty = serde_json::from_str::<StrategyReturns>(r#"{"strategy": "s-a", "returns": []}"#)
        .expect_err("a strategy with no returns has nothing to be clustered on");
    assert!(
        empty.to_string().contains("has no returns"),
        "the refusal must name the defect: {empty}"
    );

    // The constructor's other refusal — a non-finite return — is not reachable
    // through JSON, which has no literal for one and whose parser rejects
    // `1e400` as out of range before this crate sees it. That guard is proved
    // against the constructor in `families_and_horizons.rs`; asserting it here
    // would be asserting serde_json's behaviour and calling it ours.
    let restored: StrategyReturns =
        serde_json::from_str(r#"{"strategy": "s-a", "returns": [0.01, -0.02, 0.03]}"#)
            .expect("an ordinary series loads");
    assert_eq!(restored.len(), 3);
    assert_eq!(restored.strategy(), &strategy("s-a"));
}

// --- the assignment and its diagnostics -------------------------------------

/// Diagnostics that describe a two-strategy, one-family population. Every
/// figure is one a measurement could have produced, so a test using this is
/// testing whatever it changes and nothing else.
fn coherent_diagnostics() -> String {
    format!(
        r#"{{
          "linkage": "average",
          "target_families": 1,
          "strategies": 2,
          "stress_observations": {MIN_WINDOW_OBSERVATIONS},
          "calm_observations": {MIN_WINDOW_OBSERVATIONS},
          "mean_stress_excess": 0.3,
          "mean_intra_family_correlation": 0.9,
          "mean_inter_family_correlation": 0.0,
          "pairs_calm_would_have_misfiled": 1,
          "pairs_total": 1
        }}"#
    )
}

#[test]
fn a_family_assignment_whose_two_indexes_disagree_is_refused() {
    // `families` says strategy-b is in family 0; `of_strategy` files it under
    // family 7. A caller reading `members(0)` and a journal written from
    // `family_of` would describe different portfolios. The diagnostics here
    // are coherent, so the membership check is what has to fire.
    let forged = format!(
        r#"{{
          "families": {{"0": ["strategy-a", "strategy-b"]}},
          "of_strategy": {{"strategy-a": 0, "strategy-b": 7}},
          "diagnostics": {}
        }}"#,
        coherent_diagnostics()
    );
    let error = serde_json::from_str::<FamilyAssignment>(&forged)
        .expect_err("the two indexes are one fact");
    assert!(
        error
            .to_string()
            .contains("the reverse index files it under family-007"),
        "the refusal must name both answers: {error}"
    );

    // The other half: the same document with the indexes agreeing loads.
    let honest = format!(
        r#"{{
          "families": {{"0": ["strategy-a", "strategy-b"]}},
          "of_strategy": {{"strategy-a": 0, "strategy-b": 0}},
          "diagnostics": {}
        }}"#,
        coherent_diagnostics()
    );
    let restored: FamilyAssignment =
        serde_json::from_str(&honest).expect("an agreeing assignment loads");
    assert_eq!(restored.family_count(), 1);
    assert_eq!(restored.strategy_count(), 2);
    assert_eq!(
        restored.family_of(&strategy("strategy-b")),
        Some(FamilyId::new(0))
    );
}

#[test]
fn a_family_assignment_holding_a_strategy_no_family_claims_is_refused() {
    // The reverse index holds three strategies and the families hold two. The
    // extra one has a family in every lookup a caller makes and is in no
    // family in every iteration it makes.
    //
    // The diagnostics here describe *three* strategies, deliberately: with the
    // two-strategy set they would disagree with the reverse index and that
    // check would refuse the document first, leaving the partition check
    // itself unproved. A test has to fail for the reason it names.
    let forged = r#"{
          "families": {"0": ["strategy-a", "strategy-b"]},
          "of_strategy": {"strategy-a": 0, "strategy-b": 0, "strategy-c": 0},
          "diagnostics": {
            "linkage": "average",
            "target_families": 1,
            "strategies": 3,
            "stress_observations": 12,
            "calm_observations": 12,
            "mean_stress_excess": 0.3,
            "mean_intra_family_correlation": 0.9,
            "mean_inter_family_correlation": 0.0,
            "pairs_calm_would_have_misfiled": 1,
            "pairs_total": 3
          }
        }"#;
    let error = serde_json::from_str::<FamilyAssignment>(forged)
        .expect_err("an assignment is a partition of the population");
    assert!(
        error
            .to_string()
            .contains("is not a partition of the population"),
        "the refusal must name what is broken: {error}"
    );
}

#[test]
fn a_family_assignment_numbered_other_than_canonically_is_refused() {
    // Family 4 rather than family 0. The numbering is what a decision record
    // keys on, and `family-004` would name a different set of strategies on
    // re-derivation, so a replay of the decision would not be a replay.
    let forged = format!(
        r#"{{
          "families": {{"4": ["strategy-a", "strategy-b"]}},
          "of_strategy": {{"strategy-a": 4, "strategy-b": 4}},
          "diagnostics": {}
        }}"#,
        coherent_diagnostics()
    );
    let error = serde_json::from_str::<FamilyAssignment>(&forged)
        .expect_err("families are numbered from zero by their smallest member");
    assert!(
        error.to_string().contains("family-004 sits at position 0"),
        "the refusal must name the family and where it sits: {error}"
    );
}

#[test]
fn diagnostics_from_a_document_cannot_report_more_misfiled_pairs_than_pairs() {
    // `pairs_calm_would_have_misfiled` is the cost of this design in pairs,
    // and it is the number the LEARN stage journals as its evidence. Two
    // strategies make one pair; a record claiming two disagreements out of one
    // is not a measurement of anything.
    let forged = coherent_diagnostics().replace(
        r#""pairs_calm_would_have_misfiled": 1"#,
        r#""pairs_calm_would_have_misfiled": 2"#,
    );
    let error = serde_json::from_str::<Diagnostics>(&forged)
        .expect_err("the calm view cannot disagree about more pairs than exist");
    assert!(
        error
            .to_string()
            .contains("cannot disagree about more pairs than exist"),
        "the refusal must name the impossibility: {error}"
    );

    let restored: Diagnostics =
        serde_json::from_str(&coherent_diagnostics()).expect("a coherent record loads");
    assert_eq!(restored.pairs_total, 1);
    assert_eq!(restored.pairs_calm_would_have_misfiled, 1);
}

#[test]
fn diagnostics_from_a_document_cannot_report_a_correlation_no_correlation_can_reach() {
    // A mean intra-family correlation of 5. Read off a dashboard it says the
    // families are five times as coherent as a perfect correlation, which is
    // not a strong result; it is a corrupt record.
    let forged = coherent_diagnostics().replace(
        r#""mean_intra_family_correlation": 0.9"#,
        r#""mean_intra_family_correlation": 5.0"#,
    );
    let error =
        serde_json::from_str::<Diagnostics>(&forged).expect_err("a correlation lives in [-1, 1]");
    assert!(
        error
            .to_string()
            .contains("mean_intra_family_correlation as 5"),
        "the refusal must name the field and the value: {error}"
    );
}

#[test]
fn an_assignment_a_clustering_produced_survives_a_round_trip() -> Result<()> {
    // The whole point of the checks above is that they admit what the stage
    // itself emits. If this fails, the crate can no longer read its own
    // output, which is a worse defect than the one being fixed.
    let observations = 40usize;
    let series: Vec<StrategyReturns> = ["s-a", "s-b", "s-c"]
        .iter()
        .enumerate()
        .map(|(i, name)| {
            #[allow(clippy::cast_precision_loss)]
            let returns: Vec<f64> = (0..observations)
                .map(|t| {
                    let base = ((t % 7) as f64 - 3.0) * 0.01;
                    let own = ((t % (3 + i)) as f64 - 1.0) * 0.004 * (i as f64 + 1.0);
                    if i == 2 { -base + own } else { base + own }
                })
                .collect();
            StrategyReturns::new(strategy(name), returns)
        })
        .collect::<Result<Vec<_>>>()?;
    let window = StressWindow::explicit(observations, (0..16).collect(), "fixture")?;
    let correlation = StressCorrelation::from_returns(&series, &window)?;
    let assignment = FamilyClustering::new(2)?.cluster(&correlation)?;

    // Premise: there is structure to lose.
    assert_eq!(assignment.family_count(), 2);
    assert_eq!(assignment.strategy_count(), 3);

    let document = serde_json::to_string(&assignment).expect("an assignment serialises");
    let restored: FamilyAssignment =
        serde_json::from_str(&document).expect("the stage can read its own output");
    assert_eq!(restored, assignment);
    Ok(())
}
