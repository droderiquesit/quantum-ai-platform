//! Which signer the centre's artefacts go out under (ADR 0110), chosen once
//! at start-up.
//!
//! The API is the process that dispatches policy payloads, halts and capital
//! envelopes down the mesh, so it is the one that signs them. Three postures,
//! read from `QIP_ARTEFACT_SIGNER`:
//!
//! * `kms` — each class signs through its own Cloud KMS key version
//!   ([`qip_transport::kms::KmsSigner`]): `asymmetricSign` over TLS with a
//!   metadata-server token, the key never in this process. The deployment
//!   names the versions in a small JSON file and ships the trust bundle and
//!   the pinned Google roots beside it.
//! * `local` — keys in a file on a developer's machine
//!   ([`qip_transport::trust::LocalSigner`]). **Refused unless
//!   `QIP_DEPLOYMENT=local`**: a private key on a deployed disk is the thing
//!   ADR 0110 exists to remove.
//! * unset — the shared HMAC key `harden_central` installed, for the one
//!   release ADR 0110 keeps it. Outside `local` it is refused from
//!   [`qip_transport::trust::SHARED_KEY_REMOVED_AFTER`], the same date every cell
//!   stops accepting it.
//!
//! Every asymmetric signer is proven before the process serves: it signs a
//! probe and the probe is verified against the trust bundle — the bundle the
//! cells verify against — so a key ring and a bundle that disagree stop the
//! API at start-up instead of producing a fleet that refuses every frame.
//!
//! Halts are signed by the `halt` key alone. Nothing here asks a halt for a
//! second approver, deliberately: a control that stops trading must never
//! wait on one (ADR 0110 decision 4).

use qip_contracts::trust::{ArtefactClass, ArtefactSigner, ArtefactVerifier};
use qip_core::Timestamp;
use qip_core::deployment::Deployment;
use qip_core::error::{Error, Result};
use qip_core::hash::from_hex;
use qip_transport::kms::{BearerSource, KmsSigner, prove_signer};
use qip_transport::trust::{BundleVerifier, LocalSigner};
use qip_transport::{TlsHttpClient, tls};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const ARTEFACT_SIGNER_VARIABLE: &str = "QIP_ARTEFACT_SIGNER";
pub const TRUST_BUNDLE_VARIABLE: &str = "QIP_TRUST_BUNDLE_PATH";
pub const KMS_KEY_VERSIONS_VARIABLE: &str = "QIP_KMS_KEY_VERSIONS_PATH";
pub const GOOGLE_TLS_ROOTS_VARIABLE: &str = "QIP_GOOGLE_TLS_ROOTS_PATH";
pub const LOCAL_SIGNING_KEYS_VARIABLE: &str = "QIP_LOCAL_SIGNING_KEYS_PATH";
/// The signed release packs (ADR 0110) the `trained_models` slot is held to:
/// a JSON array of `qip_contracts::policy::ModelPack`.
pub const MODEL_PACKS_VARIABLE: &str = "QIP_MODEL_PACKS_PATH";

/// The classes the API signs. Model packs are signed by the publish
/// workflow, never by a running process.
const SIGNED_HERE: [ArtefactClass; 3] = [
    ArtefactClass::Policy,
    ArtefactClass::Halt,
    ArtefactClass::Envelope,
];

/// The raw configuration, read by `main` and nothing else.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SigningSettings {
    pub mode: Option<String>,
    pub trust_bundle: Option<PathBuf>,
    pub kms_key_versions: Option<PathBuf>,
    pub google_roots: Option<PathBuf>,
    pub local_keys: Option<PathBuf>,
}

