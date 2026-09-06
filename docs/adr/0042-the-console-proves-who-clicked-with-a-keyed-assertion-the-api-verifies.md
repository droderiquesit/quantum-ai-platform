# 0042 — The console proves who clicked with a keyed assertion the API verifies

**Status:** proposed. Nothing in this record is applied. No code, no manifest,
no environment and no secret slot was changed by it; the mechanism below does
not exist in the tree.

**Relates to:** [ADR 0041](0041-venue-registration-is-one-operators-attributed-click-and-never-anonymous.md)
(whose amendment of 2026-09-05 established the gap this record answers, and
sketched a first version of the design), [ADR 0019](0019-identity-platform-is-the-only-identity-store.md)
(the sealed session the asserted subject comes from), [ADR 0013](0013-identity-verification-earns-a-dependency.md)
(verifying an identity *token* earns a dependency — this record is the argument
that this is not that), [ADR 0002](0002-two-dependencies.md) and
[ADR 0009](0009-tiered-dependency-policy.md) (no crate and no npm package is
added, and no primitive is written), [ADR 0018](0018-the-console-reaches-the-platform-over-the-vpc.md)
(the hop between the two processes), [ADR 0024](0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md)
(secrets reach a process as files), [ADR 0007](0007-exact-attribution.md)
(attribution reconciles exactly), [ADR 0003](0003-paper-trading-by-default.md)
and [ADR 0021](0021-the-blueprint-expects-live-capital-and-this-platform-refuses-it.md)
(the boundary this record does not touch).

**Does not amend and cannot amend:** the paper-trading boundary's three
layers; the rule that authority comes from the bearer credential and from
nowhere else; ADR 0040's placement of the reading of a vendor's terms with the
owner. This record makes an attribution verifiable. It grants nobody anything.

## Context

ADR 0041's amendment established, by quotation from the tree rather than by
suspicion, that a venue registration approved through the console is journaled
under `operator@env`: the subject of the deployment's operator bearer token,
identical for every human who signs in.

The three facts that produce it are each individually reasonable.

- `frontend/portal/src/lib/server/upstream.ts:64-68` — `upstreamHeaders` sets
  `authorization: Bearer <QIP_API_TOKEN>` and nothing else. The gateway
  verifies the sealed cookie and the CSRF pair before forwarding
  (`frontend/portal/src/app/api/gateway/[...path]/route.ts`,
  `refuseUnauthenticated`), so the browser is authenticated *to the console*,
  and then forwards none of that authentication onward.
- `frontend/portal/src/lib/server/identity.ts:75` says so in the console's own
  words: "the platform authenticates the console by its own `viewer` token, and
  nothing in this claim set reaches `qip-api`."
- `backend/crates/apps/qip-api/src/main.rs:269` builds every credential with
  the subject `format!("{}@env", role.as_str())`.

The route and the kernel are not at fault. `routes.rs:1117` builds the
`OperatorIdentity` from `principal.subject` and `principal.issued_at` and
refuses to read a name from a body; `Platform::approve_registration` takes the
operator from that identity alone. They record faithfully the only identity
they are offered. The gap is that a better one is never offered.

The consequence is narrow and it is the one that matters: **two facts the
platform claims to hold — that a decision is reproducible from the event log
alone, and that an approval is one named person's attested act — are not both
true of the same record.** The human's name exists only in the console's
sign-in and request logs, which are not hash-chained, are not correlated to the
approval, and have no retention this repository governs. Reconstructing who
clicked means joining two systems, one of which is not evidence.

What makes this hard is the thing that makes it worth writing down: the API has
no user directory, no session concept, no cookie parser, no public-key
verification and no outbound network path. Every honest route to closing the
gap therefore ends in the same question — *on whose word does the API believe a
name?* — and the wrong answers to that question are worse than the gap.

## Decision

Ten decisions. The first four are the mechanism; the rest are the parts the
sketch in ADR 0041's amendment left open, and two of them reverse it.

