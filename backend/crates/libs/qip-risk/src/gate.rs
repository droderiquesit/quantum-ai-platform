//! The witness a hard-limit evaluation demands (ADR 0430, R12-RISK-3022).
//!
//! A hard limit is computed by deterministic code. Until ADR 0430 that was
//! held by one fact only: the crates that evaluate limits — this one and
//! `qip-risk-engine` — have no edge to `qip-ai`, which `qip-acceptance`'s
//! `no_safety_critical_engine_can_reach_a_language_model` enforces. The kernel
//! did obtain a `DeterministicRouting` from `qip_cost_router::Router::determine`
//! before the risk monitor's pass, but it kept the routing as a *record*
//! beside the call: `RiskMonitor::observe` took no routing, so a second call
//! site written without asking the router compiled, ran and evaluated the
//! limits exactly as well, and the only thing that would have noticed was a
//! source scan in a kernel unit test.
//!
//! [`DeterministicGate`] closes that half. It is the model-free face of a
//! deterministic routing: the cost router mints one inside every
//! `DeterministicRouting` it returns, and `RiskMonitor::observe` takes one by
//! reference and copies its rationale onto every observation. A caller that
//! never asked the router has no value to pass.
//!
//! Why it lives here and not in `qip-cost-router`: the router depends on
//! `qip-ai`, so the risk engine cannot name the router's type without
//! acquiring the edge the architecture suite exists to refuse. This library is
//! model-free, already listed in `SAFETY_CRITICAL`, already a dependency of
//! the risk engine, and a library the router may depend on without changing
//! what either side can reach.
//!
//! What it does **not** do, stated so nobody reads more into it: Rust cannot
//! restrict a public constructor to one other crate, so
//! [`DeterministicGate::minted_by_router_determination`] is callable from
//! anywhere that links this crate. What stops a forgery is the architecture
//! suite's scan `only_the_cost_router_mints_a_deterministic_gate`
//! (`qip-acceptance/tests/architecture.rs`), which refuses the
//! constructor's name in the production source of every crate but
//! `qip-cost-router`. Tests may mint one, because a test of the monitor is not
//! a path an order takes. The type carries no `Deserialize` and no `Clone`, so
//! it cannot be read off a wire or duplicated past the routing that minted it.

/// Proof that a hard-limit evaluation was routed as a deterministic decision.
///
/// Obtained from `qip_cost_router::DeterministicRouting::gate`, never built
/// by the caller that spends it (see the module documentation for what
/// enforces that).
#[derive(Debug, PartialEq, Eq)]
pub struct DeterministicGate {
    rationale: String,
}

impl DeterministicGate {
    /// Mint a gate for a decision the cost router determined.
    ///
    /// **Called by `qip_cost_router::Router` alone in production code.** The
    /// name is deliberately long and specific so that the acceptance scan
    /// refusing it elsewhere cannot be satisfied by a near-miss or defeated by
    /// a collision with an unrelated `new`.
    pub fn minted_by_router_determination(rationale: impl Into<String>) -> Self {
        Self {
            rationale: rationale.into(),
        }
    }

    /// Why the router determined the decision, as the router wrote it.
    pub fn rationale(&self) -> &str {
        &self.rationale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_carries_the_rationale_the_router_gave_it() {
        let gate = DeterministicGate::minted_by_router_determination("limits for the book");
        assert_eq!(gate.rationale(), "limits for the book");
    }
}
