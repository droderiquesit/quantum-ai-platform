//! Stream catalogue: the committed stream declarations and the grant schema.
//!
//! See ADR 0100 §5 for the catalogue's role in fixing QoS classes (defined in
//! [`super::policy`], not here) and §1 for the event fabric's architecture.
//!
//! [`Catalogue::parse`] reads a committed catalogue document — bytes the
//! caller already read, so this stays a pure lib function with no I/O of its
//! own — and refuses, by name, everything FABRIC-057 and CONTRACT-037
//! require a stream to declare: a missing field, a replication factor besides
//! one (C2), a mirroring policy besides none (C8), an acknowledgement
//! profile weaker than the stream's class, and a stream admitting a topic
//! ADR 0089 marks lossy-tolerable outside the two classes allowed to lose
//! one. It also parses and validates the [`Grant`] schema that
//! `qip-streaming` (SLICE-35) and `qip-transport` (SLICE-53) read: this is
//! the one place that schema is defined, because the red team found the
//! committed catalogue once wrote grant fields the broker read with no
//! shared type between them.

use std::collections::BTreeMap;

use qip_core::error::{Error, Result};
use serde::Deserialize;

use crate::retention::RetentionClass;
use crate::topic::Topic;

use super::policy::{
    AckProfile, Entitlement, Mirroring, Ordering, OverloadPolicy, QosClass, StreamPolicy,
    StreamPolicySpec,
};

/// Prefixes reserved for control surfaces the platform already names
/// elsewhere: an autonomy ceiling change, the live-trading toggle, and the
/// ceiling a limit is measured against. A stream in one of these namespaces
/// could otherwise be mistaken — by an operator reading a stream list, or by
/// a future consumer pattern-matching on names — for the control itself.
///
/// Matched with the trailing delimiter (`"live."`, not `"live"`), so a
/// stream about something else that merely starts with the same letters —
/// `livestock.x` — is not caught by a prefix check that forgot where the
/// reserved word ends and the rest of the name begins.
const RESERVED_STREAM_PREFIXES: [&str; 3] = ["autonomy.", "live.", "ceiling."];

fn validate_stream_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::invalid("a stream must be named"));
    }
    if let Some(prefix) = RESERVED_STREAM_PREFIXES
        .iter()
        .find(|prefix| name.starts_with(*prefix))
    {
        return Err(Error::denied(format!(
            "stream name '{name}' begins with the reserved prefix '{prefix}'"
        )));
    }
    Ok(())
}

/// One declared stream: its policy, and the topics it carries.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamDeclaration {
    pub name: String,
    pub policy: StreamPolicy,
    pub topics: Vec<Topic>,
}

/// The permission a [`Grant`] confers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Produce,
    Consume,
    Admin,
}

impl Permission {
    fn parse(raw: &str) -> Result<Self> {
        match raw {
            "produce" => Ok(Self::Produce),
            "consume" => Ok(Self::Consume),
            "admin" => Ok(Self::Admin),
            other => Err(Error::invalid(format!(
                "unknown grant permission '{other}'"
            ))),
        }
    }
}

/// Which key an identity may act under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyScope {
    OwnKey,
    Any,
}

impl KeyScope {
    fn parse(raw: &str) -> Result<Self> {
        match raw {
            "own_key" => Ok(Self::OwnKey),
            "any" => Ok(Self::Any),
            other => Err(Error::invalid(format!("unknown grant key scope '{other}'"))),
        }
    }
}

/// The grant schema, defined once here and read by `qip-streaming` (SLICE-35)
/// and `qip-transport` (SLICE-53): an identity's permission on a declared
/// stream, and the key scope it may act under.
#[derive(Clone, Debug, PartialEq)]
pub struct Grant {
    pub identity: String,
    pub stream: String,
    pub permission: Permission,
    pub key_scope: KeyScope,
}

/// A catalogue document as committed: streams and their policies, and the
/// grants issued against them.
#[derive(Clone, Debug, PartialEq)]
pub struct Catalogue {
    streams: BTreeMap<String, StreamDeclaration>,
    grants: Vec<Grant>,
}

#[derive(Deserialize)]
struct RawCatalogue {
    streams: Vec<RawStream>,
    #[serde(default)]
    grants: Vec<RawGrant>,
}

