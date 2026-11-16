//! The microstructure trainer: a fitted, registered and distilled model of
//! one venue's execution behaviour, trained from the residuals the cells
//! concluded and LEARN folded (blueprint v11.6 §26, R12-MODEL-7022,
//! ADR 0480).
//!
//! [`crate::microstructure_model`] keeps a running error per venue and
//! residual family. That is a statistic, not a model a cell can compute:
//! nothing turned it into weights, nothing registered those weights, and so
//! nothing a cell ran on its reflex path could be traced to a registry card.
//! This module is the missing step. From the fold records on the log it
//! builds one dataset per (venue, family) — the cells' own modelled figure
//! as the input and what the venue did as the target — fits it through the
//! same `LocalTrainer` every other candidate uses, registers the fit with
//! the registry under [`MICROSTRUCTURE_PURPOSE`], and distils the linear
//! teacher to the [`DistilledModel`] a cell evaluates inline.
//!
//! **What it does not do, on purpose.** It promotes nothing and names
//! nothing to a cell. A trained microstructure model reaches the manifest
//! only through `Platform::promote_model`, which runs every gate a promotion
//! runs, and from there only under a verified release pack: a promotion of a
//! card carrying [`MICROSTRUCTURE_PURPOSE`] is recorded with
//! [`RELEASE_PACK_REQUIRED_STAGE`] among its signed stages, and
//! `Platform::model_manifest_targeted` withholds it from every cell unless a
//! release pack the composition root admitted names its artifact and inline
//! model. That holds even where no release pack was admitted at all, which
//! for every other distillate still means "ungated" — the requirement text
//! is that this model reaches the edge *only* as a registry-signed pack.

use crate::microstructure_model::{MICROSTRUCTURE_MODEL_ORIGIN, MicrostructureFold};
use qip_ai::registry::ModelRegistry;
use qip_contracts::microstructure_delta::ResidualKind;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_events::EventBody;
use qip_events::log::EventLog;
use qip_strategy::model::DistilledModel;
use qip_streaming::envelope::StreamEnvelope;
use qip_training::dataset::TrainingDataset;
use qip_training::job::TrainingSpec;
use qip_training::local::{LocalTrainer, ModelFamily, SkillPolicy, TeacherForm, TrainedTeacher};

/// The card purpose a microstructure fit is registered under, and the one
/// a promotion reads off the card to decide that the release pack is
/// required. Read off the card, never taken from the caller, so a desk
/// promoting the fit by the ordinary path cannot leave the gate behind.
pub const MICROSTRUCTURE_PURPOSE: &str = "microstructure: a venue's observed execution outcome \
                                          as a function of the cells' modelled figure";

/// The stage line a promotion of a microstructure card carries. Part of the
/// promotion's signed stages, so the requirement survives a restart (the
/// promoted set is rebuilt from those records) and a record edited to drop
/// it no longer carries the centre's signature.
pub const RELEASE_PACK_REQUIRED_STAGE: &str = "release: microstructure model; named to a cell \
                                               only under a verified release pack (ADR 0480)";

/// The one input every microstructure fit reads: the figure the cell's own
/// execution model predicted, in the family's unit.
pub const MODELLED_FEATURE: &str = "modelled";

/// Ridge penalty on the linear fit — the trainer's own default scale for a
/// one-feature regression, small enough not to bias a well-posed fit.
const RIDGE: f64 = 1e-6;

/// Share of rows held out for the skill bar, the platform's usual split.
const HOLDOUT: f64 = 0.25;

/// The residual families a cell can be handed a model for. Liquidity,
/// lead/lag and venue behaviour are not folded by the microstructure model
/// either (`VenueMicrostructure::slot`), and a model of a family nothing
/// folds would be trained on nothing.
pub fn trainable(kind: ResidualKind) -> bool {
    matches!(
        kind,
        ResidualKind::FillProbability | ResidualKind::Slippage | ResidualKind::Impact
    )
}

fn family_name(kind: ResidualKind) -> Result<&'static str> {
    match kind {
        ResidualKind::FillProbability => Ok("fill-probability"),
        ResidualKind::Slippage => Ok("slippage"),
        ResidualKind::Impact => Ok("impact"),
        other => Err(Error::invalid(format!(
            "no microstructure model is trained for {other:?} residuals; only fill probability, \
             slippage and impact are folded, so only those have data to fit"
        ))),
    }
}

