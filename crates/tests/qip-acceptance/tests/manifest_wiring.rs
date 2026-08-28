//! Does the deployment configure the binary it runs?
//!
//! Every check here is about one seam: a manifest sets environment variables,
//! a binary reads environment variables, and nothing in either file mentions
//! the other. The two drift silently in both directions, and both directions
//! have already happened in this repository:
//!
//! * **A variable nothing reads.** `fastbrain.yaml`, `deepbrain.yaml` and
//!   `edge-cell.yaml` each set `QIP_AUTONOMY_CEILING` from the `qip-config`
//!   ConfigMap while none of those three binaries read it. An operator
//!   lowering the ceiling in the ConfigMap changed nothing at all — and the
//!   manifest, the config map and the audit log all recorded a change. That is
//!   worse than an absent control, because an absent control is visibly
//!   absent. `deepbrain.yaml` set `QIP_API_ADDRESS` two lines below a comment
//!   saying that binary does not read it.
//! * **A variable nothing sets.** `QIP_MESH_CELLS` and `QIP_MESH_PEER` — the
//!   two halves of ADR 0011's in-tree mesh — appeared in no manifest at all,
//!   so the backbone was dead in every deployment while being complete and
//!   tested in process. Cells published state deltas at a peer where nothing
//!   listened; no capital grant, recall or reconciliation halt could travel.
//!
//! Neither is visible in a diff of either file alone, and neither produces an
//! error anywhere. That is what makes this a test rather than a review note.
//!
//! # How the two sides are derived
//!
//! Both from the source, never from a list here. A list would be a third copy
//! of the same fact and would drift from both of the copies it was written to
//! reconcile.
//!
//! The manifest side is the `- name: QIP_…` lines inside a workload's
//! containers. The binary side is every `QIP_…` a binary could reach:
//! quote-delimited literals in its own crate, the `const NAME: &str = "QIP_…"`
//! declarations it references by identifier, and — because a composition root
//! may delegate the whole read — the literals in the module defining any
//! `Type::from_env` it calls.
//!
//! That last pair of rules is what makes the walk honest rather than
//! convenient. `qip-api` never writes `"QIP_STORAGE_TARGET"`; it calls
//! `StorageSettings::from_env()`. `qip-fastbrain` never writes it either; it
//! passes `TARGET_VARIABLE`. A test that only matched literals would call both
//! of those manifests wrong and be edited until it stopped complaining, which
//! is how a check becomes an allowlist.
//!
//! The bias is deliberate and worth naming: resolving a constant or a
//! `from_env` module can only *add* names to the read set, so the failure mode
//! of getting a rule slightly too broad is a variable this test tolerates,
//! never a manifest it wrongly rejects. Each rule therefore asserts it
//! contributed something, so a rule that silently stops resolving anything
//! fails here instead of quietly widening the test.

// The workspace denies `panic_in_result_fn` because an assertion that aborts a
// `Result`-returning function is a bug in production code. These tests return
// `()` and assert; the lint does not apply, and neither does the reason for it.

use qip_acceptance::{files_with_extension, read, repository_root};
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// Allowlist
// ---------------------------------------------------------------------------

/// Variables a manifest sets that the binary does not read, with the reason.
///
/// One entry, and it is not a convenience. Every addition here is a control
/// that does nothing, so an entry has to argue that the workload is better off
/// with a variable it ignores than without it — and
/// `every_allowlisted_variable_is_still_set_and_still_unread` deletes the
/// entry's licence the moment either half stops being true.
const SET_BUT_NOT_READ: &[(&str, &str, &str)] = &[(
    "edge-cell.yaml",
    "QIP_AUTONOMY_CEILING",
    "A cell's ceiling is structural rather than configured: `qip_edge::Cell` \
     has no constructor taking a ceiling other than paper trading, which is \
     the third and strongest of the three layers in \
     .claude/rules/01-security-and-safety.md. No value of this variable could \
     raise it, and `qip-edge-node` accordingly never reads it — the \
     composition-root layer that refuses a live value at start-up is \
     implemented in qip-api, qip-fastbrain and qip-deepbrain only. It stays \
     set because `nothing_added_here_raises_the_autonomy_ceiling_anywhere` in \
     the infrastructure suite asserts that every workload takes the ceiling \
     from the one named resource, and a cell that opted out of that assertion \
     would be the single workload whose ceiling an operator could not read off \
     the deployment. The entry ends when qip-edge-node reads the value and \
     refuses a live one like its three siblings, which is the honest close and \
     is work in crates/apps/, not here.",
)];

