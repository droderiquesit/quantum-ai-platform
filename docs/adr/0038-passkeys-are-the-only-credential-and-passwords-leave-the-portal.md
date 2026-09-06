# 0038 — Passkeys are the only end-user credential, and passwords leave the portal

**Status:** *proposed*, 2026-09-05, and **deliberately still proposed** after
the architecture sweep of the same day, which was asked to resolve every open
record or say exactly what would close it. This one cannot be closed from
inside the repository, and the reason is worth stating rather than leaving as
an omission: the shape turns entirely on the four checks below, every one of
which is a call against Google's Identity Platform on the dev project. No
reading of this tree answers any of them, and guessing at the answer would
pick between Shape A (no dependency anywhere) and Shape B (one npm dependency
and a two-passkey cap) on no evidence — which is the one thing a record about
a credential must not do. **What closes it:** the four checks, run by the
owner against `algorik-dev`, with their outputs quoted into this status line.
Nothing else does, and no amount of further design work substitutes.

**Re-examined 2026-09-06, and still proposed.** The session that did this pass
was asked to answer the four checks against Microsoft and Google public
documentation. **It could not: no documentation-search, web-fetch or web-search
tool was available to it.** Both attempts are quoted in "The four checks,
2026-09-06" below rather than summarised, because a record about a credential
must not be able to be read as though a check had been run. What the pass could
do is answer the parts of the four questions that are settled by the WebAuthn
specification or by this repository's own code rather than by a console, and it
found four consequences that change the shape of the fallback. **The record now
recommends Shape A**, on an argument that does not depend on any of the four
checks passing — and records that **Shape B as written is not viable**, because
the custom-claim store it names cannot hold what point 5 also puts there and
would be destroyed by the console's own profile write. That section is
additive: no decision below was rewritten, and the status stays *proposed*.

Re-verified on 2026-09-05 that nothing has moved:
`grep -n 'AuthMethod' frontend/packages/auth/src/index.ts` still returns
`export type AuthMethod = "password" | "google" | "passkey" | "saml" | "oidc" | "development";`
at `:24`, so the password is still a listed peer of the passkey, which is the
sentence this record exists to reverse.

Nothing below is applied: the portal
still signs in with an email and a password, and every passkey ceremony
named here is a design, not a route. The only code that moved with this
record is copy and doc comments — the sign-in page stops presenting the
password as the platform's standing credential, and the four password routes
carry a deprecation note. Both are named under "Nothing is applied".
**Relates to:** ADR 0002 and 0009 (no new Rust crate — the workspace is not
touched by this decision at all), ADR 0012 and 0013 (where a dependency is
earned, and the precedent that token verification earned one), ADR 0014
(the shared `@algorik/auth` package and the PWA as the mobile channel), ADR
0018 (the console reaches the platform as viewer on its own credential, which
this record does not change), ADR 0019 (Identity Platform is the only
identity store; the session is a sealed cookie; custom claims are capped at
1000 bytes), ADR 0021 and 0022 (the blueprint is the architecture of record
and §40.3 says "no passwords anywhere").
**Does not touch:** the paper-trading boundary's three layers; the platform's
own credential and roles; anything under `backend/`.

## Context

The blueprint's §40.3 row for sign-in reads, in full: "Passkeys via WebAuthn
platform authenticators. Hardware-backed, non-exportable. No passwords
anywhere. TOTP fallback for a device that cannot register." §40.14 adds that
sessions are "issued after passkey verification", and §51 puts "identity with
passkeys" in Phase 0. PHASE-B8 in `docs/plan/PROJECT-PLAN.md` is the one open,
unblocked, unclaimed item of size in the blueprint backlog, and the gap map's
row for it says why it is a gap and not merely an absence:
`frontend/packages/auth/src/index.ts:24` declares
`AuthMethod = "password" | "google" | "passkey" | "saml" | "oidc" | "development"`,
so a passkey is one method among several and the password is a listed peer —
the opposite of the sentence it is meant to implement.

What exists today, so the change is measured against the tree and not against
the wish:

- Identity Platform holds the account, the password, `emailVerified` and the
  custom claims (ADR 0019). The portal calls the v1 REST surface directly —
  `accounts:signInWithPassword`, `accounts:sendOobCode` for verification and
  reset — with `fetch` and no SDK (`identity-platform.ts`), and writes claims
  through the admin endpoint on the console's service-account identity, with
  exactly two permissions.
- The session is a sealed cookie whose claims include `method`, today one of
  `"development" | "password" | "google"` (`identity.ts:88`).
