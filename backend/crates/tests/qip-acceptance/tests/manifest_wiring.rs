//! Does the deployment configure the binary it runs?
//!
//! Every check here is about one seam: a deployment sets environment
//! variables, a binary reads environment variables, and nothing in either file
//! mentions the other. The two drift silently in both directions, and both
//! directions have already happened in this repository:
//!
//! * **A variable nothing reads.** The Kubernetes manifests set
//!   `QIP_AUTONOMY_CEILING` for three binaries while none of them read it. An
//!   operator lowering the ceiling changed nothing at all — and the manifest,
//!   the config map and the audit log all recorded a change. That is worse than
//!   an absent control, because an absent control is visibly absent.
//! * **A variable nothing sets.** `QIP_MESH_CELLS` and `QIP_MESH_PEER` appeared
//!   in no manifest at all, so the backbone was dead in every deployment while
//!   being complete and tested in process.
//! * **A variable nothing sets, that nothing refuses to start without.**
//!   `qip-fastbrain` reads `QIP_CONNECTOR_SOURCE` and `QIP_CONNECTOR_BASE_URL`
//!   to select a licensed live market source, and no manifest set either, so
//!   every deployed Fast Brain ran the synthetic exchange while the gap matrix
//!   recorded the capability as done. An optional variable nothing sets
//!   produces a healthy process running a fallback for ever, which is what the
//!   `READ_BUT_NOT_SET` allowlist below exists to make a decision rather than
//!   an accident.
//!
//! Neither is visible in a diff of either file alone, and neither produces an
//! error anywhere. That is what makes this a test rather than a review note.
//!
//! # Which deployment
//!
//! Two artefacts, and both are walked. `infrastructure/terraform/catalogue.tf`
//! is the Cloud Run catalogue: one entry per central workload, its `env` and
//! its `secret_mounts`, which is what a service actually receives (ADR 0024).
//! `modules/execution-node/templates/startup.sh.tftpl` writes the node's
//! `node.env`, which is what `qip-edge-node` receives on the one machine that
//! is not a container. A variable correct in one of them says nothing about
//! the other.
//!
//! # How the two sides are derived
//!
//! Both from the source, never from a list here. A list would be a third copy
//! of the same fact and would drift from both of the copies it was written to
//! reconcile.
//!
//! The deployment side is the `QIP_…` keys in an entry's `env`, plus the
//! `env_file_variable` of each mounted secret, plus the `KEY=value` lines of
//! `node.env`. The binary side is every `QIP_…` a binary could reach:
//! quote-delimited literals in its own crate, the `const NAME: &str = "QIP_…"`
//! declarations it references by identifier, and — because a composition root
//! may delegate the whole read — the literals in the module defining any
//! `Type::from_env` it calls.
//!
//! The bias is deliberate and worth naming: resolving a constant or a
//! `from_env` module can only *add* names to the read set, so the failure mode
//! of getting a rule slightly too broad is a variable this test tolerates,
//! never a deployment it wrongly rejects. Each rule therefore asserts it
//! contributed something, so a rule that silently stops resolving anything
//! fails here instead of quietly widening the test.

// The workspace denies `panic_in_result_fn` because an assertion that aborts a
// `Result`-returning function is a bug in production code. These tests return
// `()` and assert; the lint does not apply, and neither does the reason for it.

use qip_acceptance::{files_with_extension, read, repository_root};
use std::collections::{BTreeMap, BTreeSet};

const CATALOGUE: &str = "infrastructure/terraform/catalogue.tf";
const NODE_STARTUP: &str =
    "infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl";
const ROOT_VARIABLES: &str = "infrastructure/terraform/variables.tf";
/// The Config Connector manifests ADR 0036 deploys the catalogue through:
/// one `RunService` per workload per environment under `envs/<env>/`. Each
/// is the catalogue entry rendered for that environment, and each is walked
/// here as a deployment of its own, because a variable the catalogue sets
/// and a manifest omits is set by nothing the reconciler applies.
const GITOPS_ENVS: &str = "infrastructure/gitops/envs";
const ENVIRONMENTS: [&str; 4] = ["dev", "test", "stage", "prod"];

// ---------------------------------------------------------------------------
// Allowlist
// ---------------------------------------------------------------------------

/// Variables a deployment sets that the binary does not read, with the reason.
///
/// One entry, and it is not a convenience. Every addition here is a control
/// that does nothing, so an entry has to argue that the workload is better off
/// with a variable it ignores than without it — and
/// `every_allowlisted_variable_is_still_set_and_still_unread` deletes the
/// entry's licence the moment either half stops being true.
const SET_BUT_NOT_READ: &[(&str, &str, &str)] = &[(
    "node.env",
    "QIP_EXECUTION_NODE_SHADOW",
    "A marker and not a control, and the startup script says so where it \
     writes it. What actually prevents a shadow-mode node from taking a venue \
     session is the absence of a venue egress rule and the absence of a \
     venue-credential binding, both decided in Terraform and visible in a \
     plan; `modules/execution-node` creates neither while `shadow_mode` is \
     true. The binary does not read this variable today. It stays set because \
     a machine's configuration should say out loud which mode its Terraform \
     put it in — an operator on the box reads node.env, not a plan — and \
     because the day the binary reads it, it reads it as a second layer behind \
     the firewall and never as the first. The entry ends when qip-edge-node \
     reads the value, which is work in crates/apps/, not here.",
)];

// ---------------------------------------------------------------------------
// The deployment
// ---------------------------------------------------------------------------

/// A configuration with its comments removed.
///
/// Load-bearing rather than tidiness: the catalogue argues with the reader at
/// length, and several of the comments name the exact variables being checked
/// — the header explains why it does *not* set `QIP_MESH_CELLS`. A check that
/// cannot tell a mention from a setting makes it impossible to document why a
/// variable is absent, which is the documentation most worth having.
fn without_comments(content: &str) -> String {
    content
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The catalogue's workload entries, as `(name, body)`, comments stripped.
///
/// An entry opens at four spaces of indent with `name = {` and closes at a
/// line that is exactly `    }`. Brittle on purpose: a reformatted catalogue
/// makes this return nothing, and the assertion below is what turns that into
/// a loud failure rather than a check that quietly stops checking.
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
    assert!(
        entries.len() >= 3,
        "only {} catalogue entries were found; the walk is not reaching them \
         and every check built on it would pass vacuously",
        entries.len()
    );
    entries
}

/// The value of a scalar field at the top level of a catalogue entry.
fn catalogue_field(body: &str, field: &str) -> String {
    body.lines()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            let indent = line.len() - line.trim_start().len();
            (indent == 6 && key.trim() == field).then(|| value.trim().trim_matches('"').to_string())
        })
        .unwrap_or_else(|| panic!("the catalogue entry has no `{field}` field:\n{body}"))
}

/// The `QIP_` variables a catalogue entry sets: every `env` key, wherever it
/// sits in the entry — the fast brain's `env` is a `merge` of two maps — and
/// the `_FILE` variable of every mounted secret.
///
/// Exact names, collected into a set, because every comparison downstream is
/// membership rather than substring. `contains("QIP_MESH_PEER")` is true of a
/// deployment setting `QIP_MESH_PEER_TIMEOUT` and of a comment mentioning
/// either, and this file exists to catch a variable being one character
/// different from the one that is read.
fn variables_an_entry_sets(body: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for line in body.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.starts_with("QIP_")
            && key
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            set.insert(key.to_string());
        }
        if key == "env_file_variable" {
            set.insert(value.trim().trim_matches('"').to_string());
        }
    }
    set
}

