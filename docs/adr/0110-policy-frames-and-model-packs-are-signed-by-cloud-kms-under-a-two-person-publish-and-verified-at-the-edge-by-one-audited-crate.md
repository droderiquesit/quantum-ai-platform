# ADR 0110: Policy frames and model packs are signed by Cloud KMS under a two-person publish, and verified at the edge by one audited crate

- **Status**: **Accepted**, 2026-10-11, by the owner in the session
  conversation, adopting the recommendation as drafted: "Update as needed
  remove my decisions make the recommended". Drafted as *Proposed* the same day. **No longer blocked on the dependency
  rule** — ADR 0109 (accepted 2026-10-11, see its status line for how that
  acceptance is evidenced) permits the verify crate this record names. It is
  blocked on its own acceptance and on the infrastructure listed under "Order
  of work".
- **Date**: 2026-10-11
- **Resolves**: owner decision D3 in `docs/blueprint/v12-completion-plan.md`.
- **Implements**: ADR 0043 decision 4 ("Asymmetric signing is delegated to
  Cloud KMS, as a service call, never as an in-process primitive"), and
  makes the request 0043 deferred — "public-key verification at the cell …
  the request that should be made, with an ADR, when it is needed".
- **Supersedes, on acceptance**: nothing in ADR 0043 beyond what ADR 0109
  already superseded (0043 decision 1). 0043's gaps 2 (no CSPRNG) and 3 (the
  log has no anchor) are **not** closed here, and this record must not be
  cited as closing them.
