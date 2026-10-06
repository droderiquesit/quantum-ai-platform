# Blueprint delivery status

**Snapshot: 2026-10-06, orchestration session.** This file replaces an earlier
version that reported work as merged, running and percentage-complete when it
was none of those. What that version got wrong is listed at the end so nobody
quotes it.

## Where the numbers come from

The register of record is `docs/blueprint/assessment/*.json`, rendered into
`docs/blueprint/traceability-matrix.md` (ADR 0099). Recount rather than quote:

```
python3 - <<'EOF'
import json,glob,collections
c=collections.Counter()
for f in glob.glob('docs/blueprint/assessment/*.json'):
    d=json.load(open(f))
    rows=d if isinstance(d,list) else d.get('assessments') or d.get('requirements') or d.get('rows') or []
    c.update(r.get('status') for r in rows if isinstance(r,dict))
print(c, sum(c.values()))
EOF
```

On this snapshot it printed 1,601 rows:

| Status | Rows | Share |
|---|---:|---:|
| COMPLETE | 333 | 20.8% |
| PARTIAL | 648 | 40.5% |
| MISSING | 374 | 23.4% |
| BLOCKED (live capital / external action — refused by design) | 166 | 10.4% |
| INCORRECT | 56 | 3.5% |
| NEEDS-VALIDATION | 17 | 1.1% |
| OBSOLETE | 7 | 0.4% |

Each ID appears in exactly one assessment file (1,601 unique IDs), so the shares are of distinct requirements.

## What is verified on `ccr-0c1bacf8-kla0dd`

- `cargo test -p qip-acceptance --test m5_critical_path --no-fail-fast`:
  `test result: ok. 8 passed; 0 failed; 0 ignored`.
- The branch is 27 commits ahead of `main` and has **not** been merged. PR #21
  (M5) was closed without merging. No PR is open.
- The full gate (`make check`) has **not** been run on this branch in this
  snapshot. One worker session has been asked to run it and report.

## How work is organised now

Each worker session takes 1–3 PARTIAL/MISSING/INCORRECT rows in its scope,
works on its own branch, and opens a **draft** PR against
`ccr-0c1bacf8-kla0dd` with the moved IDs and quoted gate output. Nothing is
merged without the owner's explicit approval per PR. Progress is measured by
rows moving to COMPLETE in the register, not by sessions or "teams".

## Retracted claims from the previous version

- "M5 MERGED", "M6 75%", "Overall 49%" and every per-stage packet count: not
  measured; withdrawn.
- "1500 teams": each session is one agent; there were never teams of twenty.
- "M6 A4 executing (120 teams)": every session was idle awaiting input.
- The wave timeline ending 04:30 UTC 2026-10-07: invented; withdrawn.
