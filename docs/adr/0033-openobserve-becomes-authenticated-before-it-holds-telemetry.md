# ADR 0033 — OpenObserve becomes authenticated before it holds telemetry

- **Status:** accepted; **applied for the invoker half on 2026-09-26, not
  applied for the front door.** No committed manifest grants OpenObserve to
  an anonymous principal: `infrastructure/gitops/envs/dev/openobserve.yaml`
  carries `ingress: INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER`, `invokers.yaml`
  names no invoker on it, and `catalogue.tf` instantiates
  `modules/openobserve` as `module "openobserve_access"` at its authenticated
  default, so every plan evaluates the prod refusal. No Identity-Aware Proxy
  exists in front of the service, so it is reachable by **nobody** until one
  is built; which door that is — this record's load balancer or ADR 0095's
  IAP on the Cloud Run service itself — is an open decision, to be recorded
  as an amendment here. See "Applied" below. The condition this record fires
  on stays enforced:
  `terraform_contract.rs::no_deployment_points_telemetry_at_openobserve_while_it_is_anonymous`
  now asserts the posture is not anonymous, so restoring it fails the suite
  whether or not anything sets `QIP_OPENOBSERVE_URL`.
- **Date:** 2026-09-04
- **Amends:** ADR 0030, on the condition ADR 0030 set for itself
- **Relates to:** ADR 0028 (OpenObserve adopted), ADR 0032 (the collector)

## The decision

OpenObserve moves from **anonymous** to **authenticated** external access
before the first byte of platform telemetry reaches it. It stays reachable
from the internet — the owner's requirement was to reach it from a browser
anywhere, and that is preserved. What ends is `allUsers` on
`roles/run.invoker`.

The mechanism is Identity-Aware Proxy in front of the service, with access
granted to named principals. `ingress_posture` returns from
`open-anonymous` to `public-edge`, which is what that arm was built for.

This is not a reversal of ADR 0030 against its author's wishes. It is the
condition ADR 0030 wrote for itself, firing:

> the service is empty today and stops being empty the moment any deployment
> sets `QIP_OPENOBSERVE_URL`. That change is the one that must move this
> behind IAP or re-argue the exposure.

## Why the empty service and the full one are different questions

ADR 0030 accepted anonymous exposure of a service holding **nothing**. The
cost it weighed was an idle instance and a login prompt facing the internet.
That was a defensible trade for a deployment nobody could learn anything
from.

What ADR 0032's collector puts in it is a different object:

- **Cycle counts, per-stage durations, refusal counts by gate, limit
  breaches, permission denials, orders submitted and filled, reconciliation
  breaks by direction.** Read together over time, that is a description of
  how this desk trades — its cadence, when its controls fire, how large its
  activity is and when. It is not market data and it is not a credential, and
  it is still the most sensitive thing this platform emits.
- **A write surface.** An anonymous invoker is anonymous in both directions.
  Anyone who can reach the ingestion path can put rows in it. A telemetry
  store that anyone can write to cannot be used as evidence for a gate, which
  is the entire reason ADR 0032 exists. This alone settles it: the platform
  is about to start making claims *from* this data, and unauthenticated write
  makes those claims unfalsifiable.

The second point is the one that would have bitten. The first is about
disclosure and could be argued; the second is about whether the data means
anything at all, and cannot.

Note that OpenObserve's own login was always enforced — the API answers 401
today. This decision is not "add authentication where there was none". It is
that a single application-level credential in front of a publicly-reachable
write path is one layer where the data now warrants two, and that the outer
layer should be the platform's own identity system rather than a shared
password.

## What it costs

- **The owner can no longer send a link to someone who does not have
  access.** Anonymous reachability was convenient and that convenience ends.
  Access becomes a grant, which is a small administrative act each time.
- **IAP is another moving part** in front of a service that was, briefly,
  gratifyingly simple.
- **Work already done is partly undone.** ADR 0030's plumbing — the
  `open-anonymous` posture, the widened invoker shape check, the paired
  preconditions — stays in the module and stops being used by any workload.
  That is not waste: the posture becomes a tested, refusable capability with
  exactly zero users, and applying this record moves the acceptance suite's
  pin on the anonymous set from `["openobserve"]` to empty.
- **It does not solve exfiltration by an authorised viewer**, and nothing
  here pretends to. Named, not solved.

## What would make this wrong

**If telemetry never actually flows.** This decision is priced entirely on
ADR 0032 landing. If the collector is never deployed and OpenObserve stays
empty, ADR 0030's original reasoning is still sound and this is premature
hardening of an empty box.

**If IAP cannot be made to work for this service** — it needs an external
load balancer and a backend the Cloud Run service sits behind, which is more
infrastructure than the direct URL. If that proves disproportionate, the
honest fallback is internal-only ingress plus operator access through a
bastion or a tunnel, *not* a return to anonymous. Reachability is the
requirement that may be traded; anonymity is not.

**If someone concludes the application password is enough.** It is a single
shared secret in front of a write path on the public internet, and it is
exactly what this record judges insufficient once the store holds evidence.
An argument that OpenObserve's own login suffices is an argument this
document has already considered and rejected.

## Applied

### 2026-09-26 — the invoker half

**What changed.** The anonymous `roles/run.invoker` binding on
`qip-dev-openobserve` is gone from `infrastructure/gitops/envs/dev/invokers.yaml`,
and the RunService's ingress moved from `INGRESS_TRAFFIC_ALL` to
`INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER`, the value `modules/openobserve`
derives for the authenticated posture. The module now has a caller:
`module "openobserve_access"` in `catalogue.tf`, ungated and handed
`var.environment`, with `access_posture` and `access_principals` left at their
defaults — `authenticated`, and nobody. So the prod refusal described in the
2026-09-05 record below is evaluated by every plan of the root, in every
environment. A mocked plan of the root at `environment = "prod"` admits the
default and, with `access_posture = "anonymous"` written into that call,
stops at `catalogue.tf` with this record's refusal message.