impl SigningSettings {
    pub fn from_env(read: &dyn Fn(&str) -> Option<String>) -> Self {
        let path = |name: &str| {
            read(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            mode: read(ARTEFACT_SIGNER_VARIABLE)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            trust_bundle: path(TRUST_BUNDLE_VARIABLE),
            kms_key_versions: path(KMS_KEY_VERSIONS_VARIABLE),
            google_roots: path(GOOGLE_TLS_ROOTS_VARIABLE),
            local_keys: path(LOCAL_SIGNING_KEYS_VARIABLE),
        }
    }
}

/// The three signers and the verifier they were proven against.
#[derive(Clone, Debug)]
pub struct ArtefactSigners {
    pub policy: Arc<dyn ArtefactSigner>,
    pub halt: Arc<dyn ArtefactSigner>,
    pub envelope: Arc<dyn ArtefactSigner>,
    pub verifier: Arc<BundleVerifier>,
}

/// What the API signs with.
#[derive(Clone, Debug)]
pub enum SigningPosture {
    /// ADR 0110: every artefact under its class key.
    Asymmetric(ArtefactSigners),
    /// The shared key `harden_central` installed (or none, which leaves
    /// policy distribution off and counted).
    SharedKey,
}

impl SigningPosture {
    pub fn describe(&self) -> String {
        match self {
            Self::Asymmetric(signers) => format!(
                "ECDSA P-256 per class (ADR 0110): policy {}, halt {}, envelope {}",
                signers.policy.key_version(),
                signers.halt.key_version(),
                signers.envelope.key_version()
            ),
            Self::SharedKey => format!(
                "SHARED HMAC KEY — every cell that verifies can also sign; refused outside a \
                 local deployment from {}. Set {ARTEFACT_SIGNER_VARIABLE}=kms",
                qip_transport::trust::SHARED_KEY_REMOVED_AFTER
            ),
        }
    }
}

/// The deployment's KMS key versions, one per class this process signs.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KmsKeyVersions {
    policy: String,
    halt: String,
    envelope: String,
}

/// One local key: the version name the bundle lists it under, and the
/// PKCS#8 document in hex.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalKey {
    key_version: String,
    pkcs8_hex: String,
}

fn read_file(path: &Path, what: &str) -> Result<String> {
    std::fs::read_to_string(path).map_err(|error| {
        Error::invalid(format!(
            "cannot read the {what} at {}: {error}",
            path.display()
        ))
    })
}

fn required<'a>(value: Option<&'a PathBuf>, variable: &str, mode: &str) -> Result<&'a PathBuf> {
    value.ok_or_else(|| {
        Error::invalid(format!(
            "{ARTEFACT_SIGNER_VARIABLE}={mode} needs {variable}; without it nothing the API signs \
             can be checked against what the cells hold"
        ))
    })
}