/// The catalogue's wire shape for one stream. Every field beside `name` and
/// `topics` is optional here and required by [`Catalogue::parse`] — that gap
/// is the whole mechanism: a Rust struct field cannot be "missing" the way a
/// JSON key can, so the presence check has to happen on this type rather
/// than on [`super::policy::StreamPolicy`] itself.
#[derive(Deserialize)]
struct RawStream {
    name: String,
    qos_class: Option<String>,
    partition_key: Option<String>,
    ordering: Option<String>,
    retention: Option<String>,
    replication_factor: Option<u32>,
    mirroring: Option<String>,
    overload_policy: Option<String>,
    ack_profile: Option<String>,
    byte_quota_per_producer: Option<u64>,
    message_quota_per_producer: Option<u64>,
    lag_limit: Option<u64>,
    entitlement_dataset: Option<String>,
    entitlement_usage: Option<String>,
    seal_age_ms: Option<u64>,
    peak_bytes_per_second: Option<u64>,
    #[serde(default)]
    topics: Vec<String>,
}

#[derive(Deserialize)]
struct RawGrant {
    identity: Option<String>,
    stream: Option<String>,
    permission: Option<String>,
    key_scope: Option<String>,
}

fn required<T>(value: Option<T>, stream: &str, field: &str) -> Result<T> {
    value.ok_or_else(|| Error::invalid(format!("stream '{stream}' is missing '{field}'")))
}

fn parse_qos_class(raw: &str) -> Result<QosClass> {
    QosClass::ALL
        .into_iter()
        .find(|class| class.as_str() == raw)
        .ok_or_else(|| Error::invalid(format!("unknown qos class '{raw}'")))
}

fn parse_retention(raw: &str) -> Result<RetentionClass> {
    RetentionClass::ALL
        .into_iter()
        .find(|class| class.as_str() == raw)
        .ok_or_else(|| Error::invalid(format!("unknown retention class '{raw}'")))
}

fn parse_mirroring(raw: &str) -> Result<Mirroring> {
    match raw {
        "none" => Ok(Mirroring::None),
        "selective" => Ok(Mirroring::Selective),
        "full" => Ok(Mirroring::Full),
        other => Err(Error::invalid(format!(
            "unknown mirroring policy '{other}'"
        ))),
    }
}

fn parse_ordering(raw: &str) -> Result<Ordering> {
    match raw {
        "per_partition" => Ok(Ordering::PerPartition),
        other => Err(Error::invalid(format!(
            "unknown ordering semantics '{other}'"
        ))),
    }
}

fn parse_overload_policy(raw: &str) -> Result<OverloadPolicy> {
    match raw {
        "refuse_producer" => Ok(OverloadPolicy::RefuseProducer),
        "throttle_with_gap" => Ok(OverloadPolicy::ThrottleWithGap),
        "allow_backlog" => Ok(OverloadPolicy::AllowBacklog),
        "sample_or_shed" => Ok(OverloadPolicy::SampleOrShed),
        other => Err(Error::invalid(format!("unknown overload policy '{other}'"))),
    }
}

fn parse_ack_profile(raw: &str) -> Result<AckProfile> {
    match raw {
        "none" => Ok(AckProfile::None),
        "leader_only" => Ok(AckProfile::LeaderOnly),
        "quorum" => Ok(AckProfile::Quorum),
        other => Err(Error::invalid(format!("unknown ack profile '{other}'"))),
    }
}

