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

    `backend/crates/services/qip-training/src/vertex.rs` is a complete port with no
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
  description = <<-EOT
    Service account training jobs run as.

    Empty is refused when `enable_vertex_ai` is true, by a precondition on each
    of the three grants in main.tf rather than by their `count`. It used to
    mean "none is bound", which was the wrong answer to an unset value: the
    module provisioned the bucket, the endpoint and the metadata store and
    granted nothing on them, so the first sign of the mistake was a training
    job denied on its own staging bucket. It is still ignored entirely when
    `enable_vertex_ai` is false, because then there is nothing to bind to.
  EOT
  type        = string
  default     = ""
}

variable "deletion_protection" {
  description = <<-EOT
    Whether the training staging bucket refuses to be destroyed while it still
    holds objects. Default true, matching modules/data: a `terraform destroy`
    aimed at dev that reached another environment is not a hypothetical.

    Read by exactly one resource, `google_storage_bucket.training`, as
    `force_destroy = !var.deletion_protection`. It was read by nothing at all
    until then — declared, defaulted true, and referenced by no resource in
    this module while the root never passed it either — which is a safety
    default that protects nothing and reads in a review as one that does.

    It cannot reach the KMS key: `lifecycle.prevent_destroy` takes a literal
    and not a variable, so that key states `true` directly and always will.
  EOT
  type        = bool
  default     = true
}
