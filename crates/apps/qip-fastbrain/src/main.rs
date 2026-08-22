//! The Fast Brain node.
//!
//! Market data, microstructure, real-time risk and execution: the path that
//! must answer in microseconds to milliseconds.
//!
//! The rule this node exists to enforce is that **nothing on the fast path
//! waits for a language model**. It is checked at start-up rather than assumed:
//! the node refuses to run if any agent it would host holds
//! `call_language_model` or has a budget permitting one. A fast path that
//! blocks on a model call is not a fast path, and discovering that under load
//! is expensive.

use qip_agents::capability::Capability;
use qip_core::error::{Error, Result};
use qip_core::{Clock, SystemClock};
use qip_investment_agents::manifests;
use std::sync::Arc;

/// The agents this node hosts.
const FAST_PATH_AGENTS: &[&str] = &[manifests::ids::MICROSTRUCTURE];

/// The longest a fast-path agent may take.
///
/// Fifty milliseconds is generous for what these agents do and tight enough
/// that anything reaching for a network call fails.
const MAXIMUM_BUDGET: qip_core::Duration = qip_core::Duration::from_millis(50);

fn main() {
    if let Err(error) = run() {
        eprintln!("qip-fastbrain: {}", error.message());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let now = clock.now();
    let roster = manifests::roster(now);

    println!("qip-fastbrain");
    for id in FAST_PATH_AGENTS {
        let manifest = roster
            .get(id)
            .ok_or_else(|| Error::not_found(format!("{id} is not on the roster")))?;

        // The check that makes the guarantee real.
        if manifest
            .capabilities
            .contains(Capability::CallLanguageModel)
        {
            return Err(Error::denied(format!(
                "{id} holds call_language_model; the fast path must never wait on a model"
            )));
        }
        if manifest.budget.language_model_calls > 0 {
            return Err(Error::denied(format!(
                "{id} has a budget for {} language-model call(s); the fast path must never wait on a model",
                manifest.budget.language_model_calls
            )));
        }
        if manifest.budget.wall_time > MAXIMUM_BUDGET {
            return Err(Error::denied(format!(
                "{id} may run for {:?}, beyond the {MAXIMUM_BUDGET:?} the fast path allows",
                manifest.budget.wall_time
            )));
        }
        manifest.authorisation(now)?;

        println!(
            "  {id}: {:?} budget, {} tool call(s), no language model",
            manifest.budget.wall_time, manifest.budget.tool_calls
        );
    }

    println!("  every hosted agent is model-free and inside the fast-path budget");
    Ok(())
}
