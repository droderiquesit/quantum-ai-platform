//! SEC-004: what a browser, and the console that serves it, can reach.
//!
//! The requirement is that no browser reaches key material, ledger internals
//! or an execution node, directly or through something that forwards to them.
//! Until this file the evidence was three tests that each held one edge —
//! the portal's two mounted secrets, the API's one invoker, the crate graph's
//! direct edges — and none that listed what the console's own code dials and
//! reads. That is the list the requirement's verification asks for, and a
//! property assembled from neighbours is one nobody notices losing: a route
//! handler that fetches Secret Manager, or a client component that imports
//! the module holding the platform credential, breaks none of the three.
//!
//! So this enumerates, from the console's source, every host it names, every
//! environment value and credential it reads, which files may import the
//! modules that hold a credential, and where browser-side code may `fetch`.
//! Each list is written here a second time on purpose, as the egress
//! allow-list is: a test that read the list out of the file it is checking
//! would agree with every widening.
//!
//! # The crate graph, and why linking is not reaching
//!
//! `api_boundary.rs` asserts `qip-api` has no *direct* edge to the capital,
//! execution or chain crates. It has them transitively, through `qip-kernel`,
//! and that is the design rather than a gap: `qip-api` is the kernel's
//! composition root, and the same process signs policy and halts with the
//! capital-envelope key. What bounds a browser is therefore not what the API
//! links but what it serves, to whom: the console is its only invoker
//! (`console_route.rs`), it presents the platform token and never the
//! browser's, it refuses a write its route table does not declare, and it
//! strips a cell's mesh address from the two bodies that carry one. The last
//! of those is asserted below; the others are held where they live.
//!
//! What this does not claim: that a deployed console behaves this way.
//! Nothing here runs the console, and the portal's Playwright suites, which
//! do, are not run by this crate.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read, repository_root};
use std::collections::{BTreeMap, BTreeSet};

/// Everything a browser can be served: the console and the shared packages
/// it bundles.
const BUNDLE_ROOTS: [&str; 2] = ["frontend/portal/src", "frontend/packages"];

/// Where the modules that may hold a credential live.
const SERVER_MODULES: &str = "frontend/portal/src/lib/server/";

/// Every host the console's code names, the one file that may name it, and
/// why it is there. The platform itself is not on this list because it is
/// not a literal: it is `QIP_API_BASE_URL`, held below.
const NAMED_HOSTS: [(&str, &str, &str); 5] = [
    (
        "metadata.google.internal",
        "frontend/portal/src/lib/server/google-credentials.ts",
        "the instance metadata server, for the identity token Cloud Run's IAM check wants",
    ),
    (
        "www.gstatic.com",
        "frontend/portal/src/lib/server/iap.ts",
        "Google's published IAP signing keys, to verify the assertion header",
    ),
    (
        "identitytoolkit.googleapis.com",
        "frontend/portal/src/lib/server/identity-platform.ts",
        "Identity Platform, for sign-in; unset in every environment",
    ),
    (
        "cloud.google.com",
        "frontend/portal/src/lib/server/iap.ts",
        "not dialled: the issuer string an IAP assertion must carry",
    ),
    (
        "127.0.0.1",
        "frontend/portal/src/lib/server/upstream.ts",
        "not dialled: the example in the refusal for an unset QIP_API_BASE_URL",
    ),
];

/// What a host or a variable must never be, by the requirement's own list: a
/// key service, a secret store, a ledger store, or a cell.
const FORBIDDEN: [&str; 14] = [
    "cloudkms",
    "secretmanager",
    "spanner",
    "bigquery",
    "bigtable",
    "alloydb",
    "storage.googleapis",
    "KMS",
    "ENVELOPE_KEY",
    "LEDGER",
    "SPANNER",
    "MESH",
    "VENUE",
    "CELL",
];

/// The two credentials the console holds, each read from a mounted file by
/// `secretFromEnvironment`. `gitops.rs` pins the portal's mounted secrets to
/// the same two.
const CREDENTIALS: [(&str, &str); 2] = [
    (
        "QIP_API_TOKEN",
        "the platform's bearer token, attached by the gateway and never sent to a browser",
    ),
    (
        "ALGORIK_SESSION_SECRET",
        "the key the session cookie is sealed with",
    ),
];

