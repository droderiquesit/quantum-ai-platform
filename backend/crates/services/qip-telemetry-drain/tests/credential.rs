//! The credential reaches the collector as the operator wrote it, and can be
//! mounted as a file so the deployment keeps it out of `/proc/<pid>/environ`.
//!
//! This test lives here and not beside the code because it needs a scratch
//! file, and `std::env::temp_dir` is exactly what the library crates may not
//! name (`.claude/rules/domains/core-rust.md`). It asserts more than the unit
//! test it replaces: that one compared the parsed field, and this one reads
//! the `Authorization` header off a real socket.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use qip_core::Timestamp;
use qip_observability::Telemetry;
use qip_telemetry_drain::{
    CREDENTIAL_VARIABLE, ORG_VARIABLE, OpenObserveConfig, URL_VARIABLE, export_once,
};
use qip_transport::{ClientLimits, HttpClient};

/// Serve `connections` requests on a loopback port and return each request's
/// `authorization` header value (or `None`).
fn collector(connections: usize) -> (u16, std::thread::JoinHandle<Vec<Option<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let thread = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..connections {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).to_string();
            let length: usize = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::to_string)
                })
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            let _ = stream.read_exact(&mut body);
            seen.push(head.lines().find_map(|l| {
                let (name, value) = l.split_once(':')?;
                name.eq_ignore_ascii_case("authorization")
                    .then(|| value.trim().to_string())
            }));
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
        seen
    });
    (port, thread)
}

/// The credential can be read from a file, because the deployment must be able
/// to keep it out of the environment, and the header on the wire is the file's
/// content with its trailing newline trimmed. Before the lookup seam nothing
/// exercised this branch at all.
///
/// The direct variable is unset, so only the file can have supplied it, and an
/// unauthenticated run on the same collector sends no header: the assertion is
/// not satisfied by a header that is always present.
///
/// Mutation: make `OpenObserveConfig::read` ignore the `_FILE` spelling. The
/// config is built unauthenticated, no header is sent and the first
/// assertion fails.
#[test]
fn the_credential_can_be_read_from_a_file_and_reaches_the_collector_as_written() {
    let directory =
        std::env::temp_dir().join(format!("qip-drain-credential-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("authorization");
    std::fs::write(&path, "Basic dGVzdA==\n").unwrap();
    let file_variable = format!("{CREDENTIAL_VARIABLE}{}", qip_core::secret::FILE_SUFFIX);

    let run = |with_credential: bool| {
        let (port, thread) = collector(2);
        let mut variables: BTreeMap<String, String> = [
            (URL_VARIABLE.to_string(), format!("http://127.0.0.1:{port}")),
            (ORG_VARIABLE.to_string(), "qip".to_string()),
        ]
        .into();
        if with_credential {
            variables.insert(file_variable.clone(), path.display().to_string());
        }
        assert!(
            !variables.contains_key(CREDENTIAL_VARIABLE),
            "premise: the direct variable is unset"
        );
        let config = OpenObserveConfig::read(&|name| variables.get(name).cloned())
            .expect("the configuration is accepted")
            .expect("a set URL produces a configuration");
        assert_eq!(
            config.describe().contains(", authenticated"),
            with_credential,
            "{}",
            config.describe()
        );
        let client = HttpClient::new(ClientLimits {
            connect_timeout: Duration::from_secs(2),
            read_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
            ..ClientLimits::default()
        });
        let pass = export_once(
            &Telemetry::silent(),
            &client,
            &config,
            Timestamp::from_secs(1_790_000_000),
        )
        .expect("the endpoints build");
        assert!(pass.metrics.is_ok() && pass.traces.is_ok(), "{pass:?}");
        thread.join().unwrap()
    };

    assert_eq!(
        run(true),
        vec![Some("Basic dGVzdA==".to_string()); 2],
        "both signals carry the file's content, trimmed of its newline"
    );
    assert_eq!(run(false), vec![None, None], "no credential, no header");
    let _ = std::fs::remove_dir_all(&directory);
}
