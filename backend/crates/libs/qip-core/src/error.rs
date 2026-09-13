//! Platform error type.
//!
//! One error enum crosses crate boundaries. Variants describe *what kind of
//! failure* a caller must handle, not which module raised it — the message
//! carries the detail.

use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Input violated a documented precondition (bad symbol, negative size…).
    Invalid(String),
    /// A required entity, instrument, model or record does not exist.
    NotFound(String),
    /// Operation is legal but not permitted in the current state or role.
    Denied(String),
    /// Arithmetic overflow, non-convergence, or a numerically undefined result.
    Numeric(String),
    /// Serialization / schema / contract violation.
    Schema(String),
    /// Underlying storage, transport or provider failure.
    Io(String),
    /// A configured external dependency is not available in this deployment.
    Unavailable(String),
    /// Guard tripped: leakage, limit breach, kill switch, budget exhausted.
    Guard(String),
    /// Deadline exceeded.
    Timeout(String),
}

impl Error {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }
    pub fn denied(msg: impl Into<String>) -> Self {
        Self::Denied(msg.into())
    }
    pub fn numeric(msg: impl Into<String>) -> Self {
        Self::Numeric(msg.into())
    }
    pub fn schema(msg: impl Into<String>) -> Self {
        Self::Schema(msg.into())
    }
    pub fn io(msg: impl Into<String>) -> Self {
        Self::Io(msg.into())
    }
    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self::Unavailable(msg.into())
    }
    pub fn guard(msg: impl Into<String>) -> Self {
        Self::Guard(msg.into())
    }
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self::Timeout(msg.into())
    }

    /// Stable machine-readable code, surfaced in API responses and metrics.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid",
            Self::NotFound(_) => "not_found",
            Self::Denied(_) => "denied",
            Self::Numeric(_) => "numeric",
            Self::Schema(_) => "schema",
            Self::Io(_) => "io",
            Self::Unavailable(_) => "unavailable",
            Self::Guard(_) => "guard",
            Self::Timeout(_) => "timeout",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(m)
            | Self::NotFound(m)
            | Self::Denied(m)
            | Self::Numeric(m)
            | Self::Schema(m)
            | Self::Io(m)
            | Self::Unavailable(m)
            | Self::Guard(m)
            | Self::Timeout(m) => m,
        }
    }

    /// `self` with `message` in place of its own text, keeping this error's
    /// class.
    ///
    /// The rewrite a caller needs when the class was decided correctly the
    /// first time and only the words need to change — a start-up failure
    /// gaining the seam it came from, a resource that failed to release on
    /// the way out gaining a second failure's text. Kept as one match over
    /// every variant so that a caller cannot relabel by reconstructing the
    /// variant itself and silently changing its class in the process, which
    /// is exactly the mistake `qip-deepbrain`'s `main.rs` once had three
    /// near-identical copies of this match to make.
    pub fn relabelled(self, message: impl Into<String>) -> Self {
        let message = message.into();
        match self {
            Self::Invalid(_) => Self::Invalid(message),
            Self::NotFound(_) => Self::NotFound(message),
            Self::Denied(_) => Self::Denied(message),
            Self::Numeric(_) => Self::Numeric(message),
            Self::Schema(_) => Self::Schema(message),
            Self::Io(_) => Self::Io(message),
            Self::Unavailable(_) => Self::Unavailable(message),
            Self::Guard(_) => Self::Guard(message),
            Self::Timeout(_) => Self::Timeout(message),
        }
    }

    /// `self`, with a release that failed on the way out folded into its
    /// message, and this error's own class kept; `self` unchanged when
    /// `release` succeeded.
    ///
    /// A caller that already has a failure to report — the reason it is
    /// giving up — must not let a second failure, from releasing whatever
    /// the first failure left open (a socket, a vendor session), replace or
    /// silently vanish. `let _ = resource.shutdown(...);` at a `Result`'s
    /// error arm was that silent vanishing, found at four separate
    /// connector-release sites across three binaries by the review that
    /// asked for this method: each site already matched on the admission
    /// failure it exists to report, and each threw away whatever
    /// `shutdown` returned rather than folding it in. One method here
    /// rather than a fourth (or fifth) crate-local copy of the same
    /// class-preserving append, which is the shape that let the first three
    /// copies (`qip-deepbrain::main`'s `relabel`, `fold_releases` and
    /// `with_release`) drift out of sync with what they were documented to
    /// cover in the first place.
    pub fn and_release(self, release: Result<()>) -> Self {
        match release {
            Ok(()) => self,
            Err(failure) => {
                let message = format!(
                    "{}; and releasing what was left open on the way out failed too: {}",
                    self.message(),
                    failure.message()
                );
                self.relabelled(message)
            }
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Schema(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `relabelled` keeps the class and replaces only the text, across
    /// every variant — not just the one a test happens to construct.
    ///
    /// Mutated by making `relabelled` always return `Self::Invalid(message)`
    /// regardless of `self`'s own variant — confirmed the `Guard` case then
    /// fails (`code()` reads `"invalid"` instead of `"guard"`); restored,
    /// confirmed every variant passes again.
    #[test]
    fn relabelled_keeps_the_class_and_replaces_the_message() {
        let cases: &[(Error, &str)] = &[
            (Error::invalid("x"), "invalid"),
            (Error::not_found("x"), "not_found"),
            (Error::denied("x"), "denied"),
            (Error::numeric("x"), "numeric"),
            (Error::schema("x"), "schema"),
            (Error::io("x"), "io"),
            (Error::unavailable("x"), "unavailable"),
            (Error::guard("x"), "guard"),
            (Error::timeout("x"), "timeout"),
        ];
        for (error, code) in cases {
            let relabelled = error.clone().relabelled("new text");
            assert_eq!(
                relabelled.code(),
                *code,
                "relabelling a {code} error changed its class"
            );
            assert_eq!(relabelled.message(), "new text");
        }
    }

    /// `and_release(Ok(()))` is a no-op: the common case, where whatever was
    /// opened released cleanly, must not touch the error a caller is
    /// already reporting.
    #[test]
    fn and_release_of_a_clean_release_leaves_the_error_untouched() {
        let error = Error::invalid("the admission gate refused this manifest");
        let folded = error.clone().and_release(Ok(()));
        assert_eq!(folded, error);
    }

    /// `and_release(Err(_))` keeps the first failure's class and appends the
    /// second's message, so an operator sees both the fault that ended the
    /// attempt and the resource that is still held — the property
    /// `qip-deepbrain`'s `with_release` exists for, generalised so three
    /// other call sites do not each reinvent it.
    ///
    /// Mutated by having `and_release` discard `release` and always return
    /// `self` — confirmed this then loses the shutdown failure's text
    /// entirely (the `assert!` on it fails); restored, confirmed it appears
    /// again.
    #[test]
    fn and_release_of_a_failed_release_keeps_the_first_class_and_names_both_failures() {
        let primary = Error::denied("the licence lapsed between two polls");
        let release_failure = Error::io("the vendor socket would not close");

        let folded = primary.and_release(Err(release_failure));

        assert_eq!(
            folded.code(),
            "denied",
            "the class reported must be the primary failure's, not the release's"
        );
        assert!(
            folded
                .message()
                .contains("the licence lapsed between two polls"),
            "the original failure's text must survive: {}",
            folded.message()
        );
        assert!(
            folded
                .message()
                .contains("the vendor socket would not close"),
            "the release failure must not be silently dropped: {}",
            folded.message()
        );
    }
}
