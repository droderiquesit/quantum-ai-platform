# Architecture: layers and boundaries

## The dependency direction

```
libs  ←  services  ←  runtime  ←  apps
  ↖         ↖          ↖
        edge  (regional; depends on libs + a subset of services)
```

Dependencies point **inward only**. A lib may not depend on a service; a
service may not depend on the runtime; nothing may depend on an app.

- `backend/crates/libs/` — shared types and pure logic. **No I/O side effects.** A lib
  that opens a socket is a service in the wrong directory — with two named
  exceptions, and only two:
  - `qip-transport` is the in-tree protocol stack and owns sockets in both
    directions under ADR 0100 (the client, and since then the server moved out
    of `qip-api`). "What must not happen" below states the terms.
  - `qip-storage/src/redis.rs` opens a TCP socket to Memorystore. It predates
    ADR 0100, whose "only library" wording did not mention it. Its module doc
    gives the reason: RESP is a length-prefixed protocol small enough that
    its whole encoder and decoder are two functions, the same trade
    `qip-transport`'s HTTP client makes. It is named here so it is visible.
    That is not a second licence, and it is not widened.

  Every other library still may not perform I/O. The guard that pins the
  allowed set is
  `qip-acceptance`'s `event_fabric_architecture::only_the_named_libraries_open_sockets`.
  It has landed, so this sentence is an enforced check and no longer prose: it
  walks `backend/crates/libs/*/src` and fails if any file outside those two
  names `TcpStream`, `TcpListener`, `UdpSocket` or `ToSocketAddrs`. Locate it with
  `grep -rn 'fn only_the_named_libraries_open_sockets' backend/crates/tests`.
- `backend/crates/services/` — one domain engine each. A service owns its domain and
  exposes it through types, not through reaching into another service.
- `backend/crates/runtime/qip-kernel` — the only place that composes services into a
  cycle. If two services need to know about each other, they meet here.
- `backend/crates/apps/` — composition roots. Configuration is read **here and only
  here**: a service that reads `std::env` cannot be tested and cannot be
  deployed twice with different settings.
- `backend/crates/edge/` — the regional cell. Structurally paper-only.
  One crate there is not on the cell's pass: `qip-peer-link` (ADR 0116), the
  Lane 1 peer wire and session, under `edge/` because it opens sockets and so
  cannot be a library. The node hosts it, and its decision loop reaches it
  only through `qip_peer_link::boundary::ReflexPort`, which offers and reports
  and cannot wait; the protocol it carries is the pure library `qip-peer`.
  `qip-acceptance`'s `mesh_lane` suite (MESH-013) refuses any reflex crate
  that links or names either, and any node module but `main.rs` that reaches
  past the port.

## Composition roots

Every binary's `main.rs` does the same things in the same order, and the order
is the point:

1. Read configuration, refusing anything invalid — including a live autonomy
   ceiling, which stops the process.
2. Bind ports and prove storage writable **before** reporting healthy. A
   process that reported healthy and then discovered its journal had nowhere to
   go was trading with no record for however long that took.
3. Install the trust root; refuse to run live-capable on a reproducible key.
4. Only then serve.

## What must not happen

- No service reads the environment. No lib performs I/O — with one named
  exception. `qip-transport` is the in-tree protocol stack. It owns the
  HTTP/1.1 client and, since ADR 0100, the server moved out of `qip-api`, so
  it opens sockets in both directions. That exception is the whole of it: it
  is not a precedent for a second lib opening a socket, and ADR 0100's "What
  would make this wrong" says so.
- One async runtime, `tokio` (ADR 0109, Amendment 2, accepted 2026-10-11,
  superseding the blocking-only decision of ADR 0001/0011). It stays off the
  decision core and `Cell::work` unless a committed measurement justifies it;
  every call that leaves the process keeps an explicit timeout.
- Prefer an audited crate for TLS, consensus and signing (`rustls`, `ring`,
  and the records ADR 0110-0113 accept). Hand-rolled protocol or
  cryptographic code is no longer forbidden (ADR 0109, Amendment 2), but it
  must be tested against published vectors and its specification, and the
  ADR records a dissent worth reading: hand-written TLS, consensus or
  signature verification should not be what guards capital. The in-tree
  SHA-256 and HMAC remain proven against FIPS 180-4 and RFC 4231
  (`cargo test -p qip-core --test hashing`).
- No crate outside `scripts/dependency-allowlist.txt`; crypto, consensus,
  TLS, storage-client and persisted-codec crates each need their own ADR
  (ADR 0109).
- No second source of truth for a fact the event log already holds.

## Recording a decision

Consequential decisions go in `docs/adr/` as a numbered ADR, following the
existing ones — counted with
`ls docs/adr/ | grep -c '^0[0-9][0-9][0-9]-'`, and **no figure is written
here**. This sentence said "the existing twelve" until 2026-09-06,
understating the register by thirty-eight and telling a reader the corpus was
small enough to have read; it was corrected to a dated "fifty", and the
command printed **74** on 2026-09-14. Two corrections, both stale inside
days. They do not live in chat history, a commit message, or an
agent's memory.

**Allocating the number is a separate problem from counting them, and it has
already cost a day's work.** On 2026-09-14 six lanes running in parallel each
read the register, each found the same highest number, and each wrote the same
next one; six ADRs collided and had to be renumbered to 0065–0074 across 74
files and 74 index rows. Counting is not allocating: the count answers "how
many exist", and every concurrent reader gets the same true answer and the
same wrong conclusion. So when work may be running in parallel — and in this
repository it usually is — **claim the number in the shared index first**, in
its own commit, before writing the ADR body, and re-read the index immediately
before you claim. A number derived from a count you took earlier is a number
somebody else has taken since. If you find yourself explaining an architectural choice in a
PR comment, it needed an ADR.
