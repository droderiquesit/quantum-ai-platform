# ADR 0109: Third-party crates are admitted from a committed, audited allow-list, and the decision core stays at two

- **Status**: **Accepted**, 2026-10-11, by the owner in the session
  conversation that day. The owner's words, verbatim, as the coordinating
  session received them first-hand: (1) on the rule that no crate is added
  until this record is approved — "I amend 1 rules don't block for this No new
  crates"; (2) on the async-runtime and hand-rolled-protocol lines — "Amend
  this these not off limits An async runtime (tokio or similar) … Hand-rolled
  TLS, consensus or signing". The lane that drafted this record received both
  through the coordinator's lane amendments and did not see them first-hand;
  this status line is the coordinator's citation that it asked for. The
  second instruction widens this record — an async runtime is allowed and
  hand-rolled TLS, consensus and signing are no longer forbidden — and is
  recorded in "Amendment 2" below, with the drafting author's dissent on one
  half of it, rather than folded silently into the decision.
- **Date**: 2026-10-11
- **Supersedes**: ADR 0002's decision sentence ("The platform depends on
  `serde` and `serde_json`") **for crates outside the decision core only**;
  ADR 0009's unenumerated "vetted allowlist", which is replaced by the
  committed list below; ADR 0043 decision 1 ("No cryptography crate is
  admitted, in any tier, by this record") — 0043's *reasoning* stands and its
  decision 2 (the in-tree SHA-256/HMAC are affirmed) is unchanged. **By
  Amendment 2:** the refusal of an async runtime in ADR 0001, ADR 0011 and
  ADR 0012 ("Refused, and why: an async runtime") is superseded; ADR 0009's
  and ADR 0043's prohibition on hand-rolled protocol stacks and asymmetric
  primitives is superseded **as a prohibition** and survives as this
  record's default.
- **Keeps**: ADR 0009's decision core and its fenced list, untouched — those
  fifteen crates stay at `serde` + `serde_json`. ADR 0012's three-part test as
  the first admission route. ADR 0043 decision 2's scope paragraph, now as
  guidance with an evidence requirement rather than a ban (Amendment 2).
- **Relates to**: ADR 0013 (identity verification earns a dependency), ADR
  0100 (the event fabric's first build; C2), ADR 0099/0101 conflict C2 ("one
  dependency record per class"), ADR 0108 (any move beyond paper trading; P9's
  verifier), and the four records this one gates: ADR 0110 (signing), 0111
  (replication), 0112 (transport), 0113 (data and placement).

## Context

The owner has said the platform is to become a real-money, global-scale
system. At that scale the larger risk is no longer a transitive dependency
changing a quantile; it is the in-tree code this repository would otherwise
have to write to get there — a TLS stack, a consensus protocol, an asymmetric
signature verifier, a storage client's auth flow. ADR 0009 already called
hand-rolling those "the worst option and … the one that looks most
principled", and ADR 0012 admitted `rustls` on the same argument. **Neither
admission ever landed.** `backend/Cargo.lock` holds eleven third-party
packages, every one `serde`, `serde_json` or their closure, and
`scripts/check-dependencies.sh` is an exact eleven-name `PERMITTED` array
with no tiers. Two accepted records said "the edge may take an allow-list";
the enforcement said "two"; the rules file and `CLAUDE.md` said "two"; and
every lane that met a TLS, consensus or signing requirement stopped and
reported "blocked on dependency ADR". That is the state ADR 0101's C2 row
describes, and the R12 register carries the result: FABRIC-7046, FABRIC-8083,
SEC-8100, TICK-2057 and the W1.4/W4.4/W5.1 clusters are blocked on a rule the
accepted records had already relaxed in principle.

Three facts from the tree shape the decision:

1. **An in-tree Google REST adapter already exists and is waiting on TLS.**
   `backend/crates/libs/qip-storage/src/gcp/` speaks the Cloud Storage JSON
   API, takes bearer tokens from the instance metadata server
   (`gcp/auth.rs`: "a complete answer that needs no key material"), and
   refuses the `https` scheme by name rather than downgrading
   (`gcp/mod.rs`, `gcp/storage.rs`). The thing it lacks is exactly one crate.
2. **The in-tree primitives that pass published vectors are not the problem.**
   SHA-256, HMAC-SHA256 (`qip-core/src/hash.rs`) and CRC32C
   (`qip-events/src/event_fabric/crc32c.rs`) fail loudly against a vector.
   ADR 0043 decision 2's argument holds for them and this record does not
   reopen it.
3. **Several crates in the owner's draft list assume an async runtime.**
   When this record was first drafted the async refusal was settled, and
   admitting one of them would have admitted an executor by the back door.
   Amendment 2 settles it the other way, *explicitly*, which is the
   difference between a decision and a drift.

## Decision

### 1. Two tiers, one committed list

- **The decision core** — the fenced list in ADR 0009, read by
  `the_decision_core_named_by_adr_0009_is_the_set_actually_held_to_two`
  (`qip-acceptance/tests/architecture.rs`) — keeps `serde` and `serde_json`
  and nothing else, *including transitively*. Unchanged.
- **Every other crate** may depend on a crate that appears in a committed
  allow-list file, and on nothing else. The file — proposed as
  `scripts/dependency-allowlist.txt` — has one line per package in the
  resolved lockfile (direct and transitive), each carrying the admitting ADR
  number and a one-line reason. `scripts/check-dependencies.sh` reads it and
  refuses any lockfile package not on it. A dependency still appears in a
  diff a reviewer reads; it is now a diff to a data file rather than to a
  script.
- `no_crate_declares_a_third_party_dependency_beyond_the_two_permitted` is
  retired in the same change that lands the first admitted crate, and
  replaced by a tier test that walks each edge crate's manifest against the
  allow-list. ADR 0009 predicted this day: "whoever adds Pub/Sub will relax
  the strict check, and on the day they do, something has to still be
  holding the core". The core test is that something.

### 2. Admission conditions — every one must hold

1. **ADR 0012's test, or a measured cost.** Either the failure is silent, the
   problem adversarial or specialist, and a maintained implementation exists
   (ADR 0012) — or a *measurement* committed beside the admission shows a
   cost the in-tree alternative cannot meet (compression ratio on real tick
   volume; throughput the fabric actually needs). Intuition does not count;
   ADR 0012's async refusal set that standard.
2. **One async runtime, and never on the hot path.** *As amended by
   Amendment 2; the original condition refused every runtime.* `tokio` is the
   one admitted executor; `async-std`, `smol` and any second executor stay
   refused as a `cargo-deny` `bans` entry, because two runtimes in one binary
   is two schedulers nobody reasons about together. `tokio` is refused in the
   decision core (ADR 0009's list) and in `qip-edge`'s decision path — the
   `Cell::work` pass and everything it calls — by a tier test, unless a
   committed measurement shows it helps there (the owner's own condition).
   Every call that leaves the process keeps an explicit timeout, async or not.
3. **Pinned exactly.** `=x.y.z` in the consuming crate's `Cargo.toml`,
   `default-features = false`, only the features named in the admission. An
   upgrade is a reviewed diff with its own `cargo vet` certification.
4. **Audited, and the audit recorded in-tree.** `cargo vet` with its
   `supply-chain/` directory committed; every admitted version is either
   certified by an imported audit set the admission names or audited by a
   named person in this repository. An exemption is a dated entry with an
   owner and an expiry, never an open-ended one.
5. **Licence and source.** `cargo deny check licenses sources advisories
   bans` passes: permissive licences only (the exact set fixed in `deny.toml`
   in the change that adds it), crates.io as the only source, no yanked
   version, no open advisory without a dated acknowledgement.
6. **Offline and reproducible.** Sources vendored (`cargo vendor`) or
   mirrored with checksums, so the build that reproduces a decision still
   needs no network — ADR 0002's second reason, restored rather than given
   up as ADR 0009 conceded.
7. **SBOM.** The CycloneDX/SPDX output `make sbom` already produces names
   every admitted package and version, and is attached to the image
   attestation `deploy.yml` signs.
8. **`unsafe`, C and assembly are named, not hidden.** `unsafe_code =
   "forbid"` governs first-party code and is unchanged. An admitted crate that
   contains `unsafe`, C or assembly says so in its allow-list line and its
   admission. ADR 0043 objected to `ring` partly on this ground; the
   objection is answered by stating it, not by pretending a crypto library can
   be written without it.

### 3. What each admission costs to make

- **A crate that implements cryptography, consensus, TLS, a storage client,
  or a codec for a persisted format** needs its own ADR (0110–0113 are
  those). These are decisions with consequences beyond the crate.
- **Any other crate** — a utility a lane needs during implementation under
  the owner's "don't block" instruction — is admitted by an allow-list line
  citing this record, plus an entry in the lane's report naming the crate,
  version, features, the item that needed it and why the in-tree alternative
  was worse. It must still meet conditions 2–8. The merge reviewer rejects a
  line that does not.

### 4. The list as of this record

Only the first two rows are admitted by this record. The rest are the owner's
named candidates, each with the disposition this record gives it.

| Crate | Disposition | Why |
|---|---|---|
| `serde`, `serde_json` (+ closure) | **Admitted** (ADR 0002) | Unchanged. |
| `rustls` (+ `rustls-webpki`, `rustls-pki-types`, provider) | **Admitted on landing**, ADR 0112 | ADR 0012 already admitted it; this record makes it enforceable. Synchronous API — no runtime needed. |
| `ring` | **Admitted on landing**, ADR 0110 / 0112 | One crypto library for both the TLS provider and edge signature verification. Contains C and assembly — named. Maintenance posture **to be verified at the pinned version** in ADR 0110's admission step; ADR 0043 raised it and it is not dismissed here. |
| `ed25519-dalek` | **Not admitted alongside `ring`** | Two crypto libraries for one job is the duplicate-primitive defect ADR 0043 decision 3 removed for `constant_time_eq`. ADR 0110 chooses one verifier; if Ed25519 is chosen *and* `ring` is not admitted, this is the alternative. |
| `tokio` | **Admitted by Amendment 2** (owner instruction, relayed) | The one executor; condition 2 bounds where it may be reached from. Features named per consuming crate; `rt-multi-thread` is not a default. |
| `openraft` | **Admissible under amended condition 2; not chosen by ADR 0111** | Its storage and network traits are `async`, which Amendment 2 now permits. ADR 0111 prefers managed consensus for the reason that survives the amendment: `openraft` supplies the algorithm and leaves this repository writing the log store, snapshotting, transport and membership operations, which is where Raft deployments fail. It is 0111's named fallback. |
| `zstd` | **Deferred to measurement**, ADR 0113 | Fails ADR 0012 condition 1 (a codec fails loudly); admissible only by condition 1's measurement route. Binds C `libzstd`. |
| `crc32fast` | **Refused** | Computes CRC-32 (IEEE). The fabric codec uses CRC-32C, already in-tree and vector-checked (`qip-events/src/event_fabric/crc32c.rs`). It would duplicate a primitive and compute the wrong polynomial for its one obvious caller. Admissible only for a caller that genuinely needs IEEE CRC-32 (gzip or zip interop), with that caller named. |
| `prost` | **Not admitted** | The fabric codec is canonical JSON by design (ADR 0100 §1, hash chains over canonical bytes). FABRIC-7048's "prost codec" gap stays open by decision until a wire format outside this repository requires protobuf. |
| A Google Cloud client SDK | **Admissible under amended condition 2; not chosen by ADR 0113** | Amendment 2 removes the async objection. ADR 0113 still prefers the in-tree REST adapter plus `rustls` plus metadata-server tokens because it already exists (`qip-storage/src/gcp/`), writes no auth flow (the token is an HTTP GET), and its transitive tree is a fraction of an SDK's. An SDK is the right answer for an API with no usable REST surface; that case gets its own admission. |

### 5. Amendment 2 (owner instruction, 2026-10-11, relayed)

Recorded as received: "an async runtime is allowed … keep it out of the edge
decision thread / reflex hot path unless it measurably helps, and keep
explicit timeouts on every call that leaves the process"; and "hand-rolled
TLS, consensus or signing is no longer forbidden … prefer an audited crate by
default; if you write your own, test it against published vectors/specs".

**What this record makes of it.**

- *Async.* Condition 2 above, as amended. This supersedes ADR 0001 and ADR
  0011's blocking-I/O decision and ADR 0012's refusal of `tokio`, for every
  crate outside the decision core and the cell's pass. Within those two, the
  blocking, executor-free shape stands until a measurement says otherwise —
  which is the owner's own carve-out, made enforceable.
- *Hand-rolled protocols.* The prohibition becomes a default with an evidence
  bar. A hand-written TLS record layer, consensus protocol or signature
  scheme requires: (a) the published test vectors for every primitive it
  contains (Wycheproof for ECDSA/EdDSA/AEAD, RFC 8448 for TLS 1.3, the
  specification's own examples otherwise); (b) a model or property test of
  the protocol's state machine (for consensus, a linearisability check under
  injected partitions); (c) a statement in its module doc and the lane's
  report that it is hand-written and why the audited crate was worse.

**The author's dissent, recorded because the owner should see it before
relying on the second half.** ADR 0043 decision 2 drew the line where it did
for a reason a test vector cannot answer: TLS, signature verification and
consensus fail *silently against a live adversary and never against a
fixture* — malleability, non-canonical encodings, timing, a partition
schedule nobody wrote down. Evidence (a)–(c) catches the fixture failures and
not those. On a platform the owner intends to put real money through, this
author recommends that hand-written TLS, consensus and asymmetric
verification **never reach a path that guards capital, policy or the ledger**,
and that the first such implementation be reviewed by someone who writes the
protocol professionally. That recommendation is advice; the owner's
instruction is the decision.

## Consequences

- The decision core is unchanged and still builds from two crates. "Every
  number this platform computes traces to code in this repository" stays true
  of the core, and is stated with that qualifier from now on.
- Lanes stop reporting "blocked on dependency ADR" for utility crates. For
  cryptography, consensus, TLS, storage clients and persisted codecs, they
  stop reporting it once the named ADR is accepted — which is why 0110–0113
  say they are no longer blocked on the *rule*, only on their own acceptance.
- Supply-chain tooling arrives: `cargo-vet`, `cargo-deny`, the vendored
  source tree, the SBOM attachment. These are CI binaries, pinned by version
  and checksum, not build dependencies of the workspace.
- Seven instruction lines contradict this record and must be amended:
  `CLAUDE.md`'s "Two dependencies only … No async runtime" bullet;
  `.claude/rules/00-enterprise-governance.md` "Dependencies and licensing";
  `.claude/rules/10-product-direction.md`'s "Two dependencies" bullet;
  `.claude/rules/architecture/00-boundaries.md`'s "No new async runtime",
  "No hand-rolled protocol stack or asymmetric primitive" and "No crate added
  without an ADR" lines; and `.claude/rules/domains/core-rust.md`'s "Blocking
  I/O" and "Any new dependency without an ADR" lines. **They are not amended
  by the lane that wrote this record**, although it was asked to: they are
  the files every agent reads first, the instruction to change them reached
  this author only through another agent, and an instruction file rewritten
  on relayed authority is the failure ADR 0043 found in `00-boundaries.md`
  (a rule line whose citation did not support it). Proposed text for each is
  in the lane's hand-off report; the owner, or a session acting on the
  owner's own message, applies it. Until then they contradict an accepted
  record, and an agent reading them first will refuse what this admits.

## What it costs

- **The supply chain grows by an order of magnitude.** `rustls` and `ring`
  alone are larger than the current lockfile. That is the price ADR 0009
  named, now paid.
- **Offline builds need a vendored tree** someone keeps current.
- **`unsafe` and C enter the binary** through `ring` (and `libzstd` if 0113's
  measurement admits it). First-party code stays `forbid`.
- **A list needs reviewing.** "Two" was checkable in five seconds; a list is
  checkable only by the script and a human applying conditions honestly.
- **Two concurrency models in one workspace** (Amendment 2). Blocking
  threads in the core and the cell, an executor elsewhere, and a seam between
  them where a blocking call inside a task can stall the executor silently.
  Every such seam needs `spawn_blocking` or a channel, and a reviewer who
  knows to look.
- **Lane-added utility crates under section 3 bypass a per-crate ADR.** That
  is the owner's "don't block" instruction and it is a real loosening; the
  merge review is the only control on it.

## Alternatives rejected

- **Keep ADR 0002 unmodified and keep writing in-tree.** Rejected on the
  owner's direction and on this repository's own argument: an in-tree TLS
  stack, Raft or Ed25519 is the riskier option wearing the costume of the safer
  one (ADR 0012, ADR 0043 alternative (d)).
- **Admit the owner's draft list wholesale.** Rejected for two entries that
  Amendment 2 does not touch: `crc32fast` duplicates an in-tree primitive
  with a different polynomial, and `ed25519-dalek` beside `ring` is two
  crypto libraries for one job. (`openraft` and a Google SDK were rejected
  here on the async ground before Amendment 2; they are now admissible and
  not chosen, see the table.)
- **One ADR per crate with no exception for utilities.** The posture before
  2026-10-11; rejected by the owner's instruction. Kept for the classes in
  section 3.
- **Tier by directory (`libs/` strict, the rest free).** Rejected for ADR
  0009's reason: a crate that wants a dependency has an interest in not being
  core, so the core is a list in a document a test reads, not a directory.

## What would make this wrong

- **A dependency is found to have influenced a number the decision core
  computed** — through a shared type, a serialisation quirk, a float that
  crossed. ADR 0009's reversal, unchanged: widen the core.
- **A second async runtime appears in `Cargo.lock`**, or `tokio` becomes
  reachable from the decision core or `Cell::work` without a committed
  measurement. Condition 2 failed silently and `cargo-deny` or the tier test
  is not doing its job.
- **An `.await` on the decision thread lengthens pass latency** in
  `qip-acceptance/tests/performance.rs` or the fabric's stalled-broker test
  (ADR 0100 §8 test 3). Then Amendment 2's "unless it measurably helps" was
  read as "unless someone thought it would".
- **A hand-rolled TLS, consensus or signature implementation lands** without
  the published-vector and specification evidence Amendment 2 requires, or
  is ever placed on a path that guards real money (see the dissent below).
- **The allow-list grows past a handful of decision-class crates without
  their ADRs**, or `cargo vet` exemptions accumulate without expiry. ADR
  0012's warning: return to ADR 0002 rather than keep amending.
- **A primitive appears twice** — a crate admitted that duplicates an in-tree
  one, or an in-tree one written beside an admitted crate.
- **An admitted crate's maintenance lapses** (no release in a year, a single
  maintainer gone, an unanswered advisory) without a re-review.
- **The owner's acceptance cannot be produced** at merge (see Status).

## Dependency direction

This record moves no code. The edges it permits run: app (composition root)
→ edge-tier crate holding a client or a verifier → admitted third-party
crate. The decision core gains no edge, and the tier test is what holds
that. No lib gains I/O by this record — `qip-transport` and
`qip-storage/src/redis.rs` remain the only socket owners under
`00-boundaries.md`, and `qip-storage/src/gcp/` already speaks HTTP through
`qip-transport`'s client. No service gains a dependency on the runtime;
nothing depends on an app.

## R12 ids

This record unblocks nothing on its own. It removes the *rule* blocker from
every row whose gap is "a dependency record": the rows are listed in the
records that make each admission — ADR 0110 (W1.4), 0111 (W5.1), 0112 (D2),
0113 (W4.4, W4.5, TICK-2057).

## The paper-trading boundary

Untouched. Terraform's refusal of the three live rungs, `AutonomyLevel::deployable`
in `qip-api`, `qip-fastbrain` and `qip-deepbrain`, `Cell::new` taking no
ceiling but paper, `Determinism::Required`, and ADR 0106's ledger refusal of
`simulated=false` all stand. A dependency policy is not an occasion to revisit
them; ADR 0108 governs any move beyond paper and is not amended here.

## Gates

None was run for this record: the session that wrote it had no shell.
`./scripts/check-dependencies.sh`, `./scripts/check-secrets.sh` and
`documentation::every_internal_link_resolves` are the three with a bearing,
and each is **not run**.
