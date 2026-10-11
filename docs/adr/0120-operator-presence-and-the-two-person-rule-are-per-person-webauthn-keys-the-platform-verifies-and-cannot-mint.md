# ADR 0120: Operator presence and the two-person rule are per-person WebAuthn keys the platform verifies and cannot mint

- **Status**: **Accepted**, 2026-10-11, by the owner in the session, under the
  instruction: "You make the recommended decision and have full permission".
- **Date**: 2026-10-11
- **Decides**: the three questions ADR 0076 left as "still the owner's" —
  whether to build its decision 1, which A2 shape, and whether the desk wants
  a two-person rule. It is ADR 0075's Gate A.
- **Amends**: ADR 0076 decision 1 (the per-person credential becomes a public
  key, not a bearer token) and decision 2 (`Presence` gains the one variant
  ADR 0076 said could exist once "something checks a person").
- **Keeps**: ADR 0018 and ADR 0076 decision 3 (the console holds the `viewer`
  token and nothing more), ADR 0065 (no instant is fabricated), ADR 0076
  decision 5 (nothing here eases an autonomy escalation), ADR 0110 §4 (halts
  are one-person), ADR 0108 in full and unaccepted.
- **Relates to**: ADR 0043 gap 1, ADR 0109 (allow-list), ADR 0110 (`ring` is
  admitted for ECDSA P-256 verification), ADR 0112 (operating-system entropy
  in the composition root).

## Context

`Principal::presence` always answers `Unattested`, so
`Principal::authentication_instant` refuses, and every signature-gated route
in `qip-api` refuses with it: the two ledger decisions, registration
approval, promotion approval, recalibration approval, venue reinstatement,
clearing the kill switch, and the capital-grant route ADR 0075 authorised in
shape. The register scores the consequence on four risk rows: the
confidence governor narrows the next capital grant in kernel tests, and
**no shipped binary can ever issue a grant for it to narrow**
(R12-RISK-4111, 4112, 4114, 4115; `qip-api/src/auth.rs`, the
`authentication_instant` refusal).

ADR 0076 split Gate A into the subject, the instant and the carrier, refused
to fabricate the instant, and showed that a two-person rule is honest only
when "the two signatures are produced by two credentials the platform cannot
produce". It recommended an identity terminator in front of `qip-api` (its
shape (c)) and left the choice open because, at the time, no
signature-verification crate was admitted. ADR 0110 has since admitted
`ring` for exactly the primitive needed — ECDSA P-256 with SHA-256 — and ADR
0112 gives the composition roots an entropy source. The obstacle ADR 0076
named is gone.

## Decision

### 1. Each operator is a public key, enrolled by a reviewed change

- An operator is a **WebAuthn platform or roaming authenticator** (a passkey
  or a hardware key) holding an ECDSA P-256 key the person alone controls.
  The platform holds only the public key.
- Enrolment is a **committed file per person per environment**: the file
  name is the subject (ADR 0076 decision 1's pattern and refusals:
  `[a-z0-9][a-z0-9._-]{1,62}`, nothing ending `@env`, no duplicates), the
  contents are the 65-byte uncompressed P-256 point, hex-encoded. The
  enrolment tool derives it from the browser's SPKI by checking the fixed
  P-256 SPKI prefix exactly and refusing anything else. The files are baked
  into the attested image beside the trust bundle (ADR 0110 §2), so Binary
  Authorization proves which operators a running `qip-api` recognises.
- Adding or removing a person is a pull request under CODEOWNERS with
  required review (ADR 0108 P6/P7 are the preconditions this inherits): the
  author and the reviewer are two people, neither of whose credentials the
  platform mints.
- **This replaces ADR 0076 decision 1's per-person bearer token.** A
  directory of public keys carries the subject *and* the presence proof and
  holds no secret, so there is nothing to mount from Secret Manager and
  nothing whose theft impersonates a person. The four role bearer tokens are
  unchanged.

### 2. Presence is a fresh signature over a challenge the API issued for that request

