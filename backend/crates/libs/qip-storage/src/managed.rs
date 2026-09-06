//! What a managed storage target is given by its deployment, as values.
//!
//! The address, bucket and credential a managed adapter connects with are
//! properties of the deployment, never of the build. This crate used to read
//! them from the process environment at the moment the adapter was built —
//! inside [`crate::StorageProvider::key_value`] and [`crate::StorageProvider::blobs`], three
//! calls below any composition root. That put `std::env::var` in a library,
//! where nothing could test the resolution without racing every other test in
//! the binary, and it read the two credentials bare, so a deployment could not
//! mount either as a file the way it mounts every other secret.
//!
//! [`ManagedSettings`] is the same facts as values. A composition root
//! resolves them once through [`ManagedSettings::from_env`], with the
//! environment passed in as a lookup, and the provider builds adapters from
//! what it was given and nothing else. The two credentials go through
//! [`qip_core::secret::resolve_from`], so `QIP_MEMORYSTORE_AUTH_FILE` and
//! `QIP_GCP_ACCESS_TOKEN_FILE` are honoured and setting a variable beside its
//! file is refused.
//!
//! # A limit, stated
//!
//! Every variable named here is read only when `QIP_STORAGE_TARGET` selects
//! the managed target that needs it, and no deployment in this repository
//! does: the catalogue sets the target from a root variable whose default is
//! not managed, and the execution node writes `engine`. The acceptance
//! suite's manifest-wiring walk attributes to a binary the variables named
//! beside the `impl` of a type it constructs with `::from_env(`, and it does
//! not follow that type into this module — so the nine variables below are
//! not among those it checks a deployment for. That is the same visibility
//! they had when this crate read them itself, three calls down, and it is
//! recorded here rather than left to be rediscovered. A deployment that does
//! select a managed target and sets none of these stops at
//! [`crate::StorageSettings::preflight`], naming the variable, before the
//! process reports healthy.
//!
//! Today only the Memorystore half is reachable from a binary: no composition
//! root calls [`crate::StorageProvider::blobs`] or builds a warehouse, so the Cloud
//! Storage and BigQuery resolution is lifted out of the environment here
//! ahead of any caller rather than left as the one place a bare read
//! survived.

use crate::gcp::{BigQueryConfig, CloudStorageConfig, GcpAccess};
use crate::provider::StorageTarget;
use crate::redis::RedisConfig;
use qip_core::error::{Error, Result};
use std::sync::Arc;

/// The environment as a composition root sees it: a name, and the value set
/// for it if any.
///
/// A borrowed closure rather than a map so a binary can pass
/// `&|name| std::env::var(name).ok()` without first copying its whole
/// environment — including every secret in it — into a structure that
/// outlives the call.
pub type Environment<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// What a managed target needs from the deployment, already resolved.
///
/// The address, bucket and credential a managed adapter connects with are
/// properties of the deployment, never of the build, and this crate used to
/// read them from the process environment at the moment the adapter was built
/// — inside [`crate::StorageProvider::key_value`] and [`crate::StorageProvider::blobs`],
/// three calls deep from any composition root. That put `std::env::var` in a
/// library, where nothing could test the resolution without racing every
/// other test in the binary, and it read the two credentials bare, so a
/// deployment could not mount either as a file the way it mounts every other
/// secret.
///
/// This type is the same facts as values. A composition root resolves them
/// once through [`ManagedSettings::from_env`] and hands them to
/// [`crate::StorageSettings::with_managed`]; the provider then builds adapters from
/// what it was given and nothing else. A settings value carrying none of them
/// still resolves — the provider's refusal names the missing variable, as it
/// always did — so a binary whose target is `engine` never has to know these
/// exist.
///
/// [`std::fmt::Debug`] is written by hand so the two secrets never reach a
/// start-up log; [`PartialEq`] is derived so a test can compare two
/// resolutions, and is the reason this holds `String`s rather than the
/// adapters' own config types, which hold sockets' worth of state and cannot
/// be compared.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ManagedSettings {
    memorystore_address: Option<String>,
    memorystore_auth: Option<String>,
    gcp_endpoint: Option<String>,
    gcp_metadata_server: Option<String>,
    gcp_token_file: Option<String>,
    gcp_access_token: Option<String>,
    cloud_storage_bucket: Option<String>,
    bigquery_project: Option<String>,
    bigquery_dataset: Option<String>,
}

