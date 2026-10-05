# Operator runbook

Written 2026-10-04 against the tree at the head of this branch. Read the first
two sections before anything else, because the most useful thing this page can
tell you is what is **not** running.

Per-binary detail is in [runbooks/binaries.md](runbooks/binaries.md) and the
incident playbooks are in [runbooks/incidents.md](runbooks/incidents.md). The
older, narrower pages in [`../operations/`](../operations/README.md) (kill
switch, deployment path, disaster recovery, venue registration) are still the
detail behind several sections here and are linked where they apply.

## How every command in this document is labelled

| Label | Meaning |
|---|---|
| **ran** | I ran it in this worktree on 2026-10-04 and read the output. The exit code is quoted. |
| **repo** | It appears verbatim in a file named beside it. I did not run it. |
| **not run** | Neither. It is the shape the code or a workflow implies, and the first person to run it should expect to correct it. |

A command with no label is a bug in this document.

## 1. What runs where

### 1.1 The short answer

**No process of this platform is running anywhere that anyone but a developer
can reach.** That is a measured statement, not a caution:

- Local: every binary below starts on a workstation (section 3) except the two
  that refuse to start (`qip-fabricd`, `qip-ledgerd`).
- Dev, project `algorik-platform-dev` (`infrastructure/environments/dev/terraform.tfvars`):
  - `gcloud billing projects describe algorik-platform-dev` (**ran**, exit 0)
    printed `billingEnabled: true`. Billing is on, as of this project; the
    older project `algorik-dev` had it disabled
    ([hermes baseline](hermes-baseline-2026-10-04.md), section 3).
  - `gcloud run services list --project algorik-platform-dev` (**ran**, exit 1)
    answered `PERMISSION_DENIED: Cloud Run Admin API has not been used in
    project algorik-platform-dev before or it is disabled`. The Cloud Run API
    has never been enabled, so no service exists.
  - `gcloud storage buckets list --project algorik-platform-dev` (**ran**,
    exit 0) printed `Listed 0 items.` There is no Terraform state bucket, so
    Terraform has never been initialised against this project.
  - Nothing has been applied (`DECISIONS.md`, entry "New cloud project
    `algorik-platform-dev`, billing enabled").
- `test`, `stage`, `prod`: `project_id = "unprovisioned"` in each tfvars.
  `terraform` refuses that value at plan time and `deploy.yml` and `vendor.yml`
  refuse it before authenticating (`infrastructure/environments/README.md`).

### 1.2 Where each thing is, and what blocks it

| Thing | Local | Dev (`algorik-platform-dev`) | Blocking reason |
|---|---|---|---|
| `qip-api` | starts, serves (**ran**) | declared, not deployed | Project has only just been created; APIs not enabled; bootstrap not run (section 7) |
| `qip-fastbrain` | starts, loops, serves health (**ran**) | declared, not deployed | Same |
| `qip-deepbrain` | starts, loops, serves health (**ran**) | declared, not deployed | Same |
| `qip-edge-node` | starts with `QIP_VENUE_FEED=simulated` (**ran**) | `execution_nodes = {}`; ADR 0035 authorises one node, `newyork-1`, shadow mode | No boot image is baked, and nobody has chosen the node's capital allocation (`infrastructure/environments/README.md`) |
| `qip-fabricd` | refuses to start without its seven required settings, exit 1; starts and serves on loopback against the committed local catalogue (**ran** 2026-10-04) | not a catalogue workload | Where the broker runs is undecided (ADR 0099 C8); `docs/adr/0010-what-gets-deployed.md` excludes it from the image matrix. One broker, one disk, no replication (ADR 0100 section 3) |
| `qip-ledgerd` | **refuses to start**, exit 1 (**ran**) | not a catalogue workload | Its `config`, `consumer` and `read_api` modules are doc-only stubs (ADR 0100); excluded from the image matrix by the same ADR |
| `qip-web` | library only, no binary; the console pages are served by `qip-api` | n/a | n/a |
| `qip` (the `qip-cli` binary) | runs (**ran**) | n/a | n/a |
| Portal and landing (Next.js) | not run (no `node_modules` installed in this worktree) | manifests exist (`infrastructure/gitops/envs/dev/portal.yaml`), not deployed | Same as the services |
| GitOps control plane (Argo CD, Kargo, Config Connector on GKE Autopilot) | n/a | `gitops_enabled = true` in dev tfvars; cluster is **suspended** (ADR 0093); bootstrap has never succeeded because the Argo CD image fails the Trivy CRITICAL gate | Upstream release does not pass the scan gate; not cleared by an exception |
| Event fabric brokers, Spanner ledger, GPU pools, Vertex endpoints | n/a | `enable_* = false` everywhere; nothing in Terraform references the fabric | Not built (`docs/SYSTEM_MAP.md`, section 3) |
| Scraping and alerting | `/metrics` is served by each process | no collector, no alert policy | `workload_metrics_exist = false`; the only published sidecar fails the platform's own scan (section 9) |

### 1.3 Two facts that change how you read every other page

1. **Storage in every declared deployment is `memory`.** The dev API manifest
   sets `QIP_STORAGE_TARGET` to `memory`
   (`infrastructure/gitops/envs/dev/api.yaml`), and the start-up banner says
   `NOTHING SURVIVES A RESTART` (**ran**, all four servers). The event log's
   hash chain is the audit trail and, on a Cloud Run service, it dies with the
   instance. Do not describe the dev environment as keeping a record.
2. **The paper-trading boundary is the one thing that is enforced everywhere
   it is reachable** (section 4). Nothing in this document describes a path to
   a real order, and nothing should be added that does.

## 2. Prerequisites

| Need | Version / source | Checked |
|---|---|---|
| Rust toolchain | `1.94.1` with rustfmt, clippy (`backend/rust-toolchain.toml`); rustup installs it on first `cargo` call | **ran** (the build below used it) |
| Disk | The workspace build is large. On 2026-10-04 the baseline test run failed to link because the root filesystem was 100% full, and the failure surfaces as `ld terminated with signal 7 [Bus error]`, not as "no space" ([baseline](hermes-baseline-2026-10-04.md), section 1). Check `df -h /` first | **ran**: 92% used, 19G free at the start of this work |
| `curl`, `openssl` | any | **ran** |
| Node 22 | only for the portal and landing; CI uses 22 | not run; `node_modules` is absent here |
| Playwright | portal and landing behavioural tests | not installed on the baseline machine |
| Terraform | CI pins 1.9.8 (`infrastructure/CLAUDE.md`); the baseline machine had v1.15.8 and `required_version = ">= 1.9.0"` | not run here |
| `gcloud` | authenticated; project `algorik-platform-dev` | **ran** (read-only calls only, section 1.1) |

## 3. A ten-minute local start

Measured: a cold build of the seven binaries in this worktree took
**1m 29s** (`Finished dev profile [unoptimized] target(s) in 1m 29s`, exit 0),
so the build is not the long part. All commands run from the repository root.

**Step 1. Build** (**ran**, exit 0):

```sh
cd backend && CARGO_PROFILE_DEV_DEBUG=0 cargo build -p qip-api -p qip-cli -p qip-fastbrain -p qip-deepbrain -p qip-edge-node -p qip-fabricd -p qip-ledgerd
```

Binaries land in `backend/target/debug/`: `qip-api`, `qip` (the CLI),
`qip-fastbrain`, `qip-deepbrain`, `qip-edge-node`, `qip-fabricd`, `qip-ledgerd`.
`CARGO_PROFILE_DEV_DEBUG=0` only makes the build smaller and faster.

**Step 2. The API.** It refuses to start without two things, and says so
(both **ran**, exit 1 each):

```text
qip-api: configuration: QIP_UNIVERSE_PATH is not set. Point it at the committed instrument catalogue ...
qip-api: no credential is configured; set at least QIP_TOKEN_OPERATOR. An API that would otherwise be unauthenticated does not start.
```

So give it the committed catalogue and a token. Generate your own token; do
not reuse one from a document:

```sh
openssl rand -hex 24          # ran: prints 48 hex characters
```

```sh
QIP_UNIVERSE_PATH="$PWD/data/datasets/universe.json" \
QIP_TOKEN_OPERATOR=<the hex you generated> \
backend/target/debug/qip-api
```

(**ran**, with an absolute path and a literal token of that shape; the
`$PWD` and `$(openssl ...)` forms were not run.) It listens on
`127.0.0.1:8080` by default and prints a banner. The lines to read in it:

```text
qip-api listening on 127.0.0.1:8080
  autonomy ceiling: paper_trading (executes against a simulated broker; nothing reaches a market)
  live trading:     unreachable in this deployment
  capital trust:    SEED-DERIVED — reproducible ... live trading is refused under this key
  storage:          memory
  durability:       NOTHING SURVIVES A RESTART of this process
```

**Step 3. Read it** (**ran**, all HTTP 200 with the operator token; the
operator token satisfies the lower roles):

```sh
curl -s -H "Authorization: Bearer $QIP_TOKEN_OPERATOR" http://127.0.0.1:8080/api/v1/health
# {"status":"ok","halted":false,"autonomy":"paper_trading","live_capable":false,"reconciliation_breaks":0}
curl -s -H "Authorization: Bearer $QIP_TOKEN_OPERATOR" http://127.0.0.1:8080/api/v1/system/status
curl -s -H "Authorization: Bearer $QIP_TOKEN_OPERATOR" http://127.0.0.1:8080/api/v1/autonomy
curl -s -H "Authorization: Bearer $QIP_TOKEN_OPERATOR" http://127.0.0.1:8080/api/v1/metrics
curl -s -X POST -H "Authorization: Bearer $QIP_TOKEN_OPERATOR" http://127.0.0.1:8080/api/v1/cycle
```

With no token the answer is `{"error":"no credential was presented"}`, HTTP
401 (**ran**). `POST /api/v1/cycle` returned HTTP 202 and a report whose first
stage says `no observations have been fed in; the platform is running blind`
(**ran**), which is the honest answer when no feed is configured.

**Step 4. The command line** (**ran**, exit 0 each):

```sh
backend/target/debug/qip help
backend/target/debug/qip status      # autonomy, ceiling, halted, log chain, store
backend/target/debug/qip limits      # the risk limits and why each exists
```

Every `qip` invocation builds a fresh platform and exits; without a store
configured, two invocations are two unrelated runs, and `status` says so.

**Step 5. The brains and a cell**, each in its own terminal, each bounded so it
stops by itself (all **ran**; the exact settings are in
[runbooks/binaries.md](runbooks/binaries.md)):

```sh
QIP_UNIVERSE_PATH=... QIP_FASTBRAIN_HEALTH_ADDRESS=127.0.0.1:8081 QIP_FASTBRAIN_MAX_RUNTIME_SECS=20 backend/target/debug/qip-fastbrain
QIP_UNIVERSE_PATH=... QIP_DEEPBRAIN_HEALTH_ADDRESS=127.0.0.1:8082 QIP_DEEPBRAIN_MAX_RUNTIME_SECS=20 backend/target/debug/qip-deepbrain
```

**Step 6. Policy gates** (**ran**, exit 0 each):

```sh
./scripts/check-dependencies.sh   # dependency policy: 11 third-party package(s), all permitted
./scripts/check-secrets.sh        # secret scan: nothing found
```

The full gate is `make check` (**repo**, `Makefile`): `fmt-check lint test
deps secrets`. It was **not run** here; the last full measurement on the
integrated tree is in `STATUS.md`: `cargo test --workspace --no-fail-fast`
exit 0, 493 `test result:` lines, 6502 passed, 0 failed, 0 ignored.

**Frontend** (**repo**, `frontend/portal/package.json`; **not run**):
`npm run dev`, `npm run lint`, `npm run build`, `npm run typecheck`,
`npm run test` in `frontend/portal/`. The portal reads `QIP_API_BASE_URL`,
`QIP_API_TIMEOUT_MS`, `NEXT_PUBLIC_QIP_ENVIRONMENT`, `ALGORIK_AUTH_REQUIRED`,
`ALGORIK_COOKIE_SECURE`, `ALGORIK_IDENTITY_API_KEY`,
`ALGORIK_IDENTITY_PROJECT_ID` and `ALGORIK_IDENTITY_STORE_DIR` from the
environment (found by search in `frontend/portal/src`; their semantics were
not read).

## 4. The paper-trading boundary, and the halt procedures

**This platform never submits a live order.** Three independent layers hold
that line; none may be weakened, bypassed or "temporarily" disabled
(`.claude/rules/01-security-and-safety.md`):

| Layer | What it does | What I saw |
|---|---|---|
| Terraform | `infrastructure/terraform/variables.tf` refuses `supervised_live`, `limited_autonomous_live`, `autonomous_live` at plan time | read, not planned |
| Composition roots | `AutonomyLevel::deployable` refuses the same three at start-up in `qip-api`, `qip-fastbrain`, `qip-deepbrain` | **ran**: `QIP_AUTONOMY_CEILING=autonomous_live` stopped `qip-api` and `qip-fastbrain` with exit 1 and `This platform is paper-trading only and will not start there. Set the ceiling to paper_trading, advisory or observation` |
| Type system | `qip-edge`'s `Cell` has no constructor taking a ceiling other than paper; `qip-cost-router`'s `Determinism::Required` arm cannot name a model rung | read |

The edge node adds a fourth, visible one: with no order-entry venue configured
it places against the in-process matching engine and says
`ORDER DESTINATION: the in-process simulated exchange. No order leaves this
process, no socket is opened` (**ran**). Selecting the REST adapter needs the
operator to write the destination out, and the node refuses a live-class
venue at two seams (`.claude/rules/domains/observability.md`,
`qip_edge_refusals_total{gate="live_venue"}`).

If a task seems to need a live-order path, stop and ask. That request has
never yet been legitimate.

### 4.1 Reading the state

```sh
backend/target/debug/qip status                       # ran
curl -s -H "Authorization: Bearer $QIP_TOKEN_MONITOR" $API/api/v1/health   # ran (health); Monitor is the role the route requires
curl -s -H "Authorization: Bearer $QIP_TOKEN_VIEWER"  $API/api/v1/system/status   # ran
```

Tokens are the four roles `Monitor`, `Viewer`, `Analyst`, `Operator`, read from
`QIP_TOKEN_<ROLE>` or `QIP_TOKEN_<ROLE>_FILE`. `QIP_TOKEN_APPROVER` and
`QIP_TOKEN_APPROVER_FILE` are retired and **stop the process** if set.

### 4.2 Halting the central plane

**Tripping needs only the Operator credential and no second person. If you
are unsure whether to stop, stop.** A false stop costs minutes; a missed one
costs whatever the platform does next.

```sh
curl -s -X POST -H "Authorization: Bearer $QIP_TOKEN_OPERATOR" \
  "$API/api/v1/kill-switch?reason=<url-encoded reason>"
```

**ran**: HTTP 200, `{"halted":true,"by":"operator@env","reason":"runbook
drill","broadcast":null}`, after which `/health` read `"status":"halted"` and
`/system/status` read `"autonomy":"observation"` with `configured_autonomy`
still `paper_trading`. `broadcast` is `null` because no mesh was configured;
with `QIP_MESH_CELLS` set the same call also broadcasts the halt to the cells,
best effort, and the counts it returns say what reached them.

Two details the older runbooks get slightly wrong or leave out:

- The reason is read from the **query string** (`?reason=`), not a JSON body
  (`routes.rs`, `request.query_param("reason")`). `reconciliation-break.md`
  shows `-d '{"reason": ...}'`; the code does not read it. The call still halts,
  with the default reason `halted through the API`.
- It does not cancel working orders, flatten positions or lower the autonomy
  ceiling. Those are separate decisions ([kill-switch](../operations/kill-switch.md)).

**Clearing the halt is not available through the API.** `DELETE
/api/v1/kill-switch` returns **HTTP 403** for every caller (**ran**), because
a standing bearer token carries no authentication instant and the
fifteen-minute freshness gate cannot be satisfied (ADR 0065). The only lift
today is **restarting `qip-api`**, which starts unhalted, because the switch is
not resumed from the log. Consequences, all from
[kill-switch](../operations/kill-switch.md): the clearance record (who decided
it was safe) is *not* written, so write the decision down by hand; and on the
`memory` store the restart also discards the event chain. The console is
read-only and can trip the switch but has no path that clears one (**ran**,
banner).

### 4.3 Halting a cell

A cell has three independent halt wires, each a `source` label on
`qip_edge_halted` (`kill_switch`, `policy`, `polled`; a fourth, `journal`, is
written only by a cell built with `CellConfig::with_journal_wire`, which no
composition root in `backend/crates/apps` does today).

The wire an operator with a shell controls is the **polled flag**
(`QIP_HALT_FLAG_PATH`, an absolute path; relative or empty is refused). Drilled
locally (**ran**):

```sh
echo "engaged: <reason>" > <flag path>     # halted within one pass; qip_edge_halted{source="polled"} 1
rm <flag path>                            # released; qip_edge_halted{source="polled"} 0
```

Semantics, from `halt.rs` and `cell.rs`: file absent means released; file
present and empty, or `engaged`, or `engaged:<reason>` means halted; `released`
means released; anything else, a flag over 256 bytes, an unreadable file, or a
**missing directory** means halted, because an unmounted volume must not
release a kill switch. The node log printed `polled halt flag is engaged:
runbook drill; the cell is halted` and later `polled halt flag is absent; the
cell is running` (**ran**).

On the execution node the template puts the flag at `/run/qip/halt/engaged`
(`startup.sh.tftpl`, **repo**): `touch /run/qip/halt/engaged` to halt, `rm` to
release, root-owned so the service user can read but not clear it. `/run` is a
tmpfs, so a reboot clears it; the broadcast halt is what survives a reboot.
No node exists (section 1), so this has never run on a node.

The **broadcast** halt (`kill_switch`) is engage-only: "there is no release
command, because release is a fresh policy decision and rides a newer signed
payload" (`Cell::apply_halt`). No operator command in this tree issues that
payload. Treat a broadcast-halted cell as halted until the centre's policy
path exists in a deployment.

## 5. The binaries

One paragraph each here; the full table of keys, ports, probes and failure
modes per binary is [runbooks/binaries.md](runbooks/binaries.md).

| Binary | Role | Default listen | Probe | Metrics | Refusal exit |
|---|---|---|---|---|---|
| `qip-api` | REST and SSE API, operator console HTML, mesh endpoints, central plane | `127.0.0.1:8080` (`QIP_API_ADDRESS`; `0.0.0.0:8080` in the manifest) | `GET /api/v1/health` (Monitor) | `GET /api/v1/metrics` | 1 |
| `qip-fastbrain` | Fast path; hosts model-free agents only | `0.0.0.0:8080` (`QIP_FASTBRAIN_HEALTH_ADDRESS`) | `/health`, `/ready` | `/metrics` | 78 |
| `qip-deepbrain` | Research loop; may consult a language model | `0.0.0.0:8080` (`QIP_DEEPBRAIN_HEALTH_ADDRESS`) | `/health`, `/ready` | `/metrics` | 78 |
| `qip-edge-node` | One regional cell | `0.0.0.0:8080` (`QIP_HEALTH_PORT`) | JSON body on any path except `/metrics` | `/metrics` | 78 |
| `qip-fabricd` | Event fabric broker | none | none | none | **1, always** |
| `qip-ledgerd` | Ledger single writer | none | none | none | **1, always** |
| `qip-web` | library | n/a | n/a | n/a | n/a |
| `qip` | operator CLI | n/a | n/a | n/a | 0 ok, 1 could not answer, 3 verdict |

Exit 78 is `EX_CONFIG`: "this node was deployed wrong", as opposed to "this
node broke", so an orchestrator can stop restarting it. All exit codes in the
table were **ran** except the CLI's 3 (**repo**, `qip-cli/src/main.rs`).

## 6. CI/CD

All five workflows are in `.github/workflows/`. There is **one** deployment
path and [the deployment path](../operations/deployment-path.md) is its
authority; this section is the operator's summary of it. None of the five has
been dispatched by me.

| Workflow | Trigger | What it does | Never |
|---|---|---|---|
| `ci.yml` | push to `main` and `claude/**`; pull request to `main` | The gate. Jobs: `format`, `clippy`, `test`, `release-build`, `dependency-policy`, `security-audit`, `dependency-supply-chain`, `sbom`, `frontend-portal`, `frontend-landing`, `trivy`, `trunk`, `infrastructure`, `secrets` | Deploys |
| `deploy.yml` | `workflow_run` of `ci` on `main` (success only); `workflow_dispatch` with `environment` in dev, test, stage, prod | `gate` (refuses a commit ci did not pass), `images` (build, scan, push by commit tag, sign and attest), `deploy` (rolls execution node groups; moves no Cloud Run service) | Deploys prod without a human dispatch: the first step exits 1 with `prod is not deployed automatically.` |
| `infra.yml` | `workflow_dispatch` only; `environment` in dev, test, stage | Actions `plan`, `up`, `apps`, `suspend`, `down`, `teardown`, `force-unlock`, `diagnose` | Touches prod: not in the choice list and refused again in the first step |
| `image.yml` | `workflow_dispatch` only; dev, test, stage | `bake`: builds the execution node's Compute Engine image from an attested `qip-edge-node` image. A base image must be a full self-link, never a family | Runs on push: a machine created by a push is a bill nobody decided on |
| `vendor.yml` | push to `main` touching `infrastructure/egress/vendored-images.txt`; `workflow_dispatch` | `mirror`: copies each third-party image by digest into the environment's registry, proves the digest survived, scans, attests | Admits a tag |

### 6.1 Promoting dev to stage

The designed path (ADR 0036) is: `deploy.yml` produces attested images; a Kargo
`Warehouse` sees them; the `dev` Stage promotes automatically by committing the
digests into `infrastructure/gitops/envs/dev/`; Argo CD syncs and a post-sync
hook proves the serving revision carries that digest; `test` and `stage`
promote on **a person's approval in Kargo**, the same commit shape.

**None of this has run.** Kargo has never made a promotion commit
(`git log --all --format='%an' | grep qip-kargo` found nothing when the
deployment-path page was written; **not re-run**), and `stage` has
`project_id = "unprovisioned"`, which every workflow refuses. So today there is
no promotion to do. When there is:

1. `gcloud container fleet memberships get-credentials qip-dev-gitops --project algorik-platform-dev` (**repo**, `infrastructure/gitops/README.md`; **not run**)
2. `kubectl -n kargo port-forward svc/kargo-api 8081:443` (**repo**; **not run**), then promote in the Kargo UI or `kargo` CLI.
3. Record the promotion commit sha, the Argo CD sync id and the hook's `serves` line, which is the evidence the deployment-path page asks for.

**Prod: needs a human dispatch, and is never automated.** `deploy.yml` accepts
a `prod` dispatch from a person; `infra.yml` and `image.yml` do not offer it at
all; the prod Kargo Stage is refused by policy until an ADR lifts it
(`prod_is_promoted_by_nobody_until_an_adr_says_otherwise`). Production
infrastructure is applied by a person running `scripts/bootstrap-deploy.sh
prod`. Do not write automation that does any of it.

### 6.2 Rolling back a deploy

A rollback is a git operation: `git revert <promotion sha>` on the default
branch, or promote earlier freight in Kargo. Not rollbacks: re-dispatching
`deploy.yml` at an older commit (it moves no service), `infra.yml up`
(Terraform no longer manages the services), `gcloud run services update` by
hand (undone by `selfHeal` in dev). **Not exercised**; no promotion has ever
happened, so no revert has either.

## 7. Terraform operations

Root: `infrastructure/terraform/`. Environments: `infrastructure/environments/<env>/terraform.tfvars`.
Provider `hashicorp/google ~> 6.12`; backend is `gcs` with prefix `qip/state`.

- **State bucket**: `<project_id>-qip-tfstate`, created by
  `scripts/bootstrap-deploy.sh` (`readonly STATE_BUCKET="${PROJECT}-qip-tfstate"`).
  For dev that is `algorik-platform-dev-qip-tfstate`, which **does not exist**
  today (`gcloud storage buckets list` listed 0 items, **ran**).
- **Identity**: workload identity federation only; no service-account keys. The
  workflows derive provider, service account, attestor and key version from
  `project_id`, `project_number` and `region` in the tfvars, and the pool admits
  this repository's `refs/heads/*` only. No `${{ vars.* }}` is allowed in a
  workflow. The apply identity is `qip-infra-<env>`; the first apply and prod use
  your own identity impersonating `claude-builder@<project>` via the bootstrap
  script, so the audit log names a person.
- **First apply of an environment**: `scripts/bootstrap-deploy.sh <env>`
  (**repo**; **not run**). It checks tools, enables the project APIs, creates the
  bootstrap account and the state bucket, runs `terraform init` and `apply`
  showing the plan and asking, grants the infra account its one delete right on
  the state bucket, and seeds the six generated secrets. It never auto-approves
  and is idempotent, so re-running after a partial failure is the recovery.
- **Order, once an environment exists**: `infra.yml` with `action=plan` (read
  the plan), then `up` (applies with `-auto-approve` because the dispatch is the
  review, then runs the controllers' bootstrap), then `apps`. After a `teardown`,
  `up` first *reclaims* surviving KMS keys and buckets by import; the
  deployment-path page records why (run 44: `Plan: 184 to add, 0 to change, 0 to
  destroy`).
- **Local checks that need no cloud** (**repo**, `Makefile`; **not run**):
  `make infra` is `terraform fmt -check -recursive`, the tfvars format check,
  `./scripts/check-manifests.py` and `terraform validate`. `terraform test` is
  a fourth gate that `make infra` does not run; CI discovers every
  `*.tftest.hcl` (`infrastructure/CLAUDE.md`).
- **Adding an environment** (`infrastructure/environments/README.md`): a project
  of its own, never one another environment uses; its own state bucket;
  `project_id` (and the number) in a new `environments/<env>/terraform.tfvars`.
  `var.environment` is validated against four names, so a fifth name is a
  `variables.tf` change, and `adr_0035_authorises_one_shadow_node_in_dev_and_none_anywhere_else`
  refuses any `execution_nodes` entry outside dev.
- **Suspending the one idle cost**: `infra.yml` `action=suspend` (section 10).
- **Teardown needs the owner's explicit approval, naming the environment.**
  `infra.yml` `action=teardown` deletes the Cloud Run services and the
  control-plane cluster and runs a targeted destroy of every module but
  `module.services` and `module.cicd`; five buckets and the KMS keys stay, out
  of state. It is not recoverable ("there is no thirty-day window"). The
  repository's guard hook blocks agent-issued teardowns; do not route around it.
  On 2026-09-13 one teardown happened on the owner's instruction (ADR 0040).

## 8. Secrets

- **Files, never environment values.** Secret Manager secrets are mounted into
  Cloud Run as 0400 volumes (`versionRef: latest`), and the binary is told the
  path with a `_FILE` variable, read through `qip_core::secret`. The dev API
  manifest shows seven such volumes: Alpaca key id and secret key, capital
  envelope key, and the four role tokens. A secret that is both set directly
  and by `_FILE` is refused.
- **Terraform creates the secret, never a version**, because a version has a
  value and a value in state is a leaked credential. Values are written out of
  band: `gcloud secrets versions add <secret> --data-file=-` (**repo**,
  `docs/operations/provisioning-managed-services.md` and
  `docs/operations/enabling-live-trading.md`; **not run**). `scripts/bootstrap-deploy.sh`
  step 7 and `infra.yml` seed the generated ones and never overwrite one.
- **Rotation**: each secret carries a 90-day rotation schedule
  (`rotation_period = "7776000s"`, first `next_rotation_time` 2026-12-01 in
  `modules/secrets/main.tf`) that publishes a notice to the
  `qip-secret-rotation-<env>` topic. Nothing rotates a value for you: the
  notice means a person adds a new version. After adding one, **restart the
  consumer**: `qip-api` reads its tokens once at start-up (ADR 0065), so a new
  version reaches it only on a new instance. Whether a running Cloud Run
  instance sees a new `latest` without a restart was not tested.
- **KMS** keys rotate automatically only when symmetric; asymmetric signing keys
  (the Binary Authorization attestor) are a deliberate sequence, and disabling
  the old version early refuses running images one at a time
  (`docs/operations/external-dependencies.md`).
- **The capital envelope key.** Without one the process falls back to a
  seed-derived key and prints `SEED-DERIVED — reproducible, so anyone who knows
  the seed can mint a grant; live trading is refused under this key` (**ran**).
  Acceptable for a local run, and for a deployment only because live is refused
  anyway. `qip-edge-node` refuses to start without `QIP_CAPITAL_ENVELOPE_KEY` or
  its `_FILE` form.
- **Never** put a token, key, hostname or account id in code, a comment, an
  example or a commit message. `./scripts/check-secrets.sh` must pass before
  any commit (**ran**: `secret scan: nothing found`, exit 0).

## 9. Observability: what is emitted, what is scraped

**Do not describe this platform as observable.** Every process emits;
nothing has been shown to scrape any of them.

- **Emitted** (**ran** for each): `qip-api` at `/api/v1/metrics`;
  `qip-fastbrain` and `qip-deepbrain` at `/metrics` on their health port;
  `qip-edge-node` at `/metrics` on `QIP_HEALTH_PORT` (`qip_edge_*` series,
  including `qip_edge_halted{source}`). `/metrics` on a process that has not yet
  run a cycle is short rather than empty, which is honest, not a gap.
- **Scraped: nothing.** The execution node's startup script declares an Ops
  Agent Prometheus receiver on the health port (`startup.sh.tftpl`), but no node
  exists. No Cloud Run service has a collector: the managed-Prometheus sidecar
  is rendered only under `metrics_collector_image_digest`, null everywhere,
  because the only version Google publishes (`cloud-run-gmp-sidecar` 1.9.2)
  failed the platform's Trivy gate on an unfixed CRITICAL (`vendor.yml` run 11,
  `CVE-2026-56854`). The blocker is upstream. Do not clear it with a scanner
  exception. Record: `infrastructure/terraform/modules/observability/NOT-SCRAPED.md`.
- **Alerts**: nine policies in `modules/observability/main.tf`, all gated on
  `workload_metrics_exist`, which is `false` in every environment. Recount
  with `grep -c '^resource "google_monitoring_alert_policy"'
  infrastructure/terraform/modules/observability/main.tf` (**not run** here;
  `docs/ops/observability/README.md` gives 9). Two of them fire on a control
  *working* (risk figure unevaluated, sign-off withheld on liquidity): the
  platform stopped trading that book, it did not trade badly.
- **Logs**: each process writes its banner and run log to stdout and stderr
  (**ran**). `QIP_OPENOBSERVE_URL` (with `_ORG`, `_AUTHORIZATION`,
  `_INTERVAL_SECS`) turns on a drain to OpenObserve; unset, the banner says
  `not draining ... telemetry stays local`. OpenObserve v0.92.2 is vendored by
  digest and has a manifest; it is not deployed.
- **Flipping `workload_metrics_exist` to `true`** is allowed **only** with
  evidence that a scrape was *observed*: a `prometheus.googleapis.com/qip_*`
  descriptor present in Cloud Monitoring for that project (Cloud Monitoring
  refuses a policy naming a descriptor it has never ingested). A deployment that
  merely exists is not evidence. Procedure: obtain the evidence, set the flag in
  that environment's tfvars in a reviewed change, `infra.yml plan`, read it, then
  `up`. **Not run.** Today there is no evidence and no running process.

## 10. Cost control

- **Ceiling: 25 USD per day**, which is 750 USD per month by arithmetic
  (`HERMES_MISSION.md`, `DECISIONS.md`). Neither the ceiling nor the budget
  stops spend: a billing budget only notifies.
- **Budget**: `DECISIONS.md` (2026-10-04) records a 750 USD/month budget on the
  billing account alerting at 50, 70, 90 and 100 percent, created by hand with
  `gcloud`, and `billingbudgets.googleapis.com` enabled. I did not re-verify the
  budget (**not run**). Separately, `modules/observability/budget.tf` declares the
  same budget in Terraform behind `billing_budget_enabled`, which is `false` by
  default; it needs `billing_account_id`, passed outside committed tfvars. The
  hand-made one and the Terraform one must not both be enabled without a
  decision about which owns it.
- **Spend today**: nothing is deployed, so expected cloud spend is close to
  zero; I did not read a spend figure (**not run**), and there is no
  billing-export dataset in this tree.
- **How to check spend**: the Billing reports page in the Cloud Console for the
  billing account, filtered to project `algorik-platform-dev`. `gcloud billing
  projects describe algorik-platform-dev` (**ran**) only says whether billing
  is attached. No repository command reports a spend figure.
- **Kill list, in the order they bill while idle** (from the `infra.yml` header
  and the manifests; none exists in `algorik-platform-dev` today):
  1. The **control-plane cluster** (GKE Autopilot, Argo CD, Kargo, Config
     Connector): bills while idle. `infra.yml` `action=suspend` destroys it and
     nothing else (ADR 0093); a later `up` recreates it.
  2. **`qip-fastbrain` and `qip-deepbrain`**: pinned to exactly one instance
     (`minInstanceCount: 1`, `maxInstanceCount: 1`, `gitops/envs/dev/*.yaml`)
     because each runs a loop on its own clock over one hash-chained log; a
     second instance forks the chain. They bill by the second with no requests.
     There is no workflow action for this; it is a manifest change.
  3. **Execution nodes** (Compute Engine C3): none, `execution_nodes = {}`.
     `infra.yml` `action=down` destroys them and is a no-op today.
  4. Everything else (`qip-api`, the portal, OpenObserve) scales to zero
     (`minInstanceCount: 0`).
- **Never** delete a cloud resource without approval naming it, and never widen
  an IAM grant to make an error go away.

## 11. Backup and restore

State which are proven. **None is proven.** Nobody has restored from any of
this (`docs/operations/disaster-recovery.md`).

| State | Backed up by | Proven? |
|---|---|---|
| Event log hash chain | Whatever `QIP_STORAGE_TARGET` names. In every declared deployment that is `memory`: **no backup exists** | No. `qip-cli replay` and `ChainArchive::verify` verify a chain; the replay test exists (`a_clean_journal_replays_into_three_identical_registries_and_the_run_exits_zero`), not run here |
| Execution node journal | Daily Compute Engine snapshot schedule `modules/backup`, retained 90 days by default, **but it covers nothing until attached** to a disk | No: no node, no disk |
| Evidence bucket | Versioned, KMS-encrypted, locked retention, `force_destroy = false` | Configuration only |
| Terraform state | A versioned GCS bucket the bootstrap creates | The bucket does not exist yet |
| Positions and open orders | **Not backed up, by design.** Reconcile from the venue; never restore | Principle stated in code and docs |

Attaching the snapshot schedule after a node's first boot (**repo**,
`docs/operations/disaster-recovery.md`; **not run**):
`terraform -chdir=infrastructure/terraform output -raw journal_snapshot_attachment_command`,
then run what it prints. Restoring a journal is read-only onto an
operator-controlled machine, **never** onto a serving node, and verified with
`verify_continuity` and `verify_against`; no operator command wraps those yet.

