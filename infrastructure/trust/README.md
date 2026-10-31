# infrastructure/trust/

The committed trust files every image carries at `/etc/qip/trust/`
(`infrastructure/docker/Dockerfile` copies this directory; `.github/workflows/image.yml`
carries it on into the execution node's boot image). ADR 0110 decision 2: the
trust bundle is a committed file baked into the attested image, **never fetched
at run time**.

**No trust bundle is committed here, on purpose.** A bundle is the public
keys of Cloud KMS key versions that exist; no environment has applied
`artefact_signing`, so there is no key to publish, and a placeholder bundle
would be a file a process could be pointed at that verifies nothing.

**`google-roots.pem` is committed (2026-10-31).** It holds the three Google
Trust Services roots GTS Root R1, R3 and R4, in that order, as plain PEM
certificates with nothing between them (`qip_tls::roots_from_pem` reads every
certificate block and refuses an empty file). Each was taken from the Mozilla
store this repository's build host ships (`/etc/ssl/certs/ca-certificates.crt`)
and checked byte for byte against the copy `https://pki.goog/repo/certs/gtsr<n>.der`
publishes; the SHA-256 of each root's DER is pinned in
`qip-tls`'s `the_committed_google_roots_are_the_pinned_gts_roots_and_build_a_client`,
so a root added, dropped or swapped here fails that test. GTS Root R2 is left
out: only one of the two sources carried it. Nothing points at the file yet:
`google_tls_roots_file` is null in every environment until the owner sets it
(`docs/ops/runbooks/owner-switches.md`).

## What may live here

| File | What | Named by |
|---|---|---|
| `<env>.json` | The environment's trust bundle, in `qip_contracts::trust::TrustBundle`'s shape: one entry per class (`policy`, `halt`, `envelope`, `model-pack`) with the full `cryptoKeyVersions` resource name, the hex DER `SubjectPublicKeyInfo`, a `not_after`, and `revoked` | `artefact_trust_bundle_file` |
| `<name>.pem` | The pinned Google root certificates a process authenticates KMS and CA Service against (ADR 0112 decision 1) | `google_tls_roots_file` |

One bundle per environment, never one shared: a prod cell holding the dev
key's public half would accept a frame dev signed.

## How a bundle gets here

Rendered, not written by hand:

1. An environment sets `artefact_signing = { bundle_not_after = "…" }` and the
   owner applies the plan. That creates the key ring and the four HSM keys.
2. `terraform output -json artefact_trust_bundle | jq . > infrastructure/trust/<env>.json`
   from the applied state. The output reads each key version's public key from
   Cloud KMS — nothing is pasted.
3. The commit that adds the file also sets `artefact_trust_bundle_file` and
   `google_tls_roots_file`. The plan refuses a bundle that does not publish,
   for every class, the version the signers use and that version's public
   key, and one whose `not_after` has passed.
4. `deploy.yml` builds images carrying the file; only those images verify
   under the keys.

A rotation adds the new version to the bundle and ships the image **before**
`artefact_signing.key_versions` moves the signers to it. Revocation lists the
version under `revoked` and ships an image.
