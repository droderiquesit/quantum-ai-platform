//! The Cognitive Compiler (blueprint §9.13; R12-REASON-7203, 3115): a
//! verified slow-lane discovery in, a symbolic formalisation and a bounded,
//! signed reflex package out.
//!
//! # The failure this prevents
//!
//! The deep brain could *find* things — a venue whose fills the twin
//! mis-predicts by a stable margin, proven over four environments — and the
//! only way that knowledge reached a reflex lane was a person re-typing it
//! as configuration. A discovery that cannot be shipped in a form the fast
//! path can afford is a finding in a report; one shipped without a bound is
//! a slow-lane model running on the order path. Both are what §9.13 forbids:
//! fast intelligence is *derived* in the slow lane, and the order path never
//! calls back into it (R12-REFLEX-3116).
//!
//! # What the compiler refuses
//!
//! * A discovery whose verification did not pass all nine criteria. The
//!   compiler does not re-judge research; it refuses to compile research
//!   that was not judged.
//! * A formalisation whose residual uncertainty exceeds the bound: a rule a
//!   cell applies unconditionally must rest on evidence the slow lane was
//!   nearly sure of.
//! * A rule set outside the reflex bounds ([`MAX_RULES`], [`MAX_CONDITIONS`],
//!   [`MAX_ADJUSTMENT_BPS`]) — refused by the package contract itself, so
//!   the bound holds at load as well as at compile.
//!
//! # Never on the hot path
//!
//! This module lives in the reasoning engine, which the edge crates do not
//! and may not link. Its output is a [`ReflexPackage`] — a contract in
//! `qip-contracts` the cell can load without this crate — and its only
//! production caller is the deep brain's research loop, between cycles.
//!
//! # Signing
//!
//! [`CognitiveCompiler::sign`] takes any [`PackageSigner`]. ADR 0110's KMS
//! signer is a separate lane; until it is installed the caller holds the
//! compiled package unsigned and publishes nothing, because
//! `SignedReflexPackage::load` has no unsigned path and a package nobody can
//! load is not worth writing.

use qip_contracts::policy::DistilledModelContract;
pub use qip_contracts::reflex_package::{
    Comparison, Condition, LoadedReflex, MAX_ADJUSTMENT_BPS, MAX_CONDITIONS, MAX_RULES,
    PackageSigner, PackageVerifier, ReflexInput, ReflexPackage, ReflexPackageFields, ReflexRule,
    SignedReflexPackage, worst_case_ops,
};
use qip_contracts::symbolic::{
    Premise, ProofResult, SymbolicDerivation, SymbolicDerivationFields, TraceStep,
};
use qip_contracts::validated::MAX_BP;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};

/// The nine criteria a discovery's evaluation must have passed, by count.
/// Named rather than imported so this crate does not depend on the
/// expansion engine that produces the evaluation.
pub const CRITERIA: u32 = 9;

/// How a slow-lane discovery was verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    /// The evaluation record's id or question, cited in the derivation.
    pub evaluation: String,
    /// Criteria of [`CRITERIA`] that passed.
    pub criteria_passed: u32,
}

/// One step of the reasoning that established the discovery, citing
/// evidence by id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub statement: String,
    pub evidence: Vec<String>,
}

/// A verified discovery, ready to distil.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    pub discovery_id: String,
    /// What was asked.
    pub question: String,
    /// What was found, and what the package will apply.
    pub conclusion: String,
    /// The findings the conclusion rests on, in order.
    pub findings: Vec<Finding>,
    pub verification: Verification,
    /// Residual uncertainty in the conclusion, in basis points.
    pub uncertainty_bp: u32,
    /// The bounded executable form proposed for the conclusion.
    pub rules: Vec<ReflexRule>,
}

/// The compiler's bounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompilerBounds {
    /// The most residual uncertainty a compiled discovery may carry.
    pub max_uncertainty_bp: u32,
    /// How long a package is valid before the cell stops applying it.
    pub validity: Duration,
    /// The latency a single operation is budgeted at on the reflex path.
    pub micros_per_op: u64,
}

impl Default for CompilerBounds {
    fn default() -> Self {
        Self {
            max_uncertainty_bp: 2_000,
            validity: Duration::from_days(7),
            micros_per_op: 1,
        }
    }
}

/// What one compilation produced.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    pub derivation: SymbolicDerivation,
    pub package: ReflexPackage,
    /// Worst-case operations per evaluation: the package's stated cost.
    pub stated_cost: u64,
}

/// The compiler. Stateless apart from its bounds and a sequence for ids.
#[derive(Debug)]
pub struct CognitiveCompiler {
    bounds: CompilerBounds,
    issued: u64,
}

impl CognitiveCompiler {
    pub fn new(bounds: CompilerBounds) -> Result<Self> {
        if bounds.max_uncertainty_bp > MAX_BP {
            return Err(Error::invalid(format!(
                "a maximum uncertainty of {} bp is above {MAX_BP}; it would admit anything",
                bounds.max_uncertainty_bp
            )));
        }
        if bounds.validity.as_nanos() <= 0 || bounds.micros_per_op == 0 {
            return Err(Error::invalid(
                "a package needs a positive validity and a positive per-operation latency budget",
            ));
        }
        Ok(Self { bounds, issued: 0 })
    }

