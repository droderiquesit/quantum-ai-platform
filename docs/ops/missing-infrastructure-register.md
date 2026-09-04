# The missing-infrastructure register

What the blueprint requires, what `infrastructure/terraform` provides, and the
delta between them — one row per gap, with the evidence on both sides.

This is the second of two registers and it asks a different question from the
first. `docs/ops/off-gates-register.md` asks which switches are closed in every
environment. This one asks what has no switch at all: a resource the blueprint
names that no module creates, a module input no root ever assigns, a workload
that runs in the project without passing through this configuration.

## What counts as a gap here

Not every absence is one. This tree is full of argued absences —
`execution_nodes = {}` (`infrastructure/environments/dev/terraform.tfvars:75-77`),
a null collector digest, `workload_metrics_exist = false` — and each carries a
paragraph at the point of absence saying why. A documented absence is a
decision, and a decision is not a gap however inconvenient it is. The test
applied to every row below is:

  * Is the thing required by ADR 0022, ADR 0024 or blueprint §46.1?
  * Does some Terraform create it?
  * If not, is there a sentence somewhere in the tree that says it is absent
    and why — and is that sentence still true?

A row is a **gap** only when the third answer is no. Rows where the third
answer is yes but the sentence has expired are collected separately, under
*Stale decisions*, because the action there is to rewrite a paragraph rather
than to build a resource.

## Method, and what was not done

Every quotation below was read in the file at the line cited, on branch
`claude/algorik-architecture-refactor-pmp0zy`.

**No Terraform ran.** There is no `terraform` binary in this environment. No
plan, no `validate` and no `fmt` output backs any statement here, and the
Google APIs were not called either. Every claim about what a plan would create
is read off a `count`, a `for_each`, a `precondition` or a validation block in
source, and every claim about what exists in the `algorik-dev` project is read
off a file in this repository that says so — never observed. Where that
distinction changes the severity of a row, the row says it again.

**No module is instantiated nowhere.** The audit that produced this file
anticipated that class and it is empty: eighteen directories under
`infrastructure/terraform/modules/`, seventeen `module` blocks in `main.tf`
and two in `catalogue.tf`, `modules/cloudrun` being the one instantiated twice
(`catalogue.tf:324` and `catalogue.tf:425`). Counted with

    ls infrastructure/terraform/modules/ | wc -l
    grep -c '^module "' infrastructure/terraform/main.tf
    grep -n 'source *= *"\./modules' infrastructure/terraform/main.tf infrastructure/terraform/catalogue.tf

Recount before quoting those numbers; an earlier document in this repository
carried a module count from a version of the file that no longer existed.

**One cited file moved while this was being written.**
`infrastructure/environments/dev/terraform.tfvars` was edited by a concurrent
change mid-audit: `execution_nodes = {}` moved from line 67 to line 77, the
identity block moved by about twenty lines, and
`vendored_openobserve_image_digest` went from commented out to set. Every
citation of that file below was re-read after the change and is current as of
this writing, and gap 8 records what the change did to its own severity. A line
number in a file a hundred agents share is the weakest part of any citation
here; the quoted text is the part to match on.

## Severity

**BLOCKING-A-GATE** — a control this repository claims to hold does not hold,
or a test asserts a property of a subset it does not name. Reading the
configuration gives a reviewer a false belief about the platform.

**BLOCKING-DEPLOY** — a resource the blueprint requires that nothing creates,
so a deployment of it cannot be planned at all.

**COSMETIC** — a document, comment or scorecard row that is wrong about a tree
that is right. Nothing behaves differently; a reader is misled.

## The register

