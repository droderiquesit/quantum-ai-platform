# ADR 0054: A source is probed only where a reviewed egress route already exists, and there is no crawler

- **Status**: Proposed
- **Date**: 2026-09-09
- **Supersedes**: nothing
- **Related**: ADR 0009 (tiered dependency policy), ADR 0024 (the egress path), ADR 0034 (licensing before use), ADR 0050 (what an option-quote source must satisfy)

## Context

`DataFinder::assess` evaluates a candidate data source — its licensing posture,
its tier, its robots policy, its freshness, its schema — and either registers it
into the catalogue or refuses it by name. It is built, tested, and reached from
no application: `grep -rn 'assess_sources' backend/crates/apps --include=*.rs`
returns nothing. Three rows in `docs/DELIVERY-STATUS.md` are scored `UNREACHED`
on that one fact — §7.5 (the dark-web hard line), §7.6 (deep-web ingestion) and
§7.6.2 (access modes) — and each has been read, reasonably, as a coding gap.

**It is not a coding gap.** Establishing that is the point of this record.

The obvious blocker is that `NetworkProbe` is a stub whose every method returns
`Error::Unavailable` naming a missing HTTP transport. That looks like the thing
to fix, and it is not, because `qip_transport::http` exists and already carries
every outbound call this platform makes: a plaintext HTTP/1.1 client with
bounded status lines, headers, bodies and chunks, explicit connect, read and
write timeouts, `GET` and `HEAD`, and a URL parser that **refuses `https` by
name** rather than silently downgrading it. `qip-storage`, `qip-quantum` and
`qip-market-ingestion` all speak through it. Wiring it into the probe is an
afternoon.

The real blocker is one paragraph in `infrastructure/egress/envoy.yaml`, and it
is a property somebody chose on purpose:

> `HttpRequest::encode` writes an origin-form request line […] and a `host:`
> header carrying whatever authority the configured base URL had. It never
> emits `CONNECT`, and it never emits an absolute-form URI. A conventional
> forward proxy has nothing to route on […] So the destination is chosen here,
> not by the caller. […] **A process cannot reach a host by asking for it,
> because there is no field in the request in which a host can be asked for.
> That is a stronger property than a host allowlist on a CONNECT proxy, which
> is a filter over something the client controls.**

Every outbound destination is a named `cluster` behind a listener bound to
`127.0.0.1` on a port of its own, and the client picks a destination by picking
a port. On 2026-09-09 there are five: Cloud Storage, BigQuery, Vertex AI, two
IBM Quantum hosts, and `api.frankfurter.dev`. There is no catch-all route to an
unnamed host.

A source-discovery crawler is, definitionally, a process that reaches hosts it
names at runtime — from a catalogue, a directory, a sitemap. That is exactly and
precisely the capability this design was built to make impossible. The two
cannot both be true, and the conflict has to be settled rather than coded
around.

## Decision

**A candidate source is probed only where a reviewed egress route already exists
for its host. The platform does not crawl, and `NetworkProbe` reaches a
destination the same way every other outbound adapter here does: by a base URL
naming a loopback port that Envoy has been configured to mean one upstream.**

Precisely:

- `qip-data-finder` gains a dependency on `qip-transport` — an in-workspace
  library, no third-party crate, so ADR 0002 and ADR 0009's prohibition is not
  engaged. The direction is legal: a service may depend on a lib.
- `NetworkProbe` is constructed with a base URL per source, exactly as
  `qip_storage::gcp` and the Frankfurter connector are. Its `robots`, `head` and
  `sample` become real calls through that base URL.
- The candidate catalogue is a committed file. Each entry names the source **and
  the egress route it is reached through**, so a candidate whose host has no
  route is refused at load rather than attempted and failed at the socket.
- Adding a probeable source is therefore a reviewed act in three files: a
  cluster and listener in `envoy.yaml`, a port in the environment's tfvars, and
  an entry in the catalogue. It is not something a running process can do.

## What this decision is not

**It is not "discovery" in the sense §7.6 uses the word.** The blueprint's
"deep web ingestion — where the edge actually lives" describes finding sources
nobody had listed. Under this decision the platform assesses a list somebody
listed. That is a real capability — the licensing evaluation, the tier
classification, the robots check, the schema fingerprint and the freshness
finding all run against a real endpoint, and none of them ran before — but it is
**not** the section's claim, and the delivery status must not be re-scored as
though it were.

