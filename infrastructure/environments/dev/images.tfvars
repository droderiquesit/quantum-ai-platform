# dev's promoted image digests — written by .github/workflows/deploy.yml.
#
# Not hand-edited. The pipeline moves each Cloud Run service to a digest it
# has built, scanned, signed and attested, waits for the revision to serve,
# and then records that digest here in the same run — so what this file names
# is always something a numbered run attested and Cloud Run admitted.
# Terraform creates a service at the digest recorded here and ignores the
# image thereafter; `modules/cloudrun` says why.
#
# These three are the digests the GKE runtime's last reconciled values file
# carried, which Binary Authorization admitted on that cluster: the same
# bytes, in the same registry, at the same digest. Nothing here has been
# deployed to Cloud Run — see ADR 0024 — so the first pipeline run overwrites
# this file with what it actually moved the services to.
image_digests = {
  qip-api       = "sha256:f66c1578f5cb80918db30d9520d5cdbfe16b3fc2877fefaf1f6ee9e0802a45e3"
  qip-deepbrain = "sha256:319d4eb32dfcc66cf19485281e3ae012087c03698755ccd6eba1279b406bfb3e"
  qip-fastbrain = "sha256:1daf48d4ac04042f5ca3abbf809248cec9b0a646b1a63d3d3b1565ca305bcd87"
}
