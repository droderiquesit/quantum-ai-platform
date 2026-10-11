//! Throwaway X.509 certificates for the mutual-TLS tests, minted in memory.
//!
//! Test-only, and in a test directory on purpose. ADR 0112 has Certificate
//! Authority Service issue the deployment's certificates and writes no ASN.1
//! encoder into the product; the tests still need a CA, a server certificate
//! and client certificates — including ones from the *wrong* CA — and
//! committing PEM private keys as fixtures would put key material in the
//! repository, which the governance rules forbid even for tests. So each run
//! generates fresh P-256 keys with `ring` and encodes the few DER structures
//! a certificate needs. Nothing here is reachable from a binary.
//!
//! The encoding is the minimum `webpki` accepts: v3, a positive serial,
//! `ecdsa-with-SHA256`, UTF8String common names, UTCTime validity, basic
//! constraints and key usage on every certificate, and for leaves an
//! extended key usage naming both server and client authentication and a DNS
//! subject alternative name.

#![allow(dead_code)]

use qip_transport::kms::encode_base64;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];
const OID_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
const OID_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
const OID_EXTENDED_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25];
const OID_SUBJECT_ALT_NAME: &[u8] = &[0x55, 0x1d, 0x11];
const OID_SERVER_AUTH: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
const OID_CLIENT_AUTH: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x02];

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let length = content.len();
    if length < 0x80 {
        out.push(length as u8);
    } else if length < 0x100 {
        out.extend_from_slice(&[0x81, length as u8]);
    } else {
        out.extend_from_slice(&[0x82, (length >> 8) as u8, length as u8]);
    }
    out.extend_from_slice(content);
    out
}

fn sequence(parts: &[Vec<u8>]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}

fn oid(bytes: &[u8]) -> Vec<u8> {
    tlv(0x06, bytes)
}

fn name(common_name: &str) -> Vec<u8> {
    sequence(&[tlv(
        0x31,
        &sequence(&[oid(OID_COMMON_NAME), tlv(0x0c, common_name.as_bytes())]),
    )])
}

fn extension(id: &[u8], critical: bool, value: Vec<u8>) -> Vec<u8> {
    let mut parts = vec![oid(id)];
    if critical {
        parts.push(tlv(0x01, &[0xff]));
    }
    parts.push(tlv(0x04, &value));
    sequence(&parts)
}

fn pem(label: &str, der: &[u8]) -> String {
    let body = encode_base64(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// A key and the certificate over it.
pub(crate) struct Minted {
    pub(crate) certificate_der: Vec<u8>,
    pub(crate) pkcs8: Vec<u8>,
    pair: EcdsaKeyPair,
    subject: String,
}

impl Minted {
    pub(crate) fn certificate_pem(&self) -> String {
        pem("CERTIFICATE", &self.certificate_der)
    }

    pub(crate) fn key_pem(&self) -> String {
        pem("PRIVATE KEY", &self.pkcs8)
    }
}

fn key() -> (EcdsaKeyPair, Vec<u8>) {
    let random = SystemRandom::new();
    let document =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &random).expect("a key");
    let pair =
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, document.as_ref(), &random)
            .expect("the key just generated parses");
    (pair, document.as_ref().to_vec())
}

fn certificate(
    serial: u8,
    issuer: &str,
    issuer_key: &EcdsaKeyPair,
    subject: &str,
    subject_key: &EcdsaKeyPair,
    extensions: Vec<Vec<u8>>,
) -> Vec<u8> {
    certificate_over(
        serial,
        issuer,
        issuer_key,
        subject,
        subject_key.public_key().as_ref(),
        ("200101000000Z", "491231235959Z"),
        extensions,
    )
}

/// The general form: a certificate over any uncompressed P-256 `point`,
/// valid between two `UTCTime`s (`YYMMDDHHMMSSZ`).
fn certificate_over(
    serial: u8,
    issuer: &str,
    issuer_key: &EcdsaKeyPair,
    subject: &str,
    subject_point: &[u8],
    (not_before, not_after): (&str, &str),
    extensions: Vec<Vec<u8>>,
) -> Vec<u8> {
    let algorithm = sequence(&[oid(OID_ECDSA_SHA256)]);
    let mut point = vec![0x00];
    point.extend_from_slice(subject_point);
    let spki = sequence(&[
        sequence(&[oid(OID_EC_PUBLIC_KEY), oid(OID_P256)]),
        tlv(0x03, &point),
    ]);
    let tbs = sequence(&[
        tlv(0xa0, &tlv(0x02, &[0x02])),
        tlv(0x02, &[serial]),
        algorithm.clone(),
        name(issuer),
        sequence(&[
            tlv(0x17, not_before.as_bytes()),
            tlv(0x17, not_after.as_bytes()),
        ]),
        name(subject),
        spki,
        tlv(0xa3, &sequence(&extensions)),
    ]);
    let signature = issuer_key.sign(&SystemRandom::new(), &tbs).expect("signs");
    let mut bits = vec![0x00];
    bits.extend_from_slice(signature.as_ref());
    sequence(&[tbs, algorithm, tlv(0x03, &bits)])
}

