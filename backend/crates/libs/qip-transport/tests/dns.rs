//! `qip_transport::dns` — the SRV bootstrap of ADR 0135.
//!
//! The parser is proven against hand-built wire vectors from RFC 1035 §4.1.4
//! (message compression) and RFC 2782 (the `_foobar._tcp` example), and
//! against every refusal the ADR names. The resolver is proven over a real
//! loopback UDP socket, and the fallback ladder against a manual clock.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use qip_core::Error;
use qip_core::time::{Duration, ManualClock, Timestamp};
use qip_transport::dns::{
    AddressSource, Bootstrap, FABRIC_SRV_SERVICE, SrvAnswer, SrvRecord, SrvResolver, decode_answer,
    encode_query, fabric_srv_owner, read_name,
};

const OWNER: &str = "_foobar._tcp.example.com";
const ID: u16 = 0xBEEF;

/// Appends `name` as uncompressed labels.
fn push_name(out: &mut Vec<u8>, name: &str) {
    let name = name.trim_end_matches('.');
    if name.is_empty() {
        out.push(0); // the root, "."
        return;
    }
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
}

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// A record header: owner (uncompressed), type, class, TTL, RDLENGTH, RDATA.
fn push_record(out: &mut Vec<u8>, owner: &str, rtype: u16, ttl: u32, rdata: &[u8]) {
    push_name(out, owner);
    push_u16(out, rtype);
    push_u16(out, 1);
    push_u32(out, ttl);
    push_u16(out, rdata.len() as u16);
    out.extend_from_slice(rdata);
}

fn srv_rdata(priority: u16, weight: u16, port: u16, target: &str) -> Vec<u8> {
    let mut rdata = Vec::new();
    push_u16(&mut rdata, priority);
    push_u16(&mut rdata, weight);
    push_u16(&mut rdata, port);
    push_name(&mut rdata, target);
    rdata
}

/// The header of a no-error response to a one-question query.
fn header(id: u16, flags: u16, ancount: u16, arcount: u16) -> Vec<u8> {
    let mut out = Vec::new();
    push_u16(&mut out, id);
    push_u16(&mut out, flags);
    push_u16(&mut out, 1);
    push_u16(&mut out, ancount);
    push_u16(&mut out, 0);
    push_u16(&mut out, arcount);
    out
}

fn question(out: &mut Vec<u8>, owner: &str, qtype: u16) {
    push_name(out, owner);
    push_u16(out, qtype);
    push_u16(out, 1);
}

/// RFC 2782's example, as a response: four SRV records in the answer
/// section in an order that is not priority order, and A records for two
/// of the targets in the additional section.
fn rfc_2782_answer(id: u16) -> Vec<u8> {
    let mut m = header(id, 0x8180, 4, 2);
    question(&mut m, OWNER, 33);
    push_record(
        &mut m,
        OWNER,
        33,
        3600,
        &srv_rdata(1, 0, 9, "sysadmins-box.example.com"),
    );
    push_record(
        &mut m,
        OWNER,
        33,
        300,
        &srv_rdata(0, 1, 9, "old-slow-box.example.com"),
    );
    push_record(
        &mut m,
        OWNER,
        33,
        3600,
        &srv_rdata(1, 0, 9, "server.example.com"),
    );
    push_record(
        &mut m,
        OWNER,
        33,
        3600,
        &srv_rdata(0, 3, 9, "new-fast-box.example.com"),
    );
    push_record(
        &mut m,
        "old-slow-box.example.com",
        1,
        3600,
        &[172, 30, 79, 10],
    );
    push_record(
        &mut m,
        "new-fast-box.example.com",
        1,
        3600,
        &[172, 30, 79, 11],
    );
    m
}

