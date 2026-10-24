//! Event Fabric infrastructure basics: FABRIC-006, FABRIC-007, FABRIC-008, FABRIC-009, FABRIC-010.
//!
//! Tests for fabric durability/replay, control distribution, regional infrastructure,
//! transport security (mutual TLS 1.3 over HTTP/1.1; ADR 0112, ADR 0156 4a), and RF3 replication across zones.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

#[path = "../../../libs/qip-transport/tests/support/certs.rs"]
mod certs;
mod event_fabric_support;

use event_fabric_support::{STREAM, batch, broker_module, broker_with, norm, spec};

fn norm_text(text: &str) -> String {
    text.lines().map(norm).collect::<Vec<_>>().join("\n")
}

/// FABRIC-006: every acknowledged record is retrievable in exact append order
/// after a restart, and the log replays to the same bytes. The previous test
/// pushed `FabricRecord`s into a `Vec` it built and counted them; this one
/// appends through the broker, drops it, reopens the same data directory and
/// reads back through `fetch`.
///
/// Mutation: cap `Broker::fetch`'s `high_watermark` at 50 — the reopened
/// broker serves nothing for offset 50 onward and the test fails there,
/// naming the offset; restored.
#[test]
fn fabric_006_fabric_records_are_durable_and_replayable() {
    use qip_core::{Clock, ManualClock, Timestamp};
    use qip_events::event_fabric::codec::{Batch, DecodeOutcome};
    use qip_events::event_fabric::policy::QosClass;
    use qip_events::event_fabric::policy::StreamPolicy;
    use qip_storage::segment::log::SegmentLogConfig;
    use qip_streaming::event_fabric::broker::Broker;
    use std::sync::Arc;

    let (broker, dir) = broker_with("fabric-006", 1, spec(QosClass::P1Outcomes));
    let mut acknowledged = Vec::new();
    for sequence in 0..100u64 {
        let id = format!("record-{sequence}");
        let produced = broker
            .produce_to(STREAM, 0, batch("venue", 1, sequence, &id))
            .expect("the broker answers")
            .expect("the batch is admitted");
        acknowledged.push((produced.base_offset(), id));
    }
    assert_eq!(
        acknowledged.len(),
        100,
        "premise: every batch was acknowledged"
    );
    assert!(
        acknowledged.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "premise: acknowledged offsets rise in append order"
    );
    drop(broker);

    // A new process: same directory, nothing carried over in memory.
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 4)));
    let reopened = Broker::open(dir, Arc::clone(&clock)).expect("the directory reopens");
    reopened
        .declare_stream(
            STREAM,
            1,
            StreamPolicy::new(spec(QosClass::P1Outcomes)).expect("a permitted policy"),
            SegmentLogConfig::new(clock).with_roll_after_bytes(10_000_000),
        )
        .expect("the stream is re-declared over its recovered log");

    let mut replayed = Vec::new();
    for (offset, _) in &acknowledged {
        let served = reopened
            .fetch(STREAM, 0, *offset, 1)
            .expect("an acknowledged offset is served");
        let bytes = qip_core::hash::from_hex(served.batches()).expect("hex");
        match Batch::decode(&bytes).expect("decodes") {
            DecodeOutcome::Complete(batch) => {
                assert_eq!(batch.base_offset, *offset);
                replayed.push(batch.records[0].event_id.clone());
            }
            DecodeOutcome::Torn => panic!("offset {offset} was served torn after a restart"),
        }
    }
    let expected: Vec<String> = acknowledged.into_iter().map(|(_, id)| id).collect();
    assert_eq!(replayed, expected, "replay is not the acknowledged order");
}

