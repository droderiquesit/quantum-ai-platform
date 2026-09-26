//! The event fabric's bearer-token identity (ADR 0100 §7).
//!
//! Five properties, each of which has a cheap way to be lost: a comparison
//! that accepts a near miss, a loader that takes a token where a hash belongs,
//! a missing header read as an anonymous caller, a token printed by `{:?}`,
//! and a constant-time comparison quietly replaced by `==` — the last of which
//! no behavioural test can see, so it is checked against the source.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_transport::event_fabric::auth::{BearerToken, IdentityTable, verify};

/// A fixture token: long enough, in the RFC 6750 alphabet, and distinctive
/// enough that finding it inside a rendered string is not an accident.
const TOKEN: &str = "fabricProducerTokenAlpha0123456789abcdefXYZ";
const OTHER: &str = "fabricProducerTokenBravo0123456789abcdefXYZ";

fn table_for(lines: &[(&str, String)]) -> Result<IdentityTable> {
    let text: String = lines
        .iter()
        .map(|(name, digest)| format!("{name} {digest}\n"))
        .collect();
    IdentityTable::parse(&text)
}

/// A digest that agrees with `digest` on every character except the last.
fn near_miss(digest: &str) -> String {
    let mut chars: Vec<char> = digest.chars().collect();
    if let Some(last) = chars.last_mut() {
        *last = if *last == '0' { '1' } else { '0' };
    }
    chars.into_iter().collect()
}

/// A near-miss token must be refused, and so must a stored digest that agrees
/// with the presented token's digest on all but its last character. The
/// second is the case a prefix-only comparison accepts: a token cannot be
/// ground to share a 32-bit digest prefix in a test's time budget, but a
/// stored digest can simply be written that way, and it exercises exactly the
/// bytes a truncated comparison would skip.
#[test]
fn a_presented_token_is_accepted_only_when_its_hash_matches_in_constant_time() -> Result<()> {
    let digest = sha256_hex(TOKEN.as_bytes());
    let nearly = near_miss(&digest);
    // Premise: the near miss differs, and only after the first 8 characters.
    assert_ne!(nearly, digest);
    assert_eq!(nearly[..63], digest[..63]);

    let exact = table_for(&[("producer-a", digest.clone())])?;
    let header = format!("Bearer {TOKEN}");
    let identity = verify(&exact, Some(&header))?;
    assert_eq!(identity.as_str(), "producer-a");

    // A token one character away is a different digest entirely.
    let near_token = format!("Bearer {}", TOKEN.replacen('A', "B", 1));
    assert!(verify(&exact, Some(&near_token)).is_err());

    // A stored digest one character away from the real one accepts nothing.
    let almost = table_for(&[("producer-a", nearly)])?;
    let refused = verify(&almost, Some(&header));
    assert!(
        matches!(refused, Err(Error::Denied(_))),
        "a digest differing in its last character accepted the token: {refused:?}"
    );

    // With several entries, the matching one is found wherever it sits.
    let several = table_for(&[
        ("producer-b", sha256_hex(OTHER.as_bytes())),
        ("producer-a", digest),
    ])?;
    assert_eq!(verify(&several, Some(&header))?.as_str(), "producer-a");
    Ok(())
}

/// The file holds hashes and never tokens. A pasted token — here 64
/// characters, the length a digest has, so that a length-only check would
/// wave it through — and a digest of the wrong shape are both refused, and
/// the refusal does not repeat the line, because the line may be a secret.
#[test]
fn an_identities_file_holding_a_plaintext_token_or_a_malformed_hash_refuses_to_load() -> Result<()>
{
    let plaintext = "fabricProducerTokenAlpha0123456789abcdefXYZfabricProducerTokenAl";
    // Premise: the plaintext fixture is exactly digest-length, so only the
    // alphabet can reject it.
    assert_eq!(plaintext.len(), 64);
    let good = sha256_hex(TOKEN.as_bytes());
    assert!(IdentityTable::parse(&format!("producer-a {good}\n")).is_ok());

    let cases = [
        format!("producer-a {plaintext}\n"),
        format!("{plaintext}\n"),
        format!("producer-a {}\n", &good[..63]),
        format!("producer-a {}\n", good.to_uppercase()),
        format!("producer-a {good} trailing\n"),
        "# only a comment\n\n".to_string(),
        format!("producer-a {good}\nproducer-b {good}\n"),
        format!(
            "producer-a {good}\nproducer-a {}\n",
            sha256_hex(OTHER.as_bytes())
        ),
    ];
    for text in &cases {
        let outcome = IdentityTable::parse(text);
        let Err(error) = outcome else {
            panic!("loaded an identities file that should have been refused: {text:?}");
        };
        assert!(
            !error.to_string().contains(plaintext),
            "the refusal repeated the plaintext token: {error}"
        );
    }

    // The same through the file loader, which is what a deployment calls.
    let path =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("event-fabric-identities-plaintext");
    std::fs::write(&path, format!("# producers\nproducer-a {plaintext}\n"))
        .map_err(|error| Error::io(error.to_string()))?;
    let loaded = IdentityTable::load(&path);
    let Err(error) = loaded else {
        panic!("the loader accepted a file holding a plaintext token");
    };
    assert!(error.to_string().contains("line 2"), "{error}");
    assert!(!error.to_string().contains(plaintext), "{error}");
    Ok(())
}

