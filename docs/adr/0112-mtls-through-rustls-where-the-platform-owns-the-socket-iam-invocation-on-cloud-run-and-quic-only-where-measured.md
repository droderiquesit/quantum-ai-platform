# ADR 0112: mTLS through rustls where the platform owns the socket, IAM-authenticated invocation on Cloud Run, and QUIC only where measured

- **Status**: **Accepted**, 2026-10-11, by the owner in the session
  conversation, adopting the recommendation as drafted: "Update as needed
  remove my decisions make the recommended". Drafted as *Proposed* the same day. **No longer blocked on the dependency
  rule** — ADR 0109 (accepted 2026-10-11) lists `rustls` and `ring`, and ADR
  0012 had admitted `rustls` in principle since it was written.
- **Date**: 2026-10-11
- **Resolves**: owner decision D2 in `docs/blueprint/v12-completion-plan.md`;
  ADR 0100 §7's "mTLS is BLOCKED(C2)".
- **Amends, on acceptance**: ADR 0100 §7 (plaintext TCP inside the VPC with
  bearer tokens) — replaced on every hop below. ADR 0012's "What would make
  this wrong" (TLS terminated by a sidecar) — answered: on Cloud Run Google
  terminates TLS, so the platform does not; everywhere else the platform owns
  the socket and does.
- **Relates to**: ADR 0024 (Cloud Run + Compute Engine runtime; the egress
  proxy), ADR 0043 (gap 2: no CSPRNG), ADR 0110 (one crypto library), ADR
  0109 (Amendment 2 permits an async runtime and so QUIC stacks).

## Context

The transport is the in-tree HTTP/1.1 client and server in `qip-transport`,
plaintext, with bearer-token identity (`qip-transport/src/event_fabric/auth.rs`).
`qip-storage/src/gcp/` refuses `https` by name because there is no TLS stack.
The register: FABRIC-7046 and FABRIC-8083 BLOCKED on this; SEC-8100 MISSING
("CAS issuance, mTLS verification"); API-8113 and GCP-8119/8055 PARTIAL (no
authenticated service-to-service path). *The completion plan cites the last
three as FABRIC-8100/8113/8119; their ids in the register are R12-SEC-8100,
R12-API-8113 and R12-GCP-8119.*

Two facts decide the shape. **Cloud Run terminates TLS at Google's front end**
and does not hand a container the client certificate, so in-container mTLS to
a Cloud Run service cannot work. **Compute Engine processes** — `qip-edge-node`,
`qip-fabricd` (`modules/event-fabric-broker`), `qip-ledgerd` — own their
sockets, and nothing but the platform can authenticate them.

## Decision

### 1. Three kinds of hop, three answers

| Hop | Mechanism |
|---|---|
| Between processes on Compute Engine, and Compute Engine → Compute Engine across regions (edge-node ↔ fabricd, fabricd ↔ ledgerd, cell ↔ cell on the peer mesh) | **mTLS 1.3 through `rustls`**, both sides presenting workload certificates from Certificate Authority Service; the peer identity is the certificate's SAN, mapped to the workload's service account, and replaces the bearer token for authorisation. |
| Into a Cloud Run service (any caller) | **IAM-authenticated invocation**: an ID token from the metadata server with the service URL as audience, verified by Cloud Run's front end; the service keeps `INGRESS_TRAFFIC_INTERNAL_ONLY`. No in-container TLS. |
| Out to a Google API (KMS, Spanner, Storage, CA Service) | **TLS through `rustls`** from the process, server-authenticated against a pinned root set, metadata-server bearer token. Loopback to the egress sidecar for third-party vendors stays plaintext: it is not a network hop and the proxy enforces the allow-list (ADR 0024). |

### 2. Workload certificates

- A CA pool per environment in CA Service, with a certificate template that
  constrains the SAN to the caller's own service-account identity, so a node
  can only obtain a certificate naming itself.
- Each Compute Engine process generates an ECDSA P-256 key **in memory at
  start** from the operating system's entropy (`/dev/urandom` read in the
  composition root — ADR 0043 decision 5, finally implemented by this need)
  through `ring`, and requests a short-lived certificate (hours, not months)
  with the CA Service REST API, sending the public key in the request's
  `config` rather than a CSR, so no ASN.1 request encoder is written. It
  renews at two-thirds of lifetime. The private key never touches disk.
- **Fail closed**: a process that cannot obtain or renew its certificate
  stops accepting and making mTLS connections and reports not-ready, rather
  than falling back to plaintext. There is no plaintext fallback flag.

### 3. One crypto library

`rustls` with the `ring` provider (ADR 0110 §3). If `ring` fails its
admission check, `aws-lc-rs` replaces it for both records at once.

### 4. QUIC only where measured

No QUIC now. The blueprint's QUIC fabric is admitted only for a hop where a
committed measurement shows head-of-line blocking or connection setup on
TCP+TLS costs something the platform needs — most plausibly the
cross-region peer mesh. ADR 0109's Amendment 2 makes an async QUIC stack
(`quinn`) admissible; the measurement, not the dependency rule, is the gate.

