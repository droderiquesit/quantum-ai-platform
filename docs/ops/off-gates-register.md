# The off-gates register

Every switch in `infrastructure/terraform` that is in its closed position in
**every** environment, what it gates, and whether its being closed is a
decision somebody wrote down or a gap nobody has noticed.

The distinction is the whole point of this file. This tree is full of
deliberate, argued absences — `execution_nodes = {}`, a null collector digest,
`workload_metrics_exist = false` — and each of them carries a paragraph saying
why. A documented absence is a decision. An absence with no paragraph is a
gap, and there are four of them below.

Scope: the root variables in `infrastructure/terraform/variables.tf` and the
four `infrastructure/environments/<env>/terraform.tfvars`, plus the module
defaults those variables reach. Read-only audit; nothing here was changed.

## Method, and what was not done

The set of variables set per environment was computed by matching each
`variable "<name>"` in the root against an assignment line in each
environment's `terraform.tfvars` and `images.tfvars`. Everything quoted below
was read in the file at the line cited.

**No Terraform ran.** There is no `terraform` binary in this environment, so
no plan, no validate and no fmt output backs any statement here. Every claim
about what a plan would do is read off a `count`, a `for_each`, a
`precondition` or a validation block in the source — inference from the
configuration, not observed behaviour. Where that distinction matters it is
said again in the row.

## Which variables are set where

Computed as described above; `unset` means the environment takes the root
default.

| Variable | dev | test | stage | prod |
|---|---|---|---|---|
| `egress_allowed_upstreams` | unset | unset | unset | unset |
| `storage_target` | unset | unset | unset | unset |
| `cycle_interval_seconds` | unset | unset | unset | unset |
| `market_data_connector` | unset | unset | unset | unset |
| `notification_channels` | unset | unset | unset | unset |
| `enable_partner_interconnect` | unset | unset | unset | unset |
| `partner_interconnects` | unset | unset | unset | unset |
| `cloud_router_asn` | unset | unset | unset | unset |
| `enable_private_service_connect` | unset | unset | unset | unset |
| `private_service_connect_address` | unset | unset | unset | unset |
| `private_service_connect_target` | unset | unset | unset | unset |
| `disable_services_on_destroy` | unset | unset | unset | unset |
| `scc_muted_findings` | unset | unset | unset | unset |
| `snapshot_start_time` | unset | unset | unset | unset |
| `snapshot_retain_days` | unset | unset | unset | unset |
| `workload_metrics_exist` | unset | unset | unset | unset |
| `metrics_collector_image_digest` | unset | unset | unset | unset |
| `vendored_openobserve_image_digest` | unset | unset | unset | unset |
| `identity_mfa_state` | unset | unset | unset | unset |
| `enable_identity_platform` | **set** | unset | unset | unset |
| `identity_authorized_domains` | **set** | unset | unset | unset |
| `console_egress_cidr` | **set** | unset | unset | unset |
| `image_digests` | set (3) | set `{}` | set `{}` | set `{}` |
| `execution_nodes` | set `{}` | set `{}` | set `{}` | set `{}` |
| the seven `enable_*` data/AI/SCC flags | set `false` | set `false` | set `false` | set `false` |

Everything else (`project_id`, `region`, `environment`, `autonomy_ceiling`,
`trust_zones`, `permitted_paths`, `external_egress`, `public_ingress`,
`github_repository`, `project_number`) is set in all four.

---

# BLOCKING-DEPLOY

## 1. `notification_channels = []` in every environment, and no tfvars mentions it

**Required.** The root variable states the requirement itself:

> `variables.tf:371-375`
> ```
> variable "notification_channels" {
>   description = "Where alerts are sent. An alert with nowhere to go is not an alert."
>   type        = list(string)
>   default     = []
> }
> ```

**Exists.** The value is threaded to the observability module at
`main.tf:342` (`notification_channels = var.notification_channels`) and from
there onto all seven policies — `modules/observability/main.tf` lines 40, 76,
114, 148, 193, 233 and 273 each read `notification_channels = var.notification_channels`.
The module's own variable takes no default at all
(`modules/observability/variables.tf:13-15`), so the root's `[]` is the only
value any policy can get.

