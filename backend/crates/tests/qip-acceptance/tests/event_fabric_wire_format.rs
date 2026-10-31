#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report
//! FABRIC-038: only the versioned wire format crosses the network; no
//! unversioned in-memory layout is written to a socket.
//!
//! The property held before this file and nothing said so. `unsafe` is
//! forbidden at the workspace root, which rules out the pointer cast, but it
//! does not rule out the two ways a layout still leaks without one: a
//! native-endian integer written straight to a buffer, which is this
//! machine's byte order rather than a format, and a socket write whose bytes
//! were built somewhere other than the codec. The first test below refuses
//! the first; the second pins each place the fabric writes to a socket to
//! the encoder that produces its bytes.
//!
//! What "versioned" means here, concretely. A batch is framed by
//! `qip_events::event_fabric::codec`: a magic, a `FORMAT_VERSION`, explicit
//! little-endian integers, and a decoder that refuses any other version. The
//! request and response around it are JSON under a path that begins `/v1/`.
//! Those are the two encodings; there is no third.
//!
//! The requirement also names the brokers' replication and mirror paths.
//! There are none: ADR 0100 §3 rejected in-tree replication (C2) and the
//! mirror is not built (C8). The second test asserts that absence, so the
//! commit that adds either one fails here until its send path is pinned too.

use std::path::{Path, PathBuf};

use qip_acceptance::{files_with_extension, read, repository_root};

/// Every directory holding code on the fabric's path to or from a socket.
const FABRIC_SOURCES: [&str; 5] = [
    "backend/crates/libs/qip-events/src/event_fabric",
    "backend/crates/libs/qip-transport/src/event_fabric",
    "backend/crates/services/qip-streaming/src/event_fabric",
    "backend/crates/apps/qip-fabricd/src",
    "backend/crates/apps/qip-edge-node/src/event_fabric",
];

/// Ways a value's in-memory representation becomes bytes without a format.
/// `to_le_bytes` and `to_be_bytes` are deliberately absent: an explicit byte
/// order is what a format is made of.
const REINTERPRETATIONS: [&str; 11] = [
    "to_ne_bytes",
    "from_ne_bytes",
    "transmute",
    "from_raw_parts",
    "align_to",
    "repr(C",
    "repr(packed",
    "MaybeUninit",
    "as_ptr()",
    "bytemuck",
    "zerocopy",
];

