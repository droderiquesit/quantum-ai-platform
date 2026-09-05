# 0041 — Venue registration is one operator's attributed click, and never anonymous

**Status:** accepted, by owner instruction of 2026-09-05, given twice in the
same session and answered differently each time.

The first instruction asked for **"a scraper that registers itself with any
exchange or venue whose API needs an account, anonymously, with a browser
driver if that is what it takes"** — the form the repository's own status log
records it in (`docs/plan/PROJECT-PLAN.md`, the row for `ba8e767`). It was
refused, and the reasons are the Context below.

The second, after the refusal and its reasons had been put in front of the
owner, asked for the opposite arrangement: sign the company up under the
company's own identity, and only where a named operator has already approved
that venue's terms. The tree records it as the job's own opening sentence —
**"Performs a venue's signup form under the company's own identity, after a
named operator has approved that venue's terms"**
(`scripts/venue-signup/signup.mjs:4`). That was accepted, in the narrow form
decision 4 states and no wider.

Neither quotation is a transcript; the session is not a committed artefact.
Each is the form the repository itself records the instruction in, cited so
that a reader can check the citation rather than take this record's word for
what was said.

**Relates to:** ADR 0034 (the three candidate sources and the licensing gate
that decides them), ADR 0040 ("a record cannot read them on the owner's
behalf"), ADR 0003 and 0021 (paper trading; a data credential is never an
order credential), ADR 0002 and 0009 (no crate is added by this, and no npm
package either), ADR 0019 (the sealed session the operator's subject comes
from), `.claude/rules/domains/data-and-streaming.md` (licensing posture
before use).
**Does not amend and cannot amend:** the paper-trading boundary's three
layers, and ADR 0040's placement of the reading of a vendor's terms with the
owner. This record makes registration attributable; it does not make anyone
except the owner responsible for what a venue's terms say.

## Context

The platform needs data from venues, two of the three sources ADR 0034 named
need an account, and an account is a person's agreement with a venue. The
first request read that as an obstacle to automate: a service that opens
accounts by itself, anonymously, with a browser driver if the form resists.

That was refused for three reasons that are not matters of taste.

- **A venue binds a key to an identity that accepted its terms.** The signup
  form, the identity check and the terms box are how the venue knows who is
  bound by what. A licence nobody read is one nobody can be held to, and an
  account nobody owns is one nobody can be asked about. An anonymous account
  does not remove the obligation; it removes the name from it.
- **This repository puts licensing posture before use.** The data-and-
  streaming rules say the posture is evaluated in `qip-data-finder` *before*
  a source is used, and `qip-data-finder`'s admission gate is where that is
  enforced. A scraper that acquires access first and asks afterwards inverts
  the one ordering the domain rule exists to fix.
- **ADR 0040 left the reading of a vendor's terms with the owner**, in the
  same paragraph in which it authorised an agent to apply an environment.
  A component that accepts terms on the owner's behalf is exactly the
  artefact that sentence was written to refuse, and building it would have
  been an agent granting itself what a decision record had just reserved.

What could be built instead is the honest counterpart: not a system that
registers, but a system that **refuses to read a source until a named person
has registered, and carries that person's name wherever the source is used**.
The second instruction then asked for the mechanical part of the person's
work to be typed by a job — the form, under the company's own identity, after
that person's approval — which is a different thing from the first request
and is admissible on its own terms, because the agreement stays the person's
and only the typing moves.

### The alternatives that were rejected, and why

- **The anonymous self-registering scraper** (the first request). Rejected
  for the three reasons above. It is not refused by a flag defaulting to off;
  it is refused because no type in the tree can express an unattributed
  registration — see decision 2.