### 1. One assertion key per environment, mounted as a file to both processes

A single Secret Manager slot per environment, projected as a file:
`QIP_OPERATOR_ASSERTION_KEY_FILE` to the console, read through the existing
`_FILE` indirection in `frontend/portal/src/lib/server/secret.ts`
(`secretFromEnvironment`), and the same variable to `qip-api`, read through
`qip_core::secret::from_environment` in `main.rs` and nowhere else — the
composition-root rule, unchanged. Never an environment value on either side
(ADR 0024).

**One slot per environment, never one shared across environments.** A key
shared between dev and stage means an assertion minted by dev's console is
valid at stage's API, and the whole point of binding an assertion to a request
is defeated by the one binding nobody thought to write down.

### 2. The gateway mints a per-request assertion, and what it signs is stated exactly

For the routes named in decision 6 only, the gateway computes

```
mac = HMAC-SHA256(key, canonical)
```

where `canonical` is a **domain-separated, length-prefixed** byte string over,
in this order:

| field | source | why it is signed |
|---|---|---|
| `"qip-operator-assertion-v1"` | literal | domain separation: this key can never produce a MAC another protocol using the same key would accept, and a version bump is how the tuple changes |
| `nonce` | 16 random bytes | replay — see decision 3 |
| `subject` | the sealed session's `userId` | the fact being asserted |
| `authenticated_at` | the sealed session's `authenticatedAt` | so the kernel's freshness gate measures the person's sign-in |
| `issued_at` | mint instant | bounds the assertion's own life |
| `method`, `path` | the outgoing request | so an assertion for one route cannot be presented at another |
| `sha256(body)` | the forwarded body | so an assertion for one approval cannot be moved onto a different one |

The MAC and the signed fields travel in a request header **beside, never
instead of, the bearer token**. Node's `createHmac` is already imported by
`src/lib/server/identity.ts`; no npm package is added.

**Length-prefixed rather than delimiter-joined, and this is not fussiness.** If
the fields were joined by a separator and any field could contain it, a caller
who controls a subject could shift the field boundaries and make one canonical
string mean two different tuples. Subjects today are `gip:<localId>` or a
base64url identifier and contain no newline — but "in practice it does not
occur" is the reasoning that produces the bug the first time the identity
provider changes its identifier format. Each field is emitted as its byte
length followed by its bytes, and the mint additionally refuses a subject
containing a control character rather than encoding one.

### 3. A nonce, and a bounded window in which it may not be seen twice

The body hash and the issue instant bind an assertion to *a* request. They do
not stop that identical request being sent twice. An approval replayed inside
the freshness window journals a second attested act for one click, which is a
false record in the direction this whole exercise exists to avoid — and the hop
between console and API is plaintext HTTP/1.1 by design (the data-and-streaming
rule; the egress proxy fronts outbound calls, not this one).

So the API holds a seen-nonce set, and the set is **bounded by construction**,
in the shape `Authenticator` and `RateLimiter` already use: a fixed window and
an explicit capacity, cleared when the window rolls. `Authenticator`'s own
documentation states at length why per-attempt state keyed on attacker-chosen
bytes is the defect to avoid; a nonce is attacker-chosen bytes, and an
unbounded nonce set would be exactly that defect wearing a replay control's
clothes.

**A full set refuses.** That is safe here specifically because of decision 5's
ordering: the nonce is only consulted after the bearer token has matched and
the role check has passed, so only a holder of the operator token can fill the
set. A full set means the console is flooding, which is a bug in the console,
and refusing costs an operator a retry after the window rolls. It cannot become
an anonymous lockout, and it cannot touch the kill switch, because assertions
are not carried on that route.

### 4. The API verifies before it believes, and the assertion changes attribution only — never authority