// ---------------------------------------------------------------------------
// The manifests
// ---------------------------------------------------------------------------

/// A manifest with its comments removed.
///
/// Load-bearing rather than tidiness: these manifests argue with the reader at
/// length, and several of the comments name the exact variables being checked
/// — `deepbrain.yaml` explains why it does *not* set `QIP_API_ADDRESS`. A
/// check that cannot tell a mention from a setting makes it impossible to
/// document why a variable is absent, which is the documentation most worth
/// having.
fn without_comments(content: &str) -> String {
    content
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every YAML document in every manifest, paired with the file it came from.
fn manifest_documents() -> Vec<(String, String)> {
    let mut documents = Vec::new();
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let name = path
            .file_name()
            .expect("a manifest has a file name")
            .to_string_lossy()
            .to_string();
        let content = std::fs::read_to_string(&path).expect("a manifest is readable");
        for document in content.split("\n---\n") {
            documents.push((name.clone(), document.to_string()));
        }
    }
    assert!(
        documents.len() > 10,
        "only {} documents were found; the manifest walk is not reaching them",
        documents.len()
    );
    documents
}

/// Documents whose **own** kind is `kind`.
///
/// Anchored at column zero rather than trimmed, because `kind:` appears nested
/// inside several Kubernetes objects — a `HorizontalPodAutoscaler` names
/// `kind: Deployment` inside its `scaleTargetRef` — and a trimmed match reads
/// those as the document's own kind, then panics on an autoscaler having no
/// container.
fn documents_of_kind(kind: &str) -> Vec<(String, String)> {
    manifest_documents()
        .into_iter()
        .filter(|(_, document)| document.lines().any(|line| line == format!("kind: {kind}")))
        .collect()
}

/// Every workload document, of either controller kind.
///
/// A `StatefulSet` is a `Deployment` that keeps its volumes, and the question
/// here — does this container's configuration match the binary in its image —
/// has nothing to do with which controller manages it. Keying on
/// `kind: Deployment` alone is how the edge cell would stop being checked.
fn workload_documents() -> Vec<(String, String)> {
    ["Deployment", "StatefulSet"]
        .iter()
        .flat_map(|kind| documents_of_kind(kind))
        .collect()
}

/// The container blocks inside a workload document.
///
/// Split on the fixed indentation these manifests use. Brittle on purpose: a
/// reindented manifest makes this return nothing, and every caller asserts it
/// found containers, so the failure is loud rather than a check that quietly
/// stops checking.
fn containers(document: &str) -> Vec<String> {
    let Some(start) = document.find("\n      containers:\n") else {
        return Vec::new();
    };
    let body = &document[start..];
    let body = body.split("\n      volumes:").next().unwrap_or(body);
    body.split("\n        - name: ")
        .skip(1)
        .map(str::to_string)
        .collect()
}

/// The binary a container runs, taken from its image reference.
///
/// The image is the honest link between a manifest and a binary: a container
/// name is a label somebody chose, and an image is what will execute.
fn container_binary(container: &str) -> String {
    let image = container
        .lines()
        .find_map(|line| line.trim().strip_prefix("image:"))
        .unwrap_or_else(|| panic!("a container declares an image:\n{container}"))
        .trim();
    let without_tag = image.split(':').next().unwrap_or(image);
    without_tag
        .rsplit('/')
        .next()
        .unwrap_or(without_tag)
        .to_string()
}