- **A platform-side signup service**: the same job, but inside `backend/` as
  a deployed capability behind a configuration switch. Rejected because it
  puts a browser driver in the workspace (a dependency decision under ADR
  0002 and 0009 that nothing here needs), because a deployed binary opening
  accounts is an unattributed actor on the venue's side of the terms however
  the record is written afterwards, and because a switch that enables signup
  is the switch that will one day be set by something that is not a person.
  The job stays dev tooling on the operator's machine.
- **Taking the operator's name from the approval request body.** Rejected:
  then a request, an agent finding or a model output could put any name on a
  registration, and the audit trail would record a claim rather than an
  identity. The kernel takes the subject from the authenticated
  `OperatorIdentity` and there is no overload taking a name
  (`backend/crates/runtime/qip-kernel/src/platform.rs:2795`).

## Decision

1. **A source that needs an account is refused until an operator-attributed
   record exists.** `RegistrationRequirement` is declared per source in
   `RegistrationRegistry::shipped`, a reviewed source-file literal, and an
   *undeclared* source is refused too — an unasked question is not a keyless
   source. Both admission paths ask the same registry the same question:
   `admission::admit_registered` and `admit_from_registered` for the
   connector feed, `DataFinder::assess` for a discovered source. Every
   refusal carries one sentence verbatim, so an operator and a test look for
   a delimited phrase rather than a paraphrase:

   > anonymous or automated registration is not a path this platform offers

2. **The record has one constructor, and deserialisation goes through it.**
   `RegistrationRecord`'s fields are private, `RegistrationRecord::new` is
   the only way to build one, and it refuses a blank operator, a blank terms
   citation and a blank source. The `Deserialize` impl is
   `#[serde(try_from = "RegistrationRecordWire")]`, whose `TryFrom` calls the
   same constructor, so a config file with no operator is refused at load
   rather than discovered later when the refusal has nobody to name. The
   credential is a `SecretRef` — the ingestion manifest's own shape screen —
   so the *value* cannot be passed at all: a pasted key is refused, and the
   refusal does not echo it.

3. **The operator's click is the attestation that the terms were read, and it
   is journaled with the authenticated subject.** `POST
   /api/v1/registrations/{source}/approve` requires the `operator` role,
   builds an `OperatorIdentity` from the sealed session exactly as `DELETE
   /kill-switch` does, and raises `Platform::approve_registration`.
   *(The clause "from the sealed session" is wrong as deployed, and the
   heading's "the authenticated subject" is narrower than it reads: the
   subject is the API bearer credential's, which for a console approval is
   one deployment identity shared by every session. See the amendment of
   2026-09-05 below.)* That
   refuses a credential older than fifteen minutes — the same freshness an
   eligibility decision requires, because a session token from this morning
   is not evidence that the person named on the record is at the keyboard
   now — writes the record to the hash-chained event log *before* the
   registry adopts it, and checks the registry would accept it on a scratch
   copy first, so the log never carries a record that did not stand.
   `Platform::replay_registrations` rebuilds the registry from the log alone,
   which is what makes "who registered this source" a fact a person can
   re-derive rather than one the process asserts about itself.

4. **The signup job automates the mechanical form and nothing else.**
   `scripts/venue-signup/signup.mjs` fills a reviewed recipe's fields from a
   company identity file holding exactly five values, under an approval no
   older than twenty-four hours whose `source_id` matches the recipe and
   whose `terms` equal the reference the recipe's consent box cites — a
   mismatch means the box is not ticked and the run stops. It **stops without
   retry**, exit 70, at each of: a captcha or bot challenge; an identity
   document, selfie, date-of-birth or tax-identifier field; a verification or
   second-factor code; a consent box the approval does not cover; and any
   field the recipe does not list. None of these is solved, worked around or
   retried. Each hand-back names the reason and leaves a screenshot in a
   scratch directory outside the repository, which is treated as identity
   data. A recipe declaring `identity_verification_required: true` — Kalshi's
   does — refuses before a browser is opened.