## Consequences

- The bearer-token identities file and its key-scoped produce ACL are
  replaced by certificate identity. A stolen token no longer impersonates a
  cell; a stolen key dies with its certificate within hours.
- `qip-storage/src/gcp/`'s `https` refusal is lifted by construction, which is
  what ADR 0110 (KMS) and ADR 0113 (Spanner) need first.
- ADR 0043's gap 2 closes as a side effect: the composition roots gain an
  OS-entropy source, injected as a value.

## What it costs

- `rustls` and `ring` in every Compute Engine binary — the largest supply
  chain growth in the repository's history (ADR 0109).
- CA Service cost and a dependency on it at start-up and renewal; a CA outage
  longer than a certificate's remaining life stops the fabric's links. The
  lifetime is the trade between that and key exposure.
- Handshake cost on every new connection; the client keeps connections alive
  to amortise it.

## Alternatives rejected

- **A service mesh (Cloud Service Mesh, sidecars).** ADR 0012's own reversal
  condition. Rejected because no workload runs on Kubernetes (ADR 0036, ADR
  0114) and a sidecar on a bare Compute Engine node is an Envoy the platform
  operates in every hot region, in front of the order path.
- **mTLS into Cloud Run.** Not possible in-container; an internal load
  balancer with mTLS adds a hop to authenticate a caller Cloud Run IAM
  already authenticates.
- **Long-lived certificates from a private CA in Secret Manager.** A key at
  rest for a year is the thing short-lived workload certificates exist to
  avoid.
- **Hand-written TLS.** Permitted by ADR 0109's Amendment 2; rejected for the
  same reason ADR 0009 gave and ADR 0109 §5 repeats.

## What would make this wrong

- Any plaintext fallback, flag or environment value that lets a link come up
  without mTLS.
- A certificate issued with a SAN that is not its holder's identity.
- A private key written to disk or logged.
- TLS added to the cell's decision thread (it belongs on the drain and mesh
  threads).
- QUIC adopted without its measurement.

## Dependency direction

TLS lives in `qip-transport` — the one lib permitted sockets (ADR 0100,
`00-boundaries.md`) — behind its existing client and server types, so no
other lib gains I/O. Certificate acquisition, entropy and token fetching live
in the composition roots (`apps/**`) and are injected as values. `qip-edge`
sees a connected, authenticated stream and a peer identity; it does not learn
what a certificate is. No decision-core crate gains a dependency; no service
depends on the runtime; nothing depends on an app.

## R12 ids it unblocks

**R12-FABRIC-7046**, **R12-FABRIC-8083**, **R12-SEC-8100** (CAS issuance and
mTLS clauses; KMS signing is ADR 0110), **R12-API-8113** (IAM-authenticated
invocation), **R12-GCP-8119**, **R12-GCP-8055**; and the transport half of
W5.4 (peer mesh sockets). **R12-FABRIC-7048** ("prost codec") is **not**
unblocked: the codec stays canonical JSON (ADR 0109's table).

## The paper-trading boundary

Untouched. An authenticated channel opens no order path; the venue
credential's IAM binding and every one of the four fences are unchanged.

## Gates

None run: no shell in the session that wrote this record.

## Amendment 1 — 2026-10-11, what satisfies the rows this record named

Made by the owner in the session under the instruction "You make the
recommended decision and have full permission". The status line above is
unchanged.

1. **IAM-authenticated invocation is the platform's answer to R12-API-8113,
   and the internal Application Load Balancer with mTLS is refused.** The
   round-2 audit was right that this substitutes a mechanism; this amendment
   is the owner deciding the substitution, for the reason §"Alternatives
   rejected" gives (an extra hop to authenticate a caller Cloud Run's front
   end already authenticates). 8113 is satisfied when a plan shows every
   catalogue service at `INGRESS_TRAFFIC_INTERNAL_ONLY` with
   `roles/run.invoker` held only by named service accounts, the invoker
   manifests are applied, and a call from inside the VPC with no ID token,
   and one with a token for a principal not granted, are each refused — the
   evidence the audit found missing.
2. **R12-SEC-10031 ("Cloud Service Mesh mTLS for warm services")** is
   satisfied by the hop table in §1: mTLS through `rustls` wherever the
   platform owns the socket, IAM invocation into Cloud Run. The mesh itself
   stays refused (ADR 0114 D4). Its "the mesh never carries Reflex traffic"
   clause holds already and keeps its test.
3. **QUIC stays measured-only** (R12-FABRIC-7046, R12-FABRIC-8083). Their
   mTLS halves are built. The remaining work is the measurement §4 asks for —
   TCP+TLS against QUIC on the cross-region peer-mesh hop, committed — and
   `quinn` is admitted by its own allow-list record only if that measurement
   shows a cost the platform needs removed. Both rows are buildable work, not
   blocked rows.
4. **`prost` stays unadmitted** (R12-FABRIC-7048), confirming ADR 0109's
   table: the fabric's codec is canonical JSON under the schema lock until a
   wire format outside this repository requires protobuf. 7048 stays BLOCKED
   by that decision.