The mechanism differs from the one "The decision" names. `ingress_posture`
and its `public-edge` arm left `modules/cloudrun` with the service resource
under ADR 0036; the posture is now `modules/openobserve`'s, and the manifests
carry what it derives.

**What the acceptance suite holds.** Two suites pinned the anonymous posture
as a property and were inverted in place:
`gitops.rs::openobserve_is_deployed_at_the_reviewed_digest_on_ephemeral_storage_and_answers_no_anonymous_caller`
(formerly `..._anonymous_as_adr_0030_records_...`) moves its pin on
manifests naming an anonymous principal from one to zero, over every
environment and every parsed manifest, annotations included; and
`console_route.rs`'s OpenObserve arm asserts the load-balancer-only ingress
where it demanded `INGRESS_TRAFFIC_ALL`. `infrastructure.rs`'s `.tf` scan lost
a carve-out that admitted an anonymous HCL invoker line no file had contained
since ADR 0036, and gained
`the_openobserve_posture_module_is_instantiated_and_its_invokers_are_the_manifests`,
which holds the catalogue call to the default and `invokers.yaml` to the
module's member set.

**What it costs, beyond what this record priced.** Reachability, for now.
"The decision" says OpenObserve stays reachable from the internet behind IAP;
no IAP exists, so after this change it is reachable by nobody. That is the
trade "What would make this wrong" already permits — reachability may be
traded, anonymity may not — and it fails in the direction a safety default
has to. Nothing is running to lose access to: the dev environment's Cloud Run
services were torn down on 2026-09-13, the 2026-09-20 re-apply created none,
and the control-plane cluster that would reconcile these manifests is
suspended under ADR 0093 (`infrastructure/CLAUDE.md`). The manifests are what
the reconciler applies when it runs again.

**Not applied: the front door.** Granting an operator is now one line in
the catalogue call and its mirror in `invokers.yaml`, but a grant reaches
nobody without a door. Building it needs `infra.yml`'s IAP enablement step and
a choice between this record's external load balancer — which the ingress
above already admits — and ADR 0095's IAP on the Cloud Run service itself,
which would need `INGRESS_TRAFFIC_ALL` back beside an IAP-only invoker, as the
portal has. That choice is an amendment to this record, not a manifest edit.

**One stale sentence outside this change's reach.** `modules/openobserve/main.tf`'s
header still says the module has no caller. It is outside the files this
change could edit and is left for the next change to that module.

### 2026-09-05 — the module, with no caller

*Kept as written on 2026-09-05, when it was true; the section above says
what changed. The status line read "not yet applied" until 2026-09-26.*

**Not applied. What exists on 2026-09-05 is code with no caller and no
plan behind it**, and the status line above stays as it is. This section
records what is in the tree and what is not, so that a reader who finds the
module does not read it as the decision having landed.

In the tree: `infrastructure/terraform/modules/openobserve/`, which carries
the posture as one variable. `access_posture` defaults to `authenticated`
and is `nullable = false`, so a caller who passes nothing — or a null —
gets the posture this record requires; `anonymous` stays selectable, by
name, and is refused for `prod` by a `validation` block that reads both the
posture and the environment. `access_principals` refuses a member without an
IAM prefix and refuses the two anonymous members outright, so the invoker
list this module derives cannot be widened into ADR 0030's binding. The
module emits the ingress and the invoker members the manifests must carry
and creates nothing itself. Three tests in
`backend/crates/tests/qip-acceptance/tests/infrastructure.rs` hold the
default, evaluate the prod refusal at four environment/posture pairs, and
refuse an anonymous member anywhere in the module outside a `validation`
block. The gate was exercised by a real `terraform plan` against a harness
root in a scratch directory that sources these files and declares no
provider: it refuses `prod` + `anonymous` with the message above and admits
`prod` at the default, `dev` + `anonymous`, and a named group.

Not in the tree, and each of these is the difference between this record and
its application:

  * **No Identity-Aware Proxy.** No load balancer, no serverless NEG, no
    backend service, no IAP setting. Choosing `authenticated` today declares
    the posture and narrows the invoker set; it configures no IAP, and this
    record's own "what it costs" — IAP is another moving part — is unpaid.
  * **No caller.** Nothing in `infrastructure/terraform` instantiates the
    module, so no plan evaluates the refusal and no environment's posture is
    decided by it yet. The root wiring is a separate change.
  * **The service is unchanged.** `infrastructure/gitops/envs/dev/` still
    carries `ingress: INGRESS_TRAFFIC_ALL` and the anonymous `IAMPolicyMember`
    under ADR 0030, and the running service is still reachable without a
    credential.

**The apply waits on the dev cluster path.** Since ADR 0036 decision 4 the
RunService and its invoker are manifests, and a manifest reaches Cloud Run
only through Config Connector on the control-plane cluster. That cluster does
not exist in `dev`: run 38's `plan` found the previous one tainted and
proposed replacing it, and under ADR 0040 decision 1 no `up` was dispatched
on a plan that destroys a cluster
(`docs/DELIVERY-STATUS.md` (which absorbed the missing-infrastructure register on 2026-09-07), "Observed, not predicted —
run 38"). So the posture cannot be moved by an apply from here — it moves
when the cluster is rebuilt by a person and the manifests are edited beside
the root wiring. Nothing in this change brings that forward, and nothing in
it should be read as having.