/// Choose and prove the signing posture.
///
/// `bearer` is only called for `kms`, and `clock` is what each KMS signer
/// asks the bundle about.
pub fn configure(
    settings: &SigningSettings,
    deployment: Deployment,
    now: Timestamp,
    bearer: &dyn Fn() -> Arc<dyn BearerSource>,
    clock: Arc<dyn qip_core::Clock>,
) -> Result<SigningPosture> {
    let Some(mode) = settings.mode.as_deref() else {
        if !deployment.permits_reproducible_keys() {
            qip_transport::trust::refuse_shared_key_after_removal(now)?;
        }
        return Ok(SigningPosture::SharedKey);
    };
    let verifier = match mode {
        "kms" | "local" => Arc::new(BundleVerifier::from_file(required(
            settings.trust_bundle.as_ref(),
            TRUST_BUNDLE_VARIABLE,
            mode,
        )?)?),
        other => {
            return Err(Error::invalid(format!(
                "{ARTEFACT_SIGNER_VARIABLE}={other:?} is not a signer; use `kms`, or `local` on a \
                 developer machine, or leave it unset for the shared key's last release"
            )));
        }
    };
    let mut signers: BTreeMap<ArtefactClass, Arc<dyn ArtefactSigner>> = BTreeMap::new();
    if mode == "kms" {
        let versions: KmsKeyVersions = serde_json::from_str(&read_file(
            required(
                settings.kms_key_versions.as_ref(),
                KMS_KEY_VERSIONS_VARIABLE,
                mode,
            )?,
            "KMS key-version map",
        )?)
        .map_err(|error| {
            Error::invalid(format!(
                "the KMS key-version map does not parse — it is {{\"policy\", \"halt\", \
                 \"envelope\"}} each a full cryptoKeyVersions resource name: {error}"
            ))
        })?;
        let roots = read_file(
            required(
                settings.google_roots.as_ref(),
                GOOGLE_TLS_ROOTS_VARIABLE,
                mode,
            )?,
            "Google TLS roots",
        )?;
        let client = TlsHttpClient::new(
            KmsSigner::limits(),
            tls::server_authenticated_client(roots.as_bytes())?,
        );
        let source = bearer();
        for (class, version) in [
            (ArtefactClass::Policy, versions.policy),
            (ArtefactClass::Halt, versions.halt),
            (ArtefactClass::Envelope, versions.envelope),
        ] {
            let signer = KmsSigner::new(
                class,
                version,
                client.clone(),
                Arc::clone(&source),
                verifier.as_ref().clone(),
                Arc::clone(&clock),
            )?;
            signers.insert(class, Arc::new(signer));
        }
    } else {
        if !deployment.permits_reproducible_keys() {
            return Err(Error::denied(format!(
                "{ARTEFACT_SIGNER_VARIABLE}=local in the {deployment} deployment; a private key on \
                 a deployed disk is what ADR 0110 removes. Use `kms`"
            )));
        }
        let keys: BTreeMap<String, LocalKey> = serde_json::from_str(&read_file(
            required(
                settings.local_keys.as_ref(),
                LOCAL_SIGNING_KEYS_VARIABLE,
                mode,
            )?,
            "local signing keys",
        )?)
        .map_err(|error| {
            Error::invalid(format!(
                "the local signing keys file does not parse — it maps policy, halt and envelope \
                 to {{\"key_version\", \"pkcs8_hex\"}}: {error}"
            ))
        })?;
        for class in SIGNED_HERE {
            let key = keys.get(class.as_str()).ok_or_else(|| {
                Error::invalid(format!("the local signing keys file has no {class} key"))
            })?;
            let pkcs8 = from_hex(key.pkcs8_hex.trim()).ok_or_else(|| {
                Error::invalid(format!("the local {class} key is not hexadecimal"))
            })?;
            signers.insert(
                class,
                Arc::new(LocalSigner::from_pkcs8(class, &key.key_version, &pkcs8)?),
            );
        }
    }
    let take = |class: ArtefactClass| -> Result<Arc<dyn ArtefactSigner>> {
        let signer = signers
            .get(&class)
            .cloned()
            .ok_or_else(|| Error::invalid(format!("no {class} signer was configured")))?;
        prove_signer(
            signer.as_ref(),
            verifier.as_ref() as &dyn ArtefactVerifier,
            now,
        )
        .map_err(|error| {
            Error::denied(format!(
                "the {class} signer ({}) does not verify under the trust bundle the cells \
                     hold: {}",
                signer.key_version(),
                error.message()
            ))
        })?;
        Ok(signer)
    };
    Ok(SigningPosture::Asymmetric(ArtefactSigners {
        policy: take(ArtefactClass::Policy)?,
        halt: take(ArtefactClass::Halt)?,
        envelope: take(ArtefactClass::Envelope)?,
        verifier,
    }))
}

/// A verifier that refuses everything, for holding the slot to an empty
/// release where no bundle is configured: with no packs it is never asked.
#[derive(Debug)]
struct RefusesEverything;

impl ArtefactVerifier for RefusesEverything {
    fn verify(
        &self,
        class: ArtefactClass,
        _canonical: &[u8],
        _signature: &str,
        _now: Timestamp,
    ) -> Result<()> {
        Err(Error::denied(format!(
            "no trust bundle is configured, so no {class} artefact can verify"
        )))
    }
}

