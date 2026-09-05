//! The GitOps control plane ADR 0036 brings back, pinned to the shape it
//! records.
//!
//! ADR 0024 retired Argo CD and Kargo with the cluster they ran on; ADR 0036
//! returns them on a per-environment GKE Autopilot control-plane cluster that
//! runs controllers and nothing else, with Config Connector applying
//! `RunService` manifests to the Cloud Run runtime ADR 0024 provisioned. The
//! trading binaries never run as Pods; the execution nodes stay with
//! Terraform.
//!
//! Every test here reads the committed configuration and asserts a property
//! the design is worth nothing without. The failures they prevent are the
//! ones the first GitOps attempt actually produced, recorded in ADR 0017 and
//! ADR 0024: a manifest tree a reviewer took for the running system that
//! nothing applied; a crash-looping build that produced a green pipeline
//! because nothing assessed health; a values file that recorded a digest no
//! service served. And the ones that would be new: a `Deployment` running a
//! `qip-*` image, which is the runtime ADR 0024 retired coming back through
//! the door the controllers opened; a prod `Stage` a person could click; an
//! Argo server with a public address.
//!
//! # Premise first
//!
//! Every test asserts that the thing it reads exists and has the shape it
//! expects before asserting anything about it. A test written against a
//! directory that is not there yet fails naming the directory rather than
//! passing over an empty walk, so the suite can be committed ahead of the
//! files it pins and is red — correctly — until they land.
//!
//! # YAML
//!
//! The workspace permits `serde` and `serde_json` and no YAML crate (ADR
//! 0002, ADR 0009). The manifests are read by a small reader for the subset
//! of YAML a hand-written Kubernetes manifest uses — block mappings, block
//! sequences, quoted and plain scalars, literal and folded block scalars,
//! empty flow collections and one-line flow sequences of scalars — into
//! `serde_json::Value`. Anything outside that subset (anchors, aliases, tags,
//! multi-line plain scalars, nested flow collections) makes the reader panic
//! naming the file and line rather than guess, which is the safe direction: a
//! construct the reader cannot see is a property it cannot check.

// The workspace denies `panic_in_result_fn` for production code. In a test
// the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read, repository_root};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

// --- where everything is ------------------------------------------------------

const GITOPS: &str = "infrastructure/gitops";
const BOOTSTRAP: &str = "infrastructure/gitops/bootstrap";
const ARGOCD: &str = "infrastructure/gitops/argocd";
const KARGO: &str = "infrastructure/gitops/kargo";
const ENVS: &str = "infrastructure/gitops/envs";
const CATALOGUE: &str = "infrastructure/terraform/catalogue.tf";
const ROOT_VARIABLES: &str = "infrastructure/terraform/variables.tf";
const MODULES: &str = "infrastructure/terraform/modules";
const CLOUD_RUN_MODULE: &str = "infrastructure/terraform/modules/cloudrun/main.tf";
const VENDORED: &str = "infrastructure/egress/vendored-images.txt";
const DEPLOY_WORKFLOW: &str = ".github/workflows/deploy.yml";
const INFRA_WORKFLOW: &str = ".github/workflows/infra.yml";

/// The four environments, in promotion order. The Kargo chain is asserted
/// in exactly this order and the Argo CD Applications are one per entry.
const ENVIRONMENTS: [&str; 4] = ["dev", "test", "stage", "prod"];

/// The binaries the workspace builds. None may be the image of a Kubernetes
/// workload anywhere under `infrastructure/gitops`; three of them are the
/// images of `RunService` manifests, which is a different thing.
const TRADING_BINARIES: [&str; 6] = [
    "qip-api",
    "qip-fastbrain",
    "qip-deepbrain",
    "qip-edge-node",
    "qip-web",
    "qip-cli",
];

/// The marker an implementer may leave on an image reference that has not
/// been pinned yet. A reference carrying it fails the pin test naming it,
/// which is the intended state until the digest is resolved and reviewed:
/// the marker is a placeholder, not an exemption.
const TO_PIN: &str = "TO-PIN";

/// Environments whose kustomization still carries `TO-PIN` for the trading
/// binaries, each with why. An entry is a decision, not a convenience: a
/// failing digest check would otherwise be the permanent state of the
/// gate, because prod is promoted by nobody until an ADR says otherwise and
/// no pipeline builds for an `unprovisioned` project. Each entry is
/// re-proved by `every_environment_awaiting_its_first_promotion_is_still_unpinned_and_dev_is_not`,
/// so it expires the moment a promotion pins the environment; and `dev`,
/// the environment the pipeline builds for and the chain's first link, may
/// never be listed.
const NEVER_PROMOTED_TO: &[(&str, &str)] = &[
    (
        "test",
        "No pipeline has ever built for test: its project_id is `unprovisioned` in its tfvars, \
         so no registry exists to hold an image and no Warehouse has seen freight for it. The \
         marker is replaced by Kargo's first promotion into test, which a person approves \
         (ADR 0036 decision 6), and this entry is deleted in the same commit.",
    ),
    (
        "stage",
        "As test: an `unprovisioned` project, no registry, no freight. Stage takes freight \
         from test, so it is pinned only after test is, by a person's approval in Kargo.",
    ),
    (
        "prod",
        "ADR 0036 decision 6: prod is promoted by nobody until an ADR lifts the refusal in \
         its Stage's `fail` step, so no promotion will ever write a digest here as the chain \
         stands. The marker is that decision made visible in the manifest, and lifting it is \
         the ADR, not an edit here.",
    ),
];

/// Whether an environment is one `NEVER_PROMOTED_TO` argues for.
fn awaiting_first_promotion(environment: &str) -> bool {
    NEVER_PROMOTED_TO
        .iter()
        .any(|(name, _)| *name == environment)
}

/// The root every secret volume is mounted under, `modules/cloudrun`'s
/// `local.secret_root`.
const SECRET_ROOT: &str = "/var/run/secrets/qip/";

/// The Config Connector annotation that turns a delete into a release.
const DELETION_POLICY: &str = "cnrm.cloud.google.com/deletion-policy";

// --- a YAML subset reader ------------------------------------------------------

mod yaml {
    use serde_json::{Map, Value};

    /// One source line: its raw text, its text with any comment removed, and
    /// the indent of the raw line.
    struct Line {
        number: usize,
        indent: usize,
        raw: String,
        text: String,
    }

    /// A line with an inline comment removed, outside quotes.
    fn strip_comment(line: &str) -> String {
        let mut out = String::new();
        let mut single = false;
        let mut double = false;
        let mut previous = ' ';
        let mut escaped = false;
        for c in line.chars() {
            if escaped {
                escaped = false;
                out.push(c);
                previous = c;
                continue;
            }
            match c {
                '\\' if double => escaped = true,
                '\'' if !double => single = !single,
                '"' if !single => double = !double,
                '#' if !single && !double && previous.is_whitespace() => break,
                _ => {}
            }
            out.push(c);
            previous = c;
        }
        out.trim_end().to_string()
    }

    /// A double-quoted scalar's escapes, resolved: the ones a manifest uses.
    fn unescape(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(' ') => out.push(' '),
                Some('/') => out.push('/'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        }
        out
    }

    /// Whether a double-quoted scalar's text closes on this line: it ends
    /// with a `"` that is not escaped and is not the opening quote.
    fn closes_double_quoted(text: &str) -> bool {
        let bytes = text.as_bytes();
        if bytes.len() < 2 || bytes[bytes.len() - 1] != b'"' {
            return false;
        }
        let mut backslashes = 0;
        let mut index = bytes.len() - 1;
        while index > 0 && bytes[index - 1] == b'\\' {
            backslashes += 1;
            index -= 1;
        }
        backslashes % 2 == 0
    }

    /// A double-quoted scalar that may run over several lines, folded the
    /// way YAML folds it: a line break becomes a space, a break after a
    /// trailing `\` becomes nothing, and each continuation line's leading
    /// whitespace is dropped. The kustomize overlays write their patches
    /// this way.
    fn double_quoted(
        lines: &[Line],
        pos: &mut usize,
        first: &str,
        origin: &str,
        line: &Line,
    ) -> Value {
        let mut pieces: Vec<String> = vec![first[1..].to_string()];
        let mut closed = closes_double_quoted(first);
        while !closed {
            if *pos >= lines.len() {
                unsupported(origin, line, "an unterminated double-quoted scalar");
            }
            let next = &lines[*pos];
            *pos += 1;
            let text = next.raw.trim().to_string();
            closed = closes_double_quoted(&text);
            pieces.push(text);
        }
        if let Some(last) = pieces.last_mut() {
            last.pop();
        }
        let mut folded = String::new();
        for (index, piece) in pieces.iter().enumerate() {
            if index > 0 {
                if folded.ends_with('\\') && !folded.ends_with("\\\\") {
                    folded.pop();
                } else if piece.is_empty() {
                    folded.push('\n');
                    continue;
                } else {
                    folded.push(' ');
                }
            }
            folded.push_str(piece);
        }
        Value::String(unescape(&folded))
    }

    /// Whether a value's text opens a double-quoted scalar that this line
    /// does not close.
    fn opens_multiline_double_quoted(text: &str) -> bool {
        text.starts_with('"') && !closes_double_quoted(text)
    }

    /// Whether a single-quoted scalar's text closes on this line: it ends
    /// with a `'` that is not half of an escaped `''`, and is not the
    /// opening quote.
    fn closes_single_quoted(text: &str) -> bool {
        let bytes = text.as_bytes();
        if bytes.len() < 2 || bytes[bytes.len() - 1] != b'\'' {
            return false;
        }
        let mut quotes = 0;
        let mut index = bytes.len();
        while index > 1 && bytes[index - 1] == b'\'' {
            quotes += 1;
            index -= 1;
        }
        quotes % 2 == 1
    }

    fn opens_multiline_single_quoted(text: &str) -> bool {
        text.starts_with('\'') && !closes_single_quoted(text)
    }

    /// A single-quoted scalar over several lines, folded like the
    /// double-quoted one; `''` is the one escape.
    fn single_quoted(
        lines: &[Line],
        pos: &mut usize,
        first: &str,
        origin: &str,
        line: &Line,
    ) -> Value {
        let mut pieces: Vec<String> = vec![first[1..].to_string()];
        let mut closed = closes_single_quoted(first);
        while !closed {
            if *pos >= lines.len() {
                unsupported(origin, line, "an unterminated single-quoted scalar");
            }
            let next = &lines[*pos];
            *pos += 1;
            let text = next.raw.trim().to_string();
            closed = closes_single_quoted(&text);
            pieces.push(text);
        }
        if let Some(last) = pieces.last_mut() {
            last.pop();
        }
        let mut folded = String::new();
        for (index, piece) in pieces.iter().enumerate() {
            if index > 0 {
                if piece.is_empty() {
                    folded.push('\n');
                    continue;
                }
                folded.push(' ');
            }
            folded.push_str(piece);
        }
        Value::String(folded.replace("''", "'"))
    }

    /// A plain scalar continued on following lines indented deeper than its
    /// key, folded with single spaces — the shape a long annotation value
    /// takes when a YAML writer wraps it.
    fn plain_continuation(lines: &[Line], pos: &mut usize, indent: usize, first: Value) -> Value {
        let Value::String(mut text) = first else {
            return first;
        };
        while let Some(next) = next_structural(lines, *pos) {
            let line = &lines[next];
            if line.indent <= indent || is_item(&line.text) {
                break;
            }
            text.push(' ');
            text.push_str(line.text.trim());
            *pos = next + 1;
        }
        Value::String(text)
    }

    fn lex(source: &str) -> Vec<Line> {
        source
            .lines()
            .enumerate()
            .map(|(index, raw)| {
                let indent = raw.len() - raw.trim_start().len();
                let trimmed = raw.trim();
                let text = if trimmed.starts_with('#') {
                    String::new()
                } else {
                    strip_comment(trimmed)
                };
                Line {
                    number: index + 1,
                    indent,
                    raw: raw.to_string(),
                    text,
                }
            })
            .collect()
    }

    fn is_item(text: &str) -> bool {
        text == "-" || text.starts_with("- ")
    }

    /// The next line that carries structure, at or after `from`.
    fn next_structural(lines: &[Line], from: usize) -> Option<usize> {
        (from..lines.len()).find(|&index| !lines[index].text.is_empty())
    }

    fn unsupported(origin: &str, line: &Line, what: &str) -> ! {
        panic!(
            "{origin}:{}: {what}; the YAML subset reader in gitops.rs does not read this and \
             refuses to guess: `{}`",
            line.number, line.raw
        )
    }

    /// Whether the text after a `- ` is a mapping entry rather than a scalar.
    fn looks_like_entry(text: &str) -> bool {
        if text.starts_with('"') || text.starts_with('\'') {
            // `- "a: b"` is a quoted scalar; `- "a": b` is an entry.
            let quote = text.chars().next().unwrap_or('"');
            let Some(close) = text[1..].find(quote) else {
                return false;
            };
            let after = text[close + 2..].trim_start();
            return after.starts_with(':');
        }
        if text.starts_with('[') || text.starts_with('{') {
            return false;
        }
        text.ends_with(':') || text.contains(": ")
    }

    /// `key: rest`, with a quoted or plain key.
    fn split_entry(text: &str, origin: &str, line: &Line) -> (String, String) {
        if let Some(quote) = text.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let Some(close) = text[1..].find(quote) else {
                unsupported(origin, line, "an unterminated quoted key");
            };
            let key = text[1..=close].to_string();
            let after = text[close + 2..].trim_start();
            let Some(rest) = after.strip_prefix(':') else {
                unsupported(origin, line, "a quoted key not followed by a colon");
            };
            return (key, rest.trim().to_string());
        }
        if let Some(key) = text.strip_suffix(':') {
            if !key.contains(": ") {
                return (key.trim().to_string(), String::new());
            }
        }
        let Some((key, rest)) = text.split_once(": ") else {
            unsupported(
                origin,
                line,
                "a line that is neither `key: value` nor a sequence item",
            );
        };
        (key.trim().to_string(), rest.trim().to_string())
    }

    fn scalar(text: &str, origin: &str, line: &Line) -> Value {
        let text = text.trim();
        if let Some(inner) = text.strip_prefix('"') {
            let Some(inner) = inner.strip_suffix('"') else {
                unsupported(origin, line, "an unterminated double-quoted scalar");
            };
            return Value::String(unescape(inner));
        }
        if let Some(inner) = text.strip_prefix('\'') {
            let Some(inner) = inner.strip_suffix('\'') else {
                unsupported(origin, line, "an unterminated single-quoted scalar");
            };
            return Value::String(inner.replace("''", "'"));
        }
        if text == "[]" {
            return Value::Array(Vec::new());
        }
        if text == "{}" {
            return Value::Object(Map::new());
        }
        if let Some(inner) = text.strip_prefix('[') {
            let Some(inner) = inner.strip_suffix(']') else {
                unsupported(origin, line, "an unterminated flow sequence");
            };
            if inner.contains('[') || inner.contains('{') {
                unsupported(origin, line, "a nested flow collection");
            }
            return Value::Array(
                inner
                    .split(',')
                    .map(|item| scalar(item, origin, line))
                    .collect(),
            );
        }
        if text.starts_with('{') {
            unsupported(origin, line, "a flow mapping");
        }
        if text.starts_with('&') || text.starts_with('*') || text.starts_with('!') {
            unsupported(origin, line, "an anchor, alias or tag");
        }
        match text {
            "true" | "True" => return Value::Bool(true),
            "false" | "False" => return Value::Bool(false),
            "null" | "~" | "Null" => return Value::Null,
            _ => {}
        }
        // `256` is a number; `0400` is a string, because YAML 1.1 reads it as
        // octal and YAML 1.2 as decimal, and a mode that depends on which is
        // a mode nobody set.
        if !text.is_empty()
            && text.chars().all(|c| c.is_ascii_digit())
            && (text.len() == 1 || !text.starts_with('0'))
        {
            if let Ok(number) = text.parse::<u64>() {
                return Value::Number(number.into());
            }
        }
        Value::String(text.to_string())
    }

    /// A `|` or `>` block scalar: every following line deeper than the
    /// parent, with the common indent removed.
    fn block_scalar(lines: &[Line], pos: &mut usize, parent: usize, folded: bool) -> Value {
        let mut collected: Vec<&str> = Vec::new();
        while *pos < lines.len() {
            let line = &lines[*pos];
            if line.raw.trim().is_empty() || line.indent > parent {
                collected.push(&line.raw);
                *pos += 1;
            } else {
                break;
            }
        }
        while collected.last().is_some_and(|last| last.trim().is_empty()) {
            collected.pop();
        }
        let common = collected
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.len() - line.trim_start().len())
            .min()
            .unwrap_or(0);
        let body: Vec<&str> = collected
            .iter()
            .map(|line| {
                if line.len() >= common {
                    &line[common..]
                } else {
                    ""
                }
            })
            .collect();
        Value::String(body.join(if folded { " " } else { "\n" }))
    }

    fn parse_block(lines: &[Line], pos: &mut usize, indent: usize, origin: &str) -> Value {
        if is_item(&lines[*pos].text) {
            parse_sequence(lines, pos, indent, origin)
        } else {
            parse_mapping(lines, pos, indent, None, origin)
        }
    }

    fn parse_sequence(lines: &[Line], pos: &mut usize, indent: usize, origin: &str) -> Value {
        let mut items = Vec::new();
        while *pos < lines.len() {
            let line = &lines[*pos];
            if line.text.is_empty() {
                *pos += 1;
                continue;
            }
            if line.indent < indent || (line.indent == indent && !is_item(&line.text)) {
                break;
            }
            if line.indent > indent {
                unsupported(
                    origin,
                    line,
                    "a line indented deeper than the sequence it is in",
                );
            }
            let rest = line.text[1..].trim_start();
            *pos += 1;
            if rest.is_empty() {
                items.push(match next_structural(lines, *pos) {
                    Some(next) if lines[next].indent > indent => {
                        *pos = next;
                        parse_block(lines, pos, lines[next].indent, origin)
                    }
                    _ => Value::Null,
                });
            } else if is_item(rest) {
                unsupported(
                    origin,
                    line,
                    "a sequence nested on the same line as its parent",
                );
            } else if looks_like_entry(rest) {
                let inner = indent + (line.text.len() - rest.len());
                items.push(parse_mapping(lines, pos, inner, Some((rest, line)), origin));
            } else if rest == "|" || rest == "|-" || rest == ">" || rest == ">-" {
                items.push(block_scalar(lines, pos, indent, rest.starts_with('>')));
            } else if opens_multiline_double_quoted(rest) {
                items.push(double_quoted(lines, pos, rest, origin, line));
            } else if opens_multiline_single_quoted(rest) {
                items.push(single_quoted(lines, pos, rest, origin, line));
            } else {
                let value = scalar(rest, origin, line);
                let quoted = rest.starts_with('"') || rest.starts_with('\'');
                items.push(if quoted {
                    value
                } else {
                    plain_continuation(lines, pos, indent, value)
                });
            }
        }
        Value::Array(items)
    }

    fn parse_mapping(
        lines: &[Line],
        pos: &mut usize,
        indent: usize,
        first: Option<(&str, &Line)>,
        origin: &str,
    ) -> Value {
        let mut map = Map::new();
        let mut pending: Option<(String, &Line)> =
            first.map(|(text, line)| (text.to_string(), line));
        loop {
            let (text, line) = match pending.take() {
                Some(entry) => entry,
                None => {
                    let Some(next) = next_structural(lines, *pos) else {
                        break;
                    };
                    let line = &lines[next];
                    if line.indent < indent || (line.indent == indent && is_item(&line.text)) {
                        *pos = next;
                        break;
                    }
                    if line.indent > indent {
                        unsupported(
                            origin,
                            line,
                            "a line indented deeper than the mapping it is in (a multi-line plain \
                             scalar, or a mis-indented key)",
                        );
                    }
                    *pos = next + 1;
                    (line.text.clone(), line)
                }
            };
            let (key, rest) = split_entry(&text, origin, line);
            if key == "<<" {
                unsupported(origin, line, "a merge key");
            }
            let value = if rest.is_empty() {
                match next_structural(lines, *pos) {
                    Some(next) if lines[next].indent > indent => {
                        *pos = next;
                        parse_block(lines, pos, lines[next].indent, origin)
                    }
                    Some(next) if lines[next].indent == indent && is_item(&lines[next].text) => {
                        *pos = next;
                        parse_sequence(lines, pos, indent, origin)
                    }
                    _ => Value::Null,
                }
            } else if rest == "|" || rest == "|-" || rest == "|+" || rest == ">" || rest == ">-" {
                block_scalar(lines, pos, indent, rest.starts_with('>'))
            } else if opens_multiline_double_quoted(&rest) {
                double_quoted(lines, pos, &rest, origin, line)
            } else if opens_multiline_single_quoted(&rest) {
                single_quoted(lines, pos, &rest, origin, line)
            } else {
                let value = scalar(&rest, origin, line);
                let quoted = rest.starts_with('"') || rest.starts_with('\'');
                if quoted {
                    value
                } else {
                    plain_continuation(lines, pos, indent, value)
                }
            };
            if map.contains_key(&key) {
                unsupported(origin, line, "a duplicate key");
            }
            map.insert(key, value);
        }
        Value::Object(map)
    }

    /// Every document in a YAML source, in order, empty documents dropped.
    pub(crate) fn documents(source: &str, origin: &str) -> Vec<Value> {
        let mut documents = Vec::new();
        for (index, document) in source.split("\n---").enumerate() {
            // The first chunk may itself start with `---`.
            let document = if index == 0 {
                document.strip_prefix("---").unwrap_or(document)
            } else {
                document
            };
            let lines = lex(document);
            let Some(start) = next_structural(&lines, 0) else {
                continue;
            };
            let mut pos = start;
            let value = parse_block(&lines, &mut pos, lines[start].indent, origin);
            if let Some(left) = next_structural(&lines, pos) {
                unsupported(
                    origin,
                    &lines[left],
                    "content after the document's root node",
                );
            }
            documents.push(value);
        }
        documents
    }
}