- `qip-api` gains `POST /operator-challenges`, which takes the method, path
  and SHA-256 of the body of the act the operator intends, and returns a
  challenge: `issued_at`, sixteen bytes from the operating system's entropy,
  and an HMAC (in-tree, RFC 4231-proven) over those plus the request digest,
  under a key held by `qip-api` alone. The API keeps no table of challenges.
- The act is then sent with a WebAuthn assertion over that challenge. The
  API accepts it only if every one of these holds, and refuses naming the
  first that fails:
  1. the challenge's MAC verifies and it is no older than **120 seconds**;
  2. the request's method, path and body digest are the ones the challenge
     was issued for;
  3. `clientDataJSON` has type `webauthn.get`, the challenge, and the
     deployment's configured origin, exactly;
  4. `authenticatorData`'s relying-party hash is SHA-256 of the configured
     relying-party id, and both the *user present* and *user verified* flags
     are set;
  5. the signature verifies with `ring`'s `ECDSA_P256_SHA256_ASN1` over
     `authenticatorData ‖ SHA-256(clientDataJSON)` under an enrolled key;
  6. the challenge's nonce has not been used: it is recorded in the event log
     in the same append that records the act, and an act whose nonce is
     already in the log is refused.
- `Presence` gains `Attested { subject, authenticated_at }`, constructed in
  exactly one place — the verifier above — with `authenticated_at` set to the
  challenge's `issued_at`, a server instant no older than 120 seconds when
  checked. ADR 0076's condition for this variant ("the refusal stands until
  something checks a person") is met: a signature over a server nonce, by a
  key the platform cannot produce, from an authenticator that asserts user
  verification. A bearer token, a header, or the request clock still produces
  `Unattested`, and the routes above still refuse it.
- **Halts stay one-person and need no challenge** (ADR 0110 §4): a control
  that only restricts must never wait. Clearing the kill switch is not a
  halt; it widens, and it needs an attested operator.

### 3. The carrier is the assertion, not the console's token

The console keeps the `viewer` token (ADR 0076 decision 3 holds). It relays
the operator's request with the assertion attached, and the API derives
`Role::Operator` **for that one request** from a verified assertion and from
nothing else. A console compromise therefore still cannot act: it holds no
key and cannot produce an assertion. This is a fourth shape beside ADR 0076's
(a), (b) and (c), and it removes (b)'s defect — two identity namespaces
joined by a table — because the subject is the enrolled file name and the
console's Identity Platform subject is never consulted for authority.

### 4. The two-person rule is kept, and is now honest

`ApprovalChain`'s dual-approval threshold, `Platform::approve_promotion`'s
"a second session is not a second person", and
`qip_compliance::approval::MAXIMUM_CREDENTIAL_AGE` are unchanged. Each
approver's `OperatorCredential` is built only from an `Attested` presence,
with method `"webauthn"` — which, from the day this lands, is true. Two
approvals need two enrolled keys; one person with two sessions has one key.

### 5. What this record does not open

- **No order path, and no seam to one.** The verifier may be called only
  from `qip-api`'s routes listed in Context. It is not wired to `Cell::send`,
  to `AutonomyController::request_change` beyond the paper ceiling, or to any
  order seam. ADR 0108's P9 per-order approval is that record's to design if
  it is ever accepted; this record neither accepts ADR 0108 nor moves any of
  its stages.
- **Autonomy.** `request_change` becomes satisfiable by two attested
  operators, and it still refuses any level above the deployment ceiling,
  which every one of the three layers holds at paper trading. No live level
  is reachable.
- **Capital.** The capital grants this unblocks are ADR 0110's paper
  envelopes. ADR 0021's refusal of any signing path for real capital stands.

## What it costs

- **Every operator needs an authenticator** and a reviewed enrolment per
  environment, and losing one is an enrolment change and an image promotion
  before that person can approve again. Revoking a person has the same
  latency; ADR 0076's runbook point stands — a halt needs no approval, so
  revocation latency never delays stopping the platform.
