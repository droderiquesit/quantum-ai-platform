//! Reading a credential the deployment supplied, from a variable or a file.
//!
//! Every credential this platform reads has two possible sources, and one
//! resolver so that all of them behave the same way.
//!
//! # Why a file at all
//!
//! An environment variable holding a credential is readable from
//! `/proc/<pid>/environ`, is inherited by every child process, and lands in a
//! crash dump. That is tolerable for a local run and it is not how a cluster
//! should carry a signing key.
//!
//! It is also what the deployment can actually provide. The Secret Manager CSI
//! driver projects a secret into a pod as a file; turning that back into an
//! environment variable means synchronising it into a Kubernetes Secret, which
//! writes the plaintext into etcd and — because a container's environment is
//! resolved before its CSI volume has been mounted — does not exist yet the
//! first time a pod starts on a new cluster. Reading the file directly has
//! neither problem.
//!
//! # The contract
//!
//! For a credential named `QIP_EXAMPLE`:
//!
//! * `QIP_EXAMPLE` set — that is the value.
//! * `QIP_EXAMPLE_FILE` set — the value is that file's contents.
//! * Neither — [`None`]. The caller decides whether absence is fatal; for some
//!   credentials it is and for others it means a feature stays off.
//! * **Both — refused.** Two sources that can disagree is a configuration
//!   whose behaviour depends on which branch this function happens to test
//!   first, and the failure that produces is a process authenticating with a
//!   credential nobody thinks is in use.
//!
//! Trailing whitespace is stripped from a file's contents, because `echo` adds
//! a newline and so does every editor, and a key that fails to verify for one
//! invisible byte is a bad afternoon. The cost is that a credential cannot end
//! in whitespace; no credential format in use here does.
//!
//! A file that exists and is empty is refused rather than returned. An empty
//! credential is never what was meant, and the callers that treat `None` as
//! "this feature is off" would silently take that path — turning a missing
//! secret into a disabled control.

use crate::error::{Error, Result};
use std::fmt;

/// The suffix naming the file variant of a credential variable.
pub const FILE_SUFFIX: &str = "_FILE";

/// Resolve a credential from `variable` or from the file `variable_FILE` names.
///
/// See the module documentation for the contract. Returns `Ok(None)` only when
/// neither is set; every other failure is an error that names the variable, so
/// an operator learns which of the two they got wrong rather than that "a
/// credential" is missing.
pub fn from_environment(variable: &str) -> Result<Option<String>> {
    let file_variable = format!("{variable}{FILE_SUFFIX}");
    resolve(
        variable,
        std::env::var(variable).ok(),
        std::env::var(&file_variable).ok(),
    )
}

/// The rule, with the two sources passed in rather than read.
///
/// Separated from [`from_environment`] so it can be tested. The process
/// environment is global and this workspace forbids `unsafe`, so
/// `std::env::set_var` — unsafe since the 2024 edition, because another thread
/// reading the environment concurrently is undefined — is not available to a
/// test here. A pure function takes the decision out of the part that cannot
/// be exercised.
///
/// Public for the same reason it exists: a composition root that has already
/// collected its variables into a map — because that is how it makes its own
/// configuration testable — should resolve a credential through the same rule
/// as everything else rather than reimplement the `_FILE` indirection beside
/// it. A second implementation of this rule is a second place for the two
/// sources to disagree.
pub fn resolve_from(
    variable: &str,
    direct: Option<String>,
    path: Option<String>,
) -> Result<Option<String>> {
    resolve(variable, direct, path)
}

fn resolve(variable: &str, direct: Option<String>, path: Option<String>) -> Result<Option<String>> {
    let file_variable = format!("{variable}{FILE_SUFFIX}");
    match (direct, path) {
        (Some(_), Some(_)) => Err(Error::invalid(format!(
            "{variable} and {file_variable} are both set. They can disagree, and which one \
             wins would be an implementation detail of the process rather than a decision \
             anybody made; set exactly one"
        ))),
        (Some(value), None) => Ok(Some(value)),
        (None, Some(path)) => Ok(Some(read_file(&file_variable, &path)?)),
        (None, None) => Ok(None),
    }
}

/// Read and trim a credential file, naming the variable that pointed at it.
///
/// The path is in the message because the variable alone does not say which
/// file was missing, and on a pod the interesting half is usually the mount
/// path rather than the variable holding it.
fn read_file(file_variable: &str, path: &str) -> Result<String> {
    let contents = std::fs::read_to_string(path).map_err(|error| {
        Error::invalid(format!(
            "{file_variable} names {path}, which could not be read: {error}"
        ))
    })?;

    let value = contents.trim_end();
    if value.is_empty() {
        return Err(Error::invalid(format!(
            "{file_variable} names {path}, which is empty. An empty credential is not a \
             credential, and returning it as absent would turn a missing secret into a \
             disabled control"
        )));
    }

    Ok(value.to_string())
}

// --- the redacting value ----------------------------------------------------

