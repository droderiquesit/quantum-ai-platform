//! The language-model port, and the boundary that keeps it out of arithmetic.
//!
//! A language model is used here for exactly one thing: turning evidence into
//! structured, readable reasoning — a causal chain, a statement of what would
//! falsify a thesis, a summary of what changed. It is never used to compute.
//! Expected returns, confidences, position sizes, risk numbers and prices all
//! come from deterministic code operating on observed data.
//!
//! That boundary is enforced by [`NumericGuard`] rather than left to
//! discipline. The failure it prevents is specific: a model that emits a
//! confident-looking `"expected_return": 0.08` alongside genuine narrative, and
//! a caller that reads the field without noticing where it came from.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// What a caller wants from the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    /// Role and constraints.
    pub system: String,
    /// The task.
    pub prompt: String,
    /// Named context blocks: retrieved evidence, computed statistics, prior
    /// hypotheses. Supplied separately from the prompt so a caller cannot
    /// accidentally let context text be read as instructions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
    /// Expected output shape, when structure is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<OutputSchema>,
    pub max_output_tokens: u32,
    /// Zero for reproducibility, which is the default for anything whose output
    /// is recorded as part of a decision.
    pub temperature: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<Duration>,
}

impl ModelRequest {
    pub fn new(system: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            prompt: prompt.into(),
            context: BTreeMap::new(),
            schema: None,
            max_output_tokens: 1024,
            temperature: 0.0,
            deadline: None,
        }
    }

    pub fn with_context(mut self, name: impl Into<String>, body: impl Into<String>) -> Self {
        self.context.insert(name.into(), body.into());
        self
    }

    pub fn with_schema(mut self, schema: OutputSchema) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Rough token estimate, for budgeting before a call is made.
    ///
    /// Four characters per token is the usual approximation for English; it is
    /// used only to enforce budgets, never billed against.
    pub fn estimated_input_tokens(&self) -> u32 {
        let characters: usize = self.system.len()
            + self.prompt.len()
            + self.context.values().map(String::len).sum::<usize>();
        (characters / 4).max(1) as u32
    }
}

/// The kinds of field a model may fill.
///
/// There is deliberately no numeric variant. Numbers in a decision come from
/// computation, so a schema cannot ask a model for one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldKind {
    /// Free text.
    Text,
    /// One of a fixed set of labels.
    Enumeration { options: Vec<String> },
    /// A list of free-text items.
    TextList,
    /// A reference to an identifier the caller supplied in context, which the
    /// caller then verifies exists.
    Reference,
}

/// One field of an expected output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSpec {
    pub name: String,
    pub kind: FieldKind,
    pub required: bool,
    pub description: String,
}

impl FieldSpec {
    pub fn text(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: FieldKind::Text,
            required: true,
            description: description.into(),
        }
    }

    pub fn list(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: FieldKind::TextList,
            required: true,
            description: description.into(),
        }
    }

    pub fn one_of(
        name: impl Into<String>,
        options: &[&str],
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: FieldKind::Enumeration {
                options: options.iter().map(|s| (*s).to_string()).collect(),
            },
            required: true,
            description: description.into(),
        }
    }

    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }
}

/// The expected shape of a structured completion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSchema {
    pub name: String,
    pub fields: Vec<FieldSpec>,
}

impl OutputSchema {
    pub fn new(name: impl Into<String>, fields: Vec<FieldSpec>) -> Self {
        Self {
            name: name.into(),
            fields,
        }
    }

    /// Validate a structured output against the schema.
    pub fn validate(&self, value: &Value) -> Result<()> {
        let Some(object) = value.as_object() else {
            return Err(Error::schema(format!(
                "{} output must be an object",
                self.name
            )));
        };
        for field in &self.fields {
            match object.get(&field.name) {
                None if field.required => {
                    return Err(Error::schema(format!(
                        "{} output is missing required field `{}`",
                        self.name, field.name
                    )));
                }
                None => {}
                Some(actual) => match &field.kind {
                    FieldKind::Text | FieldKind::Reference => {
                        if !actual.is_string() {
                            return Err(Error::schema(format!(
                                "`{}` must be a string in {} output",
                                field.name, self.name
                            )));
                        }
                    }
                    FieldKind::TextList => {
                        let Some(items) = actual.as_array() else {
                            return Err(Error::schema(format!(
                                "`{}` must be a list in {} output",
                                field.name, self.name
                            )));
                        };
                        if items.iter().any(|i| !i.is_string()) {
                            return Err(Error::schema(format!(
                                "`{}` must contain only strings",
                                field.name
                            )));
                        }
                    }
                    FieldKind::Enumeration { options } => {
                        let Some(text) = actual.as_str() else {
                            return Err(Error::schema(format!(
                                "`{}` must be a string in {} output",
                                field.name, self.name
                            )));
                        };
                        if !options.iter().any(|o| o == text) {
                            return Err(Error::schema(format!(
                                "`{}` = `{text}` is not one of {options:?}",
                                field.name
                            )));
                        }
                    }
                },
            }
        }
        Ok(())
    }

