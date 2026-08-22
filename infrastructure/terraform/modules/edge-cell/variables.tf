variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "cell_id" {
  description = <<-EOT
    The cell's identifier, matching the `cell` field of a `CapitalEnvelope`.

    An envelope is granted to a strategy *at a cell*, and the cell refuses one
    addressed elsewhere. A mismatch here is a cell that starts and then rejects
    every grant it is sent, so the two names are the same name.
  EOT

  type = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,28}[a-z0-9]$", var.cell_id))
    error_message = "A cell id is lower case, starts with a letter and contains only letters, digits and hyphens."
  }
}

variable "region" {
  description = "The region the cell runs in. Chosen for its distance to the venues, not for convenience."
  type        = string
}

variable "network_id" {
  type = string
}

variable "subnet_cidr" {
  description = "The cell's own primary range. Must not overlap another cell's."
  type        = string
}

variable "pod_cidr" {
  description = "The cell's secondary range for pods."
  type        = string
}

variable "service_cidr" {
  description = "The cell's secondary range for services."
  type        = string
}

variable "venues" {
  description = <<-EOT
    The venues this cell may reach, keyed by venue identifier.

    An empty map is no venues, not all of them. That reading is deliberate and
    matches `CapitalEnvelope`, whose venue list has the same rule for the same
    reason: the permissive reading of an empty list is how grants leak.

    The address ranges are not guessed here. They come from the venue's own
    connectivity documentation or from the extranet provider, and a cell
    deployed with this map empty can reach no venue at all — which is the
    correct state for a cell whose connectivity has not been confirmed.
  EOT

  type = map(object({
    cidr = string
    port = number
  }))

  default = {}

  validation {
    condition = alltrue([
      for venue in var.venues : venue.cidr != "0.0.0.0/0" && venue.cidr != "::/0"
    ])
    error_message = "A venue range of the whole internet is not a venue range. Name the ranges the venue publishes."
  }

  validation {
    condition = alltrue([
      for venue in var.venues : venue.port > 0 && venue.port <= 65535
    ])
    error_message = "A venue port must be a port."
  }
}

variable "central_plane_ranges" {
  description = <<-EOT
    Where the central plane is, and the private Google API endpoint.

    Empty by default: a cell that cannot reach the centre still trades inside
    the envelope it holds, so a missing value here degrades the deployment
    rather than opening it.
  EOT

  type    = list(string)
  default = []

  validation {
    condition = alltrue([
      for range in var.central_plane_ranges : range != "0.0.0.0/0"
    ])
    error_message = "The central plane is not the whole internet."
  }
}

variable "capital_envelope_secret_id" {
  description = "The Secret Manager secret holding the key the cell verifies capital envelopes against."
  type        = string
}

variable "evidence_bucket" {
  description = "The write-once evidence bucket. The cell is granted object creation on it and nothing else."
  type        = string
}

variable "registry_location" {
  type = string
}

variable "registry_repository" {
  type = string
}
