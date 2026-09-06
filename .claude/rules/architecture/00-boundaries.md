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
  that opens a socket is a service in the wrong directory.
- `backend/crates/services/` — one domain engine each. A service owns its domain and
  exposes it through types, not through reaching into another service.
- `backend/crates/runtime/qip-kernel` — the only place that composes services into a
  cycle. If two services need to know about each other, they meet here.
- `backend/crates/apps/` — composition roots. Configuration is read **here and only
  here**: a service that reads `std::env` cannot be tested and cannot be
  deployed twice with different settings.
- `backend/crates/edge/` — the regional cell. Structurally paper-only.

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

- No service reads the environment. No lib performs I/O.
- No new async runtime. Blocking I/O with explicit timeouts is a decision
  (ADR 0001, ADR 0011), not an omission.
- No hand-rolled protocol stack or asymmetric primitive. ADR 0009's actual
  prohibition is on **clients** — "a gRPC implementation, a TLS stack and a
  Google auth flow written in-tree, guarding real money, reviewed by nobody who
  writes TLS for a living". ADR 0002's Decision section authorises the in-tree
  hashing by name: "SHA-256 and HMAC, the random number generator". Both are
  proven against published vectors (`qip-core/tests/hashing.rs` — FIPS 180-4
  and RFC 4231, 6 passed), which is what makes that narrow case defensible and
  is exactly what a TLS or JWT implementation could never claim: those fail
  silently against a live adversary and never against a fixture.
  This line used to read "No in-tree cryptography. ADR 0009 forbids hand-rolled
  crypto." It was a mis-citation, it contradicted ADR 0002 in the file agents
  read first, and ADR 0043 found it. Do not read the correction as permission:
  asymmetric signing, a CSPRNG, and anchoring the event-log chain are three
  gaps no in-tree code may close — see ADR 0043 (proposed).
- No crate added without an ADR (ADR 0002, ADR 0009).
- No second source of truth for a fact the event log already holds.

## Recording a decision

Consequential decisions go in `docs/adr/` as a numbered ADR, following the
existing ones — **fifty on 2026-09-06**, counted with
`ls docs/adr/ | grep -c '^0[0-9][0-9][0-9]-'`. This sentence said "the
existing twelve" until then, understating the register by thirty-eight and
telling a reader the corpus was small enough to have read. Run the command
rather than trusting this number; it moves every time a decision is
recorded, which is the point of it. They do not live in chat history, a commit message, or an
agent's memory. If you find yourself explaining an architectural choice in a
PR comment, it needed an ADR.
