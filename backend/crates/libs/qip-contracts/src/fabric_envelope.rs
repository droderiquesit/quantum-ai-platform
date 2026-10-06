//! FabricEnvelope contract (CONTRACT-036): the one internal event model.
//!
//! Every asynchronous event the platform emits travels in this one envelope,
//! carrying all metadata needed for ordering, fencing, idempotency, replay, and
//! audit. Separating envelope from payload means every frame carries the same
//! guarantee set, independent of what the payload happens to be.

use qip_core::error::{Error, Result};
use qip_core::{EventId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Wire format version of FabricEnvelope
pub const FABRIC_ENVELOPE_VERSION: u32 = 1;

/// The one internal event model carrying metadata for ordering, durability, and audit
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FabricEnvelope {
    pub stream_namespace: String,
    pub schema_id: u32,
    pub schema_version: u32,
    pub region: String,
    pub partition: u32,
    pub ordering_key: String,
    pub producer_id: String,
    pub producer_epoch: u64,
    pub producer_sequence: u64,
    pub leader_epoch: u64,
    pub offset: u64,
    pub event_id: EventId,
    pub idempotency_key: String,
    pub source_time: Timestamp,
    pub receive_time: Timestamp,
    pub logical_time: i64,
    pub priority_class: QoSClass,
    pub trace_id: String,
    pub payload_hash: String,
    pub provenance: BTreeMap<String, String>,
    pub auth_context: AuthContext,
}

/// QoS/Priority classes for fabric topics and delivery guarantees
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QoSClass {
    P0CriticalControl,
    P1FinancialOutcomes,
    P2MarketJournal,
    P3DurableTelemetry,
    P4LossyTelemetry,
}

impl QoSClass {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::P0CriticalControl => "p0_critical_control",
            Self::P1FinancialOutcomes => "p1_financial_outcomes",
            Self::P2MarketJournal => "p2_market_journal",
            Self::P3DurableTelemetry => "p3_durable_telemetry",
            Self::P4LossyTelemetry => "p4_lossy_telemetry",
        }
    }

    pub const fn replication_factor(&self) -> u32 {
        match self {
            Self::P0CriticalControl | Self::P1FinancialOutcomes | Self::P2MarketJournal => 3,
            Self::P3DurableTelemetry => 2,
            Self::P4LossyTelemetry => 1,
        }
    }

    pub const fn requires_quorum_ack(&self) -> bool {
        matches!(
            self,
            Self::P0CriticalControl | Self::P1FinancialOutcomes | Self::P2MarketJournal
        )
    }
}

/// Authentication context
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthContext {
    pub workload_id: String,
    pub signature: String,
    pub trust_root_version: u32,
}

impl AuthContext {
    pub fn new(
        workload_id: impl Into<String>,
        signature: impl Into<String>,
        trust_root_version: u32,
    ) -> Self {
        Self {
            workload_id: workload_id.into(),
            signature: signature.into(),
            trust_root_version,
        }
    }

    pub fn is_signed(&self) -> bool {
        !self.signature.is_empty()
    }
}

impl FabricEnvelope {
    /// Build a fabric envelope with all mandatory fields
    pub fn new(
        stream_namespace: impl Into<String>,
        schema_id: u32,
        schema_version: u32,
        region: impl Into<String>,
        partition: u32,
        ordering_key: impl Into<String>,
        producer_id: impl Into<String>,
        producer_epoch: u64,
        producer_sequence: u64,
        leader_epoch: u64,
        offset: u64,
        event_id: EventId,
        idempotency_key: impl Into<String>,
        source_time: Timestamp,
        receive_time: Timestamp,
        logical_time: i64,
        priority_class: QoSClass,
        trace_id: impl Into<String>,
        payload_hash: impl Into<String>,
        auth_context: AuthContext,
    ) -> Result<Self> {
        if offset == u64::MAX {
            return Err(Error::invalid("envelope offset cannot be u64::MAX"));
        }

        let ordering_key = ordering_key.into();
        if ordering_key.is_empty() {
            return Err(Error::invalid("envelope ordering key cannot be empty"));
        }

        let stream_namespace = stream_namespace.into();
        if stream_namespace.is_empty() {
            return Err(Error::invalid("envelope stream namespace cannot be empty"));
        }

        Ok(Self {
            stream_namespace,
            schema_id,
            schema_version,
            region: region.into(),
            partition,
            ordering_key,
            producer_id: producer_id.into(),
            producer_epoch,
            producer_sequence,
            leader_epoch,
            offset,
            event_id,
            idempotency_key: idempotency_key.into(),
            source_time,
            receive_time,
            logical_time,
            priority_class,
            trace_id: trace_id.into(),
            payload_hash: payload_hash.into(),
            provenance: BTreeMap::new(),
            auth_context,
        })
    }

    pub fn with_provenance(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.provenance.insert(key.into(), value.into());
        self
    }

    pub fn validate(&self) -> Result<()> {
        if self.stream_namespace.is_empty() {
            return Err(Error::invalid("stream namespace is empty"));
        }
        if self.ordering_key.is_empty() {
            return Err(Error::invalid("ordering key is empty"));
        }
        if self.offset == u64::MAX {
            return Err(Error::invalid("offset is uninitialized"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qos_class_properties_are_consistent() {
        assert!(QoSClass::P0CriticalControl.requires_quorum_ack());
        assert_eq!(QoSClass::P0CriticalControl.replication_factor(), 3);

        assert!(!QoSClass::P4LossyTelemetry.requires_quorum_ack());
        assert_eq!(QoSClass::P4LossyTelemetry.replication_factor(), 1);
    }

    #[test]
    fn all_qos_classes_have_string_names() {
        for class in [
            QoSClass::P0CriticalControl,
            QoSClass::P1FinancialOutcomes,
            QoSClass::P2MarketJournal,
            QoSClass::P3DurableTelemetry,
            QoSClass::P4LossyTelemetry,
        ] {
            assert!(!class.as_str().is_empty());
        }
    }
}
