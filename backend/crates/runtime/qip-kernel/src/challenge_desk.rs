//! Challenges raised from outside the tournament's own cycle
//! (R12-REASON-9032, ADR 0637).
//!
//! The verifier society already attacked every important prediction as the
//! cycle formed it, and kept the §27 pair on the journaled review. What no
//! caller did was *answer* a [`ChallengeRequest`] a second brain raised:
//! `VerifierSociety::challenge_prediction` was called by tests only. This
//! module is that seam, over the log the platform already journals rather
//! than a new fabric topic (a new `Topic` variant forces every exhaustive
//! match in the platform to move, and this wave has thirty-seven other lanes
//! in that tree): a request is journaled under
//! [`Topic::HypothesisChallenged`], the LEARN stage answers each unanswered
//! one against the dossier the tournament retained when the prediction was
//! made, and the answer is journaled beside it.
//!
//! **A fatal challenge rejects.** An answer whose verdict is `Refuted` is
//! recorded with `rejected: true`, and [`Platform::rejected_by_challenge`]
//! reads it back from the log, so the rejection survives a restart and is
//! attributable to the request that caused it.
//!
//! **Bounded.** At most [`ANSWERS_PER_CYCLE`] requests are answered per
//! cycle; the rest wait for the next, in journal order.

use crate::platform::Platform;
use qip_contracts::challenge::{
    ChallengeRequest, ChallengeTarget, ChallengeVerdict, VerificationResult,
};
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use qip_streaming::envelope::StreamEnvelope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The producer the desk's records are written under.
pub const CHALLENGE_ORIGIN: &str = "challenge-desk";

/// Requests answered per cycle.
pub const ANSWERS_PER_CYCLE: usize = 16;

/// One record of the challenge desk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChallengeRecord {
    Requested {
        request: ChallengeRequest,
    },
    Answered {
        request_id: String,
        hypothesis_id: String,
        result: VerificationResult,
        /// Whether the verdict was `Refuted`: the prediction is rejected.
        rejected: bool,
    },
    /// A request the desk could not answer, with the reason, so it is not
    /// retried every cycle and not silently dropped.
    Refused {
        request_id: String,
        reason: String,
    },
}

impl EventBody for ChallengeRecord {
    const TOPIC: Topic = Topic::HypothesisChallenged;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(match self {
            Self::Requested { request } => format!("challenge-requested:{}", request.request_id),
            Self::Answered { request_id, .. } => format!("challenge-answered:{request_id}"),
            Self::Refused { request_id, .. } => format!("challenge-refused:{request_id}"),
        })
    }
}

impl Platform {
    fn challenge_records(&self) -> Vec<ChallengeRecord> {
        self.event_log()
            .by_topic(Topic::HypothesisChallenged)
            .into_iter()
            .filter(|event| event.lineage.producer == CHALLENGE_ORIGIN)
            .filter_map(|event| {
                StreamEnvelope::from_frame(event)
                    .and_then(|envelope| envelope.decode::<ChallengeRecord>())
                    .map(|envelope| envelope.body)
                    .ok()
            })
            .collect()
    }

    /// Journal a challenge a second brain raised. Refused for a request id
    /// already on the record, so a retried submission is not a second attack.
    pub fn submit_challenge(&mut self, request: ChallengeRequest, now: Timestamp) -> Result<()> {
        let id = request.request_id.clone();
        let known = self
            .challenge_records()
            .into_iter()
            .any(|record| match record {
                ChallengeRecord::Requested { request } => request.request_id == id,
                _ => false,
            });
        if known {
            return Err(Error::invalid(format!(
                "challenge `{id}` is already on the record; a new attack carries a new request id"
            )));
        }
        self.journal_record(
            ChallengeRecord::Requested { request },
            CHALLENGE_ORIGIN,
            now,
        )
    }

    /// Answer the unanswered requests, oldest first, at most
    /// [`ANSWERS_PER_CYCLE`]. Returns how many were answered or refused.
    pub fn answer_challenges(&mut self, now: Timestamp) -> Result<usize> {
        let records = self.challenge_records();
        let done: BTreeSet<String> = records
            .iter()
            .filter_map(|record| match record {
                ChallengeRecord::Answered { request_id, .. }
                | ChallengeRecord::Refused { request_id, .. } => Some(request_id.clone()),
                ChallengeRecord::Requested { .. } => None,
            })
            .collect();
        let pending: Vec<ChallengeRequest> = records
            .into_iter()
            .filter_map(|record| match record {
                ChallengeRecord::Requested { request } if !done.contains(&request.request_id) => {
                    Some(request)
                }
                _ => None,
            })
            .take(ANSWERS_PER_CYCLE)
            .collect();
        let mut handled = 0;
        for request in pending {
            let record = match self.tournament.answer_challenge(&request, now) {
                Ok(result) => {
                    let ChallengeTarget::Prediction { hypothesis_id, .. } = &request.target else {
                        continue;
                    };
                    let rejected = matches!(result.verdict, ChallengeVerdict::Refuted { .. });
                    ChallengeRecord::Answered {
                        request_id: request.request_id.clone(),
                        hypothesis_id: hypothesis_id.clone(),
                        result,
                        rejected,
                    }
                }
                Err(error) => ChallengeRecord::Refused {
                    request_id: request.request_id.clone(),
                    reason: error.message().to_string(),
                },
            };
            self.journal_record(record, CHALLENGE_ORIGIN, now)?;
            handled += 1;
        }
        Ok(handled)
    }

