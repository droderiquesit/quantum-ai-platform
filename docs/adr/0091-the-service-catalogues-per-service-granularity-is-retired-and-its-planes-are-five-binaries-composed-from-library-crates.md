# ADR 0091: The service catalogue's per-service granularity is retired, and its planes are five binaries composed from library crates

- **Status**: Accepted on the authority the owner delegated on 2026-09-19
  (ADR 0081's status line records the delegation), on ADR 0081's model: a
  name in the blueprint is retired as a deliberate non-goal, the outcomes
  behind the name are scored against what exists, and a retired clause is
  not `REACHED`. Nothing is built by this record and no file outside `docs/`
  changes.
- **Date**: 2026-09-20
- **Supersedes**: nothing. Corrects two counts the register's §41.6 row
  carried — "twelve planes" and "six binaries" — both re-counted below.
- **Related**: ADR 0001 and ADR 0011 (one language, one workspace; no
  managed client in the process), ADR 0009 (what the dependency policy
  actually forbids is clients), ADR 0010 (four of the six application
  crates are deployed, and `qip-web` is a library), ADR 0016 (one layout),
  ADR 0024 (every warm binary is a Cloud Run service; one execution node per
  region), ADR 0008 (a cell decides alone — the one plane the blueprint and
  this platform both make a separate process), ADR 0069 (capability with no
  consumer is not declared — the spot GPU pool the catalogue's Intelligence
  row runs on), ADR 0081 (the model this record follows), ADR 0082 (a
  reversal condition that needs a scrape cannot fire before something
  scrapes).

## Context

Blueprint §41.6 is a table: a plane, its services, its runtime. Read from
source rather than from the register's paraphrase
(`awk '/^41\.6 /{f=1} f&&/^42\. /{exit} f' docs/architecture/algorik-blueprint-v10.1-source.md`),
it has **thirteen** plane rows, not the twelve the register's row has said
since 2026-09-19:
`awk '/^41\.6 /{f=1} f&&/^42\. /{exit} f' docs/architecture/algorik-blueprint-v10.1-source.md | sed -n '5,$p' | grep -v '^Roughly' | awk 'NR%3==1'`
prints Ingestion, Cognition, Valuation, Intelligence, Optimisation, Capital
and risk, Ledger and treasury, Wallet and inventory, Registries and
lifecycle, Experience and identity, Data and observability, Control fabric,
Execution. The same command with `NR%3==2` and the names split on commas
prints 114 — and that number is wrong in the way that proves the row's
point: "deep-web adapters (query, api, registered, licensed, rendered,
bulk)" splits into seven, "algorik-node × 3" is one, and the table's own
footer says "roughly seventy binaries". The register's `~140` was somebody's
judgement about what one service is; so is seventy; so is 114. Any
denominator here is a judgement, and the row was right to refuse one.

**What the tree has instead.** `ls backend/crates/apps` prints six crates.
`ls backend/crates/apps/*/src/main.rs` prints **five** — `qip-web` is a
library `qip-api` links and renders from (ADR 0010; its `Cargo.toml` has no
`[[bin]]` and its `src/` has no `main.rs`), so the register's "six
binaries" was a count of directories. Of the five, three are Cloud Run
services (`grep -n 'binary *= *"qip-' infrastructure/terraform/catalogue.tf`
prints `qip-api`, `qip-fastbrain`, `qip-deepbrain`), one is the execution
node on Compute Engine under systemd (`modules/execution-node`; no
instance anywhere, `grep -h execution_nodes infrastructure/environments/*/terraform.tfvars`
prints `execution_nodes = {}` four times), and one, `qip-cli`, is a tool a
person runs (ADR 0010).

**How the planes get into the binaries.** Four of the application crates
depend on `qip-kernel` (`grep -l '^qip-kernel' backend/crates/apps/*/Cargo.toml`),
and the kernel composes thirty-eight workspace crates
(`grep -c '^qip-.*workspace = true' backend/crates/runtime/qip-kernel/Cargo.toml`)
— every one of the twenty-four service crates under `backend/crates/services`
(`ls backend/crates/services | wc -l`) plus the libraries — into one
`Platform` that runs the eight-stage cycle. `qip-edge-node` composes sixteen
without the kernel (`grep -c '^qip-.*workspace = true' backend/crates/apps/qip-edge-node/Cargo.toml`).
So the catalogue's Ingestion, Cognition, Valuation, Intelligence,
Optimisation, Capital, Ledger, Wallet, Registries and Control-fabric rows are
library crates inside the three central binaries, its Experience row is the
TypeScript portal (ADR 0081) plus `qip-api`, and its Execution row is
`qip-edge-node` — the one row honoured in the catalogue's own shape, and the
one with no instance.

### The fact that decides it