/// RFC 1035 §4.1.4's compression example laid out at its own offsets: the
/// name F.ISI.ARPA at 20, FOO.F.ISI.ARPA at 40 as one label and a pointer
/// to 20, ARPA at 64 as a pointer to 26, and the root at 92. Every pointer
/// points backward, which is what the decoder requires.
///
/// Mutation: in `read_name`, replace the backward check `target >= position`
/// with `target > position` — this still passes, but
/// `a_compression_pointer_that_points_forward_or_at_itself_is_refused` below
/// fails on the self-pointer. The two tests are a pair.
#[test]
fn the_rfc_1035_compression_example_decodes_to_its_four_names() {
    let mut m = vec![0u8; 100];
    let f_isi_arpa: &[u8] = b"\x01F\x03ISI\x04ARPA\x00";
    m[20..20 + f_isi_arpa.len()].copy_from_slice(f_isi_arpa);
    let foo: &[u8] = b"\x03FOO\xC0\x14";
    m[40..40 + foo.len()].copy_from_slice(foo);
    m[64..66].copy_from_slice(&[0xC0, 26]);
    m[92] = 0;

    assert_eq!(read_name(&m, 20).unwrap(), ("f.isi.arpa".to_string(), 32));
    assert_eq!(
        read_name(&m, 40).unwrap(),
        ("foo.f.isi.arpa".to_string(), 46)
    );
    assert_eq!(read_name(&m, 64).unwrap(), ("arpa".to_string(), 66));
    assert_eq!(read_name(&m, 92).unwrap(), (String::new(), 93));
}

/// A pointer may refer only to an earlier occurrence (RFC 1035 §4.1.4 says
/// "prior"). A forward pointer or one at itself is how an answer loops; a
/// decoder that followed either would spin or read garbage as a name.
///
/// Mutation: drop the `target >= position` check — the self-pointer case
/// recurses to the pointer cap and the message changes from "points
/// forward" to "loops", and the forward case decodes "later" as a name.
#[test]
fn a_compression_pointer_that_points_forward_or_at_itself_is_refused() {
    let mut forward = vec![0u8; 40];
    forward[12..14].copy_from_slice(&[0xC0, 20]);
    forward[20..27].copy_from_slice(b"\x05later\x00");
    let err = read_name(&forward, 12).unwrap_err();
    assert!(err.to_string().contains("points forward"), "{err}");

    let mut own = vec![0u8; 20];
    own[12..14].copy_from_slice(&[0xC0, 12]);
    let err = read_name(&own, 12).unwrap_err();
    assert!(err.to_string().contains("points forward"), "{err}");
}

/// RFC 1035 §2.3.4's limits. A label type with the top bits `01` or `10` is
/// neither plain nor a pointer (RFC 6891 reserves them), and a name past 253
/// characters is over what the protocol allows.
///
/// Mutation: raise `MAX_NAME` to 320 — the five-label name decodes.
/// Mutation: match `0x80` beside `0x00` as a plain label — the `0x80` case
/// decodes a garbage label instead of refusing.
#[test]
fn an_unknown_label_type_and_a_name_over_the_protocols_length_are_refused() {
    for first in [0x40u8, 0x80] {
        let odd_type = [first, 0x01, b'a', 0x00];
        let err = read_name(&odd_type, 0).unwrap_err();
        assert!(err.to_string().contains("label type"), "{err}");
    }

    let mut long = Vec::new();
    for _ in 0..5 {
        long.push(63);
        long.extend(std::iter::repeat_n(b'x', 63));
    }
    long.push(0);
    let err = read_name(&long, 0).unwrap_err();
    assert!(err.to_string().contains("over 253"), "{err}");
}

/// A label followed by a pointer back to that label passes the backward
/// check — the pointer does point to an earlier offset — and a reader that
/// only checked direction would cycle until some other limit stopped it,
/// reporting a length fault for what is a loop. The refusal names the loop.
///
/// Mutation: drop the `visited.contains(&target)` check — the error becomes
/// "over 253 characters" and the assertion on "loops" fails.
#[test]
fn a_compression_pointer_that_revisits_an_offset_is_refused_as_a_loop() {
    let mut m = vec![0u8; 20];
    m[12..14].copy_from_slice(b"\x01a");
    m[14..16].copy_from_slice(&[0xC0, 12]);
    let err = read_name(&m, 12).unwrap_err();
    assert!(err.to_string().contains("loops"), "{err}");
}

/// RFC 2782: a target of "." says the service is decidedly not available at
/// this domain. It is not an address, and an answer made only of it is a
/// refusal naming that meaning rather than an authority of `:port` that
/// replaces the last good answer. A "." record beside real ones is dropped
/// and the real ones kept.
///
/// Mutation: drop the `target.is_empty()` arm in `decode_answer` — the
/// first assertion gets an `Ok` with a `":9"` authority.
#[test]
fn a_target_of_dot_is_the_service_being_unavailable_and_never_an_authority() {
    let mut only_dot = header(ID, 0x8180, 1, 0);
    question(&mut only_dot, OWNER, 33);
    push_record(&mut only_dot, OWNER, 33, 60, &srv_rdata(0, 0, 9, "."));
    let err = decode_answer(ID, OWNER, &only_dot).unwrap_err();
    assert!(err.to_string().contains("decidedly not available"), "{err}");

    let mut mixed = header(ID, 0x8180, 2, 0);
    question(&mut mixed, OWNER, 33);
    push_record(&mut mixed, OWNER, 33, 60, &srv_rdata(0, 0, 9, "."));
    push_record(
        &mut mixed,
        OWNER,
        33,
        60,
        &srv_rdata(0, 0, 9, "b.example.com"),
    );
    let answer = decode_answer(ID, OWNER, &mixed).unwrap();
    assert_eq!(answer.authorities(), ["b.example.com:9"]);
}