/// The model's name, which is also the name the distillate carries inline
/// and the cell's manifest names it under: one per venue and family.
pub fn model_name(venue: &str, kind: ResidualKind) -> Result<String> {
    if venue.trim().is_empty() {
        return Err(Error::invalid(
            "a microstructure model needs a venue; one fitted over every venue at once describes \
             none of them",
        ));
    }
    Ok(format!("microstructure-{}-{venue}", family_name(kind)?))
}

/// Every fold record LEARN journaled, in log order — the training data, read
/// from the record rather than from the in-memory model, so a fit can be
/// reproduced from the log alone.
pub fn folds_in(log: &EventLog) -> Result<Vec<MicrostructureFold>> {
    let mut folds = Vec::new();
    for event in log.events() {
        if event.topic != MicrostructureFold::TOPIC
            || event.lineage.producer != MICROSTRUCTURE_MODEL_ORIGIN
        {
            continue;
        }
        folds.push(
            StreamEnvelope::from_frame(event)
                .and_then(|envelope| envelope.decode::<MicrostructureFold>())?
                .body,
        );
    }
    Ok(folds)
}

/// One row per residual of `kind` at `venue`, across every fold: the cell's
/// modelled figure as the input, the venue's observed figure as the target,
/// stamped at the fold's window end and ordered by it.
///
/// Decimal leaves here: the residuals are `Decimal` basis points, and the
/// fit is a statistic over them, so each crosses to `f64` once, at this
/// line. A residual with no finite `f64` form is refused rather than
/// dropped — a row silently missing is a dataset nobody can reproduce.
/// A residual summarising many samples is still one row; weighting rows by
/// samples is a choice the trainer does not offer, and the card records the
/// row count it was fitted on.
pub fn dataset(
    folds: &[MicrostructureFold],
    venue: &str,
    kind: ResidualKind,
) -> Result<TrainingDataset> {
    let name = model_name(venue, kind)?;
    let mut ordered: Vec<&MicrostructureFold> =
        folds.iter().filter(|fold| fold.venue == venue).collect();
    ordered.sort_by_key(|fold| fold.window_end);
    let mut rows = Vec::new();
    let mut targets = Vec::new();
    let mut times = Vec::new();
    for fold in ordered {
        for residual in fold.residuals.iter().filter(|r| r.kind == kind) {
            let modelled = residual.modelled.to_f64();
            let observed = residual.observed.to_f64();
            if !modelled.is_finite() || !observed.is_finite() {
                return Err(Error::numeric(format!(
                    "delta {} from {} carries a {name} residual for {} with no finite form",
                    fold.delta_id, fold.cell, residual.instrument
                )));
            }
            rows.push(vec![modelled]);
            targets.push(observed);
            times.push(fold.window_end);
        }
    }
    if rows.is_empty() {
        return Err(Error::invalid(format!(
            "no folded residual trains {name}: no cell has concluded a {} delta at {venue}",
            family_name(kind)?
        )));
    }
    TrainingDataset::new(
        format!("{name}-folds"),
        vec![MODELLED_FEATURE.to_string()],
        rows,
        targets,
        times,
    )
}

/// Fit the linear microstructure model for `venue` and `kind` on `folds`,
/// at `version`.
pub fn train(
    folds: &[MicrostructureFold],
    venue: &str,
    kind: ResidualKind,
    version: &str,
    now: Timestamp,
) -> Result<TrainedTeacher> {
    let data = dataset(folds, venue, kind)?;
    let spec = TrainingSpec::new(
        model_name(venue, kind)?,
        version,
        "kernel/microstructure-trainer",
        data.name(),
        ModelFamily::Linear { ridge: RIDGE },
    )
    .with_holdout(HOLDOUT);
    LocalTrainer::new().fit(&spec, &data, now)
}

/// Register `teacher` with the registry through the platform's own
/// registration (the skill bar and the card), then mark the card with
/// [`MICROSTRUCTURE_PURPOSE`] — the mark the promotion reads to require the
/// release pack. Refuses a teacher whose features are not this trainer's,
/// so an arbitrary fit cannot be given the purpose and borrow the name.
pub fn register(
    registry: &mut ModelRegistry,
    teacher: &TrainedTeacher,
    policy: &SkillPolicy,
    owner: &str,
    now: Timestamp,
) -> Result<crate::central::models::ModelRegistration> {
    if teacher.feature_names() != [MODELLED_FEATURE.to_string()] {
        return Err(Error::invalid(format!(
            "{} reads {:?}; a microstructure fit reads the one feature `{MODELLED_FEATURE}`",
            teacher.reference(),
            teacher.feature_names()
        )));
    }
    let registration = crate::central::models::register_fit(registry, teacher, policy, owner, now)?;
    let card = registry.get_mut(&registration.reference).ok_or_else(|| {
        Error::not_found(format!(
            "{} was registered and the registry does not hold it",
            registration.reference
        ))
    })?;
    card.purpose = MICROSTRUCTURE_PURPOSE.to_string();
    Ok(registration)
}

