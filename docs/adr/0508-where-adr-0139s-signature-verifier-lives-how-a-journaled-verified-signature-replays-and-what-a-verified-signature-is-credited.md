# ADR 0508: Where ADR 0139's signature verifier lives, how a journaled `VerifiedSignature` replays, and what a verified signature and a registry-listed TLS domain are credited

- **Status**: **Accepted**, 2026-11-13, under the owner's delegation of
  2026-10-31 (`docs/blueprint/owner-decisions-2026-10-31.md`), session t50-41.
- **Date**: 2026-11-13
- **Amends**: ADR 0139 decision 1 (how the registry reaches the connector)
  and decision 2 (which crate holds the verifier). Every other decision of
  ADR 0139 stands as written.
- **Keeps**: ADR 0110 (one `ring` pin, verification against committed keys,
  no network on the verify path), ADR 0043 (a claim's own word is not
  credited), `.claude/rules/architecture/00-boundaries.md` (a library
  performs no I/O).

## Context

ADR 0139 decided that a committed issuer registry lets a connector verify a
source's detached signature, and that `AuthenticitySignal::SignatureVerified`
carries a `VerifiedSignature` token with private fields and no public
constructor outside the verification function. Building it raised four
questions the record did not answer.

1. Rust's privacy is per crate. A token only the verifier can construct must
   be defined in the verifier's crate, and the fabric (`qip-evidence`) must
   be able to name it. ADR 0139 put the verifier in `qip_transport::trust`;
   `qip-evidence` linking `qip-transport` would pull the event mesh and its
   sockets into an I/O-free library, and `qip-transport` linking
   `qip-evidence` would put the fabric's types under the transport.
2. `EvidenceRecord` is journaled and decoded back for
   `EvidenceFabric::rebuild` (`qip-kernel/src/evidence_intake.rs`,
   `journaled_evidence_in`). A token that cannot be decoded makes the journal
   unreplayable; a token that decodes into a credited one makes it forgeable
   by anyone who can write JSON, because a key id and a digest are public.
3. ADR 0139 gave no figure for what a verified signature earns, nor for
   decision 4's "full weight" for a registry-listed TLS domain, and a lane
   may not invent one.
4. ADR 0139 said the registry is "read by the composition root". Every
   deployed connector path (`qip-fastbrain`, `qip-deepbrain`) builds its
   runtime inside `ConnectorFeed::over_transport`, and a path setting would
   be a deployment value nobody reviews beside the keys.

## Decision

1. **The registry is embedded at build from a committed file, and
   `ConnectorFeed` installs it.** The file is
   `backend/crates/services/qip-market-ingestion/issuer-registry.json`,
   read with `include_str!` as `COMMITTED_ISSUER_REGISTRY` and admitted by
   `committed_issuer_registry(at)` every time a feed opens. The binary carries
   exactly the registry its commit holds; no environment value can point it
   elsewhere or widen it. A registry that does not admit (a key that has
   expired since it was committed, an unknown algorithm, a duplicate key id)
   stops the feed from opening and names the key. It is committed with no
   issuers: no source in the present catalogue serves a detached signature,
   and an entry naming one would be a key this lane cannot obtain.
2. **The verifier and the token live in `qip-evidence::signature`**, on
   `qip_tls::ring`, the platform's one `ring` pin (ADR 0110). Verification is
   pure computation over bytes in memory; the module opens nothing.
   `verify_detached` is called by the connector runtime
   (`ConnectorRuntime::verify_signature`), once per poll body, and never by
   the fabric, which reads the token and checks no signature — ADR 0139
   decision 2's substance. The token carries a private `origin` that is never
   serialised. `verify_detached` makes `Verified`; deserialisation always
   yields `Decoded`, which the fabric records and refuses ("a claim that a
   check happened, not a check"); `EvidenceFabric::rebuild`, the one door
   that replays the platform's own hash-chained journal, marks decoded tokens
   `Journaled`, which is credited, so a rebuilt fabric weighs each claim as
   the fabric that journaled it did. The trust this places in `rebuild`'s
   input is the trust the platform already places in its event log.
3. **A verified signature earns `INDEPENDENTLY_VOUCHED_CREDIT` (0.3)**, the
   figure the pinned-hash credit already used, now named once for both. Both
   are the same fact — something other than the source vouches for exactly
   these bytes — so ranking a signature above or below a pin would be a
   figure nobody measured. A verified token whose digest is not the record's
   document hash is spoofing (ADR 0139 decision 3).
4. **A registry-listed TLS domain earns `REGISTERED_IDENTITY_CREDIT` (0.25)
   on top of `TLS_HOST_CREDIT` (0.2)**, both existing figures. The committed
   issuer registry is a registry this deployment trusts, so a listing in it
   is an identity registration and is credited as one; a host match alone
   keeps today's credit (ADR 0139 decision 4).
5. **The wire is one header**, `detached-signature`, a structured-field
   dictionary in the shape RFC 9421's `Signature` header uses:
   `keyid="<registry key id>", sig=:<base64>:`. The signature is over the
   body as it arrived, before the credential scrub; when the scrub rewrote
   the body the token is not attached and the poll reports why
   (`PollReport::signature_refusal`), because the fabric would otherwise read
   this platform's redaction as the source's forgery. A malformed header is
   no signature. A signature the registry cannot verify is reported and its
   claims go up unsigned.

## What it costs

- `qip-evidence` gains two in-workspace dependencies, `qip-tls` and
  `serde_json`; no third-party package is added (`ring` was already pinned).
  The library's dependency graph now reaches `rustls` through `qip-tls`
  although it uses only `ring`.
- Replay trusts `rebuild`'s input. A caller who can hand `rebuild` a forged
  `JournaledClaim` can forge a credited signature; in production that input
  is the hash-chained log, and the guarantee is exactly the log's.
- A token crossing a process boundary by serialisation arrives `Decoded` and
  is not credited there. The connector's verdict travels with the claim, but
  a fabric in another process weighs a signed claim as unsigned unless it
  re-verifies, which it cannot without the document.
- A source's key rotation makes its feeds refuse to open once the old key
  passes `not_after` in the committed file — louder than ADR 0139's "score
  drops", deliberately: a quiet downgrade is the failure this platform names.
- One fixed header name; a source that publishes its signature elsewhere (a
  sidecar `.sig` file, a PGP envelope) needs a connector change when it is
  registered.

## What would make this wrong

- If a fabric in another process must credit signed claims, the token has to
  carry the signature and the fabric the registry and the document, which is
  verification in the fabric and a reversal of ADR 0139 decision 2.
- If a measured comparison showed a registered issuer's signature is worth
  more or less than an independently pinned hash, decision 3's shared figure
  should split, under its own record.
- If the event log stopped being hash-chained, decision 2's replay rule would
  turn into a forgery path and must go with it.

## Rows

R12-EVID-2034.
