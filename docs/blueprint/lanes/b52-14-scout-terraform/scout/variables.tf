variable "project_id" {
  type = string
}

variable "region" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "image" {
  description = "The deep brain's image, pinned by digest. The scout runs its `scout` entry point (ADR 0144); it is not a separate binary."
  type        = string

  validation {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", var.image))
    error_message = "The scout image is pinned by digest, never by tag: a policy that trusts a tag trusts whoever can push it."
  }
}

variable "deepbrain_service_account_email" {
  description = "The deep brain's service account. It is granted read on the scout bucket and nothing else here."
  type        = string
}

variable "connector_sources" {
  description = "The catalogued research connector ids the job polls, comma-separated."
  type        = string
}

variable "egress_proxy_url" {
  description = "The scout's own proxy listener. Loopback, as every connector base URL is."
  type        = string
  default     = "http://127.0.0.1:9105"
}

variable "schedule" {
  description = "Cron schedule of an execution. Null creates the job with no trigger: it is then run by hand."
  type        = string
  default     = null
}

variable "task_timeout_seconds" {
  description = "The job's own timeout. Provisional: ADR 0144 leaves the quota to the catalogue and none is stated, so the restrictive end of a poll's worth is used."
  type        = number
  default     = 300
}