**Delta.** The variable is assigned in **none** of the four tfvars, and the
string `notification_channels` does not appear in any file under
`infrastructure/environments/`. So on the day `workload_metrics_exist` is
flipped, all seven alert policies are created with an empty channel list:
every one of them evaluates, none of them can page anybody. This is precisely
the failure the variable's own one-line description names, and it is the only
gate in this register with **no comment anywhere in the environment
configuration** — the other closed switches each carry a paragraph in the
tfvars saying why they are closed. Nothing in
`modules/observability/NOT-SCRAPED.md` mentions channels either; that file's
"What would change this file" list (lines 74-84) names a collector digest, a
plan, a node and the metrics flag, and stops.

**Deliberate?** **No.** This one is undocumented. Every other row in this
register is argued somewhere; this is the gap.

**Severity: BLOCKING-DEPLOY.** It blocks nothing today, because
`workload_metrics_exist` is false and the policies do not exist — which is
exactly why it is easy to miss. It becomes live the moment the observability
gate is flipped, and its failure mode is the one
`.claude/rules/domains/observability.md` already warns about in the other
direction: a policy stored and never acted on "reads in the console as a
project being watched".

## 2. `console_egress_cidr` unset in test, stage and prod — the API has no invoker there

**Required.** ADR 0018's route: the console reaches `qip-api` as a named
Cloud Run invoker, over a subnet its direct VPC egress attaches to. dev writes
it down:

> `environments/dev/terraform.tfvars:144`
> ```
> console_egress_cidr = "10.0.16.0/26"
> ```

**Exists.** Null is threaded as the off state in two places. `main.tf:187`
passes it to the network module, whose subnet is `count = var.console_egress_cidr == null ? 0 : 1`
(`modules/network/main.tf:131`). `main.tf:273` derives the secrets module's
`console_enabled = var.console_egress_cidr != null`, which gates the console
service account (`modules/secrets/main.tf:206-207`), its viewer-token read
(`:223-224`) and its profile-claims role (`:242-243`, `:258-259`). The output
is `one(google_service_account.console[*].email)` — null when disabled
(`modules/secrets/outputs.tf:25`).

**Delta.** In test, stage and prod the variable is unset, so it takes the
root default of `null` (`variables.tf:748-752`). That makes
`console_service_account_email` null, and the API's invoker list in the
catalogue is built by compacting exactly that one entry:

> `catalogue.tf:80-82`
> ```
> invokers = compact([
>   module.secrets.console_service_account_email == null ? "" : "serviceAccount:${module.secrets.console_service_account_email}",
> ])
> ```

`compact` drops the empty string, so the list is `[]`. Combined with
`ingress_posture = "internal"` (`catalogue.tf:344`, applied to every
catalogue workload), the API in test, stage and prod is a service reachable
over the VPC that **no identity is permitted to invoke**. The variable's own
description says as much — "Null means the console has no route to the
platform and says so on every page, which is the state this variable exists
to end" (`variables.tf:749-751`) — but it describes the console's view, not
the API's empty invoker list.

**Deliberate?** **Partly, and undocumented where it bites.** dev carries
twelve lines explaining the choice (`environments/dev/terraform.tfvars:132-144`).
test, stage and prod carry nothing: the variable is simply absent from all
three files, so a reader of `environments/prod/terraform.tfvars` has no way
to learn that the console route is a decision rather than an omission. The
same is true of `enable_identity_platform` and `identity_authorized_domains`
— dev sets them (`:125-130`), the other three never mention them.

**Severity: BLOCKING-DEPLOY** for those three environments, though behind
gate 3 below: they cannot be planned at all today.

## 3. `project_id = "unprovisioned"`, `project_number = 0` in test, stage and prod

**Required.** A project per environment.

**Exists.** The refusal, by name, at plan time:

> `variables.tf:24-27`
> ```
> validation {
>   condition     = var.project_id != "unprovisioned"
>   error_message = "This environment is not provisioned: its tfvars still carry the `unprovisioned` marker. ..."
> }
> ```

**Delta.** Three of four environments carry the marker verbatim —
`environments/test/terraform.tfvars:14-15`, `stage:14-15`, `prod:16-17`, each
`project_id = "unprovisioned"` and `project_number = 0`. Only dev names a
project (`dev:15`, `dev:23`). So `terraform plan` is refused for three of the
four environments, by design, before anything else in this register can
matter.

**Deliberate?** **Yes**, and documented three times over — an eight-line
paragraph in each of the three tfvars, plus the variable's own comment at
`variables.tf:16-23`.

**Severity: BLOCKING-DEPLOY**, and correctly so. Not a gap.

## 4. `image_digests = {}` in test, stage and prod

