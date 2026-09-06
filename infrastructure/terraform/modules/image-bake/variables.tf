variable "project_id" {
  description = "The project the staging bucket, the builder identity and the builder's subnet live in."
  type        = string
}

variable "environment" {
  description = "dev, test, stage or prod. Every name this module creates carries it."
  type        = string
}

variable "region" {
  description = "The region the staging bucket and the builder's subnet live in. The builder machine boots in a zone of this region, which is why the two are not independent."
  type        = string
}

variable "network_id" {
  description = "The VPC the builder's subnet belongs to."
  type        = string
}

variable "subnet_cidr" {
  description = <<-EOT
    The range the throwaway builder machine gets an address from.

    A /28 is ample: one machine at a time, and the group that would need more
    does not exist — a bake is one instance created, imaged and deleted. The
    range must not overlap a trust zone, the console's egress subnet, the
    control plane's endpoint range, or any execution node's block.
  EOT

  type = string

  validation {
    condition     = can(cidrnetmask(var.subnet_cidr))
    error_message = "image_bake_subnet_cidr must be a CIDR block, for example 10.0.37.0/28."
  }

  validation {
    # Split into its own validation because the prefix cannot be read out of a
    # value the syntax check above has not already proven well-formed —
    # `split` on a malformed string produces "Invalid index", which is the
    # error the reader sees instead of the one they caused.
    #
    # A comprehension over `regexall` rather than a guard, because HCL's `||`
    # and its conditional both evaluate the branch they do not take: a
    # `!can(...) || split(...)[1]` still reports "Invalid index" beside the
    # real message, which was measured rather than assumed. `regexall`
    # returns no match for a malformed value, `alltrue([])` is true, and this
    # rule is then simply not applicable — so the reader gets exactly the one
    # error they caused. `console_egress_cidr` emits both; that is a wart
    # worth not copying, and this is where the difference is recorded.
    #
    # A /29 is the smallest subnet Compute Engine accepts and it holds three
    # usable addresses; a /24 is far more than a single throwaway machine
    # needs and is where a range starts colliding with a zone's.
    condition = alltrue([
      for prefix in regexall("/([0-9]+)$", var.subnet_cidr) :
      tonumber(prefix[0]) >= 24 && tonumber(prefix[0]) <= 29
    ])
    error_message = "image_bake_subnet_cidr must be between a /24 and a /29. Smaller than a /29 is refused by Compute Engine; wider than a /24 is a range one throwaway machine has no use for and that will collide with a trust zone's."
  }
}

variable "payload_retention_days" {
  description = <<-EOT
    How many days a staged payload survives before the bucket deletes it.

    The bucket expires payloads because the identity that writes them cannot:
    `qip-infra-<env>`'s custom storage role in `modules/cicd` carries
    `storage.objects.create` and deliberately not `storage.objects.delete`, so
    that nothing it runs can delete from the evidence bucket. Seven days is
    long enough to re-run a failed bake against the same bytes and short
    enough that a bucket of hundred-megabyte tarballs is not a standing cost.

    The durable record of what went into an image is the image's own labels
    and the run that produced it, not this object.
  EOT

  type    = number
  default = 7

  validation {
    condition     = var.payload_retention_days >= 1
    error_message = "A retention of zero days deletes the payload the bake is still reading. Set at least one."
  }
}

variable "labels" {
  description = "Labels applied to the staging bucket."
  type        = map(string)
  default     = {}
}
