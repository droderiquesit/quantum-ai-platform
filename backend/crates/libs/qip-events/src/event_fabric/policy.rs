//! `StreamPolicy` and the quality-of-service classes for event fabric streams.
//!
//! See ADR 0100 §5 for QoS class definitions and §1 for the event fabric's
//! architecture. CONTRACT-037 lists the fields a `StreamPolicy` must carry;
//! FABRIC-057 requires every one of them to be an explicit declaration, never
//! a broker default, and FABRIC-049 adds the per-producer quotas and lag
//! limit. `StreamPolicy` has no `Default` and its only constructor,
//! [`StreamPolicy::new`], refuses a value that is present but wrong — a
//! replication factor besides one, a mirroring policy besides none, an
//! acknowledgement profile weaker than its class's floor — rather than a
//! value that is merely absent, which is [`crate::event_fabric::catalogue`]'s
//! job: it parses a committed catalogue's text form, where a field can be
//! missing from the document in a way no Rust struct field can be.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::retention::RetentionClass;

/// ADR 0100 §5's five durability classes, in the table's own order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QosClass {
    /// Capital grants, policy, halt, package announcements, signed ledger
    /// watermarks. Never dropped; the producer is refused and narrows.
    P0Control,
    /// Fills, cancels, settlement records, `ChainSpan` continuity records.
    /// Never dropped; the producer is refused and narrows.
    P1Outcomes,
    /// Every reflex journal entry. Throttled; an explicit `Gap` if shed.
    P2MarketJournal,
    /// Episodes, knowledge deltas. Backlog allowed.
    P3Research,
    /// Derived telemetry events. Sampled or shed first.
    P4Telemetry,
}

impl QosClass {
    pub const ALL: [Self; 5] = [
        Self::P0Control,
        Self::P1Outcomes,
        Self::P2MarketJournal,
        Self::P3Research,
        Self::P4Telemetry,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::P0Control => "p0_control",
            Self::P1Outcomes => "p1_outcomes",
            Self::P2MarketJournal => "p2_market_journal",
            Self::P3Research => "p3_research",
            Self::P4Telemetry => "p4_telemetry",
        }
    }

    /// Whether a topic ADR 0089 marks lossy-tolerable
    /// (`RetentionClass::is_replaceable`, e.g. raw ticks) may be admitted to
    /// a stream of this class.
    ///
    /// Only research and telemetry may (M18): a control, outcome or journal
    /// stream that admitted one would be a firehose reaching a stream ADR
    /// 0100 §5 says is never dropped, for records the platform has already
    /// decided are cheap to lose.
    pub const fn admits_lossy_tolerable_topics(&self) -> bool {
        matches!(self, Self::P3Research | Self::P4Telemetry)
    }

    /// The weakest [`AckProfile`] this class tolerates (FABRIC-067).
    ///
    /// P0 and P1 are "never dropped" in ADR 0100 §5's own words, which only a
    /// quorum acknowledgement can back without the class becoming a label
    /// rather than a guarantee. P2 and P3 still commit to the leader before
    /// acknowledging. P4 may run with none at all — FABRIC-041 permits
    /// at-most-once delivery for telemetry alone.
    pub const fn ack_floor(&self) -> AckProfile {
        match self {
            Self::P0Control | Self::P1Outcomes => AckProfile::Quorum,
            Self::P2MarketJournal | Self::P3Research => AckProfile::LeaderOnly,
            Self::P4Telemetry => AckProfile::None,
        }
    }
}

/// CONTRACT-044/FABRIC-067: the acknowledgement a producer requires before it
/// considers a record durable.
///
/// Ordered weakest first, so a stream's floor can be compared with `<`
/// without a hand-written table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AckProfile {
    None,
    LeaderOnly,
    Quorum,
}

/// FABRIC-057's declared ordering semantics.
///
/// One variant today, because ADR 0100 §4 claims no ordering but
/// per-partition. Declaring it is still mandatory — leaving it out is
/// exactly the broker default FABRIC-057 refuses — and a second ordering
/// mode gets a second variant here when the platform gains one, rather than
/// this field being a `bool` that would need renaming to hold it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ordering {
    PerPartition,
}

