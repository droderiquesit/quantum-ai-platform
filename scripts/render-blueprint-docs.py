#!/usr/bin/env python3
"""Render the blueprint registers from their machine-readable sources.

The requirement catalogue (`docs/blueprint/requirements/*.json`) and the
assessment (`docs/blueprint/assessment/*.json`) are the sources of truth. The
Markdown files here are *views*. This script exists so that nobody edits a
view by hand, because a view edited by hand drifts from its source silently.
That is how nineteen status documents came to disagree before 2026-09-07.

    python3 scripts/render-blueprint-docs.py            # write the views
    python3 scripts/render-blueprint-docs.py --check    # exit 1 if a view is stale

Standard library only: this is repository tooling, and it must not become a
second dependency surface.
"""
import collections
import glob
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REQ_DIR = os.path.join(ROOT, "docs", "blueprint", "requirements")
ASSESS_DIR = os.path.join(ROOT, "docs", "blueprint", "assessment")
REQ_MD = os.path.join(ROOT, "docs", "blueprint", "requirements.md")
MATRIX_MD = os.path.join(ROOT, "docs", "blueprint", "traceability-matrix.md")

DOMAINS = {
    "ARCH": "Architecture: time lanes, planes, cross-cutting invariants, build-order phases",
    "CONTRACT": "Typed contracts between brains and components",
    "FABRIC": "Native Rust Event & Control Fabric",
    "REFLEX": "Regional Reflex Cell / Node (hot path)",
    "MESH": "Reflex Mesh and multi-leg arbitrage coordination",
    "EXEC": "Execution venue mesh, market making, market creation",
    "RISK": "Risk Brain and deterministic Risk Gate",
    "CAPITAL": "Capital Brain and Capital Bank / treasury",
    "ASSET": "Asset / Portfolio Brain",
    "LEDGER": "Ledger, accounting, settlement, reconciliation",
    "DATA": "Scout fabric, source manifests, pass-through data, knowledge tiers, storage",
    "EVID": "Evidence, provenance and truth fabric",
    "TICK": "Market intelligence and tick learning, replay, digital twin",
    "WORLD": "World Model Federation, memory, self-model, specialist brains",
    "REASON": "Symbolic and neuro-symbolic reasoning fabric",
    "AMBIENT": "Proactive ambient model mesh and attention",
    "AGENCY": "Causal agency, interventions, conduct gate, effect attribution",
    "MODEL": "Model and strategy foundry, training, evaluation brain",
    "QUANT": "Quantum Foundry and classical baselines",
    "EXPAND": "Intelligence expansion engine, registries, curriculum",
    "EVENT": "Prediction / event markets, regulated wagering isolation",
    "COMMERCE": "Physical commerce and product arbitrage",
    "GOV": "Governance, legal isolation, authority envelopes",
    "SEC": "Security and trust",
    "OBS": "Observability, SLOs, AIOps",
    "CICD": "CI/CD, supply chain, GitOps, autonomous development",
    "GCP": "GCP organisation, network, placement",
    "API": "Public edge, portal/BFF, identity, API boundary",
    "RES": "Failure, degradation, disaster recovery, game days",
    "FINOPS": "Cost governance, capacity, scaling",
    "COVERAGE": "Asset and market class coverage",
}

FLAGS = {
    "LIVE_CAPITAL": "needs live orders or real money movement; refused (ADR 0003/0021, conflict C1)",
    "EXTERNAL_ACTION": "needs the platform to act on the outside world; refused (C1)",
    "NEW_DEPENDENCY": "needs a crate beyond serde/serde_json (C2)",
    "MANAGED_SERVICE": "needs a GCP managed data/AI service (C4)",
    "KUBERNETES": "needs GKE / Argo / service mesh (C3)",
    "NON_RUST": "needs a non-Rust runtime (C5)",
    "MULTI_REPO": "needs a multi-repository split (C6, declined)",
    "MULTI_REGION": "needs more than one region (C8)",
    "COST": "implies significant recurring spend (C8)",
}