/// The `QIP_` variables a container sets.
///
/// Exact names, collected into a set, because every comparison downstream is
/// membership rather than substring. `contains("QIP_MESH_PEER")` is true of a
/// manifest setting `QIP_MESH_PEER_TIMEOUT` and of a comment mentioning
/// either, and this file exists to catch a variable being one character
/// different from the one that is read.
fn variables_a_container_sets(container: &str) -> BTreeSet<String> {
    without_comments(container)
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- name: "))
        .map(|name| name.trim().trim_matches('"').to_string())
        .filter(|name| name.starts_with("QIP_"))
        .collect()
}

// ---------------------------------------------------------------------------
// The binaries
// ---------------------------------------------------------------------------

/// Whether `text` mentions `word` as a whole identifier.
///
/// `SEED_VARIABLE` is declared twice in `qip-edge-node` alone — the gateway's
/// and the mesh's — and a plain `contains` would also match it inside a longer
/// identifier somebody adds later. The delimiters are what make this a match
/// on a name rather than on a spelling.
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
/// name — `"QIP_VENUE_FEED_ENDPOINT and its multicast group or session
/// credential"` — and a rule that took everything after `"QIP_` would read
/// that sentence as a variable the binary reads. Requiring the whole literal
/// to be a variable name keeps a prose mention from being counted as a read,
/// which is the same distinction `without_comments` draws on the manifest
/// side.
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
///
/// Workspace-wide because the declaration and the use are routinely in
/// different crates: `TARGET_VARIABLE` and `ROOT_VARIABLE` live in
/// `qip-storage` and are read by three composition roots. Restricted to `src`
/// so a constant declared in someone's test fixture cannot teach this walk a
/// variable the shipped binary does not read.
fn variables_by_constant() -> BTreeMap<String, BTreeSet<String>> {
    let mut declarations: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in files_with_extension("crates", "rs") {
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
///
/// A composition root may delegate an entire read — `qip-api` names neither
/// storage variable and calls `StorageSettings::from_env()` — and a walk that
/// did not follow the call would report both of the API's storage variables as
/// set by a manifest and read by nobody.
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
///
/// The three rules are additive and are reported separately so that each can
/// assert it is still resolving something. A rule that silently stops matching
/// does not make this test fail — it makes it pass while checking less, which
/// is the failure this whole file exists to catch one directory over.
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
    let sources = sources_under(&format!("crates/apps/{binary}/src"));
    assert!(
        !sources.is_empty(),
        "no source was found for {binary}; the walk from a container's image \
         to the crate that builds it is broken"
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

    // Follow each delegation to the module that defines it, and take both the
    // literals there and the constants that module names. `StorageSettings`
    // reads through `TARGET_VARIABLE`, so literals alone would not reach it
    // even one hop away.
    //
    // A delegate defined in the binary's own crate wins, and that is not an
    // optimisation. `MeshSettings` is the name of two different types —
    // `qip-api`'s centre-side settings and `qip-edge-node`'s cell-side ones —
    // and a workspace-wide search gave the API the cell's `QIP_MESH_PEER` and
    // `QIP_MESH_SEED`. Nothing failed, which is the point: an over-wide read
    // set is a test that would sit quietly through `api.yaml` setting a
    // variable only the cell reads.
    let workspace: Vec<(String, String)> = files_with_extension("crates", "rs")
        .iter()
        .filter(|path| path.to_string_lossy().contains("/src/"))
        .map(|path| {
            (
                path.to_string_lossy().to_string(),
                std::fs::read_to_string(path).expect("a source file is readable"),
            )
        })
        .collect();
    let home = format!("crates/apps/{binary}/src/");
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
///
/// `required("QIP_X", &mut missing)` and its credential variant are the two
/// calls that put a name on the list the process exits on, so matching them is
/// matching the actual refusal rather than a second statement of it.
fn variables_required_by(binary: &str) -> BTreeSet<String> {
    sources_under(&format!("crates/apps/{binary}/src"))
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

/// The variables a binary refuses to *start with*.
///
/// A retired variable is not the same as an unread one. `qip-edge-node` refuses
/// a non-empty `QIP_MIRROR_PATH` and exits 78 rather than ignoring it, because
/// a cell deployed with the old variable would write its journal nowhere while
/// its configuration still claimed a path. Read out of the refusal's own
/// message, so retiring the next variable needs no edit here.
fn variables_refused_by(binary: &str) -> BTreeSet<String> {
    let sources = sources_under(&format!("crates/apps/{binary}/src"));
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

/// Whether a manifest setting `set` satisfies a binary that reads `read`.
///
/// `qip_core::secret` accepts a `_FILE` variant of any credential variable, and
/// it is the one the manifests use for everything the Secret Manager CSI driver
/// projects: a signing key in the environment is a key in `/proc/<pid>/environ`,
/// in every child process and in every crash dump. So `QIP_TOKEN_OPERATOR_FILE`
/// is `QIP_TOKEN_OPERATOR` being read from a file, not an unrelated variable.
fn satisfies(read: &BTreeSet<String>, set: &str) -> bool {
    read.contains(set)
        || set
            .strip_suffix("_FILE")
            .is_some_and(|base| read.contains(base))
}

/// Every workload paired with the binary it runs and that binary's crate.
///
/// Skips a container whose image names no crate under `crates/apps`, which is
/// how a sidecar somebody adds later fails to be checked rather than fails the
/// suite for a reason that is not about it.
fn deployed_containers() -> Vec<(String, String, String)> {
    let mut found = Vec::new();
    for (manifest, document) in workload_documents() {
        let blocks = containers(&document);
        assert!(
            !blocks.is_empty() || !document.contains("image:"),
            "{manifest} has a workload with no container the split could find; \
             the manifest's indentation has changed and this check has stopped \
             checking"
        );
        for container in blocks {
            let binary = container_binary(&container);
            if !repository_root()
                .join(format!("crates/apps/{binary}/src"))
                .exists()
            {
                continue;
            }
            found.push((manifest.clone(), binary, container));
        }
    }
    assert!(
        found.len() >= 4,
        "only {} deployed containers were matched to a crate; the walk from an \
         image to the binary that builds it is finding nothing",
        found.len()
    );
    found
}

// ---------------------------------------------------------------------------
// Both directions of the seam
// ---------------------------------------------------------------------------

#[test]
fn every_variable_a_manifest_sets_is_one_the_binary_it_runs_actually_reads() {
    let constants = variables_by_constant();
    let mut checked = 0usize;
    let mut satisfied_by_the_file_variant = 0usize;
    let mut resolved_by_constant = 0usize;
    let mut resolved_by_delegation = 0usize;

    for (manifest, binary, container) in deployed_containers() {
        let read = variables_read_by(&binary, &constants);
        let literal = read.literal.clone();
        let all = read.all();
        assert!(
            all.len() >= 3,
            "{binary} appears to read only {all:?}; the source walk has stopped \
             finding variables and every manifest would now pass"
        );

        let set = variables_a_container_sets(&container);
        assert!(
            !set.is_empty(),
            "{manifest} sets no QIP_ variable for {binary}; the env block is no \
             longer being parsed and this check would pass on anything"
        );

        for variable in &set {
            if SET_BUT_NOT_READ
                .iter()
                .any(|(file, name, _)| *file == manifest && name == variable)
            {
                continue;
            }
            assert!(
                satisfies(&all, variable),
                "{manifest} sets {variable} for {binary} and that binary reads \
                 no such variable. A manifest that sets a variable nothing \
                 reads presents a control that does nothing: an operator who \
                 changes it sees a diff, an audit record and no effect \
                 whatsoever. Either the binary should read it, or it does not \
                 belong here. It reads {all:?}."
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
    // it from being an allowlist: without them the storage variables in
    // `api.yaml` and `fastbrain.yaml` would have to be excused by name.
    assert!(
        checked >= 20,
        "only {checked} variables were checked across every workload; the \
         manifest or the source walk is finding almost nothing"
    );
    assert!(
        satisfied_by_the_file_variant >= 1,
        "no variable was satisfied by its `_FILE` variant, so the rule that \
         maps a CSI-projected file back to the variable the binary reads is \
         never exercised and could have stopped working"
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
fn every_variable_a_binary_refuses_to_start_without_is_set_by_the_manifest_that_runs_it() {
    // The other direction, and the one whose symptom is a crash loop rather
    // than a silent no-op: `edge-cell.yaml` once set QIP_CELL_ID, QIP_CELL_REGION
    // and the envelope key but not QIP_VENUES, which `qip-edge-node` also
    // requires, so every pod exited 78 and was restarted for ever. Nothing else
    // catches it — the cell manifest is deliberately not applied by the
    // pipeline, so no rollout ever waits on it.
    let mut checked = 0usize;
    for (manifest, binary, container) in deployed_containers() {
        let set = variables_a_container_sets(&container);
        for variable in variables_required_by(&binary) {
            assert!(
                set.iter()
                    .any(|name| name == &variable || name == &format!("{variable}_FILE")),
                "{binary} refuses to start without {variable} and {manifest} \
                 does not set it. The container exits with a configuration \
                 error and is restarted for ever, which reads as a crash loop \
                 rather than as a missing value. It sets {set:?}."
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 4,
        "only {checked} required variables were checked; the walk from a \
         container to the source of the binary it runs is finding nothing"
    );
}

#[test]
fn no_manifest_sets_a_variable_its_binary_refuses_to_start_with() {
    // Retiring a variable is a third way for the two sides to disagree, and it
    // is the one that looks most harmless in a diff.
    //
    // `edge-cell.yaml` went on setting QIP_MIRROR_PATH after `qip-edge-node`
    // began refusing it — the file even carried a comment saying the node now
    // refuses it, directly beneath the line that set it. Every pod of every
    // cell deployed from that manifest exited 78 before binding anything. The
    // refusal was working exactly as designed, against a manifest nobody had
    // updated, and the comment made it look considered.
    let mut checked = 0usize;
    let mut binaries_with_a_retired_variable = 0usize;
    for (manifest, binary, container) in deployed_containers() {
        let refused = variables_refused_by(&binary);
        if !refused.is_empty() {
            binaries_with_a_retired_variable += 1;
        }
        let set = variables_a_container_sets(&container);
        for variable in &refused {
            assert!(
                !set.contains(variable),
                "{manifest} sets {variable} and {binary} refuses to start when \
                 it is set, naming its replacement in the refusal. The pod \
                 never binds a port: it exits with a configuration error \
                 before serving anything."
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
    // silently excuses whatever later takes the same name. Both halves of each
    // entry are re-proved here, so the day `qip-edge-node` reads the autonomy
    // ceiling, this fails and the entry has to go rather than quietly
    // continuing to excuse a variable that no longer needs excusing.
    let constants = variables_by_constant();
    let deployed = deployed_containers();
    for (manifest, variable, reason) in SET_BUT_NOT_READ {
        assert!(
            reason.len() > 120,
            "the allowlist entry for {variable} in {manifest} does not argue \
             its case; an exception without a reason is an exception nobody can \
             review"
        );
        let mut found = false;
        for (file, binary, container) in &deployed {
            if file != manifest {
                continue;
            }
            found = true;
            assert!(
                variables_a_container_sets(container).contains(*variable),
                "{manifest} no longer sets {variable}, so its allowlist entry \
                 excuses nothing and should be deleted"
            );
            let read = variables_read_by(binary, &constants).all();
            assert!(
                !satisfies(&read, variable),
                "{binary} now reads {variable}, so the allowlist entry in \
                 {manifest} is obsolete. Delete it: the general rule covers \
                 this variable now, and an obsolete exception is one that will \
                 excuse the next mistake with the same name."
            );
        }
        assert!(
            found,
            "the allowlist names {manifest}, which declares no workload this \
             walk can see"
        );
    }
}

// ---------------------------------------------------------------------------
// The mesh, which is where the general property was first violated
// ---------------------------------------------------------------------------

/// The `qip-config` ConfigMap's data, by key.
fn config_map_data() -> BTreeMap<String, String> {
    let content = without_comments(&read("infrastructure/kubernetes/base/config.yaml"));
    let mut data = BTreeMap::new();
    let mut inside = false;
    for line in content.lines() {
        if line.trim_end() == "data:" {
            inside = true;
            continue;
        }
        if !inside || line.trim().is_empty() {
            continue;
        }
        if line.len() - line.trim_start().len() != 2 {
            continue;
        }
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        data.insert(
            key.trim().to_string(),
            value.trim().trim_matches('"').to_string(),
        );
    }
    assert!(
        data.len() >= 4,
        "only {data:?} was parsed out of the config map; the walk is not \
         reading its keys"
    );
    data
}

/// The document declaring a named resource.
fn document_named(name: &str) -> String {
    manifest_documents()
        .into_iter()
        .find(|(_, document)| {
            without_comments(document)
                .lines()
                .any(|line| line.trim() == format!("name: {name}"))
        })
        .map(|(_, document)| document)
        .unwrap_or_else(|| panic!("{name} is declared in no manifest"))
}

/// The values of a key in a piece of manifest, as written.
///
/// `port` and `targetPort` are matched separately rather than by a prefix: a
/// `strip_prefix("port:")` reads `targetPort: 9110` as nothing at all, which is
/// how the first version of this walk counted half the Service and reported the
/// manifest as the fault.
fn values_of(text: &str, key: &str) -> Vec<String> {
    without_comments(text)
        .lines()
        .filter_map(|line| line.trim().strip_prefix(&format!("{key}:")))
        .map(|value| value.trim().to_string())
        .collect()
}

/// The `port:` values in a piece of manifest, as written.
fn ports_in(text: &str) -> Vec<String> {
    values_of(text, "port")
}

/// The cells and bind addresses `mesh_cells` names.
fn mesh_cells() -> Vec<(String, String)> {
    let data = config_map_data();
    let value = data
        .get("mesh_cells")
        .unwrap_or_else(|| panic!("the config map has no mesh_cells key: {data:?}"));
    let cells: Vec<(String, String)> = value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (cell, address) = entry
                .split_once('=')
                .unwrap_or_else(|| panic!("{entry} is not `cell=host:port`"));
            (cell.trim().to_string(), address.trim().to_string())
        })
        .collect();
    assert!(
        !cells.is_empty(),
        "mesh_cells names no cell, so the centre serves no mesh and every cell \
         pointed at it is partitioned. Unsetting the key is how a deployment \
         says it serves no mesh; an empty list is how it says nothing."
    );
    cells
}

#[test]
fn the_mesh_names_the_same_port_at_both_ends_of_the_wire() {
    // The bug this was written for: `QIP_MESH_CELLS` and `QIP_MESH_PEER`
    // appeared in no manifest at all, so ADR 0011's backbone was inert in every
    // deployment while being complete and tested in process.
    //
    // Wiring it introduces a fact that has to be written in four places —
    // Kubernetes cannot read a ConfigMap into a Service or into a
    // NetworkPolicy — and two independent claims about one fact will disagree.
    // This is what makes them agree.
    let data = config_map_data();
    let cells = mesh_cells();

    // The centre's half comes from the config map rather than from a literal,
    // so the two ends are edited in one file.
    let api = read("infrastructure/kubernetes/base/api.yaml");
    let api_env = without_comments(&api);
    assert!(
        api_env.contains("- name: QIP_MESH_CELLS"),
        "api.yaml does not set QIP_MESH_CELLS, so `qip-api` builds no mesh: no \
         listener binds, and every cell publishes its state at an address \
         nothing answers"
    );
    assert!(
        api_env.contains("key: mesh_cells"),
        "api.yaml sets QIP_MESH_CELLS from something other than the mesh_cells \
         config-map key, so the centre's listeners and the cells' peer \
         addresses no longer come from one place"
    );

    let mesh_service = document_named("qip-api-mesh");
    assert!(
        mesh_service.contains("app: qip-api"),
        "the qip-api-mesh Service does not select the API's pods, so it \
         publishes ports with nothing behind them"
    );
    let published = ports_in(&mesh_service);
    let targeted = values_of(&mesh_service, "targetPort");
    assert_eq!(
        published.len(),
        cells.len(),
        "the qip-api-mesh Service publishes {published:?} for {} cell(s). One \
         published port per cell: a cell without one cannot reach the centre \
         at all, and a port with no cell behind it is a connection that hangs.",
        cells.len()
    );
    assert_eq!(
        published, targeted,
        "the qip-api-mesh Service publishes {published:?} and targets \
         {targeted:?}. The centre binds the port the config map names, so a \
         Service that translates one port to another sends every cell to a \
         listener that is not its own — and the address is the cell's identity \
         on this transport."
    );

    let cell_manifest = read("infrastructure/kubernetes/base/edge-cell.yaml");
    let cell_env = without_comments(&cell_manifest);
    assert!(
        cell_env.contains("- name: QIP_MESH_PEER"),
        "edge-cell.yaml does not set QIP_MESH_PEER, so every cell runs detached: \
         it publishes no state, receives no capital, and stops when its current \
         envelope expires"
    );
    assert!(
        cell_env.contains("key: mesh_peer_CELL_ID"),
        "edge-cell.yaml takes its peer from a key other than mesh_peer_CELL_ID. \
         The per-cell key is what lets the runbook's existing CELL_ID \
         substitution carry a per-cell value into a shared manifest without a \
         placeholder nobody replaces."
    );
    assert!(
        cell_env.contains("optional: true"),
        "edge-cell.yaml requires the peer key. A cell with no peer is detached, \
         which ADR 0008 makes a legitimate state; a required reference turns \
         `the centre has not been told about this cell yet` into \
         `CreateContainerConfigError`, which is exactly backwards for an \
         architecture whose argument is that a cell survives losing the centre."
    );

    let mut checked = 0usize;
    for (cell, address) in &cells {
        let (host, port) = address
            .split_once(':')
            .unwrap_or_else(|| panic!("{address} names no port for {cell}"));
        assert_eq!(
            host, "0.0.0.0",
            "the centre binds {cell}'s listener on {host}, which no other pod \
             can reach. The peer is a cell in another pod, so a loopback bind \
             is a listener that exists and answers nobody."
        );

        let key = format!("mesh_peer_{cell}");
        let peer = data.get(&key).unwrap_or_else(|| {
            panic!(
                "mesh_cells names {cell} and the config map has no {key}, so \
                 the centre binds a listener for a cell that is never told \
                 where to find it: {data:?}"
            )
        });
        assert!(
            peer.starts_with("http://"),
            "{key} is {peer}. `qip_transport::http` has no TLS stack and \
             refuses the https scheme by name rather than downgrading it, so \
             anything else here is a cell that cannot connect at all."
        );
        assert!(
            peer.contains("qip-api-mesh.qip.svc.cluster.local"),
            "{key} is {peer}, which does not name the mesh Service. The \
             operator Service must not carry this traffic: the affinity that \
             pins a cell to one replica would pin every operator request to one \
             replica too."
        );
        let peer_port = peer
            .rsplit(':')
            .next()
            .unwrap_or_default()
            .trim_end_matches('/');
        assert_eq!(
            peer_port, port,
            "{cell} polls {peer} and the centre binds it on {address}. The \
             address is the cell's identity on this transport — a poll is a \
             destructive read with a single cursor and nothing else on the wire \
             names the cell — so a mismatched port is a cell publishing into a \
             connection nothing answers."
        );
        assert!(
            published.contains(&port.to_string()),
            "the qip-api-mesh Service does not publish {port} for {cell}; the \
             listener binds and no Service routes to it"
        );
        checked += 1;
    }
    assert!(
        checked >= 1,
        "no cell was checked; mesh_cells parsed to nothing and this test \
         measured only its own parser"
    );

    // And no peer key describes a cell the centre does not serve. That
    // direction is the silent one: the cell starts, believes it is attached,
    // and publishes every delta into a port nothing binds.
    for key in data.keys() {
        let Some(cell) = key.strip_prefix("mesh_peer_") else {
            continue;
        };
        assert!(
            cells.iter().any(|(named, _)| named == cell),
            "the config map gives {cell} a peer address and mesh_cells does not \
             name it, so that cell would run believing it is attached while the \
             centre binds no listener for it"
        );
    }
}

#[test]
fn the_mesh_port_is_permitted_by_the_policies_that_would_otherwise_drop_it() {
    // A mesh wired in environment variables and blocked by a default-deny
    // policy is still a mesh that serves nothing — and the failure is a
    // connection that hangs rather than one that is refused, so what an
    // operator sees is a cell reporting an open circuit breaker and no
    // indication that a policy is the cause.
    //
    // The namespace's ingress rule for the API permitted the ingress
    // controller on 8080 and nothing else, so before this rule existed the
    // cell's egress permission had nothing on the other side of it.
    let namespace = read("infrastructure/kubernetes/base/namespace.yaml");
    assert!(
        namespace.contains("podSelector: {}"),
        "the default deny is gone from namespace.yaml; every assertion below \
         is about a policy that would no longer be the only thing permitting \
         anything"
    );

    let ingress = document_named("allow-api-mesh-ingress");
    assert!(
        ingress.contains("app: qip-api"),
        "allow-api-mesh-ingress does not select the API's pods"
    );
    assert!(
        ingress.contains("app: qip-edge-node"),
        "allow-api-mesh-ingress does not permit the cells, which are the only \
         pods that speak this wire"
    );
    let permitted = ports_in(&ingress);

    // The cell's side of the same flow. Its `to:` block for the API is what
    // must carry the mesh port as well as 8080.
    let egress = document_named("allow-edge-egress");
    let toward_the_api = without_comments(&egress)
        .split("    - to:")
        .find(|block| block.contains("app: qip-api"))
        .unwrap_or_else(|| panic!("allow-edge-egress no longer permits the central plane at all"))
        .to_string();
    let outbound = ports_in(&toward_the_api);
    assert!(
        outbound.iter().any(|port| port == "8080"),
        "the cell may no longer reach the API on 8080, which is how the centre \
         delivers and recalls a capital envelope"
    );

    let mut checked = 0usize;
    for (cell, address) in mesh_cells() {
        let port = address.rsplit(':').next().unwrap_or_default().to_string();
        assert!(
            permitted.contains(&port),
            "allow-api-mesh-ingress permits {permitted:?} and {cell} publishes \
             to {port}. Under the namespace's default deny that flow does not \
             error, it hangs: the cell reports an open breaker and nothing \
             names the policy."
        );
        assert!(
            outbound.contains(&port),
            "allow-edge-egress lets a cell reach the API on {outbound:?} and \
             {cell} must reach {port}. The centre would listen, the Service \
             would route, and the packet would never leave the cell."
        );
        checked += 1;
    }
    assert!(
        checked >= 1,
        "no cell's port was checked against the policies; mesh_cells parsed to \
         nothing"
    );
}