| # | Gap | Severity |
|---|---|---|
| 1 | Two Cloud Run services exist that no Terraform creates and the deny-by-default admission policy never evaluates | BLOCKING-A-GATE |
| 2 | The landing runs as the project's default compute identity, and the test forbidding exactly that does not look where it happens | BLOCKING-A-GATE |
| 3 | The console-egress subnet is the one subnet with no egress deny | BLOCKING-A-GATE |
| 4 | The portal's session secret reaches the process as an environment value | BLOCKING-A-GATE |
| 5 | `ledger_database` and `control_fabric_topic` are module inputs no root assigns | BLOCKING-DEPLOY |
| 6 | The control fabric of §46.1 is Pub/Sub and no resource of any kind exists | BLOCKING-DEPLOY (decision, listed for completeness) |
| 7 | `catalogue.tf`'s "deliberately not here" list omits the two workloads that are most deliberately not there | COSMETIC |
| 8 | The §2.1 scorecard row still reads "no third-party SaaS at runtime" after ADR 0028 | COSMETIC |
| 9 | The LAYER 1 scorecard row is right about the world and wrong about this repository | COSMETIC |

---

### 1. Two Cloud Run services exist that no Terraform creates and the deny-by-default admission policy never evaluates

**Severity: BLOCKING-A-GATE.**

**Required.** ADR 0024:31-34 — "Every warm binary is a Cloud Run service from
`infrastructure/terraform/catalogue.tf` through `modules/cloudrun` — internal
ingress, secrets mounted as volumes and never as environment values, an image
pinned by the digest `deploy.yml` attested."

**Exists.** `local.cloud_run_catalogue` has exactly three keys: `api`
(`catalogue.tf:52`), `fastbrain` (`:148`), `deepbrain` (`:222`), and
`catalogue.tf:12` says so — "Three binaries today". `deploy.yml`'s matrix names
the same three plus `qip-edge-node` (`.github/workflows/deploy.yml:161-165`).
Neither names the portal or the landing.

The two of them are deployed instead by `scripts/deploy-frontends.sh` —
`gcloud run deploy algorik-portal` at `:98` and `gcloud run deploy
algorik-landing` at `:133`, against a literal `PROJECT="algorik-dev"` at `:18`,
with the image referenced by the tag `${AR}/algorik-portal:${SHA}` (`:100`,
`:135`) where `SHA` is `git rev-parse --short HEAD` (`:21`).

That they run there rather than in the catalogue is said once, in a comment on
an unrelated resource: `modules/network/main.tf:124-125`, "The portal runs on
Cloud Run outside the catalogue — `scripts/deploy-frontends.sh` deploys it".

**The delta.** `modules/cloudrun/main.tf:550-552` sets

    binary_authorization {
      use_default = true
    }

on every catalogue service, and `:920-922` does the same for the job shape.
That is what makes Cloud Run evaluate the project policy. The policy is
`modules/binaryauthorization/main.tf:228-232`: `REQUIRE_ATTESTATION` by the
build attestor, `ENFORCED_BLOCK_AND_AUDIT_LOG`, with
`admission_whitelist_patterns` empty because `exempt_image_patterns` is
deliberately not surfaced from the root (`main.tf:545-549`).

`scripts/deploy-frontends.sh` passes no `--binary-authorization` flag on either
deploy. A Cloud Run service that does not opt in is not evaluated against the
policy. So the two services that face the public internet
(`--allow-unauthenticated`, `:101` and `:136`) are the two the platform's only
admission control does not see, and they are the two whose images are named by
a mutable tag rather than by a digest anything signed.

The module's own opening comment names precisely this failure mode in its
previous form (`modules/binaryauthorization/main.tf:1-8`): "enforcement was
switched on against no policy … and refused nothing while reading as though it
did. A switch that reports 'on' and admits every image is worse than an absent
control, because a reviewer reads the line and stops looking." The policy is
real now. The hole moved: it is no longer in the policy, it is in which
workloads are subject to it, and the same sentence applies unchanged.

**Not a documented absence.** `catalogue.tf:16-29` is the list of things
deliberately not in the catalogue — `qip-edge-node`, `QIP_MESH_CELLS`, a job —
and the frontends are not on it. `docs/adr/0010-what-gets-deployed.md` is where
`deploy.yml:130-131` says exclusions are decided, and the matrix comment
(`deploy.yml:137-141`) accounts for `qip` and `qip-web` and not for these.
There is no ADR arguing that a browser surface is deployed by hand. The one
sentence that records the arrangement is a comment on a subnet.

