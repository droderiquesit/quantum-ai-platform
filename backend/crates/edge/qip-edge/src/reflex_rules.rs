//! The cell's loader and evaluator for compiled reflex packages
//! (R12-REASON-3063, ADR 0600).
//!
//! The deep brain compiles a promoted discovery into a bounded
//! [`ReflexPackage`](qip_contracts::reflex_package::ReflexPackage) and, by the
//! owner's ruling, never signs it: a person with the `model-pack` key
//! publishes it as a [`PublishedReflexPackage`]. Until this module nothing
//! that could run on a cell read one — `LoadedReflex::evaluate` had no
//! production caller, so a compiled predicate was a record of a rule and not
//! a rule. Here a cell admits a published package only through
//! [`PublishedReflexPackage::load`] against its own [`TrustRoot`], and
//! evaluates every held package on the pass.
//!
//! **A refusal keeps what stood.** A package that does not verify, has
//! expired, or collides with a held package id is refused and the book is
//! unchanged: a bad publish must not evict a good rule.
//!
//! **Bounded.** At most [`MAX_PACKAGES`] packages are held; the next is
//! refused rather than evicting one, because an evicted rule silently stops
//! applying.
//!
//! **Expiry is checked at every evaluation, not only at load.** A cell that
//! is never restarted would otherwise apply a package for ever after its
//! `expires_at`.

use crate::trust::TrustRoot;
use qip_contracts::reflex_package::{LoadedReflex, PublishedReflexPackage, ReflexInput};
use qip_contracts::trust::{ArtefactClass, ArtefactVerifier};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use std::collections::BTreeMap;

/// Packages a cell holds at once.
pub const MAX_PACKAGES: usize = 64;

/// The input a compiled rule reads to name the venue the pass is about. The
/// deep brain's compiler writes `Condition::Equals { input: "venue", .. }`.
pub const INPUT_VENUE: &str = "venue";

/// A [`TrustRoot`] as the verifier `PublishedReflexPackage::load` takes.
#[derive(Debug)]
struct RootVerifier<'a>(&'a TrustRoot);

impl ArtefactVerifier for RootVerifier<'_> {
    fn verify(
        &self,
        class: ArtefactClass,
        canonical: &[u8],
        signature: &str,
        now: Timestamp,
    ) -> Result<()> {
        self.0.check(class, canonical, signature, now)
    }
}

/// What the held packages said about one venue on one pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReflexAdjustment {
    pub venue: String,
    /// Output name to summed basis points, over every unexpired package.
    pub outputs: BTreeMap<String, i32>,
}

/// The packages a cell has admitted.
#[derive(Debug, Default)]
pub struct ReflexRuleBook {
    held: BTreeMap<String, LoadedReflex>,
}

impl ReflexRuleBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit a published package under `root`. Returns its package id.
    pub fn install(&mut self, bytes: &[u8], root: &TrustRoot, now: Timestamp) -> Result<String> {
        let loaded = PublishedReflexPackage::load(bytes, &RootVerifier(root), now)?;
        let id = loaded.package().package_id.clone();
        if self.held.contains_key(&id) {
            return Err(Error::invalid(format!(
                "reflex package `{id}` is already held; a republish carries a new package id"
            )));
        }
        if self.held.len() >= MAX_PACKAGES {
            return Err(Error::invalid(format!(
                "a cell holds at most {MAX_PACKAGES} reflex packages; withdraw one before \
                 installing `{id}`"
            )));
        }
        self.held.insert(id.clone(), loaded);
        Ok(id)
    }

    pub fn len(&self) -> usize {
        self.held.len()
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// Withdraw one package. `true` if it was held.
    pub fn withdraw(&mut self, package_id: &str) -> bool {
        self.held.remove(package_id).is_some()
    }

    /// Evaluate every unexpired package for each of `venues`. A venue no rule
    /// moves is absent from the result, not present with zeros.
    pub fn evaluate(&self, venues: &[&str], now: Timestamp) -> Vec<ReflexAdjustment> {
        let mut out = Vec::new();
        for venue in venues {
            let inputs = BTreeMap::from([(
                INPUT_VENUE.to_string(),
                ReflexInput::Label((*venue).to_string()),
            )]);
            let mut outputs: BTreeMap<String, i32> = BTreeMap::new();
            for loaded in self.held.values() {
                if now >= loaded.package().expires_at {
                    continue;
                }
                for (name, bps) in loaded.evaluate(&inputs) {
                    *outputs.entry(name).or_default() += bps;
                }
            }
            if !outputs.is_empty() {
                out.push(ReflexAdjustment {
                    venue: (*venue).to_string(),
                    outputs,
                });
            }
        }
        out
    }
}