**Required.** Every catalogue workload is created at a digest the pipeline
attested.

**Exists.** The precondition that refuses a catalogue without one:

> `catalogue.tf:307-312`
> ```
> precondition {
>   condition = alltrue([
>     for workload in values(local.cloud_run_catalogue) : contains(keys(var.image_digests), workload.binary)
>   ])
>   error_message = "No digest is recorded for ${...}. A service is created at the digest deploy.yml last attested; run the pipeline for this environment, which writes infrastructure/environments/<env>/images.tfvars."
> }
> ```

**Delta.** `image_digests = {}` at `environments/test/terraform.tfvars:54`,
`stage:54` and `prod:56`. dev has three entries in a separate file
(`environments/dev/images.tfvars:15-19`) — `qip-api`, `qip-deepbrain`,
`qip-fastbrain` — but that file states in its own header that they are the
GKE runtime's last reconciled values and that "Nothing here has been deployed
to Cloud Run — see ADR 0024" (`images.tfvars:12-14`). So dev's digests are
real bytes that a *different* runtime admitted, not evidence of a Cloud Run
deployment.

**Deliberate?** **Yes**, documented identically in all three tfvars ("No
image has ever been built for this environment…", `test:51-53`, `stage:51-53`,
`prod:53-55`).

**Severity: BLOCKING-DEPLOY.** Not a gap. Note it is *downstream* of gate 3:
an unprovisioned project cannot run the pipeline that would write the file.

## 5. `metrics_collector_image_digest = null` in every environment

**Required.** Something must scrape `qip-fastbrain` and `qip-deepbrain`. The
`PodMonitoring` that did it on GKE left with the cluster (ADR 0024).

**Exists.** The whole mechanism, declared and closed. The root variable is
null-by-default with a digest-shape validation
(`variables.tf:667-696`, default at `:690`). The catalogue composes it against
the registry prefix and attaches it only to the workloads that ask:

> `catalogue.tf:383`
> ```
> collector_image_digest = each.value.metrics_collector && var.metrics_collector_image_digest != null ? "${module.registry.image_prefix}/vendor/cloud-run-gmp-sidecar@${var.metrics_collector_image_digest}" : null
> ```

`metrics_collector = true` on fastbrain (`catalogue.tf:177`) and deepbrain
(`:243`); deliberately `false` on the API (`:74`), because its `/metrics`
sits behind `Role::Monitor` and would answer a tokenless sidecar 401.

**Delta.** Null in all four environments — commented out, never set, in dev
(`environments/dev/terraform.tfvars:108`), absent from the other three. So no
Cloud Run workload carries a collector and every service's
`metrics_collected` output is false. The binaries emit; nothing collects.