**Evidence that they are running, and its limits.** Google Cloud was not
queried. `infrastructure/environments/dev/terraform.tfvars:137-138` says the
authorized domains "are the Cloud Run URLs a deploy actually printed (never
invented, per the variable's contract)", and lists
`algorik-portal-rgxpsss2lq-uk.a.run.app` and
`algorik-landing-rgxpsss2lq-uk.a.run.app` (`:141-145`, the two hostnames at `:143-144`). That is this repository
asserting the services exist, which is the strongest evidence available here
and is not observation.

**Paper trading.** Both surfaces are browser applications, which by
`.claude/rules/domains/frontend.md` hold no trading logic and no risk logic.
Nothing in this row touches the three layers that hold the paper boundary.

---

### 2. The landing runs as the project's default compute identity, and the test forbidding exactly that does not look where it happens

**Severity: BLOCKING-A-GATE.**

**Required.**
`backend/crates/tests/qip-acceptance/tests/infrastructure.rs:981`,
`fn no_workload_runs_as_the_projects_default_compute_identity`, whose comment
gives the reason: "The default compute service account is shared by everything
in the project that does not name one; a grant given to it for one workload is
a grant given to all of them."

**Exists.** The test reads two paths — `CLOUD_RUN_MODULE` and `NODE_MODULE`
(`infrastructure.rs:985`) — and asserts neither contains
`compute@developer.gserviceaccount.com`. Both pass, correctly.

**The delta.** `scripts/deploy-frontends.sh:98-108` deploys the portal with
`--service-account "${CONSOLE_SA}"` (`:102`). The landing deploy at `:133-138`
has no `--service-account` flag at all. `gcloud run deploy` without one uses
the project's default compute service account — the exact identity the test is
named after. The test cannot fail on it, because the script is not one of the
two paths it reads.

The adjacent test is what makes this a gate failure rather than a bug.
`infrastructure.rs:959-970` enumerates every service account Terraform creates
and asserts the set is exactly five, the fifth being `("secrets", "console")`
with the comment "The portal, deployed by `scripts/deploy-frontends.sh`." The
suite therefore already knows that script deploys workloads, and already
reasons about the identities they carry. It reasons about one of the two.

---

### 3. The console-egress subnet is the one subnet with no egress deny

**Severity: BLOCKING-A-GATE.**

**Required.** ADR 0024:38-39 — "The thirteen trust zones of blueprint §46.1 are
`modules/trust-zones`, default deny in both directions."

**Exists.** `modules/trust-zones/main.tf:259-279` writes the deny, and its
comment (`:255-258`) states the invariant exactly: "Delete a path rule and the
zone stops talking; delete this one and the zone talks to everything, which is
why it is declared per zone rather than assumed from the platform default." It
is `for_each = var.zones` and scoped `target_tags = [local.zone_tag[each.key]]`
(`:275`). The execution node carries its own equivalent at
`modules/execution-node/main.tf:474-479`.

**The delta.** `modules/network/main.tf:130-147` creates
`qip-${var.environment}-console-egress` outside the zone model — its comment
says why (`:127-129`): "its own range rather than a share of a zone's, because
the console is not a trust zone and a range it drew from would be a zone with
a second tenant." The argument is sound and the consequence is unwritten: an
interface in that subnet carries no zone tag, so no `deny_egress` rule targets
it, and Terraform's implied allow-all egress at priority 65535 — the thing
`:255-258` says the per-zone rule exists to sit above — is what applies.

Verified by enumerating every subnet and every egress rule in the tree:

    grep -n 'resource "google_compute_subnetwork"\|direction *= *"EGRESS"' \
      infrastructure/terraform/modules/network/main.tf \
      infrastructure/terraform/modules/trust-zones/main.tf \
      infrastructure/terraform/modules/execution-node/main.tf

