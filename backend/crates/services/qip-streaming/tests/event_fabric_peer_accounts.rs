//! R12-SEC-8100: on a listener configured with a peer-account table the
//! certificate SAN, mapped to a GCP service account, is the caller's identity
//! on every route, and a bearer token authorises nothing there. Before this a
//! token stolen from a mounted file was a producer on every non-admin route
//! even over mutual TLS.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use qip_core::{Clock, SystemClock, sha256_hex};
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::policy::StreamPolicy;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::peer_accounts::PeerAccounts;
use qip_streaming::event_fabric::service::{SegmentConfigFor, Service, TopiclessStreams};
use qip_transport::event_fabric::auth::IdentityTable;
use qip_transport::event_fabric::protocol::{ProducerInitRequest, Request, Route, encode_request};

const STREAM: &str = "telemetry.topicless";
const AGENT: &str = "agent:telemetry-1";
const TOKEN: &str = "fixture-token-for-the-agent-000000000000000000000000";
const SAN: &str = "telemetry-1.fabric.internal";
const ACCOUNT: &str = "telemetry-1@qip-dev.iam.gserviceaccount.com";

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

fn service(accounts: Option<PeerAccounts>) -> Service {
    let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let dir: PathBuf =
        std::env::temp_dir().join(format!("qip-peer-accounts-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let json = format!(
        r#"{{"streams":[{{"name":"{STREAM}","qos_class":"p4_telemetry","partition_key":"cell",
            "ordering":"per_partition","retention":"derived_state","replication_factor":1,
            "mirroring":"none","overload_policy":"sample_or_shed","ack_profile":"none",
            "byte_quota_per_producer":262144,"message_quota_per_producer":4096,
            "lag_limit":4096,"entitlement_dataset":"qip-test","entitlement_usage":"research",
            "seal_age_ms":5000,"peak_bytes_per_second":262144,"topics":[]}}],
            "grants":[{{"identity":"{AGENT}","stream":"{STREAM}","permission":"produce","key_scope":"any"}}]}}"#
    );
    let catalogue = Catalogue::parse(json.as_bytes()).unwrap();
    let identities =
        IdentityTable::parse(&format!("{AGENT} {}\n", sha256_hex(TOKEN.as_bytes()))).unwrap();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let broker = Arc::new(Broker::open(dir, clock.clone()).unwrap());
    let segments: SegmentConfigFor = Arc::new(move |policy: &StreamPolicy| {
        SegmentLogConfig::new(clock.clone()).with_archive_required(policy.archive_required())
    });
    let service = Service::new(
        broker,
        identities,
        &catalogue,
        1,
        segments,
        TopiclessStreams::AdmitNone,
    )
    .unwrap();
    match accounts {
        Some(accounts) => service.with_peer_accounts(accounts),
        None => service,
    }
}

fn table() -> PeerAccounts {
    PeerAccounts::new([(SAN.to_string(), ACCOUNT.to_string(), AGENT.to_string())]).unwrap()
}

fn init_body() -> Vec<u8> {
    encode_request(&Request::ProducerInit(ProducerInitRequest {
        stream: STREAM.to_string(),
        partition: 0,
        producer_id: AGENT.to_string(),
    }))
    .unwrap()
}

fn status(service: &Service, bearer: bool, peer: Option<&str>) -> u16 {
    let header = format!("Bearer {TOKEN}");
    service
        .handle(
            bearer.then_some(header.as_str()),
            peer,
            Route::ProducerInit.path(),
            &init_body(),
        )
        .status
}

#[test]
fn a_certificate_mapped_to_a_service_account_is_authorised_with_no_token() {
    // Premise: the same request is refused with neither credential, so the
    // 200 below is the mapping's doing and not an open route.
    assert_eq!(status(&service(None), false, None), 401);
    assert_eq!(status(&service(Some(table())), false, Some(SAN)), 200);
}

#[test]
fn a_valid_bearer_token_authorises_nothing_where_certificates_are_required() {
    // Premise: the token is genuine — without a table it is accepted.
    assert_eq!(status(&service(None), true, None), 200);
    let strict = service(Some(table()));
    assert_eq!(status(&strict, true, None), 403);
    assert_eq!(status(&strict, true, Some("other.fabric.internal")), 403);
}

#[test]
fn the_table_refuses_a_malformed_account_a_duplicate_san_and_a_shared_identity() {
    let row = |san: &str, account: &str, identity: &str| {
        (san.to_string(), account.to_string(), identity.to_string())
    };
    assert!(PeerAccounts::new([row("a", "a@gmail.com", "x")]).is_err());
    assert!(
        PeerAccounts::new([row("a", "a@a-long-host-that-is-not-iam.example.com", "x")]).is_err()
    );
    assert!(PeerAccounts::new([row("a", ACCOUNT, "x"), row("a", ACCOUNT, "y")]).is_err());
    assert!(PeerAccounts::new([row("a", ACCOUNT, "x"), row("b", ACCOUNT, "x")]).is_err());
    assert!(PeerAccounts::new(Vec::<(String, String, String)>::new()).is_err());
}