// --- reading manifests -------------------------------------------------------

/// One YAML document and where it came from, relative to the repository.
#[derive(Debug, Clone)]
struct Manifest {
    path: String,
    value: Value,
}

/// The value at a path of keys, or `None`.
fn at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    Some(current)
}

/// A scalar at a path, rendered as text, or `None`.
fn text_at(value: &Value, path: &[&str]) -> Option<String> {
    match at(value, path)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The items of a list at a path, or nothing.
fn list_at<'a>(value: &'a Value, path: &[&str]) -> Vec<&'a Value> {
    match at(value, path) {
        Some(Value::Array(items)) => items.iter().collect(),
        _ => Vec::new(),
    }
}

/// Every value stored under a key of that name, anywhere in the document.
fn find_key<'a>(value: &'a Value, key: &str, found: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            for (name, child) in map {
                if name == key {
                    found.push(child);
                }
                find_key(child, key, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                find_key(item, key, found);
            }
        }
        _ => {}
    }
}

/// Every string scalar in a document, in order: a hook's script is a string
/// in a list, and a check on the script's text should not depend on how the
/// list is serialised.
fn strings_of(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    fn walk(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::String(text) => found.push(text.clone()),
            Value::Object(map) => map.values().for_each(|child| walk(child, found)),
            Value::Array(items) => items.iter().for_each(|item| walk(item, found)),
            _ => {}
        }
    }
    walk(value, &mut found);
    found
}

impl Manifest {
    fn kind(&self) -> String {
        text_at(&self.value, &["kind"]).unwrap_or_default()
    }

    fn api_version(&self) -> String {
        text_at(&self.value, &["apiVersion"]).unwrap_or_default()
    }

    fn name(&self) -> String {
        text_at(&self.value, &["metadata", "name"]).unwrap_or_default()
    }

    fn namespace(&self) -> Option<String> {
        text_at(&self.value, &["metadata", "namespace"])
    }

    fn annotation(&self, key: &str) -> Option<String> {
        text_at(&self.value, &["metadata", "annotations", key])
    }

    /// `kind` and name together, for messages.
    fn describe(&self) -> String {
        format!("{} `{}` in {}", self.kind(), self.name(), self.path)
    }
}

