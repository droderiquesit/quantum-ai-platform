# Where the platform's own logs land, and for how long (R12-OBS-10058).
#
# Until this file every workload's stdout went to the project's `_Default`
# bucket, which this repository did not create and cannot size: its retention
# is Google's, and it is shared with everything else in the project. A
# dedicated bucket with a stated retention makes the volume a number a
# reviewer reads, and the sink routes only the platform's own workloads to it.
#
# Per-event lines are already bounded at the source (`PassLog::new(5, 10s)` in
# `qip-edge-node`), so no exclusion filter is declared here: an exclusion
# written against a log shape nobody has observed in a deployed process would
# drop lines on a guess. The node is not deployed; when one is, the shape can
# be read and the exclusion written against evidence.

resource "google_logging_project_bucket_config" "workloads" {
  project        = var.project_id
  location       = "global"
  bucket_id      = "qip-${var.environment}-workloads"
  retention_days = var.log_retention_days
  description    = "The platform's workload logs, kept for ${var.log_retention_days} days."
}

resource "google_logging_project_sink" "workloads" {
  project     = var.project_id
  name        = "qip-${var.environment}-workloads"
  destination = "logging.googleapis.com/${google_logging_project_bucket_config.workloads.id}"
  description = "Routes the platform's Cloud Run revisions and execution nodes to the bounded workload bucket."

  # Only what this platform runs: the Cloud Run services named for the
  # environment and the Compute Engine nodes carrying its prefix. A filter
  # that matched every `cloud_run_revision` in the project would route another
  # team's logs into a bucket whose retention they did not choose.
  filter = "(resource.type=\"cloud_run_revision\" AND resource.labels.service_name:\"qip-${var.environment}-\") OR (resource.type=\"gce_instance\" AND labels.\"compute.googleapis.com/resource_name\":\"qip-${var.environment}-\")"

  unique_writer_identity = false
}
