//! Map detected anomalies to GapSignals (EXPAND-025).
//!
//! Each anomaly detection feeds into a GapTrigger and GapSignal with the
//! detector's confidence, magnitude and context as severity and economic value.
//!
//! # Why this is in the Deep Brain's root and not in the DISCOVER stage
//!
//! It arrived in `qip-opportunity-engine`, which put `qip-expansion` — and,
//! through it, `qip-agents` — under a Lane 2 service and under `qip-kernel`,
//! which composes that service. That is the failure EXPAND-005 names: the
//! research curriculum becoming a dependency of the thing that decides, so a
//! stalled queue can stall a cycle. The expansion engine reads the brains and
//! nothing the brains do may wait on it, so the only crate that may hold both
//! the anomaly vocabulary and the gap vocabulary is a composition root off the
//! reflex path. `qip-acceptance`'s
//! `nothing_below_a_composition_root_depends_on_the_expansion_engine` and
//! `lane_placement`'s cognitive-slow-lane test are what refused the first
//! placement.
//!
//! Called by [`crate::research::ResearchLoop::sweep`] on every anomaly the
//! deep brain's own detector sweep finds over the platform's price history.
//! An observation naming two lacks is split there, one per lack, which is
//! what `gap::classify` asks for when it refuses mixed evidence; one naming
//! [`Wanting::Unknown`] is counted unclassified rather than given a class.

use qip_core::Decimal;
use qip_expansion::gap::{GapTrigger, Observation, Wanting};
use qip_opportunity_engine::detector::{Anomaly, AnomalyKind};

