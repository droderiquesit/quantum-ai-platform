//! The constructs Terraform refuses, checked where Terraform cannot run.
//!
//! `infrastructure.rs` catches a configuration that parses perfectly and would
//! deploy something unsafe. This catches the other half: a configuration that
//! reads correctly to a person and that `terraform init` or `terraform plan`
//! rejects outright, so nothing deploys at all and the refusal is a message
//! nobody working in this repository can reproduce — there is no Terraform
//! binary here, and the first place the refusal appears is CI.
//!
//! That failure has already shipped here. `modules/cloudrun` declared
//! `variable "source"` — a name Terraform reserves for a module's own address
//! — and the whole suite passed straight through while every plan failed. The
//! rename to `image_source` and the reason are recorded at
//! `modules/cloudrun/variables.tf:272`. That comment protects one file;
//! `egress.rs` guards one `lifecycle` rule in the same file. Neither
//! generalises, and this file is the generalisation: every module, both
//! directions, and the caller-to-callee input correspondence that nothing
//! checked at all.
//!
//! Three of these properties find nothing today, which is the point — they are
//! regression guards. What stops a regression guard from becoming a guard over
//! nothing is the pair of fixture tests at the end: each drives the same
//! scanner over a fixture carrying the defect and asserts it is found. A scan
//! proven only by the tree it scans passes forever once the parser stops
//! parsing.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::repository_root;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// The names Terraform refuses as a `variable` because a `module` block
/// already gives them a meaning.
///
/// `source` is the one this repository has already been caught by; the
/// refusal is quoted verbatim in `modules/cloudrun/variables.tf`. The rest are
/// the remaining module-block meta-arguments, refused for the same reason and
/// equally invisible until `terraform init` runs.
const RESERVED_VARIABLE_NAMES: [&str; 8] = [
    "source",
    "version",
    "providers",
    "count",
    "for_each",
    "lifecycle",
    "depends_on",
    "locals",
];

/// The arguments a `module` block takes that are not module inputs.
///
/// These are Terraform's own, so they are never declared in the callee's
/// `variables.tf` and a correspondence check has to skip them.
const MODULE_META_ARGUMENTS: [&str; 6] = [
    "source",
    "count",
    "for_each",
    "depends_on",
    "providers",
    "version",
];

/// The `lifecycle` arguments Terraform evaluates before any value exists.
///
/// Each takes a literal — a static list, or a literal bool — and refuses
/// anything computed from an input with "A static list expression is required"
/// or its equivalent. The refusal is at validate time, so a tree carrying one
/// fails every commit rather than one plan.
const LITERAL_ONLY_LIFECYCLE_ARGUMENTS: [&str; 4] = [
    "ignore_changes",
    "replace_triggered_by",
    "prevent_destroy",
    "create_before_destroy",
];

/// The constructs that make a value computed rather than literal.
const COMPUTED_CONSTRUCTS: [&str; 6] = ["?", "var.", "local.", "each.", "count.", "data."];

