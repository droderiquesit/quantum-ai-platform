//! Bearer-token producer identity for the event fabric (ADR 0100 §7).
//!
//! Every process that speaks to the fabric authenticates the same way, so the
//! rule is written once, here, rather than once per binary where the copies
//! could drift. The pattern is `qip-api`'s `Authenticator`, copied rather than
//! linked — a library cannot depend on an application — and narrowed to what
//! the fabric needs: *which producer is this*, and nothing about roles.
//!
//! # What this is not
//!
//! **The fabric runs over plaintext TCP.** A bearer token on a plaintext
//! connection is readable, and replayable, by anything on the path. What this
//! module buys is a guard against *misconfiguration* — a producer pointed at
//! the wrong fabric, a consumer started with another deployment's
//! credentials, a process nobody provisioned publishing at all — and an
//! attribution on every accepted request. It is not proof of identity against
//! an on-path attacker, and nothing in this file should be read as one.
//! Mutual TLS is what would be, and it is BLOCKED(C2): it needs either a TLS
//! stack, which is not a dependency this workspace takes (ADR 0002, ADR 0009),
//! or a mesh sidecar terminating it outside the process. There is deliberately
//! no hand-rolled handshake here in the meantime — see the security section of
//! this crate's root documentation for why a home-made scheme is worse than an
//! honest plaintext one.
//!
//! # Why the verifier holds hashes, never tokens
//!
//! The verifying side keeps SHA-256 digests of the tokens it accepts, loaded
//! from an identities file (see [`IdentityTable::parse`] for the format). A
//! file of digests leaked from a consumer's disk or a backup does not let the
//! reader publish; a file of tokens would. So the loader **refuses** anything
//! that is not a well-formed digest, and refuses without echoing the line —
//! the most likely thing to be sitting in a malformed line is a plaintext
//! token somebody pasted by mistake, and the error message is the one place
//! that would copy it into a log.
//!
//! One case cannot be told apart and is stated rather than hidden: a token
//! that is itself exactly 64 lowercase hexadecimal characters is
//! indistinguishable from a digest. Such a token pasted into the file loads,
//! and then matches nothing, because the file holds the digest of that string
//! and not the string. It fails closed — every request from that producer is
//! refused — but it fails as "not recognised", not at load time.
//!
//! # Why the comparison is constant-time
//!
//! [`verify`] compares the presented token's digest against every entry with
//! [`qip_core::hash::constant_time_eq`], and visits every entry even after a
//! match. A comparison that returns at the first differing byte, or a loop
//! that stops at the first matching entry, makes response time a function of
//! the secret, which is an oracle a patient caller can read. The comparison
//! is pinned structurally by
//! `the_token_check_goes_through_the_constant_time_comparison_and_never_through_equality`
//! in `tests/event_fabric_auth.rs`, because a behavioural test cannot tell
//! `==` from a constant-time comparison — both give the same answers.
//!
//! # No ambient configuration
//!
//! Everything arrives from the caller: header text, token bytes, a path. This
//! module never reads the process environment and names no deployment
//! variable; the composition root does that and hands the result in, which
//! is what lets two fabric processes run in one test with different
//! identities.

