//! A quantum answer never bypasses the deterministic pre-trade gate
//! (R12-QUANT-4064).
//!
//! The structural claim — the router returns weights, and an order reaches a
//! broker only through `OrderManager::submit`, which runs `PreTradeChecker`
//! first — was asserted by text searches over `platform.rs` that passed if
//! the file contained the word "baseline". Nothing drove a construction the
//! quantum arm actually *won* into the order path, so nothing would notice a
//! shortcut that let a winning solver's weights skip the checker. This suite
//! does that: a scripted provider beats the classical baseline, the router
//! adopts it, and the position it sizes breaches a limit and is refused
//! before the venue is asked.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::sync::Arc;

use qip_core::error::Result;
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::Timestamp;
use qip_core::{Decimal, dec};
use qip_execution_engine::broker::{Broker, SimulatedBroker, SimulationSettings};
use qip_execution_engine::oms::{OrderManager, RefusalReason};
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_numerics::anneal::Qubo;
use qip_optimization_engine::problem::{Objective, PortfolioProblem, QuboEncoding};
use qip_optimization_engine::router::{ComputeRouter, RoutingPolicy, Solver};
use qip_quantum::provider::{ProviderCapabilities, QuantumProvider};
use qip_quantum::qaoa::{QaoaResult, QaoaSettings};
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::autonomy::AutonomyController;
use qip_risk_engine::pretrade::PreTradeChecker;

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

/// Fourteen names, four held, on a one-factor covariance whose loadings
/// alternate sign: hedging pairs make the landscape rugged enough that an
/// annealer baseline can miss the optimum, which is what gives a quantum
/// answer something to win.
fn problem() -> Result<PortfolioProblem> {
    let n = 14;
    let loading = |i: usize| ((i as f64 * 0.618_034) % 1.0 - 0.5) * 2.0;
    let covariance: Vec<Vec<f64>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| {
                    let common = 0.08 * loading(i) * loading(j);
                    if i == j {
                        common + 0.02 + 0.01 * (i % 3) as f64
                    } else {
                        common
                    }
                })
                .collect()
        })
        .collect();
    Ok(
        PortfolioProblem::new((0..n).map(|i| format!("obj-A{i}")).collect(), covariance)?
            .with_objective(Objective::MeanVariance)
            .with_expected_returns(
                (0..n)
                    .map(|i| 0.05 + 0.03 * ((i as f64 * 0.414_214) % 1.0))
                    .collect(),
            )
            .with_risk_aversion(5.0)
            .with_bounds(vec![0.0; n], vec![0.3; n])
            .fully_invested(1.0)
            .with_cardinality(4),
    )
}

/// A non-simulated provider that answers with one fixed assignment and its
/// true energy — a QPU that, this once, found something better.
#[derive(Debug)]
struct Winning(Vec<u8>);

impl QuantumProvider for Winning {
    fn name(&self) -> &str {
        "scripted-winner"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            max_qubits: 64,
            simulated: false,
            noisy: false,
            typical_queue: qip_core::time::Duration::ZERO,
            cost_per_job_micros: 0,
        }
    }
    fn solve_qubo(&self, qubo: &Qubo, _settings: &QaoaSettings) -> Result<QaoaResult> {
        Ok(QaoaResult {
            energy: qubo.evaluate(&self.0),
            assignment: self.0.clone(),
            expectation: 0.0,
            success_probability: 1.0,
            layers: 1,
            angles: vec![0.0, 0.0],
            evaluations: 1,
        })
    }
}