STATUSES = ["COMPLETE", "PARTIAL", "MISSING", "INCORRECT", "BLOCKED", "OBSOLETE", "NEEDS-VALIDATION"]


def load_requirements():
    reqs = []
    for path in sorted(glob.glob(os.path.join(REQ_DIR, "*.json"))):
        with open(path, encoding="utf-8") as f:
            reqs.extend(json.load(f))
    return reqs


def load_assessment():
    rows = {}
    for path in sorted(glob.glob(os.path.join(ASSESS_DIR, "*.json"))):
        with open(path, encoding="utf-8") as f:
            for row in json.load(f):
                rows[row["id"]] = row
    return rows


def cell(text):
    """One Markdown table cell: no pipes, no newlines."""
    return " ".join(str(text or "").replace("|", "\\|").split())


def cite(sources):
    out = []
    for s in sources or []:
        doc = {"M": "v11.6", "G": "GCP v2.1", "H": "diagram"}.get(s.get("doc"), s.get("doc"))
        out.append(f"{doc} p{s.get('page', '?')} {s.get('section', '')}".strip())
    return "; ".join(out)


def render_requirements(reqs):
    by_domain = collections.defaultdict(list)
    for r in reqs:
        by_domain[r["domain"]].append(r)
    prio = collections.Counter(r["priority"] for r in reqs)
    flags = collections.Counter(f for r in reqs for f in r.get("policy_flags", []))
    lines = [
        "# Blueprint requirement catalogue",
        "",
        "Generated by `scripts/render-blueprint-docs.py` from",
        "`docs/blueprint/requirements/*.json`. **Do not edit this file by hand**:",
        "edit the JSON and re-render. `--check` fails CI when this view is stale.",
        "",
        "Sources and precedence: [README.md](README.md). Architecture of record:",
        "ADR 0099.",
        "",
        f"**{len(reqs)} requirements** in {len(by_domain)} domains. Priority: "
        + ", ".join(f"{p} {prio[p]}" for p in ["P0", "P1", "P2", "P3"])
        + ".",
        "",
        "P0 covers safety and correctness invariants, contracts, and the exit criteria of the",
        "foundation phases. P1 covers the cognitive data loop, the tick lake and replay,",
        "multi-asset capital and risk, and multi-region. P2 covers arbitrage, market making,",
        "quantum, prediction and commerce, and AGI autonomy. P3 covers open-ended expansion",
        "and causal agency.",
        "",
        "## Policy flags",
        "",
        "A flag marks a requirement that cannot be fully met without something a standing",
        "decision refuses. The conflict register is in ADR 0099.",
        "",
        "| Flag | Requirements | Meaning |",
        "|---|---|---|",
    ]
    for k, v in FLAGS.items():
        lines.append(f"| `{k}` | {flags.get(k, 0)} | {v} |")
    lines += ["", "## Domains", "", "| Domain | Requirements | P0 | Scope |", "|---|---|---|---|"]
    for d in DOMAINS:
        rs = by_domain.get(d, [])
        lines.append(f"| [{d}](#{d.lower()}) | {len(rs)} | {sum(1 for r in rs if r['priority'] == 'P0')} | {DOMAINS[d]} |")
    for d in DOMAINS:
        rs = by_domain.get(d, [])
        if not rs:
            continue
        lines += ["", f"## {d}", "", DOMAINS[d] + ".", "",
                  "| ID | P | Kind | Requirement | Flags | Verification | Source |", "|---|---|---|---|---|---|---|"]
        for r in rs:
            v = r.get("verification") or {}
            lines.append(
                f"| {r['id']} | {r['priority']} | {r['kind']} | **{cell(r['title'])}** — {cell(r['statement'])} "
                f"| {cell(' '.join('`'+f+'`' for f in r.get('policy_flags', [])))} "
                f"| {cell(v.get('method', ''))}: {cell(v.get('check', ''))} | {cell(cite(r.get('sources')))} |"
            )
    return "\n".join(lines) + "\n"