5. **Credentials reach Secret Manager and nowhere else.** The password is
   generated in memory from the CSPRNG and goes to `gcloud secrets versions
   add <slot> --data-file=-` on stdin, as does any API key the venue displays;
   neither reaches a file, an argument, a shell history, stdout, the job's
   one-line JSON outcome, or a response body. The platform's own surfaces
   carry only the *name* of a deployment variable in either direction: the
   list route, the approval body, the record, the banner. The value is read
   at runtime through `qip_core::secret`'s `_FILE` indirection and is
   projected as a file, never as an environment value (ADR 0024).

6. **Anonymous or automated account creation is not a path, and no flag
   enables it.** There is no configuration, no test-only shortcut and no
   environment in which the platform opens an account. The refusal is
   structural: the type cannot express an unattributed record, and the job
   hands back rather than proceeding at every point where a person's presence
   is what the venue is asking for. A change that added such a flag would be
   reopening this record, not configuring it.

### Where this sits in the layering

The shape adds no edge that points outward. `RegistrationRequirement`,
`RegistrationRecord`, `RegistrationStanding` and `RegistrationRegistry` live
in `qip-data-finder`, a service, whose manifest depends on `qip-core`,
`qip-contracts`, `qip-financial` and `qip-events` (libs) and on `qip-mesh`
and `qip-market-ingestion` (peer services), and on no runtime crate and no
app. `SecretRef` is imported from `qip-market-ingestion`'s manifest module —
an edge the crate already had — so the screen that decides what a credential
name looks like is one implementation shared by the manifest and the record,
rather than two that can drift apart.

Composition happens where it must. The kernel is the only place the record
meets the event log and the operator identity; `qip-api` is the only place a
session becomes an `OperatorIdentity` and the only place configuration is
read. No service reads the environment, and the record type performs no I/O:
it is a value the runtime journals. The signup job is outside the workspace
entirely — nothing under `backend/` or `frontend/` references
`scripts/venue-signup/`, so no crate and no npm package depends on a browser
driver, and the platform does not depend on the job existing.

## What it costs

- **A person still creates the API key.** Where a venue does not show a key
  at signup — Alpaca does not; it issues keys from the dashboard afterwards —
  step 3 of `docs/operations/registering-a-venue.md` is the operator's: create
  the key with the narrowest scope the venue offers, read-only market data,
  never a trading scope. The job cannot reach it and does not try.
- **A venue whose terms forbid automated signup is registered by hand.** The
  job is a convenience that some venues will not permit and others will
  defeat with a challenge. When that happens the answer is the runbook, not a
  better recipe.
- **The job cannot finish a run that reaches an e-mail verification**, which
  for Alpaca is every run: it reads no mail and polls nothing. The password
  is written to its slot first, because the venue may have created the
  account and losing the password would leave one nobody can enter, and the
  code is the operator's.
- **An approval does not re-open a connector.** The feed's admission gate
  runs once, at start, so a source approved at runtime stands in the platform
  and in the log and reaches the feed at the next start. That is a restart's
  delay between a person registering and the data arriving.
- **The requirement table is code.** Raising Alpaca from `account` to
  `account_with_identity_verification` when the terms say so is a one-line
  change, reviewed and deployed like any other. That is deliberate — a
  requirement editable at runtime is one an incident can lower — and it is
  still a deployment on the critical path of a compliance fact.
- **The record proves attribution, not comprehension.** `terms_read_at` is
  the instant the operator says they read the terms. Nothing here can tell a
  careful reading from a click, and this record does not pretend otherwise;
  what it buys is a name, a date and a citation that can be re-read when the
  terms change.

## What would make this wrong

- **A recipe that solves a challenge.** A captcha exists to establish that a
  person is present; a script that defeats one is a statement that none was,
  and it would make every registration record downstream of it a false
  attribution. If a recipe ever carries a solver, a token for a solving
  service, or a retry across a hand-back, this record is void and the job
  comes out of the tree.