impl Catalogue {
    /// Parse a committed catalogue document.
    ///
    /// Pure: `bytes` is whatever the caller already read, so this function
    /// performs no I/O of its own.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let raw: RawCatalogue = serde_json::from_slice(bytes)?;
        let mut streams = BTreeMap::new();
        for raw_stream in raw.streams {
            let declaration = Self::declaration_from_raw(raw_stream)?;
            streams.insert(declaration.name.clone(), declaration);
        }
        let mut grants = Vec::with_capacity(raw.grants.len());
        for raw_grant in raw.grants {
            grants.push(Self::grant_from_raw(raw_grant, &streams)?);
        }
        Ok(Self { streams, grants })
    }

    fn declaration_from_raw(raw: RawStream) -> Result<StreamDeclaration> {
        let name = raw.name;
        validate_stream_name(&name)?;

        let qos_class = parse_qos_class(&required(raw.qos_class, &name, "qos_class")?)?;
        let partition_key = required(raw.partition_key, &name, "partition_key")?;
        let ordering = parse_ordering(&required(raw.ordering, &name, "ordering")?)?;
        let retention = parse_retention(&required(raw.retention, &name, "retention")?)?;
        let replication_factor = required(raw.replication_factor, &name, "replication_factor")?;
        let mirroring = parse_mirroring(&required(raw.mirroring, &name, "mirroring")?)?;
        let overload_policy =
            parse_overload_policy(&required(raw.overload_policy, &name, "overload_policy")?)?;
        let ack_profile = parse_ack_profile(&required(raw.ack_profile, &name, "ack_profile")?)?;
        let byte_quota_per_producer = required(
            raw.byte_quota_per_producer,
            &name,
            "byte_quota_per_producer",
        )?;
        let message_quota_per_producer = required(
            raw.message_quota_per_producer,
            &name,
            "message_quota_per_producer",
        )?;
        let lag_limit = required(raw.lag_limit, &name, "lag_limit")?;
        let entitlement_dataset = required(raw.entitlement_dataset, &name, "entitlement_dataset")?;
        let entitlement_usage = required(raw.entitlement_usage, &name, "entitlement_usage")?;
        let entitlement = Entitlement::new(entitlement_dataset, entitlement_usage)?;
        let seal_age_ms = required(raw.seal_age_ms, &name, "seal_age_ms")?;
        let peak_bytes_per_second =
            required(raw.peak_bytes_per_second, &name, "peak_bytes_per_second")?;

        let policy = StreamPolicy::new(StreamPolicySpec {
            qos_class,
            partition_key,
            ordering,
            retention,
            replication_factor,
            mirroring,
            overload_policy,
            ack_profile,
            byte_quota_per_producer,
            message_quota_per_producer,
            lag_limit,
            entitlement,
            seal_age_ms,
            peak_bytes_per_second,
        })?;

        let mut topics = Vec::with_capacity(raw.topics.len());
        for topic_name in &raw.topics {
            let topic = Topic::from_name(topic_name).ok_or_else(|| {
                Error::invalid(format!(
                    "stream '{name}' names unknown topic '{topic_name}'"
                ))
            })?;
            // ADR 0089 / M18: raw ticks and the rest of ADR 0089's
            // `Transient` class stay lossy-tolerable and never reach a
            // stream that ADR 0100 §5 says is never dropped or merely
            // throttled with a gap on loss.
            if !qos_class.admits_lossy_tolerable_topics() && topic.is_lossy_tolerable() {
                return Err(Error::denied(format!(
                    "stream '{name}' is {} and cannot admit '{topic_name}', which ADR 0089 marks lossy-tolerable",
                    qos_class.as_str()
                )));
            }
            topics.push(topic);
        }

        Ok(StreamDeclaration {
            name,
            policy,
            topics,
        })
    }

    fn grant_from_raw(
        raw: RawGrant,
        streams: &BTreeMap<String, StreamDeclaration>,
    ) -> Result<Grant> {
        let identity = raw
            .identity
            .ok_or_else(|| Error::invalid("a grant must name an identity"))?;
        if identity.trim().is_empty() {
            return Err(Error::invalid("a grant must name a non-empty identity"));
        }
        let stream = raw
            .stream
            .ok_or_else(|| Error::invalid("a grant must name a stream"))?;
        let declaration = streams
            .get(&stream)
            .ok_or_else(|| Error::invalid(format!("grant names undeclared stream '{stream}'")))?;
        let permission_raw = raw
            .permission
            .ok_or_else(|| Error::invalid("a grant must name a permission"))?;
        let permission = Permission::parse(&permission_raw)?;
        let key_scope_raw = raw
            .key_scope
            .ok_or_else(|| Error::invalid("a grant must name a key scope"))?;
        let key_scope = KeyScope::parse(&key_scope_raw)?;

        // A reflex cell may only ever hold its own key. A cell scoped to
        // `any` could read or forge another cell's partition, which is
        // exactly the key-scoped produce ACL ADR 0100 §7 exists to prevent —
        // whatever permission the grant names.
        if identity.starts_with("reflex:") && key_scope != KeyScope::OwnKey {
            return Err(Error::denied(format!(
                "'{identity}' is a cell identity and must be scoped to its own key, not '{key_scope_raw}'"
            )));
        }

        // A P0 control stream carries capital grants, policy and halts.
        // ADR 0100 §7's HMAC only signs the watermark's bytes; it says
        // nothing about who was allowed to write them. If any identity but
        // the release controller could produce here, that signature would
        // be authenticating the wrong question.
        if permission == Permission::Produce
            && declaration.policy.qos_class() == QosClass::P0Control
            && identity != "release-controller"
        {
            return Err(Error::denied(format!(
                "only 'release-controller' may produce to the P0 stream '{stream}', not '{identity}'"
            )));
        }

        Ok(Grant {
            identity,
            stream,
            permission,
            key_scope,
        })
    }

    pub fn streams(&self) -> &BTreeMap<String, StreamDeclaration> {
        &self.streams
    }

    pub fn stream(&self, name: &str) -> Option<&StreamDeclaration> {
        self.streams.get(name)
    }

    pub fn grants(&self) -> &[Grant] {
        &self.grants
    }
}
