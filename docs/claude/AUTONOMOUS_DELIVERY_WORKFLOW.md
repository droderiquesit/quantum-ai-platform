# The autonomous delivery workflow

How a stated outcome becomes verified change. Driven by
`.claude/agents/chief-orchestrator.md`.

## Starting it

```
Use the chief-orchestrator agent to deliver: <the outcome, in one sentence>.
Follow the vision-to-plan skill, then implement, and stop only on a genuine
blocker.
```

## The loop

```
   interpret ──► inspect ──► assumptions, risks, criteria
        │                              │
        ▼                              ▼
   task graph ──► assign ──► parallel where independent
        │                              │
        ▼                              ▼
   integrate in dependency order ──► run the gates
        │                              │
        │                              ▼
        │                        independent review
        │                              │
        └────── repair and re-run ◄────┘
                       │
                       ▼
        docs and ADRs ──► learnings ──► evidence report
```

## Parallelism

Independent work runs concurrently; dependent work does not. The rule that
matters: **two agents must never hold the same file.** Each brief names its own
allowed paths *and* the paths other agents hold, so each knows what not to
touch. This is the failure mode that costs most — overlapping edits destroy
work that then has to be redone, and the loss is discovered late.

## Gates

Applied by risk, not by ritual. Every gate that applies runs; every gate
omitted is named with its reason.

| Gate | Command |
|---|---|
| Build | `cargo build --workspace` |
| Format | `cargo fmt --all --check` |
| Lint | `cargo clippy --workspace --all-targets` |
| Unit + integration | `cargo test --workspace --no-fail-fast` |
| Contract / acceptance | `cargo test -p qip-acceptance` |
| Dependency policy | `./scripts/check-dependencies.sh` |
| Secrets | `./scripts/check-secrets.sh` |
| Infrastructure | `terraform fmt -check`, `terraform validate` |
| Frontend | `npm run lint`, `npm run build` |
| Hooks | `python3 .claude/hooks/test_hooks.py` |
| Everything | `make check` |

## When the loop may stop

Only on satisfied acceptance criteria, or a genuine blocker: missing
credentials, missing authority, external coordination, or a material product
decision.

**Not blockers.** Failing tests, lint errors, compile errors, merge conflicts,
implementation defects, and a subagent that died mid-task. A dead subagent's
work is in the tree; finish it or park it on a WIP branch, but never discard it
and never leave the tree broken.

## The evidence report

Every run ends with:

1. What was asked, restated as something checkable.
2. What changed, as what the system now does or refuses that it did not.
3. Files created and modified.
4. Commands executed, with quoted output for each gate.
5. Gates skipped, each with its reason.
6. Assumptions made, and how to reverse them.
7. Risks and residual uncertainty.
8. Remaining work and its owner.
9. The rollback plan.

A report that omits a failure is not an optimistic report. It is a false one,
and every decision made downstream of it inherits the error.