`qip-api` recomputes the MAC with `qip_core::hash::hmac_sha256` and compares
with `qip_core::hash::constant_time_eq` — both already in the tree
(`backend/crates/libs/qip-core/src/hash.rs:163` and `:219`), no crate added,
no primitive written. It refuses, naming which of these failed:

- a MAC that does not match, compared in constant time against every configured
  key with no early return, mirroring `Authenticator::authenticate`'s existing
  discipline;
- a method, path or body hash that is not this request's;
- an `issued_at` outside a sixty-second window, or in the future;
- an `authenticated_at` in the future, or **after** its own `issued_at` — the
  assertion must never be able to make freshness looser than the kernel's rule,
  and an `authenticated_at` stamped at mint time would defeat the fifteen-minute
  gate in exactly the way `routes.rs:1109-1116` records having already happened
  once with `now`;
- a nonce already seen in this window;
- a subject that is empty, contains a control character, or ends in `@env` —
  the namespace the deployment credentials occupy.

Only then is the `OperatorIdentity` built, from `console:<subject>` and the
assertion's `authenticated_at`.

**The role still comes from the bearer credential.** An assertion adds no
authority, names no role, and can never raise a viewer token to operator. If
the mechanism ever grows a field that influences what a caller may do, this
record is void; see "What would make this wrong".

### 5. The order of checks is part of the design

Bearer authentication, then rate limit, then role, then assertion. An
unauthenticated caller must not be able to reach the MAC computation or the
nonce set at all. This is the same ordering `routes.rs:855-871` already
performs, with the assertion appended rather than inserted.

### 6. The routes that carry an assertion are a reviewed source-file literal

Not "any route". A mechanism that attaches to whatever route exists is one
that silently gains a new identity door the next time a route is added. The
initial set is the two operator-attributed writes ADR 0041's amendment names:

- `POST /registrations/{source_id}/approve`
- `POST /ledger/users/:user/eligibility`

Adding a third is an edit to that literal, reviewed like the
`RegistrationRegistry::shipped` table it is modelled on, and deliberately not a
configuration value — a requirement editable at runtime is one an incident can
lower.

### 7. The requirement is derived from the key, not configured — this reverses the sketch

ADR 0041's amendment proposed that "with no assertion present the route behaves
exactly as it does today". Half of that is right and half of it is a hole.

The right half: a deployment that has no key must keep working, and a `curl`
holding the operator token is a legitimate caller honestly attributed as a
deployment. Failing closed on a deployment that has no key would delete a
working control to install an unbuilt one.

The hole: if the API accepts an unasserted approval *while holding a key*, then
anything that can strip a header downgrades a named attribution to an anonymous
one, and no operator reading `operator@env` afterwards can tell a `curl` from a
console request whose header went missing. The fact is lost silently, which is
this record's own failure mode.

So the requirement is not a switch. **It is derived:**

| the API resolved | assertion present | outcome |
|---|---|---|
| no key | no | accepted, recorded as `operator@env`, mode `bearer` |
| no key | yes | **refused** — a header this process cannot verify is never believed, and never ignored |
| a key | no | **refused** on the decision-6 routes |
| a key | yes, valid | accepted as `console:<subject>`, mode `asserted` |

There is no state in which the API holds a key and still accepts an
unattributed approval, and no state in which an unverifiable header is quietly
dropped. A deployment moves from one column to the other by mounting one
secret, which is a single reviewed act with a plan a person reads.

Note the second row specifically. Ignoring an unverifiable header is the more
common choice and it is wrong here: it trains the console's authors to believe
attribution is working when the API is discarding it.

### 8. The mode is journaled, not inferred

The record carries how the subject was established — `asserted` or `bearer` —
as a fact beside the subject, rather than leaving a reader to infer it from the
subject's shape. Two independent claims about the same fact will disagree and
the louder one will be wrong; a prefix convention is a claim the log makes
about itself, and a mode field is the fact.

### 9. Rotation accepts two keys, and mints with one

