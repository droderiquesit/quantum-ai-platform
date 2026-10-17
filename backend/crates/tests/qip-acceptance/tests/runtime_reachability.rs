//! Requirements-to-runtime traceability gate.
//!
//! A completion assessment found hundreds of requirements marked implemented
//! whose code no binary could reach: tested in isolation, called by nothing
//! that ships. "Implemented" and "running" had drifted apart and nothing in the
//! build said so. This suite makes the gap a build failure at two grains.
//!
//! **Part A, crate level (hard gate).** Every non-test crate must be in the
//! transitive `[dependencies]` closure of some crate under `crates/apps/`. A
//! crate outside it cannot execute in any deployed process whatever its tests
//! say. The only exemptions are named in `NOT_A_RUNTIME_CRATE`, each with a
//! reason; an exemption that has become reachable fails too, so the list can
//! only shrink.
//!
//! **Part B, symbol level (ratchet, best effort).** A crate can be linked and
//! still never call the thing a requirement is about. `PROBES` names
//! production entry points and the text of a call that reaches them. The scan
//! looks for that call in non-test source outside the defining file. Probes in
//! `EXPECTED_UNWIRED` are the known backlog: they do not fail the build while
//! unwired, but they fail the moment they are wired (remove them from the
//! list) and any probe not listed that is found unwired fails. The backlog
//! can only shrink.
//!
//! Limits, stated so the gate is not read as more than it is. The scan is
//! textual: comments, string literals, `#[cfg(test)]`/`#[test]` items,
//! out-of-line test modules, and `tests/`, `benches/`, `examples/` are
//! excluded, but a call that exists in production source is not proven to be
//! *executed* (it may sit in a function nothing calls), a call through a
//! re-exporting alias or a macro is not seen, and a renamed dependency
//! (`package = "..."`) is not followed in Part A. "Wired" here means "a
//! production call site exists", which is necessary for reachability and not
//! sufficient for it.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("backend/crates exists beside the acceptance crate")
}

// ---------------------------------------------------------------- Part A

/// Crates that no app depends on, and why that is acceptable. Every entry
/// must be unreachable today (checked) so a stale exemption cannot linger.
const NOT_A_RUNTIME_CRATE: &[(&str, &str)] = &[
    // A test-suite crate: it depends on everything in `[dev-dependencies]` so
    // it can assert across seams, and nothing may depend on it.
    ("qip-acceptance", "cross-cutting test suite; never shipped"),
];

struct CrateInfo {
    dir: PathBuf,
    deps: BTreeSet<String>,
}

/// Read `name` and the `[dependencies]` names (including `target.*` tables and
/// the `[dependencies.x]` table form) with plain string handling. `[dev-` and
/// `[build-` dependencies are deliberately not followed: they do not link into
/// a shipped binary.
fn parse_manifest(text: &str) -> (Option<String>, BTreeSet<String>) {
    let mut name = None;
    let mut deps = BTreeSet::new();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            section = line.trim_matches(|c| c == '[' || c == ']').to_string();
            // `[dependencies.qip-x]` names the dependency in the header.
            if let Some(rest) = section.strip_prefix("dependencies.") {
                deps.insert(rest.trim().to_string());
            }
            continue;
        }
        let is_dep_table = section == "dependencies"
            || (section.starts_with("target.") && section.ends_with(".dependencies"));
        if section == "package" && line.starts_with("name") {
            if let Some((_, v)) = line.split_once('=') {
                name = Some(v.trim().trim_matches('"').to_string());
            }
        } else if is_dep_table {
            let key: String = line
                .chars()
                .take_while(|c| !matches!(c, ' ' | '=' | '.' | '\t'))
                .collect();
            if !key.is_empty() {
                deps.insert(key);
            }
        }
    }
    (name, deps)
}

fn load_crates(root: &Path) -> BTreeMap<String, CrateInfo> {
    let mut out = BTreeMap::new();
    for group in fs::read_dir(root).expect("read crates root").flatten() {
        if !group.path().is_dir() {
            continue;
        }
        for member in fs::read_dir(group.path()).expect("read group").flatten() {
            let manifest = member.path().join("Cargo.toml");
            let Ok(text) = fs::read_to_string(&manifest) else {
                continue;
            };
            let (name, deps) = parse_manifest(&text);
            let name = name.unwrap_or_else(|| panic!("{} has no package name", manifest.display()));
            out.insert(
                name,
                CrateInfo {
                    dir: member.path(),
                    deps,
                },
            );
        }
    }
    out
}