/// `text` with every `//` comment removed, so prose about a forbidden call
/// is not mistaken for one.
fn code_of(text: &str) -> String {
    text.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn fabric_files() -> Vec<PathBuf> {
    FABRIC_SOURCES
        .iter()
        .flat_map(|directory| files_with_extension(directory, "rs"))
        .collect()
}

fn relative(path: &Path) -> String {
    path.strip_prefix(repository_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn code_at(relative_path: &str) -> String {
    code_of(&read(relative_path))
}

/// No file on the fabric's path turns an in-memory layout into bytes.
///
/// Mutation (run, failed, restored): `let _ = 1u32.to_ne_bytes();` added to
/// `HttpTransport::call` — fails, naming that file and the call.
#[test]
fn no_fabric_source_writes_a_native_layout_where_a_format_belongs() {
    let files = fabric_files();
    // Premise: the walk found the fabric, including the one file that opens
    // the SDK's socket and the one that frames a batch.
    assert!(files.len() >= 20, "walked only {} files", files.len());
    for expected in ["event_fabric/transport.rs", "event_fabric/codec.rs"] {
        assert!(
            files.iter().any(|path| relative(path).ends_with(expected)),
            "{expected} was not scanned"
        );
    }
    assert!(
        read("backend/Cargo.toml").contains("unsafe_code = \"forbid\""),
        "premise: the workspace forbids unsafe, so a pointer cast cannot compile"
    );

    let mut explicit_orders = 0usize;
    let mut offenders = Vec::new();
    for path in &files {
        let code = code_of(&std::fs::read_to_string(path).unwrap());
        explicit_orders +=
            code.matches("to_le_bytes").count() + code.matches("to_be_bytes").count();
        for (number, line) in code.lines().enumerate() {
            for needle in REINTERPRETATIONS {
                if line.contains(needle) {
                    offenders.push(format!(
                        "{}:{}: {needle}: {}",
                        relative(path),
                        number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    // Premise: the scan reads code. The codec encodes its integers with an
    // explicit byte order, so a scan that sees none is not seeing the codec.
    assert!(
        explicit_orders >= 5,
        "the scan saw only {explicit_orders} explicit-endian encodings; it is not reading the code"
    );
    assert!(
        offenders.is_empty(),
        "an in-memory layout must not become wire bytes (FABRIC-038); encode it through \
         qip_events::event_fabric::codec or the JSON protocol instead:\n{}",
        offenders.join("\n")
    );
}

/// Every place the fabric writes to a socket writes bytes its encoder built,
/// the batch encoding declares and checks its version, and every route is
/// under a versioned path.
///
/// Mutation (run, failed, restored): `HttpTransport::call` building its body
/// with `format!("{request:?}").into_bytes()` instead of `encode_request` —
/// fails on the SDK's send path.
#[test]
fn every_fabric_socket_write_carries_bytes_from_the_versioned_encoders() {
    // The SDK's send path: one request built, from the protocol's encoder.
    let transport = code_at("backend/crates/libs/qip-transport/src/event_fabric/transport.rs");
    // ADR 0112 put mutual TLS on the fabric, so the one request is now
    // built by one of two constructors — `json` for a plaintext link,
    // `json_tls` for an `https` one — and both must carry the same
    // `body`, the encoder's output. Two sites, pinned by their exact
    // expression, so a third construction, or either arm building its body
    // from anything but `encode_request`, still fails here.
    assert_eq!(
        transport.matches("HttpRequest::").count(),
        2,
        "the SDK builds exactly one kind of request, over a plaintext or a mutual-TLS link"
    );
    assert!(
        transport.contains("let body = encode_request(&request)?;")
            && transport.contains("HttpRequest::json(Method::Post, &url, body)")
            && transport.contains("HttpRequest::json_tls(Method::Post, &url, body)"),
        "the SDK's request body, on either link, is the protocol encoder's output and nothing \
         else"
    );

    // What that encoder is, and the batch it carries.
    let protocol = code_at("backend/crates/libs/qip-transport/src/event_fabric/protocol.rs");
    assert!(
        protocol.contains("pub fn encode_request(request: &Request) -> Result<Vec<u8>> {\n    Ok(serde_json::to_vec(request)?)")
            && protocol.contains("pub fn encode_response(response: &Response) -> Result<Vec<u8>> {\n    Ok(serde_json::to_vec(response)?)"),
        "both protocol encoders serialise the typed request or response as JSON"
    );
    let routes = protocol.matches("=> \"/v1/event-fabric/").count();
    assert_eq!(
        routes, 9,
        "all nine routes are served under the versioned path"
    );
    assert_eq!(
        protocol.matches("=> \"/").count(),
        routes,
        "no route is served outside the versioned path"
    );
    let producer = code_at("backend/crates/libs/qip-transport/src/event_fabric/producer.rs");
    assert!(
        producer.contains("let encoded = batch.encode()?;")
            && producer.contains("to_hex(&encoded)"),
        "a produced batch travels as the codec's own bytes"
    );

    // The batch encoding declares its version first and refuses any other.
    let codec = code_at("backend/crates/libs/qip-events/src/event_fabric/codec.rs");
    let magic = codec
        .find("out.extend_from_slice(&BATCH_MAGIC);")
        .expect("the encoder writes the magic");
    // Since ADR 0129 the encoder writes one of two versions — version 1's
    // layout for a batch carrying neither of the fields version 2 added —
    // and the decoder reads exactly those two and refuses every other.
    let version = codec
        .find("out.extend_from_slice(&version.to_le_bytes());")
        .expect("the encoder writes the format version");
    assert!(magic < version, "magic, then version, before anything else");
    assert!(
        codec.contains("let version = self.format_version();"),
        "the version written is the batch's own layout, not a constant beside it"
    );
    assert!(
        codec
            .contains("if version != FORMAT_VERSION && version != FORMAT_VERSION_WITHOUT_ORIGIN {"),
        "the decoder refuses a version it was not built for"
    );

    // The broker's answer: one response body per request, from the handler,
    // whose success body is the protocol encoder's output.
    let service = code_at("backend/crates/services/qip-streaming/src/event_fabric/service.rs");
    assert!(
        service.contains("encode_response(&response)"),
        "the broker's answer is the protocol encoder's output"
    );
    let fabricd = code_at("backend/crates/apps/qip-fabricd/src/lib.rs");
    assert_eq!(
        fabricd.matches("Response::new(").count(),
        1,
        "the protocol listener writes exactly one kind of response body"
    );
    assert!(
        fabricd.contains("Response::new(handled.status, \"application/json\", handled.body)"),
        "and it is the handler's"
    );

    // The mirror's copy logic (ADR 0119, R12-FABRIC-7038) is pinned here: it
    // writes no socket and builds no bytes of its own. A copy's bytes are the
    // source row's already-encoded batch, moved unchanged, and the only
    // codec call it makes is the decoder that checks them. The send itself
    // is the producer's, above, so there is still no third send path.
    //
    // Mutation: in `plan_copy`, build `MirrorCopy::bytes` from
    // `format!("{row:?}").into_bytes()` — the `bytes: row.bytes` pin fails.
    const PINNED_MIRROR: &str = "backend/crates/services/qip-streaming/src/event_fabric/mirror.rs";
    let mirror = code_at(PINNED_MIRROR);
    assert!(
        mirror.contains("bytes: row.bytes,") && mirror.contains("Batch::decode(&row.bytes)?"),
        "a mirrored copy carries the source batch's own encoded bytes, checked by the codec's \
         decoder"
    );
    for native in [
        "TcpStream",
        "TcpListener",
        "write_all",
        ".write(",
        "to_ne_bytes",
        "to_le_bytes",
        "to_be_bytes",
        "transmute",
        ".encode(",
        "format!(\"{row",
        "into_bytes()",
    ] {
        assert!(
            !mirror.contains(native),
            "the mirror writes or builds bytes with `{native}`; a mirrored copy must be the \
             source batch's encoded bytes and nothing else (FABRIC-038)"
        );
    }

    // The mirror role that drives that logic between two brokers (ADR 0119
    // §3, R12-OBS-9148) is pinned the same way. It too builds no bytes: a
    // copy reaches the target as the source batch decoded by the codec and
    // handed to the target broker's `produce_to`, the one place a batch's
    // broker fields are stamped, so the role adds no send path of its own.
    //
    // Mutation: add `let _ = batch.encode()?;` before the produce in
    // `qip-fabricd/src/mirror.rs`'s `copy` — the loop below fails naming
    // `.encode(`.
    const PINNED_MIRROR_ROLE: &str = "backend/crates/apps/qip-fabricd/src/mirror.rs";
    let role = code_at(PINNED_MIRROR_ROLE);
    assert!(
        role.contains("Batch::decode(&copied.bytes)?")
            && role.contains("self.produce_to(stream, partition, batch)?"),
        "the mirror role hands the target the source batch, decoded by the codec, through the \
         broker's produce"
    );
    for native in [
        "TcpStream",
        "TcpListener",
        "write_all",
        ".write(",
        "to_ne_bytes",
        "to_le_bytes",
        "to_be_bytes",
        "transmute",
        ".encode(",
        "into_bytes()",
    ] {
        assert!(
            !role.contains(native),
            "the mirror role writes or builds bytes with `{native}`; a copy must reach the \
             target as the source batch and nothing else (FABRIC-038)"
        );
    }

    // Replication and mirroring: every other such path is unpinned, so there
    // is no further send path.
    for directory in [
        "backend/crates/libs/qip-transport/src/event_fabric",
        "backend/crates/services/qip-streaming/src/event_fabric",
        "backend/crates/apps/qip-fabricd/src",
    ] {
        for path in files_with_extension(directory, "rs") {
            let name = relative(&path);
            if name == PINNED_MIRROR || name == PINNED_MIRROR_ROLE {
                continue;
            }
            assert!(
                !name.contains("replica") && !name.contains("mirror"),
                "{name} looks like a replication or mirror path; pin its send path here before \
                 it ships (FABRIC-038)"
            );
        }
    }
}
