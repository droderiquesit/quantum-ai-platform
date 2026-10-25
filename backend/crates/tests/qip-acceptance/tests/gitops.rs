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
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

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

/// The Kargo chain, in promotion order (R12 CICD-9112): the four
/// environments with the `replay` and `paper` gates between `test` and
/// `stage`. The gates are stages with no environment of their own.
const CHAIN: [&str; 6] = ["dev", "test", "replay", "paper", "stage", "prod"];

/// The links of [`CHAIN`] that are gates rather than environments.
const GATES: [&str; 2] = ["replay", "paper"];

/// The one registry every platform image is built into and promoted from.
/// `deploy.yml` builds each commit once, here, and every environment runs
/// the digest it built (R12 CICD-9097, CICD-9112). It is dev's repository
/// because dev is the environment the automatic path builds for; the
/// vendored images are still mirrored per environment.
const BUILD_REGISTRY: &str = "us-east4-docker.pkg.dev/algorik-qip-dev/qip-dev";

/// The binaries the workspace builds. None may be the image of a Kubernetes
/// workload anywhere under `infrastructure/gitops`; three of them are the
/// images of `RunService` manifests, which is a different thing.
const TRADING_BINARIES: [&str; 8] = [
    "qip-api",
    "qip-fastbrain",
    "qip-deepbrain",
    "qip-edge-node",
    "qip-web",
    "qip-cli",
    "qip-fabricd",
    "qip-ledgerd",
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

/// Images `deploy.yml` builds that no run of it has ever produced, each with
/// why.
///
/// A different exemption from `NEVER_PROMOTED_TO` above, and the difference
/// is the axis. That list is about an *environment* nothing has promoted
/// into; this is about an *image* nothing has ever built, in any environment
/// — including dev, which that list may never name. Both allow `TO-PIN` and
/// neither is an exemption from the pin: the marker is the honest value while
/// there are no bytes, and a manifest carrying it is one Argo CD refuses to
/// reconcile, which is the correct behaviour for a service that would
/// otherwise deploy from a name nobody attested.
///
/// An entry expires the moment a run pins the digest, and
/// `every_image_deploy_yml_has_never_built_is_still_unbuilt_and_still_in_the_matrix`
/// is what expires it.
///
/// Empty since 2026-10-13: `qip-portal`, the one entry this list ever held,
/// was built, scanned and attested by deploy.yml run 268 and dev now pins
/// that digest, so the entry expired exactly as
/// `every_image_deploy_yml_has_never_built_is_still_unbuilt_and_still_in_the_matrix`
/// demands.
const NEVER_BUILT: &[(&str, &str)] = &[];

/// Whether an image reference names something `NEVER_BUILT` argues for, by
/// the last component of its repository path.
///
/// Matched on the whole component rather than with `contains`, because
/// `qip-portal` is a substring of nothing here today and this repository has
/// already been bitten by a check that matched a neighbouring name.
fn never_built(image: &str) -> bool {
    let (repository, _) = split_reference(image);
    let last = repository.rsplit('/').next().unwrap_or(&repository);
    NEVER_BUILT.iter().any(|(name, _)| *name == last)
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
        if let Some(key) = text.strip_suffix(':')
            && !key.contains(": ")
        {
            return (key.trim().to_string(), String::new());
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
            && let Ok(number) = text.parse::<u64>()
        {
            return Value::Number(number.into());
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
                if in_metadata && let Some(value) = line.strip_prefix("  name: ") {
                    name = value.trim().trim_matches('"').to_string();
                    break;
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
        if inside && let Some((key, _)) = line.split_once('=') {
            let key = key.trim();
            if key.starts_with("QIP_") {
                names.insert(key.to_string());
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
        // is a reconciler reading a line nobody promoted to. Until 2026-09-13
        // the default branch was a `claude/...` branch and this admitted any
        // `claude/` prefix — which, once the default was `main`, would have
        // let an Application quietly reconcile some other working branch.
        // The default is `main` now and the matcher says exactly that.
        let revision = text_at(&application.value, &["spec", "source", "targetRevision"])
            .unwrap_or_else(|| {
                panic!(
                    "{} pins no spec.source.targetRevision",
                    application.describe()
                )
            });
        assert!(
            revision == "HEAD" || revision == "main",
            "{} reconciles targetRevision `{revision}`, which is neither the default branch \
             (`main`) nor HEAD; the promotion commits land on the default branch and nothing \
             else",
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
            if manifest.kind() == "Kustomization"
                && let Some(namespace) = text_at(&manifest.value, &["namespace"])
            {
                permitted.insert(namespace);
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

/// The Kargo Stages, keyed by name; exactly the links of [`CHAIN`].
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
    let mut expected: Vec<String> = CHAIN.iter().map(|link| (*link).to_string()).collect();
    expected.sort();
    assert_eq!(
        names,
        expected.iter().collect::<Vec<_>>(),
        "the chain declares Stages {names:?}; ADR 0036 decision 6 with R12 CICD-9112's gates is \
         exactly {CHAIN:?}"
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
            // The one build registry on every control plane. A warehouse on
            // an environment's own registry promotes bytes that environment
            // built for itself, which is the per-environment rebuild R12
            // CICD-9112 prohibits.
            assert!(
                repo_url.starts_with(&format!("{BUILD_REGISTRY}/")),
                "{} subscribes to `{repo_url}`, which is not in the build registry \
                 {BUILD_REGISTRY}; every environment is promoted the digest built there once",
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
fn the_kargo_stages_chain_dev_test_replay_paper_stage_prod_from_one_warehouse() {
    // Promotion order is the whole safety argument for `test` and `stage`:
    // freight reaches `stage` only after `test`, the replay gate and the
    // paper gate each passed it, in that order (R12 CICD-9112). A Stage
    // taking freight directly from the Warehouse skips every link before
    // it, and a Stage naming the wrong predecessor promotes something a
    // different link approved — `stage` reading from `test` would skip both
    // gates while every name in the file still looked right. The same chain
    // on every control plane, because each renders the one base.
    for environment in ENVIRONMENTS {
        let rendered = kargo_rendered(environment);
        let warehouse = of_kind(&rendered, "Warehouse")
            .first()
            .map(Manifest::name)
            .expect("one Warehouse; the subscription test says so");
        let stages = stages_by_name(&rendered);
        let mut previous: Option<&str> = None;
        for stage_name in CHAIN {
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
                        CHAIN.join(" → ")
                    );
                }
            }
            previous = Some(stage_name);
        }
    }
}

/// A promotion template's stage-level variable, by name.
fn stage_var(stage: &Manifest, name: &str) -> Option<String> {
    list_at(&stage.value, &["spec", "promotionTemplate", "spec", "vars"])
        .into_iter()
        .find(|var| text_at(var, &["name"]).as_deref() == Some(name))
        .and_then(|var| text_at(var, &["value"]))
}

#[test]
fn every_link_promotes_the_digest_built_once_in_the_one_registry() {
    // R12 CICD-9112: the same immutable artifact through every link, and a
    // per-environment rebuild prohibited. Until this held, each stage's
    // `registry` named its own environment's repository and `deploy.yml`
    // rebuilt per environment, so a promotion wrote dev's digest under a
    // registry that never held it — and had it held one, it would have been
    // different bytes from a second build. The digest a promotion writes is
    // `imageFrom(warehouseRegistry)`, under the name `registry`; the two
    // being the one build registry is what makes "promoted" mean "the same
    // bytes".
    for environment in ENVIRONMENTS {
        let rendered = kargo_rendered(environment);
        let stages = stages_by_name(&rendered);
        // Premise: the promoting links exist and carry the variables, so
        // the loop below is not vacuously satisfied by stages with none.
        let mut promoting = 0usize;
        for link in CHAIN {
            if link == "prod" {
                continue;
            }
            let stage = &stages[link];
            let steps = list_at(
                &stage.value,
                &["spec", "promotionTemplate", "spec", "steps"],
            );
            let task = steps
                .first()
                .and_then(|step| text_at(step, &["task", "name"]))
                .unwrap_or_default();
            // `paper` runs its automated ceiling check first and records
            // second; the order is pinned by
            // `the_paper_stage_cannot_promote_without_asserting_the_paper_trading_ceiling_first`.
            let expected_task = if link == "paper" {
                assert!(
                    steps
                        .iter()
                        .any(|step| text_at(step, &["task", "name"]).as_deref()
                            == Some("record-gate")),
                    "Stage `paper` records nothing"
                );
                "verify-paper-ceiling"
            } else if GATES.contains(&link) {
                "record-gate"
            } else {
                "promote-cloud-run"
            };
            assert_eq!(
                task,
                expected_task,
                "{} runs `{task}`; a {} runs `{expected_task}`",
                stage.describe(),
                if GATES.contains(&link) {
                    "gate, which has no environment to write,"
                } else {
                    "stage with an environment"
                }
            );
            let warehouse_registry = stage_var(stage, "warehouseRegistry");
            assert_eq!(
                warehouse_registry.as_deref(),
                Some(BUILD_REGISTRY),
                "{} reads freight digests from {warehouse_registry:?}, not the build registry",
                stage.describe()
            );
            if !GATES.contains(&link) {
                let registry = stage_var(stage, "registry");
                assert_eq!(
                    registry.as_deref(),
                    Some(BUILD_REGISTRY),
                    "{} writes the digest under {registry:?}; any registry but the one it was \
                     built in names bytes that are not there, or bytes built a second time",
                    stage.describe()
                );
            }
            promoting += 1;
        }
        assert_eq!(promoting, CHAIN.len() - 1);
    }

    // And the environments' own records name the build registry for the
    // three promoted images, so the first promotion into test, stage or
    // prod does not move an image between registries as a side effect.
    for environment in ENVIRONMENTS {
        let path = format!("{ENVS}/{environment}/kustomization.yaml");
        let text = read(&path);
        for binary in ["qip-api", "qip-fastbrain", "qip-deepbrain"] {
            let expected = format!("newName: {BUILD_REGISTRY}/{binary}");
            assert!(
                text.lines().any(|line| line.trim() == expected),
                "{path} does not name `{BUILD_REGISTRY}/{binary}`; the promoted digest lives \
                 there and nowhere else"
            );
        }
    }
}

#[test]
fn the_replay_and_paper_gates_are_promoted_by_a_person_and_record_what_passed() {
    // The two gates are only gates if nothing walks freight through them on
    // its own. An auto-promotion policy on either turns "a person checked
    // the replay held" into "five minutes passed", and the stage after it
    // would read freight as verified that nobody looked at.
    for environment in ENVIRONMENTS {
        let rendered = kargo_rendered(environment);
        let mut policies: Vec<&Value> = Vec::new();
        for manifest in &rendered {
            find_key(&manifest.value, "promotionPolicies", &mut policies);
        }
        // Premise: the policies were found at all, and dev's is among them.
        let entries: Vec<&Value> = policies
            .iter()
            .flat_map(|list| list.as_array().into_iter().flatten())
            .collect();
        assert!(
            entries
                .iter()
                .any(|policy| text_at(policy, &["stage"]).as_deref() == Some("dev")),
            "{environment}'s chain shows no promotion policy for dev; this test cannot see \
             policies at all"
        );
        for policy in entries {
            let stage = text_at(policy, &["stage"]).unwrap_or_default();
            let enabled = at(policy, &["autoPromotionEnabled"]) == Some(&Value::Bool(true));
            assert!(
                !(GATES.contains(&stage.as_str()) && enabled),
                "a promotion policy promotes the `{stage}` gate automatically on \
                 {environment}'s control plane; a gate nobody looks at is not a gate"
            );
        }

        // The task the gates run writes a record and nothing else.
        let tasks = of_kind(&rendered, "PromotionTask");
        let gate = tasks
            .iter()
            .find(|task| task.name() == "record-gate")
            .unwrap_or_else(|| panic!("{environment}'s chain declares no `record-gate` task"));
        let uses: Vec<String> = list_at(&gate.value, &["spec", "steps"])
            .iter()
            .filter_map(|step| text_at(step, &["uses"]))
            .collect();
        assert!(
            uses.iter().any(|step| step == "yaml-update")
                && uses.iter().any(|step| step == "git-commit")
                && uses.iter().any(|step| step == "git-push"),
            "the record-gate task runs {uses:?}; it must write, commit and push the record"
        );
        assert!(
            !uses
                .iter()
                .any(|step| step == "kustomize-set-image" || step == "argocd-update"),
            "the record-gate task runs {uses:?}; a gate has no environment to change"
        );
    }
    for gate in GATES {
        let record = format!("{GITOPS}/gates/{gate}/freight.yaml");
        assert!(
            repository_root().join(&record).is_file(),
            "{record} does not exist; the `{gate}` gate's task writes there"
        );
    }
}

#[test]
fn the_promotion_commit_skips_ci_so_an_auto_promoted_stage_cannot_feed_itself() {
    // Dev promotes automatically (projectconfig.yaml), the promotion pushes
    // to the default branch, `ci.yml` runs on every push there with no path
    // filter, and `deploy.yml` follows ci and tags each image with the
    // commit it built — a new tag, so new freight, so another promotion.
    // `[skip ci]` in the commit's subject is what breaks that loop; nothing
    // pinned it, and a reworded subject would restart the loop silently.
    // Inferred from configuration: Kargo has never run in this repository.
    for environment in ENVIRONMENTS {
        let rendered = kargo_rendered(environment);
        let tasks = of_kind(&rendered, "PromotionTask");
        let promote = tasks
            .iter()
            .find(|task| task.name() == "promote-cloud-run")
            .unwrap_or_else(|| {
                panic!("{environment}'s chain declares no `promote-cloud-run` task")
            });
        let steps = list_at(&promote.value, &["spec", "steps"]);
        // Premise: this is the task that writes digests, so its commit is the
        // one that reaches the default branch.
        assert!(
            steps
                .iter()
                .any(|step| text_at(step, &["uses"]).as_deref() == Some("kustomize-set-image")),
            "{environment}'s promote-cloud-run task no longer sets images; this test is reading \
             the wrong task"
        );
        let commits: Vec<String> = steps
            .iter()
            .filter(|step| text_at(step, &["uses"]).as_deref() == Some("git-commit"))
            .filter_map(|step| text_at(step, &["config", "message"]))
            .collect();
        assert_eq!(
            commits.len(),
            1,
            "{environment}'s promote-cloud-run task has {} git-commit step(s) with a message; \
             expected exactly one",
            commits.len()
        );
        let subject = commits[0].lines().next().unwrap_or_default();
        // The bracketed token, not the words: `skip ci` without the brackets
        // is not something GitHub reads.
        assert!(
            subject.contains("[skip ci]"),
            "{environment}'s promotion commit subject is `{subject}`; without `[skip ci]` the \
             push re-runs ci, deploy.yml re-tags every image, and dev's auto-promotion promotes \
             its own commit forever"
        );
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

/// Services under `envs/` that are deliberately not catalogue workloads,
/// each with the record that says so.
///
/// Two, and each is a named exception rather than a category. OpenObserve is
/// a vendored image with its own instantiation of `modules/cloudrun` (ADR
/// 0028). The portal is `frontend/portal` — no Rust binary, no autonomy
/// ceiling, no instrument universe, and the only service here a browser
/// reaches — with no `modules/cloudrun` instantiation at all, because the
/// account it runs as already existed (ADR 0018, ADR 0094).
///
/// A service named here is not exempt from anything; it is checked by the
/// test written for it rather than by the parity walk, which compares a
/// manifest against a catalogue entry that does not exist for either of
/// these. `the_portal_is_reachable_only_behind_its_iap_edge_and_holds_only_what_it_needs`
/// below is the portal's, and
/// `openobserve_is_deployed_at_the_reviewed_digest_on_ephemeral_storage_and_answers_no_anonymous_caller`
/// is OpenObserve's.
const NOT_A_CATALOGUE_WORKLOAD: [&str; 2] = ["openobserve", "portal"];

/// The RunServices under one environment, each matched to the catalogue key
/// its name carries. Any RunService that is neither a catalogue workload
/// nor one of `NOT_A_CATALOGUE_WORKLOAD` fails here: an unlisted service is
/// a workload with nothing to be checked against.
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
        if NOT_A_CATALOGUE_WORKLOAD.contains(&key) {
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

/// The volume kinds the RunService CRD admits (Config Connector v1.156.0,
/// byte-identical at upstream `master` on 2026-10-13). `gcs` is not among
/// them, and the API server prunes an unknown kind silently rather than
/// refusing the object.
const ADMITTED_VOLUME_KINDS: [&str; 3] = ["cloudSqlInstance", "emptyDir", "secret"];

/// The kinds a volume entry declares: every key of it but `name`.
fn volume_kinds(volume: &Value) -> Vec<String> {
    volume
        .as_object()
        .map(|map| map.keys().filter(|key| *key != "name").cloned().collect())
        .unwrap_or_default()
}

/// The one bootstrap path the `qip-egress` container's `-c` argument names.
fn sidecar_bootstrap_argument(describe: &str, sidecar: &Value) -> String {
    let args: Vec<String> = list_at(sidecar, &["args"])
        .iter()
        .filter_map(|arg| arg.as_str().map(str::to_string))
        .collect();
    let configs: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(_, arg)| arg.as_str() == "-c")
        .filter_map(|(index, _)| args.get(index + 1))
        .collect();
    assert_eq!(
        configs.len(),
        1,
        "{describe}'s qip-egress container passes `-c` {} time(s) in {args:?}; Envoy takes \
         exactly one bootstrap",
        configs.len()
    );
    configs[0].clone()
}

/// A committed file's volume, in the shape ADR 0036's amendment of 2026-10-13
/// gives every file a RunService reads out of this checkout: `volume` is the
/// `spec.template.volumes` entry named `volume_name`, `slot` the Secret
/// Manager container it must project, `file_name` the one item it must
/// project it as.
///
/// A `secret` volume and no other kind, because the CRD admits
/// `ADMITTED_VOLUME_KINDS` and the API server prunes anything else silently:
/// the manifests carried a `gcs` volume for three weeks, no revision ever
/// received the file, and Argo CD reported the three services OutOfSync
/// forever. Exactly one item, at the file's own name, from `latest`, because
/// a secret volume projects its items flat and the process opens the path it
/// was told and no other — `infra.yml` run 117 (2026-10-13) read Envoy dying
/// on `Invalid path` and the fast brain exiting 78 off the containers. Mode
/// 292 (0444) rather than a credential's 256 (0400): the bytes are a reviewed
/// file, and both images run as a non-root user that 0400 would lock out.
fn assert_committed_file_volume(
    describe: &str,
    volume: &Value,
    volume_name: &str,
    slot: &str,
    file_name: &str,
) {
    let kinds = volume_kinds(volume);
    assert_eq!(
        kinds,
        vec!["secret".to_string()],
        "{describe}'s `{volume_name}` volume is of kind(s) {kinds:?}; the RunService CRD admits \
         only {ADMITTED_VOLUME_KINDS:?}, the API server prunes any other kind silently, and a \
         `secret` volume is the one of the three that carries a committed file (ADR 0036, \
         amendment of 2026-10-13)"
    );
    let secret = at(volume, &["secret"]).expect("asserted present above");
    let referenced = text_at(secret, &["secretRef", "external"])
        .or_else(|| text_at(secret, &["secretRef", "name"]))
        .unwrap_or_default();
    assert_eq!(
        referenced, slot,
        "{describe}'s `{volume_name}` volume projects secret `{referenced}` rather than `{slot}`, \
         the container Terraform creates beside every other slot and infra.yml's seed step \
         writes the committed bytes into"
    );
    assert_eq!(
        text_at(secret, &["defaultMode"]).as_deref(),
        Some("292"),
        "{describe}'s `{volume_name}` volume has defaultMode {:?} rather than 292 (0444): a \
         reviewed file the image's non-root user must be able to read, not a credential",
        text_at(secret, &["defaultMode"])
    );
    let items = list_at(secret, &["items"]);
    assert_eq!(
        items.len(),
        1,
        "{describe}'s `{volume_name}` volume projects {} item(s); the file is one item at \
         `{file_name}`, and a secret volume projects its items flat, so a second item is a \
         second file the process was never told about",
        items.len()
    );
    let item = items[0];
    assert_eq!(
        text_at(item, &["path"]).as_deref(),
        Some(file_name),
        "{describe}'s `{volume_name}` volume projects its item at {:?} rather than \
         `{file_name}`, the name the process opens",
        text_at(item, &["path"])
    );
    assert_eq!(
        text_at(item, &["versionRef", "external"]).as_deref(),
        Some("latest"),
        "{describe}'s `{volume_name}` volume projects `{file_name}` from version {:?} rather \
         than `latest`, the version the seed step writes whenever the committed bytes change",
        text_at(item, &["versionRef", "external"])
    );
    assert_eq!(
        text_at(item, &["mode"]).as_deref(),
        Some("292"),
        "{describe}'s `{volume_name}` volume projects `{file_name}` with mode {:?} rather than \
         292 (0444): a reviewed file the image's non-root user must be able to read, not a \
         credential",
        text_at(item, &["mode"])
    );
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
    let mut committed_volumes_checked = 0usize;
    for environment in environment_directories() {
        let project = tfvars_value(&environment, "project_id")
            .unwrap_or_else(|| panic!("{environment}'s tfvars name no project_id"));
        for (key, entry, service, siblings) in run_services(&environment) {
            let Some(entry) = entry else {
                // A `NOT_A_CATALOGUE_WORKLOAD` service. OpenObserve carries
                // ADR 0033's authenticated posture and ADR 0031's secret_env; the portal
                // carries ADR 0094's load-balancer ingress and runs as an
                // account this module never created. Neither has a catalogue
                // entry to be compared against, and each has a test of its
                // own. The one property that still applies to both is
                // asserted here: they are released, never destroyed.
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
                // The identity is not repeated here: `assert_eq!` prints both
                // sides on failure, and the static-analysis gate reads a
                // service account formatted into a message as cleartext
                // logging of it (ADR 0117's CodeQL gate).
                "{describe} does not run as the workload's own account"
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
                repository.starts_with(&format!("{BUILD_REGISTRY}/")),
                "{describe} pulls `{image}` from a repository other than the build registry \
                 {BUILD_REGISTRY}, the one deploy.yml builds every commit into once and \
                 Kargo promotes from"
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
                        // The slot name is not repeated: the gate reads it as a
                        // secret logged in clear (ADR 0117), and `{file_name}`
                        // with `{describe}` already names the one mount this is.
                        "{describe} projects `{file_name}` with a mode other than 256 \
                         (0400): readable by the process and nobody else"
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

            // The committed configuration: the universe, projected by a
            // `secret` volume at exactly the directory QIP_UNIVERSE_PATH
            // names, which the walk above showed is under /etc/qip. The
            // directory is derived from the variable rather than written
            // here, because it is a hash of the file's bytes and changes
            // whenever they do; the parity test recomputes that hash, and a
            // literal here would be a second copy of it. This was a
            // read-only `gcs` mount of the workload's bucket at /etc/qip,
            // which the RunService CRD cannot carry (ADR 0036, amendment of
            // 2026-10-13); `assert_committed_file_volume` says what it is now.
            let universe_path = env
                .iter()
                .find(|(name, _, _)| name == "QIP_UNIVERSE_PATH")
                .and_then(|(_, value, _)| value.clone())
                .unwrap_or_else(|| panic!("{describe} sets no QIP_UNIVERSE_PATH"));
            let (universe_directory, universe_file) = universe_path
                .rsplit_once('/')
                .filter(|(directory, file)| !directory.is_empty() && !file.is_empty())
                .unwrap_or_else(|| {
                    panic!(
                        "{describe} sets QIP_UNIVERSE_PATH={universe_path}, which does not name \
                         a file inside a directory"
                    )
                });
            assert_eq!(
                mounts.get("config-files").map(String::as_str),
                Some(universe_directory),
                "{describe} does not mount `config-files` at {universe_directory}, the directory \
                 QIP_UNIVERSE_PATH={universe_path} names; it mounts {mounts:?}. A secret volume \
                 projects its items flat, so the mount point is the hash directory itself and \
                 not /etc/qip above it"
            );
            let config_volume = volumes
                .get("config-files")
                .unwrap_or_else(|| panic!("{describe} declares no `config-files` volume"));
            assert_committed_file_volume(
                &describe,
                config_volume,
                "config-files",
                &format!("qip-universe-catalogue-{environment}"),
                universe_file,
            );
            committed_volumes_checked += 1;

            // The Envoy bootstrap the same way, wherever there is a sidecar to
            // read it: the one item is the name the sidecar's `-c` opens and
            // the mount is that name's directory. A sidecar without the
            // volume dies on `Invalid path` (infra.yml run 117, 2026-10-13);
            // a volume without the sidecar is a mount nothing reads.
            let sidecar = list_at(&service.value, &["spec", "template", "containers"])
                .into_iter()
                .find(|other| text_at(other, &["name"]).as_deref() == Some("qip-egress"));
            let bootstrap_volume = volumes.get("egress-bootstrap");
            assert_eq!(
                sidecar.is_some(),
                bootstrap_volume.is_some(),
                "{describe} {} and {}; the sidecar reads nothing but that volume and the volume \
                 feeds nothing but the sidecar",
                if sidecar.is_some() {
                    "carries a qip-egress container"
                } else {
                    "carries no qip-egress container"
                },
                if bootstrap_volume.is_some() {
                    "declares an `egress-bootstrap` volume"
                } else {
                    "declares no `egress-bootstrap` volume"
                }
            );
            if let (Some(sidecar), Some(bootstrap_volume)) = (sidecar, bootstrap_volume) {
                let bootstrap_path = sidecar_bootstrap_argument(&describe, sidecar);
                let (bootstrap_directory, bootstrap_file) = bootstrap_path
                    .rsplit_once('/')
                    .filter(|(directory, file)| !directory.is_empty() && !file.is_empty())
                    .unwrap_or_else(|| {
                        panic!(
                            "{describe}'s qip-egress container starts with `-c {bootstrap_path}`, \
                             which does not name a file inside a directory"
                        )
                    });
                let sidecar_mounts: BTreeMap<String, String> = list_at(sidecar, &["volumeMounts"])
                    .iter()
                    .map(|mount| {
                        (
                            text_at(mount, &["name"]).unwrap_or_default(),
                            text_at(mount, &["mountPath"]).unwrap_or_default(),
                        )
                    })
                    .collect();
                assert_eq!(
                    sidecar_mounts.get("egress-bootstrap").map(String::as_str),
                    Some(bootstrap_directory),
                    "{describe}'s qip-egress container does not mount `egress-bootstrap` at \
                     {bootstrap_directory}, the directory its `-c {bootstrap_path}` names; it \
                     mounts {sidecar_mounts:?}"
                );
                assert_committed_file_volume(
                    &describe,
                    bootstrap_volume,
                    "egress-bootstrap",
                    &format!("qip-egress-bootstrap-{environment}"),
                    bootstrap_file,
                );
                committed_volumes_checked += 1;
            }

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
    assert_eq!(
        committed_volumes_checked,
        12 + 2 * 4,
        "{committed_volumes_checked} committed-file volumes were checked; a universe in each \
         of twelve services and a bootstrap beside each of eight sidecars is twenty, and a walk \
         that found fewer skipped a mount a revision will die on"
    );
    assert!(
        overrides_seen.get() >= 1,
        "no image was resolved through an `images:` transformer; the promotion writes the \
         digest there, and a check that never reads it is checking a placeholder"
    );
}

const NETWORK_MODULE: &str = "infrastructure/terraform/modules/network";
const TRUST_ZONES_MODULE: &str = "infrastructure/terraform/modules/trust-zones/main.tf";
const TERRAFORM_ROOT: &str = "infrastructure/terraform/main.tf";

/// The `key = value` lines of an HCL block, whitespace collapsed and a quoted
/// key or value unquoted.
fn hcl_assignments(block: &str) -> BTreeMap<String, String> {
    block
        .lines()
        .map(collapsed)
        .filter_map(|line| {
            let (key, value) = line.split_once(" = ")?;
            Some((
                key.trim_matches('"').to_string(),
                value.trim_matches('"').to_string(),
            ))
        })
        .collect()
}

/// The role inside a `google_compute_network.vpc["<role>"]` reference.
fn vpc_role(reference: &str) -> Option<String> {
    reference
        .split("google_compute_network.vpc[\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(str::to_string)
}

/// Which VPC network Terraform creates each subnetwork on, read from the
/// modules that create them rather than restated here: `modules/network`
/// names the networks and creates the console's egress subnet, and
/// `modules/trust-zones` creates one subnet per zone on whichever network the
/// root wires to that zone's variable.
struct TerraformNetworks {
    /// `local.networks`: a role (`reflex`, `fabric`, `service`) to the
    /// network's name, `${var.environment}` unexpanded.
    names: BTreeMap<String, String>,
    /// `modules/network`'s outputs: an output's name to the role of the
    /// network it exports.
    exported: BTreeMap<String, String>,
    /// The root's `module "trust_zones"` arguments: a variable of that module
    /// to the `modules/network` output it is given.
    wired: BTreeMap<String, String>,
    /// `local.zone_network`: a zone to the variable naming its network.
    zones: BTreeMap<String, String>,
    /// The variable `lookup` falls back to for a zone the map does not name.
    zone_fallback: String,
    /// The console's egress subnet: its name, unexpanded, and its network's role.
    console_egress: (String, String),
}

impl TerraformNetworks {
    fn read() -> Self {
        let network = without_comments(&read(&format!("{NETWORK_MODULE}/main.tf")));
        let names = hcl_assignments(&block_under(&network, "networks = {"));
        assert!(
            names.contains_key("service"),
            "modules/network's `local.networks` names no `service` network ({names:?}); the \
             three-VPC split this test reads (GCP-009) has moved"
        );

        let outputs = without_comments(&read(&format!("{NETWORK_MODULE}/outputs.tf")));
        let exported: BTreeMap<String, String> = outputs
            .lines()
            .filter_map(|line| {
                let name = line.strip_prefix("output \"")?.strip_suffix("\" {")?;
                let role = vpc_role(&block_under(&outputs, line.trim()))?;
                Some((name.to_string(), role))
            })
            .collect();

        let root = without_comments(&read(TERRAFORM_ROOT));
        let wired: BTreeMap<String, String> =
            hcl_assignments(&block_under(&root, "module \"trust_zones\" {"))
                .into_iter()
                .filter_map(|(variable, value)| {
                    let output = value.strip_prefix("module.network.")?;
                    Some((variable, output.to_string()))
                })
                .collect();

        let trust_zones = without_comments(&read(TRUST_ZONES_MODULE));
        let zones = hcl_assignments(&block_under(&trust_zones, "zone_network = {"));
        let subnet = hcl_assignments(&block_under(
            &trust_zones,
            "resource \"google_compute_subnetwork\" \"zone\" {",
        ));
        assert_eq!(
            subnet.get("name").map(String::as_str),
            Some("qip-${var.environment}-tz-${each.key}"),
            "modules/trust-zones no longer names a zone's subnet qip-<env>-tz-<zone>, which is \
             how this test finds the zone a manifest's subnetwork belongs to"
        );
        let zone_fallback = subnet
            .get("network")
            .and_then(|network| {
                network
                    .strip_prefix("lookup(local.zone_network, each.key, ")?
                    .strip_suffix(')')
            })
            .unwrap_or_else(|| {
                panic!(
                    "modules/trust-zones no longer places a zone's subnet by \
                     `lookup(local.zone_network, …)`: {subnet:?}"
                )
            })
            .to_string();

        let console = hcl_assignments(&block_under(
            &network,
            "resource \"google_compute_subnetwork\" \"console_egress\" {",
        ));
        let console_egress = (
            console.get("name").cloned().unwrap_or_default(),
            console
                .get("network")
                .and_then(|network| vpc_role(network))
                .unwrap_or_default(),
        );
        assert!(
            !console_egress.0.is_empty() && !console_egress.1.is_empty(),
            "modules/network's console egress subnet names no name or no network: {console:?}"
        );

        Self {
            names,
            exported,
            wired,
            zones,
            zone_fallback,
            console_egress,
        }
    }

    /// The name of the network Terraform creates `subnetwork` on in
    /// `environment`, or `None` for a subnetwork no module here creates.
    fn network_of(&self, environment: &str, subnetwork: &str) -> Option<String> {
        let expand = |template: &str| template.replace("${var.environment}", environment);
        let role = if subnetwork == expand(&self.console_egress.0) {
            self.console_egress.1.clone()
        } else {
            let zone = subnetwork.strip_prefix(&format!("qip-{environment}-tz-"))?;
            let variable = self.zones.get(zone).unwrap_or(&self.zone_fallback);
            let output = self.wired.get(variable.strip_prefix("var.")?)?;
            self.exported.get(output)?.clone()
        };
        self.names.get(&role).map(|name| expand(name))
    }
}

/// The plain switches of a catalogue entry: each `var.x == null ? {} : {` arm
/// of its `env` merge whose every line is `QIP_Y = var.x`, as `(x, QIP_Y)`. An
/// arm that transforms the value — the deep brain's connector `join`s a list
/// and carries a second key — is left out, because its rendering is not the
/// tfvars value and comparing the two would assert a coincidence.
fn catalogue_plain_switches(body: &str) -> Vec<(String, String)> {
    /// The arm being read: its root variable, the keys it renders from that
    /// variable unchanged, and whether every line so far has been one.
    struct Arm {
        variable: String,
        keys: Vec<(String, String)>,
        plain: bool,
    }
    let mut switches = Vec::new();
    let mut arm: Option<Arm> = None;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(condition) = trimmed.strip_suffix("== null ? {} : {") {
            let variable = condition.trim().strip_prefix("var.").map(str::to_string);
            arm = variable.map(|variable| Arm {
                variable,
                keys: Vec::new(),
                plain: true,
            });
            continue;
        }
        let Some(Arm {
            variable,
            keys,
            plain,
        }) = arm.as_mut()
        else {
            continue;
        };
        if trimmed == "}," {
            if *plain && !keys.is_empty() {
                switches.append(keys);
            }
            arm = None;
            continue;
        }
        match trimmed.split_once('=') {
            Some((key, value))
                if key.trim().starts_with("QIP_") && value.trim() == format!("var.{variable}") =>
            {
                keys.push((variable.clone(), key.trim().to_string()));
            }
            _ => *plain = false,
        }
    }
    switches
}

/// A switch an environment commits in its tfvars reaches the service, and one
/// it leaves null does not.
///
/// The parity walk above skips every conditional variable on purpose — an
/// environment that leaves the root variable null renders none of them — and
/// in skipping them it stopped checking them at all. A switch set in
/// `environments/dev/terraform.tfvars` and never written into
/// `gitops/envs/dev/` passed every suite here: Terraform plans the value,
/// Argo CD applies the manifest, and the running binary reads its own
/// default, so the tfvars line reads as configuration the process never
/// received. The converse is the same lie the other way: a manifest carrying
/// a switch its tfvars leaves null runs a value no reviewed variable set.
#[test]
fn every_switch_an_environment_commits_reaches_its_run_service_with_the_committed_value() {
    let mut committed = 0usize;
    let mut withheld = 0usize;
    for environment in environment_directories() {
        for (key, entry, service, _) in run_services(&environment) {
            let Some(entry) = entry else { continue };
            let describe = service.describe();
            let env = container_env(workload_container(&service, &key));
            for (variable, name) in catalogue_plain_switches(&entry) {
                let set = tfvars_value(&environment, &variable).filter(|value| value != "null");
                let rendered = env
                    .iter()
                    .find(|(entry_name, _, _)| *entry_name == name)
                    .map(|(_, value, _)| value.clone());
                match set {
                    Some(value) => {
                        committed += 1;
                        assert_eq!(
                            rendered,
                            Some(Some(value.clone())),
                            "{environment}'s tfvars set `{variable} = \"{value}\"`, which the \
                             catalogue renders as {name}, and {describe} does not carry \
                             {name}={value}; the binary would run its own default under a \
                             tfvars line that says otherwise"
                        );
                    }
                    None => {
                        withheld += 1;
                        assert!(
                            rendered.is_none(),
                            "{describe} sets {name}, and {environment}'s tfvars leave \
                             `{variable}` null; the value runs without a reviewed variable \
                             behind it"
                        );
                    }
                }
            }
        }
    }
    // Both halves have a case, or the walk proved nothing: no committed switch
    // means the arm parse found nothing to compare, and no withheld one means
    // the absence half never ran.
    assert!(
        committed > 0,
        "no environment commits a plain catalogue switch; the arm parse has drifted from \
         catalogue.tf's shape"
    );
    assert!(
        withheld > 0,
        "every plain switch is committed in every environment; the absence half never ran"
    );
}

/// Every RunService's network interface names the VPC network Terraform
/// created its subnetwork on.
///
/// Cloud Run refuses a revision whose network and subnetwork disagree, and
/// refuses it at deploy time — not at plan, not at sync. `infra.yml` run 113
/// (`apps`, dev) is that refusal: "Direct VPC subnetwork qip-dev-tz-cognition
/// do not belong to network qip-dev". GCP-009 (`560d953a`) split the single
/// VPC, then named `qip-<env>`, into reflex, fabric and service networks and
/// renamed the one every Cloud Run zone sits on to `qip-<env>-service`; the
/// manifests, written on a branch that predated the split, kept the old name.
/// Argo CD applied every one, Config Connector accepted every one, and no
/// revision of deepbrain, fastbrain or OpenObserve could start; the api and the
/// portal carried the same pair behind a missing secret. Nothing in this suite
/// read the network: the parity walk checks that the subnetwork names the zone
/// and stops there, and it skips the portal and OpenObserve entirely.
///
/// So the expected network is derived from the Terraform that creates the
/// subnetwork — the zone's entry in `zone_network`, the variable the root wires
/// it to, the output that variable is given and the name `modules/network`
/// gives that network — and checked on every RunService in every environment.
/// An interface naming no network is skipped rather than failed: Cloud Run then
/// looks the network up from the subnetwork, and there is nothing to disagree.
#[test]
fn every_run_service_names_the_network_terraform_put_its_subnetwork_on() {
    let terraform = TerraformNetworks::read();
    let mut checked: BTreeSet<String> = BTreeSet::new();
    for environment in environment_directories() {
        let project = tfvars_value(&environment, "project_id")
            .unwrap_or_else(|| panic!("{environment}'s tfvars name no project_id"));
        let before = checked.len();
        for (_, _, service, _) in run_services(&environment) {
            let describe = service.describe();
            for interface in list_at(
                &service.value,
                &["spec", "template", "vpcAccess", "networkInterfaces"],
            ) {
                let Some(network) = text_at(interface, &["networkRef", "external"]) else {
                    continue;
                };
                let subnetwork =
                    text_at(interface, &["subnetworkRef", "external"]).unwrap_or_else(|| {
                        panic!("{describe} names network `{network}` and no subnetwork")
                    });
                let subnetwork = subnetwork.rsplit('/').next().unwrap_or_default();
                let expected = terraform
                    .network_of(&environment, subnetwork)
                    .unwrap_or_else(|| {
                        panic!(
                            "{describe} egresses through `{subnetwork}`, which neither \
                             modules/trust-zones nor modules/network creates in {environment}"
                        )
                    });
                assert_eq!(
                    network,
                    format!("projects/{project}/global/networks/{expected}"),
                    "{describe} names network `{network}`, but Terraform creates `{subnetwork}` \
                     on `{expected}`. Cloud Run refuses that pair when the revision is \
                     created — run 113's \"Direct VPC subnetwork … do not belong to network\" \
                     — after Argo CD and Config Connector have both accepted it"
                );
                checked.insert(service.name());
            }
        }
        assert!(
            checked.len() > before,
            "no RunService under {ENVS}/{environment} names a network, so nothing there was \
             checked; if the manifests now name only subnetworks, this test's premise has \
             changed and it should say so"
        );
    }
    // The two the parity walk skips are the two most worth reading: run 113
    // could not start either of them.
    for service in ["qip-dev-portal", "qip-dev-openobserve"] {
        assert!(
            checked.contains(service),
            "{service} was not among the services whose network was checked: {checked:?}"
        );
    }
}

/// Every secret a RunService references is one `infra.yml`'s seeding step
/// either writes or names, and that step runs on the action that starts
/// revisions.
///
/// The step keeps three lists: platform-internal keys it writes a first
/// version into, owner-supplied credentials it refuses to invent and names in
/// a warning instead, and — since ADR 0036's amendment of 2026-10-13 —
/// committed files it writes from the checkout as `<slot>=<path>` pairs
/// whenever the file's digest differs from the latest version's. A secret on
/// no list is the silent case — Terraform creates the container, nothing
/// writes a version, no warning names it, and the first anybody hears of it
/// is a revision that will not start. That is `infra.yml` run 113: the portal
/// mounted `qip-session-secret-dev`, which `scripts/deploy-frontends.sh` had
/// seeded until the portal became a manifest (ADR 0094) and which nothing
/// seeded afterwards. And the step ran only on `up`, so even a listed key
/// would have waited for the next apply while `apps` failed on it — which is
/// how run 80 died on the envelope key. A slot on two lists is the opposite
/// failure: seeded twice, by two rules that disagree about what it holds.
#[test]
fn every_secret_a_run_service_references_is_seeded_or_named_by_the_seeding_step() {
    let workflow = read(INFRA_WORKFLOW);
    let step = workflow
        .split("- name: seed any credential that has no version")
        .nth(1)
        .expect("infra.yml has the credential seeding step");
    let step = step
        .split("\n      - name:")
        .next()
        .expect("the step is followed by another step or the end of the job");
    let condition = step
        .lines()
        .find_map(|line| line.trim().strip_prefix("if: "))
        .unwrap_or_default();
    assert!(
        condition.contains("inputs.action == 'apps'"),
        "the seeding step runs under `{condition}`, not on `apps` — the action that starts \
         revisions, and the one runs 80 and 113 both died in on an empty container"
    );
    let list = |name: &str| -> Vec<String> {
        step.split(&format!("{name}=\""))
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| panic!("the seeding step declares no `{name}` list"))
            .split_whitespace()
            .filter(|word| *word != "\\")
            .map(str::to_string)
            .collect()
    };
    let generated: BTreeSet<String> = list("generated").into_iter().collect();
    let owner: BTreeSet<String> = list("owner_supplied").into_iter().collect();
    // The third list is `<slot>=<path>` pairs: the slot is what the loop
    // seeds and the path is what it seeds it from. The step hashes the path
    // and fails on a missing one, on purpose, so a path that is not in the
    // checkout fails here first rather than in every `apps` afterwards.
    let mut committed: BTreeSet<String> = BTreeSet::new();
    for pair in list("committed") {
        let (slot, path) = pair.split_once('=').unwrap_or_else(|| {
            panic!(
                "the seeding step's `committed` list holds `{pair}`, which is not a \
                 `<slot>=<path>` pair; the loop reads the slot before the `=` and seeds it \
                 from the path after"
            )
        });
        assert!(
            !slot.is_empty() && !path.is_empty(),
            "the seeding step's `committed` list holds `{pair}`, whose slot or path is empty"
        );
        assert!(
            repository_root().join(path).is_file(),
            "the seeding step's `committed` list seeds `{slot}` from `{path}`, which is not a \
             file in this checkout; the step hashes that path and fails on a missing one, so \
             every `apps` would die there"
        );
        assert!(
            committed.insert(slot.to_string()),
            "the seeding step's `committed` list names `{slot}` twice"
        );
    }
    assert!(
        !committed.is_empty(),
        "the seeding step's `committed` list is empty; the universe and the Envoy bootstrap \
         reach a revision only through it (ADR 0036, amendment of 2026-10-13)"
    );
    // No slot on two lists. Each loop seeds by its own rule — random bytes,
    // nothing, the checkout's file — and a slot on two of them is seeded
    // twice by rules that disagree about what it holds, so which bytes a
    // revision reads depends on the order the loops happened to run in.
    for (first_name, first, second_name, second) in [
        ("generated", &generated, "owner_supplied", &owner),
        ("generated", &generated, "committed", &committed),
        ("owner_supplied", &owner, "committed", &committed),
    ] {
        let shared: Vec<&String> = first.intersection(second).collect();
        assert!(
            shared.is_empty(),
            "the seeding step lists {shared:?} on both `{first_name}` and `{second_name}`; a \
             slot on two lists is seeded twice by two rules that disagree about what it holds"
        );
    }

    let mut referenced = 0usize;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for environment in environment_directories() {
        for (_, _, service, _) in run_services(&environment) {
            let describe = service.describe();
            let mut references = Vec::new();
            find_key(&service.value, "secretRef", &mut references);
            for reference in references {
                let slot = text_at(reference, &["external"])
                    .or_else(|| text_at(reference, &["name"]))
                    .unwrap_or_else(|| panic!("{describe} has a secretRef naming no secret"));
                let slot = slot.rsplit('/').next().unwrap_or_default();
                // The step seeds `<name>-<environment>`; a reference without
                // that suffix is a secret the step could not reach either way.
                let base = slot
                    .strip_suffix(&format!("-{environment}"))
                    .unwrap_or_else(|| {
                        panic!(
                            "{describe} references `{slot}`, which lacks the \
                             `-{environment}` suffix the seeding step appends, so no list \
                             there can cover it"
                        )
                    });
                let lists: Vec<&str> = [
                    ("generated", generated.contains(base)),
                    ("owner_supplied", owner.contains(base)),
                    ("committed", committed.contains(base)),
                ]
                .into_iter()
                .filter_map(|(name, listed)| listed.then_some(name))
                .collect();
                assert_eq!(
                    lists.len(),
                    1,
                    "{describe} references `{slot}`, and `{base}` is on {} of the seeding \
                     step's three lists ({lists:?}) rather than exactly one. On none, nothing \
                     writes it a version and no warning names it, so its revision cannot start \
                     and nothing says why until Cloud Run does; on two, it is seeded twice by \
                     rules that disagree. A platform-internal key belongs on `generated`; a \
                     credential for an account somebody else holds belongs on `owner_supplied`; \
                     a file in this repository belongs on `committed` as `<slot>=<path>`",
                    lists.len()
                );
                referenced += 1;
                seen.insert(base.to_string());
            }
        }
    }
    // Premise: the walk found references, including the one that started
    // this test and the two the third list exists for, so a parse that
    // stopped finding `secretRef` cannot pass it.
    assert!(
        referenced >= 30,
        "only {referenced} secret references were found across the RunServices"
    );
    assert!(
        seen.contains("qip-session-secret"),
        "the portal's session key was not among the references checked: {seen:?}"
    );
    for slot in ["qip-universe-catalogue", "qip-egress-bootstrap"] {
        assert!(
            seen.contains(slot),
            "`{slot}` was not among the references checked: {seen:?}; the committed files \
             reach a revision only as that secret's versions (ADR 0036, amendment of \
             2026-10-13)"
        );
    }
}

#[test]
fn every_catalogue_workload_egresses_all_traffic_through_the_vpc_and_portal_uses_private_ranges_only_by_design()
 {
    // ADR 0036 decision 4. Every catalogue workload sends all traffic through
    // the VPC, through its zone's subnet with its zone's firewall tag, so the
    // zone's rules are what apply. The portal is an exception, by architectural
    // design: its only route into the VPC is to the API, and the zone's
    // deny-egress rule on its tag enforces that. A portal sending all traffic
    // through the VPC would also send Identity Platform and every other Google
    // API call through it, a wider hole than the rule was written to cover. So
    // the portal uses PRIVATE_RANGES_ONLY, restricting egress to RFC1918
    // private ranges, and the zone rules further restrict what is permitted
    // (infrastructure/gitops/envs/dev/portal.yaml, lines 77–82).
    //
    // This test pins that design: every catalogue workload uses ALL_TRAFFIC,
    // the portal uses PRIVATE_RANGES_ONLY, and nothing else carries an
    // unexpected configuration. See `.claude/rules/domains/infrastructure.md`
    // and `docs/blueprint/assessment/SEC-b3.json` (SEC-052).
    let mut catalogue_workloads_checked = 0usize;
    let mut portal_checked = false;
    for environment in environment_directories() {
        for (key, entry, service, _) in run_services(&environment) {
            let describe = service.describe();
            let egress_config =
                text_at(&service.value, &["spec", "template", "vpcAccess", "egress"]);
            if let Some(_entry) = entry {
                // Catalogue workload: must use ALL_TRAFFIC
                assert_eq!(
                    egress_config.as_deref(),
                    Some("ALL_TRAFFIC"),
                    "{describe} is a catalogue workload and must send all traffic through the VPC; \
                     it currently has egress configured as {:?}",
                    egress_config
                );
                catalogue_workloads_checked += 1;
            } else if key == "portal" {
                // Portal: architectural exception, uses PRIVATE_RANGES_ONLY
                assert_eq!(
                    egress_config.as_deref(),
                    Some("PRIVATE_RANGES_ONLY"),
                    "{describe} is the portal and must use PRIVATE_RANGES_ONLY to restrict egress \
                     to the VPC (to the API only, not to Google services); it currently has egress \
                     configured as {:?}. See portal.yaml lines 77–82 for the architectural reason.",
                    egress_config
                );
                portal_checked = true;
            } else if key == "openobserve" {
                // OpenObserve carries ADR 0033's authenticated posture and has its
                // own configuration. It should also route through the VPC, but
                // may have different egress configuration. For now, just verify
                // it has some egress configuration.
                assert!(
                    egress_config.is_some(),
                    "{describe} (openobserve) has no vpcAccess.egress configured"
                );
            }
        }
    }
    assert_eq!(
        catalogue_workloads_checked, 12,
        "expected 12 catalogue workloads (3 per environment × 4 environments) but checked {catalogue_workloads_checked}"
    );
    assert!(
        portal_checked,
        "portal service was not found across all environments; the test premise that it exists is broken"
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
            // A Gateway and its routes publish a controller outside the
            // cluster, which this test refused outright until 2026-09-20.
            // The refusal is now conditional, and the condition is checked
            // below rather than here: **a published backend must be behind
            // Identity-Aware Proxy.**
            //
            // The old blanket ban was the right default and the wrong
            // invariant. It said "nothing is published", which stopped
            // being true the moment an operator needed to see what Argo CD
            // was reconciling, and a ban that has to be lifted teaches the
            // next reader that the whole check is negotiable. What actually
            // matters is that Google decides who may reach these services
            // before a request enters the VPC — and nothing enforced that,
            // so deleting one `GCPBackendPolicy` would have published a
            // GitOps control plane nakedly with every test still green.
            //
            // `Ingress`, `ManagedCertificate` and `FrontendConfig` stay
            // refused. They are the older path to the same load balancer and
            // this platform does not use them, so admitting them would widen
            // the surface for nothing.
            "Ingress" | "ManagedCertificate" | "FrontendConfig" => {
                panic!(
                    "{} publishes a controller outside the cluster by a path this platform does \
                     not use; the Gateway API is the one that is checked for IAP",
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

    // Publication implies IAP, and this is where that is enforced.
    //
    // A `GCPBackendPolicy` with `iap.enabled: true` is what makes Google
    // check the caller against `roles/iap.httpsResourceAccessor` before the
    // request is forwarded into the VPC at all. Without one, the Gateway is
    // a plain load balancer and the only thing between a stranger and a
    // controller that can reconcile arbitrary manifests into this cluster is
    // that controller's own login page.
    //
    // So every backend any HTTPRoute names must carry such a policy. The
    // check is written this way round — over routes, requiring a policy —
    // because the reverse would pass a route somebody added without one.
    let manifests = manifests_under(GITOPS);
    let protected: Vec<String> = manifests
        .iter()
        .filter(|manifest| manifest.kind() == "GCPBackendPolicy")
        .filter(|manifest| {
            text_at(&manifest.value, &["spec", "default", "iap", "enabled"]).as_deref()
                == Some("true")
        })
        .filter_map(|manifest| text_at(&manifest.value, &["spec", "targetRef", "name"]))
        .collect();
    let mut published = 0usize;
    for manifest in manifests
        .iter()
        .filter(|manifest| manifest.kind() == "HTTPRoute")
    {
        for backend in list_at(&manifest.value, &["spec", "rules"])
            .into_iter()
            .flat_map(|rule| list_at(rule, &["backendRefs"]))
        {
            let name = text_at(backend, &["name"]).unwrap_or_default();
            assert!(
                protected.contains(&name),
                "{} routes to `{name}`, which no GCPBackendPolicy protects with `iap.enabled: \
                 true`; that publishes a GitOps controller to the internet with nothing but its \
                 own login page in front of it",
                manifest.describe()
            );
            published += 1;
        }
    }
    // A route with no backend at all is the redirect, which carries no
    // traffic to a controller and needs no policy. But if *nothing* is
    // published, the loop above asserted nothing, and this test would pass
    // on a tree where somebody had deleted every policy and every route
    // together. Say which state we are in rather than being silently vacuous.
    if published == 0 {
        assert!(
            protected.is_empty(),
            "GCPBackendPolicy objects exist but no HTTPRoute routes to any backend; either the \
             routes were removed and the policies left behind, or this test is checking nothing"
        );
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

/// Images `deploy.yml` builds and its attestor signs that are not workspace
/// binaries: the console, from `infrastructure/docker/portal.Dockerfile`
/// (ADR 0094). Admitted as `attested` on the same terms as a catalogue
/// binary, and only while `deploy.yml`'s build matrix still names each one —
/// `every_pipeline_built_image_is_still_in_deploy_yml_s_build_matrix` holds
/// that, so an image the pipeline stopped building cannot stay admitted by
/// this list. It held nothing until 2026-10-13, when deploy.yml run 268
/// produced the portal's first attested digest and dev pinned it.
const PIPELINE_BUILT_IMAGES: [&str; 1] = ["qip-portal"];

/// Whether an image reference names one of `PIPELINE_BUILT_IMAGES`, by the
/// last component of its repository path, matched whole.
fn pipeline_built(image: &str) -> bool {
    let (repository, _) = split_reference(image);
    let last = repository.rsplit('/').next().unwrap_or(&repository);
    PIPELINE_BUILT_IMAGES.contains(&last)
}

#[test]
fn every_pipeline_built_image_is_still_in_deploy_yml_s_build_matrix() {
    // The failure this prevents: an image admitted as attested because a
    // list says the pipeline builds it, after the pipeline stopped building
    // it — a digest nobody will ever re-sign, admitted on a promise.
    let deploy = read(DEPLOY_WORKFLOW);
    for image in PIPELINE_BUILT_IMAGES {
        assert!(
            deploy
                .lines()
                .any(|line| line.trim() == format!("- binary: {image}")
                    || line.trim() == format!("- {image}")),
            "PIPELINE_BUILT_IMAGES admits {image} as attested and {DEPLOY_WORKFLOW}'s build \
             matrix no longer names it"
        );
    }
}

/// Whether an image reference is one the platform's attestor could have
/// signed: a catalogue binary the pipeline built, an image the pipeline
/// builds beside them, or a mirror of a line in the vendored list at that
/// line's digest. Returns which, or the refusal.
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
    if trading_binary_of(image).is_some() || pipeline_built(image) {
        return Ok("attested");
    }
    let mirrored = vendored.iter().any(|(source_digest, destination)| {
        *source_digest == digest && repository.ends_with(&format!("/{destination}"))
    });
    if mirrored {
        return Ok("mirrored");
    }
    // The second legitimate path, and the reason it is second rather than an
    // exception: an upstream image can carry a finding with no upstream
    // remedy, and `infrastructure/egress/derived-images.txt` declares the
    // ones this pipeline patches instead. A derived image's digest cannot
    // appear in the mirror list, because it is produced by the build rather
    // than copied — what makes it admissible is that `vendor.yml` builds it
    // from a reviewed base, scans it with the identical
    // `--severity CRITICAL --exit-code 1` gate its base failed, and signs it
    // with the identical attestor.
    //
    // Narrow deliberately: the destination must be declared in that file. An
    // overlay naming some other image this pipeline never built is still
    // refused, which is the property this whole function exists for.
    let derived =
        std::fs::read_to_string(repository_root().join("infrastructure/egress/derived-images.txt"))
            .unwrap_or_default();
    let declared = derived.lines().any(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return false;
        }
        line.split_whitespace()
            .nth(2)
            .is_some_and(|destination| repository.ends_with(&format!("/{destination}")))
    });
    if declared {
        Ok("derived")
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
    let mut unbuilt = 0usize;
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
            // Two ways a marker is legitimate, on two different axes: an
            // environment nothing has promoted into, or an image nothing has
            // ever built. Each is a written decision, and anything else is a
            // digest somebody forgot.
            if never_built(&image) {
                assert!(
                    dir.starts_with(ENVS),
                    "{place} runs `{image}`, and a NEVER_BUILT marker belongs to an \
                     environment's own manifests rather than to the bootstrap"
                );
                unbuilt += 1;
                continue;
            }
            assert!(
                dir.starts_with(ENVS) && awaiting_first_promotion(environment),
                "{place} runs `{image}`, which still carries the {TO_PIN} marker, and \
                 {environment} is not an environment NEVER_PROMOTED_TO argues for, nor is the \
                 image one NEVER_BUILT argues for"
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
    // The other axis, counted separately so the two exemptions cannot cover
    // for each other: an image nothing has ever built carries the marker
    // wherever it is deployed, and the premise here is that at least one such
    // manifest was actually read. Zero would mean the walk saw no NEVER_BUILT
    // image at all, and the entry above would be excusing nothing while
    // reading as an exemption in force.
    assert!(
        unbuilt >= NEVER_BUILT.len(),
        "{unbuilt} image reference(s) carried the marker for an image NEVER_BUILT argues for, \
         and there are {} such images; either they are pinned now — delete the entries — or \
         this walk is no longer reading the manifests that run them",
        NEVER_BUILT.len()
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
                trading_binary_of(&entry.name).is_some() || never_built(&entry.name),
                "{environment} leaves `{}` unpinned; only a catalogue binary awaits a \
                 promotion, or an image NEVER_BUILT argues for, and a vendored image is \
                 pinned by its line",
                entry.name
            );
        }
    }
    // The other direction: dev is pinned everywhere — except for an image no
    // run of the pipeline has ever produced, which has no digest to be
    // pinned to and says so.
    let dev = image_overrides(&manifests_under(&format!("{ENVS}/dev")));
    assert!(
        dev.len() >= 3,
        "dev's transformer names {} images; the three binaries at least",
        dev.len()
    );
    let mut pinned = 0usize;
    for entry in &dev {
        if never_built(&entry.name) {
            continue;
        }
        assert!(
            entry.digest.as_deref().is_some_and(is_digest_pinned_bare),
            "dev's transformer leaves `{}` at `{:?}`, not a sha256 digest",
            entry.name,
            entry.digest
        );
        pinned += 1;
    }
    // The premise, because the skip above is a hole and a hole nobody counts
    // is a hole that grows: dev still pins the images it has. Adding every
    // name in dev to NEVER_BUILT would otherwise leave this loop asserting
    // nothing while reading as the check that dev is pinned.
    assert!(
        pinned >= 3,
        "only {pinned} of dev's {} image(s) were checked for a digest; the rest were excused \
         by NEVER_BUILT, and an environment excused from being pinned is not pinned",
        dev.len()
    );
}

#[test]
fn every_image_deploy_yml_has_never_built_is_still_unbuilt_and_still_in_the_matrix() {
    // What stops NEVER_BUILT becoming a place things are left. An entry has
    // to argue its case; the pipeline has to actually build it, or it is an
    // image that will never acquire a digest and the marker is permanent;
    // and it has to still be unpinned somewhere, or a run has produced the
    // bytes and the entry now silently excuses whatever next takes the name.
    let deploy = read(DEPLOY_WORKFLOW);
    let overrides: Vec<ImageOverride> = ENVIRONMENTS
        .iter()
        .flat_map(|environment| image_overrides(&manifests_under(&format!("{ENVS}/{environment}"))))
        .collect();
    assert!(
        !overrides.is_empty(),
        "no image transformer was read out of {ENVS}; this test would excuse every entry"
    );
    for (image, reason) in NEVER_BUILT {
        assert!(
            reason.len() > 120,
            "the entry for {image} does not argue its case; an exception without a reason is \
             an exception nobody can review"
        );
        // Matched as a whole matrix line rather than with `contains`: the
        // workflow's prose names `qip-portal` too, and a test satisfied by a
        // comment is a test satisfied by deleting the build.
        assert!(
            deploy
                .lines()
                .any(|line| line.trim() == format!("- {image}")),
            "NEVER_BUILT says nothing has built {image}, and {DEPLOY_WORKFLOW}'s matrix does \
             not name it either; an image no pipeline builds never acquires a digest, so the \
             marker would be permanent rather than pending"
        );
        assert!(
            overrides.iter().any(|entry| entry.name == *image
                && entry
                    .digest
                    .as_deref()
                    .is_some_and(|digest| digest.contains(TO_PIN))),
            "NEVER_BUILT excuses {image} and no environment leaves it unpinned; a run has \
             produced the bytes, so delete the entry and pin the reviewed digest"
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
        deploy.contains("refuse a production deployment on every trigger")
            && deploy.contains("[ \"$TARGET_ENVIRONMENT\" = \"prod\" ]"),
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

/// The shell script of one step of `infra.yml`, dedented, as bash receives it.
///
/// The `env:` mapping is deliberately not read back: a value bound there is an
/// environment value, so the caller supplies it, which is exactly the property
/// under test. What is checked here is that the body carries no `${{ … }}` in
/// live code, because an expression is text the runner substitutes before bash
/// sees it and running the body without that substitution would be testing a
/// script the runner never produces.
fn infra_step_script(step_name: &str) -> String {
    let step = infra_steps()
        .into_iter()
        .find(|step| step.contains(&format!("- name: {step_name}")))
        .unwrap_or_else(|| panic!("{INFRA_WORKFLOW} has no step named {step_name:?}"));
    let body = step
        .split_once("run: |\n")
        .unwrap_or_else(|| panic!("the step {step_name:?} has no `run: |` block"))
        .1;
    let indent = body
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .unwrap_or_default();
    // The block ends at the first non-blank line indented less than its
    // content, as YAML ends it. `infra_steps` hands a step everything up to the
    // next `- `, so a comment that introduces the following step rode along
    // here and was cut mid-word into a line bash could not parse: R12-CICD-9066
    // put one after the prod refusal, and this test failed on a syntax error
    // in a script the runner never produces.
    let script: String = body
        .lines()
        .take_while(|line| line.trim().is_empty() || line.len() - line.trim_start().len() >= indent)
        .map(|line| {
            if line.len() >= indent {
                &line[indent..]
            } else {
                line.trim_start()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let live: String = script
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !live.contains("${{"),
        "the step {step_name:?} still pastes an expression into its script, so what runs \
         below is not what the runner would produce:\n{live}"
    );
    script
}

/// What that script does with one value of `ENVIRONMENT`: its exit status and
/// everything it printed.
fn run_infra_step(script: &str, environment: &str) -> (i32, String) {
    let path = std::env::temp_dir().join(format!(
        "qip-infra-refusal-{}-{}.sh",
        std::process::id(),
        environment.len()
    ));
    std::fs::write(&path, script).expect("the probe script is writable");
    let output = std::process::Command::new("bash")
        .arg(&path)
        .env("ENVIRONMENT", environment)
        .output()
        .unwrap_or_else(|error| panic!("bash could not be run: {error}"));
    let _ = std::fs::remove_file(&path);
    let mut printed = String::from_utf8_lossy(&output.stdout).to_string();
    printed.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.code().unwrap_or(-1), printed)
}

/// `infra.yml`'s prod refusal, driven rather than read.
///
/// A test that matches the text of a comparison pins a spelling, and the
/// spelling is exactly what changed when the dispatch value moved out of the
/// script and into the step's `env:`: `[ "${{ inputs.environment }}" = "prod" ]`
/// became `[ "$ENVIRONMENT" = "prod" ]`, and the assertion naming the first
/// read a correct refusal as a missing one. So this one runs the step's own
/// script under bash and asks what it does.
///
/// Three values, because a guard that refuses everything passes two of the
/// three and is still broken:
///
///   * `prod` is refused, and the refusal names prod rather than failing for
///     some other reason;
///   * `dev` is admitted — the half that proves the step is a gate and not a
///     wall, and the half a guard written too tightly fails;
///   * a value whose first line is a well-formed environment name and whose
///     second line is a command of its own is refused. With that value pasted
///     into the script by the runner it did not merely pass: the injected
///     command ran, inside this very step, and the step exited 0. The step no
///     longer takes it that way, and `no_workflow_pastes_a_dispatch_input_or_an_event_value_into_a_shell`
///     in infrastructure.rs is what keeps it from going back; this asserts the
///     other half, that the value is refused rather than quietly treated as an
///     environment that simply is not prod.
#[test]
fn the_prod_refusal_in_the_infrastructure_workflow_refuses_prod_when_it_is_run() {
    let script = infra_step_script("refuse what this workflow must never do");

    let (code, printed) = run_infra_step(&script, "prod");
    assert_ne!(
        code, 0,
        "the refusal step exited 0 for environment `prod`; {INFRA_WORKFLOW} holds the identity \
         that can reshape an environment and prod is applied by a person, from the bootstrap \
         script. It printed: {printed}"
    );
    assert!(
        printed.contains("prod"),
        "the refusal step refused `prod` for a reason it did not name; an operator reading \
         this learns nothing about which rule fired: {printed}"
    );

    let (code, printed) = run_infra_step(&script, "dev");
    assert_eq!(
        code, 0,
        "the refusal step exited {code} for environment `dev`, which is the one provisioned \
         environment and a dispatch choice. A guard that refuses everything is not a guard, \
         and this is the half that proves it works. It printed: {printed}"
    );

    let injection = "dev\" ]; then :; fi\ntouch /tmp/qip-should-never-exist\nif [ \"x";
    let (code, printed) = run_infra_step(&script, injection);
    assert_ne!(
        code, 0,
        "the refusal step admitted a dispatch value carrying a newline and a command. Bound \
         through `env:` it cannot execute, but admitting it means the character-set guard is \
         gone, and the guard is what stands between the next spelling of this step and the \
         injection it already suffered once. It printed: {printed}"
    );
    assert!(
        !std::path::Path::new("/tmp/qip-should-never-exist").exists(),
        "the injected command ran"
    );
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
    // The prod refusal stands in front of it, and the value it compares
    // arrives through that step's own `env:` rather than pasted into its
    // script. This assertion used to name the pasted spelling —
    // `[ "${{ inputs.environment }}" = "prod" ]` — which the runner substitutes
    // as source text, so a dispatch value carrying a newline ran a command of
    // its own inside the refusal step and the step exited 0.
    //
    // Scoped to the step that refuses, not to the file. A whole-file
    // `contains` for the `env:` binding passes while the refusal step alone
    // loses it, because six other steps in this workflow bind the same value —
    // which a mutation of exactly that shape demonstrated.
    let refusal = steps
        .iter()
        .find(|step| step.contains("[ \"$ENVIRONMENT\" = \"prod\" ]"))
        .unwrap_or_else(|| panic!("{INFRA_WORKFLOW} no longer refuses prod in a step"));
    assert!(
        refusal.contains("ENVIRONMENT: ${{ inputs.environment }}"),
        "{INFRA_WORKFLOW}'s prod refusal no longer binds the dispatch value in its own \
         `env:`; an expression inside `run:` is text the runner pastes in before bash \
         parses it"
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

/// Every probe's `timeoutSeconds` is strictly below its `periodSeconds`, on
/// every container of every RunService in every environment.
///
/// Not hypothetical. Cloud Run refuses a probe whose timeout is not smaller
/// than its period, on both of its API surfaces, but Config Connector's v2
/// create admitted `dev/portal.yaml`'s startup probe at 5 over 3 and the
/// service served. `infra.yml` run 119's IAP gate was the first thing to
/// re-submit that template through the v1 surface — `gcloud run services
/// update --iap` — and v1 refused it: `startup_probe.timeout_seconds: must
/// be less than period_seconds`. The console stayed ungated, and eight more
/// probes — the egress sidecar's startup probe in api and deepbrain, every
/// environment, at 2 over 2 — were waiting to fail the same way the first
/// time anything re-submitted them. So the rule is held here for every
/// probe rather than for the one that failed: the refusal arrives only on
/// whichever surface happens to re-read the template next, and nothing in
/// the manifest tree says which that will be.
///
/// Strict, not `<=`: 2 over 2 is what the sidecars carried, and equality is
/// the case Cloud Run's message names. A field the manifest omits is judged
/// at the value Cloud Run gives it rather than skipped, because a probe that
/// omits its period is still a probe Cloud Run validates.
#[test]
fn every_probe_times_out_before_its_period_because_cloud_run_refuses_the_update_otherwise() {
    // Cloud Run's values for an absent field.
    const DEFAULT_TIMEOUT_SECONDS: u64 = 1;
    const DEFAULT_PERIOD_SECONDS: u64 = 10;
    // A floor rather than an equality, so adding a probe does not fail here;
    // but a floor all the same, because a walk that found fewer has lost a
    // file, a container or a probe kind and would otherwise pass over
    // exactly the probe it was written to catch. The arithmetic on
    // 2026-10-13: four environments times (api 3, deepbrain 3, fastbrain 2),
    // plus dev's openobserve 2 and portal 2.
    const PROBES_IN_THE_TREE: usize = 4 * (3 + 3 + 2) + 2 + 2;

    let seconds = |probe: &Value, field: &str, default: u64, probe_description: &str| -> u64 {
        match text_at(probe, &[field]) {
            None => default,
            Some(text) => text.parse::<u64>().unwrap_or_else(|_| {
                panic!("{probe_description} has {field}: `{text}`, not a whole number of seconds")
            }),
        }
    };

    let mut probes = 0usize;
    let mut containers_seen = 0usize;
    for environment in environment_directories() {
        for (_, _, service, _) in run_services(&environment) {
            let describe = service.describe();
            let containers = list_at(&service.value, &["spec", "template", "containers"]);
            assert!(
                !containers.is_empty(),
                "{describe} declares no containers; nothing here would be probed"
            );
            for container in containers {
                let container_name = text_at(container, &["name"])
                    .unwrap_or_else(|| panic!("{describe} has a container with no name"));
                // Premise: the walk sees probes. modules/cloudrun gave every
                // container a startup probe and the manifests kept it, so a
                // container with none is the reader on the wrong key, not a
                // container Cloud Run would run unprobed.
                assert!(
                    at(container, &["startupProbe"]).is_some(),
                    "{describe}'s `{container_name}` container has no startupProbe; every \
                     container under envs/ carries one, so the walk is reading the wrong key"
                );
                containers_seen += 1;
                for kind in ["startupProbe", "livenessProbe"] {
                    let Some(probe) = at(container, &[kind]) else {
                        continue;
                    };
                    let probe_description = format!("{describe}'s `{container_name}` {kind}");
                    let timeout = seconds(
                        probe,
                        "timeoutSeconds",
                        DEFAULT_TIMEOUT_SECONDS,
                        &probe_description,
                    );
                    let period = seconds(
                        probe,
                        "periodSeconds",
                        DEFAULT_PERIOD_SECONDS,
                        &probe_description,
                    );
                    assert!(
                        timeout < period,
                        "{describe}: container `{container_name}` {kind} has timeoutSeconds \
                         {timeout} over periodSeconds {period}. Cloud Run refuses a probe whose \
                         timeoutSeconds is not smaller than periodSeconds; run 119's IAP-gate \
                         update was refused on dev/portal.yaml's 5 over 3"
                    );
                    probes += 1;
                }
            }
        }
    }
    assert!(
        probes >= PROBES_IN_THE_TREE,
        "{probes} probe(s) on {containers_seen} container(s) were checked; the tree held \
         {PROBES_IN_THE_TREE} on 2026-10-13, so the walk has lost a file, a container or a \
         probe kind and the probe it would miss is the one this test exists for"
    );
}

/// The instrument universe every central root reads at start-up, which
/// `modules/cloudrun` publishes under a directory named by its hash.
const UNIVERSE_CATALOGUE: &str = "data/datasets/universe.json";

/// The one Envoy bootstrap, which `modules/egress-proxy` publishes as an
/// object named by its hash.
const EGRESS_BOOTSTRAP: &str = "infrastructure/egress/envoy.yaml";

/// The root variables `catalogue.tf`'s `optional_config_files` local keys a
/// second configuration file on. Read from the catalogue rather than listed
/// here, because a list goes stale silently: a variable added there and not
/// here would be the one this walk never checked.
fn optional_config_file_variables() -> BTreeSet<String> {
    block_under(
        &without_comments(&read(CATALOGUE)),
        "optional_config_files = {",
    )
    .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
    .filter_map(|token| token.strip_prefix("var."))
    .filter(|name| name.ends_with("_file"))
    .map(str::to_string)
    .collect()
}

/// Two paths in every `RunService` are not values anybody chose. They are
/// names Terraform derives from the bytes of a committed file, and they
/// change whenever the file does.
///
/// `modules/cloudrun` publishes `data/datasets/universe.json` under
/// `/etc/qip/<prefix>/universe.json`, where `<prefix>` is
/// `substr(sha256(join("\n", [sha256(content) …])), 0, 16)`: the first
/// sixteen hex characters of the SHA-256 of the file's own lowercase-hex
/// SHA-256 digest, the universe being the only file in the join while no
/// tfvars names a second one. `modules/egress-proxy` publishes
/// `infrastructure/egress/envoy.yaml` as
/// `envoy-${substr(sha256(bootstrap), 0, 16)}.yaml`. Each manifest carries
/// the result as a literal — `QIP_UNIVERSE_PATH`, and the sidecar's `-c`.
///
/// Nothing compared the literal to the bytes. `envs/README.md` said a parity
/// test did; none existed. So `universe.json` changed in `4a571de6`
/// (2026-09-21) and the bootstrap in `349638db` (2026-10-05), Terraform
/// published both under their new names, and the twelve manifests kept
/// naming the old ones. `infra.yml` run 114's diagnose (2026-10-13) found the
/// result: `qip-dev-fastbrain` exiting 78 with "QIP_UNIVERSE_PATH names
/// /etc/qip/7ec25303c3f3ecac/universe.json, which cannot be read: No such
/// file or directory", and `qip-dev-deepbrain`'s Envoy dying on "Invalid
/// path: /etc/envoy/envoy-fc9574203f973d2d.yaml". The bucket held both
/// files; the revision was told the wrong names.
///
/// This test recomputes both names from the committed bytes with the in-tree
/// SHA-256 `qip-core` proves against FIPS 180-4, and refuses any manifest
/// whose literal differs. Exact comparison, never `contains`: a hash is
/// sixteen characters that are a substring of nothing else by accident, but
/// the rule is the rule.
#[test]
fn every_hash_named_path_a_manifest_mounts_is_the_name_terraform_publishes_the_committed_bytes_under()
 {
    // Premise: both committed files exist and hold bytes. A hash of an empty
    // file is a perfectly good hash, and a manifest pinned to it would be
    // pinned to nothing.
    let universe = std::fs::read(repository_root().join(UNIVERSE_CATALOGUE))
        .unwrap_or_else(|error| panic!("cannot read {UNIVERSE_CATALOGUE}: {error}"));
    assert!(
        !universe.is_empty(),
        "{UNIVERSE_CATALOGUE} is empty; the prefix this test derives would name nothing"
    );
    let bootstrap = std::fs::read(repository_root().join(EGRESS_BOOTSTRAP))
        .unwrap_or_else(|error| panic!("cannot read {EGRESS_BOOTSTRAP}: {error}"));
    assert!(
        !bootstrap.is_empty(),
        "{EGRESS_BOOTSTRAP} is empty; the object name this test derives would name nothing"
    );

    // Premise: the universe is the only file in every workload's
    // `config_files`. The module's prefix is the hash of every file's digest
    // joined in key order, so one more file is a different prefix for that
    // workload, and this test — which derives the prefix from the universe
    // alone — would then be asserting the wrong name with confidence.
    let optional = optional_config_file_variables();
    assert!(
        !optional.is_empty(),
        "{CATALOGUE}'s `optional_config_files` local names no `var.*_file`; the scan that \
         proves no environment mounts a second configuration file read nothing"
    );
    let environments = environment_directories();
    for environment in &environments {
        for variable in &optional {
            assert!(
                tfvars_value(environment, variable).is_none(),
                "{environment}'s tfvars set `{variable}`, so that workload's `config_files` \
                 holds a second file and modules/cloudrun's prefix is the hash of both \
                 digests joined; this test derives the prefix from the universe alone and \
                 must be extended before that environment's manifests can be right"
            );
        }
    }

    // modules/cloudrun/main.tf: `config_file_hashes[key] = sha256(content)`
    // (lowercase hex) and `config_prefix = substr(sha256(join("\n", [hashes
    // in key order])), 0, 16)`. One file, so the join is its digest alone.
    let universe_digest = qip_core::sha256_hex(&universe);
    let config_prefix = qip_core::sha256_hex(universe_digest.as_bytes())[..16].to_string();
    let universe_path = format!("/etc/qip/{config_prefix}/universe.json");
    // modules/egress-proxy/main.tf:
    // `name = "envoy-${substr(sha256(local.bootstrap), 0, 16)}.yaml"`.
    let bootstrap_path = format!(
        "/etc/envoy/envoy-{}.yaml",
        &qip_core::sha256_hex(&bootstrap)[..16]
    );

    let mut universe_paths = 0usize;
    let mut sidecars = 0usize;
    for environment in &environments {
        for (key, entry, service, _) in run_services(environment) {
            if entry.is_none() {
                continue;
            }
            let describe = service.describe();
            let container = workload_container(&service, &key);
            let under_config_root: Vec<(String, String)> = container_env(container)
                .into_iter()
                .filter_map(|(name, value, _)| value.map(|value| (name, value)))
                .filter(|(_, value)| value.starts_with("/etc/qip/"))
                .collect();
            assert_eq!(
                under_config_root.len(),
                1,
                "{describe} mounts {} path(s) under /etc/qip ({under_config_root:?}); the \
                 universe is the one configuration file every environment mounts, and a second \
                 changes the directory both are published under",
                under_config_root.len()
            );
            let (name, value) = &under_config_root[0];
            assert_eq!(
                name.as_str(),
                "QIP_UNIVERSE_PATH",
                "{describe}'s one path under /etc/qip is `{name}`, not QIP_UNIVERSE_PATH"
            );
            assert_eq!(
                value, &universe_path,
                "{describe} sets QIP_UNIVERSE_PATH={value}; Terraform publishes the committed \
                 {UNIVERSE_CATALOGUE} at {universe_path} and nowhere else, so the process exits \
                 78 at start-up unable to read it (infra.yml run 114, 2026-10-13)"
            );
            universe_paths += 1;

            let containers = list_at(&service.value, &["spec", "template", "containers"]);
            let Some(sidecar) = containers
                .iter()
                .find(|container| text_at(container, &["name"]).as_deref() == Some("qip-egress"))
            else {
                continue;
            };
            let args: Vec<String> = list_at(sidecar, &["args"])
                .iter()
                .filter_map(|arg| arg.as_str().map(str::to_string))
                .collect();
            let configs: Vec<&String> = args
                .iter()
                .enumerate()
                .filter(|(_, arg)| arg.as_str() == "-c")
                .filter_map(|(index, _)| args.get(index + 1))
                .collect();
            assert_eq!(
                configs.len(),
                1,
                "{describe}'s qip-egress container passes `-c` {} time(s) in {args:?}; Envoy \
                 takes exactly one bootstrap",
                configs.len()
            );
            assert_eq!(
                configs[0], &bootstrap_path,
                "{describe}'s qip-egress container starts with `-c {}`; modules/egress-proxy \
                 publishes the committed {EGRESS_BOOTSTRAP} as {bootstrap_path} and nowhere \
                 else, so Envoy dies on `Invalid path` and the workload waiting on it never \
                 starts (infra.yml run 114, 2026-10-13)",
                configs[0]
            );
            sidecars += 1;
        }
    }
    assert!(
        universe_paths >= 12,
        "{universe_paths} QIP_UNIVERSE_PATH value(s) were checked; three catalogue workloads \
         in four environments is twelve, and a walk that found fewer read the wrong tree"
    );
    assert!(
        sidecars >= 8,
        "{sidecars} qip-egress bootstrap argument(s) were checked; api and deepbrain in four \
         environments is eight, and a walk that found fewer read the wrong tree"
    );
}

/// No manifest under `envs/` carries a `gcs` key, in any document, anywhere.
///
/// The RunService CRD (Config Connector v1.156.0, byte-identical at upstream
/// `master` on 2026-10-13) admits `cloudSqlInstance`, `emptyDir` and
/// `secret` volumes and nothing else, and the API server prunes an unknown
/// key silently rather than refusing the object. The manifests carried a
/// `gcs` volume for three weeks: Config Connector reconciled a service with
/// no such mount, Argo CD reported the three services OutOfSync forever
/// because the target held a field the live object can never hold, and
/// `infra.yml` run 117 (2026-10-13) read the consequence off the containers
/// — Envoy dying on `Invalid path`, the fast brain exiting 78 unable to read
/// the universe. ADR 0036's amendment of that day moved both files into
/// secret versions and says this suite refuses the pruned mount coming back
/// by habit.
///
/// Every document and every key, not the volume list alone, because a `gcs`
/// key nested anywhere is pruned the same way; and the walk is over the
/// parsed value, which never sees a comment, so a manifest explaining why it
/// carries no `gcs` volume is allowed to say so. Beside it, every volume a
/// RunService does declare is exactly one of the three admitted kinds, so a
/// fourth kind the CRD also lacks — `nfs`, say — is refused by the same
/// test rather than by the next outage.
#[test]
fn no_manifest_declares_a_gcs_volume_because_the_run_service_crd_has_none_and_the_api_server_prunes_it_silently()
 {
    let manifests = manifests_under(ENVS);
    // Premise: the walk read the services, so a reader that stopped parsing
    // the volume list could not pass this by finding nothing.
    let run_services = manifests
        .iter()
        .filter(|manifest| manifest.kind() == "RunService")
        .count();
    assert!(
        run_services >= 12,
        "only {run_services} RunService(s) were read under {ENVS}; three catalogue workloads \
         in four environments is twelve, and a walk that found fewer read the wrong tree"
    );
    let mut volumes_seen = 0usize;
    for manifest in &manifests {
        let mut found = Vec::new();
        find_key(&manifest.value, "gcs", &mut found);
        assert!(
            found.is_empty(),
            "{} carries a `gcs` key ({} of them); the RunService CRD admits only the volume \
             kinds {ADMITTED_VOLUME_KINDS:?}, the API server prunes an unknown key silently \
             rather than refusing the object, Config Connector reconciles a service with no \
             such mount, and Argo CD reports the service OutOfSync forever because its target \
             holds a field the live object can never hold. A committed file travels as a \
             secret version (ADR 0036, amendment of 2026-10-13)",
            manifest.describe(),
            found.len()
        );
        if manifest.kind() != "RunService" {
            continue;
        }
        for volume in list_at(&manifest.value, &["spec", "template", "volumes"]) {
            let name = text_at(volume, &["name"]).unwrap_or_default();
            let kinds = volume_kinds(volume);
            assert!(
                kinds.len() == 1 && ADMITTED_VOLUME_KINDS.contains(&kinds[0].as_str()),
                "{}'s volume `{name}` is of kind(s) {kinds:?}; the RunService CRD admits \
                 exactly one of {ADMITTED_VOLUME_KINDS:?} per volume and the API server prunes \
                 anything else silently, so the mount never reaches a revision and the \
                 service never reaches Synced",
                manifest.describe()
            );
            volumes_seen += 1;
        }
    }
    // Premise: the kind check saw the volumes it exists for — a universe in
    // each of twelve services and a bootstrap beside each of eight sidecars.
    assert!(
        volumes_seen >= 12 + 2 * 4,
        "only {volumes_seen} volume(s) were seen across the RunServices; twelve universes and \
         eight bootstraps are more"
    );
}

/// The two IAM members that name everybody. `allUsers` is the public
/// internet; `allAuthenticatedUsers` is every Google account in existence,
/// and reads as a restriction in an audit while being none.
const ANONYMOUS_PRINCIPALS: [&str; 2] = ["allUsers", "allAuthenticatedUsers"];

/// The anonymous principals a parsed manifest names anywhere — a binding's
/// `member`, an annotation, any key or value — each matched as a whole token.
///
/// Over the manifest's serialised form rather than its `member` field alone,
/// because an annotation is part of what Config Connector is handed and a
/// principal written there is a principal in the applied object. A token and
/// not a substring, because a substring check on a principal is the shape
/// that has already passed a mutation in this repository.
fn anonymous_principals_named(manifest: &Manifest) -> Vec<&'static str> {
    let text = serde_json::to_string(&manifest.value).expect("a parsed manifest serialises");
    ANONYMOUS_PRINCIPALS
        .iter()
        .copied()
        .filter(|principal| {
            text.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|token| token == *principal)
        })
        .collect()
}

/// The ingress `modules/openobserve` derives for the authenticated posture:
/// the true arm of its `run_service_ingress` local.
///
/// Read out of the module rather than written here, so the manifest is held
/// to the decision where it is made; a module whose authenticated arm moved
/// and a manifest that did not would otherwise both stay green.
fn authenticated_openobserve_ingress() -> String {
    const MODULE: &str = "infrastructure/terraform/modules/openobserve/main.tf";
    const PREFIX: &str = "run_service_ingress = local.authenticated ? \"";
    let module = without_comments(&read(MODULE));
    let derived: Vec<String> = module
        .lines()
        .map(collapsed)
        .filter_map(|line| {
            line.strip_prefix(PREFIX)
                .and_then(|rest| rest.split('"').next())
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        derived.len(),
        1,
        "{MODULE} derives `run_service_ingress` {} time(s) in the form `{PREFIX}…`; expected \
         exactly one, so the ingress OpenObserve's manifest must carry cannot be read",
        derived.len()
    );
    derived[0].clone()
}

#[test]
fn openobserve_is_deployed_at_the_reviewed_digest_on_ephemeral_storage_and_answers_no_anonymous_caller()
 {
    // ADR 0028 adopted OpenObserve as a vendored image and ADR 0031 let it
    // take its root credential as an environment value; both are exceptions
    // to a rule the parity test holds for every other service, and both are
    // pinned here.
    //
    // **This test pinned the defect it now refuses, and was inverted rather
    // than deleted.** Until 2026-09-26 it was
    // `openobserve_is_deployed_at_the_reviewed_digest_anonymous_as_adr_0030_records_and_on_ephemeral_storage`,
    // and it asserted ADR 0030's posture as a property: `INGRESS_TRAFFIC_ALL`
    // on the RunService and exactly one manifest per environment naming the
    // anonymous principal, the `roles/run.invoker` binding on OpenObserve. So
    // the one change that closed the hole — a store of the desk's cycle,
    // refusal and order telemetry, writable by anyone the moment the
    // reconciler ran — was the change that turned this suite red. ADR 0033
    // decided to end that binding before the store held telemetry, and
    // `modules/openobserve` derives the posture that replaces it.
    //
    // Both halves are asserted again, each inverted, because either alone is
    // a deployment that lies about itself: the ingress the module derives for
    // the authenticated posture, and **zero** manifests under any
    // environment naming either anonymous principal — counted over whole
    // parsed manifests, annotations included, and over every environment
    // whether OpenObserve is deployed there or not, because a grant is a
    // grant wherever it is written. Which posture is in force is
    // `module.openobserve_access` in catalogue.tf, and `infrastructure.rs`
    // holds that call to the authenticated default and `invokers.yaml` to its
    // member set.
    let digest = vendored_digest("vendor/openobserve");
    let authenticated_ingress = authenticated_openobserve_ingress();
    // The premise the manifest assertion rests on: the authenticated arm is
    // the value that admits a load balancer and nothing else from outside the
    // VPC. A module whose authenticated arm became `INGRESS_TRAFFIC_ALL`
    // would have the manifest below agree with it and publish the run.app
    // URL around the proxy the posture is named for.
    assert_eq!(
        authenticated_ingress, INTERNAL_LOAD_BALANCER,
        "modules/openobserve derives `{authenticated_ingress}` for the authenticated posture; \
         ADR 0033 puts the service behind Identity-Aware Proxy on a load balancer, which only \
         {INTERNAL_LOAD_BALANCER} admits without leaving the run.app URL answering around it"
    );
    let mut checked = 0usize;
    let mut manifests_read = 0usize;
    let mut bindings_read = 0usize;
    for environment in environment_directories() {
        let manifests = manifests_under(&format!("{ENVS}/{environment}"));
        manifests_read += manifests.len();
        bindings_read += manifests
            .iter()
            .filter(|manifest| manifest.kind() == "IAMPolicyMember")
            .count();
        // No anonymous principal in any manifest, before anything about
        // whether OpenObserve is deployed here: the refusal is not
        // OpenObserve's, it is the environment's.
        let anonymous: Vec<String> = manifests
            .iter()
            .filter_map(|manifest| {
                let named = anonymous_principals_named(manifest);
                (!named.is_empty()).then(|| format!("{} names {named:?}", manifest.describe()))
            })
            .collect();
        assert!(
            anonymous.is_empty(),
            "{} manifest(s) under {ENVS}/{environment} name an anonymous principal, and none \
             may. ADR 0033 ended the one ADR 0030 granted — `roles/run.invoker` on OpenObserve \
             — because an anonymous invoker is anonymous in both directions, and a store anyone \
             can write to cannot be evidence for a gate. Name the operators, the group or the \
             service account instead, through `module.openobserve_access`:\n{}",
            anonymous.len(),
            anonymous.join("\n")
        );
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
            continue;
        };
        assert!(
            deployed,
            "{} exists and {environment}'s tfvars name no vendored_openobserve_image_digest, so \
             Terraform creates no identity for it",
            service.describe()
        );
        let describe = service.describe();
        // Equality with the derived value, never the absence of
        // `INGRESS_TRAFFIC_ALL`: a RunService with no `ingress` line is one
        // Cloud Run reads as all traffic, so a deleted line is the public
        // posture rather than a safe omission.
        let ingress = text_at(&service.value, &["spec", "ingress"]);
        assert_eq!(
            ingress.as_deref(),
            Some(authenticated_ingress.as_str()),
            "{describe} carries ingress {ingress:?}; ADR 0033's authenticated posture is \
             `{authenticated_ingress}`, the value modules/openobserve derives, which admits a \
             load balancer and nothing else from outside the VPC. `INGRESS_TRAFFIC_ALL` is ADR \
             0030's anonymous posture: it leaves the run.app URL answering the internet around \
             any proxy put in front of it"
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
    // The refusal's own premises. A walk that read no manifest, or read
    // manifests holding no invoker binding at all, would have found no
    // anonymous principal by never looking where one is written.
    assert!(
        manifests_read > 0 && bindings_read > 0,
        "the anonymous-principal refusal read {manifests_read} manifest(s) holding \
         {bindings_read} IAMPolicyMember binding(s) under {ENVS}; with none it proved nothing \
         about who may invoke anything"
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

// --- the console (ADR 0094, narrowed by ADR 0095) -----------------------------

/// The one ingress a service fronted by an IAP **load balancer** may carry.
///
/// It admits the global external Application Load Balancer `modules/iap-edge`
/// creates and nothing else from the internet, so the service's own `run.app`
/// URL stays unreachable and IAP cannot be walked around by dialling the
/// origin. Required of an environment that names a `gitops_portal_hostname`,
/// and of no other.
const INTERNAL_LOAD_BALANCER: &str = "INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER";

/// The one ingress a service fronted by **Cloud Run's own IAP** may carry.
///
/// ADR 0095. Cloud Run enforces IAP on the service across every ingress path,
/// the `run.app` URL included, so that URL has to be reachable for the gate
/// to have anything to gate. Required of an environment that names no
/// `gitops_portal_hostname`, which is every environment today.
///
/// **This value is safe here and nowhere else in this tree, and what makes it
/// safe is a second fact rather than a judgement**: the portal has exactly one
/// invoker and it is IAP's service agent. With no `allUsers` grant, a service
/// whose IAP was somehow not enabled answers 403 to everybody — a console
/// nobody can reach, never one anybody can reach. The test below asserts both
/// halves, because either alone is the shape most documentation shows and this
/// platform refuses.
const ALL_TRAFFIC: &str = "INGRESS_TRAFFIC_ALL";

/// The secrets the portal may hold, the variable naming each, and the path
/// each must appear at.
///
/// A list rather than a rule, because the rule — "what it needs" — is a
/// judgement, and this is that judgement written down where a diff shows it
/// changing. The browser receives nothing the public may not see and the
/// server holds nothing it cannot spend: no venue credential, no
/// capital-envelope key, no operator, analyst or monitor token, no
/// market-data key.
const PORTAL_SECRETS: [(&str, &str, &str); 2] = [
    (
        "qip-session-secret",
        "ALGORIK_SESSION_SECRET_FILE",
        "/var/run/secrets/algorik/session-secret/session-secret",
    ),
    (
        "qip-token-viewer",
        "QIP_API_TOKEN_FILE",
        "/var/run/secrets/qip/token-viewer/token-viewer",
    ),
];

#[test]
fn the_portal_is_reachable_only_behind_its_iap_edge_and_holds_only_what_it_needs() {
    // The console is the one service here a browser reaches, and it is not a
    // catalogue workload, so the parity walk above compares it to nothing.
    // This is what it is compared to instead.
    //
    // The first property is the one the whole design rests on, and since ADR
    // 0095 it is a **pairing** rather than a single value, because there are
    // two doors and Google refuses both at once ("You cannot configure IAP on
    // both the load balancer and the Cloud Run service").
    //
    //   * An environment naming `gitops_portal_hostname` has ADR 0094's load
    //     balancer, and must carry `INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER` —
    //     which admits that balancer and nothing else from the internet.
    //   * An environment naming none is reached at its Google-issued run.app
    //     URL behind IAP on the service itself, and must carry
    //     `INGRESS_TRAFFIC_ALL` — the URL has to be reachable for IAP to
    //     gate it.
    //
    // Either value is checked as equality with the exact string, never as the
    // absence of the other: a `RunService` with no `ingress` field at all is
    // one Cloud Run defaults to all traffic, so a deleted line reads as the
    // permissive value rather than as a safe omission — the same trap
    // `console_route.rs` names for the API.
    //
    // **`INGRESS_TRAFFIC_ALL` is only safe because of what is absent, so this
    // test asserts the absence too.** There is no `allUsers` invoker for the
    // portal: `invokers.yaml` grants `roles/run.invoker` to exactly one
    // principal, IAP's service agent. Without that half, flipping the ingress
    // would be the shape most documentation shows and this platform refuses —
    // a console answering the internet anonymously. With it, a service whose
    // IAP was somehow not enabled answers 403 to everybody, which is the
    // direction a safety default has to fail in.
    let mut checked = 0usize;
    for environment in environment_directories() {
        let project = tfvars_value(&environment, "project_id")
            .unwrap_or_else(|| panic!("{environment}'s tfvars name no project_id"));
        let Some((_, _, service, siblings)) = run_services(&environment)
            .into_iter()
            .find(|(key, _, _, _)| key == "portal")
        else {
            continue;
        };
        let describe = service.describe();

        let ingress = text_at(&service.value, &["spec", "ingress"]).unwrap_or_default();
        // `Some("")` is what the helper returns for `gitops_portal_hostname = ""`,
        // and it means the Cloud Run door. `None` — the key absent entirely —
        // means the same thing, because the variable defaults to the empty
        // string.
        let hostname = tfvars_value(&environment, "gitops_portal_hostname").unwrap_or_default();
        let expected = if hostname.is_empty() {
            ALL_TRAFFIC
        } else {
            INTERNAL_LOAD_BALANCER
        };
        assert_eq!(
            ingress, expected,
            "{describe} carries ingress `{ingress}` and {environment}'s tfvars set \
             gitops_portal_hostname to {hostname:?}, which selects `{expected}`. The two move \
             together: a named hostname is ADR 0094's load balancer, which only \
             {INTERNAL_LOAD_BALANCER} admits, and no hostname is ADR 0095's IAP on the service \
             itself, which needs {ALL_TRAFFIC} for the run.app URL to be reachable at all. \
             Neither is asserted as the absence of the other, because a `RunService` with no \
             `ingress` line is one Cloud Run reads as all traffic"
        );

        // What makes `INGRESS_TRAFFIC_ALL` safe, checked here rather than
        // assumed by the sentence above it: the portal has exactly one
        // invoker and it is IAP's service agent.
        //
        // This is the assertion that would fail if somebody "fixed" a 403 by
        // adding `allUsers` — which is the shape most documentation shows and
        // the shape that turns an IAP-gated console into a public one. The
        // whole-file walk matters: an `allUsers` binding for the portal in
        // any manifest under this directory is the hole, not only one in the
        // file a reader happens to open.
        let number = tfvars_value(&environment, "project_number")
            .unwrap_or_else(|| panic!("{environment}'s tfvars name no project_number"));
        let agent = format!("serviceAccount:service-{number}@gcp-sa-iap.iam.gserviceaccount.com");
        let invokers: Vec<(String, String)> = siblings
            .iter()
            .filter(|manifest| manifest.kind() == "IAMPolicyMember")
            .filter(|manifest| {
                text_at(&manifest.value, &["spec", "resourceRef", "name"]).as_deref()
                    == Some(service.name().as_str())
                    && text_at(&manifest.value, &["spec", "resourceRef", "kind"]).as_deref()
                        == Some("RunService")
            })
            .map(|manifest| {
                (
                    text_at(&manifest.value, &["spec", "role"]).unwrap_or_default(),
                    text_at(&manifest.value, &["spec", "member"]).unwrap_or_default(),
                )
            })
            .collect();
        // The premise first. A filter that found nothing would satisfy every
        // "no allUsers" assertion below for ever, and the portal would have
        // no invoker at all — which IAP itself needs, because it forwards as
        // its own agent.
        assert_eq!(
            invokers,
            vec![("roles/run.invoker".to_string(), agent.clone())],
            "{describe}'s invoker bindings under {ENVS}/{environment} are {invokers:?}; the \
             console has exactly one, `roles/run.invoker` for IAP's service agent {agent}. \
             None at all means IAP authenticates the caller correctly and the service answers \
             403; `allUsers` means the run.app URL answers the internet anonymously and IAP \
             guards a door with the side entrance open"
        );

        // It runs as the console identity ADR 0018 created, not an account of
        // its own. A second identity would have needed a second invoker on
        // the API, which is widening who may call the platform to make a
        // module fit.
        let account = text_at(
            &service.value,
            &["spec", "template", "serviceAccountRef", "external"],
        )
        .unwrap_or_else(|| panic!("{describe} names no template.serviceAccountRef.external"));
        assert_eq!(
            account,
            format!("qip-{environment}-console@{project}.iam.gserviceaccount.com"),
            // Both identities are printed by `assert_eq!` itself; repeating the
            // account in the message reads to the CodeQL gate as cleartext
            // logging of it (ADR 0117).
            "{describe} does not run as the console identity ADR 0018 created and ADR 0094 \
             reuses"
        );

        // Admission, and release rather than destruction on a prune. Two
        // properties `modules/cloudrun` held for every catalogue workload and
        // nothing holds for a service with no catalogue entry.
        assert_eq!(
            at(
                &service.value,
                &["spec", "binaryAuthorization", "useDefault"]
            ),
            Some(&Value::Bool(true)),
            "{describe} does not opt into the default Binary Authorization policy; a service \
             that does not opt in is never evaluated against it, and this one faces the \
             internet"
        );
        assert_eq!(
            service.annotation(DELETION_POLICY).as_deref(),
            Some("abandon"),
            "{describe} lacks `{DELETION_POLICY}: abandon`"
        );

        let container = workload_container(&service, "portal");

        // Exactly the two secrets, as files, with the `_FILE` variable of
        // each carrying its path — and no secret anywhere in the
        // environment. ADR 0031 permits an environment value for a vendored
        // image that cannot read a file; this image can, and `secret.ts`
        // resolves the same `_FILE` contract `qip_core::secret` does.
        let env = container_env(container);
        for (name, _, from_secret) in &env {
            assert!(
                !from_secret,
                "{describe} takes `{name}` from a secret as an environment value; a secret in \
                 the environment is a secret in /proc/<pid>/environ, in every child process \
                 and in every crash dump"
            );
        }
        let mounted: BTreeSet<String> = list_at(container, &["volumeMounts"])
            .iter()
            .filter_map(|mount| text_at(mount, &["mountPath"]))
            .collect();
        let volumes: BTreeSet<String> = list_at(&service.value, &["spec", "template", "volumes"])
            .iter()
            .filter_map(|volume| text_at(volume, &["secret", "secretRef", "external"]))
            .collect();
        let expected: BTreeSet<String> = PORTAL_SECRETS
            .iter()
            .map(|(secret, _, _)| format!("{secret}-{environment}"))
            .collect();
        assert_eq!(
            volumes, expected,
            "{describe} mounts the secrets {volumes:?}; the console needs exactly {expected:?} \
             — the key it signs its own cookies with and the viewer credential its gateway \
             proxies. Anything else is a credential in a container that faces the internet \
             and has no way to spend it"
        );
        for (secret, variable, path) in PORTAL_SECRETS {
            let directory = path
                .rsplit_once('/')
                .map(|(directory, _)| directory.to_string())
                .unwrap_or_default();
            assert!(
                mounted.contains(&directory),
                "{describe} names {secret}'s path as `{path}` in {variable} and mounts nothing \
                 at `{directory}`; the process opens the path that variable holds and nothing \
                 else"
            );
            let value = env
                .iter()
                .find(|(name, _, _)| name == variable)
                .and_then(|(_, value, _)| value.clone());
            assert_eq!(
                value.as_deref(),
                Some(path),
                "{describe} sets {variable} to {value:?} rather than `{path}`"
            );
        }

        // The session check that stands behind IAP. Matched as the exact
        // value: `contains("ALGORIK_AUTH_REQUIRED")` is true of a manifest
        // that sets it to `false`, which is the console answering anyone with
        // the platform's data on a token nobody had to present — a state a
        // deploy was once actually in.
        let auth = env
            .iter()
            .find(|(name, _, _)| name == "ALGORIK_AUTH_REQUIRED")
            .and_then(|(_, value, _)| value.clone());
        assert_eq!(
            auth.as_deref(),
            Some("true"),
            "{describe} sets ALGORIK_AUTH_REQUIRED to {auth:?}; IAP decides who may reach the \
             console and this decides who may read the platform through it, and one gate in \
             front of the platform's book is one gate too few"
        );

        // The posture the chrome renders. There is no deployment of this
        // platform that is not paper (ADR 0003), so there is no value here
        // but `paper`.
        let posture = env
            .iter()
            .find(|(name, _, _)| name == "ALGORIK_POSTURE")
            .and_then(|(_, value, _)| value.clone());
        assert_eq!(
            posture.as_deref(),
            Some("paper"),
            "{describe} sets ALGORIK_POSTURE to {posture:?}; a console that renders any other \
             posture is a console implying a live path that does not exist"
        );

        // Where the platform is, in the deterministic form modules/cloudrun
        // computes: the service name, the project number, the region. A
        // literal that drifted from it is a console whose every gateway call
        // fails with a name-resolution error nobody attributes to this file.
        let number = tfvars_value(&environment, "project_number")
            .unwrap_or_else(|| panic!("{environment}'s tfvars name no project_number"));
        let region = tfvars_value(&environment, "region")
            .unwrap_or_else(|| panic!("{environment}'s tfvars name no region"));
        let upstream = env
            .iter()
            .find(|(name, _, _)| name == "QIP_API_BASE_URL")
            .and_then(|(_, value, _)| value.clone());
        assert_eq!(
            upstream,
            Some(format!(
                "https://qip-{environment}-api-{number}.{region}.run.app"
            )),
            "{describe} dials {upstream:?}, which is not the API's own URL as \
             modules/cloudrun computes it"
        );

        // The image is the portal's, from this environment's own registry,
        // and it is pinned or is one nothing has built yet.
        let declared = text_at(container, &["image"])
            .unwrap_or_else(|| panic!("{describe}'s `portal` container has no image"));
        let image = resolve_image(&declared, &image_overrides(&siblings));
        assert!(
            image.contains(&format!("/qip-{environment}/qip-portal")),
            "{describe} runs `{image}`, which is not qip-portal from {environment}'s own \
             registry"
        );
        assert!(
            is_digest_pinned(&image) || never_built(&image),
            "{describe} runs `{image}`, which is neither pinned by a digest nor an image \
             NEVER_BUILT argues for"
        );

        checked += 1;
    }
    // The premise. Without it a renamed manifest would make every assertion
    // above run zero times and the test would pass describing nothing.
    assert!(
        checked >= 1,
        "no environment under {ENVS} declares a `portal` RunService, so none of the properties \
         above was checked against anything"
    );
}

/// The proving hook can write the configuration directory `gcloud` insists on
/// before it will run at all.
///
/// ADR 0036 decision 7's PostSync hook runs `gcloud run services describe` as
/// a non-root user, `runAsUser: 65532`. `gcloud` creates a configuration
/// directory derived from `$HOME` before executing any command, and with no
/// `HOME` set that path is `/.config/gcloud` — the filesystem root, which a
/// non-root user cannot write. The hook therefore failed on every deployment
/// with
///
///   ERROR: (gcloud.run.services.describe) Could not create directory
///   [/.config/gcloud]: Permission denied.
///
/// and nothing about that is visible from outside: `backoffLimit: 0` plus
/// `hook-delete-policy: BeforeHookCreation` deletes the failed pod before the
/// next attempt, so Argo CD reported only "Job has reached the specified
/// backoff limit" and the deployment's last gate refused for three runs
/// without being able to say why. It was read as a Cloud Run problem, a
/// digest problem and a permission problem in turn, and it was none of them.
///
/// This test is cheap and the failure it prevents cost more than the whole
/// suite. It is written over all four environments rather than dev alone,
/// because the manifests are copies and a copy is exactly what goes stale.
#[test]
fn the_proving_hook_has_a_writable_configuration_directory_in_every_environment() {
    let jobs: Vec<Manifest> = manifests_under(ENVS)
        .into_iter()
        .filter(|manifest| manifest.kind() == "Job" && manifest.name() == "qip-prove-serving")
        .collect();

    // Premise: there are four, one per environment. A test that iterates an
    // empty list asserts nothing while reporting success, and this one is a
    // conjunction of per-environment checks.
    assert_eq!(
        jobs.len(),
        4,
        "expected one qip-prove-serving Job per environment and found {}: {:?}",
        jobs.len(),
        jobs.iter().map(|job| &job.path).collect::<Vec<_>>()
    );

    for job in &jobs {
        let pod = at(&job.value, &["spec", "template", "spec"])
            .unwrap_or_else(|| panic!("{} has no pod template", job.describe()));
        let container = pod
            .get("containers")
            .and_then(|containers| containers.as_array())
            .and_then(|containers| containers.first())
            .unwrap_or_else(|| panic!("{} declares no container", job.describe()));

        // The variable `gcloud` actually reads for its configuration
        // directory. `HOME` alone would work today and is set beside it, but
        // naming the directory directly is what stops this depending on how
        // the vendored image happens to set `HOME`.
        let config_dir = container
            .get("env")
            .and_then(|env| env.as_array())
            .and_then(|env| {
                env.iter().find(|entry| {
                    entry.get("name").and_then(|n| n.as_str()) == Some("CLOUDSDK_CONFIG")
                })
            })
            .and_then(|entry| entry.get("value"))
            .and_then(|value| value.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "{} sets no CLOUDSDK_CONFIG, so gcloud derives its configuration directory \
                     from HOME and a non-root pod cannot create it. The hook fails before it \
                     checks anything, and reports only that it reached its backoff limit.",
                    job.describe()
                )
            });

        // Naming a directory is not being able to write it. The path must sit
        // under a mount, and that mount must be a volume the pod supplies
        // rather than a directory in the image, which is not the pod's to
        // write.
        let mount = container
            .get("volumeMounts")
            .and_then(|mounts| mounts.as_array())
            .and_then(|mounts| {
                mounts.iter().find(|mount| {
                    mount
                        .get("mountPath")
                        .and_then(|path| path.as_str())
                        .is_some_and(|path| {
                            config_dir == path || config_dir.starts_with(&format!("{path}/"))
                        })
                })
            })
            .unwrap_or_else(|| {
                panic!(
                    "{} points CLOUDSDK_CONFIG at {config_dir} and mounts nothing that contains \
                     it, so the directory is in the image's filesystem and uid 65532 cannot \
                     create it — the same failure with a different path.",
                    job.describe()
                )
            });

        let mount_name = mount
            .get("name")
            .and_then(|name| name.as_str())
            .unwrap_or_default();
        let supplied = pod
            .get("volumes")
            .and_then(|volumes| volumes.as_array())
            .is_some_and(|volumes| {
                volumes.iter().any(|volume| {
                    volume.get("name").and_then(|name| name.as_str()) == Some(mount_name)
                        && volume.get("emptyDir").is_some()
                })
            });
        assert!(
            supplied,
            "{} mounts `{mount_name}` at the configuration directory and the pod declares no \
             emptyDir by that name, so the mount resolves to nothing writable",
            job.describe()
        );
    }
}

/// The proving hook waits for the controller it checks, and says why when it
/// refuses.
///
/// Two properties, both bought at the cost of a run each.
///
/// It runs `PostSync` — immediately after Argo CD applies the manifests, and
/// seconds before Config Connector has finished asking Cloud Run for
/// anything. With `backoffLimit: 0` a single instantaneous look is the whole
/// decision, so a slow reconcile and a broken one produce the same verdict.
///
/// And when no revision serves traffic, "routes traffic to no revision" names
/// the symptom while withholding the cause: a revision that failed to start,
/// a revision never created, and a service nobody asked to serve all read
/// identically. Runs 87 and 88 reported exactly that string and three
/// different causes were proposed for it, none of which could be checked.
/// Cloud Run's own conditions say which, and they are one call away.
#[test]
fn the_proving_hook_waits_for_convergence_and_reports_what_cloud_run_says() {
    let jobs: Vec<Manifest> = manifests_under(ENVS)
        .into_iter()
        .filter(|manifest| manifest.kind() == "Job" && manifest.name() == "qip-prove-serving")
        .collect();
    assert_eq!(jobs.len(), 4, "expected one proving Job per environment");

    for job in &jobs {
        let command = at(&job.value, &["spec", "template", "spec", "containers"])
            .and_then(|containers| containers.as_array())
            .and_then(|containers| containers.first())
            .and_then(|container| container.get("command"))
            .and_then(|command| command.as_array())
            .and_then(|command| command.last())
            .and_then(|script| script.as_str())
            .unwrap_or_else(|| panic!("{} declares no container command", job.describe()));

        // Premise: this is the script under discussion and not some other
        // container's, so the two absences below mean something.
        assert!(
            command.contains("runservices"),
            "{} runs a command that never reads the RunServices; this test is pinned to the \
             wrong container",
            job.describe()
        );

        // Three parts, because any one alone is satisfied by a script that
        // does not wait: a clock reading proves nothing without a loop, and a
        // loop proves nothing without something that yields between passes.
        // The first spelling of this assertion checked for a constant name
        // and a clock call, and a mutation renaming the constant left both in
        // place at the use site — it passed a script it should have refused.
        for token in ["time.sleep(", "while True:", "time.time()"] {
            assert!(
                command.contains(token),
                "{} has no `{token}`, so it decides on a single look. It runs PostSync, before \
                 Config Connector has finished reconciling, so a slow reconcile is reported as \
                 a failed deployment and `backoffLimit: 0` means there is no second attempt.",
                job.describe()
            );
        }

        assert!(
            command.contains("latestCreatedRevisionName"),
            "{} reports that no revision serves traffic without asking Cloud Run why. That \
             string is true of a revision that failed to start, a revision never created, and \
             a service nobody asked to serve, and the deployment cannot be diagnosed from it.",
            job.describe()
        );
    }
}

#[test]
fn the_proving_hook_refreshes_from_the_api_server_and_not_from_a_file_written_once() {
    // The wait loop used to re-open /tmp/runservices.json at the bottom of
    // every pass, under a comment saying it re-read the RunServices "so a
    // Config Connector condition that changes while this loop runs is seen
    // rather than held from the first read". A shell curl wrote that file
    // once, before python started. So every pass re-tested second zero: a
    // service that went Ready at second 11 was still reported unready at
    // second 300, and the hook failed the sync it was watching succeed.
    //
    // This is not hypothetical and it is not a wording defect. Waiting
    // exists to observe a change, and that loop could observe none -- which
    // is the same shape as a limit that cannot fire: it read as proof while
    // being structurally incapable of producing any.
    //
    // The comment is what went stale, so the property is pinned here where
    // it gets re-derived instead.
    let jobs: Vec<Manifest> = manifests_under(ENVS)
        .into_iter()
        .filter(|manifest| manifest.kind() == "Job" && manifest.name() == "qip-prove-serving")
        .collect();
    assert_eq!(jobs.len(), 4, "expected one proving Job per environment");

    for job in &jobs {
        let command = at(&job.value, &["spec", "template", "spec", "containers"])
            .and_then(|containers| containers.as_array())
            .and_then(|containers| containers.first())
            .and_then(|container| container.get("command"))
            .and_then(|command| command.as_array())
            .and_then(|command| command.last())
            .and_then(|script| script.as_str())
            .unwrap_or_else(|| panic!("{} declares no container command", job.describe()));

        // Premise: this is the waiting script and it does refresh something.
        // Without both, every absence below is asserted of nothing.
        assert!(
            command.contains("while True:") && command.contains("refreshed"),
            "{} has no refresh inside a loop; this test is pinned to the wrong container",
            job.describe()
        );

        // The one read whose answer can differ between passes must reach the
        // API server. Naming the function is not enough on its own -- a
        // function so named could still open a file -- so its body is checked
        // for the API host below.
        assert!(
            command.contains("refreshed = read_runservices()"),
            "{} refreshes from something other than `read_runservices()`, so the loop may be \
             re-testing a value that cannot change.",
            job.describe()
        );
        assert!(
            command.contains("def read_runservices():")
                && command.contains("https://kubernetes.default.svc")
                && command.contains("\"curl\""),
            "{} has no `read_runservices` that asks the API server, so the refresh reaches no \
             live state.",
            job.describe()
        );

        // The defect itself, refused by name: nothing in this script may read
        // its state from a file. A file inside this pod can only have been
        // written before the loop started, and a value written before the
        // wait cannot be the thing the wait is waiting for.
        assert!(
            !command.contains("json.load(open("),
            "{} loads JSON from a file. Whatever it holds was written before the wait began \
             and cannot change while the wait runs, so the loop would spin the full deadline \
             on a snapshot of second zero and then fail a sync that had already converged.",
            job.describe()
        );
    }
}

#[test]
fn a_cloud_run_service_with_cnrm_deletion_policy_abandon_survives_gitops_control_plane_loss() {
    // RES-087: A Cloud Run service is structurally independent of the GitOps
    // control-plane cluster. Once a revision is routed, Cloud Run serves it
    // independently; the cluster only produces new desired state. A chaos test
    // that actually stops the cluster is deferred (no deployment exists yet),
    // but this test verifies the precondition: every RunService carries the
    // cnrm deletion-policy: abandon, so losing or rebuilding the control plane
    // cannot cascade-delete a serving service.
    //
    // Mutation: If the deletion-policy is omitted, the service would be
    // deleted when the cluster is deleted. If the metadata is changed or the
    // annotation key is misspelled, the service is unprotected.

    let services: Vec<Manifest> = manifests_under(ENVS)
        .into_iter()
        .filter(|manifest| manifest.kind() == "RunService")
        .collect();
    assert!(
        !services.is_empty(),
        "no RunService manifests found; this test is pinned to the wrong filter"
    );

    for service in &services {
        let metadata = at(&service.value, &["metadata"]).unwrap_or_else(|| {
            panic!(
                "{} has no metadata (this is not a valid Kubernetes resource)",
                service.describe()
            )
        });

        let annotations = at(metadata, &["annotations"])
            .and_then(|a| a.as_object())
            .unwrap_or_else(|| {
                panic!(
                    "{} has no annotations object; Cloud NAT Manager protection requires \
                     cnrm.cloud.google.com/deletion-policy: abandon",
                    service.describe()
                )
            });

        let deletion_policy = annotations
            .get("cnrm.cloud.google.com/deletion-policy")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "{} carries no cnrm deletion-policy annotation. Without it, the service \
                     is deleted when the control plane is deleted, even though Cloud Run serves \
                     it independently of the control plane.",
                    service.describe()
                )
            });

        assert_eq!(
            deletion_policy,
            "abandon",
            "{} has deletion-policy: {}, not abandon. The service will be cascade-deleted \
             if the control plane is lost.",
            service.describe(),
            deletion_policy
        );
    }
}

// --- the paper gate's automated check (R12-CICD-7120) ----------------------

/// The environments the paper gate's check reads: the one paper-passed
/// freight is promoted to next, and the one it can never reach.
const PAPER_CHECKED: [&str; 2] = ["stage", "prod"];
const PAPER_SERVICES: [&str; 3] = ["api", "fastbrain", "deepbrain"];
const LIVE_CEILINGS: [&str; 3] = [
    "supervised_live",
    "limited_autonomous_live",
    "autonomous_live",
];

#[test]
fn the_paper_stage_cannot_promote_without_asserting_the_paper_trading_ceiling_first() {
    // Before this, `paper` was a person's act and nothing else: its template
    // recorded the freight and no step looked at the posture it would run
    // under. Three halves, because each alone is one edit from gone: the stage
    // runs the check before it records; the check has a step per manifest the
    // freight could roll out under; and each step fails the promotion on
    // anything but exactly `paper_trading`.
    for environment in ENVIRONMENTS {
        let rendered = kargo_rendered(environment);
        let stages = stages_by_name(&rendered);
        let paper = &stages["paper"];
        let steps = list_at(
            &paper.value,
            &["spec", "promotionTemplate", "spec", "steps"],
        );
        let tasks: Vec<String> = steps
            .iter()
            .map(|step| text_at(step, &["task", "name"]).unwrap_or_default())
            .collect();
        let verify = tasks.iter().position(|t| t == "verify-paper-ceiling");
        let record = tasks.iter().position(|t| t == "record-gate");
        assert!(
            record.is_some(),
            "{environment}: {} no longer runs record-gate ({tasks:?}); this test cannot see the \
             step it orders the check before",
            paper.describe()
        );
        assert!(
            verify.is_some() && verify < record,
            "{environment}: {} runs {tasks:?}; the paper-trading ceiling must be asserted by \
             `verify-paper-ceiling` before `record-gate` writes anything, or freight can pass the \
             gate with no check",
            paper.describe()
        );

        let task = of_kind(&rendered, "PromotionTask")
            .into_iter()
            .find(|task| task.name() == "verify-paper-ceiling")
            .unwrap_or_else(|| panic!("{environment}'s chain declares no verify-paper-ceiling"));
        let task_steps = list_at(&task.value, &["spec", "steps"]);
        assert_eq!(
            task_steps
                .first()
                .and_then(|step| text_at(step, &["uses"]))
                .as_deref(),
            Some("git-clone"),
            "verify-paper-ceiling must read the committed tree first"
        );
        for checked in PAPER_CHECKED {
            for service in PAPER_SERVICES {
                let alias = format!("{checked}_{service}");
                let path = format!("{GITOPS}/envs/{checked}/{service}.yaml");
                assert!(
                    repository_root().join(&path).is_file(),
                    "{path} does not exist; the check reads it"
                );
                let parse = task_steps
                    .iter()
                    .position(|step| text_at(step, &["as"]).as_deref() == Some(alias.as_str()))
                    .unwrap_or_else(|| panic!("verify-paper-ceiling has no `{alias}` step"));
                let parse_step = task_steps[parse];
                assert_eq!(
                    text_at(parse_step, &["uses"]).as_deref(),
                    Some("yaml-parse")
                );
                assert!(
                    text_at(parse_step, &["config", "path"])
                        .unwrap_or_default()
                        .ends_with(&path),
                    "`{alias}` does not read {path}"
                );
                let outputs = list_at(parse_step, &["config", "outputs"]);
                let expression = outputs
                    .iter()
                    .filter(|o| text_at(o, &["name"]).as_deref() == Some("ceiling"))
                    .filter_map(|o| text_at(o, &["fromExpression"]))
                    .next()
                    .unwrap_or_default();
                assert!(
                    expression.contains("QIP_AUTONOMY_CEILING"),
                    "`{alias}` does not select QIP_AUTONOMY_CEILING: `{expression}`"
                );
                let refusal = task_steps
                    .get(parse + 1)
                    .unwrap_or_else(|| panic!("`{alias}` is the last step; nothing refuses"));
                assert_eq!(
                    text_at(refusal, &["uses"]).as_deref(),
                    Some("fail"),
                    "the step after `{alias}` does not fail the promotion"
                );
                let condition = text_at(refusal, &["if"]).unwrap_or_default();
                let wanted = format!("task.outputs.{alias}.ceiling != \"paper_trading\"");
                assert!(
                    condition.contains(&wanted),
                    "`{alias}`'s refusal tests `{condition}`, not `{wanted}`. Exact equality is \
                     the fail-closed form: a deny-list of live names misses the next one."
                );
            }
        }
    }
}

#[test]
fn the_ceiling_the_paper_check_reads_is_paper_trading_in_every_manifest_it_could_roll_out() {
    // The other side of the check: the committed manifests satisfy it. Run
    // here with the check's own semantics (every env entry named
    // QIP_AUTONOMY_CEILING across every container, joined) so a manifest that
    // would make the gate refuse every promotion, or one that would slip a
    // live ceiling past it, is found at build time. Premise first: the three
    // services each carry exactly one entry, so "all equal" is not vacuous.
    for environment in ENVIRONMENTS {
        for service in PAPER_SERVICES {
            let path = format!("{ENVS}/{environment}/{service}.yaml");
            let manifests = manifests_under(&path.replace(&format!("/{service}.yaml"), ""));
            let manifest = manifests
                .iter()
                .find(|m| m.path.ends_with(&format!("{environment}/{service}.yaml")))
                .unwrap_or_else(|| panic!("{path} holds no document"));
            let mut ceilings = Vec::new();
            for container in list_at(&manifest.value, &["spec", "template", "containers"]) {
                for (name, value, _) in container_env(container) {
                    if name == "QIP_AUTONOMY_CEILING" {
                        ceilings.push(value.unwrap_or_default());
                    }
                }
            }
            assert_eq!(
                ceilings.len(),
                1,
                "{path} sets QIP_AUTONOMY_CEILING {} time(s); exactly one is the premise of the \
                 paper gate's check",
                ceilings.len()
            );
            assert_eq!(
                ceilings.join(","),
                "paper_trading",
                "{path} sets a ceiling other than paper_trading (ADR 0003)"
            );
            assert!(
                !LIVE_CEILINGS.contains(&ceilings[0].as_str()),
                "{path} names a live ceiling"
            );
        }
    }
}

// --- ADR 0114: the control plane runs controllers and nothing else -------------

/// The bootstrap components ADR 0114 admits, by directory name under
/// `bootstrap/`, and the controller each installs. Config Sync and Argo
/// Rollouts are not here on purpose: ADR 0114 decides the baseline is
/// reconciled by Config Connector and progressive delivery is Cloud Run's
/// own instance bounds (R12 CICD-9113), so a directory for either is a
/// decision reopened without an ADR.
const CONTROLLERS: [(&str, &str); 5] = [
    ("argocd", "Argo CD"),
    ("cert-manager", "cert-manager"),
    (
        "config-connector",
        "the ConfigConnector object and the qip-run namespace",
    ),
    ("config-connector-operator", "the Config Connector operator"),
    ("kargo", "Kargo"),
];

/// The kinds and the API group a control plane would carry if a reconciler
/// ADR 0114 refused had been vendored anyway. Matched on the whole `kind`
/// value, never as a substring, because `Rollout` is a token inside
/// `argoproj.io` documentation strings and a substring match would refuse the
/// very manifests the cluster needs.
const REFUSED_KINDS: [(&str, &str); 3] = [
    (
        "RootSync",
        "Config Sync (ADR 0114: Config Connector reconciles the baseline)",
    ),
    (
        "RepoSync",
        "Config Sync (ADR 0114: Config Connector reconciles the baseline)",
    ),
    (
        "Rollout",
        "Argo Rollouts (ADR 0114: R12 CICD-9113 is Cloud Run's instance bounds)",
    ),
];
const REFUSED_API_GROUP: &str = "configsync.gke.io/";

/// The immediate subdirectories of a repository path, by name.
fn subdirectories(relative: &str) -> Vec<String> {
    let root = repository_root().join(relative);
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("cannot list {relative}: {error}"))
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn the_control_plane_bootstrap_installs_only_the_controllers_adr_0114_names() {
    // The failure: a fifth reconciler — Config Sync, Argo Rollouts, anything
    // with a `Deployment` — lands under `bootstrap/` with an overlay, and
    // `infra.yml`'s bootstrap step applies whatever has one. The register
    // kept asking for Config Sync for a year after ADR 0114 decided against
    // it; this test is where that decision stops being prose.
    let components = subdirectories(BOOTSTRAP);
    assert!(
        components.len() >= CONTROLLERS.len(),
        "{BOOTSTRAP} holds {components:?}; ADR 0036 decision 2 vendors at least the {} \
         controllers",
        CONTROLLERS.len()
    );
    let mut applied: Vec<String> = Vec::new();
    for component in &components {
        let directory = format!("{BOOTSTRAP}/{component}");
        // A component is applied to an environment exactly when it carries an
        // overlay for it (`infra.yml`: `if [ -d .../overlays/${ENVIRONMENT} ]`).
        let overlays: Vec<&str> = ENVIRONMENTS
            .iter()
            .copied()
            .filter(|environment| {
                repository_root()
                    .join(format!("{directory}/overlays/{environment}"))
                    .is_dir()
            })
            .collect();
        let admitted = CONTROLLERS.iter().any(|(name, _)| name == component);
        if admitted {
            assert_eq!(
                overlays,
                ENVIRONMENTS.to_vec(),
                "{directory} carries overlays for {overlays:?}; a controller ADR 0114 admits is \
                 applied to every environment's control plane or the environments differ in \
                 what reconciles them"
            );
            applied.push(component.clone());
        } else {
            assert!(
                overlays.is_empty(),
                "{directory} carries overlays for {overlays:?} and is not a controller ADR 0114 \
                 names ({:?}); the bootstrap step would apply it, and a reconciler the ADR \
                 refused reaches the cluster",
                CONTROLLERS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
            );
        }
    }
    assert_eq!(
        applied,
        CONTROLLERS
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect::<Vec<_>>(),
        "the components with overlays are not exactly the controllers ADR 0114 names"
    );

    // What those components render — the vendored installs and every base
    // and overlay the step applies — carries none of the refused kinds and no
    // trading binary. A `qip-*` image in any Pod spec is ADR 0114's own
    // "what would make this wrong".
    let upstream = upstream_documents();
    // (path, kind, apiVersion, text) for every document the step could apply:
    // the upstream installs, read line by line, and every base and overlay
    // document, parsed.
    let mut documents: Vec<(String, String, String, String)> = upstream
        .iter()
        .map(|document| {
            let api_version = document
                .text
                .lines()
                .find_map(|line| line.strip_prefix("apiVersion: "))
                .unwrap_or_default()
                .trim()
                .to_string();
            (
                document.path.clone(),
                document.kind.clone(),
                api_version,
                document.text.clone(),
            )
        })
        .collect();
    for manifest in manifests_under(BOOTSTRAP) {
        documents.push((
            manifest.path.clone(),
            manifest.kind(),
            manifest.api_version(),
            manifest.value.to_string(),
        ));
    }
    assert!(
        documents.len() > upstream.len(),
        "{BOOTSTRAP} renders no document outside the upstream installs; the bases and overlays \
         were not read"
    );
    let mut workloads = 0usize;
    for document in &upstream {
        if matches!(
            document.kind.as_str(),
            "Deployment" | "StatefulSet" | "CronJob" | "DaemonSet"
        ) {
            workloads += 1;
            let owner = CONTROLLERS.iter().any(|(name, _)| {
                let prefix = name.replace('-', "");
                document.name.replace('-', "").starts_with(&prefix)
            });
            assert!(
                owner,
                "{} `{}` in {} is a workload no controller ADR 0114 names owns",
                document.kind, document.name, document.path
            );
        }
    }
    assert!(
        workloads >= 4,
        "only {workloads} controller workloads were found in the upstream installs; Argo CD, \
         Kargo, cert-manager and the operator are more"
    );
    for (path, kind, api_version, text) in &documents {
        for (refused, why) in REFUSED_KINDS {
            assert_ne!(
                kind, refused,
                "{path} declares a `{refused}`, which is {why}"
            );
        }
        assert!(
            !api_version.starts_with(REFUSED_API_GROUP),
            "{path} declares a {kind} of {api_version}, which is Config Sync's API group (ADR \
             0114: Config Connector reconciles the baseline)"
        );
        for image in images_in(text) {
            assert!(
                trading_binary_of(&image).is_none(),
                "{path} runs `{image}`, a trading binary, on the control plane; ADR 0036 and ADR \
                 0114 keep every qip-* binary off Kubernetes"
            );
        }
    }
}

#[test]
fn no_object_is_rendered_by_both_an_argo_cd_application_and_the_bootstrap() {
    // Two owners of one object is the failure Config Sync was once proposed
    // to solve and ADR 0114 solved by having one: the bootstrap step applies
    // the controllers and the Argo CD Application applies the environment,
    // and nothing is in both trees. Were a `RunService` or a `Namespace` in
    // both, the step and the controller would fight over it, and a reviewer
    // would read the losing side's manifest as the running system.
    for environment in ENVIRONMENTS {
        // Every base and overlay document under `bootstrap/` (the kustomize
        // reader refuses a file patch, and the bootstrap's bases use one), so
        // the set is a superset of what any one environment's step applies —
        // which errs towards finding a collision, never towards missing one.
        let mut bootstrap: BTreeSet<(String, String)> = BTreeSet::new();
        for manifest in manifests_under(BOOTSTRAP) {
            if manifest.kind() != "Kustomization" {
                bootstrap.insert((manifest.kind(), manifest.name()));
            }
        }
        for document in upstream_documents() {
            bootstrap.insert((document.kind.clone(), document.name.clone()));
        }
        for manifest in kustomize(&format!("{ARGOCD}/overlays/{environment}")) {
            bootstrap.insert((manifest.kind(), manifest.name()));
        }
        for manifest in kargo_rendered(environment) {
            bootstrap.insert((manifest.kind(), manifest.name()));
        }
        assert!(
            bootstrap.len() >= 100,
            "the bootstrap for {environment} renders only {} objects; the controllers' installs \
             alone are more",
            bootstrap.len()
        );
        let application: Vec<(String, String)> = kustomize(&format!("{ENVS}/{environment}"))
            .iter()
            .map(|manifest| (manifest.kind(), manifest.name()))
            .collect();
        assert!(
            application.iter().any(|(kind, _)| kind == "RunService"),
            "{ENVS}/{environment} renders no RunService; the Application would apply nothing \
             that runs"
        );
        for object in &application {
            assert!(
                !bootstrap.contains(object),
                "{} `{}` is rendered by both {ENVS}/{environment} (the Argo CD Application) and \
                 the bootstrap for {environment}; two appliers of one object fight, and the \
                 one that loses is the one a reviewer reads",
                object.0,
                object.1
            );
        }
    }
}

// --- ADR 0115: one build, promoted by digest ------------------------------------

/// The `deploy.yml` properties that make a later environment a reader of
/// dev's registry and never a builder, checked over the text so the check
/// can be fed a mutated copy without touching the protected workflow.
fn assert_later_environments_only_read_the_build_registry(
    deploy: &str,
    root_variables: &str,
    root_main: &str,
    promotion_tf: &str,
) {
    let build_environment: Vec<&str> = deploy
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("BUILD_ENVIRONMENT:"))
        .map(str::trim)
        .collect();
    assert_eq!(
        build_environment,
        vec!["dev"],
        "deploy.yml declares BUILD_ENVIRONMENT {build_environment:?}; exactly one, dev, is the \
         environment every image is built into (ADR 0115)"
    );
    let refusal = deploy
        .lines()
        .position(|line| {
            line.trim() == "- name: a promotion attests the image built once and never builds one"
        })
        .unwrap_or_else(|| {
            panic!("deploy.yml has no step refusing to build for a later environment (ADR 0115)")
        });
    let condition = deploy
        .lines()
        .skip(refusal + 1)
        .map(str::trim)
        .find(|line| line.starts_with("if:"))
        .unwrap_or_else(|| panic!("the refusal step has no `if:`; it would fire on every build"));
    assert!(
        condition.contains("env.TARGET_ENVIRONMENT != env.BUILD_ENVIRONMENT"),
        "the refusal step's condition is `{condition}`; it must fire exactly when the target is \
         not the build environment, or dev refuses its own build and a later environment \
         builds one"
    );
    assert!(
        condition.contains("steps.existing.outputs.digest == ''"),
        "the refusal step's condition is `{condition}`; it must fire when the build registry \
         holds no digest for the commit, which is the only case in which a promotion would \
         otherwise build"
    );
    assert!(
        deploy
            .lines()
            .any(|line| line.trim().starts_with("echo \"registry=")
                && line.contains("qip-${BUILD_ENVIRONMENT}")),
        "deploy.yml's registry output does not name qip-${{BUILD_ENVIRONMENT}}; a dispatch for \
         a later environment would then look for the digest in a registry nothing built into"
    );
    assert!(
        root_variables.contains("variable \"registry_promotion_readers\""),
        "variables.tf declares no registry_promotion_readers; later environments have no \
         identity that may read the build registry"
    );
    assert!(
        root_main
            .lines()
            .any(|line| collapsed(line) == "promotion_readers = var.registry_promotion_readers"),
        "main.tf does not pass registry_promotion_readers to the registry module"
    );
    let roles: Vec<&str> = promotion_tf
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("role"))
        .map(|rest| rest.trim_start_matches([' ', '=']).trim().trim_matches('"'))
        .collect();
    assert_eq!(
        roles,
        vec!["roles/artifactregistry.reader"],
        "modules/registry/promotion.tf grants {roles:?}; a later environment reads the build \
         registry and nothing that writes, or a promotion could push bytes nobody built"
    );
}

#[test]
fn later_environments_read_the_build_registry_and_build_nothing() {
    // The failure removed on 2026-10-11: each environment built into its own
    // registry, so a promotion to test wrote dev's digest under a path that
    // held no such bytes, and a rebuild there would have been a different
    // image from the one dev tested. The register still said "single
    // artifact promoted across environments" was the gap.
    assert_later_environments_only_read_the_build_registry(
        &read(".github/workflows/deploy.yml"),
        &read(ROOT_VARIABLES),
        &read("infrastructure/terraform/main.tf"),
        &read(&format!("{MODULES}/registry/promotion.tf")),
    );
}

// --- R12 GCP-10143: the blueprint's environments, mapped -------------------------

#[test]
fn every_blueprint_stage_maps_to_an_environment_directory_or_a_documented_environment_less_stage() {
    // The blueprint names integration, replay and paper environments; the
    // tree has four Terraform environments and a six-link Kargo chain. The
    // failure is a stage a person reads as an environment with a project,
    // secrets and a control plane behind it when it is a gate that records a
    // freight and runs nothing — or the reverse, an environment directory the
    // chain never reaches. Each Stage is one or the other, and the README
    // says which.
    let readme = read(&format!("{KARGO}/README.md"));
    let stages = stages_by_name(&kargo_rendered("dev"));
    assert_eq!(stages.len(), CHAIN.len(), "the chain is not {CHAIN:?}");
    let environment_directories = environment_directories();
    let mut environments = 0usize;
    let mut gates = 0usize;
    for name in stages.keys() {
        let tfvars = format!("infrastructure/environments/{name}/terraform.tfvars");
        let overlay = format!("{ENVS}/{name}/kustomization.yaml");
        let freight = format!("{GITOPS}/gates/{name}/freight.yaml");
        let has_tfvars = repository_root().join(&tfvars).is_file();
        let has_overlay = repository_root().join(&overlay).is_file();
        let has_freight = repository_root().join(&freight).is_file();
        if has_tfvars {
            environments += 1;
            assert!(
                has_overlay,
                "stage `{name}` has {tfvars} and no {overlay}; the Application would have \
                 nothing to apply"
            );
            assert!(
                environment_directories
                    .iter()
                    .any(|directory| directory == name),
                "stage `{name}` has {tfvars} but is not among the environment directories the \
                 Argo CD Applications target"
            );
            assert!(
                !has_freight,
                "stage `{name}` is an environment and also has {freight}; a gate record on an \
                 environment stage is a second account of what it runs"
            );
        } else {
            gates += 1;
            assert!(
                !has_overlay,
                "stage `{name}` has {overlay} and no {tfvars}; an environment with manifests and \
                 no project is applied nowhere"
            );
            assert!(
                has_freight,
                "stage `{name}` has no environment and no {freight}; a gate that records nothing \
                 has no audit trail"
            );
            // The README names it, as a delimited token, in the sentence that
            // says gates have no environment.
            let documented = readme.lines().any(|line| {
                line.split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|token| token == name)
                    && line.contains("no environment")
            });
            assert!(
                documented,
                "{KARGO}/README.md does not say `{name}` is a stage with no environment"
            );
        }
    }
    assert_eq!(
        environments, 4,
        "{environments} stages are environments; dev, test, stage and prod are four"
    );
    assert_eq!(
        gates, 2,
        "{gates} stages are gates; replay and paper are two"
    );
    // The blueprint's "integration" environment is the chain's `test`, and
    // the README says so in those words.
    let mapped = readme.lines().any(|line| {
        let tokens: Vec<&str> = line
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .collect();
        tokens.contains(&"integration") && tokens.contains(&"test")
    });
    assert!(
        mapped,
        "{KARGO}/README.md does not map the blueprint's integration environment to `test`"
    );
}

// --- R12-CICD-10203: no hand-copied model files, no undocumented configuration --

/// Model-weight file extensions. A weight file committed to the tree is a
/// model deployed by `git add`, outside the registry that records who trained
/// it, on what, and whether it beat its baseline.
const MODEL_WEIGHT_EXTENSIONS: [&str; 4] = ["onnx", "pt", "safetensors", "gguf"];

/// Paths under which a weight file would be declared, not hand-copied. Empty
/// today: the platform commits no weights, and a registry path is added here,
/// with the ADR that creates it, in the same change that puts a file there.
const DECLARED_MODEL_REGISTRY_PATHS: [&str; 0] = [];

fn undeclared_model_weights<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<String> {
    paths
        .filter(|path| {
            std::path::Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    MODEL_WEIGHT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
                })
        })
        .filter(|path| {
            !DECLARED_MODEL_REGISTRY_PATHS
                .iter()
                .any(|registry| path.starts_with(registry))
        })
        .map(str::to_string)
        .collect()
}

/// Every file under the repository that is not build output or a dependency
/// cache, relative to the root with `/` separators.
fn tracked_looking_files() -> Vec<String> {
    let root = repository_root();
    let mut found = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !matches!(name.as_str(), ".git" | "target" | "node_modules" | ".next") {
                    stack.push(path);
                }
            } else if let Ok(relative) = path.strip_prefix(&root) {
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn no_model_weights_are_committed_outside_a_declared_registry_path() {
    // R12-CICD-10203: "no hand-copied model files". Nothing in the tree held
    // that: a `.onnx` dropped next to a service would deploy with the image
    // and appear in no model record.
    let files = tracked_looking_files();
    assert!(
        files.len() > 1000,
        "only {} files were walked; the scan reached too little of the tree to mean anything",
        files.len()
    );
    assert!(files.iter().any(|path| path == "backend/Cargo.toml"));
    assert_eq!(
        undeclared_model_weights(files.iter().map(String::as_str)),
        Vec::<String>::new(),
        "a model weight file is committed outside a declared registry path"
    );

    // The detector sees each extension, in either case, and ignores a near
    // miss, so the empty result above is a finding and not a blind filter.
    let fixture = [
        "services/a/model.onnx",
        "services/a/model.PT",
        "x/weights.safetensors",
        "x/llm.gguf",
        "x/model.onnx.md",
        "x/notes-about-onnx.txt",
    ];
    assert_eq!(
        undeclared_model_weights(fixture.into_iter()),
        vec![
            "services/a/model.onnx".to_string(),
            "services/a/model.PT".to_string(),
            "x/weights.safetensors".to_string(),
            "x/llm.gguf".to_string(),
        ]
    );
}

/// The `QIP_*` variables a workload manifest source sets: `NAME = value`
/// map keys and `env_file_variable = "NAME"` in `catalogue.tf`, and
/// `NAME=value` lines in the execution node's startup template. A heredoc
/// delimiter or a name in a comment is not a variable and is not matched.
fn configured_variables(text: &str) -> BTreeSet<String> {
    let is_name = |word: &str| {
        word.starts_with("QIP_")
            && word
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    };
    let mut names = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if is_name(key) {
                names.insert(key.to_string());
            }
            if key == "env_file_variable" {
                let value = value.trim().trim_matches('"');
                if is_name(value) {
                    names.insert(value.to_string());
                }
            }
        }
    }
    names
}

fn undocumented_variables(configured: &BTreeSet<String>, docs: &str) -> Vec<String> {
    let documented: BTreeSet<&str> = docs
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    configured
        .iter()
        .filter(|name| !documented.contains(name.as_str()))
        .cloned()
        .collect()
}

#[test]
fn every_environment_variable_a_workload_reads_is_documented() {
    // R12-CICD-10203: "no undocumented emergency configuration". A variable
    // that Terraform sets and no page names is configuration an operator
    // learns about from an incident. Before docs/ops/runbooks/environment-variables.md
    // most of the execution node's and the signer's variables were in that
    // state, among them the fencing lease and node role, which decide whether
    // a second primary can exist.
    let mut configured = BTreeSet::new();
    for path in [
        "infrastructure/terraform/catalogue.tf",
        "infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl",
    ] {
        configured.extend(configured_variables(&qip_acceptance::read(path)));
    }
    assert!(
        configured.len() > 40,
        "only {} variables were found; the scan no longer reads the manifests",
        configured.len()
    );
    for known in ["QIP_DEPLOYMENT", "QIP_FENCING_LEASE", "QIP_TLS_MODE"] {
        assert!(
            configured.contains(known),
            "the scan missed a known variable"
        );
    }
    assert!(
        !configured.contains("QIP_EGRESS_BOOTSTRAP_EOF"),
        "a heredoc delimiter was read as a variable"
    );

    let mut docs = String::new();
    let documents = qip_acceptance::files_with_extension("docs/ops", "md");
    assert!(documents.len() > 10, "only {} documents", documents.len());
    for document in documents {
        docs.push_str(&std::fs::read_to_string(&document).expect("a document is readable"));
        docs.push('\n');
    }
    assert_eq!(
        undocumented_variables(&configured, &docs),
        Vec::<String>::new(),
        "Terraform sets a variable that no page under docs/ops names; add it to \
         docs/ops/runbooks/environment-variables.md"
    );

    // The check fails for an undocumented name and passes for a documented
    // one, on a mutated copy of the manifest text.
    let mutated = format!(
        "{}\n          QIP_EMERGENCY_OVERRIDE_UNDOCUMENTED = \"1\"\n",
        qip_acceptance::read("infrastructure/terraform/catalogue.tf")
    );
    assert_eq!(
        undocumented_variables(&configured_variables(&mutated), &docs),
        vec!["QIP_EMERGENCY_OVERRIDE_UNDOCUMENTED".to_string()]
    );
}

// --- R12-GCP-8114: what Config Connector may set IAM on -------------------------------

/// Whether a manifest is one of Config Connector's IAM resources — any kind
/// in `iam.cnrm.cloud.google.com`, which is every `IAM*` kind: the policy
/// kinds that bind a member (`IAMPolicyMember`, `IAMPartialPolicy`,
/// `IAMPolicy`, `IAMAuditConfig`) and the ones that mint an identity or a key
/// (`IAMServiceAccount`, `IAMServiceAccountKey`, `IAMCustomRole`, …), which
/// carry no `resourceRef` at all and so fail the rule below by construction.
fn is_config_connector_iam(manifest: &Manifest) -> bool {
    manifest.kind().starts_with("IAM")
        || manifest
            .api_version()
            .starts_with("iam.cnrm.cloud.google.com/")
}

/// Why an IAM manifest binds something other than a Cloud Run service, or
/// `None` when its `resourceRef` names a `RunService` in Config Connector's
/// Cloud Run group.
fn binds_beyond_a_run_service(manifest: &Manifest) -> Option<String> {
    let kind = text_at(&manifest.value, &["spec", "resourceRef", "kind"]);
    let group = text_at(&manifest.value, &["spec", "resourceRef", "apiVersion"]);
    let admitted = kind.as_deref() == Some("RunService")
        && group
            .as_deref()
            .is_some_and(|group| group.starts_with("run.cnrm.cloud.google.com/"));
    (!admitted).then(|| {
        format!(
            "{} has resourceRef kind {kind:?} in {group:?}",
            manifest.describe()
        )
    })
}

/// R12-GCP-8114, the manifest half: every IAM manifest Argo CD can apply
/// binds a Cloud Run service and nothing wider. Config Connector applies
/// these as its own identity, so an `IAMPolicyMember` whose `resourceRef` is
/// the project, a secret or a service account is a grant no Terraform
/// scanner reads — `roles/secretmanager.secretAccessor` on the venue
/// credential for a Cloud Run account is one manifest away. The reconciler's
/// project roles (the next test) refuse such a grant at the API; this
/// refuses it in review, before a sync is attempted.
///
/// Read twice per environment: as committed files, which catches a manifest
/// no kustomization lists yet, and as the kustomization renders them, which
/// catches a patch rewriting a `resourceRef`.
///
/// Mutation: add an `IAMPolicyMember` with `resourceRef.kind: Project` under
/// `infrastructure/gitops/envs/dev/`, and the test names it.
#[test]
fn every_iam_manifest_under_envs_binds_a_cloud_run_service_and_nothing_wider() {
    // The rule, on documents whose answer is known.
    let manifest = |value: Value| Manifest {
        path: "premise".to_string(),
        value,
    };
    let invoker = manifest(serde_json::json!({
        "apiVersion": "iam.cnrm.cloud.google.com/v1beta1",
        "kind": "IAMPolicyMember",
        "metadata": {"name": "x"},
        "spec": {"member": "group:g@example.com", "role": "roles/run.invoker",
                 "resourceRef": {"apiVersion": "run.cnrm.cloud.google.com/v1beta1",
                                 "kind": "RunService", "name": "qip-dev-api"}}
    }));
    let project = manifest(serde_json::json!({
        "apiVersion": "iam.cnrm.cloud.google.com/v1beta1",
        "kind": "IAMPolicyMember",
        "metadata": {"name": "x"},
        "spec": {"member": "serviceAccount:a@p.iam.gserviceaccount.com",
                 "role": "roles/secretmanager.secretAccessor",
                 "resourceRef": {"apiVersion": "resourcemanager.cnrm.cloud.google.com/v1beta1",
                                 "kind": "Project", "external": "projects/p"}}
    }));
    let key = manifest(serde_json::json!({
        "apiVersion": "iam.cnrm.cloud.google.com/v1beta1",
        "kind": "IAMServiceAccountKey",
        "metadata": {"name": "x"},
        "spec": {"serviceAccountRef": {"name": "qip-dev-api"}}
    }));
    let service = manifest(serde_json::json!({
        "apiVersion": "run.cnrm.cloud.google.com/v1beta1",
        "kind": "RunService",
        "metadata": {"name": "qip-dev-api"}
    }));
    assert!(
        [&invoker, &project, &key]
            .iter()
            .all(|m| is_config_connector_iam(m))
            && !is_config_connector_iam(&service),
        "premise: the IAM-kind detector misreads a known document"
    );
    assert!(
        binds_beyond_a_run_service(&invoker).is_none()
            && binds_beyond_a_run_service(&project).is_some()
            && binds_beyond_a_run_service(&key).is_some(),
        "premise: the resourceRef rule admits a project grant or a key, or refuses an invoker \
         on a RunService"
    );

    let mut iam_read = 0usize;
    let mut wider = Vec::new();
    for environment in environment_directories() {
        let directory = format!("{ENVS}/{environment}");
        let mut documents = manifests_under(&directory);
        documents.extend(kustomize(&directory));
        for document in documents.iter().filter(|m| is_config_connector_iam(m)) {
            iam_read += 1;
            if let Some(finding) = binds_beyond_a_run_service(document) {
                wider.push(finding);
            }
        }
    }
    // Dev's two invoker bindings at least, read both ways: with none the
    // rule below held over nothing.
    assert!(
        iam_read >= 2,
        "premise: {iam_read} IAM manifest(s) were read under {ENVS}; dev's invoker bindings \
         are not being seen"
    );
    assert!(
        wider.is_empty(),
        "an IAM manifest under {ENVS} binds something other than a Cloud Run service. Config \
         Connector applies it as its own identity, and a grant on the project, a secret or an \
         account here is one no Terraform scanner reads. Grant it in Terraform, in the module \
         that owns the resource, or bind a RunService:\n{}",
        wider.join("\n")
    );
}

/// Config Connector's account, by every name the Terraform tree uses for it.
const CONFIG_CONNECTOR_IDENTITY: [&str; 2] =
    ["google_service_account.kcc.", "kcc_service_account_email"];

/// The project roles Config Connector holds, pinned, and what each lets it
/// do with IAM — which is the whole of the argument for the pin.
const CONFIG_CONNECTOR_PROJECT_ROLES: [(&str, &str); 2] = [
    (
        "roles/run.admin",
        "creates and updates Cloud Run services and sets IAM on them (run.services.setIamPolicy) \
         and on nothing outside Cloud Run",
    ),
    (
        "roles/secretmanager.viewer",
        "reads a secret's metadata so a RunService's mount resolves; never a payload, never a \
         policy",
    ),
];

/// Whether a role lets its holder set IAM policy outside Cloud Run, or act as
/// another identity that could. Fail closed on what cannot be read: a custom
/// role's permissions are not in its name, so one is treated as able to.
fn sets_iam_beyond_cloud_run(role: &str) -> bool {
    let Some(id) = role.strip_prefix("roles/") else {
        return true;
    };
    let folded = id.to_ascii_lowercase();
    if folded.ends_with("serviceagent") {
        return true;
    }
    if folded.starts_with("run.") {
        return false;
    }
    matches!(
        folded.as_str(),
        "owner"
            | "editor"
            | "iam.serviceaccountuser"
            | "iam.serviceaccounttokencreator"
            | "iam.serviceaccountopenidtokencreator"
            | "iam.workloadidentityuser"
    ) || folded.ends_with("admin")
}

/// Every `resource "<kind>" "<name>"` block of the given kinds in the
/// Terraform tree, as `(file, kind, name, body)` with comments gone.
fn terraform_resources_of(kinds: &[&str]) -> Vec<(String, String, String, String)> {
    let mut found = Vec::new();
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let text = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        let file = path
            .strip_prefix(repository_root())
            .unwrap_or(&path)
            .display()
            .to_string();
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            let Some(kind) = kinds
                .iter()
                .find(|kind| line.starts_with(&format!("resource \"{kind}\" \"")))
            else {
                continue;
            };
            let name = line.split('"').nth(3).unwrap_or_default().to_string();
            let mut body = vec![line];
            let mut depth = line.matches('{').count() as i64 - line.matches('}').count() as i64;
            while depth > 0 {
                let Some(next) = lines.next() else { break };
                depth += next.matches('{').count() as i64 - next.matches('}').count() as i64;
                body.push(next);
            }
            found.push((file.clone(), (*kind).to_string(), name, body.join("\n")));
        }
    }
    found
}

/// The literal on the right of `key = "…"` in a block, whitespace collapsed,
/// or `None` when the value is not a single literal.
fn literal_assignment(body: &str, key: &str) -> Option<String> {
    body.lines().map(collapsed).find_map(|line| {
        let value = line.strip_prefix(&format!("{key} = "))?;
        let inner = value.strip_prefix('"')?.strip_suffix('"')?;
        (!inner.contains('"') && !inner.contains("${")).then(|| inner.to_string())
    })
}

/// R12-GCP-8114, the identity half: Config Connector — the account every
/// manifest above is applied as — holds exactly `roles/run.admin` and
/// `roles/secretmanager.viewer` at project scope and nowhere higher, neither
/// of which can set IAM policy outside Cloud Run or act as another account;
/// and the accounts it may act as (`iam.serviceAccountUser`, which Cloud Run
/// demands of a deployer) are the Cloud Run workloads' own, never the
/// execution node's. So the reconciler can neither grant a Cloud Run account
/// the venue credential nor deploy a Cloud Run service running as the one
/// account that reads it.
///
/// Read across the whole Terraform tree rather than the module alone: a
/// grant to `module.gitops_control_plane[0].kcc_service_account_email`
/// written in `catalogue.tf` is the same grant.
///
/// Mutation: add a `google_project_iam_member` granting
/// `roles/iam.securityAdmin` to `google_service_account.kcc.email` in any
/// `.tf` under `infrastructure/terraform/`; the rule names it. A viewer role
/// instead fails the pin.
#[test]
fn config_connector_holds_a_pinned_set_of_project_roles_none_of_which_sets_iam_beyond_cloud_run() {
    // The rule, on roles whose answer is known.
    for refused in [
        "roles/owner",
        "roles/editor",
        "roles/resourcemanager.projectIamAdmin",
        "roles/iam.securityAdmin",
        "roles/iam.serviceAccountUser",
        "roles/iam.serviceAccountTokenCreator",
        "roles/iam.workloadIdentityUser",
        "roles/secretmanager.admin",
        "roles/storage.admin",
        "roles/run.serviceAgent",
        "projects/p/roles/deployer",
    ] {
        assert!(
            sets_iam_beyond_cloud_run(refused),
            "premise: the rule admits {refused}"
        );
    }
    for (role, _) in CONFIG_CONNECTOR_PROJECT_ROLES {
        assert!(
            !sets_iam_beyond_cloud_run(role),
            "the pinned role {role} fails the rule it is pinned under"
        );
    }

    let names_kcc = |body: &str| {
        CONFIG_CONNECTOR_IDENTITY
            .iter()
            .any(|name| body.contains(name))
    };
    let above_a_resource = terraform_resources_of(&[
        "google_project_iam_member",
        "google_project_iam_binding",
        "google_project_iam_policy",
        "google_folder_iam_member",
        "google_folder_iam_binding",
        "google_folder_iam_policy",
        "google_organization_iam_member",
        "google_organization_iam_binding",
        "google_organization_iam_policy",
    ]);
    assert!(
        above_a_resource.len() > 10,
        "premise: the walk found {} project, folder or organisation grants",
        above_a_resource.len()
    );
    let mut held = BTreeSet::new();
    let mut located = Vec::new();
    for (file, kind, name, body) in above_a_resource
        .iter()
        .filter(|(_, _, _, body)| names_kcc(body))
    {
        let role = literal_assignment(body, "role").unwrap_or_else(|| {
            panic!(
                "{file}: {kind}.{name} grants Config Connector a role that is not a literal, so \
                 it cannot be pinned; write the role out"
            )
        });
        assert!(
            !sets_iam_beyond_cloud_run(&role),
            "{file}: {kind}.{name} grants Config Connector {role}, which sets IAM policy \
             beyond Cloud Run services or acts as another identity. The reconciler applies \
             every manifest under {ENVS} as itself; with this it could grant a Cloud Run \
             account the venue credential"
        );
        held.insert(role);
        located.push(format!("{file}:{kind}.{name}"));
    }
    located.sort();
    let pinned: BTreeSet<String> = CONFIG_CONNECTOR_PROJECT_ROLES
        .iter()
        .map(|(role, _)| (*role).to_string())
        .collect();
    assert_eq!(
        held, pinned,
        "Config Connector's project roles are {held:?}, pinned at {pinned:?} (granted at \
         {located:?}). A change here is a change to what every manifest under {ENVS} may do; \
         re-pin it with the reason, in CONFIG_CONNECTOR_PROJECT_ROLES"
    );
    assert_eq!(
        located,
        [
            "infrastructure/terraform/modules/gitops-control-plane/main.tf:google_project_iam_member.kcc_runs",
            "infrastructure/terraform/modules/gitops-control-plane/main.tf:google_project_iam_member.kcc_sees_secrets",
        ],
        "Config Connector's project roles are granted somewhere other than its module's two \
         named grants"
    );

    // Whom it may act as: Cloud Run's own workload accounts, one by one.
    let acting_as: Vec<String> = terraform_resources_of(&[
        "google_service_account_iam_member",
        "google_service_account_iam_binding",
        "google_service_account_iam_policy",
    ])
    .into_iter()
    .filter(|(file, _, _, body)| {
        names_kcc(body)
            || (file.ends_with("modules/cloudrun/main.tf")
                && body.contains("var.deployer_service_account"))
    })
    .map(|(file, kind, name, body)| {
        let account = body
            .lines()
            .map(collapsed)
            .find_map(|line| {
                line.strip_prefix("service_account_id = ")
                    .map(str::to_string)
            })
            .unwrap_or_default();
        format!("{file}:{kind}.{name} on {account}")
    })
    .collect();
    let mut acting_as = acting_as;
    acting_as.sort();
    assert_eq!(
        acting_as,
        [
            "infrastructure/terraform/catalogue.tf:google_service_account_iam_member.portal_deployer on module.secrets.console_service_account_id",
            "infrastructure/terraform/modules/cloudrun/main.tf:google_service_account_iam_member.deployer on google_service_account.workload.name",
        ],
        "Config Connector may act as an account other than a Cloud Run workload's own. One \
         that may read the venue credential — the execution node's — lets it deploy a Cloud Run \
         service that reads it"
    );
}
