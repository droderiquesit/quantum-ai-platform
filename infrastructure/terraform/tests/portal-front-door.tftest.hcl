# The console's IAP front door, planned. ADR 0094.
#
# `gitops_portal_hostname` is the switch for `module.portal_edge`: empty and
# nothing exists — no address, no certificate, no Cloud Armor policy, no
# backend, no listener — set and the whole door comes up. The value reaches a
# managed certificate's domain list, so the validation on it is the difference
# between a name Google issues for and an apply that fails after the address
# has been reserved.
#
# Planned rather than asserted as text, for the reason `paper-boundary.tftest.hcl`
# gives and `region-dark-window.tftest.hcl` repeats: a validation that read
# correctly once killed every plan by handing a null to a function, and only a
# plan could have shown it. `terraform validate` evaluates no `validation`
# block and no `lifecycle.precondition`, which is where every safety refusal
# in this tree lives.
#
# Both halves, because the second is the one that matters here. A hostname
# check proven only to refuse would pass on an expression that refuses every
# name, and the door would then be a module that can never be switched on —
# which reads, from a one-sided harness, exactly like a strict gate.
#
# Mocked providers, plan only, three modules overridden with fixed outputs —
# the same shape as the sibling harnesses and for the same reasons: nothing
# here can reach a project, and the three overrides replace `for_each` inputs
# a mock cannot compute. None of the three is on this variable's path.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id        = "portal-door-plan-harness"
  project_number    = 123456789012
  environment       = "dev"
  github_repository = "example/example"

  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

# --- the closed state --------------------------------------------------------

run "an_environment_naming_no_hostname_gets_no_front_door_at_all" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/portal-door-plan-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/portal-door-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/portal-door-plan-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/portal-door-plan-harness/harness"
    }
  }

  # The default, and the state of three of the four environments. Asserted on
  # the output rather than on a resource count because the output is null
  # exactly when the module's `count` is zero, and a reader checking "is there
  # a door" reads the output.
  assert {
    condition     = output.portal_front_door == null
    error_message = "a front door was planned for an environment that named no hostname; an address on the internet created because a variable had a default is one nobody decided to open"
  }
}

# --- the admitting half ------------------------------------------------------

run "a_reviewed_hostname_brings_up_the_door_and_publishes_the_address_to_point_at" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/portal-door-plan-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/portal-door-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/portal-door-plan-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/portal-door-plan-harness/harness"
    }
  }

  variables {
    gitops_portal_hostname = "portal.algorik.ai"
    # The portal runs as the console identity, so the door needs the console
    # to exist. The precondition below is what says so when it does not.
    console_egress_cidr = "10.0.16.0/26"
  }

  assert {
    condition     = output.portal_front_door.hostname == "portal.algorik.ai"
    error_message = "the reviewed hostname did not reach the front door; the value in the tfvars would be a name nothing answers on"
  }

  # https, and the output says so, because there is no listener on 80 and an
  # operator handed an http URL would spend an afternoon on a connection
  # refused.
  assert {
    condition     = output.portal_front_door.url == "https://portal.algorik.ai"
    error_message = "the front door's URL is not https on the reviewed hostname"
  }

  # The address is what the A record points at, and it is the one step this
  # repository cannot perform, so the output has to carry it: an operator who
  # cannot read it from `terraform output` reads it out of the console, and
  # then the record and the reservation are two facts nobody reconciles.
  #
  # The *value* cannot be asserted here and that is a property of the harness
  # rather than of the configuration: a mocked provider leaves every computed
  # attribute unknown at plan, so `google_compute_global_address.address` is
  # null in every run of this file. `modules/public-edge`'s harness records
  # the same limit about its security-policy attachment. What is assertable,
  # and what actually fails when somebody trims the output, is that the key
  # is published at all.
  assert {
    condition     = contains(keys(output.portal_front_door), "address")
    error_message = "the front door output publishes no address; the registrar record would have nothing to point at and an operator would read it out of the console instead"
  }
}

# --- the refusing half -------------------------------------------------------
#
# One run per shape of bad value. A scheme and a path are what somebody pastes
# out of a browser; an upper-case name is what a manager types; a wildcard is
# what somebody reaches for to cover a second console, and Google's managed
# certificates do not issue for one. Each would reach a certificate's domain
# list and fail at apply, after the address had been reserved and the
# certificate ordered.

run "a_hostname_carrying_a_scheme_stops_the_plan" {
  command = plan

  variables {
    gitops_portal_hostname = "https://portal.algorik.ai"
  }

  expect_failures = [var.gitops_portal_hostname]
}

run "a_hostname_carrying_a_path_stops_the_plan" {
  command = plan

  variables {
    gitops_portal_hostname = "portal.algorik.ai/console"
  }

  expect_failures = [var.gitops_portal_hostname]
}

run "an_upper_case_hostname_stops_the_plan" {
  command = plan

  variables {
    gitops_portal_hostname = "Portal.algorik.ai"
  }

  expect_failures = [var.gitops_portal_hostname]
}

run "a_wildcard_hostname_stops_the_plan" {
  command = plan

  variables {
    gitops_portal_hostname = "*.algorik.ai"
  }

  expect_failures = [var.gitops_portal_hostname]
}

# --- the door with nothing behind it -----------------------------------------

run "a_front_door_without_a_console_identity_stops_the_plan" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/portal-door-plan-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/portal-door-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/portal-door-plan-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/portal-door-plan-harness/harness"
    }
  }

  variables {
    gitops_portal_hostname = "portal.algorik.ai"
    # And no console_egress_cidr, so `modules/secrets` creates no console
    # identity, no session-secret grant and no subnet. The door would be an
    # address, a certificate and a backend in front of a service with nothing
    # to run as — and the first sign of it would be a `RunService` Config
    # Connector could not reconcile, three steps downstream of the cause.
  }

  expect_failures = [terraform_data.portal_edge_has_a_console]
}
