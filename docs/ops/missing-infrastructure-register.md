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

**Terraform ran, and did not query Google.** A `terraform` binary is present
at `/usr/local/bin/terraform` (v1.9.8). From `infrastructure/terraform`,
`terraform init -backend=false` succeeded, `terraform fmt -check -recursive`
exited 0 with no output (nothing to reformat), and `terraform validate`
printed `Success! The configuration is valid.` — all re-run and read on
2026-09-04 for this correction. None of the three calls the Google APIs, so
the earlier caveat about the `algorik-dev` project stands unchanged: every
claim about what exists in that project is read off a file in this repository
that says so — never observed. What has changed is narrower — the syntax and
internal references of the HCL are now machine-checked, not merely read.
Every claim about what a plan would *create*, as opposed to whether the
configuration is well-formed, is still read off a `count`, a `for_each`, a
`precondition` or a validation block in source, because `validate` does not
evaluate those against real state and no `plan` was run. Where that
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
| 8 | The §2.1 scorecard row still reads "no third-party SaaS at runtime" after ADR 0028 — OpenObserve is now deployed and serving | BLOCKING-A-GATE (promoted; was COSMETIC) |
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
`backend/crates/tests/qip-acceptance/tests/infrastructure.rs:1076`,
`fn no_workload_runs_as_the_projects_default_compute_identity`, whose comment
gives the reason: "The default compute service account is shared by everything
in the project that does not name one; a grant given to it for one workload is
a grant given to all of them." (Re-derived by
`grep -n "fn no_workload_runs_as_the_projects_default_compute_identity" backend/crates/tests/qip-acceptance/tests/infrastructure.rs`
on 2026-09-04; the register previously cited `:981`, which has drifted.)

**Exists.** The test reads two paths — `CLOUD_RUN_MODULE` and `NODE_MODULE`
(`infrastructure.rs:1080`, `for path in [CLOUD_RUN_MODULE, NODE_MODULE]`) —
and asserts neither contains `compute@developer.gserviceaccount.com`. Both
pass, correctly.

**The delta.** `scripts/deploy-frontends.sh:98-108` deploys the portal with
`--service-account "${CONSOLE_SA}"` (`:102`). The landing deploy at `:133-138`
has no `--service-account` flag at all. `gcloud run deploy` without one uses
the project's default compute service account — the exact identity the test is
named after. The test cannot fail on it, because the script is not one of the
two paths it reads.

The adjacent test is what makes this a gate failure rather than a bug.
`infrastructure.rs:1033-1073`
(`fn every_service_account_terraform_creates_runs_something_or_signs_something`;
the register previously cited `:959-970`, which has drifted) enumerates every
service account Terraform creates and asserts the set is exactly five, the
fifth being `("secrets", "console")` at `:1064` with the comment "The portal,
deployed by `scripts/deploy-frontends.sh`." at `:1063`. The suite therefore
already knows that script deploys workloads, and already reasons about the
identities they carry. It reasons about one of the two.

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

**ADR 0031 does not reach this row.** Since this register was first written,
ADR 0031 amended `.claude/rules/01-security-and-safety.md`'s blanket
"never as environment variables" — but narrowly: `modules/cloudrun` gains a
`secret_env` input that "projects a Secret Manager version into an
environment variable through Cloud Run's own `value_source.secret_key_ref`,
and it is **refused unless `image_source == "vendored"`**. The platform's own
binaries cannot reach it, in any environment, by any tfvars edit."
(`docs/adr/0031-a-vendored-workload-may-take-a-secret-as-an-environment-value.md:12-16`).
`image_source` defaults to `"built"` for every catalogue workload (ADR 0028
decision 3) and the only value in the tree set to `"vendored"` is
`catalogue.tf:468`'s `module.openobserve` — confirmed by
`grep -n "image_source" infrastructure/terraform/catalogue.tf`, one hit. The
portal is not that module, and is not in `modules/cloudrun` at all: it is
deployed by `scripts/deploy-frontends.sh`'s own `gcloud run deploy`, entirely
outside Terraform (gap 1). ADR 0031's `secret_env` is a Terraform input on a
module the portal never passes through, so the exception cannot structurally
reach it even in the most generous reading, independent of the "built" vs
"vendored" distinction. **Gap 4 stands.**

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