    /// Whether a challenge answered `hypothesis_id` with a fatal verdict.
    /// Read from the log, so it holds across a restart.
    pub fn rejected_by_challenge(&self, hypothesis_id: &str) -> bool {
        self.challenge_records().into_iter().any(|record| {
            matches!(&record, ChallengeRecord::Answered { hypothesis_id: h, rejected: true, .. } if h == hypothesis_id)
        })
    }

    /// The answer journaled for `request_id`, if any.
    pub fn challenge_answer(&self, request_id: &str) -> Option<VerificationResult> {
        self.challenge_records()
            .into_iter()
            .find_map(|record| match record {
                ChallengeRecord::Answered {
                    request_id: id,
                    result,
                    ..
                } if id == request_id => Some(result),
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlatformConfig;
    use qip_core::Context;
    use qip_financial::universe::Universe;
    use qip_learning_engine::verifier::{
        EvidenceItem, PanelView, PredictionDossier, VerifierSociety,
    };
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;
    use std::collections::BTreeMap;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn platform() -> Platform {
        let config = PlatformConfig::default();
        let (context, _clock) = Context::deterministic(at(), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
        .expect("a default platform builds")
    }

    fn dossier(id: &str, adverse_tail_bps: f64) -> PredictionDossier {
        PredictionDossier {
            hypothesis_id: id.to_string(),
            subject: "obj-AAA".to_string(),
            direction: 1.0,
            confidence: 0.7,
            as_of: at(),
            expected_move_bps: 200.0,
            evidence: vec![
                EvidenceItem {
                    origin: "exchange".to_string(),
                    supports: true,
                    weight: 0.5,
                    known_at: at(),
                },
                EvidenceItem {
                    origin: "filings".to_string(),
                    supports: true,
                    weight: 0.4,
                    known_at: at(),
                },
            ],
            panel: vec![PanelView {
                agent: "valuation".to_string(),
                direction: 1.0,
                conviction: 0.6,
            }],
            missing_inputs: Vec::new(),
            coverage: 0.8,
            regime_crossed_recently: Some(false),
            spread_ratio: Some(1.1),
            adverse_tail_bps: Some(adverse_tail_bps),
            forecaster_failure: BTreeMap::from([("detector:momentum".to_string(), 0.2)]),
        }
    }

    fn request(request_id: &str, d: &PredictionDossier) -> ChallengeRequest {
        VerifierSociety::standard()
            .request_for_prediction(request_id, d, "deep-brain", at())
            .expect("a request builds")
    }

    /// A challenge a second brain journals is answered by the LEARN stage's
    /// seam, a fatal one rejects the prediction, a survivable one does not,
    /// and a challenge for a prediction nobody retained is refused on the
    /// record rather than attacked from a reconstruction.
    #[test]
    fn a_journaled_challenge_is_answered_and_a_fatal_one_rejects_the_prediction() {
        let mut platform = platform();
        let sound = dossier("hyp-sound", 150.0);
        let fragile = dossier("hyp-fragile", 900.0);
        platform.tournament.retain_dossier(&sound);
        platform.tournament.retain_dossier(&fragile);
        for (id, d) in [("ext-sound", &sound), ("ext-fragile", &fragile)] {
            platform
                .submit_challenge(request(id, d), at())
                .expect("a challenge is journaled");
        }
        let stranger = dossier("hyp-nobody-retained", 900.0);
        platform
            .submit_challenge(request("ext-stranger", &stranger), at())
            .expect("journaled");
        assert!(
            platform
                .submit_challenge(request("ext-sound", &sound), at())
                .is_err(),
            "a repeated request id was a second attack"
        );
        assert!(
            !platform.rejected_by_challenge("hyp-fragile"),
            "premise: nothing is rejected before the desk answers"
        );

        assert_eq!(platform.answer_challenges(at()).expect("answers"), 3);

        assert!(platform.rejected_by_challenge("hyp-fragile"));
        assert!(!platform.rejected_by_challenge("hyp-sound"));
        let answer = platform
            .challenge_answer("ext-fragile")
            .expect("the fatal challenge has an answer");
        assert!(matches!(answer.verdict, ChallengeVerdict::Refuted { .. }));
        assert!(platform.challenge_answer("ext-sound").is_some());
        assert!(
            platform.challenge_answer("ext-stranger").is_none(),
            "a prediction nobody retained was attacked from nothing"
        );
        // Nothing is answered twice.
        assert_eq!(platform.answer_challenges(at()).expect("answers"), 0);
    }

    #[test]
    fn a_cycle_bounded_number_of_challenges_is_answered_and_the_rest_wait() {
        let mut platform = platform();
        let d = dossier("hyp-sound", 150.0);
        platform.tournament.retain_dossier(&d);
        for n in 0..(ANSWERS_PER_CYCLE + 3) {
            platform
                .submit_challenge(request(&format!("ext-{n}"), &d), at())
                .expect("journaled");
        }
        assert_eq!(
            platform.answer_challenges(at()).expect("answers"),
            ANSWERS_PER_CYCLE
        );
        assert_eq!(platform.answer_challenges(at()).expect("answers"), 3);
    }

    /// The LEARN stage is what calls the desk: without that call a journaled
    /// challenge waits for ever and the row is a method nothing runs.
    #[test]
    fn a_cycle_answers_a_journaled_challenge_without_anyone_calling_the_desk() {
        let mut platform = platform();
        let d = dossier("hyp-fragile", 900.0);
        platform.tournament.retain_dossier(&d);
        platform
            .submit_challenge(request("ext-cycle", &d), at())
            .expect("journaled");
        assert!(
            platform.challenge_answer("ext-cycle").is_none(),
            "premise: unanswered before the cycle"
        );
        let _ = platform.run_cycle(at());
        assert!(platform.rejected_by_challenge("hyp-fragile"));
    }
}
