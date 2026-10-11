# infrastructure/trust/

The committed trust files every image carries at `/etc/qip/trust/`
(`infrastructure/docker/Dockerfile` copies this directory; `.github/workflows/image.yml`
carries it on into the execution node's boot image). ADR 0110 decision 2: the
trust bundle is a committed file baked into the attested image, **never fetched
at run time**.

**Nothing but this README is committed here today, on purpose.** A bundle is
the public keys of Cloud KMS key versions that exist; no environment has
applied `artefact_signing`, so there is no key to publish, and a placeholder
bundle would be a file a process could be pointed at that verifies nothing.

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
