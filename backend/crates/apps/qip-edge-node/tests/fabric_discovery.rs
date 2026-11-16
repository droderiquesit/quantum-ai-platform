//! The edge node bootstraps the fabric endpoint from the broker's SRV record
//! and starts on the configured address when the resolver is unreachable
//! (R12-FABRIC-10016, ADR 0135).

#![allow(clippy::panic_in_result_fn, clippy::expect_used)]

use qip_core::{Clock, ManualClock, Timestamp};
use qip_edge_node::fabric_discovery::{
    ATTEMPTS_VARIABLE, DiscoveredEndpoint, FabricDiscovery, RESOLVER_VARIABLE, TIMEOUT_VARIABLE,
    ZONE_VARIABLE, record_source,
};
use qip_observability::metrics::{Labels, Metrics, names};
use qip_transport::dns::{AddressSource, fabric_srv_owner};
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Duration;

const ZONE: &str = "qip.internal";
const CONFIGURED: &str = "http://configured-broker.internal:7100";

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(Timestamp::from_secs(1_760_000_000)))
}

fn name(out: &mut Vec<u8>, value: &str) {
    for label in value.trim_end_matches('.').split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
}

/// One SRV record for the fabric owner, naming `target:port`.
fn answer(query: &[u8], target: &str, port: u16) -> Vec<u8> {
    let owner = fabric_srv_owner(ZONE);
    let mut out = Vec::new();
    out.extend_from_slice(&query[0..2]);
    out.extend_from_slice(&0x8180u16.to_be_bytes());
    for count in [1u16, 1, 0, 0] {
        out.extend_from_slice(&count.to_be_bytes());
    }
    name(&mut out, &owner);
    out.extend_from_slice(&33u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    name(&mut out, &owner);
    out.extend_from_slice(&33u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&300u32.to_be_bytes());
    let mut rdata = Vec::new();
    rdata.extend_from_slice(&0u16.to_be_bytes());
    rdata.extend_from_slice(&1u16.to_be_bytes());
    rdata.extend_from_slice(&port.to_be_bytes());
    name(&mut rdata, target);
    out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    out.extend_from_slice(&rdata);
    out
}

fn resolver_answering(target: &'static str, port: u16) -> std::net::SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").expect("bind");
    let address = socket.local_addr().expect("addr");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    std::thread::spawn(move || {
        let mut buffer = [0u8; 512];
        while let Ok((read, from)) = socket.recv_from(&mut buffer) {
            let _ = socket.send_to(&answer(&buffer[..read], target, port), from);
        }
    });
    address
}

fn discovery(resolver: std::net::SocketAddr) -> FabricDiscovery {
    let resolver = resolver.to_string();
    FabricDiscovery::read(&move |variable| match variable {
        RESOLVER_VARIABLE => Some(resolver.clone()),
        ZONE_VARIABLE => Some(ZONE.to_string()),
        TIMEOUT_VARIABLE => Some("100".to_string()),
        ATTEMPTS_VARIABLE => Some("1".to_string()),
        _ => None,
    })
    .expect("valid")
    .expect("configured")
}

/// Mutation: `resolve` returning the configured URL without asking fails the
/// first assertion; the record's authority replaced by the configured one
/// fails it the same way.
#[test]
fn the_endpoint_comes_from_the_srv_record_and_keeps_the_configured_scheme() {
    let found = discovery(resolver_answering("broker-7.fabric.internal", 7777))
        .resolve(CONFIGURED, clock())
        .expect("resolves");
    assert_eq!(
        found,
        DiscoveredEndpoint {
            url: "http://broker-7.fabric.internal:7777".to_string(),
            source: AddressSource::Srv,
            lookup_failure: None,
        }
    );
}

/// Mutation: propagating the lookup's error from `resolve` fails this test
/// at the `expect`; returning an empty authority list fails the URL assertion.
#[test]
fn an_unreachable_resolver_starts_the_node_on_the_configured_address() {
    // A port nothing listens on: bound to learn a free port, then dropped.
    let dead = UdpSocket::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr");
    let found = discovery(dead)
        .resolve(CONFIGURED, clock())
        .expect("DNS loss does not stop the node");
    assert_eq!(found.url, CONFIGURED);
    assert_eq!(found.source, AddressSource::Configured);
    assert!(
        found.lookup_failure.is_some(),
        "the refusal travels beside the rung"
    );
}

#[test]
fn a_partial_resolver_configuration_is_refused_and_none_is_no_discovery() {
    assert!(FabricDiscovery::read(&|_| None).expect("ok").is_none());
    for only in [
        RESOLVER_VARIABLE,
        ZONE_VARIABLE,
        TIMEOUT_VARIABLE,
        ATTEMPTS_VARIABLE,
    ] {
        let result = FabricDiscovery::read(&|variable| {
            (variable == only).then(|| "127.0.0.1:53".to_string())
        });
        assert!(result.is_err(), "{only} alone was accepted");
    }
}

#[test]
fn the_rung_in_force_is_one_and_the_others_zero_on_the_address_source_gauge() {
    let metrics = Metrics::new("qip-edge-node");
    record_source(&metrics, AddressSource::Cached);
    let snapshot = metrics.snapshot();
    let read = |source: &str| {
        let mut labels = Labels::new();
        labels.insert("source".to_string(), source.to_string());
        snapshot.gauge(names::FABRIC_ADDRESS_SOURCE, &labels)
    };
    assert_eq!(read("cached"), Some(1.0));
    assert_eq!(read("srv"), Some(0.0));
    assert_eq!(read("configured"), Some(0.0));
}