def render_matrix(reqs, rows):
    lines = [
        "# Blueprint traceability matrix",
        "",
        "Generated by `scripts/render-blueprint-docs.py` from the requirement catalogue and",
        "`docs/blueprint/assessment/*.json`. **Do not edit by hand.** This is the live",
        "register (ADR 0099). `docs/DELIVERY-STATUS.md` is the v10.1 historical register.",
        "",
    ]
    applicable = [r for r in reqs if rows.get(r["id"], {}).get("status") != "OBSOLETE"]
    n = len(applicable) or 1

    def pct(pred):
        k = sum(1 for r in applicable if pred(rows.get(r["id"], {})))
        return k, f"{100.0 * k / n:.1f}%"

    status = collections.Counter(rows.get(r["id"], {}).get("status", "UNASSESSED") for r in reqs)
    measures = [
        ("Blueprint completion (COMPLETE)", pct(lambda a: a.get("status") == "COMPLETE")),
        ("Implemented (behaviour exists in code)", pct(lambda a: a.get("implemented") is True)),
        ("Tested (a named test demonstrates it)", pct(lambda a: a.get("tested") is True)),
        ("Integrated (reached from a composition root)", pct(lambda a: a.get("integrated") is True)),
        ("Deployable (provisionable from committed config)", pct(lambda a: a.get("deployable") is True)),
        ("End-to-end demonstrated", pct(lambda a: a.get("e2e") is True)),
    ]
    lines += [
        f"**{len(reqs)} requirements; {len(applicable)} applicable** (OBSOLETE excluded from the denominator).",
        "",
        "| Measure | Count | Share of applicable |",
        "|---|---|---|",
    ]
    for name, (k, p) in measures:
        lines.append(f"| {name} | {k} | {p} |")
    lines += ["", "| Status | Requirements |", "|---|---|"]
    for s in STATUSES + ["UNASSESSED"]:
        if status.get(s):
            lines.append(f"| {s} | {status[s]} |")
    by_domain = collections.defaultdict(list)
    for r in reqs:
        by_domain[r["domain"]].append(r)
    lines += ["", "## By domain", "", "| Domain | Reqs | COMPLETE | PARTIAL | MISSING | BLOCKED | Other | Implemented | Tested | E2E |",
              "|---|---|---|---|---|---|---|---|---|---|"]
    for d in DOMAINS:
        rs = by_domain.get(d, [])
        if not rs:
            continue
        c = collections.Counter(rows.get(r["id"], {}).get("status", "UNASSESSED") for r in rs)
        other = len(rs) - c["COMPLETE"] - c["PARTIAL"] - c["MISSING"] - c["BLOCKED"]
        imp = sum(1 for r in rs if rows.get(r["id"], {}).get("implemented") is True)
        tst = sum(1 for r in rs if rows.get(r["id"], {}).get("tested") is True)
        e2e = sum(1 for r in rs if rows.get(r["id"], {}).get("e2e") is True)
        lines.append(f"| [{d}](#{d.lower()}) | {len(rs)} | {c['COMPLETE']} | {c['PARTIAL']} | {c['MISSING']} | {c['BLOCKED']} | {other} | {imp} | {tst} | {e2e} |")
    for d in DOMAINS:
        rs = by_domain.get(d, [])
        if not rs:
            continue
        lines += ["", f"## {d}", "",
                  "| Requirement | Blueprint target | Current implementation | Status | Gap | Dependency | Priority | Verification | Work item |",
                  "|---|---|---|---|---|---|---|---|---|"]
        for r in rs:
            a = rows.get(r["id"], {})
            lines.append(
                f"| {r['id']} | {cell(r['title'])} | {cell(a.get('current', ''))} | {cell(a.get('status', 'UNASSESSED'))} "
                f"| {cell(a.get('gap', ''))} | {cell(', '.join(a.get('depends_on', []) or []))} | {r['priority']} "
                f"| {cell(a.get('verification', ''))} | {cell(a.get('work_item', ''))} |"
            )
    return "\n".join(lines) + "\n"


