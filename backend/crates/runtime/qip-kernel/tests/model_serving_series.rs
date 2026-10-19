//! `Platform::serve_model` feeds the model-serving objective (R12-OBS-7128).
//!
//! The failure prevented: the only model-serving objective read
//! `qip_model_packages_total`, which says a promoted artifact was written. A
//! provider that refused every artifact left that series untouched, so the
//! objective read "unmeasured" — indistinguishable from a healthy platform
//! that had served nothing. The serve seam now counts, and the objective is
//! evaluated from the count.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_ai::serving::{ModelArtifact, ModelFormat, ModelProvider, ServedModel};
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_observability::critical_paths::evaluate;
use qip_observability::metrics::{labels, names};
use qip_risk::limits::LimitSet;

const OBJECTIVE: &str = "model-serving";

/// A provider that serves one format and nothing else, so the same platform
/// can be asked for an artifact it serves and one it refuses.
#[derive(Debug)]
struct OneFormat;

#[derive(Debug)]
struct Constant {
    reference: String,
}

impl ServedModel for Constant {
    fn reference(&self) -> &str {
        &self.reference
    }
    fn format(&self) -> ModelFormat {
        ModelFormat::TeacherLinear
    }
    fn arity(&self) -> usize {
        1
    }
    fn cost(&self) -> usize {
        1
    }
    fn score(&self, _inputs: &[f64]) -> Result<f64> {
        Ok(0.0)
    }
}

impl ModelProvider for OneFormat {
    fn name(&self) -> &str {
        "the one-format test provider"
    }
    fn serves(&self) -> &[ModelFormat] {
        &[ModelFormat::TeacherLinear]
    }
    fn serve(&self, artifact: &ModelArtifact) -> Result<Box<dyn ServedModel>> {
        self.admit(artifact)?;
        Ok(Box::new(Constant {
            reference: artifact.reference.clone(),
        }))
    }
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default().with_initial_equity(Decimal::from_int(1_000_000));
    let (context, _clock) =
        Context::deterministic(Timestamp::from_secs(1_760_000_000), config.seed);
    Platform::new_serving(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
        Box::new(OneFormat),
    )
}

fn artifact(reference: &str, format: ModelFormat) -> ModelArtifact {
    ModelArtifact::new(reference, format, serde_json::json!({ "weights": [1.0] }))
}

#[test]
fn a_served_model_request_moves_the_serving_series_and_an_unserved_objective_reads_unmeasured()
-> Result<()> {
    let platform = platform()?;
    let objective = |platform: &Platform| {
        evaluate(&platform.telemetry().metrics.snapshot())
            .into_iter()
            .find(|status| status.slo.name == OBJECTIVE)
            .ok_or_else(|| Error::not_found("model-serving is not in the catalogue"))
    };

    // Premise: nothing has been asked of the provider, the series is absent,
    // and the objective reads unmeasured rather than met.
    let before = objective(&platform)?;
    assert!(
        !before.is_observed(),
        "the objective was observed before any request"
    );
    assert_eq!(
        platform
            .telemetry()
            .metrics
            .snapshot()
            .counter_total(names::MODEL_SERVES),
        0
    );

    // A request the provider serves moves the `served` arm and only it.
    platform.serve_model(&artifact("model-a", ModelFormat::TeacherLinear))?;
    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::MODEL_SERVES, &labels([("outcome", "served")])),
        1,
        "a served request did not move the serving series"
    );
    assert_eq!(
        snapshot.counter(names::MODEL_SERVES, &labels([("outcome", "refused")])),
        0
    );
    let served = objective(&platform)?;
    assert!(served.is_observed());
    assert!(
        served.is_met,
        "one served request in one is not under 0.99: {served:?}"
    );

    // A request it refuses is the other arm, and drags the objective under
    // its 0.99 target: one in two.
    assert!(
        platform
            .serve_model(&artifact("model-b", ModelFormat::DistilledTree))
            .is_err(),
        "premise: the provider refuses a format it does not serve"
    );
    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::MODEL_SERVES, &labels([("outcome", "refused")])),
        1
    );
    let mixed = objective(&platform)?;
    assert_eq!(mixed.observations, 2);
    assert!(
        !mixed.is_met,
        "half the requests refused still met 0.99: {mixed:?}"
    );
    Ok(())
}