/// A configuration with its comments removed.
///
/// A comment naming a construct is not the same as writing one, and a check
/// that cannot tell the difference makes it impossible to document why the
/// construct is refused — which is exactly what `modules/cloudrun` now does at
/// length.
fn without_comments(content: &str) -> String {
    content
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The Terraform root directory.
fn terraform_root() -> PathBuf {
    repository_root().join("infrastructure/terraform")
}

/// Every committed `.tf` file under `infrastructure/terraform`.
///
/// Deliberately not `qip_acceptance::files_with_extension`: that recurses into
/// hidden directories, and `terraform init` fills `.terraform/` with whatever a
/// provider or a remote module brought with it. That directory is gitignored,
/// so a scan that reads it answers one way on a machine that has run `init` and
/// another in CI. The answer that matters is the committed one.
fn committed_terraform_files() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![terraform_root()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "tf") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// One `module` block, as the caller wrote it.
#[derive(Debug)]
struct ModuleCall {
    /// The file the block is in, for a finding a person can go and look at.
    file: String,
    /// The block label.
    name: String,
    /// The line the block opens on, 1-based.
    line: usize,
    /// The `source` value, unquoted, when the block sets one.
    source: Option<String>,
    /// Every top-level argument the block passes, with its line.
    arguments: Vec<(String, usize)>,
}

/// The name a line assigns to, when the line is a top-level argument of the
/// block it sits in.
///
/// Two spaces of indent and no more. `terraform fmt --check` is a gate
/// (`.claude/rules/02-change-management.md`), and fmt puts every top-level
/// argument at exactly two spaces and every continuation — a list element, a
/// nested block's body — deeper. That makes the shallow depth the reliable
/// signal and a brace counter the fragile one: `catalogue.tf` holds a quoted
/// string containing an interpolation containing a quoted string, which a
/// counter has to get exactly right and this never has to read.
///
/// The known limit, stated rather than discovered: a heredoc body indented two
/// spaces and containing `key = value` would be read as an argument. No module
/// block in the tree contains a heredoc today, and the failure that would
/// cause is a loud one — a name reported as undeclared, which a person then
/// looks at — rather than a silent pass.
fn top_level_argument(line: &str) -> Option<String> {
    let rest = line.strip_prefix("  ")?;
    if rest.starts_with(' ') {
        return None;
    }
    let (name, _) = rest.split_once('=')?;
    let name = name.trim_end();
    // `console_enabled = var.console_egress_cidr != null` is an assignment
    // whose right-hand side holds a `!=`. Splitting on the *first* `=` already
    // puts that past the name; requiring the name to be a bare identifier is
    // what rejects a line that opens with a comparison instead.
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(name.to_string())
}

/// The value of a `name = "value"` line, unquoted.
fn quoted_value(line: &str) -> Option<String> {
    let (_, value) = line.split_once('=')?;
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .map(str::to_string)
}

/// Every `module` block in one file, with the arguments it passes.
///
/// Panics on a block that never closes rather than reading the rest of the
/// file as its arguments — the shape that would turn every assertion below
/// into one about the wrong block.
fn module_calls(file: &str, text: &str) -> Vec<ModuleCall> {
    let lines: Vec<&str> = text.lines().collect();
    let mut calls = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let Some(name) = lines[index]
            .strip_prefix("module \"")
            .and_then(|rest| rest.strip_suffix("\" {"))
        else {
            index += 1;
            continue;
        };
        let opened = index + 1;
        let mut arguments = Vec::new();
        let mut source = None;
        let mut cursor = index + 1;
        while cursor < lines.len() && lines[cursor] != "}" {
            if let Some(argument) = top_level_argument(lines[cursor]) {
                if argument == "source" {
                    source = quoted_value(lines[cursor]);
                }
                arguments.push((argument, cursor + 1));
            }
            cursor += 1;
        }
        assert!(
            cursor < lines.len(),
            "{file}: module \"{name}\" opened at line {opened} never closes at column zero; \
             everything below it would be read as its arguments"
        );
        calls.push(ModuleCall {
            file: file.to_string(),
            name: name.to_string(),
            line: opened,
            source,
            arguments,
        });
        index = cursor;
    }
    calls
}

/// The variables a `variables.tf` declares.
fn declared_variables(text: &str) -> BTreeSet<String> {
    without_comments(text)
        .lines()
        .filter_map(|line| {
            line.strip_prefix("variable \"")
                .and_then(|rest| rest.strip_suffix("\" {"))
                .map(str::to_string)
        })
        .collect()
}

/// The variables a `variables.tf` declares with no `default`.
///
/// Terraform refuses a plan whose module block omits one of these, and the
/// refusal names the variable and nothing about the caller that omitted it.
fn variables_without_a_default(text: &str) -> BTreeSet<String> {
    let stripped = without_comments(text);
    let lines: Vec<&str> = stripped.lines().collect();
    let mut required = BTreeSet::new();
    let mut index = 0;
    while index < lines.len() {
        let Some(name) = lines[index]
            .strip_prefix("variable \"")
            .and_then(|rest| rest.strip_suffix("\" {"))
        else {
            index += 1;
            continue;
        };
        let mut has_default = false;
        let mut cursor = index + 1;
        while cursor < lines.len() && lines[cursor] != "}" {
            if top_level_argument(lines[cursor]).as_deref() == Some("default") {
                has_default = true;
            }
            cursor += 1;
        }
        if !has_default {
            required.insert(name.to_string());
        }
        index = cursor.max(index + 1);
    }
    required
}