**Deliberate?** **Yes**, and unusually thoroughly. `NOT-SCRAPED.md:53-59`
gives the reason (Binary Authorization admits only mirrored, attested images,
and nobody has reviewed the sidecar's digest), and `dev/terraform.tfvars:102-108`
repeats it in the environment that would flip it first.

**Severity: BLOCKING-DEPLOY** of the observability plane. Not a gap — but see
the staleness note below.

## 6. `workload_metrics_exist = false` in every environment

**Required.** Seven alert policies.

**Exists.** All seven are `count = var.workload_metrics_exist ? 1 : 0` —
`modules/observability/main.tf` lines 24, 60 (with an extra prod exclusion),
98, 132, 177, 217, 257. The flag reaches the module at `main.tf:338`.

**Delta.** Never assigned; commented out in dev
(`environments/dev/terraform.tfvars:100`, `# workload_metrics_exist = true`)
and absent from test, stage and prod, so all four take the root default of
`false` (`variables.tf:664`). Zero alert policies exist in any environment.

**Deliberate?** **Yes**, and the reason is a real failure rather than a
preference: Cloud Monitoring refuses a policy naming a descriptor it has
never ingested, "as two failed applies proved"
(`modules/observability/variables.tf:22-26`).

**Severity: BLOCKING-DEPLOY.** Not a gap. It is, however, gated behind gate 5
— the flag is flippable only once something has scraped, and nothing can
scrape without a collector digest.

## 7. `execution_nodes = {}` in every environment

**Required.** Blueprint §41.4: one dedicated C3 per region.

**Exists.** One module, instantiated per entry — `main.tf:479-481`,
`for_each = var.execution_nodes`, with `shadow_mode = true` hardcoded at
`main.tf:504` ("Not a tfvars value: letting a node out of shadow mode is an
edit here that a reviewer sees").

**Delta.** Empty in all four: `dev:67`, `test:49`, `stage:49`, `prod:51`. No
node exists anywhere, so the edge plane's entire pass-time series reaches no
deployed process, and the three edge alert policies would have nothing to
query even if gate 6 were flipped (`NOT-SCRAPED.md:20-24`).

**Deliberate?** **Yes**, argued in four places: the variable
(`variables.tf:227-254`), `main.tf:471-478`, each tfvars, and
`NOT-SCRAPED.md`. The argument is specific and holds — a node must be
configured for at least one venue, and no venue's published address ranges
are recorded anywhere in this repository, so the first entry is a venue
decision rather than a configuration one.

**Severity: BLOCKING-DEPLOY** of the edge plane. Not a gap.

## 8. `vendored_openobserve_image_digest = null` in every environment

**Required.** ADR 0028's metrics, logs and traces backend.

**Exists.** `module "openobserve"` at `catalogue.tf:425-541`, with
`count = var.vendored_openobserve_image_digest != null ? 1 : 0` (`:427`), and
a second precondition refusing the digest without a `management` trust zone
(`catalogue.tf:395-405`). The secrets for its root login are created in every
environment regardless (`main.tf:233-234`).

**Delta.** Null everywhere: commented out in dev
(`environments/dev/terraform.tfvars:118`), and named-as-absent in the other
three (`test:56-59`, `stage:56-59`, `prod:58-61`). No environment declares a
`management` trust zone either, so setting the digest alone would be refused.

**Deliberate?** **Yes**, in all four tfvars and in the variable
(`variables.tf:698-728`).

Worth distinguishing from gate 5, because the two look alike and are not:
OpenObserve's image **has** been reviewed and mirrored —
`infrastructure/egress/vendored-images.txt:73` carries
`docker.io/openobserve/openobserve@sha256:88fb692ac791d3eaff69653a4a4686f1c7eceb9e105491d58d29ac2739560b3b vendor/openobserve v0.92.2`.
So the only things still missing here are a tfvars line and a `management`
zone, both of them local decisions. The collector at gate 5 is missing the
mirror itself, which is a review nobody has done.

The catalogue additionally states an honest
half-finished edge: the root credential is mounted as a file per house rule,
but OpenObserve reads `ZO_ROOT_USER_EMAIL`/`ZO_ROOT_USER_PASSWORD` as plain
environment values and has no `_FILE` indirection, so bridging the two is
"entrypoint-level work this pass does not do" (`catalogue.tf:512-520`). That
is named as open, not assumed solved.

**Severity: BLOCKING-DEPLOY** of OpenObserve. Not a gap.

---

# COSMETIC

## 9. Two variable descriptions say "four" alert policies; there are seven

**Delta.** `modules/observability/main.tf:3` and `:7` say "Seven alerts" and
"All seven are gated on `workload_metrics_exist`", and `grep -c` over that
file returns 7 `google_monitoring_alert_policy` resources: `kill_switch`
(`:23`), `live_fill` (`:59`), `persistent_breach` (`:97`),
`permission_violation` (`:131`), `edge_halted` (`:176`),
`edge_reconciliation_break` (`:216`), `central_reconciliation_break` (`:257`).

Two descriptions still say four:

> `variables.tf:657-659`
> ```
> metrics. False until the first deployment runs; the four workload alert
> policies exist only when it is true, because Cloud Monitoring refuses a
> policy naming a metric it has never seen.
> ```

> `modules/observability/variables.tf:23-25`
> ```
> and PromQL both, as two failed applies proved — so the four workload
> alerts cannot exist before the workloads do.
> ```

The tree moved and these two sentences did not: the three edge and
central-plane reconciliation policies were added afterwards.
`NOT-SCRAPED.md:69-71` is correct where it counts ("the five central-plane
policies").

**Severity: COSMETIC.** No behaviour depends on the number.

## 10. `variables.tf:479-481` points at a prod tfvars paragraph that does not exist

**Delta.**

> `variables.tf:479-481`
> ```
> # See modules/connectivity/NOT-ORDERED.md for the four things a deployment
> # must arrange first, and environments/prod/terraform.tfvars for why three cells
> # need them.
> ```

`environments/prod/terraform.tfvars` contains no occurrence of
"interconnect", and its only use of "cell" is a pointer to
`environments/README.md` and `docs/operations/deploying-an-edge-cell.md` at
`prod:6`. The paragraph the cross-reference promises is not in that file.
`enable_partner_interconnect` and `enable_private_service_connect` are
themselves properly argued at `variables.tf:470-481` and in
`main.tf:571-578`; only the pointer is stale.

**Severity: COSMETIC.**

## 11. The live-fill alarm is excluded from prod, and prod is paper-only

**Delta.**

> `modules/observability/main.tf:59-63`
> ```
> resource "google_monitoring_alert_policy" "live_fill" {
>   count = var.workload_metrics_exist && var.environment != "prod" ? 1 : 0
>
>   project      = var.project_id
>   display_name = "qip ${var.environment}: a live fill occurred in a non-production environment"
> ```

The exclusion assumes prod is the environment where a live fill would be
legitimate. It is not: `environments/prod/terraform.tfvars:21` sets
`autonomy_ceiling = "paper_trading"`, and `variables.tf:105-116` refuses all
three live rungs in every environment, prod included. So the one environment
whose name implies live trading is also the only one with no alarm on the
event the policy's own documentation calls impossible ("If this fires, one of
those two controls has failed and the other did not catch it",
`main.tf:82-86`).

This is latent rather than live — prod is unprovisioned, and
`workload_metrics_exist` is false everywhere. Flagged as a decision worth
re-reading rather than a defect: the shape made sense when prod was expected
to become live, and the plan-time refusal at `variables.tf:105-116` has since
made that impossible from this repository.

**Severity: COSMETIC** today.

## 12. `identity_mfa_state` unset in dev, where Identity Platform is on

**Delta.** dev is the one environment that runs customer sign-in
(`environments/dev/terraform.tfvars:125`, `enable_identity_platform = true`)
and it does not set `identity_mfa_state`, so it takes the root default:

> `variables.tf:742-746`
> ```
> variable "identity_mfa_state" {
>   description = "Customer MFA posture: OFF, ENABLED (optional), or MANDATORY. MANDATORY locks out every unenrolled account when it applies."
>   type        = string
>   default     = "ENABLED"
> }
> ```

`ENABLED` means optional, not enforced. The default is the middle rung rather
than the restrictive one, which is a departure from this configuration's own
stated habit ("Every variable that could make the deployment less safe has a
restrictive default", `variables.tf:3-5`) — though `MANDATORY` as a default
would lock out every unenrolled account on the apply that introduced it,
which is the reason the middle rung is defensible.

**Severity: COSMETIC** — dev only, customer identity only.

---

# Decisions that look stale

Three of the argued absences rest on premises the tree has since moved past.
None is a defect; each is a decision worth re-reading before it is quoted
again.

1. **The execution-node absence (gate 7) is argued partly on the binary not
   being ready.** It no longer is: `.claude/rules/domains/observability.md`
   records that `qip-edge-node` runs `Cell::work` under
   `QIP_VENUE_FEED=simulated` since `6340610`, and that the pass-time series
   move in `qip-edge-node/tests/pass.rs`. The venue-ranges argument is the
   one that still holds; the readiness one does not, and the tfvars comments
   do not distinguish them.

2. **Gates 5 and 6 are described as sequential and are actually blocked at
   the first step.** `NOT-SCRAPED.md:74-84` lays out the order — review a
   digest, mirror it, apply, observe a descriptor, then flip. The first item
   has not moved, and that file says so itself:
   `infrastructure/egress/vendored-images.txt:29-30` — "from a line here as
   `vendor/cloud-run-gmp-sidecar`. No digest has been reviewed, so there is
   no line, no mirror and no sidecar". Everything downstream of it is a
   statement about a configuration. The contrast with OpenObserve on line 73
   of the same file, which *is* mirrored, is the useful one: two gates that
   read identically in the tfvars are at different stages.

3. **dev's `images.tfvars` digests describe a runtime that no longer
   exists.** They are "the digests the GKE runtime's last reconciled values
   file carried" (`environments/dev/images.tfvars:10-14`). They satisfy the
   `catalogue_is_placed` precondition and would create three Cloud Run
   services at bytes that a retired cluster admitted. That is defensible —
   same bytes, same registry, same digest — but it means dev's only
   distinguishing feature relative to test and stage is inherited from a
   runtime ADR 0024 retired.

---

# What is not in this register, and why

These are closed and correctly closed, each with an argument in place; listed
so a later reader can see they were checked rather than missed.

- The six managed-data flags and `enable_vertex_ai` — `false` in all four
  tfvars (`dev:71-77` and the same block in each of the other three), argued
  at `variables.tf:415-468` and in `modules/data/NOT-PROVISIONED.md`. The
  argument is exact: `StorageTarget::is_implemented` returns true for three
  targets, and provisioning a store no adapter can open is "a bill, an attack
  surface, and a diagram that reads as a capability" (`main.tf:420-423`).
- `enable_security_command_center = false` — argued at `variables.tf:560-590`
  and `modules/scc/ORGANISATION-SCOPED.md`: the detectors only evaluate if SCC
  is activated at the organisation, which this project-scoped configuration
  cannot check, and a stored-but-never-run detector "read in the console as a
  project being watched" is worse than a visible gap.
- `enable_partner_interconnect` / `enable_private_service_connect = false` —
  `variables.tf:470-518`, `modules/connectivity/NOT-ORDERED.md`. Terraform
  cannot order a cross-connect. (The cross-reference is stale; see gate 10.)
- `disable_services_on_destroy = false` — `variables.tf:534-556`. Disabling
  `compute.googleapis.com` deletes every Compute resource in the project,
  including ones this configuration never created, and the plan shows one API
  being disabled rather than the resources that go with it.
- `market_data_connector = null` — `variables.tf:341-369`, `catalogue.tf:184-193`.
  Absent, the fast brain runs the synthetic exchange; a half-configuration is
  structurally impossible because the type is an object of two required keys.
- `permitted_paths`, `external_egress`, `public_ingress` all `{}` — argued in
  `environments/dev/terraform.tfvars:55-60`, bare with no comment in the other
  three (`test:42-44`, `stage:42-44`, `prod:44-46`). Empty is the fail-closed
  reading. Worth noting: `egress_allowed_upstreams` defaults to five hosts
  (`variables.tf:295-301`), so the proxy is permitted to dial them while no
  zone is permitted to reach the proxy — two layers, and the outer one closed.
- `scc_muted_findings = {}`, `partner_interconnects = {}` — empty is the
  correct and intended state of both, not an unfinished one.
- `shadow_mode = true` at `main.tf:504` — hardcoded rather than a tfvars
  value, on purpose, so leaving shadow mode is a reviewed diff.
- `deployer_service_account = null` on the OpenObserve module
  (`catalogue.tf:540`) — deliberate: the pipeline does not move a vendored
  workload.
- `storage_target` unset in all four, defaulting to `memory`
  (`variables.tf:309-328`). Not a gap: a Cloud Run instance has no volume, and
  naming a managed target would stop the service at
  `StorageSettings::preflight`. It is, however, the fifth variable that no
  environment file mentions at all.

# Summary

| # | Gate | Documented as deliberate | Severity |
|---|---|---|---|
| 1 | `notification_channels = []` | **No — nowhere** | BLOCKING-DEPLOY |
| 2 | `console_egress_cidr` null in test/stage/prod | Partly — dev only | BLOCKING-DEPLOY |
| 3 | `project_id = "unprovisioned"` ×3 | Yes, three times | BLOCKING-DEPLOY |
| 4 | `image_digests = {}` ×3 | Yes | BLOCKING-DEPLOY |
| 5 | `metrics_collector_image_digest` null | Yes | BLOCKING-DEPLOY |
| 6 | `workload_metrics_exist = false` | Yes | BLOCKING-DEPLOY |
| 7 | `execution_nodes = {}` | Yes | BLOCKING-DEPLOY |
| 8 | `vendored_openobserve_image_digest` null | Yes | BLOCKING-DEPLOY |
| 9 | "four" alert policies, seven exist | n/a — drift | COSMETIC |
| 10 | stale prod-tfvars cross-reference | n/a — drift | COSMETIC |
| 11 | live-fill alarm excluded from a paper prod | Yes, on a stale premise | COSMETIC |
| 12 | `identity_mfa_state` optional in dev | Yes, in the variable | COSMETIC |

No gate in this register is BLOCKING-A-GATE: none of the twelve stops
`make check`, and the acceptance suite asserts the *shape* of several of them
rather than their value — `the_execution_nodes_are_one_module_rather_than_nine_copies`
requires each tfvars to declare the map (`infrastructure.rs:841-842`), not to
fill it, and the console-CIDR test checks the validation rather than the
assignment (`infrastructure.rs:873-890`). That is the right division; it is
also why an empty `notification_channels` has gone unremarked.
