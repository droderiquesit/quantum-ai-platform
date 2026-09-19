# ADR 0038 — implementation brief for the two implementing lanes

**Written 2026-09-19 by the lane that took the decision. Design only; no code
in this document is authoritative — the crate and route names below are
where the work lands, not what it looks like.** The decision this brief
implements is
[ADR 0038](../adr/0038-passkeys-are-the-only-credential-and-passwords-leave-the-portal.md),
accepted with conditions on 2026-09-19. Read its status line and the section
"The four checks, 2026-09-19" before starting; the decision table there
selects which half of §3 applies, and **no row of that table is selected
yet.**

Two lanes. **R3 owns `backend/crates/apps/qip-api`.** **R4 owns `frontend/`.**
Nothing here touches `backend/crates/libs`, `services`, `runtime` or `edge`
except the one lib type §3B names, and that only under Shape B.

## 0. What is settled and what is not

Settled by the ADR and not reopened here:

- Passkeys are the only end-user credential; the password leaves (decision 1).
- The session is ADR 0019's sealed cookie, `viewer`-scoped, unchanged (3).
- Password, forgot-password and reset-password retire *after* a passkey can
  be enrolled and asserted on a deployment, not before (4).
- Recovery is a second passkey or an operator-issued single-use,
  single-attempt code that opens **only** the enrolment ceremony (5, as
  amended 2026-09-19). No TOTP. No email-link sign-in.
- The development provider keeps parity with whichever shape ships (6).
- No Rust relying party. No in-tree WebAuthn verifier. No new Rust crate.
- Under Shape B, credentials live in the platform's hash-chained event log,
  never in the claims blob (decision 2, amended 2026-09-19).

Not settled: **which surface verifies the passkey.** One probe decides it
(§1). Everything in §2 is shape-independent and may start now. Everything in
§3 waits for the probe.

## 1. The one activation step — Lane R4, first task, before any ceremony

Add `gipPasskeyProbe()` to `frontend/portal/src/lib/server/identity-platform.ts`
beside the existing calls, reaching
`https://identitytoolkit.googleapis.com/v2/accounts/passkeyEnrollment:start`
through a second endpoint constant (`ENDPOINT_V2`) and the same `call()`
discipline — explicit timeout, ALL_CAPS code extracted from `error.message`.
It takes an ID token obtained by the existing `gipSignIn` for a **scratch
account on `algorik-dev`**, sends `{ idToken, tenantId: "" }`, and returns
`{ status, code }` or `{ status, hasCredentialCreationOptions: true }`. It
logs nothing. It is exercised once by hand (or through a one-off script the
lane deletes), and the two values are quoted into ADR 0038's status line.

**What it must never do:** fall back to the development store, write to any
account, or print a token, key or `localId`. The account is scratch; the
probe is read-only on it; the only two facts that leave the session are an
HTTP status and a code.

Then read ADR 0038's table. Until a row is selected, §3 does not start.

## 2. Shape-independent work — may start now

### 2.1 Lane R4 — `frontend/`

**Vocabulary.**

- `frontend/portal/src/lib/server/identity.ts`: the sealed cookie's `method`
  union gains `"passkey"`. `"password"` stays until decision 4's retirement
  moment; do not remove it in this wave.
- `frontend/packages/auth/src/index.ts`: `AuthMethod` gains nothing (it
  already lists `"passkey"`); `"password"` leaves at retirement, not before.
  The register row §40.3 will stop being contradicted by this line only at
  that moment, and the row says so.

**Pages under `frontend/portal/src/app/(auth)/`.**

| Page | Change |
|---|---|
| `sign-in/page.tsx` | Passkey assertion is the primary control: one button that calls `navigator.credentials.get` with the options the BFF returns from `POST /api/auth/passkey/sign-in/start`, and posts the response to `.../finish`. The password form remains **only** when the development provider is active (decision 6); on a deployment with `ALGORIK_IDENTITY_PROJECT_ID` set it is not rendered and its routes answer `410` (§2.1, routes). Keep the never-reveal-existence discipline on every failure path. |
| `sign-up/page.tsx` | Becomes the enrolment ceremony: email as identifier → existing `sendOobCode` verification → `navigator.credentials.create` with the options from `POST /api/auth/passkey/enrol/start` → `.../finish` → **prompt for a second authenticator before the console is reachable** (declinable) → agreements. A fresh enrolment page that says "that address is already registered" undoes the oracle discipline; the response to a known address is indistinguishable from the response to an unknown one. |
| `recover/page.tsx` (new) | Redeems an operator-issued code. On success it opens the enrolment ceremony and **nothing else**: no cookie is minted, no portal route is reachable, and the person signs in afterwards with the new passkey. A wrong code invalidates the entry (ADR 0038 decision 5 as amended) and the page says to ask the desk again. |
| `forgot-password/`, `reset-password/` | Unchanged this wave. Deleted at retirement. |

