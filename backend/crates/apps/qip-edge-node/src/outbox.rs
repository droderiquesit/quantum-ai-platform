//! The outbox's bounds, where its spool lives, and whether a fabric
//! partition receives what the spool holds.
//!
//! The bounds are constants, not variables, as every working-set bound on
//! this platform is: a dial on a bound is raised on the night the bound
//! fires, which is the night it is working, and the result is a node that
//! runs out of disk instead of narrowing. Changing one is a code change.
//!
//! The fabric endpoint is the one value read, through a lookup the
//! composition root hands in, so its refusals are testable without a process
//! environment.

use crate::event_fabric::control::CONTROL_STREAM;
use crate::event_fabric::drain::{FabricSink, JOURNAL_STREAM, OUTCOMES_STREAM};
use crate::scheduler::JournalBound;
use qip_core::error::{Error, Result};
use qip_core::{Clock, sha256_hex};
use qip_events::event_fabric::policy::QosClass;
use qip_storage::provider::StorageTarget;
use qip_storage::settings::StorageSettings;
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::consumer::{Consumer, ConsumerConfig};
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::transport::{HttpTransport, Timeouts};
use qip_transport::tls::LinkSecurity;
use qip_transport::{RetryPolicy, ThreadSleeper};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// The fabric endpoint, `https://host:port` over mutual TLS (ADR 0112), or
/// `http://host:port` on a `local` deployment that chose plaintext. Unset:
/// no fabric sink.
pub const FABRIC_URL_VARIABLE: &str = "QIP_FABRIC_URL";
/// The bearer token the fabric checks, directly or through `_FILE`.
pub const FABRIC_TOKEN_VARIABLE: &str = "QIP_FABRIC_TOKEN";
/// The bearer token this cell reads `control.local` under, directly or
/// through `_FILE`. Its own variable, not `QIP_FABRIC_TOKEN`: the broker
/// grants the produce identity and the consume identity separately
/// (SLICE-53), and one token for both would make a cell that can ship its
/// journal a cell that can read control, or the reverse. Unset: this cell
/// reads no control from the fabric and announces that it does not.
pub const FABRIC_CONTROL_TOKEN_VARIABLE: &str = "QIP_FABRIC_CONTROL_TOKEN";
/// This cell's partition on the reflex streams.
pub const FABRIC_PARTITION_VARIABLE: &str = "QIP_FABRIC_PARTITION";

/// The credit window of the control consumer's fetches. Control is a few
/// signed frames, not a data stream; the window is room for a backlog of
/// grants and halts in one answer, not a throughput figure.
pub const CONTROL_FETCH_CREDIT_BYTES: u32 = 256 * 1024;

/// The spool's budget: one gibibyte, room for hours of a quiet cell's
/// journal and minutes of a busy one's — the outage the producer-retained
/// spool is sized to absorb before it narrows.
pub const SPOOL_BUDGET_BYTES: u64 = 1 << 30;

/// Unshipped journal entries before new exposure halts. In a healthy node
/// the journal holds one turn's entries; this many is a writer or store that
/// has been refusing for a long time at any realistic decision rate.
pub const JOURNAL_MAX_UNSHIPPED: usize = 65_536;

/// The decision loop's period. A constant because the pass meter's
/// saturation, the poller's staleness bound and the mesh worker's cadence
/// are argued against it together.
pub const PASS_INTERVAL: Duration = Duration::from_secs(1);

/// The journal bound the node runs under.
pub fn journal_bound() -> Result<JournalBound> {
    JournalBound::new(JOURNAL_MAX_UNSHIPPED)
}

/// The spool's directory: `spool/` under a local storage root, else a
/// per-cell directory under the system's temporary directory, which does
/// not survive the machine — [`spool_survives_the_machine`] lets the
/// start-up banner say which.
pub fn spool_dir(cell: &str, storage: &StorageSettings) -> PathBuf {
    if spool_survives_the_machine(storage) {
        storage.root().join("spool")
    } else {
        std::env::temp_dir().join(format!("qip-edge-spool-{cell}"))
    }
}

/// Whether [`spool_dir`] is on the storage root rather than in a temporary
/// directory.
pub fn spool_survives_the_machine(storage: &StorageSettings) -> bool {
    matches!(
        storage.target(),
        StorageTarget::Engine | StorageTarget::File
    )
}