Three subnet resources — `network.console_egress:130`, `trust-zones.zone:190`,
`execution-node.node:109` — and `modules/network/main.tf` declares no egress
rule of any kind. Its only firewall is `deny_ingress` at `:51-68`, network-wide
at priority 65534. Ingress is covered. Egress is not.

The portal is partly held by `--vpc-egress private-ranges-only`
(`scripts/deploy-frontends.sh:105`), which is a Cloud Run setting on a service
no Terraform manages, not a firewall rule. The landing attaches to no VPC at
all.

---

### 4. The portal's session secret reaches the process as an environment value

**Severity: BLOCKING-A-GATE.**

**Required.** ADR 0024:31-34 — "secrets mounted as volumes and never as
environment values". `.claude/rules/01-security-and-safety.md` gives the
reason: "a key in the environment is a key in `/proc/<pid>/environ`, every
child process, and every crash dump."

**Exists.** The catalogue mounts secrets as files: each entry carries a
`secret_mounts` map (`catalogue.tf:100`, `:203`, `:258`), passed to the module
at `:373`, and `modules/cloudrun/main.tf:627-632` renders each as a
`volume_mounts` block at `${local.secret_root}/${key}`. The script does the
same for the platform token: `scripts/deploy-frontends.sh:80` defines
`TOKEN_MOUNT="/var/run/secrets/qip/token-viewer"`, and
`${TOKEN_MOUNT}=qip-token-viewer-dev:latest` in the `--set-secrets` argument at
`:107` is a path, so Cloud Run projects it as a file. That half is right, and
the script argues it correctly at `:75-79`: "an environment variable holding a
token is readable from /proc/<pid>/environ, is inherited by every child, and
lands in a crash dump."

**The delta.** The same `--set-secrets` argument's first element is
`ALGORIK_SESSION_SECRET=algorik-session-secret:latest`. A `--set-secrets` entry
whose left-hand side is a name rather than a path sets an environment variable.
The session secret — the key that signs the console's cookies — is in the
portal's environment, in the same comma-separated argument, four lines below
the comment that states why it must not be.

The script's header (`:10-12`) says "Session secret: generated once into Secret
Manager, never printed, injected by Cloud Run — nothing here or in the image
carries it." Every clause of that is true and it is describing the wrong
hazard: the concern is not the script or the image, it is the process.

No secret value appears in this register or in the script; both name secret
ids only.

---

### 5. `ledger_database` and `control_fabric_topic` are module inputs no root assigns

**Severity: BLOCKING-DEPLOY.**

**Required.** `modules/trust-zones/main.tf:543-561` grants
`roles/spanner.databaseReader` and `roles/spanner.databaseUser` on the ledger
by declared path mode; `:568-590` grants `roles/pubsub.publisher` and
`roles/pubsub.subscriber` on the control fabric. Four resources, and the
variables' descriptions (`modules/trust-zones/variables.tf:376-411`) argue the
null default carefully: "a deployment whose ledger has not been provisioned
gets no bindings rather than bindings against a database that does not exist."

**Exists.** The `module "trust_zones"` block, `main.tf:305-325`, passes
`project_id`, `environment`, `region`, `network_id`, `zones`,
`permitted_paths`, `external_egress`, `public_ingress` and `zone_identities`.
It passes neither `ledger_database` nor `control_fabric_topic`. Verified with

    grep -rn "ledger_database\|control_fabric_topic" infrastructure/

Every hit is inside `infrastructure/terraform/modules/trust-zones/`. No root
file, no environment tfvars, nothing else in the tree names either.

**The delta.** This is a different class from `execution_nodes = {}`. That is a
root variable an operator sets in a tfvars file to change the outcome. These
two have no root variable: no tfvars value can reach them, so the four
resources cannot be created from this root at all, in any environment, ever.
`modules/data/outputs.tf:38-41` already publishes `spanner_database`, so one of
the two inputs has a value sitting one module away from the module that needs
it and no wire between them.