/// Real resolvers compress record owners to a pointer at the question
/// (`0xC00C`) and may compress an SRV target; both decode through
/// `decode_answer`, not only through `read_name`.
///
/// Mutation: in `decode_answer`, read the owner with an uncompressed-only
/// reader (treat `0xC0` as a label type refusal) — this fails on the first
/// record.
#[test]
fn a_compressed_owner_and_a_compressed_target_decode_through_the_answer() {
    let mut m = header(ID, 0x8180, 1, 1);
    question(&mut m, OWNER, 33);
    // Answer: owner as a pointer to the question name at offset 12.
    m.extend_from_slice(&[0xC0, 12]);
    push_u16(&mut m, 33);
    push_u16(&mut m, 1);
    push_u32(&mut m, 60);
    // Target "box.example.com": label "box" then a pointer to "example.com"
    // inside the question name (offset 12 + 1 + 7 + 1 + 4 = 25).
    let mut rdata = Vec::new();
    push_u16(&mut rdata, 0);
    push_u16(&mut rdata, 0);
    push_u16(&mut rdata, 9);
    rdata.extend_from_slice(b"\x03box\xC0\x19");
    push_u16(&mut m, rdata.len() as u16);
    m.extend_from_slice(&rdata);
    // Additional: an A record for the target, owner compressed to the
    // target's own bytes inside the SRV rdata.
    let target_at = m.len() - 6;
    m.extend_from_slice(&[0xC0, target_at as u8]);
    push_u16(&mut m, 1);
    push_u16(&mut m, 1);
    push_u32(&mut m, 60);
    push_u16(&mut m, 4);
    m.extend_from_slice(&[10, 0, 0, 7]);

    let answer = decode_answer(ID, OWNER, &m).unwrap();
    assert_eq!(answer.records[0].target, "box.example.com");
    assert_eq!(answer.authorities(), ["10.0.0.7:9"]);
}

/// The RFC 2782 example answer decodes to its four records ordered by
/// priority ascending and weight descending, with the additional A records
/// attached to their targets and the shortest TTL kept; and `authorities`
/// expands a target to the addresses the answer carried and keeps a name
/// the answer did not resolve.
///
/// Mutation: sort by weight ascending — `new-fast-box` and `old-slow-box`
/// swap and the first assertion fails. Mutation: take the longest TTL —
/// the TTL assertion fails.
#[test]
fn the_rfc_2782_example_decodes_in_priority_then_weight_order_with_its_addresses() {
    let answer = decode_answer(ID, OWNER, &rfc_2782_answer(ID)).unwrap();
    let order: Vec<(u16, u16, &str)> = answer
        .records
        .iter()
        .map(|r| (r.priority, r.weight, r.target.as_str()))
        .collect();
    assert_eq!(
        order,
        [
            (0, 3, "new-fast-box.example.com"),
            (0, 1, "old-slow-box.example.com"),
            (1, 0, "server.example.com"),
            (1, 0, "sysadmins-box.example.com"),
        ]
    );
    assert_eq!(answer.ttl, StdDuration::from_secs(300));
    assert_eq!(
        answer.addresses.get("new-fast-box.example.com"),
        Some(&vec![Ipv4Addr::new(172, 30, 79, 11)])
    );
    assert_eq!(
        answer.authorities(),
        [
            "172.30.79.11:9",
            "172.30.79.10:9",
            "server.example.com:9",
            "sysadmins-box.example.com:9",
        ]
    );
}

