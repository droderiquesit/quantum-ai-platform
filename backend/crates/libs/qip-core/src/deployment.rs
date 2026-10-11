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
//!
//! # A name is a claim; the platform it runs on is a fact
//!
//! `local` is an ordinary environment value, and an ordinary environment
//! value is exactly what an unreviewed `gcloud run services update
//! --update-env-vars` or `gcloud compute instances add-metadata` sets. Taken
//! on its word, that one edit re-admitted the guessable seed-derived key and
//! plaintext links in a deployed environment — the two things ADR 0110 and
//! ADR 0112 exist to refuse — with nothing in the process to notice.
//!
//! So `local` is admitted only where the process can see no sign of being
//! deployed. The composition root observes the platform's own markers —
//! Cloud Run's `K_SERVICE`, `K_REVISION` and `K_CONFIGURATION`, which the
//! runtime injects into every container; Kubernetes'
//! `KUBERNETES_SERVICE_HOST`; and Compute Engine's DMI product name, read
//! from [`COMPUTE_ENGINE_PRODUCT_FILE`] — into [`DeploymentMarkers`], and
//! [`Deployment::parse_observed`] refuses `local` when any is present. The
//! markers are facts the edit that set the name does not remove: Cloud Run
//! sets `K_SERVICE` whatever the revision's environment says, and the DMI
//! table is the hypervisor's, not the guest's.
//!
//! The markers only ever *tighten*: no marker changes what `dev` means, and
//! their absence relaxes nothing the name did not already relax.

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
    ///
    /// Private on purpose: a composition root that parsed the name without
    /// the markers would take `local` on its word, which is the hole
    /// [`Self::parse_observed`] closes. Every root goes through that.
    fn parse(raw: Option<&str>) -> Result<Self> {
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

    /// Parse the raw value of [`DEPLOYMENT_VARIABLE`] as [`Self::parse`]
    /// does, and refuse `local` when `markers` shows the process is running
    /// on a deployment platform.
    ///
    /// The only public way to obtain a [`Deployment`] from configuration.
    /// The refusal names the marker seen and what to set instead, and is a
    /// refusal rather than a correction: mapping `local` to some deployed
    /// name would be guessing which environment this is, and a process that
    /// guessed would run under a posture nobody chose.
    pub fn parse_observed(raw: Option<&str>, markers: &DeploymentMarkers) -> Result<Self> {
        let deployment = Self::parse(raw)?;
        if deployment == Self::Local && !markers.is_empty() {
            return Err(Error::denied(format!(
                "{DEPLOYMENT_VARIABLE}=local and this process can see it is deployed: {}. \
                 `local` admits a reproducible signing key and plaintext links (ADR 0110, ADR \
                 0112) and is honoured only on a developer machine or in a test harness, so an \
                 environment edit cannot relax either guard where it runs. Set \
                 {DEPLOYMENT_VARIABLE} to the Terraform environment this process runs in — one \
                 of `dev`, `test`, `stage` or `prod`",
                markers.seen().join("; ")
            )));
        }
        Ok(deployment)
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

/// The variables Cloud Run injects into every container it runs, whatever
/// the revision's own environment says.
pub const CLOUD_RUN_VARIABLES: [&str; 3] = ["K_SERVICE", "K_REVISION", "K_CONFIGURATION"];

/// The variable the kubelet injects into every pod.
pub const KUBERNETES_VARIABLE: &str = "KUBERNETES_SERVICE_HOST";

/// Where a Linux guest reads the hypervisor's product name. The composition
/// root reads it — this module performs no I/O — and passes the contents to
/// [`DeploymentMarkers::observe`].
pub const COMPUTE_ENGINE_PRODUCT_FILE: &str = "/sys/class/dmi/id/product_name";

/// What [`COMPUTE_ENGINE_PRODUCT_FILE`] contains on a Compute Engine VM.
pub const COMPUTE_ENGINE_PRODUCT: &str = "Google Compute Engine";

/// The signs of a deployment platform a composition root observed, each
/// described the way a refusal should name it.
///
/// A value rather than a reader, so a library never touches the environment
/// and a test states the platform it means instead of inheriting the host's:
/// a suite that read the real markers would pass on a laptop and refuse on a
/// Compute Engine CI runner.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeploymentMarkers {
    seen: Vec<String>,
}

impl DeploymentMarkers {
    /// No marker: a developer machine or a test harness.
    pub fn none() -> Self {
        Self::default()
    }

    /// The markers among what `variable` returns for
    /// [`CLOUD_RUN_VARIABLES`] and [`KUBERNETES_VARIABLE`], and the DMI
    /// `product_name` the root read from [`COMPUTE_ENGINE_PRODUCT_FILE`]
    /// (`None` where the file is absent or unreadable, as on most
    /// workstations and in every container).
    ///
    /// A blank variable is not a marker — the platforms never inject one
    /// blank — so an operator's stray `K_SERVICE=` on a laptop is not refused
    /// for a platform it is not on. The product name is matched as a
    /// substring because the file ends in a newline and a refusal that a
    /// trailing byte could dodge is not one.
    pub fn observe(variable: &dyn Fn(&str) -> Option<String>, product_name: Option<&str>) -> Self {
        let mut seen = Vec::new();
        for name in CLOUD_RUN_VARIABLES {
            if variable(name).is_some_and(|value| !value.trim().is_empty()) {
                seen.push(format!("{name} is set, which Cloud Run injects"));
            }
        }
        if variable(KUBERNETES_VARIABLE).is_some_and(|value| !value.trim().is_empty()) {
            seen.push(format!(
                "{KUBERNETES_VARIABLE} is set, which Kubernetes injects into every pod"
            ));
        }
        if product_name.is_some_and(|name| name.contains(COMPUTE_ENGINE_PRODUCT)) {
            seen.push(format!(
                "{COMPUTE_ENGINE_PRODUCT_FILE} reads `{COMPUTE_ENGINE_PRODUCT}`"
            ));
        }
        Self { seen }
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// Each marker observed, described for a refusal.
    pub fn seen(&self) -> &[String] {
        &self.seen
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

    fn variables(set: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let set: Vec<(String, String)> = set
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        move |name| {
            set.iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, value)| value.clone())
        }
    }

    /// The failure this prevents: one `gcloud` edit setting
    /// `QIP_DEPLOYMENT=local` on a Cloud Run service or a Compute Engine
    /// node re-admitted the seed-derived key and plaintext links there.
    /// Mutation: drop the `!markers.is_empty()` refusal in
    /// `parse_observed`, and every deployed marker below is admitted as
    /// `local`; this fails.
    #[test]
    fn local_is_refused_wherever_the_process_can_see_a_deployment_platform() {
        // Premise: with no marker, `local` is what it always was.
        assert_eq!(
            Deployment::parse_observed(Some("local"), &DeploymentMarkers::none()).ok(),
            Some(Deployment::Local)
        );
        let deployed = [
            DeploymentMarkers::observe(&variables(&[("K_SERVICE", "qip-api")]), None),
            DeploymentMarkers::observe(&variables(&[("K_REVISION", "qip-api-00042-abc")]), None),
            DeploymentMarkers::observe(&variables(&[("K_CONFIGURATION", "qip-api")]), None),
            DeploymentMarkers::observe(&variables(&[(KUBERNETES_VARIABLE, "10.0.0.1")]), None),
            DeploymentMarkers::observe(&variables(&[]), Some("Google Compute Engine\n")),
        ];
        for markers in &deployed {
            // Premise: each fixture really observed exactly one marker.
            assert_eq!(markers.seen().len(), 1, "{markers:?}");
            let error = Deployment::parse_observed(Some("local"), markers)
                .expect_err("local was admitted on a deployment platform");
            assert_eq!(error.code(), Error::denied("").code());
            assert!(
                error.message().contains(&markers.seen()[0]),
                "the refusal does not name the marker it saw: {}",
                error.message()
            );
            assert!(
                error.message().contains("`prod`"),
                "the refusal does not say what to set instead: {}",
                error.message()
            );
            // The markers tighten `local` and nothing else.
            for named in [
                Deployment::Dev,
                Deployment::Test,
                Deployment::Stage,
                Deployment::Prod,
            ] {
                assert_eq!(
                    Deployment::parse_observed(Some(named.as_str()), markers).ok(),
                    Some(named)
                );
            }
        }
    }

    /// Mutation: treat a blank variable, or any DMI product name, as a
    /// marker and a developer's workstation is refused `local` for a
    /// platform it is not on; this fails.
    #[test]
    fn a_blank_variable_or_another_hypervisor_is_not_a_deployment_marker() {
        let markers = DeploymentMarkers::observe(
            &variables(&[("K_SERVICE", "  "), (KUBERNETES_VARIABLE, "")]),
            Some("Virtual Machine\n"),
        );
        assert!(markers.is_empty(), "{markers:?}");
        assert!(Deployment::parse_observed(Some("local"), &markers).is_ok());
        // An absent name is still refused, markers or none.
        assert!(Deployment::parse_observed(None, &DeploymentMarkers::none()).is_err());
    }
}
