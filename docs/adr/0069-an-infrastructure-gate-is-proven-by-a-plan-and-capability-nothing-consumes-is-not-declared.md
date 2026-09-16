# ADR 0069: An infrastructure gate is proven by a plan, and capability nothing consumes is not declared

- **Status**: Proposed
- **Date**: 2026-09-14
- **Supersedes**: nothing
- **Related**: ADR 0003 (paper trading by default), ADR 0022 (the blueprint runtime), ADR 0024 (Cloud Run and one execution node per region), ADR 0035 (one shadow node in dev), ADR 0036 (delivery through Config Connector and Argo CD), ADR 0040 (the dev teardown)

## Context

Two questions came up together while closing blueprint §46.1, §40.5, §40.14
and §45.1, and the answer to each turns out to be the answer to the other.

### One: nothing had ever planned this configuration

`modules/trust-zones` holds the thirteen-zone model. Its refusals — only
`optimisation` may reach IBM; the wallet read path may not reach the treasury
write path; a load balancer may not be put in front of a trading zone — are
`lifecycle.precondition` blocks, and `NOT-ENFORCED-HERE.md` said plainly that
`terraform fmt -check`, `validate` and `plan` had never been run against any
of them, because no Terraform binary existed where the module was written. An
unrun precondition is an assertion about an assertion.

What stood in for a plan was the Rust acceptance suite, which reads the HCL as
text. That is the right tool for "no Cloud Run service may be
`INGRESS_TRAFFIC_ALL`", and it is structurally unable to answer "does this
rule run". Running the plan proved it, twice, in the first minutes:

- **`modules/network`'s prefix check on `console_egress_cidr` could not run at
  all.** The condition was
  `x == null || tonumber(split("/", x)[1]) <= 26`. Terraform evaluates both
  operands of `||`; `split` refuses a null argument; the plan dies on a
  provider error no `error_message` can reach. The variable is null by default
  and null in `test`, `stage` and `prod`, so `terraform plan` was impossible in
  three of the four environments. It had a Rust test named
  `the_console_egress_cidr_validation_refuses_a_range_smaller_than_a_26_rather_than_only_its_syntax`
  which asserted the rule "both fires and admits" — by re-implementing the
  arithmetic in Rust and checking the mirror. A mirror of an expression is not
  the expression, and the mirror is never handed a null.
- **`modules/trust-zones`' Cloud NAT precondition could not fire in the case it
  was written for.** It refuses a zone declaring external egress from a region
  other than the module's. Both NAT resources counted on the egress zones
  *narrowed to this region*, so a single out-of-region zone emptied the list,
  `count` went to zero, the resource was never planned, and a precondition on
  an unplanned resource is never evaluated. The zone would have received its
  egress firewall rules, no address translation at all, and the message saying
  exactly that would never have been printed. It fired only when a second,
  in-region zone happened to be declared beside it.

Both are the failure this repository already has a name for: a control that
reads as protection and cannot fire. `MaxExpectedShortfall` is the standing
example in the risk domain; these are the same defect in the infrastructure
one, and neither was findable by reading.

The obstacle was never willingness. It was that `terraform plan` needs a
backend and a credential, `infra.yml` is dispatched by a person, and since
2026-09-13 there is no project to plan against: ADR 0040 decision 13's
teardown deleted four Cloud Run services, the control-plane cluster and 166
Terraform-managed resources. A gate that can only be exercised against live
infrastructure is a gate nobody exercises, and this one went unexercised for
the whole life of the module.

### Two: which absent things should be written down

Blueprint §45.1 lists Cloud Armor with the global load balancer and CDN
(phase 1), Cloud Workflows (phase 2), Cloud HSM (phase 5) and spot GPU
(phase 2). All four were absent. The obvious move is to declare all four,
gated off, and score the rows closed.

That is right for one of them and wrong for three, and the difference is not
phase order. It is whether the thing being declared carries a **refusal** that
can be argued about now.

## Decision

### 1. A gate is proven by a plan, and the plan runs without a credential

Every variable validation and every `lifecycle.precondition` that holds a
safety property is exercised by a `terraform test` harness, in a
`*.tftest.hcl` file, with `mock_provider` replacing the google providers and
`command = plan` on every run. Such a harness needs no credential, reaches no
project, creates nothing, and does not touch the GCS backend — `terraform
test` keeps its own state. It is therefore runnable in CI, in a worktree, and
against an environment that has been torn down.