## 12. Incident response

The playbooks are in [runbooks/incidents.md](runbooks/incidents.md): a halted
cell, a reconciliation break, fabric spool pressure, a ledger consumer stall,
and a failed deploy. Two of them (spool pressure, ledger stall) describe
components that cannot run in this tree; the page says so and says what to do
instead.

## 13. The agent fleet

The development and research agent fleet on GCP (up to 40 single-packet Cloud Run
Job tasks calling Vertex AI under the 25 USD/day ceiling) is **planned and not running**.
[ADR 0102](../adr/0102-the-development-and-research-fleet-is-up-to-forty-single-packet-cloud-run-job-tasks-on-vertex-ai-under-a-25-usd-day-ceiling-and-it-supersedes-adr-0098s-claude-only-clause.md) is written and its status is
**Proposed**; its own text separates what was observed from what is unproven. No
fleet exists, no Cloud Run Job exists (the Cloud Run API is not enabled in
`algorik-platform-dev`, **ran**, section 1.1), and this runbook describes no
fleet operation. Today's agents are the in-process roster each brain hosts (18 agents
declared; `qip-fastbrain` hosts one model-free agent, `qip-deepbrain` hosts 17,
**ran**, both banners), governed by `AgentManifest::validate`. The development
factory that works on this repository from the owner's desktop is ADR 0098.