/// The variables the node's startup script writes into `node.env`.
///
/// The heredoc between `cat >"$CONF_DIR/node.env" <<EOF` and the next `EOF`,
/// one `KEY=value` per line. The values are Terraform template expressions
/// and are not interpreted; only the names matter here.
fn variables_the_node_sets() -> BTreeSet<String> {
    let startup = read(NODE_STARTUP);
    let block = startup
        .split("cat >\"$CONF_DIR/node.env\" <<EOF\n")
        .nth(1)
        .and_then(|rest| rest.split("\nEOF\n").next())
        .expect("the startup script writes node.env");
    let set: BTreeSet<String> = block
        .lines()
        .filter_map(|line| line.split_once('=').map(|(key, _)| key.trim().to_string()))
        .filter(|key| key.starts_with("QIP_"))
        .collect();
    assert!(
        set.len() >= 5,
        "only {set:?} were read out of node.env; the heredoc has moved and this \
         check is reading nothing"
    );
    set
}

/// The binary the node runs, from the unit's `ExecStart`.
fn node_binary() -> String {
    // The startup script writes two units: the egress proxy's, whose
    // ExecStart is the vendored Envoy, and the node's. Only the second runs
    // a binary this workspace builds, so the walk starts at its
    // Description line rather than at the first ExecStart in the file —
    // which is the proxy's, and would make every test here ask whether
    // `envoy` is a crate.
    let startup = read(NODE_STARTUP);
    let unit = startup
        .split("Description=qip execution node")
        .nth(1)
        .expect("the startup script writes a unit described as the execution node");
    unit.lines()
        .find_map(|line| line.trim().strip_prefix("ExecStart=/usr/local/bin/"))
        .map(|rest| rest.split_whitespace().next().unwrap_or(rest).to_string())
        .expect("the node's unit names the binary it runs")
}

/// Every deployed thing, as (where it is configured, the binary, the
/// variables it is given).
fn deployments() -> Vec<(String, String, BTreeSet<String>)> {
    let mut found: Vec<(String, String, BTreeSet<String>)> = catalogue_workloads()
        .into_iter()
        .map(|(name, body)| {
            let binary = catalogue_field(&body, "binary");
            assert!(
                repository_root()
                    .join(format!("backend/crates/apps/{binary}/src"))
                    .exists(),
                "the catalogue's {name} runs {binary}, which is not a crate under crates/apps"
            );
            (
                format!("catalogue.tf:{name}"),
                binary,
                variables_an_entry_sets(&body),
            )
        })
        .collect();
    let node = node_binary();
    assert!(
        repository_root()
            .join(format!("backend/crates/apps/{node}/src"))
            .exists(),
        "the node's unit runs {node}, which is not a crate under crates/apps"
    );
    found.push(("node.env".to_string(), node, variables_the_node_sets()));
    found.extend(run_service_deployments());
    assert!(
        found.len() >= 4 + 3 * ENVIRONMENTS.len(),
        "only {} deployments were matched to a crate; the walk from a \
         deployment to the binary that builds it is finding nothing",
        found.len()
    );
    found
}

/// The `RunService` documents under one environment's directory, as
/// `(file, document text)`. Line-based, like every other walk here: a
/// document is the text between `---` separators, and it is a RunService
/// when it carries a column-zero `kind: RunService`.
fn run_service_documents(environment: &str) -> Vec<(String, String)> {
    let directory = format!("{GITOPS_ENVS}/{environment}");
    assert!(
        repository_root().join(&directory).is_dir(),
        "{directory} does not exist; ADR 0036 decision 4 puts one RunService per workload \
         there, and until it lands this walk has no manifest to read"
    );
    let mut documents = Vec::new();
    for extension in ["yaml", "yml"] {
        for path in files_with_extension(&directory, extension) {
            let content = std::fs::read_to_string(&path).expect("readable");
            let display = path
                .strip_prefix(repository_root())
                .unwrap_or(&path)
                .display()
                .to_string();
            for document in content.split("\n---") {
                if document
                    .lines()
                    .any(|line| line.trim_end() == "kind: RunService")
                {
                    documents.push((display.clone(), document.to_string()));
                }
            }
        }
    }
    documents
}

/// The `QIP_` variables a RunService sets: every `name: QIP_…` entry of a
/// container's `env` list. Exact identifiers, for the reason
/// `variables_an_entry_sets` gives.
fn variables_a_run_service_sets(document: &str) -> BTreeSet<String> {
    document
        .lines()
        .filter_map(|line| {
            let entry = line.trim_start();
            let entry = entry.strip_prefix("- ").unwrap_or(entry);
            let value = entry.strip_prefix("name:")?.trim().trim_matches('"');
            (value.starts_with("QIP_")
                && value
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .then(|| value.to_string())
        })
        .collect()
}

/// Every catalogue workload's RunService in every environment, as a
/// deployment: `(envs/<env>:<workload>, binary, variables)`.
fn run_service_deployments() -> Vec<(String, String, BTreeSet<String>)> {
    let catalogue = catalogue_workloads();
    let mut found = Vec::new();
    for environment in ENVIRONMENTS {
        let documents = run_service_documents(environment);
        for (name, body) in &catalogue {
            let binary = catalogue_field(body, "binary");
            let service = format!("qip-{environment}-{name}");
            let matching: Vec<&(String, String)> = documents
                .iter()
                .filter(|(_, document)| {
                    document
                        .lines()
                        .any(|line| line.trim_end() == format!("  name: {service}"))
                })
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "{} RunService document(s) under {GITOPS_ENVS}/{environment} are named \
                 `{service}`; the catalogue's {name} is deployed by exactly one",
                matching.len()
            );
            let set = variables_a_run_service_sets(&matching[0].1);
            assert!(
                set.len() >= 3,
                "{} sets only {set:?} for {binary}; the env list is not being read",
                matching[0].0
            );
            found.push((format!("envs/{environment}:{name}"), binary, set));
        }
    }
    found
}

/// The variables a catalogue entry sets only when a root variable is
/// non-null — the keys inside a `var.x == null ? {} : {` arm of its `env`
/// merge. A RunService is the entry rendered for one environment, and an
/// environment whose tfvars leave that variable null renders none of them;
/// that absence is the tfvars' reviewed decision, not a variable nothing
/// sets, and it is admitted for a RunService alone.
fn catalogue_conditional_variables() -> BTreeMap<String, BTreeSet<String>> {
    let mut by_workload = BTreeMap::new();
    for (name, body) in catalogue_workloads() {
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
                    if key.trim().starts_with("QIP_") {
                        names.insert(key.trim().to_string());
                    }
                }
            }
        }
        by_workload.insert(name, names);
    }
    by_workload
}

// ---------------------------------------------------------------------------
// The binaries
// ---------------------------------------------------------------------------

/// Whether `text` mentions `word` as a whole identifier.
fn mentions_identifier(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(index, _)| {
        let before = text[..index].chars().next_back();
        let after = text[index + word.len()..].chars().next();
        let boundary =
            |character: Option<char>| !character.is_some_and(|c| c.is_alphanumeric() || c == '_');
        boundary(before) && boundary(after)
    })
}

/// The quote-delimited `"QIP_…"` literals in a piece of source.
///
/// Delimited on both sides deliberately. `qip-edge-node` builds its list of
/// unmet production requirements out of strings that *begin* with a variable
/// name, and a rule that took everything after `"QIP_` would read that
/// sentence as a variable the binary reads.
fn variable_literals(text: &str) -> BTreeSet<String> {
    text.split("\"QIP_")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        })
        .map(|name| format!("QIP_{name}"))
        .collect()
}

/// Every `.rs` file under one crate's `src`, as text.
fn sources_under(relative: &str) -> Vec<String> {
    files_with_extension(relative, "rs")
        .iter()
        .map(|path| std::fs::read_to_string(path).expect("a source file is readable"))
        .collect()
}

