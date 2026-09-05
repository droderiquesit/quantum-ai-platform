# Algorik — worker orchestration policy

**Status:** active. **Amends** `docs/plan/algorik-instruction-precedence.md`;
supersedes the three-agent cap recorded there under "Required specialist
capabilities". **Restarts nothing** — slices 1 and 2 are complete and stand as
committed, slice 3 continues under this policy from its current point.

The objective is up to 100 workers doing *independent, verifiable* work. Not
100 workers. The distinction is the whole document: every rule below exists to
stop concurrency from becoming token spend with no accepted output behind it.

## 1. Capacity is measured, not declared

`scripts/worker-capacity.sh` prints what the machine sustains and which limit
binds. Run it before raising concurrency and after any container change. The
operating point is its output; the ladder in §2 is a target the measurement
either permits or refuses.

**Measured on this container, 2026-08-30:**

```
cpus 4 · memory available 15020 MB · disk available 961 MB
concurrent workers       2   <- binding constraint: cpu (4 cores, 2 reserved)
isolated worktrees       1   (at 700 MB each)
```

So the honest position: **this container runs 2 concurrent workers.** The
requested floor of 8 is not reachable here, and 100 is two orders of magnitude
away. Two cores are reserved on purpose — one for the orchestrator's own turn,
one for the builds and test runs workers trigger, which are what actually
saturate a four-core box. Declaring 8 here would not produce 8 workers; the
harness would queue six of them, and queued workers still hold context,
still cost tokens on their prompts, and produce nothing while they wait.

Disk is the second wall. At 961 MB free — `target/` alone is 26 GB — there is
room for **one** isolated worktree. Any plan needing more isolation than that
is blocked on disk before it is blocked on anything else.

**This is a container limit, not a policy limit.** On a 64-core box with disk,
the same script returns 16 (the harness cap per workflow), and the ladder below
climbs on its own. Nothing in this policy needs editing to scale; the script is
the thing that changes its answer.

## 2. The ladder, and the gate between rungs

Rungs: **8 → 16 → 32 → 64 → 100**, starting at whichever is highest without
exceeding `worker-capacity.sh`. On this container that start is 2.

A rung is climbed only when the previous rung **improved verified output per
token**. That phrase is made measurable rather than left to judgement:

```
verified output per token  =  accepted diff lines / output tokens spent
```

where *accepted* means the change survived every gate in §8 and was committed.
Work that was written and reverted, reviewed and rejected, or produced a test
that had to be weakened counts as zero output and full token cost — which is
the point, because that is exactly the failure mode of over-parallelising.

Rules for the gate:

- Measure over at least **six completed tasks** at a rung. Fewer is noise.
- Climb only on a **≥10% improvement** over the previous rung's ratio. A rung
  that improves by 2% is inside the measurement error of six samples.
- On a *decline*, drop one rung and stay there. Ratios fall when workers start
  contending — for files, for the disk, for the reviewer's attention — and the
  cheapest response is to stop adding them.
- Record each rung's ratio in the task log. A ladder with no recorded rungs is
  a ladder nobody can tell is working.

## 3. What Claude does, and what may be routed elsewhere

**Claude retains, without exception:** orchestration and planning; architecture
and ADRs; anything touching the paper-trading boundary, authentication,
credentials, risk or execution; difficult debugging; cross-cutting integration;
and final approval of every merge.

**Routable to a lower-cost or local model** — bounded, low-risk, verifiable by
a machine rather than by judgement: mechanical refactors with a compiler check
behind them; docstring and comment drafting; test-fixture generation; format
and lint fixes; translating one well-specified shape to another; enumerating
call sites.

The test for routability is one question: **if this comes back wrong, does
something fail loudly?** A rename that breaks the build is routable. A change
to a limit check is not, because a wrong one passes the build and fails in
production. That is the same three-part test ADR 0012 applies to dependencies,
applied to labour.