    pub const fn bounds(&self) -> &CompilerBounds {
        &self.bounds
    }

    /// Formalise `discovery` symbolically and distil it into a bounded
    /// reflex package issued at `now`. See the module doc for the refusals.
    pub fn compile(&mut self, discovery: &Discovery, now: Timestamp) -> Result<Compiled> {
        if discovery.verification.criteria_passed != CRITERIA {
            return Err(Error::denied(format!(
                "discovery `{}` passed {} of {CRITERIA} criteria in `{}`; only research that \
                 passed every criterion is compiled into a reflex",
                discovery.discovery_id,
                discovery.verification.criteria_passed,
                discovery.verification.evaluation
            )));
        }
        if discovery.uncertainty_bp > self.bounds.max_uncertainty_bp {
            return Err(Error::denied(format!(
                "discovery `{}` carries {} bp of residual uncertainty, above the {} bp a reflex \
                 may rest on; keep it in the slow lane",
                discovery.discovery_id, discovery.uncertainty_bp, self.bounds.max_uncertainty_bp
            )));
        }
        if discovery.findings.is_empty() {
            return Err(Error::invalid(format!(
                "discovery `{}` states no finding; a conclusion with no trace is an assertion",
                discovery.discovery_id
            )));
        }
        self.issued = self.issued.saturating_add(1);
        let derivation = self.formalise(discovery)?;
        let ops = worst_case_ops(&discovery.rules);
        let rules_bytes = serde_json::to_vec(&discovery.rules)
            .map_err(|e| Error::invalid(format!("the rules did not serialise: {e}")))?;
        let bounds = DistilledModelContract {
            name: format!("reflex:{}", discovery.discovery_id),
            // The encoded rule set is the whole of what a cell holds.
            memory_bytes: rules_bytes.len() as u64,
            latency_micros: ops.saturating_mul(self.bounds.micros_per_op),
            reflex_compatible: true,
            worst_case_cost: ops,
        };
        let package = ReflexPackage::new(ReflexPackageFields {
            schema_version: 1,
            package_id: format!("reflex-{:06}-{}", self.issued, discovery.discovery_id),
            discovery_id: discovery.discovery_id.clone(),
            derivation_id: derivation.derivation_id.clone(),
            issued_at: now,
            expires_at: now.saturating_add(self.bounds.validity),
            rules: discovery.rules.clone(),
            bounds,
        })?;
        Ok(Compiled {
            derivation,
            package,
            stated_cost: ops,
        })
    }

    /// The trace: each finding established from its evidence, each rule
    /// from the findings, and the conclusion from the rules — machine
    /// checkable by the contract (`SymbolicDerivation::check`).
    fn formalise(&self, discovery: &Discovery) -> Result<SymbolicDerivation> {
        let mut evidence: Vec<String> = Vec::new();
        for finding in &discovery.findings {
            for id in &finding.evidence {
                if !evidence.contains(id) {
                    evidence.push(id.clone());
                }
            }
        }
        if !evidence.contains(&discovery.verification.evaluation) {
            evidence.push(discovery.verification.evaluation.clone());
        }
        let rules: Vec<String> = discovery.rules.iter().map(|r| r.id.clone()).collect();
        let mut trace: Vec<TraceStep> = Vec::new();
        for finding in &discovery.findings {
            let premises = if finding.evidence.is_empty() {
                vec![Premise::Evidence {
                    id: discovery.verification.evaluation.clone(),
                }]
            } else {
                finding
                    .evidence
                    .iter()
                    .map(|id| Premise::Evidence { id: id.clone() })
                    .collect()
            };
            trace.push(TraceStep {
                statement: finding.statement.clone(),
                inference: "measured".to_string(),
                premises,
            });
        }
        let findings = trace.len() as u32;
        let mut premises: Vec<Premise> =
            (0..findings).map(|index| Premise::Step { index }).collect();
        premises.push(Premise::Evidence {
            id: discovery.verification.evaluation.clone(),
        });
        premises.extend(rules.iter().map(|id| Premise::Rule { id: id.clone() }));
        trace.push(TraceStep {
            statement: discovery.conclusion.clone(),
            inference: "distilled".to_string(),
            premises,
        });
        SymbolicDerivation::new(SymbolicDerivationFields {
            schema_version: 1,
            derivation_id: format!("derivation-{:06}-{}", self.issued, discovery.discovery_id),
            goal: discovery.question.clone(),
            conclusion: discovery.conclusion.clone(),
            rules,
            evidence,
            solver: concat!("qip-reasoning-engine::compiler@", env!("CARGO_PKG_VERSION"))
                .to_string(),
            result: ProofResult::Proved,
            violated_assumptions: Vec::new(),
            uncertainty_bp: discovery.uncertainty_bp,
            trace,
        })
    }

    /// Sign a compiled package. A signer that fails is the caller's error;
    /// the package stays unsigned and unpublished.
    pub fn sign(
        &self,
        package: ReflexPackage,
        signer: &dyn PackageSigner,
    ) -> Result<SignedReflexPackage> {
        SignedReflexPackage::sign(package, signer)
    }
}
