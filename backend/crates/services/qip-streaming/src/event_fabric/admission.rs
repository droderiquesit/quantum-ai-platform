//! QoS admission: what the broker answers a producer it cannot take, per
//! class (ADR 0100 §5; FABRIC-7036, FABRIC-7056).
//!
//! The broker has two overload signals — a producer past its per-window
//! byte or message quota, and a consumer group trailing a partition past the
//! stream's lag limit — and this module decides, from the stream's policy,
//! what each one means. [`super::broker::Broker`] measures; this decides.
//! Until this module existed the measuring was in the broker and the deciding
//! was nowhere: every class over quota got the same `Quota` answer, and
//! nothing distinguished the classes ADR 0100 §5 says must never be dropped
//! from the one it says may be shed.
//!
//! # Never dropped is decided by the class, not the declaration
//!
//! [`effective_overload`] reads the class's
//! [`QosClass::required_overload_policy`] before the declared one. A P0 or
//! P1 stream is therefore answered as `refuse_producer` whatever its policy
//! says. [`StreamPolicy::new`] already refuses to build such a policy; this
//! is the second lock on the same door, so that a policy deserialised by some
//! later path, or a constructor someone relaxes, still cannot turn a fill
//! into a shed record. The failure it prevents is the one FABRIC-7056 names:
//! an outcome silently shed is an accounting hole, and a shed answer carries
//! an acknowledgement-shaped "we dealt with it" that a refusal does not.
//!
//! # Every answer is visible
//!
//! Nothing here drops a batch and answers success. A refusal goes back to the
//! producer as a named [`Refusal`], and `qip-fabricd` counts it per class and
//! reason (`qip_fabric_refusals_total`), so a producer that is being shed or
//! throttled is a number on a chart, not a silence.

use qip_events::event_fabric::policy::{OverloadPolicy, StreamPolicy};
use qip_transport::event_fabric::protocol::Refusal;

/// The overload behaviour the broker applies to `policy`'s stream: the
/// class's own where the class fixes one, the declared one otherwise.
pub fn effective_overload(policy: &StreamPolicy) -> OverloadPolicy {
    policy
        .qos_class()
        .required_overload_policy()
        .unwrap_or_else(|| policy.overload_policy())
}

/// What a producer past its quota is told.
///
/// `Quota` — wait `retry_after_ms` and resend, nothing was written — for
/// every class that may not lose a record: P0/P1 narrow, P2 throttles (and
/// writes its own `Gap` if it then sheds at the source), P3 backs up.
/// `Shed` only for a stream whose effective overload policy is
/// `sample_or_shed`, which only P4 telemetry can have: the producer is told
/// the record is gone and must not retry it, because retrying telemetry
/// under load is the load.
pub fn over_quota(policy: &StreamPolicy, retry_after_ms: u64) -> Refusal {
    match effective_overload(policy) {
        OverloadPolicy::SampleOrShed => Refusal::Shed {
            class: policy.qos_class(),
        },
        OverloadPolicy::RefuseProducer
        | OverloadPolicy::ThrottleWithGap
        | OverloadPolicy::AllowBacklog => Refusal::Quota { retry_after_ms },
    }
}

/// Whether a consumer group trailing past the lag limit refuses producers
/// on this stream. Only for `refuse_producer`: a never-dropped stream whose
/// consumers cannot keep up must stop growing rather than outrun its
/// archive and its ledger, while every other class tolerates a backlog by
/// declaration.
pub fn refuses_for_lag(policy: &StreamPolicy) -> bool {
    effective_overload(policy) == OverloadPolicy::RefuseProducer
}

/// Whether a consumer group trailing past the lag limit makes the broker
/// drop the partition's oldest batches instead, keeping the newest. Only for
/// `sample_or_shed`, which only telemetry can have: under pressure the
/// freshest telemetry is the useful telemetry, so the backlog is bounded by
/// spending its oldest end rather than by refusing the newest or growing.
pub fn drops_oldest_for_lag(policy: &StreamPolicy) -> bool {
    effective_overload(policy) == OverloadPolicy::SampleOrShed
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_events::RetentionClass;
    use qip_events::event_fabric::policy::{
        Entitlement, Mirroring, Ordering, QosClass, StreamPolicySpec,
    };

    fn policy(class: QosClass, overload: OverloadPolicy) -> StreamPolicy {
        StreamPolicy::new(StreamPolicySpec {
            qos_class: class,
            partition_key: "cell".to_string(),
            ordering: Ordering::PerPartition,
            retention: RetentionClass::DerivedState,
            replication_factor: 1,
            mirroring: Mirroring::None,
            overload_policy: overload,
            ack_profile: class.ack_floor(),
            byte_quota_per_producer: 1,
            message_quota_per_producer: 1,
            lag_limit: 1,
            entitlement: Entitlement::new("d", "research").expect("valid entitlement"),
            seal_age_ms: 1,
            peak_bytes_per_second: 1,
        })
        .expect("a policy the class permits")
    }

    /// FABRIC-7056: across every class and every overload policy the class
    /// may be declared with, only telemetry is ever answered `Shed`, and a
    /// never-dropped class is always answered with a wait and refused for
    /// lag. Mutation: make `effective_overload` return the declared policy
    /// alone and swap P1's in `required_overload_policy` for `None` — the P1
    /// row is then answered `Shed` and this fails.
    #[test]
    fn only_telemetry_is_ever_shed_and_a_never_dropped_class_is_always_refused_visibly() {
        let overloads = [
            OverloadPolicy::RefuseProducer,
            OverloadPolicy::ThrottleWithGap,
            OverloadPolicy::AllowBacklog,
            OverloadPolicy::SampleOrShed,
        ];
        let mut combinations = 0;
        for class in QosClass::ALL {
            for overload in overloads {
                if !class.permits_overload_policy(overload) {
                    continue;
                }
                combinations += 1;
                let policy = policy(class, overload);
                let answer = over_quota(&policy, 250);
                match answer {
                    Refusal::Shed { class: shed } => {
                        assert_eq!(shed, QosClass::P4Telemetry, "{class:?} was shed");
                    }
                    Refusal::Quota { retry_after_ms } => assert_eq!(retry_after_ms, 250),
                    other => panic!("{class:?}/{overload:?} answered {other:?}"),
                }
                if matches!(class, QosClass::P0Control | QosClass::P1Outcomes) {
                    assert!(refuses_for_lag(&policy), "{class:?} must refuse for lag");
                }
            }
        }
        // Premise: the loop judged something on each side of the line.
        assert!(combinations >= 6, "only {combinations} combinations judged");
        assert_eq!(
            over_quota(
                &policy(QosClass::P4Telemetry, OverloadPolicy::SampleOrShed),
                1
            ),
            Refusal::Shed {
                class: QosClass::P4Telemetry
            }
        );
        assert!(!refuses_for_lag(&policy(
            QosClass::P3Research,
            OverloadPolicy::AllowBacklog
        )));
    }
}