/// Every other environment value the console reads.
const SETTINGS: [(&str, &str); 11] = [
    ("QIP_API_BASE_URL", "destination: the platform's API"),
    (
        "ALGORIK_IAP_JWKS_URL",
        "destination: an override for the IAP key set",
    ),
    (
        "QIP_API_AUDIENCE",
        "the Cloud Run audience; a URL, not a secret",
    ),
    ("QIP_API_TIMEOUT_MS", "a timeout"),
    (
        "ALGORIK_IAP_AUDIENCE",
        "the audience an IAP assertion must name",
    ),
    (
        "ALGORIK_AUTH_REQUIRED",
        "the session gate; closed unless `false`",
    ),
    ("ALGORIK_COOKIE_SECURE", "the cookie's Secure attribute"),
    (
        "ALGORIK_IDENTITY_API_KEY",
        "Identity Platform's browser key; an identifier, set nowhere",
    ),
    ("ALGORIK_IDENTITY_PROJECT_ID", "Identity Platform's project"),
    (
        "ALGORIK_IDENTITY_STORE_DIR",
        "the development account store",
    ),
    ("NODE_ENV", "the runtime mode"),
];

/// The one value compiled into the browser bundle.
const BROWSER_VISIBLE: [&str; 1] = ["NEXT_PUBLIC_QIP_ENVIRONMENT"];

/// Files outside the route handlers and the server modules that import a
/// server module. Server components; a `"use client"` here fails below.
const SERVER_COMPONENTS: [&str; 1] = ["frontend/portal/src/app/(auth)/sign-in/page.tsx"];

/// Browser-side `fetch` calls whose argument is a variable, and the variable.
/// Each is built from a same-origin prefix, asserted where it is listed.
const WRAPPED_FETCHES: [(&str, &str); 2] = [
    ("frontend/portal/src/lib/api/client.ts", "url"),
    ("frontend/portal/src/app/(auth)/_lib/api.ts", "path"),
];

/// The console's shipped source, as path and text. Tests and specs are not
/// served to anyone.
fn bundle() -> Vec<(String, String)> {
    let root = repository_root();
    let mut files = Vec::new();
    for directory in BUNDLE_ROOTS {
        for extension in ["ts", "tsx"] {
            for path in files_with_extension(directory, extension) {
                let relative = path
                    .strip_prefix(&root)
                    .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());
                if relative.contains("node_modules")
                    || relative.contains(".test.")
                    || relative.contains(".spec.")
                    || relative.contains("/.next/")
                {
                    continue;
                }
                files.push((relative.clone(), read(&relative)));
            }
        }
    }
    files
}

/// Lines that are code. A comment naming a host is not a host the code dials.
fn code(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|line| {
        let line = line.trim_start();
        !(line.starts_with("//") || line.starts_with('*') || line.starts_with("/*"))
    })
}

/// The string a `const NAME = "…"` in `text` binds, exported or not.
fn constant(text: &str, name: &str) -> Option<String> {
    code(text).find_map(|line| {
        let line = line.trim_start().trim_start_matches("export ");
        let rest = line.strip_prefix(&format!("const {name} = \""))?;
        rest.split('"').next().map(str::to_string)
    })
}

fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Mutation (run 2026-10-04): bind a Secret Manager access URL
/// (`https://secretmanager.googleapis.com/v1/…:access`) to a constant in the
/// gateway route — the host is not on the list and the first assertion
/// names it and the file.
#[test]
fn the_console_names_no_host_beyond_the_five_listed_here_and_none_is_a_key_a_secret_store_or_a_ledger_store()
 {
    let files = bundle();
    assert!(
        files.len() > 100,
        "premise: only {} source files were read; the walk is not seeing the console",
        files.len()
    );

    let mut named: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (path, text) in &files {
        for line in code(text) {
            for scheme in ["https://", "http://"] {
                for (at, _) in line.match_indices(scheme) {
                    let host: String = line[at + scheme.len()..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                        .collect();
                    // `http://${host}` names nothing; what fills it is a
                    // variable, and those are held in the next test.
                    if !host.is_empty() {
                        named.entry(host).or_default().insert(path.clone());
                    }
                }
            }
        }
    }

    for (host, paths) in &named {
        let reviewed = NAMED_HOSTS.iter().find(|(name, _, _)| name == host);
        let Some((_, home, _)) = reviewed else {
            panic!(
                "the console names the host `{host}` in {paths:?}, and it is not one of the \
                 five this test lists. A destination is added here with the reason, or not at \
                 all (SEC-004)"
            );
        };
        assert_eq!(
            paths.iter().collect::<Vec<_>>(),
            vec![&(*home).to_string()],
            "`{host}` is named outside {home}; only a server module may name a destination"
        );
        for class in FORBIDDEN {
            assert!(
                !host.contains(class),
                "`{host}` is a `{class}` endpoint; a console that dials one can forward a \
                 browser to it"
            );
        }
    }
    for (host, home, _) in NAMED_HOSTS {
        assert!(
            named.contains_key(host),
            "premise: `{host}` was expected in {home} and was not read; either it is gone and \
             this list is stale, or the walk is not reading that file"
        );
        assert!(
            home.starts_with(SERVER_MODULES),
            "{home} is not a server module, so `{host}` is named in code a browser may receive"
        );
    }
}

/// Mutation (run 2026-10-04): read a third credential in the gateway route,
/// `secretFromEnvironment(ENVELOPE)` with `ENVELOPE` bound to
/// `"QIP_CAPITAL_ENVELOPE_KEY"` — the credential set is then three names and
/// the equality fails, printing all three. And: read
/// `process.env.NEXT_PUBLIC_QIP_API_TOKEN` in `EnvironmentBadge.tsx` —
/// refused as a value compiled into the bundle.
#[test]
fn the_console_reads_two_credentials_from_server_modules_and_compiles_none_into_the_browser_bundle()
{
    let mut read_by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut credentials: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (path, text) in bundle() {
        for line in code(&text) {
            for (at, _) in line.match_indices("process.env.") {
                let name: String = line[at + "process.env.".len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                read_by_name.entry(name).or_default().insert(path.clone());
            }
            for (at, _) in line.match_indices("process.env[") {
                let key = line[at + "process.env[".len()..]
                    .split(']')
                    .next()
                    .unwrap_or("");
                if is_identifier(key) {
                    let name = constant(&text, key).unwrap_or_else(|| {
                        panic!(
                            "{path}: `process.env[{key}]` reads a constant this file does not bind"
                        )
                    });
                    read_by_name.entry(name).or_default().insert(path.clone());
                } else {
                    // A computed name is the secret resolver's own lookup and
                    // nothing else's: anywhere else it is a variable this
                    // test cannot list.
                    assert_eq!(
                        path, "frontend/portal/src/lib/server/secret.ts",
                        "{path} reads `process.env[{key}]`, a name computed at run time"
                    );
                }
            }
            for (at, _) in line.match_indices("secretFromEnvironment(") {
                let argument = line[at + "secretFromEnvironment(".len()..]
                    .split(')')
                    .next()
                    .unwrap_or("");
                if argument.starts_with("variable") {
                    continue; // the resolver's own declaration
                }
                let name = constant(&text, argument).unwrap_or_else(|| {
                    panic!(
                        "{path}: `secretFromEnvironment({argument})` names no constant this \
                         file binds, so the credential it reads cannot be listed"
                    )
                });
                credentials.entry(name).or_default().insert(path.clone());
            }
        }
    }

    assert_eq!(
        credentials.keys().map(String::as_str).collect::<Vec<_>>(),
        {
            let mut expected: Vec<&str> = CREDENTIALS.iter().map(|(name, _)| *name).collect();
            expected.sort_unstable();
            expected
        },
        "the credentials the console reads are not the two this test lists"
    );
    for (name, paths) in &credentials {
        for path in paths {
            assert!(
                path.starts_with(SERVER_MODULES),
                "{path} reads the credential {name}; only a server module may, because \
                 nothing else is certain never to be bundled for a browser"
            );
        }
    }

    assert!(
        read_by_name.len() >= 8,
        "premise: only {} environment values were read as used; the walk is not reading the \
         console",
        read_by_name.len()
    );
    for (name, paths) in &read_by_name {
        if name.starts_with("NEXT_PUBLIC_") {
            assert!(
                BROWSER_VISIBLE.contains(&name.as_str()),
                "{name} is compiled into the browser bundle (read in {paths:?}); the only \
                 value a browser is given is the environment's name"
            );
            continue;
        }
        assert!(
            SETTINGS.iter().any(|(setting, _)| setting == name),
            "the console reads {name} in {paths:?}, which this test does not list. Say what \
             it is here before a console reads it"
        );
    }
    for name in read_by_name.keys().chain(credentials.keys()) {
        for class in FORBIDDEN {
            assert!(
                !name.contains(class),
                "the console reads {name}, a `{class}` value; a key, a ledger store or a \
                 cell's address is not the console's to hold"
            );
        }
    }
}

/// Mutation (run 2026-10-04): import `upstream` from `@/lib/server/upstream`
/// in the client component `AccountMenu.tsx` — refused as a client file
/// importing a server module. And: replace its sign-out call with
/// `fetch(target, {` — refused as a browser fetch that is not same-origin.
#[test]
fn no_browser_side_file_imports_a_server_module_or_fetches_anything_but_this_consoles_own_routes() {
    let files = bundle();
    let mut client_files = 0usize;
    let mut importers = 0usize;
    let mut browser_fetches = 0usize;

    for (path, text) in &files {
        let is_client = code(text)
            .map(str::trim)
            .find(|line| !line.is_empty())
            .is_some_and(|first| {
                first.starts_with("\"use client\"") || first.starts_with("'use client'")
            });
        client_files += usize::from(is_client);

        let imports_server = code(text).any(|line| {
            (line.contains("from \"") || line.trim_start().starts_with("import "))
                && (line.contains("@/lib/server/") || line.contains("lib/server/"))
        });
        let is_route =
            path.starts_with("frontend/portal/src/app/api/") && path.ends_with("/route.ts");
        let is_server_module = path.starts_with(SERVER_MODULES);

        if imports_server && !is_server_module {
            importers += 1;
            assert!(
                !is_client,
                "{path} is sent to the browser and imports a server module; the platform \
                 token and the session key live there"
            );
            assert!(
                is_route || SERVER_COMPONENTS.contains(&path.as_str()),
                "{path} imports a server module and is neither a route handler nor a server \
                 component this test lists"
            );
        }

        if is_route || is_server_module {
            continue;
        }
        for line in code(text) {
            for (at, _) in line.match_indices("fetch(") {
                // `refetch(`, `prefetch(` and a method named `fetch` are not
                // the global.
                let before = line[..at].chars().last();
                if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                    continue;
                }
                browser_fetches += 1;
                let argument = &line[at + "fetch(".len()..];
                let same_origin = argument.starts_with("\"/api/") || argument.starts_with("`/api/");
                let wrapped = WRAPPED_FETCHES.iter().any(|(file, variable)| {
                    file == path && argument.starts_with(&format!("{variable},"))
                });
                assert!(
                    same_origin || wrapped,
                    "{path} calls `fetch({argument}`: browser-side code may call this \
                     console's own `/api/` routes and nothing else"
                );
            }
        }
    }

    assert!(
        client_files >= 20 && importers >= 5 && browser_fetches >= 5,
        "premise: {client_files} client files, {importers} importers of a server module and \
         {browser_fetches} browser-side fetches were read; the walk is not reading the console"
    );
    for component in SERVER_COMPONENTS {
        assert!(
            files.iter().any(|(path, _)| path == component),
            "premise: {component} is gone, so this list names a file nobody has"
        );
    }

    // The two wrapped fetches are same-origin by what builds their argument.
    let client = read("frontend/portal/src/lib/api/client.ts");
    assert!(
        client.contains("export const GATEWAY_PREFIX = \"/api/gateway\";")
            && client.contains("const url = `${GATEWAY_PREFIX}${path}${search}`;"),
        "the API client no longer builds its URL from the gateway's own prefix"
    );
    for (path, text) in &files {
        if !path.starts_with("frontend/portal/src/app/(auth)/") {
            continue;
        }
        for line in code(text) {
            for (at, _) in line.match_indices("\"/api/") {
                assert!(
                    line[at..].starts_with("\"/api/auth/"),
                    "{path} names a route outside `/api/auth/`: {}",
                    line.trim()
                );
            }
        }
    }
}

/// Mutation (run 2026-10-04): add `fetch("/api/gateway/mesh")` to the public
/// site's `lib/site.js` — refused, because the public site calls nothing.
#[test]
fn the_public_site_fetches_nothing_and_reads_one_public_url_and_no_credential() {
    // The other thing a browser runs. It sits in front of the console's
    // sign-in and has no session, so anything it could call or read is
    // something every visitor can.
    let root = repository_root();
    let mut files = 0usize;
    let mut variables: BTreeSet<String> = BTreeSet::new();
    let mut hosts: BTreeSet<String> = BTreeSet::new();
    for directory in [
        "frontend/landing/app",
        "frontend/landing/components",
        "frontend/landing/lib",
    ] {
        for extension in ["js", "jsx", "ts", "tsx"] {
            for path in files_with_extension(directory, extension) {
                let relative = path
                    .strip_prefix(&root)
                    .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());
                files += 1;
                let text = read(&relative);
                for line in code(&text) {
                    for (at, _) in line.match_indices("fetch(") {
                        let before = line[..at].chars().last();
                        assert!(
                            before
                                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.'),
                            "{relative} calls fetch; the public site has no session and \
                             nothing to call: {}",
                            line.trim()
                        );
                    }
                    for (at, _) in line.match_indices("process.env.") {
                        variables.insert(
                            line[at + "process.env.".len()..]
                                .chars()
                                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                                .collect(),
                        );
                    }
                    for scheme in ["https://", "http://"] {
                        for (at, _) in line.match_indices(scheme) {
                            let host: String = line[at + scheme.len()..]
                                .chars()
                                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                                .collect();
                            if !host.is_empty() && host != "www.w3.org" {
                                hosts.insert(host);
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        files > 20,
        "premise: only {files} source files were read; the walk is not seeing the public site"
    );
    assert_eq!(
        variables.iter().map(String::as_str).collect::<Vec<_>>(),
        ["NEXT_PUBLIC_ALGORIK_PORTAL_URL"],
        "the public site reads something other than the console's public URL"
    );
    assert_eq!(
        hosts.iter().map(String::as_str).collect::<Vec<_>>(),
        ["127.0.0.1", "www.youtube-nocookie.com"],
        "the public site names a host other than the local default for the console's URL and \
         the one video embed"
    );
}

/// Mutation (run 2026-10-04): delete the `/mesh` entry from
/// `WIRE_REDACTIONS` — a cell's mesh address then crosses the gateway to the
/// browser, and the first assertion fails.
#[test]
fn the_gateway_strips_a_cells_address_from_both_bodies_that_carry_one_and_the_stream_carries_none()
{
    // The one fact the API serves that *is* an execution node's address:
    // `cells[].address`, at viewer role, on two routes.
    let redaction = read("frontend/portal/src/lib/api/redaction.ts");
    let table = redaction
        .split("export const WIRE_REDACTIONS")
        .nth(1)
        .and_then(|rest| rest.split("\n];").next())
        .expect("the redaction table is declared where this test reads");
    for route in ["/mesh", "/system/status"] {
        let entry = table
            .split("\n  {")
            .find(|entry| entry.contains(&format!("route: \"{route}\",")))
            .unwrap_or_else(|| {
                panic!(
                    "the gateway no longer redacts anything on {route}; that body carries \
                     every cell's mesh address"
                )
            });
        assert!(
            entry.contains("\"address\"]"),
            "the {route} redaction no longer ends at `address`"
        );
    }
    let gateway = read("frontend/portal/src/app/api/gateway/[...path]/route.ts");
    assert!(
        gateway.contains("redactBody("),
        "the gateway no longer applies the redaction table to what it forwards"
    );
    // The stream route applies no redaction, which is right only while no
    // frame carries an address.
    assert!(
        !read("backend/crates/apps/qip-api/src/stream.rs").contains("address"),
        "an SSE frame now names an address, and the stream route forwards frames unredacted"
    );
    // And the API serves the field at all only from the mesh status.
    assert!(
        read("backend/crates/apps/qip-api/src/mesh.rs").contains("address"),
        "premise: the mesh status is where a cell's address is served from"
    );
}