/// Every YAML document under a directory, which must exist and hold at
/// least one. A missing directory is the premise failing, and it is named.
fn manifests_under(relative: &str) -> Vec<Manifest> {
    assert!(
        repository_root().join(relative).is_dir(),
        "{relative} does not exist. ADR 0036 places the GitOps configuration there; until it \
         lands this test has nothing to read and says so rather than passing over nothing."
    );
    let mut found = Vec::new();
    for extension in ["yaml", "yml"] {
        for path in files_with_extension(relative, extension) {
            // A vendored upstream install manifest is a byte-for-byte copy
            // whose sha256 its SOURCE.md records; its CRDs fold descriptions
            // over lines, which the subset reader refuses. Those files are
            // read line by line by `upstream_documents`, never parsed.
            if path.components().any(|c| c.as_os_str() == "upstream") {
                continue;
            }
            let display = path
                .strip_prefix(repository_root())
                .unwrap_or(&path)
                .display()
                .to_string();
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {display}: {error}"));
            for value in yaml::documents(&source, &display) {
                found.push(Manifest {
                    path: display.clone(),
                    value,
                });
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(
        !found.is_empty(),
        "{relative} exists and holds no YAML document; every test reading it would pass over \
         nothing"
    );
    found
}

/// The manifests of one kind under a directory.
fn of_kind(manifests: &[Manifest], kind: &str) -> Vec<Manifest> {
    manifests
        .iter()
        .filter(|manifest| manifest.kind() == kind)
        .cloned()
        .collect()
}

/// One document of a vendored upstream install manifest, read line by line.
#[derive(Debug, Clone)]
struct UpstreamDocument {
    path: String,
    kind: String,
    name: String,
    text: String,
}

/// Every document of every `bootstrap/<component>/upstream/*.yaml`, the
/// byte-for-byte copies of the controllers' own install manifests. A
/// column-zero `kind:` and the first `  name:` under `metadata:` identify a
/// document; that is all the tests need of them, and the provenance test
/// pins the bytes themselves.
fn upstream_documents() -> Vec<UpstreamDocument> {
    let mut documents = Vec::new();
    let mut files = 0usize;
    for path in files_with_extension(BOOTSTRAP, "yaml") {
        if !path.components().any(|c| c.as_os_str() == "upstream") {
            continue;
        }
        files += 1;
        let display = path
            .strip_prefix(repository_root())
            .unwrap_or(&path)
            .display()
            .to_string();
        let source = std::fs::read_to_string(&path).expect("readable");
        for chunk in source.split("\n---") {
            let kind = chunk
                .lines()
                .find_map(|line| line.strip_prefix("kind: "))
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            if kind.is_empty() {
                continue;
            }
            let mut in_metadata = false;
            let mut name = String::new();
            for line in chunk.lines() {
                if line.starts_with("metadata:") {
                    in_metadata = true;
                    continue;
                }
                if in_metadata && !line.starts_with("  ") && !line.trim().is_empty() {
                    in_metadata = false;
                }
                if in_metadata {
                    if let Some(value) = line.strip_prefix("  name: ") {
                        name = value.trim().trim_matches('"').to_string();
                        break;
                    }
                }
            }
            documents.push(UpstreamDocument {
                path: display.clone(),
                kind,
                name,
                text: chunk.to_string(),
            });
        }
    }
    assert!(
        files >= 2,
        "only {files} upstream install manifest(s) exist under {BOOTSTRAP}; Argo CD's and \
         Kargo's are at least two (ADR 0036 decision 2)"
    );
    assert!(
        documents.len() >= 50,
        "only {} documents were read out of the upstream manifests; the split has stopped \
         finding them",
        documents.len()
    );
    documents
}

/// The `image:` values a piece of manifest text names, quotes stripped.
fn images_in(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let entry = line.trim_start();
            let entry = entry.strip_prefix("- ").unwrap_or(entry);
            entry.strip_prefix("image: ")
        })
        .map(|value| value.trim().trim_matches('"').to_string())
        .collect()
}

/// The environments that have a directory under `envs/`, which must be
/// exactly the four.
fn environment_directories() -> Vec<String> {
    assert!(
        repository_root().join(ENVS).is_dir(),
        "{ENVS} does not exist; ADR 0036 decision 4 puts one directory per environment there"
    );
    let mut found: Vec<String> = std::fs::read_dir(repository_root().join(ENVS))
        .expect("readable")
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    found.sort();
    let mut expected: Vec<String> = ENVIRONMENTS.iter().map(|env| (*env).to_string()).collect();
    expected.sort();
    assert_eq!(
        found, expected,
        "{ENVS} holds {found:?}; ADR 0036 names exactly the four environments, so an extra one \
         is a chain link nothing promotes to and a missing one is a chain with a gap"
    );
    ENVIRONMENTS.iter().map(|env| (*env).to_string()).collect()
}

// --- the kustomize images transformer -----------------------------------------

/// One entry of a kustomize `images:` transformer, wherever it is declared —
/// `kustomization.yaml` or the `images.yaml` a Kargo promotion writes.
#[derive(Debug, Clone)]
struct ImageOverride {
    name: String,
    new_name: Option<String>,
    digest: Option<String>,
    new_tag: Option<String>,
    path: String,
}

fn image_overrides(manifests: &[Manifest]) -> Vec<ImageOverride> {
    let mut found = Vec::new();
    for manifest in manifests {
        // Only a top-level `images:` list is the transformer; a container's
        // `image:` is a scalar and a Pod spec has no `images` key.
        for entry in list_at(&manifest.value, &["images"]) {
            let Some(name) = text_at(entry, &["name"]) else {
                panic!(
                    "{}: an `images:` transformer entry has no `name`: {entry}",
                    manifest.path
                );
            };
            found.push(ImageOverride {
                name,
                new_name: text_at(entry, &["newName"]),
                digest: text_at(entry, &["digest"]),
                new_tag: text_at(entry, &["newTag"]),
                path: manifest.path.clone(),
            });
        }
    }
    found
}

/// An image reference split into repository and the reference after it —
/// `@sha256:…`, `:tag`, or nothing.
fn split_reference(image: &str) -> (String, String) {
    if let Some((repository, digest)) = image.split_once('@') {
        return (repository.to_string(), format!("@{digest}"));
    }
    // A registry port (`host:5000/repo`) is a colon before the last slash.
    let last_slash = image.rfind('/').map_or(0, |index| index + 1);
    match image[last_slash..].find(':') {
        Some(colon) => (
            image[..last_slash + colon].to_string(),
            image[last_slash + colon..].to_string(),
        ),
        None => (image.to_string(), String::new()),
    }
}

/// An image reference as kustomize would render it under the transformer.
fn resolve_image(image: &str, overrides: &[ImageOverride]) -> String {
    let (repository, reference) = split_reference(image);
    let matching: Vec<&ImageOverride> = overrides
        .iter()
        .filter(|entry| entry.name == image || entry.name == repository)
        .collect();
    match matching.as_slice() {
        [] => image.to_string(),
        [entry] => {
            let repository = entry.new_name.clone().unwrap_or(repository);
            let reference = match (&entry.digest, &entry.new_tag) {
                (Some(digest), _) => format!("@{digest}"),
                (None, Some(tag)) => format!(":{tag}"),
                (None, None) => reference,
            };
            format!("{repository}{reference}")
        }
        several => panic!(
            "`{image}` is overridden by {} `images:` entries ({:?}); kustomize applies them in an \
             order nobody reviewed",
            several.len(),
            several.iter().map(|entry| &entry.path).collect::<Vec<_>>()
        ),
    }
}

/// Whether a reference is `…@sha256:<64 hex>` and nothing else after the `@`.
fn is_digest_pinned(image: &str) -> bool {
    let Some((_, digest)) = image.split_once('@') else {
        return false;
    };
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// The binary an image repository path names, if its last component is one
/// of the workspace's.
fn trading_binary_of(image: &str) -> Option<&'static str> {
    let (repository, _) = split_reference(image);
    let last = repository.rsplit('/').next().unwrap_or(&repository);
    TRADING_BINARIES
        .iter()
        .find(|binary| **binary == last)
        .copied()
}

// --- what a kustomization renders --------------------------------------------------

/// A strategic-merge patch as these overlays use it: maps merge key by key
/// and anything else — a list, a scalar — is replaced. Enough for a patch
/// that fills a Warehouse's subscriptions or a Stage's vars.
fn merge_patch(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (key, value) in patch {
                match base.get_mut(key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge_patch(existing, value);
                    }
                    _ => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

/// A path with its `..` segments collapsed.
fn normalised_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// What a kustomization renders, as far as these overlays go: the documents
/// of every listed resource — a file, or a directory with a kustomization of
/// its own — with each `patches` entry applied to the document its target
/// names. Upstream install files are not rendered here; the tests that need
/// them read them line by line, and a patch aimed at one is not checked for
/// a match.
fn kustomize(directory: &str) -> Vec<Manifest> {
    let path = format!("{directory}/kustomization.yaml");
    let source = read(&path);
    let documents = yaml::documents(&source, &path);
    assert_eq!(
        documents.len(),
        1,
        "{path} holds {} documents; a kustomization is one",
        documents.len()
    );
    let kustomization = &documents[0];
    let mut rendered: Vec<Manifest> = Vec::new();
    let mut reaches_upstream = false;
    for resource in list_at(kustomization, &["resources"]) {
        let entry = resource.as_str().unwrap_or_default();
        let relative = normalised_path(&format!("{directory}/{entry}"));
        let target = repository_root().join(&relative);
        if target.is_dir() {
            rendered.extend(kustomize(&relative));
        } else if relative.contains("/upstream/") {
            reaches_upstream = true;
        } else {
            let source = std::fs::read_to_string(&target).unwrap_or_else(|error| {
                panic!("{path} lists {entry}, which cannot be read: {error}")
            });
            for value in yaml::documents(&source, &relative) {
                rendered.push(Manifest {
                    path: relative.clone(),
                    value,
                });
            }
        }
    }
    for patch in list_at(kustomization, &["patches"]) {
        let text = text_at(patch, &["patch"]).unwrap_or_else(|| {
            panic!("{path} has a patch with no inline `patch:`; a patch file is not read here")
        });
        let body = yaml::documents(&text, &format!("{path} (patch)"));
        assert_eq!(body.len(), 1, "{path} has a patch that is not one document");
        let body = &body[0];
        let kind = text_at(patch, &["target", "kind"]).or_else(|| text_at(body, &["kind"]));
        let name =
            text_at(patch, &["target", "name"]).or_else(|| text_at(body, &["metadata", "name"]));
        let matching: Vec<usize> = rendered
            .iter()
            .enumerate()
            .filter(|(_, manifest)| Some(manifest.kind()) == kind && Some(manifest.name()) == name)
            .map(|(index, _)| index)
            .collect();
        if body.get("$patch").and_then(Value::as_str) == Some("delete") {
            for index in matching.into_iter().rev() {
                rendered.remove(index);
            }
            continue;
        }
        assert!(
            !matching.is_empty() || reaches_upstream,
            "{path} patches {kind:?} `{name:?}`, which nothing it renders declares"
        );
        for index in matching {
            merge_patch(&mut rendered[index].value, body);
        }
    }
    rendered
}

// --- the catalogue, read the way infrastructure.rs reads it -------------------

/// A configuration with its comments removed.
fn without_comments(content: &str) -> String {
    content
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A line's whitespace collapsed to single spaces, so `terraform fmt`'s
/// alignment cannot change what a check reads.
fn collapsed(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The lines under `key`, deeper than it, up to the first line at or above
/// its indent.
fn block_under(text: &str, key: &str) -> String {
    let mut lines = text.lines();
    let Some(opening) = lines.find(|line| line.trim() == key) else {
        return String::new();
    };
    let indent = opening.len() - opening.trim_start().len();
    lines
        .take_while(|line| line.trim().is_empty() || line.len() - line.trim_start().len() > indent)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The catalogue's workload entries as `(name, body)`. The same walk
/// `infrastructure.rs` and `manifest_wiring.rs` make, with the same
/// premise: exactly the three workloads ADR 0010 records.
fn catalogue_workloads() -> Vec<(String, String)> {
    let text = without_comments(&read(CATALOGUE));
    let start = text
        .find("cloud_run_catalogue = {")
        .expect("catalogue.tf declares local.cloud_run_catalogue");
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in text[start..].lines().skip(1) {
        if line == "  }" {
            break;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 4 && line.trim_end().ends_with("= {") {
            let name = line.trim().trim_end_matches("= {").trim().to_string();
            current = Some((name, Vec::new()));
            continue;
        }
        if line == "    }" {
            if let Some((name, body)) = current.take() {
                entries.push((name, body.join("\n")));
            }
            continue;
        }
        if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
    }
    assert_eq!(
        entries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["api", "fastbrain", "deepbrain"],
        "the catalogue parsed to something other than the three workloads ADR 0010 records; \
         the entry shape has changed and every check reading it is reading the wrong thing"
    );
    entries
}

/// A scalar field at the top level of a catalogue entry.
fn catalogue_field(body: &str, field: &str) -> String {
    body.lines()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            let indent = line.len() - line.trim_start().len();
            (indent == 6 && key.trim() == field).then(|| value.trim().trim_matches('"').to_string())
        })
        .unwrap_or_else(|| panic!("the catalogue entry has no `{field}` field:\n{body}"))
}

/// The `QIP_` variable names a catalogue entry sets in its `env`, wherever
/// they sit — the fast brain's `env` is a `merge` of two maps.
fn catalogue_env_names(body: &str) -> BTreeSet<String> {
    body.lines()
        .filter_map(|line| {
            let (key, _) = line.split_once('=')?;
            let key = key.trim();
            (key.starts_with("QIP_")
                && key
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .then(|| key.to_string())
        })
        .collect()
}

/// The variables a catalogue entry sets only when a root variable is
/// non-null: the keys inside a `var.x == null ? {} : {` arm of its `env`
/// merge. A rendering for an environment whose tfvars leave that variable
/// null carries none of them, and that absence is the tfvars' decision.
fn catalogue_conditional_env_names(body: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut inside = false;
    for line in body.lines() {
        if line.contains("== null ? {} : {") {
            inside = true;
            continue;
        }
        if inside && line.trim() == "}," {
            inside = false;
            continue;
        }
        if inside {
            if let Some((key, _)) = line.split_once('=') {
                let key = key.trim();
                if key.starts_with("QIP_") {
                    names.insert(key.to_string());
                }
            }
        }
    }
    names
}

/// The secrets a catalogue entry mounts, as `(key, file_name, _FILE variable)`.
fn catalogue_secret_mounts(body: &str) -> Vec<(String, String, String)> {
    let block = block_under(body, "secret_mounts = {");
    assert!(
        !block.is_empty(),
        "the catalogue entry has no `secret_mounts` block; the entry shape has changed"
    );
    let mut mounts = Vec::new();
    let mut current: Option<(String, Option<String>, Option<String>)> = None;
    for line in block.lines() {
        let indent = line.len() - line.trim_start().len();
        if indent == 8 && line.trim_end().ends_with("= {") {
            current = Some((
                line.trim().trim_end_matches("= {").trim().to_string(),
                None,
                None,
            ));
            continue;
        }
        if indent == 8 && line.trim() == "}" {
            if let Some((key, file, variable)) = current.take() {
                mounts.push((
                    key.clone(),
                    file.unwrap_or_else(|| panic!("secret mount {key} names no file_name")),
                    variable
                        .unwrap_or_else(|| panic!("secret mount {key} names no env_file_variable")),
                ));
            }
            continue;
        }
        if let (Some(entry), Some((field, value))) = (current.as_mut(), line.split_once('=')) {
            let value = value.trim().trim_matches('"').to_string();
            match field.trim() {
                "file_name" => entry.1 = Some(value),
                "env_file_variable" => entry.2 = Some(value),
                _ => {}
            }
        }
    }
    assert!(
        !mounts.is_empty(),
        "no secret mount was read out of the catalogue entry's block:\n{block}"
    );
    mounts
}

/// The config files a catalogue entry mounts, as their `_PATH` variables.
fn catalogue_config_file_variables(body: &str) -> BTreeSet<String> {
    block_under(body, "config_files = {")
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "env_file_variable").then(|| value.trim().trim_matches('"').to_string())
        })
        .collect()
}

/// The value of a top-level tfvars key, quotes stripped.
fn tfvars_value(environment: &str, key: &str) -> Option<String> {
    let text = without_comments(&read(&format!(
        "infrastructure/environments/{environment}/terraform.tfvars"
    )));
    text.lines().find_map(|line| {
        let rest = line.strip_prefix(key)?;
        let rest = rest.trim_start().strip_prefix('=')?;
        Some(rest.trim().trim_matches('"').to_string())
    })
}

/// This repository's URL, from the one committed fact every workflow's
/// identity is already derived from: `github_repository` in each
/// environment's tfvars, which the workload-identity pool's attribute
/// condition pins. `.git/config` is cross-checked where it names a GitHub
/// origin, but a checkout can be cloned from anywhere and the tfvars cannot.
fn this_repository() -> String {
    let mut named: BTreeSet<String> = BTreeSet::new();
    for environment in ENVIRONMENTS {
        named.insert(
            tfvars_value(environment, "github_repository")
                .unwrap_or_else(|| panic!("{environment}'s tfvars name no github_repository")),
        );
    }
    assert_eq!(
        named.len(),
        1,
        "the environments' tfvars name different repositories: {named:?}"
    );
    let slug = named.into_iter().next().unwrap_or_default();
    let url = format!("https://github.com/{slug}");
    if let Ok(config) = std::fs::read_to_string(repository_root().join(".git/config")) {
        let origin = config
            .split("[remote \"origin\"]")
            .nth(1)
            .and_then(|section| {
                section
                    .lines()
                    .find_map(|line| line.trim().strip_prefix("url = ").map(str::trim))
            })
            .map(|origin| {
                origin
                    .trim_end_matches(".git")
                    .replace("git@github.com:", "https://github.com/")
            });
        if let Some(origin) = origin.filter(|origin| origin.starts_with("https://github.com/")) {
            assert_eq!(
                origin, url,
                "the checkout's origin and the tfvars' github_repository disagree; the \
                 Applications have to point at the one the identity pool admits"
            );
        }
    }
    url
}

/// A repository URL with the spellings git and Argo CD accept made equal.
fn normalised_repository(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .replace("git@github.com:", "https://github.com/")
        .to_lowercase()
}

// --- Argo CD ------------------------------------------------------------------------

/// The Argo CD Applications, keyed by the environment their source path
/// names. The premise that there is one per environment and no other is
/// asserted here, because every Application test is built on it.
fn applications_by_environment() -> BTreeMap<String, Manifest> {
    let manifests = manifests_under(ARGOCD);
    let applications = of_kind(&manifests, "Application");
    assert!(
        !applications.is_empty(),
        "{ARGOCD} declares no `kind: Application`; nothing would reconcile anything"
    );
    let mut by_environment: BTreeMap<String, Vec<Manifest>> = BTreeMap::new();
    for application in &applications {
        assert!(
            application.api_version().starts_with("argoproj.io/"),
            "{} is an Application of {}, not Argo CD's",
            application.describe(),
            application.api_version()
        );
        let path = text_at(&application.value, &["spec", "source", "path"])
            .unwrap_or_else(|| panic!("{} has no spec.source.path", application.describe()));
        let environment = path
            .trim_matches('/')
            .strip_prefix("infrastructure/gitops/envs/")
            .unwrap_or_else(|| {
                panic!(
                    "{} sources `{path}`, which is not a directory under {ENVS}",
                    application.describe()
                )
            })
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string();
        by_environment
            .entry(environment)
            .or_default()
            .push(application.clone());
    }
    let mut result = BTreeMap::new();
    for environment in ENVIRONMENTS {
        let found = by_environment.remove(environment).unwrap_or_default();
        assert_eq!(
            found.len(),
            1,
            "{environment} has {} Application(s) under {ARGOCD} ({:?}); ADR 0036 decision 7 is \
             exactly one per environment",
            found.len(),
            found.iter().map(Manifest::describe).collect::<Vec<_>>()
        );
        result.insert(environment.to_string(), found[0].clone());
    }
    assert!(
        by_environment.is_empty(),
        "Applications source environments that are not the four: {:?}",
        by_environment.keys().collect::<Vec<_>>()
    );
    result
}

#[test]
fn every_argo_cd_application_points_at_this_repository_and_at_one_environment_directory_that_exists()
 {
    // An Application sourcing another repository, or a path that is not
    // there, reconciles either somebody else's manifests or nothing — and
    // both show in the Argo CD UI as an Application, which reads as a
    // deployment. The repository is the one the identity pool admits; the
    // path is the environment's directory, and it must exist, because Argo
    // CD reports a missing path as a sync error the next person will "fix"
    // by pointing it somewhere.
    let repository = normalised_repository(&this_repository());
    let mut checked = 0usize;
    for (environment, application) in applications_by_environment() {
        let repo_url = text_at(&application.value, &["spec", "source", "repoURL"])
            .unwrap_or_else(|| panic!("{} has no spec.source.repoURL", application.describe()));
        assert_eq!(
            normalised_repository(&repo_url),
            repository,
            "{} sources `{repo_url}`, which is not this repository ({repository})",
            application.describe()
        );
        let path = text_at(&application.value, &["spec", "source", "path"]).unwrap_or_default();
        assert_eq!(
            path.trim_matches('/'),
            format!("{ENVS}/{environment}"),
            "{} sources `{path}` rather than the environment's own directory",
            application.describe()
        );
        assert!(
            repository_root().join(path.trim_matches('/')).is_dir(),
            "{} sources `{path}`, which does not exist",
            application.describe()
        );
        // A targetRevision naming a branch other than the default, or a tag,
        // is a reconciler reading a line nobody promoted to.
        let revision = text_at(&application.value, &["spec", "source", "targetRevision"])
            .unwrap_or_else(|| {
                panic!(
                    "{} pins no spec.source.targetRevision",
                    application.describe()
                )
            });
        assert!(
            revision == "HEAD" || revision == "main" || revision.starts_with("claude/"),
            "{} reconciles targetRevision `{revision}`, which is neither the default branch nor \
             HEAD; the promotion commits land on the default branch and nothing else",
            application.describe()
        );
        checked += 1;
    }
    assert_eq!(checked, ENVIRONMENTS.len());
}

#[test]
fn only_dev_syncs_automatically_and_it_prunes_and_heals() {
    // ADR 0036 decision 7. `dev` reconciles on its own with `prune` and
    // `selfHeal`, so a service edited in the console is put back and a
    // manifest removed is released — never destroyed, because every
    // RunService carries `deletion-policy: abandon`, which the parity test
    // pins. `test`, `stage` and `prod` sync when a person syncs them: an
    // automated block on any of them is a promotion nobody approved
    // reaching an environment on the next reconcile.
    let applications = applications_by_environment();
    let dev = &applications["dev"];
    let automated = at(&dev.value, &["spec", "syncPolicy", "automated"]).unwrap_or_else(|| {
        panic!(
            "{} has no spec.syncPolicy.automated; dev reconciles only when somebody syncs it",
            dev.describe()
        )
    });
    for flag in ["prune", "selfHeal"] {
        assert_eq!(
            at(automated, &[flag]),
            Some(&Value::Bool(true)),
            "{} automates without `{flag}: true`; drift in dev is then found by the next person \
             to look rather than by the next reconcile",
            dev.describe()
        );
    }
    let mut manual = 0usize;
    for environment in ["test", "stage", "prod"] {
        let application = &applications[environment];
        assert!(
            at(&application.value, &["spec", "syncPolicy", "automated"]).is_none(),
            "{} carries an automated sync policy; {environment} syncs when a person syncs it \
             (ADR 0036 decision 7)",
            application.describe()
        );
        manual += 1;
    }
    assert_eq!(manual, 3);
}

#[test]
fn the_app_project_admits_this_repository_and_the_controllers_namespaces_and_no_wildcard() {
    // Argo CD's `default` project admits any repository into any namespace.
    // An Application in it can be re-pointed at another repository by an
    // edit to one field, and the reconciler will apply whatever it finds
    // there. The project is the boundary: this repository, the namespaces
    // the controllers own, and nothing spelled `*`.
    let manifests = manifests_under(ARGOCD);
    let projects = of_kind(&manifests, "AppProject");
    assert!(
        !projects.is_empty(),
        "{ARGOCD} declares no `kind: AppProject`; every Application then lives in `default`, \
         which admits any repository into any namespace"
    );
    let repository = normalised_repository(&this_repository());
    // The namespaces a manifest under the environment directories declares
    // — where Config Connector resources go — plus Argo CD's own.
    let mut permitted: BTreeSet<String> = BTreeSet::new();
    permitted.insert("argocd".to_string());
    for environment in environment_directories() {
        for manifest in manifests_under(&format!("{ENVS}/{environment}")) {
            if let Some(namespace) = manifest.namespace() {
                permitted.insert(namespace);
            }
            // The kustomization's `namespace:` is where these manifests get
            // theirs, so it is the destination the project has to admit.
            if manifest.kind() == "Kustomization" {
                if let Some(namespace) = text_at(&manifest.value, &["namespace"]) {
                    permitted.insert(namespace);
                }
            }
        }
    }
    assert!(
        permitted.len() >= 2,
        "no manifest under {ENVS} declares a namespace, so the set of destinations a project \
         may name is just `argocd` and this test cannot tell a right destination from a wrong one"
    );

    let applications = applications_by_environment();
    let mut destinations_checked = 0usize;
    for project in &projects {
        let repos = list_at(&project.value, &["spec", "sourceRepos"]);
        assert!(
            !repos.is_empty(),
            "{} names no sourceRepos; Argo CD reads an absent list as none, and every \
             Application in it fails — or somebody adds `*`",
            project.describe()
        );
        for repo in repos {
            let repo = repo.as_str().unwrap_or_default();
            assert!(
                !repo.contains('*'),
                "{} admits `{repo}`; a wildcard source repository admits anyone's manifests",
                project.describe()
            );
            assert_eq!(
                normalised_repository(repo),
                repository,
                "{} admits `{repo}`, which is not this repository",
                project.describe()
            );
        }
        let destinations = list_at(&project.value, &["spec", "destinations"]);
        assert!(
            !destinations.is_empty(),
            "{} names no destinations",
            project.describe()
        );
        for destination in destinations {
            let namespace = text_at(destination, &["namespace"]).unwrap_or_else(|| {
                panic!("{} has a destination with no namespace", project.describe())
            });
            let server = text_at(destination, &["server"])
                .or_else(|| text_at(destination, &["name"]))
                .unwrap_or_else(|| {
                    panic!(
                        "{} has a destination naming neither a server nor a cluster",
                        project.describe()
                    )
                });
            assert!(
                !namespace.contains('*') && !server.contains('*'),
                "{} admits destination {namespace}@{server}; a wildcard admits the whole cluster",
                project.describe()
            );
            assert!(
                permitted.contains(&namespace),
                "{} admits namespace `{namespace}`, which neither Argo CD nor any manifest under \
                 {ENVS} uses; the permitted set is {permitted:?}",
                project.describe()
            );
            destinations_checked += 1;
        }
        // No cluster-scoped resource: a RunService is namespaced, and a
        // project admitting cluster resources admits a ClusterRoleBinding.
        assert!(
            list_at(&project.value, &["spec", "clusterResourceWhitelist"]).is_empty(),
            "{} whitelists cluster-scoped resources; nothing this platform syncs is one",
            project.describe()
        );
        // And no Pod-bearing kind but the proving hook's Job: the
        // project-level form of "no trading binary runs as a Pod".
        let whitelist = list_at(&project.value, &["spec", "namespaceResourceWhitelist"]);
        assert!(
            !whitelist.is_empty(),
            "{} whitelists every namespaced kind, so an Application in it may sync a Deployment",
            project.describe()
        );
        for entry in whitelist {
            let kind = text_at(entry, &["kind"]).unwrap_or_default();
            assert!(
                ![
                    "Deployment",
                    "StatefulSet",
                    "DaemonSet",
                    "Pod",
                    "ReplicaSet",
                    "CronJob"
                ]
                .contains(&kind.as_str()),
                "{} admits `{kind}`, a Pod-bearing kind; the cluster runs controllers and the \
                 hook's Job, and a manifest could then put a trading binary on it",
                project.describe()
            );
        }
    }
    assert!(destinations_checked >= 1);

    // Every Application lives in one of these projects, and its destination
    // is one the project admits.
    let project_names: BTreeSet<String> = projects.iter().map(Manifest::name).collect();
    for (environment, application) in &applications {
        let project = text_at(&application.value, &["spec", "project"]).unwrap_or_default();
        assert!(
            project_names.contains(&project),
            "{} is in project `{project}`, which {ARGOCD} does not declare (it declares \
             {project_names:?}); `default` admits everything",
            application.describe()
        );
        let namespace = text_at(&application.value, &["spec", "destination", "namespace"])
            .unwrap_or_else(|| panic!("{} names no destination namespace", application.describe()));
        assert!(
            permitted.contains(&namespace),
            "{}'s destination `{namespace}` is not a namespace any manifest for {environment} \
             declares",
            application.describe()
        );
    }
}

// --- Kargo ------------------------------------------------------------------------------

/// One environment's Kargo control plane, as its overlay renders the base:
/// the project, the promotion task, the warehouse pointed at the
/// environment's registry, and the four stages.
fn kargo_rendered(environment: &str) -> Vec<Manifest> {
    assert!(
        repository_root().join(KARGO).is_dir(),
        "{KARGO} does not exist; ADR 0036 decision 6 puts the chain there"
    );
    let overlay = format!("{KARGO}/overlays/{environment}");
    assert!(
        repository_root().join(&overlay).is_dir(),
        "{overlay} does not exist; every environment's control plane carries the chain"
    );
    let rendered = kustomize(&overlay);
    for manifest in &rendered {
        assert!(
            manifest.api_version().starts_with("kargo.akuity.io/")
                || manifest.api_version() == "v1"
                || manifest
                    .api_version()
                    .starts_with("rbac.authorization.k8s.io/"),
            "{} is a {} of {}; {KARGO} holds Kargo's resources and the namespace and bindings \
             they need, nothing else",
            manifest.describe(),
            manifest.kind(),
            manifest.api_version()
        );
    }
    assert!(
        rendered.len() >= 7,
        "{overlay} renders only {} documents; a project, its config, a task, a warehouse and \
         four stages are more",
        rendered.len()
    );
    rendered
}

/// The Kargo Stages, keyed by name; exactly the four environments.
fn stages_by_name(manifests: &[Manifest]) -> BTreeMap<String, Manifest> {
    let stages = of_kind(manifests, "Stage");
    let mut by_name = BTreeMap::new();
    for stage in stages {
        assert!(
            by_name.insert(stage.name(), stage.clone()).is_none(),
            "{} is declared twice",
            stage.describe()
        );
    }
    let names: Vec<&String> = by_name.keys().collect();
    let mut expected: Vec<String> = ENVIRONMENTS.iter().map(|env| (*env).to_string()).collect();
    expected.sort();
    assert_eq!(
        names,
        expected.iter().collect::<Vec<_>>(),
        "the chain declares Stages {names:?}; ADR 0036 decision 6 is exactly the four environments"
    );
    by_name
}

#[test]
fn the_warehouse_subscribes_to_every_catalogue_binary_and_never_selects_by_semver() {
    // The Warehouse is what turns a pushed image into freight. It has to
    // watch every binary the catalogue deploys — a binary it does not watch
    // is one whose new build nothing ever promotes — in the environment's
    // own registry, and it has to select by the newest build of a tag that
    // is a commit sha, never by a semantic version: the pipeline pushes no
    // version tag, so a SemVer constraint would match nothing for ever and
    // read as "no new freight". And freight that mixes commits — three
    // images from two shas because a matrix job was slow — is refused at
    // creation by a criterion naming every binary's tag.
    let catalogue: BTreeSet<String> = catalogue_workloads()
        .iter()
        .map(|(_, body)| catalogue_field(body, "binary"))
        .collect();
    let mut checked = 0usize;
    for environment in ENVIRONMENTS {
        let rendered = kargo_rendered(environment);
        let warehouses = of_kind(&rendered, "Warehouse");
        assert_eq!(
            warehouses.len(),
            1,
            "{environment}'s chain renders {} Warehouse(s); ADR 0036 decision 6 is one",
            warehouses.len()
        );
        let warehouse = &warehouses[0];
        let subscriptions = list_at(&warehouse.value, &["spec", "subscriptions"]);
        assert!(
            !subscriptions.is_empty(),
            "{} subscribes to nothing after {environment}'s overlay; the base's empty list was \
             never filled",
            warehouse.describe()
        );
        let mut subscribed: BTreeSet<String> = BTreeSet::new();
        for subscription in subscriptions {
            assert!(
                at(subscription, &["git"]).is_none() && at(subscription, &["chart"]).is_none(),
                "{} subscribes to a git repository or a chart; freight here is images and \
                 nothing else",
                warehouse.describe()
            );
            let image = at(subscription, &["image"]).unwrap_or_else(|| {
                panic!(
                    "{} has a subscription that is not an image",
                    warehouse.describe()
                )
            });
            let repo_url = text_at(image, &["repoURL"]).unwrap_or_else(|| {
                panic!(
                    "{} has an image subscription with no repoURL",
                    warehouse.describe()
                )
            });
            let (_, reference) = split_reference(&repo_url);
            assert!(
                reference.is_empty(),
                "{} subscribes to `{repo_url}`, which carries `{reference}`; a subscription \
                 names a repository and the freight names the digest",
                warehouse.describe()
            );
            assert!(
                repo_url.contains(&format!("/qip-{environment}/")),
                "{} subscribes to `{repo_url}`, which is not {environment}'s own registry \
                 qip-{environment}",
                warehouse.describe()
            );
            let binary = trading_binary_of(&repo_url).unwrap_or_else(|| {
                panic!(
                    "{} subscribes to `{repo_url}`, whose last path component is no binary this \
                     workspace builds",
                    warehouse.describe()
                )
            });
            subscribed.insert(binary.to_string());
            let strategy = text_at(image, &["imageSelectionStrategy"]).unwrap_or_else(|| {
                panic!(
                    "{} subscribes to `{repo_url}` with no imageSelectionStrategy; Kargo's default \
                     is SemVer, which matches no tag this pipeline pushes",
                    warehouse.describe()
                )
            });
            assert!(
                strategy == "NewestBuild" || strategy == "Digest",
                "{} selects `{repo_url}` by `{strategy}`; the tag is the commit sha and \
                 immutable, so only NewestBuild (the latest attested commit) or Digest names \
                 bytes rather than a version nothing pushes",
                warehouse.describe()
            );
            assert!(
                at(image, &["semverConstraint"]).is_none(),
                "{} constrains `{repo_url}` by semver; no version tag exists",
                warehouse.describe()
            );
            // Only a tag that is a commit sha: the immutable tag deploy.yml
            // pushes. Anything else in the repository — a tag somebody
            // pushed by hand — is not freight.
            let patterns: Vec<String> = list_at(image, &["allowTagsRegexes"])
                .iter()
                .filter_map(|pattern| pattern.as_str().map(str::to_string))
                .collect();
            assert!(
                patterns.iter().any(|pattern| pattern == "^[0-9a-f]{40}$"),
                "{} admits tags {patterns:?} for `{repo_url}`; only a full commit sha is one the \
                 pipeline pushed",
                warehouse.describe()
            );
        }
        assert_eq!(
            subscribed, catalogue,
            "{environment}'s Warehouse subscribes to {subscribed:?} and the catalogue deploys \
             {catalogue:?}; a binary in one and not the other is either never promoted or \
             promoted to nowhere"
        );
        let criteria = text_at(
            &warehouse.value,
            &["spec", "freightCreationCriteria", "expression"],
        )
        .unwrap_or_else(|| {
            panic!(
                "{} has no freightCreationCriteria; freight mixing commits is then \
                         created and promoted",
                warehouse.describe()
            )
        });
        for binary in &catalogue {
            assert!(
                criteria.contains(&format!("/{binary}')")) && criteria.contains(".Tag"),
                "{}'s freight criterion does not compare {binary}'s tag: {criteria}",
                warehouse.describe()
            );
        }
        checked += 1;
    }
    assert_eq!(checked, ENVIRONMENTS.len());
}

#[test]
fn the_kargo_stages_chain_dev_to_test_to_stage_to_prod_from_one_warehouse() {
    // Promotion order is the whole safety argument for `test` and `stage`:
    // freight reaches `stage` only after `test` verified it. A Stage taking
    // freight directly from the Warehouse skips every environment before it,
    // and a Stage naming the wrong predecessor promotes something a
    // different environment approved. The same chain on every control
    // plane, because each renders the one base.
    for environment in ENVIRONMENTS {
        let rendered = kargo_rendered(environment);
        let warehouse = of_kind(&rendered, "Warehouse")
            .first()
            .map(Manifest::name)
            .expect("one Warehouse; the subscription test says so");
        let stages = stages_by_name(&rendered);
        let mut previous: Option<&str> = None;
        for stage_name in ENVIRONMENTS {
            let stage = &stages[stage_name];
            let requested = list_at(&stage.value, &["spec", "requestedFreight"]);
            assert_eq!(
                requested.len(),
                1,
                "{} requests {} kinds of freight; the chain is one Warehouse",
                stage.describe(),
                requested.len()
            );
            let request = requested[0];
            assert_eq!(
                text_at(request, &["origin", "kind"]).as_deref(),
                Some("Warehouse"),
                "{} requests freight from something other than a Warehouse",
                stage.describe()
            );
            assert_eq!(
                text_at(request, &["origin", "name"]).as_deref(),
                Some(warehouse.as_str()),
                "{} requests freight from a Warehouse other than `{warehouse}`",
                stage.describe()
            );
            let direct = at(request, &["sources", "direct"]) == Some(&Value::Bool(true));
            let upstream: Vec<String> = list_at(request, &["sources", "stages"])
                .iter()
                .filter_map(|stage| stage.as_str().map(str::to_string))
                .collect();
            match previous {
                None => {
                    assert!(
                        direct && upstream.is_empty(),
                        "{} is the first link and must take freight directly from the Warehouse \
                         and from no Stage; it takes direct={direct}, stages={upstream:?}",
                        stage.describe()
                    );
                }
                Some(before) => {
                    assert!(
                        !direct,
                        "{} takes freight directly from the Warehouse, skipping every environment \
                         before it",
                        stage.describe()
                    );
                    assert_eq!(
                        upstream,
                        vec![before.to_string()],
                        "{} takes freight from {upstream:?}; the chain is {}",
                        stage.describe(),
                        ENVIRONMENTS.join(" → ")
                    );
                }
            }
            previous = Some(stage_name);
        }
    }
}

#[test]
fn prod_is_promoted_by_nobody_until_an_adr_says_otherwise() {
    // The third prod refusal, beside `deploy.yml`'s and `infra.yml`'s.
    // ADR 0036 decision 6: the prod Stage exists so the chain is whole and a
    // person can see what is eligible, and its promotion is refused by
    // policy until an ADR lifts it. A Kargo promotion to prod a person could
    // click is a fourth path around both existing refusals — and a policy
    // that enables auto-promotion for prod is that click made automatic.
    // Two halves, because either alone is one edit from gone: no policy
    // promotes prod on its own, and prod's promotion template does nothing
    // but refuse, in a step that names the record that would have to exist.
    //
    // Lifting this is an ADR, not an edit here.
    for environment in ENVIRONMENTS {
        let rendered = kargo_rendered(environment);
        let stages = stages_by_name(&rendered);

        // Wherever Kargo keeps promotion policies — `Project.spec` in one
        // release, `ProjectConfig.spec` in a later one — the shape is a
        // `promotionPolicies` list of `{stage, autoPromotionEnabled}`. Found
        // by key so a move between kinds does not silently hide it.
        let mut policies: Vec<&Value> = Vec::new();
        for manifest in &rendered {
            find_key(&manifest.value, "promotionPolicies", &mut policies);
        }
        assert!(
            !policies.is_empty(),
            "{environment}'s chain declares no `promotionPolicies`; dev's automatic promotion \
             has nowhere to be enabled and this test cannot see a policy at all"
        );
        let mut dev_automatic = false;
        let mut entries = 0usize;
        for list in policies {
            for policy in list.as_array().into_iter().flatten() {
                entries += 1;
                let stage = text_at(policy, &["stage"]).unwrap_or_default();
                let enabled = at(policy, &["autoPromotionEnabled"]) == Some(&Value::Bool(true));
                if stage == "dev" && enabled {
                    dev_automatic = true;
                }
                assert!(
                    !(stage == "prod" && enabled),
                    "a promotion policy enables automatic promotion to prod; ADR 0036 decision 6 \
                     refuses any promotion to prod until an ADR lifts it, and this one would \
                     need nobody"
                );
            }
        }
        assert!(entries >= 1, "the promotionPolicies lists are all empty");
        assert!(
            dev_automatic,
            "no policy enables automatic promotion to dev; ADR 0036 decision 6 promotes dev on \
             its own, and the premise that this test can see an enabled policy at all is what \
             makes its refusal of one for prod worth anything"
        );

        // The Stage's own template: its first step refuses, naming the ADR.
        // A promotion that ran a real step before refusing would have
        // written something first.
        let prod = &stages["prod"];
        let steps = list_at(&prod.value, &["spec", "promotionTemplate", "spec", "steps"]);
        let first = steps.first().unwrap_or_else(|| {
            panic!(
                "{} has no promotion steps; a Stage with no template is promoted by Kargo's \
                 default, which is a plain sync",
                prod.describe()
            )
        });
        assert_eq!(
            text_at(first, &["uses"]).as_deref(),
            Some("fail"),
            "{}'s first promotion step is not `fail`; a person who can click prod can promote \
             to it",
            prod.describe()
        );
        let message = text_at(first, &["config", "message"]).unwrap_or_default();
        assert!(
            message.contains("ADR") && message.contains("prod"),
            "{}'s refusal names no ADR; the person who clicked is not told which record has to \
             exist before it works: `{message}`",
            prod.describe()
        );
        // And the premise: the other stages do promote, so the refusal is a
        // refusal and not the shape every stage has.
        for stage_name in ["dev", "test", "stage"] {
            let stage = &stages[stage_name];
            let steps = list_at(
                &stage.value,
                &["spec", "promotionTemplate", "spec", "steps"],
            );
            assert!(
                steps
                    .first()
                    .is_some_and(|step| text_at(step, &["uses"]).as_deref() != Some("fail")),
                "{}'s first promotion step refuses; only prod's may",
                stage.describe()
            );
        }
    }
}

// --- the RunService manifests against the catalogue -------------------------------

/// The RunServices under one environment, each matched to the catalogue key
/// its name carries. Any RunService that is neither a catalogue workload
/// nor OpenObserve fails here: a fourth service is a workload with no
/// catalogue entry to be checked against.
fn run_services(environment: &str) -> Vec<(String, Option<String>, Manifest, Vec<Manifest>)> {
    let manifests = manifests_under(&format!("{ENVS}/{environment}"));
    let services: Vec<Manifest> = manifests
        .iter()
        .filter(|manifest| manifest.kind() == "RunService")
        .cloned()
        .collect();
    assert!(
        !services.is_empty(),
        "{ENVS}/{environment} declares no `kind: RunService`; nothing there deploys anything"
    );
    let catalogue = catalogue_workloads();
    let mut matched = Vec::new();
    for service in services {
        assert!(
            service
                .api_version()
                .starts_with("run.cnrm.cloud.google.com/"),
            "{} is not a Config Connector RunService",
            service.describe()
        );
        let name = service.name();
        let Some(key) = name.strip_prefix(&format!("qip-{environment}-")) else {
            panic!(
                "{} is not named qip-{environment}-<workload>, which is the name Terraform \
                 created the service under and the name Config Connector acquires it by",
                service.describe()
            );
        };
        if key == "openobserve" {
            matched.push((key.to_string(), None, service, manifests.clone()));
            continue;
        }
        let entry = catalogue
            .iter()
            .find(|(workload, _)| workload == key)
            .unwrap_or_else(|| {
                panic!(
                    "{} names workload `{key}`, which the catalogue does not declare; every \
                     service is a catalogue entry rendered, and one with no entry has nothing \
                     to be checked against",
                    service.describe()
                )
            });
        matched.push((
            key.to_string(),
            Some(entry.1.clone()),
            service,
            manifests.clone(),
        ));
    }
    matched
}

/// The container named after the workload in a RunService template.
fn workload_container<'a>(service: &'a Manifest, key: &str) -> &'a Value {
    let containers = list_at(&service.value, &["spec", "template", "containers"]);
    let named: Vec<&&Value> = containers
        .iter()
        .filter(|container| text_at(container, &["name"]).as_deref() == Some(key))
        .collect();
    assert_eq!(
        named.len(),
        1,
        "{} has {} container(s) named `{key}`; modules/cloudrun names the workload's container \
         after the catalogue entry and the post-sync hook selects it by that name",
        service.describe(),
        named.len()
    );
    named[0]
}

/// The environment entries of a container as `(name, value, has a secret source)`.
fn container_env(container: &Value) -> Vec<(String, Option<String>, bool)> {
    list_at(container, &["env"])
        .iter()
        .map(|entry| {
            let name = text_at(entry, &["name"])
                .unwrap_or_else(|| panic!("an env entry has no name: {entry}"));
            let secret =
                at(entry, &["valueSource"]).is_some() || at(entry, &["valueFrom"]).is_some();
            (name, text_at(entry, &["value"]), secret)
        })
        .collect()
}

#[test]
fn every_run_service_holds_the_invariants_its_catalogue_entry_and_the_cloud_run_module_held() {
    // ADR 0036 decision 4. `modules/cloudrun` held every property that had
    // to be true of every service in one place; a manifest per environment
    // per workload is twelve places, and the one that drifts is the one
    // nobody reads. So each manifest is read beside its catalogue entry and
    // the module's own text, and every property the module enforced is
    // re-asserted on the manifest:
    //
    //   internal ingress; the default admission policy; the workload's own
    //   identity; the image by digest, naming the entry's binary; every
    //   catalogue variable present, and every secret a mounted file under
    //   /var/run/secrets/qip with its _FILE variable carrying the path, and
    //   never an environment value; direct VPC egress; the instance bounds
    //   the entry justified; the probes on the entry's health path; the
    //   egress sidecar exactly where the entry has one; and the deletion
    //   policy that turns a prune into a release.
    //
    // A property the module held and this test does not re-assert is a
    // property lost in the move and reading as kept.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        module.contains(&format!(
            "secret_root = \"{}\"",
            SECRET_ROOT.trim_end_matches('/')
        )),
        "modules/cloudrun no longer names {SECRET_ROOT} as the secret root; this test's \
         premise about where a secret is mounted has to be re-read"
    );
    let overrides_seen = std::cell::Cell::new(0usize);
    let mut services_checked = 0usize;
    let mut secrets_checked = 0usize;
    let mut sidecars_seen = 0usize;
    for environment in environment_directories() {
        let project = tfvars_value(&environment, "project_id")
            .unwrap_or_else(|| panic!("{environment}'s tfvars name no project_id"));
        for (key, entry, service, siblings) in run_services(&environment) {
            let Some(entry) = entry else {
                // OpenObserve: ADR 0030's posture and ADR 0031's secret_env are
                // the two named exceptions, and it is not a catalogue entry.
                // It is checked by the secret test below for the one property
                // that still applies: it is released, never destroyed.
                assert_eq!(
                    service.annotation(DELETION_POLICY).as_deref(),
                    Some("abandon"),
                    "{} lacks `{DELETION_POLICY}: abandon`",
                    service.describe()
                );
                continue;
            };
            let describe = service.describe();
            let binary = catalogue_field(&entry, "binary");

            // Ingress. Every catalogue workload is internal today and the
            // trading class may never be anything else.
            let ingress = text_at(&service.value, &["spec", "ingress"]).unwrap_or_default();
            assert_eq!(
                ingress,
                "INGRESS_TRAFFIC_INTERNAL_ONLY",
                "{describe} has ingress `{ingress}`; catalogue.tf places every workload internal \
                 and the {} traffic class has no posture that publishes it",
                catalogue_field(&entry, "traffic_class")
            );

            // Admission.
            assert_eq!(
                at(
                    &service.value,
                    &["spec", "binaryAuthorization", "useDefault"]
                ),
                Some(&Value::Bool(true)),
                "{describe} does not opt into the default Binary Authorization policy"
            );
            assert!(
                at(
                    &service.value,
                    &["spec", "binaryAuthorization", "breakglassJustification"]
                )
                .is_none(),
                "{describe} carries a breakglass justification, which bypasses the admission \
                 policy for that revision"
            );

            // Identity: the workload's own, which Terraform keeps creating.
            let account = text_at(
                &service.value,
                &["spec", "template", "serviceAccountRef", "external"],
            )
            .unwrap_or_else(|| panic!("{describe} names no template.serviceAccountRef.external"));
            let expected_account =
                format!("qip-{key}-{environment}@{project}.iam.gserviceaccount.com");
            assert_eq!(
                account, expected_account,
                "{describe} runs as `{account}` rather than the workload's own account"
            );

            // Deletion policy.
            assert_eq!(
                service.annotation(DELETION_POLICY).as_deref(),
                Some("abandon"),
                "{describe} lacks `{DELETION_POLICY}: abandon`; a prune, or a manifest deleted, \
                 then destroys the service rather than releasing it, which is the \
                 deletion_protection the module held"
            );

            // The image: the entry's binary, by digest, after the kustomize
            // transformer the promotion writes.
            let container = workload_container(&service, &key);
            let declared = text_at(container, &["image"])
                .unwrap_or_else(|| panic!("{describe}'s `{key}` container has no image"));
            let overrides = image_overrides(&siblings);
            let image = resolve_image(&declared, &overrides);
            if image != declared {
                overrides_seen.set(overrides_seen.get() + 1);
            }
            if image.contains(TO_PIN) {
                // Unpinned by a written decision, or not at all.
                assert!(
                    awaiting_first_promotion(&environment),
                    "{describe} runs `{image}`, which still carries the {TO_PIN} marker, and \
                     {environment} is not in NEVER_PROMOTED_TO; either promote into it or \
                     write down why nothing has"
                );
            } else {
                assert!(
                    is_digest_pinned(&image),
                    "{describe} runs `{image}` (declared `{declared}`), which is not pinned by \
                     a sha256 digest; a tag is what a reader trusts and a digest is what the \
                     attestation names"
                );
            }
            assert_eq!(
                trading_binary_of(&image),
                Some(binary.as_str()),
                "{describe} runs `{image}`, which is not the catalogue entry's binary `{binary}`"
            );
            let (repository, _) = split_reference(&image);
            assert!(
                repository.contains(&format!("/qip-{environment}/")),
                "{describe} pulls `{image}` from a repository other than qip-{environment}, the \
                 one modules/registry creates and deploy.yml pushes to"
            );

            // Environment: every variable the entry sets, by exact name, and
            // every secret a file with its _FILE variable carrying the path.
            let env = container_env(container);
            let names: BTreeSet<String> = env.iter().map(|(name, _, _)| name.clone()).collect();
            let conditional = catalogue_conditional_env_names(&entry);
            for variable in catalogue_env_names(&entry) {
                if conditional.contains(&variable) {
                    continue;
                }
                assert!(
                    names.contains(&variable),
                    "{describe} does not set {variable}, which the catalogue entry sets; it sets \
                     {names:?}"
                );
            }
            for variable in catalogue_config_file_variables(&entry) {
                let value = env
                    .iter()
                    .find(|(name, _, _)| *name == variable)
                    .and_then(|(_, value, _)| value.clone())
                    .unwrap_or_else(|| panic!("{describe} does not set {variable}"));
                assert!(
                    value.starts_with("/etc/qip/"),
                    "{describe} sets {variable}={value}, which is not under /etc/qip where the \
                     committed configuration is mounted"
                );
            }
            for (name, _, secret) in &env {
                assert!(
                    !secret,
                    "{describe} takes {name} from a secret as an environment value; a secret in \
                     the environment is in /proc/<pid>/environ and every crash dump. Mount it \
                     (ADR 0031 permits this for a vendored image only)"
                );
            }
            let mounts: BTreeMap<String, String> = list_at(container, &["volumeMounts"])
                .iter()
                .map(|mount| {
                    (
                        text_at(mount, &["name"]).unwrap_or_default(),
                        text_at(mount, &["mountPath"]).unwrap_or_default(),
                    )
                })
                .collect();
            let volumes: BTreeMap<String, &Value> =
                list_at(&service.value, &["spec", "template", "volumes"])
                    .iter()
                    .map(|volume| (text_at(volume, &["name"]).unwrap_or_default(), *volume))
                    .collect();
            for (secret_key, file_name, variable) in catalogue_secret_mounts(&entry) {
                let expected_path = format!("{SECRET_ROOT}{secret_key}/{file_name}");
                let value = env
                    .iter()
                    .find(|(name, _, _)| *name == variable)
                    .and_then(|(_, value, _)| value.clone())
                    .unwrap_or_else(|| {
                        panic!("{describe} does not set {variable} for secret mount `{secret_key}`")
                    });
                assert_eq!(
                    value, expected_path,
                    "{describe} sets {variable}={value}; the file is mounted at {expected_path}"
                );
                assert_eq!(
                    mounts.get(&secret_key).map(String::as_str),
                    Some(format!("{SECRET_ROOT}{secret_key}").as_str()),
                    "{describe} does not mount volume `{secret_key}` at {SECRET_ROOT}{secret_key}; \
                     it mounts {mounts:?}"
                );
                let volume = volumes.get(&secret_key).unwrap_or_else(|| {
                    panic!("{describe} declares no volume named `{secret_key}`")
                });
                let secret = at(volume, &["secret"]).unwrap_or_else(|| {
                    panic!("{describe}'s volume `{secret_key}` is not a Secret Manager secret")
                });
                let items = list_at(secret, &["items"]);
                assert!(
                    items
                        .iter()
                        .any(|item| text_at(item, &["path"]).as_deref() == Some(&file_name)),
                    "{describe}'s volume `{secret_key}` projects no item at `{file_name}`"
                );
                for item in &items {
                    assert_eq!(
                        text_at(item, &["mode"]).as_deref(),
                        Some("256"),
                        "{describe}'s volume `{secret_key}` projects `{file_name}` with mode {:?} \
                         rather than 256 (0400): readable by the process and nobody else",
                        text_at(item, &["mode"])
                    );
                }
                secrets_checked += 1;
            }

            // Direct VPC egress, all traffic, through the zone's subnet with
            // the zone's tag, so the zone's firewall rules are the ones that
            // apply.
            assert_eq!(
                text_at(&service.value, &["spec", "template", "vpcAccess", "egress"]).as_deref(),
                Some("ALL_TRAFFIC"),
                "{describe} does not send all traffic through the VPC"
            );
            let interfaces = list_at(
                &service.value,
                &["spec", "template", "vpcAccess", "networkInterfaces"],
            );
            assert_eq!(
                interfaces.len(),
                1,
                "{describe} declares {} network interfaces",
                interfaces.len()
            );
            assert!(
                at(interfaces[0], &["subnetworkRef"]).is_some()
                    || at(interfaces[0], &["subnetwork"]).is_some(),
                "{describe}'s network interface names no subnetwork"
            );
            let tags = list_at(interfaces[0], &["tags"]);
            assert!(
                !tags.is_empty(),
                "{describe}'s network interface carries no network tag; every rule in \
                 modules/trust-zones targets the zone's tag"
            );

            // Instance bounds, from the entry.
            let scaling = at(&service.value, &["spec", "template", "scaling"])
                .unwrap_or_else(|| panic!("{describe} declares no scaling"));
            for (field, manifest_key) in [
                ("min_instances", "minInstanceCount"),
                ("max_instances", "maxInstanceCount"),
            ] {
                let expected = catalogue_field(&entry, field);
                let actual = text_at(scaling, &[manifest_key]).unwrap_or_default();
                assert_eq!(
                    actual, expected,
                    "{describe} sets {manifest_key}={actual}; the catalogue entry says {expected}"
                );
            }
            if catalogue_field(&entry, "min_instances") != "0" {
                assert!(
                    catalogue_field(&entry, "always_on_justification").len() > 40,
                    "{key} keeps an instance warm and the catalogue entry does not say why"
                );
            }

            // Probes on the entry's health path.
            let health_path = catalogue_field(&entry, "health_path");
            for probe in ["startupProbe", "livenessProbe"] {
                let path = text_at(container, &[probe, "httpGet", "path"]).unwrap_or_else(|| {
                    panic!("{describe}'s `{key}` container has no HTTP {probe}")
                });
                assert_eq!(
                    path, health_path,
                    "{describe}'s {probe} is on `{path}`; the catalogue entry serves health at \
                     `{health_path}`"
                );
            }

            // Limits, on every container: a memory limit without a CPU limit
            // lets a busy instance starve its neighbours; a CPU limit without
            // a memory limit lets a leak take the instance down. The
            // workload's are the catalogue entry's.
            for other in list_at(&service.value, &["spec", "template", "containers"]) {
                let container_name = text_at(other, &["name"]).unwrap_or_default();
                for limit in ["cpu", "memory"] {
                    assert!(
                        text_at(other, &["resources", "limits", limit]).is_some(),
                        "{describe}'s `{container_name}` container declares no {limit} limit"
                    );
                }
            }
            for (field, limit) in [("cpu", "cpu"), ("memory", "memory")] {
                let expected = catalogue_field(&entry, field);
                let actual =
                    text_at(container, &["resources", "limits", limit]).unwrap_or_default();
                assert_eq!(
                    actual, expected,
                    "{describe}'s `{key}` container has {limit} limit {actual}; the catalogue \
                     entry says {expected}"
                );
            }

            // The committed configuration: a read-only mount of the workload's
            // own bucket at /etc/qip, which is where every _PATH variable
            // above was just shown to point.
            assert_eq!(
                mounts.get("config-files").map(String::as_str),
                Some("/etc/qip"),
                "{describe} does not mount `config-files` at /etc/qip; it mounts {mounts:?}"
            );
            let config_volume = volumes
                .get("config-files")
                .unwrap_or_else(|| panic!("{describe} declares no `config-files` volume"));
            assert_eq!(
                at(config_volume, &["gcs", "readOnly"]),
                Some(&Value::Bool(true)),
                "{describe}'s config-files volume is not a read-only GCS mount"
            );

            // The zone: the subnet and the tag both name the entry's trust
            // zone, so the zone's firewall rules are the ones that apply.
            let zone = catalogue_field(&entry, "trust_zone");
            let subnetwork = text_at(interfaces[0], &["subnetworkRef", "external"])
                .or_else(|| text_at(interfaces[0], &["subnetworkRef", "name"]))
                .or_else(|| text_at(interfaces[0], &["subnetwork"]))
                .unwrap_or_default();
            assert!(
                subnetwork.contains(&zone),
                "{describe} egresses through subnetwork `{subnetwork}`, which does not name the \
                 entry's `{zone}` zone"
            );
            assert!(
                tags.iter()
                    .any(|tag| tag.as_str().is_some_and(|tag| tag.contains(&zone))),
                "{describe} carries tags {tags:?}, none naming the `{zone}` zone whose rules \
                 target it"
            );

            // The metrics collector, exactly where the entry asks for one and
            // the environment names a digest, and nowhere else: a collector
            // declared under no digest is a sidecar nobody vendored.
            let collectors = list_at(&service.value, &["spec", "template", "containers"])
                .iter()
                .filter(|other| {
                    text_at(other, &["name"]).as_deref() == Some("qip-metrics-collector")
                })
                .count();
            let wants_collector = catalogue_field(&entry, "metrics_collector") == "true"
                && tfvars_value(&environment, "metrics_collector_image_digest").is_some();
            assert_eq!(
                collectors,
                usize::from(wants_collector),
                "{describe} carries {collectors} metrics collector(s); the catalogue entry has \
                 metrics_collector = {} and {environment}'s tfvars {} a digest",
                catalogue_field(&entry, "metrics_collector"),
                if tfvars_value(&environment, "metrics_collector_image_digest").is_some() {
                    "name"
                } else {
                    "do not name"
                }
            );

            // The egress sidecar exactly where the entry has one. The fast
            // brain has none, deliberately (ADR 0008): port 9102 on the proxy
            // is a route to a language model.
            let sidecars: Vec<String> =
                list_at(&service.value, &["spec", "template", "containers"])
                    .iter()
                    .filter_map(|container| text_at(container, &["name"]))
                    .filter(|name| name == "qip-egress")
                    .collect();
            let wants_proxy = catalogue_field(&entry, "egress_proxy") == "true";
            assert_eq!(
                sidecars.len(),
                usize::from(wants_proxy),
                "{describe} carries {} egress sidecar(s) and the catalogue entry says \
                 egress_proxy = {wants_proxy}",
                sidecars.len()
            );
            if wants_proxy {
                sidecars_seen += 1;
            }
            services_checked += 1;
        }
    }
    // The premises: twelve services, every secret mount exercised, the
    // sidecar arm exercised, and the transformer arm exercised — a
    // resolution rule that stops resolving anything makes the digest check
    // read the placeholder instead of the promotion.
    assert_eq!(
        services_checked, 12,
        "{services_checked} services were checked; twelve exist"
    );
    assert!(
        secrets_checked >= 8 * 4,
        "only {secrets_checked} secret mounts were checked"
    );
    assert_eq!(
        sidecars_seen,
        2 * 4,
        "{sidecars_seen} sidecars were seen; api and deepbrain carry one in each environment"
    );
    assert!(
        overrides_seen.get() >= 1,
        "no image was resolved through an `images:` transformer; the promotion writes the \
         digest there, and a check that never reads it is checking a placeholder"
    );
}