The catalogue's footer says "Everything except the execution node scales to
zero." This platform's own manifest says otherwise, and the reason is
written beside the number.
`grep -n 'min_instances' infrastructure/terraform/catalogue.tf` prints `0`
for `qip-api` and `1` for each brain, and the comment on `qip-fastbrain`'s
floor and ceiling says why both are one: the binary "opens the event log
and runs the cycle on its own clock … two instances would each run the
cycle and each append to the same hash-chained log, and a fork in the chain
is the corruption the chain exists to detect, not one it tolerates."

That is not a Cloud Run detail. It is the reason the catalogue's granularity
cannot be this platform's. The event log has one writer per chain, by
design (ADR 0002's hashing, the governance rule that nothing may write a
record that cannot be replayed). A plane split into scale-to-zero services
is either a second writer of the chain — the fork — or a client of the one
writer over a transport. And the transport is the other settled decision:
ADR 0011 admits no managed client into the process, ADR 0009 names clients
as the thing the dependency policy forbids, and the only transport in the
tree is `qip-transport`'s in-tree HTTP/1.1 behind an egress proxy that ADR
0024 has never applied. Seventy services would be seventy hops over a
hand-written transport in the process that moves money, each one a place
where the cycle's evidence could arrive late, twice or not at all. The
blueprint's own §54.4 argument — a supply chain that rests on one dependency
graph — cuts the same way here that ADR 0081 found it cutting against a
Leptos tree.

### What the catalogue's runtime column asks for, row by row

- **Cloud Run + Jobs** (Ingestion, Cognition, Valuation, Optimisation): the
  functions are crates in the brains; there are no Jobs, because a job is a
  cycle stage and the cycle has one clock.
- **Jobs + spot GPU** (Intelligence): the trainer is in-tree ridge, boosted
  stumps and distillation on CPU (ADR 0011's substitution table; ADR 0083
  serves the result in-process). ADR 0069 refused the spot GPU pool as
  capability with no consumer, and nothing here consumes one.
- **Cloud Run + Pub/Sub** (Control fabric: `policy-distributor`,
  `outcome-collector`): the policy payload ships over the mesh and cell
  reports come back through `Platform::ingest_cell_report`
  (`grep -n 'pub fn ingest_cell_report' backend/crates/runtime/qip-kernel/src/platform.rs`).
  No Pub/Sub, by ADR 0011.
- **Cloud Run + CDN** (Experience and identity): the portal and landing are
  TypeScript by ADR 0081, and identity lives in the portal's server (ADR
  0019).
- **GCE C3, systemd** (Execution): `qip-edge-node`, honoured in shape, no
  instance.

## Decision

### 1. Per-service granularity is retired as a deliberate non-goal

The unit of deployment is the binary and the unit of composition is the
crate. The catalogue's thirteen plane rows are scored by whether the
functions they name have a crate with a production call path inside a
binary, and never by a count of services. No number stands in the register
as a denominator for §41.6 — not `~140`, not "roughly seventy", not 114 —
and a lane that wants one must first write down what a service is, in its
own paragraph, and expect to be argued with.

### 2. What makes a plane a binary is a writer of the log or a clock of its own, not a row in a table

The five binaries exist for five reasons the catalogue does not give: the
fast brain is the cycle's one writer of the chain; the deep brain runs the
slower stages on a clock of its own and is pinned at one instance for the
same reason; the API is the one workload that can scale, because it cycles
only when asked and serves stateless reads; the execution node is a cell
that decides alone on its own clock, in its own region, against its own
envelope (ADR 0008) — the one plane the blueprint and this platform both
make a separate process, for the same reason; and the CLI is a person's
tool. A future plane earns a binary the same way: by needing its own chain
or its own clock, shown in a record that says which.

### 3. A retired granularity is not `REACHED`

The register's §41.6 row stays `PARTIAL`, re-worded as retired by decision
with its fraction still deliberately unstated. ADR 0081's rule applies
unchanged: the name is retired, the outcomes are scored, and the row says
which outcomes are met — the functions have crates and production call
paths, scored plane by plane at §5.1–§5.7 — and which are not: no plane
scales independently, no plane fails independently except the node, and
the node has no instance.

## Consequences

- Register row §41.6 is re-scored as retired by decision, `PARTIAL`, with
  "twelve planes" corrected to thirteen and "six binaries" to five, each with
  its counting command. No other row changes on this record's account; the
  §5 roll-ups are the orchestrator's.
- No file under `backend/`, `frontend/` or `infrastructure/` changes. The
  paper-trading boundary is untouched at all three layers.
- `qip-web` stays what ADR 0010 and ADR 0081 made it: a library, not a
  sixth binary, and the register stops counting it as one.

## What it costs