/// ADR 0100 §3: replication and mirroring are both refused down to a single
/// value today, because consensus (C2) and a mirror target (C8) are both
/// unbuilt. The rejected variants still exist so a catalogue can *state*
/// what it wants and be refused by name, rather than the concept having no
/// representation to refuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mirroring {
    None,
    Selective,
    Full,
}

/// FABRIC-049/057's declared overload behaviour, matching ADR 0100 §5's
/// "overload behaviour" column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverloadPolicy {
    /// P0/P1: never dropped; the producer is refused and narrows.
    RefuseProducer,
    /// P2: throttled; an explicit `Gap` if shed.
    ThrottleWithGap,
    /// P3: backlog allowed.
    AllowBacklog,
    /// P4: sampled or shed first.
    SampleOrShed,
}

/// The closed set of usage words, matching
/// `qip_contracts::governance::Usage::as_str` exactly.
///
/// Duplicated as text rather than imported: `qip-events` depends on
/// `qip-core` alone, and naming `qip_contracts::governance::Usage` here would
/// add an unowned manifest edge for one field. A divergence between the two
/// lists has to be caught by a person rather than the compiler, which is the
/// price of not taking the dependency — see [`Entitlement`]'s own doc
/// comment.
const USAGE_WORDS: [&str; 4] = ["research", "derive", "trade", "redistribute"];

/// CONTRACT-037's `entitlement` field, carried as validated text rather than
/// `qip_contracts::governance::Entitlement`.
///
/// `qip-events` is a lib beneath `qip-contracts` in the dependency order
/// (`libs` do not depend on one another's domain types) and its own
/// `Cargo.toml` is not touched by this packet, so the catalogue cannot name
/// that type. Carrying the same two facts — a dataset id and a usage word
/// from the same closed set `Usage::as_str` produces — as validated `String`s
/// keeps the two agreeing in fact without agreeing in type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entitlement {
    dataset: String,
    usage: String,
}

impl Entitlement {
    /// Refuses an empty dataset id and a usage word outside
    /// [`USAGE_WORDS`] — a typo here would otherwise silently license
    /// nothing, or license everything, depending on how the broker later
    /// chose to read a word it did not recognise.
    pub fn new(dataset: impl Into<String>, usage: impl Into<String>) -> Result<Self> {
        let dataset = dataset.into();
        let usage = usage.into();
        if dataset.trim().is_empty() {
            return Err(Error::invalid(
                "an entitlement must name a non-empty dataset",
            ));
        }
        if !USAGE_WORDS.contains(&usage.as_str()) {
            return Err(Error::invalid(format!(
                "entitlement usage '{usage}' is not one of {USAGE_WORDS:?}"
            )));
        }
        Ok(Self { dataset, usage })
    }

    pub fn dataset(&self) -> &str {
        &self.dataset
    }

    pub fn usage(&self) -> &str {
        &self.usage
    }
}

/// Every value a [`StreamPolicy`] needs, gathered so [`StreamPolicy::new`]
/// takes one argument that names its fields rather than fourteen positional
/// ones two of which would eventually be swapped.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamPolicySpec {
    pub qos_class: QosClass,
    pub partition_key: String,
    pub ordering: Ordering,
    pub retention: RetentionClass,
    pub replication_factor: u32,
    pub mirroring: Mirroring,
    pub overload_policy: OverloadPolicy,
    pub ack_profile: AckProfile,
    pub byte_quota_per_producer: u64,
    pub message_quota_per_producer: u64,
    pub lag_limit: u64,
    pub entitlement: Entitlement,
    pub seal_age_ms: u64,
    pub peak_bytes_per_second: u64,
}