- **Relates to**: ADR 0008 (cells decide alone), ADR 0039 (a region's grant
  is shared across its cells), ADR 0076 (a two-person rule needs a credential
  this platform cannot issue), ADR 0101 C9a (a distilled model package is a
  deterministic artifact loaded only through a reviewed release), ADR 0108
  (P9's per-order approval needs exactly this verifier), ADR 0109.

## Context

Every signature in the platform is an HMAC-SHA256 (ADR 0043 §2): the policy
payload and halt command (`qip-contracts/src/policy.rs`, verified by
`qip-edge/src/policy.rs`), the capital envelope (`qip-capital/src/envelope.rs`,
`qip-edge/src/envelope.rs`), compliance artefacts and strategy DNA
(`qip-compliance/src/signing.rs`, `qip-kernel/src/central/dna.rs`). The code
says itself that "whoever can verify can also sign". Model packs
(`qip-kernel/src/model_serving.rs`, `ModelPack`) are checked only for being
non-empty (R12-MODEL-4023). The central signing key defaults to one derived
from the configured seed (`CentralPlane::with_reproducible_key`,
`qip-kernel/src/central/plane.rs`), and `qip-api/src/trust.rs`'s
`harden_central` refuses that combination **only when the ceiling is
live-capable** — which it never is. So in every deployable configuration
today, anyone who knows the seed can mint a policy frame or a grant.

At real-money, multi-region scale this is the largest carried risk 0043
named: compromise of one node is compromise of the policy and capital
authority for every region.

## Decision

### 1. Keys live in Cloud KMS, HSM-protected, and never leave it

- One **asymmetric signing key per artefact class**: `policy` (policy
  payloads), `halt` (halt commands), `envelope` (paper capital grants),
  `model-pack` (model and Cognitive Compiler packages). Separate keys so that
  a compromise or a rotation of one class does not move the others, and so
  that the halt key can have a different publish rule (decision 4).
- Protection level `HSM`. Algorithm **ECDSA P-256 / SHA-256**
  (`EC_SIGN_P256_SHA256`), chosen because it is the algorithm this record can
  be sure Cloud KMS offers at HSM level and the chosen verifier supports.
  Ed25519 is acceptable instead *if* the admission step verifies KMS offers
  it at HSM level; that is not assumed here.
- Keys are Terraform resources in the platform's key ring (in the security
  project once ADR 0113's organisation exists), with `prevent_destroy`, a
  rotation schedule expressed as *new key versions*, and no workload service
  account holding `cloudkms.signer` on any of them except as stated below.

### 2. Signing is a REST call; verification is local

- **Signing** is `asymmetricSign` over the Cloud KMS REST API, made from a
  composition root through the in-tree HTTP client over `rustls` (ADR 0112)
  with a metadata-server token — the same shape as
  `qip-storage/src/gcp/`. No Google SDK, no auth library, no key material in
  process memory.
- **What is signed** is the existing injective, length-prefixed canonical
  bytes (`policy.rs`'s signing string discipline), prefixed by a domain tag
  naming the artefact class and the full KMS key-version resource name. A
  signature under the `policy` key can therefore never verify as a
  `model-pack`.
- **Verification** at the cell and in each brain is a pure function
  `(trust bundle, class, bytes, signature) -> Result<()>` over `ring`'s
  `ECDSA_P256_SHA256_ASN1` (KMS returns DER-encoded signatures). No network
  call on the verify path — ADR 0043 alternative (e)'s reason, and ADR 0008's.
- **The trust bundle** is a committed file of public keys, one per active key
  version per class, each with its resource name and a not-after instant, and
  a revocation list. It reaches a node baked into the attested image
  (Binary Authorization already proves which bytes run), not fetched at run
  time. Rotation adds a version to the bundle *before* the publisher starts
  using it; revocation removes it and ships an image.

### 3. One audited verify crate: `ring`

`ring` serves both this record and ADR 0112 (it is the `rustls` crypto
provider), so the platform carries one cryptography library, not two. Its
admission under ADR 0109 states: it contains C and assembly; the pinned
version and the features used (signature verification; the TLS provider);
and its maintenance posture **verified at the pinned version**, which ADR
0043 named as the weakest part of the case and which this author could not
check from the session that wrote this record. If that check fails,
`aws-lc-rs` (which also offers a FIPS-validated mode, relevant if the owner's
jurisdiction review E2 under ADR 0108 requires one) is the substitute, and
the substitution is a one-line change to this record, not a new record.

### 4. Publishing is two-person for widening artefacts, one-person for halts

- **Policy, envelope policy and model packs** are published by a workflow
  that runs in a GitHub protected environment with required reviewers and
  self-review prevented, authenticates through Workload Identity Federation
  to the one service account holding `cloudkms.signer` on those keys, and
  signs exactly the bytes in the reviewed commit. The two persons are the
  author of the commit and the environment approver; neither credential is
  minted by this platform, which is the property ADR 0076 says a two-person
  rule needs. Branch protection (ADR 0108 P6) is a precondition.
- **Envelope issuance** — the central plane signing each paper grant per
  cycle — is automated and single-signer by necessity; the envelope key's
  signer is the central plane's own service account. The two-person rule
  applies to the limits an envelope may carry, which arrive through the
  `policy` path above.
- **Halts are one-person.** A halt only restricts. A control that stops
  trading must never wait for a second approver; the on-call operator's
  identity (ADR 0076's per-person credential) signs through the `halt` key.
- **Break-glass** for the widening keys is a Privileged Access Manager
  entitlement with an approver distinct from the requester, logged; whether
  PAM refuses self-approval is to be verified when the entitlement is
  written, not assumed here.

### 5. Until this lands: refuse the seed-derived key outside tests

Independently of KMS, and first: `harden_central` in `qip-api`,
`qip-fastbrain` and `qip-deepbrain` refuses to start **whenever the
deployment is not a test** and the central signing key is reproducible —
not only when the ceiling is live-capable. "Not a test" is an explicit
environment name the composition root reads and refuses when absent (fail
closed), not the absence of a flag. An operator-supplied HMAC key from
Secret Manager is accepted in the interim; it is still symmetric, and the
record says so. **Cost**: `dev` must seed the key slot before this lands, or
every central binary stops in `dev`.

### Order of work

1. Decision 5 (no crate, no KMS — an implementer's change now).
2. `rustls` lands (ADR 0112), lifting `qip-storage/src/gcp/`'s `https`
   refusal; outbound HTTPS from a deployed process exists (egress, ADR 0024).
3. KMS keys and the publish workflow in Terraform, plan shown, owner applies.
4. `ring` verification in `qip-edge` behind the existing `verify` seams, with
   HMAC verification kept for one release behind a refusal that names the
   date it is removed, then removed. A cell accepting both indefinitely
   would be as weak as the weaker.

## Amendment 1 (2026-10-11): a per-cycle policy payload is issuance, and what it may widen carries its own release

**Why an amendment.** Decision 4's first bullet lists "policy" among the
widening artefacts a two-person workflow publishes, and decision 1 names the
`policy` key as the one that signs "policy payloads". Read together they say
a human approves every payload. But a payload is issued per cell per cycle
by the API (`qip-api/src/signing.rs` signs it under the `policy` key; the
round-2 code already did), it carries the halt bit and every slot's
freshness, and a cell narrows as soon as it stops receiving one. A second
approver per cycle is either an outage — no payload, every cell stale and
narrowing, fleet-wide — or a rubber stamp that a two-person rule exists to
forbid. The record already reasoned this way once: decision 4's second
bullet makes *envelope issuance* automated and single-signer "by necessity"
and moves the two-person rule onto "the limits an envelope may carry".
This amendment applies the same reasoning to the payload, by name, rather
than leaving the text saying one thing and the code doing another.

**Decision.**

1. **The per-cycle policy payload is exempt from the two-person rule.** It
   is signed by the API's own service account under the `policy` key, which
   is that account's alone (`cloudkms.signer` on `policy`, `halt` and
   `envelope`; never on `model-pack`). The publish workflow's identity holds
   `cloudkms.signer` on `model-pack` and on nothing the API signs. The two
   sets are disjoint and the Terraform tests assert it.
2. **Exempt is not the same as trusted.** A payload's signature proves one
   process issued it; it must therefore never be what admits a widening item
   to the order path. Each widening item a payload carries must carry its own
   two-person signature, verified *by the cell*, not merely by the centre
   before issuance:
   - **Models — implemented.** The `trained_models` manifest carries, beside
     each named model, the `ModelPack` that released it
     (`ModelManifest::packs`), and every pack names the inline model the
     cell computes — name and `DistilledModel::digest()` — inside its signed
     bytes (`ModelPack::inline_model`). A cell under a trust bundle refuses
     the whole payload when a named model has no pack, a pack names other
     weights or none, a pack fails the `model-pack` key, expires, or names
     one person twice, or a pack is filed for a model the manifest does not
     name (`ModelManifest::check_releases`, called from
     `VerifiedPolicy::verify_with`; proven by
     `qip-edge/tests/model_release.rs`). The kernel names a distillate only
     when an admitted pack covers its artifact *and* its inline model
     (`Platform::model_manifest`). So a compromised API can withhold a model,
     never introduce one.
   - **Limits — not yet, and said so.** The limits an envelope or the
     `risk_envelope` slot may carry still arrive as reviewed configuration
     deployed by `deploy.yml`, not as a separately signed two-person
     artefact the cell verifies. Until they do, a compromised API could
     issue a payload or envelope inside whatever the cell's own
     construction-time ceilings admit — which is the cell's own fence (ADR
     0008, ADR 0039) and paper capital only, but it is not this record's
     two-person rule. That artefact is named here as the remaining gap
     rather than claimed.
3. **Halts stay one-person** (decision 4, unchanged): a halt only restricts.
4. A shared-key cell cannot verify a pack and keeps the pre-amendment
   reading until `SHARED_KEY_REMOVED_AFTER`, after which it does not start.

**What would make this amendment wrong.** A cell under a bundle running an
inline model whose pack it did not verify; the API's service account holding
`cloudkms.signer` on `model-pack`; a per-cycle payload cited as a two-person
artefact; or the limits gap above described as closed before a signed limits
artefact exists and a cell checks it.

## Consequences

- A cell holds only public keys. Reading a node's disk no longer grants the
  ability to mint policy, grants or packages for every region.
- A model pack loads only if its signed digest matches its bytes, which is
  C9a's "loaded only through a reviewed release" made structural.
- ADR 0108's P9 has a verifier to use; P9's approval-record design is still
  its own ADR.

## What it costs

- A KMS call per published artefact and per issued envelope at the centre
  (never at the cell); KMS quota and latency on the issuance path, unmeasured.
- HSM key cost per key version per month (unpriced here; billing on `dev` is
  disabled).
- A trust bundle that must be shipped before a rotation — a missed bundle is
  a fleet that refuses every new frame. That fails closed, which is right,
  and it is an outage.
- One more cryptography library with C and assembly in the binary.

## Alternatives rejected

- **`ed25519-dalek` with the key in process memory.** ADR 0043's argument:
  it gives integrity at the cell and no non-repudiation at the centre, and
  reads as though both were closed.
- **KMS `MacSign`/`MacVerify`.** Removes the key from memory but keeps the
  symmetric story and puts a Google API on every verification.
- **KMS `asymmetricVerify` as a service call at the cell.** A network round
  trip per verification, and a cell that cannot reach Google cannot accept a
  halt — the opposite of ADR 0008.
- **Fetching the trust bundle at run time.** Moves trust from the attested
  image to whatever serves the bundle.
- **Hand-written ECDSA or Ed25519 verification.** ADR 0043 alternative (d).
  ADR 0109's Amendment 2 no longer forbids it; this record rejects it for
  this path anyway, because the verifier guards policy and capital and its
  failure modes (non-canonical encodings, malleability, timing) are invisible
  to the vectors Amendment 2 asks for — ADR 0109 §5's dissent, applied.

## What would make this wrong

- A private key exported, or a signing key created with a protection level
  other than `HSM`.
- A verify path that makes a network call, or accepts HMAC beyond the stated
  removal date.
- A widening artefact published without two distinct human identities in its
  record, or a halt that needs two.
- The seed-derived key starting in any non-test deployment after decision 5.
- This record cited as closing ADR 0043's gap 2 or gap 3.

## Dependency direction

The KMS client (a REST call over `qip-transport` + `rustls`) lives in the
composition roots (`apps/**`) — the only place that reads configuration and
holds a token. The verification predicate lives in `qip-edge` (edge) and in
a lib-level type for the trust bundle (`qip-contracts`), taking bytes and keys
as values; `qip-edge` never learns what a KMS is. `ring` is depended on by
`qip-edge` and the apps, **not** by any crate in ADR 0009's decision core —
`qip-contracts` holds the bundle's *type* and the signing-string encoding, and
stays at two. No lib depends on a service; no service on the runtime;
nothing on an app.

## R12 ids it unblocks

From W1.4: **R12-MODEL-4023**, **R12-MODEL-9009**, **R12-REFLEX-9011**,
**R12-SEC-9109**, **R12-SEC-9137**; and the "KMS-backed signing" clause of
**R12-SEC-8100** (its CA Service and mTLS clauses are ADR 0112's).

## The paper-trading boundary

Untouched. Signing artefacts asymmetrically opens no order path, and the
envelope key signs *paper* grants; ADR 0021's refusal of any signing path for
real capital stands. All four fences — Terraform, `AutonomyLevel::deployable`,
`Cell::new`, ADR 0106 — and `Determinism::Required` are unchanged. Decision 5
tightens a refusal and relaxes none.

## Gates

None run: no shell in the session that wrote this record.
`documentation::every_internal_link_resolves` and `./scripts/check-secrets.sh`
are **not run**.

## Amendment 1 — 2026-10-11, which keys are HSM where, and the order of work's step 3

Made by the owner in the session under the instruction "You make the
recommended decision and have full permission". The status line is unchanged.

1. **Protection level by key purpose, then by environment**
   (R12-SEC-10053). Every **asymmetric signing** key — the four artefact
   classes in decision 1 and the Binary Authorization attestor — is `HSM` in
   **every** environment, `dev` included, because `KmsSigner` refuses a
   non-HSM answer and a `dev` that signs with SOFTWARE keys would be a `dev`
   that cannot run the signer at all. **Encryption** keys (CMEK) are `HSM` in
   `stage` and `prod` and may stay `SOFTWARE` in `dev` and `test`. The single
   `kms_protection_level` variable is replaced by one per purpose, and a
   Terraform test refuses `SOFTWARE` for any signing key anywhere and for any
   key in `stage` or `prod`, with the admitting half proven beside it.
2. **Step 3 of the order of work is authorised now** (R12-SEC-9109): the four
   signing keys and the publish workflow are written in Terraform and the
   workflow tree, applied to `dev` through `infra.yml` with the plan shown,
   and `QIP_ARTEFACT_SIGNER=kms` is set in `dev` once a signed artefact has
   verified against a bundle built from the real key versions. The HMAC
   removal date already in the tree stands.