/// Hold the `trained_models` slot to signed release packs (ADR 0110,
/// R12-REFLEX-9011): a cell runs inline only what a two-person release
/// signed under the `model-pack` key.
///
/// * packs configured — the trust bundle is required, every pack is
///   verified, and one that fails stops the API.
/// * no packs, outside `local` — the slot is held to an empty release and
///   names no distillate to any cell. Fail closed: an unreviewed training
///   run never reaches the order path because a variable was missing.
/// * no packs, `local` — ungated, as before, and the banner says so.
///
/// Returns the banner line.
pub fn hold_models_to_release(
    platform: &mut qip_kernel::Platform,
    deployment: Deployment,
    packs: Option<&Path>,
    trust_bundle: Option<&Path>,
    now: Timestamp,
) -> Result<String> {
    match packs {
        Some(path) => {
            let bundle = trust_bundle.ok_or_else(|| {
                Error::invalid(format!(
                    "{MODEL_PACKS_VARIABLE} is set and {TRUST_BUNDLE_VARIABLE} is not; a release \
                     pack verifies only against the trust bundle's model-pack key"
                ))
            })?;
            let verifier = BundleVerifier::from_file(bundle)?;
            let packs: Vec<qip_contracts::policy::ModelPack> =
                serde_json::from_str(&read_file(path, "release packs")?).map_err(|error| {
                    Error::invalid(format!(
                        "the release packs at {} do not parse: {error}",
                        path.display()
                    ))
                })?;
            let admitted = platform.admit_model_packs(packs, &verifier, now)?;
            Ok(format!(
                "{admitted} signed release pack(s) admitted; the trained_models slot names only \
                 their artifacts (ADR 0110)"
            ))
        }
        None if !deployment.permits_reproducible_keys() => {
            platform.admit_model_packs(Vec::new(), &RefusesEverything, now)?;
            Ok(format!(
                "no {MODEL_PACKS_VARIABLE}: no signed release, so no distillate is named to any \
                 cell and every inline model is refused (ADR 0110, fail closed)"
            ))
        }
        None => Ok(format!(
            "UNGATED — {MODEL_PACKS_VARIABLE} is unset in a local deployment, so every promoted \
             distillate is named to the cells without a signed release"
        )),
    }
}

/// The metadata server's tokens as the KMS signer's bearer source.
#[derive(Debug)]
pub struct MetadataBearer(pub qip_storage::gcp::auth::MetadataServerTokens);