/// Read the fabric endpoint through `lookup`. `None` when none is set —
/// announced at start. A partition without an endpoint, or an endpoint that
/// is not a bare `http://host:port`, is refused.
pub fn read_fabric(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Option<FabricSettings>> {
    let set = |name: &str| lookup(name).filter(|value| !value.trim().is_empty());
    match set(FABRIC_URL_VARIABLE) {
        Some(url) => FabricSettings::read(url.trim(), lookup).map(Some),
        None if set(FABRIC_PARTITION_VARIABLE).is_some() => Err(Error::invalid(format!(
            "configuration: {FABRIC_PARTITION_VARIABLE} is set and {FABRIC_URL_VARIABLE} is \
             not; a partition nothing produces to reads as a destination. Set \
             {FABRIC_URL_VARIABLE} or unset the partition"
        ))),
        None => Ok(None),
    }
}

/// Where the fabric is and how this cell authenticates to it.
#[derive(Debug)]
pub struct FabricSettings {
    pub url: String,
    pub partition: u32,
    /// One token per producer: `BearerToken` is deliberately not `Clone`.
    outcomes_token: BearerToken,
    journal_token: BearerToken,
    /// The consume identity for `control.local`, taken by
    /// [`Self::control_consumer`].
    control_token: Option<BearerToken>,
}

impl FabricSettings {
    fn read(url: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        if !(url.starts_with("http://") || url.starts_with("https://"))
            || url.trim_end_matches('/') != url
            || url.matches('/').count() > 2
        {
            return Err(Error::invalid(format!(
                "configuration: {FABRIC_URL_VARIABLE}={url} must be a scheme and authority, \
                 `https://host:port` (or `http://` on a local deployment), with no path; the \
                 producer appends each route"
            )));
        }
        let partition = match lookup(FABRIC_PARTITION_VARIABLE).filter(|v| !v.trim().is_empty()) {
            Some(value) => value.trim().parse::<u32>().map_err(|_| {
                Error::invalid(format!(
                    "configuration: {FABRIC_PARTITION_VARIABLE}={value} is not a partition number"
                ))
            })?,
            None => 0,
        };
        let token = || {
            BearerToken::resolve(
                FABRIC_TOKEN_VARIABLE,
                lookup(FABRIC_TOKEN_VARIABLE),
                lookup(&format!(
                    "{FABRIC_TOKEN_VARIABLE}{}",
                    qip_core::secret::FILE_SUFFIX
                )),
            )
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))
        };
        Ok(Self {
            url: url.to_string(),
            partition,
            outcomes_token: token()?,
            journal_token: token()?,
            control_token: {
                let file = format!(
                    "{FABRIC_CONTROL_TOKEN_VARIABLE}{}",
                    qip_core::secret::FILE_SUFFIX
                );
                let (direct, path) = (
                    lookup(FABRIC_CONTROL_TOKEN_VARIABLE).filter(|v| !v.trim().is_empty()),
                    lookup(&file).filter(|v| !v.trim().is_empty()),
                );
                // Set means resolved and shaped, with no fallback: a control
                // token that is present and malformed is a misconfigured
                // cell, not an unconfigured one.
                if direct.is_none() && path.is_none() {
                    None
                } else {
                    Some(
                        BearerToken::resolve(FABRIC_CONTROL_TOKEN_VARIABLE, direct, path).map_err(
                            |error| Error::invalid(format!("configuration: {}", error.message())),
                        )?,
                    )
                }
            },
        })
    }

    /// The consumer of `control.local` on this cell's partition, when a
    /// control token is configured; `None` when it is not. Call before
    /// [`Self::sink`], which consumes the settings.
    ///
    /// Same scheme rule as the sink, for the same reason: an endpoint that
    /// disagrees with the link is refused, never upgraded or downgraded.
    pub fn control_consumer(
        &mut self,
        cell: &str,
        clock: Arc<dyn Clock>,
        link: &LinkSecurity,
    ) -> Result<Option<Consumer>> {
        let Some(token) = self.control_token.take() else {
            return Ok(None);
        };
        let authority = self.authority(link)?;
        let seed = u64::from_str_radix(sha256_hex(cell.as_bytes()).get(..16).unwrap_or("0"), 16)
            .unwrap_or(0);
        Consumer::new(ConsumerConfig {
            transport: Box::new(HttpTransport::over(link, &authority, token)?),
            stream: CONTROL_STREAM.to_string(),
            partition: self.partition,
            group_id: format!("control-{cell}"),
            retry_policy: RetryPolicy::default(),
            breaker_policy: BreakerPolicy::default(),
            clock,
            sleeper: Arc::new(ThreadSleeper),
            retry_seed: seed.wrapping_add(4),
            breaker_seed: seed.wrapping_add(5),
            timeouts: Timeouts::default(),
            fetch_credit_bytes: CONTROL_FETCH_CREDIT_BYTES,
        })
        .map(Some)
    }

    fn authority(&self, link: &LinkSecurity) -> Result<String> {
        let scheme = format!("{}://", link.scheme());
        self.url
            .strip_prefix(&scheme)
            .map(str::to_string)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "configuration: {FABRIC_URL_VARIABLE}={} and the fabric link is {}; the \
                     endpoint's scheme must be `{scheme}` for this link",
                    self.url,
                    link.describe()
                ))
            })
    }

    /// The sink: a P1 producer at the quorum acknowledgement its class
    /// requires, and a P2 producer at leader-only, each with this cell as
    /// its producer id so the broker fences a second incarnation of it.
    ///
    /// `link` is the security the composition root chose (ADR 0112). The
    /// endpoint's scheme must agree with it: an `http://` endpoint on a
    /// mutual-TLS link is refused rather than upgraded, because the
    /// configuration that wrote it believed the link was plaintext, and an
    /// `https://` endpoint on a plaintext link is refused rather than
    /// downgraded.
    pub fn sink(
        self,
        cell: &str,
        region: &str,
        clock: Arc<dyn Clock>,
        link: &LinkSecurity,
    ) -> Result<FabricSink> {
        let authority = self.authority(link)?;
        let seed = u64::from_str_radix(sha256_hex(cell.as_bytes()).get(..16).unwrap_or("0"), 16)
            .unwrap_or(0);
        let producer = |stream: &str, class: QosClass, token: BearerToken, offset: u64| {
            Producer::new(ProducerConfig {
                transport: Box::new(HttpTransport::over(link, &authority, token)?),
                stream: stream.to_string(),
                partition: self.partition,
                producer_id: cell.to_string(),
                qos_class: class,
                ack_profile: class.ack_floor(),
                retry_policy: RetryPolicy::default(),
                breaker_policy: BreakerPolicy::default(),
                clock: Arc::clone(&clock),
                sleeper: Arc::new(ThreadSleeper),
                retry_seed: seed.wrapping_add(offset),
                breaker_seed: seed.wrapping_add(offset.wrapping_add(1)),
                timeouts: Timeouts::default(),
            })
        };
        let outcomes = producer(
            OUTCOMES_STREAM,
            QosClass::P1Outcomes,
            self.outcomes_token,
            0,
        )?;
        let journal = producer(
            JOURNAL_STREAM,
            QosClass::P2MarketJournal,
            self.journal_token,
            2,
        )?;
        FabricSink::new(outcomes, journal, region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn an_unset_fabric_is_no_fabric_and_the_journal_bound_narrows_before_it_halts() {
        assert!(
            read_fabric(&lookup(&[]))
                .expect("unset is allowed")
                .is_none()
        );
        let bound = journal_bound().expect("the stated bound is valid");
        assert!(bound.narrow_at() > 0 && bound.narrow_at() < bound.exhaust_at());
    }

    #[test]
    fn a_fabric_partition_without_an_endpoint_is_refused() {
        assert!(read_fabric(&lookup(&[(FABRIC_PARTITION_VARIABLE, "3")])).is_err());
    }

    /// ADR 0112: the endpoint's scheme follows the link, never the other way.
    /// Mutation: drop the scheme check in `sink` (build the transport from
    /// the link alone) and the plaintext endpoint on a TLS link is quietly
    /// upgraded; this fails.
    #[test]
    fn a_fabric_endpoint_whose_scheme_disagrees_with_the_link_is_refused() {
        let fake_bearer = "obviously-fake-outbox-test-bearer-0123456789";
        let plaintext = read_fabric(&lookup(&[
            (FABRIC_URL_VARIABLE, "http://fabricd:7100"),
            (FABRIC_TOKEN_VARIABLE, fake_bearer),
        ]))
        .expect("a well-formed endpoint")
        .expect("configured");
        let clock: Arc<dyn Clock> = Arc::new(qip_core::ManualClock::new(
            qip_core::Timestamp::from_secs(1_800_000_000),
        ));
        // Premise: the same endpoint over the link it names builds a sink.
        assert!(
            plaintext
                .sink(
                    "london-1",
                    "eu-west",
                    Arc::clone(&clock),
                    &LinkSecurity::plaintext_for_tests()
                )
                .is_ok()
        );
        let tls_endpoint = read_fabric(&lookup(&[
            (FABRIC_URL_VARIABLE, "https://fabricd:7100"),
            (FABRIC_TOKEN_VARIABLE, fake_bearer),
        ]))
        .expect("https is accepted")
        .expect("configured");
        assert!(
            tls_endpoint
                .sink(
                    "london-1",
                    "eu-west",
                    clock,
                    &LinkSecurity::plaintext_for_tests()
                )
                .is_err(),
            "an https endpoint was downgraded onto a plaintext link"
        );
    }

    #[test]
    fn a_fabric_endpoint_with_a_path_is_refused_before_a_token_is_read() {
        let error = read_fabric(&lookup(&[(FABRIC_URL_VARIABLE, "http://fabricd:7100/v1")]))
            .expect_err("a path on the endpoint would double every route");
        assert!(error.message().contains("no path"));
    }
}