- **A record without an operator.** If any path — a new constructor, a
  `Deserialize` that bypasses `try_from`, a config loader, a test helper
  promoted to production, a body field that names somebody — produces a
  registration with no authenticated subject behind it, then the platform is
  reading venue data on an agreement nobody made, which is the thing the
  first request asked for by another route.
- **A key value in any committed file.** A credential in a recipe, a
  manifest, a tfvars file, a fixture, a screenshot committed for
  convenience, or an error message that echoes what it refused. The screens
  exist because this is the failure that cannot be undone by a later commit.
- **Attribution drifting from the log.** If `Platform::registrations` and
  `Platform::replay_registrations` can disagree, the registry has become a
  second source of truth for a fact the event log already holds, and the
  replay is the half to believe.

## What is still the owner's

Named here rather than left to be discovered, because each blocks something
and none is an agent's to close:

1. **The Alpaca and Kalshi terms have not been read.** Both sources are
   refused today by the licensing gate for that reason, and that refusal
   comes *before* the registration question. The shortest sufficient form is
   ADR 0040's: one sentence in the session naming the document read and the
   date, from which the catalogue entry and the record's `terms` citation are
   written.
2. **Their true requirement level.** `RegistrationRegistry::shipped` declares
   both `account`, chosen as the restrictive default — Kalshi's manifest
   reads an unauthenticated endpoint and is still declared `account`, because
   whether an anonymous reader is permitted is exactly what the unread terms
   would say. If the terms demand identity verification, the requirement is
   raised to `account_with_identity_verification` and, for Kalshi, the recipe
   already refuses before a browser opens.
3. **Whether to mount the registrations file.** No composition root reads one
   today (`backend/crates/apps/qip-api/src/main.rs:99-104`), so the registry
   the feed is admitted against is the shipped table with no record, and the
   only way a registration exists is the approval route and the log. Mounting
   a file would let a registration be present at start; it would also make a
   committed file an input to a compliance fact, which is why it is a
   decision and not a default.
4. **When the first watched run happens.** Neither recipe has been run
   against a real venue; the selectors were written from the public forms and
   not exercised against them. The first run should be watched, knowing that
   a field the page has and the recipe does not is a hand-back rather than a
   guess.

## Applied by this record

This record is written after the code it describes and records the decisions
that code already embodies, so that they stop living in a runbook and a
module comment. What holds each decision, for a reader checking the claim
rather than taking it:

- `backend/crates/services/qip-data-finder/src/registration.rs` — the
  requirement, the record with its single constructor and `try_from` wire
  shape, the registry, the `NOT_OFFERED` sentence.
- `backend/crates/services/qip-data-finder/src/admission.rs` — the two
  admission gates that ask the registry before a source is read.
- `backend/crates/runtime/qip-kernel/src/platform.rs` — the credential
  freshness, the journal-before-adopt discipline, and the replay.
- `backend/crates/apps/qip-api/src/routes.rs`,
  `src/registration_views.rs` and `ROUTES-REGISTRATIONS.md` — the two routes
  and their contract in prose.
- `frontend/portal/src/app/(portal)/data-sources/registrations/page.tsx` —
  the operator's view and the one control that records their own act.
- `docs/operations/registering-a-venue.md` — the runbook, including what may
  be automated and what may not.
- `scripts/venue-signup/` — the job, its two reviewed recipes and its tests.

**Nothing else is applied by this record.** No environment mounts a
registrations file, no Secret Manager version holds a venue credential, no
registration record exists for any source, and the job has never been run
against a venue. Both `account` sources therefore stand refused today, which
is the correct state for a registration nobody has made.

The paper-trading boundary is untouched by all of it. Terraform's refusal of
the three live ceilings, `AutonomyLevel::deployable` at every composition
root, and the `qip-edge` `Cell` and `qip-cost-router` `Determinism` types
stand exactly as ADR 0003 and 0021 left them. Nothing in this record names a
venue as an execution target: the credential it governs is a read-only
market-data key, and ADR 0034's closing warning is repeated here because
Alpaca is the place it is most likely to be tested by accident — a data
credential must never become an order credential.

