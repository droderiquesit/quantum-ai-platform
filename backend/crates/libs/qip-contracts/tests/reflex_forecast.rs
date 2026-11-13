//! R12-WORLD-3084 / ADR 0506: the reflex forecast contract — the
//! microsecond row's output, promoted to the local reflex and nothing else.

use qip_contracts::forecast::{Calibration, OutcomeDistribution, OutcomeMass, QuantilePoint};
use qip_contracts::reflex_forecast::{
    MAX_REFLEX_HORIZON_US, NOT_OCCURRED, OCCURRED, ReflexForecast, ReflexForecastFields,
    ReflexPromotion, ReflexTarget,
};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp};

fn at(us: i64) -> Timestamp {
    Timestamp::from_secs(1_800_000_000).saturating_add(Duration::from_micros(us))
}

fn event(p_bp: u32) -> OutcomeDistribution {
    OutcomeDistribution::Categorical {
        outcomes: vec![
            OutcomeMass {
                outcome: OCCURRED.into(),
                probability_bp: p_bp,
            },
            OutcomeMass {
                outcome: NOT_OCCURRED.into(),
                probability_bp: 10_000 - p_bp,
            },
        ],
    }
}

fn magnitude() -> OutcomeDistribution {
    OutcomeDistribution::Quantiles {
        points: vec![
            QuantilePoint {
                quantile_bp: 1_000,
                value: Decimal::from(1),
            },
            QuantilePoint {
                quantile_bp: 9_000,
                value: Decimal::from(5),
            },
        ],
    }
}

fn fields(target: ReflexTarget) -> ReflexForecastFields {
    ReflexForecastFields {
        schema_version: 1,
        forecast_id: "rf-1".into(),
        cell: "newyork-1".into(),
        venue: VenueId::new("sim-a"),
        instrument: "ABC".into(),
        target,
        horizon_us: 500,
        issued_at: at(0),
        expires_at: at(500),
        distribution: if target.is_event() {
            event(3_000)
        } else {
            magnitude()
        },
        calibration: Calibration::Uncalibrated,
        model: "reflex-frequency/1".into(),
        promotion: ReflexPromotion::LocalReflexOnly,
    }
}

#[test]
fn every_reflex_target_is_admitted_in_its_own_shape() -> Result<()> {
    for target in ReflexTarget::ALL {
        let f = ReflexForecast::new(fields(target))?;
        let decoded = ReflexForecast::decode(&f.encode()?)?;
        assert_eq!(decoded, f);
    }
    Ok(())
}

#[test]
fn a_horizon_of_a_second_or_more_belongs_to_another_row_and_is_refused() -> Result<()> {
    // Premise: the bound itself is admitted, so the refusal below is the
    // horizon's and not the rest of the record's.
    let mut edge = fields(ReflexTarget::FillProbability);
    edge.horizon_us = MAX_REFLEX_HORIZON_US;
    ReflexForecast::new(edge)?;

    for bad in [0, MAX_REFLEX_HORIZON_US + 1, 2_000_000] {
        let mut f = fields(ReflexTarget::FillProbability);
        f.horizon_us = bad;
        let err = ReflexForecast::new(f).err().map(|e| e.to_string());
        assert!(
            err.as_deref().is_some_and(|e| e.contains("µs")),
            "horizon {bad}: {err:?}"
        );
    }
    Ok(())
}

#[test]
fn a_forecast_that_expires_after_its_own_horizon_is_refused() {
    let mut f = fields(ReflexTarget::QueueMovement);
    f.expires_at = at(501);
    let err = ReflexForecast::new(f).err().map(|e| e.to_string());
    assert!(
        err.as_deref()
            .is_some_and(|e| e.contains("forecast of the past")),
        "{err:?}"
    );
}

#[test]
fn an_event_stated_as_quantiles_or_a_magnitude_as_a_categorical_is_refused() {
    let mut f = fields(ReflexTarget::FillProbability);
    f.distribution = magnitude();
    let err = ReflexForecast::new(f).err().map(|e| e.to_string());
    assert!(
        err.as_deref()
            .is_some_and(|e| e.contains("event fill_probability as quantiles")),
        "{err:?}"
    );

    let mut f = fields(ReflexTarget::ExecutableEdge);
    f.distribution = event(5_000);
    let err = ReflexForecast::new(f).err().map(|e| e.to_string());
    assert!(
        err.as_deref()
            .is_some_and(|e| e.contains("magnitude executable_edge as a categorical")),
        "{err:?}"
    );

    let mut f = fields(ReflexTarget::AdverseSelection);
    f.distribution = OutcomeDistribution::Categorical {
        outcomes: vec![
            OutcomeMass {
                outcome: "up".into(),
                probability_bp: 4_000,
            },
            OutcomeMass {
                outcome: "down".into(),
                probability_bp: 6_000,
            },
        ],
    };
    let err = ReflexForecast::new(f).err().map(|e| e.to_string());
    assert!(
        err.as_deref()
            .is_some_and(|e| e.contains("exactly `occurred`")),
        "{err:?}"
    );
}

#[test]
fn a_record_naming_any_promotion_target_but_the_local_reflex_does_not_decode() -> Result<()> {
    let good = ReflexForecast::new(fields(ReflexTarget::FillProbability))?.encode()?;
    let text =
        String::from_utf8(good).map_err(|e| qip_core::error::Error::invalid(e.to_string()))?;
    // Premise: the encoded record names the one target, delimited, so the
    // replacement below swaps the field's value and nothing else.
    assert!(
        text.contains("\"promotion\":\"local_reflex_only\""),
        "{text}"
    );
    for other in [
        "reflex_warm_packages",
        "strategy_capital_risk",
        "portfolio_treasury",
        "strategic_allocation_research",
    ] {
        let bad = text.replace(
            "\"promotion\":\"local_reflex_only\"",
            &format!("\"promotion\":\"{other}\""),
        );
        assert!(
            ReflexForecast::decode(bad.as_bytes()).is_err(),
            "{other} decoded"
        );
    }
    Ok(())
}

#[test]
fn the_occurred_mass_is_read_only_from_an_event_target() -> Result<()> {
    let f = ReflexForecast::new(fields(ReflexTarget::FillProbability))?;
    assert_eq!(f.fields().occurred_bp(), Some(3_000));
    let m = ReflexForecast::new(fields(ReflexTarget::QueueMovement))?;
    assert_eq!(m.fields().occurred_bp(), None);
    Ok(())
}

#[test]
fn a_sub_millisecond_forecast_keeps_its_issue_and_expiry_apart_on_the_wire() -> Result<()> {
    // Issued and expiring inside one millisecond: an RFC 3339 instant at
    // millisecond precision would collapse the two and the decode would
    // refuse the record it had just encoded.
    let mut f = fields(ReflexTarget::FillProbability);
    f.issued_at = at(100);
    f.expires_at = at(600);
    let admitted = ReflexForecast::new(f)?;
    let decoded = ReflexForecast::decode(&admitted.encode()?)?;
    assert_eq!(decoded.fields().issued_at, at(100));
    assert_eq!(decoded.fields().expires_at, at(600));
    Ok(())
}