// Here rather than in `qip-brokers`, where it was defined until 2026-10-24,
// so a workload can hold a credential in it without linking the crate that
// holds the REST order-entry adapter (R12-GCP-8114). `qip_brokers::Secret`
// re-exports it, so no adapter changed.

/// A secret value, which can be handed on but not looked at by accident.
///
/// The three ways a secret escapes are a log line, a serialised event and a
/// panic message, and all three go through the same two traits. So `Debug` and
/// `Display` are written by hand and redact, and neither is derived: a derive
/// here would print the value, and it would do it in the one place nobody
/// re-reads — an error path.
///
/// `Serialize` and `Deserialize` are not implemented *at all*, which is the
/// stronger statement. A struct holding a `Secret` cannot derive them either,
/// so the compiler refuses the snapshot rather than emitting one with a token
/// in it.
///
/// The bytes are zeroed on drop. That is best-effort — a `Clone`, a reallocated
/// `Vec` or a core dump can still hold a copy — but it costs one safe line and
/// shortens the window, and this workspace forbids the `unsafe` a stronger
/// guarantee would need.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret {
    bytes: Vec<u8>,
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            bytes: value.into().into_bytes(),
        }
    }

    /// Hand the value out.
    ///
    /// Named to be conspicuous: a reviewer scanning a diff should see the word
    /// `expose` at every point a secret leaves this type, and every such point
    /// should be a transport writing an authentication frame.
    pub fn expose_str(&self) -> Result<&str> {
        std::str::from_utf8(&self.bytes).map_err(|_| {
            Error::invalid("this secret is not valid UTF-8 and cannot be read as text")
        })
    }

    /// Whether anything was supplied. One bit, and the bit an operator needs:
    /// an environment variable set to the empty string is the failure that
    /// looks exactly like success.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately not `debug_struct`: no field, no length, no prefix. A
        // length is a hint and a prefix is a head start.
        f.write_str("Secret(<redacted>)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    /// A path in the scratch directory, unique per test so two tests running
    /// concurrently cannot see each other's fixture.
    fn fixture(name: &str, contents: &str) -> Result<std::path::PathBuf> {
        let path = std::env::temp_dir().join(format!("qip-secret-{name}"));
        std::fs::write(&path, contents)
            .map_err(|error| Error::invalid(format!("could not write the fixture: {error}")))?;
        Ok(path)
    }

    #[test]
    fn an_unset_credential_is_absent_rather_than_an_error() -> Result<()> {
        assert_eq!(resolve("QIP_EXAMPLE", None, None)?, None);
        Ok(())
    }

    #[test]
    fn the_variable_is_used_when_it_is_the_only_one_set() -> Result<()> {
        assert_eq!(
            resolve(
                "QIP_EXAMPLE",
                Some("from-the-environment".to_string()),
                None
            )?,
            Some("from-the-environment".to_string())
        );
        Ok(())
    }

    #[test]
    fn a_file_supplies_the_credential_and_its_trailing_newline_is_not_part_of_it() -> Result<()> {
        // The newline is the point: it is what `echo secret > file` writes,
        // and a key that verifies only without it fails in the field for a
        // byte nobody can see.
        let path = fixture("newline", "a-value-from-a-file\n")?;

        assert_eq!(
            resolve("QIP_EXAMPLE", None, Some(path.display().to_string()))?,
            Some("a-value-from-a-file".to_string())
        );

        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn setting_both_is_refused_rather_than_one_of_them_quietly_winning() -> Result<()> {
        let error = resolve(
            "QIP_EXAMPLE",
            Some("from-the-environment".to_string()),
            Some("/nonexistent".to_string()),
        )
        .expect_err("both sources were set and one of them won");

        assert!(
            error.message().contains("QIP_EXAMPLE_FILE"),
            "the refusal does not name the file variable, so an operator cannot tell which \
             of the two to remove: {}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn an_unreadable_file_names_the_path_rather_than_reporting_no_credential() -> Result<()> {
        let error = resolve(
            "QIP_EXAMPLE",
            None,
            Some("/var/run/secrets/qip/definitely-not-here".to_string()),
        )
        .expect_err("a missing credential file was reported as no credential at all");

        assert!(
            error.message().contains("definitely-not-here"),
            "the refusal does not name the path: {}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn an_empty_file_is_refused_rather_than_read_as_an_absent_credential() -> Result<()> {
        // Whitespace rather than nothing, because trimming is what makes this
        // empty: a check on file length alone would pass this file through.
        let path = fixture("empty", "  \n")?;

        let error = resolve("QIP_EXAMPLE", None, Some(path.display().to_string()))
            .expect_err("an empty credential file was accepted or read as absent");
        assert!(error.message().contains("empty"));

        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn the_file_variable_is_the_variable_with_one_suffix() {
        // Pins the name the deployment has to use. The manifests and the
        // runbook both write `QIP_CAPITAL_ENVELOPE_KEY_FILE` by hand, and a
        // change to this suffix would break them silently — the credential
        // would simply read as absent.
        assert_eq!(FILE_SUFFIX, "_FILE");
    }
}
