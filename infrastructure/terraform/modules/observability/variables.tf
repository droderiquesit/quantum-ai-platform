variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "notification_channels" {
  type = list(string)
}

variable "workload_metrics_exist" {
  description = <<-EOT
    Whether the platform's own Prometheus metrics have ever been scraped in
    this project.

    False until something is proven to have scraped a process. Cloud
    Monitoring refuses an alert policy naming a PromQL metric it has never
    ingested — filter conditions and PromQL both, as two failed applies
    proved — so no workload alert in this module can exist before the
    descriptors do. Flip this to true in the environment's tfvars once there
    is evidence of an ingested descriptor and re-apply; leaving it false
    thereafter silently removes the alerts, which is why the tfvars comment,
    not this default, is the reminder.

    This text said "the four workload alerts" after the module had grown past
    four, so count them here rather than believing a number in a comment:

      grep -c '^resource "google_monitoring_alert_policy"' main.tf
      grep -c 'count *=.*workload_metrics_exist'           main.tf

    Both answered 9 on 2026-09-06, and they must stay equal — a policy added
    without the gate is a policy that fails the apply on a project that has
    ingested nothing.
  EOT
  type        = bool
  default     = false
}