#[test]
fn every_run_service_takes_the_paper_trading_ceiling_as_a_literal_and_no_manifest_names_a_live_rung()
 {
    // The catalogue takes the ceiling from `var.autonomy_ceiling`, whose
    // validation refuses the three live rungs at plan time. A manifest has
    // no plan: it is a value somebody edits and a reconciler applies. So the
    // literal in every RunService is `paper_trading`, and nothing under
    // infrastructure/gitops names a live rung anywhere — matched as the
    // delimited token, because `autonomous_live` is a substring of
    // `limited_autonomous_live` and a substring match has already passed a
    // mutation that deleted the value it existed to protect.
    let mut ceilings = 0usize;
    for environment in environment_directories() {
        let tfvars = tfvars_value(&environment, "autonomy_ceiling").unwrap_or_default();
        assert_eq!(
            tfvars, "paper_trading",
            "{environment}'s tfvars carry ceiling `{tfvars}`"
        );
        for (key, entry, service, _) in run_services(&environment) {
            if entry.is_none() {
                continue;
            }
            let container = workload_container(&service, &key);
            let value = container_env(container)
                .into_iter()
                .find(|(name, _, _)| name == "QIP_AUTONOMY_CEILING")
                .and_then(|(_, value, _)| value)
                .unwrap_or_else(|| panic!("{} sets no QIP_AUTONOMY_CEILING", service.describe()));
            assert_eq!(
                value,
                "paper_trading",
                "{} sets QIP_AUTONOMY_CEILING={value}; the only value a manifest may carry is \
                 paper_trading (ADR 0036 decision 11)",
                service.describe()
            );
            ceilings += 1;
        }
    }
    assert_eq!(ceilings, 12, "{ceilings} ceilings were checked");

    let live = [
        "supervised_live",
        "limited_autonomous_live",
        "autonomous_live",
    ];
    let mut scanned = 0usize;
    for extension in ["yaml", "yml"] {
        for path in files_with_extension(GITOPS, extension) {
            // Comment lines are dropped: a manifest explaining that it never
            // names a live rung has to be allowed to say so.
            let content: String = std::fs::read_to_string(&path)
                .expect("readable")
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n");
            scanned += 1;
            for token in content.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                assert!(
                    !live.contains(&token),
                    "{} names the live rung `{token}`",
                    path.display()
                );
            }
        }
    }
    assert!(
        scanned >= 12,
        "only {scanned} files were scanned under {GITOPS}"
    );
}

