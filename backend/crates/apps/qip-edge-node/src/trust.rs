//! What this node verifies the centre against, chosen once at start-up
//! (ADR 0110).
//!
//! A node is configured with **one** of two roots, never both:
//!
//! * `QIP_TRUST_BUNDLE_PATH` — the committed trust bundle baked into the
//!   attested image. The node holds public keys only: policy payloads, halts
//!   and capital envelopes must carry `ecdsa-p256-sha256` signatures under
//!   the key published for their class, and an HMAC is refused.
//! * `QIP_CAPITAL_ENVELOPE_KEY` (or its `_FILE`) — the shared HMAC key, for
//!   the release ADR 0110 keeps it. Outside a `local` deployment it is
//!   refused from [`qip_edge::trust::SHARED_KEY_REMOVED_AFTER`].
//!
//! Both set is refused rather than resolved by precedence. A node with the
//! bundle *and* the key on its disk still holds the secret ADR 0110 removes,
//! and a precedence rule would hide that the mount was never taken away.
//!
//! A bundle that cannot verify one of the three classes this node receives
//! is refused at start-up: it would otherwise refuse every frame of that
//! class, which is an outage that looks like an attack.

use qip_contracts::trust::ArtefactClass;
use qip_core::Timestamp;
use qip_core::deployment::Deployment;
use qip_core::error::{Error, Result};
use qip_edge::TrustRoot;
use qip_transport::trust::BundleVerifier;
use std::path::Path;

/// The trust bundle's path.
pub const TRUST_BUNDLE_VARIABLE: &str = "QIP_TRUST_BUNDLE_PATH";

/// The classes a cell verifies. Model packs are not among them: a cell runs
/// inline models the signed payload's manifest names by digest.
const RECEIVED: [ArtefactClass; 3] = [
    ArtefactClass::Policy,
    ArtefactClass::Halt,
    ArtefactClass::Envelope,
];