/// CONTRACT-037's stream policy.
///
/// Every field is mandatory and there is no `Default`. [`StreamPolicy::new`]
/// is the only constructor, and it is where a value that is present but
/// wrong is refused: a replication factor besides one (ADR 0100 §3, C2), a
/// mirroring policy besides none (C8), and an acknowledgement profile weaker
/// than the class's floor (FABRIC-067). `seal_age_ms` and
/// `peak_bytes_per_second` are the single source of seal cadence and peak
/// byte rate for the broker (SLICE-38) and the node's sizing refusal
/// (SLICE-36); neither has a fallback value here or anywhere else.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamPolicy {
    qos_class: QosClass,
    partition_key: String,
    ordering: Ordering,
    retention: RetentionClass,
    replication_factor: u32,
    mirroring: Mirroring,
    overload_policy: OverloadPolicy,
    ack_profile: AckProfile,
    byte_quota_per_producer: u64,
    message_quota_per_producer: u64,
    lag_limit: u64,
    entitlement: Entitlement,
    seal_age_ms: u64,
    peak_bytes_per_second: u64,
}

impl StreamPolicy {
    pub fn new(spec: StreamPolicySpec) -> Result<Self> {
        if spec.partition_key.trim().is_empty() {
            return Err(Error::invalid(
                "a stream policy must name a non-empty partition key",
            ));
        }
        // ADR 0100 §3: in-tree replication with in-sync replicas was
        // proposed and rejected as "ADR 0009's mistake wearing different
        // clothes"; replication waits for C2's consensus record, so RF1 is
        // the only value this build can honestly run.
        if spec.replication_factor != 1 {
            return Err(Error::denied(format!(
                "replication factor {} is refused (C2): the fabric has no consensus record yet and can only run at replication factor one",
                spec.replication_factor
            )));
        }
        // ADR 0100 §1's component table: "fabric-mirror ... not built:
        // Mirror: BLOCKED(C8)."
        if spec.mirroring != Mirroring::None {
            return Err(Error::denied(format!(
                "mirroring policy {:?} is refused (C8): fabric-mirror is not built",
                spec.mirroring
            )));
        }
        let floor = spec.qos_class.ack_floor();
        if spec.ack_profile < floor {
            return Err(Error::denied(format!(
                "ack profile {:?} is weaker than {} requires ({floor:?})",
                spec.ack_profile,
                spec.qos_class.as_str()
            )));
        }
        Ok(Self {
            qos_class: spec.qos_class,
            partition_key: spec.partition_key,
            ordering: spec.ordering,
            retention: spec.retention,
            replication_factor: spec.replication_factor,
            mirroring: spec.mirroring,
            overload_policy: spec.overload_policy,
            ack_profile: spec.ack_profile,
            byte_quota_per_producer: spec.byte_quota_per_producer,
            message_quota_per_producer: spec.message_quota_per_producer,
            lag_limit: spec.lag_limit,
            entitlement: spec.entitlement,
            seal_age_ms: spec.seal_age_ms,
            peak_bytes_per_second: spec.peak_bytes_per_second,
        })
    }

    pub fn qos_class(&self) -> QosClass {
        self.qos_class
    }
    pub fn partition_key(&self) -> &str {
        &self.partition_key
    }
    pub fn ordering(&self) -> Ordering {
        self.ordering
    }
    pub fn retention(&self) -> RetentionClass {
        self.retention
    }
    pub fn replication_factor(&self) -> u32 {
        self.replication_factor
    }
    pub fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
    pub fn overload_policy(&self) -> OverloadPolicy {
        self.overload_policy
    }
    pub fn ack_profile(&self) -> AckProfile {
        self.ack_profile
    }
    pub fn byte_quota_per_producer(&self) -> u64 {
        self.byte_quota_per_producer
    }
    pub fn message_quota_per_producer(&self) -> u64 {
        self.message_quota_per_producer
    }
    pub fn lag_limit(&self) -> u64 {
        self.lag_limit
    }
    pub fn entitlement(&self) -> &Entitlement {
        &self.entitlement
    }
    pub fn seal_age_ms(&self) -> u64 {
        self.seal_age_ms
    }
    pub fn peak_bytes_per_second(&self) -> u64 {
        self.peak_bytes_per_second
    }
}
