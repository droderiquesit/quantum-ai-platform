variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "region" {
  description = "The region the snapshot schedule lives in. A resource policy is regional and attaches only to disks in its own region, so a node in another region needs a schedule of its own."
  type        = string
}

variable "labels" {
  type = map(string)
}

variable "key_ring_id" {
  description = "The platform's existing KMS key ring. The backup key is created in it rather than in a second ring nobody rotates."
  type        = string
}

variable "snapshot_start_time" {
  description = <<-EOT
    When the journal snapshot is taken, as `HH:MM` in UTC.

    A snapshot of a persistent disk is incremental after the first and taken
    without pausing the writer, so this does not need a quiet window and is
    not constrained by market hours. Off the hour deliberately: everything
    scheduled on the hour in a Google Cloud project contends with everything
    else scheduled on the hour.
  EOT
  type        = string
  default     = "05:00"

  validation {
    condition     = can(regex("^([01][0-9]|2[0-3]):00$", var.snapshot_start_time))
    error_message = "A snapshot start time is `HH:00` in UTC; Compute Engine accepts only whole hours."
  }
}

variable "snapshot_retain_days" {
  description = <<-EOT
    How long a journal snapshot is kept. Ninety days: these are the copies
    that keep covering a journal after the node that wrote it has been
    replaced, and the question they answer is a compliance one rather than an
    operational one, so the window is months rather than weeks.
  EOT
  type        = number
  default     = 90

  validation {
    condition     = var.snapshot_retain_days >= 1 && var.snapshot_retain_days <= 365
    error_message = "Snapshot retention is between 1 and 365 days."
  }
}