/// Crates reachable from `roots` through `[dependencies]`, roots included.
fn closure(crates: &BTreeMap<String, CrateInfo>, roots: &[String]) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = roots.iter().cloned().collect();
    let mut stack: Vec<String> = roots.to_vec();
    while let Some(c) = stack.pop() {
        if let Some(info) = crates.get(&c) {
            for d in &info.deps {
                if crates.contains_key(d) && seen.insert(d.clone()) {
                    stack.push(d.clone());
                }
            }
        }
    }
    seen
}

fn app_roots(root: &Path, crates: &BTreeMap<String, CrateInfo>) -> Vec<String> {
    let apps = root.join("apps");
    crates
        .iter()
        .filter(|(_, i)| i.dir.starts_with(&apps))
        .map(|(n, _)| n.clone())
        .collect()
}

/// Unreachable crates not covered by `exempt`.
fn unreachable_crates(
    crates: &BTreeMap<String, CrateInfo>,
    roots: &[String],
    exempt: &BTreeSet<&str>,
) -> Vec<String> {
    let reach = closure(crates, roots);
    crates
        .keys()
        .filter(|n| !reach.contains(*n) && !exempt.contains(n.as_str()))
        .cloned()
        .collect()
}

#[test]
fn every_non_test_crate_is_reachable_from_an_app() {
    let root = crates_root();
    let crates = load_crates(&root);
    let roots = app_roots(&root, &crates);
    assert!(
        roots.len() >= 8,
        "premise: the apps directory holds the eight binaries, found {roots:?}"
    );
    assert!(
        crates.len() > 40,
        "premise: the scan found the workspace's crates, found {}",
        crates.len()
    );
    let exempt: BTreeSet<&str> = NOT_A_RUNTIME_CRATE.iter().map(|(n, _)| *n).collect();
    let orphans = unreachable_crates(&crates, &roots, &exempt);
    println!("unreachable non-test crates: {orphans:?}");
    assert!(
        orphans.is_empty(),
        "crates no app links, directly or transitively: {orphans:?}. Either an app \
         (or a crate an app depends on) must list each under [dependencies] and \
         call it, or the crate belongs in NOT_A_RUNTIME_CRATE with a reason"
    );
}

#[test]
fn the_exemption_list_holds_only_crates_that_are_still_unreachable() {
    let root = crates_root();
    let crates = load_crates(&root);
    let reach = closure(&crates, &app_roots(&root, &crates));
    for (name, why) in NOT_A_RUNTIME_CRATE {
        assert!(!why.is_empty(), "{name} is exempt without a reason");
        assert!(
            crates.contains_key(*name),
            "{name} is exempt but no such crate exists; remove the entry"
        );
        assert!(
            !reach.contains(*name),
            "{name} is now reachable from an app; remove it from NOT_A_RUNTIME_CRATE"
        );
    }
}

#[test]
fn the_manifest_parser_ignores_dev_dependencies_and_follows_tables() {
    // Premise for the gate: a dev-dependency edge must not make a crate look
    // shipped, or the whole of Part A passes vacuously through qip-acceptance.
    let (name, deps) = parse_manifest(
        "[package]\nname = \"x\"\n[dependencies]\nqip-a.workspace = true\n\
         qip-b = { path = \"../b\" }\n[dependencies.qip-c]\npath = \"../c\"\n\
         [target.'cfg(unix)'.dependencies]\nqip-d = \"1\"\n\
         [dev-dependencies]\nqip-e.workspace = true\n[build-dependencies]\nqip-f = \"1\"\n",
    );
    assert_eq!(name.as_deref(), Some("x"));
    let got: Vec<&str> = deps.iter().map(String::as_str).collect();
    assert_eq!(got, ["qip-a", "qip-b", "qip-c", "qip-d"]);
}

// ---------------------------------------------------------------- Part B

struct Probe {
    /// Human name of the requirement's entry point.
    symbol: &'static str,
    /// File that defines it; call sites in this file do not count, because a
    /// definition's own recursion or doc example is not a caller.
    defined_in: &'static str,
    /// Text of a call that reaches it, matched at an identifier boundary.
    needle: &'static str,
}

