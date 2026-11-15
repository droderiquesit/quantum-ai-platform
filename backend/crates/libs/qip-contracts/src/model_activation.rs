//! The record a cell keeps of a model it activated (R12-MODEL-10180).
//!
//! "Every model executing in Reflex can be traced to its source commit,
//! data/evaluation manifest, signature, approval event and activation epoch"
//! is five links. An incident reader holding only the record must be able to
//! walk each one; a record missing a link names a model nobody can explain,
//! so it is not made. [`ActivationRecord::admit`] is the one constructor that
//! reads a verified [`ModelPack`], and [`ActivationRecord::validate`] is the
//! same check for a record that arrives by deserialisation.

use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::policy::{ModelPack, PackProvenance};
use crate::trust::{ArtefactVerifier, Publication};

/// A model a cell activated and the five links that explain it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationRecord {
    /// The model's name as the manifest names it.
    pub model: String,
    /// SHA-256 of the artifact the pack releases.
    pub artifact_digest: String,
    /// Link 1: the build that fitted it.
    pub source_commit: String,
    /// Link 2: the data and evaluation manifest it was fitted and judged on.
    pub data_manifest: String,
    /// Link 3: the `model-pack` signature over all of the above.
    pub signature: String,
    /// Link 4: the two-person release that approved it.
    pub approval: Publication,
    /// Link 5: the policy sequence under which the cell activated it. A
    /// centre's promotion is not an activation; this is the cell's own act.
    pub activation_epoch: u64,
    /// When the cell activated it.
    pub activated_at: Timestamp,
}

impl ActivationRecord {
    /// Record the activation of `model` from `pack` at `epoch`.
    ///
    /// The pack is verified first, under the `model-pack` key, so a record is
    /// never built from a pack whose signature does not hold. A pack with no
    /// provenance is refused here and not given placeholder links: filling
    /// them would make the record look complete and say nothing.
    pub fn admit(
        model: &str,
        pack: &ModelPack,
        activation_epoch: u64,
        activated_at: Timestamp,
        verifier: &dyn ArtefactVerifier,
    ) -> Result<Self> {
        pack.verify(verifier, activated_at)?;
        let PackProvenance {
            source_commit,
            data_manifest,
        } = pack.provenance.clone().ok_or_else(|| {
            Error::denied(format!(
                "the release pack for model `{model}` records no source commit and no data \
                 manifest, so its activation cannot be traced; republish it with provenance"
            ))
        })?;
        let record = Self {
            model: model.to_string(),
            artifact_digest: pack.artifact_digest.clone(),
            source_commit,
            data_manifest,
            signature: pack.signature.clone(),
            approval: pack.publication.clone(),
            activation_epoch,
            activated_at,
        };
        record.validate()?;
        Ok(record)
    }