**Pages under `frontend/portal/src/app/(portal)/`.**

| Page | Change |
|---|---|
| `account/credentials/page.tsx` (new) | Lists each enrolled credential — label, created date, transports, last used — and lets the person revoke one. §40.3's "registered, listed, individually revocable". No key material is rendered; the browser receives nothing the public may not see, and a public key is public, but there is no reason to show it. |
| `AppShell.tsx` | When the account holds one credential, every page shows a persistent line saying so and what losing it costs. Rendered beside, never instead of, the `PAPER TRADING` label. |
| `admin/recovery/page.tsx` (new, role-gated with the existing admin area) | The operator surface decision 5 says does not exist yet: pick a user, issue a code, see it once, hand it over by voice. The page never sends it anywhere. |

**BFF routes under `frontend/portal/src/app/api/auth/`.** All server-side; the
browser never sees an ID token, exactly as today.

| Route | Does |
|---|---|
| `passkey/enrol/start`, `passkey/enrol/finish` | Under Shape A, relays to Identity Platform's `passkeyEnrollment:start` / `:finalize`. Under Shape B, mints the challenge (Node `crypto.randomBytes`) and verifies the attestation with the reviewed dependency. Either way it refuses: an unverified email; an `rp.id` other than the configured host; an origin not in the allowed list; a response whose challenge is not the one issued; `userVerification` not `required`; a session older than the ceremony's own window. |
| `passkey/sign-in/start`, `passkey/sign-in/finish` | The assertion. Refuses an unknown credential id, a `uv` flag of false, an origin mismatch, and (Shape B) a signature counter that did not increase. On success, seals the ADR 0019 cookie with `method: "passkey"` and `authenticatedAt` from the finish instant. |
| `passkey/credentials` (`GET`, `DELETE`) | List and revoke for the account page. Revoking the last credential is refused unless the person has just been shown what it costs and confirmed; the response never says whether a given id exists for another account. |
| `recovery/redeem` | Verifies the HMAC of the presented code against the `recovery` field of `StoredProfileClaims`, checks `issued_at` + 15 min, invalidates on any outcome, and on success issues a one-time enrolment grant that the enrol routes accept **instead of** a session — never a cookie. |
| `admin/recovery/issue` | Operator-only. Writes `{ hmac, issued_at, issued_by }` into `StoredProfileClaims.recovery` through `gipWriteProfile` — the one writer — and refuses while a profile write for the same account is in flight. Returns the code once. |
| `sign-in`, `sign-up`, `forgot-password`, `reset-password` | On a deployment with `ALGORIK_IDENTITY_PROJECT_ID` set: `410 Gone` naming the sign-in page, from the moment a passkey can be enrolled and asserted there. Under the development provider: unchanged. |

**What the recovery code must never be**: emailed, logged, written to a
committed file, or displayed twice. Its HMAC is the only thing stored, in
the profile claim, behind the one writer.

**Persistence under the development provider** (decision 6): the JSON file
holds credentials — public key, id, counter — instead of password hashes,
behind the existing `DEVELOPMENT IDENTITY` label. Under Shape A the
development provider is the one place a password may still exist; the
deployed console refuses the password routes whenever the identity project
is set.

**Tests.** `tests/auth.spec.ts` is rewritten around a virtual authenticator
over the Chromium DevTools protocol (`WebAuthn.enable`,
`WebAuthn.addVirtualAuthenticator` with `hasUserVerification: true`,
`isUserVerified: true`) — no dependency. Each of the refusals above is a
test that drives the wrong input and asserts the refusal *and* that the
response is indistinguishable from the not-found case where the discipline
requires it. Every new test is mutation-verified and the report says which
line was broken and that it fired. The password suite is deleted only at
retirement.

**Gates.** `npm run lint`, `npm run build`, `npx playwright test` in
`frontend/portal/`; `npm run tokens:check` at `frontend/` if a token moves.
Under Shape B only: the dependency and its transitive tree in the diff, and
the ADR 0012 three-part argument quoted in the commit message.

### 2.2 Lane R3 — `backend/crates/apps/qip-api`

