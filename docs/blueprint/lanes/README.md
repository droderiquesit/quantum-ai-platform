# Lanes: the standing rules every lane session follows

The lane plan is `triage-2026-10-18.json` in this directory: every PARTIAL
register row graded on 2026-10-18 and the closable ones clustered by crate and
primary file, so sessions running in parallel edit disjoint code and meet only
in the register files. A lane closes a row only when the behaviour runs on a
default path or a committed deployment and a mutated test proves it. A row a
lane cannot close gets its gap corrected instead. This file is the brief; a
lane's own prompt names only its lane key and its neighbours.

## Reading your rows

```
python3 -I -c "import json; d=json.load(open('docs/blueprint/lanes/triage-2026-10-18.json')); print(json.dumps([l for l in d['lanes'] if l['lane']=='LXX'][0], indent=1))"
```

Top-level keys other than `lanes` (`already_complete`, `owner_switch`,
`design_adr`, `deploy_dependent`, `real_missing_large`) are read the same way
by key. Each row has `id`, `unmet_clause`, `closure` (a grader's proposal,
read and not run) and `files`. The grader may be wrong; the code is the
authority.

**Before starting a row, re-read it in the register**
(`grep -l '"R12-XXX-NNNN"' docs/blueprint/assessment/*.json`). Skip it if it
is already COMPLETE, or if a lane commit today already touched it
(`git log --oneline --since=2026-10-18T04:00 -S '"R12-XXX-NNNN"' -- docs/blueprint/assessment`):
an earlier session on your lane may have closed it or corrected its gap.

## Wave 9: lanes that triage their own rows

`wave9-2026-10-19.json` holds the 761 open rows that no ADR-c lane
(`adr-c-2026-10-19.json`) and no running lane owned on 2026-10-19. It splits
them into 26 lanes by the crate each row's text names most. Nobody graded these
rows against the code first, so a W9 lane does that itself. Read your rows with:

```
python3 -I -c "import json; d=json.load(open('docs/blueprint/lanes/wave9-2026-10-19.json')); print(json.dumps([l for l in d['lanes'] if l['lane']=='W9-XXX'][0], indent=1))"
```

Before implementing, give every row exactly one class from the code as it now
stands. Write the classes to `docs/blueprint/lanes/<lane>-triage.json` as
`[{"id", "class", "reason"}]`, where `reason` cites a `path::symbol` or a grep
result. Push the file before the first code commit.

- `CODE_CLOSABLE`: closable here by one lane. Close it per "Per row" below.
  Do these first: MISSING and INCORRECT before PARTIAL.
- `LARGE_SPLITTABLE`: in-repo but bigger than one lane. Do one slice, keep the
  row PARTIAL, and list the remaining slices in its `gap`.
- `PROTECTED_PATCH`: needs a protected path. The owner approved such patches
  on 2026-10-19, so write the exact diff to
  `docs/blueprint/lanes/<lane>-protected.patch`. The parent session applies
  it. Keep the row PARTIAL until then.
- `RISK_TIGHTEN`: needs a new or stricter control. The owner authorised
  tighten-only changes on 2026-10-19. Prove the control fires, and change no
  existing limit value. A loosening is never in scope.
- `BILLED`, `DEPLOY_DEPENDENT`, `EXTERNAL_BLOCKED`, `DESIGN_OPEN`: finish any
  in-repo part. Then make `gap` say exactly what is missing (the resource and
  its cost, the deployment evidence, the third party, or the open question),
  dated 2026-10-19.
- `FORBIDDEN_LIVE`: needs live orders or real capital. Never closable. Say so
  in `gap`.
- `ALREADY_COMPLETE`: regrade only after running the cited test and
  mutation-verifying it on the production caller.

## Per row

1. Read the code and tests the closure names with `grep -n` and `sed -n 'a,bp'`
   ranges. Never read a large file whole (`platform.rs`, `central/plane.rs`,
   `cell.rs` are several thousand lines). For "where is X" use the
   `caveman:cavecrew-investigator` agent if it is available, so the file dumps
   stay out of your context.
2. Implement the closure on the default path, so the behaviour runs in a real
   binary or a committed deployment, not only in a test.
3. Add a test named as a full sentence stating the property, asserting its own
   premise first. Substring matching is a trap: match delimited tokens.
4. **Mutation-verify every new test**: break the implementation, run the test,
   confirm it fails for the right reason, restore byte for byte, confirm it
   passes. Record the mutation in one line.