/// Convert an anomaly detection to a gap observation that can be raised.
pub fn anomaly_to_observation(anomaly: &Anomaly) -> Observation {
    let trigger = match anomaly.kind {
        AnomalyKind::PriceMove
        | AnomalyKind::VolatilityShift
        | AnomalyKind::VolumeSpike
        | AnomalyKind::StructuralBreak
        | AnomalyKind::RegimeChange
        | AnomalyKind::CorrelationBreakdown
        | AnomalyKind::LiquidityDeterioration
        | AnomalyKind::FundamentalSurprise
        | AnomalyKind::MacroSurprise
        | AnomalyKind::SentimentShift
        | AnomalyKind::AlternativeDataDivergence
        | AnomalyKind::UnexplainedMove => GapTrigger::Surprise,
        AnomalyKind::Catalyst => GapTrigger::Contradiction,
    };

    // Severity is the detector's confidence calibrated by the anomaly's importance.
    let severity = (anomaly.confidence() * anomaly.importance()).clamp(0.0, 1.0);

    // Economic value is estimated from the z-score magnitude and sample size.
    // A larger deviation from expected, with more evidence behind it,
    // represents greater economic significance.
    let magnitude = anomaly.z_score.abs().min(10.0) / 10.0;
    let evidence_weight =
        (anomaly.sample_size as f64 / (anomaly.sample_size as f64 + 200.0)).clamp(0.0, 1.0);
    let economic_value = Decimal::from_f64(magnitude * evidence_weight * 100.0).unwrap_or_default();

    // Risk value is the move itself, not its significance: the observed
    // deviation as a share of what was expected, capped at the whole of it,
    // scaled by the confidence that the anomaly is real, on the same
    // [0, 100] scale. A thin-history ten-sigma print on a quiet series is
    // economically significant and exposes little; a modest z-score on a
    // series that halved exposes a great deal, and one figure cannot say
    // both. With nothing expected the whole deviation is the exposure.
    // Statistics stay f64 and cross to Decimal once, here.
    let relative = if anomaly.expected.abs() > f64::EPSILON {
        ((anomaly.observed - anomaly.expected) / anomaly.expected).abs()
    } else if anomaly.observed.abs() <= f64::EPSILON {
        0.0
    } else {
        1.0
    };
    let risk_value = if relative.is_finite() {
        Decimal::from_f64(relative.min(1.0) * anomaly.confidence() * 100.0).unwrap_or_default()
    } else {
        Decimal::ZERO
    };

    // What the anomaly indicates is missing or wrong.
    let wanting = match anomaly.kind {
        AnomalyKind::StructuralBreak | AnomalyKind::RegimeChange => {
            vec![Wanting::CausalLink, Wanting::Model]
        }
        AnomalyKind::FundamentalSurprise | AnomalyKind::MacroSurprise => {
            vec![Wanting::Source, Wanting::Type]
        }
        AnomalyKind::UnexplainedMove => vec![Wanting::CausalLink, Wanting::Source],
        AnomalyKind::CorrelationBreakdown => vec![Wanting::Model],
        AnomalyKind::SentimentShift | AnomalyKind::AlternativeDataDivergence => {
            vec![Wanting::Source]
        }
        _ => vec![Wanting::Unknown],
    };

    Observation {
        trigger,
        wanting,
        observation: anomaly.description.clone(),
        affected_domains: vec!["market".to_string()],
        evidence: vec![anomaly.detector.clone()],
        severity,
        economic_value,
        risk_value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anomaly_with_high_confidence_maps_to_high_severity() {
        let anomaly = Anomaly {
            kind: AnomalyKind::StructuralBreak,
            subject: "AAPL".to_string(),
            detector: "structural_break_detector".to_string(),
            z_score: 6.5,
            observed: 150.0,
            expected: 140.0,
            sample_size: 500,
            detected_at: Default::default(),
            description: "Structural break detected in AAPL".to_string(),
            catalyst: None,
        };

        let observation = anomaly_to_observation(&anomaly);
        let confidence = anomaly.confidence();
        let importance = anomaly.importance();
        let expected_severity = (confidence * importance).clamp(0.0, 1.0);

        assert!((observation.severity - expected_severity).abs() < 0.01);
        assert_eq!(observation.trigger, GapTrigger::Surprise);
    }

    #[test]
    fn price_move_anomaly_maps_to_surprise_trigger() {
        let anomaly = Anomaly {
            kind: AnomalyKind::PriceMove,
            subject: "EURUSD".to_string(),
            detector: "price_move_detector".to_string(),
            z_score: 4.2,
            observed: 1.15,
            expected: 1.10,
            sample_size: 100,
            detected_at: Default::default(),
            description: "Large price move in EURUSD".to_string(),
            catalyst: None,
        };

        let observation = anomaly_to_observation(&anomaly);
        assert_eq!(observation.trigger, GapTrigger::Surprise);
        assert!(!observation.evidence.is_empty());
        assert_eq!(observation.evidence[0], "price_move_detector");
    }

    #[test]
    fn unexplained_move_has_causal_link_wanting() {
        let anomaly = Anomaly {
            kind: AnomalyKind::UnexplainedMove,
            subject: "BTC".to_string(),
            detector: "unexplained_move_detector".to_string(),
            z_score: 8.0,
            observed: 50000.0,
            expected: 45000.0,
            sample_size: 1000,
            detected_at: Default::default(),
            description: "Unexplained move in BTC".to_string(),
            catalyst: None,
        };

        let observation = anomaly_to_observation(&anomaly);
        assert!(observation.wanting.contains(&Wanting::CausalLink));
    }

    /// CONTRACT-7084: the mapper states exposure apart from significance. A
    /// series that halved on a modest z-score exposes more than a thin
    /// ten-sigma print that moved a fraction of a percent, and a gap signal
    /// carrying one figure ranked them the other way round.
    #[test]
    fn a_large_move_on_a_modest_z_score_carries_more_risk_than_a_tiny_move_on_a_huge_one() {
        let anomaly = |z_score: f64, observed: f64, expected: f64| Anomaly {
            kind: AnomalyKind::CorrelationBreakdown,
            subject: "XYZ".to_string(),
            detector: "correlation_detector".to_string(),
            z_score,
            observed,
            expected,
            sample_size: 400,
            detected_at: Default::default(),
            description: "correlation".to_string(),
            catalyst: None,
        };
        let halved = anomaly_to_observation(&anomaly(4.0, 50.0, 100.0));
        let sliver = anomaly_to_observation(&anomaly(10.0, 100.5, 100.0));
        assert!(
            sliver.economic_value > halved.economic_value,
            "premise: the ten-sigma print is the more significant one"
        );
        assert!(
            halved.risk_value > sliver.risk_value,
            "the halved series must carry the larger exposure: {} vs {}",
            halved.risk_value,
            sliver.risk_value
        );
        assert!(halved.risk_value > Decimal::ZERO);
        let raised = qip_expansion::gap::raise(halved).expect("raises");
        assert!(
            raised.signal.risk_value > Decimal::ZERO,
            "raise carries the figure through"
        );
    }
}
