# 0019 — Identity Platform is the only identity store, and the session is stateless

**Status:** accepted

## Context

The portal's identity store is a JSON file. `identity-store.ts` says so at
length and says why: it is the development provider, written atomically by
rename, and "its interface is the contract a production store (Identity
Platform plus the platform's own user record) implements later."

Later did not happen. The deployment sets
`ALGORIK_IDENTITY_STORE_DIR=/tmp/algorik-identity` on Cloud Run, where `/tmp`
is a per-instance in-memory filesystem. It is destroyed on every scale-to-zero,
every scale event and every revision, and it is not shared between two
instances that exist at the same time. Three consequences, in the order a user
meets them:

1. **Signing in appears to do nothing.** The session is created in the file on
   the instance that served `POST /api/auth/sign-in`. The next request may be
   served by another instance, where `identityStore.session(id)` returns null,
   so `/api/auth/session` answers unauthenticated and the gateway refuses. The
   cookie is valid, correctly signed, and useless.
2. **Every record is lost on scale-to-zero.** `min-instances` is 0, so an idle
   console discards its entire store.
3. **Agreements are re-invented rather than remembered.** This is the quiet
   one. In platform mode `platformProfile()` finds no local record and creates
   one with `agreements: { terms: true, privacy: true, riskDisclosure: true }`.
   The record asserts the user accepted the terms, the privacy policy and the
   risk disclosures. Nobody asked them, and nothing checked. A compliance fact
   was being manufactured from its own absence.

What is *not* broken is worth stating, because it narrows the fix: the user
account itself is durable. `gipSignUp` creates it in Identity Platform, the
password lives there, `emailVerified` lives there, and the one-time codes for
verification and reset are Google's `oobCode`s, not ours. The only things the
file held that mattered were the session and the profile.

## Decision

**Identity Platform holds identity. The console holds no identity store.**

- **The session becomes a stateless sealed cookie.** It carries the claims —
  user id, email, issue and expiry instants, authentication method — under an
  HMAC with `ALGORIK_SESSION_SECRET`. Any instance can verify it, no instance
  has to remember it, and a scale event is invisible.
- **The facts the console owns become Identity Platform custom claims.**
  Account type, the agreements accepted and their version, and roles are
  written to the account record at sign-up and read back at sign-in. They live
  where the account lives, so there is one store and no reconciliation.
- **Absent agreements are refused, never assumed.** An account whose claims
  carry no agreements record is sent to re-accept. "Refuse rather than guess"
  is a house principle, and a manufactured consent record is the worst possible
  guess.
- **The file store stays, and shrinks to what it always was.** It backs the
  development provider — the offline slice that runs with no Google project —
  and nothing else. It is no longer on the path of a deployed console.

## Why not a database

A database was the obvious answer and it is the wrong one here.

**Firestore.** The natural choice, and it would work: per-document atomicity,
a TTL policy for sessions, no npm dependency because the REST API is reachable
with `fetch` and a metadata-server token. It costs an API to enable, a database
whose mode is close to irreversible, an IAM grant, and — the real objection —
a *second* place a user exists. Two stores holding one user disagree
eventually, and the console would then need to decide which is right about
`emailVerified`, a question Identity Platform already answers.

**Cloud Storage.** Enabled already, and a trap. The store does
read-modify-write over one JSON object with no locking; two instances writing
concurrently silently lose one of the writes. `ifGenerationMatch` would fix it
and would turn every sign-in into a compare-and-swap retry loop over a global
object.

**A Cloud Run GCS volume mount.** Zero code change, and the same lost-update
race as above, now invisible because the code still looks like a local file
write.

## What it costs

Two things: revocation, and one grant on the console's identity.

### Revocation

A sealed cookie is valid until it expires; there is no server
to tell it otherwise. Signing out clears the cookie, which handles the case
that actually happens, but it does not invalidate a copy someone else took.
Before, revocation was possible in principle — and in practice a session
disappeared whenever Cloud Run felt like it, which is not revocation, it is
loss.

This is acceptable *here* and the reasons are specific, not general. The
session grants `viewer`: reading a paper-trading console. It cannot run a
cycle (`analyst`) or touch the kill switch (`operator`). Its lifetime is
twelve hours. The platform's own credential is separate, is not in the
browser, and is not derived from the session.

It would stop being acceptable the moment a session could authorise an action
on the platform. If that day comes, the fix is a revocation check against
Identity Platform's `validSince` on each request, and this ADR is where to
start.

### A grant on the console's identity

Writing custom claims is an
administrative operation on the Identity Platform project. The console gets a
custom role holding exactly two permissions — `firebaseauth.users.get` and
`firebaseauth.users.update` — rather than `roles/identitytoolkit.admin`, which
also carries `firebaseauth.configs.getSecret`, `firebaseauth.users.delete` and
`identitytoolkit.tenants.setIamPolicy`. The console reads an account and
updates an account. It cannot delete one, cannot read the project's signing
configuration, and cannot change who administers the tenant.

The credential is the Cloud Run service account, resolved through the metadata
server. No key file exists, which is the standing rule.

## What would make this wrong

If the console ever grows a fact about a user that Identity Platform cannot
hold — custom claims are capped at 1000 bytes, and an audit trail of
agreement acceptances over time would not fit — then it needs a real store,
and the honest place for an auditable acceptance record is the platform's own
hash-chained event log rather than a second database beside it.
