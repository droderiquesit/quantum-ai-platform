//! The language model this node's organisation narrates through.
//!
//! One chain, assembled from the configuration: the hosted adapter first when
//! `QIP_LANGUAGE_MODEL_PROVIDER` names one, and the deterministic model last
//! in every case, so a provider outage — or a credential nobody mounted —
//! degrades to templates rather than stopping reasoning (ADR 0037, decision
//! 4). This is the only composition root that builds the hosted adapter:
//! `qip-fastbrain` reads none of the variables because nothing on the fast
//! path consults a model (ADR 0008) and it has no proxy to reach one through
//! (ADR 0032); `qip-api` serves what the brains recorded.
//!
//! Separated from `main` so the property "provider unset means the
//! deterministic model alone, provider set means the hosted model is first
//! and named" is asserted by a test rather than read off a banner.

use crate::config::{DeepBrainConfig, HostedLanguageModel};
use qip_ai::language::{DeterministicModel, FallbackChain, LanguageModel};
use qip_core::error::{Error, Result};
use qip_reasoning_engine::providers::huggingface::{
    DEFAULT_DEADLINE, DEFAULT_MAX_BODY_BYTES, HuggingFaceConfig, HuggingFaceModel,
};
use std::sync::Arc;

/// The chain, with what it was assembled from.
#[derive(Debug)]
pub struct AssembledModel {
    pub chain: Arc<FallbackChain>,
    /// Whether a hosted adapter sits ahead of the deterministic model.
    pub hosted: Option<HostedSummary>,
}

/// What the banner says about the hosted adapter. Never the token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedSummary {
    pub model: String,
    pub base_url: String,
    pub credential_present: bool,
}

impl AssembledModel {
    /// The model that would actually serve a request: `FallbackChain::name`.
    pub fn active_name(&self) -> String {
        self.chain.name().to_string()
    }

    /// One line for the start-up banner.
    pub fn describe(&self) -> String {
        match &self.hosted {
            None => format!(
                "{} (no hosted provider; {} is not set)",
                self.active_name(),
                crate::config::LANGUAGE_MODEL_PROVIDER_VARIABLE
            ),
            Some(hosted) => format!(
                "{} — hosted {} via {}, {}; deterministic model behind it",
                self.active_name(),
                hosted.model,
                hosted.base_url,
                if hosted.credential_present {
                    "credential mounted"
                } else {
                    "NO CREDENTIAL, so the adapter reports unavailable and templates narrate"
                }
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
            hosted: None,
        });
    };
    let adapter = hosted_adapter(hosted)?;
    Ok(AssembledModel {
        hosted: Some(HostedSummary {
            model: hosted.model.clone(),
            base_url: hosted.base_url.clone(),
            credential_present: adapter.is_available(),
        }),
        chain: Arc::new(FallbackChain::new(vec![Arc::new(adapter), deterministic])),
    })
}

/// The adapter, with the reasoning stage's defaults for deadline and body.
///
/// The base URL was checked for loopback by the configuration; the adapter's
/// own constructor checks that it parses as plaintext `http`, which is the
/// refusal that catches `https://` and a malformed port.
fn hosted_adapter(hosted: &HostedLanguageModel) -> Result<HuggingFaceModel> {
    let config = HuggingFaceConfig::new(
        &hosted.model,
        &hosted.base_url,
        DEFAULT_DEADLINE,
        DEFAULT_MAX_BODY_BYTES,
    )
    .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    Ok(HuggingFaceModel::new(config, hosted.token.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        HUGGING_FACE_PROVIDER, LANGUAGE_MODEL_BASE_URL_VARIABLE, LANGUAGE_MODEL_PROVIDER_VARIABLE,
        LANGUAGE_MODEL_VARIABLE,
    };
    use qip_reasoning_engine::providers::huggingface::HF_TOKEN_VARIABLE;
    use std::collections::BTreeMap;

    /// Shaped like a token and not one.
    const TEST_TOKEN: &str = "hf_not_a_real_token_for_this_test";

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn configured(pairs: &[(&str, &str)]) -> DeepBrainConfig {
        DeepBrainConfig::parse(&vars(pairs)).expect("a valid configuration")
    }

    #[test]
    fn with_the_provider_unset_the_chain_is_the_deterministic_model_alone() {
        // The state of every deployment today (ADR 0037). The failure this
        // prevents: a hosted adapter installed with nothing configured,
        // reporting unavailable on every call while the banner names it.
        let assembled = assemble(&configured(&[])).expect("no provider is a valid chain");
        assert!(assembled.hosted.is_none());
        assert_eq!(assembled.active_name(), "deterministic-local-v1");
        assert!(assembled.chain.is_available());
        assert!(
            assembled
                .describe()
                .contains(LANGUAGE_MODEL_PROVIDER_VARIABLE),
            "the banner line does not say which variable would turn a provider on: {}",
            assembled.describe()
        );
    }

    #[test]
    fn with_the_provider_set_and_a_token_the_chain_names_the_hosted_model_first() {
        // The failure this prevents: the deterministic model installed ahead
        // of the hosted one, so the provider is configured, billed for
        // nothing, and never consulted.
        let assembled = assemble(&configured(&[
            (LANGUAGE_MODEL_PROVIDER_VARIABLE, HUGGING_FACE_PROVIDER),
            (LANGUAGE_MODEL_VARIABLE, "example-org/example-model"),
            (LANGUAGE_MODEL_BASE_URL_VARIABLE, "http://127.0.0.1:9106"),
            (HF_TOKEN_VARIABLE, TEST_TOKEN),
        ]))
        .expect("a fully configured provider assembles");
        let hosted = assembled
            .hosted
            .as_ref()
            .expect("a hosted adapter is installed");
        assert!(hosted.credential_present, "premise: the token was resolved");
        assert_eq!(
            assembled.active_name(),
            "example-org/example-model",
            "FallbackChain::name does not name the hosted model"
        );
        let line = assembled.describe();
        assert!(
            line.contains("example-org/example-model") && line.contains("127.0.0.1:9106"),
            "the banner line names neither the model nor the listener: {line}"
        );
        assert!(
            !line.contains(TEST_TOKEN) && !format!("{assembled:?}").contains(TEST_TOKEN),
            "the banner or Debug output carries the credential"
        );
    }

    #[test]
    fn with_the_provider_set_and_no_token_the_hosted_adapter_is_dark_and_templates_narrate() {
        // Built dark, exactly: the adapter is installed, reports unavailable,
        // and the chain's active model is the deterministic one. The banner
        // says so in capitals rather than reading as configured.
        let assembled = assemble(&configured(&[
            (LANGUAGE_MODEL_PROVIDER_VARIABLE, HUGGING_FACE_PROVIDER),
            (LANGUAGE_MODEL_VARIABLE, "example-org/example-model"),
            (LANGUAGE_MODEL_BASE_URL_VARIABLE, "http://localhost:9106"),
        ]))
        .expect("a provider without a credential assembles dark");
        let hosted = assembled
            .hosted
            .as_ref()
            .expect("a hosted adapter is installed");
        assert!(!hosted.credential_present);
        assert_eq!(assembled.active_name(), "deterministic-local-v1");
        assert!(assembled.describe().contains("NO CREDENTIAL"));
    }
}