/// Every module block in the root, paired with the text of the callee's
/// `variables.tf`.
///
/// Carries the premise both correspondence tests depend on: the parse found
/// blocks, and it found arguments in them. A parser that had quietly stopped
/// matching would otherwise satisfy every assertion below by having nothing
/// left to compare.
fn root_module_calls() -> Vec<(ModuleCall, String)> {
    let root = terraform_root();
    let mut calls = Vec::new();
    for path in committed_terraform_files() {
        // Only the root calls modules;
        // `the_module_graph_stays_one_level_deep_and_entirely_local` is what
        // keeps that true.
        if path.parent() != Some(root.as_path()) {
            continue;
        }
        let file = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<non-utf8>")
            .to_string();
        let text = without_comments(&std::fs::read_to_string(&path).expect("a readable .tf file"));
        for call in module_calls(&file, &text) {
            let source = call
                .source
                .clone()
                .unwrap_or_else(|| panic!("{file}: module \"{}\" declares no source", call.name));
            let variables = root
                .join(source.trim_start_matches("./"))
                .join("variables.tf");
            let declared = std::fs::read_to_string(&variables).unwrap_or_else(|error| {
                panic!(
                    "{file}: module \"{}\" sources {source}, whose variables.tf cannot be read: \
                     {error}",
                    call.name
                )
            });
            calls.push((call, declared));
        }
    }
    assert!(
        calls.len() >= 15,
        "only {} module blocks were parsed out of the Terraform root; the tree declared 19 when \
         this scan was written, so the parser has stopped matching and every check below is \
         comparing nothing",
        calls.len()
    );
    let arguments: usize = calls.iter().map(|(call, _)| call.arguments.len()).sum();
    assert!(
        arguments >= 180,
        "only {arguments} module arguments were parsed; 226 were present when this scan was \
         written, so the argument walk has stopped matching and the correspondence checks are \
         comparing almost nothing"
    );
    calls
}

// --- the correspondence nothing checked ------------------------------------

#[test]
fn every_argument_a_module_block_passes_is_a_variable_the_module_declares() {
    // Terraform refuses this at validate: "An argument named "x" is not
    // expected here." Nothing in this workspace could see it — the only
    // handling of a `module` block anywhere in the suite is a block-delimiter
    // split and a substring slice, and neither reads an argument name. A
    // rename in a module's variables.tf that missed one of its callers, or an
    // argument added to a call and never added to the module, blocks every
    // apply and no test here notices.
    let mut findings = Vec::new();
    for (call, declared_text) in root_module_calls() {
        let declared = declared_variables(&declared_text);
        let source = call.source.clone().unwrap_or_default();
        for (argument, line) in &call.arguments {
            if MODULE_META_ARGUMENTS.contains(&argument.as_str()) {
                continue;
            }
            if !declared.contains(argument) {
                findings.push(format!(
                    "{}:{line} module \"{}\" passes \"{argument}\", which {source} does not \
                     declare",
                    call.file, call.name
                ));
            }
        }
    }
    assert!(
        findings.is_empty(),
        "Terraform refuses each of these with \"An argument named ... is not expected here\", so \
         no plan runs at all until they are fixed: {findings:#?}"
    );
}

#[test]
fn every_module_input_without_a_default_is_passed_by_every_block_that_calls_it() {
    // The same refusal from the other side: "No value for required variable".
    // Adding a required input to a module and forgetting one of its callers is
    // the ordinary way this happens, and it is invisible in the diff of the
    // module, which is where the reviewer is looking.
    let mut findings = Vec::new();
    for (call, declared_text) in root_module_calls() {
        let passed: BTreeSet<&str> = call
            .arguments
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        let source = call.source.clone().unwrap_or_default();
        for required in variables_without_a_default(&declared_text) {
            if !passed.contains(required.as_str()) {
                findings.push(format!(
                    "{}:{} module \"{}\" omits required input \"{required}\" ({source})",
                    call.file, call.line, call.name
                ));
            }
        }
    }
    assert!(
        findings.is_empty(),
        "Terraform refuses each of these with \"No value for required variable\": {findings:#?}"
    );
}

// --- the names Terraform will not accept ------------------------------------

