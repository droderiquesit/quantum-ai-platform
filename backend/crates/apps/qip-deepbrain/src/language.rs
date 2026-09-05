//! The language model this node's organisation narrates through.
//!
//! One chain, assembled from the configuration: the hosted adapter first when
//! every precondition for it is met, and the deterministic model last in every
//! case, so a provider outage — or a precondition nobody satisfied — degrades
//! to templates rather than stopping reasoning (ADR 0037, decision 4). This is
//! the only composition root that builds the hosted adapter: `qip-fastbrain`
//! reads none of the variables because nothing on the fast path consults a
//! model (ADR 0008) and it has no proxy to reach one through (ADR 0032);
//! `qip-api` serves what the brains recorded.
//!
//! # Why naming a provider is not enough to get one
//!
//! ADR 0037 is accepted for the platform lane and applied nowhere, and the two
//! acts it waits on are a person's: reading the terms of the providers the
//! chosen model resolves to, and creating the secret. Until both have
//! happened, the only thing standing between a deployed process and a vendor
//! was whether anybody had set three environment variables — so the ADR's
//! sequence lived in a document and no process could tell whether it had been
//! followed. It is structural here instead. The adapter is constructed only
//! when all of:
//!
//! 1. the model identifier pins the provider the router must serve it from,
//!    because an unpinned model resolves to whichever provider the router
//!    picks at call time and that pick is invisible here;
//! 2. a committed attestation says a named operator read that provider's terms
//!    at a stated instant ([`crate::attestation`]);
//! 3. the credential resolved, through `qip_core::secret` and so through the
//!    `_FILE` indirection the Secret Manager mount projects.
//!
//! Anything missing withholds the adapter — it is not constructed at all,
//! rather than constructed dark — and the banner says which piece is missing
//! and names the variable that supplies it. The templates keep narrating,
//! which is what every deployment does today.
//!
//! Separated from `main` so each of those decisions is asserted by a test
//! rather than read off a banner on a running process.

use crate::attestation::ATTESTATION_PATH_VARIABLE;
use crate::config::{DeepBrainConfig, HostedLanguageModel, LANGUAGE_MODEL_VARIABLE};
use qip_ai::language::{DeterministicModel, FallbackChain, LanguageModel};
use qip_core::error::{Error, Result};
use qip_reasoning_engine::providers::huggingface::{
    DEFAULT_DEADLINE, DEFAULT_MAX_BODY_BYTES, HF_TOKEN_VARIABLE, HuggingFaceConfig,
    HuggingFaceModel,
};
use std::sync::Arc;

/// The chain, with what it was assembled from.
#[derive(Debug)]
pub struct AssembledModel {
    pub chain: Arc<FallbackChain>,
    /// What this root did about a hosted provider, and why.
    pub hosted: HostedDecision,
}

/// The three outcomes, kept apart because an operator reading the banner has
/// to tell them apart: nothing was asked for, what was asked for is running,
/// or what was asked for is withheld and here is the missing piece.
///
/// One `Option<HostedSummary>` conflated the last two, and the conflation is
/// the failure: "no hosted provider" and "a hosted provider whose terms nobody
/// has read" read the same on a banner and mean opposite things about what an
/// operator has left to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostedDecision {
    /// No provider named. Every deployment today.
    NotRequested,
    /// Every precondition met: the adapter is first in the chain.
    Installed(HostedSummary),
    /// A provider was named and one precondition is not met.
    Withheld(Withheld),
}

/// What the banner says about an installed adapter. Never the token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedSummary {
    pub model: String,
    pub base_url: String,
    /// The provider the model identifier pins, which is the provider the
    /// attestation was checked against.
    pub provider: String,
    /// Who read that provider's terms, which terms, and when.
    pub attested: String,
}

/// The precondition that is missing, so the banner names one thing to do next
/// rather than everything that could be wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Withheld {
    /// The model identifier pins no provider, so the router would choose one.
    NoProviderPinned { model: String },
    /// No attestation is mounted.
    NoAttestation { provider: String },
    /// One is mounted and it does not name this model's provider.
    ProviderNotAttested { provider: String, attested: String },
    /// Provider pinned and its terms attested; no credential resolved.
    NoCredential { provider: String },
}