/// The distillate a cell evaluates inline: the linear teacher's own
/// intercept and coefficient under the model's name. Exact, not an
/// approximation — a one-feature linear teacher is already a function a
/// cell can compute in bounded cost — so the fidelity question a distillate
/// usually raises does not arise. Refuses any other form.
pub fn distil(teacher: &TrainedTeacher) -> Result<DistilledModel> {
    match teacher.form() {
        TeacherForm::Linear {
            intercept,
            coefficients,
        } => DistilledModel::linear(teacher.name(), *intercept, coefficients.clone()),
        TeacherForm::BoostedStumps { .. } => Err(Error::invalid(format!(
            "{} is an ensemble; the microstructure trainer distils only its own linear fits",
            teacher.reference()
        ))),
    }
}

impl crate::Platform {
    /// [`train`] over every fold record on this platform's own log: the
    /// microstructure model the cells' concluded residuals support, ready to
    /// [`register`], [`distil`] and promote. Promotes nothing itself.
    pub fn train_microstructure_model(
        &self,
        venue: &str,
        kind: ResidualKind,
        version: &str,
        now: Timestamp,
    ) -> Result<TrainedTeacher> {
        train(&folds_in(self.event_log())?, venue, kind, version, now)
    }
}

/// What one training pass over the log did: the references it registered and,
/// for each (venue, family) it declined to fit, the reason.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicrostructureTrainingPass {
    pub registered: Vec<String>,
    pub declined: Vec<(String, String)>,
}

/// The families a pass fits, in the order it fits them.
const PASS_KINDS: [ResidualKind; 3] = [
    ResidualKind::FillProbability,
    ResidualKind::Slippage,
    ResidualKind::Impact,
];

/// Train and register a model for every (venue, family) the folds on `log`
/// support (R12-MODEL-7022). Called from LEARN's side of the deep brain's
/// cycle, so the trainer is reached by a cycle and not by its own test.
///
/// The version is `0.<rows>`, the row count the fit read, so the pass is
/// idempotent: a (venue, family) whose fold rows have not grown since the
/// last pass names a reference the registry already holds and is declined,
/// not registered twice, and a new fold produces a new version beside the old
/// card. A family with no rows, or a fit the skill bar or the registry
/// refuses, is a declined entry naming why; it never stops the pass, because
/// one venue's thin history is no reason for another's to go unfitted.
///
/// Registers only. Promotion stays where ADR 0480 put it: `promote_model`
/// (which requires a recorded trial, `ModelCard::trial_required`) and then a
/// verified release pack before any cell is named the model.
pub fn train_and_register_all(
    log: &EventLog,
    registry: &mut ModelRegistry,
    policy: &SkillPolicy,
    owner: &str,
    now: Timestamp,
) -> Result<MicrostructureTrainingPass> {
    Ok(pass_from_folds(
        &folds_in(log)?,
        registry,
        policy,
        owner,
        now,
    ))
}

/// [`train_and_register_all`] over folds already read from the log.
pub fn pass_from_folds(
    folds: &[MicrostructureFold],
    registry: &mut ModelRegistry,
    policy: &SkillPolicy,
    owner: &str,
    now: Timestamp,
) -> MicrostructureTrainingPass {
    let venues: std::collections::BTreeSet<&str> =
        folds.iter().map(|fold| fold.venue.as_str()).collect();
    let mut pass = MicrostructureTrainingPass::default();
    for venue in venues {
        for kind in PASS_KINDS {
            let label = format!("{venue}/{kind:?}");
            let rows = match dataset(folds, venue, kind) {
                Ok(data) => data.len(),
                Err(error) => {
                    pass.declined.push((label, error.message().to_string()));
                    continue;
                }
            };
            let version = format!("0.{rows}");
            let reference = match train(folds, venue, kind, &version, now) {
                Ok(teacher) => {
                    if registry.get(&teacher.reference()).is_some() {
                        pass.declined.push((
                            label,
                            format!(
                                "{} is already registered at {rows} row(s)",
                                teacher.reference()
                            ),
                        ));
                        continue;
                    }
                    match register(registry, &teacher, policy, owner, now) {
                        Ok(registration) => registration.reference,
                        Err(error) => {
                            pass.declined.push((label, error.message().to_string()));
                            continue;
                        }
                    }
                }
                Err(error) => {
                    pass.declined.push((label, error.message().to_string()));
                    continue;
                }
            };
            pass.registered.push(reference);
        }
    }
    pass
}