/// The query this module builds is what the answer is checked against:
/// encode, then decode an answer to it, and the question matches. Case is
/// not significant in a name (RFC 1035 §2.3.3), so an upper-case question
/// in the answer still matches.
///
/// Mutation: in `read_name`, push the label without `to_ascii_lowercase` —
/// the upper-case question no longer matches and is refused.
#[test]
fn an_encoded_query_is_answered_by_a_question_that_differs_only_in_case() {
    let query = encode_query(ID, OWNER).unwrap();
    assert_eq!(&query[..2], &ID.to_be_bytes());
    assert_eq!(
        &query[2..4],
        &[0x01, 0x00],
        "recursion desired, nothing else"
    );
    assert_eq!(&query[4..6], &[0, 1], "one question");
    let mut expected = Vec::new();
    push_name(&mut expected, OWNER);
    push_u16(&mut expected, 33);
    push_u16(&mut expected, 1);
    assert_eq!(&query[12..], &expected[..]);

    let mut m = header(ID, 0x8180, 1, 0);
    question(&mut m, "_FOOBAR._TCP.Example.COM", 33);
    push_record(
        &mut m,
        OWNER,
        33,
        60,
        &srv_rdata(0, 0, 7100, "b.example.com"),
    );
    let answer = decode_answer(ID, OWNER, &m).unwrap();
    assert_eq!(answer.records[0].port, 7100);
}

/// `encode_query` refuses a name the protocol cannot carry rather than
/// sending a malformed query the resolver would answer with FORMERR.
#[test]
fn a_query_for_an_empty_label_or_an_oversized_label_is_refused_before_it_is_sent() {
    assert!(encode_query(1, "a..b").is_err());
    assert!(encode_query(1, "").is_err());
    let long = format!("{}.example", "x".repeat(64));
    assert!(encode_query(1, &long).is_err());
    assert!(encode_query(1, &format!("{}.example", "x".repeat(63))).is_ok());
}

/// Every header refusal ADR 0135 names, each with the reason in the
/// message: a truncated answer, an ID that is not the query's, a message
/// that is a query, a response code that is an error, and a question that
/// is not the one asked.
///
/// Mutation: drop the TC check — the truncated case decodes a record.
/// Mutation: drop the ID check — the wrong-ID case decodes.
#[test]
fn a_truncated_mismatched_or_failed_answer_is_refused_with_its_reason() {
    let good = rfc_2782_answer(ID);
    assert!(decode_answer(ID, OWNER, &good).is_ok(), "premise");

    let mut truncated = good.clone();
    truncated[2] |= 0x02;
    let err = decode_answer(ID, OWNER, &truncated).unwrap_err();
    assert!(err.to_string().contains("truncated"), "{err}");

    let err = decode_answer(ID ^ 1, OWNER, &good).unwrap_err();
    assert!(
        err.to_string().contains("answers another question"),
        "{err}"
    );

    let mut query_not_response = good.clone();
    query_not_response[2] &= 0x7F;
    let err = decode_answer(ID, OWNER, &query_not_response).unwrap_err();
    assert!(err.to_string().contains("not a response"), "{err}");

    let mut nxdomain = good.clone();
    nxdomain[3] = (nxdomain[3] & 0xF0) | 3;
    let err = decode_answer(ID, OWNER, &nxdomain).unwrap_err();
    assert!(err.to_string().contains("no such name"), "{err}");

    let mut wrong_type = header(ID, 0x8180, 0, 0);
    question(&mut wrong_type, OWNER, 1);
    let err = decode_answer(ID, OWNER, &wrong_type).unwrap_err();
    assert!(err.to_string().contains("type 1"), "{err}");

    let mut two_questions = good.clone();
    two_questions[5] = 2;
    let err = decode_answer(ID, OWNER, &two_questions).unwrap_err();
    assert!(err.to_string().contains("2 questions"), "{err}");
}

/// A no-error answer with no SRV record (NODATA) is a refusal, not an
/// empty list: an empty list would blank the broker set, and the ladder's
/// lower rungs exist for exactly this case. A record whose target runs
/// past its own RDLENGTH is a malformed answer, refused rather than read
/// into the next record.
///
/// Mutation: return `Ok` with empty `records` when none decode — the first
/// assertion fails.
#[test]
fn an_answer_with_no_srv_record_or_a_target_past_its_length_is_refused() {
    let mut nodata = header(ID, 0x8180, 0, 0);
    question(&mut nodata, OWNER, 33);
    let err = decode_answer(ID, OWNER, &nodata).unwrap_err();
    assert!(err.to_string().contains("no SRV record"), "{err}");

    let mut short = header(ID, 0x8180, 1, 0);
    question(&mut short, OWNER, 33);
    let rdata = srv_rdata(0, 0, 9, "b.example.com");
    push_name(&mut short, OWNER);
    push_u16(&mut short, 33);
    push_u16(&mut short, 1);
    push_u32(&mut short, 60);
    push_u16(&mut short, 8); // declares less than the target needs
    short.extend_from_slice(&rdata);
    let err = decode_answer(ID, OWNER, &short).unwrap_err();
    assert!(err.to_string().contains("malformed"), "{err}");
}

