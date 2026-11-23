//! The unit tests' handle on a book decision the cost router refuses
//! (round-81 review, R12-RISK-3022).
//!
//! `Router::determine` refuses only a blank subject or an invalid policy,
//! and the platform holds `Router::default` with no setter, so every arm that
//! answers a refused routing — the survival reseed, sizing, sign-off,
//! release and the reconciliation halt — ran in no test and no deployment.
//! [`RefusedBookDecisions`] is what `Platform::determine_book_decision`
//! consults before the router, and it can refuse only under `#[cfg(test)]`.
//!
//! In its own file, not in `platform.rs`, on purpose. `qip-acceptance`'s
//! `security` weight-mover scan and `api_boundary`'s mutator scan read
//! `platform.rs` only up to its first `#[cfg(test)]`, and the first version
//! of this seam put that attribute on a `Platform` field near the top of the
//! file: the scans went blind to every shipped method after it, and
//! `security.rs` reported `issue_capital` and `set_central` as vanished.

use qip_core::error::Error;

/// The book-decision subjects a unit test has asked the platform to refuse.
///
/// Zero-sized outside `#[cfg(test)]`: a deployed binary holds one with no
/// field at all, [`Self::refusal`] answers `None` for every subject, and
/// nothing outside this crate's unit tests can make it answer anything else —
/// no configuration can make a production routing refuse, and none can make
/// one succeed that would not.
#[derive(Debug, Default)]
pub(crate) struct RefusedBookDecisions {
    #[cfg(test)]
    subjects: std::collections::BTreeSet<String>,
}

impl RefusedBookDecisions {
    /// The refusal to return for `subject` instead of routing it, if a unit
    /// test asked for one.
    pub(crate) fn refusal(&self, subject: &str) -> Option<Error> {
        #[cfg(test)]
        if self.subjects.contains(subject) {
            return Some(Error::denied(format!(
                "the unit test's router refuses '{subject}'; this refusal exists only under \
                 #[cfg(test)]"
            )));
        }
        let _ = subject;
        None
    }

    /// Refuse `subject` from now on. Test-only.
    #[cfg(test)]
    pub(crate) fn refuse(&mut self, subject: impl Into<String>) {
        self.subjects.insert(subject.into());
    }
}