const PROBES: &[Probe] = &[
    Probe {
        symbol: "rank_acquisitions",
        defined_in: "libs/qip-contracts/src/ambient.rs",
        needle: "rank_acquisitions(",
    },
    Probe {
        symbol: "HostedProvider::connected",
        defined_in: "libs/qip-quantum/src/provider.rs",
        needle: "HostedProvider::connected(",
    },
    // The ingestion correlation id reaches the platform only through this
    // path: the fast brain's run loop holds one SensePath, whose bridge feeds
    // Platform::observe_with_lineage. If the node stops constructing it, the
    // fast brain is back to observe() with no lineage.
    Probe {
        symbol: "PlatformBridge (via SensePath)",
        defined_in: "runtime/qip-kernel/src/bus_bridge.rs",
        needle: "SensePath::new(",
    },
    Probe {
        symbol: "ConductGate::from_policy",
        defined_in: "services/qip-agency/src/conduct_brain.rs",
        needle: "ConductGate::from_policy(",
    },
    Probe {
        symbol: "ToolRegistry::new",
        defined_in: "libs/qip-contracts/src/action.rs",
        needle: "ToolRegistry::new(",
    },
    Probe {
        symbol: "AttentionRouter",
        defined_in: "libs/qip-contracts/src/ambient.rs",
        needle: "AttentionRouter::new(",
    },
    // `Federation` itself is built through `#[derive(Default)]` on
    // `FederationStage`, so the construction a text scan can see is the stage's.
    Probe {
        symbol: "Federation (via FederationStage)",
        defined_in: "runtime/qip-kernel/src/federation_stage.rs",
        needle: "FederationStage::new(",
    },
    Probe {
        symbol: "set_survival_policy",
        defined_in: "runtime/qip-kernel/src/platform.rs",
        needle: "set_survival_policy(",
    },
    Probe {
        symbol: "drain_evidence",
        defined_in: "services/qip-market-ingestion/src/connector_feed.rs",
        needle: ".drain_evidence(",
    },
    // R12-CONTRACT-7089: the envelope is the unit both ends of the fabric
    // read. The producer's drain opens every record before it sends one
    // (qip-edge-node drain.rs); the ledger's consumer opens every ledger
    // record it applies (qip-ledgerd consumer.rs). Two probes, because one
    // caller disappearing must not hide behind the other.
    Probe {
        symbol: "FabricEnvelope::open_all (producer drain)",
        defined_in: "libs/qip-events/src/event_fabric/envelope.rs",
        needle: "FabricEnvelope::open_all(",
    },
    Probe {
        symbol: "FabricEnvelope::open_decoded (ledger consumer)",
        defined_in: "libs/qip-events/src/event_fabric/envelope.rs",
        needle: "FabricEnvelope::open_decoded(",
    },
    Probe {
        symbol: "absorb_evidence (kernel)",
        defined_in: "runtime/qip-kernel/src/evidence_intake.rs",
        needle: "platform.absorb_evidence(",
    },
];

/// The known backlog: probes with no production call site. Each entry is a
/// requirement implemented and tested but reached by no binary
/// (docs/blueprint/v12-completion-plan.md, Wave 1). Wiring one makes this
/// test fail until the entry is deleted, so the list shrinks and never grows.
const EXPECTED_UNWIRED: &[&str] = &[
    // The hosted quantum provider is constructed only by tests/hosted.rs;
    // no app builds a connected provider.
    "HostedProvider::connected",
];