/// The owner a client queries is the label the broker module publishes,
/// followed by the zone, whether or not the zone came with its trailing dot.
#[test]
fn the_fabric_owner_is_the_published_service_label_in_the_zone() {
    assert_eq!(FABRIC_SRV_SERVICE, "_qip-event-fabric._tcp");
    assert_eq!(
        fabric_srv_owner("fabric.internal."),
        "_qip-event-fabric._tcp.fabric.internal"
    );
    assert_eq!(
        fabric_srv_owner("fabric.internal"),
        fabric_srv_owner("fabric.internal.")
    );
}

/// A loopback resolver that answers each query it receives by `answer`,
/// returning the socket address to point the client at.
fn loopback_resolver(
    answer: impl Fn(&[u8]) -> Option<Vec<u8>> + Send + 'static,
) -> std::net::SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    socket
        .set_read_timeout(Some(StdDuration::from_secs(5)))
        .unwrap();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 512];
        while let Ok((read, from)) = socket.recv_from(&mut buffer) {
            if let Some(reply) = answer(&buffer[..read]) {
                let _ = socket.send_to(&reply, from);
            }
        }
    });
    address
}

/// The resolver sends a real query over UDP and decodes the answer under
/// the ID it chose, so a server that echoes the query's ID is answered and
/// one that answers under another ID is refused rather than retried into.
///
/// Mutation: in `SrvResolver::query`, decode with `matching ^ 1` — the good
/// server's answer is refused.
#[test]
fn the_resolver_asks_over_udp_and_takes_only_an_answer_under_its_own_id() {
    let good = loopback_resolver(|query| {
        let id = u16::from_be_bytes([query[0], query[1]]);
        Some(rfc_2782_answer(id))
    });
    let resolver = SrvResolver::new(good, StdDuration::from_secs(2), 2).unwrap();
    let answer = resolver.query(OWNER).unwrap();
    assert_eq!(answer.records.len(), 4);

    let forged = loopback_resolver(|query| {
        let id = u16::from_be_bytes([query[0], query[1]]) ^ 0x5555;
        Some(rfc_2782_answer(id))
    });
    let resolver = SrvResolver::new(forged, StdDuration::from_secs(2), 2).unwrap();
    let err = resolver.query(OWNER).unwrap_err();
    assert!(
        err.to_string().contains("answers another question"),
        "{err}"
    );
}

/// A resolver slower than one timeout answers the first query while the
/// second is pending. That answer carries the first ID and is a correct
/// answer to this call; refusing it would make the retry a way to fail
/// against a resolver that is merely slow.
///
/// Mutation: in `SrvResolver::query`, match the answer against `id` alone
/// instead of every ID sent — the late answer is refused as another
/// question's.
#[test]
fn an_answer_to_the_first_query_arriving_during_the_second_is_accepted() {
    let slow = loopback_resolver(|query| {
        let id = u16::from_be_bytes([query[0], query[1]]);
        std::thread::sleep(StdDuration::from_millis(300));
        Some(rfc_2782_answer(id))
    });
    let resolver = SrvResolver::new(slow, StdDuration::from_millis(200), 3).unwrap();
    let answer = resolver.query(OWNER).unwrap();
    assert_eq!(answer.records.len(), 4);
}

/// A resolver that never answers is asked exactly `attempts` times, each
/// within the timeout, and then refused with the resolver and the count
/// named; it does not hang and it does not ask once more.
///
/// Mutation: loop `0..=self.attempts` — the server counts three queries.
#[test]
fn a_silent_resolver_is_given_up_on_after_the_bounded_attempts() {
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = asked.clone();
    let silent = loopback_resolver(move |_| {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        None
    });
    let resolver = SrvResolver::new(silent, StdDuration::from_millis(200), 2).unwrap();
    let started = std::time::Instant::now();
    let err = resolver.query(OWNER).unwrap_err();
    let elapsed = started.elapsed();
    assert!(err.to_string().contains("any of 2 attempts"), "{err}");
    assert!(elapsed >= StdDuration::from_millis(380), "{elapsed:?}");
    // The server thread has received the second query by the time the second
    // timeout elapsed; give it a moment to count it.
    std::thread::sleep(StdDuration::from_millis(50));
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 2);

    assert!(SrvResolver::new(silent, StdDuration::ZERO, 1).is_err());
    assert!(SrvResolver::new(silent, StdDuration::from_secs(1), 0).is_err());
}

