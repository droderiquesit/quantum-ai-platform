//! The blueprint registers, rendered from their machine-readable sources.
//!
//! Three sources of truth live in the repository as JSON, and three views
//! are rendered from them:
//!
//! - the requirement catalogue, `docs/blueprint/requirements/*.json`, is
//!   rendered to `docs/blueprint/requirements.md`;
//! - the assessment, `docs/blueprint/assessment/*.json`, is rendered with the
//!   catalogue to `docs/blueprint/traceability-matrix.md`;
//! - the current-state map, `docs/architecture/current-state/*.json`, is
//!   rendered to `docs/architecture/current-state.md`.
//!
//! A fourth view has no source of its own beyond a list of citations:
//! `docs/blueprint/v12-completeness-targets.json` names, for each of the ten
//! COMPLETE TARGET items of blueprint v12.0 §31.1, the requirements that
//! carry it, and `docs/blueprint/v12-completeness-targets.md` is rendered
//! from that list and the assessment. A target has no status of its own to
//! edit — see [`target_score`] — which is the point of it (ARCH-074).
//!
//! This exists so that nobody edits a view by hand. A hand-edited view
//! drifts from its source silently, and nineteen status documents came to
//! disagree with each other that way before 2026-09-07. The matrix is
//! rendered even before any assessment exists, with every row reading
//! UNASSESSED, because an absent register invites someone to start a second
//! one. See ADR 0099.
//!
//! [`render_views`] is pure: sources in, views out. [`views`] and [`write`]
//! do the file I/O, which is why this lives in the operator CLI and not in a
//! library.

use qip_core::error::{Error, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The exit code `qip blueprint check` returns when a view is stale. It is
/// the same "I looked and it is wrong" code the CLI's other verdict commands
/// use.
pub const STALE: u8 = 3;

const DOMAINS: [(&str, &str); 31] = [
    (
        "ARCH",
        "Architecture: time lanes, planes, cross-cutting invariants, build-order phases",
    ),
    ("CONTRACT", "Typed contracts between brains and components"),
    ("FABRIC", "Native Rust Event & Control Fabric"),
    ("REFLEX", "Regional Reflex Cell / Node (hot path)"),
    ("MESH", "Reflex Mesh and multi-leg arbitrage coordination"),
    (
        "EXEC",
        "Execution venue mesh, market making, market creation",
    ),
    ("RISK", "Risk Brain and deterministic Risk Gate"),
    ("CAPITAL", "Capital Brain and Capital Bank / treasury"),
    ("ASSET", "Asset / Portfolio Brain"),
    ("LEDGER", "Ledger, accounting, settlement, reconciliation"),
    (
        "DATA",
        "Scout fabric, source manifests, pass-through data, knowledge tiers, storage",
    ),
    ("EVID", "Evidence, provenance and truth fabric"),
    (
        "TICK",
        "Market intelligence and tick learning, replay, digital twin",
    ),
    (
        "WORLD",
        "World Model Federation, memory, self-model, specialist brains",
    ),
    ("REASON", "Symbolic and neuro-symbolic reasoning fabric"),
    ("AMBIENT", "Proactive ambient model mesh and attention"),
    (
        "AGENCY",
        "Causal agency, interventions, conduct gate, effect attribution",
    ),
    (
        "MODEL",
        "Model and strategy foundry, training, evaluation brain",
    ),
    ("QUANT", "Quantum Foundry and classical baselines"),
    (
        "EXPAND",
        "Intelligence expansion engine, registries, curriculum",
    ),
    (
        "EVENT",
        "Prediction / event markets, regulated wagering isolation",
    ),
    ("COMMERCE", "Physical commerce and product arbitrage"),
    ("GOV", "Governance, legal isolation, authority envelopes"),
    ("SEC", "Security and trust"),
    ("OBS", "Observability, SLOs, AIOps"),
    (
        "CICD",
        "CI/CD, supply chain, GitOps, autonomous development",
    ),
    ("GCP", "GCP organisation, network, placement"),
    ("API", "Public edge, portal/BFF, identity, API boundary"),
    ("RES", "Failure, degradation, disaster recovery, game days"),
    ("FINOPS", "Cost governance, capacity, scaling"),
    ("COVERAGE", "Asset and market class coverage"),
];

const FLAGS: [(&str, &str); 9] = [
    (
        "LIVE_CAPITAL",
        "needs live orders or real money movement; refused (ADR 0003/0021, conflict C1)",
    ),
    (
        "EXTERNAL_ACTION",
        "needs the platform to act on the outside world; refused (C1)",
    ),
    (
        "NEW_DEPENDENCY",
        "needs a crate beyond serde/serde_json (C2)",
    ),
    (
        "MANAGED_SERVICE",
        "needs a GCP managed data/AI service (C4)",
    ),
    ("KUBERNETES", "needs GKE / Argo / service mesh (C3)"),
    ("NON_RUST", "needs a non-Rust runtime (C5)"),
    (
        "MULTI_REPO",
        "needs a multi-repository split (C6, declined)",
    ),
    ("MULTI_REGION", "needs more than one region (C8)"),
    ("COST", "implies significant recurring spend (C8)"),
];

const STATUSES: [&str; 8] = [
    "COMPLETE",
    "PARTIAL",
    "MISSING",
    "INCORRECT",
    "BLOCKED",
    "OBSOLETE",
    "NEEDS-VALIDATION",
    "UNASSESSED",
];

/// Everything the views are rendered from, already parsed.
#[derive(Debug, Default)]
pub struct Sources {
    /// Every requirement, in file-name order and then in file order.
    pub requirements: Vec<Value>,
    /// Assessment rows keyed by requirement id.
    pub assessment: BTreeMap<String, Value>,
    /// Current-state groups in file-name order, if the map exists.
    pub current_state: Option<CurrentState>,
    /// The v12.0 §31.1 completeness targets and the requirements each cites,
    /// in the blueprint's order; empty where the list does not exist.
    pub completeness_targets: Vec<Value>,
}

/// The current-state map: one summary per group and the records of each group.
#[derive(Debug, Default)]
pub struct CurrentState {
    /// Each group's summary, keyed by group id.
    pub summaries: BTreeMap<String, Value>,
    /// `(group id, records)` in file-name order.
    pub groups: Vec<(String, Vec<Value>)>,
}

/// One rendered view: the repository-relative path and the text it should hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// Path relative to the repository root.
    pub path: &'static str,
    /// The complete rendered text.
    pub text: String,
}

