//! Control 2 — data licensing and entitlements.
//!
//! A market data licence is not a boolean. The common shape is a feed that may
//! be researched and derived from but never used to base a live order on, and
//! the common breach is a backtest that was promoted without anyone
//! re-reading the contract. Collapsing "we have the data" into "we may use the
//! data" is what makes that breach invisible until an audit.
//!
//! [`LicensedData`] holds its value privately and has no accessor that does
//! not take a [`Usage`] and an [`EntitlementRegistry`]. Reaching the value is
//! therefore the same act as proving the intended use is licensed, and every
//! attempt — granted or refused — is recorded in the registry before the
//! borrow is returned. This mirrors `qip_agents::runtime::Gated`, whose
//! accessor charges a budget and writes an audit entry for the same reason: a
//! caller that forgets to check is still contained.
//!
//! Derivation carries the licence with it. [`LicensedData::derive`] needs
//! [`Usage::Derive`] and returns a value still tagged with the originating
//! dataset, so a feature computed from research-only data is itself
//! research-only. Without that, laundering a licence takes one `map`.

use qip_contracts::governance::{Entitlement, Usage};
use qip_core::error::{Error, Result};
use qip_core::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One recorded entitlement check.
///
/// Refusals are recorded as well as grants. Code repeatedly asking whether it
/// may trade on a research-only feed is a finding in its own right, and it is
/// only visible if the refusals are kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntitlementCheck {
    pub at: Timestamp,
    pub dataset: String,
    pub usage: Usage,
    pub granted: bool,
    /// Why it was refused. Empty when granted.
    pub refusal: String,
}

/// Which datasets may be used for what, and until when.
///
/// Keyed by dataset *and* usage: a dataset has one entitlement per usage, and
/// there is no "all usages" entry, because the permissive reading of a missing
/// entry is how a licence gets exceeded.
#[derive(Debug, Default)]
pub struct EntitlementRegistry {
    entries: BTreeMap<(String, Usage), Entitlement>,
    checks: Vec<EntitlementCheck>,
}

