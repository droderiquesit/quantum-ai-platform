//! The platform reaches a language model through one gate, and nothing else
//! names a model provider (R12 SEC-9078, ADR 0117).
//!
//! # The path, as built
//!
//! A hosted model is reached by exactly one route. The composition root
//! (`qip-deepbrain`) builds the one hosted adapter, `qip-reasoning-engine`'s
//! `HuggingFaceModel` (ADR 0037), behind the `qip_ai::language::LanguageModel`
//! port and hands it to the kernel. The kernel's REASON stage asks
//! `qip-cost-router` to place the decision first — and the router's
//! `Determinism::Required` arm returns a type that cannot name a model rung —
//! and convenes the agent organisation only after the router placed it. An
//! agent then reaches the model only through `AgentRuntime::complete`, which
//! refuses an agent not granted `CallLanguageModel` and charges the run's
//! token budget before the call.
//!
//! # What fails here
//!
//! A crate that grew its own route around that path: a second adapter, a
//! provider URL or wire path written into some service, a model invoked from
//! anywhere but the agent runtime, the hosted adapter constructed outside a
//! composition root, or the organisation convened before — or without — the
//! router's placement. "A direct call fails" is the requirement; in a
//! codebase that cannot call anything it has not been compiled to call, the
//! structural form of it is that no other code can be compiled to.
//!
//! # What it cannot see
//!
//! These are text scans of production source, as the neighbouring
//! architecture suites are. A provider reached through a URL assembled from
//! parts at run time, or through the generic HTTP client pointed at a
//! configured address, names no host here; the egress proxy's route list
//! (`egress.rs`) is the check on where a configured address can lead. The
//! development agents' half of SEC-9078 — no model call from the agent
//! sandbox — is `scripts/agent-egress.test.mjs`'s.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use std::collections::{BTreeMap, BTreeSet};

/// Hosts that serve a hosted language model, and the wire markers of the
/// chat APIs they speak. Not exhaustive and not meant to be: a provider that
/// is not on this list is still caught by the port and invocation checks
/// below, because it can only be called through an adapter.
const PROVIDER_MARKERS: [&str; 21] = [
    "api.anthropic.com",
    "api.openai.com",
    "generativelanguage.googleapis.com",
    "aiplatform.googleapis.com",
    "router.huggingface.co",
    "api-inference.huggingface.co",
    "api.deepseek.com",
    "api.mistral.ai",
    "api.cohere.ai",
    "api.cohere.com",
    "bedrock-runtime",
    "api.together.xyz",
    "api.groq.com",
    "openrouter.ai",
    "api.x.ai",
    "/v1/chat/completions",
    "/v1/completions",
    "/v1/messages",
    "anthropic-version",
    ":generateContent",
    ":streamGenerateContent",
];

/// The production files permitted to name a provider marker, each with why.
/// Equality, not a subset: a file that stops naming one must be struck, so
/// the list cannot become a record of permissions nobody uses.
const DECLARED_PROVIDER_FILES: [(&str, &str); 3] = [
    (
        "backend/crates/services/qip-reasoning-engine/src/providers/huggingface.rs",
        "the one hosted language-model adapter (ADR 0037): its upstream host and chat path",
    ),
    (
        "backend/crates/services/qip-training/src/vertex.rs",
        "the Vertex AI training control plane (ADR 0083): submits training jobs, calls no model",
    ),
    (
        "backend/crates/apps/qip-deepbrain/src/config.rs",
        "the composition root's refusal text naming the proxy's upstream for an operator",
    ),
];

/// The production files permitted to implement the language-model port.
const DECLARED_PORT_IMPLEMENTATIONS: [&str; 2] = [
    // The port itself, its deterministic stand-in, the unavailable remote
    // placeholder, the fallback chain and the reply-counting decorator the
    // deep brain's tally wraps a model in — none performs I/O.
    "backend/crates/libs/qip-ai/src/language.rs",
    "backend/crates/services/qip-reasoning-engine/src/providers/huggingface.rs",
];

/// The production files permitted to invoke a model.
const DECLARED_INVOCATIONS: [&str; 2] = [
    // `complete_structured`'s default body, the fallback chain delegating,
    // and the reply-counting decorator passing each call through.
    "backend/crates/libs/qip-ai/src/language.rs",
    // `AgentRuntime::complete`: capability check, then budget, then the call.
    "backend/crates/libs/qip-agents/src/runtime.rs",
];

/// Every production Rust file in the workspace, repository-relative, with its
/// production text. The acceptance crate's own tests and every `tests/`
/// directory are excluded: a fixture naming a provider is not a route to one.
fn production_sources() -> BTreeMap<String, String> {
    let root = qip_acceptance::repository_root();
    let mut sources = BTreeMap::new();
    for path in qip_acceptance::files_with_extension("backend/crates", "rs") {
        let relative = path
            .strip_prefix(&root)
            .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());
        if !relative.contains("/src/") || relative.contains("/tests/") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {relative}: {error}"));
        sources.insert(relative, qip_acceptance::production_text(&text));
    }
    sources
}