/// FABRIC-007 (R12-FABRIC-10195): the fabric distributes capital, risk and
/// strategy control to Reflex Cells, and every cell receives the updates for
/// its region and the global constraints that apply to it.
///
/// The earlier form built `ControlMessage` structs in the test and counted
/// them. This one produces control frames to a real P0 stream through the
/// broker, keyed by cell (the control stream's partition key, ADR 0100 §5),
/// fans a region's frame out to each cell in the region and a global frame to
/// every cell, then fetches each cell's partition back and asserts what it
/// holds: its region's frames, the global one, and nothing addressed to
/// another region. A frame that landed on the wrong cell's partition, or a
/// partition that lost one on the way back, fails.
///
/// Mutation: in `Broker::produce_to`, append to partition 0 whatever
/// partition the caller named. The cells' own partitions are empty and the
/// fetch fails at the first cell ("offset 0 was served torn"). Ran, failed,
/// restored.
#[test]
fn fabric_007_fabric_distributes_capital_risk_and_strategy_control_to_reflex_cells() {
    use qip_events::event_fabric::codec::{Batch, DecodeOutcome};
    use qip_events::event_fabric::policy::QosClass;
    use qip_streaming::event_fabric::partition::partition_for;
    use std::collections::{BTreeMap, BTreeSet};

    const PARTITIONS: u32 = 16;
    let (broker, _dir) = broker_with("fabric-007", PARTITIONS, spec(QosClass::P0Control));

    // (cell, region): two cells in one region, one in another.
    let cells = [
        ("cell-usw-1", "us-west1"),
        ("cell-usw-2", "us-west1"),
        ("cell-euw-1", "eu-west1"),
    ];
    let partition_of = |cell: &str| partition_for(cell, PARTITIONS).expect("a partition");
    let partitions: BTreeSet<u32> = cells.iter().map(|(cell, _)| partition_of(cell)).collect();
    assert_eq!(
        partitions.len(),
        cells.len(),
        "premise: the three cells route to three partitions, or a cell's partition could hold \
         another cell's frame legitimately"
    );

    // (frame id, the region it is addressed to, or None for global).
    let frames: [(&str, Option<&str>); 5] = [
        ("capital_grant|us-west1", Some("us-west1")),
        ("risk_limit|us-west1", Some("us-west1")),
        ("strategy_activation|us-west1", Some("us-west1")),
        ("risk_limit|eu-west1", Some("eu-west1")),
        ("halt|global", None),
    ];

    let mut sequence: BTreeMap<u32, u64> = BTreeMap::new();
    let mut expected: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for (id, region) in frames {
        for (cell, cell_region) in cells {
            if region.is_some_and(|region| region != cell_region) {
                continue;
            }
            let partition = partition_of(cell);
            let next = sequence.entry(partition).or_insert(0);
            broker
                .produce_to(STREAM, partition, batch("control-centre", 1, *next, id))
                .expect("the broker answers")
                .expect("a control frame is admitted");
            *next += 1;
            expected.entry(cell).or_default().insert(id.to_string());
        }
    }
    assert_eq!(
        expected["cell-usw-1"].len(),
        4,
        "premise: a us-west1 cell is addressed by three regional frames and the global one"
    );

    for (cell, _) in cells {
        let partition = partition_of(cell);
        let mut held = BTreeSet::new();
        for offset in 0..sequence[&partition] {
            let served = broker
                .fetch(STREAM, partition, offset, 1)
                .expect("an acknowledged offset is served");
            let bytes = qip_core::hash::from_hex(served.batches()).expect("hex");
            match Batch::decode(&bytes).expect("decodes") {
                DecodeOutcome::Complete(batch) => {
                    held.extend(batch.records.iter().map(|r| r.event_id.clone()));
                }
                DecodeOutcome::Torn => panic!("{cell}'s offset {offset} was served torn"),
            }
        }
        assert_eq!(
            held, expected[cell],
            "{cell}'s partition does not hold exactly its region's frames and the global one"
        );
    }
}

