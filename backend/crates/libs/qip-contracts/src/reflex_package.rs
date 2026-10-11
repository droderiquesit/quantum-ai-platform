//! `ReflexPackage` (blueprint v12.0 §9.13, R12-REASON-7203, 3115): the
//! bounded form a slow-lane discovery is distilled into, and the only form
//! a reflex lane may load it in.
//!
//! The Cognitive Compiler (`qip_reasoning_engine::compiler`) is the
//! producer and runs in the deep brain, never on the order path. This file
//! is the *contract*, in the crate the edge already depends on, so a cell
//! can load a package without linking the compiler, the reasoning engine,
//! or anything that can reach a language model (R12-REFLEX-3116).
//!
//! # What "bounded" means here, enforced
//!
//! * At most [`MAX_RULES`] rules, each with at most [`MAX_CONDITIONS`]
//!   conditions; each rule adds a signed number of basis points to one
//!   named output, and the absolute sum per output is at most
//!   [`MAX_ADJUSTMENT_BPS`]. A package cannot express a loop, a call, or an
//!   unbounded value.
//! * Its worst-case cost — one operation per condition plus one per rule —
//!   is stated in `bounds.worst_case_cost` and re-computed on every
//!   admission; a package that understates its own cost is refused.
//! * `bounds` is the existing [`DistilledModelContract`] and must say
//!   `reflex_compatible`.
//!
//! # Signed, or not loaded
//!
//! [`SignedReflexPackage::load`] verifies the signature over the package's
//! digest with a [`PackageVerifier`] the loader supplies, then re-admits the
//! package through [`Validated::new`]. There is no unsigned load. The
//! signing key is ADR 0110's (KMS); this crate only names the two traits so
//! that whichever lands first — the compiler or the signer — the other
//! plugs in without an edit here.

use crate::policy::DistilledModelContract;
use crate::validated::{Contract, Validated, after, each_named, listed, named, refuse};
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_RULES: usize = 16;
pub const MAX_CONDITIONS: usize = 4;
pub const MAX_ADJUSTMENT_BPS: i32 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Below,
    AtMost,
    Above,
    AtLeast,
}

/// One test a rule's input must pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    /// A label input equals `value` (a venue, a regime).
    Equals { input: String, value: String },
    /// A numeric input compared with `threshold`.
    Compare {
        input: String,
        op: Comparison,
        threshold: Decimal,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflexRule {
    pub id: String,
    /// All must hold; may be empty (the rule always applies).
    pub when: Vec<Condition>,
    /// The output the adjustment is added to.
    pub output: String,
    /// Signed basis points added to `output` when the rule holds.
    pub add_bps: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflexPackageFields {
    pub schema_version: u32,
    pub package_id: String,
    /// The verified discovery the package distils.
    pub discovery_id: String,
    /// The `SymbolicDerivation` the discovery was formalised as.
    pub derivation_id: String,
    pub issued_at: Timestamp,
    /// After this a cell stops applying it.
    pub expires_at: Timestamp,
    /// At least one.
    pub rules: Vec<ReflexRule>,
    pub bounds: DistilledModelContract,
}

/// The §9.13 reflex package. Built only through [`Validated::new`].
pub type ReflexPackage = Validated<ReflexPackageFields>;

/// One operation per condition and one per rule.
pub fn worst_case_ops(rules: &[ReflexRule]) -> u64 {
    rules.iter().map(|r| r.when.len() as u64 + 1).sum()
}

impl Contract for ReflexPackageFields {
    const NAME: &'static str = "ReflexPackage";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses blank names, no rule or more than [`MAX_RULES`], a rule id
    /// twice, a rule over [`MAX_CONDITIONS`], an output whose absolute sum
    /// of adjustments exceeds [`MAX_ADJUSTMENT_BPS`], bounds that are not
    /// reflex-compatible or not valid, a stated cost that differs from the
    /// computed one, and an expiry not after issue.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "package_id", &self.package_id)?;
        named(c, "discovery_id", &self.discovery_id)?;
        named(c, "derivation_id", &self.derivation_id)?;
        after(
            c,
            "expires_at",
            self.expires_at,
            "issued_at",
            self.issued_at,
        )?;
        listed(c, "rules", &self.rules)?;
        if self.rules.len() > MAX_RULES {
            return refuse(
                c,
                format!(
                    "carries {} rules; a reflex package holds at most {MAX_RULES}",
                    self.rules.len()
                ),
            );
        }
        let ids: Vec<String> = self.rules.iter().map(|r| r.id.clone()).collect();
        each_named(c, "rules.id", &ids)?;
        let mut per_output: BTreeMap<&str, i64> = BTreeMap::new();
        for rule in &self.rules {
            named(c, "rules.output", &rule.output)?;
            if rule.when.len() > MAX_CONDITIONS {
                return refuse(
                    c,
                    format!(
                        "has rule `{}` with {} conditions; at most {MAX_CONDITIONS}",
                        rule.id,
                        rule.when.len()
                    ),
                );
            }
            for condition in &rule.when {
                match condition {
                    Condition::Equals { input, value } => {
                        named(c, "when.input", input)?;
                        named(c, "when.value", value)?;
                    }
                    Condition::Compare { input, .. } => named(c, "when.input", input)?,
                }
            }
            *per_output.entry(rule.output.as_str()).or_default() += i64::from(rule.add_bps).abs();
        }
        if let Some((output, total)) = per_output
            .iter()
            .find(|(_, total)| **total > i64::from(MAX_ADJUSTMENT_BPS))
        {
            return refuse(
                c,
                format!(
                    "can move `{output}` by up to {total} bp; a reflex adjustment is bounded at \
                     {MAX_ADJUSTMENT_BPS} bp per output, refused rather than clamped"
                ),
            );
        }
        if !self.bounds.reflex_compatible {
            return refuse(c, "has bounds that are not reflex-compatible");
        }
        self.bounds.validate()?;
        let ops = worst_case_ops(&self.rules);
        if self.bounds.worst_case_cost != ops {
            return refuse(
                c,
                format!(
                    "states a worst-case cost of {} and its rules cost {ops}; a package that \
                     misstates its own cost cannot be budgeted by the cell",
                    self.bounds.worst_case_cost
                ),
            );
        }
        Ok(())
    }
}

impl Validated<ReflexPackageFields> {
    /// SHA-256 of the package's canonical encoding: what is signed.
    pub fn digest(&self) -> Result<String> {
        Ok(sha256_hex(&self.encode()?))
    }
}

/// Produces a signature over a package digest. ADR 0110's KMS signer
/// implements it; nothing in this crate holds key material.
pub trait PackageSigner {
    fn key_id(&self) -> &str;
    fn sign(&self, digest: &str) -> Result<String>;
}

/// Checks a signature over a package digest under a named key.
pub trait PackageVerifier {
    fn verify(&self, key_id: &str, digest: &str, signature: &str) -> Result<()>;
}

/// A package and its signature, as written and as loaded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedReflexPackage {
    pub package: ReflexPackage,
    pub digest: String,
    pub key_id: String,
    pub signature: String,
}