fn files_containing<'a>(
    sources: &'a BTreeMap<String, String>,
    needles: &[&str],
) -> BTreeMap<&'a str, BTreeSet<String>> {
    let mut found: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for (file, text) in sources {
        for needle in needles {
            if text.contains(needle) {
                found
                    .entry(file.as_str())
                    .or_default()
                    .insert((*needle).to_string());
            }
        }
    }
    found
}

/// The premise every scan below shares, asserted once per test: a walk that
/// read nothing finds no violation.
fn assert_the_walk_sees_the_workspace(sources: &BTreeMap<String, String>) {
    assert!(
        sources.len() > 500,
        "only {} production Rust files were read under backend/crates; the walk is \
         not seeing the workspace, so every absence asserted here is trivially true",
        sources.len()
    );
    for file in DECLARED_PROVIDER_FILES.map(|(file, _)| file) {
        assert!(
            sources
                .get(file)
                .is_some_and(|text| !text.trim().is_empty()),
            "{file} was not read as production source; the scan is blind to the one \
             place a provider is meant to be named"
        );
    }
}

#[test]
fn only_the_declared_adapters_name_a_model_provider_endpoint() {
    let sources = production_sources();
    assert_the_walk_sees_the_workspace(&sources);

    let found = files_containing(&sources, &PROVIDER_MARKERS);
    let named: BTreeSet<&str> = found.keys().copied().collect();
    let declared: BTreeSet<&str> = DECLARED_PROVIDER_FILES.map(|(file, _)| file).into();

    let undeclared: Vec<_> = found
        .iter()
        .filter(|(file, _)| !declared.contains(*file))
        .collect();
    assert!(
        undeclared.is_empty(),
        "production code outside the declared adapters names a model provider: \
         {undeclared:?}. The platform reaches a model only through the \
         `qip_ai::language::LanguageModel` port, behind the cost router's placement \
         in the REASON stage (ADR 0117); a provider endpoint written anywhere else is a \
         route around both. Put the call behind the port, or argue for the file in \
         DECLARED_PROVIDER_FILES and ADR 0117"
    );
    let unused: Vec<_> = declared.difference(&named).collect();
    assert!(
        unused.is_empty(),
        "these files are declared as naming a model provider and no longer do: \
         {unused:?}. Strike them so the declaration stays a description of the tree"
    );
}

#[test]
fn only_the_declared_adapters_implement_the_language_model_port() {
    let sources = production_sources();
    assert_the_walk_sees_the_workspace(&sources);

    let found = files_containing(
        &sources,
        &[
            "impl LanguageModel for",
            "impl qip_ai::language::LanguageModel for",
            "impl language::LanguageModel for",
        ],
    );
    let implementing: BTreeSet<&str> = found.keys().copied().collect();
    let declared: BTreeSet<&str> = DECLARED_PORT_IMPLEMENTATIONS.into();
    assert_eq!(
        implementing, declared,
        "the set of production files implementing `LanguageModel` changed. A new \
         implementation is a new way to reach a model and needs ADR 0117's list and \
         an argument; a removed one must be struck"
    );
}

#[test]
fn a_model_is_invoked_only_through_the_agent_runtime() {
    let sources = production_sources();
    assert_the_walk_sees_the_workspace(&sources);

    // `.complete(` with an argument, so `request.complete()` on an unrelated
    // acquisition request is not mistaken for a model call.
    let mut invoking: BTreeSet<&str> = BTreeSet::new();
    for (file, text) in &sources {
        let calls_structured = text.contains(".complete_structured(");
        let calls_plain = text
            .match_indices(".complete(")
            .any(|(at, needle)| !text[at + needle.len()..].starts_with(')'));
        if calls_structured || calls_plain {
            invoking.insert(file.as_str());
        }
    }
    let declared: BTreeSet<&str> = DECLARED_INVOCATIONS.into();
    assert_eq!(
        invoking, declared,
        "a model is invoked from somewhere other than the agent runtime and the \
         port's own chain. `AgentRuntime::complete` is the one call that checks the \
         agent's CallLanguageModel grant and charges its token budget first; a call \
         anywhere else does neither"
    );

    // And inside the runtime, the grant and the budget come before the call.
    let runtime = &sources["backend/crates/libs/qip-agents/src/runtime.rs"];
    let start = runtime
        .find("pub fn complete(")
        .unwrap_or_else(|| panic!("AgentRuntime::complete is gone from qip-agents"));
    let body = &runtime[start..];
    let at = |needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("AgentRuntime::complete no longer contains {needle}"))
    };
    let authorise = at("self.authorise(Capability::CallLanguageModel");
    let charge = at("self.ledger.charge_language_model(");
    let call = at(".complete_structured(");
    assert!(
        authorise < call && charge < call,
        "AgentRuntime::complete calls the model before it checks the agent's grant \
         or charges its budget"
    );
}