The API resolves an ordered list of keys and verifies against every one of
them, in constant time, without an early return. The console mints with the
newest. Rotation is: mount both, restart the API, restart the console, drop the
old. A design accepting exactly one key makes every rotation a window in which
approvals refuse, and a rotation that causes an outage is a rotation nobody
performs.

### 10. Verification is a pure function

The check is a function of `(header, keys, method, path, body, now)` returning
`Result<AssertedSubject>`, living in `backend/crates/apps/qip-api/src/auth.rs`,
testable without a socket, a clock or a network. This is the reason
`qip_core::secret::resolve_from` exists in the shape it does, given in that
module's own documentation, and the reason is the same: the decision must be
exercisable apart from the part that cannot be.

## Where this sits in the layering

The dependency-direction argument, since it is what makes this shape
admissible at all.

The change touches two crates and no others: `backend/crates/apps/qip-api`
(a composition root and its own `auth.rs`) and the portal. `qip-api` is an
**app**; it depends on `qip_core::hash` and `qip_core::secret`, which are
**libs**, and on `qip-risk-engine`'s `OperatorIdentity`, a **service** type it
already constructs. Every edge points inward: app → runtime → service → lib.

- **No lib gains a dependency on a service.** `qip-core::hash` learns nothing;
  it is called, not changed.
- **No service gains a dependency on the runtime.** `qip-risk-engine` is
  unchanged; `OperatorIdentity::verified` is called with a different string.
- **No crate depends on an app.** The assertion type is `qip-api`-private.
- **The kernel does not learn what an assertion is.**
  `Platform::approve_registration` and `Platform::decide_eligibility` keep
  taking an `OperatorIdentity` and keep having exactly one way to obtain a
  subject. This is the load-bearing property: the kernel already refuses to
  read an operator name from a request body, and **this design must not become
  a second door into that field.** It does not, because the only thing that
  reaches the kernel is an `OperatorIdentity` the API constructed after
  verifying a MAC — the same type, from the same constructor, on the same
  path.
- **No environment is read outside `main.rs`.** `auth.rs` receives key material
  as a constructed value.
- **No new dependency, either side.** Rust: `hmac_sha256` and
  `constant_time_eq` exist. Node: `createHmac` is a built-in already imported.
- **No new primitive.** SHA-256, HMAC-SHA256 and a constant-time compare are
  composed. If this design had needed Ed25519 or RSA it would have been
  rejected rather than written — see the alternatives.

## The alternatives rejected, and why

**Forwarding the subject as an unauthenticated header.** The gateway adds an
operator header from the sealed claims and the API believes it. Rejected: the
API would record a name it cannot verify, and anything reaching the API with
the operator token could set it to any string. That is strictly worse than the
honest gap, which at least does not lie about who acted, and it is the same
refusal ADR 0041 already applies to taking the name from the request body.
This is the alternative the whole record exists to refuse, and the difference
between it and decision 4 is one MAC.

**Asymmetric signatures — the console signs, the API verifies a public key.**
Rejected on two counts. It needs Ed25519 or RSA verification the workspace does
not have and may not hand-roll (ADR 0009, ADR 0002), which alone settles it.
But it is worth saying that the property it would buy is not one that protects
anything here: with a shared HMAC the API can also mint assertions, so the
attribution is only as good as the API's own integrity — and the API is the
process that writes the log. An API that wanted to write a false attribution
does not need to forge an assertion first. The asymmetry would defend against a
third holder of the secret, and the slot's readers are exactly the two
workloads.

**Verifying the console's sealed session cookie at the API.** Rejected: it
needs the console's `configuredSessionSecret()` at the API, which would let the
API mint console sessions — a strictly larger trust grant than the one being
avoided — plus a cookie parser, a claim schema both sides agree on, and a
session concept `qip-api` does not have.