**It is not a permanent refusal of dark-tier sources.** §7.5's hard line is
already enforced at classification, before registration, and this changes
nothing about it. What this decision adds is that a dark-tier candidate now
cannot even be *reached*, because no route to one would survive review — a
second, independent layer under the one `tier.rs` already holds.

**It is not a claim that any deployed process can do this today.** Nothing is
deployed. `execution_nodes = {}` in every environment, the Cloud Run services
carry the sidecar only where the collector digest is set, and no process has
been observed making an outbound call. The wire being real and the wire having
run are different facts, and this record asserts only the first.

## Why not a forward proxy

This is the alternative that would make §7.6 achievable as written, and it is
rejected.

A `CONNECT` proxy with a host allowlist is **strictly weaker** than what stands
today, and the difference is not a matter of degree. Today the client has no
field in which to name a host; the destination is a property of the socket it
connected to. With `CONNECT`, the host is a value the client sends, and the
allowlist is a filter over attacker-controlled input — the same class of control
as a denylist on a URL parser, which `endpoint.rs` already documents as the
thing a permissive parser guesses past.

It would also make the egress proxy the platform's only remaining barrier
between a compromised research process and the open internet, in a repository
whose whole posture is that a guarantee the type system holds beats one a
runtime check holds. Trading that for a capability whose product value is
"assess sources nobody reviewed" is not a trade this record is willing to make
on its own authority.

**If it is ever made, it is a decision of its own**, with its own ADR, its own
threat model, and — at minimum — the enclave confinement `tier.rs::needs_enclave`
already describes, applied to the process doing the fetching rather than to the
data afterwards.

## What it costs

**Three rows that cannot reach `REACHED`.** §7.6 asks for a capability this
decision declines to build. It can be `PARTIAL` — the assessment path runs
against real endpoints — and it cannot be more, and the row should say why
rather than reading as unfinished work.

**A per-source cost that scales badly.** Every new source is three files and a
review. At five sources that is correct and cheap; at five hundred it is a
bottleneck, and the pressure to add a forward proxy will come from that number
rather than from an argument. Naming it here is the point: the number is not the
argument.

**A probe that will look over-engineered for one source.** The first candidate
is `api.frankfurter.dev`, because it already has a cluster, a listener on
`127.0.0.1:9105` and the only end-to-end proof any source in this repository
has. A robots check, a tier classification and a licensing evaluation against a
free public exchange-rate API is a lot of machinery for a foregone conclusion.
It is also the only way to prove the machinery runs at all, and the alternative
— proving it against a source that needs a new egress route — puts an
infrastructure change and a code change in one untested step.

## What would make this wrong

**A route being added without the review it assumes.** The whole property rests
on `envoy.yaml` being read by a person before a host becomes reachable. A
generated cluster list, a wildcard SAN match, or a route added to unblock a
failing test would void this decision without changing a line of it.

**The catalogue admitting a host with no route.** The refusal has to happen at
load, loudly, naming the source. A catalogue that accepted such an entry and
failed later at the socket would produce a run in which some candidates were
assessed and others were "unreachable" for a reason that looks like the
publisher's fault and is ours.

**Anyone reading a `PARTIAL` on §7.6 as progress toward the section.** It is
progress toward *assessing reviewed candidates*. The section's own claim is
declined here, and a later reader who takes the row for a half-built crawler
will build the other half.

## Consequences

**What becomes reachable.** `DataFinder::assess` acquires a production caller
against a real endpoint: robots fetched and parsed, tier classified before
registration, licensing posture evaluated before the catalogue, schema
fingerprinted from a body actually read. Four capabilities that executed only in
tests.

**What stays refused.** Any host without a reviewed route. Every dark-tier
candidate, twice over. The `Registered` and `Licensed` access modes, which need
a credential the probe deliberately does not carry — `tier.rs` already models
them, and modelling is as far as this goes.

**Reversibility.** Nothing is written that a later decision would migrate. The
probe is a client; the catalogue is a file; the routes are configuration. A
future decision to add a forward proxy would add a mechanism beside this one
rather than unpicking it.