**Current routing state: everything routes to Claude, for one reason —
there is no credential.** `scripts/model-gateway.mjs` now exists and fails
closed without one. Everything else is in place; see §4 for the gate-by-gate
position.

**The provider is chosen: Hugging Face Inference Providers**, on the
account holder's instruction of 2026-09-04 ("use huggingface for
development"). The gateway carries it as the `huggingface` preset — base URL
fixed to `https://router.huggingface.co`, credential read from
`HF_TOKEN_FILE` (preferred) or `HF_TOKEN`, a Hugging Face token shape added
to the payload screen — and `node --test scripts/model-gateway.test.mjs`
holds all of that (eight tests, each mutation-verified). The router is a
front for third-party inference providers chosen per model, so
`node scripts/model-gateway.mjs --probe` prints, keylessly and sending
nothing, which providers a model resolves to; gate 4 is answered per
provider on that list, not once for the router.

## 4. Provider authorization gate

No repository code, diff, path, log, test output, prompt containing source, or
error message leaves this machine for a third-party model provider until **all
six** are established and written down:

1. **Availability** — the endpoint exists and is reachable from this network.
2. **Authorization** — the account holder has approved this specific provider
   for this repository's code. Approval for one provider is not approval for
   another.
3. **Credentials** — issued through the approved path (Secret Manager or the
   configured workflow), never pasted into chat, never in a committed file,
   never in an environment variable where a crash dump can hold it.
4. **Privacy** — the provider's retention and training position is known and
   acceptable for proprietary trading source. "Free tier" very often means
   "trains on your input", and this repository holds risk and execution logic.
5. **Expected cost** — priced per task class, with a ceiling, before the first
   call.
6. **Data classification** — which classes may be sent at all. Anything under
   `backend/crates/services/qip-risk-engine`, `qip-execution-engine`, `qip-capital`,
   `qip-compliance` or `backend/crates/edge` does not leave, at any price.

**Nothing is assumed free, open, self-hosted, or authorized because its name is
familiar.** Ollama and vLLM are local *runtimes*, not permissions — running a
model on someone else's hardware is still egress. LiteLLM is a proxy that
forwards to whatever sits behind it, and the thing behind it is the provider
that matters.

Until all six hold, the routing table in §3 resolves every row to Claude, and
no further work is blocked by that: the ladder still climbs, the workers still
run, they are simply all the same model.

### Gate status, 2026-08-30

| # | Gate | State | Evidence |
|---|---|---|---|
| 1 | Availability | **met** | HEAD probes, no code sent: `api.deepseek.com` → 401, `openrouter.ai` → 200, `api.moonshot.cn` → 404, `dashscope.aliyuncs.com` → 404. All four answered, so egress works. `api-inference.huggingface.co` → no response. |
| 2 | Authorization | **met** | Granted by the account holder in session, for this repository's source. |
| 3 | Credentials | **NOT MET — the only blocker** | No key for any provider. DeepSeek's 401 is the precise shape of this: reachable, and refusing us for want of a key. |
| 4 | Privacy | **met** | Decision made by the account holder: sharing this repository's source with an external provider is acceptable. Recorded here because a privacy decision that lives only in chat is one nobody can audit later. |
| 5 | Expected cost | **pending** | Cannot be priced until a provider and plan are chosen. `ALGORIK_WORKER_MAX_CALLS` is mandatory and the gateway refuses to start without it. |
| 6 | Data classification | **superseded, in part** | The account holder authorized sharing source, which is theirs to authorize. It does **not** extend to credentials: the gateway screens every payload and refuses — rather than scrubs — on any credential shape. Scrubbing would normalise sending files that need scrubbing. |

### Gate status, 2026-09-04 — Hugging Face