impl SignedReflexPackage {
    /// Sign `package` with `signer`.
    pub fn sign(package: ReflexPackage, signer: &dyn PackageSigner) -> Result<Self> {
        let digest = package.digest()?;
        let signature = signer.sign(&digest)?;
        if signature.trim().is_empty() || signer.key_id().trim().is_empty() {
            return Err(Error::invalid(
                "a signer returned a blank signature or key id; nothing unsigned is published",
            ));
        }
        Ok(Self {
            package,
            digest,
            key_id: signer.key_id().to_string(),
            signature,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|e| Error::invalid(format!("a signed reflex package did not serialise: {e}")))
    }

    /// Decode, re-admit, recompute the digest, verify the signature, and
    /// refuse a package past its expiry at `now`. The one way a cell loads.
    pub fn load(
        bytes: &[u8],
        verifier: &dyn PackageVerifier,
        now: Timestamp,
    ) -> Result<LoadedReflex> {
        let signed: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error::schema(format!("not a valid signed ReflexPackage: {e}")))?;
        let digest = signed.package.digest()?;
        if digest != signed.digest {
            return Err(Error::denied(format!(
                "reflex package `{}` carries digest {} and hashes to {digest}; it was altered \
                 after signing",
                signed.package.package_id, signed.digest
            )));
        }
        verifier.verify(&signed.key_id, &digest, &signed.signature)?;
        if now >= signed.package.expires_at {
            return Err(Error::denied(format!(
                "reflex package `{}` expired at {}; a cell does not apply a package past its \
                 expiry",
                signed.package.package_id,
                signed.package.expires_at.to_rfc3339()
            )));
        }
        Ok(LoadedReflex {
            package: signed.package,
        })
    }
}

/// An input a loaded package reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReflexInput {
    Number(Decimal),
    Label(String),
}

/// A package that passed `load`. Evaluation is bounded by construction.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedReflex {
    package: ReflexPackage,
}

impl LoadedReflex {
    pub const fn package(&self) -> &ReflexPackage {
        &self.package
    }

    /// Every output's summed adjustment, in basis points, over `inputs`. A
    /// condition whose input is absent or of the other kind does not hold:
    /// a missing reading never fires a rule.
    pub fn evaluate(&self, inputs: &BTreeMap<String, ReflexInput>) -> BTreeMap<String, i32> {
        let mut out: BTreeMap<String, i32> = BTreeMap::new();
        for rule in &self.package.rules {
            let holds = rule.when.iter().all(|condition| match condition {
                Condition::Equals { input, value } => {
                    matches!(inputs.get(input), Some(ReflexInput::Label(v)) if v == value)
                }
                Condition::Compare {
                    input,
                    op,
                    threshold,
                } => match inputs.get(input) {
                    Some(ReflexInput::Number(v)) => match op {
                        Comparison::Below => v < threshold,
                        Comparison::AtMost => v <= threshold,
                        Comparison::Above => v > threshold,
                        Comparison::AtLeast => v >= threshold,
                    },
                    _ => false,
                },
            });
            if holds {
                *out.entry(rule.output.clone()).or_default() += rule.add_bps;
            }
        }
        out
    }

    /// The outputs the package can move.
    pub fn outputs(&self) -> BTreeSet<&str> {
        self.package
            .rules
            .iter()
            .map(|r| r.output.as_str())
            .collect()
    }
}