## 14. Known gaps

Copied from measured facts in [the baseline](hermes-baseline-2026-10-04.md),
[SYSTEM_MAP](../SYSTEM_MAP.md), and the checks in this document. Nothing here
is a forecast.

| Gap | Fact | Source |
|---|---|---|
| Nothing deployed in dev | Cloud Run API not enabled; 0 buckets in `algorik-platform-dev` | **ran**, section 1.1 |
| Billing | Enabled on `algorik-platform-dev`; was disabled on `algorik-dev` | **ran** / baseline s3 |
| Durable storage | Every declared deployment is `memory`; the audit chain does not survive a restart | dev `api.yaml`; banners |
| Kill switch cannot be cleared by API | `DELETE /api/v1/kill-switch` returns 403 for everyone; only a restart lifts it, and writes no clearance record | **ran**; ADR 0065 |
| GitOps control plane | Suspended; Argo CD image fails the Trivy CRITICAL gate; bootstrap never succeeded; Kargo never promoted | ADR 0093; deployment-path |
| Fabric and ledger | `qip-fabricd` and `qip-ledgerd` refuse to start; archiver, config and health are stubs; plaintext TCP, RF1, bearer-token design | **ran**; ADR 0100; SYSTEM_MAP S39, S05 |
| Execution nodes | `execution_nodes = {}` in all four environments; no boot image, no allocation | tfvars |
| Scraping | Nothing scrapes anything; every alert policy is absent | NOT-SCRAPED.md |
| Cell-to-cell mesh | Not built; only cell-to-centre exists | SYSTEM_MAP S28, E03 |
| Central plane shares no edge | fast brain and deep brain each build their own `Platform` and have `invokers = []`; how they share state is unstated | SYSTEM_MAP E17, U21 |
| Subsystems not built | 13 of 44 subsystem rows (for example NOW Brain, forecast lattice, symbolic reasoning, AIOps); 2 blocked (market creation, physical commerce) | SYSTEM_MAP s2.1 |
| Cost enforcement | The 25 USD/day ceiling is enforced by nothing; the budget alerts only | SYSTEM_MAP s3; DECISIONS.md |
| Backup | No restore has ever been performed; the snapshot schedule attaches to nothing | disaster-recovery.md |
| Node count | CLAUDE.md and ADR 0008 say seven cells; `qip-edge/src/mesh.rs` says nine; the blueprint says three regions | SYSTEM_MAP U14 |
| Local tooling | Playwright not installed; local Terraform v1.15.8 vs CI 1.9.8 pin | baseline s4 |
| Tests (last full run) | 6502 passed, 0 failed, 0 ignored; I did not rerun the workspace suite | `STATUS.md` |
| Stale doc | `reconciliation-break.md` shows a JSON body for the halt reason; the route reads the query string | section 4.2 |
| Stale doc | `SYSTEM_MAP.md` section 3 says no `google_billing_budget` exists; `modules/observability/budget.tf` now declares one behind a flag | `budget.tf` |

## 15. Where the rest lives

- [Operations index](../operations/README.md) · [kill switch](../operations/kill-switch.md) · [deployment path](../operations/deployment-path.md) · [disaster recovery](../operations/disaster-recovery.md) · [reconciliation break](../operations/reconciliation-break.md) · [limit breach](../operations/limit-breach.md) · [reinstating a venue](../operations/reinstating-a-venue.md)
- [Observability](observability/README.md) · [policies](policies/README.md) · [security](security/README.md)
- [System map](../SYSTEM_MAP.md) · [roadmap](../MASTER_ROADMAP.md) · [ADR index](../adr/README.md)