5. Re-grade the row honestly: `status` COMPLETE only when the clause is met on
   a running path and a mutated test proves it; update `current`, `evidence`
   (file:line), `gap` (empty when COMPLETE), `verification` (the exact test
   command), `notes`. If the closure is wrong, needs a protected path, a paid
   resource, a credential or a licence, or exceeds what the row asks, correct
   `gap` and `notes` to say precisely what is missing and keep PARTIAL.
6. Never inflate a status. Never weaken, skip, `#[ignore]` or delete a test to
   pass. Leave rows you did not reach exactly as they were. Do not edit
   `docs/blueprint/v12-completion-plan.md`; the parent session recounts it.

## Boundaries

- Branch `ccr-d1bc1e54-kf6mr0` only. Never rebase, amend, force-push, open a
  pull request or merge to main.
- Protected paths, never touched by a lane unless its prompt names one:
  `.github/**`, `.claude/**`, `CLAUDE.md`, `scripts/agent-*`, `scripts/check-*`,
  `scripts/sast-*`, `scripts/dependency-allowlist.txt`,
  `infrastructure/terraform/variables.tf`,
  `infrastructure/terraform/execution-nodes-enabled.tf`,
  `infrastructure/terraform/build-pool.tf`, `infrastructure/environments/**`,
  `docs/adr/**`. One exception (ADR 0124 §1): a NEW `docs/adr/0*.md` plus its
  `docs/adr/README.md` index row, with the number claimed in its own pushed
  commit after re-reading the index. Never edit an existing ADR. If a row
  needs a protected path, put the exact patch in your report.
- Paper trading is absolute: never create, enable or ease a live-order path,
  never weaken `AutonomyLevel::deployable`, the Terraform refusal or the
  `Cell` constructors.
- No new crate (ADR 0109). No `unwrap`/`expect` outside tests. `Decimal` for
  money. `BTreeMap` where order reaches output. An explicit timeout on every
  call that leaves the process. `std::env` only under `backend/crates/apps/**`.
- If a permission layer refuses an action, do not route around it: say so in
  your report.

## Gates

Every cargo call, from `backend/`:

```
export CARGO_TARGET_DIR=$HOME/.cache/lane-target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cargo +1.94.1 fmt --all --check
cargo +1.94.1 clippy --workspace --all-targets -- -D warnings
cargo +1.94.1 test -p <each touched crate> --no-fail-fast
```

Before the final push only: `cargo +1.94.1 test --workspace --no-fail-fast`,
reading every `test result:` line. From the repository root:
`./scripts/check-secrets.sh` must say nothing found and
`./scripts/check-dependencies.sh` must say all permitted. Then
`cargo +1.94.1 run -q -p qip-cli -- blueprint render --root ..` and
`cargo +1.94.1 run -q -p qip-cli -- blueprint check --root ..`; commit what the
render writes. A gate that did not run is reported as not run, with the reason.

## Push discipline

Commit in slices of two to four closed rows. Before each push:
`git fetch origin ccr-d1bc1e54-kf6mr0 && git merge --no-edit origin/ccr-d1bc1e54-kf6mr0`.
On a conflict in the four generated docs (`docs/blueprint/requirements.md`,
`docs/blueprint/traceability-matrix.md`, `docs/architecture/current-state.md`,
`docs/blueprint/v12-completeness-targets.md`) take either side and re-run the
render. On a register JSON conflict keep both sides' row edits. On a test
file where both sides appended tests keep both. Re-run the touched crates'
tests after the merge, then `git push -u origin ccr-d1bc1e54-kf6mr0`; on a
non-fast-forward rejection fetch, merge and push again. Many sessions share
the branch.

Commit messages are argumentative prose in the register of `git log`: the
failure the change prevents, then the evidence (test names, the mutation that
fired, the `test result:` line). No model identifier anywhere. End each
message with the two trailers your own session's system reminder gives you.

## Economy

Answer in caveman mode: terse, every technical fact kept, no ceremony. Locate
by symbol, read ranges, run the full workspace suite at most once, spawn no
sub-agent for what a few shell calls do. When your context passes about 70
percent, stop at a clean point, push, and report the rows not reached.

## Report

Your last message, under 400 words: rows moved to COMPLETE with ids; rows kept
PARTIAL with the corrected gap in one line each; tests added with the mutation
that fired; each gate's decisive line (or "not run" and why); commit shas
pushed; rows not reached; any protected-path patch, verbatim.
