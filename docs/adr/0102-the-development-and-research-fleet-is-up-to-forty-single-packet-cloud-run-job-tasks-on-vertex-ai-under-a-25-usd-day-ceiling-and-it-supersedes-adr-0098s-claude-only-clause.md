# ADR 0102: The development and research fleet is up to forty single-packet Cloud Run Job tasks calling Vertex AI under a 25 USD/day ceiling, and it supersedes ADR 0098's Claude-only clause

- **Status**: Proposed, 2026-10-04. The ceiling, the project, the count and the
  choice of Vertex AI with free or free-tier models are the owner's, given in
  session on 2026-10-04. What is proposed is the shape around them, and every
  fact about Vertex AI below is either marked observed or marked UNPROVEN —
  see "What was and was not observed".
- **Date**: 2026-10-04
- **Supersedes**, in ADR 0098 and in that part only: decision 2's sentence "No
  packet goes to a non-Anthropic model" and its tier table's *Executor*
  column for T1 and T2; the first and second of decision 6's three conditions
  for reopening the GCP fabric; and decision 6's "not in the trading
  platform's project". Decision 6's third condition (an owner cost ceiling) is
  met by this instruction. **Amends** nothing else in ADR 0098 and nothing in
  `docs/plan/algorik-orchestration-policy.md`, whose §4 gates, §5 contract and
  §8 budgets apply to the fleet as written.
- **Related**: ADR 0098 (the factory this extends), ADR 0037 (Hugging Face,
  still dark, and not touched), ADR 0093 and ADR 0024 (no Kubernetes), ADR
  0091 (binaries composed from libraries — the fleet adds none), ADR 0099 and
  ADR 0101 (billing on the dev project is recorded there as disabled, C8),
  ADR 0053 and ADR 0069 (capability nothing consumes is not declared), ADR
  0002 and ADR 0009 (dependencies), ADR 0003 (paper trading — not touched, see
  the last section).

## Context

ADR 0098 decided that the owner's desktop is the worker fabric, that tiers are
deterministic tools and Claude models, and that a GCP fabric waits on three
conditions: the ladder climbed to the desktop's ceiling on measurement, CPU
binding with ready work queued, and an owner cost ceiling. It chose Cloud Run
Jobs over GKE and a separate project over this one.

On 2026-10-04 the owner asked for up to forty low-cost agents on GCP, calling
Vertex AI with the most highly rated free or free-tier models, to develop,
test and operate the roadmap, under 25 USD a day in `algorik-platform-dev`.
That is an instruction, and it overrides two of ADR 0098's three conditions
and its project choice. It does not override anything ranked above it in
`docs/plan/algorik-instruction-precedence.md`: the safety rules are rank 2,
and a request to weaken them is refused wherever it arrives.

Three facts about the ground the request lands on, each checked in this tree:

- **The target project has the trading platform's dev environment in it.**
  `infrastructure/environments/dev/terraform.tfvars:36` sets
  `project_id = "algorik-platform-dev"`. ADR 0098 put workers elsewhere because
  an identity able to produce branches "sits badly beside KMS keys, Binary
  Authorization and the venue credential's secret". The owner has chosen this
  project, so the separation has to be rebuilt out of identities rather than
  out of a project boundary. Decision 3 does that.
- **Vertex AI is not enabled; billing is on.** `enable_vertex_ai = false` in
  all four environments' tfvars. On 2026-10-04 `algorik-platform-dev` was
  created and `gcloud billing projects describe` printed `billingEnabled: True`;
  the older ADR 0099 and ADR 0101 statements that dev billing is disabled were
  about the retired `algorik-dev` project. `gcloud services list --enabled`
  on the new project showed none of `aiplatform`, `run`, `artifactregistry` or
  `secretmanager` enabled.
- **The model router in the tree is not the fleet's.** `qip-cost-router`
  routes the platform's own decisions by intelligence tier and
  `Determinism`; no crate named `qip-model*` exists in this tree. The only
  code that sends a worker packet to a model is `scripts/model-gateway.mjs`:
  fixed provider presets, a credential screen that refuses rather than
  scrubs, a mandatory call budget, and a spend ledger. The fleet extends the
  gateway. It does not import the platform's router, and nothing in
  `backend/crates/` may import the gateway.

## What was and was not observed

**This record was drafted by a session that had file tools and no shell.** It
could read and write the repository and could not run `gcloud`, `cargo` or
`node`. So no Vertex AI fact was observed, and the register below is the
honest one rather than a hopeful one.