#[test]
fn no_module_variable_is_named_one_of_the_words_terraform_reserves() {
    // This has already happened. `modules/cloudrun` declared `variable
    // "source"` and Terraform refused it outright — "The variable name
    // "source" is reserved due to its special meaning inside module blocks" —
    // at init, before any plan. The suite passed; CI's Terraform job is what
    // caught it. The module was renamed to `image_source` and the reason is
    // written at `modules/cloudrun/variables.tf:272`, which protects that one
    // file and no other. The other seventeen modules are what this covers.
    let mut scanned = 0usize;
    let mut findings = Vec::new();
    for path in committed_terraform_files() {
        let text = without_comments(&std::fs::read_to_string(&path).expect("a readable .tf file"));
        for (index, line) in text.lines().enumerate() {
            scanned += 1;
            let Some(name) = line
                .strip_prefix("variable \"")
                .and_then(|rest| rest.strip_suffix("\" {"))
            else {
                continue;
            };
            // The whole name, never a substring: `image_source` ends in
            // `source` and is the *correction*, so a `contains` here would
            // report the fix as the defect.
            if RESERVED_VARIABLE_NAMES.contains(&name) {
                findings.push(format!(
                    "{}:{} variable \"{name}\"",
                    path.display(),
                    index + 1
                ));
            }
        }
    }
    // The premise: lines were read. A walk that reached no file would satisfy
    // the assertion below by never running it.
    assert!(
        scanned > 8000,
        "only {scanned} lines of Terraform were scanned; there were 10,974 when this scan was \
         written, so this proved nothing about the ones it did not read"
    );
    assert!(
        findings.is_empty(),
        "Terraform refuses each of these at init with \"The variable name ... is reserved due to \
         its special meaning inside module blocks\", and a caller could never pass it: \
         {findings:#?}"
    );
}

#[test]
fn every_lifecycle_argument_terraform_requires_to_be_literal_is_written_as_one() {
    // Also already shipped here: an `ignore_changes` computed with a ternary
    // from an input, refused with "A static list expression is required" and
    // failing validate on every commit that carried it. `egress.rs` guards
    // that one rule's *content* — that it still names the workload image.
    // This guards the *form*, in every module and the root: fourteen
    // literal-only lifecycle arguments, thirteen of which nothing scans.
    let mut examined = 0usize;
    let mut findings = Vec::new();
    for path in committed_terraform_files() {
        let text = without_comments(&std::fs::read_to_string(&path).expect("a readable .tf file"));
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            let Some(argument) = LITERAL_ONLY_LIFECYCLE_ARGUMENTS
                .iter()
                .find(|argument| trimmed.starts_with(&format!("{argument} =")))
            else {
                continue;
            };
            examined += 1;
            // A ternary, or any reference to an input, a local, a data source
            // or an iteration variable. Terraform evaluates `lifecycle` before
            // values exist, so all of them are refused however sensible the
            // line reads.
            for construct in COMPUTED_CONSTRUCTS {
                if trimmed.contains(construct) {
                    findings.push(format!(
                        "{}:{} {argument} is computed from `{construct}`: {trimmed}",
                        path.display(),
                        index + 1
                    ));
                }
            }
        }
    }
    // The premise. A tree that had stopped declaring these arguments entirely
    // would pass by having nothing to look at, and the rules that make an
    // apply idempotent — the workload image, the managed instance count —
    // would be gone.
    assert!(
        examined >= 10,
        "only {examined} literal-only lifecycle arguments were found; there were 14 when this \
         scan was written, so either the rules are gone or the scan no longer sees them"
    );
    assert!(
        findings.is_empty(),
        "Terraform refuses a computed value in `lifecycle`, so every commit carrying one fails \
         validate and nothing deploys: {findings:#?}"
    );
}