**Severity: BLOCKING-A-GATE, promoted from COSMETIC.** The promotion
condition this row itself named — "the promotion condition has already half
fired" — has now fully fired: OpenObserve is not merely planned, it is
running. `docs/plan/gate-completion-plan.md:54` states "**OpenObserve is now
deployed and serving** at its Cloud Run URL, anonymous on the public internet
under ADR 0030, with its own login enforced (the API answers 401
unauthenticated)." A reader of the §2.1 row who trusts "no third-party SaaS
at runtime" now holds a false belief about a Cloud Run service that is
actually running in the project — the definition of BLOCKING-A-GATE in this
register's own terms, not merely a document out of date about a plan.

**Required.** `docs/architecture/algorik-blueprint-traceability.md:61` scores
blueprint §2.1 ALIGNED on the ground that managed services are "GCP + IBM
Quantum; no third-party SaaS at runtime", citing
`infrastructure/terraform/modules/`.

**Exists.** `catalogue.tf:425-473` is `module "openobserve"`, a second
instantiation of `modules/cloudrun` running a vendored third-party image
(`image_source = "vendored"`, `:468` as of this correction; re-derived by
`grep -n "image_source" infrastructure/terraform/catalogue.tf`, one hit — the
register previously cited `:460`) under ADR 0028.

**The delta — no longer conditional.** `count = var.vendored_openobserve_image_digest != null ? 1 : 0`
(`catalogue.tf:427`) was the mitigation this row originally relied on: with the
digest unset, no service was created and the row was merely out of date.
`environments/dev/terraform.tfvars:139` (re-derived by
`grep -n vendored_openobserve_image_digest infrastructure/environments/dev/terraform.tfvars` —
the register previously cited `:133`) sets that digest, so `dev` plans a
third-party service, and — beyond planning — `docs/plan/gate-completion-plan.md:54`
records it "now deployed and serving" on a real Cloud Run URL. The row still
says "no third-party SaaS at runtime". This is no longer a row that is merely
out of date about a plan; it is wrong about a running service.

The promotion this paragraph once called "half fired" is now complete, which
is why the severity above moved to BLOCKING-A-GATE. Whoever fixes it should
re-check the digest's state and the deploy record rather than trusting this
paragraph on its own — both files are shared with concurrent changes and a
line number here is the weakest part of the citation.

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

## Re-scored at HEAD

Four ADRs landed after this register's rows were first written. None of them
close a numbered gap outright, and one closes a long-open question this
register's own domain rule (`observability.md`) tracked as "A6". Read in
full before citing; one line each below is the decision, not the argument.

- **ADR 0032** — telemetry from all three central roots drains to a collector
  running **inside the VPC** on plaintext OTLP, forwarding to OpenObserve on
  its own TLS; no root drains to a public URL directly.
- **ADR 0033** — OpenObserve moves from anonymous to Identity-Aware-Proxy
  authenticated *before* the collector's first byte reaches it, firing the
  condition ADR 0030 wrote for itself ("the moment any deployment sets
  `QIP_OPENOBSERVE_URL`").
- **ADR 0034** — names the first three egress-allowed vendor sources (Coinbase
  Exchange first) for market data and a prediction feed, each with a
  licensing-posture record ahead of use.
- **ADR 0035** — deploys **exactly one** execution node, in `us-east4`, in
  shadow mode, in `dev` only; `execution_nodes` stays `{}` in test, stage and
  prod.

**A6 (the managed-Prometheus collector for Cloud Run, tracked in
`.claude/rules/domains/observability.md`) is REFUSED, not open.** ADR 0032
names the collector image as "the vendored, digest-pinned,
Binary-Authorization-attested image A6 has been blocked on" and states
plainly what would make the decision wrong: "If the collector image cannot be
vendored and attested. That is the whole premise." That is exactly what
happened. `infrastructure/egress/vendored-images.txt` records the review of
`cloud-run-gmp-sidecar` (Google's own image, confirmed by reading
`confgenerator.Version=1.9.2` out of its entrypoint binary) and its result:

    run-gmp-entrypoint (gobinary)  Total: 1 (CRITICAL: 1)
    rungmpcol (gobinary)           Total: 1 (CRITICAL: 1)
    golang.org/x/crypto  CVE-2026-56854  CRITICAL  fixed
      installed v0.54.0, fixed 0.55.0
      golang.org/x/crypto/ssh: authentication bypass due to unenforced
      source-address restrictions

and "There is no patched release to move to. The registry's tag list carries
1.0.0 through 1.9.2 and nothing above it... The adoption is therefore blocked
upstream, not here." The line is left commented in `vendored-images.txt`, not
merely unreviewed — "resolved but nobody looked" and "looked at and refused"
are different states, and this is the second one. `workload_metrics_exist`
therefore cannot flip on this path; ADR 0032's own fallback (a second, narrow
egress-proxy route, accepting the public-internet hop under ADR 0033's
mitigation) is the remaining route, and nothing in this tree has taken it yet.