Four harnesses exist at this ADR: the root's autonomy ceiling, the zone
model, the console egress range, and the public edge. `ci.yml`'s
infrastructure job discovers and runs all of them, and adding a fifth needs no
edit to the workflow.

**Every harness proves a refusal and an admission.** The infrastructure rules
already require this of a validation change; making it a property of the
harness rather than of a particular review is what keeps it. A gate proven
only to refuse may refuse everything — which is precisely what
`console_egress_cidr` was doing. `qip-acceptance`'s `terraform_plan` suite
fails a harness that has only `expect_failures` runs, only asserting runs, no
`mock_provider`, or any `command = apply`.

This does not make the Rust structural tests redundant and does not replace
an apply. A mocked plan leaves every computed attribute unknown, so a
property whose value is not known until apply — that a backend service is
attached to the Cloud Armor policy, for instance — cannot be asserted in a
harness and is asserted on the configuration instead. And a plan that
succeeds says the configuration is coherent and its refusals fire. It says
nothing about whether Google would accept a single resource in it. Nothing in
this repository has been applied since the teardown.

### 2. Capability with no consumer is not declared; a refusal with a consumer is

The **public edge is declared** — `modules/public-edge`: Cloud Armor with a
rate limit and a geographic policy, the global HTTPS load balancer, Cloud CDN
for the static shell — with `hostnames` empty in all four environments, so it
creates nothing anywhere. It is declared because its content is a set of
refusals that can be argued about today and proven today:

- a backend may front only `public-edge` or `application-identity`, so §40.5's
  "customer traffic and trading traffic never share a load balancer" is a plan
  that stops rather than a review comment;
- there is no listener on port 80, because a redirect from an unencrypted
  endpoint is advice a client may ignore and the first request has already
  travelled in the clear;
- a wildcard hostname, a duplicate hostname, a rate limit of zero, a rate
  limit too large to bind, and a lower-case country code that would match
  nothing are each refused by name.

Those arguments are cheaper to have now, with nothing deployed, than on the
day a customer surface first ships. That is the same reason
`modules/execution-node` is fully written while `execution_nodes = {}`
everywhere.

**Cloud Workflows, Cloud HSM and a spot GPU pool are not declared.** Each
would be capability with no consumer and no refusal:

| Row | Consumer | What declaring it would produce |
|---|---|---|
| Cloud Workflows | No lifecycle or ingestion orchestration exists; Pub/Sub carries what there is | A workflow definition nothing triggers, whose steps nobody can review because there is no sequence to review |
| Cloud HSM | ~~No custody key material, no signing key and no asymmetric key of any kind exists. ADR 0043 records asymmetric signing as a gap no in-tree code may close~~ — **this premise was false when written; see the amendment below** | An HSM-backed key encrypting nothing, at roughly ten times the cost of a software key, rotating forever |
| Spot GPU | No training job, no causal estimation run and no simulation job exists. `enable_vertex_ai = false` everywhere | An instance template for a machine that boots and idles — and a GPU is the most expensive thing on this platform to leave running by accident |

None of the three has a gate to prove. A plan against any of them would show
a resource created, and that is not evidence of anything. Writing them would
move three rows from `PARTIAL` to `PARTIAL` while adding three more things a
reader must check are switched off.

The condition for reversing each is the same and is stated so the next agent
does not re-litigate it: **declare it when the thing that consumes it exists,
or when it carries a refusal somebody can argue about before it runs.**

### 3. `workload_metrics_exist` stays false, and no SLO object is declared

Blueprint §49.1's targets include node availability, cycle completion for
paths 1 and 2, and arrival dispersion after equalisation. None of the three is
declared as a Cloud Monitoring SLO, and none may be, for the reason the
observability rules give: naming a metric in a policy that nothing emits
produces a policy that reads in the console as a project being watched and
evaluates nothing.

Checked rather than assumed:

- **Availability** has no descriptor in `qip-observability`, and Cloud
  Monitoring's own `run.googleapis.com/*` series require a Cloud Run service
  that has served. Four were deleted on 2026-09-13.
- **Cycle completion** would need a completion count and a path label.
  `qip_cycles_total` is described, in the code that registers it, as "cycles
  of the eight-stage loop **begun**", and it carries no labels at all. There is
  no per-path counter; ADR 0068's router assigns one of eight paths and records
  no series for it.
- **Arrival dispersion after equalisation** has no descriptor and no recording
  site anywhere in the workspace.