The null default is argued and correct. What is missing is the argument for why
nothing can ever set it — which nothing anywhere makes.

---

### 6. The control fabric of §46.1 is Pub/Sub and no resource of any kind exists

**Severity: BLOCKING-DEPLOY — and a documented decision, listed here because a
register of what is missing that omits the largest missing thing is not a
register.**

**Required.** Blueprint §46.1's control fabric, as ADR 0024:177-178 states:
"The blueprint's control fabric is Pub/Sub (§46.1), and building it is work
this record names and does not do."

**Exists.** One Pub/Sub topic in the whole configuration, and it is not this
one: `modules/secrets/main.tf:78`, the secret-rotation topic, whose own comment
(`:54-56`) says it "is not a contradiction of ADR 0011. That decision replaced
Pub/Sub as the *data* bus". `modules/services/main.tf:56` enables the API for
that topic and no other.

**The delta.** Nothing. There is no resource, and the absence is stated twice in
the tree — ADR 0024:172-178 and `catalogue.tf:21-27` — which is what keeps it
out of the gap list proper. Its consequence is gap 5: the fabric bindings in
`modules/trust-zones` are written against a topic that does not exist and
cannot be named. **This decision is not stale.**

---

### 7. `catalogue.tf`'s "deliberately not here" list omits the two workloads that are most deliberately not there

**Severity: COSMETIC.**

**Required.** Nothing external. The list is the catalogue's own account of its
boundary, `catalogue.tf:16-29`.

**Exists.** Three entries: `qip-edge-node` (`:18-20`), `QIP_MESH_CELLS` on the
API (`:21-27`), and a job (`:28-29`).

**The delta.** The portal and the landing — the only two workloads this
repository deploys outside the catalogue, by `scripts/deploy-frontends.sh` —
are not on it. A reader of the catalogue's own exclusions list learns that the
execution node and a job are outside it and is not told that two internet-facing
Cloud Run services are too. The tree behaves correctly; the account of it is one
list with two omissions.

---

### 8. The §2.1 scorecard row still reads "no third-party SaaS at runtime" after ADR 0028

**Severity: COSMETIC.**

**Required.** `docs/architecture/algorik-blueprint-traceability.md:61` scores
blueprint §2.1 ALIGNED on the ground that managed services are "GCP + IBM
Quantum; no third-party SaaS at runtime", citing
`infrastructure/terraform/modules/`.

**Exists.** `catalogue.tf:425-460` is `module "openobserve"`, a second
instantiation of `modules/cloudrun` running a vendored third-party image
(`image_source = "vendored"`, `:460`) under ADR 0028.

**The delta.** The row predates the module, and it is now wrong about the plan
as well as the tree. `count = var.vendored_openobserve_image_digest != null ? 1 : 0`
(`catalogue.tf:427`) was the mitigation: with the digest unset, no service was
created and the row was merely out of date. `environments/dev/terraform.tfvars:133`
now sets that digest, so `dev` plans a third-party service. The row still says
"no third-party SaaS at runtime".

COSMETIC is the severity because a scorecard row misleads a reader and changes
nothing; but this is the row in this register most likely to be promoted, and
the promotion condition has already half fired. Whoever fixes it should check
the digest's state rather than trusting this paragraph — `dev/terraform.tfvars`
was being edited by another change while this audit ran, and the line number
above may have moved again.

---

### 9. The LAYER 1 scorecard row is right about the world and wrong about this repository

**Severity: COSMETIC.**

**Required.** `docs/architecture/algorik-blueprint-traceability.md:301`, the
LAYER 1/7 row: "*Current:* Next.js portal and landing on Cloud Run".

**Exists.** That is a true statement about the `algorik-dev` project, on this
repository's own evidence (`environments/dev/terraform.tfvars:137-138,143-144`). It is a
false statement about this repository, which is what the scorecard scores:
nothing in `infrastructure/terraform` puts either on Cloud Run.

**The delta.** One row, reading as though the catalogue covers them. It is the
row that hid gap 1 — a reader checking whether the frontends are configured
finds a row saying they are on Cloud Run and stops.