#[test]
fn a_position_sized_from_a_winning_quantum_answer_is_still_refused_by_pre_trade_risk() -> Result<()>
{
    let problem = problem()?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        // The annealer, not exhaustive search, is the baseline, so a better
        // answer exists to be found and a job is worth sending.
        exact_enumeration_limit: 2,
        quantum_margin: 0.0001,
        ..RoutingPolicy::default()
    };
    // The exhaustive optimum over every four-name selection; the provider
    // returns it, so the quantum arm wins exactly when the annealer missed it.
    // The first seed, in order, whose annealer baseline misses is used: the
    // search is deterministic, and finding none is a failed premise rather
    // than a pass.
    let encoding = QuboEncoding::equal_weight_bounded(4, 1.0, 0.3);
    let n = problem.assets.len();
    let mut found = None;
    for seed in 0..256u64 {
        let baseline = ComputeRouter::classical(seed)
            .with_policy(policy)?
            .solve(&problem)?;
        let mut best = (baseline.classical_objective, None::<Vec<u8>>);
        for mask in 0u32..(1 << n) {
            if mask.count_ones() != 4 {
                continue;
            }
            let assignment: Vec<u8> = (0..n).map(|i| ((mask >> i) & 1) as u8).collect();
            let objective = problem.objective_at(&encoding.to_weights(&assignment));
            if objective < best.0 - 1e-6 {
                best = (objective, Some(assignment));
            }
        }
        if let Some(better) = best.1 {
            found = Some((seed, better));
            break;
        }
    }
    let (seed, better) = found
        .expect("premise: some annealer baseline missed the optimum, so a quantum answer can win");

    let router = ComputeRouter::classical(seed)
        .with_policy(policy)?
        .with_quantum(Arc::new(Winning(better)))?;
    // Cycle one submits and answers classically; cycle two judges the answer.
    let first = router.solve(&problem)?;
    assert!(
        first.quantum_dispatch.submitted.is_some(),
        "no job was sent"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while router.quantum_job_ready()? != Some(true) {
        assert!(
            std::time::Instant::now() < deadline,
            "the job never returned"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let decision = router.solve(&problem)?;
    assert_eq!(
        decision.chosen,
        Solver::Quantum,
        "premise: the quantum answer won routing: {}",
        decision.rationale
    );
    assert!(decision.objective < decision.classical_objective);

    // Size the largest position the winning weights call for. A quarter of
    // the book in one name, against a 10% single-name cap.
    let (index, weight) = decision
        .weights
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("weights");
    assert!(
        weight > 0.10,
        "premise: the winning weight breaches the cap: {weight}"
    );
    let equity = dec!("1000000");
    let price = dec!("100");
    let quantity = (equity * Decimal::from_f64(weight).expect("a weight") / price).round_dp(0);
    let order = Order::new(
        OrderId::from_string("ord-quantum-sized"),
        ObjectId::from_string(problem.assets[index].clone()),
        Side::Buy,
        quantity,
        OrderType::Market,
        price,
        "prop-quantum-sized",
        vec!["hyp-quantum-sized".to_string()],
        "portfolio-construction",
        start(),
    );
    let limits = LimitSet::new("quantum-pretrade").with(
        Limit::new(
            "max-position-weight",
            LimitKind::MaxPositionWeight { limit: 0.10 },
        )
        .with_rationale("no single name may dominate the book, however it was chosen"),
    );
    let state = RiskState {
        equity,
        cash: equity,
        ..RiskState::default()
    };
    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 7);
    let mut manager = OrderManager::new(PreTradeChecker::new(limits));
    let result = manager.submit(
        order,
        &mut broker,
        &AutonomyController::new(),
        &state,
        BTreeMap::new(),
        None,
        start(),
    );
    assert!(
        !result.accepted,
        "a quantum-sized order breaching the single-name cap was sent"
    );
    assert!(result.fills.is_empty(), "the venue filled a refused order");
    match result.refusal {
        Some(RefusalReason::RiskRejected { breaches, .. }) => assert!(
            breaches
                .iter()
                .any(|b| b.limit_kind == "max_position_weight"),
            "pre-trade refused, but not on the single-name cap: {breaches:?}"
        ),
        other => panic!("refused for something other than pre-trade risk: {other:?}"),
    }
    assert!(broker.is_simulated(), "premise: paper trading only");
    Ok(())
}