An SLO on any of the three would name a descriptor nothing emits. The gap is
closed by a recording site in the binaries and evidence that something scraped
it — not by a Terraform object, and not by this ADR.

## Consequences

- CI gains a step that plans. It needs no secret, so it runs on a fork's pull
  request like every other step in that job.
- A new validation or precondition that holds a safety property now has an
  obvious home for its proof, and an obvious way to be caught without one.
- Two defects are fixed that no amount of reading would have found, and the
  class of the first is now refused by an acceptance test: a validation that
  hands the value it is null-guarding to a *function*, which the existing test
  for this class did not cover because it looked only for attribute access.
- Three §45.1 rows stay open on purpose, with the condition for closing each
  written down. `docs/DELIVERY-STATUS.md` should say `PARTIAL` for them and say
  why, rather than `ABSENT` as though nobody had considered them.
- Nothing here changes what is deployed. Nothing is deployed.

## What it costs

**A harness is a second thing to keep in step with the module.** A precondition
renamed, or a resource that gains a `count`, changes the address
`expect_failures` names and the harness fails — noisily, which is the right
direction, but it is maintenance that did not exist before. The same is true of
the fixed module outputs the root harness overrides: add an output to
`modules/registry` that the root reads and the harness has to learn it.

**Mocked plans are not real plans, and the gap is easy to forget.** Every
computed attribute is unknown, so a harness cannot assert that a backend
service ended up attached to the Cloud Armor policy, that a bucket name is what
a consumer expects, or anything else the provider fills in. Those assertions
move to the Rust suite, where they are text checks again — and a reader who
sees "the gates are planned" may take it for more than it is. The honest
summary is: a mocked plan proves the configuration is coherent and that its
refusals fire on the values given. It proves nothing about Google.

**Three modules are overridden in the root harness** — `ai`, `evidence`,
`registry` — because they drive `for_each` from values a mock cannot know.
Nothing in the autonomy ceiling's path goes through any of them, but the
override is a hole in the coverage of that one harness and will stay one.

**CI gets slower.** Four `terraform init` runs and four `terraform test` runs,
each downloading or caching a provider. It is under a minute today and grows
with each harness added.

**Three §45.1 rows stay open, visibly.** Declaring Cloud Workflows, Cloud HSM
and a spot GPU pool switched off would have read as four rows closed instead of
one. Choosing not to means the register keeps showing work nobody has done,
which is the point, and also means the next agent may write them anyway unless
they read the reversal condition above.

**The public edge is code nothing runs.** It will be reviewed as though it
works and has never been applied; its Cloud Armor rules in particular are the
kind of thing that is subtly wrong until traffic hits them — a
`rate_based_ban` keyed on `IP` behaves differently behind a shared NAT, and no
plan says so.

## What would make this wrong

**A harness that starts being edited to pass.** The failure mode of any gate
is that the thing it guards changes and somebody adjusts the gate instead. If a
review ever finds an `expect_failures` address changed to match a refusal that
moved, rather than the refusal restored, the harnesses have become
documentation with a green tick and are worth less than nothing. The signal to
watch is a commit that touches a `*.tftest.hcl` and no `.tf`.

**Mocked plans being read as applies.** If a delivery record, a status row or
a handoff ever says an environment is "proven" or "validated" on the strength
of `terraform test`, this decision has been misused. It proves refusals fire.
It has never spoken to Google.

**A real plan becoming available again.** If a project is stood up and
`infra.yml`'s `plan` can run against it, a genuine credentialed plan is
strictly better evidence for the admitting half of every gate, and the
harnesses become the fast check rather than the only one. They should not be
deleted then — they are the only thing that runs on a pull request from a fork
— but the hierarchy of evidence changes and this record should say so.

**A consumer appearing for one of the three refused rows.** A training job, a
lifecycle orchestration, or any asymmetric key material makes the spot GPU
pool, Cloud Workflows or Cloud HSM a thing with a consumer, and decision 2
then argues for declaring it rather than against. That is the reversal
condition and it is deliberately easy to check: it is a grep for a caller, not
a judgement about phase.

**Something scraping a deployed process.** The moment there is evidence of
ingestion, `workload_metrics_exist` and the §49.1 SLOs stop being refused for
the reason given here and become ordinary work — with the same rule as the
alert policies, that the descriptor must have a production recording site and
not merely a registered constant.

