# ADR 0033 — OpenObserve becomes authenticated before it holds telemetry

- **Status:** accepted
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
  That is not waste: the posture is now a tested, refusable capability with
  exactly zero users, and the acceptance suite pins the anonymous set to
  empty rather than to `["openobserve"]`.
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