impl EntitlementRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a dataset is licensed for a usage until `expires_at`.
    ///
    /// An already-expired grant is refused at registration. Accepting one
    /// would put a licence in the registry that reads as configured and
    /// behaves as absent, which is the worst of both.
    pub fn grant(
        &mut self,
        dataset: impl Into<String>,
        usage: Usage,
        expires_at: Timestamp,
        now: Timestamp,
    ) -> Result<()> {
        let dataset = dataset.into();
        if dataset.trim().is_empty() {
            return Err(Error::invalid("an entitlement must name a dataset"));
        }
        if expires_at <= now {
            return Err(Error::invalid(format!(
                "the licence for {dataset}/{} expires at {expires_at}, at or before {now}",
                usage.as_str()
            )));
        }
        self.entries.insert(
            (dataset.clone(), usage),
            Entitlement::Granted {
                dataset,
                usage,
                expires_at,
            },
        );
        Ok(())
    }

    /// Record that a dataset is explicitly *not* licensed for a usage.
    ///
    /// Distinct from saying nothing: an explicit denial carries the reason a
    /// reviewer needs, and shows the question was asked rather than missed.
    pub fn deny(
        &mut self,
        dataset: impl Into<String>,
        usage: Usage,
        reason: impl Into<String>,
    ) -> Result<()> {
        let dataset = dataset.into();
        if dataset.trim().is_empty() {
            return Err(Error::invalid("an entitlement must name a dataset"));
        }
        self.entries.insert(
            (dataset.clone(), usage),
            Entitlement::Denied {
                dataset,
                usage,
                reason: reason.into(),
            },
        );
        Ok(())
    }

    /// The recorded entitlement for a dataset and usage, if any was recorded.
    pub fn entitlement(&self, dataset: &str, usage: Usage) -> Option<&Entitlement> {
        self.entries.get(&(dataset.to_string(), usage))
    }

    /// Decide a use without recording it.
    ///
    /// For a caller that wants to adapt — computing the research variant of a
    /// signal rather than failing — in the same spirit as
    /// `qip_agents::runtime::Gated::is_available`.
    pub fn permits(&self, dataset: &str, usage: Usage, now: Timestamp) -> bool {
        self.entitlement(dataset, usage)
            .is_some_and(|e| e.is_granted(now))
    }

    /// Every usage a dataset is licensed for right now.
    pub fn permitted_usages(&self, dataset: &str, now: Timestamp) -> Vec<Usage> {
        [Usage::Research, Usage::Derive, Usage::Trade, Usage::Redistribute]
            .into_iter()
            .filter(|u| self.permits(dataset, *u, now))
            .collect()
    }

    /// Decide a use and record the decision.
    ///
    /// The refusal names the dataset and the usage, because an audit that says
    /// "entitlement check failed" tells nobody which contract to go and read.
    pub fn authorise(&mut self, dataset: &str, usage: Usage, now: Timestamp) -> Result<()> {
        let outcome = match self.entitlement(dataset, usage) {
            None => Err(format!(
                "{dataset} has no recorded entitlement for {}; an unrecorded licence is \
                 treated as absent, not as permission",
                usage.as_str()
            )),
            Some(Entitlement::Denied { reason, .. }) => Err(format!(
                "{dataset} is not licensed for {}: {reason}",
                usage.as_str()
            )),
            Some(Entitlement::Granted { expires_at, .. }) if now >= *expires_at => Err(format!(
                "the licence for {dataset} covering {} expired at {expires_at}",
                usage.as_str()
            )),
            Some(Entitlement::Granted { .. }) => Ok(()),
        };
        self.checks.push(EntitlementCheck {
            at: now,
            dataset: dataset.to_string(),
            usage,
            granted: outcome.is_ok(),
            refusal: outcome.as_ref().err().cloned().unwrap_or_default(),
        });
        outcome.map_err(Error::denied)
    }

    /// Every check that has been made, granted or refused.
    pub fn checks(&self) -> &[EntitlementCheck] {
        &self.checks
    }

    /// Checks that were refused — the ones worth alerting on.
    pub fn refusals(&self) -> Vec<&EntitlementCheck> {
        self.checks.iter().filter(|c| !c.granted).collect()
    }

    /// Datasets with at least one recorded entitlement.
    pub fn datasets(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.entries.keys().map(|(d, _)| d.as_str()).collect();
        names.dedup();
        names
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A value that carries the licence of the dataset it came from.
///
/// The inner value is private and the only ways out —
/// [`LicensedData::open`], [`LicensedData::into_inner`] and
/// [`LicensedData::derive`] — all require the registry and a stated usage.
/// Wrapping a value is therefore sufficient to enforce its licence, with no
/// cooperation required from whatever consumes it.
#[derive(Debug, Clone)]
pub struct LicensedData<T> {
    value: T,
    dataset: String,
}

impl<T> LicensedData<T> {
    /// Tag a value with the dataset it came from.
    ///
    /// The tag is the licence boundary, so it is set once, at the point the
    /// data enters the platform, and never changed afterwards.
    pub fn from_dataset(dataset: impl Into<String>, value: T) -> Self {
        Self {
            value,
            dataset: dataset.into(),
        }
    }

    /// Which dataset this value's licence comes from.
    ///
    /// Safe to expose: the name is what a refusal has to quote, and knowing
    /// the provenance of a value is not the same as being allowed to use it.
    pub fn dataset(&self) -> &str {
        &self.dataset
    }

    /// Borrow the value, having proved the intended usage is licensed.
    ///
    /// Takes the registry mutably so the check is recorded before the borrow
    /// is handed over. An access that left no trace could not be reviewed, and
    /// a licensing control nobody can review is a document, not a control.
    pub fn open<'a>(
        &'a self,
        registry: &mut EntitlementRegistry,
        usage: Usage,
        now: Timestamp,
    ) -> Result<&'a T> {
        registry.authorise(&self.dataset, usage, now)?;
        Ok(&self.value)
    }

    /// Take the value out, under the same proof.
    pub fn into_inner(
        self,
        registry: &mut EntitlementRegistry,
        usage: Usage,
        now: Timestamp,
    ) -> Result<T> {
        registry.authorise(&self.dataset, usage, now)?;
        Ok(self.value)
    }

    /// Transform the value into a derived one that keeps the same licence.
    ///
    /// Requires [`Usage::Derive`], and the result is still tagged with the
    /// originating dataset. A feature built from a research-only feed stays
    /// research-only; without that, a single `map` would launder the licence
    /// and the derived column would look unencumbered forever after.
    pub fn derive<U>(
        self,
        registry: &mut EntitlementRegistry,
        now: Timestamp,
        f: impl FnOnce(T) -> U,
    ) -> Result<LicensedData<U>> {
        registry.authorise(&self.dataset, Usage::Derive, now)?;
        Ok(LicensedData {
            value: f(self.value),
            dataset: self.dataset,
        })
    }

    /// Whether the value could be opened for a usage, without recording it.
    pub fn is_available_for(
        &self,
        registry: &EntitlementRegistry,
        usage: Usage,
        now: Timestamp,
    ) -> bool {
        registry.permits(&self.dataset, usage, now)
    }
}
