# dev's deployed image digests — written by .github/workflows/deploy.yml.
#
# Not hand-edited. Each digest below was built, scanned, signed and attested
# by run 33780092495 for c3140aff007a629f9fc0d654efde6cfd7e339f5a, moved onto its Cloud Run service,
# and proven serving before this line was written. Terraform creates a
# service at the digest recorded here and ignores the image thereafter;
# modules/cloudrun says why.
image_digests = {
  qip-api = "sha256:6e432a5a1770127bb9ef03bb32b93d35f93b1029a6fbf738546939975c5af565"
  qip-fastbrain = "sha256:8607d52c528877fce4545b1d45613c72afde8025bf99aa20c5b195de8ea1f797"
  qip-deepbrain = "sha256:aaf7cc5a644bfd6cd13df70a0bc461e9c3107963405cab214865a9021b63786e"
}