**Re-checked 2026-09-05, and the refusal stands on today's evidence rather
than on 2026-09-04's.** The registry's tag list
(`https://us-docker.pkg.dev/v2/cloud-ops-agents-artifacts/cloud-run-gmp-sidecar/cloud-run-gmp-sidecar/tags/list`,
anonymous, HTTP 200) carries the version tags `1.0.0 1.1.0 1.1.1 1.2.0 1.3.0
1.4.0 1.6.0 1.7.0 1.8.0 1.9.1 1.9.2` and `latest`; `latest` and `1.9.2` both
name `sha256:ff1fc68871118f1032a3ce17e2b0abd703292e883989d220244330ebdf522fd1`
(uploaded 2026-07-15), the digest run 11 refused. Nothing newer is published.
The twenty manifests the registry gained on 2026-09-05 are not images: each is
an Artifact Analysis attachment (`application/vnd.in-toto.vuln+dsse` or
`.triage+dsse`, `artifactregistry.attachment_namespace:
artifactanalysis.googleapis.com`) whose `subject` is one of the existing
tagged digests. Google's own vulnerability attestation on `ff1fc688…`
(`scanFinishedOn: 2026-09-04T18:40:40-07:00`) lists CVE-2026-33997,
CVE-2026-84304 and GHSA-hrxh-6v49-42gf with every triage decision
`TRIAGE_STATUS_UNSPECIFIED` — nothing Google has waived, and nothing that
names the finding this platform's gate fires on. A Trivy 0.74.0 scan of that
digest from this checkout on 2026-09-05 (`trivy image --severity
CRITICAL,HIGH --ignore-unfixed --exit-code 1 --scanners vuln
us-docker.pkg.dev/…/cloud-run-gmp-sidecar@sha256:ff1fc688…`, the same flags
`vendor.yml` runs, DB from `mirror.gcr.io/aquasec/trivy-db:2`) exited 1:

    run-gmp-entrypoint (gobinary)  Total: 10 (HIGH: 9, CRITICAL: 1)
    rungmpcol (gobinary)           Total: 12 (HIGH: 11, CRITICAL: 1)
    golang.org/x/crypto  CVE-2026-56854  CRITICAL  fixed  v0.54.0 -> 0.55.0

The CRITICAL is the one run 11 found; the HIGHs (grpc-go CVE-2026-84304 and
GHSA-hrxh-6v49-42gf, and nine Go 1.26.4 stdlib advisories fixed in 1.26.6)
are new to the database since, not to the image. `vendor.yml`'s refusing
pass is `--severity CRITICAL --exit-code 1`, so the image still fails on
exactly the finding it failed on before. No digest was added to
`vendored-images.txt` and no tfvars line changed. Re-check the tag list
before the next observability pass; the condition for adoption is unchanged
— a published tag carrying `x/crypto >= 0.55.0`, scanned to zero CRITICAL.

**F1–F4 are in progress this wave, not closed.** Other agents are working the
follow-ups this register lists (`scripts/deploy-frontends.sh`'s missing
`--binary-authorization` flags, the landing's missing service account, the
console-egress subnet's missing deny rule, and the session secret's
environment-variable mount). Re-read as of this correction,
`scripts/deploy-frontends.sh` still has no `--binary-authorization` flag on
either deploy and still sets
`ALGORIK_SESSION_SECRET=algorik-session-secret:latest` as a `--set-secrets`
name rather than a path (`:115`) — gaps 1 and 4 are unchanged in fact. This
register does not mark F1–F4 done, and whoever closes one should update this
file rather than leave the row to be re-audited from scratch.

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

## Observed on 2026-09-04, from outside the project

The first entry in this register that rests on an observation of
`algorik-dev` rather than on a file that says something about it. Everything
above this heading was read off the tree; this section says what was seen,
what could not be, and which rows move.

**What could not be done, and why.** The session held a Google access token
in `CLOUDSDK_AUTH_ACCESS_TOKEN` for read-only use. It was fourteen characters
long, and every Google API refused it the same way — Cloud Run, Compute
Engine, Binary Authorization, Secret Manager and Cloud Monitoring, one call
each:

    "code": 401, "status": "UNAUTHENTICATED",
    "reason": "ACCESS_TOKEN_TYPE_UNSUPPORTED"

