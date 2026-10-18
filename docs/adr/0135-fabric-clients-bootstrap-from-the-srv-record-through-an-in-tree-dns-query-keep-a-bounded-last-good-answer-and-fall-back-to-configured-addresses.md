# ADR 0135: Fabric clients bootstrap from the SRV record through an in-tree DNS query, keep a bounded last good answer, and fall back to configured addresses

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It writes a small protocol client in
  the one library licensed to own sockets and admits no crate; the DNS record
  it reads is already declared by the broker module.
- **Date**: 2026-10-18
- **Extends**: ADR 0100 (the fabric, and `qip-transport` as the library that
  owns sockets), ADR 0109 Amendment 2 (hand-written protocol code is allowed
  when tested against its specification and published vectors), ADR 0111
  Amendment 1 (three brokers per production region).
- **Keeps**: ADR 0112 (mTLS on every non-local link; DNS is discovery, never
  authentication), `event_fabric_architecture::only_the_named_libraries_open_sockets`.

## Context

The broker module publishes discovery records and nothing reads them.
`infrastructure/terraform/modules/event-fabric-broker/main.tf` declares an A
record for the brokers and an SRV record
`_qip-event-fabric._tcp.<zone>` pointing at it on the broker port
(`google_dns_record_set.event_fabric_broker_srv_tcp`). The acceptance test
pins both halves of the present state:
`event_fabric_scaling.rs::fabric_087_the_broker_module_publishes_an_srv_record_that_points_at_its_brokers_and_no_client_reads_it`.
Every client is handed one endpoint by configuration —
`qip_transport::event_fabric::transport::HttpTransport` holds a single
`base_url` — and `Broker::metadata` (`qip-streaming/src/event_fabric/broker.rs`)
answers for one stream and partition, so there is no cluster map either.

R12-FABRIC-10016 asks that clients bootstrap from SRV records and tolerate
DNS loss with cached or local behaviour; R12-FABRIC-8098 asks for SRV
bootstrap and the live broker/partition map. `std` has no SRV lookup
(`ToSocketAddrs` resolves A/AAAA only), and no crate on the allow-list does
it.

Locate them with
`grep -n '_tcp' infrastructure/terraform/modules/event-fabric-broker/main.tf`,
`grep -n 'pub struct HttpTransport' backend/crates/libs/qip-transport/src/event_fabric/transport.rs` and
`grep -n 'pub fn metadata' backend/crates/services/qip-streaming/src/event_fabric/broker.rs`.

## Decision

1. **An in-tree SRV query in `qip-transport`.** One function sends a single
   DNS query (RFC 1035 message, QTYPE SRV per RFC 2782) over UDP to the
   resolver the host is configured with, with an explicit timeout and a
   bounded retry, and parses the answer's SRV records and any additional A
   records. It refuses rather than guesses on a truncated answer (TC set),
   a response ID or question that does not match, a compression pointer that
   loops or points forward, or a name longer than the protocol allows. No
   TCP fallback: the answer for a three-broker region fits a UDP message, and
   a truncated one is a configuration fault to surface. It lives in
   `qip-transport` because that is the one library licensed to own sockets.
2. **Tested against the specification.** The parser is tested against
   hand-built wire vectors from RFC 1035 and RFC 2782 (a compressed answer,
   priority and weight ordering, an A record in the additional section) and
   against every refusal above, each with its mutation.
3. **Discovery is never trust.** A name from DNS selects an address; the mTLS
   handshake still authenticates the broker by its certificate's SAN (ADR
   0112). A poisoned answer can make a client fail to connect, never make it
   talk to an unauthenticated broker. In `local` (plaintext, loopback) SRV is
   not used.
4. **DNS loss degrades to the last good answer, then to configuration.** The
   client keeps the last answer it resolved for at most a bounded staleness
   (the record's TTL multiplied by a fixed factor, capped), and past that uses
   the addresses its configuration names. Configured addresses remain
   required, so a client with no DNS at all still starts. Which source
   answered is a metric label (`srv`, `cached`, `configured`), so an operator
   can see a region running on stale discovery.
5. **The cluster map is a broker request arm** (R12-FABRIC-8098). A new
   `Service` request returns, for the streams a caller may read, each
   partition's current leader address and epoch, from the same lease state
   `metadata` already reads. Clients refresh it on a leader-epoch refusal,
   not on a timer.
6. **`fabric_087`'s "no client reads it" half flips in the same commit** that
   makes a client read it: the test is rewritten to assert the client's
   bootstrap names the SRV owner the module publishes, never weakened to
   assert less.

## Consequences

- A broker added or replaced in a region is found by clients without a
  redeploy of every client's configuration.
- Code slices left: the SRV query and parser with vectors (FABRIC-10016,
  8098); the cached/configured fallback with a test that a client keeps
  working when the resolver stops answering (FABRIC-10016); the cluster-map
  arm with a test that a client follows a leader change (FABRIC-8098); and
  the `fabric_087` rewrite.

## What it costs

- A DNS client is protocol code this repository now maintains: about two
  hundred lines of parser and its tests, and every DNS edge case it does not
  handle is a refusal someone may have to read.
- Clients gain a second source of addresses beside configuration, so "which
  broker is this client talking to" has three possible answers; the metric
  label is what keeps that legible.
- Cloud DNS private zones carry a small monthly charge per zone and per
  query volume; the zone and record are already declared, so this record
  adds queries, not resources.

## What would make this wrong

- If the platform ever needed DNSSEC validation (discovery used for anything
  but address selection), an in-tree parser is the wrong tool and an audited
  resolver crate would need its own admission.
- If answers grew past a UDP message (many brokers per region), the refusal
  on truncation would bite and TCP fallback would be needed, with its own
  tests.

## Rows

R12-FABRIC-10016, R12-FABRIC-8098. Each stays PARTIAL until its slice lands
with a mutated test.
