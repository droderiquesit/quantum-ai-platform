# ADR 0098: The development factory is the orchestration policy run on the owner's desktop, with Claude models as its only tiers, and the GCP worker fabric waits on a measured rung

- **Status**: Proposed, 2026-09-25. The four choices it records were the
  owner's, made in session on 2026-09-25 when asked (toolchain: install;
  plugins: the official vetted set; routing: Claude only; GCP fabric: defer,
  ADR only). What remains proposed is the shape around them.
- **Date**: 2026-09-25
- **Supersedes**: nothing. **Amends** nothing in
  `docs/plan/algorik-orchestration-policy.md`; it adds one measurement to
  its §1 and applies its §3–§5 as written.
- **Related**: ADR 0022 (the blueprint is the architecture of record), ADR
  0024 and ADR 0093 (Kubernetes retired, then the one cluster suspended on
  cost), ADR 0037 (the hosted model provider, still dark), ADR 0002 and ADR
  0009 (dependencies), ADR 0003 (paper trading — not touched, see the last
  section).

## Context

The owner asked for an "AI development factory" to carry the next
blueprint-driven refactor: a lead Claude as architect and integrator, a
planning chain producing a dependency graph of small task packets, a
cost/risk router sending each packet to the cheapest tier that can do it,
and a worker fabric on Google Cloud — Cloud Run Jobs, Google Batch on Spot,
GKE Autopilot serving open models through vLLM — scaling to a thousand
logical workers. The proposal named some thirty plugins and tools to adopt.

**Most of that factory already exists in this repository, and the proposal
did not know it.** `docs/plan/algorik-orchestration-policy.md` already
holds: a ladder of rungs (8 → 16 → 32 → 64 → 100) climbed only on a ≥10%
gain in *verified output per token* (§2); the list of work Claude keeps and
the one-question test for what may be routed to a cheaper model (§3); a
six-gate authorisation for any third-party model provider (§4); a worker
contract that is the proposal's "task packet" in all but name — task,
context, token limit, acceptance criteria, exclusive paths (§5); isolation,
anti-duplication, "workers may not spawn workers", budgets with automatic
pause, and a handoff format (§6–§9). `chief-orchestrator` and the
`vision-to-plan`, `implement-slice`, `test-change` and `security-review`
skills are its drivers. `scripts/model-gateway.mjs` is the router's
fail-closed front end.

What was missing was not design. It was **a machine the design can run
on** and **a decision about which parts of the proposal to take.**

The machine, measured on 2026-09-25 before anything was installed: no
`cargo`, no `rustc`, no `node`, no `gh`, no `rg`, no `docker`, no
`gcloud`. Every gate in the policy's §10 was unrunnable, so every worker
output would have been unverifiable — which under §2's formula is zero
accepted output at full token cost, however many workers ran.

## Decision

### 1. The owner's desktop is the worker fabric, and its measured ceiling is 10

`scripts/worker-capacity.sh` on the desktop, 2026-09-25:

```
cpus                     12
memory available         21248 MB
disk available           118823 MB
concurrent workers       10   <- binding constraint: cpu (12 cores, 2 reserved)
isolated worktrees       169   (at 700 MB each)
```

So the ladder starts at **8**, the highest rung not exceeding 10. Disk,
which bound the old container to one worktree, is no longer a constraint.

**One precondition is not met, and until it is the verified ceiling is 0,
not 8.** The Rust toolchain pinned by `backend/rust-toolchain.toml`
(1.94.1, rustfmt, clippy, rust-analyzer) is installed under `~/.cargo`,
but the machine has **no C linker**, and `cargo build` cannot link a binary
without one. Installing it needs `sudo` (`sudo apt install build-essential`),
which is the owner's to run. Node 22 (matching CI's `node-version: 22`),
`gh` and `ripgrep` are installed under `~/.local`, each from its upstream
release checked against the published SHA-256.

### 2. The tiers are Claude models and deterministic tools; nothing leaves for another provider