#[test]
fn the_module_graph_stays_one_level_deep_and_entirely_local() {
    // Both correspondence checks read the root's calls and the callee's
    // variables.tf. A module block nested inside a module, or a `source`
    // pointing at a registry or a git URL, is a call this file never sees —
    // and it stays green while covering less, which is the failure mode that
    // reads as coverage.
    let root = terraform_root();
    let mut examined = 0usize;
    let mut nested = Vec::new();
    let mut remote = Vec::new();
    for path in committed_terraform_files() {
        let text = without_comments(&std::fs::read_to_string(&path).expect("a readable .tf file"));
        let in_root = path.parent() == Some(root.as_path());
        for call in module_calls(&path.display().to_string(), &text) {
            examined += 1;
            if !in_root {
                nested.push(format!("{}: module \"{}\"", path.display(), call.name));
                continue;
            }
            match call.source.as_deref() {
                Some(source) if source.starts_with("./modules/") => {}
                other => remote.push(format!(
                    "{}: module \"{}\" sources {other:?}",
                    path.display(),
                    call.name
                )),
            }
        }
    }
    assert!(
        examined >= 15,
        "only {examined} module blocks were seen anywhere in the tree; 19 were present when this \
         scan was written, so the scan has stopped seeing them and proves nothing about their \
         sources"
    );
    assert!(
        nested.is_empty(),
        "a module calls another module; the correspondence scan reads only the root's calls and \
         would silently stop covering these: {nested:#?}"
    );
    assert!(
        remote.is_empty(),
        "a module block sources something other than ./modules/<dir>; its variables are not in \
         this repository and the correspondence scan cannot read them: {remote:#?}"
    );
}

// --- the proof that the scan fires ------------------------------------------

/// A root file with both defects in it, and nothing else wrong.
///
/// The `source` is real, so the fixture exercises the same lookup the tree
/// does: `network` is passed a name it does not declare — `console_egress_cidr`
/// misspelled — and omits several it requires.
const A_ROOT_THAT_WOULD_NOT_PLAN: &str = "module \"network\" {
  source = \"./modules/network\"

  project_id         = var.project_id
  consol_egress_cidr = var.console_egress_cidr
}
";

#[test]
fn a_module_block_passing_an_argument_the_module_never_declared_is_found_by_this_scan() {
    // This is the mutation, kept rather than performed. The tree is shared
    // with other work and `infrastructure/**` belongs to another workstream,
    // so breaking a real module to watch a test fail and restoring it is a
    // window in which somebody else reads or commits the break. A fixture
    // carrying the defect proves the same thing and proves it on every run,
    // which is the stronger claim: the scan above finds nothing today, and
    // this is what says it would find something if there were something.
    let calls = module_calls("fixture.tf", A_ROOT_THAT_WOULD_NOT_PLAN);
    assert_eq!(calls.len(), 1, "the fixture parses as one module block");
    let declared = declared_variables(
        &std::fs::read_to_string(terraform_root().join("modules/network/variables.tf"))
            .expect("modules/network/variables.tf is readable"),
    );
    // The premise: the *correct* spelling is declared. A module that declared
    // neither name would make this test pass for the wrong reason.
    assert!(
        declared.contains("console_egress_cidr"),
        "modules/network no longer declares console_egress_cidr; this fixture's premise needs \
         rewriting rather than the scan"
    );
    let undeclared: Vec<&str> = calls[0]
        .arguments
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !MODULE_META_ARGUMENTS.contains(name) && !declared.contains(*name))
        .collect();
    assert_eq!(
        undeclared,
        vec!["consol_egress_cidr"],
        "the scan did not find the misspelled argument, so it would not have found the one that \
         stopped every plan"
    );
}

#[test]
fn a_module_block_omitting_a_required_input_is_found_by_this_scan() {
    // The other direction, same reasoning: a fixture rather than a mutation of
    // a file another workstream owns.
    let text = std::fs::read_to_string(terraform_root().join("modules/network/variables.tf"))
        .expect("modules/network/variables.tf is readable");
    let required = variables_without_a_default(&text);
    // The premise, on the delimited name rather than a substring: `region` is
    // a substring of nothing here today, and writing it as a set membership is
    // what keeps that true if a `region_override` is ever added.
    assert!(
        required.contains("region"),
        "modules/network no longer requires `region` without a default; this fixture's premise \
         needs rewriting rather than the scan"
    );
    let calls = module_calls("fixture.tf", A_ROOT_THAT_WOULD_NOT_PLAN);
    let passed: BTreeSet<&str> = calls[0]
        .arguments
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        !passed.contains("region"),
        "the fixture passes `region`, so it cannot demonstrate the omission"
    );
    let omitted: Vec<&String> = required
        .iter()
        .filter(|name| !passed.contains(name.as_str()))
        .collect();
    assert!(
        omitted.contains(&&"region".to_string()),
        "the scan found no omitted required input in a block that omits several: {omitted:#?}"
    );
}