    /// A human-readable rendering of the schema, for the prompt.
    pub fn describe(&self) -> String {
        let mut out = format!("Respond with a JSON object named {}:\n", self.name);
        for field in &self.fields {
            let kind = match &field.kind {
                FieldKind::Text => "string".to_string(),
                FieldKind::TextList => "array of strings".to_string(),
                FieldKind::Reference => "string identifier".to_string(),
                FieldKind::Enumeration { options } => format!("one of {options:?}"),
            };
            let requirement = if field.required {
                "required"
            } else {
                "optional"
            };
            out.push_str(&format!(
                "  {} ({kind}, {requirement}): {}\n",
                field.name, field.description
            ));
        }
        out.push_str("Do not include numeric values; numbers are computed elsewhere.\n");
        out
    }
}

/// What a model returned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Completion {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<Value>,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub finish_reason: FinishReason,
    pub produced_at: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Complete,
    MaxTokens,
    Refused,
    Timeout,
}

impl Completion {
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }

    pub fn is_complete(&self) -> bool {
        self.finish_reason == FinishReason::Complete
    }
}

/// Rejects numeric content in model output.
///
/// The guard walks a structured completion and reports every numeric leaf. The
/// reasoning engine treats any finding as a hard failure: a model that
/// volunteered a number is a model whose narrative may also be inventing
/// quantities, and the platform's numbers must be traceable to computation.
#[derive(Debug, Clone, Copy, Default)]
pub struct NumericGuard;

impl NumericGuard {
    /// Paths at which a numeric value appears, in dotted notation.
    pub fn find_numbers(value: &Value) -> Vec<String> {
        let mut found = Vec::new();
        Self::walk(value, String::new(), &mut found);
        found
    }

    /// Reject the output if it contains any numeric leaf.
    pub fn enforce(value: &Value) -> Result<()> {
        let numbers = Self::find_numbers(value);
        if numbers.is_empty() {
            return Ok(());
        }
        Err(Error::guard(format!(
            "model output contains numeric values at {numbers:?}; numbers must be computed, \
             not generated"
        )))
    }

    fn walk(value: &Value, path: String, found: &mut Vec<String>) {
        match value {
            Value::Number(_) => found.push(if path.is_empty() { "$".into() } else { path }),
            Value::Object(map) => {
                for (key, child) in map {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    Self::walk(child, child_path, found);
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    Self::walk(child, format!("{path}[{index}]"), found);
                }
            }
            _ => {}
        }
    }
}

/// A language model the platform can call.
pub trait LanguageModel: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;

    /// Whether the model is usable in this deployment. A remote model without
    /// credentials reports `false` and the caller falls back.
    fn is_available(&self) -> bool;

    fn complete(&self, request: &ModelRequest, now: Timestamp) -> Result<Completion>;

    /// Complete and validate against the request's schema, rejecting numbers.
    fn complete_structured(&self, request: &ModelRequest, now: Timestamp) -> Result<Completion> {
        let completion = self.complete(request, now)?;
        let Some(schema) = &request.schema else {
            return Ok(completion);
        };
        let Some(structured) = &completion.structured else {
            return Err(Error::schema(format!(
                "{} returned no structured output for schema {}",
                self.name(),
                schema.name
            )));
        };
        schema.validate(structured)?;
        NumericGuard::enforce(structured)?;
        Ok(completion)
    }
}

/// A deterministic, credential-free stand-in for a language model.
///
/// It is not a language model and does not pretend to be: it composes narrative
/// from the structured context the caller already supplied, following templates
/// registered per schema. That is enough for the platform to run its full
/// reasoning loop offline and for every test to be reproducible, and it makes
/// the shape of what a real model must return explicit.
#[derive(Debug, Default)]
pub struct DeterministicModel {
    templates: BTreeMap<String, fn(&ModelRequest) -> Value>,
}

impl DeterministicModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a generator for one schema.
    pub fn register(
        &mut self,
        schema_name: impl Into<String>,
        generator: fn(&ModelRequest) -> Value,
    ) {
        self.templates.insert(schema_name.into(), generator);
    }
}