| Tier | Work | Executor |
|---|---|---|
| T0 | search, AST, format, lint, build, test, scans | no model — `rg`, `cargo fmt`/`clippy`/`test`, the check scripts |
| T1–T2 | exploration, fixtures, mechanical refactors, doc drafts | a cheaper Claude model as a subagent |
| T3 | difficult implementation and debugging | a stronger Claude model |
| T4 | architecture, security, integration, merge | the lead session |

A packet's tier ceiling is set by §3's question — *if this comes back
wrong, does something fail loudly?* — and not by how routine it looks. §3's
reserved list (the paper-trading boundary, authentication, credentials,
risk, execution, cross-cutting integration, every merge) stays with the lead
at T4 whatever its size.

**No packet goes to a non-Anthropic model.** §4's gates stand unchanged;
Hugging Face stays blocked on its credential and on reading each resolved
provider's terms, as ADR 0037 left it. OpenCode Delegate, Claude Code
Router and free model pools are not adopted, because each is a way of
sending repository source to a provider §4 has not cleared, and "free" is
§4's named example of a tier that trains on its input.

### 3. The task packet is §5's contract plus two fields

§5 already requires task, context, token limit, acceptance criteria and
exclusive paths. A packet adds **model ceiling** (the tier from decision 2)
and **escalate if** (the condition under which the worker stops and hands
back rather than widening its own scope). No new file format and no new
directory: a packet is the brief `chief-orchestrator` already writes.

### 4. Plugins: the official set at user scope, three of it left out, no community plugins

Installed at user scope from `anthropics/claude-plugins-official`:
`rust-analyzer-lsp`, `typescript-lsp`, `skill-creator`, `plugin-dev`,
`hookify`, `code-review`, `pr-review-toolkit`, `commit-commands`,
`feature-dev`, `claude-md-management`, `context7`, `serena`, `playwright`.
**User scope, not project scope**, so nothing in `.claude/settings.json`
changes and no agent in another checkout inherits them without its own
review.

Left out of the official set, each for a reason a reader can check:

- **`github`** — its MCP server is GitHub Copilot's endpoint with a
  personal access token read from `GITHUB_PERSONAL_ACCESS_TOKEN`, an
  environment value, which is the pattern `01-security-and-safety.md`
  refuses. The session already has a GitHub connector, and `gh` is now
  installed.
- **`security-guidance`** — it runs an LLM review of the git diff on every
  `Stop`, which spends premium tokens on every turn of every session. That
  is the opposite of the cost goal, and `security-engineer` and the
  `security-review` skill already do the same review when a change warrants
  one.
- **`terraform`** — its MCP server runs in Docker, which is not installed.

Two that were installed carry unpinned upstream code and are named so the
choice is visible: `serena` runs `uvx --from git+https://github.com/oraios/serena`
at whatever commit is current, and `playwright` runs
`npx @playwright/mcp@latest`. `context7` sends library-documentation
queries to `mcp.context7.com`; a query is a library name and a question,
and a packet's source should not be pasted into one.