- The password journey is five pages and five routes: sign-in, sign-up,
  verify-email, forgot-password, reset-password. It is the only journey with
  a passing Playwright suite (`tests/auth.spec.ts`).
- The development provider — the offline slice that runs with no Google
  project — is a JSON file holding password hashes, and it is what every
  Playwright run signs in against.
- `grep -rln -i passkey backend/crates frontend/portal/src` is empty, as
  `docs/plan/wave-7-backlog.md` §7 records.

### Whether Identity Platform verifies a passkey itself

This is the question the shape turns on, and it is stated here as what is
known and what must be checked, because it could not be checked from this
session (no outbound web access was available to the agent that wrote this
record, and nothing in the repository records the answer).

*What is known.* The Identity Toolkit **v2** API surface has, for some time,
exposed `accounts/passkeyEnrollment:start`, `accounts/passkeyEnrollment:finalize`,
`accounts/passkeySignIn:start` and `accounts/passkeySignIn:finalize`, and the
Firebase Apple SDK carried client wrappers for them marked as preview. The
Identity Platform product documentation, to the knowledge this record was
written with, does **not** list passkeys among the sign-in providers a
project can enable, and the Terraform provider's
`google_identity_platform_config` `sign_in` block (which
`modules/identity/main.tf:53` uses) has `email`, `phone_number` and
`anonymous` — no passkey block.

*What must be checked, by the owner, before the first ceremony is written*
— each with the evidence that settles it:

1. Whether the four v2 passkey endpoints are documented as generally
   available for Identity Platform (not Firebase-only, not allowlisted
   preview), and whether they work on the project's tier. Evidence: a
   `passkeyEnrollment:start` call on the dev project answering with a
   `credentialCreationOptions` body rather than `PERMISSION_DENIED` or
   `UNSUPPORTED_PASSKEY`-class error.
2. Whether an account can exist with a passkey as its **only** provider —
   no password set, email/password provider disabled — and still sign in.
   Evidence: `enable_email_password = false` in `modules/identity`, one
   account, one successful `passkeySignIn:finalize` returning an ID token.