| # | Gate | State | Evidence |
|---|---|---|---|
| 1 | Availability | **met** | `node scripts/model-gateway.mjs --probe` with the `huggingface` preset, from this container through the session proxy: `reachable yes`, `catalogue 137 model(s) listed anonymously`, `Qwen/Qwen2.5-Coder-32B-Instruct resolves to: nscale, featherless-ai`. The earlier `api-inference.huggingface.co` probe found the legacy host; the router is the current one. A keyless POST to `/v1/chat/completions` answers 401 — reachable, refusing for want of a key, the same shape as DeepSeek's. |
| 2 | Authorization | **met** | Account holder, in session, 2026-09-04, for this repository's source. |
| 3 | Credentials | **NOT MET — still the only blocker** | No token is present. It must not be pasted into chat (see below). The two paths that keep it out of the transcript: (a) in a local checkout, write it to a file outside the repository and set `HF_TOKEN_FILE` to that path; (b) for a Claude Code on the web session, add `HF_TOKEN` to the environment's variables in the environment settings before starting the session, which is the environment-variable path the gateway accepts with its stated warning. Either way `./scripts/check-secrets.sh` must still say nothing found, and the token never appears in a committed file. |
| 4 | Privacy | **per provider, not yet read** | The owner's decision to share source stands. The router forwards to a provider chosen per model; `--probe` names them. A model is used only after the terms of every provider it resolves to have been read. Not done for any model yet. |
| 5 | Expected cost | **pending** | `/v1/models` publishes per-provider input/output prices; a ceiling is set with `ALGORIK_WORKER_MAX_CALLS` before the first call, and the ledger counts. |
| 6 | Data classification | **as above** | The gateway refuses a credential-shaped payload; risk, execution, capital, compliance and edge sources do not leave at any price. |

**No local runtime is available and none can be installed.** `ollama`, `vllm`
and `llama.cpp` are all absent, and with 961 MB free there is no room for a
runtime, let alone weights. Local inference is not a route out of the
credential gate on this container.

### What a credential must not do

It must not arrive in chat. `.claude/rules/01-security-and-safety.md` forbids a
credential in code, logs, fixtures or commits, and a key pasted into a
transcript is in all of those by the time anyone notices. The gateway reads
`ALGORIK_WORKER_API_KEY_FILE` — a path to a file outside the repository — which
is the same `_FILE` indirection the platform uses for every other secret. The
environment variable is accepted and warns about itself, because a value in the
environment is in `/proc/<pid>/environ`, in every child process, and in every
crash dump.

## 5. The worker contract

No worker is dispatched without all five. A worker missing any of them is a
worker whose output cannot be judged, which is a worker that costs tokens and
produces an argument.

| Field | Requirement |
|---|---|
| **Task** | One outcome, stated as something checkable. Not a theme. |
| **Context** | The specific files and prior conclusions it needs — never "read the repository". |
| **Token limit** | An explicit ceiling. Exceeding it is a stop, not a warning. |
| **Acceptance criteria** | The exact command or observation that closes the task. |
| **Exclusive paths** | The files this worker may write. Disjoint from every concurrent worker's. |

Path ownership is checked before dispatch, not after. Two workers editing one
file is the most expensive mistake available here: it destroys work that then
has to be redone, and it is discovered late, in a merge.

## 6. Isolation

- **Disjoint paths, shared tree** — the default. Cheapest, and sufficient when
  ownership is genuinely disjoint.
- **Isolated worktree** — required when paths could overlap, when a worker runs
  a destructive command, or when it must build against a different state.
  Costs ~700 MB each; **capacity here is one**, so at most one such worker
  runs at a time until disk is reclaimed.
- **Branch per worker** — for anything that rewrites shared state. Merged by
  the orchestrator, never by the worker.

Workers never push. The orchestrator merges, and merging is where the §8 gates
run.

## 7. Anti-duplication, and the spawning rule

Four failures, each with a mechanism rather than an instruction:

- **Duplicate scans.** Discovery is done once and written to a repository
  document; workers are handed conclusions, not told to go and look.
  `docs/plan/algorik-instruction-precedence.md` exists for this reason.