#[test]
fn fabric_008_each_region_runs_fabric_on_dedicated_broker_vms_across_three_zones() {
    let main = norm_text(&broker_module("main.tf"));
    assert!(
        main.contains("resource \"google_compute_instance_from_template\" \"event_fabric_broker\""),
        "premise: the module declares the broker machine"
    );
    let machine = main
        .split("resource \"google_compute_instance_from_template\"")
        .nth(1)
        .and_then(|rest| rest.split("resource \"google_dns_record_set\"").next())
        .expect("the machine resource is followed by the DNS records");
    assert!(
        machine.contains(
            "condition = length(distinct([for address in local.brokers : local.broker_zone[address]])) == length(local.brokers)"
        ),
        "two brokers may share a zone at plan: {machine}"
    );
    assert!(
        machine
            .contains("condition = startswith(local.broker_zone[each.value], \"${var.region}-\")"),
        "a broker may be planned outside its region: {machine}"
    );
    assert!(
        main.contains("broker_count = var.enabled ? 1 : 0"),
        "the default is one broker per region (RF1), not three"
    );
}

/// FABRIC-009 (R12-FABRIC-10195): fabric sessions use mutual TLS 1.3 over
/// HTTP/1.1 through `rustls`, both certificates verified, the peer identity
/// taken from the certificate; QUIC is admitted for a hop only on a committed
/// TCP+TLS against QUIC measurement (ADR 0112 §4 and Amendment 1).
///
/// The earlier form asserted a struct it built said "QUIC" and named a trust
/// anchor ("the event log's root") that no mechanism in the tree implements;
/// the trust anchor is the CA pool. This one binds a real `Server::bind_tls`
/// listener and drives it: a client holding a certificate its pool chains is
/// served first, over TLS 1.3 and an HTTP/1.1 status line, and is named by the
/// certificate; a client with no certificate, one from another authority, and
/// one the server does not admit never reach the handler. The no-QUIC fact
/// stays pinned by `fabric_036`'s gap-pin.
///
/// Mutation: make `Server::bind_tls` not require a client certificate (build
/// the verifier with `allow_unauthenticated`). The certificate-less client
/// then completes the handshake and is answered (403, from the server's
/// second layer, so the handler still does not run), and the assertion that
/// an anonymous client gets no answer fails. Ran, failed, restored.
#[test]
fn fabric_009_fabric_sessions_are_mutual_tls_over_http_one_and_a_client_without_a_certificate_never_reaches_a_handler()
 {
    use qip_transport::rustls::pki_types::ServerName;
    use qip_transport::rustls::{ClientConnection, ProtocolVersion, Stream};
    use qip_transport::server::{
        Handler, PEER_IDENTITY_HEADER, Request, Response, Server, ServerLimits,
    };
    use qip_transport::tls::{PeerNames, TlsServer, WorkloadIdentity, server_authenticated_client};
    use std::io::{Read, Write};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting(Arc<AtomicUsize>);
    impl Handler for Counting {
        fn handle(&self, request: &Request) -> Response {
            self.0.fetch_add(1, Ordering::SeqCst);
            Response::text(
                200,
                request
                    .header(PEER_IDENTITY_HEADER)
                    .unwrap_or("none")
                    .to_string(),
            )
        }
    }

    let ca = certs::authority("fabric pool");
    let server_leaf = certs::leaf(&ca, 2, "localhost");
    let broker_name = "edge-node.test.qip.internal";
    let client_leaf = certs::leaf(&ca, 3, broker_name);
    let load = |leaf: &certs::Minted, roots: &certs::Minted| {
        WorkloadIdentity::from_pem(
            leaf.certificate_pem().as_bytes(),
            leaf.key_pem().as_bytes(),
            roots.certificate_pem().as_bytes(),
        )
        .expect("minted material loads")
    };

    let calls = Arc::new(AtomicUsize::new(0));
    let server = Server::bind_tls(
        "127.0.0.1:0",
        Arc::new(Counting(Arc::clone(&calls))),
        ServerLimits::default(),
        TlsServer {
            config: load(&server_leaf, &ca)
                .server_config()
                .expect("server config"),
            peers: PeerNames::new([broker_name.to_string()]).expect("names"),
        },
    )
    .expect("binds an ephemeral port");
    assert!(
        server.is_mutual_tls(),
        "premise: the listener demands client certificates"
    );
    let address = server.local_address().expect("address");
    let serving = std::thread::spawn(move || {
        for _ in 0..4 {
            let _ = server.serve_once();
        }
    });

    // One session: TLS handshake, one HTTP/1.1 request, the protocol version
    // and status line read back. Errors surface where the handshake or the
    // first read fails, which for a refused client is the point.
    let session = |config: Arc<qip_transport::rustls::ClientConfig>| -> Result<(ProtocolVersion, String, String), String> {
        let mut tcp = std::net::TcpStream::connect(&address).map_err(|e| e.to_string())?;
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
        tcp.set_write_timeout(Some(std::time::Duration::from_secs(5))).ok();
        let name = ServerName::try_from("localhost").expect("a server name");
        let mut connection = ClientConnection::new(config, name).map_err(|e| e.to_string())?;
        let mut stream = Stream::new(&mut connection, &mut tcp);
        stream
            .write_all(b"GET /who HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .map_err(|e| e.to_string())?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer).map_err(|e| e.to_string())?;
        let version = connection.protocol_version().ok_or("no protocol version")?;
        let status = answer.lines().next().unwrap_or("").to_string();
        let body = answer.rsplit("\r\n\r\n").next().unwrap_or("").to_string();
        Ok((version, status, body))
    };

    // 1. A client with a certificate the pool chains and the server admits:
    //    served, over TLS 1.3, with an HTTP/1.1 status line, named by the
    //    certificate and not by anything it claimed.
    let (version, status, body) = session(load(&client_leaf, &ca).client_config().expect("config"))
        .expect("a certificate-bearing client is served");
    assert_eq!(version, ProtocolVersion::TLSv1_3);
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert_eq!(
        body, broker_name,
        "the peer identity is the certificate's name"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "premise: the handler ran for the good client"
    );

    // 2. No certificate: refused before the handler.
    let anonymous =
        session(server_authenticated_client(ca.certificate_pem().as_bytes()).expect("config"));
    assert!(
        anonymous.is_err(),
        "a client with no certificate was answered: {anonymous:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a client without a certificate reached the handler"
    );

    // 3. A certificate from another authority.
    let stranger_ca = certs::authority("someone else's pool");
    let stranger = certs::leaf(&stranger_ca, 9, broker_name);
    let foreign = session(
        WorkloadIdentity::from_pem(
            stranger.certificate_pem().as_bytes(),
            stranger.key_pem().as_bytes(),
            ca.certificate_pem().as_bytes(),
        )
        .expect("loads")
        .client_config()
        .expect("config"),
    );
    assert!(
        foreign.is_err(),
        "a foreign certificate was admitted: {foreign:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a foreign certificate reached the handler"
    );

    // 4. A certificate the pool chains but the server does not name.
    let unlisted = certs::leaf(&ca, 4, "ledgerd.test.qip.internal");
    let (_, status, _) = session(load(&unlisted, &ca).client_config().expect("config"))
        .expect("the refusal is an HTTP answer");
    assert!(status.starts_with("HTTP/1.1 403"), "{status}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "an unadmitted workload reached the handler"
    );

    serving.join().expect("the server thread");
}

#[test]
fn fabric_010_gap_a_durable_partition_cannot_be_declared_at_rf3() {
    // GAP PIN. FABRIC-010 asks for RF3 across three zones with a two-of-three
    // write quorum. The previous test built a `Partition` with three
    // `BrokerNode`s in three zones and asserted it had three. The production
    // decision is the opposite and is code: `StreamPolicy::new` refuses a
    // replication factor of three for every class (ADR 0100 §3, blocked on
    // C2), and the broker module creates at most one broker template. This
    // test asserts the refusal for the two durable classes the requirement is
    // about, so the gap is evidence and RF3 arriving is noticed.
    //
    // Mutation: delete the replication-factor refusal in `StreamPolicy::new` —
    // RF3 is admitted and the assertion fires.
    use qip_events::event_fabric::policy::{
        Entitlement, Mirroring, Ordering, OverloadPolicy, QosClass, StreamPolicy, StreamPolicySpec,
    };
    for class in [QosClass::P0Control, QosClass::P1Outcomes] {
        let spec = |replication_factor| StreamPolicySpec {
            qos_class: class,
            partition_key: "key".to_string(),
            ordering: Ordering::PerPartition,
            retention: qip_events::retention::RetentionClass::Irreplaceable,
            replication_factor,
            mirroring: Mirroring::None,
            overload_policy: OverloadPolicy::RefuseProducer,
            ack_profile: class.ack_floor(),
            byte_quota_per_producer: 1 << 20,
            message_quota_per_producer: 1_000,
            lag_limit: 10_000,
            entitlement: Entitlement::new("control", "trade").expect("valid"),
            seal_age_ms: 60_000,
            peak_bytes_per_second: 1 << 16,
        };
        assert!(
            StreamPolicy::new(spec(1)).is_ok(),
            "premise: RF1 is admitted"
        );
        let refused = StreamPolicy::new(spec(3)).expect_err("RF3 was admitted");
        assert!(refused.to_string().contains("(C2)"), "{refused}");
    }
}

#[test]
fn fabric_010_a_stream_is_admitted_at_rf3_only_for_p0_and_p1_acknowledged_managed_majority() {
    // FABRIC-010 / R12-FABRIC-10027. The pin above proves RF3 is refused for
    // an `archived` stream; it cannot tell a working managed path from no path
    // at all. This one drives the admission rule ADR 0111 §3 wrote: RF3 is a
    // claim only the Spanner-committed `managed_majority` acknowledgement
    // honours, only P0 and P1 may make it, and the factor must be exactly
    // three. Each refusal is checked for its own reason, because a stream that
    // is refused for the wrong reason (a bad overload policy, say) would make
    // "P2 cannot claim RF3" pass while admitting it.
    //
    // Mutation: in `StreamPolicy::new`, delete the `QosClass::P0Control |
    // QosClass::P1Outcomes` guard on a replicated ack profile — the P2 stream
    // is admitted at RF3 and the `expect_err` fires.
    use qip_events::event_fabric::policy::{
        AckProfile, Entitlement, Mirroring, Ordering, OverloadPolicy, QosClass, StreamPolicy,
        StreamPolicySpec,
    };
    let spec = |class: QosClass, replication_factor, ack_profile| StreamPolicySpec {
        qos_class: class,
        partition_key: "key".to_string(),
        ordering: Ordering::PerPartition,
        retention: qip_events::retention::RetentionClass::Irreplaceable,
        replication_factor,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile,
        byte_quota_per_producer: 1 << 20,
        message_quota_per_producer: 1_000,
        lag_limit: 10_000,
        entitlement: Entitlement::new("control", "trade").expect("valid"),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 16,
    };
    for class in [QosClass::P0Control, QosClass::P1Outcomes] {
        assert!(
            StreamPolicy::new(spec(class, 3, AckProfile::ManagedMajority)).is_ok(),
            "premise: {class:?} at RF3 managed_majority is the one admitted claim"
        );
        let local = StreamPolicy::new(spec(class, 3, AckProfile::Archived))
            .expect_err("RF3 on one disk was admitted");
        assert!(local.to_string().contains("(C2)"), "{local}");
        let wrong_factor = StreamPolicy::new(spec(class, 1, AckProfile::ManagedMajority))
            .expect_err("managed_majority at RF1 was admitted");
        assert!(
            wrong_factor.to_string().contains("managed_majority stream"),
            "{wrong_factor}"
        );
    }
    let p2 = StreamPolicy::new(spec(
        QosClass::P2MarketJournal,
        3,
        AckProfile::ManagedMajority,
    ))
    .expect_err("a P2 stream claimed the managed consensus");
    assert!(p2.to_string().contains("(ADR 0111 §2)"), "{p2}");
}