/// Replace comments and string/char literals with spaces, newlines kept so
/// line numbers survive.
fn blank_comments_and_strings(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(b[i]));
                    i += 1;
                }
            }
        } else if c == 'r' && (i == 0 || !(b[i - 1].is_alphanumeric() || b[i - 1] == '_')) && {
            let mut j = i + 1;
            while b.get(j) == Some(&'#') {
                j += 1;
            }
            b.get(j) == Some(&'"')
        } {
            let mut hashes = 0;
            let mut j = i + 1;
            while b[j] == '#' {
                hashes += 1;
                j += 1;
            }
            j += 1; // opening quote
            while j < b.len() {
                if b[j] == '"' && (0..hashes).all(|k| b.get(j + 1 + k) == Some(&'#')) {
                    j += 1 + hashes;
                    break;
                }
                j += 1;
            }
            for &ch in &b[i..j.min(b.len())] {
                out.push(blank(ch));
            }
            i = j;
        } else if c == '"' {
            out.push(' ');
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' && i + 1 < b.len() {
                    out.push(blank(b[i]));
                    i += 1;
                }
                out.push(blank(b[i]));
                i += 1;
            }
            out.push(' ');
            i += 1;
        } else if c == '\'' {
            // A char literal closes within a few chars; a lifetime does not.
            let close = if next == Some('\\') {
                (i + 2..(i + 12).min(b.len())).find(|&k| b[k] == '\'')
            } else if b.get(i + 2) == Some(&'\'') {
                Some(i + 2)
            } else {
                None
            };
            if let Some(k) = close {
                for _ in i..=k {
                    out.push(' ');
                }
                i = k + 1;
            } else {
                out.push(c);
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Blank every item introduced by `#[cfg(test...)]` or `#[test]`. Returns the
/// stripped text and the names of out-of-line `mod x;` declarations that were
/// test-only, so their files can be skipped. Input must already have comments
/// and strings blanked, so braces balance.
fn strip_test_items(src: &str) -> (String, Vec<String>) {
    let mut chars: Vec<char> = src.chars().collect();
    let mut test_mods = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' && chars.get(i + 1) == Some(&'[') {
            let rest: String = chars[i..chars.len().min(i + 24)].iter().collect();
            let compact: String = rest.chars().filter(|c| !c.is_whitespace()).collect();
            if compact.starts_with("#[cfg(test") || compact.starts_with("#[test]") {
                // Skip to the end of this attribute, then through the item.
                let mut j = i;
                let mut depth = 0i32;
                while j < chars.len() {
                    match chars[j] {
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                j += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                let item_start = j;
                let mut end = chars.len();
                let mut k = item_start;
                while k < chars.len() {
                    if chars[k] == ';' {
                        end = k + 1;
                        let item: String = chars[item_start..k].iter().collect();
                        let words: Vec<&str> = item.split_whitespace().collect();
                        if let Some(n) = words
                            .iter()
                            .position(|w| *w == "mod")
                            .and_then(|p| words.get(p + 1))
                        {
                            test_mods.push((*n).to_string());
                        }
                        break;
                    }
                    if chars[k] == '{' {
                        let mut d = 0;
                        while k < chars.len() {
                            if chars[k] == '{' {
                                d += 1;
                            } else if chars[k] == '}' {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            k += 1;
                        }
                        end = (k + 1).min(chars.len());
                        break;
                    }
                    k += 1;
                }
                for c in &mut chars[i..end] {
                    if *c != '\n' {
                        *c = ' ';
                    }
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    (chars.into_iter().collect(), test_mods)
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Production sources: `(path relative to crates root, stripped text)`.
fn production_sources(root: &Path, crates: &BTreeMap<String, CrateInfo>) -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut stripped: Vec<(PathBuf, String)> = Vec::new();
    let mut test_only: BTreeSet<PathBuf> = BTreeSet::new();
    for (name, info) in crates {
        if name == "qip-acceptance" {
            continue;
        }
        let mut files = Vec::new();
        walk(&info.dir.join("src"), &mut files);
        for f in files {
            let Ok(text) = fs::read_to_string(&f) else {
                continue;
            };
            let (clean, mods) = strip_test_items(&blank_comments_and_strings(&text));
            let dir = f.parent().map(Path::to_path_buf).unwrap_or_default();
            for m in mods {
                test_only.insert(dir.join(format!("{m}.rs")));
                test_only.insert(dir.join(&m).join("mod.rs"));
            }
            stripped.push((f, clean));
        }
    }
    stripped
        .into_iter()
        .filter(|(p, _)| !test_only.contains(p))
        .map(|(p, t)| {
            let rel = p
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_default();
            (rel, t)
        })
        .collect()
}

/// First production call site of `probe` outside its defining file.
fn find_call(probe: &Probe, sources: &[(String, String)]) -> Option<(String, usize)> {
    for (rel, text) in sources {
        if rel == probe.defined_in {
            continue;
        }
        for (n, line) in text.lines().enumerate() {
            let mut from = 0;
            while let Some(off) = line[from..].find(probe.needle) {
                let at = from + off;
                let before = line[..at].chars().next_back();
                let is_def = line[..at].trim_end().ends_with("fn");
                let boundary = probe.needle.starts_with('.') || !before.is_some_and(is_ident);
                if boundary && !is_def {
                    return Some((rel.clone(), n + 1));
                }
                from = at + probe.needle.len();
            }
        }
    }
    None
}

fn evaluate(
    probes: &[Probe],
    expected_unwired: &[&str],
    sources: &[(String, String)],
) -> Result<String, String> {
    let mut table = String::from("symbol | defining file | status | caller\n");
    let mut errors = Vec::new();
    for p in probes {
        let call = find_call(p, sources);
        let listed = expected_unwired.contains(&p.symbol);
        let (status, caller) = match &call {
            Some((f, l)) => ("wired", format!("{f}:{l}")),
            None => ("UNWIRED", "-".to_string()),
        };
        table.push_str(&format!(
            "{} | {} | {} | {}\n",
            p.symbol, p.defined_in, status, caller
        ));
        match (&call, listed) {
            (Some(_), true) => errors.push(format!(
                "`{}` is now wired (caller {}); remove it from EXPECTED_UNWIRED so the \
                 backlog ratchets down",
                p.symbol, caller
            )),
            (None, false) => errors.push(format!(
                "`{}` has no production call site outside {} and is not on \
                 EXPECTED_UNWIRED; wire it, or add it to the list with the plan item \
                 that owns it",
                p.symbol, p.defined_in
            )),
            _ => {}
        }
    }
    for name in expected_unwired {
        if !probes.iter().any(|p| p.symbol == *name) {
            errors.push(format!(
                "EXPECTED_UNWIRED names `{name}` which is not a probe; remove it or add the probe"
            ));
        }
    }
    if errors.is_empty() {
        Ok(table)
    } else {
        Err(format!("{table}\n{}", errors.join("\n")))
    }
}

#[test]
fn the_unwired_backlog_is_exactly_the_listed_symbols_and_only_shrinks() {
    let root = crates_root();
    let crates = load_crates(&root);
    let sources = production_sources(&root, &crates);
    assert!(
        sources.len() > 200,
        "premise: the scan read the workspace's production sources, read {}",
        sources.len()
    );
    for p in PROBES {
        assert!(
            root.join(p.defined_in).is_file(),
            "probe `{}` names a defining file that does not exist: {}",
            p.symbol,
            p.defined_in
        );
    }
    match evaluate(PROBES, EXPECTED_UNWIRED, &sources) {
        Ok(table) => println!("{table}"),
        Err(report) => panic!("{report}"),
    }
}

#[test]
fn the_scanner_ignores_comments_strings_and_test_items() {
    // Premise for the ratchet: a call that exists only in a comment, a string
    // or a #[cfg(test)] module must not count as wiring, or a probe goes
    // "wired" on the strength of its own documentation.
    let src = "// foo_probe(\nfn a() { let s = \"foo_probe(\"; let c = '\"'; }\n\
               #[cfg(test)]\nmod tests { fn t() { foo_probe(); } }\n\
               fn real() { bar_probe(); }\n";
    let (clean, _) = strip_test_items(&blank_comments_and_strings(src));
    assert!(!clean.contains("foo_probe"), "leaked: {clean}");
    assert!(clean.contains("bar_probe("), "over-stripped: {clean}");
    assert_eq!(clean.lines().count(), src.lines().count());
}

#[test]
fn the_ratchet_fires_both_ways() {
    let probe = [Probe {
        symbol: "p",
        defined_in: "a.rs",
        needle: "p_call(",
    }];
    let called = vec![
        ("a.rs".to_string(), "fn p_call() { p_call(); }".to_string()),
        ("b.rs".to_string(), "fn f() { p_call(); }".to_string()),
    ];
    let uncalled = vec![("a.rs".to_string(), "fn p_call() {}".to_string())];
    assert!(evaluate(&probe, &[], &called).is_ok());
    assert!(evaluate(&probe, &["p"], &uncalled).is_ok());
    let stale = evaluate(&probe, &["p"], &called).expect_err("wired but listed must fail");
    assert!(stale.contains("remove it from EXPECTED_UNWIRED"), "{stale}");
    let unlisted = evaluate(&probe, &[], &uncalled).expect_err("unwired and unlisted must fail");
    assert!(unlisted.contains("not on EXPECTED_UNWIRED"), "{unlisted}");
}