3. Whether the relying-party ID and allowed origins are configurable per
   project (the portal's host), and what happens to enrolled credentials if
   the host changes.
4. Whether the ID token a passkey sign-in returns carries the same custom
   claims the password sign-in returns today, so ADR 0019's agreements and
   roles survive the switch unchanged.

### The four checks, 2026-09-06 — what was tried, and what is answerable without a console

**What was tried, quoted rather than summarised.** The pass was asked to answer
the four checks against Microsoft and Google public documentation. No
documentation-search tool was reachable, under either name it is published as:

```
Error: No such tool available: microsoft_docs_search
Error: No such tool available: mcp__Microsoft_Learn__microsoft_docs_search
```

No web-fetch or web-search tool was available either. **So no check below is
answered with a citation to a vendor's documentation, and none should be read
as though it were.** Every claim carries its evidence class: **[tree]** read
from this repository at the line given; **[spec]** a property of the W3C Web
Authentication specification, stated from the author's knowledge and **not
fetched this session** — check it against the specification before relying on
it; **[open]** not answerable without a console or a credential.

**Check 1 — are the four v2 endpoints usable, at GA, on this project? [open].**
What a person must look at, exactly: the Identity Platform sign-in-providers
documentation, for whether passkeys are listed as a provider a project may
enable; the Identity Toolkit **v2** REST reference, for whether
`accounts/passkeyEnrollment:start|finalize` and
`accounts/passkeySignIn:start|finalize` are documented as generally available
rather than preview or allowlisted; and then one call against `algorik-dev`
whose answer is a `credentialCreationOptions` body rather than an error code.
The probe is small because the machinery exists: `call()` already extracts
Google's stable ALL_CAPS code from the error body
(`identity-platform.ts:52-57`), so a `PERMISSION_DENIED` or an
`UNSUPPORTED_PASSKEY`-class refusal arrives as a code and not as a stack trace.
**[tree]** The only repository evidence is weak and must not be mistaken for an
answer: the `sign_in` block this platform configures has `email`,
`phone_number` and `anonymous` and no passkey block
(`modules/identity/main.tf:44,53-68`). That says what this configuration
enables. It does not say what the API supports.

**Check 2 — can an account exist with a passkey as its only provider?
[open].** What a person must do: set `enable_email_password = false` against a
scratch project — **not `dev`**, whose Identity Platform configuration is
applied (`enable_identity_platform = true`,
`environments/dev/terraform.tfvars:299`) and whose account pool this record
cannot inspect, so disabling the only working sign-in method there would be a
change nobody has measured the blast radius of — then create one account,
complete `passkeySignIn:finalize`, and confirm an ID token comes back.
**[tree]** Note the coupling the module already has: `password_required = true`
lives inside the same `email` block that the `enabled` flag switches
(`main.tf:56-61`), so this check also settles whether the email block can be
disabled without disturbing the `authorized_domains` and quota configuration
that share the resource.

**Check 3 — the relying-party ID, and what a host change does. Half answered,
and the answered half is decisive.** **[spec]** A credential is created against
one RP ID; an authenticator will not produce an assertion for a different one,
and the RP ID must equal the origin's effective domain or be a registrable
suffix of it. **There is no migration path for a credential whose RP ID
changes** — every user re-enrols. **[tree]** The portal's origin today is
`algorik-portal-rgxpsss2lq-uk.a.run.app`
(`environments/dev/terraform.tfvars:300-304`), and an algorik.ai migration is
on the books and already referenced by the identity module
(`main.tf:14-18`). If `run.app` or `a.run.app` is on the Public Suffix List —
**check the list; do not assume it** — the RP ID cannot be shortened to a
shared parent, so every credential is bound to that exact service hostname,
which the migration changes. **The consequence holds under both shapes and is
the most actionable thing this pass produced: do not enrol a population before
the destination hostname is the origin, or plan to discard every credential
enrolled first.** Under Shape A there is a second half — what RP ID Identity
Platform uses on the project's behalf — and that half stays `[open]`.

**Check 4 — do the custom claims survive? [open]** for the token itself. The
platform-side half of the same question is answerable from the tree, and the
answer is worse than the question assumes; it is the next section.

### What the tree established instead, and why it decides the shape

**(a) Shape B's credential store would be destroyed by the console's own
profile write.** `gipWriteProfile` serialises `{ [PROFILE_CLAIM]: profile }`
and sets `customAttributes` to exactly that string
(`identity-platform.ts:237-247`); `accounts:update` replaces the blob rather
than merging it. **So a credential list stored under a second key inside the
same claims blob is deleted by the next profile write** — the next agreements
re-acceptance, the next role change. Shape B would therefore need
`gipWriteProfile` to become a read-merge-write against the account record,
which is the lost-update race ADR 0019 examined and rejected for Cloud Storage,
relocated onto Identity Platform and now sitting on the credential path instead
of the profile path. **[tree]**

**(b) The two-passkey cap is a byte budget whose largest term is chosen by
hardware, not by this platform.** `PROFILE_CLAIM_LIMIT = 1000`, and
`gipWriteProfile` refuses locally above it and returns false, which the caller
treats as a sign-up that did not complete (`:147-157`, `:233-248`). The shipped
profile — account type, three agreements, a version, one role — serialises to
roughly 150 bytes (hand-counted from `StoredProfileClaims`; an estimate, not a
measurement). **[spec]** A credential ID is chosen by the *authenticator*: a
key-wrapping roaming authenticator's is commonly 64 bytes, which is 88
characters once base64url-encoded, before the COSE public key beside it. So
decision 2's "roughly 220 bytes each" is an assumption about the hardware the
desk happens to buy, and the cap is not "two passkeys" — it is a budget whose
overflow is a whole write refused, taking the compliance record with it.

**(c) Decision 5's recovery code has nowhere to count its attempts.** Five
attempts in fifteen minutes needs a counter, and ADR 0019 removed the console's
store on purpose. The only place left is the same custom claim — a
read-modify-write per attempt across stateless instances, which is (a) again,
and a counter an attacker races is not a counter that stops one. **This is an
open consequence of decision 5 under *either* shape**, named here rather than
discovered at implementation; under Shape A it is smaller only because the
claim then holds the recovery HMAC and not also the credentials.

**(d) A Rust relying party is not merely undesirable — it is impossible
today.** ADR 0043 establishes that no Rust process in this workspace can
produce an unpredictable byte: `Xoshiro256` is deterministic by design and
there is no other source, and 0043 decision 5's `/dev/urandom`-in-a-composition-root
answer is specified and not implemented. A relying party mints challenges. The
console can (Node's `crypto.randomBytes`, which is why ADR 0042's 16-byte nonce
is safe as specified); `qip-api` cannot. The rejection already in decision 2
gains a second and harder reason.

**(e) ADR 0042 gains from this record and is not depended on by it.** The
console's assertion carries `method` and `authenticated_at`; a passkey makes
both mean more than a password does. It changes attribution only, never
authority, and nothing here moves its key, its slot or its verification.

### The recommendation: Shape A — and what to do if it cannot be had

**Shape A**, on an argument that does not depend on any of the four checks
passing:

1. **Shape B as written is refuted by (a) and (b).** Its credential store is a
   claims blob that the console's own profile write replaces, inside a byte
   budget whose largest term is a property of the authenticator. Shape A stores
   the credential where the account is and asks neither question.
2. **Trying Shape A first is cheap and trying Shape B first is not.** Shape A's
   first step is one HTTP call through code that already exists. Shape B's is a
   dependency, a verifier, a storage decision and a Playwright rewrite, most of
   which is discarded if check 1 passes.
3. **Shape A fails loudly.** An endpoint that is absent, preview-gated or
   tier-restricted answers with a code the portal already parses. It cannot
   half-pass silently, which is the property that makes it safe to attempt
   before it is decided.

**Against it, stated because it is the real objection:** Shape A makes the
console's *only* credential depend on a Google surface whose general
availability is exactly what check 1 is about, and a preview API underneath a
sign-in page is a worse dependency than an npm package. **If check 1 answers
"exists, preview only", that is a third answer this record has no shape for** —
the honest response is to stay on the deprecated password path and revisit,
not to fall through to Shape B by default.

**If a check genuinely fails, Shape B is admitted only with a named amendment
saying where a credential record lives.** Three candidates, and this record
ranks them rather than leaving the choice to whoever implements it:
(i) the profile claim, *merged* rather than replaced, with the byte budget
stated and an overflow that refuses the enrolment rather than the profile;
(ii) the platform's own hash-chained event log, which ADR 0019 already names as
the honest home for a fact Identity Platform cannot hold — at the cost of
making the console's sign-in depend on the platform, a dependency direction to
take deliberately and not by accident; (iii) one passkey plus the operator code,
which drops the second-authenticator recovery path. **The preference is (ii)
over (i), and (iii) is refused** — decision 5 requires both recovery paths, and
the one that does not need an operator is the one that works at 3 a.m.

**What still closes this record** is unchanged: the four checks, run by the
owner against `algorik-dev`, with their outputs quoted into the status line.
This pass narrowed what they have to decide; it did not answer one of them.

## Decision

1. **Passkeys (WebAuthn) become the only end-user credential of the portal.**
   An account has no password. `AuthMethod` in `@algorik/auth` loses
   `"password"`; the sealed cookie's `method` becomes
   `"passkey" | "development"` (Google federation stays a separate decision,
   and its button stays disabled with its caption until it is one). The
   sign-up page becomes an enrolment ceremony: email as the *identifier*,
   verified through the existing `sendOobCode` step because the mailbox is
   still how the desk reaches a person, and a passkey as the *credential*.

2. **Identity Platform issues and verifies the passkey where it can, and the
   portal is the relying party only where it cannot.** Two shapes, decided by
   the four checks above, in this order of preference:

   - **Shape A — native.** If checks 1–4 pass, the portal calls the v2
     passkey endpoints the way it calls v1 today: `fetch`, no SDK, no new
     dependency anywhere. The browser runs `navigator.credentials.create`
     and `.get` with the options Identity Platform returns; the portal
     forwards the authenticator's response to `:finalize` and seals the
     returned identity into the ADR 0019 cookie exactly as the password
     path does now. Credentials live in Identity Platform beside the account.
     **No cryptography is written in this repository, and no Rust crate and
     no npm package is added.** This is the shape the record prefers, and
     it is the reason the checks are the first step and not an afterthought.

   - **Shape B — the portal is the relying party.** If any of checks 1–4
     fails, the portal generates the challenge, verifies the attestation on
     enrolment and the assertion on sign-in, and then mints an Identity
     Platform **custom token** for the verified account through the IAM
     Credentials `signJwt` API on the console's service-account identity
     (no key file — the standing rule) and exchanges it with
     `accounts:signInWithCustomToken` for the ID token the rest of the
     journey already expects. The dependency consequence, stated plainly:
     **Shape B earns one npm dependency in `frontend/portal/package.json`
     for WebAuthn verification**, and it earns it by ADR 0012's three-part
     test exactly as token verification did in ADR 0013 — getting the
     `rpIdHash`, origin, type, challenge binding, user-verification flag,
     signature-counter and credential-ownership checks wrong is silent and
     authenticates an attacker; the problem is adversarial and specialist
     (CBOR attestation objects, COSE key encodings, half a dozen attestation
     formats); maintained, widely audited implementations exist. Node's
     `crypto.verify` covers the signature itself, so what the dependency
     buys is the checks around the signature, which is where the defects
     are. That package and its transitive tree are reviewed as the frontend
     rule requires, and admitting it is a *frontend* dependency decision;
     ADR 0002's two Rust crates are unaffected. Shape B also needs a place
     for each credential's public key, id and counter. Under ADR 0019 that
     place is a custom claim, which caps an account at **two** passkeys
     (roughly 220 bytes each beside the agreements and roles, inside the
     1000-byte limit). Two is enough for the recovery rule below and is
     also the pressure that makes Shape A preferable.

   A hand-written verifier in-tree, under either shape, is refused on ADR
   0013's grounds. A Rust relying-party service inside `qip-api` is refused
   because it would put a second identity store beside Identity Platform
   (ADR 0019) and a third-party crate inside the workspace (ADR 0002) to
   solve a browser-side problem.

3. **The session stays the sealed cookie of ADR 0019.** Nothing about
   issuance, lifetime, `viewer`-only scope or the absence of revocation
   changes. The blueprint's "bound to a device" and per-action step-up are
   later rows; a passkey sign-in makes them possible and this record does
   not claim them.

4. **The password, forgot-password and reset-password flows are retired**
   once a passkey can be enrolled and asserted on a deployment. Retired
   means: the routes return `410 Gone` with a message naming the sign-in
   page, the pages are deleted, `enable_email_password` in the identity
   module becomes `false` and the variable's description says why, and
   `password` leaves `AuthMethod`. Until that moment the routes work and
   carry a deprecation note, so nobody reads them as the destination.

5. **Recovery is never a password, and never a shared secret.** Two paths,
   both required:

   - **A second passkey.** Enrolment prompts for a second authenticator
     before it lets the person into the console, and the account page lists
     each credential and lets the person revoke one (§40.3's "registered,
     listed, individually revocable"). The prompt can be declined; the
     console then shows, on every page, that the account has one credential
     and what losing it costs.
   - **An operator-issued one-time re-enrolment code.** A person who has
     lost every authenticator asks the desk. An operator, after verifying
     identity out of band (the desk knows its users; this is a research and
     risk desk, not the public), issues a single-use six-digit code with a
     fifteen-minute life and five attempts, the same discipline the existing
     one-time codes use. Its HMAC is written to the account as a custom
     claim by the console's admin path — the same two-permission identity
     ADR 0019 already grants — and the code is spoken or handed over, never
     emailed. Redeeming it opens **only** the enrolment ceremony: no cookie
     is issued, the console is not reachable, and the person signs in with
     the new passkey afterwards. A recovery path that cannot open the
     console is a recovery path that cannot be phished into a session.

   Rejected: **TOTP fallback**, which §40.3 names — a TOTP seed is a shared
   secret held on both sides, which is a password with a clock. It is
   reopened only if a device class that cannot register a platform or
   roaming authenticator is actually observed among the desk's users, and
   then as a *second factor beside* a roaming key, not as a credential.
   Rejected: **email-link sign-in** as recovery — it makes the mailbox the
   credential, and the mailbox is exactly the thing the desk does not
   control.

6. **The development provider keeps parity with whichever shape is
   chosen.** Under Shape B the same verifier serves both providers, with the
   JSON file holding credentials instead of custom claims, and the Playwright
   suite drives a virtual authenticator over the Chromium DevTools protocol
   (`WebAuthn.addVirtualAuthenticator`), which needs no dependency. Under
   Shape A there is no offline Identity Platform to call, so the development
   provider is the one place a password may still exist, behind the existing
   `DEVELOPMENT IDENTITY` label, and the deployed console refuses the
   password routes whenever `ALGORIK_IDENTITY_PROJECT_ID` is set. That
   exception is written down here so it is a decision and not a leak.

## What it costs

**The blueprint sentence, in operator practice.** "No passwords, anywhere"
means every lost phone, wiped laptop and replaced hardware key is a ticket
to the desk, and someone must answer it, verify a person out of band, and
issue a code by voice or in person. There is no self-service reset, on
purpose — self-service reset is a password by email. The desk must own at
least two authenticators per person, keep a roaming key for shared
workstations, and accept that an enterprise-managed browser that blocks
WebAuthn blocks the console. The re-enrolment code needs an operator surface
that does not exist yet, and an audit record of each issuance, which is a
row in the platform's own hash-chained log rather than a second store — the
first fact about a user ADR 0019 said would belong there.

**The Playwright suite is rewritten.** `tests/auth.spec.ts` signs in with a
password on four paths. Every one becomes an enrolment-then-assertion against
a virtual authenticator, and until that suite passes, the only behavioural
evidence for sign-in is the password suite this record deprecates.

**Under Shape B, the frontend takes a dependency and a two-passkey cap.**
The dependency is argued above; the cap is a consequence of ADR 0019 and is
the reason the native check comes first.

**Google federation is not advanced by this record.** The disabled button
stays disabled. A federated identity with a Google-side password is a
password somewhere, which is a separate decision to take honestly rather
than fold in here.

**Email is still verified, still an oracle risk.** The identifier is
unchanged, so the existing never-reveal-existence discipline on sign-up and
on the re-enrolment request must survive the rewrite; a fresh enrolment
page that says "that address is already registered" undoes it.

## What would make this wrong

* **If the desk's users are on devices that cannot register an authenticator
  at all** — not a browser policy, which is fixable, but a platform without
  WebAuthn — then the TOTP fallback the blueprint names is reopened as
  decided in point 5.
* **If Identity Platform's passkey endpoints turn out to be usable but
  return an identity without custom claims** (check 4 fails while 1–3
  pass), Shape A is not viable as written; the honest answer is Shape B, not
  a second claim-reading path bolted onto Shape A.
* **If a session ever authorises an action on the platform**, ADR 0019's
  revocation cost stops being acceptable and a passkey does not fix that; a
  `validSince` check on each request does, and that is ADR 0019's reversal
  clause, not this record's.
* **If the operator-issued code is ever found emailed, logged or written to
  a committed file**, the recovery path has become a password, and the
  answer is to remove the path rather than to relabel it.
* **If a credential is enrolled before the portal is served from the hostname
  it will keep.** Added 2026-09-06. A credential is bound to the relying-party
  ID it was created against and does not survive a change of it, so enrolling
  a population on today's Cloud Run hostname buys a re-enrolment of everyone at
  the domain migration. Either the migration comes first or the discard is
  accepted deliberately.
* **If a Shape B credential record is ever written into the same
  `customAttributes` blob `gipWriteProfile` replaces.** Added 2026-09-06. The
  next profile write deletes it, silently, and the account then reads as having
  no credential — or, if the write overflows the 1000-byte cap instead, as
  having no accepted agreements. Either failure is a user locked out by a
  routine role change.

## Nothing is applied

No passkey ceremony exists. No dependency was added under either shape. No
Terraform variable changed. `AuthMethod` still lists `"password"`, and the
password routes still sign a person in. What changed with this record, and
only this:

- `frontend/portal/src/app/(auth)/sign-in/page.tsx` — the working password
  form stays; its copy no longer presents the password as the platform's
  standing method, and a posture line beneath the form says passkeys are the
  recorded destination under this ADR and that it is not applied.
- `frontend/portal/src/app/api/auth/{sign-in,sign-up,forgot-password,reset-password}/route.ts`
  — a `@deprecated` doc comment naming this record and what replaces each.

The first step toward applying it is the four checks under "Whether Identity
Platform verifies a passkey itself", run by the owner against the dev
project, with their outputs quoted into this record's status line.

**The 2026-09-06 pass changed no file outside this one.** No probe was run
against any project — the session had no credential and no web access of any
kind — no Terraform variable moved, no npm package was added under either
shape, `AuthMethod` still lists `"password"` at
`frontend/packages/auth/src/index.ts:24`, and the word `passkey` appears under
`frontend/portal/src` only in the sign-in page and three of the deprecated auth
routes — the copy and the notes this section already lists. **No ceremony
exists**: the portal's one Identity Platform client
(`src/lib/server/identity-platform.ts`) reaches the v1 surface only —
`accounts:signUp`, `:signInWithPassword`, `:sendOobCode`, `:resetPassword`,
`:update`, `:lookup` — and names no v2 endpoint at all. **No gate was run for this
pass**: no Rust, Terraform or TypeScript file was touched, so `cargo fmt`,
`cargo clippy`, `cargo test`, `terraform validate` and the frontend gates have
nothing to judge; the documentation acceptance suite is the one gate a change
to this file reaches, and whoever accepts this pass runs
`cargo test -p qip-acceptance --test documentation` and quotes its
`test result:` line.