// --- the controllers' exposure and images ----------------------------------------------

#[test]
fn no_controller_manifest_publishes_a_service_and_the_argo_server_is_cluster_ip() {
    // The control-plane cluster has a private endpoint and no public one.
    // A `Service` of type LoadBalancer on it is a public address on the one
    // identity that can change what Config Connector applies to every Cloud
    // Run service in the environment; an Ingress is the same with a
    // hostname. The Argo server is ClusterIP, reached through the private
    // endpoint, and nothing else is reached at all. The upstream installs
    // are read line by line — they are the bytes the projects published —
    // and every parsed manifest and every overlay patch is read too, because
    // a patch is where a Service's type would be changed.
    let mut services = 0usize;
    let mut argo_server = false;
    for document in upstream_documents() {
        match document.kind.as_str() {
            "Service" => {
                services += 1;
                let kind = document
                    .text
                    .lines()
                    .find_map(|line| line.strip_prefix("  type: "))
                    .map(str::trim)
                    .unwrap_or("ClusterIP");
                assert_eq!(
                    kind, "ClusterIP",
                    "{} `{}` in {} is a Service of type {kind}; only ClusterIP is reachable from \
                     nowhere but the private endpoint",
                    document.kind, document.name, document.path
                );
                assert!(
                    !document.text.contains("externalIPs")
                        && !document.text.contains("loadBalancerIP"),
                    "{} `{}` in {} names an external address",
                    document.kind,
                    document.name,
                    document.path
                );
                if document.name == "argocd-server" {
                    argo_server = true;
                }
            }
            "Ingress" | "Gateway" | "HTTPRoute" | "ManagedCertificate" | "FrontendConfig" => {
                panic!(
                    "{} `{}` in {} publishes a controller outside the cluster",
                    document.kind, document.name, document.path
                );
            }
            _ => {}
        }
    }
    assert!(
        services >= 5,
        "only {services} Service(s) were found in the upstream installs; Argo CD's alone \
         declares several"
    );
    assert!(
        argo_server,
        "no Service named argocd-server in the upstream installs; the Argo server is \
         unreachable or reached through something this test cannot see"
    );
    for manifest in manifests_under(GITOPS) {
        match manifest.kind().as_str() {
            "Service" => {
                let kind = text_at(&manifest.value, &["spec", "type"])
                    .unwrap_or_else(|| "ClusterIP".to_string());
                assert_eq!(
                    kind,
                    "ClusterIP",
                    "{} is a Service of type {kind}",
                    manifest.describe()
                );
            }
            "Ingress" | "Gateway" | "HTTPRoute" | "ManagedCertificate" | "FrontendConfig" => {
                panic!(
                    "{} publishes a controller outside the cluster",
                    manifest.describe()
                );
            }
            "Kustomization" => {
                // A patch is a string; what it would do is read as text.
                let patches = strings_of(&manifest.value).join("\n");
                for forbidden in ["LoadBalancer", "NodePort", "kind: Ingress", "externalIPs"] {
                    assert!(
                        !patches.contains(forbidden),
                        "{} patches something to `{forbidden}`; a controller is then published \
                         outside the cluster",
                        manifest.describe()
                    );
                }
            }
            _ => {}
        }
    }
}

/// The vendored-images list as `(source digest, destination path)`.
fn vendored_images() -> Vec<(String, String)> {
    let list = read(VENDORED);
    let entries: Vec<(String, String)> = list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(
                fields.len(),
                3,
                "{VENDORED} line is not `<source@digest> <dest> <tag>`: {line}"
            );
            let (_, digest) = fields[0]
                .split_once('@')
                .unwrap_or_else(|| panic!("{VENDORED}: `{}` is not pinned by digest", fields[0]));
            (digest.to_string(), fields[1].to_string())
        })
        .collect();
    assert!(
        entries.len() >= 2,
        "{VENDORED} carries {entries:?}; the proxy and OpenObserve alone are two"
    );
    entries
}

