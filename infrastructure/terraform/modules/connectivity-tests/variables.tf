variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type    = map(string)
  default = {}
}

# The paths Network Intelligence Center checks (R12-GCP-10060). Each names
# its source and destination by instance self-link or IP address, the
# protocol and port, and what the platform intends: `reachable` for a path a
# workload needs, `unreachable` for a path the design forbids. The intent is
# carried as a label so a test whose result disagrees with it is visible in
# the console and to anything that reads the results.
variable "tests" {
  description = "Connectivity tests by name: source and destination (instance self-link or IP), protocol, destination port, and expect = reachable | unreachable."
  type = map(object({
    source_instance      = optional(string)
    source_ip            = optional(string)
    destination_instance = optional(string)
    destination_ip       = optional(string)
    protocol             = string
    port                 = number
    expect               = string
  }))
  default  = {}
  nullable = false

  validation {
    condition     = alltrue([for t in values(var.tests) : contains(["reachable", "unreachable"], t.expect)])
    error_message = "Every connectivity test states expect = reachable or unreachable. A test with no intent reports a path and says nothing about whether it should exist."
  }

  validation {
    condition     = alltrue([for t in values(var.tests) : contains(["TCP", "UDP", "ICMP"], t.protocol)])
    error_message = "protocol is TCP, UDP or ICMP."
  }

  validation {
    condition     = alltrue([for t in values(var.tests) : t.port >= 1 && t.port <= 65535])
    error_message = "port is between 1 and 65535."
  }

  validation {
    condition = alltrue([for t in values(var.tests) :
      (t.source_instance != null || t.source_ip != null) && (t.destination_instance != null || t.destination_ip != null)
    ])
    error_message = "Every connectivity test names a source and a destination, by instance or by IP."
  }

  validation {
    # A set of tests that only proves what should work never notices a path
    # that opened which should not: at least one forbidden path is checked
    # whenever any is declared.
    condition     = length(var.tests) == 0 || anytrue([for t in values(var.tests) : t.expect == "unreachable"])
    error_message = "Declare at least one forbidden path (expect = unreachable) beside the intended ones; a suite of only reachable paths cannot notice a path that should not exist."
  }
}