// --- the condition ADR 0030 set for itself, enforced ------------------------
//
// ADR 0030 put OpenObserve on the internet anonymously *because it was empty*,
// and wrote its own revisit trigger: "the moment any deployment sets
// `QIP_OPENOBSERVE_URL`". ADR 0033 is that trigger firing and is not yet
// applied. Nothing checked the trigger — the comment beside the posture in
// `catalogue.tf` said it, and a comment stops nobody. The first catalogue
// entry to name the variable would have pointed the platform's telemetry at a
// store anyone on the internet can read and write, with every gate green.

/// The variable whose first appearance in a deployment is ADR 0030's trigger.
const OPENOBSERVE_URL_VARIABLE: &str = "QIP_OPENOBSERVE_URL";

/// Where the RunService manifests live since ADR 0036: one directory per
/// environment, each holding OpenObserve's manifest beside the catalogue's.
const GITOPS_ENVS: &str = "infrastructure/gitops/envs";
const ENVIRONMENTS: [&str; 4] = ["dev", "test", "stage", "prod"];

/// The `RunService` documents under every environment, as
/// `(file, environment, document text)`. Line-based, like the catalogue
/// reads here: a document is the text between `---` separators.
fn run_service_documents() -> Vec<(String, String, String)> {
    let mut documents = Vec::new();
    for environment in ENVIRONMENTS {
        let directory = terraform_root()
            .parent()
            .expect("the terraform root sits under infrastructure/")
            .parent()
            .expect("infrastructure/ sits under the repository root")
            .join(GITOPS_ENVS)
            .join(environment);
        assert!(
            directory.is_dir(),
            "{GITOPS_ENVS}/{environment} does not exist; ADR 0036 decision 4 puts OpenObserve's \
             RunService there, and until it lands this tripwire cannot read the posture"
        );
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut stack = vec![directory];
        while let Some(current) = stack.pop() {
            for entry in std::fs::read_dir(&current).expect("readable").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        for path in paths {
            let content = std::fs::read_to_string(&path).expect("readable");
            for document in content.split("\n---") {
                if document
                    .lines()
                    .any(|line| line.trim_end() == "kind: RunService")
                {
                    documents.push((
                        path.display().to_string(),
                        environment.to_string(),
                        document.to_string(),
                    ));
                }
            }
        }
    }
    documents
}

/// Whether OpenObserve is anonymously reachable — `ingress: INGRESS_TRAFFIC_ALL`
/// on its RunService — in every environment. The four must agree: a posture
/// that differs by environment is one this tripwire cannot reason about.
fn openobserve_is_anonymous() -> bool {
    let mut postures: Vec<(String, bool)> = Vec::new();
    for (file, environment, document) in run_service_documents() {
        let named = document
            .lines()
            .any(|line| line.trim_end() == format!("  name: qip-{environment}-openobserve"));
        if !named {
            continue;
        }
        let open = document
            .lines()
            .any(|line| line.trim() == "ingress: INGRESS_TRAFFIC_ALL");
        postures.push((file, open));
    }
    // Deployed only where the tfvars name its digest — dev today — so the
    // premise is at least one, and every environment that has it agrees.
    assert!(
        !postures.is_empty(),
        "no environment under {GITOPS_ENVS} deploys an OpenObserve RunService; the posture \
         this tripwire reads is nowhere"
    );
    let mut distinct: Vec<bool> = postures.iter().map(|(_, open)| *open).collect();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        1,
        "OpenObserve's posture differs by environment: {postures:?}"
    );
    distinct[0]
}

/// Every RunService container `env` entry naming `name`, as `file:line`.
///
/// The whole identifier: `- name: QIP_OPENOBSERVE_URL_FILE` is not the
/// variable, and the value after `name:` is compared as a token.
fn run_services_setting(name: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (file, _, document) in run_service_documents() {
        for (index, line) in document.lines().enumerate() {
            let entry = line.trim_start();
            let entry = entry.strip_prefix("- ").unwrap_or(entry);
            if entry
                .strip_prefix("name:")
                .is_some_and(|value| value.trim().trim_matches('"') == name)
            {
                found.push(format!("{file}:{}", index + 1));
            }
        }
    }
    found
}