## Amendment of 2026-09-05: the click is attributed to a deployment, not a person

This record's title and its decision 3 claim more than the tree delivers. The
claim was checked against the code and it does not hold: **a venue
registration made through the console is journaled under the console's own
deployment credential, identical for every human who signs in, and the event
log therefore names which deployment acted and not which person clicked.**
Naming the gap is the amendment; nothing about the refusal of anonymous
registration changes, and no authentication is weakened to make this true.

### What was found, and where

- **The console forwards one deployment credential for every session.**
  `frontend/portal/src/app/api/gateway/[...path]/route.ts` calls
  `upstreamHeaders(target, …)`, and
  `frontend/portal/src/lib/server/upstream.ts:64-68` is the whole of it:
  `if (target.token) headers.set("authorization", `Bearer ${target.token}`)`,
  where the token is `QIP_API_TOKEN` read once per request out of the
  deployment's mounted secret. The gateway verifies the session cookie's
  signature and the CSRF pair before forwarding — the browser is
  authenticated *to the console* — and then forwards none of it.
- **The console's own source already said so.**
  `frontend/portal/src/lib/server/identity.ts:75`, on the sealed claim set:
  "the platform authenticates the console by its own `viewer` token, and
  nothing in this claim set reaches `qip-api`."
- **The API's subjects are per role, not per person.**
  `backend/crates/apps/qip-api/src/main.rs:268` builds every credential with
  the subject `format!("{}@env", role.as_str())`. So `principal.subject` on
  the approve route resolves to `operator@env` — for every caller holding the
  operator token, console or `curl`.
- **The kernel and the route are not at fault.** `routes.rs` builds the
  `OperatorIdentity` from `principal.subject` and `principal.issued_at` and
  refuses to read a name from the body; `Platform::approve_registration`
  takes the operator from that identity alone. They faithfully record the
  only identity they are given. The gap is that no better one is offered.
- **The console said otherwise on the button.** The confirm step read
  "Approve registration as <name>" and "Confirm as <name>", where the name
  came from `useSessionIdentity` — a value the browser holds, never sent, and
  never recorded. The record it produced said `operator@env`, which the
  card then displayed as "registered by operator@env".

### What is recorded, and what is not

**Recorded, and replayable from the log alone:** that an approval happened,
which source it names, the terms citation and the instant, the deployment
subject the credential carries (`operator@env`), and that the credential was
no older than fifteen minutes when it was used.

**Not recorded anywhere the platform can replay:** which human clicked. That
fact exists only in the console's own sign-in records and its request logs,
which are not the hash-chained event log and are not correlated to the
approval. A second person holding a console session with the operator role
produces a byte-identical attribution.

**Consequently:** the `operator` field of a `RegistrationStanding` must be
read as *which credential approved*, not as *who approved*. ADR 0041's
"attributed click" is true of the platform's boundary — no anonymous or
automated path exists, and a person must click — and false of the person: the
attribution stops at the deployment.

### Why the gap was not closed instead

Carrying the end-user subject was the preferred outcome and it is not
reachable today without weakening something.

- **Forwarding the subject as a header** — the gateway adding, say, an
  operator header from the sealed claims — would have the API record a name
  it cannot verify. Anything that can reach the API with the operator token
  could then set that header to any string, and the log would hold a claim
  wearing an identity's clothes. That is strictly worse than the present gap,
  which at least does not lie about who acted. It is refused for the same
  reason this record's own "alternatives rejected" refuses taking the name
  from the body.
- **Verifying the console's sealed cookie at the API** needs a key both
  processes hold and a claim schema both agree on. Neither exists: the seal
  is HMAC'd with `configuredSessionSecret()`, a console-only secret, and
  `qip-api` has no cookie parsing, no session concept and no route that
  accepts one.