`terraform init -input=false -backend-config="bucket=algorik-dev-qip-tfstate"`
against the real state bucket failed on the same credential:

    Error: Failed to get existing workspaces: querying Cloud Storage failed:
    googleapi: Error 401: Invalid Credentials, authError

So the serving revisions, digests, traffic splits, service accounts, secret
volumes, the instance groups, the attestations, the secret-version counts,
the real drift plan and the `qip_*` metric descriptors were **not
observed**. No row below claims any of them. The token was not echoed,
written or quoted.

**What ran instead.** From a copy of `infrastructure/` in a scratch
directory with a `backend "local"` override — the working tree was not
touched — `terraform init` succeeded and

    terraform plan -input=false -refresh=false -lock=false \
      -var-file=../environments/dev/terraform.tfvars \
      -var-file=../environments/dev/images.tfvars
    Plan: 164 to add, 0 to change, 0 to destroy.

with no error and no warning. Against empty state that is not a drift
measurement — every resource is "to add" — but it is more than `validate`:
every precondition in `catalogue.tf`, every `for_each`, every validation
block and the provider schema were evaluated against the committed dev
inputs and all passed. The two data sources that read the project
(`module.evidence.data.google_storage_project_service_account.storage`,
`module.binary_authorization.data.google_kms_crypto_key_version.attestor`)
were deferred to apply, so the bogus credential was never presented. In the
real tree, `terraform fmt -check -recursive` exited 0 and `terraform validate`
printed `Success! The configuration is valid.`