impl Withheld {
    /// The variable that supplies the missing piece.
    ///
    /// Returned rather than only written into prose so the banner names
    /// exactly one variable per missing piece, and so a test asserts that
    /// mapping against a delimited token: `QIP_LANGUAGE_MODEL` is a substring
    /// of `QIP_LANGUAGE_MODEL_PROVIDER`, and a `contains` check against the
    /// shorter one passes on a message about the longer.
    pub fn variable(&self) -> &'static str {
        match self {
            Self::NoProviderPinned { .. } => LANGUAGE_MODEL_VARIABLE,
            Self::NoAttestation { .. } | Self::ProviderNotAttested { .. } => {
                ATTESTATION_PATH_VARIABLE
            }
            Self::NoCredential { .. } => HF_TOKEN_VARIABLE,
        }
    }

    /// What is missing, and what reading it as satisfied would have cost.
    pub fn describe(&self) -> String {
        match self {
            Self::NoProviderPinned { model } => format!(
                "the model `{model}` pins no provider, so the router would choose one per call \
                 and this process cannot see which; no attestation can be checked against a \
                 choice made after the fact. Name the model as `<org>/<model>:<provider>`, as \
                 the router's catalogue and the gateway's --probe spell it"
            ),
            Self::NoAttestation { provider } => format!(
                "nothing mounted says anybody read the terms `{provider}` serves under, and a \
                 credential is not permission (ADR 0037)"
            ),
            Self::ProviderNotAttested { provider, attested } => format!(
                "the mounted attestation has {attested}, which does not cover `{provider}`; \
                 reading one provider's terms is not reading another's"
            ),
            Self::NoCredential { provider } => format!(
                "the terms for `{provider}` are attested and no credential resolved; set it as \
                 {HF_TOKEN_VARIABLE}{} to name the mounted secret file, which is what the \
                 Secret Manager projection gives the process",
                qip_core::secret::FILE_SUFFIX
            ),
        }
    }
}

impl AssembledModel {
    /// The model that would actually serve a request: `FallbackChain::name`.
    pub fn active_name(&self) -> String {
        self.chain.name().to_string()
    }

    /// One line for the start-up banner.
    pub fn describe(&self) -> String {
        match &self.hosted {
            HostedDecision::NotRequested => format!(
                "{} (no hosted provider; {} is not set)",
                self.active_name(),
                crate::config::LANGUAGE_MODEL_PROVIDER_VARIABLE
            ),
            HostedDecision::Installed(hosted) => format!(
                "{} — hosted {} on `{}` via {}, {}, credential mounted; deterministic model \
                 behind it",
                self.active_name(),
                hosted.model,
                hosted.provider,
                hosted.base_url,
                hosted.attested
            ),
            HostedDecision::Withheld(withheld) => format!(
                "{} — the hosted provider is WITHHELD and templates narrate: {}. The variable \
                 is {}",
                self.active_name(),
                withheld.describe(),
                withheld.variable()
            ),
        }
    }
}

/// Assemble the chain the configuration describes.
pub fn assemble(config: &DeepBrainConfig) -> Result<AssembledModel> {
    let deterministic: Arc<dyn LanguageModel> = Arc::new(DeterministicModel::new());
    let Some(hosted) = &config.language_model else {
        return Ok(AssembledModel {
            chain: Arc::new(FallbackChain::new(vec![deterministic])),
            hosted: HostedDecision::NotRequested,
        });
    };

    // Validated before any precondition is weighed, so a base URL that does
    // not parse stops the process instead of appearing on the banner as a
    // provider withheld for want of a credential.
    let adapter_config = HuggingFaceConfig::new(
        &hosted.model,
        &hosted.base_url,
        DEFAULT_DEADLINE,
        DEFAULT_MAX_BODY_BYTES,
    )
    .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;

    match precondition(hosted, &adapter_config) {
        Err(withheld) => Ok(AssembledModel {
            chain: Arc::new(FallbackChain::new(vec![deterministic])),
            hosted: HostedDecision::Withheld(withheld),
        }),
        Ok(met) => {
            let summary = HostedSummary {
                model: hosted.model.clone(),
                base_url: hosted.base_url.clone(),
                provider: met.provider,
                attested: met.attested,
            };
            // The token is present — that is what `precondition` returning
            // `Ok` means — and is handed over as the `Option` the
            // configuration resolved, because the adapter is the one place
            // that decides what an absent credential means.
            let adapter = HuggingFaceModel::new(adapter_config, hosted.token.clone());
            Ok(AssembledModel {
                chain: Arc::new(FallbackChain::new(vec![Arc::new(adapter), deterministic])),
                hosted: HostedDecision::Installed(summary),
            })
        }
    }
}