impl BearerSource for MetadataBearer {
    fn bearer(&self) -> Result<String> {
        use qip_storage::gcp::auth::TokenSource;
        Ok(self.0.token()?.expose().to_string())
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_contracts::trust::{TrustBundle, TrustedKey};
    use std::collections::BTreeSet;

    #[derive(Debug)]
    struct NoBearer;
    impl BearerSource for NoBearer {
        fn bearer(&self) -> Result<String> {
            Err(Error::unavailable("no token in a test"))
        }
    }

    fn bearer() -> Arc<dyn BearerSource> {
        Arc::new(NoBearer)
    }

    fn clock() -> Arc<dyn qip_core::Clock> {
        Arc::new(qip_core::ManualClock::new(Timestamp::from_secs(
            1_800_000_000,
        )))
    }

    const NOW: Timestamp = Timestamp::from_secs(1_800_000_000);

    /// Mutation: drop the deployment check on `local` and a private key on a
    /// deployed disk signs the fleet's policy; this fails before any file is
    /// read.
    #[test]
    fn a_local_signer_is_refused_in_every_deployment_but_local() {
        let dir = std::env::temp_dir().join(format!("qip-signing-local-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let signers: Vec<LocalSigner> = SIGNED_HERE
            .iter()
            .map(|class| LocalSigner::generate(*class, format!("local/{class}/1")).expect("key"))
            .collect();
        let bundle = TrustBundle::new(
            signers
                .iter()
                .map(|signer| TrustedKey {
                    class: signer.class(),
                    key_version: signer.key_version().to_string(),
                    public_key: signer.public_key_hex(),
                    not_after: Timestamp::from_secs(4_000_000_000),
                })
                .collect(),
            BTreeSet::new(),
        )
        .expect("bundle");
        let bundle_path = dir.join("bundle.json");
        std::fs::write(&bundle_path, serde_json::to_string(&bundle).expect("json")).expect("write");
        let settings = SigningSettings {
            mode: Some("local".into()),
            trust_bundle: Some(bundle_path),
            local_keys: Some(dir.join("absent-keys.json")),
            ..SigningSettings::default()
        };
        for deployment in [
            Deployment::Dev,
            Deployment::Test,
            Deployment::Stage,
            Deployment::Prod,
        ] {
            let error = configure(&settings, deployment, NOW, &bearer, clock())
                .expect_err("a local signer was admitted in a deployment");
            assert!(error.message().contains("kms"), "{}", error.message());
        }
        // Premise of the refusal: in `local` the same settings get past the
        // deployment check and fail on the missing keys file instead.
        let local = configure(&settings, Deployment::Local, NOW, &bearer, clock())
            .expect_err("the keys file is absent");
        assert!(
            local.message().contains("local signing keys"),
            "{}",
            local.message()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Mutation: let an unset signer pass in a deployment after the removal
    /// date (drop `refuse_shared_key_after_removal`) and this fails.
    #[test]
    fn the_shared_key_posture_ends_on_its_removal_date_outside_local() {
        let settings = SigningSettings::default();
        let before = Timestamp::from_civil(2027, 1, 1);
        let after = Timestamp::from_civil(2027, 3, 1);
        assert!(matches!(
            configure(&settings, Deployment::Prod, before, &bearer, clock()),
            Ok(SigningPosture::SharedKey)
        ));
        assert!(configure(&settings, Deployment::Prod, after, &bearer, clock()).is_err());
        assert!(configure(&settings, Deployment::Local, after, &bearer, clock()).is_ok());
    }

    /// Mutation: skip `prove_signer` in `take` and a signer whose key the
    /// bundle does not hold is installed; this fails.
    #[test]
    fn a_local_signer_whose_key_is_not_in_the_bundle_stops_start_up() {
        let dir = std::env::temp_dir().join(format!("qip-signing-prove-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let mut keys = serde_json::Map::new();
        let mut entries = Vec::new();
        for class in SIGNED_HERE {
            let document = ring_pkcs8();
            let signer =
                LocalSigner::from_pkcs8(class, format!("local/{class}/1"), &document).expect("key");
            // The bundle lists a *different* key for the envelope class.
            let published = if class == ArtefactClass::Envelope {
                LocalSigner::generate(class, "other")
                    .expect("key")
                    .public_key_hex()
            } else {
                signer.public_key_hex()
            };
            entries.push(TrustedKey {
                class,
                key_version: signer.key_version().to_string(),
                public_key: published,
                not_after: Timestamp::from_secs(4_000_000_000),
            });
            keys.insert(
                class.as_str().to_string(),
                serde_json::json!({
                    "key_version": signer.key_version(),
                    "pkcs8_hex": qip_core::hash::to_hex(&document),
                }),
            );
        }
        let bundle = TrustBundle::new(entries, BTreeSet::new()).expect("bundle");
        std::fs::write(
            dir.join("bundle.json"),
            serde_json::to_string(&bundle).expect("json"),
        )
        .expect("write");
        std::fs::write(
            dir.join("keys.json"),
            serde_json::Value::Object(keys).to_string(),
        )
        .expect("write");
        let settings = SigningSettings {
            mode: Some("local".into()),
            trust_bundle: Some(dir.join("bundle.json")),
            local_keys: Some(dir.join("keys.json")),
            ..SigningSettings::default()
        };
        let error = configure(&settings, Deployment::Local, NOW, &bearer, clock())
            .expect_err("a signer the bundle cannot verify was installed");
        assert!(error.message().contains("envelope"), "{}", error.message());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn platform() -> Result<qip_kernel::Platform> {
        let config = qip_kernel::PlatformConfig::default();
        let (context, _clock) = qip_core::Context::deterministic(NOW, config.seed);
        qip_kernel::Platform::new(
            config,
            context,
            qip_observability::Telemetry::silent(),
            qip_financial::universe::Universe::new(),
            qip_risk::limits::LimitSet::conservative_default(),
        )
    }

    /// Mutation: leave a deployed platform ungated when no packs are
    /// configured (return early in the `None` arm) and this fails.
    #[test]
    fn a_deployed_api_with_no_release_holds_the_model_slot_to_nothing_and_a_local_one_does_not()
    -> Result<()> {
        let mut deployed = platform()?;
        // Premise: a fresh platform is ungated.
        assert!(!deployed.model_packs_gate_the_manifest());
        hold_models_to_release(&mut deployed, Deployment::Prod, None, None, NOW)?;
        assert!(deployed.model_packs_gate_the_manifest());

        let mut local = platform()?;
        hold_models_to_release(&mut local, Deployment::Local, None, None, NOW)?;
        assert!(!local.model_packs_gate_the_manifest());

        // Packs without a bundle to verify them stop the API.
        assert!(
            hold_models_to_release(
                &mut platform()?,
                Deployment::Prod,
                Some(Path::new("/nonexistent/packs.json")),
                None,
                NOW
            )
            .is_err()
        );
        Ok(())
    }

    fn ring_pkcs8() -> Vec<u8> {
        qip_transport::trust::generate_pkcs8().expect("key")
    }
}