impl LanguageModel for DeterministicModel {
    fn name(&self) -> &str {
        "deterministic-local-v1"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn complete(&self, request: &ModelRequest, now: Timestamp) -> Result<Completion> {
        let structured = match &request.schema {
            Some(schema) => match self.templates.get(&schema.name) {
                Some(generator) => Some(generator(request)),
                None => {
                    return Err(Error::unavailable(format!(
                        "no deterministic template registered for schema `{}`; register one or \
                         configure a language model provider",
                        schema.name
                    )));
                }
            },
            None => None,
        };

        let text = structured
            .as_ref()
            .map(|v| serde_json::to_string_pretty(v).unwrap_or_default())
            .unwrap_or_else(|| {
                // Without a schema, echo the context back as a summary. Honest
                // about being a stand-in rather than inventing prose.
                let mut summary = String::from("Deterministic summary of supplied context:\n");
                for (name, body) in &request.context {
                    let head: String = body.chars().take(280).collect();
                    summary.push_str(&format!("- {name}: {head}\n"));
                }
                summary
            });

        let output_tokens = (text.len() / 4).max(1) as u32;
        Ok(Completion {
            input_tokens: request.estimated_input_tokens(),
            output_tokens,
            finish_reason: if output_tokens > request.max_output_tokens {
                FinishReason::MaxTokens
            } else {
                FinishReason::Complete
            },
            text,
            structured,
            model: self.name().to_string(),
            produced_at: now,
        })
    }
}

/// Configuration for a hosted model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteModelConfig {
    /// Provider key, e.g. `anthropic` or `vertex`.
    pub provider: String,
    /// Model identifier as the provider names it.
    pub model: String,
    /// Environment variable holding the credential. The value is never stored
    /// in configuration or logged.
    pub credential_env: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

/// A hosted model this build cannot reach.
///
/// The platform is built to use one, and everything downstream is written
/// against [`LanguageModel`], but no credential, egress path or HTTP client
/// with TLS exists in this build. Rather than pretend, the adapter reports
/// itself unavailable and says exactly what is missing, so a deployment fails
/// at start-up instead of at the first investigation.
#[derive(Debug, Clone)]
pub struct RemoteModel {
    config: RemoteModelConfig,
    credential_present: bool,
}

impl RemoteModel {
    pub fn new(config: RemoteModelConfig) -> Self {
        let credential_present =
            std::env::var(&config.credential_env).is_ok_and(|v| !v.trim().is_empty());
        Self {
            config,
            credential_present,
        }
    }

    /// Construct without consulting the environment, for tests.
    pub fn with_credential_present(config: RemoteModelConfig, present: bool) -> Self {
        Self {
            config,
            credential_present: present,
        }
    }

    pub fn config(&self) -> &RemoteModelConfig {
        &self.config
    }

    /// What a deployment must supply for this adapter to work.
    pub fn requirement(&self) -> String {
        format!(
            "provider `{}` model `{}` requires the credential in ${} and outbound HTTPS to {}. \
             This build has no TLS-capable HTTP client, so the transport must be supplied as well. \
             See docs/operations/external-dependencies.md",
            self.config.provider,
            self.config.model,
            self.config.credential_env,
            self.config
                .endpoint
                .as_deref()
                .unwrap_or("the provider's default endpoint")
        )
    }
}

impl LanguageModel for RemoteModel {
    fn name(&self) -> &str {
        &self.config.model
    }

    fn is_available(&self) -> bool {
        // Even with a credential, the transport is absent in this build.
        false
    }

    fn complete(&self, _request: &ModelRequest, _now: Timestamp) -> Result<Completion> {
        Err(Error::unavailable(format!(
            "{} is not callable from this build{}. {}",
            self.config.model,
            if self.credential_present {
                ""
            } else {
                " and no credential is configured"
            },
            self.requirement()
        )))
    }
}

/// Tries each model in order, falling back to the next unavailable one.
///
/// Configured with a hosted model first and the deterministic stand-in last, so
/// a deployment with credentials uses them and one without still runs.
#[derive(Debug)]
pub struct FallbackChain {
    models: Vec<std::sync::Arc<dyn LanguageModel>>,
}

impl FallbackChain {
    pub fn new(models: Vec<std::sync::Arc<dyn LanguageModel>>) -> Self {
        Self { models }
    }

    /// The model that would actually serve a request.
    pub fn active(&self) -> Option<&std::sync::Arc<dyn LanguageModel>> {
        self.models.iter().find(|m| m.is_available())
    }
}

impl LanguageModel for FallbackChain {
    fn name(&self) -> &str {
        self.active().map_or("none", |m| m.name())
    }

    fn is_available(&self) -> bool {
        self.active().is_some()
    }

    fn complete(&self, request: &ModelRequest, now: Timestamp) -> Result<Completion> {
        for model in &self.models {
            if !model.is_available() {
                continue;
            }
            match model.complete(request, now) {
                Ok(completion) => return Ok(completion),
                // A model that fails at call time falls through to the next, so
                // a provider outage degrades rather than stops reasoning.
                Err(_) => continue,
            }
        }
        Err(Error::unavailable(
            "no language model in the chain is available",
        ))
    }
}
