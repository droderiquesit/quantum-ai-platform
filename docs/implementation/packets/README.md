# Task packets

A packet is the unit a worker is dispatched on — the orchestration policy's §5
worker contract plus a model ceiling and an escalation trigger (ADR 0098 §3).
A worker gets its packet and nothing else: never the whole blueprint, never
"read the repository".

| Field | Meaning |
|---|---|
| `id`, `title`, `epic`, `wave` | Identity; `wave` 1 has no dependencies |
| `objective`, `why` | One checkable outcome, and the failure it prevents |
| `requirements` | Requirement IDs it advances (see [the catalogue](../../blueprint/requirements.md)) |
| `owns` | Files it may create or modify — **exclusive**: no concurrently runnable packet owns the same file |
| `reads`, `symbols` | Context it needs, with file:line |
| `depends_on` | Packets that must be merged first |
| `constraints` | ADR and rule constraints that bite this packet |
| `tests` | Tests it adds, each with the mutation that must make it fail |
| `acceptance` | Exact commands and the output that closes it |
| `tier`, `model_ceiling`, `token_budget` | T0–T4 and the most capable model it may use; a hard token ceiling |
| `escalate_if` | Conditions under which the worker stops and hands back instead of widening scope |

`slice.json` holds the 57 packets for ADR 0100's first working system. Every
worker commits on its own branch `slice/<ID>` in its own worktree with its own
`CARGO_TARGET_DIR`; an independent reviewer checks the diff against `owns` and
re-runs the acceptance before the lead merges in DAG order and re-runs the
workspace.
