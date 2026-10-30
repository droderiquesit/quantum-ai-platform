//! ADR 0110 at the deploy stage: once a composition root admits signed
//! release packs, the `trained_models` slot names to a cell only the
//! distillates whose artifact a verified, two-person pack covers.
//!
//! Every promotion here is a real fit through `LocalTrainer`, registered and
//! promoted through the platform's provider, so the manifest being gated is
//! the one a deployment ships and not a hand-built map.

#![allow(clippy::panic_in_result_fn)]

use qip_ai::registry::ModelRegistry;
use qip_contracts::policy::{InlineModel, ModelPack};
use qip_contracts::trust::{ArtefactClass, ArtefactVerifier, Publication};
use qip_core::error::{Error, Result};
use qip_core::{Context, Timestamp};
use qip_financial::universe::Universe;
use qip_kernel::central::models::register_fit;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_strategy::model::DistilledModel;
use qip_training::dataset::TrainingDataset;
use qip_training::job::TrainingSpec;
use qip_training::local::{LocalTrainer, ModelFamily, SkillPolicy, TrainedTeacher};
use qip_training::serve::InTreeProvider;
use std::collections::BTreeSet;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new_serving(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
        Box::new(InTreeProvider),
    )?;
    platform.name_model_desk("release-pack-tests/model-desk")?;
    Ok(platform)
}

fn teacher(name: &str) -> Result<TrainedTeacher> {
    let mut rows = Vec::new();
    let mut targets = Vec::new();
    let mut times = Vec::new();
    for i in 0..200usize {
        let a = (i as f64 * 0.37).sin();
        let b = (i as f64 * 0.11).cos();
        let residual = ((i * 7919) % 13) as f64 / 13.0 * 0.02 - 0.01;
        rows.push(vec![a, b]);
        targets.push(0.5 * a - 0.25 * b + 0.1 + residual);
        times.push(start().saturating_add(qip_core::Duration::from_mins(i as i64)));
    }
    let data = TrainingDataset::new(
        format!("{name}-data"),
        vec!["a".to_string(), "b".to_string()],
        rows,
        targets,
        times,
    )?;
    let spec = TrainingSpec::new(
        name,
        "0.1.0",
        "release-pack-tests",
        data.name(),
        ModelFamily::Linear { ridge: 1e-6 },
    )
    .with_holdout(0.25);
    LocalTrainer::new().fit(&spec, &data, start())
}

/// Admits a pack whose signature is exactly the one given, standing in for
/// the bundle verifier so this test is about the kernel's gate and not the
/// curve (which `qip-transport`'s own tests prove).
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

fn pack_for(digest: &str, inline: Option<(&str, String)>) -> ModelPack {
    ModelPack {
        artifact_digest: digest.to_string(),
        signature: "released".to_string(),
        features: vec!["a".to_string(), "b".to_string()],
        calibration: 0.9,
        allowed_universes: BTreeSet::from(["XNYS:ACME".to_string()]),
        budget_microseconds: 50,
        expires_at: 1_900_000_000,
        rollback_parent: String::new(),
        publication: Publication {
            author: "author".to_string(),
            approver: "approver".to_string(),
            approved_at: start(),
        },
        inline_model: inline.map(|(name, digest)| InlineModel {
            name: name.to_string(),
            digest,
        }),
    }
}

/// Mutation: drop the `release_packs` arm in `Platform::model_manifest`
/// and the unreleased distillate is named to every cell; this fails.
#[test]
fn a_promoted_distillate_reaches_the_manifest_only_when_a_verified_release_pack_names_its_artifact()
-> Result<()> {
    let mut platform = platform()?;
    let mut registry = ModelRegistry::new();
    let fitted = teacher("release-linear")?;
    let registration = register_fit(
        &mut registry,
        &fitted,
        &SkillPolicy::default(),
        "release-pack-tests",
        start(),
    )?;
    assert!(
        registration.passed,
        "premise: the fit cleared the skill bar"
    );
    let artifact = InTreeProvider::pack(&fitted)?;
    let student = DistilledModel::linear("release-linear", 0.1, vec![0.5, -0.25])?;
    platform.promote_model(&mut registry, &artifact, Some(&student), &[], start())?;

    // Premise: ungated, the promotion is named to the cells.
    assert!(!platform.model_packs_gate_the_manifest());
    assert_eq!(
        platform
            .model_manifest()?
            .manifest()
            .models
            .get(student.name()),
        Some(&student.digest())
    );

    // A release that covers some other artifact: the promotion is withheld.
    platform.admit_model_packs(
        vec![pack_for(
            "not-this-artifact",
            Some((student.name(), student.digest())),
        )],
        &Admits("released"),
        start(),
    )?;
    let refused = platform
        .model_manifest()
        .expect_err("a distillate no release pack names was named to a cell");
    assert!(
        refused.message().contains("release pack"),
        "{}",
        refused.message()
    );

    // A release of this artifact that does not name the inline model the
    // cells compute — none, or other weights under the same name — is one
    // every bundle cell would refuse (ADR 0110, Amendment 1), so it is
    // withheld here rather than shipped to be refused fleet-wide.
    for inline in [None, Some((student.name(), "other-weights".to_string()))] {
        platform.admit_model_packs(
            vec![pack_for(&artifact.digest, inline.clone())],
            &Admits("released"),
            start(),
        )?;
        assert!(
            platform.model_manifest().is_err(),
            "a pack naming {inline:?} released the distillate"
        );
    }

    // The release that covers it at its inline model: named again, and the
    // pack rides beside it so a cell can verify the release itself.
    platform.admit_model_packs(
        vec![pack_for(
            &artifact.digest,
            Some((student.name(), student.digest())),
        )],
        &Admits("released"),
        start(),
    )?;
    let issue = platform.model_manifest()?;
    assert_eq!(
        issue.manifest().models.get(student.name()),
        Some(&student.digest())
    );
    assert_eq!(
        issue
            .manifest()
            .packs
            .get(student.name())
            .map(|pack| pack.artifact_digest.as_str()),
        Some(artifact.digest.as_str())
    );
    assert_eq!(issue.unreleased(), 0);

    // A pack that does not verify is refused whole and leaves the gate as it
    // was, rather than half-replacing it.
    assert!(
        platform
            .admit_model_packs(
                vec![pack_for(
                    &artifact.digest,
                    Some((student.name(), student.digest()))
                )],
                &Admits("other"),
                start()
            )
            .is_err()
    );
    assert!(platform.model_manifest().is_ok());
    Ok(())
}