#[test]
fn the_hosted_adapter_is_constructed_only_in_a_composition_root() {
    let sources = production_sources();
    assert_the_walk_sees_the_workspace(&sources);

    let constructing: BTreeSet<&str> = files_containing(&sources, &["HuggingFaceModel::new("])
        .keys()
        .copied()
        .collect();
    let declared: BTreeSet<&str> = ["backend/crates/apps/qip-deepbrain/src/language.rs"].into();
    assert_eq!(
        constructing, declared,
        "the hosted adapter is built outside qip-deepbrain's composition root. A \
         service that constructs it holds a credential and a route the root never \
         handed it"
    );
}

#[test]
fn the_organisation_is_convened_only_after_the_cost_router_places_the_decision() {
    let sources = production_sources();
    let platform = &sources["backend/crates/runtime/qip-kernel/src/platform.rs"];
    assert!(
        platform.len() > 100_000,
        "premise: platform.rs was read as production source"
    );

    let dispatches: Vec<usize> = platform
        .match_indices("self.organisation.dispatch(")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        dispatches.len(),
        1,
        "the agent organisation — the only holder of a language model in the kernel — \
         is convened from {} places; it is convened once, from the REASON stage",
        dispatches.len()
    );
    // The REASON stage hands its queue to `reason_about_the_queue`, and that
    // is where the router is asked and the organisation convened.
    let function = |name: &str| {
        let start = platform
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("`{name}` is gone from platform.rs"));
        let end = platform[start + 1..]
            .find("\n    fn ")
            .map_or(platform.len(), |offset| start + 1 + offset);
        (start, end)
    };
    let (reason_start, reason_end) = function("stage_reason");
    assert!(
        platform[reason_start..reason_end].contains("self.reason_about_the_queue("),
        "the REASON stage no longer reasons through `reason_about_the_queue`"
    );
    let (stage, next) = function("reason_about_the_queue");
    let dispatch = dispatches[0];
    assert!(
        (stage..next).contains(&dispatch),
        "the organisation is convened outside `reason_about_the_queue`"
    );
    let select = platform[stage..next]
        .find("self.cost_router.select(")
        .map(|offset| stage + offset)
        .unwrap_or_else(|| {
            panic!("`reason_about_the_queue` no longer asks the cost router to place the decision")
        });
    assert!(
        select < dispatch,
        "`reason_about_the_queue` convenes the organisation before the cost router has placed the \
         decision, so a model can be reached without the router's rationale on record"
    );
}

/// Words that tie a type or a wire name to one cloud vendor's model or data
/// API. Whole words, case folded.
const VENDOR_API_WORDS: [&str; 7] = [
    "vertex",
    "aiplatform",
    "googleapis",
    "gemini",
    "google",
    "bigquery",
    "spanner",
];

fn vendor_api_words_in(text: &str) -> Vec<&'static str> {
    // CamelCase identifiers are split first: `VertexRequest` names the
    // vendor and a whole-word match on the folded text would miss it.
    let mut spaced = String::with_capacity(text.len() * 2);
    let mut previous = ' ';
    for c in text.chars() {
        if c.is_ascii_uppercase() && previous.is_ascii_lowercase() {
            spaced.push(' ');
        }
        spaced.push(c);
        previous = c;
    }
    let folded = spaced.to_ascii_lowercase();
    let words: BTreeSet<&str> = folded.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    VENDOR_API_WORDS
        .into_iter()
        .filter(|word| words.contains(word))
        .collect()
}

#[test]
fn the_contract_layer_names_no_cloud_vendor_model_or_data_api() {
    // R12-MODEL-10043: product contracts must not depend on Google branding
    // or API shape. `qip-contracts` is the shared vocabulary every crate
    // links, so a `VertexRequest` type or an `aiplatform` field there would
    // make every consumer's wire format a vendor's, and swapping the vendor
    // a rewrite of the contract rather than of one adapter. The endpoint
    // marker test above pins the adapter; this pins the layer beneath it.
    let root = qip_acceptance::repository_root();
    let mut scanned = 0usize;
    let mut offenders: Vec<(String, Vec<&'static str>)> = Vec::new();
    let mut files =
        qip_acceptance::files_with_extension("backend/crates/libs/qip-contracts/src", "rs");
    files.push(root.join("backend/crates/libs/qip-contracts/Cargo.toml"));
    for file in files {
        scanned += 1;
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
        let production = if file.extension().is_some_and(|e| e == "rs") {
            qip_acceptance::production_text(&text)
        } else {
            text
        };
        let found = vendor_api_words_in(&production);
        if !found.is_empty() {
            offenders.push((
                file.strip_prefix(&root)
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .into_owned(),
                found,
            ));
        }
    }
    assert!(scanned > 20, "only {scanned} files were scanned");
    assert_eq!(
        offenders,
        Vec::<(String, Vec<&'static str>)>::new(),
        "the contract layer names a cloud vendor's model or data API"
    );

    // The scan sees each word as a word, and a near miss is not one.
    assert_eq!(
        vendor_api_words_in("pub struct VertexRequest { aiplatform: String }"),
        vec!["vertex", "aiplatform"]
    );
    assert!(vendor_api_words_in("let vertexes = 1; // not a word match").is_empty());
}