STATE_DIR = os.path.join(ROOT, "docs", "architecture", "current-state")
STATE_MD = os.path.join(ROOT, "docs", "architecture", "current-state.md")


def render_current_state():
    with open(os.path.join(STATE_DIR, "_summaries.json"), encoding="utf-8") as f:
        summaries = {s["group"]: s for s in json.load(f)}
    groups = []
    for path in sorted(glob.glob(os.path.join(STATE_DIR, "*.json"))):
        name = os.path.basename(path)[:-5]
        if name.startswith("_"):
            continue
        with open(path, encoding="utf-8") as f:
            groups.append((name, json.load(f)))
    total = sum(len(r) for _, r in groups)
    reach = collections.Counter(r.get("reached_from_production") for _, rs in groups for r in rs)
    lines = [
        "# Current-state architecture map",
        "",
        "Generated by `scripts/render-blueprint-docs.py` from",
        "`docs/architecture/current-state/*.json`. **Do not edit by hand.** Each record was",
        "mapped from the code with file:line evidence (see the JSON `evidence` field).",
        "Mapped on 2026-09-25 at `53fc1f42` for the v11.6 gap analysis (ADR 0099).",
        "",
        "## The fact that governs everything below",
        "",
        "**No process of this platform is running in any environment today.** `execution_nodes = {}`",
        "in every environment, so the edge cell's hot path runs only under `cargo test`. The three",
        "Cloud Run binaries are built and attested but have no live service: the GitOps control",
        "plane that reconciles them is suspended (ADR 0093), and the dev project's billing is",
        "disabled, so `deploy.yml` fails at image push. A capability below that is \"reached from",
        "production\" is reached from a *composition root*, not from a running process.",
        "",
        f"**{total} component records** in {len(groups)} groups. Reached from a composition root: "
        + ", ".join(f"{k} {v}" for k, v in reach.most_common()) + ".",
        "",
    ]
    for name, recs in groups:
        s = summaries.get(name) or next((v for k, v in summaries.items() if k.startswith(name)), {})
        lines += ["", f"## {name}", "", cell(s.get("headline", "")), ""]
        gaps = s.get("notable_gaps") or []
        if gaps:
            lines += ["Notable gaps:", ""] + [f"- {cell(g)}" for g in gaps] + [""]
        lines += ["| Component | Path | Reached | Status | Tests | Blueprint domains | Purpose |", "|---|---|---|---|---|---|---|"]
        for r in recs:
            tests = r.get("tests") or {}
            lines.append(
                f"| {cell(r.get('name'))} | `{cell(r.get('path'))}` | {cell(r.get('reached_from_production'))} "
                f"| {cell(', '.join(r.get('status') or []))} | {cell(tests.get('count', ''))} "
                f"| {cell(', '.join(r.get('blueprint_capabilities') or []))} | {cell(r.get('purpose'))} |"
            )
    return "\n".join(lines) + "\n"


def main():
    check = "--check" in sys.argv[1:]
    reqs = load_requirements()
    views = {REQ_MD: render_requirements(reqs)}
    if os.path.exists(os.path.join(STATE_DIR, "_summaries.json")):
        views[STATE_MD] = render_current_state()
    # Rendered even before any assessment exists, so every row reads UNASSESSED
    # rather than the matrix being absent: an absent register invites a second one.
    views[MATRIX_MD] = render_matrix(reqs, load_assessment())
    stale = []
    for path, text in views.items():
        current = open(path, encoding="utf-8").read() if os.path.exists(path) else None
        if current != text:
            stale.append(os.path.relpath(path, ROOT))
            if not check:
                with open(path, "w", encoding="utf-8") as f:
                    f.write(text)
    if check and stale:
        print("stale blueprint views (re-run scripts/render-blueprint-docs.py): " + ", ".join(stale))
        return 1
    print(("up to date: " if check else "rendered: ") + ", ".join(os.path.relpath(p, ROOT) for p in views))
    return 0


if __name__ == "__main__":
    sys.exit(main())