- **A compromised console can show one thing and ask the authenticator to
  sign another.** The challenge binds the exact request, but the person sees
  the request through the console. The two-person rule does not defend
  against this, because both approvers read the same console. The mitigation
  is that the challenge endpoint returns the canonical terms and the console
  is required to render them verbatim, and an incident review can compare the
  journaled terms with what the person says they approved; it is a residual
  risk, stated rather than solved.
- **An HMAC challenge key** in `qip-api`, rotated like the other platform
  secrets; its theft lets an attacker mint *challenges*, never assertions.
- **A WebAuthn verifier in-tree.** The parsing is small (a fixed binary
  layout for `authenticatorData`, JSON for `clientDataJSON` through
  `serde_json`), and the cryptography is `ring`'s; it must still be tested
  against the WebAuthn specification's examples and mutation-verified field
  by field.
- **The CLI loses operator acts** until it has an authenticator path; it
  keeps viewer and monitor use.

## Alternatives rejected

- **ADR 0076 decision 1 as written (per-person bearer tokens).** It names a
  person and proves nothing about presence; it would have been a second
  credential system to retire once this one existed.
- **Identity-Aware Proxy in front of `qip-api` (ADR 0076's shape (c)).**
  Still a reasonable design, rejected because the assertion it forwards is
  signed by a Google key under a session the platform does not bind to the
  request, so the instant is a session's age rather than a person's act, and
  the two-person rule would rest on one external session store.
- **The console holding the operator token.** ADR 0076 decision 3; refused
  again.
- **Per-person Cloud KMS keys.** Unmintable by the platform, but a KMS key a
  person signs with through a service account is a key the service account
  can sign with; presence would be a claim about IAM, not a person.

## What would make this wrong

- An `Attested` presence constructed anywhere but the verifier, or from
  anything other than a verified assertion over an unexpired, unused,
  request-bound challenge.
- An assertion accepted without the *user verified* flag, for a different
  origin or relying party, or under a key not in the attested image.
- A dual approval completed with one enrolled key.
- The verifier reached from an order seam, or used to raise autonomy above
  the deployment ceiling.
- Evidence that operators approve terms the console misrendered — then the
  residual risk above has become an incident, and the answer is an
  out-of-band rendering channel, not a weaker check.

## Dependency direction

- The verifier is a pure function in `qip-api` (an **app**) or in a small lib
  taking `(enrolled keys, challenge key, request digest, assertion, now)` as
  values; `ring` is depended on by that crate and the app, not by any
  decision-core crate (ADR 0110's rule). The enrolled keys and the challenge
  key are read in `qip-api`'s `main.rs`, the composition root.
- `qip-compliance` (lib) and `qip-kernel` (runtime) receive an
  `OperatorCredential` value exactly as today; they learn nothing about
  WebAuthn.
- The portal calls the browser's own `navigator.credentials` API: **no npm
  package and no crate is added.** `ring` is already on the allow-list under
  ADR 0110 for ECDSA P-256 verification; this record adds a use, not a
  dependency.
- No lib gains I/O; no lib depends on a service; no service depends on the
  runtime; nothing depends on an app.

## R12 ids

Unblocks to build: **R12-RISK-4111**, **4112**, **4114**, **4115** (a grant
becomes issuable, and therefore narrowable, in a deployed binary once this
lands and ADR 0075's Gates B and C follow), and **R12-GOV-6125** (an
authority level bound to a signed approval). It supplies the credential ADR
0114 named as missing for a human-in-the-loop action approval; ADR 0114 D8's
refusal of external action is unchanged.

## The paper-trading boundary

Untouched, and checked rather than assumed against the rules file: Terraform
refuses `supervised_live`, `limited_autonomous_live` and `autonomous_live` at
plan time; `AutonomyLevel::deployable` refuses them at start-up in `qip-api`,
`qip-fastbrain` and `qip-deepbrain`; `Cell` has no constructor taking a
non-paper ceiling and `Determinism::Required` cannot name a model rung; ADR
0106's ledger refuses `simulated=false`. This record identifies people; it
creates, enables and eases no order path.

## Gates

None run. Markdown only, and no shell in the session that wrote it. The
documentation acceptance suite's conditions hold by construction and were
not executed.