/// No header, the wrong scheme, and an empty or malformed token are each a
/// refusal — never an anonymous caller — and each names the scheme expected.
#[test]
fn a_missing_or_malformed_authorization_header_is_refused_naming_the_scheme() -> Result<()> {
    let table = table_for(&[("producer-a", sha256_hex(TOKEN.as_bytes()))])?;
    // Premise: the table does accept the right header.
    assert!(verify(&table, Some(&format!("Bearer {TOKEN}"))).is_ok());

    let headers: [Option<String>; 7] = [
        None,
        Some(String::new()),
        Some("Bearer".to_string()),
        Some("Bearer ".to_string()),
        Some(format!("Basic {TOKEN}")),
        Some(format!("bearer {TOKEN}")),
        Some(format!("Bearer {TOKEN}\n")),
    ];
    for header in &headers {
        let outcome = verify(&table, header.as_deref());
        let Err(error) = outcome else {
            panic!("the header {header:?} was admitted");
        };
        assert!(
            matches!(error, Error::Denied(_)),
            "the header {header:?} was refused as the wrong class: {error}"
        );
        assert!(
            error.message().contains("`Bearer <token>`")
                || error.message().contains("Bearer token"),
            "the refusal for {header:?} does not name the scheme: {error}"
        );
    }
    Ok(())
}

/// Nothing a caller can render — a token's `Debug`, a verified identity, a
/// table, the refusal for a wrong token or a malformed one — contains the
/// token. `{:?}` in a log line is how credentials reach log aggregators.
#[test]
fn no_debug_or_error_rendering_of_an_identity_contains_its_token() -> Result<()> {
    let token = BearerToken::new(TOKEN.to_string())?;
    // Premise: the token is really inside the holder.
    assert_eq!(token.header_value(), format!("Bearer {TOKEN}"));

    let table = table_for(&[("producer-a", sha256_hex(TOKEN.as_bytes()))])?;
    let identity = verify(&table, Some(&token.header_value()))?;

    let other = format!("Bearer {OTHER}");
    let refused = match verify(&table, Some(&other)) {
        Err(error) => error.to_string(),
        Ok(identity) => panic!("a wrong token verified as {identity}"),
    };
    let malformed = match BearerToken::new(format!("{TOKEN} ")) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("a token with a trailing space was accepted"),
    };
    let unresolved = match BearerToken::resolve("FABRIC_TOKEN", Some(format!("{TOKEN}\n")), None) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("a token with a trailing newline was accepted"),
    };

    let renderings = [
        format!("{token:?}"),
        format!("{identity:?}"),
        format!("{identity}"),
        format!("{table:?}"),
        refused,
        malformed,
        unresolved,
    ];
    for rendering in &renderings {
        assert!(!rendering.is_empty());
        assert!(
            !rendering.contains(TOKEN) && !rendering.contains(OTHER),
            "a rendering contains a token: {rendering}"
        );
    }
    Ok(())
}

/// Replacing the constant-time comparison with `==` passes every behavioural
/// test above, because both give the same answers; only the timing differs.
/// So the body of `verify` is read from source and must call
/// `constant_time_eq(` and contain neither `==` nor `!=`.
#[test]
fn the_token_check_goes_through_the_constant_time_comparison_and_never_through_equality() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/event_fabric/auth.rs");
    let source = std::fs::read_to_string(path).unwrap_or_default();
    let body = function_body(&source, "pub fn verify(").unwrap_or_default();
    // Premise: the body was found and is not empty.
    assert!(
        body.trim().len() > 2,
        "could not find the body of `pub fn verify(` in {path}"
    );
    assert!(
        body.contains("constant_time_eq("),
        "verify does not call constant_time_eq:\n{body}"
    );
    assert!(
        !body.contains("==") && !body.contains("!="),
        "verify compares with an equality operator:\n{body}"
    );
}

/// The text between the braces of the first function whose signature starts
/// with `signature`, braces matched by depth.
fn function_body(source: &str, signature: &str) -> Option<String> {
    let start = source.find(signature)?;
    let open = start + source[start..].find('{')?;
    let mut depth = 0usize;
    for (offset, c) in source[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(source[open + 1..open + offset].to_string());
                }
            }
            _ => {}
        }
    }
    None
}
