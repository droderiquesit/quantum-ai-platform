variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "enabled" {
  description = <<-EOT
    Whether to create the Network Connectivity Center hub and its spokes.

    Default false. The hub itself is free, but every VPC spoke is billed for
    data it processes between spokes, and a spoke makes the five planes
    reachable from one another through the hub. The trust-zone map is what
    keeps those planes apart, so joining them is a decision a person takes
    with a plan in front of them, not a default.
  EOT
  type        = bool
  default     = false
}

variable "vpc_spokes" {
  description = "Spoke name to the VPC network resource ID it links (projects/<p>/global/networks/<n>)."
  type        = map(string)
  default     = {}

  validation {
    condition     = alltrue([for id in values(var.vpc_spokes) : can(regex("^projects/[^/]+/global/networks/[^/]+$", id))])
    error_message = "a vpc spoke names a network by its resource ID, projects/<project>/global/networks/<name>; a bare name or a URL links nothing."
  }
}

variable "interconnect_spokes" {
  description = "Spoke name to the interconnect attachment resource URIs it links, with the region they sit in. Empty until a partner circuit exists (modules/connectivity/NOT-ORDERED.md)."
  type = map(object({
    location = string
    uris     = list(string)
  }))
  default = {}

  validation {
    condition     = alltrue([for s in values(var.interconnect_spokes) : length(s.uris) > 0])
    error_message = "an interconnect spoke with no attachment links nothing and reads as a hybrid path."
  }
}