/// Whether an image reference is one the platform's attestor could have
/// signed: a catalogue binary the pipeline built, or a mirror of a line in
/// the vendored list at that line's digest. Returns which, or the refusal.
fn admissible(image: &str, vendored: &[(String, String)]) -> Result<&'static str, String> {
    if !is_digest_pinned(image) {
        return Err(format!("`{image}` is not pinned by a sha256 digest"));
    }
    let (repository, reference) = split_reference(image);
    let digest = reference.trim_start_matches('@').to_string();
    for upstream in [
        "docker.io/",
        "quay.io/",
        "ghcr.io/",
        "gcr.io/",
        "registry.k8s.io/",
        "public.ecr.aws/",
    ] {
        if repository.starts_with(upstream) {
            return Err(format!(
                "`{image}` is pulled from an upstream registry; the admission policy admits only \
                 what the platform's attestor signed, which is the mirrored copy"
            ));
        }
    }
    if trading_binary_of(image).is_some() {
        return Ok("attested");
    }
    let mirrored = vendored.iter().any(|(source_digest, destination)| {
        *source_digest == digest && repository.ends_with(&format!("/{destination}"))
    });
    if mirrored {
        Ok("mirrored")
    } else {
        Err(format!(
            "`{image}`: {VENDORED} has no line whose source digest is {digest} and whose \
             destination is the tail of `{repository}`; nothing mirrored, scanned or attested it"
        ))
    }
}

/// The kinds whose spec carries containers, and so images that run.
const RUNS_AN_IMAGE: [&str; 8] = [
    "RunService",
    "Job",
    "CronJob",
    "Deployment",
    "StatefulSet",
    "DaemonSet",
    "ReplicaSet",
    "Pod",
];

#[test]
fn every_image_under_gitops_is_pinned_by_digest_and_is_either_attested_by_the_pipeline_or_vendored()
{
    // Binary Authorization keeps one rule and no exemptions (ADR 0036
    // decision 2), so every image the control plane runs has to be one the
    // platform's attestor signed: a catalogue binary the pipeline built, or
    // a third-party image vendor.yml mirrored from a line in
    // vendored-images.txt. An image named by tag is admitted by nothing; an
    // image from an upstream registry is refused at admission and the
    // controller never starts — which reads, in the UI nobody can reach, as
    // a cluster doing nothing.
    //
    // Two shapes. The manifests this suite parses name a logical image and
    // a kustomize `images:` transformer in the same directory turns it into
    // a registry path and a digest — the file a promotion edits. The
    // upstream installs name their images by tag, and each environment's
    // overlay moves every one of them to the environment's registry at the
    // digest the vendored list reviewed; a document the base deletes is not
    // deployed and needs no pin.
    let vendored = vendored_images();
    let mut attested = 0usize;
    let mut mirrored = 0usize;
    let mut awaiting = 0usize;
    let mut upstream_moved = 0usize;

    // The parsed manifests, each resolved through its own directory's
    // transformer.
    let manifests = manifests_under(GITOPS);
    let parent = |path: &str| {
        path.rsplit_once('/')
            .map(|(dir, _)| dir.to_string())
            .unwrap_or_default()
    };
    let mut overrides_by_dir: BTreeMap<String, Vec<ImageOverride>> = BTreeMap::new();
    for manifest in &manifests {
        overrides_by_dir
            .entry(parent(&manifest.path))
            .or_default()
            .extend(image_overrides(std::slice::from_ref(manifest)));
    }
    let mut images: Vec<(String, String, String)> = Vec::new();
    for manifest in &manifests {
        if !RUNS_AN_IMAGE.contains(&manifest.kind().as_str()) {
            continue;
        }
        let mut found = Vec::new();
        find_key(&manifest.value, "image", &mut found);
        for image in found {
            if let Some(image) = image.as_str() {
                images.push((
                    manifest.describe(),
                    parent(&manifest.path),
                    image.to_string(),
                ));
            }
        }
    }
    assert!(
        images.len() >= 12 + 4,
        "only {} images were found in the parsed manifests; twelve RunServices and four hooks \
         are more",
        images.len()
    );
    for (place, dir, declared) in &images {
        let image = resolve_image(
            declared,
            overrides_by_dir.get(dir).map_or(&[][..], Vec::as_slice),
        );
        if image.contains(TO_PIN) {
            let environment = dir.rsplit('/').next().unwrap_or_default();
            assert!(
                dir.starts_with(ENVS) && awaiting_first_promotion(environment),
                "{place} runs `{image}`, which still carries the {TO_PIN} marker, and \
                 {environment} is not an environment NEVER_PROMOTED_TO argues for"
            );
            assert!(
                trading_binary_of(&image).is_some(),
                "{place} runs `{image}`; only a catalogue binary awaits its first promotion — \
                 a vendored image is pinned by its line whether or not anything promoted"
            );
            awaiting += 1;
            continue;
        }
        match admissible(&image, &vendored) {
            Ok("attested") => attested += 1,
            Ok(_) => mirrored += 1,
            Err(why) => panic!("{place} (declared `{declared}`): {why}"),
        }
    }

    // The upstream installs, through each environment's overlay.
    let mut components = 0usize;
    for entry in std::fs::read_dir(repository_root().join(BOOTSTRAP))
        .expect("readable")
        .flatten()
    {
        if !entry.path().is_dir() {
            continue;
        }
        let component = entry.file_name().to_string_lossy().to_string();
        let component_dir = format!("{BOOTSTRAP}/{component}");
        let upstream: Vec<UpstreamDocument> = upstream_documents()
            .into_iter()
            .filter(|document| document.path.starts_with(&format!("{component_dir}/")))
            .collect();
        if upstream.is_empty() {
            continue;
        }
        components += 1;
        // What the base deletes is not deployed.
        let base = manifests_under(&format!("{component_dir}/base"));
        let mut deleted: BTreeSet<(String, String)> = BTreeSet::new();
        for kustomization in of_kind(&base, "Kustomization") {
            for patch in list_at(&kustomization.value, &["patches"]) {
                let text = text_at(patch, &["patch"]).unwrap_or_default();
                if text.contains("$patch: delete") {
                    deleted.insert((
                        text_at(patch, &["target", "kind"]).unwrap_or_default(),
                        text_at(patch, &["target", "name"]).unwrap_or_default(),
                    ));
                }
            }
        }
        let mut upstream_images: BTreeSet<String> = BTreeSet::new();
        for document in &upstream {
            if deleted.contains(&(document.kind.clone(), document.name.clone())) {
                continue;
            }
            if RUNS_AN_IMAGE.contains(&document.kind.as_str()) {
                upstream_images.extend(images_in(&document.text));
            }
        }
        assert!(
            !upstream_images.is_empty(),
            "{component_dir}'s upstream install runs no image; the split is reading nothing"
        );
        for environment in ENVIRONMENTS {
            let overlay = format!("{component_dir}/overlays/{environment}");
            assert!(
                repository_root().join(&overlay).is_dir(),
                "{overlay} does not exist; {environment}'s control plane then pulls {component} \
                 from its upstream registry, which the admission policy refuses"
            );
            let overrides = image_overrides(&manifests_under(&overlay));
            for image in &upstream_images {
                let moved = resolve_image(image, &overrides);
                assert_ne!(
                    &moved, image,
                    "{overlay} has no `images:` entry for `{image}`; the Pod pulls the upstream \
                     tag, which nothing attested"
                );
                assert!(
                    moved.contains(&format!("/qip-{environment}/")),
                    "{overlay} moves `{image}` to `{moved}`, which is not {environment}'s own \
                     registry"
                );
                match admissible(&moved, &vendored) {
                    Ok(_) => upstream_moved += 1,
                    Err(why) => panic!("{overlay} moves `{image}` to {why}"),
                }
            }
            // And nothing is pinned that nothing runs: a stale entry is a
            // digest a reviewer reads as deployed.
            for entry in &overrides {
                assert!(
                    upstream_images
                        .iter()
                        .any(|image| split_reference(image).0 == entry.name || *image == entry.name),
                    "{overlay} pins `{}`, which no document the base renders runs",
                    entry.name
                );
                assert!(
                    entry.new_tag.is_none(),
                    "{overlay} moves `{}` to a tag; the admission policy names bytes",
                    entry.name
                );
            }
        }
    }
    assert!(
        components >= 3,
        "only {components} bootstrap component(s) carry an upstream install; Argo CD, Kargo \
         and cert-manager are three"
    );
    assert!(
        upstream_moved >= 6 * 4,
        "only {upstream_moved} upstream images were moved across the overlays; six images in \
         four environments are more"
    );
    assert!(
        attested >= 3,
        "only {attested} images were catalogue binaries at a digest; dev's three at least"
    );
    assert!(
        awaiting == 3 * NEVER_PROMOTED_TO.len(),
        "{awaiting} images await a first promotion; three binaries in each of the {} \
         environments NEVER_PROMOTED_TO argues for is the expected count",
        NEVER_PROMOTED_TO.len()
    );
    assert!(
        mirrored >= 3 * 4,
        "only {mirrored} parsed images were matched to a vendored line; the sidecar, \
         OpenObserve and the hook in four environments are more"
    );
}

#[test]
fn every_environment_awaiting_its_first_promotion_is_still_unpinned_and_dev_is_not() {
    // An allowlist that outlives its reason is worse than none: it silently
    // excuses whatever later takes the same name. Each entry has to argue
    // its case, has to still be true — the environment still carries the
    // marker — and dev, the environment the pipeline builds for and the
    // chain's first link, may never be one.
    assert!(
        !awaiting_first_promotion("dev"),
        "dev is listed as awaiting its first promotion; it is the environment the pipeline \
         builds for and the chain's first link, and an unpinned dev is a dev that serves \
         whatever was there before"
    );
    for (environment, reason) in NEVER_PROMOTED_TO {
        assert!(
            reason.len() > 120,
            "the entry for {environment} does not argue its case; an exception without a \
             reason is an exception nobody can review"
        );
        assert!(
            ENVIRONMENTS.contains(environment),
            "{environment} is not an environment"
        );
        let overrides = image_overrides(&manifests_under(&format!("{ENVS}/{environment}")));
        let unpinned: Vec<&ImageOverride> = overrides
            .iter()
            .filter(|entry| {
                entry
                    .digest
                    .as_deref()
                    .is_some_and(|digest| digest.contains(TO_PIN))
            })
            .collect();
        assert!(
            !unpinned.is_empty(),
            "{environment} no longer carries {TO_PIN}; a promotion has pinned it, so its entry \
             in NEVER_PROMOTED_TO excuses nothing and should be deleted"
        );
        for entry in unpinned {
            assert!(
                trading_binary_of(&entry.name).is_some(),
                "{environment} leaves `{}` unpinned; only a catalogue binary awaits a \
                 promotion, and a vendored image is pinned by its line",
                entry.name
            );
        }
    }
    // The other direction: dev is pinned everywhere.
    let dev = image_overrides(&manifests_under(&format!("{ENVS}/dev")));
    assert!(
        dev.len() >= 3,
        "dev's transformer names {} images; the three binaries at least",
        dev.len()
    );
    for entry in &dev {
        assert!(
            entry.digest.as_deref().is_some_and(is_digest_pinned_bare),
            "dev's transformer leaves `{}` at `{:?}`, not a sha256 digest",
            entry.name,
            entry.digest
        );
    }
}

/// Whether a bare `sha256:<64 hex>` digest is well-formed.
fn is_digest_pinned_bare(digest: &str) -> bool {
    is_digest_pinned(&format!("x@{digest}"))
}

