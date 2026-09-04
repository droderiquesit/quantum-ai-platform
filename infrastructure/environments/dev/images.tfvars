# dev's deployed image digests — written by .github/workflows/deploy.yml.
#
# Not hand-edited. Each digest below was built, scanned, signed and attested
# by run 33891084271 for bb4711998ecaec49a1b3f78a61bdba7f8ef10e8b, moved onto its Cloud Run service,
# and proven serving before this line was written. Terraform creates a
# service at the digest recorded here and ignores the image thereafter;
# modules/cloudrun says why.
image_digests = {
  qip-api       = "sha256:1f09b5c3e9205e6723f72a9de57c4aca04e80a24cb76eb0a2f9153699a18daf6"
  qip-fastbrain = "sha256:458a25afc17a3eaefdc0c8ff35542fa6abd57260725ad14aa6f4f9973a618c51"
  qip-deepbrain = "sha256:09138ad3b9738587315d28f8a9a19435ff35942c39edde3931dad7c324c081bc"
}
