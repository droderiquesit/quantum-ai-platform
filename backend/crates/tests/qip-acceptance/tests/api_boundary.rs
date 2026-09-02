//! The application layer's boundary, made executable.
//!
//! The blueprint's application services (§40.9) are interface services around
//! the domains, not replacements for them: they compose reads, raise typed
//! intents, hold no financial state, and never issue an order, sign a
//! transfer, bypass a gate or reach a node. In this workspace that layer is
//! `qip-api` and `qip-web`, and every property below is one a reviewer would
//! otherwise have to take on faith from a manifest and a route table.
//!
//! Each test asserts an *absence* — an edge, a constructor, a field type, a
//! mutator — so each asserts its premise first: that the graph was read, that
//! the enumeration it searches for is non-empty, that the scanner saw the
//! code it claims to have scanned. A boundary test whose search space is
//! silently empty is green over a live violation, which is worse than no test.
//!
//! Where the boundary is a *pinned* set rather than an absence — the stores
//! the API holds, the routes that mutate, the two things the API signs — the
//! set is written out here so that adding to it is a reviewed change with the
//! reason beside it, rather than a line in a file nobody re-reads.
//!
//! The dependency graph comes from `cargo metadata`, for the reasons
//! `architecture.rs` records at length: four rounds of a hand-written manifest
//! reader each missed an edge Cargo saw. Development dependencies are
//! excluded there and here — `qip-api`'s tests link `qip-edge` to decode a
//! genuine cell delta, and what a crate's tests link is not what its shipped
//! code can call.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, repository_root};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The two crates that make up the application layer.
const APPLICATION_CRATES: [&str; 2] = ["qip-api", "qip-web"];

/// The crates the application layer must not reach.
///
/// Each one either constructs orders, adapts a venue, signs capital, or is the
/// regional cell itself. `qip-chain` is the JSON-RPC chain adapter — the
/// "node" the blueprint says a portal never reaches. `qip-capital` is the
/// brain that signs a grant; the API dispatches grants the platform already
/// signed and must not be able to mint one.
const FORBIDDEN_CRATES: [&str; 10] = [
    "qip-execution-engine",
    "qip-brokers",
    "qip-chain",
    "qip-capital",
    "qip-capital-fabric",
    "qip-edge",
    "qip-edge-node",
    "qip-routing",
    "qip-orderbook",
    "qip-sequencing",
];

// --- the graph, from cargo metadata ------------------------------------------

/// Run `cargo metadata` against the backend workspace, or fail loudly.
///
/// Every assertion built on this graph is that an edge is *absent*, so a
/// silent error here is a green suite over a live violation. A missing binary,
/// a non-zero exit and unparseable output all panic rather than yielding an
/// empty graph.
fn workspace_metadata() -> serde_json::Value {
    let manifest = repository_root().join("backend/Cargo.toml");
    assert!(
        manifest.is_file(),
        "no workspace manifest at {}; this test cannot see the workspace",
        manifest.display()
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(&cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .unwrap_or_else(|error| panic!("could not run `{cargo} metadata`: {error}"));
    assert!(
        output.status.success(),
        "`{cargo} metadata` exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("`{cargo} metadata` produced JSON this test cannot parse: {error}")
    })
}

/// Shipped in-workspace edges, by crate. Dev and build dependencies excluded.
fn dependency_graph() -> BTreeMap<String, BTreeSet<String>> {
    let metadata = workspace_metadata();
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .expect("cargo metadata always reports a `packages` array");
    let members: BTreeSet<String> = packages.iter().map(package_name).collect();
    let mut graph = BTreeMap::new();
    for package in packages {
        let mut edges = BTreeSet::new();
        let dependencies = package
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        for dependency in &dependencies {
            let kind = dependency.get("kind").and_then(serde_json::Value::as_str);
            if matches!(kind, Some("dev") | Some("build")) {
                continue;
            }
            let name = dependency
                .get("name")
                .and_then(serde_json::Value::as_str)
                .expect("every dependency has a name")
                .to_string();
            if members.contains(&name) {
                edges.insert(name);
            }
        }
        graph.insert(package_name(package), edges);
    }
    assert!(
        graph.len() > 25,
        "cargo metadata reported only {} workspace members; the graph is not being read",
        graph.len()
    );
    graph
}

fn package_name(package: &serde_json::Value) -> String {
    package
        .get("name")
        .and_then(serde_json::Value::as_str)
        .expect("every package has a name")
        .to_string()
}

// --- the sources, shipped code only ------------------------------------------

/// Every `.rs` file under a crate's `src/`, with its text cut at the first
/// `#[cfg(test)]`.
///
/// Only shipped code is scanned. `qip-api`'s in-file tests build genuine
/// envelopes and orders to prove the decode against, and a scan that read
/// them would refuse the very tests that keep the composition root honest.
/// Every file in these two crates keeps its test module at the tail, which
/// the cfg count in the premise below checks.
fn shipped_sources(crate_dir: &str) -> Vec<(PathBuf, String)> {
    let files = files_with_extension(&format!("backend/crates/apps/{crate_dir}/src"), "rs");
    assert!(
        !files.is_empty(),
        "no sources under backend/crates/apps/{crate_dir}/src; the scan has nothing to read"
    );
    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            assert!(
                text.matches("#[cfg(test)]").count() <= 1,
                "{} has more than one `#[cfg(test)]`; the shipped-code cut assumes one test \
                 module at the tail",
                path.display()
            );
            let shipped = match text.find("#[cfg(test)]") {
                Some(cut) => text[..cut].to_string(),
                None => text,
            };
            (path, shipped)
        })
        .collect()
}