use qip_core::error::{Error, Result};
use qip_core::hash::{constant_time_eq, sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::Path;

/// The HTTP header a bearer token travels in.
pub const HEADER: &str = "Authorization";

/// The authentication scheme, as it appears before the token.
///
/// Matched exactly and case-sensitively. RFC 7235 makes the scheme
/// case-insensitive, but every client of the fabric is built from this module
/// and sends exactly this spelling; accepting variants would be guessing on
/// behalf of a client nobody wrote.
pub const SCHEME: &str = "Bearer";

/// The shortest token accepted.
///
/// A token is the whole of the secret. Thirty-two characters of the RFC 6750
/// alphabet is roughly 190 bits if generated randomly; anything shorter is
/// more likely to be a placeholder or a hand-typed word than a generated
/// credential, and a guessable token makes the constant-time comparison moot.
pub const MINIMUM_TOKEN_LENGTH: usize = 32;

/// The longest token accepted, so a header cannot make the verifier hash an
/// arbitrarily large input on a caller's say-so.
pub const MAXIMUM_TOKEN_LENGTH: usize = 512;

/// The largest identities file read, in bytes.
///
/// The file is a few lines per producer. A bound keeps a mis-pointed path —
/// a log, a device — from being read into memory whole.
pub const MAXIMUM_IDENTITIES_FILE_BYTES: u64 = 64 * 1024;

/// The most identities a table holds. Bounded for the same reason as the
/// file, and because [`verify`] visits every entry on every request.
pub const MAXIMUM_IDENTITIES: usize = 256;

/// The longest identity name accepted.
pub const MAXIMUM_IDENTITY_LENGTH: usize = 128;

/// A producer's own bearer token — the secret, on the sending side.
///
/// Deliberately has no derived `Debug`, no `Display`, no `Clone` and no
/// serialisation. `Debug` is written by hand and prints nothing of the value,
/// because a `{:?}` in a log line or a panic message is the ordinary way a
/// credential ends up in a log aggregator. The only way to get the token out
/// is [`BearerToken::header_value`], which builds the one string the token
/// exists to produce.
///
/// The bytes are not wiped on drop. Doing that reliably needs a volatile
/// write, which is `unsafe`, which this workspace forbids.
pub struct BearerToken {
    token: String,
}

impl BearerToken {
    /// Accept a token the caller has already read, refusing one that could
    /// not be sent or that is too short to be a generated secret.
    ///
    /// Refused rather than trimmed: a token with a trailing newline or an
    /// embedded space is a token the verifying side's file was not computed
    /// from, and correcting it here would make the two sides disagree about
    /// what the secret is. The refusal never contains the token.
    pub fn new(token: String) -> Result<Self> {
        check_token_shape(&token).map_err(Error::invalid)?;
        Ok(Self { token })
    }

    /// Resolve the token through `qip_core::secret`'s `_FILE` rule, with both
    /// sources passed in by the composition root.
    ///
    /// `variable` is only the name used in messages. Absence is refused: a
    /// fabric client with no token would be refused by every consumer, and
    /// finding that out at start-up is cheaper than finding it out as a
    /// stream of rejected publishes.
    pub fn resolve(variable: &str, direct: Option<String>, path: Option<String>) -> Result<Self> {
        let Some(token) = qip_core::secret::resolve_from(variable, direct, path)? else {
            return Err(Error::invalid(format!(
                "no event-fabric token was supplied: set {variable} or {variable}{}",
                qip_core::secret::FILE_SUFFIX
            )));
        };
        check_token_shape(&token)
            .map_err(|reason| Error::invalid(format!("{variable}: {reason}")))?;
        Ok(Self { token })
    }

    /// The value of the [`HEADER`] header: the scheme, one space, the token.
    pub fn header_value(&self) -> String {
        format!("{SCHEME} {}", self.token)
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BearerToken(<redacted>)")
    }
}

/// A verified producer: the name the identities file gave the matching hash.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Identity(String);

impl Identity {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One accepted token, as the verifier knows it: a name and a digest.
struct Entry {
    identity: Identity,
    digest: [u8; 32],
}

/// The tokens a fabric endpoint accepts, as SHA-256 digests keyed by name.
///
/// `Debug` lists names only. The digests are not secret in the way a token
/// is, but nothing reading a log needs them, and a table printed whole is a
/// table somebody can start grinding against.
pub struct IdentityTable {
    entries: Vec<Entry>,
}

impl IdentityTable {
    /// Load an identities file from `path`. See [`IdentityTable::parse`].
    ///
    /// Read with a bound, so a mis-pointed path cannot be read whole.
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(|error| {
            Error::io(format!(
                "the event-fabric identities file {} could not be opened: {error}",
                path.display()
            ))
        })?;
        let mut bytes = Vec::new();
        file.take(MAXIMUM_IDENTITIES_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                Error::io(format!(
                    "the event-fabric identities file {} could not be read: {error}",
                    path.display()
                ))
            })?;
        if bytes.len() as u64 > MAXIMUM_IDENTITIES_FILE_BYTES {
            return Err(Error::invalid(format!(
                "the event-fabric identities file {} is larger than {MAXIMUM_IDENTITIES_FILE_BYTES} \
                 bytes; it holds one short line per producer, so check the path names the right file",
                path.display()
            )));
        }
        let text = String::from_utf8(bytes).map_err(|_| {
            Error::invalid(format!(
                "the event-fabric identities file {} is not UTF-8 text",
                path.display()
            ))
        })?;
        Self::parse(&text).map_err(|error| {
            let message = format!("{}: {}", path.display(), error.message());
            error.relabelled(message)
        })
    }

    /// Parse identities-file text.
    ///
    /// One producer per line: `<identity> <sha256>`, separated by whitespace,
    /// where `<sha256>` is the 64-character lowercase hexadecimal digest of
    /// the producer's token — what `qip_core::hash::sha256_hex` prints. Blank
    /// lines and lines whose first non-space character is `#` are ignored.
    ///
    /// Refused, each naming the line number and never the line's content:
    /// a line that is not exactly two fields (a bare token is one field); a
    /// digest that is not 64 lowercase hex characters (a token is almost
    /// always that); an identity outside `[A-Za-z0-9._-]` or longer than
    /// [`MAXIMUM_IDENTITY_LENGTH`]; a repeated identity; a repeated digest,
    /// because two names for one token makes every request from it
    /// attributable to either; more than [`MAXIMUM_IDENTITIES`] entries; and a
    /// file with no entries at all, which would refuse every producer and is
    /// never what a deployment meant.
    pub fn parse(text: &str) -> Result<Self> {
        let mut entries = Vec::new();
        let mut names = BTreeMap::new();
        let mut digests = BTreeMap::new();
        for (index, raw) in text.lines().enumerate() {
            let number = index + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [name, digest_hex] = fields.as_slice() else {
                return Err(Error::invalid(format!(
                    "identities line {number} does not have exactly two fields; each line is \
                     `<identity> <sha256 hex of the token>`. The line is not repeated here in \
                     case it holds a token: if it does, that token is exposed and must be rotated"
                )));
            };
            check_identity(name)
                .map_err(|reason| Error::invalid(format!("identities line {number}: {reason}")))?;
            let digest = parse_digest(digest_hex).ok_or_else(|| {
                Error::invalid(format!(
                    "identities line {number}: the second field is not a SHA-256 digest (64 \
                     lowercase hexadecimal characters). This file holds hashes of tokens, never \
                     tokens; if a token was written here it is exposed and must be rotated"
                ))
            })?;
            if let Some(first) = names.insert((*name).to_string(), number) {
                return Err(Error::invalid(format!(
                    "identities line {number}: the same identity as line {first}; one identity, \
                     one token"
                )));
            }
            if let Some(first) = digests.insert(digest, number) {
                return Err(Error::invalid(format!(
                    "identities line {number}: the same digest as line {first}. Two identities \
                     sharing one token cannot be told apart; give each producer its own token"
                )));
            }
            if entries.len() >= MAXIMUM_IDENTITIES {
                return Err(Error::invalid(format!(
                    "the identities file holds more than {MAXIMUM_IDENTITIES} identities, and \
                     every request is compared against every one"
                )));
            }
            entries.push(Entry {
                identity: Identity((*name).to_string()),
                digest,
            });
        }
        if entries.is_empty() {
            return Err(Error::invalid(
                "the identities file holds no identities, so every producer would be refused; \
                 add a `<identity> <sha256 hex of the token>` line per producer",
            ));
        }
        Ok(Self { entries })
    }

    /// How many identities the table accepts.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Always false for a table that loaded; present because clippy expects
    /// it beside [`IdentityTable::len`].
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl fmt::Debug for IdentityTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdentityTable")
            .field(
                "identities",
                &self
                    .entries
                    .iter()
                    .map(|entry| entry.identity.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Verify the [`HEADER`] value a request carried against `table`.
///
/// A missing or malformed header is refused, naming the scheme expected, and
/// never treated as an anonymous caller — an endpoint that admits an
/// unauthenticated request "for now" admits it for good. Every entry is
/// compared, with no early exit, through the constant-time comparison; see
/// the module documentation for why both halves matter. No refusal contains
/// the presented header or token.
pub fn verify(table: &IdentityTable, header: Option<&str>) -> Result<Identity> {
    let token = presented_token(header)?;
    let presented = sha256(token.as_bytes());
    let mut matched: Option<&Identity> = None;
    for entry in &table.entries {
        let same = constant_time_eq(&entry.digest, &presented);
        if same && matched.is_none() {
            matched = Some(&entry.identity);
        }
    }
    matched
        .cloned()
        .ok_or_else(|| Error::denied("the event-fabric bearer token was not recognised"))
}

/// Extract the token from a header value, refusing anything that is not
/// exactly `Bearer <token>` with a well-formed token.
fn presented_token(header: Option<&str>) -> Result<&str> {
    let Some(header) = header else {
        return Err(Error::denied(format!(
            "no {HEADER} header was presented; the event fabric requires `{SCHEME} <token>`"
        )));
    };
    let Some(token) = header
        .strip_prefix(SCHEME)
        .and_then(|rest| rest.strip_prefix(' '))
    else {
        return Err(Error::denied(format!(
            "the {HEADER} header does not use the {SCHEME} scheme; the event fabric requires \
             `{SCHEME} <token>`"
        )));
    };
    check_token_shape(token).map_err(|reason| {
        Error::denied(format!(
            "the {HEADER} header's {SCHEME} token is malformed: {reason}"
        ))
    })?;
    Ok(token)
}

/// Why a token is unusable, or nothing. The reason never quotes the token.
///
/// The alphabet is RFC 6750's `b64token`: letters, digits, `-._~+/`, then
/// optional trailing `=`. Whitespace and control characters are the common
/// accident — a newline from `echo`, a space from a copy — and each would
/// yield a digest the identities file was not computed from.
fn check_token_shape(token: &str) -> std::result::Result<(), String> {
    if token.len() < MINIMUM_TOKEN_LENGTH {
        return Err(format!(
            "the token is shorter than {MINIMUM_TOKEN_LENGTH} characters, which is too short to \
             be a generated secret"
        ));
    }
    if token.len() > MAXIMUM_TOKEN_LENGTH {
        return Err(format!(
            "the token is longer than {MAXIMUM_TOKEN_LENGTH} characters"
        ));
    }
    let body = token.trim_end_matches('=');
    let alphabet =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~' | '+' | '/');
    if body.is_empty() || !body.chars().all(alphabet) {
        return Err(
            "the token contains a character outside RFC 6750's token alphabet (letters, digits, \
             `-._~+/`, trailing `=`); whitespace from a copy or a trailing newline is the usual \
             cause"
                .to_string(),
        );
    }
    Ok(())
}

/// Whether `name` may name an identity. The reason never quotes a rejected
/// name that might be a token: a name failing the alphabet is not echoed.
fn check_identity(name: &str) -> std::result::Result<(), String> {
    if name.len() > MAXIMUM_IDENTITY_LENGTH {
        return Err(format!(
            "the identity is longer than {MAXIMUM_IDENTITY_LENGTH} characters"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("the identity may contain only letters, digits, `.`, `_` and `-`".to_string());
    }
    Ok(())
}

/// Decode a 64-character lowercase hex digest. `None` for anything else,
/// including uppercase: one canonical spelling means one file format, and a
/// file generated by a different tool is refused rather than guessed at.
fn parse_digest(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let bytes = qip_core::hash::from_hex(text)?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}
