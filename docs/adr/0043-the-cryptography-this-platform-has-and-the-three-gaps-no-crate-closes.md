# 0043 — The cryptography this platform has, and the three gaps no crate closes

**Status:** proposed. Nothing in this record is applied. No crate was added, no
`Cargo.toml` and no `Cargo.lock` was touched, no code was changed. It resolves
DEC-D2 by rejecting the question DEC-D2 asks and answering the one underneath
it.

**Resolves:** DEC-D2 ("In-tree HMAC vs ADR 0009") and unblocks PHASE-B9 ("PQC
keys / real signatures for the payload channel") to the extent a record can —
see "What is still the owner's" for the part it cannot.

**Amends:** [ADR 0002](0002-two-dependencies.md), whose "What would make this
wrong" clause names TLS as the reversal condition and is silent on signatures.
It gains a second named condition and, more importantly, a statement of what
the in-tree hash functions it authorises do *not* provide.

**Relates to:** [ADR 0009](0009-tiered-dependency-policy.md) (the decision
core, and what an I/O edge may take), [ADR 0012](0012-where-a-library-earns-its-place.md)
(the three-part test this record applies and does not pass),
[ADR 0013](0013-identity-verification-earns-a-dependency.md) (the same test
applied and passed, for a different problem),
[ADR 0042](0042-the-console-proves-who-clicked-with-a-keyed-assertion-the-api-verifies.md)
(which composes these primitives and depends on gap 2 below being closed
before it can be implemented as written),
[ADR 0008](0008-edge-cells-decide-alone.md) and
[ADR 0039](0039-a-regions-grant-is-shared-across-its-cells-over-the-mesh.md)
(the capital envelope is the artefact whose signature matters most),
[ADR 0003](0003-paper-trading-by-default.md) and
[ADR 0021](0021-the-blueprint-expects-live-capital-and-this-platform-refuses-it.md)
(the boundary this record does not touch, and the reason the gaps below are
currently affordable).

**Does not amend and cannot amend:** the paper-trading boundary's four layers;
ADR 0021's refusal of any signing or withdrawal path for real capital; ADR
0040's placement of a vendor decision with the owner. This record names a
weakness. It authorises nothing.

---

## Context: the contradiction as posed, and why it is not the real one

The standing statement of the problem is that ADR 0009 forbids in-tree
cryptography while ADR 0002 permits only `serde` and `serde_json`, leaving the
platform with cryptographic obligations, a prohibition on writing the
primitives, and no vetted primitive available.

**The first half of that is a mis-citation, and it is worth correcting before
anything else, because the register has been blocked on it since PHASE-B9 was
opened.** ADR 0009 does not forbid in-tree cryptography. What it says is
narrower and is about one thing:

> **Hand-rolling the clients.** A gRPC implementation, a TLS stack and a
> Google auth flow written in-tree, guarding real money, reviewed by nobody who
> writes TLS for a living. This is the worst option and it looks like the most
> principled one.
> — `docs/adr/0009-tiered-dependency-policy.md:45-48`

That is an argument about *protocol stacks*, made in a document whose subject
is managed-service clients. ADR 0012 extends it to TLS specifically and admits
`rustls` for the I/O edge. Neither record says a word against a hash function,
and ADR 0002 does the opposite — it authorises the in-tree hashes by name:

> Everything else is written in-tree: the linear algebra, the statistics, the
> distributions, the optimisers, the HTTP server, **SHA-256 and HMAC**, the
> random number generator, …
> — `docs/adr/0002-two-dependencies.md:7-10` (emphasis added)

The blanket prohibition exists in exactly one place, a rules file, as one line
with a citation that does not support it:

> - No in-tree cryptography. ADR 0009 forbids hand-rolled crypto.
> — `.claude/rules/architecture/00-boundaries.md:43`

So the contradiction the register recorded is between a rule file and the ADR
it cites, not between two ADRs. That matters because the two produce different
work: reconciling two accepted decisions is an architecture problem, and
correcting a citation is an edit. **Correcting the citation is not this
record's to make** — `.claude/rules/` is outside the paths this work may
change, and a rules file is not an agent's to rewrite on its own reading. It is
named in "What is still the owner's".

Removing the mis-citation does not make the problem go away. It relocates it,
and the relocated problem is sharper and worse than the one on the register.

---

## What the tree actually does today

Read, not inferred from names. Every claim below is a file and a line.

### 1. SHA-256 and HMAC-SHA256 are real, and are not placeholders

`backend/crates/libs/qip-core/src/hash.rs` contains a complete SHA-256: the
sixty-four round constants (`:8-17`), the FIPS initial state (`:19-21`), a
streaming `Hasher256` with a 64-byte buffer and a separate `update_raw` that
"feed[s] padding bytes without disturbing the recorded message length"
(`:89-100`), and the compression function with the real message schedule and
round function (`:102-147`). `hmac_sha256` (`:163-187`) is RFC 2104 as written:
key hashed when longer than the block, `ipad`/`opad` at `0x36`/`0x5c`, inner
then outer.

It is verified against published vectors rather than against itself.
`backend/crates/libs/qip-core/tests/hashing.rs` checks the four FIPS 180-4
short-message vectors (`:6-22`), the million-`a` long-message vector through
the streaming path (`:25-36`), agreement between streaming and one-shot at
every chunk size around the 64-byte block boundary — "where padding bugs hide"
(`:39-50`) — and RFC 4231 HMAC cases 1, 2, 3 and 6, the last of which
"exercis[es] the key-hashing path" (`:53-81`).

**Finding: this is not a stub, it does not shell out, and `sign` does not
return a digest of its input.** The specific failure this record was asked to
look for is not present. Nothing here would be improved by replacing it with
`sha2` and `hmac`, and the suggestion that it would is the least interesting
thing that could be said about this tree.

Two blemishes, neither load-bearing:

- RFC 4231 cases 4, 5 and 7 are not tested. Case 5 is a truncation case and
  does not apply. Cases 4 and 7 are a `0x01..0x19` key and a
  larger-than-block key *and* data; both paths are exercised by other cases,
  so this is thin coverage rather than a hole.
- `constant_time_eq` exists **twice**: `qip-core/src/hash.rs:219-228` and a
  `pub(crate)` copy at `qip-edge/src/envelope.rs:135-144`, in a file that
  already imports `qip_core::hash::to_hex` and `qip_core::hmac_sha256`
  (`:17-18`). Both are correct today and both have tests. Two implementations
  of one security primitive is still a defect of shape: a correction to one
  does not reach the other, and the reason to prefer the shared one is the
  reason the shared one exists.

### 2. Every "signature" in this platform is a symmetric MAC, and the code says so

There are four signing sites. The acceptance suite pins them by name —
`the_application_layer_signs_nothing_but_the_centres_policy_and_halt`,
`backend/crates/tests/qip-acceptance/tests/api_boundary.rs:446-466`, whose
premise assertion reads each file and fails if the declaration moved.

| Site | What it signs | Primitive |
|---|---|---|
| `libs/qip-compliance/src/signing.rs:95` `SigningKey::sign` | compliance artefacts, strategy DNA | `hmac_sha256`, hex |
| `services/qip-capital/src/envelope.rs:167` `EnvelopeIssuer::sign` | the capital envelope a cell trades against | `hmac_sha256`, hex |
| `libs/qip-contracts/src/policy.rs:555` `PolicyPayload::signed` and `:644` `HaltCommand::signed` | the twelve-slot policy payload and the halt command | `hmac_sha256`, hex |
| `edge/qip-edge/src/envelope.rs:153` `sign_payload` | the same envelope, so a test and the allocator agree | `hmac_sha256`, hex |

The discipline around them is better than average, and it is worth recording
because a decision to leave it alone should say what it is leaving alone:

- **Domain separation and key binding.** `SigningKey::sign` mixes the key id
  into the message — "so a signature made under one key cannot be replayed as
  though it were made under another after a rotation" (`signing.rs:92-97`).
- **Injective signing strings.** `policy.rs:567-583` documents the collision
  that a delimiter-joined string admits — `{cell: "a", reason: "100|b|c"}` and
  `{cell: "a|100", reason: "b|c"}` sharing one MAC — states it is not
  reachable today, and length-prefixes anyway, because "a signing scheme that
  is only injective while its inputs stay polite is a defect waiting for the
  field that makes it reachable".
- **Constant-time comparison.** `signing.rs:104-110`, and the edge's own copy.
- **A key-length floor that refuses rather than warns.** 32 bytes, in two
  independent places — `signing.rs:56` ("Below this, a secret is guessable and
  the signature is decoration") and `envelope.rs:147-151` ("A short HMAC key is
  not a weaker signature, it is a guessable one").
- **A refusal, not a warning, for the combination that must not run.**
  `qip-api/src/trust.rs:93-101`: a live-capable ceiling plus a seed-derived
  envelope key stops the process. The same module exists in `qip-fastbrain`
  and `qip-deepbrain`.
- **Secrets excluded from `Debug`.** `signing.rs:46-53`, `envelope.rs:130-137`,
  with a test that the mesh backbone's debug output never carries the trust
  root (`qip-api/src/mesh.rs:1555`).

And the limitation is stated by the code that has it, in three separate module
docs, before this record found it:

> HMAC is symmetric. Whoever can verify can also sign, so a signature proves
> only that *someone holding the key* produced the artifact — never which
> person or service did.
> — `libs/qip-compliance/src/signing.rs:12-14`

> [`EnvelopeIssuer`] signs with HMAC-SHA-256 …, which is **symmetric**.
> Verification and signing use the same key, so every cell that can check a
> grant can also mint one. That is adequate for a single-operator deployment
> and for tests, and it is not adequate for production.
> — `services/qip-capital/src/envelope.rs:22-26`

> It is not a production signing path: HMAC proves possession of a shared
> secret, not the identity of a signer.
> — `edge/qip-edge/src/envelope.rs:149-151`

`docs/operations/external-dependencies.md:322-332` says the same thing about
the deployed shape and states the test it fails: "`grep -rl
'kms\|ed25519\|asymmetric' backend/crates/edge/qip-edge/src` finds only the
comment in `envelope.rs` that points here."

**Finding: the signature is real and the claim it supports is narrower than
the word "signature" implies.** Nothing in the tree overstates it. The
platform's problem is not that a control reads as protection and is not — the
shape the risk rules name by example — it is that a control which really does
fire provides integrity without attribution, and the difference is only
written in module documentation rather than in a type.

### 3. The event-log chain is an unkeyed SHA-256 with no external anchor

`backend/crates/libs/qip-events/src/log.rs`:

```rust
fn compute_record_hash(sequence: u64, previous_hash: &str, event: &AnyEvent) -> Result<String> {
    let value = serde_json::to_value(event)?;
    let material = format!("{sequence}|{previous_hash}|{}", canonical_json(&value));
    Ok(sha256_hex(material.as_bytes()))
}
```
— `:576-580`. `verify_chain` (`:514-528`) walks from `GENESIS_HASH` (`:82`,
sixty-four zeroes), checking each `previous_hash` against the running expected
value and recomputing each `record_hash`.

There is no key, no signature over the head, and no witness outside the
process. Sealing is named as absent and as needing its own record:

> Segmenting the file — sealing a segment, recording its final hash as the next
> segment's genesis, and archiving it — is the remaining half of retention and
> is a separate change; it needs an ADR because it changes what "the log" means
> to a replay.
> — `:51-57`

**Finding: the chain detects a partial edit and does not detect a rewrite.**
Anyone who can write the JSONL can recompute every record hash forward from
the edit in one pass, and `verify_chain` will return `Ok(())`. That is a
correct and useful property against corruption, truncation and a careless
tamper; it is not the property the phrase "hash-chained on purpose" is doing
work for in the governance rule. The gap is not cryptographic — it is custody
— and no crate closes it. This is stated here because it is the claim most
likely to be over-read by a reader who knows the log is hash-chained and does
not know what the chain is anchored to.

### 4. The "attestation path" in `qip-deepbrain` is not cryptographic and does not claim to be

`backend/crates/apps/qip-deepbrain/src/attestation.rs` is a JSON document
mounted from disk recording that a named operator read a named provider's
terms at a stated instant (`ProviderTerms::new`, `:102-136`). It is loaded
with `serde_json::from_str` (`:195`), refuses a blank operator, a blank
provider, a blank citation, an empty array and a repeated provider, and routes
`Deserialize` through the constructor so a file cannot bypass the refusals
(`:68-74`). There is no signature over it and none is implied — the doc says
"The record is evidence, not permission" (`:21`).

The word "attestation" in this repository therefore names two unrelated
things, and the second one *is* asymmetric cryptography: Binary Authorization.
`infrastructure/terraform/modules/binaryauthorization` provisions "an
asymmetric KMS signing key in the platform's existing key ring, a Container
Analysis note, an attestor holding the public half, and a policy whose default
rule is `REQUIRE_ATTESTATION` with `ENFORCED_BLOCK_AND_AUDIT_LOG`"
(`docs/operations/external-dependencies.md:182-186`), and `image.yml` refuses
to bake an artefact the attestor has not signed
(`.github/workflows/image.yml:317-368`).

**Finding, and it is the pivot of this record: the platform already meets an
asymmetric-signature obligation, today, in production, and it does so without
a Rust crate — by delegating the private-key operation to Cloud KMS and the
verification to Google's own enforcement point.** Nobody proposed adding
`ed25519-dalek` to sign container images, and the reason is not the dependency
policy. It is that the private key belongs in a KMS and the verifier belongs
outside the process being verified.

### 5. There is no cryptographically secure random source anywhere in the tree

`backend/crates/libs/qip-core/src/rng.rs:1-6`:

> Every stochastic component (synthetic markets, Monte Carlo, bootstrap, QAOA
> sampling, agent tie-breaks) draws from a seeded [`Xoshiro256`]. The same seed
> always produces the same run — a hard requirement for replay and for the
> deterministic simulation tests.

Xoshiro256 is a fast, well-distributed, entirely predictable generator. It is
the right choice for everything that module names. It is not a source of key
material, nonces or tokens, and the platform has no other source: the kernel
says so in its own words at `runtime/qip-kernel/src/central/plane.rs:587-591` —
the platform "has no ambient entropy and must not grow one, because a replay
of the same [configuration must reproduce]" — which is why
`with_reproducible_key` exists as a separate constructor from `new` (`:596`)
rather than as a defaulted flag, "so the distinction then lives in the call
site a reviewer reads".

**Finding, and it is the one nothing in the tree currently states: this is a
gap with a live consequence for a design already written down.** ADR 0042
decision 3 requires a nonce of "16 random bytes" minted by the console. That
is Node's problem and Node has `crypto.randomBytes`, so ADR 0042 is safe as
specified. But the same absence means no Rust process in this workspace can
generate a nonce, a session token, a one-time re-enrolment code (ADR 0038's
recovery path) or a key, and the next design that needs one will either reach
for `Xoshiro256` — which would be a catastrophic and entirely silent defect,
the exact class ADR 0012 condition 1 describes — or discover the gap at
implementation time. It is recorded here so it is discovered now.

---

## The three gaps, stated separately because they have different answers

Collapsing them into "the platform needs a crypto crate" is what kept DEC-D2
open. They are not one problem.

| # | Gap | Would a vetted crate close it? |
|---|---|---|
| 1 | **No asymmetric signature.** Every signer is also a verifier. A cell holding the envelope key can mint itself capital; the API that writes the audit log can sign any record in it. | **Partly, and the wrong part.** See below. |
| 2 | **No CSPRNG.** No Rust process here can produce an unpredictable byte. | **No — and it does not need one.** See below. |
| 3 | **The event log has no anchor.** A writer with file access recomputes the chain. | **No. Not a cryptography problem.** |

### Gap 1 in detail

An asymmetric signature buys one property that matters here and one that does
not.

**It matters at the cell.** ADR 0008's whole shape is that a cell decides alone
against a grant it already holds. Today `QIP_CAPITAL_ENVELOPE_KEY` is
symmetric and "one variable, one trust root" (`qip-api/src/trust.rs:18-21`),
so the seven regional cells and the centre share one secret and any one of them
can issue capital to any other. Giving each cell only a public half is a real
reduction in blast radius and is exactly what
`docs/operations/external-dependencies.md:327-332` names as the fix.

**It does not matter at the centre, and a crate is the wrong way to get it.**
The property the compliance module asks for is that "verification does not
confer the ability to sign" *and* that the private key is "held in a KMS or
HSM and never in process memory" (`signing.rs:16-18`). A crate gives the first
and explicitly not the second: `ed25519-dalek` signing with a key on the same
heap as the process that writes the log means an API that wanted to write a
false record does not need to forge anything — it holds the key. ADR 0042
makes precisely this argument for its own scope and reaches the same
conclusion ("the asymmetry would defend against a third holder of the secret,
and the slot's readers are exactly the two workloads",
`0042:301-310`).

So the crate would close the cell-side half of gap 1 and leave the
non-repudiation half open while making the platform *read* as though it were
closed. That is the failure mode this repository has a documented history of —
`MaxExpectedShortfall` shipped in every default limit set and could never fire
— reappearing one layer up.

### Gap 2 in detail

The honest answer to "where do sixteen unpredictable bytes come from" is not a
crate. It is `std::fs::File::open("/dev/urandom")` — the operating system's
CSPRNG, read through the standard library, no dependency and no primitive
written. On Linux, which is the only target
(`infrastructure/images/execution-node/`, Cloud Run), this is the same source
`getrandom` and `rand` reach for.

It is an **I/O call**, so it may not live in a lib
(`.claude/rules/architecture/00-boundaries.md:40`: "No lib performs I/O"). It
belongs in a composition root under `backend/crates/apps/**`, injected as a
value the way `qip_core::secret::resolve_from` and ADR 0042 decision 10 already
establish — so that the consuming logic stays a pure function testable without
a device node. That placement is the whole design, it costs nothing, and it is
not implemented. Naming it is the useful half of this gap.

### Gap 3 in detail

A hash chain is evidence against an editor who does not hold the file, and the
platform's log writer holds the file. What closes it is not a primitive but a
**witness the writer does not control**: sealing a segment and recording its
final hash somewhere with an independent retention and an independent write
path — the Cloud Storage evidence bucket that
`docs/operations/external-dependencies.md` already lists among the seventeen
modules, with object versioning and a retention policy, is the obvious
candidate. That is an infrastructure change and a change to what "the log"
means to a replay, which `log.rs:51-57` correctly says needs its own ADR. It is
named here so that gap 3 is never again bundled into a dependency argument.

---

## Decision

**Six decisions.**

### 1. No cryptography crate is admitted, in any tier, by this record

`serde` and `serde_json` remain the workspace's only third-party packages.
`scripts/check-dependencies.sh` is unchanged and its eleven-entry `PERMITTED`
list (`:19-35`) is unchanged.

### 2. The in-tree SHA-256 and HMAC-SHA256 are affirmed, not merely tolerated

They are authorised by ADR 0002 by name, they are verified against FIPS 180-4
and RFC 4231, and they satisfy ADR 0012's three-part test in the *negative*
direction: condition 3 is met by a crate, but condition 1 is not met by the
problem. A wrong SHA-256 does not fail silently — it fails against the first
published test vector, and this workspace runs those vectors. That is the
distinguishing property. TLS and JWT verification fail silently against a live
adversary and never against a fixture; a hash function does not.

**This is the argument that decides it, and it is narrow on purpose.** It
applies to a fixed-output primitive with published vectors and no protocol
surface, no parsing, no negotiation, no key agreement and no ambient
adversary. It does **not** generalise. Ed25519, X25519, RSA, AES-GCM, a TLS
record layer and a JWT parser all have failure modes invisible to a test
vector — malleability, non-canonical encodings, small-subgroup and cofactor
handling, nonce reuse, invalid-curve points, timing — and this record must not
be cited for any of them. Writing any of those in-tree would be, in ADR 0012's
words, "the riskier option wearing the costume of the safer one".

### 3. The duplicated `constant_time_eq` in `qip-edge` is a defect and is scheduled, not tolerated

`edge/qip-edge/src/envelope.rs:135-144` is deleted and its two callers use
`qip_core::hash::constant_time_eq`, which the file's neighbours already import
from. One implementation of a security primitive, one place a correction
lands. This is a small implementer's change and this record does not make it.

### 4. Asymmetric signing is delegated to Cloud KMS, as a service call, never as an in-process primitive — and is blocked on egress, not on the dependency policy

The platform's answer to gap 1 is the shape it already uses for Binary
Authorization: the private half lives in the KMS key ring, the signing
operation is an API call, each cell holds only the public half and verifies
locally.

That is an I/O-edge concern under ADR 0009's tiering, and it needs, in order:

1. **An outbound HTTPS path from a deployed process.** There is none — the
   egress proxy is rendered but nothing is applied (PHASE-B2,
   `.claude/rules/domains/data-and-streaming.md`), and the HTTP client speaks
   plaintext HTTP/1.1 by design.
2. **TLS**, which ADR 0012 already admits (`rustls`) and which is not yet in
   the lockfile because nothing has needed it.
3. **A Google auth flow**, which is the thing ADR 0009 names as the worst
   possible in-tree write.
4. **Public-key verification at the cell**, which is the one piece a small
   crate is genuinely the right answer for, and which is the request that
   should be made — with an ADR — *when it is needed and not before*, in the
   pattern ADR 0013 established: "Nothing is admitted before it is needed"
   (`0013:72-76`).

**The dependency policy is not the blocker and never was.** A crate admitted
today would sit in the lockfile with no key to use, no path to a KMS, and no
deployed process — the platform has `execution_nodes = {}` in every
environment, so no cell of any kind runs anywhere. Admitting it now would take
the supply-chain cost immediately and deliver the property never.

### 5. Unpredictable bytes come from the operating system, read in a composition root

`/dev/urandom` through `std::fs`, in `backend/crates/apps/**`, injected into
the logic that needs it as a value. No crate, no primitive, no I/O in a lib.
Not implemented; specified so that the next design needing a nonce does not
reach for `Xoshiro256`.

**And a refusal to go with it: `Xoshiro256` must never be used for a nonce, a
token, a key, a salt or an identifier that must be unguessable.** A generator
whose entire purpose is that the same seed reproduces the same stream cannot
also be the source of a value an attacker must not predict, and the two uses
are one `use` statement apart in this workspace.

### 6. Anchoring the event log is a separate record and is not a cryptography problem

Stated so it stops being absorbed into this one. `log.rs:51-57` already says a
sealing design needs its own ADR; this record agrees and adds the reason: what
is missing is a witness outside the writer's control, and no primitive supplies
one.

---

## The amendment to ADR 0002

ADR 0002's "What would make this wrong" names one condition — real TLS. It
gains a second, and a statement of scope for the primitives it authorises. The
proposed text, to be applied to `docs/adr/0002-two-dependencies.md` by whoever
accepts this record:

> **What the in-tree hashes are, and are not.** The SHA-256 and HMAC above are
> verified against FIPS 180-4 and RFC 4231 and are affirmed by ADR 0043. They
> provide integrity and proof-of-possession of a shared secret. They do not
> provide non-repudiation, signer identity, or an unpredictable byte, and this
> policy has never authorised writing a primitive that would — Ed25519, ECDSA,
> RSA, X25519, AES-GCM and a TLS record layer are outside it, because their
> failure modes are invisible to a test vector.
>
> **The second reversal condition.** If a cell must hold only a public half —
> so that reading a node's key no longer grants the ability to mint capital for
> every node — then the platform needs asymmetric verification. ADR 0043
> routes the *signing* half to Cloud KMS, which takes no crate. The
> *verification* half at the cell is the one place a small, audited
> signature crate is the right answer, and the day a node is deployed with real
> capital is the day that request should be made, in its own record.

---

## Where this sits in the layering

The dependency-direction argument, since it is what makes the shape admissible.

**This record changes no code, so it moves no edge.** What it commits to is a
shape, and the shape's directions are:

- **Decision 3 (deduplicate `constant_time_eq`)** removes an edge rather than
  adding one. `qip-edge` (edge) already depends on `qip-core` (lib) and already
  imports `to_hex` and `hmac_sha256` from it (`envelope.rs:17-18`). Using one
  more function from the same module adds nothing to the graph. Direction:
  edge → lib. Inward.
- **Decision 5 (entropy from `/dev/urandom`)** is placed in
  `backend/crates/apps/**` precisely so no lib performs I/O. The consuming
  logic takes bytes as a value, so a service or a lib that needs a nonce
  receives one and does not open a device. Direction: app → service → lib.
  Inward. **No lib gains I/O and no lib gains a dependency on a service.**
- **Decision 4 (KMS signing)** is an I/O-edge concern by construction. A KMS
  client is a client; under ADR 0009 it belongs in a crate holding no decision
  logic, and the decision core named in ADR 0009's fenced block —
  which includes `qip-core`, `qip-contracts`, `qip-capital` and `qip-compliance`,
  the four crates that hold every signing site in the table above — stays at
  two dependencies. **The verification seam at the cell is a function of
  `(public key, payload, signature)`**, so `qip-edge` keeps taking bytes and a
  key and never learns what a KMS is. Direction: app (composition root, holds
  the client) → edge/service (holds the verification predicate) → lib (holds
  the encoding). Inward.
- **No service depends on the runtime.** `qip-capital`, `qip-compliance` and
  `qip-contracts` are unchanged by every decision here.
- **Nothing depends on an app.** The entropy source and any future KMS client
  are app-private values, injected downward.
- **No service reads the environment.** `QIP_CAPITAL_ENVELOPE_KEY` is read in
  `main.rs` and passed to `harden_central` as an `Option<&str>`
  (`qip-api/src/trust.rs:72-75`); this record keeps that and adds nothing to it.

---

## The alternatives rejected, and why

**(a) Admit `sha2` + `hmac` (RustCrypto) and delete the in-tree hashes.** The
literal reading of the mis-cited rule. Rejected. `sha2` 0.10 pulls
`digest`, `block-buffer`, `crypto-common`, `generic-array`, `typenum`,
`cpufeatures` and `libc`; `hmac` adds `subtle`. Eight to ten packages against a
current lockfile of eleven, nearly doubling the third-party surface — and
`check-dependencies.sh` lists every transitive package explicitly, by design,
"so that when serde changes what it depends on the change appears here"
(`:15-18`), so this is eight to ten reviewed lines forever. The maintenance
posture is good and the audit history is real (RustCrypto's hashes have had
third-party review and are the de facto standard). It buys, precisely,
nothing: the in-tree code passes the same published vectors the crate passes,
and it would *cost* the ADR 0002 property that a numeric result computed today
is computed identically in five years, for the one part of the tree where
"somebody else may change the semantics" is a real risk to a hash-chained log.
A minor version that changed nothing about SHA-256 would still be a new
lockfile every audit has to re-read.

**(b) Admit `ring` or `aws-lc-rs`, a single audited primitive library.** The
strongest form of "admit a crate", and it deserves its reasons. `ring` is
BoringSSL-derived, is the most widely deployed Rust crypto in existence
(`rustls` depends on it), has genuine adversarial review, and would supply
SHA-256, HMAC, Ed25519, X25519, AES-GCM and — the point — a CSPRNG, in one
package. Rejected on three counts, in order of weight. **It contains
assembly and `unsafe`**, and this workspace is `unsafe_code = "forbid"` at the
root; forbidding `unsafe` in first-party code while depending on a crate built
from it is defensible, but it is a change to what that lint means and it should
be argued in its own record rather than smuggled in under a hash function.
**Its maintenance posture is the weakest part of the case** — `ring` has had
long release gaps and a small maintainer count relative to its blast radius,
which is exactly ADR 0012 condition 3 ("its maintenance is somebody's actual
job rather than a side effect") read honestly rather than charitably;
`aws-lc-rs` answers that and brings a C build dependency and a CMake toolchain
into a build ADR 0002 requires to work offline. **And it still does not solve
gap 1**, because the private key would be in process memory — the thing
`signing.rs:16-18` says must not happen.

**(c) Admit `ed25519-dalek` alone, for signatures only.** Rejected for the
reason set out under "Gap 1 in detail": it closes the cell-side half and
leaves the non-repudiation half open while making the system read as though
both were closed. Additionally, it has no caller today — no execution node
exists in any environment, so the cell that would hold the public half does
not run. Revisit when a node is deployed with a grant, which is decision 4's
step 4.

**(d) Hand-roll Ed25519 in-tree, extending the precedent of the in-tree
SHA-256.** Rejected without qualification, and named because it is the
alternative that would look most consistent with this repository's history and
would be the most dangerous thing in it. Ed25519 has failure modes no test
vector catches: signature malleability, non-canonical scalar and point
encodings, cofactor and small-subgroup handling, the batch-versus-single
verification divergence that produced a cross-ecosystem consensus split, and
constant-time field arithmetic. ADR 0012 condition 2 is met maximally and
condition 1 is met maximally. This is the case its sentence was written for.

**(e) Replace the in-tree HMAC with the Cloud KMS `MacSign` API.** Genuinely
attractive: it removes key material from process memory without changing the
symmetric/asymmetric story, and it takes no crate. Rejected on latency and
availability. `Cell::admit` verifies a capital envelope on the order path, and
`qip-acceptance/tests/performance.rs:1668` holds it to a measured budget. A
network round trip per verification would put a Google API on the critical path
of every order in a design whose founding property is that a cell that cannot
reach the centre keeps working (ADR 0008). It also would not work at all: there
is no egress path from any deployed process.

**(f) Leave DEC-D2 open and change nothing.** The status quo, and it is
cheaper than every option above. Rejected because "blocked-external, awaiting
an owner's call" has been the register's answer since the row was opened, and
the reason it stayed there is that the question was wrong. A register row that
cannot be closed because it asks an unanswerable question is not a blocked
decision; it is a mis-stated one, and it hides three real gaps behind one fake
one. If this record is rejected, the register should at minimum be corrected to
name gaps 1, 2 and 3 separately.

**(g) Argue direction (a) of the brief — admit a specific vetted crate.** Set
out fully above as (a), (b) and (c) rather than dismissed, because it is the
answer this record would have given if the tree had contained what it was sent
to look for. It does not. Had `sign` returned `sha256(payload)`, or had the
chain been a length counter, or had the attestation been a `todo!()`, the
answer would be to admit a crate today and say so. The finding is the opposite
and the honest response is to say the opposite.

---

## What it costs

Declining is not free, and the costs are not hypothetical.

- **A cell that verifies a grant can mint one.** Seven regional cells and the
  centre share `QIP_CAPITAL_ENVELOPE_KEY`. Compromise of any single node is
  compromise of the capital-granting authority for every region. This is
  bounded today only by the facts that nothing is deployed
  (`execution_nodes = {}` in every environment) and that the ceiling is paper
  trading. **Both of those bounds are circumstantial, not structural**, and the
  first one disappears on the day ADR 0035's single shadow node is applied.
  This is the largest carried risk this record accepts.
- **No signature in this platform names a signer.** `Provenance::signer` is a
  self-declared string, as `signing.rs:19-21` says. The platform's claim that
  every decision is attributable therefore rests on the integrity of the
  process that wrote the record, not on evidence a third party could check.
  ADR 0007's exact attribution is arithmetic that reconciles; it is not proof
  of authorship.
- **The audit trail is tamper-evident against an outsider and not against the
  writer.** Gap 3, carried.
- **The maintenance burden of the hash code stays.** Roughly 230 lines in
  `hash.rs` plus its test file, owned forever, and one of the few places in
  this workspace where a subtle bug would be silent until an external party
  disagreed with a digest.
- **This record will be argued from.** ADR 0012 warned about exactly this
  ("The next proposal will cite this ADR"), and the risk here is sharper
  because decision 2's argument — "a wrong hash fails against a published
  vector" — is seductive and wrong for almost every other primitive. Decision 2
  states its own scope for that reason. If it is ever cited for a signature
  scheme, a cipher or a key exchange, the citation is the defect.
- **Three named gaps replace one comfortable ambiguity.** "We use HMAC" reads
  adequately in a summary. "No signer is identified, no byte is unpredictable
  and the log's chain is unanchored" does not, and someone will have to say it
  out loud to a reviewer who expected the first sentence.

---

## What would make this wrong

- **A deployed execution node holding real capital.** The moment a cell exists
  outside `cargo test` with a grant it can spend, the symmetric trust root
  stops being an acceptable bound and decision 4's sequence becomes urgent.
  ADR 0035's node in `dev`, shadow mode, is the trigger to re-open — not the
  day it is authorised, the day it boots.
- **Any autonomy ceiling above paper trading.** Refused by four independent
  layers today, but if it ever changed, every cost above changes category. A
  seed-derived key on a live-capable platform is already refused at start-up
  (`trust.rs:93-101`); an *operator-supplied symmetric* key on a live-capable
  platform is currently accepted, and that is the next refusal to write.
- **Anything in this workspace using `Xoshiro256` for a value that must be
  unguessable.** Decision 5's refusal. If it appears, treat every token,
  nonce and identifier derived from it as public.
- **A second implementation of any primitive appearing anywhere.** Decision 3
  removes one; a third would mean the shared module is not discoverable and
  the answer is to fix that rather than to allow the copy.
- **This record being cited to justify writing a signature scheme, a cipher,
  a key exchange or a protocol parser in-tree.** Decision 2's scope paragraph.
  It is the most likely misuse and it is the one that would do real damage.
- **A crate arriving without its own record.** If a crypto dependency appears
  in `Cargo.lock` and `check-dependencies.sh` grows entries without an ADR
  superseding this one, the policy has failed in the direction ADR 0012 names:
  "the answer is to return to ADR 0002 unmodified rather than to keep
  amending".
- **The mis-cited rules line surviving.** If
  `.claude/rules/architecture/00-boundaries.md:43` still reads "No in-tree
  cryptography. ADR 0009 forbids hand-rolled crypto" after this record is
  accepted, the contradiction is not resolved — it is resolved in one file and
  contradicted in the file agents read first, which is where it did its damage
  the first time.

---

## Applied by this record

**Nothing.** No crate, no manifest, no lockfile, no code, no test, no
configuration and no secret slot. `constant_time_eq` still exists twice, no
process can produce an unpredictable byte, every signature is still a symmetric
MAC, and the event log's chain is still unanchored. This record is the
argument and its evidence; the three implementer's changes it names — decision
3's deduplication, decision 5's entropy source, and the amendment to ADR 0002 —
are separate commits with their own gates.

**Gates run for this record: none, and the reason is not that they were
skipped.** No Rust, TOML, Terraform or TypeScript file was modified, so
`cargo fmt`, `cargo clippy`, `cargo test`, `terraform validate` and the
frontend gates have nothing to judge. `./scripts/check-dependencies.sh` would
be the one gate with a bearing — this record's central claim is that the
lockfile is unchanged — and it **was not run**: the session that produced this
record had no shell. `./scripts/check-secrets.sh` **was not run**, for the same
reason. Neither should be taken on trust. Whoever accepts this record runs
both, and the second one matters because this document quotes source lines
from files that handle key material — every quotation above is a comment, a
constant name, an error message or a variable name, and no secret, key, token
or digest of one appears in it, but that is this author's reading and not a
scanner's finding.

---

## What is still the owner's

1. **Whether to accept decision 1 at all** — the alternative is direction (a)
   of the brief, and (b) `ring`/`aws-lc-rs` is the version of it worth
   considering rather than (a) `sha2`. A desk that wants one audited primitive
   library and is willing to reopen `unsafe_code = "forbid"` and the offline
   build has a coherent position, and this record's rejection of it is a
   judgement, not a proof.
2. **Correcting `.claude/rules/architecture/00-boundaries.md:43`.** A rules
   file is not an agent's to rewrite, and this record's first section is
   useless if the line stands. The proposed replacement: *"No in-tree
   cryptography beyond the SHA-256 and HMAC-SHA256 ADR 0002 authorises and
   ADR 0043 affirms. No signature scheme, cipher, key exchange or protocol
   parser is written in-tree, ever (ADR 0009, ADR 0012). No CSPRNG is written
   in-tree; unpredictable bytes come from the operating system, read in a
   composition root (ADR 0043)."*
3. **Whether the symmetric envelope key is acceptable for ADR 0035's single
   shadow node.** It is one node in `dev` with no real capital, so the answer
   is probably yes — but it should be a stated yes with a date, not an absence
   of a question, because decision 4's sequence takes weeks and starting it
   after the node is trading is starting it late.
4. **Updating DEC-D2 and PHASE-B9 in `docs/plan/PROJECT-PLAN.md`.** This record
   proposes closing DEC-D2 as *mis-stated and answered* and splitting PHASE-B9
   into the three gaps, of which only gap 1 is blocked-external and it is
   blocked on PHASE-B2's egress path rather than on a crypto decision.

## The paper-trading boundary

Untouched, and confirmed rather than assumed. Terraform's refusal of
`supervised_live`, `limited_autonomous_live` and `autonomous_live` at plan time
(`infrastructure/terraform/variables.tf`); `AutonomyLevel::deployable` refusing
the same three at start-up in `qip-api`, `qip-fastbrain` and `qip-deepbrain`;
`qip-edge`'s `Cell` having no constructor taking a ceiling other than paper
trading; and `qip-cost-router`'s `Determinism::Required` arm returning a type
that cannot name a model rung — all four stand exactly as ADR 0003 and ADR 0021
left them, and nothing in this record touches any of them.

Nothing here creates, enables or eases an order path. It adds no capability,
removes no refusal, and changes no default. The one control it discusses that
does fire — `harden_central`'s refusal of a live-capable ceiling on a
seed-derived key (`qip-api/src/trust.rs:93-101`) — is left exactly as it is,
and decision 4's fifth bullet under "What would make this wrong" proposes
tightening rather than relaxing it.