/// Whether a promotion's stages carry the release-pack requirement.
pub fn requires_release_pack(stages: &[String]) -> bool {
    stages
        .iter()
        .any(|stage| stage == RELEASE_PACK_REQUIRED_STAGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::microstructure_delta::Residual;
    use qip_core::Decimal;

    fn fold(
        venue: &str,
        ends: i64,
        kind: ResidualKind,
        pairs: &[(i64, i64)],
    ) -> MicrostructureFold {
        MicrostructureFold {
            venue: venue.to_string(),
            cell: "c1".to_string(),
            delta_id: format!("d{ends}"),
            model_version: "m1".to_string(),
            window_end: Timestamp::from_secs(ends),
            residuals: pairs
                .iter()
                .enumerate()
                .map(|(i, (modelled, observed))| Residual {
                    kind,
                    instrument: format!("I{i}"),
                    modelled: Decimal::from_int(*modelled),
                    observed: Decimal::from_int(*observed),
                    samples: 3,
                })
                .collect(),
            at: Timestamp::from_secs(ends),
        }
    }

    /// The dataset is the venue's rows of the family asked for, ordered by
    /// window end, and nothing else. Mutation: drop the venue filter, or the
    /// kind filter, and the row count moves; this fails.
    #[test]
    fn the_dataset_holds_only_the_venues_rows_of_the_family_in_window_order() {
        let folds = vec![
            fold("XV", 20, ResidualKind::Slippage, &[(5, 7)]),
            fold("XV", 10, ResidualKind::Slippage, &[(1, 2), (3, 4)]),
            fold("XV", 15, ResidualKind::Impact, &[(9, 9)]),
            fold("OTHER", 12, ResidualKind::Slippage, &[(8, 8)]),
        ];
        let data = dataset(&folds, "XV", ResidualKind::Slippage).expect("XV has slippage rows");
        assert_eq!(data.len(), 3);
        assert_eq!(data.targets(), &[2.0, 4.0, 7.0]);
        assert!(dataset(&folds, "XV", ResidualKind::Liquidity).is_err());
        assert!(dataset(&folds, "NONE", ResidualKind::Slippage).is_err());
    }

    /// A pass over a log with folds registers one card per (venue, family),
    /// under the microstructure purpose, and a second pass over the same
    /// folds registers nothing. Mutation: skip the already-registered check
    /// and the second assertion fails; return an empty pass and the first does.
    #[test]
    fn a_training_pass_registers_each_venue_and_family_once() {
        let pairs: Vec<(i64, i64)> = (1..=40).map(|i| (i, 2 * i + 1)).collect();
        let folds = vec![
            fold("XV", 10, ResidualKind::Slippage, &pairs),
            fold("XV", 20, ResidualKind::Slippage, &pairs),
        ];
        let mut registry = ModelRegistry::new();
        let first = pass_over(&folds, &mut registry);
        assert_eq!(first.registered.len(), 1, "{first:?}");
        let card = registry
            .get(&first.registered[0])
            .expect("the registered card is in the registry");
        assert_eq!(card.purpose, MICROSTRUCTURE_PURPOSE);
        assert!(card.trial_required, "a microstructure card needs a trial");
        let second = pass_over(&folds, &mut registry);
        assert!(second.registered.is_empty(), "{second:?}");
        assert!(
            second
                .declined
                .iter()
                .any(|(_, why)| why.contains("already registered")),
            "{second:?}"
        );
    }

    /// The pass logic over in-memory folds, mirroring `train_and_register_all`
    /// without an event log, so the test needs no journal fixture.
    fn pass_over(
        folds: &[MicrostructureFold],
        registry: &mut ModelRegistry,
    ) -> MicrostructureTrainingPass {
        pass_from_folds(
            folds,
            registry,
            &SkillPolicy::default(),
            "test",
            Timestamp::from_secs(1_000),
        )
    }
}
