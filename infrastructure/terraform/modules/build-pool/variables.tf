variable "project_id" {
  description = "The project the pool, its identity and its staging bucket live in."
  type        = string
}

variable "region" {
  description = "Where the pool and its staging bucket live: the build registry's region, so the saved image never leaves it."
  type        = string
}

variable "environment" {
  description = "dev, test, stage or prod. Only the build environment builds (deploy.yml's BUILD_ENVIRONMENT)."
  type        = string
}

variable "labels" {
  description = "The labels every resource carries."
  type        = map(string)
  default     = {}
}

variable "peered_network_id" {
  description = <<-EOT
    The VPC the pool's workers are peered into: the Engineering/build plane
    network `modules/network` creates for exactly this (GCP-051), and never
    the service network. A build worker peered into the network the
    platform's workloads sit on would be a route from untrusted build input
    to every workload subnet.
  EOT
  type        = string

  validation {
    condition     = can(regex("^projects/[a-z][a-z0-9-]{4,28}[a-z0-9]/global/networks/[a-z]([a-z0-9-]{0,61}[a-z0-9])?$", var.peered_network_id))
    error_message = "peered_network_id must be a network id: projects/<project>/global/networks/<name>."
  }
}

variable "peering_prefix_length" {
  description = <<-EOT
    The size of the range reserved for the pool's workers in the peered
    network. Google requires at least a /24 for a Cloud Build private pool;
    /24 is that floor and is what a single build environment needs.
  EOT
  type        = number
  default     = 24

  validation {
    condition     = var.peering_prefix_length >= 16 && var.peering_prefix_length <= 24
    error_message = "peering_prefix_length must be between 16 and 24: Cloud Build refuses a peered range smaller than a /24."
  }
}

variable "machine_type" {
  description = "The worker shape. A release build of the workspace is CPU-bound; e2-standard-8 is the shape the GitHub runner it replaces approximates."
  type        = string
  default     = "e2-standard-8"

  validation {
    condition     = can(regex("^e2-(standard|highcpu)-(4|8|16|32)$", var.machine_type))
    error_message = "machine_type must be an e2 standard or highcpu shape with 4 to 32 vCPUs, which is what Cloud Build private pools offer."
  }
}

variable "disk_size_gb" {
  description = "Worker disk. Two uncached builds of the workspace and their layers, side by side, for the reproducibility check."
  type        = number
  default     = 200

  validation {
    condition     = var.disk_size_gb >= 100 && var.disk_size_gb <= 4000
    error_message = "disk_size_gb must be between 100 and 4000."
  }
}

variable "no_external_ip" {
  description = <<-EOT
    Whether the workers have no public address. False by default, and the
    reason is stated rather than hidden: the build pulls its base image from
    Docker Hub by digest and its crates from crates.io (`cargo build
    --locked`), and a worker with no address has no route to either — Cloud
    NAT does not translate a peered producer network. True is correct once
    both are served from inside the project (a pull-through Artifact Registry
    remote for each); until then it is a pool that cannot build.
  EOT
  type        = bool
  default     = false
}

variable "ci_service_account" {
  description = "The pipeline's identity (modules/cicd). It submits builds into the pool and runs them as the build identity; it gains nothing else here."
  type        = string
}