/// Choose the node's root from what the deployment configured.
pub fn choose_root(
    bundle_path: Option<&str>,
    shared_key: Option<&[u8]>,
    deployment: Deployment,
    now: Timestamp,
) -> Result<TrustRoot> {
    let bundle_path = bundle_path.map(str::trim).filter(|path| !path.is_empty());
    let shared_key = shared_key.filter(|key| !key.is_empty());
    match (bundle_path, shared_key) {
        (Some(_), Some(_)) => Err(Error::denied(format!(
            "configuration: {TRUST_BUNDLE_VARIABLE} and QIP_CAPITAL_ENVELOPE_KEY are both set. \
             A node verifying with the trust bundle must not also hold the shared key: the key \
             can mint what the bundle verifies, and keeping it mounted keeps the risk ADR 0110 \
             removes. Remove the key's mount"
        ))),
        (Some(path), None) => {
            let verifier = BundleVerifier::from_file(Path::new(path))
                .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
            let usable = verifier.bundle().classes_at(now);
            let missing: Vec<&str> = RECEIVED
                .iter()
                .filter(|class| !usable.contains(class))
                .map(|class| class.as_str())
                .collect();
            if !missing.is_empty() {
                return Err(Error::invalid(format!(
                    "configuration: the trust bundle at {path} has no usable key for {} at \
                     start-up; this node would refuse every such frame. Ship a bundle carrying \
                     the active version of each class",
                    missing.join(", ")
                )));
            }
            Ok(TrustRoot::bundle(verifier))
        }
        (None, Some(key)) => {
            if !deployment.permits_reproducible_keys() {
                qip_edge::trust::refuse_shared_key_after_removal(now).map_err(|error| {
                    Error::denied(format!("configuration: {}", error.message()))
                })?;
            }
            // The same cut, carried into every check: the start-up refusal
            // alone let a node started before the date verify with the key
            // for as long as it stayed up.
            TrustRoot::shared_key_in(key, deployment)
        }
        (None, None) => Err(Error::invalid(format!(
            "configuration: the node has nothing to verify the centre against; set \
             {TRUST_BUNDLE_VARIABLE} to the committed trust bundle (or, until {}, \
             QIP_CAPITAL_ENVELOPE_KEY). A cell that cannot verify a grant must not trade",
            qip_edge::trust::SHARED_KEY_REMOVED_AFTER
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"an-edge-node-shared-key-of-32-bytes";

    fn bundle_file(classes: &[ArtefactClass], name: &str) -> std::path::PathBuf {
        let keys: Vec<qip_contracts::trust::TrustedKey> = classes
            .iter()
            .map(|class| {
                let signer =
                    qip_transport::trust::LocalSigner::generate(*class, format!("test/{class}/1"))
                        .expect("key");
                qip_contracts::trust::TrustedKey {
                    class: *class,
                    key_version: format!("test/{class}/1"),
                    public_key: signer.public_key_hex(),
                    not_after: Timestamp::from_secs(4_000_000_000),
                }
            })
            .collect();
        let bundle =
            qip_contracts::trust::TrustBundle::new(keys, Default::default()).expect("bundle");
        let path = std::env::temp_dir().join(format!(
            "qip-edge-node-trust-{}-{name}.json",
            std::process::id()
        ));
        std::fs::write(&path, serde_json::to_string(&bundle).expect("json")).expect("write");
        path
    }

    /// Mutation: give the bundle precedence instead of refusing both, and
    /// the node starts holding the secret ADR 0110 removes; this fails.
    #[test]
    fn a_node_given_both_the_bundle_and_the_shared_key_refuses_to_start() {
        let path = bundle_file(&RECEIVED, "both");
        let now = Timestamp::from_secs(1_800_000_000);
        // Premise: each alone is accepted.
        assert!(
            choose_root(path.to_str(), None, Deployment::Prod, now)
                .expect("the bundle alone")
                .is_asymmetric()
        );
        assert!(choose_root(None, Some(KEY), Deployment::Prod, now).is_ok());
        assert!(choose_root(path.to_str(), Some(KEY), Deployment::Prod, now).is_err());
        let _ = std::fs::remove_file(path);
    }

    /// Mutation: drop the class-coverage check and a bundle with no halt key
    /// starts a node that refuses every halt; this fails.
    #[test]
    fn a_bundle_missing_a_class_the_cell_receives_is_refused_at_start_up() {
        let path = bundle_file(&[ArtefactClass::Policy, ArtefactClass::Envelope], "no-halt");
        let error = choose_root(
            path.to_str(),
            None,
            Deployment::Prod,
            Timestamp::from_secs(1_800_000_000),
        )
        .expect_err("a bundle with no halt key was accepted");
        assert!(error.message().contains("halt"), "{}", error.message());
        let _ = std::fs::remove_file(path);
    }

    /// Mutation: skip the removal date outside `local` and a deployed node
    /// keeps the shared key after it; this fails.
    #[test]
    fn the_shared_key_stops_a_deployed_node_after_its_removal_date_but_not_a_local_one() {
        let after = Timestamp::from_civil(2027, 6, 1);
        assert!(choose_root(None, Some(KEY), Deployment::Dev, after).is_err());
        assert!(choose_root(None, Some(KEY), Deployment::Local, after).is_ok());
        assert!(choose_root(None, None, Deployment::Local, after).is_err());
    }

    /// The failure this prevents: a node started the day before the removal
    /// date passed the start-up refusal and then verified under the shared
    /// key for as long as it ran. Mutation: build the root with
    /// `TrustRoot::shared_key_in(key, Deployment::Local)` in `choose_root`
    /// and the deployed node's root accepts the MAC after the date; build it
    /// with `TrustRoot::shared_key(key)` and the local one refuses it. Either
    /// fails this.
    #[test]
    fn a_deployed_node_started_before_the_removal_date_refuses_the_shared_key_once_it_passes() {
        let before = Timestamp::from_civil(2027, 1, 31);
        let after = Timestamp::from_civil(2027, 2, 1);
        let mac = qip_core::hash::to_hex(&qip_core::hmac_sha256(KEY, b"payload"));
        let deployed = choose_root(None, Some(KEY), Deployment::Dev, before)
            .expect("premise: the deployed node starts before the date");
        assert!(
            deployed
                .check(ArtefactClass::Policy, b"payload", &mac, before)
                .is_ok()
        );
        assert!(
            deployed
                .check(ArtefactClass::Policy, b"payload", &mac, after)
                .is_err(),
            "a deployed node kept verifying with the shared key past its removal date"
        );
        let local = choose_root(None, Some(KEY), Deployment::Local, before).expect("local starts");
        assert!(
            local
                .check(ArtefactClass::Policy, b"payload", &mac, after)
                .is_ok()
        );
    }
}