**No plane scales to zero, and two never scale down at all.** The catalogue
promised scale-to-zero for everything but the node. Here the two brains are
always on, at one instance each, because chain integrity requires it, and
the API scales as a whole or not at all. That is a standing cost on every
invoice and this record pays it in the open: it is the price of one writer
per chain.

**One failure domain per binary.** A fault in any of the thirty-eight crates
the kernel composes stops every plane in that binary. The catalogue would
have given each of roughly seventy services its own blast radius. The tree's
mitigations are real but partial — `panic!` is denied in `Result`-returning
functions, `unwrap()` is forbidden outside tests, and the node keeps working
within its envelope when the centre is down — and the honest statement is
that the only independent failure domain is the one the blueprint also made
a separate binary.

**Secrets are scoped per binary, not per service.** A secret mounted on
`qip-api` is readable by every plane's code in that process. The catalogue's
`entitlement-service` and `auth-service` would each have had their own
identity and their own mounts; here they are functions in one process with
one identity.

**Deployment is all-or-nothing per binary.** One image, one attestation, one
rollout; a change to the Valuation plane ships the Cognition plane's binary
too. Binary Authorization attests the image, not the plane.

**The catalogue's vocabulary is absent from the tree.** A reader who greps
for `policy-distributor` or `outcome-collector` finds nothing
(`grep -rn 'policy-distributor\|outcome-collector' infrastructure/terraform backend/crates`
prints nothing), and the register's §34.2 row has already recorded what a
bare vocabulary grep is worth here. The translation from the catalogue's
114 names to the tree's crates and functions is the register's to make,
row by row at the section that owns each function, and this record does
not make it for all 114 — it makes it for the runtime column above and
stops.

**No independent scaling within a binary.** Ingestion's adapters and
Cognition's world model share the deep brain's CPU; a slow connector delays
a stage that has nothing to do with it. The stage timer
(`qip_stage_duration_milliseconds`) would show this, and nothing scrapes it.

## What would make this wrong

- **A plane needs an independent failure domain, shown by an incident.** A
  journaled, replayable incident in which one plane's fault stopped another
  plane that should have kept running. The node is the existing instance of
  this argument and it was answered with a binary; a second instance is
  answered the same way, by decision 2, and not by a service in the
  catalogue's sense.
- **A plane needs independent scaling, shown by a scraped series.** A series
  from a running process showing one plane's load starving another in the
  same binary, with the attribution shown rather than asserted. Nothing
  scrapes any process today (the observability rule file is the record), so
  this condition cannot fire before the collection gap closes — the same
  honest limit ADR 0082 states for its own reversal.
- **A plane's latency budget cannot be met in the single binary, shown by a
  scraped series.** `qip_stage_duration_milliseconds` over its budget for a
  stage, attributable to co-tenancy and not to the stage's own arithmetic.
  Same limit: it needs a scrape.
- **A second chain becomes necessary.** If a plane genuinely needs its own
  event log — its own hash chain, its own writer — it is a binary by
  decision 2, argued in its own record, and the catalogue's granularity is
  still not the answer.
- **The owner wants the catalogue by name.** Then ADR 0011's refusal of
  managed clients and ADR 0009's transport tier are reopened first, because
  seventy services need a transport somebody other than this repository
  wrote, and the work is a programme with an ADR per plane rather than a
  lane.

## Alternatives considered

**Build the catalogue: roughly seventy Cloud Run services and Jobs.**
Rejected. Each service is a secret mount, an identity, an attestation and a
Terraform module; each hop between them is `qip-transport`'s HTTP/1.1 over
a proxy that is not applied; and the cycle's evidence would cross those hops
on its way to a chain that has one writer. The cost is a distributed system
with a hand-written transport in front of real money, and the benefit —
scale-to-zero per plane — is one the platform's own manifest already
declines for the two binaries that matter.

**One binary.** Rejected. The node must be its own process (ADR 0008: a cell
that cannot reach the centre keeps working, and a cell in one region should
not share a process with a cell in another); the cycle's writer must be
alone; the API must scale; and a person's tool is not a service. Five is
the number those constraints give, and it is not a round one.

**Score the row against a service count.** Rejected: the row was right to
refuse a denominator, and this record's own recount (114, with a
parenthetical inflating it) shows why — the number depends on a judgement
the reader did not make.

**Leave the row open.** Rejected, as ADR 0081 rejected it: an open question
in the register reads as work somebody will do, and three waves of lanes
have re-derived the same analysis. A non-goal with a reversal condition is
cheaper to carry.

## Dependency-direction argument

Nothing in this record adds, removes or redirects an edge in the workspace.
Libraries still depend on nothing above them, services on libraries, the
kernel on services, the applications on the kernel or — for the node — on
the edge crates directly. The five binaries are the leaves of that graph,
and this record decides only that they stay its leaves.