- **Verifying an Identity Platform ID token at the API** needs RSA signature
  verification against Google's rotating JWKS — an outbound HTTPS path no
  deployed process has (ADR 0024, nothing applied) and public-key crypto the
  workspace does not have and may not hand-roll (ADR 0009, two dependencies).

### The design that would close it

Stated concretely enough to be implemented, and needing its own ADR because
it creates a trust relationship between two processes that have none.

1. **A shared assertion key.** One Secret Manager slot, projected as a file
   to both the console (`QIP_OPERATOR_ASSERTION_KEY_FILE`, read through the
   existing `_FILE` indirection in `src/lib/server/secret.ts`) and to
   `qip-api` (through `qip_core::secret::from_environment`, in `main.rs`
   only). Rotation is one secret rolled in one place, as the mesh envelope
   key already is.
2. **The gateway mints a per-request assertion.** For the approve route only,
   the gateway HMACs a compact, canonical byte string over `{subject, issued
   at, authenticated at, method, path, body hash}` where `subject` is the
   sealed session's `userId` and `authenticated at` is its `authenticatedAt`
   — and sends it in a request header beside, never instead of, the bearer
   token. Node's `createHmac` is already imported by
   `src/lib/server/identity.ts`; no npm package is added.
3. **The API verifies before it believes.** `qip-api` recomputes the MAC with
   `qip_core::hmac_sha256` — already in the tree, no crate added — compares
   in constant time, refuses a mismatch, a missing header, an assertion whose
   path or body hash is not this request's, or one issued outside a short
   window. Only then does it build the `OperatorIdentity` from the asserted
   subject, prefixed to keep the two kinds of identity distinguishable in the
   log (`console:<userId>`), with `authenticated_at` taken from the
   assertion's own claim so the kernel's fifteen-minute freshness gate
   measures the person's sign-in rather than the deployment token's issue.
   With no assertion present the route behaves exactly as it does today and
   records `operator@env` — a `curl` with the operator token is still a
   legitimate caller, and it is honestly attributed as one.
4. **What this buys and what it does not.** It makes the console's claim
   about the clicker verifiable by the API, and it makes forging one require
   the assertion key rather than the ability to set a header. It does not
   make the console's session store trustworthy to a third party: the API is
   trusting the console's authentication of the human, which is the same
   trust it already places in the console's holding of the operator token.
   Saying that out loud is part of the ADR this design needs.

### Applied by this amendment

- `frontend/portal/src/app/(portal)/data-sources/registrations/page.tsx` —
  the button is "Approve registration" and the confirm control is "Confirm
  approval"; neither names a person. The dialog states what is recorded, in
  the `registration-attribution` paragraph, before the click. The page's
  declaration paragraph no longer says the control records that a *named*
  operator registered.
- `backend/crates/apps/qip-api/ROUTES-REGISTRATIONS.md` — the `operator`
  field is documented as the subject of the credential the approval arrived
  on, with the console's case named; the sentence claiming the identity is
  built "from the session" is corrected.
- `frontend/portal/tests/registrations-page.spec.ts` — the copy assertions
  follow, and one test asserts the attribution paragraph names the deployment
  subject and not the signed-in name.
- Decision 3 above carries an inline pointer to this amendment.

Nothing else changed. No route, no handler, no kernel path and no credential
was touched; `docs/operations/registering-a-venue.md` is being edited
elsewhere in this session and its attribution sentences are left for that
work, which is a known remaining inconsistency rather than a closed one. The
paper-trading boundary is untouched: Terraform's refusal of the three live
ceilings, `AutonomyLevel::deployable` at every composition root, and the
`qip-edge` `Cell` and `qip-cost-router` `Determinism` types stand exactly as
ADR 0003 and 0021 left them, and nothing here creates, enables or eases an
order path.