**The public edge being switched on anywhere.** The Cloud Armor rules in
particular have never met traffic. If a `hostnames` entry is ever added, the
rate limit's key, the ban duration and the geographic expression need a real
review against real clients, and this record's confidence in them should not be
inherited.

## Alternatives considered

**Run `terraform plan` in CI against a real project.** It needs a credential
with read access to everything, a state bucket, and a project — and would make
CI's result depend on the state of somebody's environment. It also could not
have run at all since the teardown.

**Keep asserting the gates from Rust.** That is what was being done, and it is
how a validation that could not run survived alongside a test asserting it both
fires and admits. The Rust tests stay for what they are good at; they do not
stay as the proof that a rule runs.

**Use `command = apply` in the harnesses with mocked providers.** It would
resolve the unknown attributes a mocked plan leaves and allow a few more
assertions. Rejected: the rule in this repository is that an agent shows a plan
and a person applies, and a harness is not the place to start blurring which
verb is being run, however simulated the apply.

**Declare Cloud Workflows, Cloud HSM and spot GPU switched off, for
completeness.** Rejected for the reason in decision 2. The public edge earns
its place by the refusals it carries; those three would carry none.

---

## Amendment, 2026-09-16 — decision 2's Cloud HSM row rested on a false premise

The row above said Cloud HSM had no consumer because "no custody key
material, no signing key and no asymmetric key of any kind exists", and cited
ADR 0043 as recording that gap. Both halves were wrong on the day this ADR
was written, and the citation was wrong in the direction that made the
decision look safer than it was.

An asymmetric signing key already existed:

```
$ grep -rn 'purpose *= *"ASYMMETRIC_SIGN"' infrastructure/terraform --include=*.tf
infrastructure/terraform/modules/binaryauthorization/main.tf:57:  purpose  = "ASYMMETRIC_SIGN"
```

It landed in `6e3aad0` on 2026-09-02; this ADR is `4b07f44`, 2026-09-14.
`git merge-base --is-ancestor 6e3aad0 4b07f44` exits 0, so the key predates
the decision that said it did not exist. And ADR 0043, cited here as
recording the gap, says the opposite in terms: the platform "already meets an
asymmetric-signature obligation, today, in production". This is the second
time in this repository that a rule file has asserted a prohibition by citing
an ADR that authorises the thing — `.claude/rules/architecture/00-boundaries.md`
records the first, where "No in-tree cryptography" mis-cited ADR 0009 against
ADR 0002's Decision section. The shape is worth naming because it is not
carelessness about facts: it is a citation quoted forward because it was
quoted forward, which is exactly the failure mode the rest of this repository
attacks by citing commands instead of numbers. **A citation deserves the same
treatment as a line number: run it, do not inherit it.**

A second, smaller error made the gap unfalsifiable. `§45.1`'s evidence
command grepped for `google_cloud_hsm`, which is not a resource type in any
provider. The question could not return yes however the tree was written, so
the row would have read the same forever.

**What changes, and what does not.** The reversal condition this ADR set —
"declare it when the thing that consumes it exists" — is met for HSM and is
still unmet for Cloud Workflows and the spot GPU pool, both re-checked on
2026-09-16 and both still without a consumer. So decision 2 stands for those
two and is superseded only for HSM.

HSM is therefore expressed as `var.kms_protection_level`, defaulting to
`SOFTWARE` — the level all four keys already carried — and threaded to each
of them. This declares **no new resource** and changes **no plan** until
somebody sets it, so the cost objection in the table above is answered rather
than overruled: nothing bills differently at the default. One value governs
the whole configuration, because a mixed posture is a claim about protection
that the weakest key falsifies.

Proven both ways, as decision 1 of this ADR requires, in
`infrastructure/terraform/tests/kms-protection.tftest.hcl`: `SOFTWARE` and
`HSM` plan to the end **and are asserted to reach the key**; `hsm`,
`EXTERNAL`, `EXTERNAL_VPC` and the empty string stop the plan. Six runs,
`6 passed, 0 failed`. The admitting half is the half that matters — a gate
that refuses everything is indistinguishable from a working one when only its
refusals are tested.

One limit, stated rather than gated: raising the level on an environment that
has already been applied does not upgrade a key. `version_template` is
immutable, so Terraform plans a replacement and `prevent_destroy` stops the
apply. No validation can catch this, because a variable validation sees the
value and never the prior state — a check for it could never fire, and this
repository's standing rule is that a control which cannot fire reads as
protection and is not one.