**Verifying an Identity Platform ID token at the API.** Rejected: RSA
verification against Google's rotating JWKS, over an outbound HTTPS path no
deployed process has (ADR 0024, nothing applied), needing public-key crypto the
workspace may not write. ADR 0013 says verifying an identity token earns a
dependency; that dependency is admissible in the browser tier and is not
admissible in the Rust core, so the conclusion is not "add the crate" but "do
not put this verification in the API".

**Leave the gap; correlate the console's own audit log by request id.**
Genuinely cheaper, and honest as far as it goes. Rejected because the platform
claims a decision is reproducible from the event log *alone*, and an
attribution that requires joining a hash-chained log to a request log that is
not hash-chained, not retained under any rule this repository states, and not
replayable is not that. If the design below is ever abandoned, this is the
fallback — and abandoning it means amending the reproducibility claim in the
same commit, not quietly holding both.

**One bearer token per human, in the API's existing credential table.** The
strongest alternative, and it deserves its reasons. `Credential::from_token`
already takes a subject; issuing `QIP_TOKEN_OPERATOR_<person>` would put a real
name in the log with no new mechanism at all. Rejected **for the console**: the
console holds one deployment secret, and per-human tokens would need a
per-user secret store, a provisioning path and a place to keep every user's
credential inside the console process — a larger new mechanism than the HMAC,
and a credential-per-user surface exactly where the secrets rule says not to
put one. It is, however, the right answer **for a human using `curl`**, and
nothing in this record prevents a deployment from issuing per-person operator
tokens today: that is configuration, not code, and it is named in "What is
still the owner's".

## What it costs

- **A new secret, and one more thing that must be mounted in two places to be
  right.** A key mounted to the API and not the console makes every console
  approval refuse (decision 7, row three) until the console is given it. That
  refusal is loud and correct, and it is still an operator's afternoon if the
  rollout order is not the one in decision 9.
- **Mounting the key retires the `curl` path for the decision-6 routes.** By
  design — that is what closes the downgrade — but it means a runbook step
  that works on a keyless deployment stops working on a keyed one, and the
  runbook has to say which deployment it is describing.
- **Two clocks with no stated synchronisation.** The sixty-second window is
  between Node's `Date.now()` and the API's `Timestamp`. Skew beyond a minute
  reads to an operator as "your approval was refused as stale", so the refusal
  message must say that the assertion's issue instant was outside the window
  and that the clocks are what to check. Widening the window to make a skew
  problem go away is the wrong repair and would weaken the replay bound.
- **The API trusts the console's authentication of the human.** This is the
  honest limit and it does not go away. What the API verifies is that *the
  console said this*, not that a person exists. It is the same trust the API
  already places in the console's holding of the operator token, and the
  mechanism narrows it rather than removing it: forging an attribution goes
  from "set a header" to "hold the assertion key". The subject is recorded as
  `console:<userId>` precisely so that no reader mistakes it for an identity
  the platform established itself.
- **A subject that is an opaque identifier, not a name.** `gip:<localId>` is
  stable and meaningless to a reader; turning it into a person requires the
  console's user records. That is a smaller join than the one this record
  replaces — an identifier the log holds, versus a correlation nobody kept —
  but it is still a join, and the log alone will not tell you whose desk it was.
- **More surface in the highest-consequence auth path.** `auth.rs` is the file
  every credential passes through. The verification is a pure function
  precisely so the added surface is exercisable, but it is added surface.
- **A bounded nonce set that can fill.** Named rather than hidden: under a
  console bug, approvals refuse for up to one window. The alternative —
  evicting to make room — is a replay control that silently stops controlling.

## What would make this wrong

- **An assertion that carries or influences a role, a capability, an autonomy
  level or a limit.** The moment a field in this tuple changes what a caller
  may *do* rather than what the log *says*, this is no longer an attribution
  mechanism, it is a second authentication scheme with a shared secret and no
  expiry, and this record is void.
