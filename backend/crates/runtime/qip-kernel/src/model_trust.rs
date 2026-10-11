//! Naming to a cell only what a signed release pack covers (ADR 0110,
//! R12-MODEL-4023, R12-REFLEX-9011, ADR 0101 C9a).
//!
//! The `trained_models` slot is how a cell learns which inline models it may
//! run: `Cell::check_models_promoted` refuses any model the manifest does
//! not name at its digest. Until this module the manifest named every
//! distillate the deep brain promoted, so the only thing between a training
//! run and the order path was an automated desk. ADR 0110 puts a reviewed
//! release there: a [`ModelPack`] signed under the `model-pack` key by the
//! publish workflow, naming two distinct people, unexpired.
//!
//! [`Platform::admit_model_packs`] verifies every pack against the verifier
//! the composition root built over the trust bundle and holds the admitted
//! set; from then on `Platform::model_manifest` names a promoted distillate
//! only when an admitted pack names its artifact, and counts the rest as
//! withheld. The kernel holds no key and links no cryptography: the verifier
//! is handed in as a value. Admission refuses the whole set on the first
//! pack that fails, because a process that started with some packs silently
//! dropped would name a different set from the one the release reviewed.

use crate::platform::Platform;
use qip_contracts::policy::ModelPack;
use qip_contracts::trust::ArtefactVerifier;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use std::collections::BTreeMap;

/// Verify every pack, refusing the whole set on the first that fails, and
/// return the admitted packs by the artifact digest each names.
///
/// Two packs naming one digest are refused: one artifact is one release,
/// and two records of it could disagree about its universes or its budget.
pub fn admit_packs(
    packs: Vec<ModelPack>,
    verifier: &dyn ArtefactVerifier,
    now: Timestamp,
) -> Result<BTreeMap<String, ModelPack>> {
    let mut admitted = BTreeMap::new();
    for pack in packs {
        pack.verify(verifier, now).map_err(|error| {
            Error::denied(format!(
                "model pack for artifact {} is not loaded: {}",
                pack.artifact_digest,
                error.message()
            ))
        })?;
        if admitted.contains_key(&pack.artifact_digest) {
            return Err(Error::invalid(format!(
                "two model packs name artifact {}; one artifact is one release",
                pack.artifact_digest
            )));
        }
        admitted.insert(pack.artifact_digest.clone(), pack);
    }
    Ok(admitted)
}

impl Platform {
    /// Hold the `trained_models` slot to these packs from now on.
    ///
    /// An empty list is legitimate and names no distillate to any cell —
    /// the fail-closed state of a deployment whose release shipped no packs.
    /// Returns how many packs were admitted.
    pub fn admit_model_packs(
        &mut self,
        packs: Vec<ModelPack>,
        verifier: &dyn ArtefactVerifier,
        now: Timestamp,
    ) -> Result<usize> {
        let admitted = admit_packs(packs, verifier, now)?;
        let count = admitted.len();
        self.release_packs = Some(admitted);
        Ok(count)
    }

    /// Whether the slot is held to signed release packs.
    pub const fn model_packs_gate_the_manifest(&self) -> bool {
        self.release_packs.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::trust::{ArtefactClass, Publication};
    use std::collections::BTreeSet;

    /// A verifier that admits exactly one signature string, so the tests
    /// exercise this module's refusals and not a curve.
    #[derive(Debug)]
    struct Admits(&'static str);

    impl ArtefactVerifier for Admits {
        fn verify(
            &self,
            class: ArtefactClass,
            _canonical: &[u8],
            signature: &str,
            _now: Timestamp,
        ) -> Result<()> {
            if class == ArtefactClass::ModelPack && signature == self.0 {
                Ok(())
            } else {
                Err(Error::denied("does not verify"))
            }
        }
    }

    fn pack(digest: &str, signature: &str, approver: &str) -> ModelPack {
        ModelPack {
            artifact_digest: digest.to_string(),
            signature: signature.to_string(),
            features: vec!["mid".to_string()],
            calibration: 0.8,
            allowed_universes: BTreeSet::from(["XNYS:ACME".to_string()]),
            budget_microseconds: 50,
            expires_at: 1_900_000_000,
            rollback_parent: String::new(),
            publication: Publication {
                author: "author".to_string(),
                approver: approver.to_string(),
                approved_at: Timestamp::from_secs(1_800_000_000),
            },
            inline_model: None,
        }
    }

    /// Mutation: skip `pack.verify` in `admit_packs` and the forged pack is
    /// admitted; drop the publication check from `ModelPack::verify` and the
    /// one-person pack is; drop the expiry check and the late one is. Each
    /// arm fails.
    #[test]
    fn a_pack_that_does_not_verify_names_one_person_or_has_expired_stops_the_whole_set() {
        let now = Timestamp::from_secs(1_850_000_000);
        let verifier = Admits("good");
        // Premise: a genuine pack is admitted.
        assert_eq!(
            admit_packs(vec![pack("d1", "good", "approver")], &verifier, now)
                .expect("a genuine pack")
                .len(),
            1
        );
        assert!(
            admit_packs(
                vec![
                    pack("d1", "good", "approver"),
                    pack("d2", "forged", "approver")
                ],
                &verifier,
                now
            )
            .is_err()
        );
        assert!(admit_packs(vec![pack("d1", "good", "Author")], &verifier, now).is_err());
        let late = Timestamp::from_secs(1_900_000_001);
        assert!(admit_packs(vec![pack("d1", "good", "approver")], &verifier, late).is_err());
        assert!(
            admit_packs(
                vec![
                    pack("d1", "good", "approver"),
                    pack("d1", "good", "approver")
                ],
                &verifier,
                now
            )
            .is_err(),
            "two packs for one artifact were admitted"
        );
    }
}
