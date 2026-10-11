//! Which deployment a process believes it is, named rather than inferred.
//!
//! ADR 0110 decision 5: a central binary refuses to start on a signing key
//! derived from the configured seed "whenever the deployment is not a test",
//! and "not a test" is "an explicit environment name the composition root
//! reads and refuses when absent (fail closed), not the absence of a flag".
//! ADR 0112 makes the same cut for transport: plaintext links exist only where
//! configuration says so, for local development.
//!
//! Both rules need one fact — *is this a developer's machine or a test
//! harness, or is it a deployment?* — and the failure this type exists to
//! prevent is deciding it by absence. A flag such as `QIP_PRODUCTION=1` fails
//! open: the deployment that forgot to set it runs on the guessable key, and
//! nothing distinguishes that from a laptop. So the name is required, an
//! unrecognised name is refused rather than mapped to the nearest one, and
//! exactly one value — [`Deployment::Local`] — relaxes anything.
//!
//! **The Terraform environment called `test` is a deployment.** It is a GCP
//! project with services in it, and a seed-derived key there is a key anyone
//! with the repository can reproduce. ADR 0110's "a test" means a test
//! harness, which is why the relaxing value is spelled `local` and not
//! `test`: had it been `test`, the fourth environment in
//! `infrastructure/environments/` would have been the one place the
//! guessable key was admitted.
//!
//! This module reads nothing. The composition root reads the variable named
//! by [`DEPLOYMENT_VARIABLE`] and hands the raw value here, so a library that
//! needs the fact takes a [`Deployment`] as a value and never learns where it
//! came from.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// The variable every composition root reads the deployment name from.
///
/// Documented here so the refusal below can name it. The roots declare their
/// own copy of the literal beside the read, because the acceptance suite's
/// manifest walk resolves a variable from the binary's own crate.
pub const DEPLOYMENT_VARIABLE: &str = "QIP_DEPLOYMENT";

/// The deployment a process runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Deployment {
    /// A developer's machine or a test harness: the only value under which a
    /// reproducible key or a plaintext link is admitted.
    Local,
    /// The four Terraform environments, by the names
    /// `infrastructure/terraform/variables.tf` admits for `environment`.
    Dev,
    Test,
    Stage,
    Prod,
}

impl Deployment {
    /// Every value, in the order a refusal lists them.
    pub const ALL: [Self; 5] = [Self::Local, Self::Dev, Self::Test, Self::Stage, Self::Prod];

    /// Parse the raw value of [`DEPLOYMENT_VARIABLE`], refusing an absent,
    /// blank or unrecognised one.
    ///
    /// Absent is refused rather than read as `local`: a deployment that forgot
    /// to say what it is must not be treated as the one environment where the
    /// guards are relaxed. Matching is exact after trimming — `Prod` and
    /// `production` are refused by name, not guessed at, because a value
    /// silently corrected is a configuration bug that survives.
    pub fn parse(raw: Option<&str>) -> Result<Self> {
        let Some(value) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            return Err(Error::invalid(format!(
                "{DEPLOYMENT_VARIABLE} is not set. Every central binary and every node must \
                 say which deployment it is — one of {} — because the guards ADR 0110 and \
                 ADR 0112 relax for local development must not be relaxed by an omission. Set \
                 it to `local` on a developer machine or in a test harness, and to the \
                 Terraform environment name everywhere else",
                Self::listing()
            )));
        };
        Self::ALL
            .into_iter()
            .find(|deployment| deployment.as_str() == value)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{DEPLOYMENT_VARIABLE}={value:?} is not a deployment this platform knows; \
                     use exactly one of {} (lower case, as Terraform's `environment` spells \
                     them)",
                    Self::listing()
                ))
            })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Dev => "dev",
            Self::Test => "test",
            Self::Stage => "stage",
            Self::Prod => "prod",
        }
    }

    /// Whether a signing key derived from the configured seed may be used.
    ///
    /// Only [`Self::Local`]. Everywhere else anyone who knows the seed — which
    /// is committed configuration — could mint a policy frame or a capital
    /// grant (ADR 0110 decision 5).
    pub const fn permits_reproducible_keys(self) -> bool {
        matches!(self, Self::Local)
    }

    /// Whether a link the platform owns may come up without mutual TLS.
    ///
    /// Only [`Self::Local`] (ADR 0112: "plaintext only where config says so
    /// for local dev"). A deployment that cannot load its workload
    /// certificate stops; it does not fall back.
    pub const fn permits_plaintext_links(self) -> bool {
        matches!(self, Self::Local)
    }

    fn listing() -> String {
        Self::ALL
            .iter()
            .map(|deployment| format!("`{}`", deployment.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl std::fmt::Display for Deployment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutation: make `parse(None)` return `Ok(Deployment::Local)` — the
    /// fail-open default ADR 0110 decision 5 forbids — and this fails.
    #[test]
    fn an_absent_or_blank_deployment_is_refused_rather_than_read_as_local() {
        for raw in [None, Some(""), Some("   ")] {
            let error = Deployment::parse(raw).expect_err("an unnamed deployment was accepted");
            assert!(
                error.message().contains(DEPLOYMENT_VARIABLE),
                "the refusal does not name the variable to set: {}",
                error.message()
            );
        }
    }

    /// Mutation: match case-insensitively or by prefix and `Prod` or
    /// `production` would be admitted; this fails.
    #[test]
    fn a_near_miss_name_is_refused_rather_than_guessed() {
        for raw in ["Prod", "production", "dev ", "LOCAL", "testing"] {
            // `"dev "` trims to `dev` and is the one admitted here; the rest
            // are refused.
            let parsed = Deployment::parse(Some(raw));
            if raw.trim() == "dev" {
                assert_eq!(parsed.ok(), Some(Deployment::Dev));
            } else {
                assert!(parsed.is_err(), "{raw:?} was admitted as a deployment");
            }
        }
    }

    /// Mutation: let `Test` relax the guards (the Terraform `test`
    /// environment is a deployment) and this fails.
    #[test]
    fn only_local_relaxes_either_guard_and_the_terraform_test_environment_does_not() {
        // Premise: every value parses from its own name.
        for deployment in Deployment::ALL {
            assert_eq!(
                Deployment::parse(Some(deployment.as_str())).ok(),
                Some(deployment)
            );
        }
        let relaxed: Vec<Deployment> = Deployment::ALL
            .into_iter()
            .filter(|deployment| {
                deployment.permits_reproducible_keys() || deployment.permits_plaintext_links()
            })
            .collect();
        assert_eq!(relaxed, vec![Deployment::Local]);
        assert!(!Deployment::Test.permits_reproducible_keys());
        assert!(!Deployment::Test.permits_plaintext_links());
    }
}
