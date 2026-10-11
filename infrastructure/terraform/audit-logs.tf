# Data-write audit logs on the services that change what runs (R12 SEC-9135,
# R12 CICD-9081; ported from recover/l016-cicd-037, its CICD-037).
#
# Admin-activity logs — a deploy, a cluster update, an instance created or
# deleted — are on by Google's default and cannot be turned off, so they are
# not configured here. The port deliberately drops the `ADMIN_WRITE` entries
# the recovered branch declared: that is not a log type Google accepts
# (`ADMIN_READ`, `DATA_READ` and `DATA_WRITE` are), and the apply would have
# failed on it. What is not on by default is DATA_WRITE: the writes to a
# resource's contents that sit below admin activity. With it, a change an
# agent makes through Cloud Run, the GitOps control plane's Kubernetes API or
# a Compute Engine machine leaves a record of who made it, which is what
# makes it attributable and revertible rather than merely present.
#
# Secret Manager and KMS have their own, wider audit config in
# modules/secrets (DATA_READ as well), and a service may hold only one
# `google_project_iam_audit_config`, so neither is named here.
#
# Unconditional: the infrastructure account holds
# `resourcemanager.projectIamAdmin`, which carries the setIamPolicy an audit
# config is written with, so no grant is widened for this.
resource "google_project_iam_audit_config" "runtime_changes" {
  for_each = toset([
    "run.googleapis.com",
    "container.googleapis.com",
    "compute.googleapis.com",
  ])

  project = var.project_id
  service = each.key

  audit_log_config {
    log_type = "DATA_WRITE"
  }
}