/// What was established when every precondition held: which provider this
/// adapter is pinned to, and the attestation clause for it.
struct Met {
    provider: String,
    attested: String,
}

/// Every precondition, in the order ADR 0037 asks for the acts that satisfy
/// them: pin the provider, read its terms, then create the secret. Reported in
/// that order so an operator following the banner does them in the sequence
/// the record requires, rather than mounting a credential for a provider
/// nobody has read.
fn precondition(
    hosted: &HostedLanguageModel,
    adapter_config: &HuggingFaceConfig,
) -> std::result::Result<Met, Withheld> {
    let Some(provider) = adapter_config.routed_provider() else {
        return Err(Withheld::NoProviderPinned {
            model: hosted.model.clone(),
        });
    };
    let Some(attestation) = &hosted.attestation else {
        return Err(Withheld::NoAttestation {
            provider: provider.to_string(),
        });
    };
    let Some(terms) = attestation.attesting(provider) else {
        return Err(Withheld::ProviderNotAttested {
            provider: provider.to_string(),
            attested: attestation.describe(),
        });
    };
    if hosted.token.is_none() {
        return Err(Withheld::NoCredential {
            provider: provider.to_string(),
        });
    }
    Ok(Met {
        provider: provider.to_string(),
        attested: terms.describe(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attestation::ProviderTerms;
    use crate::config::{
        HUGGING_FACE_PROVIDER, LANGUAGE_MODEL_BASE_URL_VARIABLE, LANGUAGE_MODEL_PROVIDER_VARIABLE,
    };
    use qip_core::Timestamp;
    use std::collections::BTreeMap;

    /// Shaped like a token and not one.
    const TEST_TOKEN: &str = "hf_not_a_real_token_for_this_test";

    /// The provider the fixture model pins, spelled as the router's catalogue
    /// spells one.
    const TEST_PROVIDER: &str = "example-provider";

    /// The identifier with that provider pinned, which is what the router
    /// accepts and what the attestation is checked against.
    const PINNED_MODEL: &str = "example-org/example-model:example-provider";

    /// The same model with nothing pinned.
    const UNPINNED_MODEL: &str = "example-org/example-model";

    /// Whether `line` names `variable` as a whole token.
    ///
    /// `QIP_LANGUAGE_MODEL` is a prefix of `QIP_LANGUAGE_MODEL_PROVIDER` and
    /// of `QIP_LANGUAGE_MODEL_BASE_URL`, so a `contains` check for the short
    /// one passes on a sentence about either of the long ones and would report
    /// the wrong variable as named. This is the delimited-token check the
    /// testing rules ask for.
    fn names_variable(line: &str, variable: &str) -> bool {
        line.match_indices(variable).any(|(at, _)| {
            line[at + variable.len()..]
                .chars()
                .next()
                .is_none_or(|next| !(next.is_ascii_uppercase() || next == '_' || next.is_numeric()))
        })
    }

    fn configured(pairs: &[(&str, &str)]) -> DeepBrainConfig {
        let vars: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        DeepBrainConfig::parse(&vars).expect("a valid configuration")
    }

    /// The four variables a hosted provider needs, less whichever the caller
    /// blanks out. The attestation is added separately, being a file.
    fn provider_vars(without: &str) -> Vec<(&'static str, &'static str)> {
        [
            (LANGUAGE_MODEL_PROVIDER_VARIABLE, HUGGING_FACE_PROVIDER),
            (LANGUAGE_MODEL_VARIABLE, PINNED_MODEL),
            (LANGUAGE_MODEL_BASE_URL_VARIABLE, "http://127.0.0.1:9106"),
            (HF_TOKEN_VARIABLE, TEST_TOKEN),
        ]
        .into_iter()
        .filter(|(name, _)| *name != without)
        .collect()
    }

    /// A committed attestation on disk, named for the test that wrote it so
    /// two tests running at once cannot see each other's fixture.
    fn attestation_file(name: &str, providers: &[&str]) -> String {
        let records: Vec<String> = providers
            .iter()
            .map(|provider| {
                let record = ProviderTerms::new(
                    *provider,
                    "A. Operator",
                    Timestamp::from_civil(2026, 9, 4),
                    "https://example.test/terms",
                )
                .expect("a well-formed attestation");
                serde_json::to_string(&record).expect("a record serialises")
            })
            .collect();
        let path = std::env::temp_dir().join(format!(
            "qip-deepbrain-attestation-{}-{name}.json",
            std::process::id()
        ));
        std::fs::write(&path, format!("[{}]", records.join(","))).expect("the fixture is written");
        path.to_string_lossy().to_string()
    }

    fn with_attestation(pairs: &[(&'static str, &'static str)], path: &str) -> DeepBrainConfig {
        let mut all: Vec<(&str, &str)> = pairs.to_vec();
        all.push((ATTESTATION_PATH_VARIABLE, path));
        configured(&all)
    }

    #[test]
    fn with_the_provider_unset_the_chain_is_the_deterministic_model_alone() {
        // The state of every deployment today (ADR 0037). The failure this
        // prevents: a hosted adapter installed with nothing configured,
        // reporting unavailable on every call while the banner names it.
        let assembled = assemble(&configured(&[])).expect("no provider is a valid chain");
        assert_eq!(assembled.hosted, HostedDecision::NotRequested);
        assert_eq!(assembled.active_name(), "deterministic-local-v1");
        assert!(assembled.chain.is_available());
        assert!(
            names_variable(&assembled.describe(), LANGUAGE_MODEL_PROVIDER_VARIABLE),
            "the banner line does not say which variable would turn a provider on: {}",
            assembled.describe()
        );
    }

    #[test]
    fn a_pinned_provider_an_attestation_naming_it_and_a_credential_install_the_hosted_model() {
        // The one path that constructs a provider, and the premise every
        // refusal below depends on: with all three preconditions met the
        // adapter is installed, sits first in the chain, and the chain names
        // it. Asserted from the root's own state and its banner; nothing here
        // opens a socket.
        let path = attestation_file("installed", &[TEST_PROVIDER]);
        let assembled = assemble(&with_attestation(&provider_vars(""), &path))
            .expect("a fully attested provider assembles");
        let HostedDecision::Installed(hosted) = &assembled.hosted else {
            panic!("the provider was withheld: {}", assembled.describe());
        };
        assert_eq!(hosted.provider, TEST_PROVIDER);
        assert_eq!(
            hosted.attested,
            "attested by A. Operator under https://example.test/terms read at \
             2026-09-04T00:00:00.000Z"
        );
        assert_eq!(
            assembled.active_name(),
            PINNED_MODEL,
            "FallbackChain::name does not name the hosted model, so the deterministic model is \
             ahead of it and the provider would be configured, billed for nothing and never \
             consulted"
        );
        let line = assembled.describe();
        assert!(
            line.contains(PINNED_MODEL)
                && line.contains("127.0.0.1:9106")
                && line.contains("A. Operator"),
            "the banner does not name the model, the listener and who attested it: {line}"
        );
        assert!(
            !line.contains(TEST_TOKEN) && !format!("{assembled:?}").contains(TEST_TOKEN),
            "the banner or the Debug output carries the credential"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn without_a_credential_the_hosted_model_is_not_constructed_and_the_banner_names_the_variable()
    {
        // Not "installed dark": existing at all is what is refused, because an
        // adapter that exists is one a later refactor can make a call through.
        // The banner sends the operator to the mounted secret file rather than
        // to an environment value, which is a credential in
        // /proc/<pid>/environ and in every crash dump.
        let path = attestation_file("no-credential", &[TEST_PROVIDER]);
        let assembled = assemble(&with_attestation(&provider_vars(HF_TOKEN_VARIABLE), &path))
            .expect("a provider without a credential is withheld, not a start-up failure");
        assert_eq!(
            assembled.hosted,
            HostedDecision::Withheld(Withheld::NoCredential {
                provider: TEST_PROVIDER.to_string()
            })
        );
        assert_eq!(
            assembled.active_name(),
            "deterministic-local-v1",
            "the templates are not narrating"
        );
        let line = assembled.describe();
        assert!(
            line.contains("WITHHELD")
                && names_variable(&line, HF_TOKEN_VARIABLE)
                && line.contains(&format!(
                    "{HF_TOKEN_VARIABLE}{}",
                    qip_core::secret::FILE_SUFFIX
                )),
            "the banner does not name the credential variable and its file spelling: {line}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn without_an_attestation_the_hosted_model_is_not_constructed_and_the_banner_names_the_variable()
     {
        // The state ADR 0037 describes: a secret can exist while the terms are
        // still unread. A credential is not permission.
        let assembled = assemble(&configured(&provider_vars("")))
            .expect("a provider with no attestation is withheld, not a start-up failure");
        assert_eq!(
            assembled.hosted,
            HostedDecision::Withheld(Withheld::NoAttestation {
                provider: TEST_PROVIDER.to_string()
            })
        );
        assert_eq!(assembled.active_name(), "deterministic-local-v1");
        let line = assembled.describe();
        assert!(
            line.contains("WITHHELD") && names_variable(&line, ATTESTATION_PATH_VARIABLE),
            "the banner does not name the variable that mounts the attestation: {line}"
        );
    }

    #[test]
    fn an_attestation_for_another_provider_does_not_construct_this_models_provider() {
        // The failure this prevents, and the one a substring match would let
        // through: terms read for some provider on the router's list standing
        // in for terms read for the provider actually serving this model.
        let path = attestation_file("other-provider", &["another-provider"]);
        let assembled = assemble(&with_attestation(&provider_vars(""), &path))
            .expect("an attestation for another provider is withheld, not a start-up failure");
        assert_eq!(
            assembled.hosted,
            HostedDecision::Withheld(Withheld::ProviderNotAttested {
                provider: TEST_PROVIDER.to_string(),
                attested: "terms read for another-provider".to_string(),
            })
        );
        assert_eq!(assembled.active_name(), "deterministic-local-v1");
        let line = assembled.describe();
        assert!(
            line.contains(TEST_PROVIDER)
                && line.contains("another-provider")
                && names_variable(&line, ATTESTATION_PATH_VARIABLE),
            "the banner does not say which provider is unattested and which is: {line}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_model_that_pins_no_provider_is_not_constructed_however_complete_the_rest_is() {
        // Without the `:provider` suffix the router picks a provider per call,
        // so an attestation naming every provider on today's list would still
        // be a statement about a choice this process cannot see. Everything
        // else here is present, which is the point: the pin alone withholds
        // the adapter.
        let path = attestation_file("unpinned", &[TEST_PROVIDER]);
        let mut pairs = provider_vars(LANGUAGE_MODEL_VARIABLE);
        pairs.push((LANGUAGE_MODEL_VARIABLE, UNPINNED_MODEL));
        let assembled = assemble(&with_attestation(&pairs, &path))
            .expect("an unpinned model is withheld, not a start-up failure");
        let HostedDecision::Withheld(withheld) = &assembled.hosted else {
            panic!(
                "an unpinned model constructed a provider: {}",
                assembled.describe()
            );
        };
        assert_eq!(
            withheld,
            &Withheld::NoProviderPinned {
                model: UNPINNED_MODEL.to_string()
            }
        );
        assert_eq!(
            withheld.variable(),
            LANGUAGE_MODEL_VARIABLE,
            "the missing piece is attributed to the wrong variable"
        );
        assert_eq!(assembled.active_name(), "deterministic-local-v1");
        assert!(
            names_variable(&assembled.describe(), LANGUAGE_MODEL_VARIABLE),
            "the banner does not name the variable carrying the pin: {}",
            assembled.describe()
        );
        let _ = std::fs::remove_file(&path);
    }
}