/// Every `const NAME: &str = "QIP_…";` in the workspace, by identifier.
fn variables_by_constant() -> BTreeMap<String, BTreeSet<String>> {
    let mut declarations: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in files_with_extension("backend/crates", "rs") {
        if !path.to_string_lossy().contains("/src/") {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("a source file is readable");
        for line in content.lines() {
            let Some(rest) = line.split_once("const ").map(|(_, rest)| rest) else {
                continue;
            };
            let Some((identifier, tail)) = rest.split_once(':') else {
                continue;
            };
            if !tail.trim_start().starts_with("&str") {
                continue;
            }
            for variable in variable_literals(tail) {
                declarations
                    .entry(identifier.trim().to_string())
                    .or_default()
                    .insert(variable);
            }
        }
    }
    assert!(
        declarations.len() >= 10,
        "only {} environment-variable constants were found; the declaration \
         walk has stopped matching the form these are written in",
        declarations.len()
    );
    declarations
}

/// The types a binary constructs from the environment: the `X` in
/// `X::from_env(`.
fn types_read_from_the_environment(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (index, _) in text.match_indices("::from_env(") {
        let identifier: String = text[..index]
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if identifier
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_uppercase())
        {
            found.insert(identifier);
        }
    }
    found
}

/// Every `QIP_` variable one binary can read, and by which rule.
struct VariablesRead {
    literal: BTreeSet<String>,
    by_constant: BTreeSet<String>,
    by_delegation: BTreeSet<String>,
}

impl VariablesRead {
    fn all(&self) -> BTreeSet<String> {
        self.literal
            .union(&self.by_constant)
            .chain(self.by_delegation.iter())
            .cloned()
            .collect()
    }
}

fn variables_read_by(
    binary: &str,
    constants: &BTreeMap<String, BTreeSet<String>>,
) -> VariablesRead {
    let sources = sources_under(&format!("backend/crates/apps/{binary}/src"));
    assert!(
        !sources.is_empty(),
        "no source was found for {binary}; the walk from a deployment to the \
         crate that builds it is broken"
    );

    let mut literal = BTreeSet::new();
    let mut by_constant = BTreeSet::new();
    let mut by_delegation = BTreeSet::new();
    let mut delegates = BTreeSet::new();

    for source in &sources {
        literal.extend(variable_literals(source));
        for (identifier, variables) in constants {
            if mentions_identifier(source, identifier) {
                by_constant.extend(variables.iter().cloned());
            }
        }
        delegates.extend(types_read_from_the_environment(source));
    }

    // A delegate defined in the binary's own crate wins: `MeshSettings` is
    // the name of two different types, and a workspace-wide search gave the
    // API the cell's variables.
    let workspace: Vec<(String, String)> = files_with_extension("backend/crates", "rs")
        .iter()
        .filter(|path| path.to_string_lossy().contains("/src/"))
        .map(|path| {
            (
                path.to_string_lossy().to_string(),
                std::fs::read_to_string(path).expect("a source file is readable"),
            )
        })
        .collect();
    let home = format!("backend/crates/apps/{binary}/src/");
    for delegate in &delegates {
        let marker = format!("impl {delegate} {{");
        let defining: Vec<&(String, String)> = workspace
            .iter()
            .filter(|(_, content)| content.contains(&marker))
            .collect();
        let own: Vec<&(String, String)> = defining
            .iter()
            .filter(|(path, _)| path.contains(&home))
            .copied()
            .collect();
        let chosen = if own.is_empty() { defining } else { own };
        for (_, content) in chosen {
            by_delegation.extend(variable_literals(content));
            for (identifier, variables) in constants {
                if mentions_identifier(content, identifier) {
                    by_delegation.extend(variables.iter().cloned());
                }
            }
        }
    }

    VariablesRead {
        literal,
        by_constant,
        by_delegation,
    }
}

