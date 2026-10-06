//! The Quantum Foundry's workload-family registry.
//!
//! The blueprint widens quantum from allocation-only to a foundry of named
//! workload families. Without a registry in front of them "broad" becomes a set
//! of one-off experiments no gate can see or compare, so a family is admitted
//! only with the three things that make its quantum path judgeable: a classical
//! baseline, the one objective both paths are scored on, and the promotion
//! threshold the quantum path must clear. A family missing any of them is
//! refused rather than defaulted, because a defaulted threshold is a margin
//! nobody chose.
//!
//! A family that is not built is recorded as such, with the reason. The
//! registry's job is that no family named by the blueprint is silently absent:
//! [`Foundry::unaccounted`] must be empty for the platform to claim it covers
//! the table.

use qip_core::error::{Error, Result};
use std::collections::BTreeMap;

/// The workload families the blueprint names (M p6 and p20-21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FamilyKind {
    PortfolioAllocation,
    GraphSearch,
    FeatureEnsembleSelection,
    KernelsAndVariationalModels,
    ReservoirAndExtremeLearning,
    ScenarioHypothesisSelection,
    SymbolicCombinatorialSearch,
}

impl FamilyKind {
    pub const ALL: [FamilyKind; 7] = [
        FamilyKind::PortfolioAllocation,
        FamilyKind::GraphSearch,
        FamilyKind::FeatureEnsembleSelection,
        FamilyKind::KernelsAndVariationalModels,
        FamilyKind::ReservoirAndExtremeLearning,
        FamilyKind::ScenarioHypothesisSelection,
        FamilyKind::SymbolicCombinatorialSearch,
    ];
}

/// What a family must declare to be admitted. Fields are optional so that a
/// missing one is a refusal the registry names, not a compile error a caller
/// papers over with an empty string.
#[derive(Debug, Clone, Default)]
pub struct FamilyDeclaration {
    pub classical_baseline: Option<String>,
    pub objective: Option<String>,
    /// Margin the quantum path must beat the baseline by. Positive and finite.
    pub promotion_threshold: Option<f64>,
}

/// An admitted family: all three declarations present and valid.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadFamily {
    kind: FamilyKind,
    classical_baseline: String,
    objective: String,
    promotion_threshold: f64,
}

impl WorkloadFamily {
    pub fn kind(&self) -> FamilyKind {
        self.kind
    }
    pub fn classical_baseline(&self) -> &str {
        &self.classical_baseline
    }
    pub fn objective(&self) -> &str {
        &self.objective
    }
    pub fn promotion_threshold(&self) -> f64 {
        self.promotion_threshold
    }
}

/// The registry's account of one family.
#[derive(Debug, Clone, PartialEq)]
pub enum FamilyStatus {
    Registered(WorkloadFamily),
    NotBuilt { reason: String },
}

#[derive(Debug, Default)]
pub struct Foundry {
    entries: BTreeMap<FamilyKind, FamilyStatus>,
}

fn declared(name: &str, value: &Option<String>) -> Result<String> {
    match value.as_deref().map(str::trim) {
        Some(v) if !v.is_empty() => Ok(v.to_string()),
        _ => Err(Error::invalid(format!(
            "a workload family is refused without {name}; declare it, because a quantum path with no {name} cannot be judged"
        ))),
    }
}

impl Foundry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, kind: FamilyKind, declaration: &FamilyDeclaration) -> Result<()> {
        if self.entries.contains_key(&kind) {
            return Err(Error::invalid(format!(
                "{kind:?} is already accounted for; a second declaration would silently replace the first"
            )));
        }
        let classical_baseline = declared("a classical baseline", &declaration.classical_baseline)?;
        let objective = declared("a shared objective", &declaration.objective)?;
        let promotion_threshold = match declaration.promotion_threshold {
            Some(t) if t.is_finite() && t > 0.0 => t,
            _ => {
                return Err(Error::invalid(
                    "a workload family is refused without a positive, finite promotion threshold; a zero margin promotes a tie",
                ));
            }
        };
        self.entries.insert(
            kind,
            FamilyStatus::Registered(WorkloadFamily {
                kind,
                classical_baseline,
                objective,
                promotion_threshold,
            }),
        );
        Ok(())
    }

    pub fn record_not_built(&mut self, kind: FamilyKind, reason: &str) -> Result<()> {
        if reason.trim().is_empty() {
            return Err(Error::invalid(
                "a family recorded as not built must say why; an unexplained gap is a silent one",
            ));
        }
        if self.entries.contains_key(&kind) {
            return Err(Error::invalid(format!("{kind:?} is already accounted for")));
        }
        self.entries.insert(
            kind,
            FamilyStatus::NotBuilt {
                reason: reason.trim().to_string(),
            },
        );
        Ok(())
    }

    pub fn status(&self, kind: FamilyKind) -> Option<&FamilyStatus> {
        self.entries.get(&kind)
    }

    /// Blueprint families with neither a registration nor a not-built record.
    pub fn unaccounted(&self) -> Vec<FamilyKind> {
        FamilyKind::ALL
            .into_iter()
            .filter(|k| !self.entries.contains_key(k))
            .collect()
    }

    /// The platform's own foundry: the one family that exists, registered with
    /// the baseline and margin the compute router already uses, and every other
    /// family recorded as not built.
    pub fn platform() -> Result<Self> {
        let mut f = Self::new();
        f.register(
            FamilyKind::PortfolioAllocation,
            &FamilyDeclaration {
                classical_baseline: Some(
                    "ComputeRouter::solve_classical (exact enumeration, else annealing)".into(),
                ),
                objective: Some("PortfolioProblem::objective_at".into()),
                promotion_threshold: Some(0.01),
            },
        )?;
        let reason = "research family not built: no encoding, baseline or benchmark exists in-tree, and a variational or kernel model would need an ADR for any dependency it implies";
        for kind in FamilyKind::ALL {
            if kind != FamilyKind::PortfolioAllocation {
                f.record_not_built(kind, reason)?;
            }
        }
        Ok(f)
    }
}

/// The three job classes the Quantum Gateway admits (QUANT-035). A job outside
/// them is refused rather than defaulted into the nearest one, because a class
/// is what decides which benchmark gates the job answers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JobClass {
    Optimisation,
    HypothesisSearch,
    QuantumMlResearch,
}

impl JobClass {
    pub const ALL: [JobClass; 3] = [
        JobClass::Optimisation,
        JobClass::HypothesisSearch,
        JobClass::QuantumMlResearch,
    ];

    pub fn name(self) -> &'static str {
        match self {
            JobClass::Optimisation => "optimisation",
            JobClass::HypothesisSearch => "hypothesis-search",
            JobClass::QuantumMlResearch => "quantum-ml-research",
        }
    }
}

/// Admission filter for a gateway job. Refuses a synchronous job (a caller that
/// needs the answer inline would make execution depend on a queue it does not
/// control, QUANT-008) and any class outside [`JobClass::ALL`]; both refusals
/// name the eligible classes so the caller knows what to ask for instead.
///
/// Matching is on the whole delimited name, never a substring.
pub fn admit_job(class: &str, synchronous: bool) -> Result<JobClass> {
    let eligible = JobClass::ALL.map(JobClass::name).join(", ");
    if synchronous {
        return Err(Error::denied(format!(
            "a synchronous quantum job is refused; submit it asynchronously as one of: {eligible}"
        )));
    }
    JobClass::ALL
        .into_iter()
        .find(|c| c.name() == class)
        .ok_or_else(|| {
            Error::denied(format!(
                "job class {class:?} is not admitted; the eligible classes are: {eligible}"
            ))
        })
}