---

## Stale decisions

Not gaps. Absences that are argued, where the argument no longer holds. The
action is a paragraph, not a resource, and it belongs to the owner of the file.

### `execution_nodes = {}` — half-stale

ADR 0024:186-187 gives one reason: "`execution_nodes` is empty in every
environment, because a node needs a venue and no venue decision exists." Half of
that has been overtaken. `6340610` gave the node a venue for the purpose of
running a pass — the in-process simulated feed, written by
`modules/execution-node/templates/startup.sh.tftpl:174`, with every other value
stopping the process naming ADR 0003. What still holds is the other half, which
the tfvars states and the ADR does not:
`environments/dev/terraform.tfvars:75-76` points at
`modules/execution-node/README.md` "for the entry a node needs when a venue's
published ranges exist", and no venue's ranges have been recorded. The decision
survives; one of its two reasons has expired, and the surviving one is written
in the weaker of the two places.

### ADR 0024's closing sentence — stale

ADR 0024:191-195: "this repository now has one runtime in its Terraform and none
observed. Until a plan is read and applied by a person, 'the platform runs on
Cloud Run' is a statement about a configuration, and the honest sentence is that
the platform is not running anywhere."

That was true when written. Three things in the tree now say otherwise:
`environments/dev/images.tfvars:1-12` records three digests, its header
attributing them to run 33780092495 for commit `c3140aff` and stating each was
"built, scanned, signed and attested … moved onto its Cloud Run service, and
proven serving before this line was written";
`environments/dev/terraform.tfvars:137-138,143-144` names two hostnames "a deploy
actually printed"; `modules/network/main.tf:124` describes the portal in the
present tense. None of it was observed here — these are this repository's own
claims — but a document whose point is honesty about deployment state should not
be the last file to hear about one.

### `workload_metrics_exist = false` — not stale

The gate is evidence that something scraped.
`modules/observability/NOT-SCRAPED.md` holds the argument and nothing has
changed it. All seven alert policies in `modules/observability/main.tf` remain
gated on it (`grep -c 'resource "google_monitoring_alert_policy"'` returns 7).

### Null image digests, no proxy on the fast brain, no Cloud Run job — not stale

Each fails closed and each says so at the point of absence: the digest
precondition at `catalogue.tf:306-312`, the fast-brain proxy precondition at
`:314-320`, and the job at `:28-29`.

## What this register does not cover

Switches closed in every environment are the other register's subject:
`docs/ops/off-gates-register.md`. Its `notification_channels` finding — the root
default `[]` (`infrastructure/terraform/variables.tf:381-385`) reaching all
seven policies — is a real gap of that kind and is not restated here. Two
registers disagreeing about one switch is the failure this pair exists to avoid.

## The four tests this register wants and could not add

Every test in this repository lives under `backend/crates/`, which this audit
does not own. The designs are recorded here so the owning change inherits a
finished specification rather than a complaint. All four belong in
`backend/crates/tests/qip-acceptance/tests/infrastructure.rs`, beside
`no_workload_runs_as_the_projects_default_compute_identity` — cross-cutting,
reading two sides of a seam, which is where
`.claude/rules/architecture/01-testing-strategy.md` puts them.