- **The API ignoring an unverifiable assertion instead of refusing it.**
  Decision 7's second row. Dropping the header is the change that would make
  the console's authors believe attribution works while the API discards it,
  and it is the most likely well-intentioned regression here.
- **Accepting an unasserted approval on a deployment that holds a key.** The
  downgrade this record was written to close. If it reappears as a
  compatibility flag, the flag is the defect.
- **The nonce set becoming unbounded, or the window growing to hide skew.**
  Either turns a control that fires into one that reads as protection and does
  not — the shape the risk rules name by example.
- **A second constructor for `OperatorIdentity` on this path.** If an asserted
  subject ever reaches the kernel by any route other than the same
  `OperatorIdentity::verified` call the bearer path uses, then the second door
  into the operator field has been built after all, and ADR 0041's "a record
  without an operator" reversal condition applies to this record too.
- **The key reaching a committed file, an environment value, a log line, an
  error message or a response body.** It is a minting key: whoever holds it can
  attribute an act to any user id.
- **One key across environments.** Decision 1's second paragraph. It would make
  every binding in decision 2 defeatable by the one binding that was omitted.
- **Verification moving out of the pure function, or losing the forgery test.**
  A verification test that exercises only the valid path proves nothing about a
  signature. If the suite ever holds only the happy path, treat the mechanism
  as unverified.

## Applied by this record

**Nothing.** No key exists, no environment mounts one, `upstreamHeaders` still
attaches only the bearer token, `auth.rs` has no assertion type, and both
`Platform::approve_registration` and `Platform::decide_eligibility` still
journal `operator@env` for every console caller. This record is the design and
its reasons; the code is the next change.

The console's copy is already honest about this, by ADR 0041's amendment: the
approve control names no person, the dialog states what is recorded before the
click, and `qip-api/ROUTES-REGISTRATIONS.md` documents the `operator` field as
the subject of the credential the approval arrived on. Nothing in the product
currently claims the attribution this record would create.

### What would make it implementable

Named so that "not now" is a state with an exit rather than a mood:

1. **A shell that can run the gates.** The mandatory evidence for this change
   is a mutation in which a *forged* assertion is presented and refused. That
   test is the entire point of the mechanism, and a change to the API's
   authentication path landed without it would be the worst kind of
   half-landing: a new trust relationship whose refusal has never been
   observed. It was not run for this record and nothing was implemented.
2. **The owner's answer to two questions** in the section below.
3. **The rollout order agreed as a runbook step**, because decision 7 makes
   mount order observable to operators.

## What is still the owner's

1. **Whether to create the assertion key slot at all**, and in which
   environments. It is a new secret and a new rotation obligation, and the
   thing it buys is an audit fact rather than a capability. A desk that is
   content with "which credential approved" should say so, and this record's
   last alternative — per-person operator tokens, which are configuration and
   need no code — may be the cheaper answer for a small desk.
2. **Whether an approval should remain possible with `curl` once a key is
   mounted.** Decision 7 says no, deliberately. The opposite choice is
   defensible for a break-glass path and would need its own record, because it
   reopens the downgrade this one closes.

## The paper-trading boundary

Untouched, and confirmed rather than assumed. Terraform's refusal of
`supervised_live`, `limited_autonomous_live` and `autonomous_live` at plan
time; `AutonomyLevel::deployable` refusing the same three at start-up in
`qip-api`, `qip-fastbrain` and `qip-deepbrain`; `qip-edge`'s `Cell` having no
constructor taking a ceiling other than paper trading; and `qip-cost-router`'s
`Determinism::Required` arm returning a type that cannot name a model rung —
all four stand exactly as ADR 0003 and ADR 0021 left them.

Nothing in this record creates, enables or eases an order path. The routes it
touches record a registration approval and an eligibility decision; neither
moves capital, and an eligibility is a precondition of a funding rather than
one. An assertion confers no authority (decision 4), so it cannot reach the
autonomy control, which additionally requires a second approver. The mechanism
is a name in a log entry and nothing else.