fn answer_with_ttl(ttl_secs: u64, target: &str) -> SrvAnswer {
    SrvAnswer {
        records: vec![SrvRecord {
            priority: 0,
            weight: 0,
            port: 7100,
            target: target.to_string(),
        }],
        addresses: Default::default(),
        ttl: StdDuration::from_secs(ttl_secs),
    }
}

/// ADR 0135 decision 4: a resolver outage degrades to the last good answer
/// for its TTL times the staleness factor, then to the configured
/// addresses; and which rung answered is reported, so a region on stale
/// discovery is visible rather than quietly working.
///
/// Mutation: in `Bootstrap::resolve`, return `Cached` without the staleness
/// check — the `Configured` assertion fails. Mutation: skip the
/// `last_good` update on a fresh answer — the `Cached` assertion fails
/// with the configured list.
#[test]
fn a_resolver_loss_degrades_to_the_last_good_answer_and_then_to_configuration() {
    let clock = Arc::new(ManualClock::new(Timestamp::from_secs(1_000)));
    let mut bootstrap =
        Bootstrap::new(vec!["static.example:7100".to_string()], clock.clone()).unwrap();

    let fresh = bootstrap.resolve(|| Ok(answer_with_ttl(60, "a.example")));
    assert_eq!(fresh.source, AddressSource::Srv);
    assert_eq!(fresh.authorities, ["a.example:7100"]);

    assert_eq!(fresh.lookup_failure, None);

    // Exactly at the staleness bound the cached answer still serves.
    clock.advance(Duration::from_secs(180));
    let lost = bootstrap.resolve(|| Err(Error::timeout("resolver down")));
    assert_eq!(lost.source, AddressSource::Cached);
    assert_eq!(lost.authorities, ["a.example:7100"]);
    assert!(
        lost.lookup_failure
            .as_deref()
            .is_some_and(|why| why.contains("resolver down")),
        "{lost:?}"
    );

    clock.advance(Duration::from_secs(1));
    let stale = bootstrap.resolve(|| Err(Error::invalid("truncated (TC set)")));
    assert_eq!(stale.source, AddressSource::Configured);
    assert_eq!(stale.authorities, ["static.example:7100"]);
    assert!(
        stale
            .lookup_failure
            .as_deref()
            .is_some_and(|why| why.contains("TC set")),
        "{stale:?}"
    );

    let back = bootstrap.resolve(|| Ok(answer_with_ttl(60, "b.example")));
    assert_eq!(back.source, AddressSource::Srv);
    assert_eq!(back.authorities, ["b.example:7100"]);
    assert_eq!(AddressSource::Cached.as_label(), "cached");
}

/// A record published with a day's TTL would otherwise hide a day-long
/// outage behind a cached answer; the cap bounds it to an hour. Configured
/// addresses are required, because the ladder's last rung must hold
/// something.
///
/// Mutation: remove `.min(Self::STALENESS_CAP)` from `staleness` — the
/// day-TTL case stays `Cached` at an hour and a second.
#[test]
fn a_long_ttl_is_capped_and_an_empty_configuration_is_refused() {
    assert_eq!(
        Bootstrap::staleness(StdDuration::from_secs(86_400)),
        Bootstrap::STALENESS_CAP
    );
    let clock = Arc::new(ManualClock::new(Timestamp::from_secs(0)));
    let mut bootstrap = Bootstrap::new(vec!["s:1".to_string()], clock.clone()).unwrap();
    bootstrap.resolve(|| Ok(answer_with_ttl(86_400, "a.example")));
    clock.advance(Duration::from_secs(3_601));
    let past_cap = bootstrap.resolve(|| Err(Error::timeout("down")));
    assert_eq!(past_cap.source, AddressSource::Configured);

    assert!(Bootstrap::new(vec![], clock.clone()).is_err());
    assert!(Bootstrap::new(vec![" ".to_string()], clock.clone()).is_err());
    assert!(Bootstrap::new(vec!["h:1".to_string(), String::new()], clock).is_err());
}