const REQ_DIR: &str = "docs/blueprint/requirements";
const ASSESS_DIR: &str = "docs/blueprint/assessment";
const STATE_DIR: &str = "docs/architecture/current-state";
const REQ_MD: &str = "docs/blueprint/requirements.md";
const MATRIX_MD: &str = "docs/blueprint/traceability-matrix.md";
const STATE_MD: &str = "docs/architecture/current-state.md";
/// Deliberately beside the two register directories and not inside either:
/// [`load`] reads every `*.json` under them as requirements or assessment
/// rows, and this is neither.
const TARGETS_JSON: &str = "docs/blueprint/v12-completeness-targets.json";
const TARGETS_MD: &str = "docs/blueprint/v12-completeness-targets.md";

/// Read every source under `root`.
///
/// A requirement file or assessment file that does not parse is an error
/// rather than a skipped file: a view rendered from a silently truncated
/// catalogue would report fewer requirements than the blueprint has.
pub fn load(root: &Path) -> Result<Sources> {
    let mut requirements = Vec::new();
    for path in json_files(&root.join(REQ_DIR))? {
        requirements.extend(read_array(&path)?);
    }
    if requirements.is_empty() {
        return Err(Error::not_found(format!(
            "no requirements under {}; run this from the repository root or pass --root",
            root.join(REQ_DIR).display()
        )));
    }
    let mut assessment = BTreeMap::new();
    let assess_dir = root.join(ASSESS_DIR);
    if assess_dir.is_dir() {
        for path in json_files(&assess_dir)? {
            for row in read_array(&path)? {
                let id = text(&row["id"]);
                if id.is_empty() {
                    return Err(Error::schema(format!(
                        "{} holds a row with no id",
                        path.display()
                    )));
                }
                assessment.insert(id, row);
            }
        }
    }
    let state_dir = root.join(STATE_DIR);
    let summaries_path = state_dir.join("_summaries.json");
    let current_state = if summaries_path.is_file() {
        let mut summaries = BTreeMap::new();
        for summary in read_array(&summaries_path)? {
            summaries.insert(text(&summary["group"]), summary);
        }
        let mut groups = Vec::new();
        for path in json_files(&state_dir)? {
            let name = stem(&path);
            if name.starts_with('_') {
                continue;
            }
            groups.push((name, read_array(&path)?));
        }
        Some(CurrentState { summaries, groups })
    } else {
        None
    };
    let targets_path = root.join(TARGETS_JSON);
    let completeness_targets = if targets_path.is_file() {
        read_array(&targets_path)?
    } else {
        Vec::new()
    };
    // A target that cites nothing, or cites an id the catalogue does not
    // hold, is refused here rather than rendered: it would otherwise appear
    // in the view as a row with no evidence under it, which is a target
    // scored on its own text — the one thing the view exists to prevent.
    for target in &completeness_targets {
        let cited = strings(&target["requirements"]);
        if text(&target["target"]).is_empty() || cited.is_empty() {
            return Err(Error::schema(format!(
                "{TARGETS_JSON} holds a target with no text or no cited requirement; a \
                 completeness target is scored from the requirements that carry it, so name \
                 at least one requirement id for it"
            )));
        }
        for id in cited {
            if !requirements.iter().any(|r| text(&r["id"]) == id) {
                return Err(Error::schema(format!(
                    "{TARGETS_JSON} cites {id}, which is not a requirement under {REQ_DIR}; \
                     cite an id the catalogue holds, or add the requirement first"
                )));
            }
        }
    }
    Ok(Sources {
        requirements,
        assessment,
        current_state,
        completeness_targets,
    })
}

