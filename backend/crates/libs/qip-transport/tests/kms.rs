//! The Cloud KMS signer refuses every answer it cannot stand behind (ADR
//! 0110 decision 2): one for another key version, one from a key that is
//! not HSM-protected, and one whose signature does not verify under the
//! trust bundle the fleet holds — each over a real TLS socket, against a
//! stand-in for the KMS REST endpoint.
//!
//! The admitting half — a genuine KMS signature accepted — cannot be built
//! here: KMS signs a *digest*, and `ring` signs only messages, so no in-tree
//! stand-in can produce a valid signature over a digest it was sent. That
//! half is proven where it can be, by `prove_signer` at the API's start-up
//! against the real service.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

#[path = "support/certs.rs"]
mod certs;

use qip_contracts::trust::{ArtefactClass, ArtefactSigner, TrustBundle, TrustedKey};
use qip_core::error::Result;
use qip_core::{ManualClock, Timestamp};
use qip_transport::TlsHttpClient;
use qip_transport::kms::{BearerSource, KmsSigner, encode_base64};
use qip_transport::rustls;
use qip_transport::trust::{BundleVerifier, LocalSigner};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

const VERSION: &str = "projects/p/locations/l/keyRings/r/cryptoKeys/policy/cryptoKeyVersions/1";

#[derive(Debug)]
struct Token;

impl BearerSource for Token {
    fn bearer(&self) -> Result<String> {
        Ok("test-token".to_string())
    }
}

/// A TLS listener that answers one request with `body`, then closes. Built
/// on `rustls` directly because the platform's own server always demands a
/// client certificate, and Cloud KMS does not ask for one.
fn fake_kms(server: &certs::Minted, body: String) -> (u16, std::thread::JoinHandle<()>) {
    let chain = vec![CertificateDer::from(server.certificate_der.clone())];
    let key = PrivateKeyDer::from_pem_slice(server.key_pem().as_bytes()).expect("key");
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .expect("tls 1.3")
    .with_no_client_auth()
    .with_single_cert(chain, key)
    .expect("server config");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let config = Arc::new(config);
    let handle = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut session = rustls::ServerConnection::new(config).expect("session");
        while session.is_handshaking() {
            if session.complete_io(&mut socket).is_err() {
                return;
            }
        }
        let mut stream = rustls::StreamOwned::new(session, socket);
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(n) => request.extend_from_slice(&buffer[..n]),
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        stream.conn.send_close_notify();
        let _ = stream.flush();
    });
    (port, handle)
}

fn signer_against(answer: String) -> (Result<Vec<u8>>, std::thread::JoinHandle<()>) {
    let ca = certs::authority("fake kms ca");
    let server = certs::leaf(&ca, 2, "localhost");
    let published = LocalSigner::generate(ArtefactClass::Policy, VERSION).expect("key");
    let bundle = TrustBundle::new(
        vec![TrustedKey {
            class: ArtefactClass::Policy,
            key_version: VERSION.to_string(),
            public_key: published.public_key_hex(),
            not_after: Timestamp::from_secs(4_000_000_000),
        }],
        BTreeSet::new(),
    )
    .expect("bundle");
    let (port, handle) = fake_kms(&server, answer);
    let client = TlsHttpClient::new(
        KmsSigner::limits(),
        qip_transport::tls::server_authenticated_client(ca.certificate_pem().as_bytes())
            .expect("client config"),
    );
    let signer = KmsSigner::new(
        ArtefactClass::Policy,
        VERSION,
        client,
        Arc::new(Token),
        BundleVerifier::new(bundle),
        Arc::new(ManualClock::new(Timestamp::from_secs(1_800_000_000))),
    )
    .expect("a signer whose version the bundle publishes")
    .with_endpoint(&format!("https://localhost:{port}/v1"))
    .expect("https endpoint");
    (signer.sign_message(b"a policy message"), handle)
}

fn answer(name: &str, protection: &str, signature: &[u8]) -> String {
    serde_json::json!({
        "signature": encode_base64(signature),
        "name": name,
        "protectionLevel": protection,
        "verifiedDigestCrc32c": true,
    })
    .to_string()
}

fn some_der() -> Vec<u8> {
    let other = LocalSigner::generate(ArtefactClass::Policy, "elsewhere").expect("key");
    other.sign_message(b"a policy message").expect("signs")
}

/// Mutation: drop the name comparison in `KmsSigner::call` and a signature
/// attributed to another version is returned; this fails.
#[test]
fn an_answer_for_another_key_version_is_refused() {
    let (outcome, handle) = signer_against(answer(
        "projects/p/locations/l/keyRings/r/cryptoKeys/policy/cryptoKeyVersions/2",
        "HSM",
        &some_der(),
    ));
    handle.join().expect("server thread");
    let error = outcome.expect_err("an answer for another version was accepted");
    assert!(error.message().contains("asked"), "{}", error.message());
}

/// Mutation: drop the protection-level check and a software key signs
/// policy; this fails.
#[test]
fn an_answer_from_a_key_that_is_not_hsm_protected_is_refused() {
    let (outcome, handle) = signer_against(answer(VERSION, "SOFTWARE", &some_der()));
    handle.join().expect("server thread");
    let error = outcome.expect_err("a software key's signature was accepted");
    assert!(error.message().contains("HSM"), "{}", error.message());
}

/// Mutation: drop the self-check against the bundle and a signature the
/// fleet will refuse is returned as good; this fails.
#[test]
fn a_signature_that_does_not_verify_under_the_bundle_is_refused() {
    let (outcome, handle) = signer_against(answer(VERSION, "HSM", &some_der()));
    handle.join().expect("server thread");
    let error = outcome.expect_err("a signature the bundle rejects was returned");
    assert!(error.message().contains("trust"), "{}", error.message());
}
