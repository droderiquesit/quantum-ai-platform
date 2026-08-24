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

    False until the first deployment runs. Cloud Monitoring refuses an alert
    policy naming a PromQL metric it has never ingested — filter conditions
    and PromQL both, as two failed applies proved — so the four workload
    alerts cannot exist before the workloads do. Flip this to true in the
    environment's tfvars after the first deployment and re-apply; leaving it
    false thereafter silently removes the alerts, which is why the tfvars
    comment, not this default, is the reminder.
  EOT
  type        = bool
  default     = false
}