- **Overlapping implementation.** Prevented by exclusive path ownership at
  dispatch (§5).
- **Repeated review.** A diff is reviewed once per dimension. A second review
  of the same dimension is only justified by a *changed* diff.
- **Uncontrolled spawning.** Workers may not spawn workers. Fan-out is the
  orchestrator's alone, which is what keeps the concurrency count knowable and
  therefore boundable.

## 8. Budgets and automatic pause

**Global:** a token ceiling for the programme; a wall-clock ceiling per slice;
a cost ceiling if any paid route is ever activated under §4.

**Per worker:** the token limit from §5; at most **one** retry, and only for a
failure that is demonstrably transient (a timeout, a lost connection) — never
for a wrong answer, because retrying a wrong answer usually buys a differently
wrong answer at full price; a wall-clock timeout after which the worker is
stopped and its partial output judged as-is.

**New workers pause automatically on any of:**

- a provider rate limit reported, in which case the fix is to wait, not to
  spawn more and hit it harder;
- a merge conflict anywhere in the working tree — new work on a conflicted tree
  compounds the conflict;
- a failing test or build on the integration branch — the first duty is to get
  back to green, and parallel work on a red tree is work that cannot be
  validated;
- 80% of any budget, which leaves room to finish what is running rather than
  stopping mid-task;
- disk below 2 GB free, which is where builds begin to fail in ways that look
  like code defects. **This container is at 961 MB and therefore already past
  that line** — see §10.

Pause stops *new* dispatch. Running workers finish and hand off; killing them
wastes what they have already spent.

## 9. Handoff

Every worker returns exactly this, and nothing longer:

```
Files changed:    paths, one per line
Tests run:        the commands, verbatim
Results:          the outcome line of each, quoted
Blockers:         what stopped it, or "none"
Deviations:       what it did differently from its brief, and why
```

An unquoted result is not a result. `.claude/rules/00-enterprise-governance.md`
applies to workers exactly as to the orchestrator: never report a check as
passed without having run it and read its output. A summary that omits a
failure is a false statement about the system, whoever wrote it.

## 10. Validation, and what actually gates a merge

Deterministic and machine-checked, in this order:

```
cd backend
cargo fmt --all --check
cargo clippy --workspace --all-targets        # zero warnings
cargo test --workspace --no-fail-fast         # --no-fail-fast is not optional
./scripts/check-dependencies.sh
./scripts/check-secrets.sh
terraform fmt -check && terraform validate    # infrastructure changes
npm run lint && npm run build                 # frontend changes
npx playwright test                           # behavioural changes
node scripts/generate-theme-css.mjs --check   # design-token changes
```

Plus, for every new test, the mutation duty from
`.claude/rules/architecture/01-testing-strategy.md`: break the implementation,
confirm the test fails for the right reason, restore byte-for-byte, confirm it
passes. A worker that adds an unverified test has not finished.

## 11. Standing operational risk

**Disk is at 98% (961 MB free); `target/` holds 26 GB.** This is already past
the pause threshold in §8 and it constrains the current work, not just future
concurrency: a `cargo build` or `npm install` can fail with ENOSPC in a way
that reads as a code defect. `target/` is a rebuildable cache and reclaiming it
is safe — it costs a full recompile, which is minutes, not work. Not reclaimed
unilaterally here because nothing is currently failing and a full workspace
rebuild is expensive; raised so the decision is made deliberately rather than
discovered by a confusing build error.

## 12. What this changes about work in flight

Nothing restarts. Slices 1 and 2 are committed and stand. Slice 3 — the local
authentication vertical slice — continues from where it is, under this policy.
The practical effect on it is small and specific: at a measured ceiling of 2,
its remaining work runs as the orchestrator plus at most one or two bounded
workers with disjoint paths, which is roughly how it was already proceeding.