/// The body of a column-zero `module "<name>" {` block, comments already
/// stripped by the caller, or `None` when the file declares no such block.
fn module_block_body<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let opening = format!("module \"{name}\" {{\n");
    let start = text.find(&opening)? + opening.len();
    let end = text[start..].find("\n}\n")?;
    Some(&text[start..start + end])
}

/// Every line, 1-based, whose left-hand side is exactly `name`.
///
/// The whole identifier, never a substring: `QIP_OPENOBSERVE_URL_FILE` would
/// contain the token, and the `env` and `secret_env` maps both write the
/// variable as `NAME = ...`, so the name up to the `=` is the delimited thing.
fn lines_setting(text: &str, name: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            line.split_once('=')
                .is_some_and(|(lhs, _)| lhs.trim() == name && !lhs.trim().is_empty())
        })
        .map(|(index, _)| index + 1)
        .collect()
}

/// The tripwire, over a catalogue's text and the manifests' posture: refused
/// when OpenObserve is still anonymous and any deployment — a catalogue
/// entry, or a RunService's own env — sets the variable that would fill it.
///
/// The posture is the manifest's since ADR 0036 (`ingress: INGRESS_TRAFFIC_ALL`
/// on OpenObserve's RunService, where the catalogue's module block used to
/// say `open-anonymous`), so it is passed in rather than read from the
/// catalogue. Returns the refusal as an `Err` rather than panicking so the
/// fixture test below can prove it fires without the process aborting on
/// the fixture.
fn openobserve_stays_empty_while_anonymous(
    catalogue: &str,
    anonymous: bool,
    manifest_setters: &[String],
) -> Result<(), String> {
    if !anonymous {
        return Ok(());
    }
    let setters = lines_setting(catalogue, OPENOBSERVE_URL_VARIABLE);
    if setters.is_empty() && manifest_setters.is_empty() {
        return Ok(());
    }
    Err(format!(
        "catalogue.tf sets {OPENOBSERVE_URL_VARIABLE} at line(s) {setters:?} and the RunService \
         manifests set it at {manifest_setters:?} while OpenObserve's RunService is still \
         `ingress: INGRESS_TRAFFIC_ALL` with `allUsers` as its invoker (open-anonymous). \
         That is the trigger ADR 0030 set for itself — the store stops being empty the \
         moment a deployment sets this variable — and ADR 0033 is the decision it fires: \
         apply ADR 0033 (IAP, `public-edge`, named principals) before any workload points \
         its telemetry here, or re-argue the exposure in a new record. Do not set the \
         variable first."
    ))
}

#[test]
fn no_deployment_points_telemetry_at_openobserve_while_it_is_anonymous() {
    let catalogue = without_comments(
        &std::fs::read_to_string(terraform_root().join("catalogue.tf"))
            .expect("catalogue.tf is readable"),
    );
    // The premises, each stated so the assertion below cannot pass by
    // scanning the wrong thing. First: OpenObserve is deployed and is
    // anonymous today, on its RunService in every environment. When ADR
    // 0033 is applied this line fails alongside the pins in gitops.rs, and
    // the change is to retire this tripwire with them, not to loosen it.
    assert!(
        module_block_body(&catalogue, "openobserve").is_some(),
        "catalogue.tf no longer declares module \"openobserve\"; the workload's identity is gone \
         and this tripwire's subject with it"
    );
    let anonymous = openobserve_is_anonymous();
    assert!(
        anonymous,
        "OpenObserve's RunService is no longer INGRESS_TRAFFIC_ALL; ADR 0033 has been applied \
         and this tripwire's premise is gone — retire it with the pins in gitops.rs"
    );
    // Second: the scanner sees the catalogue's variables at all. The three
    // built entries each set the autonomy ceiling, so a walk that found fewer
    // has stopped reading `env` maps and would find nothing below either.
    let ceilings = lines_setting(&catalogue, "QIP_AUTONOMY_CEILING");
    assert!(
        ceilings.len() >= 3,
        "only {ceilings:?} lines set QIP_AUTONOMY_CEILING; the three catalogue entries each \
         set it, so the scan has stopped reading env maps"
    );
    // Third: today nothing sets the variable — neither the catalogue nor a
    // RunService — which is what ADR 0030 relied on and `manifest_wiring.rs`
    // records for each binary. The manifest scan sees env entries at all:
    // every RunService sets the autonomy ceiling, so a walk finding fewer
    // than four has stopped reading env lists.
    let manifest_ceilings = run_services_setting("QIP_AUTONOMY_CEILING");
    assert!(
        manifest_ceilings.len() >= 4,
        "only {manifest_ceilings:?} RunService env entries set QIP_AUTONOMY_CEILING; the \
         manifest scan has stopped reading env lists"
    );
    assert!(
        lines_setting(&catalogue, OPENOBSERVE_URL_VARIABLE).is_empty(),
        "catalogue.tf now sets {OPENOBSERVE_URL_VARIABLE}; this test's premise that nothing \
         does is false, and the assertion below is the one that says what to do"
    );
    let manifest_setters = run_services_setting(OPENOBSERVE_URL_VARIABLE);
    if let Err(refusal) =
        openobserve_stays_empty_while_anonymous(&catalogue, anonymous, &manifest_setters)
    {
        panic!("{refusal}");
    }
}