/// The variables a binary refuses to start without.
fn variables_required_by(binary: &str) -> BTreeSet<String> {
    sources_under(&format!("backend/crates/apps/{binary}/src"))
        .iter()
        .flat_map(|source| {
            ["required(\"", "required_secret(\""]
                .iter()
                .flat_map(|call| {
                    source
                        .split(call)
                        .skip(1)
                        .filter_map(|rest| rest.split('"').next())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .filter(|name| name.starts_with("QIP_"))
        .collect()
}

/// The variables a binary refuses to *start with*, read out of the refusal's
/// own message.
fn variables_refused_by(binary: &str) -> BTreeSet<String> {
    let sources = sources_under(&format!("backend/crates/apps/{binary}/src"));
    let mut refused = BTreeSet::new();
    for source in &sources {
        for name in variable_literals(source) {
            if sources
                .iter()
                .any(|text| text.contains(&format!("{name} is no longer read")))
            {
                refused.insert(name);
            }
        }
    }
    refused
}

/// Whether a deployment setting `set` satisfies a binary that reads `read`.
///
/// `qip_core::secret` accepts a `_FILE` variant of any credential variable, and
/// it is the one every deployment here uses for anything mounted from Secret
/// Manager: a signing key in the environment is a key in `/proc/<pid>/environ`,
/// in every child process and in every crash dump.
fn satisfies(read: &BTreeSet<String>, set: &str) -> bool {
    read.contains(set)
        || set
            .strip_suffix("_FILE")
            .is_some_and(|base| read.contains(base))
}

// ---------------------------------------------------------------------------
// Both directions of the seam
// ---------------------------------------------------------------------------

#[test]
fn every_variable_a_deployment_sets_is_one_the_binary_it_runs_actually_reads() {
    let constants = variables_by_constant();
    let mut checked = 0usize;
    let mut satisfied_by_the_file_variant = 0usize;
    let mut resolved_by_constant = 0usize;
    let mut resolved_by_delegation = 0usize;

    for (place, binary, set) in deployments() {
        let read = variables_read_by(&binary, &constants);
        let literal = read.literal.clone();
        let all = read.all();
        assert!(
            all.len() >= 3,
            "{binary} appears to read only {all:?}; the source walk has stopped \
             finding variables and every deployment would now pass"
        );
        assert!(
            !set.is_empty(),
            "{place} sets no QIP_ variable for {binary}; the env is no longer \
             being parsed and this check would pass on anything"
        );

        for variable in &set {
            if SET_BUT_NOT_READ
                .iter()
                .any(|(file, name, _)| place.ends_with(file) && name == variable)
            {
                continue;
            }
            assert!(
                satisfies(&all, variable),
                "{place} sets {variable} for {binary} and that binary reads no \
                 such variable. A deployment that sets a variable nothing reads \
                 presents a control that does nothing: an operator who changes \
                 it sees a diff, a new revision and no effect whatsoever. It \
                 reads {all:?}."
            );
            if !all.contains(variable) {
                satisfied_by_the_file_variant += 1;
            }
            if !literal.contains(variable) && read.by_constant.contains(variable) {
                resolved_by_constant += 1;
            }
            if !literal.contains(variable)
                && !read.by_constant.contains(variable)
                && read.by_delegation.contains(variable)
            {
                resolved_by_delegation += 1;
            }
            checked += 1;
        }
    }

    // The premises. Each of these is a way this test could pass while
    // measuring nothing, and the last two are the resolution rules that keep
    // it from being an allowlist.
    assert!(
        checked >= 15,
        "only {checked} variables were checked across every deployment; the \
         catalogue or the source walk is finding almost nothing"
    );
    assert!(
        satisfied_by_the_file_variant >= 1,
        "no variable was satisfied by its `_FILE` variant, so the rule that \
         maps a mounted file back to the variable the binary reads is never \
         exercised and could have stopped working"
    );
    assert!(
        resolved_by_constant >= 1,
        "no variable was resolved through a `const NAME: &str = \"QIP_…\"` \
         declaration; `qip-fastbrain` reads QIP_STORAGE_TARGET through \
         `TARGET_VARIABLE` and nothing else would find it"
    );
    assert!(
        resolved_by_delegation >= 1,
        "no variable was resolved by following a `Type::from_env` call; \
         `qip-api` reads QIP_STORAGE_TARGET only through \
         `StorageSettings::from_env` and nothing else would find it"
    );
}

#[test]
fn every_variable_a_binary_refuses_to_start_without_is_set_by_the_deployment_that_runs_it() {
    // The other direction, and the one whose symptom is a crash loop rather
    // than a silent no-op: the cell manifest once set QIP_CELL_ID,
    // QIP_CELL_REGION and the envelope key but not QIP_VENUES, which
    // `qip-edge-node` also requires, so every pod exited 78 and was restarted
    // for ever.
    let mut checked = 0usize;
    for (place, binary, set) in deployments() {
        for variable in variables_required_by(&binary) {
            assert!(
                set.iter()
                    .any(|name| name == &variable || name == &format!("{variable}_FILE")),
                "{binary} refuses to start without {variable} and {place} does \
                 not set it. The process exits with a configuration error and \
                 is restarted for ever, which reads as a crash loop rather than \
                 as a missing value. It sets {set:?}."
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 4,
        "only {checked} required variables were checked; the walk from a \
         deployment to the source of the binary it runs is finding nothing"
    );
}

#[test]
fn no_deployment_sets_a_variable_its_binary_refuses_to_start_with() {
    // Retiring a variable is a third way for the two sides to disagree, and it
    // is the one that looks most harmless in a diff: the cell manifest went on
    // setting QIP_MIRROR_PATH after `qip-edge-node` began refusing it, under a
    // comment saying the node now refuses it.
    let mut checked = 0usize;
    let mut binaries_with_a_retired_variable = 0usize;
    for (place, binary, set) in deployments() {
        let refused = variables_refused_by(&binary);
        if !refused.is_empty() {
            binaries_with_a_retired_variable += 1;
        }
        for variable in &refused {
            assert!(
                !set.contains(variable),
                "{place} sets {variable} and {binary} refuses to start when it \
                 is set, naming its replacement in the refusal."
            );
            checked += 1;
        }
    }
    assert!(
        binaries_with_a_retired_variable >= 1 && checked >= 1,
        "no retired variable was found in any deployed binary, so this test \
         proved nothing. `qip-edge-node` refuses QIP_MIRROR_PATH and the \
         refusal is matched by its message; if the message was reworded, this \
         walk needs rewording with it rather than deleting."
    );
}

#[test]
fn every_allowlisted_variable_is_still_set_and_still_unread() {
    // An allowlist that outlives its reason is worse than no allowlist: it
    // silently excuses whatever later takes the same name.
    let constants = variables_by_constant();
    let deployed = deployments();
    for (file, variable, reason) in SET_BUT_NOT_READ {
        assert!(
            reason.len() > 120,
            "the allowlist entry for {variable} in {file} does not argue its \
             case; an exception without a reason is an exception nobody can \
             review"
        );
        let mut found = false;
        for (place, binary, set) in &deployed {
            if !place.ends_with(file) {
                continue;
            }
            found = true;
            assert!(
                set.contains(*variable),
                "{file} no longer sets {variable}, so its allowlist entry \
                 excuses nothing and should be deleted"
            );
            let read = variables_read_by(binary, &constants).all();
            assert!(
                !satisfies(&read, variable),
                "{binary} now reads {variable}, so the allowlist entry in \
                 {file} is obsolete. Delete it: the general rule covers this \
                 variable now, and an obsolete exception is one that will \
                 excuse the next mistake with the same name."
            );
        }
        assert!(
            found,
            "the allowlist names {file}, which declares no deployment this \
             walk can see"
        );
    }
}

// ---------------------------------------------------------------------------
// The direction whose symptom is silence
// ---------------------------------------------------------------------------

/// A bound the platform keeps rather than a dial an environment turns.
const A_BOUND_NOT_A_DIAL: &str = "A bounded working set is a safety property, \
     not an environment preference: bounded retention and bounded buffers are \
     a standing decision in .claude/rules/10-product-direction.md. A dial on a \
     bound is raised on the night the bound fires, which is the night it is \
     working, and the result is an out-of-memory kill of the process holding \
     every cell's capital state rather than a refusal. Changing it should be a \
     code change carrying a test that says what the new bound survives.";

/// A path that needs a volume the workload does not have.
const NO_VOLUME_TO_ROOT_IT_IN: &str = "A storage root only means something on \
     a workload with somewhere to write. A Cloud Run instance has no volume \
     and keeps nothing across a restart, so a path here would be refused by \
     `StorageSettings::preflight` at boot — which is the correct refusal, and \
     a catalogue offering the value would be offering a way to stop the \
     service. Only the execution node sets it, because only the node has a \
     disk. The entry ends when a central workload gets a volume, which is a \
     decision of its own.";

/// A bounded run, which is the opposite of what a service wants.
const A_BOUNDED_RUN_IS_A_CRASH_LOOP: &str = "These stop the process on their \
     own — a bounded run is how the binary is exercised by hand and in CI. \
     Cloud Run restarts what exits, so setting either from the catalogue turns \
     a long-running node into a restart loop that reads as a failing image, \
     and `run_is_bounded` says so in the banner where nobody would look for \
     the cause. The value belongs on the command that runs a bounded \
     experiment, never in the configuration of a workload meant to stay up.";

/// A per-investigation input, not an environment standing value.
const PER_INVESTIGATION_NOT_PER_ENVIRONMENT: &str = "A replay reads a recorded \
     file, so this names a path that has to exist in the instance: with no \
     volume mounted there is nothing to point it at, and the value would be a \
     configured path that resolves to nothing. Replay is also an \
     investigation, done deliberately against one recording, rather than a \
     standing property of an environment — a catalogue default would leave \
     every restart replaying the same file for ever.";

/// A recorded tape, which is a demonstration's input and not a feed.
const A_DEMONSTRATION_FIXTURE_NO_DEPLOYMENT_SHIPS: &str = "Optional, and \
     unset selects the synthetic feed, which is what every environment runs. \
     The tape is a recorded fixture — `data/datasets/loop-demonstration-tape.json` \
     — played on its own clock so a demonstration of the loop is reproducible \
     from one committed file; no deployment ships it, no image carries it, and \
     a Cloud Run instance has no volume to mount it from, so a value here would \
     be a configured path that resolves to nothing and the feed refuses it at \
     start-up. It also contradicts a replay: each root refuses both set at once \
     by name. A tape belongs on the command that runs a demonstration, never in \
     the configuration of a workload meant to sense a market.";

/// The API's own source selection, which no deployment makes yet.
///
/// The API's default is not the synthetic feed the brains fall back to: it
/// is no source at all, said in the banner, because the API is the process an
/// operator reads and generated prices there would be indistinguishable from
/// a sensed market.
const THE_API_SENSES_NOTHING_UNTIL_A_SOURCE_IS_CHOSEN: &str = "Optional, and \
     unset means the API senses nothing and says so in its banner — not a \
     synthetic exchange, because this is the process an operator reads and \
     generated prices here would be indistinguishable from a sensed market. \
     The tape is the same committed demonstration fixture the brains replay, \
     played on its own clock; no deployment ships it, no image carries it, \
     and a Cloud Run instance has no volume to mount it from, so a value here \
     would be a configured path that resolves to nothing and the feed refuses \
     it at start-up. The connector pair is the real path ADR 0034 decides, and \
     it is admitted by the data finder's licensing catalogue before any socket \
     opens; the catalogue sets it for the fast brain from \
     `var.market_data_connector` and for nothing else, and extending that arm \
     to the API is a catalogue change with its own plan evidence — the API's \
     egress sidecar has never been applied, and the one FX source this build \
     carries answers a 301 from the host its manifest names, so a value today \
     would start a process whose feed refuses to open at boot. Each root \
     refuses a tape and a connector set at once by name.";

/// A number the tests and the probes are written against.
const THE_PACE_THE_PROBES_ASSUME: &str = "The cadence, the budget and the \
     tolerance are one set of numbers: /ready returns 503 once cycles breach \
     the fast-path ceiling for longer than the breach tolerance, so an \
     environment that moved one of them from the catalogue would move the \
     line that makes readiness mean anything, without touching the tests that \
     pin it. The default is the reviewed number and changing it is a change \
     to the code that argues for it.";

/// The direct-vendor live feed, which cannot be half-configured.
const CREDENTIAL_BEARING_AND_UNLICENSED: &str = "The direct-vendor feed takes \
     six variables, one of which is a credential, and a credential reaches a \
     workload as a mounted file through secret_mounts — never as an \
     environment value. A catalogue offering the other five would therefore \
     offer only half a configuration, which `live_feed` refuses by name rather \
     than falling back, so the revision would not start. Wiring it is a \
     secret mount, an egress listener for the vendor's host, and a licensing \
     decision recorded before the source is used — not a catalogue key. The \
     connector path is the one that is selectable, because it needs no \
     credential.";

/// Order entry, which is the one thing a deployment must not aim.
const NOT_AN_ORDER_ENTRY_DIAL: &str = "This aims order entry, and the \
     deployment deliberately gives no way to aim it. Absent, `QIP_VENUE_ADAPTER` \
     selects the in-process matching engine — the simulated venue every \
     deployment of this binary has ever run — and the acknowledgement and \
     idempotency variables are the witnesses the REST adapter demands before \
     it will send anything anywhere. A value for any of the three would put \
     the distinction between a simulator and a venue into a file an \
     environment edits, which is exactly the boundary \
     .claude/rules/01-security-and-safety.md keeps structural. Bringing a node \
     up against a real venue is the runbook's deliberate, per-node act, and \
     shadow mode's firewall is what refuses it until then.";

/// The requote policy, which a deployment leaves unset on purpose.
const REPRICING_IS_A_PER_NODE_DECISION_NOT_A_DEFAULT: &str = "Absent, a resting \
     child order stays where the cell sent it until its time to live, and the \
     node's production requirements say so. `QIP_REPRICE` is `<tick>:<ticks>:<bps>` \
     — the instrument's price increment and two staleness thresholds — and the \
     right values are a property of the venue and the instruments a particular \
     node quotes, not of an environment: a one-cent tick written into every \
     node's template would be wrong for every book that is not priced in cents, \
     and the policy refuses a zero tick at start-up rather than at the first \
     pass. Shadow mode sends nothing, so a requote there is a cancel and a \
     replacement of an order no venue holds. Setting it is the runbook's \
     per-node act when a node is brought up against a book whose tick is known.";

/// A seed, whose default is derived and whose override is for reproduction.
const A_SEED_IS_DERIVED_NOT_DEPLOYED: &str = "The seed is derived from the \
     node's own identity so that two cells do not retry in lockstep, and the \
     variable exists to override that when a specific sequence has to be \
     reproduced in an investigation. Pinning it in the deployment would give \
     every instance of every environment the same jitter and the same \
     simulated rejections — a synchronised retry storm and a reproducible \
     sequence nobody asked to reproduce.";

/// The mesh, which this runtime cannot publish.
const THE_MESH_HAS_NO_PORT_ON_CLOUD_RUN: &str = "The in-tree mesh binds one \
     listener per cell on its own port — the address is the cell's identity on \
     that transport — and a Cloud Run service exposes exactly one port. So the \
     API cannot bind QIP_MESH_CELLS anywhere a node could reach, and a node \
     given a QIP_MESH_PEER would publish every delta into a port nothing \
     binds. Unset on both ends, the API builds no mesh and /api/v1/mesh \
     answers `available: false`, and the node starts detached, which ADR 0008 \
     makes a legitimate state — and there is no node today to attach. The \
     blueprint's control fabric is Pub/Sub (§46.1); wiring the centre-to-node \
     path on this runtime is that work, recorded in ADR 0024, not a port that \
     cannot be published. The entry ends when the fabric exists.";

/// The OpenObserve drain (ADR 0028), which nothing has vendored or applied
/// yet.
const NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET: &str = "The drain cannot \
     reach the collector ADR 0030 created, and the reason is a property of the \
     transport rather than a missing value. `qip_transport::Url::parse` \
     refuses every scheme but plaintext `http` by name -- this platform ships \
     no TLS stack, deliberately (ADR 0001) -- and ADR 0030 put OpenObserve on \
     the public internet, which is https. So a URL naming it is refused at \
     start-up with EX_CONFIG, and a URL naming anything else names something \
     that does not exist: the egress proxy's allowlist \
     (`infrastructure/egress/envoy.yaml`, six hosts) does not carry the \
     service's own run.app address, and `qip-fastbrain` has no proxy at all \
     because `catalogue.tf` refuses it one at plan time (ADR 0008: nothing on \
     the hot path may consult a model, and port 9102 is that route). \
     Setting the variable on any of the three would therefore stop the \
     process or dial nothing, and the drain fails closed (principle 10) \
     rather than stopping, so a wrong target reads as quiet rather than as a \
     boot refusal -- which is exactly why it stays unset. What ends this \
     entry is a plaintext collector inside the VPC that the proxy's allowlist \
     names, or a TLS hop this process may use; both are infrastructure work \
     with their own evidence, not a value added here as a side effect.";

const READ_BUT_NOT_SET: &[(&str, &str, &str)] = &[
    (
        "qip-api",
        "QIP_MESH_CELLS",
        THE_MESH_HAS_NO_PORT_ON_CLOUD_RUN,
    ),
    (
        "qip-api",
        "QIP_ARBITRAGE_POLICY_PATH",
        THE_MESH_HAS_NO_PORT_ON_CLOUD_RUN,
    ),
    (
        "qip-api",
        "QIP_OPENOBSERVE_URL",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    (
        "qip-fastbrain",
        "QIP_OPENOBSERVE_URL",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    (
        "qip-fastbrain",
        "QIP_OPENOBSERVE_ORG",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    (
        "qip-fastbrain",
        "QIP_OPENOBSERVE_INTERVAL_SECS",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    (
        "qip-fastbrain",
        "QIP_OPENOBSERVE_AUTHORIZATION",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    (
        "qip-deepbrain",
        "QIP_OPENOBSERVE_URL",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    (
        "qip-deepbrain",
        "QIP_OPENOBSERVE_ORG",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    (
        "qip-deepbrain",
        "QIP_OPENOBSERVE_INTERVAL_SECS",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    (
        "qip-deepbrain",
        "QIP_OPENOBSERVE_AUTHORIZATION",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    (
        "qip-api",
        "QIP_OPENOBSERVE_ORG",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    (
        "qip-api",
        "QIP_OPENOBSERVE_INTERVAL_SECS",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    (
        "qip-api",
        "QIP_OPENOBSERVE_AUTHORIZATION",
        NO_COLLECTOR_IS_VENDORED_OR_APPLIED_YET,
    ),
    ("qip-api", "QIP_MESH_INBOX_CAPACITY", A_BOUND_NOT_A_DIAL),
    ("qip-api", "QIP_MESH_SPOOL_CAPACITY", A_BOUND_NOT_A_DIAL),
    ("qip-api", "QIP_STORAGE_ROOT", NO_VOLUME_TO_ROOT_IT_IN),
    ("qip-deepbrain", "QIP_STORAGE_ROOT", NO_VOLUME_TO_ROOT_IT_IN),
    ("qip-fastbrain", "QIP_STORAGE_ROOT", NO_VOLUME_TO_ROOT_IT_IN),
    (
        "qip-deepbrain",
        "QIP_DEEPBRAIN_MAX_CYCLES",
        A_BOUNDED_RUN_IS_A_CRASH_LOOP,
    ),
    (
        "qip-deepbrain",
        "QIP_DEEPBRAIN_MAX_RUNTIME_SECS",
        A_BOUNDED_RUN_IS_A_CRASH_LOOP,
    ),
    (
        "qip-fastbrain",
        "QIP_FASTBRAIN_MAX_CYCLES",
        A_BOUNDED_RUN_IS_A_CRASH_LOOP,
    ),
    (
        "qip-fastbrain",
        "QIP_FASTBRAIN_MAX_RUNTIME_SECS",
        A_BOUNDED_RUN_IS_A_CRASH_LOOP,
    ),
    (
        "qip-deepbrain",
        "QIP_DEEPBRAIN_REPLAY_PATH",
        PER_INVESTIGATION_NOT_PER_ENVIRONMENT,
    ),
    (
        "qip-fastbrain",
        "QIP_FASTBRAIN_REPLAY_PATH",
        PER_INVESTIGATION_NOT_PER_ENVIRONMENT,
    ),
    (
        "qip-fastbrain",
        "QIP_FASTBRAIN_TAPE_PATH",
        A_DEMONSTRATION_FIXTURE_NO_DEPLOYMENT_SHIPS,
    ),
    (
        "qip-api",
        "QIP_API_TAPE_PATH",
        THE_API_SENSES_NOTHING_UNTIL_A_SOURCE_IS_CHOSEN,
    ),
    (
        "qip-api",
        "QIP_CONNECTOR_SOURCE",
        THE_API_SENSES_NOTHING_UNTIL_A_SOURCE_IS_CHOSEN,
    ),
    (
        "qip-api",
        "QIP_CONNECTOR_BASE_URL",
        THE_API_SENSES_NOTHING_UNTIL_A_SOURCE_IS_CHOSEN,
    ),
    (
        "qip-deepbrain",
        "QIP_DEEPBRAIN_TAPE_PATH",
        A_DEMONSTRATION_FIXTURE_NO_DEPLOYMENT_SHIPS,
    ),
    (
        "qip-deepbrain",
        "QIP_DEEPBRAIN_ARCHIVE_EVERY",
        THE_PACE_THE_PROBES_ASSUME,
    ),
    (
        "qip-deepbrain",
        "QIP_DEEPBRAIN_EVOLUTION_CANDIDATES",
        THE_PACE_THE_PROBES_ASSUME,
    ),
    (
        "qip-deepbrain",
        "QIP_DEEPBRAIN_EVOLUTION_EVERY",
        THE_PACE_THE_PROBES_ASSUME,
    ),
    (
        "qip-deepbrain",
        "QIP_DEEPBRAIN_FAILURE_TOLERANCE",
        THE_PACE_THE_PROBES_ASSUME,
    ),
    (
        "qip-deepbrain",
        "QIP_DEEPBRAIN_SHUTDOWN_BUDGET_SECS",
        THE_PACE_THE_PROBES_ASSUME,
    ),
    (
        "qip-fastbrain",
        "QIP_FASTBRAIN_ARCHIVE_EVERY",
        THE_PACE_THE_PROBES_ASSUME,
    ),
    (
        "qip-fastbrain",
        "QIP_FASTBRAIN_BREACH_TOLERANCE",
        THE_PACE_THE_PROBES_ASSUME,
    ),
    (
        "qip-fastbrain",
        "QIP_FASTBRAIN_CYCLE_BUDGET_MS",
        THE_PACE_THE_PROBES_ASSUME,
    ),
    (
        "qip-fastbrain",
        "QIP_FASTBRAIN_CYCLE_INTERVAL_MS",
        THE_PACE_THE_PROBES_ASSUME,
    ),
    (
        "qip-fastbrain",
        "QIP_FASTBRAIN_SHUTDOWN_BUDGET_MS",
        THE_PACE_THE_PROBES_ASSUME,
    ),
    (
        "qip-fastbrain",
        "QIP_FASTBRAIN_SEED",
        A_SEED_IS_DERIVED_NOT_DEPLOYED,
    ),
    (
        "qip-edge-node",
        "QIP_GATEWAY_SEED",
        A_SEED_IS_DERIVED_NOT_DEPLOYED,
    ),
    (
        "qip-edge-node",
        "QIP_MESH_SEED",
        A_SEED_IS_DERIVED_NOT_DEPLOYED,
    ),
    (
        "qip-edge-node",
        "QIP_MESH_PEER",
        THE_MESH_HAS_NO_PORT_ON_CLOUD_RUN,
    ),
    (
        "qip-edge-node",
        "QIP_VENUE_ADAPTER",
        NOT_AN_ORDER_ENTRY_DIAL,
    ),
    (
        "qip-edge-node",
        "QIP_VENUE_IDEMPOTENCY",
        NOT_AN_ORDER_ENTRY_DIAL,
    ),
    (
        "qip-edge-node",
        "QIP_VENUE_ORDER_ENTRY_ACKNOWLEDGED",
        NOT_AN_ORDER_ENTRY_DIAL,
    ),
    (
        "qip-edge-node",
        "QIP_REPRICE",
        REPRICING_IS_A_PER_NODE_DECISION_NOT_A_DEFAULT,
    ),
    (
        "qip-fastbrain",
        "QIP_MARKET_DATA_BASE_URL",
        CREDENTIAL_BEARING_AND_UNLICENSED,
    ),
    (
        "qip-fastbrain",
        "QIP_MARKET_DATA_KEY",
        CREDENTIAL_BEARING_AND_UNLICENSED,
    ),
    (
        "qip-fastbrain",
        "QIP_MARKET_DATA_KEY_HEADER",
        CREDENTIAL_BEARING_AND_UNLICENSED,
    ),
    (
        "qip-fastbrain",
        "QIP_MARKET_DATA_PATH",
        CREDENTIAL_BEARING_AND_UNLICENSED,
    ),
    (
        "qip-fastbrain",
        "QIP_MARKET_DATA_SYMBOLS",
        CREDENTIAL_BEARING_AND_UNLICENSED,
    ),
    (
        "qip-fastbrain",
        "QIP_MARKET_DATA_VENUE",
        CREDENTIAL_BEARING_AND_UNLICENSED,
    ),
    (
        "qip-edge-node",
        "QIP_MARKET_DATA_VENUE",
        CREDENTIAL_BEARING_AND_UNLICENSED,
    ),
    (
        "qip-deepbrain",
        "QIP_DEEPBRAIN_CYCLE_INTERVAL_SECS",
        "The Deep Brain's own override of an interval the catalogue already \
         sets from the root's `cycle_interval_seconds`, which this binary \
         reads as QIP_CYCLE_INTERVAL_SECONDS. Setting both would be two claims \
         about one fact in one process, and the louder one would win silently \
         — the per-binary override exists for a bounded experiment that wants \
         a different pace without editing the value every workload shares.",
    ),
    (
        "qip-deepbrain",
        "QIP_DEEPBRAIN_EVENT_LOG",
        "Absent, `event_log_destination` derives the log's place from the \
         storage the instance actually has: beside a durable store when there \
         is one, in memory when there is not, and the banner says which. A \
         path set here would be used whatever the storage target is, so on \
         this volumeless instance it would name a file on a read-only \
         filesystem and be a configured destination for the hash-chained \
         record that does not exist. It becomes settable the day the workload \
         has a volume, which is the same day QIP_STORAGE_ROOT does.",
    ),
];

#[test]
fn every_variable_a_deployed_binary_reads_is_set_by_the_deployment_or_argued_to_be_unset() {
    // The direction nothing checked, and the one whose symptom is silence.
    //
    // A variable a binary refuses to start without is caught by the check
    // above it. An *optional* variable no deployment sets produces a healthy
    // process running its fallback for ever — a Fast Brain producing
    // investment decisions from generated prices in every environment, with
    // the connector it was told to use reachable from nowhere.
    let constants = variables_by_constant();
    let deployed = deployments();

    let conditional = catalogue_conditional_variables();
    let mut set_by_the_deployment = 0usize;
    let mut argued_unset = 0usize;
    let mut left_to_the_tfvars = 0usize;
    let mut unexplained: BTreeSet<String> = BTreeSet::new();
    for (place, binary, set) in &deployed {
        // A RunService is the catalogue entry rendered for one environment;
        // the variables the entry sets only when a root variable is non-null
        // are absent where that environment's tfvars leave it null.
        let rendered_from = place
            .strip_prefix("envs/")
            .and_then(|rest| rest.split_once(':'))
            .map(|(_, workload)| workload.to_string());
        // The narrow read set: literals in the binary's own crate and the
        // literals of a type it constructs with `::from_env`. The constant
        // rule is deliberately excluded here — it is biased towards
        // over-reading, which is safe in the other direction and unsafe in
        // this one, where it would demand that a deployment set a variable
        // the binary never reads.
        let found = variables_read_by(binary, &constants);
        let read: BTreeSet<String> = found.literal.union(&found.by_delegation).cloned().collect();
        assert!(
            read.len() >= 3,
            "{binary} appears to read only {read:?}; the source walk has \
             stopped finding variables and this check would demand nothing"
        );
        let refused = variables_refused_by(binary);

        let mut unaccounted: BTreeSet<String> = BTreeSet::new();
        for variable in &read {
            if refused.contains(variable) {
                continue;
            }
            if set.contains(variable) || set.contains(&format!("{variable}_FILE")) {
                set_by_the_deployment += 1;
                continue;
            }
            if READ_BUT_NOT_SET
                .iter()
                .any(|(crate_name, name, _)| crate_name == binary && name == variable)
            {
                argued_unset += 1;
                continue;
            }
            if rendered_from
                .as_ref()
                .and_then(|workload| conditional.get(workload))
                .is_some_and(|names| names.contains(variable))
            {
                left_to_the_tfvars += 1;
                continue;
            }
            unaccounted.insert(variable.clone());
        }
        if !unaccounted.is_empty() {
            unexplained.insert(format!(
                "{place} runs {binary}, which reads {unaccounted:?}"
            ));
        }
    }

    assert!(
        unexplained.is_empty(),
        "{unexplained:?}. Each of those is a variable the binary reads, the \
         deployment that runs it does not set, and no entry in READ_BUT_NOT_SET \
         argues should be unset. An optional variable no deployment sets is a \
         capability no environment can select: the binary takes its fallback, \
         the revision is healthy, and nothing anywhere says the feature is off. \
         Either the catalogue should set it — from a root variable, so an \
         absent value still means the fallback — or say in the allowlist why \
         an operator is better off unable to."
    );
    assert!(
        set_by_the_deployment >= 10,
        "only {set_by_the_deployment} variable(s) matched a value a deployment \
         sets; the correspondence is no longer being measured"
    );
    assert!(
        argued_unset >= 1,
        "no variable was excused by READ_BUT_NOT_SET, so the allowlist arm is \
         never exercised and could have stopped matching"
    );
    // The connector pair is conditional in the catalogue and no environment
    // sets `market_data_connector` today, so every fast-brain RunService
    // omits it and the arm has to have fired; if every environment comes to
    // set the connector, this premise is the line to revisit.
    assert!(
        left_to_the_tfvars >= 1,
        "no variable was left to the tfvars' decision through a conditional \
         catalogue arm, so that rule is never exercised and could have \
         stopped matching"
    );
}

#[test]
fn every_read_but_not_set_entry_is_still_read_and_still_unset() {
    let constants = variables_by_constant();
    let deployed = deployments();
    for (binary, variable, reason) in READ_BUT_NOT_SET {
        assert!(
            reason.len() > 120,
            "the allowlist entry for {variable} in {binary} does not argue its \
             case; an exception without a reason is one nobody can review"
        );
        let mut found = false;
        for (place, deployed_binary, set) in &deployed {
            if deployed_binary != binary {
                continue;
            }
            found = true;
            let read = variables_read_by(deployed_binary, &constants).all();
            assert!(
                read.contains(*variable),
                "{binary} no longer reads {variable}, so its allowlist entry \
                 excuses nothing and should be deleted"
            );
            assert!(
                !set.contains(*variable) && !set.contains(&format!("{variable}_FILE")),
                "{place} now sets {variable}, so the entry for it is obsolete. \
                 Delete it: the general rule covers this variable now, and an \
                 obsolete exception will excuse the next mistake with the same \
                 name."
            );
        }
        assert!(
            found,
            "the allowlist names {binary}, which no deployment this walk can \
             see runs"
        );
    }
}

#[test]
fn the_catalogue_lets_an_operator_select_the_live_market_connector_without_editing_it() {
    // The specific gap, asserted by name as well as by the general rule above.
    // The general check would catch a deletion of the merge; this one says
    // what the merge is for, and asserts the properties the general check has
    // no opinion about: the values come from one root variable rather than a
    // literal, both or neither, and the address is refused unless it is the
    // proxy on loopback.
    let workloads = catalogue_workloads();
    let (_, fastbrain) = workloads
        .iter()
        .find(|(name, _)| name == "fastbrain")
        .expect("the catalogue deploys the fast brain");
    for (variable, field) in [
        ("QIP_CONNECTOR_SOURCE", "source"),
        ("QIP_CONNECTOR_BASE_URL", "base_url"),
    ] {
        let sets_from_root = fastbrain.lines().any(|line| {
            let Some((key, value)) = line.split_once('=') else {
                return false;
            };
            key.trim() == variable && value.trim() == format!("var.market_data_connector.{field}")
        });
        assert!(
            sets_from_root,
            "the fast brain's entry does not set {variable} from the root's \
             market_data_connector.{field}, so no environment can select a \
             live market source"
        );
    }
    assert!(
        fastbrain.contains("var.market_data_connector == null ? {} : {"),
        "the connector pair is not gated on the root variable being set, so \
         the synthetic default is no longer deployable"
    );

    let variables = read(ROOT_VARIABLES);
    let block = variables
        .split("variable \"market_data_connector\" {")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("the root declares market_data_connector");
    assert!(
        block.contains("default = null"),
        "market_data_connector has a default, so a deployment starts fetching \
         a vendor because the catalogue was applied"
    );
    assert!(
        block.contains("source   = string") && block.contains("base_url = string"),
        "market_data_connector no longer requires both keys, so half a \
         configuration can reach a service that refuses it at start-up"
    );
    assert!(
        block.contains("startswith(var.market_data_connector.base_url, \"http://127.0.0.1:\")"),
        "the connector's base URL may point somewhere other than the egress \
         proxy on loopback; `qip_transport::http` refuses https by name and an \
         address off the instance is a route that does not exist"
    );
}

#[test]
fn the_centre_declares_no_mesh_listener_because_cloud_run_publishes_one_port() {
    // The mesh was the general property's first violation: complete, tested,
    // and configured by nothing. On this runtime it is unconfigured for a
    // reason rather than by omission, and the reason has to stay written
    // where the variable would be set — a reader who finds QIP_MESH_CELLS
    // absent from the catalogue and no explanation will add it, and a
    // listener nothing can reach is the failure this suite was born from.
    for (name, body) in catalogue_workloads() {
        let set = variables_an_entry_sets(&body);
        assert!(
            !set.contains("QIP_MESH_CELLS"),
            "{name} sets QIP_MESH_CELLS. The API binds one listener per cell \
             on its own port and a Cloud Run service publishes exactly one, so \
             this is a listener no node can reach that reads as a mesh."
        );
    }
    let node = variables_the_node_sets();
    assert!(
        !node.contains("QIP_MESH_PEER"),
        "node.env sets QIP_MESH_PEER, so a node would publish every delta into \
         a port nothing binds"
    );
    let catalogue = read(CATALOGUE);
    assert!(
        catalogue.contains("`QIP_MESH_CELLS` on the API") && catalogue.contains("exactly one port"),
        "catalogue.tf no longer says why the mesh is not wired, so the next \
         reader will wire it to a port nothing can reach"
    );
    assert!(
        READ_BUT_NOT_SET
            .iter()
            .any(|(binary, variable, _)| *binary == "qip-api" && *variable == "QIP_MESH_CELLS"),
        "the mesh's absence is no longer argued in the allowlist"
    );
}

#[test]
fn the_node_takes_its_values_from_a_file_and_its_secrets_from_files_never_from_the_environment() {
    // The node is the one deployment whose configuration a person could read
    // off the machine. Its values live in node.env; its credentials are
    // fetched to a tmpfs and named by path, exactly the `_FILE` indirection
    // `qip_core::secret` reads — never written into the environment, where a
    // key is a key in /proc/<pid>/environ and in every crash dump.
    let startup = read(NODE_STARTUP);
    let set = variables_the_node_sets();
    assert!(
        set.contains("QIP_CAPITAL_ENVELOPE_KEY_FILE"),
        "node.env no longer names the envelope key by path"
    );
    assert!(
        !set.contains("QIP_CAPITAL_ENVELOPE_KEY") && !set.contains("QIP_VENUE_CREDENTIAL"),
        "node.env carries a credential as a value"
    );
    assert!(
        startup.contains("/usr/local/bin/qip-fetch-secret \"${project_id}\" \"${capital_envelope_secret_id}\" \"$RUN_DIR/capital-envelope-key\""),
        "the startup script no longer fetches the envelope key to the secrets tmpfs"
    );
    assert!(
        startup.contains("mount -t tmpfs -o size=8m,mode=0700,noswap tmpfs \"$RUN_DIR\"")
            && startup.contains("chmod 0400 \"$RUN_DIR/capital-envelope-key\""),
        "the node's secrets are not on a tmpfs at mode 0400"
    );
    // And the ceiling is not set on the node: a cell's ceiling is structural,
    // and a fourth apparent decision point would weaken the three.
    assert!(
        !set.contains("QIP_AUTONOMY_CEILING"),
        "node.env sets QIP_AUTONOMY_CEILING, a fourth apparent decision point \
         on a boundary that is stronger with three"
    );
}

// ---------------------------------------------------------------------------
// The alerting layer, which had no name in common with the platform
// ---------------------------------------------------------------------------

#[test]
fn every_metric_an_alert_policy_queries_is_one_the_platform_emits() {
    // The alerting layer and the platform had no name in common.
    //
    // Before the kernel emitted anything, the four policies' metric names
    // appeared in **zero** Rust files. So the layer was not merely gated off
    // by `workload_metrics_exist = false` — it was unreachable by
    // construction. Two independent artefacts naming the same fact will
    // disagree, and the louder one will be wrong. This is the test that makes
    // them agree, in both directions.
    let policies = read("infrastructure/terraform/modules/observability/main.tf");

    let queried: BTreeSet<String> = policies
        .match_indices("qip_")
        .map(|(index, _)| {
            policies[index..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .collect();
    assert!(
        queried.len() >= 4,
        "only {} metric name(s) were read out of the alert policies; the scan \
         is not reaching them",
        queried.len()
    );

    let emitted = read("backend/crates/libs/qip-observability/src/metrics.rs");
    for metric in &queried {
        assert!(
            emitted.contains(&format!("\"{metric}\"")),
            "an alert policy queries {metric} and no metric declares that \
             name. Cloud Monitoring refuses a policy naming a descriptor it \
             has never ingested, so this policy cannot be created — and if it \
             could, it would watch a series nothing produces."
        );
    }

    // The other half. A policy naming a real series is necessary; a real
    // series that nothing pages on is the failure the observability rule
    // records as still open — a reconciliation break charted and paging no
    // one. These are the series a person must be woken for, on both planes,
    // and each must be both registered by the code and named by a policy.
    for (series, why) in SERIES_THAT_MUST_PAGE {
        assert!(
            emitted.contains(&format!("\"{series}\"")),
            "{series} is on the list of series that must page and nothing registers it: {why}"
        );
        assert!(
            queried.contains(series),
            "no alert policy names {series}: {why}. It is charted and pages no one, which \
             reads in the console as a project being watched."
        );
    }
}

/// The series a person must be woken for, and why.
///
/// A second copy of the policy list on purpose: a test that read the list
/// out of the policies it checks would agree with every deletion. Adding a
/// series that pages is therefore two edits and a reviewer who sees both.
const SERIES_THAT_MUST_PAGE: [(&str, &str); 7] = [
    (
        "qip_kill_switch_tripped",
        "the platform has halted and no order will be sent until an operator clears it",
    ),
    (
        "qip_live_fills_total",
        "an order reached a real venue in an environment that should only ever trade on paper",
    ),
    (
        "qip_limit_breaches",
        "a risk limit has been breached and the book is not coming back inside on its own",
    ),
    (
        "qip_permission_denials_total",
        "an agent reached for a capability its manifest does not grant",
    ),
    (
        "qip_edge_halted",
        "an execution node is refusing to trade, by kill switch or by policy",
    ),
    (
        "qip_edge_reconciliation_breaks_total",
        "a node's book and its venue's record disagree",
    ),
    (
        "qip_central_reconciliation_breaks_total",
        "the central plane acted on a report whose exposure disagrees with the envelope it granted",
    ),
];

#[test]
fn something_collects_the_metrics_the_alert_policies_depend_on_or_says_that_nothing_does_yet() {
    // Emitting is not ingesting. The kernel records, the health servers serve
    // a valid exposition, and the policies name the right series — and none
    // of that reaches Cloud Monitoring unless something scrapes the process.
    // On this runtime there are two halves, and only one has a collector.
    //
    // The node: the Ops Agent's Prometheus receiver, written by the startup
    // script, scraping the health port on loopback on the path the binary
    // serves.
    let startup = read(NODE_STARTUP);
    let receiver = startup
        .split("cat >\"$OPS_AGENT_CONF\" <<OPS_AGENT_EOF")
        .nth(1)
        .and_then(|rest| rest.split("\nOPS_AGENT_EOF\n").next())
        .expect("the startup script writes an Ops Agent configuration");
    assert!(
        receiver.contains("type: prometheus"),
        "the node's Ops Agent configuration declares no Prometheus receiver"
    );
    assert!(
        receiver.contains("metrics_path: /metrics"),
        "the receiver scrapes a path the health server does not serve"
    );
    assert!(
        receiver.contains("targets: [\"127.0.0.1:${health_port}\"]"),
        "the receiver scrapes something other than the node's own health port on loopback"
    );
    assert!(
        receiver.contains("receivers: [qip_edge]"),
        "the receiver is declared and wired into no pipeline, so it scrapes and ships nothing"
    );
    assert!(
        startup.contains("systemctl cat google-cloud-ops-agent.service >/dev/null 2>&1 || fail"),
        "a node without the Ops Agent starts anyway, and reaches nobody"
    );

    // The Cloud Run services: a collector is declared in modules/cloudrun
    // and runs only under a digest, and no environment names one. The gap
    // is written down rather than left to be discovered — with the reason,
    // which is the same rule the proxy had to satisfy first — and the note
    // must say both halves: what is declared and that no digest is pinned,
    // because a reader who finds the sidecar in the module and no caveat
    // will flip the gate.
    let gap = read("infrastructure/terraform/modules/observability/NOT-SCRAPED.md");
    assert!(
        gap.contains("cloud-run-gmp-sidecar")
            && gap.contains("collector_image_digest")
            && gap.contains("No digest is pinned"),
        "NOT-SCRAPED.md no longer records what the Cloud Run collector is, where it is declared, or that no digest names it"
    );
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        ));
        let pinned = tfvars.lines().any(|line| {
            line.trim_start()
                .starts_with("metrics_collector_image_digest")
        });
        assert!(
            !pinned,
            "{environment} pins a metrics collector digest that no reviewed line in \
             infrastructure/egress/vendored-images.txt has been mirrored and attested from; \
             a revision carrying it would be refused at admission"
        );
    }

    // And the gate stays closed until a scrape is a fact. No environment
    // flips it.
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        ));
        let live = tfvars.lines().any(|line| {
            line.trim_start().starts_with("workload_metrics_exist") && line.contains("true")
        });
        assert!(
            !live,
            "{environment} sets workload_metrics_exist = true and nothing has been observed scraping a pod or a node"
        );
    }
}