/// Render every view from already-loaded sources. Pure.
pub fn render_views(sources: &Sources) -> Vec<View> {
    let mut views = vec![
        View {
            path: REQ_MD,
            text: render_requirements(&sources.requirements),
        },
        View {
            path: MATRIX_MD,
            text: render_matrix(&sources.requirements, &sources.assessment),
        },
    ];
    if let Some(state) = &sources.current_state {
        views.push(View {
            path: STATE_MD,
            text: render_current_state(state),
        });
    }
    if !sources.completeness_targets.is_empty() {
        views.push(View {
            path: TARGETS_MD,
            text: render_completeness(&sources.completeness_targets, &sources.assessment),
        });
    }
    views
}

/// The views under `root` whose file content differs from what the sources
/// render to, as repository-relative paths.
pub fn stale(root: &Path, views: &[View]) -> Vec<&'static str> {
    views
        .iter()
        .filter(|view| {
            std::fs::read_to_string(root.join(view.path))
                .ok()
                .as_deref()
                != Some(view.text.as_str())
        })
        .map(|view| view.path)
        .collect()
}

/// Write every stale view under `root`, returning the paths written.
pub fn write(root: &Path, views: &[View]) -> Result<Vec<&'static str>> {
    let paths = stale(root, views);
    for view in views.iter().filter(|view| paths.contains(&view.path)) {
        std::fs::write(root.join(view.path), &view.text)
            .map_err(|error| Error::io(format!("could not write {}: {error}", view.path)))?;
    }
    Ok(paths)
}

fn json_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| Error::io(format!("could not read {}: {error}", directory.display())))?;
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| Error::io(format!("could not list {}: {error}", directory.display())))?
            .path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn read_array(path: &Path) -> Result<Vec<Value>> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| Error::io(format!("could not read {}: {error}", path.display())))?;
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Array(items)) => Ok(items),
        Ok(_) => Err(Error::schema(format!(
            "{} is not a JSON array",
            path.display()
        ))),
        Err(error) => Err(Error::schema(format!(
            "{} does not parse: {error}",
            path.display()
        ))),
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A value as display text, with empty, zero, false and null all rendering
/// as nothing, so that an absent field and an empty one read the same.
fn text(value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(false) => String::new(),
        Value::Bool(true) => "True".to_string(),
        Value::String(string) => string.clone(),
        Value::Number(number) => {
            if number.as_f64() == Some(0.0) {
                String::new()
            } else {
                number.to_string()
            }
        }
        Value::Array(items) if items.is_empty() => String::new(),
        Value::Object(map) if map.is_empty() => String::new(),
        other => other.to_string(),
    }
}

