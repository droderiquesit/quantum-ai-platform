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

variable "key_ring_id" {
  description = "The platform's existing KMS key ring. The model-artifact key is created in it."
  type        = string
}

variable "network_id" {
  description = "The VPC training and serving run inside. There is no public path to either."
  type        = string
}

variable "enable_vertex_ai" {
  description = <<-EOT
    Managed training and model registry.

    `crates/services/qip-training/src/vertex.rs` is a complete port with no
    transport: it has no Google client, no credential and no egress path, and
    every method reports itself unavailable naming what is missing. Its module
    documentation gives the reason plainly — a fake connection that appeared to
    submit a job would produce a model card recording a training run that never
    happened.

    So this is default-false. Enabling it provisions somewhere for training to
    run; it does not make this build able to submit a job. Local training in
    `qip_training::local` is real and needs none of this.
  EOT
  type        = bool
  default     = false
}

variable "training_service_account" {
  description = "Service account training jobs run as. Empty means none is bound."
  type        = string
  default     = ""
}

variable "deletion_protection" {
  type    = bool
  default = true
}