/// A self-signed certificate authority.
pub(crate) fn authority(common_name: &str) -> Minted {
    let (pair, pkcs8) = key();
    let extensions = vec![
        extension(OID_BASIC_CONSTRAINTS, true, sequence(&[tlv(0x01, &[0xff])])),
        // keyCertSign and cRLSign.
        extension(OID_KEY_USAGE, true, tlv(0x03, &[0x01, 0x06])),
    ];
    let certificate_der = certificate(1, common_name, &pair, common_name, &pair, extensions);
    Minted {
        certificate_der,
        pkcs8,
        pair,
        subject: common_name.to_string(),
    }
}

/// A leaf certificate for `dns_name`, issued by `ca`, usable as a server or
/// a client certificate.
pub(crate) fn leaf(ca: &Minted, serial: u8, dns_name: &str) -> Minted {
    let (pair, pkcs8) = key();
    let extensions = vec![
        extension(OID_BASIC_CONSTRAINTS, true, sequence(&[])),
        // digitalSignature.
        extension(OID_KEY_USAGE, true, tlv(0x03, &[0x07, 0x80])),
        extension(
            OID_EXTENDED_KEY_USAGE,
            false,
            sequence(&[oid(OID_SERVER_AUTH), oid(OID_CLIENT_AUTH)]),
        ),
        extension(
            OID_SUBJECT_ALT_NAME,
            false,
            sequence(&[tlv(0x82, dns_name.as_bytes())]),
        ),
    ];
    let certificate_der = certificate(serial, &ca.subject, &ca.pair, dns_name, &pair, extensions);
    Minted {
        certificate_der,
        pkcs8,
        pair,
        subject: dns_name.to_string(),
    }
}

/// [`leaf`], valid for the IPv4 address `ip` as well as `dns_name` — for a
/// server a client reaches by a checked address rather than a name, as
/// `qip event-fabric grant` reaches its loopback broker. An iPAddress
/// subject alternative name is context tag 7 holding the four octets.
pub(crate) fn leaf_with_ip(ca: &Minted, serial: u8, dns_name: &str, ip: [u8; 4]) -> Minted {
    let (pair, pkcs8) = key();
    let extensions = vec![
        extension(OID_BASIC_CONSTRAINTS, true, sequence(&[])),
        // digitalSignature.
        extension(OID_KEY_USAGE, true, tlv(0x03, &[0x07, 0x80])),
        extension(
            OID_EXTENDED_KEY_USAGE,
            false,
            sequence(&[oid(OID_SERVER_AUTH), oid(OID_CLIENT_AUTH)]),
        ),
        extension(
            OID_SUBJECT_ALT_NAME,
            false,
            sequence(&[tlv(0x82, dns_name.as_bytes()), tlv(0x87, &ip)]),
        ),
    ];
    let certificate_der = certificate(serial, &ca.subject, &ca.pair, dns_name, &pair, extensions);
    Minted {
        certificate_der,
        pkcs8,
        pair,
        subject: dns_name.to_string(),
    }
}

/// A leaf over somebody else's public key — what a CA does with the key a
/// workload sent it — valid between two `UTCTime`s (`YYMMDDHHMMSSZ`). DER.
pub(crate) fn leaf_over(
    ca: &Minted,
    serial: u8,
    dns_name: &str,
    point: &[u8],
    validity: (&str, &str),
) -> Vec<u8> {
    let extensions = vec![
        extension(OID_BASIC_CONSTRAINTS, true, sequence(&[])),
        extension(OID_KEY_USAGE, true, tlv(0x03, &[0x07, 0x80])),
        extension(
            OID_EXTENDED_KEY_USAGE,
            false,
            sequence(&[oid(OID_SERVER_AUTH), oid(OID_CLIENT_AUTH)]),
        ),
        extension(
            OID_SUBJECT_ALT_NAME,
            false,
            sequence(&[tlv(0x82, dns_name.as_bytes())]),
        ),
    ];
    certificate_over(
        serial,
        &ca.subject,
        &ca.pair,
        dns_name,
        point,
        validity,
        extensions,
    )
}

/// `UTCTime` text for an instant, as `leaf_over` takes it.
pub(crate) fn utc_time(instant: qip_core::Timestamp) -> String {
    let (year, month, day) = instant.civil_date();
    let (hour, minute, second, _) = instant.civil_time();
    format!(
        "{:02}{month:02}{day:02}{hour:02}{minute:02}{second:02}Z",
        year.rem_euclid(100)
    )
}

/// A second handle on an authority, for a thread or a struct that must own
/// one: the key pair is reloaded from its PKCS#8, which is all `ring` allows.
pub(crate) fn authority_from(ca: &Minted) -> Minted {
    let pair = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_ASN1_SIGNING,
        &ca.pkcs8,
        &SystemRandom::new(),
    )
    .expect("a key this module made reloads");
    Minted {
        certificate_der: ca.certificate_der.clone(),
        pkcs8: ca.pkcs8.clone(),
        pair,
        subject: ca.subject.clone(),
    }
}

/// PEM armour for any DER.
pub(crate) fn pem_of(label: &str, der: &[u8]) -> String {
    pem(label, der)
}
