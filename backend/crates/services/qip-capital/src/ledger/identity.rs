//! Who a book belongs to, and where they are.
//!
//! Both are validated newtypes because both become map keys. A user id that
//! is accepted with trailing whitespace is two users the moment one caller
//! trims and another does not, and each half of that person's capital sits
//! in a book the other half cannot see. Refusing at construction is cheaper
//! than reconciling two books that were always one.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The longest user id the ledger keys on.
///
/// A bound rather than a guess: the id reaches the journal and the mesh, and
/// an unbounded key is an unbounded record.
pub const MAX_USER_ID_LENGTH: usize = 64;

/// A user the ledger holds books for.
///
/// Non-empty, at most [`MAX_USER_ID_LENGTH`] bytes, ASCII letters, digits,
/// `-`, `_` and `.` only, and stored exactly as validated — never trimmed or
/// lower-cased on the way in, because a normalisation the caller did not ask
/// for is a second identity nobody recorded.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UserId(String);

impl UserId {
    /// Validate an id, refusing by name what is wrong with it.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.is_empty() {
            return Err(Error::invalid(
                "a user id cannot be empty; a book with no owner is a book nobody can be shown",
            ));
        }
        if id.len() > MAX_USER_ID_LENGTH {
            return Err(Error::invalid(format!(
                "a user id of {} bytes exceeds the {MAX_USER_ID_LENGTH}-byte bound; the id \
                 reaches the journal and an unbounded key is an unbounded record",
                id.len()
            )));
        }
        if id != id.trim() {
            return Err(Error::invalid(format!(
                "the user id {id:?} carries leading or trailing whitespace; trim it before \
                 enrolling, or one person becomes two books"
            )));
        }
        if let Some(offender) = id
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        {
            return Err(Error::invalid(format!(
                "the user id {id:?} contains {offender:?}; only ASCII letters, digits, '-', \
                 '_' and '.' are accepted"
            )));
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UserId({})", self.0)
    }
}

impl TryFrom<String> for UserId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<UserId> for String {
    fn from(id: UserId) -> Self {
        id.0
    }
}

/// Where a user is, for product eligibility.
///
/// Two ASCII letters, upper-cased on the way in — the one normalisation this
/// module performs, because ISO 3166 codes are case-insensitive by
/// definition and `gb` and `GB` are the same place, not two. Anything else
/// is refused: a jurisdiction that cannot be looked up in an eligibility
/// table is an eligibility check that always fails open or always fails
/// closed, and neither is a check.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Jurisdiction([u8; 2]);

impl Jurisdiction {
    pub fn new(code: &str) -> Result<Self> {
        let bytes = code.as_bytes();
        if bytes.len() != 2 || !bytes.iter().all(u8::is_ascii_alphabetic) {
            return Err(Error::invalid(format!(
                "the jurisdiction {code:?} is not a two-letter ISO 3166 alpha-2 code"
            )));
        }
        Ok(Self([
            bytes[0].to_ascii_uppercase(),
            bytes[1].to_ascii_uppercase(),
        ]))
    }

    pub fn as_str(&self) -> &str {
        // Both bytes were proven ASCII alphabetic at construction, so the
        // buffer is valid UTF-8 by construction; the fallback is unreachable
        // and stated rather than unwrapped.
        std::str::from_utf8(&self.0).unwrap_or("??")
    }
}

impl fmt::Display for Jurisdiction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Jurisdiction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Jurisdiction({})", self.as_str())
    }
}

impl TryFrom<String> for Jurisdiction {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::new(&value)
    }
}

impl From<Jurisdiction> for String {
    fn from(jurisdiction: Jurisdiction) -> Self {
        jurisdiction.as_str().to_string()
    }
}
