//! Layered configuration.
//!
//! Resolution order, later wins: built-in defaults, then a JSON file, then
//! `QIP_*` environment variables. Environment overrides use double underscore
//! for nesting, so `QIP_RISK__MAX_LEVERAGE=2.0` sets `risk.max_leverage`.
//!
//! Secrets are never read from the config file. They come from the environment
//! or a secret provider, and [`Config::redacted`] keeps them out of logs and the
//! `/system/config` endpoint.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Io(String),
    Parse(String),
    Missing(String),
    Type { key: String, expected: &'static str },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(m) => write!(f, "config io error: {m}"),
            Self::Parse(m) => write!(f, "config parse error: {m}"),
            Self::Missing(k) => write!(f, "missing config key: {k}"),
            Self::Type { key, expected } => write!(f, "config key {key} is not {expected}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<ConfigError> for crate::Error {
    fn from(e: ConfigError) -> Self {
        crate::Error::Invalid(e.to_string())
    }
}

/// Keys whose values are masked wherever configuration is displayed.
const SECRET_MARKERS: [&str; 6] = [
    "secret",
    "token",
    "password",
    "key",
    "credential",
    "api_key",
];

/// A merged configuration tree.
#[derive(Debug, Clone, Default)]
pub struct Config {
    root: Map<String, Value>,
}

impl Config {
    pub fn empty() -> Self {
        Self { root: Map::new() }
    }

    pub fn from_value(value: Value) -> Self {
        match value {
            Value::Object(root) => Self { root },
            other => {
                let mut root = Map::new();
                root.insert("value".into(), other);
                Self { root }
            }
        }
    }

    /// Parse a JSON document.
    pub fn from_json(text: &str) -> Result<Self, ConfigError> {
        let value: Value =
            serde_json::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        Ok(Self::from_value(value))
    }

    /// Load a JSON file. A missing file yields an empty config, letting callers
    /// layer an optional override without special-casing existence.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::empty());
        }
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io(e.to_string()))?;
        Self::from_json(&text)
    }

    /// Overlay `other` on top of `self`; objects merge recursively, scalars and
    /// arrays are replaced wholesale.
    pub fn merge(mut self, other: Self) -> Self {
        merge_objects(&mut self.root, other.root);
        self
    }

    /// Apply `QIP_`-prefixed environment variables.
    pub fn with_env(self) -> Self {
        self.with_env_from(std::env::vars())
    }

    /// Testable form of [`Self::with_env`].
    pub fn with_env_from<I: IntoIterator<Item = (String, String)>>(mut self, vars: I) -> Self {
        let mut pairs: Vec<(String, String)> = vars
            .into_iter()
            .filter_map(|(k, v)| k.strip_prefix("QIP_").map(|rest| (rest.to_string(), v)))
            .collect();
        // Sort so a deeper key never depends on iteration order of the environment.
        pairs.sort();
        for (key, raw) in pairs {
            let path: Vec<String> = key.split("__").map(|s| s.to_ascii_lowercase()).collect();
            set_path(&mut self.root, &path, parse_scalar(&raw));
        }
        self
    }

    /// Look up a dotted key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        let mut parts = key.split('.');
        let mut current = self.root.get(parts.next()?)?;
        for part in parts {
            current = current.as_object()?.get(part)?;
        }
        Some(current)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key)?.as_bool()
    }

    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key)?.as_i64()
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key)?.as_f64()
    }

    pub fn require_str(&self, key: &str) -> Result<&str, ConfigError> {
        self.get(key)
            .ok_or_else(|| ConfigError::Missing(key.to_string()))?
            .as_str()
            .ok_or(ConfigError::Type {
                key: key.to_string(),
                expected: "a string",
            })
    }

    /// Deserialize a subtree into a typed section.
    pub fn section<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<T, ConfigError> {
        let value = self
            .get(key)
            .cloned()
            .ok_or_else(|| ConfigError::Missing(key.to_string()))?;
        serde_json::from_value(value).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Deserialize a subtree, falling back to `T::default()` when absent.
    pub fn section_or_default<T>(&self, key: &str) -> Result<T, ConfigError>
    where
        T: for<'de> Deserialize<'de> + Default,
    {
        match self.get(key) {
            None => Ok(T::default()),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|e| ConfigError::Parse(format!("{key}: {e}"))),
        }
    }

    pub fn set(&mut self, key: &str, value: Value) {
        let path: Vec<String> = key.split('.').map(|s| s.to_string()).collect();
        set_path(&mut self.root, &path, value);
    }

    pub fn as_value(&self) -> Value {
        Value::Object(self.root.clone())
    }

    /// A copy with every secret-looking leaf replaced by `"***"`.
    pub fn redacted(&self) -> Value {
        let mut root = self.root.clone();
        redact_object(&mut root);
        Value::Object(root)
    }

    /// Flatten to dotted keys, for diffing two environments.
    pub fn flatten(&self) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        flatten_into(&self.root, String::new(), &mut out);
        out
    }
}

fn merge_objects(base: &mut Map<String, Value>, overlay: Map<String, Value>) {
    for (k, v) in overlay {
        match (base.get_mut(&k), v) {
            (Some(Value::Object(existing)), Value::Object(incoming)) => {
                merge_objects(existing, incoming);
            }
            (_, incoming) => {
                base.insert(k, incoming);
            }
        }
    }
}

fn set_path(root: &mut Map<String, Value>, path: &[String], value: Value) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        root.insert(head.clone(), value);
        return;
    }
    let entry = root
        .entry(head.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Some(obj) = entry.as_object_mut() {
        set_path(obj, rest, value);
    }
}

/// Interpret an environment string as bool, integer, float, JSON or string.
fn parse_scalar(raw: &str) -> Value {
    match raw {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        "null" => return Value::Null,
        _ => {}
    }
    if let Ok(i) = raw.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(f) = raw.parse::<f64>()
        && f.is_finite()
    {
        return Value::from(f);
    }
    let looks_structured = (raw.starts_with('[') && raw.ends_with(']'))
        || (raw.starts_with('{') && raw.ends_with('}'));
    if looks_structured && let Ok(v) = serde_json::from_str::<Value>(raw) {
        return v;
    }
    Value::String(raw.to_string())
}

fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SECRET_MARKERS.iter().any(|m| lower.contains(m))
}

fn redact_object(obj: &mut Map<String, Value>) {
    for (k, v) in obj.iter_mut() {
        if is_secret_key(k) && !v.is_object() {
            *v = Value::String("***".into());
        } else if let Value::Object(inner) = v {
            redact_object(inner);
        }
    }
}

fn flatten_into(obj: &Map<String, Value>, prefix: String, out: &mut BTreeMap<String, Value>) {
    for (k, v) in obj {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Value::Object(inner) => flatten_into(inner, key, out),
            other => {
                out.insert(key, other.clone());
            }
        }
    }
}

/// Deployment environment. Controls which safety defaults apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    #[default]
    Local,
    Development,
    Staging,
    Production,
}

impl Environment {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "dev" | "development" => Self::Development,
            "stage" | "staging" => Self::Staging,
            "prod" | "production" => Self::Production,
            _ => Self::Local,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Development => "development",
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }

    /// Whether synthetic data providers may be used. Production must run on
    /// licensed feeds only, so the kernel refuses to start otherwise.
    pub fn allows_synthetic_data(&self) -> bool {
        !matches!(self, Self::Production)
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