#[test]
fn every_upstream_install_manifest_is_the_bytes_its_provenance_note_records() {
    // The controllers' install manifests are copied from upstream byte for
    // byte, and SOURCE.md beside each records the URL, the version and the
    // sha256 of what was copied. That note is the review: the next release
    // is diffed against these bytes. A file edited in place — a Service
    // type changed, a container added — with the note left saying what was
    // downloaded is a review of bytes that are not the ones deployed.
    let mut checked = 0usize;
    for path in files_with_extension(BOOTSTRAP, "yaml") {
        if !path.components().any(|c| c.as_os_str() == "upstream") {
            continue;
        }
        let component = path
            .parent()
            .and_then(std::path::Path::parent)
            .expect("upstream/ sits under its component");
        let note = std::fs::read_to_string(component.join("SOURCE.md")).unwrap_or_else(|_| {
            panic!(
                "{} has no SOURCE.md beside its upstream/; nothing records where the bytes \
                 came from",
                component.display()
            )
        });
        let recorded: Vec<String> = note
            .lines()
            .filter_map(|line| line.trim().strip_prefix("sha256"))
            // `sha256  <hex>` or `sha256  <hex>  <path>`: the hex is the first
            // token, and a note may name the file it hashed beside it.
            .filter_map(|rest| rest.split_whitespace().next().map(str::to_string))
            .filter(|hex| hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
            .collect();
        assert!(
            !recorded.is_empty(),
            "{}/SOURCE.md records no `sha256  <hex>` line",
            component.display()
        );
        // The workspace has no hashing crate (ADR 0002, ADR 0009); the
        // system's sha256sum is the one tool this test shells out to, and
        // its absence is a failure that says so rather than a pass.
        let output = std::process::Command::new("sha256sum")
            .arg(&path)
            .output()
            .unwrap_or_else(|error| panic!("sha256sum could not be run: {error}"));
        assert!(
            output.status.success(),
            "sha256sum failed on {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let actual = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        assert!(
            recorded.contains(&actual),
            "{} hashes to {actual}, and its SOURCE.md records {recorded:?}; the bytes deployed \
             are not the bytes reviewed",
            path.display()
        );
        checked += 1;
    }
    assert!(
        checked >= 2,
        "only {checked} upstream install(s) were checked; Argo CD's and Kargo's are two"
    );
}

// --- the workflows --------------------------------------------------------------------

/// Whether a workflow's commands invoke `tool` as a command — at the start
/// of a line or after a pipe, `&&`, `;` or `$(` — rather than merely
/// mentioning the word. `argocd` is a namespace name in `kubectl -n argocd`
/// and a variable in a Python heredoc, and a substring match would refuse
/// the bootstrap for both.
fn invokes(commands: &str, tool: &str) -> bool {
    commands.lines().any(|line| {
        line.split(['|', ';'])
            .flat_map(|segment| segment.split("&&"))
            .flat_map(|segment| segment.split("$("))
            .flat_map(|segment| segment.split("||"))
            .map(str::trim_start)
            .map(|segment| {
                // A one-line `run: tool …` is the same invocation as the
                // first line of a `run: |` block.
                segment
                    .trim_start_matches("- ")
                    .trim_start_matches("run:")
                    .trim_start()
                    .trim_start_matches("! ")
                    .trim_start_matches("exec ")
                    .trim_start_matches("time ")
            })
            .any(|segment| {
                segment == tool.trim_end()
                    || (segment.starts_with(tool)
                        && !segment[tool.len()..].trim_start().starts_with('='))
            })
    })
}

/// A workflow's `run:` lines, comment lines removed.
fn workflow_commands(path: &str) -> String {
    read(path)
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_pipeline_no_longer_moves_a_service_and_still_signs_attests_and_refuses_prod() {
    // ADR 0036 decision 8. The pipeline keeps build, scan, sign, attest and
    // push and loses the rollout: `gcloud run services update` from the
    // workflow would be a second writer to the same image beside Kargo's
    // promotion commit, and two writers to one fact is the disagreement this
    // repository refuses by principle. `images.tfvars` goes with it — the
    // record is the promotion commit. What must not go with it: the
    // attestation, without which nothing is admitted, and the prod refusal,
    // which ADR 0036 keeps word for word beside Kargo's.
    let commands = workflow_commands(DEPLOY_WORKFLOW);
    for retired in [
        "gcloud run services update",
        "gcloud run deploy",
        "images.tfvars",
        "prove-serving.py",
    ] {
        assert!(
            !commands.contains(retired),
            "{DEPLOY_WORKFLOW} still runs or writes `{retired}`; the pipeline stopped moving \
             services under ADR 0036 and a step that moves one again is a second writer"
        );
    }
    for kept in [
        "sign-and-create",
        "--attestor",
        "trivy image",
        "docker push",
    ] {
        assert!(
            commands.contains(kept),
            "{DEPLOY_WORKFLOW} no longer runs `{kept}`; the attestation is what makes a digest \
             admissible and Kargo promotes nothing that is not"
        );
    }
    let deploy = read(DEPLOY_WORKFLOW);
    assert!(
        deploy.contains("refuse a production deployment that nobody dispatched")
            && deploy.contains(
                "env.TARGET_ENVIRONMENT == 'prod' && github.event_name != 'workflow_dispatch'"
            ),
        "{DEPLOY_WORKFLOW} lost its prod refusal; Kargo's policy is the third refusal, not a \
         replacement for this one"
    );
    // The revision label the promotion reads to refuse mixed freight.
    assert!(
        commands.contains("org.opencontainers.image.revision="),
        "{DEPLOY_WORKFLOW} no longer labels the image with the source revision; the promotion \
         cannot then refuse freight that mixes commits"
    );
    // And no controller CLI drives the promotion or the sync from outside.
    for tool in ["argocd ", "kargo ", "helm "] {
        assert!(
            !invokes(&commands, tool),
            "{DEPLOY_WORKFLOW} runs `{tool}`; promotion is Kargo's and sync is Argo CD's, and a \
             workflow driving either from outside is the second path"
        );
    }
}

/// The steps of infra.yml's one job, as their text.
fn infra_steps() -> Vec<String> {
    let infra = read(INFRA_WORKFLOW);
    let mut steps: Vec<Vec<&str>> = Vec::new();
    let mut in_steps = false;
    for line in infra.lines() {
        if !in_steps {
            in_steps = line.trim() == "steps:";
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 6 && line.trim_start().starts_with("- ") {
            steps.push(vec![line]);
        } else if let Some(last) = steps.last_mut() {
            last.push(line);
        }
    }
    assert!(
        steps.len() >= 5,
        "only {} steps were read out of {INFRA_WORKFLOW}",
        steps.len()
    );
    steps.into_iter().map(|step| step.join("\n")).collect()
}

#[test]
fn the_bootstrap_applies_only_where_gitops_is_enabled_and_only_from_the_workflow_that_refuses_prod()
{
    // ADR 0036 decision 2: the controllers are installed by `kubectl apply`
    // of the vendored, pinned manifests, from infra.yml's `up`, against the
    // private endpoint with credentials from `gcloud container clusters
    // get-credentials`. Three properties, each one edit from gone: the step
    // runs only where the environment's tfvars enable the control plane, so
    // an environment without a cluster does not fail its apply on a
    // kubeconfig it cannot get; it runs in the one workflow that refuses
    // prod; and it is the only place any workflow runs kubectl.
    let steps = infra_steps();
    let bootstrap: Vec<&String> = steps
        .iter()
        .filter(|step| {
            step.lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .any(|line| line.contains("kubectl "))
        })
        .collect();
    assert!(
        !bootstrap.is_empty(),
        "{INFRA_WORKFLOW} has no step running kubectl; nothing installs the controllers"
    );
    for step in &bootstrap {
        let condition = step
            .lines()
            .find_map(|line| line.trim_start().strip_prefix("if:"))
            .map(str::trim)
            .unwrap_or_else(|| panic!("the bootstrap step has no `if:`, so it runs for every action and every environment:\n{step}"));
        assert!(
            condition.contains("inputs.action == 'up'"),
            "the bootstrap step's condition `{condition}` does not require action=up; a plan \
             would apply manifests"
        );
        // Gated on the tfvars' gitops_enabled — in the `if:`, or in the run
        // before anything reaches for the cluster, with an exit that is not
        // a failure: an environment with no cluster is not an error, it is
        // an environment nobody enabled the control plane for.
        let gated_in_condition = condition.contains("gitops");
        let commands = step
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let gate = commands.find("gitops_enabled");
        let first_reach = commands
            .find("gcloud container")
            .or_else(|| commands.find("kubectl"))
            .unwrap_or(usize::MAX);
        let gated_in_run = gate.is_some_and(|at| at < first_reach)
            && commands[gate.unwrap_or(0)..first_reach.min(commands.len())].contains("exit 0");
        assert!(
            gated_in_condition || gated_in_run,
            "the bootstrap step is not gated on the environment's gitops_enabled before it \
             reaches for a cluster (condition `{condition}`); an environment with no cluster \
             then fails on a kubeconfig it cannot get"
        );
        // Credentials under the workflow's own identity, for the private
        // endpoint: either the cluster's own, reached by internal address or
        // DNS endpoint, or the fleet's Connect gateway, which is how a
        // runner outside the VPC reaches a cluster with no public endpoint.
        let clusters = step.contains("gcloud container clusters get-credentials");
        let fleet = step.contains("gcloud container fleet memberships get-credentials");
        assert!(
            clusters || fleet,
            "the bootstrap step applies without fetching the cluster's credentials under the \
             workflow's own identity:\n{step}"
        );
        if clusters {
            assert!(
                step.contains("--internal-ip")
                    || step.contains("--private-endpoint")
                    || step.contains("dns-endpoint"),
                "the bootstrap step fetches credentials for the public endpoint, which the \
                 cluster does not have:\n{step}"
            );
        }
        // What it applies is the bootstrap tree — through a variable holding
        // the tree's root and per-component overlay paths under it.
        assert!(
            step.contains(GITOPS) && step.contains("/bootstrap/"),
            "the bootstrap step applies something other than {BOOTSTRAP}:\n{step}"
        );
    }
    // The gate is read from the tfvars like every other identity fact.
    let commands = workflow_commands(INFRA_WORKFLOW);
    assert!(
        commands.contains("gitops_enabled"),
        "{INFRA_WORKFLOW} never reads gitops_enabled out of the tfvars, so the bootstrap's \
         condition is decided by something this test cannot see"
    );
    // The prod refusal stands in front of it.
    assert!(
        commands.contains("inputs.environment }}\" = \"prod\" ]"),
        "{INFRA_WORKFLOW} no longer refuses prod in a step"
    );
    // kubectl nowhere else, and no controller CLI anywhere.
    for path in files_with_extension(".github/workflows", "yml") {
        let display = path
            .strip_prefix(repository_root())
            .unwrap_or(&path)
            .display()
            .to_string();
        let commands = workflow_commands(&display);
        for tool in ["helm ", "argocd ", "kargo "] {
            assert!(
                !invokes(&commands, tool),
                "{display} runs `{tool}`; the bootstrap applies pinned bytes and nothing drives \
                 a controller from a workflow"
            );
        }
        if display != INFRA_WORKFLOW {
            for tool in ["kubectl ", "kustomize "] {
                assert!(
                    !invokes(&commands, tool),
                    "{display} runs `{tool}`; only infra.yml's bootstrap step may"
                );
            }
        }
    }
}

// --- Terraform ----------------------------------------------------------------------

/// Every `removed` block in the root module as `(from, destroy)`.
fn removed_blocks() -> Vec<(String, Option<String>)> {
    let mut found = Vec::new();
    for path in files_with_extension("infrastructure/terraform", "tf") {
        if path
            .strip_prefix(repository_root().join("infrastructure/terraform"))
            .is_ok_and(|rest| rest.components().count() > 1)
        {
            continue; // a module's own file; removed blocks live in the root
        }
        let text = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        let mut depth: Option<usize> = None;
        let mut from: Option<String> = None;
        let mut destroy: Option<String> = None;
        for line in text.lines() {
            match depth {
                None => {
                    if line.trim_start().starts_with("removed ") && line.contains('{') {
                        depth = Some(line.matches('{').count() - line.matches('}').count());
                        from = None;
                        destroy = None;
                    }
                }
                Some(open) => {
                    if let Some(value) = collapsed(line).strip_prefix("from = ") {
                        from = Some(value.to_string());
                    }
                    if let Some(value) = collapsed(line).strip_prefix("destroy = ") {
                        destroy = Some(value.to_string());
                    }
                    let next = open + line.matches('{').count() - line.matches('}').count();
                    if next == 0 {
                        found.push((from.take().unwrap_or_default(), destroy.take()));
                        depth = None;
                    } else {
                        depth = Some(next);
                    }
                }
            }
        }
    }
    found
}

/// The one module directory that declares the control-plane cluster.
fn control_plane_module() -> (String, String) {
    let mut found: Vec<(String, String)> = Vec::new();
    for path in files_with_extension(MODULES, "tf") {
        let text = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        if text.contains("resource \"google_container_cluster\"") {
            let module = path
                .strip_prefix(repository_root().join(MODULES))
                .ok()
                .and_then(|rest| rest.components().next())
                .map(|component| component.as_os_str().to_string_lossy().to_string())
                .unwrap_or_default();
            found.push((module, text));
        }
    }
    assert_eq!(
        found.len(),
        1,
        "{} module file(s) under {MODULES} declare a google_container_cluster ({:?}); ADR 0036 \
         decision 1 is one control-plane module and nothing else may hold a cluster",
        found.len(),
        found.iter().map(|(module, _)| module).collect::<Vec<_>>()
    );
    let (module, text) = found.remove(0);
    assert!(
        module.contains("control-plane"),
        "the cluster lives in modules/{module}, whose name does not say it is the control plane"
    );
    (module, text)
}

#[test]
fn terraform_releases_the_services_without_destroying_them_and_gates_the_control_plane() {
    // ADR 0036 decision 5. The service leaves Terraform's state with
    // `destroy = false`, so the apply that releases it destroys nothing and
    // Config Connector acquires a service that still exists; a `removed`
    // block without that line, or the resource simply deleted from the
    // module, is a plan that destroys three services and every revision
    // they ever served. And a `removed` block for a resource the module
    // still declares is refused by Terraform, so the two halves are
    // asserted together.
    let removed = removed_blocks();
    assert!(
        !removed.is_empty(),
        "no `removed` block exists in the root module; the services either stay in Terraform \
         (two writers) or are destroyed by the apply that drops them"
    );
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    for resource in [
        "google_cloud_run_v2_service.workload",
        "google_cloud_run_v2_service_iam_member.invokers",
    ] {
        let address = format!("module.cloud_run.{resource}");
        let (_, destroy) = removed
            .iter()
            .find(|(from, _)| *from == address)
            .unwrap_or_else(|| {
                panic!(
                    "no `removed` block names `{address}`; it has {:?}",
                    removed.iter().map(|(from, _)| from).collect::<Vec<_>>()
                )
            });
        assert_eq!(
            destroy.as_deref(),
            Some("false"),
            "the `removed` block for `{address}` does not set `destroy = false`; the apply \
             would destroy the service"
        );
        let (resource_type, resource_name) = resource.split_once('.').expect("type.name");
        assert!(
            !module.contains(&format!("resource \"{resource_type}\" \"{resource_name}\"")),
            "modules/cloudrun still declares {resource} while the root removes it; Terraform \
             refuses that plan, and until it is deleted from the module nothing has moved"
        );
    }
    // OpenObserve's instantiation is released the same way.
    assert!(
        removed
            .iter()
            .any(|(from, destroy)| from.starts_with("module.openobserve")
                && destroy.as_deref() == Some("false")),
        "no `removed` block releases module.openobserve's service with destroy = false; it has {:?}",
        removed.iter().map(|(from, _)| from).collect::<Vec<_>>()
    );
    // What stays: the identity, its grants and the configuration objects.
    for kept in [
        "resource \"google_service_account\" \"workload\"",
        "resource \"google_secret_manager_secret_iam_member\" \"mounted\"",
        "resource \"google_storage_bucket_object\" \"config_files\"",
    ] {
        assert!(
            module.contains(kept),
            "modules/cloudrun no longer declares `{kept}`; ADR 0036 decision 5 keeps the \
             identity, its grants and the configuration buckets in Terraform"
        );
    }

    // The control plane is gated on one root variable that defaults off,
    // so an environment nobody enabled it for gets no cluster and no bill.
    let variables = without_comments(&read(ROOT_VARIABLES));
    let block = variables
        .split("variable \"gitops_enabled\" {")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("variables.tf declares gitops_enabled");
    assert!(
        block
            .lines()
            .any(|line| collapsed(line) == "default = false"),
        "gitops_enabled does not default to false; every environment then gets a cluster on \
         the next apply whether or not anybody decided it"
    );
    assert!(
        block.lines().any(|line| collapsed(line) == "type = bool"),
        "gitops_enabled is not a bool"
    );
    let (module_name, _) = control_plane_module();
    let root: String = files_with_extension("infrastructure/terraform", "tf")
        .iter()
        .filter(|path| path.parent() == Some(&repository_root().join("infrastructure/terraform")))
        .map(|path| without_comments(&std::fs::read_to_string(path).expect("readable")))
        .collect::<Vec<_>>()
        .join("\n");
    let calls: Vec<&str> = root
        .split("\nmodule \"")
        .skip(1)
        .filter(|body| {
            body.contains(&format!("source = \"./modules/{module_name}\""))
                || body
                    .lines()
                    .any(|line| collapsed(line) == format!("source = \"./modules/{module_name}\""))
        })
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "the root instantiates modules/{module_name} {} times",
        calls.len()
    );
    assert!(
        calls[0]
            .lines()
            .any(|line| collapsed(line) == "count = var.gitops_enabled ? 1 : 0"),
        "the root's module block for modules/{module_name} is not `count = var.gitops_enabled ? \
         1 : 0`; the cluster is then created regardless of the variable"
    );
}

#[test]
fn the_control_plane_cluster_is_private_autopilot_without_the_addon_and_no_wildcard_network() {
    // ADR 0036 decision 1: private nodes, private endpoint, no public
    // endpoint, Workload Identity on. Config Connector is NOT the GKE addon:
    // `infra.yml` runs 34 and 35 (2026-09-05) were refused with `addons
    // {"config-connector"} are not supported for Autopilot clusters`, so it
    // arrives as a vendored operator the bootstrap applies, and
    // `infrastructure.rs` asserts that path. A master authorized network of
    // 0.0.0.0/0 is a public endpoint under another name, and the identity
    // that can reach this API server can change what Config Connector
    // applies to every Cloud Run service in the environment.
    let (module, text) = control_plane_module();
    let cluster = text
        .split("resource \"google_container_cluster\"")
        .nth(1)
        .and_then(|rest| rest.split("\nresource ").next())
        .expect("the module declares a cluster");
    let sets = |key: &str, value: &str| {
        cluster
            .lines()
            .any(|line| collapsed(line) == format!("{key} = {value}"))
    };
    assert!(
        sets("enable_autopilot", "true"),
        "modules/{module}'s cluster is not Autopilot"
    );
    assert!(
        sets("enable_private_endpoint", "true"),
        "modules/{module}'s cluster has a public endpoint"
    );
    assert!(
        sets("enable_private_nodes", "true"),
        "modules/{module}'s nodes have public addresses"
    );
    assert!(
        !cluster.contains("config_connector_config"),
        "modules/{module}'s cluster declares the Config Connector addon again; the API refuses \
         it on Autopilot (infra.yml runs 34 and 35) and the operator is vendored under \
         gitops/bootstrap/config-connector-operator instead"
    );
    assert!(
        cluster.contains("workload_identity_config {"),
        "modules/{module}'s cluster has no workload identity; the controllers would need a key"
    );
    let authorized = cluster
        .split("master_authorized_networks_config {")
        .nth(1)
        .expect("the cluster declares master_authorized_networks_config; without it the API server admits any address that can route to it");
    let cidrs: Vec<String> = authorized
        .lines()
        .filter_map(|line| {
            collapsed(line)
                .strip_prefix("cidr_block = ")
                .map(str::to_string)
        })
        .collect();
    assert!(
        !cidrs.is_empty(),
        "modules/{module}'s master_authorized_networks_config names no cidr_block"
    );
    for cidr in &cidrs {
        assert!(
            !cidr.contains("0.0.0.0/0") && !cidr.contains("::/0"),
            "modules/{module} authorizes {cidr} to reach the API server, which is the whole internet"
        );
    }
    assert!(
        !text.contains("0.0.0.0/0"),
        "modules/{module} names 0.0.0.0/0 somewhere"
    );
    // The GitOps cluster gets no other provider: no kubernetes, helm or
    // kubectl provider anywhere in the Terraform.
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        for provider in [
            "hashicorp/kubernetes",
            "hashicorp/helm",
            "gavinbunney/kubectl",
            "alekc/kubectl",
        ] {
            assert!(
                !content.contains(provider),
                "{} declares the {provider} provider; ADR 0036 keeps google and google-beta only",
                path.display()
            );
        }
    }
}

// --- the proof that a promotion served ----------------------------------------------

#[test]
fn a_promotion_names_who_verifies_it() {
    // An apply that returns is not a deployment that worked. The first
    // GitOps cut-over lost the rollout wait, so a build that crash-looped on
    // boot produced a green pipeline (ADR 0024). `gcloud run services
    // update` closed that from the pipeline; ADR 0036 decision 7 moves the
    // proof to the reconciler and does not weaken it. Two halves, because
    // the first alone is satisfied by a traffic split that never moved:
    //
    // 1. Argo CD's health of a RunService reads the resource's own `Ready`
    //    condition. Argo CD has no built-in health for Config Connector
    //    kinds, so without a customization every RunService is Healthy the
    //    moment it is applied — the crash loop is green again.
    // 2. A post-sync hook reads the revisions `status.traffic` routes to and
    //    fails the sync unless the workload's container, selected by name,
    //    carries the digest the manifest names.
    let manifests = manifests_under(GITOPS);

    // The health customization, in argocd-cm.
    let config_maps: Vec<&Manifest> = manifests
        .iter()
        .filter(|manifest| manifest.kind() == "ConfigMap" && manifest.name() == "argocd-cm")
        .collect();
    assert!(
        !config_maps.is_empty(),
        "no ConfigMap named argocd-cm under {GITOPS}; the health customization has nowhere to be"
    );
    let health_key = "resource.customizations.health.run.cnrm.cloud.google.com_RunService";
    let health: Vec<String> = config_maps
        .iter()
        .filter_map(|cm| text_at(&cm.value, &["data", health_key]))
        .collect();
    assert_eq!(
        health.len(),
        1,
        "{} argocd-cm(s) carry `{health_key}`; exactly one must, or a RunService is Healthy \
         the moment it is applied",
        health.len()
    );
    let lua = &health[0];
    for needed in ["Ready", "Degraded", "Progressing", "Healthy", "conditions"] {
        assert!(
            lua.contains(needed),
            "the RunService health check never mentions `{needed}`; it cannot be reading the \
             Ready condition and reporting Degraded when it is false:\n{lua}"
        );
    }

    // The post-sync hook: a Job with the PostSync annotation, under every
    // environment, asking the two questions by name.
    let mut hooks = 0usize;
    for environment in environment_directories() {
        let hook = manifests_under(&format!("{ENVS}/{environment}"))
            .into_iter()
            .find(|manifest| {
                manifest.kind() == "Job"
                    && manifest.annotation("argocd.argoproj.io/hook").as_deref() == Some("PostSync")
            })
            .unwrap_or_else(|| {
                panic!(
                    "{ENVS}/{environment} has no Job annotated argocd.argoproj.io/hook: PostSync; \
                        nothing proves the serving revision runs the promoted digest"
                )
            });
        let text = strings_of(&hook.value).join("\n");
        // The routed revisions, by whichever spelling the hook reads
        // `status.traffic` — a jsonpath, a Python `.get`, a jq filter.
        assert!(
            text.contains("status.traffic")
                || text.contains(".get(\"traffic\"")
                || text.contains(".status.traffic")
                || text.contains("status[\"traffic\"]"),
            "{} never reads status.traffic; it is not reading the revisions traffic routes \
             to, and spec.template is what was asked for, which a split that never moved \
             satisfies",
            hook.describe()
        );
        assert!(
            text.contains("revisionName"),
            "{} never reads a routed entry's revisionName",
            hook.describe()
        );
        assert!(
            text.contains("gcloud run revisions describe")
                || text.contains("\"gcloud\", \"run\", \"revisions\", \"describe\""),
            "{} never describes the routed revision, so it cannot read what it serves",
            hook.describe()
        );
        assert!(
            text.contains("exit 1") || text.contains("sys.exit("),
            "{} never fails; a hook that prints a mismatch and succeeds is a green sync over a \
             wrong revision",
            hook.describe()
        );
        assert!(
            text.contains("== workload")
                || text.contains("== wanted")
                || text.contains("== \"$name\""),
            "{} does not select the workload's container by name",
            hook.describe()
        );
        assert!(
            !text.contains("containers[0]") && !text.contains("conditions[0]"),
            "{} selects a container or a condition by position; every service carries a sidecar",
            hook.describe()
        );
        // The hook's own image is subject to the same pin as everything
        // else (the image test covers it); here, that it is not a trading
        // binary pretending to be a hook.
        let mut images = Vec::new();
        find_key(&hook.value, "image", &mut images);
        for image in images {
            if let Some(image) = image.as_str() {
                assert!(
                    trading_binary_of(image).is_none(),
                    "{} runs `{image}`, a trading binary, as a Pod",
                    hook.describe()
                );
            }
        }
        hooks += 1;
    }
    assert_eq!(hooks, 4);
}

// --- what a failed sync says, and who may read the revision ---------------------------

/// The RunService health customisation's Lua, from the one argocd-cm that
/// carries it.
fn run_service_health_lua() -> String {
    let health_key = "resource.customizations.health.run.cnrm.cloud.google.com_RunService";
    let health: Vec<String> = manifests_under(GITOPS)
        .iter()
        .filter(|manifest| manifest.kind() == "ConfigMap" && manifest.name() == "argocd-cm")
        .filter_map(|cm| text_at(&cm.value, &["data", health_key]))
        .collect();
    assert_eq!(
        health.len(),
        1,
        "{} argocd-cm(s) carry `{health_key}`; exactly one must, or a RunService is Healthy \
         the moment it is applied",
        health.len()
    );
    health[0].clone()
}

#[test]
fn a_sync_that_fails_says_why_rather_than_reporting_healthy() {
    // What `gcloud run services update` gave on a failed rollout was one
    // true sentence and a console URL nobody in CI opens; deploy.yml grew a
    // log read so the run said what failed. On the reconciler the failure
    // shows as a RunService whose Ready condition is false, and the sentence
    // that matters is Config Connector's own condition message — Cloud Run's
    // admission refusal, the missing secret version, the image nothing
    // attested. The health check carries that message into the Application,
    // marks the terminal reasons Degraded rather than leaving them
    // Progressing for ever, and the hook names the service, the revision and
    // both images when it refuses.
    let lua = run_service_health_lua();
    assert!(
        lua.contains("hs.message = condition.message"),
        "the RunService health check does not carry Config Connector's condition message into \
         the Application; a failed sync then says Degraded and nothing else:\n{lua}"
    );
    for reason in ["CreateFailed", "UpdateFailed"] {
        assert!(
            lua.contains(reason),
            "the RunService health check does not treat `{reason}` as terminal; a service Cloud \
             Run refused stays Progressing for ever:\n{lua}"
        );
    }
    assert!(
        lua.contains("hs.status = \"Degraded\""),
        "the RunService health check never reports Degraded:\n{lua}"
    );
    let mut hooks = 0usize;
    for environment in environment_directories() {
        let hook = manifests_under(&format!("{ENVS}/{environment}"))
            .into_iter()
            .find(|manifest| {
                manifest.kind() == "Job"
                    && manifest.annotation("argocd.argoproj.io/hook").as_deref() == Some("PostSync")
            })
            .unwrap_or_else(|| panic!("{ENVS}/{environment} has no PostSync hook"));
        let text = strings_of(&hook.value).join("\n");
        assert!(
            text.contains("the manifest names"),
            "{} refuses a mismatch without naming what the manifest wanted beside what serves",
            hook.describe()
        );
        assert!(
            text.contains("routes traffic to no revision"),
            "{} does not say when a Ready service routes traffic nowhere",
            hook.describe()
        );
        hooks += 1;
    }
    assert_eq!(hooks, 4);
}

#[test]
fn the_hook_that_proves_a_promotion_may_read_the_revision_it_proves() {
    // The pipeline's log read once ran under an account nothing had granted
    // `logging.viewer`, so the diagnosis step printed a permission error
    // where the cause should be — worse than no step, because the reader
    // believed the pipeline had tried to explain and had nothing to say. The
    // hook has the same shape: it runs on the cluster as a Kubernetes
    // service account, reaches Cloud Run as a Google one through Workload
    // Identity, and reads the routed revision with `run.viewer`. Every link
    // is asserted: the Job names its account; that account is annotated
    // with the Argo CD identity Terraform creates; the control-plane module
    // binds exactly that namespace and name to it; and the identity holds
    // run.viewer and nothing that writes a service.
    let (_, module) = control_plane_module();
    let mut checked = 0usize;
    for environment in environment_directories() {
        let project = tfvars_value(&environment, "project_id").unwrap_or_default();
        let manifests = manifests_under(&format!("{ENVS}/{environment}"));
        let namespace = of_kind(&manifests, "Kustomization")
            .first()
            .and_then(|kustomization| text_at(&kustomization.value, &["namespace"]))
            .unwrap_or_else(|| {
                panic!("{ENVS}/{environment} sets no namespace in its kustomization")
            });
        let hook = of_kind(&manifests, "Job")
            .into_iter()
            .find(|job| job.annotation("argocd.argoproj.io/hook").as_deref() == Some("PostSync"))
            .unwrap_or_else(|| panic!("{ENVS}/{environment} has no PostSync hook"));
        let account = text_at(
            &hook.value,
            &["spec", "template", "spec", "serviceAccountName"],
        )
        .unwrap_or_else(|| {
            panic!(
                "{} names no serviceAccountName, so it runs as the namespace's default \
                     account, which is bound to nothing",
                hook.describe()
            )
        });
        let ksa = of_kind(&manifests, "ServiceAccount")
            .into_iter()
            .find(|sa| sa.name() == account)
            .unwrap_or_else(|| {
                panic!("{ENVS}/{environment} declares no ServiceAccount named `{account}`")
            });
        let gsa = ksa
            .annotation("iam.gke.io/gcp-service-account")
            .unwrap_or_else(|| {
                panic!(
                    "{} carries no iam.gke.io/gcp-service-account annotation, so the hook has \
                     no Google identity and every gcloud call is refused",
                    ksa.describe()
                )
            });
        assert_eq!(
            gsa,
            format!("qip-{environment}-argocd@{project}.iam.gserviceaccount.com"),
            "{} runs as `{gsa}`, not the Argo CD identity modules/gitops-control-plane creates",
            ksa.describe()
        );
        let binding = format!("\"{namespace}/{account}\"");
        assert!(
            module.contains(&binding),
            "modules/gitops-control-plane binds no Kubernetes service account {namespace}/{account} \
             to the Argo CD identity (it names {:?}); the hook's token exchange is refused and \
             the proof cannot read Cloud Run",
            module
                .lines()
                .filter(|line| line.contains("svc.id.goog[")
                    || line.contains("argocd/")
                    || line.contains("qip-run/"))
                .map(str::trim)
                .collect::<Vec<_>>()
        );
        checked += 1;
    }
    assert_eq!(checked, 4);
    let argocd_grants: Vec<&str> = module
        .lines()
        .filter(|line| line.contains("roles/"))
        .map(str::trim)
        .collect();
    assert!(
        module.contains("resource \"google_project_iam_member\" \"argocd_reads_run\"")
            && module.contains("role    = \"roles/run.viewer\""),
        "modules/gitops-control-plane grants the Argo CD identity no roles/run.viewer; the hook \
         cannot describe a revision. It grants {argocd_grants:?}"
    );
    let argocd_block = module
        .split("resource \"google_project_iam_member\" \"argocd_reads_run\"")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .unwrap_or_default();
    assert!(
        argocd_block.contains("google_service_account.argocd.email")
            && !argocd_block.contains("run.admin")
            && !argocd_block.contains("run.developer"),
        "the Argo CD identity's Cloud Run grant is not run.viewer on the argocd account alone:\n\
         {argocd_block}"
    );
}

// --- the sidecar and OpenObserve, as manifests ------------------------------------

/// The `vendor/envoy` line of the vendored-images list, as its digest.
fn vendored_digest(destination: &str) -> String {
    let digests: Vec<String> = vendored_images()
        .into_iter()
        .filter(|(_, dest)| dest == destination)
        .map(|(digest, _)| digest)
        .collect();
    assert_eq!(
        digests.len(),
        1,
        "{VENDORED} carries {} `{destination}` line(s); expected exactly one",
        digests.len()
    );
    digests[0].clone()
}

#[test]
fn the_egress_sidecar_in_every_manifest_holds_no_credential_and_the_workload_waits_for_it() {
    // The proxy terminates TLS, so every token its clients send passes
    // through it in clear text; it is the highest-value process on the
    // instance, and the mitigation is that compromising it yields nothing but
    // the traffic already flowing: no environment, no mounted secret, only
    // its bootstrap. And the workload container waits for it, because its
    // first outbound call after a cold start otherwise hits a sidecar that
    // is not listening. `modules/cloudrun` held both until ADR 0036; the
    // manifest holds them now.
    let envoy = vendored_digest("vendor/envoy");
    let mut sidecars = 0usize;
    for environment in environment_directories() {
        for (key, entry, service, siblings) in run_services(&environment) {
            let Some(entry) = entry else {
                continue;
            };
            if catalogue_field(&entry, "egress_proxy") != "true" {
                continue;
            }
            let describe = service.describe();
            let containers = list_at(&service.value, &["spec", "template", "containers"]);
            let sidecar = containers
                .iter()
                .find(|container| text_at(container, &["name"]).as_deref() == Some("qip-egress"))
                .unwrap_or_else(|| panic!("{describe} carries no qip-egress container"));
            let image = resolve_image(
                &text_at(sidecar, &["image"]).unwrap_or_default(),
                &image_overrides(&siblings),
            );
            assert!(
                image.ends_with(&format!("/vendor/envoy@{envoy}"))
                    && image.contains(&format!("/qip-{environment}/")),
                "{describe}'s sidecar runs `{image}`, not the mirrored Envoy at the digest \
                 {VENDORED} reviewed, from the environment's own registry"
            );
            assert!(
                list_at(sidecar, &["env"]).is_empty(),
                "{describe}'s sidecar carries an environment; one more thing in \
                 /proc/<pid>/environ on the most exposed process"
            );
            let mounts: Vec<String> = list_at(sidecar, &["volumeMounts"])
                .iter()
                .filter_map(|mount| text_at(mount, &["name"]))
                .collect();
            assert_eq!(
                mounts,
                vec!["egress-bootstrap".to_string()],
                "{describe}'s sidecar mounts {mounts:?}; only its bootstrap, never a secret"
            );
            assert_eq!(
                text_at(sidecar, &["startupProbe", "httpGet", "path"]).as_deref(),
                Some("/healthz"),
                "{describe}'s sidecar has no startup probe on its health listener"
            );
            let waits: Vec<String> = list_at(workload_container(&service, &key), &["dependsOn"])
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect();
            assert!(
                waits.iter().any(|name| name == "qip-egress"),
                "{describe}'s `{key}` container does not depend on qip-egress, so its first \
                 outbound call after a cold start hits a sidecar that is not listening"
            );
            sidecars += 1;
        }
    }
    assert_eq!(
        sidecars,
        2 * 4,
        "{sidecars} sidecars were checked; api and deepbrain in four environments"
    );
}

#[test]
fn openobserve_is_deployed_at_the_reviewed_digest_anonymous_as_adr_0030_records_and_on_ephemeral_storage()
 {
    // ADR 0028 adopted OpenObserve as a vendored image; ADR 0030 exposed it
    // anonymously on the owner's instruction and ADR 0031 let it take its
    // root credential as an environment value. The catalogue's module block
    // carried all three until ADR 0036; the manifest carries them now, and
    // each is pinned here because each is an exception to a rule the parity
    // test holds for every other service. Both halves of the anonymous
    // posture are asserted — the open ingress and the allUsers invoker —
    // because either alone is a deployment that lies about itself.
    let digest = vendored_digest("vendor/openobserve");
    let mut checked = 0usize;
    for environment in environment_directories() {
        let manifests = manifests_under(&format!("{ENVS}/{environment}"));
        // Deployed exactly where the tfvars name the digest: the catalogue
        // gates OpenObserve's identity on `vendored_openobserve_image_digest`,
        // and a manifest for an environment with no identity is a service
        // that cannot start, while an environment with the identity and no
        // manifest is a principal with nothing attached.
        let deployed = tfvars_value(&environment, "vendored_openobserve_image_digest").is_some();
        let found = run_services(&environment)
            .into_iter()
            .find(|(key, _, _, _)| key == "openobserve");
        let Some((_, _, service, _)) = found else {
            assert!(
                !deployed,
                "{environment}'s tfvars name vendored_openobserve_image_digest and {ENVS}/\
                 {environment} deploys no openobserve RunService"
            );
            assert!(
                !manifests
                    .iter()
                    .any(|manifest| strings_of(&manifest.value).iter().any(|s| s == "allUsers")),
                "{ENVS}/{environment} names allUsers with no OpenObserve to bind it to"
            );
            continue;
        };
        assert!(
            deployed,
            "{} exists and {environment}'s tfvars name no vendored_openobserve_image_digest, so \
             Terraform creates no identity for it",
            service.describe()
        );
        let describe = service.describe();
        assert_eq!(
            text_at(&service.value, &["spec", "ingress"]).as_deref(),
            Some("INGRESS_TRAFFIC_ALL"),
            "{describe} is not anonymously reachable; ADR 0030 records that it is, and a service \
             behind an internal posture with an allUsers invoker is a public 403"
        );
        let anonymous: Vec<&Manifest> = manifests
            .iter()
            .filter(|manifest| {
                serde_json::to_string(&manifest.value)
                    .unwrap_or_default()
                    .contains("allUsers")
            })
            .collect();
        assert_eq!(
            anonymous.len(),
            1,
            "{} manifest(s) under {ENVS}/{environment} name allUsers ({:?}); exactly one, the \
             invoker binding on OpenObserve, may",
            anonymous.len(),
            anonymous.iter().map(|m| m.describe()).collect::<Vec<_>>()
        );
        let binding = serde_json::to_string(&anonymous[0].value).unwrap_or_default();
        assert!(
            binding.contains(&service.name()) && binding.contains("run.invoker"),
            "{} names allUsers but is not the run.invoker binding on {}",
            anonymous[0].describe(),
            service.name()
        );
        let containers = list_at(&service.value, &["spec", "template", "containers"]);
        assert_eq!(
            containers.len(),
            1,
            "{describe} runs {} containers",
            containers.len()
        );
        let image = resolve_image(
            &text_at(containers[0], &["image"]).unwrap_or_default(),
            &image_overrides(&manifests),
        );
        assert!(
            image.ends_with(&format!("/vendor/openobserve@{digest}"))
                && image.contains(&format!("/qip-{environment}/")),
            "{describe} runs `{image}`, not the mirrored OpenObserve at the digest {VENDORED} \
             reviewed from the environment's own registry"
        );
        let env = container_env(containers[0]);
        assert!(
            env.iter()
                .any(|(name, value, _)| name == "ZO_LOCAL_MODE_STORAGE"
                    && value.as_deref() == Some("disk")),
            "{describe} is not on ephemeral local storage; ADR 0028 decision 4 accepts that cost \
             to avoid the static HMAC key durable storage needs"
        );
        for (name, _, secret) in &env {
            assert!(
                !name.starts_with("ZO_S3_"),
                "{describe} names an S3 destination ({name}), which needs a static credential"
            );
            if *secret {
                assert!(
                    name == "ZO_ROOT_USER_EMAIL" || name == "ZO_ROOT_USER_PASSWORD",
                    "{describe} takes {name} from a secret as an environment value; ADR 0031 \
                     permits exactly the root login"
                );
            }
        }
        assert_eq!(
            service.annotation(DELETION_POLICY).as_deref(),
            Some("abandon"),
            "{describe} lacks `{DELETION_POLICY}: abandon`"
        );
        checked += 1;
    }
    assert!(
        checked >= 1,
        "no environment deploys OpenObserve; every assertion above was skipped"
    );
}

// --- the reader's own premise -------------------------------------------------------

#[test]
fn the_yaml_reader_reads_the_manifest_subset_it_claims_and_refuses_the_rest() {
    // Every test above trusts the reader. A reader that silently dropped a
    // key, or read a `- name:` item as a scalar, would make the parity test
    // pass on a manifest it never saw. So the subset is pinned on one
    // document exercising every construct a hand-written manifest uses, and
    // the refusal of what is outside it is pinned on one that is not.
    let source = r##"# a leading comment
---
apiVersion: run.cnrm.cloud.google.com/v1beta1
kind: RunService
metadata:
  name: qip-dev-api   # trailing comment
  annotations:
    "cnrm.cloud.google.com/deletion-policy": abandon
    argocd.argoproj.io/hook: PostSync
spec:
  ingress: INGRESS_TRAFFIC_INTERNAL_ONLY
  binaryAuthorization:
    useDefault: true
  template:
    containers:
    - name: api
      image: qip-api
      command: ["/bin/sh", "-c"]
      args:
      - |
        set -eu
        echo "# not a comment"
        exit 1
      env:
        - name: QIP_API_ADDRESS
          value: "0.0.0.0:8080"
        - name: QIP_TOKEN_OPERATOR_FILE
          value: /var/run/secrets/qip/token-operator/token-operator
      ports:
      - containerPort: 8080
    volumes: []
    scaling: {}
data:
  script: >-
    folded
    text
  mode: 256
  octal: '0400'
  nothing: ~
---
# an empty document with only a comment
---
kind: Namespace
metadata:
  name: qip-dev
"##;
    let documents = yaml::documents(source, "inline");
    assert_eq!(
        documents.len(),
        2,
        "two documents carry content; the empty one is dropped"
    );
    let expected = serde_json::json!({
        "apiVersion": "run.cnrm.cloud.google.com/v1beta1",
        "kind": "RunService",
        "metadata": {
            "name": "qip-dev-api",
            "annotations": {
                "cnrm.cloud.google.com/deletion-policy": "abandon",
                "argocd.argoproj.io/hook": "PostSync"
            }
        },
        "spec": {
            "ingress": "INGRESS_TRAFFIC_INTERNAL_ONLY",
            "binaryAuthorization": {"useDefault": true},
            "template": {
                "containers": [{
                    "name": "api",
                    "image": "qip-api",
                    "command": ["/bin/sh", "-c"],
                    "args": ["set -eu\necho \"# not a comment\"\nexit 1"],
                    "env": [
                        {"name": "QIP_API_ADDRESS", "value": "0.0.0.0:8080"},
                        {"name": "QIP_TOKEN_OPERATOR_FILE", "value": "/var/run/secrets/qip/token-operator/token-operator"}
                    ],
                    "ports": [{"containerPort": 8080}]
                }],
                "volumes": [],
                "scaling": {}
            }
        },
        "data": {
            "script": "folded text",
            "mode": 256,
            "octal": "0400",
            "nothing": null
        }
    });
    assert_eq!(documents[0], expected);
    assert_eq!(
        documents[1],
        serde_json::json!({"kind": "Namespace", "metadata": {"name": "qip-dev"}})
    );

    // Outside the subset: an alias is refused naming the line, not read as
    // the string `*base`.
    let outcome = std::panic::catch_unwind(|| {
        yaml::documents("base: &base\n  a: 1\ncopy: *base\n", "inline")
    });
    let message = match outcome {
        Ok(_) => panic!("an alias was read as something rather than refused"),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_default(),
    };
    assert!(
        message.contains("inline:1") && message.contains("anchor, alias or tag"),
        "the refusal does not name the line and the construct: {message}"
    );
}
