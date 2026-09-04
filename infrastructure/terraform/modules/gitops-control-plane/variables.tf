variable "project_id" {
  type = string
}

variable "project_number" {
  description = "The project's numeric id. The GKE service agent is named by it, and the etcd key grant goes to that agent; a wrong number grants the key to an agent in somebody else's project, cleanly."
  type        = number
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

variable "network_id" {
  description = "The VPC the cluster's nodes and the two registry zones attach to."
  type        = string
}

variable "management_subnet_id" {
  description = <<-EOT
    The management trust zone's subnet, from `modules/trust-zones`. The
    cluster's nodes live in it and GKE draws the Pod and Service ranges from
    it as managed secondary ranges, so the zone's default deny and its one
    egress allowlist are the rules that bind every controller.
  EOT
  type        = string
}

variable "management_subnet_cidr" {
  description = "The management zone's range: the only network that may reach the private endpoint. There is no public endpoint to allow anything else."
  type        = string

  validation {
    condition     = can(cidrhost(var.management_subnet_cidr, 0)) && var.management_subnet_cidr != "0.0.0.0/0"
    error_message = "The authorised network is the management zone's own IPv4 range, and the whole internet is not one."
  }
}

variable "management_network_tag" {
  description = "The management zone's network tag, put on every node so the zone's firewall rules see the cluster's traffic. A node without it is a node outside every zone."
  type        = string
}

variable "master_ipv4_cidr_block" {
  description = <<-EOT
    The /28 the private endpoint is allocated from, or null.

    No default, on purpose: a range chosen as a convenience is the one that
    collides, and this one must overlap neither a trust zone, the console's
    subnet, nor an execution node's block (environments/README.md has the
    ladder). Null is admitted by the type so an environment with
    `gitops_enabled = false` need not invent one; the cluster's precondition
    refuses null the moment the cluster is asked for.
  EOT
  type        = string
  default     = null

  validation {
    condition     = var.master_ipv4_cidr_block == null || can(regex("/28$", coalesce(var.master_ipv4_cidr_block, "0.0.0.0/0")))
    error_message = "GKE allocates the private endpoint from exactly a /28. Any other size is refused at apply, after the cluster's network peering has already been created."
  }
}

variable "key_ring_id" {
  description = "The platform's key ring, from `modules/secrets`. The etcd key is created in it rather than in a second ring nobody rotates."
  type        = string
}

variable "infra_service_account" {
  description = "The account `infra.yml` applies as. It receives the bootstrap's custom role and the Connect gateway roles, and nothing on the cluster beyond those."
  type        = string
}

variable "registry_repository_name" {
  description = "The environment's Artifact Registry repository, from `modules/registry`. Kargo's reader grant is scoped to it."
  type        = string
}

variable "argocd_app_secret_id" {
  description = "The Secret Manager secret holding the read-only GitHub App's installation (ADR 0036 decision 3). Created empty by `modules/secrets`; seeded out of band; readable by the Argo CD identity alone."
  type        = string
}

variable "kargo_app_secret_id" {
  description = "The Secret Manager secret holding the write-scoped GitHub App's installation. Created empty by `modules/secrets`; seeded out of band; readable by the Kargo identity alone."
  type        = string
}
