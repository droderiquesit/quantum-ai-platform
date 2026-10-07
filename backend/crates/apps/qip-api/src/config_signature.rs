//! CICD-096: refuse to start on an environment configuration whose signature
//! does not verify.
//!
//! The decision lives here, as a pure function over values the composition
//! root has already read. `main.rs` only collects them. The process
//! environment cannot be set in a test without `unsafe`, which this workspace
//! forbids, so a check written inline in `main` could never be exercised.
//!
//! What it prevents, in the order it went wrong before:
//! - A signature checked under a key written in this crate's source. Unset,
//!   the key used to fall back to a literal, so a signature anyone could
//!   compute passed as a reviewed configuration. A signature with no
//!   provisioned key is now a refusal.
//! - A canonical form that contained the signature it is checked against.
//!   Every `QIP_*` variable went into it, `QIP_CONFIG_SIGNATURE` included, so
//!   a valid signature would have had to be an HMAC over itself and none
//!   could ever verify. The signature and its key are now left out.

use qip_core::config::Config;
use qip_core::error::{Error, Result};

/// The variable holding the hex HMAC-SHA256 over the canonical configuration.
pub const SIGNATURE_VARIABLE: &str = "QIP_CONFIG_SIGNATURE";
/// The variable naming the signing key, read through `qip_core::secret`, so
/// a deployment mounts it as `QIP_CONFIG_SIGNATURE_KEY_FILE`.
pub const KEY_VARIABLE: &str = "QIP_CONFIG_SIGNATURE_KEY";

/// The configuration the signature covers: every `QIP_*` variable except the
/// signature and its key (and the key's `_FILE` variant). The variables are
/// `QIP_<SECTION>__<FIELD>`, read as `section.field`.
pub fn canonical(variables: impl IntoIterator<Item = (String, String)>) -> Config {
    let mut config = Config::empty();
    for (name, value) in variables {
        let Some(rest) = name.strip_prefix("QIP_") else {
            continue;
        };
        if rest.starts_with("CONFIG_SIGNATURE") {
            continue;
        }
        config.set(
            &rest.to_lowercase().replace("__", "."),
            serde_json::Value::String(value),
        );
    }
    config
}

/// Admit the process when no signature is configured, or when the
/// configuration verifies under the provisioned key. Refuse it otherwise.
pub fn check(
    variables: impl IntoIterator<Item = (String, String)>,
    signature: Option<&str>,
    key: Option<&str>,
) -> Result<()> {
    let Some(signature) = signature.filter(|s| !s.is_empty()) else {
        // Not configured: the posture every deployment had before CICD-096.
        return Ok(());
    };
    let Some(key) = key.filter(|k| !k.is_empty()) else {
        return Err(Error::invalid(format!(
            "{SIGNATURE_VARIABLE} is set and {KEY_VARIABLE} is not, so there is no key to verify \
             the configuration under. Mount the signing key as {KEY_VARIABLE}_FILE, or unset \
             {SIGNATURE_VARIABLE}. A signature checked under a key nobody provisioned proves \
             nothing."
        )));
    };
    if canonical(variables).verify_signature(signature, key.as_bytes()) {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "the environment configuration does not match {SIGNATURE_VARIABLE}: a QIP_ variable \
             differs from the signed configuration, or the signature was made under another key"
        )))
    }
}
