//! Stream policies for managing message delivery and backpressure (CONTRACT-037).
//!
//! A StreamPolicy carries: partition key; replication factor; ack policy;
//! retention; durability/QoS class; byte and message limits; consumer lag
//! threshold; mirroring policy; overload/shedding behavior; and entitlement.
//! Durability is set per topic. Control, outcome, journal, research and
//! telemetry streams need different acks, retention and shedding, and a stream
//! created without a policy gets whatever the default happened to be.

use crate::{Entitlement, QoSClass};
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Acknowledgment policy for stream messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AckPolicy {
    /// Acknowledge after quorum writes (strongest durability).
    Quorum,
    /// Acknowledge immediately upon receipt (weaker durability).
    Leader,
    /// No acknowledgment required.
    None,
}

/// Behavior when a stream is overloaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverloadBehavior {
    /// Drop oldest messages (FIFO drop).
    DropOldest,
    /// Drop newest messages (LIFO drop).
    DropNewest,
    /// Block until buffer is drained.
    Block,
    /// Refuse new messages with backpressure signal.
    Refuse,
}

/// Mirroring policy for cross-region replication.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MirroringPolicy {
    /// No mirroring across regions.
    None,
    /// Mirror to all regions asynchronously.
    AsynchronousAll,
    /// Mirror to a subset of regions asynchronously.
    AsynchronousSubset,
}

/// Policy for stream message delivery, durability and handling (CONTRACT-037).
///
/// Creating a stream without a StreamPolicy, or with a policy missing any
/// listed field, is refused. Under overload a telemetry-class stream sheds
/// records and a control-class stream does not.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamPolicy {
    /// Key used to determine partition assignment for ordering and parallelism.
    pub partition_key: String,
    /// Number of replicas for fault tolerance and durability.
    pub replication_factor: u32,
    /// Acknowledgment required from replicas before write is confirmed.
    pub ack_policy: AckPolicy,
    /// How long to retain messages (messages older than this may be deleted).
    pub retention: Duration,
    /// QoS/priority class determining durability, replication and ack requirements.
    pub durability_qos_class: QoSClass,
    /// Maximum bytes to hold before enforcing overload policy.
    pub byte_limit: u64,
    /// Maximum messages to hold before enforcing overload policy.
    pub message_limit: u64,
    /// Consumer lag threshold in milliseconds before triggering monitoring alerts.
    pub consumer_lag_threshold_ms: u32,
    /// Mirroring policy for cross-region replication.
    pub mirroring_policy: MirroringPolicy,
    /// Behavior when stream is overloaded (backpressure/shedding).
    pub overload_behavior: OverloadBehavior,
    /// Data entitlement for access control and licensing.
    pub entitlement: Option<Entitlement>,
}

impl StreamPolicy {
    /// Construct a complete StreamPolicy with all ten required fields.
    /// Use this to build a spec for production streams.
    pub fn new(
        partition_key: impl Into<String>,
        replication_factor: u32,
        ack_policy: AckPolicy,
        retention: Duration,
        durability_qos_class: QoSClass,
        byte_limit: u64,
        message_limit: u64,
        consumer_lag_threshold_ms: u32,
        mirroring_policy: MirroringPolicy,
        overload_behavior: OverloadBehavior,
    ) -> Self {
        Self {
            partition_key: partition_key.into(),
            replication_factor,
            ack_policy,
            retention,
            durability_qos_class,
            byte_limit,
            message_limit,
            consumer_lag_threshold_ms,
            mirroring_policy,
            overload_behavior,
            entitlement: None,
        }
    }

    /// Check the policy is internally coherent and all fields are present.
    /// Every field is required: none may be missing, blank or empty.
    pub fn validate(&self) -> Result<()> {
        if self.partition_key.trim().is_empty() {
            return Err(Error::invalid(
                "stream policy has no partition key; it determines ordering",
            ));
        }
        if self.replication_factor == 0 {
            return Err(Error::invalid(
                "stream policy replication factor must be at least 1",
            ));
        }
        if self.retention.is_zero() {
            return Err(Error::invalid(
                "stream policy retention must be greater than zero",
            ));
        }
        if self.byte_limit == 0 {
            return Err(Error::invalid(
                "stream policy byte limit must be greater than zero",
            ));
        }
        if self.message_limit == 0 {
            return Err(Error::invalid(
                "stream policy message limit must be greater than zero",
            ));
        }
        if self.consumer_lag_threshold_ms == 0 {
            return Err(Error::invalid(
                "stream policy consumer lag threshold must be greater than zero",
            ));
        }
        Ok(())
    }

    /// Builder method to set entitlement.
    pub fn with_entitlement(mut self, entitlement: Entitlement) -> Self {
        self.entitlement = Some(entitlement);
        self
    }

    /// Builder method to set partition key.
    pub fn with_partition_key(mut self, key: impl Into<String>) -> Self {
        self.partition_key = key.into();
        self
    }

    /// Builder method to set replication factor.
    pub fn with_replication_factor(mut self, factor: u32) -> Self {
        self.replication_factor = factor;
        self
    }

    /// Builder method to set acknowledgment policy.
    pub fn with_ack_policy(mut self, policy: AckPolicy) -> Self {
        self.ack_policy = policy;
        self
    }

    /// Builder method to set retention duration.
    pub fn with_retention(mut self, duration: Duration) -> Self {
        self.retention = duration;
        self
    }

    /// Builder method to set QoS class.
    pub fn with_durability_qos_class(mut self, class: QoSClass) -> Self {
        self.durability_qos_class = class;
        self
    }

    /// Builder method to set byte limit.
    pub fn with_byte_limit(mut self, limit: u64) -> Self {
        self.byte_limit = limit;
        self
    }

    /// Builder method to set message limit.
    pub fn with_message_limit(mut self, limit: u64) -> Self {
        self.message_limit = limit;
        self
    }

    /// Builder method to set consumer lag threshold.
    pub fn with_consumer_lag_threshold_ms(mut self, ms: u32) -> Self {
        self.consumer_lag_threshold_ms = ms;
        self
    }

    /// Builder method to set mirroring policy.
    pub fn with_mirroring_policy(mut self, policy: MirroringPolicy) -> Self {
        self.mirroring_policy = policy;
        self
    }

    /// Builder method to set overload behavior.
    pub fn with_overload_behavior(mut self, behavior: OverloadBehavior) -> Self {
        self.overload_behavior = behavior;
        self
    }
}