impl std::fmt::Debug for ManagedSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redact = |value: &Option<String>| value.as_ref().map(|_| "<redacted>");
        f.debug_struct("ManagedSettings")
            .field("memorystore_address", &self.memorystore_address)
            .field("memorystore_auth", &redact(&self.memorystore_auth))
            .field("gcp_endpoint", &self.gcp_endpoint)
            .field("gcp_metadata_server", &self.gcp_metadata_server)
            .field("gcp_token_file", &self.gcp_token_file)
            .field("gcp_access_token", &redact(&self.gcp_access_token))
            .field("cloud_storage_bucket", &self.cloud_storage_bucket)
            .field("bigquery_project", &self.bigquery_project)
            .field("bigquery_dataset", &self.bigquery_dataset)
            .finish()
    }
}

impl ManagedSettings {
    /// Nothing resolved. Every managed adapter built from this refuses, naming
    /// what it needs.
    pub fn none() -> Self {
        Self::default()
    }

    /// Resolve what `target` needs, and only that, from `env`.
    ///
    /// Only the target's own variables are read. A deployment running on the
    /// engine may carry a `QIP_GCP_TOKEN_FILE` for some other purpose, and a
    /// refusal about a credential the process was never going to present
    /// would send an operator to fix the wrong thing. A target that needs
    /// nothing from a deployment — memory, file, engine, and the ports with
    /// no adapter — resolves to [`ManagedSettings::none`].
    ///
    /// Two of the values are credentials and are read through
    /// [`qip_core::secret::resolve_from`], so `QIP_MEMORYSTORE_AUTH_FILE` and
    /// `QIP_GCP_ACCESS_TOKEN_FILE` are honoured, setting both the variable
    /// and its file is refused, and an empty file is refused. An empty value
    /// in the variable itself is refused here for the same reason the file
    /// rule refuses it: [`RedisConfig::from_values`] treats an absent AUTH
    /// string as "no authentication", and a deployment template that expanded
    /// a missing secret to `""` would otherwise turn a missing credential into
    /// an unauthenticated connection attempt against an instance that has
    /// `auth_enabled = true`, failing with a `NOAUTH` that reads like a wrong
    /// password rather than a missing one.
    ///
    /// `QIP_GCP_TOKEN_FILE` is not the `_FILE` variant of anything: it names a
    /// file some other process keeps fresh and is re-read before every
    /// request, where `QIP_GCP_ACCESS_TOKEN_FILE` is read once here. The
    /// distinction is [`crate::gcp::TokenFile`] against
    /// [`crate::gcp::StaticToken`], and the exactly-one rule in
    /// [`GcpAccess::from_values`] refuses a deployment that sets both.
    pub fn from_env(target: StorageTarget, env: &Environment<'_>) -> Result<Self> {
        let mut resolved = Self::none();
        match target {
            StorageTarget::Memorystore => {
                resolved.memorystore_address = plain(env, crate::redis::ADDRESS_VARIABLE);
                resolved.memorystore_auth = credential(env, crate::redis::AUTH_VARIABLE)?;
            }
            StorageTarget::CloudStorage => {
                resolved.gcp_access_from(env)?;
                resolved.cloud_storage_bucket = plain(env, crate::gcp::BUCKET_VARIABLE);
            }
            StorageTarget::BigQuery => {
                resolved.gcp_access_from(env)?;
                resolved.bigquery_project = plain(env, crate::gcp::PROJECT_VARIABLE);
                resolved.bigquery_dataset = plain(env, crate::gcp::DATASET_VARIABLE);
            }
            _ => {}
        }
        Ok(resolved)
    }

