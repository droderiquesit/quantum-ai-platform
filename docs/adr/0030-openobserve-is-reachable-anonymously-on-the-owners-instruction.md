# ADR 0030 — OpenObserve is reachable anonymously, on the owner's instruction

- **Status:** accepted
- **Date:** 2026-09-04
- **Amends:** ADR 0028 decision 5 (OpenObserve is internal-only, invokers empty)
- **Supersedes nothing.** ADR 0003, ADR 0021 and the paper-trading boundary
  are untouched and unreachable from anything below.

## The decision

`module.openobserve` in `infrastructure/terraform/catalogue.tf` is declared
`ingress_posture = "open-anonymous"` with `invokers = ["allUsers"]`. Its own
`run.app` URL answers the public internet, unauthenticated, to anyone who
knows or guesses it.

The owner asked for this four times across one session, in escalating and
unambiguous terms — "turn the link External", "make it open", "make it
anonymous", "complete this" — after being shown, twice, what it costs and
what the alternative was. This record exists because a decision of this shape
must be readable by whoever finds the service later, and because the three
guards it removes each carried an argument that deserves an answer rather
than a deletion.

## What it costs

**Today: nothing, and that is the whole reason this is cheap now.** No apply
has created the service. The API's OTLP drain is inert in every environment —
`manifest_wiring` records that no deployment sets `QIP_OPENOBSERVE_URL`.
`workload_metrics_exist = false` everywhere, and nothing scrapes any process.
An anonymous OpenObserve at the moment of this record serves an empty
database to anyone who reaches it.

**The moment the drain is wired, the cost is the platform's whole operational
surface**, served to the internet with no credential: positions and exposure
by instrument, order flow, fills, reconciliation breaks by direction, halt
states and their causes, strategy admission and demotion, and the cost
router's rationale for every rung it chose. For a trading platform that is
material non-public information about live behaviour, and the fact that this
platform is paper-only does not make its strategy behaviour public
information.

Two further costs, stated because neither is obvious from the diff:

- **The image is pinned at v0.92.2 for as long as nobody bumps it**, by the
  `ignore_changes` rule ADR 0028 records. An internet-facing service that
  Terraform will not update on apply is one whose next CVE is nobody's alarm.
  A digest bump needs the explicit `-replace` that ADR 0028's correction
  names.
- **OpenObserve's own root credential becomes the only lock on write access.**
  It reaches the container as an environment value, which
  `.claude/rules/01-security-and-safety.md` forbids, and that gap is open at
  the time of this record. Anonymous read plus an environment-carried write
  credential is a worse pair than either alone.

## What was done instead of deleting the guards

The three controls that made this impossible are replaced by one that makes
it *declared*. The guarantee is no longer "no workload can be anonymous"; it
is "no workload can be anonymous without saying so in the one place a reader
looks".

1. **`ingress_posture` gains `open-anonymous`**, a third value, rather than
   `public-edge` being quietly widened or the mapping learning a fourth
   branch. The name is deliberately ugly: `grep -rn 'open-anonymous'
   infrastructure/` answers "what is on the internet" in one line, and a value
   called `public` or `external` would not have.
2. **The `allUsers` refusal becomes conditional, not absent.** An anonymous
   invoker is refused on every posture except `open-anonymous`, so it cannot
   be set by accident on the API, either brain, or a future workload — the
   failure the original refusal existed to prevent. Setting one without the
   other is refused from both sides.
3. **The acceptance suite asserts the new invariant** rather than the old
   absence: any workload naming an anonymous invoker also declares
   `open-anonymous`, and the set of workloads doing so is exactly the set this
   record names. A second workload joining it fails the suite until this ADR
   is amended to say why.

`traffic_class = "trading"` remains pinned to `internal` and
`open-anonymous` cannot be combined with it. The API, the fast brain, the
deep brain and every execution node are structurally unable to take this
posture.

## What would make this wrong

**Before the OTLP drain is configured in any environment.** That is the point
at which the service stops being empty, and it is the trigger this record
exists to place. Concretely: the first change that sets `QIP_OPENOBSERVE_URL`
on a deployment must either move this service behind IAP or record why
anonymous remains acceptable with real telemetry in it.

The alternative offered and declined was an external HTTPS load balancer with
Identity-Aware Proxy: the same public URL, reachable from any browser,
differing only by a Google sign-in. It remains the recommended end state and
needs no amendment to this record to adopt — it is a narrowing.

## Consequences

- `docs/ops/` gains no new runbook; this record is the runbook.
- The blueprint's "no public-facing ingress anywhere" property, true since the
  platform began and asserted in ADR 0028's own text, is no longer true. Any
  document repeating it is now wrong and is corrected in this change.
- Binary Authorization, the trust zones' default-deny, the egress allowlist
  and the three paper-trading layers are untouched. This decision widens who
  may *read* one workload; it grants nothing the ability to place an order.