| Test | Asserts | Premise asserted first | Mutation that must make it fail |
|---|---|---|---|
| `every_cloud_run_service_this_repository_deploys_is_subject_to_the_admission_policy` | Each `gcloud run deploy` in `scripts/deploy-frontends.sh` carries `--binary-authorization=default`; `modules/cloudrun/main.tf` still contains `use_default = true` | Assert the script contains **two** `gcloud run deploy` occurrences before checking their flags — a test that iterates zero deploys passes forever | Delete `--binary-authorization=default` from the landing block only. Must fail naming `algorik-landing`, not "no deploys found" |
| `no_workload_runs_as_the_projects_default_compute_identity` (**extend**, do not add a second) | Add `scripts/deploy-frontends.sh` to the paths read; every `gcloud run deploy` names `--service-account` | Same two-deploy premise | Remove `--service-account` from the **portal** block, the one that has it today. Must fail naming `algorik-portal` — proving the test reads both deploys and not only the one that was broken when it was written |
| `every_subnet_in_the_network_is_covered_by_an_egress_deny` | Every `google_compute_subnetwork` across `modules/network`, `modules/trust-zones` and `modules/execution-node` is matched by a deny-egress rule that can target it | Assert the subnet set is non-empty and contains `console_egress` by name — the exact "filtered a list, asserted empty" trap `01-testing-strategy.md` names | Delete the `target_tags` line at `modules/trust-zones/main.tf:275`, making the zone deny global. The test must **still fail**, now on over-broad coverage rather than under — a coverage test that only fails in one direction is half a test |
| `no_secret_this_repository_deploys_reaches_a_process_as_an_environment_value` | Every `--set-secrets` entry in `scripts/deploy-frontends.sh` has a left-hand side beginning `/` | Assert at least one `--set-secrets` argument was parsed | Change `${TOKEN_MOUNT}=qip-token-viewer-dev:latest` to `QIP_TOKEN=qip-token-viewer-dev:latest`. Must fail naming `QIP_TOKEN`. Note for the implementer: `contains("ALGORIK_SESSION_SECRET")` is the substring trap — match the delimited left-hand side of each comma-separated entry |

## Follow-ups, and who owns each

| # | Work | Path | Severity |
|---|---|---|---|
| F1 | Add `--binary-authorization=default` to both deploys; move the image reference from tag to digest | `scripts/deploy-frontends.sh:98-108,133-138` | BLOCKING-A-GATE |
| F2 | Give the landing a named service account, created in `modules/secrets` beside `console` | `scripts/deploy-frontends.sh:133`, `modules/secrets/main.tf` | BLOCKING-A-GATE |
| F3 | Cover the console-egress subnet with a deny-egress rule | `modules/network/main.tf:130` | BLOCKING-A-GATE |
| F4 | Mount `ALGORIK_SESSION_SECRET` as a file; the portal reads it through the `_FILE` indirection its `secret.ts` already resolves | `scripts/deploy-frontends.sh:107` and `frontend/portal` | BLOCKING-A-GATE |
| F5 | Wire `ledger_database` from `module.data.spanner_database`, or record why it can never be wired | `infrastructure/terraform/main.tf:305-325` | BLOCKING-DEPLOY |
| F6 | Rewrite ADR 0024:186-195 and traceability rows `:61` and `:301` | `docs/adr/`, `docs/architecture/` | COSMETIC |
| F7 | One `docs/ops/README.md` edit adding both registers to the index — neither is listed today, so adding one alone makes the index more wrong | `docs/ops/README.md` | COSMETIC |
| F8 | Decide whether the frontends join `catalogue.tf` or an ADR argues they stay out; either way `catalogue.tf:16-29` gains an entry | `catalogue.tf`, `docs/adr/` | COSMETIC to BLOCKING |

F1 has an honest alternative — an `exempt_image_patterns` entry — which
`infrastructure/terraform/main.tf:545-549` argues against by name, so choosing
it is a decision and possibly an ADR rather than an edit. F2 changes the
five-account set `infrastructure.rs:959-970` pins, so it lands with that test in
one commit. F8 is the root question behind gaps 1, 2, 3, 4 and 7: F1 to F4 patch
the script, and F8 decides whether the script should exist.

## The paper-trading boundary

Intact and untouched by this audit. Nothing here changes
`infrastructure/terraform/variables.tf`, an `AutonomyLevel::deployable` call
site, or `qip-edge`'s `Cell`, which are the three layers
`.claude/rules/01-security-and-safety.md` names. The dev ceiling is
`paper_trading` and this register asserts nothing that would relax it. Gaps 1 to
4 concern browser surfaces and the network around them, and browser surfaces
hold no trading logic by `.claude/rules/domains/frontend.md`.