| Claim | State |
|---|---|
| Project id is `algorik-platform-dev` | **Observed** in `terraform.tfvars:36` |
| `enable_vertex_ai` is false in dev, test, stage, prod | **Observed** in the four tfvars |
| Billing on `algorik-platform-dev` | **Observed** 2026-10-04: `billingEnabled: True`, account `012F9F-AC0200-6FDF18` |
| `aiplatform.googleapis.com` is enabled in the project | **Observed enabled** 2026-10-04, by this record's step 2 (see the appendix); `run`, `artifactregistry` and `secretmanager` are not |
| Which models the project can call, their names, regions, quotas | **Observed for one**: `google/gemini-2.5-flash-lite` answered in `global` on 2026-10-04 (appendix). Quotas and every other model remain UNPROVEN |
| Any model's price, free tier, or data-use and retention terms | **Observed** 2026-10-04 for Google's models from Google's own pricing and data-governance pages (appendix): priced per token, no free tier, no training without permission, 24-hour in-memory cache. Per-publisher terms for partner and open models remain UNPROVEN |
| Any public "rating" of any model | **UNPROVEN**, and not wanted — see decision 4 |
| Vertex AI exposes an OpenAI-shaped chat endpoint the gateway can use unchanged | **Observed** 2026-10-04: one chat-completions call returned `ready` (appendix). The gateway itself has not made the call yet; its `vertex` preset is step 3 |
| A budget alert can stop spend | **UNPROVEN**, and assumed false — see decision 5 |
| `make check` or the documentation suite passes on this record | **Not run here**; see the handback |

The first slice's first step is to turn the UNPROVEN rows into observations
and to write them into this record's appendix before anything else is built.
A model name in this ADR would be an assertion; there are none.

## Decision

### 1. Forty is a concurrency ceiling on single-packet tasks, not forty resident agents

An "agent" is **one Cloud Run Job task that takes one packet, makes at most
the packet's budgeted calls to one model, writes one output object and
exits.** Nothing is resident, nothing polls, nothing holds state between
packets, and a task has no shell, no repository checkout, no git credential
and no tool other than the gateway. A worker is a function from a packet to a
patch or a finding; applying either is somebody else's act.

That is deliberate. A fleet whose members can run commands is a fleet whose
prompt-injection surface is the whole repository, and ADR 0098's rule that
repository content is data, not instructions, is only enforceable if a worker
has nothing to execute with.

