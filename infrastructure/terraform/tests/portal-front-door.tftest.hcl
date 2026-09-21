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

# --- the door that needs no domain, which is the one every environment has ----
#
# ADR 0095. The runs above are about `module.portal_edge`, which no environment
# turns on any more. These are about what an environment gets instead, and the
# first of them is the claim the whole change rests on: **an environment that
# names no hostname still brings up a working, IAP-protected console.**

run "an_environment_naming_no_hostname_still_gets_an_iap_protected_console" {
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
    # Dev's committed state: no portal hostname, no domain, no GitOps gateway,
    # and a console that exists.
    gitops_portal_hostname = ""
    dns_zone_domain        = ""
    gitops_gateway_enabled = false
    console_egress_cidr    = "10.0.16.0/26"
    # Named rather than left to the root's default, because the region is
    # half of the Google-issued hostname and the assertion below is written
    # as the exact string. The default is a different region, and a URL
    # assertion loose enough to survive that is loose enough to survive a
    # service deployed somewhere nobody meant.
    region = "us-east4"
  }

  # The door exists and it is the Cloud Run one. Asserted on `mode` rather
  # than on the URL alone because the two doors both produce a URL, and a
  # reader — or a later edit — could satisfy a URL assertion with the
  # load-balancer door and never notice.
  assert {
    condition     = output.console_front_door.mode == "cloud-run-iap"
    error_message = "an environment naming no hostname has no Cloud Run IAP door; the console it was promised is not there"
  }

  # The URL is Google's, and the assertion is written as two facts rather than
  # one pattern: it ends in the region's run.app suffix, and it begins with
  # the service this environment deploys. A `strcontains(".run.app")` alone
  # would pass on `https://portal.algorik.ai.run.app`, a name nobody owns.
  assert {
    condition     = endswith(output.console_front_door.url, ".us-east4.run.app") && startswith(output.console_front_door.url, "https://qip-dev-portal-")
    error_message = "the console's URL is not the Google-issued run.app name; an environment that owns no domain has nothing else to be reached at"
  }

  # Nothing that needs a registrar, a quota or a certificate was planned.
  # `portal_front_door` is null exactly when `module.portal_edge`'s count is
  # zero, and that module is the address, the managed certificate, the URL
  # map, the listener **and** the Cloud Armor security policy that
  # `infra.yml` run 71 failed to create with
  # `Quota 'SECURITY_POLICY_RULES' exceeded. Limit: 0.0 globally`. This
  # assertion is what says the rest of the environment plans without it.
  assert {
    condition     = output.portal_front_door == null
    error_message = "a load-balancer door was planned beside the Cloud Run one; Google refuses IAP on both, and this project has no Cloud Armor quota for the security policy that door needs"
  }

  # No zone either, which is the registrar step this change exists to remove.
  assert {
    condition     = output.dns_zone == null
    error_message = "a DNS zone was planned for an environment that names no domain; the nameserver delegation is the manual step this door was chosen to avoid"
  }

  # The access list is empty and the command to change that is published, so
  # the one step this repository deliberately does not take is not also a step
  # somebody has to reconstruct.
  assert {
    condition     = strcontains(output.console_front_door.grant, "--resource-type=cloud-run") && strcontains(output.console_front_door.grant, "roles/iap.httpsResourceAccessor")
    error_message = "the published grant command does not name the Cloud Run IAP resource and the accessor role; without --resource-type=cloud-run the same subcommand edits the project's IAP policy, which is the wide grant ADR 0095 narrowed away from"
  }
}

# The closed state is still closed. An environment with no console at all —
# no `console_egress_cidr`, so no identity, no subnet, no session-secret grant
# — gets neither door, and the output says so rather than naming a URL for a
# service nothing will ever create.
run "an_environment_with_no_console_identity_gets_neither_door" {
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
    gitops_portal_hostname = ""
  }

  assert {
    condition     = output.console_front_door == null
    error_message = "a console door was planned for an environment that creates no console identity; the door would guard a service with nothing to run as"
  }
}

# --- the GitOps gateway's own half-configuration ------------------------------
#
# Argo CD and Kargo have no Google-issued hostname — Google publishes none for
# a GKE Gateway — so the flag and the names have to move together. The refusal
# is a precondition rather than the module's hostname regex, because the regex
# says "that is not a DNS name" about a value nobody typed.

run "a_gitops_gateway_with_no_hostname_stops_the_plan" {
  command = plan

  variables {
    gitops_gateway_enabled = true
    gitops_argocd_hostname = ""
    gitops_kargo_hostname  = ""
  }

  expect_failures = [terraform_data.gitops_gateway_has_hostnames]
}

run "a_gitops_gateway_with_only_one_hostname_stops_the_plan" {
  command = plan

  # The half nobody writes deliberately and everybody writes by accident: one
  # name renamed, the other left behind. Without this case the precondition
  # would pass on an `||` where an `&&` was meant, and Kargo would get an
  # address and a certificate for the empty string.
  variables {
    gitops_gateway_enabled = true
    gitops_argocd_hostname = "argocd.algorik.ai"
    gitops_kargo_hostname  = ""
  }

  expect_failures = [terraform_data.gitops_gateway_has_hostnames]
}

run "the_gateway_flag_left_off_is_not_a_half_configuration" {
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

  # The admitting half, and the state dev is committed in. A precondition
  # proven only to refuse is one that might refuse the configuration every
  # environment actually carries, and this run is what tells the two apart.
  variables {
    gitops_gateway_enabled = false
    gitops_argocd_hostname = ""
    gitops_kargo_hostname  = ""
  }

  assert {
    condition     = length(terraform_data.gitops_gateway_has_hostnames) == 0
    error_message = "the gateway precondition was instantiated for an environment that opens no gateway; a refusal that fires on the closed state is a refusal nobody can satisfy"
  }
}
