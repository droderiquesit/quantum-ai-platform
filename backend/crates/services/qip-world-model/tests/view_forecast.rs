//! R2.5 in the federation: every member view is admitted as lane H's §9.14
//! `Forecast`, or it is not admitted.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_contracts::forecast::{Calibration, OutcomeDistribution};
use qip_core::time::{Duration, Timestamp};
use qip_world_model::federation::{
    Abstraction, Assessment, CalibrationClaim, Declaration, Federation, ModelKind, Scope,
    WorldModelSpec,
};

fn declaration(id: &str, lineage: &[&str]) -> Declaration {
    Declaration {
        id: id.into(),
        scope: Some(Scope {
            domain: "macro".into(),
            region: "global".into(),
            horizon: Duration::from_days(30),
            abstraction: Abstraction::Macro,
            hypothesis: "base".into(),
            kind: ModelKind::Observational,
        }),
        evidence_lineage: lineage.iter().map(|s| (*s).to_string()).collect(),
        calibration: Some(CalibrationClaim { max_brier: 0.25 }),
        update_cadence: Some(Duration::from_hours(1)),
        expires_at: Some(Timestamp::from_secs(1_000_000)),
    }
}

fn view(p: f64) -> Assessment {
    Assessment {
        proposition: "rates-rise".into(),
        probability: p,
        evidence_fit: 0.6,
        causal_coherence: 0.7,
        uncertainty: 0.25,
        at: Timestamp::from_secs(10),
    }
}

/// Mutation: drop the `view_forecast` call from `Federation::assess`, and no
/// forecast exists for the view and the refused one is admitted.
#[test]
fn a_member_view_is_admitted_as_a_forecast_and_one_the_contract_refuses_is_not() {
    let mut f = Federation::new();
    f.register(
        WorldModelSpec::declare(declaration("m1", &["feed:rates"])).unwrap(),
        Timestamp::from_secs(0),
    )
    .unwrap();
    f.assess("m1", view(0.7)).unwrap();
    let forecast = f
        .forecast("m1", "rates-rise")
        .unwrap()
        .expect("the view is a forecast");
    assert_eq!(forecast.model, "m1");
    assert_eq!(forecast.horizon, Duration::from_days(30));
    assert_eq!(
        forecast.expires_at,
        Timestamp::from_secs(10).saturating_add(Duration::from_hours(1))
    );
    assert!(matches!(forecast.calibration, Calibration::Uncalibrated));
    let OutcomeDistribution::Categorical { outcomes } = &forecast.distribution else {
        panic!("a view is categorical");
    };
    assert_eq!(outcomes[0].probability_bp, 7_000);
    // A member whose lineage names one source twice declares something the
    // forecast contract refuses; its view is refused, not admitted unread.
    f.register(
        WorldModelSpec::declare(declaration("m2", &["feed:rates", "feed:rates"])).unwrap(),
        Timestamp::from_secs(0),
    )
    .unwrap();
    assert!(f.assess("m2", view(0.7)).is_err());
    assert!(f.forecast("m2", "rates-rise").unwrap().is_none());
}