#[test]
fn a_catalogue_entry_setting_the_openobserve_url_while_it_is_anonymous_is_refused() {
    // The mutation, kept rather than performed, for the reason the two
    // fixture tests above give: `infrastructure/**` is another workstream's
    // and a real edit is a window somebody else reads. The fixture is the
    // real catalogue with one line added to the API entry's `env` map, so
    // the parse being proven is the parse of the file as it is.
    let catalogue = without_comments(
        &std::fs::read_to_string(terraform_root().join("catalogue.tf"))
            .expect("catalogue.tf is readable"),
    );
    let anchor = "        QIP_AUTONOMY_CEILING = var.autonomy_ceiling\n";
    assert!(
        catalogue.contains(anchor),
        "the API entry no longer sets QIP_AUTONOMY_CEILING at the indentation this fixture \
         splices after; move the anchor rather than the tripwire"
    );
    let with_url = catalogue.replacen(
        anchor,
        &format!(
            "{anchor}        {OPENOBSERVE_URL_VARIABLE} = \"http://openobserve.internal:5080\"\n"
        ),
        1,
    );
    assert_eq!(
        lines_setting(&with_url, OPENOBSERVE_URL_VARIABLE).len(),
        1,
        "the fixture sets the variable exactly once and the scan must find that line"
    );
    let refusal = openobserve_stays_empty_while_anonymous(&with_url, true, &[])
        .expect_err("a catalogue setting the URL while OpenObserve is anonymous is refused");
    for named in [
        "ADR 0030",
        "ADR 0033",
        OPENOBSERVE_URL_VARIABLE,
        "open-anonymous",
    ] {
        assert!(
            refusal.contains(named),
            "the refusal does not name {named}, so the person reading it is not told which \
             record to apply: {refusal}"
        );
    }

    // The same fixture with the posture already moved is admitted: the
    // tripwire is about the combination, and a gate that refused the URL
    // under every posture would refuse the fix ADR 0033 prescribes.
    assert_eq!(
        openobserve_stays_empty_while_anonymous(&with_url, false, &[]),
        Ok(()),
        "the tripwire refuses the URL even once OpenObserve is behind IAP, so it would refuse \
         ADR 0033's own outcome"
    );
    // And a manifest setting it is refused the same way, naming the file.
    let from_manifest = openobserve_stays_empty_while_anonymous(
        &catalogue,
        true,
        &["infrastructure/gitops/envs/dev/api.yaml:40".to_string()],
    )
    .expect_err("a RunService setting the URL while OpenObserve is anonymous is refused");
    assert!(
        from_manifest.contains("envs/dev/api.yaml:40"),
        "the refusal does not name the manifest that set the variable: {from_manifest}"
    );

    // And the delimited match: a name that merely starts with the variable
    // is not the variable.
    let lookalike = catalogue.replacen(
        anchor,
        &format!("{anchor}        {OPENOBSERVE_URL_VARIABLE}_FILE = \"/etc/qip/collector\"\n"),
        1,
    );
    assert_eq!(
        openobserve_stays_empty_while_anonymous(&lookalike, true, &[]),
        Ok(()),
        "the scan matched `{OPENOBSERVE_URL_VARIABLE}_FILE` as the variable; the match is on a \
         substring, which is the trap the testing rules name"
    );
}