Not adopted at all: `superpowers` and `semgrep` (not in the official
marketplace on this machine), `claude-mem` (a second memory beside the
event log, ADRs and the plan — "no second source of truth"), `repomix` (a
packed copy of the repository handed to every worker is the "read the
repository" context §5 forbids), and the delegation and routing tools under
decision 2.

### 5. No second planning system

GitHub Spec Kit and Task Master are not adopted. The blueprint is already
the architecture of record (ADR 0022); `docs/plan/` and
`docs/architecture/deployed-vs-blueprint.md` already carry the gap analysis;
`vision-to-plan` already produces the dependency graph. A second tool that
keeps its own copy of the specification and its own task list would be two
registers of the same facts, and this repository has an ADR (0049) whose
whole subject is two registers disagreeing about one identifier.

### 6. The GCP worker fabric is deferred, with the conditions that reopen it

Nothing is provisioned. The fabric is reopened by a new record when **all
three** hold:

1. The ladder has climbed on measurement to the desktop's ceiling — six
   completed tasks at each rung, each climb a ≥10% gain in verified output
   per token, recorded in the task log as §2 requires.
2. At that ceiling, CPU is the constraint that binds, **with ready work
   queued** in the dependency graph. A ceiling that binds on an empty queue
   is not a reason to buy capacity; it is a finished graph.
3. The owner has set a monthly cost ceiling for it.

When reopened, the starting shape is **Cloud Run Jobs in a separate
project**, with its own Terraform root outside `infrastructure/terraform/`.
Not GKE: ADR 0024 retired Kubernetes as the runtime, ADR 0093 suspended the
one cluster that came back because of its cost, and a GKE Autopilot pool for
development workers would repeat both mistakes for a workload that is short
and stateless. Not in the trading platform's project: a worker identity able
to push branches sits badly beside KMS keys, Binary Authorization and the
venue credential's secret. Shared inference (vLLM on GPUs) additionally
needs §4's gates answered for a self-hosted model — running a model is not
a permission to send it the source — and a GPU cost line, so it is the last
piece and not the first.

## What it costs

- **Nothing runs until the linker is installed.** Every Rust gate fails at
  the link step, and the failure reads as a toolchain error, not a code
  defect. It is the first line of the handoff for that reason.
- **One machine, one failure domain.** A desktop that sleeps, reboots or is
  in use for something else stops the factory. The Cloud fabric is what
  removes that, and decision 6 delays it on purpose.
- **Claude-only tiers cost more per token than a free pool.** The saving
  given up is real. The cost avoided — repository source, risk logic
  included, sent to providers whose terms nobody here has read — is not
  measurable in tokens, and §4 already decided it.
- **Plugins at user scope are invisible to review.** They are not in the
  repository, so a reviewer of a change cannot see which tools the author
  had. The list above is the only record.
- **Two unpinned MCP servers.** `serena` and `playwright` run whatever their
  upstream published last.
- **A thousand workers stays a slogan.** Ten is the measured ceiling. The
  proposal's own closing argument — that thirty well-partitioned workers
  beat a thousand overlapping ones — is the reason not to pretend otherwise.

## Alternatives rejected

- **Adopt the proposal as written.** It would add a second planner, a
  second memory, a model router sending source to uncleared providers, a
  Kubernetes cluster, and around thirty tools, most duplicating something in
  `.claude/` or `docs/plan/`. It also scales before measuring, which §2
  exists to stop.
- **Build the GCP fabric now, in this repository's Terraform.** It spends
  money before any rung has been measured on a machine that now has room for
  ten workers, and it puts development identities in the production
  project.
- **Install the plugins at project scope.** It changes `.claude/settings.json`
  for every agent in every checkout, including the unpinned servers. That can
  still be done later, plugin by plugin, once each has been used here.
- **Route T1–T2 to Hugging Face now.** Its credential is not seeded, and no
  resolved provider's terms have been read. The owner chose Claude only.

## What would make this wrong

- **The ladder not climbing.** If output per token falls from 8 to 16 on the
  desktop, the constraint is contention — files, review, merges — and more
  machines would make it worse. Decision 6 should then stay closed.
- **The C linker being refused.** If `build-essential` cannot be installed,
  the desktop is not a fabric, and a Cloud Run build worker becomes the
  first piece rather than the last.
- **A plugin changing what it sends.** `serena` and `playwright` are
  unpinned; a release that adds network calls would move them under §4.
- **The owner clearing a provider under §4.** Then decision 2's table gains
  a row, and this record is amended rather than overridden.

## Paper trading

Untouched at all three layers. No file under `infrastructure/terraform/`,
no composition root, and no type in `qip-edge` or `qip-cost-router` is
changed by this record. A development worker that can edit the repository
is still bound by `01-security-and-safety.md` and by the guard hook, and
§3 keeps every change near the boundary at the lead's tier.