fn application_sources() -> Vec<(PathBuf, String)> {
    let mut sources = Vec::new();
    for crate_name in APPLICATION_CRATES {
        sources.extend(shipped_sources(crate_name));
    }
    sources
}

fn is_identifier_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Every identifier token in `text`, with the byte offset just past it.
///
/// A token scanner rather than `contains`: `Order` is a substring of
/// `OrderRow` and of `DeltaOrder`, and a substring match would refuse the
/// view struct the interface renders orders *through* while a whole-token
/// match refuses exactly the constructor.
fn identifiers(text: &str) -> Vec<(&str, usize)> {
    let mut found = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let c = bytes[index] as char;
        if is_identifier_char(c) {
            let start = index;
            while index < bytes.len() && is_identifier_char(bytes[index] as char) {
                index += 1;
            }
            found.push((&text[start..index], index));
        } else {
            index += 1;
        }
    }
    found
}

/// Whether the text after an identifier reads as a constructor: a `::` path
/// (`Order::new`, `CapitalEnvelope::new`) or a struct literal (`Order {`).
fn continues_as_construction(text: &str, after: usize) -> bool {
    let rest = text[after..].trim_start();
    rest.starts_with("::") || rest.starts_with('{')
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// `pub struct`/`pub enum` names under a crate's `src/` whose name ends in
/// one of `suffixes`, or equals one of `exact`.
fn public_type_names(crate_dir: &str, suffixes: &[&str], exact: &[&str]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for path in files_with_extension(&format!("backend/crates/{crate_dir}/src"), "rs") {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        for keyword in ["pub struct ", "pub enum "] {
            for (index, _) in text.match_indices(keyword) {
                let rest = &text[index + keyword.len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| is_identifier_char(*c))
                    .collect();
                if name.is_empty() {
                    continue;
                }
                if suffixes.iter().any(|suffix| name.ends_with(suffix))
                    || exact.contains(&name.as_str())
                {
                    names.insert(name);
                }
            }
        }
    }
    names
}

// --- (a) the graph ----------------------------------------------------------

#[test]
fn the_application_layer_depends_on_no_execution_venue_capital_or_edge_crate() {
    let graph = dependency_graph();

    // Premise: every forbidden crate is a real workspace member. A renamed
    // crate would otherwise turn this into a search for names nothing has.
    for forbidden in FORBIDDEN_CRATES {
        assert!(
            graph.contains_key(forbidden),
            "{forbidden} is not a workspace member; the forbidden list has drifted from the tree"
        );
    }
    // Premise: the API's edges were read. It composes the kernel, so a graph
    // that shows it with no edges is a graph that was not read.
    let api = graph.get("qip-api").expect("qip-api is a workspace member");
    assert!(
        api.contains("qip-kernel"),
        "qip-api's edges were not read: {api:?}"
    );
    let web = graph.get("qip-web").expect("qip-web is a workspace member");

    for (crate_name, edges) in [("qip-api", api), ("qip-web", web)] {
        let reached: Vec<&str> = FORBIDDEN_CRATES
            .iter()
            .copied()
            .filter(|forbidden| edges.contains(*forbidden))
            .collect();
        assert!(
            reached.is_empty(),
            "{crate_name} ships with an edge to {reached:?}; the application layer composes \
             reads and raises intents, and a crate that can name an order constructor or a \
             venue adapter is no longer that layer"
        );
    }

    // The interface renders what it is handed. It has no in-workspace edge at
    // all, which is what makes "the browser holds no trading logic" a fact
    // about the build rather than a discipline.
    assert!(
        web.is_empty(),
        "qip-web reaches {web:?}; the interface is meant to depend on nothing in the workspace"
    );
}

#[test]
fn the_api_uses_only_the_centre_half_of_the_mesh_and_none_of_its_service_clients() {
    // Premise: the mesh really exports managed-service clients to refuse.
    let clients = public_type_names(
        "services/qip-mesh",
        &[],
        &["MeshProvider", "MeshTarget", "MeshPort"],
    );
    assert_eq!(
        clients.len(),
        3,
        "qip-mesh's provider types have moved; the refusal list no longer names them: {clients:?}"
    );

    let mut modules_named = BTreeSet::new();
    let mut offenders = Vec::new();
    for (path, text) in shipped_sources("qip-api") {
        for (index, _) in text.match_indices("qip_mesh::") {
            let rest = &text[index + "qip_mesh::".len()..];
            let module: String = rest
                .chars()
                .take_while(|c| is_identifier_char(*c))
                .collect();
            modules_named.insert(module);
        }
        for (identifier, after) in identifiers(&text) {
            if clients.contains(identifier)
                || identifier == "adapters" && text[after..].starts_with("::")
            {
                offenders.push(format!(
                    "{}:{} names {identifier}",
                    path.display(),
                    line_of(&text, after)
                ));
            }
        }
    }
    // Premise: the API does use the mesh, so an empty module set would mean
    // the import scan missed it.
    assert!(
        modules_named.contains("spine") && modules_named.contains("delta"),
        "qip-api's mesh imports were not seen: {modules_named:?}"
    );
    assert!(
        offenders.is_empty(),
        "the API reaches a mesh service client; the blueprint's portal never reaches a node:\n{}",
        offenders.join("\n")
    );
    let unexpected: Vec<&String> = modules_named
        .iter()
        .filter(|module| !["spine", "delta"].contains(&module.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "qip-api names mesh modules beyond the centre's spine and the delta decode: {unexpected:?}"
    );
}

// --- (b) constructors and signatures ----------------------------------------

/// Order, transfer and envelope types, enumerated from the crates that define
/// them rather than guessed.
fn order_transfer_and_envelope_types() -> BTreeSet<String> {
    let defining = [
        "services/qip-execution-engine",
        "services/qip-brokers",
        "services/qip-risk-engine",
        "services/qip-chain",
        "services/qip-capital",
        "services/qip-capital-fabric",
        "services/qip-mesh",
        "services/qip-simulation-engine",
        "edge/qip-edge",
        "edge/qip-routing",
        "edge/qip-orderbook",
        "libs/qip-contracts",
    ];
    let mut names = BTreeSet::new();
    for crate_dir in defining {
        names.extend(public_type_names(
            crate_dir,
            &["Order", "Transfer", "Envelope"],
            &["TransferId"],
        ));
    }
    names
}

#[test]
fn the_application_layer_constructs_no_order_transfer_or_envelope() {
    let types = order_transfer_and_envelope_types();
    // Premise: the enumeration found the types this test exists to refuse.
    // If the execution engine's `Order`, the chain adapter's `BridgeTransfer`
    // or the contract layer's `CapitalEnvelope` is missing, the scan is
    // looking for the wrong thing.
    for expected in [
        "Order",
        "VenueOrder",
        "BridgeTransfer",
        "CapitalEnvelope",
        "ProposedOrder",
    ] {
        assert!(
            types.contains(expected),
            "{expected} was not enumerated from its defining crate; found {types:?}"
        );
    }

    let sources = application_sources();
    let mut mentions = 0usize;
    let mut offenders = Vec::new();
    for (path, text) in &sources {
        for (identifier, after) in identifiers(text) {
            if !types.contains(identifier) {
                continue;
            }
            mentions += 1;
            if continues_as_construction(text, after) {
                offenders.push(format!(
                    "{}:{} constructs {identifier}",
                    path.display(),
                    line_of(text, after)
                ));
            }
        }
    }
    // Premise: the scanner saw the code. The API holds a `CapitalEnvelope`
    // *by value* on its way down the mesh (a snapshot of one the platform
    // signed), so the type is mentioned even though nothing constructs it.
    assert!(
        mentions > 0,
        "no order, transfer or envelope type is mentioned anywhere in the application layer; \
         the token scan is not seeing the sources"
    );
    assert!(
        offenders.is_empty(),
        "the application layer constructs what it may only read through:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_application_layer_signs_nothing_but_the_centres_policy_and_halt() {
    // Premise: the signing operations exist where this test believes they do.
    let signing_sites = [
        ("libs/qip-compliance/src/signing.rs", "pub fn from_secret("),
        ("libs/qip-compliance/src/signing.rs", "pub fn sign("),
        ("edge/qip-edge/src/envelope.rs", "pub fn sign_payload("),
        ("libs/qip-contracts/src/policy.rs", "pub fn signed("),
        (
            "libs/qip-contracts/src/capital.rs",
            "pub fn signing_payload(",
        ),
    ];
    for (relative, declaration) in signing_sites {
        let path = repository_root().join("backend/crates").join(relative);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        assert!(
            text.contains(declaration),
            "{relative} no longer declares `{declaration}`; the refusal list has drifted"
        );
    }

    // Anything that could produce a capital signature, or verify one under a
    // key the API chose: the key type, the edge's HMAC, the envelope's
    // signing bytes, and a `.sign(` on anything.
    let forbidden_identifiers = [
        "SigningKey",
        "sign_payload",
        "from_secret",
        "signing_payload",
    ];
    let mut offenders = Vec::new();
    let mut signed_calls = Vec::new();
    for (path, text) in application_sources() {
        for (identifier, after) in identifiers(&text) {
            let line = line_of(&text, after);
            if forbidden_identifiers.contains(&identifier) {
                offenders.push(format!("{}:{line} names {identifier}", path.display()));
            }
            let preceded_by_dot = text[..after - identifier.len()].ends_with('.');
            let called = text[after..].starts_with('(');
            if identifier == "sign" && preceded_by_dot && called {
                offenders.push(format!("{}:{line} calls .sign(", path.display()));
            }
            if identifier == "signed" && preceded_by_dot && called {
                let line_text = text
                    .lines()
                    .nth(line - 1)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                signed_calls.push((path.clone(), line, line_text));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the application layer holds or uses signing material it must never hold:\n{}",
        offenders.join("\n")
    );

    // The two signatures the API *does* make, pinned by their exact
    // expression. The centre signs its policy payload and its halt command
    // with the trust root, because a cell refuses an unsigned one and a halt
    // that anyone could forge is a denial-of-service lever. Neither is a
    // transfer, a grant or an order. A third `.signed(` anywhere in the
    // application layer fails here until it is named below with its reason.
    let permitted = [
        "let signed = match payload.signed(&key) {",
        "let command = match HaltCommand::new(cell.clone(), now, reason).signed(&key) {",
    ];
    let observed: Vec<String> = signed_calls
        .iter()
        .map(|(_, _, line)| line.clone())
        .collect();
    assert_eq!(
        observed.len(),
        permitted.len(),
        "the application layer signs {} things and this test knows {}: {signed_calls:?}",
        observed.len(),
        permitted.len()
    );
    for (path, line, text) in &signed_calls {
        assert!(
            permitted.contains(&text.as_str()) && path.ends_with(Path::new("qip-api/src/mesh.rs")),
            "{}:{line} signs something this test has not reviewed: `{text}`",
            path.display()
        );
    }
}

// --- (c) financial state ----------------------------------------------------

/// Struct field declarations in shipped code: `name: Type,` lines inside a
/// `struct` body.
///
/// Only struct bodies are read. A struct *literal* has the same `name: value,`
/// shape — the API builds a contract-typed reconciliation break with
/// `Decimal::ZERO` on its way into the platform — and reading those would
/// report a value the API hands over as a field it keeps.
fn field_declarations(text: &str) -> Vec<(usize, String, String)> {
    let mut fields = Vec::new();
    let mut body_indent: Option<usize> = None;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let indent = raw.len() - raw.trim_start().len();
        match body_indent {
            None => {
                let opens = ["pub struct ", "pub(crate) struct ", "struct "]
                    .iter()
                    .any(|keyword| line.starts_with(keyword));
                if opens && line.ends_with('{') {
                    body_indent = Some(indent);
                }
            }
            Some(open_indent) => {
                if line == "}" && indent == open_indent {
                    body_indent = None;
                    continue;
                }
                if !line.ends_with(',') || line.starts_with("//") || line.starts_with('#') {
                    continue;
                }
                let Some((name, ty)) = line[..line.len() - 1].split_once(':') else {
                    continue;
                };
                let name = name
                    .trim()
                    .trim_start_matches("pub(crate) ")
                    .trim_start_matches("pub ");
                if name.is_empty() || !name.chars().all(is_identifier_char) {
                    continue;
                }
                fields.push((index + 1, name.to_string(), ty.trim().to_string()));
            }
        }
    }
    fields
}

#[test]
fn the_interface_holds_money_as_text_it_was_handed_and_never_as_a_number_it_owns() {
    // Premise: the view structs carry money-named fields, and they are the
    // rendered string the platform produced, not a quantity the interface
    // could add to.
    let sources = shipped_sources("qip-web");
    let mut money_fields = Vec::new();
    let mut decimal_mentions = Vec::new();
    for (path, text) in &sources {
        for (line, name, ty) in field_declarations(text) {
            if [
                "equity", "gross", "net", "granted", "used", "cost", "price", "quantity",
            ]
            .contains(&name.as_str())
            {
                money_fields.push((path.clone(), line, name, ty));
            }
        }
        for (identifier, after) in identifiers(text) {
            if identifier == "Decimal" {
                decimal_mentions.push(format!("{}:{}", path.display(), line_of(text, after)));
            }
        }
    }
    assert!(
        money_fields.len() >= 6,
        "the interface's money-named fields were not seen: {money_fields:?}"
    );
    let numeric: Vec<_> = money_fields
        .iter()
        .filter(|(_, _, _, ty)| ty != "String")
        .collect();
    assert!(
        numeric.is_empty(),
        "the interface holds money as a number rather than as the text it was handed; a view \
         that can add is a view that can drift from the platform's own figure: {numeric:?}"
    );
    assert!(
        decimal_mentions.is_empty(),
        "qip-web names Decimal at {decimal_mentions:?}; the interface has no workspace edge and \
         no business holding a money type"
    );
}

#[test]
fn the_api_owns_no_field_typed_as_money_and_every_store_it_holds_is_named_here() {
    let sources = shipped_sources("qip-api");

    // Premise: shipped API code does name `Decimal` — the console formats the
    // platform's figures through it — so a scan that finds no mention is not
    // reading the code.
    let mentions = sources
        .iter()
        .flat_map(|(_, text)| identifiers(text))
        .filter(|(identifier, _)| *identifier == "Decimal")
        .count();
    assert!(
        mentions > 0,
        "qip-api's shipped code never names Decimal; the scan is blind"
    );

    let mut fields = 0usize;
    let mut saw_envelope_field = false;
    let mut money_fields = Vec::new();
    for (path, text) in &sources {
        for (line, name, ty) in field_declarations(text) {
            fields += 1;
            if name == "envelope" && ty == "CapitalEnvelope" {
                saw_envelope_field = true;
            }
            if identifiers(&ty)
                .iter()
                .any(|(identifier, _)| *identifier == "Decimal")
            {
                money_fields.push(format!("{}:{line} {name}: {ty}", path.display()));
            }
        }
    }
    // Premise: the field scanner sees real declarations, including the one
    // envelope the API carries by value on its way down the mesh.
    assert!(
        fields > 50,
        "only {fields} field declarations were seen in qip-api"
    );
    assert!(
        saw_envelope_field,
        "the field scanner did not see `PendingGrant::envelope`; it is not reading struct bodies"
    );
    assert!(
        money_fields.is_empty(),
        "the API owns a field typed as money; every figure it serves must be read from the \
         platform at request time, not kept:\n{}",
        money_fields.join("\n")
    );

    // Every interior-mutable store the API holds, with why each is not
    // financial state:
    //
    // * `Platform` — the kernel's, shared under one lock; the API reads it and
    //   raises the three intents below into it. It is not the API's state.
    // * `Vec<StageRow>` — the last cycle's stages for the overview page.
    // * `UnrecognisedAttempts`, `BTreeMap<String, (Timestamp, u32)>` — the
    //   authenticator's lockout and rate-limit counters.
    // * `BTreeMap<String, CellObservation>` — when each cell last reported and
    //   how many positions it said it had: counts and a timestamp, so the
    //   console can say a book is stale.
    // * `crate::mesh::MeshBackbone` — the spine's lanes: grants the platform
    //   signed, spooled until a cell acknowledges them. In transit, not owned.
    // * `Option<HealthReading>` — the last health pulse for the stream.
    //
    // A new store is a reviewed change: name it here with its reason.
    let known_stores: BTreeSet<&str> = [
        "Platform",
        "Vec<StageRow>",
        "UnrecognisedAttempts",
        "BTreeMap<String, (Timestamp, u32)>",
        "BTreeMap<String, CellObservation>",
        "crate::mesh::MeshBackbone",
        "Option<HealthReading>",
    ]
    .into_iter()
    .collect();
    let mut stores = BTreeSet::new();
    for (_, text) in &sources {
        for (index, _) in text.match_indices("Mutex<") {
            let rest = &text[index + "Mutex<".len()..];
            let mut depth = 1usize;
            let mut end = 0usize;
            for (offset, c) in rest.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            end = offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            assert!(end > 0, "unbalanced `Mutex<` in qip-api source");
            stores.insert(rest[..end].to_string());
        }
    }
    assert!(
        stores.contains("Platform"),
        "the platform lock was not seen; the store scan is not reading the sources"
    );
    let unknown: Vec<&String> = stores
        .iter()
        .filter(|store| !known_stores.contains(store.as_str()))
        .collect();
    assert!(
        unknown.is_empty(),
        "the API holds a store this test has not reviewed: {unknown:?}. If it is not financial \
         state, name it in `known_stores` with the reason; if it is, it does not belong here"
    );
}

// --- (d) mutating routes ----------------------------------------------------

/// `(method, pattern)` for every row of the API's route table.
fn route_table() -> Vec<(String, String)> {
    let path = repository_root().join("backend/crates/apps/qip-api/src/routes.rs");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let mut routes = Vec::new();
    let mut method: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("method: Method::") {
            method = Some(rest.trim_end_matches(',').to_string());
        } else if let Some(rest) = line.strip_prefix("pattern: ") {
            if let Some(method) = method.take() {
                routes.push((
                    method,
                    rest.trim_end_matches(',').trim_matches('"').to_string(),
                ));
            }
        }
    }
    routes
}

#[test]
fn every_mutating_route_is_one_of_three_and_each_raises_a_typed_intent() {
    let routes = route_table();
    assert!(
        routes.len() > 20,
        "only {} routes were parsed from the route table; the parser is not reading it",
        routes.len()
    );
    let mutating: BTreeSet<(String, String)> = routes
        .into_iter()
        .filter(|(method, _)| method != "Get" && method != "Head")
        .collect();
    let expected: BTreeSet<(String, String)> = [
        ("Post", "/cycle"),
        ("Post", "/kill-switch"),
        ("Delete", "/kill-switch"),
    ]
    .into_iter()
    .map(|(method, pattern)| (method.to_string(), pattern.to_string()))
    .collect();
    assert_eq!(
        mutating, expected,
        "the set of routes that can change the platform has moved; a new one is a reviewed \
         change that must name the typed intent it raises"
    );

    // The console's one form posts to a single path and trips the same switch.
    let console = repository_root().join("backend/crates/apps/qip-api/src/console.rs");
    let console_text = std::fs::read_to_string(&console).expect("console.rs is readable");
    assert!(
        console_text.contains("Method::Post if request.path == TRIP_PATH => self.trip("),
        "the console's POST no longer routes only to `trip`"
    );

    // What each intent is. The cycle is the platform's own loop; the two
    // kill-switch routes and the console's trip go through the switch, and
    // clearing carries the verified operator identity the switch demands.
    let routes_text = std::fs::read_to_string(
        repository_root().join("backend/crates/apps/qip-api/src/routes.rs"),
    )
    .expect("routes.rs is readable");
    assert!(routes_text.contains("let report = platform.run_cycle(now);"));
    assert!(
        routes_text.contains("OperatorIdentity::verified(")
            && routes_text.contains(".clear_global(&operator, now)"),
        "clearing the kill switch no longer carries a verified operator identity"
    );
}

#[test]
fn the_api_calls_no_platform_mutator_beyond_the_four_it_is_allowed() {
    // Enumerate every `&mut self` method the platform exposes, from the
    // kernel's source rather than from memory, so a mutator added to the
    // kernel tomorrow is refused here the day it is called from a route.
    let platform = repository_root().join("backend/crates/runtime/qip-kernel/src/platform.rs");
    let text = std::fs::read_to_string(&platform).expect("platform.rs is readable");
    let shipped = match text.find("#[cfg(test)]") {
        Some(cut) => &text[..cut],
        None => text.as_str(),
    };
    let mut mutators = BTreeSet::new();
    for (index, _) in shipped.match_indices("pub fn ") {
        let rest = &shipped[index + "pub fn ".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| is_identifier_char(*c))
            .collect();
        let Some(close) = rest.find(')') else {
            continue;
        };
        if rest[..close].contains("&mut self") {
            mutators.insert(name);
        }
    }
    // Premise: the enumeration found the mutators that would breach the
    // boundary if a route called them.
    for expected in ["submit_order", "run_cycle", "autonomy_mut", "observe"] {
        assert!(
            mutators.contains(expected),
            "Platform::{expected} was not enumerated; found {mutators:?}"
        );
    }

    // What the API may call on the platform, and why:
    // * `run_cycle` — `POST /cycle` runs the loop; it is the loop's own gate.
    // * `autonomy_mut` — only to reach the kill switch, checked below.
    // * `set_central` — installs the trust root once, at start-up.
    // * `ingest_cell_report` — hands a cell's delta to the platform to judge.
    let allowed: BTreeSet<&str> = [
        "run_cycle",
        "autonomy_mut",
        "set_central",
        "ingest_cell_report",
    ]
    .into_iter()
    .collect();

    let mut saw_run_cycle = false;
    let mut offenders = Vec::new();
    let mut autonomy_sites = 0usize;
    for (path, source) in shipped_sources("qip-api") {
        // Whitespace removed so a chained call broken across lines reads the
        // same as one on a single line.
        let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
        for mutator in &mutators {
            let call = format!("platform.{mutator}(");
            if compact.contains(&call) {
                if mutator == "run_cycle" {
                    saw_run_cycle = true;
                }
                if !allowed.contains(mutator.as_str()) {
                    offenders.push(format!("{} calls Platform::{mutator}", path.display()));
                }
            }
        }
        // Reaching the controller is permitted only to reach the switch or to
        // raise the typed change request; anything else on it — a level set
        // without the operator identity — is a mutation the API must not make.
        for (index, _) in compact.match_indices("autonomy_mut()") {
            autonomy_sites += 1;
            let rest = &compact[index + "autonomy_mut()".len()..];
            assert!(
                rest.starts_with(".kill_switch_mut()") || rest.starts_with(".request_change("),
                "{} reaches the autonomy controller for something other than the kill switch or \
                 a change request: `{}`",
                path.display(),
                &rest[..rest.len().min(40)]
            );
        }
    }
    assert!(
        saw_run_cycle,
        "`platform.run_cycle(` was not seen; the call scan is blind"
    );
    assert!(
        autonomy_sites >= 3,
        "only {autonomy_sites} autonomy_mut sites were seen; the console trip and the two \
         kill-switch routes should account for three"
    );
    assert!(
        offenders.is_empty(),
        "the API mutates the platform directly rather than raising an intent:\n{}",
        offenders.join("\n")
    );
}