    /// Refuse a record missing any of its five links, naming the one.
    pub fn validate(&self) -> Result<()> {
        if self.model.is_empty() || self.artifact_digest.is_empty() {
            return Err(Error::invalid(
                "activation record: names no model or no artifact digest",
            ));
        }
        PackProvenance {
            source_commit: self.source_commit.clone(),
            data_manifest: self.data_manifest.clone(),
        }
        .validate()
        .map_err(|error| Error::invalid(format!("activation record: {}", error.message())))?;
        if self.signature.is_empty() {
            return Err(Error::invalid("activation record: signature link is empty"));
        }
        self.approval.validate().map_err(|error| {
            Error::invalid(format!(
                "activation record: approval link is not a two-person release: {}",
                error.message()
            ))
        })?;
        if self.activation_epoch == 0 {
            return Err(Error::invalid(
                "activation record: activation epoch is zero; the epoch is the policy sequence \
                 the cell applied, and none was applied",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::ArtefactClass;
    use std::collections::BTreeSet;

    #[derive(Debug)]
    struct Admits;

    impl ArtefactVerifier for Admits {
        fn verify(
            &self,
            class: ArtefactClass,
            _canonical: &[u8],
            signature: &str,
            _now: Timestamp,
        ) -> Result<()> {
            if class == ArtefactClass::ModelPack && signature == "released" {
                Ok(())
            } else {
                Err(Error::denied("does not verify"))
            }
        }
    }

    fn pack() -> ModelPack {
        ModelPack {
            artifact_digest: "ab".repeat(32),
            signature: "released".to_string(),
            features: vec!["a".to_string()],
            calibration: 0.9,
            allowed_universes: BTreeSet::from(["XNYS:ACME".to_string()]),
            budget_microseconds: 50,
            expires_at: 1_900_000_000,
            rollback_parent: String::new(),
            publication: Publication {
                author: "author".to_string(),
                approver: "approver".to_string(),
                approved_at: Timestamp::from_secs(1_760_000_000),
            },
            inline_model: None,
            provenance: Some(PackProvenance {
                source_commit: "c0".repeat(20),
                data_manifest: "d1".repeat(32),
            }),
        }
    }

    fn now() -> Timestamp {
        Timestamp::from_secs(1_760_000_100)
    }

    fn record() -> ActivationRecord {
        ActivationRecord::admit("m", &pack(), 7, now(), &Admits).expect("a complete pack admits")
    }

    /// Mutation: drop the `provenance` argument's `ok_or_else` refusal in
    /// `admit` (fill in an empty `PackProvenance`) and this fails on the
    /// `is_err` below.
    #[test]
    fn a_complete_pack_is_recorded_with_all_five_links() {
        let record = record();
        assert_eq!(record.source_commit, "c0".repeat(20));
        assert_eq!(record.data_manifest, "d1".repeat(32));
        assert_eq!(record.signature, "released");
        assert_eq!(record.approval.approver, "approver");
        assert_eq!(record.activation_epoch, 7);
    }

    #[test]
    fn a_pack_without_provenance_is_not_recorded_as_activated() {
        let mut bare = pack();
        bare.provenance = None;
        assert!(ActivationRecord::admit("m", &bare, 7, now(), &Admits).is_err());
    }

    /// Each arm removes exactly one link from an otherwise valid record, and
    /// the premise is asserted first so a record that was already refused
    /// does not pass the loop for the wrong reason.
    #[test]
    fn removing_any_one_link_is_refused_and_names_it() {
        record()
            .validate()
            .expect("premise: the whole record is valid");
        type Removal = fn(&mut ActivationRecord);
        let cases: [(&str, Removal); 5] = [
            ("source_commit", |r| r.source_commit.clear()),
            ("data_manifest", |r| r.data_manifest.clear()),
            ("signature", |r| r.signature.clear()),
            ("approval", |r| {
                r.approval.approver = r.approval.author.clone()
            }),
            ("epoch", |r| r.activation_epoch = 0),
        ];
        for (link, remove) in cases {
            let mut broken = record();
            remove(&mut broken);
            let error = broken
                .validate()
                .expect_err(&format!("a record with no {link} was accepted"));
            assert!(
                error.message().contains(match link {
                    "source_commit" => "source_commit",
                    "data_manifest" => "data_manifest",
                    "signature" => "signature",
                    "approval" => "approval",
                    _ => "epoch",
                }),
                "refusal for {link} does not name it: {}",
                error.message()
            );
        }
    }

    #[test]
    fn an_abbreviated_commit_is_refused_and_not_padded() {
        let mut short = pack();
        short.provenance = Some(PackProvenance {
            source_commit: "c0ffee1".to_string(),
            data_manifest: "d1".repeat(32),
        });
        assert!(short.validate().is_err());
    }

    #[test]
    fn provenance_is_inside_the_signed_bytes_and_absence_keeps_the_old_bytes() {
        let with = pack();
        let mut other = pack();
        other.provenance = Some(PackProvenance {
            source_commit: "c1".repeat(20),
            data_manifest: "d1".repeat(32),
        });
        let mut without = pack();
        without.provenance = None;
        assert_ne!(with.signing_payload(), other.signing_payload());
        assert_ne!(with.signing_payload(), without.signing_payload());
        assert!(
            !without
                .signing_payload()
                .windows(13)
                .any(|w| w == b"provenance/v1"),
            "a pack with no provenance must keep the bytes it was signed with"
        );
    }
}
