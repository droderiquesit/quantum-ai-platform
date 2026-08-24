# What this module does not do

The chain from a build to an admitted image now exists: a KMS signing key, a
Container Analysis note, an attestor, a deny-by-default policy, and a signing
step in `.github/workflows/deploy.yml` after the push. This file is the honest
half — what a deployment must supply, and what the chain proves and does not.

Nothing here is a caveat added for form. Each item below is something that,
left undone, produces either a cluster that refuses every image or a signature
that means less than it looks like it means.

## A deployment must supply these

**Two repository variables.** The pipeline reads the attestor and the key
version from GitHub repository variables, the same way it already reads the
workload identity provider:

    GCP_BINAUTHZ_ATTESTOR      terraform output attestor_name
    GCP_BINAUTHZ_KEY_VERSION   terraform output attestor_key_version

`deploy.yml` checks both before it builds anything and fails with those names
if either is empty. That is deliberate: the alternative is a pipeline that
builds four images, pushes them, cannot sign them, and reports success up to
the moment the cluster refuses them.

**Two APIs enabled on the project.** `binaryauthorization.googleapis.com` and
`containeranalysis.googleapis.com`. This configuration manages no
`google_project_service` anywhere — API enablement has always been out of band
here — so the first apply fails with a `SERVICE_DISABLED` error naming the API
rather than anything about signing.

**An operator who applies this before the first signed deploy.** The policy
denies by default from the moment it exists. Anything already running in the
project keeps running, because Binary Authorization decides at admission; the
refusal appears the next time a pod is scheduled, which may be hours later and
will not look like a policy change. Apply this, set the two variables, and let
one deployment through before relying on the cluster to reschedule anything.

## What the signature actually proves

It proves that **the pipeline pushed these bytes**. It is the answer to "did
this image come from our build" and to nothing else.

It does not prove the image was built from reviewed source, that its
dependencies were the ones the lockfile names, or that the tests passed — the
last of those is enforced by the `gate` job in `deploy.yml` rather than by
anything a cluster can verify at admission. A SLSA provenance statement is the
thing that would carry those claims, and this repository does not produce one.

## The signer is the pipeline, and that is the weak link

`roles/cloudkms.signerVerifier` on the attestor key is held by the deploy
service account. Anyone who can make that pipeline run a step of their choosing
can sign an image of their choosing, so Binary Authorization here raises the
bar from "anything in the registry runs" to "anything the pipeline signs runs".
That is a real improvement and it is not the same as a two-party control.

The stronger arrangement is a signer the pipeline cannot impersonate: the key
in a separate project, a human or a separate approval workflow performing the
`sign-and-create`, and the deploy account holding no KMS permission at all.
This repository cannot contain that, because the second identity and the
project that holds it are not things a repository can create for itself — the
same reason the `production` GitHub environment's required reviewers are a
repository setting recorded in `docs/operations/external-dependencies.md`
rather than a file here.

What is here does keep the two halves separate as far as one project allows:
the pipeline can sign and cannot change the policy that requires signing, and
it can attach an attestation and cannot add a public key to the attestor.

## Rotation is a sequence, not a setting

The key has no `rotation_period`, because Cloud KMS only rotates symmetric
keys automatically. Rotating it means: create a new version, add its public key
to the attestor while the old one is still listed, sign new images with the new
version, and disable the old version only once nothing signed by it is still
being scheduled. Disabling it early refuses running images at their next
reschedule, one at a time, as they happen to move.

## This does not fix the pipeline's other gap

`deploy.yml` still cannot reach the cluster's private endpoint from a
GitHub-hosted runner. That gap is listed separately in
`docs/operations/external-dependencies.md` and is unaffected by anything here:
signing happens against the registry and Cloud KMS, both of which a hosted
runner can reach.