    fn gcp_access_from(&mut self, env: &Environment<'_>) -> Result<()> {
        self.gcp_endpoint = plain(env, crate::gcp::ENDPOINT_VARIABLE);
        self.gcp_metadata_server = plain(env, crate::gcp::METADATA_VARIABLE);
        self.gcp_token_file = plain(env, crate::gcp::TOKEN_FILE_VARIABLE);
        self.gcp_access_token = credential(env, crate::gcp::TOKEN_VARIABLE)?;
        Ok(())
    }

    /// Whether anything at all was resolved.
    pub fn is_empty(&self) -> bool {
        *self == Self::none()
    }

    /// The Memorystore configuration, or the refusal naming the variable.
    pub fn redis_config(&self) -> Result<RedisConfig> {
        RedisConfig::from_values(
            self.memorystore_address.as_deref(),
            self.memorystore_auth.as_deref(),
        )
    }

    /// The endpoint and credential for a Google API, resolved by the
    /// exactly-one rule in [`GcpAccess::from_values`].
    pub fn gcp_access(&self, clock: Arc<dyn qip_core::Clock>) -> Result<GcpAccess> {
        GcpAccess::from_values(
            self.gcp_endpoint.as_deref(),
            self.gcp_metadata_server.as_deref(),
            self.gcp_token_file.as_deref(),
            self.gcp_access_token.as_deref(),
            clock,
        )
    }

    /// The Cloud Storage configuration.
    ///
    /// An unset bucket is an error rather than a default, because a default
    /// that happened to name a real bucket would be written to successfully
    /// and nobody would find out until they went looking for the archive
    /// somewhere else.
    pub fn cloud_storage_config(
        &self,
        clock: Arc<dyn qip_core::Clock>,
    ) -> Result<CloudStorageConfig> {
        let bucket = self.cloud_storage_bucket.as_deref().ok_or_else(|| {
            Error::unavailable(format!(
                "no Cloud Storage bucket: set {}. There is no default, because a default naming \
                 a real bucket would be written to successfully",
                crate::gcp::BUCKET_VARIABLE
            ))
        })?;
        Ok(CloudStorageConfig::new(bucket).with_access(self.gcp_access(clock)?))
    }

    /// The BigQuery configuration.
    ///
    /// Neither the project nor the dataset has a default: the project is what
    /// a query is billed to and the dataset is what it reads, and guessing
    /// either produces a bill or an answer that belongs to somebody else.
    pub fn big_query_config(&self, clock: Arc<dyn qip_core::Clock>) -> Result<BigQueryConfig> {
        let required = |value: &Option<String>, name: &str| -> Result<String> {
            value.clone().ok_or_else(|| {
                Error::unavailable(format!(
                    "no BigQuery {name}: set it. There is no default — the project is what a \
                     query is billed to and the dataset is what it reads"
                ))
            })
        };
        let project = required(&self.bigquery_project, crate::gcp::PROJECT_VARIABLE)?;
        let dataset = required(&self.bigquery_dataset, crate::gcp::DATASET_VARIABLE)?;
        Ok(BigQueryConfig::new(project, dataset).with_access(self.gcp_access(clock)?))
    }
}

/// A non-credential value: trimmed, and empty read as unset. A deployment
/// template that expands a missing value to `""` is common enough that
/// reading it as "the operator asked for the empty address" would turn a
/// templating mistake into a connection to nowhere.
fn plain(env: &Environment<'_>, name: &str) -> Option<String> {
    env(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// A credential, by the one rule every credential here is read by.
fn credential(env: &Environment<'_>, name: &str) -> Result<Option<String>> {
    let value = qip_core::secret::resolve_from(
        name,
        env(name),
        env(&format!("{name}{}", qip_core::secret::FILE_SUFFIX)),
    )?;
    match value {
        Some(value) if value.trim().is_empty() => Err(Error::invalid(format!(
            "{name} is set and empty. An empty credential is not a credential, and reading it \
             as absent would turn a missing secret into an unauthenticated connection attempt; \
             unset it, or supply the value"
        ))),
        other => Ok(other),
    }
}
