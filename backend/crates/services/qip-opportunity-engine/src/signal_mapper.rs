//! Map detected anomalies to GapSignals (EXPAND-025).
//!
//! Each anomaly detection feeds into a GapTrigger and GapSignal with the
//! detector's confidence, magnitude and context as severity and economic value.

use crate::detector::Anomaly;
use qip_core::Decimal;
use qip_expansion::gap::{GapTrigger, Observation, Wanting};

/// Convert an anomaly detection to a gap observation that can be raised.
pub fn anomaly_to_observation(anomaly: &Anomaly) -> Observation {
    let trigger = match anomaly.kind {
        crate::detector::AnomalyKind::PriceMove
        | crate::detector::AnomalyKind::VolatilityShift
        | crate::detector::AnomalyKind::VolumeSpike
        | crate::detector::AnomalyKind::StructuralBreak
        | crate::detector::AnomalyKind::RegimeChange
        | crate::detector::AnomalyKind::CorrelationBreakdown
        | crate::detector::AnomalyKind::LiquidityDeterioration
        | crate::detector::AnomalyKind::FundamentalSurprise
        | crate::detector::AnomalyKind::MacroSurprise
        | crate::detector::AnomalyKind::SentimentShift
        | crate::detector::AnomalyKind::AlternativeDataDivergence
        | crate::detector::AnomalyKind::UnexplainedMove => GapTrigger::Surprise,
        crate::detector::AnomalyKind::Catalyst => GapTrigger::Contradiction,
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

    // What the anomaly indicates is missing or wrong.
    let wanting = match anomaly.kind {
        crate::detector::AnomalyKind::StructuralBreak
        | crate::detector::AnomalyKind::RegimeChange => {
            vec![Wanting::CausalLink, Wanting::Model]
        }
        crate::detector::AnomalyKind::FundamentalSurprise
        | crate::detector::AnomalyKind::MacroSurprise => {
            vec![Wanting::Source, Wanting::Type]
        }
        crate::detector::AnomalyKind::UnexplainedMove => vec![Wanting::CausalLink, Wanting::Source],
        crate::detector::AnomalyKind::CorrelationBreakdown => vec![Wanting::Model],
        crate::detector::AnomalyKind::SentimentShift
        | crate::detector::AnomalyKind::AlternativeDataDivergence => {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::AnomalyKind;

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
}
