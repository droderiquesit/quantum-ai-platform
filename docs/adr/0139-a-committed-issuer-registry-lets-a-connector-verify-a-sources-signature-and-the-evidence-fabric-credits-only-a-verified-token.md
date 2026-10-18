# ADR 0139: A committed issuer registry lets a connector verify a source's signature, and the evidence fabric credits only a verified token

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It uses the ECDSA verification
  already in the tree (`ring` through `qip-tls`, ADR 0110) and admits no
  crate, no source and no credential.
- **Date**: 2026-10-18
- **Extends**: ADR 0110 decision 2 (verification is local, against committed
  public keys), ADR 0034 (a source is evaluated before it is used).
- **Keeps**: ADR 0043's position that this platform does not trust a claim's
  own word, `.claude/rules/architecture/00-boundaries.md` (a lib performs no
  I/O).

## Context

`qip_evidence::evidence::AuthenticitySignal` has five arms: `TlsVerified
{ domain }`, `HashVerified`, `SignatureVerified { signer }`,
`IdentityRegistered { registry }` and `OfficialRelease { channel }`. The
evidence fabric scores them (`qip-evidence/src/fabric.rs`): a TLS domain is
credited when it is the source's host, a registry when the deployment trusts
it — and `SignatureVerified` is always refused, with the reason in the code:
"The signal carries a signer's name and no signature bytes, so there is
nothing here to verify ... crediting this signal would be believing the
claim's own word." The connector that builds evidence
(`qip-market-ingestion/src/connector/evidence.rs`) attaches `HashVerified`
and nothing stronger. R12-EVID-2034 ("TLS/domain identity, signed releases
where available, known issuer identity") is PARTIAL because there is no
registry of issuers and their keys, and so no signature a connector could
check.

The verifier exists: `qip_transport::trust` verifies ECDSA P-256 signatures
over the platform's own artefact classes against a committed trust bundle
(`BundleVerifier`).

Locate them with
`grep -n 'pub enum AuthenticitySignal' backend/crates/libs/qip-evidence/src/evidence.rs`,
`grep -n 'AuthenticitySignal::SignatureVerified' backend/crates/libs/qip-evidence/src/fabric.rs` and
`grep -n 'pub struct BundleVerifier' backend/crates/libs/qip-transport/src/trust.rs`.

## Decision

1. **An issuer registry is a committed file**, like the trust bundle and in
   the same review: each entry names an issuer, the domains it publishes
   from, its public keys (algorithm, key bytes, key id, not-after), and the
   source catalogue entries it covers. It is read by the composition root
   and handed to the connector; no key is fetched at run time, because a key
   fetched from the source is the source vouching for itself.
2. **Signatures are verified in the connector, never in the fabric.** The
   connector, a service, verifies a document's detached signature against
   the registry with `ring` through `qip_transport::trust`'s verification
   path, generalised from artefact classes to registry issuers. ECDSA P-256
   and Ed25519 are admitted, both already in `ring`; anything else is
   refused as unsupported rather than skipped.
3. **`SignatureVerified` carries a token only the verifier can make.** The
   arm's payload becomes a `VerifiedSignature` with private fields (issuer,
   key id, document digest) and no public constructor outside the
   verification function, so the fabric credits a signature only if a
   verification produced it, and keeps refusing a name. A token whose digest
   is not the record's content hash is refused as spoofed.
4. **TLS identity is checked against the registry too.** `TlsVerified` is
   credited at full weight only when the domain is the source's host *and*
   the registry lists it for that issuer; host-only matching keeps today's
   lower credit, so a lookalike domain the source happens to sit on gains
   nothing new.
5. **A source with no registry entry is unchanged.** Nothing is inferred for
   it; its evidence scores exactly as today.

## Consequences

- A signed release from a known issuer outranks an unsigned copy of the same
  document, which is the row's test, on evidence a person can re-verify from
  the committed registry.
- Code slices left: the registry file and its loader with refusal tests
  (expired key, unknown algorithm, duplicate key id); the generalised
  verifier; the `VerifiedSignature` token and the fabric's crediting arm;
  the connector attaching it; and the row's test in
  `qip-world-model/tests/evidence_fabric.rs` that a verified-issuer claim
  outranks an unsigned copy, with its mutation (EVID-2034).

## What it costs

- A committed list of other parties' keys this repository must keep current:
  a rotated key at a source makes its evidence score drop until a reviewed
  commit adds the new key. That is the fail-closed direction and an
  operational chore.
- Few sources in the present catalogue publish detached signatures, so the
  arm will be rare in practice; the record pays for the mechanism ahead of
  most of its use.

## What would make this wrong

- If a source signed with an algorithm `ring` does not provide (RSA-PSS with
  unusual parameters, a PGP signature), decision 2's refusal would leave it
  unverifiable, and admitting a verifier for it is a dependency question for
  the owner.
- If issuers rotated keys faster than review can follow, a committed registry
  is the wrong shape and a signed registry update channel would be needed,
  which is its own record.

## Rows

R12-EVID-2034. It stays PARTIAL until its slices land with a mutated test.