**Forty is the Job's parallelism cap. It is not the starting point.** The
policy's §2 ladder governs: start at 8, climb on at least six completed
packets per rung and a gain of at least 10% in accepted diff lines per output
token, and drop a rung on a decline. The owner set the ceiling; the ladder
decides how much of it is used, because a ceiling reached without
measurement buys spend and no accepted output. The ladder is on the policy's
rungs, 8, 16, 32, then 40 as the last step, which is not a policy rung
(the policy's are 8, 16, 32, 64, 100), so the climb past 32 is recorded as a
cap, not a rung.

**The roster is slots by role, not forty named processes.** The totals are a
budget partition, and a role with no ready packets leaves its slots idle and
unbilled.

| Role | Slots | Reads | Produces | Model ceiling |
|---|---|---|---|---|
| `scout` — enumerate call sites, list files, inventory | 6 | the packet's named files | a list | T1 |
| `fixture` — generate test fixtures from a stated shape | 6 | the shape and one example | fixture files | T1 |
| `drafter` — doc comments, runbook and register prose | 4 | the packet's files | a diff on documents only | T1 |
| `refactor` — mechanical renames and shape translations with a compiler behind them | 6 | the packet's files | a patch | T2 |
| `tester` — write a test for a stated property | 6 | the unit and the property | a test file | T2 |
| `reviewer` — one dimension of one diff (read-only) | 6 | the diff | findings | T2 |
| `ops-watch` — triage a CI log, summarise a cost ledger, flag drift against the matrix | 4 | a log or ledger object | a finding | T1 |
| `catalogue` — draft `DOMAIN-NNN` requirement entries from a section | 2 | the section | JSON entries | T1 |

The slots sum to 40. The eight roles are chosen by ADR 0098 §3's question —
*if this comes back wrong, does something fail loudly?* — and each produces
something a deterministic gate or a human reads before it counts: a compiler,
a test, a link check, the requirement catalogue's schema, or the lead. A role
that fails that question has no row. In particular **there is no `architect`,
`integrator`, `security`, `risk`, `execution` or `merge` role at any size**;
those are ADR 0098 §3's reserved list and stay with the lead session at T4 on
the desktop.

**Packets are ADR 0098's worker contract.** Task, context, token limit,
acceptance criteria, exclusive paths (policy §5), plus model ceiling and
escalate-if (ADR 0098 decision 3), plus three fields this record adds, each
because the fleet is otherwise unattributable: `packet_id` (unique, and the
name of the task's output object), `role` (from the table), and
`input_sha256` (of the exact bytes sent). Exclusive paths are checked at
dispatch against every other in-flight packet, as the policy requires, and a
`reviewer` or `ops-watch` packet declares none because it may write none.

### 2. Cloud Run Jobs, one image, no Kubernetes, no service

One Cloud Run Job per role family is not needed. **One Job, one image,
`--tasks` set from the dispatch, `--parallelism` capped at the ladder's
current rung and never above 40, `--max-retries` 0.** The role is a field in
the packet, not a deployment. Retries are the dispatcher's, because the
policy's §8 allows one retry only for a demonstrably transient failure and
the platform default of retrying a task blindly is exactly "retrying a wrong
answer buys a differently wrong answer at full price".

Cloud Run Jobs have no request concurrency, which is a Services concept; the
owner's "concurrency" is therefore **task parallelism**, and one task is one
packet. A per-task timeout is set from the packet's wall-clock limit and a
task that reaches it is stopped and its partial output judged as it is (§8).

Not GKE. ADR 0024 retired Kubernetes as the runtime and ADR 0093 suspended
the one cluster that returned, on cost; a short stateless job on an
Autopilot pool would repeat both mistakes. Not Compute Engine, because
nothing here needs a long-lived machine. Not a Cloud Run Service, because a
service is something that waits for work and therefore bills while it waits
and cannot be told apart from an idle forty-way pool.

**The code is one Node script and one Dockerfile, and it needs no new
dependency.** The gateway already uses Node's built-in `fetch` on purpose.
Two additions go in it: a `vertex` provider preset whose base URL is fixed
like `huggingface`'s, and a worker entry point that reads one packet from the
bucket and writes one output object. Both have the gateway's existing tests'
shape. If the preset has to speak a protocol the gateway cannot speak with
`fetch`, **that is a stop and a new ADR**, not an SDK: ADR 0009's reasoning
about hand-written Google clients applies in the other direction here too,
and adding Google's Node client to a gateway whose whole argument is that it
has no supply chain would unmake the argument.

### 3. Identity: the runtime's attached service account, no keys, and a fence built out of absences

**The runtime path.** The Job runs as one dedicated service account,
`fleet-worker`, attached to the Job. Its credential reaches the process from
the metadata server and expires; no key file is created, downloaded or
mounted, which is `01-security-and-safety.md`'s rule. Workload Identity
Federation, which the owner named, is the **dispatch** path: GitHub Actions
reaches the project the way `deploy.yml` already does, from committed tfvars,
with no repository variable. The desktop dispatches with the owner's own
`gcloud` login, which is the owner's session and not a stored credential. The
two paths are named separately because "WIF, no keys" is true of both and
means a different mechanism in each.

**The fence.** The project boundary ADR 0098 wanted is gone, so the fence is
what `fleet-worker` is *not* able to do, which is checkable by listing its
bindings:

- `roles/aiplatform.user` on the project, and object read/write on **one**
  bucket owned by the fleet. Nothing else.
- **No** Secret Manager access, **no** KMS access, **no** Artifact Registry
  write, **no** Binary Authorization policy rights, **no** Cloud Run
  permissions of any kind (so a worker cannot start a worker: the policy's
  "workers may not spawn workers" holds as an absence of
  `run.jobs.run`, not as an instruction), **no** access to the trading
  platform's buckets, services, or the venue credential's secret, and no
  git credential of any kind.
- A separate Artifact Registry repository and a separate Terraform root for
  the fleet's own resources, as ADR 0098 specified, so that `terraform plan`
  on the platform's root cannot propose to change the fleet and the reverse.
  The root's path is chosen by the implementer and must be checked against
  the `terraform_contract` and `infrastructure` acceptance suites before it
  is committed; **that check was not run here.**
- The dispatch identity can run the Job and write packets to the bucket. It
  cannot read secrets either.

This is **weaker than a separate project**, and the weaker fence is stated as
a cost below. If the first slice finds the owner can supply a second project,
a one-file change to `project_id` restores ADR 0098's separation, and this
record should then be amended to say so.

### 4. Model tiers are classes with an acceptance test, not names, and "highly rated" is our own measurement

ADR 0098's table assumed every executor was a Claude model. The owner has
cleared Vertex AI for repository source, which satisfies policy §4 gate 2 for
this provider. Gates 1, 3, 4, 5 and 6 are re-run below.

| Tier | Work (roles) | Executor, as decided here |
|---|---|---|
| T0 | search, format, lint, build, test, scans | no model — unchanged |
| T1 | `scout`, `fixture`, `drafter`, `ops-watch`, `catalogue` | the cheapest model on Vertex AI that passes the fleet's own acceptance suite, free or free-tier where one does |
| T2 | `refactor`, `tester`, `reviewer` | the cheapest model that passes the same suite at the harder items |
| T3 | difficult implementation and debugging | a stronger Claude model, on the desktop — **not in the fleet** |
| T4 | architecture, security, integration, merge | the lead session — **not in the fleet** |

**No model is named, because none was observed.** The first slice produces
the candidate list from `gcloud ai models list` and the Model Garden listing
as the project sees them, with the observed price and free-tier status for
each, dated, in the appendix.

**"The most highly rated" is not a criterion this record can adopt.** A
public rating measures somebody else's benchmark. Policy §2 already defines
the metric that matters here, *verified output per token*, accepted diff
lines per output token, and a model the leaderboard prefers that produces
output the gates reject is, under that formula, zero output at full cost. The
selection rule is therefore: **run a fixed set of at least six real packets
per role family against each candidate; rank by the §2 ratio; pick the
cheapest within 10% of the best.** Public ratings may nominate candidates. They
cannot admit one.

**Free tier is a hypothesis to be read, not a fact to be assumed.** Policy §4
gate 4 names "free tier very often means trains on your input" as the
canonical hazard, and this repository holds risk and execution logic. So:

- Gate 4 (privacy) is answered **per model, from the provider's terms as read
  in the first slice**, and the answer is written in the appendix with the
  date and the page. Until a model's row says "does not train on input" in
  words someone can quote, it receives packets from the **non-reserved
  classes only**, and "I could not find it" counts as not met.
- Gate 6 (classification) is unchanged and stricter than the owner's general
  source authorisation: nothing under `qip-risk-engine`,
  `qip-execution-engine`, `qip-capital`, `qip-compliance` or
  `backend/crates/edge` is ever the context of a fleet packet, at any price,
  and the dispatcher refuses by path prefix rather than trusting the packet's
  author. The gateway's credential-shape screen runs on every payload and
  refuses rather than scrubs, as it does now.
- Gate 5 (cost) is decision 5.

If no Vertex AI model has an observed price and an observed no-training
statement, **the fleet has no executor and does not run**. That outcome is
legitimate and cheap, and it leaves ADR 0098 in force for everything it did
not supersede.

### 5. The cost model: arithmetic before dispatch, a ledger per packet, and a kill switch that is a file

The ceiling is **25 USD a day for the project's fleet-attributable spend**:
model calls, Cloud Run Job time, Cloud Logging for the fleet's logs, and
storage. It is not 25 USD for model calls, and it is partitioned so that the
parts that cannot be capped by the fleet are not allowed to eat the part that
can.

- **Model-call allowance: 20 USD a day.** The remaining 5 USD is reserved for
  Job time, logging and storage, which the fleet does not meter itself. If the
  observed reserve proves too small or too large the split is changed by
  amending this record, not by editing a constant.
- **Pause at 80%** of the allowance (16 USD), per policy §8: new dispatch
  stops, running tasks finish. The allowance is 20; dispatch refuses to start
  a packet when the day's committed spend plus that packet's worst case would
  exceed it.
- **Per-slot daily cap: 0.50 USD** (20 / 40). A slot is a role-slot in the
  table and not a process, so the cap is enforced per role: the role's daily
  allowance is its slots times 0.50, and a role that exhausts it stops while
  other roles continue. A cheap role cannot be starved by an expensive one,
  and an expensive one cannot starve the day.
- **Worst case is computable before the call, and dispatch refuses on it.**
  A packet carries `max_tokens` (policy §5's token limit) and the gateway
  already requires `ALGORIK_WORKER_MAX_CALLS`. A packet's worst-case spend is
  `max_calls x (input_tokens_bound + max_tokens) x the model's price`, with
  the price taken from a committed table whose every row carries the date it
  was observed. A packet with no price row has no worst case, and a packet
  with no worst case is **refused**, not run on an estimate. This is the
  structural half: the spend a packet can cause is bounded by its own fields
  before it exists.
- **The ledger.** One object per packet in the fleet bucket, written by the
  worker when it finishes: `packet_id`, `role`, `model` as the response
  reported it, prompt and completion tokens as the response reported them,
  USD computed from the price table, `input_sha256`, `output_sha256`, wall
  time, and a `why_this_tier` sentence taken from the packet. The dispatcher
  sums objects; **no shared append file**, because forty tasks appending to
  one object is a race, and the gateway's existing single-file ledger is
  correct for one process and wrong for forty. An empty or refused completion
  is billed and recorded, as the gateway already does.
- **The kill switch is an object named `HALT` in the bucket.** The
  dispatcher refuses to start any packet while it exists, and a worker checks
  for it at start and before each call. Creating it needs bucket write and
  is the owner's or the dispatch identity's. It is a file on purpose: the
  platform's other halt wires are files the process polls (`qip-edge-node`'s
  `polled` source), and a switch that needs an API to be healthy is a switch
  that fails when it is most needed. **The harder stop** is removing
  `fleet-worker`'s `aiplatform.user` binding, which stops spend at the source
  even if the dispatcher is wedged; the Terraform root exposes it as one
  boolean so it is a reviewed change and not a console click.
- **Budget alerts are wired, and are not trusted to stop anything.** A
  billing budget on the project at 50%, 80% and 100% of 25 USD, notifying the
  owner. **A budget alert reports spend after the fact and, as far as this
  record can establish, does not stop it; whether it can be made to is
  UNPROVEN and the design assumes it cannot.** The alert is therefore the
  *independent* second claim about the same fact that principle 6 asks for: if
  the ledger's day total and the billing console disagree, the larger is
  right, the dispatcher halts, and the disagreement is the finding. Creating
  a monthly budget of 750 USD (25 x 30, GCP budgets are not daily) with
  50/70/90/100% thresholds already exists on the project (id recorded in
  DECISIONS.md), created 2026-10-04 with the owner's account. It only alerts.
- **Cloud Run settings that bound cost:** `--max-retries 0`, a per-task
  timeout from the packet, `--parallelism` no higher than the current rung,
  minimum resources for a task that waits on an HTTPS call (CPU and memory
  sized by measurement in the first slice and not guessed here), no
  always-allocated CPU, no minimum instances (Jobs have none), and the fleet's
  logs excluded from any sink that duplicates them, because "uncontrolled
  duplication" is a standing product decision.

None of the numbers above is a price. They are partitions of the owner's 25.
Every price in the system arrives from the appendix, dated, or the packet is
refused.

### 6. What remains true, and what no longer does

**No longer true**, by this record and on the owner's instruction of
2026-10-04: that no packet leaves for another provider; that Claude models are
the only tiers; that the GCP fabric is deferred until the ladder reaches the
desktop's ceiling and CPU binds with ready work queued; and that workers do
not run in the trading platform's project. ADR 0098's reasons for each are
now costs, listed below, rather than rules.

**Remains true, exactly, and the fleet is built so that each of these is
structural where it can be and a refusal where it cannot:**

1. **The paper-trading boundary is untouched.** Fleet workers cannot reach
   it. See the last section for the three layers.
2. **No fleet worker receives a secret.** The packet is assembled by the
   dispatcher from named files, the gateway's screen refuses a
   credential-shaped payload, no secret path is ever a packet's context, and
   the worker's identity has no Secret Manager access. The worker's own
   credential is the metadata server's short-lived token and is not in the
   payload.
3. **No fleet worker deploys, applies, merges or pushes.** It has no git
   credential, no Cloud Run rights and no Terraform state access, and it
   produces an object. Production is refused by `infra.yml` and the deploy
   gate regardless, and nothing here is a way round either.
4. **Fleet outputs are data, not instructions.** A worker's output is never
   executed, never interpreted as a command, and never applied without the
   gates in policy §10 having run on it and the lead's merge. A reviewer
   finding is evidence for the lead, not a verdict, and an output that
   contains something shaped like an instruction ("now also run...") is
   treated as a defect in that output.
5. **Every packet is logged and attributable**: decision 5's ledger object,
   and the accepted diff's commit message names the `packet_id`. The ledger
   is dev tooling and is **not written to the platform's hash-chained event
   log**, because the platform's log is the record of what the platform
   decided and a development run is not that, and a second copy of a fact
   the log already holds is what the architecture rules forbid. The lead's
   task log (policy §2) holds the hashes.
6. **Routing records its rationale.** `qip-cost-router` is the platform's
   router and is not used by the fleet, so the rule "the cost router records
   the rationale for the rung it chose" is carried by the packet's
   `model ceiling` and `why_this_tier` fields, written to the ledger on every
   packet. The platform's router is unchanged and its `Determinism::Required`
   arm still returns a type that cannot name a model rung.
7. **Policy §3's reserved list stays with the lead at T4**, and §4's gate 6
   (the five directories that never leave) is enforced at dispatch.
8. **No async runtime, no new crate, no new npm package.** The gateway uses
   Node's built-in `fetch`, the fleet's infrastructure is Terraform on the
   provider already pinned, and the single Dockerfile adds a base image that
   `deploy.yml`'s digest-pinning discipline governs. A base image is not a
   dependency in ADR 0002's sense and is still pinned by digest, never by
   tag.

### 7. The first slice is small enough for this week, and every step has an observable end

1. **Done 2026-10-04:** billing is enabled on `algorik-platform-dev` and a
   monthly budget exists. Enabling `aiplatform.googleapis.com` is the next step.
2. **Observe, and write the answers into this record's appendix:**
   `gcloud services list --enabled --project algorik-platform-dev`, then
   enabling `aiplatform.googleapis.com` **only if it is absent**, saying so in
   the appendix; `gcloud ai models list` and the Model Garden listing from
   this machine, with each candidate's observed name, region, price and free
   tier status and the terms page read for gate 4; and whether Vertex AI
   accepts the gateway's chat shape. Exit: the UNPROVEN rows are observed or
   remain UNPROVEN with a reason.
3. **Gateway:** the `vertex` preset (base URL fixed; token from the metadata
   server; no key file) and the per-packet ledger object with the price table,
   in `scripts/model-gateway.mjs` and its test, each new test
   mutation-verified. Exit: `node --test scripts/model-gateway.test.mjs`
   quoted.
4. **Terraform, its own root:** the `fleet-worker` account with the bindings
   in decision 3 and none else, the bucket, the Job, the budget. Exit: a real
   plan shown, and `terraform validate`. **No apply without the owner reading
   the plan**, per `infrastructure.md`.
5. **One Job, one task, one `scout` packet at T1**, on a non-reserved path,
   with `HALT` tested by creating it and watching the next dispatch refuse.
   Exit: one ledger object and the owner's day total from the console, the two
   claims read side by side.
6. **Eight packets at rung 8**, the policy's starting rung, across at least
   two roles, gated on the desktop. Exit: the §2 ratio, recorded. The climb to
   16 waits for six completed packets at 8 and a measured gain.

Steps 1 and 2 are what this week's calendar can spare. Steps 3 to 6 follow
only if step 2 finds a model that passes gate 4.

## What it costs

- **A weaker fence than ADR 0098 designed.** A separate project made the
  worker's blast radius a billing boundary. An identity with a short list of
  bindings is a smaller promise: a mistake in the Terraform that widens one
  binding puts a development identity beside the dev environment's KMS keys
  and secrets. The mitigation is a reviewed binding list and a plan that
  shows it; it is not a boundary.
- **Source leaves the machine for a provider whose terms nobody here has
  read yet.** The owner authorised it on 2026-08-30 and again on 2026-10-04;
  an authorisation is not a reading of the terms, and decision 4 holds
  packets to non-reserved classes until the reading is done.
- **The first useful work is weeks away and may be zero.** Billing is off, the
  API may be off, no model has been observed, and the first slice ends in an
  observation that might say there is no admissible model. A fleet that is
  cheap because it does not run is a correct outcome.
- **Forty is mostly unused for a while.** The ladder starts at 8. The owner
  asked for a ceiling of forty and is getting a ladder that reaches it only on
  evidence, and says so here rather than leaving it to be discovered.
- **A new cost line and a new thing to watch.** The fleet's spend is one more
  number in the ledger and one more in billing, and a day's two numbers
  disagreeing is a finding someone has to chase.
- **A second gateway path.** The `vertex` preset is a second way for source to
  leave and widens `model-gateway.mjs`; the preset fixes the host, as
  Hugging Face's does, so it cannot be pointed elsewhere by editing one
  variable.
- **The ratings the owner asked for are not used.** The measure is ours. If
  the owner's intuition is that a highly rated model is cheaper per accepted
  line, the ladder will say so, and that is a better argument than a
  leaderboard.

## Alternatives rejected

- **Forty resident agent processes or a Cloud Run Service pool.** They bill
  while idle, hold context across unrelated packets (which is how one packet's
  data becomes another's prompt), and need an authentication model for
  callers. A task per packet bills only what ran, and that is the sixth
  principle in `CLAUDE.md` applied to labour.
- **Agents that run tools: a shell, a checkout, `git`.** The roster would be
  more capable and the surface would be the repository and the network. Most
  of what the roster does is bounded text-to-text and does not need either,
  and what does need a build runs on the desktop where the gates are.
- **GKE Autopilot with vLLM serving open models.** ADR 0098 rejected it and
  nothing has changed that: it is Kubernetes again, on a cluster ADR 0093 found
  too costly to keep, plus a GPU line, plus gate 4 answered for a model we
  host. Vertex AI as a managed endpoint is the cheaper experiment.
- **A second project for the fleet.** It is what ADR 0098 chose and it is
  better. It is not what the owner asked for, and this record does not
  override an instruction by quiet substitution; it records the cost and keeps
  the change to one line.
- **Rank candidates by public ratings.** Rejected in decision 4: it optimises
  somebody else's metric.
- **Trust the budget alert to stop spend.** Unproven and assumed false. A
  design whose cap rests on a feature nobody has observed is a cap that reads
  as protection and does not fire, which is the `MaxExpectedShortfall` shape.
- **Reuse Hugging Face.** ADR 0037 stands and is blocked on credentials and
  on reading each resolved provider's terms. It is not widened or retired by
  this record.
- **Let the fleet write to the platform's event log.** A second copy of a fact
  the log already does not hold, and a path from a development tool into the
  record the research desk reproduces decisions from.
- **No ADR; do it in the Terraform and the script.** An agent fleet with
  identities in the production-adjacent project, a provider change to a
  standing decision and a spend line is an architecture decision, and "it
  needed an ADR" is the rule about a choice that would otherwise live in a PR
  comment.

## What would make this wrong

- **No Vertex AI model has an observed price and an observed no-training
  statement.** Then there is no executor and the answer is no fleet. ADR 0098
  stands for the rest.
- **The ladder not climbing.** If accepted lines per output token fall from 8
  to 16, the constraint is review, merges and file contention, and forty
  workers would be forty more things to review. Stay at the best rung.
- **The ledger and the console disagreeing by more than a rounding.** Then
  something spends outside the ledger, a role's packets bypass the gateway or
  a price row is wrong, and the fleet halts until it is explained.
- **A free tier that is rate-limited into uselessness.** Forty parallel tasks
  on a free quota will meet its limit. Policy §8 says a rate limit means wait,
  not spawn more; if waiting is most of the wall time, the free tier is not a
  tier for this workload and the allowance, not the count, is what to
  revisit.
- **A packet's output containing material from outside its context.** That is
  either memorisation or the provider mixing requests, and either ends the
  model's admission.
- **`fleet-worker` acquiring a binding this record does not list.** That is a
  breach of decision 3 whatever the reason, and a Terraform check can say so
  mechanically; it should.
- **The owner supplying a second project.** Then decision 3's fence becomes a
  boundary and the "weaker fence" cost is withdrawn.
- **A violation of the paper-trading boundary by any fleet output.** The
  fleet is dissolved, not tuned.

**Reversal cost.** Low and in one direction: remove the Terraform root (its
own state, so no platform resource is touched), disable
`aiplatform.googleapis.com` if this record enabled it, delete the bucket.
ADR 0098's clauses return by superseding this record. The gateway's `vertex`
preset can stay, because a preset nobody configures sends nothing.

## Validation

- **Structural, in the Terraform root's own checks:** `fleet-worker`'s
  binding list equals decision 3's list and nothing else, the Job's
  `max_retries` is 0 and its parallelism does not exceed 40, no key resource
  exists for the account, and no `${{ vars.* }}` appears in a fleet workflow.
  The `infrastructure` and `terraform_contract` suites must still pass on the
  tree; neither was run here.
- **Gateway tests**, each mutation-verified: a packet with no price row is
  refused; a worst case over the remaining allowance is refused; `HALT`
  present refuses before any call; the `vertex` preset refuses a differing
  base URL; the credential screen runs on the packet the dispatcher
  assembled; an empty completion is billed and recorded.
- **Dispatch refusal by path**: a packet whose context names a path under
  `qip-risk-engine`, `qip-execution-engine`, `qip-capital`, `qip-compliance`
  or `backend/crates/edge` is refused, with a test that fails if the prefix
  list loses an entry.
- **The measurement that decides**: at rung 8, the §2 ratio over six
  packets, quoted from the lead's task log, and the day's ledger total beside
  the console's.
- **This record's own check**: `cargo test -p qip-acceptance --test
  documentation`, whose output is quoted in the handback and not asserted here.

## Appendix: observations to be filled in by slice step 2

Each row is a command, its date and its quoted output; a row without output
stays UNPROVEN. Filled in on 2026-10-04 from the owner's desktop, after the
owner's instruction "Get the fleet up".

| Date | Command | Output | Conclusion |
|---|---|---|---|
| 2026-10-04 | `gcloud services list --enabled --project=algorik-platform-dev`, filtered for aiplatform, run, artifactregistry, secretmanager | no line printed | Vertex AI was absent, so enabling it was this step's to do |
| 2026-10-04 | `gcloud services enable aiplatform.googleapis.com --project=algorik-platform-dev` | `Operation "operations/acat.p2-523718313246-4aa7b8b5-..." finished successfully.` | Vertex AI is enabled on the project. `run`, `artifactregistry` and `secretmanager` are still not enabled |
| 2026-10-04 | `gcloud ai model-garden models list --project=algorik-platform-dev --billing-project=algorik-platform-dev` | 297 distinct names, among them `publishers/google/models/gemini-2.5-flash-lite`, `gemini-3.1-flash-lite`, `gemini-3.5-flash-lite`, `publishers/openai/models/gpt-oss-120b-maas`, `publishers/meta/models/llama-3.3-70b-instruct-maas` | A listing is what the catalogue shows, not what the project may call; only the row below proves a call. Without `--billing-project` the command fails on a missing quota project |
| 2026-10-04 | one `POST .../v1/projects/algorik-platform-dev/locations/global/endpoints/openapi/chat/completions`, model `google/gemini-2.5-flash-lite`, `max_tokens` 16, with the owner's own short-lived token | `"model": "google/gemini-2.5-flash-lite"`, `"prompt_tokens": 7`, `"completion_tokens": 1`, `"traffic_type": "ON_DEMAND"`, content `ready` | Vertex AI accepts the gateway's OpenAI-shaped chat request, and this model is callable from this project in `global`. Eight tokens were billed |
| 2026-10-04 | `curl https://cloud.google.com/vertex-ai/generative-ai/pricing` ("Agent Platform Pricing"), read as text | per 1M tokens: Gemini 2.5 Flash Lite input `$0.10`, text output `$0.40`; Gemini 3.1 Flash-Lite input `$0.25` (global); Gemini 3.5 Flash-Lite input `$0.30` (global); gpt-oss-20b `$0.07` / `$0.25`; gpt-oss-120b `$0.09` / `$0.36`; Gemma 4 26B `$0.15` / `$0.60`; Llama 4 Maverick `$0.35` / `$1.15` | **No free tier for token usage is on the page.** "No charge" appears only for grounding-query allowances and embedding outputs. The free-tier hypothesis is refuted for Vertex AI: the fleet is low-cost, not free |
| 2026-10-04 | `curl https://docs.cloud.google.com/vertex-ai/generative-ai/docs/data-governance`, read as text | "Google won't use your data to train or fine-tune any AI/ML models without your prior permission or instruction. This applies to all managed models on Gemini Enterprise Agent Platform, including GA and pre-GA models."; Gemini models cache inputs and outputs "in-memory ... isolated at the project level, and has a 24-hour TTL"; "Google may log prompts to detect potential abuse" | Gate 4 has an observed no-training statement for Google's own models. Retention is not zero by default: a 24-hour in-memory cache and abuse-monitoring logging exist, so packets stay on non-reserved paths |

| 2026-10-04 | `curl https://docs.cloud.google.com/vertex-ai/generative-ai/docs/maas/use-open-models`, read as text | "Gemini Enterprise Agent Platform supports a curated list of open models as managed models. These open models can be used ... as a model as a service (MaaS) and are offered as a managed API."; "Managed open models are serverless"; "Customer prompts and model responses are not shared with third parties when using the Gemini Enterprise API, including open models." | The open models Google serves are *managed models*, so the data-governance page's "applies to all managed models" covers them. This is Google's statement about Google's service; a publisher's own licence for the weights was not read |
| 2026-10-04 | the same one-call probe, `max_tokens` 24, against five open models in `global` | `qwen/qwen3-coder-480b-a35b-instruct-maas`: 15 tokens in, 2 out, content `ready`. `qwen/qwen3-235b-a22b-instruct-2507-maas`: 15 in, 2 out, `ready`. `openai/gpt-oss-120b-maas`: `Operation timed out after 90002 milliseconds with 0 bytes received`. `deepseek-ai/deepseek-v3.2-maas`: `429 Resource exhausted`. `meta/llama-4-maverick-17b-128e-instruct-maas`: `404 ... was not found or your project does not have access to it` | Two large open models are callable from this project today. The other three are not, each for a different reason, and none of the three is admitted |
| 2026-10-04 | the pricing page above, same read | per 1M tokens: Qwen3-Coder-480B-A35B-Instruct input `$0.22`, output `$1.80`; Qwen3-235B-A22B-Instruct-2507 input `$0.22`, output `$0.88` | Priced, so a packet naming either has a price row |
| 2026-10-04 | `terraform plan` then `terraform apply fleet.plan` in `infrastructure/fleet`, state in `gs://algorik-platform-dev-fleet-tfstate` | `Plan: 5 to add, 0 to change, 0 to destroy.` then `Apply complete! Resources: 5 added, 0 changed, 0 destroyed.` | The repository, the bucket, the `fleet-worker` account and its two bindings exist. The Job does not: no image has been pushed |
| 2026-10-05 | one `POST .../v1/projects/algorik-platform-dev/locations/global/endpoints/openapi/chat/completions`, model `google/gemini-3.1-pro-preview`; then the pricing page above, read again | the call returned content `ready`, 7 tokens in, 1 out; the page lists "Gemini 3.1 Pro Preview" at `$2.00` per 1M input tokens and `$12.00` per 1M output and thinking tokens for prompts up to 200K tokens, and `$4.00` / `$18.00` above 200K | Callable and priced, so it has a row. It is a thinking model: thinking tokens are billed as output and count against `max_tokens`, so the price table marks it `thinking` and the worker refuses a packet with `max_output_tokens` below 1000 (1500 or more advised) or `max_input_tokens` above 200000, the only prompt size the row's prices cover |

**What step 2 admits.** Four models pass every test this record sets
(listed, called, priced, and covered by an observed no-training statement):
`google/gemini-2.5-flash-lite` as the cheap tier, `google/gemini-3.1-pro-preview`
(added 2026-10-05) as a premium thinking tier, and the two open models
`qwen/qwen3-coder-480b-a35b-instruct-maas` and
`qwen/qwen3-235b-a22b-instruct-2507-maas` as the larger tiers. gpt-oss,
DeepSeek and Llama stay out because the project could not call them; Kimi,
GLM, MiniMax and Gemma were not probed. No public rating was consulted; the
fleet's own acceptance ratio decides between tiers.

**The owner asked on 2026-10-04 for the largest open models from Hugging
Face, deployed in GCP.** The managed open models above are that, served by
Google per token with no accelerator to rent. Deploying weights from Hugging
Face onto GPUs is not done by this record: it needs accelerators, which
`infrastructure.md` and ADR 0093 hold behind their own decision, a price per
hour nobody has observed here, and a quota nobody has checked.

At the observed price, the 20 USD/day model allowance is 200 million input
tokens or 50 million output tokens of this model, by arithmetic on the two
figures above and nothing else.

## Paper trading

Untouched at all three layers. No file under `infrastructure/terraform/`'s
platform root, no composition root, and no type in `qip-edge` or
`qip-cost-router` is changed by this record. A fleet worker has no
repository checkout, no credential for anything but the model endpoint and
its own bucket, no Cloud Run, Secret Manager or KMS right, and no path to the
`qip-config` ceiling, `AutonomyLevel::deployable` or the edge `Cell`. Work
near the boundary is outside the fleet by decision 1's missing roles and by
decision 4's classification refusal, and an agent output is never an
instruction to raise autonomy. The `enable_vertex_ai` flag in the platform's
tfvars is **not** flipped by this record: the platform's training port has no
client, as that variable's own description says, and the fleet's use of the
same API is the fleet's Terraform root's business, not the platform's.