fn announcement_of(
    pack: &ModelPack,
    nodes: &[&str],
) -> Result<qip_contracts::control_pack::ControlPackAnnouncement> {
    use qip_contracts::control_pack::{
        ArtifactKind, ControlPackAnnouncement, ControlPackAnnouncementFields, RollbackParent,
        attestation_of,
    };
    use qip_contracts::validated::Contract;
    ControlPackAnnouncement::new(ControlPackAnnouncementFields {
        schema_version: ControlPackAnnouncementFields::SCHEMA_VERSION,
        kind: ArtifactKind::ModelPack,
        uri: format!("oci://registry/packs@sha256:{}", pack.artifact_digest),
        digest: pack.artifact_digest.clone(),
        version: 1,
        signer: "author and approver".to_string(),
        attestation: attestation_of(pack),
        target_regions: vec!["europe-west2".to_string()],
        target_nodes: nodes.iter().map(|node| (*node).to_string()).collect(),
        activate_from: start(),
        activate_until: Timestamp::from_secs(1_800_000_000),
        expires_at: Timestamp::from_secs(1_850_000_000),
        rollback_parent: RollbackParent::None,
        required_acks: 1,
    })
}

/// R12-CONTRACT-7091, ADR 0149 decision 4: the release's rollout record is
/// read only to narrow. A distillate whose pack is announced for one node is
/// named in that node's manifest, inside the window, and withheld from any
/// other node's, from a manifest issued for no node, and from the same node
/// once the window has closed. Before this, nothing outside `qip-contracts`
/// read an announcement, so a release targeted at one node reached every
/// cell.
///
/// Mutations: drop the `targets` check in `model_manifest_targeted` and the
/// other node is told; admit an announcement whose attestation names
/// another signing and it is refused rather than admitted.
#[test]
fn a_pack_announced_for_another_node_is_withheld_from_this_nodes_manifest() -> Result<()> {
    let mut platform = platform()?;
    let mut registry = ModelRegistry::new();
    let fitted = teacher("announced-linear")?;
    register_fit(
        &mut registry,
        &fitted,
        &SkillPolicy::default(),
        "release-pack-tests",
        start(),
    )?;
    let artifact = InTreeProvider::pack(&fitted)?;
    let student = DistilledModel::linear("announced-linear", 0.1, vec![0.5, -0.25])?;
    platform.promote_model(&mut registry, &artifact, Some(&student), &[], start())?;
    let pack = pack_for(&artifact.digest, Some((student.name(), student.digest())));
    platform.admit_model_packs(vec![pack.clone()], &Admits("released"), start())?;
    let now = start().saturating_add(qip_core::Duration::from_secs(60));

    // Premise: released and unannounced, every cell is told.
    assert!(
        platform
            .model_manifest_targeted(None, Some(("europe-west2", "tokyo-1", now)))?
            .manifest()
            .models
            .contains_key(student.name()),
        "premise: the released distillate was not named"
    );

    // An announcement made for another signing of the artifact is refused.
    let mut other_signing = pack.clone();
    other_signing.signature = "another-signing".to_string();
    assert!(
        platform
            .admit_control_announcements(vec![announcement_of(&other_signing, &["london-1"])?])
            .is_err(),
        "an announcement attesting to another signing was admitted"
    );

    platform.admit_control_announcements(vec![announcement_of(&pack, &["london-1"])?])?;
    let targeted =
        platform.model_manifest_targeted(None, Some(("europe-west2", "london-1", now)))?;
    assert!(
        targeted.manifest().models.contains_key(student.name()),
        "the targeted node was not told: {}",
        targeted.describe()
    );
    for (case, target) in [
        ("another node", Some(("europe-west2", "tokyo-1", now))),
        ("another region", Some(("asia-northeast1", "london-1", now))),
        ("no node", None),
        (
            "after the window",
            Some((
                "europe-west2",
                "london-1",
                Timestamp::from_secs(1_800_000_001),
            )),
        ),
    ] {
        let refused = platform
            .model_manifest_targeted(None, target)
            .err()
            .ok_or_else(|| Error::invalid(format!("{case}: the distillate was named")))?;
        assert!(
            refused.message().contains("rollout record"),
            "{case}: {}",
            refused.message()
        );
    }
    Ok(())
}
