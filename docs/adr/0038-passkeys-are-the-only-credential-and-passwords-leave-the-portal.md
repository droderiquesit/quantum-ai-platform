# 0038 — Passkeys are the only end-user credential, and passwords leave the portal

**Status:** *proposed*, 2026-09-05. Nothing below is applied: the portal
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