**What was observed.** Unauthenticated `GET`s over the public internet, which
is the whole of what a credential-less session can see. The discriminator
is the shape of a 404: a `*.run.app` hostname with no service behind it
answers `404 Page not found` with no `server` header (control:
`qip-dev-definitely-not-a-service-rgxpsss2lq-uk.a.run.app`), while a service
with internal ingress answers Google Frontend's `404 Not Found` — `server:
Google Frontend`, "The requested URL / was not found on this server".

| Hostname | Answer | What it shows |
|---|---|---|
| `algorik-portal-rgxpsss2lq-uk.a.run.app` | `200`, title "Algorik — paper trading" | The portal serves, publicly, and renders the paper-trading label |
| `algorik-landing-rgxpsss2lq-uk.a.run.app` | `200`, title "Algorik — AI and quantum research…" | The landing serves, publicly |
| `qip-dev-openobserve-rgxpsss2lq-uk.a.run.app` | `308 -> /web/`; `/web/` `200`; `/api/default/` `401 {"code":401,"message":"Unauthorized Access"}` | OpenObserve exists, is reachable anonymously (ADR 0030), and its own login is enforced on the API |
| `qip-dev-api-rgxpsss2lq-uk.a.run.app` | Google Frontend `404 Not Found` | A service exists at the catalogue's name with internal ingress |
| `qip-dev-fastbrain-rgxpsss2lq-uk.a.run.app` | Google Frontend `404 Not Found` | Same |
| `qip-dev-deepbrain-rgxpsss2lq-uk.a.run.app` | Google Frontend `404 Not Found` | Same |
| `qip-dev-nothing-95200532413.us-east4.run.app` (control) | `404 Page not found`, no `server` header | The shape of "no such service", on the deterministic URL form too |

One limit worth writing down so nobody repeats the probe: `/healthz` on
*any* `*.run.app` hostname — the landing's, OpenObserve's — is answered by
Google's own generic 404 page before the request reaches a container,
while `/healthzz` reaches the application. An external `GET /healthz`
therefore says nothing about `catalogue.tf:488`'s probe path; the evidence
that path is right is that the revision is Ready and serving, because the
startup probe runs inside the instance.

**Rows that move.**

| # | Gap | Severity |
|---|---|---|
| 1 | The two frontends: `scripts/deploy-frontends.sh` now deploys both by digest (`:200`, `:242`) with `--binary-authorization=default` (`:208`, `:250`). Fixed in the tree; both hostnames observed serving. Whether the *running* revisions carry the flag was not observed | Was BLOCKING-A-GATE; closed in code, unverified in the project |
| 2 | The landing has its own identity: `modules/secrets/main.tf:277` creates `landing`, `deploy-frontends.sh:252` passes it, and the acceptance suite's five-account set became six. Fixed in the tree; the running service's identity was not observed | Was BLOCKING-A-GATE; closed in code, unverified in the project |
| 3 | `modules/network/main.tf:185-199` is `console_egress_deny_egress`, with the Google-APIs allow above it at `:223-238` (`fbb73a7`). Fixed in the tree; not observed | Was BLOCKING-A-GATE; closed in code, unverified in the project |
| 4 | `deploy-frontends.sh:216` now mounts the session secret at `${SESSION_SECRET_MOUNT}`, a path, so it is a file. Fixed in the tree; not observed | Was BLOCKING-A-GATE; closed in code, unverified in the project |
| 8 | OpenObserve observed serving anonymously at its Cloud Run URL, exactly as gap 8 said. The §2.1 row is now contradicted by an observation, not only by a file | BLOCKING-A-GATE, confirmed |
| 10 | `infrastructure/CLAUDE.md:33-35` said "Nothing in this directory has been applied by an agent; the first real plan is a human's to read" while four services it describes were serving. Rewritten in this change to say what was applied and what was observed | COSMETIC, fixed |

The paragraph above headed "F1–F4 are in progress this wave" is superseded
by rows 1–4 here: each is closed in the tree as of this observation. What
none of them has is the second half — a `gcloud run services describe`, or
the Cloud Run API, showing the running revision carries the flag, the
identity and the mount. That is one authenticated read per service and it
is the next thing to do with a credential that works.

**Stale decisions, re-read.** ADR 0024's closing sentence — "the platform is
not running anywhere" — is now contradicted by observation and not only by
this repository's own files. `workload_metrics_exist = false` is **not**
stale: the Monitoring API could not be queried, so no descriptor was seen,
and a gate that stays closed for want of evidence is doing its job.

**What this section does not claim.** Nothing about which digest any
service serves, whether it matches `images.tfvars`, whether Binary
Authorization was evaluated on its revision, which secrets have a version,
whether any execution node exists, or whether anything has scraped. Each of
those is one `GET` with a real token and none of them was made.

## Applied by ADR 0040's record: infra.yml `dev` `up`, runs 34 and 35

ADR 0040 says the dispatch's run URL and terminal status are recorded here,
beside what the apply produced. Two dispatches, both on commit `e1711fb`,
both on 2026-09-05, both **failure**:

| Run | URL | Dispatched | Plan | Terminal status |
|---|---|---|---|---|
| 34 | https://github.com/droderiquesit/quantum-ai-platform/actions/runs/33944963018 | 04:35Z | 83 to add | failure — 82 created, the cluster refused |
| 35 | https://github.com/droderiquesit/quantum-ai-platform/actions/runs/33946596091 | 05:12Z | 1 to add, 0 to change, 0 to destroy | failure — the same cluster, the same refusal |

**What run 34 created.** State went from 163 to 234 resources: 82 of the 83
the plan named. The six Cloud Run objects the root's `removed` blocks
release under ADR 0036 left state undestroyed, as intended; every identity,
key, grant, zone and bucket of `modules/gitops-control-plane` except the
cluster now exists in `algorik-dev`. Run 35's plan, read before its
dispatch, was the one remaining resource.

**What both refused.** `module.gitops_control_plane[0].google_container_cluster.control_plane`
(`modules/gitops-control-plane/main.tf`, the `resource "google_container_cluster"`
block), with one `googleapi: Error 400` carrying two messages:

    addons {"config-connector"} are not supported for Autopilot clusters
    generic::permission_denied: Permission 'gkehub.memberships.create' denied
      on 'projects/algorik-dev/locations/us-east4/memberships/qip-dev-control-plane'

The first is a contradiction inside ADR 0036 itself — decision 1 wants
Autopilot and "installed as the GKE addon" in the same sentence, and the
API admits one of the two. The second is the register of missing
permissions gaining an entry: neither `roles/gkehub.gatewayEditor` nor
`roles/gkehub.viewer`, the two the module grants the infrastructure
account, carries the permission the cluster's `fleet` block needs at
create time.

**The fix, in this commit, awaiting re-dispatch.** Autopilot stays, because
it is the property that keeps a `qip-*` image off the cluster; the addon
goes. Config Connector's operator is now a vendored, digest-pinned manifest
under `infrastructure/gitops/bootstrap/config-connector-operator/`
(1.156.0, the Autopilot variant; `SOURCE.md` there records the bundle, the
hashes and the image digest), one more line in
`infrastructure/egress/vendored-images.txt` for `vendor.yml` to mirror and
attest, and one more step in `infra.yml`'s bootstrap between Kargo and the
`ConfigConnector` object, which waits for the operator's CRD before naming
it. The permission is one custom role,
`qipGitopsFleetRegistrar_<env>` — `gkehub.memberships.create`, `.get`,
`.delete` and nothing else — bound to the infrastructure account in the same
module, with the cluster's `depends_on` naming the binding. Not
`roles/gkehub.editor`, which carries every feature and scope in the fleet
API to close one permission.

**What the next `up` still has to prove, and where it will say so.** Three
things this commit asserts and no run has observed. First, that the create
succeeds at all with a binding made seconds earlier: IAM propagation can
refuse a fresh grant, and a second dispatch is the remedy, not a wider
role. Second, that `vendor.yml` has mirrored and attested the operator image
before the bootstrap pulls it — the list change triggers that workflow into
`dev` on push, and the bootstrap's `rollout status` on the operator's
StatefulSet is where an unattested image shows as a Pod that never starts.
Third, and the one this commit could not settle from here: the operator
installs the four `cnrm-system` controllers from `gcr.io/gke-release/cnrm/*`
images pinned inside its own image, which no overlay moves; they are
admitted only if Google's global policy, which `modules/binaryauthorization`
enables, admits that registry, and neither the public documentation nor an
anonymous read of the policy would say. The bootstrap's
`kubectl -n cnrm-system wait` is where that is answered, and a denial there
is a decision about `exempt_image_patterns` for a person, recorded in
`bootstrap/config-connector-operator/SOURCE.md` rather than left to be
rediscovered.

Neither the cluster nor any controller was observed running. Every claim
above about `algorik-dev` is read off the two runs' logs as reported to
this record, and the resource count is the workflow's own `what exists now`
step.

### Runs 36 and 37: the fleet grant held, the addon was gone, and the node could not reach its control plane

Two more dispatches, both on commit `3848a89`, both on 2026-09-05:

| Run | URL | Dispatched | Plan | Terminal status |
|---|---|---|---|---|
| 36 | https://github.com/droderiquesit/quantum-ai-platform/actions/runs/33960966370 | before 10:35Z | 3 to add, 0 to change, 0 to destroy | success — plan only |
| 37 | https://github.com/droderiquesit/quantum-ai-platform/actions/runs/33961066585 | 10:35Z | the same three | failure after 38 minutes — three created, the cluster's node never registered |

**What run 37 created.** State went from 234 to 237:
`google_project_iam_custom_role.fleet_registrar`, its binding
`google_project_iam_member.infra_registers_fleet`, and
`google_container_cluster.control_plane` — all three under
`module.gitops_control_plane[0]`. Both refusals of runs 34 and 35 are
closed: the API accepted the create without the addon and the fleet
membership was registered by the custom role, which is the evidence the
previous section said the next `up` had to produce. The cluster object
exists in `algorik-dev` (`qip-dev-control-plane`, `us-east4`), with its
etcd key, its peering and its one Autopilot node.

**The exact error.** The create call waited for its node and gave up:

    Error waiting for creating GKE cluster: All cluster resources were
    brought up, but: only 0 nodes out of 1 have registered. Node
    gk3-qip-dev-control-plan-default-pool-1b8ecf12-3gg8 experienced kubelet
    errors.

The node's serial log, as reported to this record:

    failed to get node info: node ... not found
    F0905 10:46:11 main.go:78] unable to lock file, error: context deadline exceeded
    Unable to write event ... Post https://10.0.36.2/api/v1/namespaces/default/events:
      getting credentials: exec: executable /home/kubernetes/bin/gke-exec-auth-plugin
      failed with exit code 1
    Failed to ensure lease exists ... Get https://10.0.36.2/apis/coordination.k8s.io/...
      getting credentials: exec ... gke-exec-auth-plugin failed with exit code 1
      (Client.Timeout exceeded while awaiting headers)

**The diagnosis, from the tree.** Every address the node failed to reach is
`10.0.36.2`: the second address of `gitops_master_ipv4_cidr_block =
"10.0.36.0/28"` (`environments/dev/terraform.tfvars`), which is the private
endpoint — a range in a VPC Google peers to ours, so a packet to it leaves
the node's interface and is subject to our egress rules. The cluster puts
the management zone's tag on every node
(`modules/gitops-control-plane/main.tf`, `node_pool_auto_config`), so the
zone's rules are the node's rules, and the zone's rules are
(`modules/trust-zones/main.tf`): `deny_egress`, priority 65000, all
protocols to `0.0.0.0/0`; `google_apis`, TCP 443 to `199.36.153.8/30`; and
the forty `external_egress` rules the root generates for GitHub's ranges on
443 (`main.tf`, `local.github_egress`). Nothing names `10.0.36.0/28`. The
kubelet's registration, its lease, its events and the certificate
`gke-exec-auth-plugin` fetches all go to the endpoint on 443, and every
one of them timed out — which is what a firewall drop looks like from the
inside, and what the serial log shows.

What was checked and found *not* to be the cause, so the next reader does
not re-check it: Private Google Access is on for every zone subnet
(`trust-zones/main.tf`, `private_ip_google_access = true`);
`container.googleapis.com`, `oauth2.googleapis.com`, `logging`, `monitoring`
and `storage` are all under the `googleapis.com.` zone `modules/network`
sends to the restricted VIP by wildcard CNAME, and the zone's `google_apis`
rule admits that /30 on 443; `pkg.dev` and `gcr.io` have their own zones to
the same addresses in the control-plane module; the metadata server
(`169.254.169.254`) is not subject to VPC firewall rules at all; and the
return path — the control plane reaching the node on 443 and 10250 — is the
`gke-<cluster>-<hash>-master` INGRESS rule GKE writes itself at priority
1000, above the zone's 65000 `deny_ingress`. No DNS change was needed and
none was made.

**The fix, in this commit, awaiting re-dispatch.** Two firewall rules in
`modules/gitops-control-plane/main.tf`, under "what a node needs that the
zone's deny refuses", both targeted at the management zone's tag and both
at priority 1000 like every allow the zone writes:

  * `nodes_reach_control_plane` — `qip-dev-control-plane-nodes-to-endpoint`
    — TCP 443, 10250 and 8132 to `var.master_ipv4_cidr_block` and nothing
    wider. 443 is the port the serial log proves; 10250 and 8132
    (Konnectivity, the tunnel the control plane reaches a private cluster's
    nodes back through — admission webhooks, `logs`, `exec`) are what GKE
    documents for a node whose egress is restricted, added now rather than
    found by the next forty-minute wait. The cluster's `depends_on` names
    it, because Terraform infers nothing from a firewall rule and a rule
    created in parallel with the create's wait may arrive after the wait
    gave up.
  * `nodes_reach_each_other` — `qip-dev-control-plane-nodes-intra-cluster`
    — TCP, UDP and ICMP to the zone's own subnet and to the Pod range the
    cluster reports (`cluster_ipv4_cidr`, known only after the cluster
    exists, so this rule follows the cluster). **Not observed by run 37**:
    one node registering needs none of it. Named here because the
    mechanism is certain — Autopilot's Pods hold VPC-native addresses and a
    Pod reaching a Pod on another node leaves the interface and meets the
    same deny — and because every failed create costs another cluster a
    person has to delete (below).

The rule lives in the control-plane module and not in `modules/trust-zones`
because only the control-plane module knows the endpoint's range; the zone
module keeps refusing a path from a zone to itself, and neither rule is a
boundary crossing — the endpoint and the Pod range are this cluster's own.
No `0.0.0.0/0`, no widening of the zone's deny, no IAM change.
`the_control_plane_nodes_may_reach_their_endpoint_and_the_cluster_waits_for_that_rule`
in the acceptance suite pins the first rule's direction, target, destination,
the three ports, its precedence over the zone's deny (read from the zone
module, not assumed) and the cluster's `depends_on`.

**The tainted cluster, and what the next `plan` and `up` will do.** The
provider recorded the cluster in state before its wait failed (the count
went to 237), so Terraform has marked it tainted. The next `plan` will show

    # module.gitops_control_plane[0].google_container_cluster.control_plane is tainted, so must be replaced
    -/+ resource "google_container_cluster" "control_plane" { ... }
      + resource "google_compute_firewall" "nodes_reach_control_plane" { ... }
      + resource "google_compute_firewall" "nodes_reach_each_other" { ... }   # destination_ranges (known after apply)
    Plan: 3 to add, 0 to change, 1 to destroy.

The next `up`, dispatched against that state, will **fail**, on purpose:
the replacement's first half is a destroy, and `deletion_protection = true`
(`modules/gitops-control-plane/main.tf`, the cluster block) makes the
provider refuse it at apply time —

    Error: Cannot destroy cluster because deletion_protection is set to true. Set it to false to proceed.

— so nothing is destroyed, the bootstrap step does not run, and the
`nodes_reach_each_other` rule, which depends on the replacement, is not
created. Whether `nodes_reach_control_plane` is created before the refusal
is a graph-ordering question this record does not settle; the apply log's
`Creation complete` line answers it. `deletion_protection` stays exactly
as it is: it is doing what it was written for, which is to make removing a
cluster a person's act.

**The narrowest path, and why the workflow does not get an `untaint`
action.** Two ways exist to clear a taint. `terraform untaint
module.gitops_control_plane[0].google_container_cluster.control_plane`
destroys nothing and would let a plain `up` add the two rules and change
nothing else — *if* the cluster is worth keeping, which is a fact about
whether its node registered on its own once the rule existed, and the rule
does not exist until an `up` that the taint makes fail. The only identity
that can run `untaint` against this state is the workflow's (Workload
Identity Federation, no key, no local state access), so "the operator runs
`untaint`" means "a fourth `action` in `infra.yml`". This commit does not
add one, for three reasons. First, the acceptance suite refuses the word
`untaint` in `infra.yml`'s commands
(`no_service_can_be_left_tainted_because_terraform_no_longer_creates_one`):
the workflow once had an untaint-on-evidence step for a Cloud Run service
and the repository retired it as a mechanism, not as a bug — a step that
untaints from a workflow is the step that puts a refused object back into
service without the evidence being in front of anyone. Second, an
`untaint` action taking a resource address is a general state-mutation
primitive dispatched by name; guarding it to one address makes it a
one-off encoded in a workflow for ever. Third, the thing a person is
deciding — "this cluster is good" — cannot be observed from the workflow
before the untaint: the node's registration is visible only through the
Connect gateway once the cluster is `RUNNING`, and the cluster is not.
Weakening the test to admit the action would be the change the test exists
to make somebody argue for, and this record argues against it.

What a person does instead, in this order, and what each step costs:

  1. Confirm nothing on the cluster is worth keeping. Nothing is: no
     bootstrap ran (the `up` step failed before it), no controller was
     installed, neither App credential was projected, and the etcd key is
     a separate resource under `prevent_destroy` that the cluster's removal
     does not touch.
  2. Delete the cluster by name, as themselves, in the console or with
     `gcloud container clusters delete qip-dev-control-plane --project
     algorik-dev --region us-east4`. `deletion_protection` is a provider
     attribute, not a GKE one, so the deletion is not refused. Then confirm
     the fleet membership went with it — `gcloud container fleet memberships
     list --project algorik-dev` should not list `qip-dev-control-plane` —
     because a membership left behind is refused by the next create as
     already existing. This is the "deliberate two-step by a person" the
     cluster block names, and the domain rule that a cloud deletion needs
     the resource named by a person is why no agent and no workflow does
     it.
  3. Dispatch `plan`. The refresh finds the cluster gone and drops it from
     state; the expected summary is **3 to add, 0 to change, 0 to destroy**
     — the cluster and the two rules — with no `-/+` and no `tainted`.
  4. Dispatch `up`. The ordering is now the module's: `nodes_reach_control_plane`
     first, the cluster after it, `nodes_reach_each_other` after the
     cluster reports its Pod range, then the bootstrap. Where the next
     failure would show, if there is one: the create's own wait again if a
     port is still missing; the bootstrap's `rollout status` on cert-manager
     if 8132 was wrong or insufficient; the `cnrm-system` wait for the
     admission question the previous section left open.

**What the next `up` still has to prove.** That a node registers with the
rule in place; that Konnectivity's tunnel comes up on 8132 and the
cert-manager webhook rollout succeeds; that Pods on a second node reach
Pods on the first; and everything the previous section listed for the
bootstrap. None of it has been observed. Every claim above about
`algorik-dev` is read off run 37's log as reported to this record and off
the tree at `3848a89` plus this commit.

## The paper-trading boundary

Intact and untouched by this audit. Nothing here changes
`infrastructure/terraform/variables.tf`, an `AutonomyLevel::deployable` call
site, or `qip-edge`'s `Cell`, which are the three layers
`.claude/rules/01-security-and-safety.md` names. The dev ceiling is
`paper_trading` and this register asserts nothing that would relax it. Gaps 1 to
4 concern browser surfaces and the network around them, and browser surfaces
hold no trading logic by `.claude/rules/domains/frontend.md`.