/// One Markdown table cell: pipes escaped, all whitespace collapsed to one
/// space. A newline inside a cell ends the table row early, and whatever
/// follows it renders as a stray paragraph that no reviewer attributes to
/// the requirement it belongs to.
fn cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| items.iter().map(text).collect())
        .unwrap_or_default()
}

fn cite(sources: &Value) -> String {
    let mut out = Vec::new();
    for source in sources.as_array().into_iter().flatten() {
        let doc = match source["doc"].as_str() {
            Some("M") => "v11.6".to_string(),
            Some("G") => "GCP v2.1".to_string(),
            // ADR 0101: v12.0 and GCP v3.0 get their own codes rather than
            // reusing M and G, because a requirement cited at "p31" must say
            // which document's page 31 — the two editions paginate differently.
            Some("M12") => "v12.0".to_string(),
            Some("G3") => "GCP v3.0".to_string(),
            Some("H") => "diagram".to_string(),
            _ => text(&source["doc"]),
        };
        let page = if source.get("page").is_some() {
            text(&source["page"])
        } else {
            "?".to_string()
        };
        out.push(
            format!("{doc} p{page} {}", text(&source["section"]))
                .trim()
                .to_string(),
        );
    }
    out.join("; ")
}

fn render_requirements(requirements: &[Value]) -> String {
    let mut by_domain: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for requirement in requirements {
        by_domain
            .entry(text(&requirement["domain"]))
            .or_default()
            .push(requirement);
    }
    let count_priority = |priority: &str| {
        requirements
            .iter()
            .filter(|r| r["priority"] == priority)
            .count()
    };
    let count_flag = |flag: &str| {
        requirements
            .iter()
            .map(|r| {
                strings(&r["policy_flags"])
                    .iter()
                    .filter(|f| f.as_str() == flag)
                    .count()
            })
            .sum::<usize>()
    };
    let mut lines: Vec<String> = vec![
        "# Blueprint requirement catalogue".into(),
        String::new(),
        "Generated by `qip blueprint render` from".into(),
        "`docs/blueprint/requirements/*.json`. **Do not edit this file by hand**:".into(),
        "edit the JSON and re-render. `qip blueprint check` fails when this view is stale.".into(),
        String::new(),
        "Sources and precedence: [README.md](README.md). Architecture of record:".into(),
        "ADR 0099.".into(),
        String::new(),
        format!(
            "**{} requirements** in {} domains. Priority: {}.",
            requirements.len(),
            by_domain.len(),
            ["P0", "P1", "P2", "P3"]
                .iter()
                .map(|p| format!("{p} {}", count_priority(p)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        String::new(),
        "P0 covers safety and correctness invariants, contracts, and the exit criteria of the"
            .into(),
        "foundation phases. P1 covers the cognitive data loop, the tick lake and replay,".into(),
        "multi-asset capital and risk, and multi-region. P2 covers arbitrage, market making,"
            .into(),
        "quantum, prediction and commerce, and AGI autonomy. P3 covers open-ended expansion".into(),
        "and causal agency.".into(),
        String::new(),
        "## Policy flags".into(),
        String::new(),
        "A flag marks a requirement that cannot be fully met without something a standing".into(),
        "decision refuses. The conflict register is in ADR 0099.".into(),
        String::new(),
        "| Flag | Requirements | Meaning |".into(),
        "|---|---|---|".into(),
    ];
    for (flag, meaning) in FLAGS {
        lines.push(format!("| `{flag}` | {} | {meaning} |", count_flag(flag)));
    }
    lines.extend([String::new(), "## Domains".into(), String::new()]);
    lines.push("| Domain | Requirements | P0 | Scope |".into());
    lines.push("|---|---|---|---|".into());
    for (domain, scope) in DOMAINS {
        let rows = by_domain.get(domain).map(Vec::as_slice).unwrap_or_default();
        let p0 = rows.iter().filter(|r| r["priority"] == "P0").count();
        lines.push(format!(
            "| [{domain}](#{}) | {} | {p0} | {scope} |",
            domain.to_lowercase(),
            rows.len()
        ));
    }
    for (domain, scope) in DOMAINS {
        let Some(rows) = by_domain.get(domain) else {
            continue;
        };
        lines.extend([
            String::new(),
            format!("## {domain}"),
            String::new(),
            format!("{scope}."),
            String::new(),
        ]);
        lines.push("| ID | P | Kind | Requirement | Flags | Verification | Source |".into());
        lines.push("|---|---|---|---|---|---|---|".into());
        for r in rows {
            let flags = strings(&r["policy_flags"])
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(" ");
            let verification = &r["verification"];
            lines.push(format!(
                "| {} | {} | {} | **{}** — {} | {} | {}: {} | {} |",
                text(&r["id"]),
                text(&r["priority"]),
                text(&r["kind"]),
                cell(&text(&r["title"])),
                cell(&text(&r["statement"])),
                cell(&flags),
                cell(&text(&verification["method"])),
                cell(&text(&verification["check"])),
                cell(&cite(&r["sources"])),
            ));
        }
    }
    lines.join("\n") + "\n"
}

fn render_matrix(requirements: &[Value], assessment: &BTreeMap<String, Value>) -> String {
    let empty = Value::Null;
    let row = |r: &Value| assessment.get(&text(&r["id"])).unwrap_or(&empty);
    let status = |r: &Value| {
        let s = text(&row(r)["status"]);
        if s.is_empty() {
            "UNASSESSED".to_string()
        } else {
            s
        }
    };
    let mut lines: Vec<String> = vec![
        "# Blueprint traceability matrix".into(),
        String::new(),
        "Generated by `qip blueprint render` from the requirement catalogue and".into(),
        "`docs/blueprint/assessment/*.json`. **Do not edit by hand.** This is the live".into(),
        "register (ADR 0099). `docs/DELIVERY-STATUS.md` is the v10.1 historical register.".into(),
        String::new(),
    ];
    let applicable: Vec<&Value> = requirements
        .iter()
        .filter(|r| status(r) != "OBSOLETE")
        .collect();
    let denominator = applicable.len().max(1);
    let measure = |predicate: &dyn Fn(&Value) -> bool| {
        let count = applicable.iter().filter(|r| predicate(row(r))).count();
        (
            count,
            format!("{:.1}%", 100.0 * count as f64 / denominator as f64),
        )
    };
    let is_true = |key: &'static str| move |a: &Value| a[key] == Value::Bool(true);
    let measures: [(&str, (usize, String)); 6] = [
        (
            "Blueprint completion (COMPLETE)",
            measure(&|a: &Value| a["status"] == "COMPLETE"),
        ),
        (
            "Implemented (behaviour exists in code)",
            measure(&is_true("implemented")),
        ),
        (
            "Tested (a named test demonstrates it)",
            measure(&is_true("tested")),
        ),
        (
            "Integrated (reached from a composition root)",
            measure(&is_true("integrated")),
        ),
        (
            "Deployable (provisionable from committed config)",
            measure(&is_true("deployable")),
        ),
        ("End-to-end demonstrated", measure(&is_true("e2e"))),
    ];
    lines.push(format!(
        "**{} requirements; {} applicable** (OBSOLETE excluded from the denominator).",
        requirements.len(),
        applicable.len()
    ));
    lines.extend([
        String::new(),
        "| Measure | Count | Share of applicable |".into(),
        "|---|---|---|".into(),
    ]);
    for (name, (count, share)) in measures {
        lines.push(format!("| {name} | {count} | {share} |"));
    }
    lines.extend([
        String::new(),
        "| Status | Requirements |".into(),
        "|---|---|".into(),
    ]);
    for s in STATUSES {
        let count = requirements.iter().filter(|r| status(r) == s).count();
        if count > 0 {
            lines.push(format!("| {s} | {count} |"));
        }
    }
    let mut by_domain: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for requirement in requirements {
        by_domain
            .entry(text(&requirement["domain"]))
            .or_default()
            .push(requirement);
    }
    lines.extend([String::new(), "## By domain".into(), String::new()]);
    lines.push("| Domain | Reqs | COMPLETE | PARTIAL | MISSING | BLOCKED | Other | Implemented | Tested | E2E |".into());
    lines.push("|---|---|---|---|---|---|---|---|---|---|".into());
    for (domain, _) in DOMAINS {
        let Some(rows) = by_domain.get(domain) else {
            continue;
        };
        let with = |s: &str| rows.iter().filter(|r| status(r) == s).count();
        let flagged = |key: &str| {
            rows.iter()
                .filter(|r| row(r)[key] == Value::Bool(true))
                .count()
        };
        let (complete, partial, missing, blocked) = (
            with("COMPLETE"),
            with("PARTIAL"),
            with("MISSING"),
            with("BLOCKED"),
        );
        lines.push(format!(
            "| [{domain}](#{}) | {} | {complete} | {partial} | {missing} | {blocked} | {} | {} | {} | {} |",
            domain.to_lowercase(),
            rows.len(),
            rows.len() - complete - partial - missing - blocked,
            flagged("implemented"),
            flagged("tested"),
            flagged("e2e"),
        ));
    }
    for (domain, _) in DOMAINS {
        let Some(rows) = by_domain.get(domain) else {
            continue;
        };
        lines.extend([String::new(), format!("## {domain}"), String::new()]);
        lines.push("| Requirement | Blueprint target | Current implementation | Status | Gap | Dependency | Priority | Verification | Work item |".into());
        lines.push("|---|---|---|---|---|---|---|---|---|".into());
        for r in rows {
            let a = row(r);
            lines.push(format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                text(&r["id"]),
                cell(&text(&r["title"])),
                cell(&text(&a["current"])),
                cell(&status(r)),
                cell(&text(&a["gap"])),
                cell(&strings(&a["depends_on"]).join(", ")),
                text(&r["priority"]),
                cell(&text(&a["verification"])),
                cell(&text(&a["work_item"])),
            ));
        }
    }
    lines.join("\n") + "\n"
}

/// How a completeness target is scored from the requirements it cites, each
/// given as `(status, tested)`.
///
/// `COMPLETE` only when every cited requirement is `COMPLETE` **and** tested.
/// The second half is the rule ARCH-074 names: a row somebody marked
/// complete with no test under it is text, and a target that inherited its
/// score from such a row would be scored on text alone. Anything else is
/// `OPEN`, with the count that says how far.
///
/// A target has no status field for this to be read from instead. That is
/// deliberate: a status that can be typed is a status that can be typed
/// ahead of the evidence.
pub fn target_score(cited: &[(String, bool)]) -> String {
    let complete = cited
        .iter()
        .filter(|(status, tested)| status == "COMPLETE" && *tested)
        .count();
    if !cited.is_empty() && complete == cited.len() {
        "COMPLETE".to_string()
    } else {
        format!("OPEN ({complete} of {} complete)", cited.len())
    }
}

fn render_completeness(targets: &[Value], assessment: &BTreeMap<String, Value>) -> String {
    let mut lines: Vec<String> = vec![
        "# Blueprint v12.0 completeness targets".into(),
        String::new(),
        "Generated by `qip blueprint render` from".into(),
        "`docs/blueprint/v12-completeness-targets.json` and".into(),
        "`docs/blueprint/assessment/*.json`. **Do not edit by hand.**".into(),
        String::new(),
        "Blueprint v12.0 §31.1 lists ten COMPLETE TARGET items. Each is scored here from".into(),
        "the requirements that carry it and from nothing else: a target is COMPLETE only".into(),
        "when every requirement it cites is COMPLETE and tested, so none is scored on text".into(),
        "alone (ARCH-074). A target has no status of its own to edit; change a cited".into(),
        "requirement's assessment, with its evidence, and render again.".into(),
        String::new(),
        "| # | Target (v12.0 §31.1) | Cited requirement: status | Scored | Reading |".into(),
        "|---|---|---|---|---|".into(),
    ];
    for (index, target) in targets.iter().enumerate() {
        let mut cited = Vec::new();
        let mut shown = Vec::new();
        for id in strings(&target["requirements"]) {
            let row = assessment.get(&id);
            let status = row
                .map(|row| text(&row["status"]))
                .filter(|status| !status.is_empty())
                .unwrap_or_else(|| "UNASSESSED".to_string());
            let tested = row.is_some_and(|row| row["tested"] == Value::Bool(true));
            // Said where it is read: a COMPLETE that does not count has to
            // look different from one that does.
            let untested = if status == "COMPLETE" && !tested {
                " (untested)"
            } else {
                ""
            };
            shown.push(format!("{id}: {status}{untested}"));
            cited.push((status, tested));
        }
        lines.push(format!(
            "| {} | {} | {} | {} | {} |",
            index + 1,
            cell(&text(&target["target"])),
            cell(&shown.join("; ")),
            target_score(&cited),
            cell(&text(&target["reading"])),
        ));
    }
    lines.join("\n") + "\n"
}

fn render_current_state(state: &CurrentState) -> String {
    let total: usize = state.groups.iter().map(|(_, records)| records.len()).sum();
    // Counted in first-seen order and then sorted by count, so equal counts
    // keep a stable order from one render to the next.
    let mut reach: Vec<(String, usize)> = Vec::new();
    for (_, records) in &state.groups {
        for record in records {
            let key = text(&record["reached_from_production"]);
            match reach.iter_mut().find(|(k, _)| *k == key) {
                Some((_, count)) => *count += 1,
                None => reach.push((key, 1)),
            }
        }
    }
    reach.sort_by(|a, b| b.1.cmp(&a.1));
    let mut lines: Vec<String> = vec![
        "# Current-state architecture map".into(),
        String::new(),
        "Generated by `qip blueprint render` from".into(),
        "`docs/architecture/current-state/*.json`. **Do not edit by hand.** Each record was".into(),
        "mapped from the code with file:line evidence (see the JSON `evidence` field).".into(),
        "Mapped on 2026-09-25 at `53fc1f42` for the v11.6 gap analysis (ADR 0099).".into(),
        String::new(),
        "## The fact that governs everything below".into(),
        String::new(),
        "**No process of this platform is running in any environment today.** `execution_nodes = {}`".into(),
        "in every environment, so the edge cell's hot path runs only under `cargo test`. The three".into(),
        "Cloud Run binaries are built and attested but have no live service: the GitOps control".into(),
        "plane that reconciles them is suspended (ADR 0093), and the dev project's billing is".into(),
        "disabled, so `deploy.yml` fails at image push. A capability below that is \"reached from".into(),
        "production\" is reached from a *composition root*, not from a running process.".into(),
        String::new(),
        format!(
            "**{total} component records** in {} groups. Reached from a composition root: {}.",
            state.groups.len(),
            reach.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")
        ),
        String::new(),
    ];
    for (name, records) in &state.groups {
        let summary = state.summaries.get(name).cloned().unwrap_or(Value::Null);
        lines.extend([
            String::new(),
            format!("## {name}"),
            String::new(),
            cell(&text(&summary["headline"])),
            String::new(),
        ]);
        let gaps = strings(&summary["notable_gaps"]);
        if !gaps.is_empty() {
            lines.push("Notable gaps:".into());
            lines.push(String::new());
            lines.extend(gaps.iter().map(|gap| format!("- {}", cell(gap))));
            lines.push(String::new());
        }
        lines.push(
            "| Component | Path | Reached | Status | Tests | Blueprint domains | Purpose |".into(),
        );
        lines.push("|---|---|---|---|---|---|---|".into());
        for record in records {
            lines.push(format!(
                "| {} | `{}` | {} | {} | {} | {} | {} |",
                cell(&text(&record["name"])),
                cell(&text(&record["path"])),
                cell(&text(&record["reached_from_production"])),
                cell(&strings(&record["status"]).join(", ")),
                cell(&text(&record["tests"]["count"])),
                cell(&strings(&record["blueprint_capabilities"]).join(", ")),
                cell(&text(&record["purpose"])),
            ));
        }
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_newline_inside_a_cell_cannot_end_the_table_row() {
        assert_eq!(cell("two\nlines | and a pipe"), "two lines \\| and a pipe");
    }

    #[test]
    fn a_zero_and_an_absent_field_render_the_same() {
        assert_eq!(text(&json!(0)), "");
        assert_eq!(text(&Value::Null), "");
        assert_eq!(text(&json!(35)), "35");
    }

    #[test]
    fn a_v12_citation_names_its_edition_and_never_reads_as_v11_6() {
        let sources = json!([
            {"doc": "M12", "page": "31", "section": "§24"},
            {"doc": "G3", "page": "4", "section": "§2"},
            {"doc": "M", "page": "31", "section": "§24"}
        ]);
        assert_eq!(
            cite(&sources),
            "v12.0 p31 §24; GCP v3.0 p4 §2; v11.6 p31 §24"
        );
    }
}