**The honest scope first.** Under either shape `qip-api` runs no ceremony
and stores no credential (ADR 0038 decision 2; ADR 0043's (d)). What a
passkey changes on the API side is one thing: the platform can, for the
first time, know that a person authenticated interactively and when. That
fact arrives through **ADR 0042's keyed assertion**, which carries `method`
and `authenticated_at`, and ADR 0042 is *proposed and unapplied*. So:

**Lane R3 has no ADR 0038 work that can honestly ship before ADR 0042's
verifier exists.** A `Presence::Attested` variant with no producer is a type
nothing reads — the lane contract's own definition of not built. If this
wave's `qip-api` scope is ADR 0038 alone, the deliverable is ADR 0042's
verifier (its decisions 1–4 and 6), with the `method` field constrained as
below. If ADR 0042 is out of scope, R3's 0038 work is deferred and this
brief says so rather than inventing a route.

**When ADR 0042's verifier lands, the 0038 work is:**

1. `auth.rs`: `Presence` gains `Attested { at: Timestamp, method:
   AttestedMethod }` where `AttestedMethod` is an enum with **one variant,
   `Passkey`** — not a string. `development` never attests on a deployment
   (the console refuses the development provider when the identity project
   is set, decision 6), and `password` and `google` are not interactive
   proofs this platform accepts. The variant is constructed in exactly one
   place: the ADR 0042 verifier, after the MAC, nonce, window, path and body
   checks have passed and the assertion's `method` is the literal `passkey`.
   `authentication_instant(..)` returns `Ok(at)` for `Attested` and keeps its
   refusal for `Unattested`, whose message stays as it is.
2. `routes.rs`: the arms that build an `OperatorIdentity` pass
   `"passkey"` as the method when the principal is `Attested`, replacing the
   literal `"api-bearer-token"` for that case only. The bearer token is
   still required beside the assertion (ADR 0042 decision 2); the assertion
   confers no authority (decision 4), so `required_role` on every route is
   unchanged.
3. Two mutating routes, both `Role::Operator`, both requiring an `Attested`
   principal, both recording a **fact about a person** and nothing about
   capital:
   - `POST /identity/recovery-issuances` — journals `{ subject, issued_by,
     issued_at, expires_at }`. Never the code, never its HMAC. This is the
     audit row ADR 0038's "What it costs" says belongs in the hash-chained
     log rather than a second store.
   - `POST /identity/credential-events` — journals `{ subject, kind:
     Enrolled | Revoked | RecoveryRedeemed, label, at }`. Under Shape A this
     is attribution only (the credential itself is Identity Platform's).
     Under Shape B it is also the credential store; see §3B.
   Both patterns are literals in `ROUTES` and in the handler arm, because
   `api_boundary.rs` and `security.rs` read the table as text and a named
   constant blinds them (the doc comment above `REINSTATEMENT_PATTERN`
   explains why). Both are reviewed by `api_boundary.rs` as mutating routes;
   the review must find that neither reaches `Platform::decide_*` on
   capital, an order, a venue or the autonomy controller.
4. Tests: `tests/security.rs` walks the two new arms as it walks the others;
   a test that an `Unattested` principal is refused at each with the
   existing refusal text; a test that `Attested` is unconstructible from a
   `method` other than `passkey` (the enum makes this a compile-time fact;
   the test asserts the verifier's refusal on the wire value). Each
   mutation-verified.

**Gates.** `cargo fmt --all --check`; `cargo clippy --workspace --all-targets`
at zero warnings; `cargo test --workspace --no-fail-fast` with the summed
totals quoted; `cargo test -p qip-acceptance --test api_boundary --test
security --test paper_boundary --test compliance_proof --no-fail-fast`;
`./scripts/check-dependencies.sh` (still 11); `./scripts/check-secrets.sh`.
Export the three `CARGO_*` variables the lane contract names before every
cargo invocation.

## 3. Shape-dependent work — waits for the probe

### 3A. Shape A — Identity Platform verifies

Lane R4 only. The four BFF passkey routes relay to
`v2/accounts/passkeyEnrollment:start|finalize` and
`v2/accounts/passkeySignIn:start|finalize` through `call()`. The BFF still
enforces the refusals in §2.1 that Identity Platform cannot be trusted to
enforce for us — the email verification precondition, the second-authenticator
prompt, the oracle discipline — and forwards the authenticator's response
otherwise unaltered. The three remaining checks, in order, each one call:

| Check | Call | Stop if |
|---|---|---|
| 2 — passkey as the only provider | On a **scratch project**, not `dev`: `enable_email_password = false`, one account, `passkeySignIn:finalize` returns an ID token | It does not. Then decision 4's `enable_email_password = false` step is not available and the retirement keeps the provider enabled with `password_required` — record that in the ADR before continuing. |
| 3 — RP ID | Read the `rp.id` in the `credentialCreationOptions` the probe returned | It is not the portal's served hostname, or it is a hostname the algorik.ai migration will change. **Do not enrol a population before the destination hostname is the origin**, or accept the discard deliberately in the ADR. |
| 4 — custom claims survive | Decode the ID token from a passkey sign-in and compare its claims to the password path's | The claims are absent. Shape A is not viable as written; go to Shape B. Do not bolt a second claim-reading path onto Shape A. |

`qip-api` is untouched by Shape A beyond §2.2.

### 3B. Shape B — the portal is the relying party

Requires ADR 0042 applied first (the console has no operator write path to
the API without it). Then:

- **Lane R4:** one npm dependency for WebAuthn verification, admitted by the
  ADR 0012 test as ADR 0038 decision 2 argues, with its transitive tree in
  the diff. Node's `crypto.verify` does the signature; the package does the
  checks around it. Challenges from `crypto.randomBytes`. A custom token
  minted through IAM Credentials `signJwt` on the console's Workload
  Identity Federation identity — no key file — exchanged with
  `accounts:signInWithCustomToken`.
- **Lane R3:** `POST /identity/credential-events` from §2.2 becomes the
  credential store. The `Enrolled` kind carries `credential_id`, the COSE
  public key bytes, `aaguid`, `transports`, `label`, `created_at`; an
  `AssertionObserved { credential_id, sign_count, at }` kind is appended per
  sign-in, and the current counter is **derived by replay**, never edited in
  place. `GET /identity/credentials?subject=` (`Role::Viewer` — the console's
  own ADR 0018 credential, since no session exists yet at sign-in time)
  returns the live set: enrolled minus revoked, each with its replayed
  counter. The record type lives in a lib (`qip-events` or a sibling under
  `backend/crates/libs`), the journaling in `qip-kernel`, the routes in
  `qip-api`. Dependency direction: libs ← runtime ← apps; the console reaches
  the API over HTTP; nothing in `backend/` learns that the frontend exists.

## 4. The paper-trading boundary, and why authentication cannot move it

Authentication under this record gates **who may view** and **whose name is
on a countersignature**. It does not and cannot gate an order, because there
is no order path to gate: `/orders` is `Method::Get`, `Role::Viewer`, and
"orders, fills, refusals and any venue/book disagreement" is a read
(`grep -n 'pattern: "/orders"' -B 2 -A 3 backend/crates/apps/qip-api/src/routes.rs`).
`api_boundary.rs` reviews every mutating route in the table; `paper_boundary.rs`
holds the line beneath it. The three layers are untouched by every line of
this brief: Terraform's refusal of the three live ceilings at plan time;
`AutonomyLevel::deployable` refusing the same three at start-up in the three
central binaries; `qip-edge`'s `Cell` with no constructor for any ceiling but
paper and `qip-cost-router`'s `Determinism::Required` arm. An `Attested`
presence unlocks a freshness gate on routes that already require
`Role::Operator` and a second signature; autonomy changes still go through
`AutonomyController::request_change` with a second approver; an assertion
confers no authority (ADR 0042 decision 4). The two new routes record facts
about people. If either lane finds itself adding a mutating route that is
not, stop: that is the reversal condition ADR 0038 added on 2026-09-19.

## 5. What this brief does not cover, named so nobody reads it as covered

- §40.3's **recovery delay and notification** — "identity verification plus
  a mandatory delay, every registered contact notified at the start" — and
  the **recovery bound** on adding a destination. ADR 0038's recovery is
  narrower: out-of-band verification by the desk, no delay, no notification.
  Whether to add both is a separate decision; the register row says so.
- **Device attestation** ("with attestation" in §40.3's devices row).
  Neither shape requires attestation conveyance beyond `none`; requiring
  `direct` and checking AAGUIDs against a desk allowlist is a later row.
- **Step-up per action** and **biometric** as §40.4's authentication column
  names them. A passkey sign-in makes both possible; this record claims
  neither.
- **Google federation.** The disabled button stays disabled.
- **Session revocation.** ADR 0019's `validSince` clause, unchanged.
